//! World storage façade (P03-06/07/09 glue, P03-12/13 driver).
//!
//! [`WorldStorage`] owns the world directory: `level.dat`, the per-dimension
//! region trees and the dirty queue. It is the single entry point Phase 04's
//! world runtime uses, and the only place that decides *when* bytes reach disk.
//!
//! ## Save lifecycle
//!
//! ```text
//! queue_chunk_save(dim, chunk)   # cheap: marks dirty + keeps the encoded chunk
//! ...                            # tick loop continues
//! on_tick(tick) -> Option<Report># autosave when the scheduler says so
//! flush() -> SaveReport          # snapshot, write, sync, write level.dat last
//! close() -> SaveReport          # flush + sync + drop handles
//! ```
//!
//! `flush` takes a [`dirty::DirtyTracker::snapshot`] of the queue, writes the
//! chunks grouped by region file, syncs each touched region, then writes
//! `level.dat` atomically **last** — so a world on disk always describes chunks
//! that are already durable. Anything that fails stays queued and is named in
//! the report.
//!
//! ## Handles
//!
//! Region handles are cached with a bounded LRU ([`WorldStorage::max_open_regions`]).
//! Evicting a handle never writes: pending chunk data lives in the queue, not in
//! the handle, so eviction is always safe.

use crate::autosave::AutosaveScheduler;
use crate::chunk::{ChunkData, ChunkKey, ChunkPos};
use crate::compression::{Compression, DEFAULT_WRITE};
use crate::dimension::{Dimension, DimensionLayout};
use crate::dirty::{DirtySnapshot, DirtyTracker, RegionCoord};
use crate::level::{DATA_VERSION_26_1_2, LevelDat};
use crate::region::RegionFile;
use crate::save::{
    SaveReport, backup_path, decode_gzip_nbt, encode_gzip_nbt, unix_millis, unix_seconds,
    write_atomic,
};
use mc_core::error::{ServerError, ServerResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// File name of the level metadata document.
pub const LEVEL_FILE: &str = "level.dat";

/// Default cap on simultaneously open region files.
///
/// One handle per region file costs a file descriptor; 64 keeps a Pi 5 well
/// inside its default `ulimit -n` while covering a 10-player view distance.
pub const DEFAULT_MAX_OPEN_REGIONS: usize = 64;

/// An open world directory.
#[derive(Debug)]
pub struct WorldStorage {
    root: PathBuf,
    level: Option<LevelDat>,
    regions: BTreeMap<(Dimension, RegionCoord), RegionFile>,
    region_use: BTreeMap<(Dimension, RegionCoord), u64>,
    use_counter: u64,
    max_open_regions: usize,
    compression: Compression,
    pending: BTreeMap<ChunkKey, Vec<u8>>,
    dirty: DirtyTracker,
    autosave: AutosaveScheduler,
}

impl WorldStorage {
    /// Open an existing world directory, creating it when absent.
    ///
    /// # Errors
    ///
    /// - [`ServerError::Operational`] when the directory cannot be created or a
    ///   region handle cannot be opened;
    /// - [`ServerError::CorruptData`] when `level.dat` exists but is unreadable,
    ///   or when its `DataVersion` is outside the readable window.
    pub fn open(root: &Path, autosave_ticks: u64) -> ServerResult<Self> {
        std::fs::create_dir_all(root).map_err(|e| {
            ServerError::Operational(format!("cannot create world dir {}: {e}", root.display()))
        })?;
        let mut storage = Self {
            root: root.to_path_buf(),
            level: None,
            regions: BTreeMap::new(),
            region_use: BTreeMap::new(),
            use_counter: 0,
            max_open_regions: DEFAULT_MAX_OPEN_REGIONS,
            compression: DEFAULT_WRITE,
            pending: BTreeMap::new(),
            dirty: DirtyTracker::new(),
            autosave: AutosaveScheduler::new(autosave_ticks),
        };
        if storage.level_path().exists() {
            let level = storage.load_level()?;
            if !LevelDat::is_supported_data_version(level.data_version) {
                return Err(ServerError::CorruptData(format!(
                    "world {} has DataVersion {}, outside the supported window \
                     {}..={} (no datafixers in this build)",
                    root.display(),
                    level.data_version,
                    crate::level::MIN_READABLE_DATA_VERSION,
                    DATA_VERSION_26_1_2
                )));
            }
            info!(
                world = %root.display(),
                data_version = level.data_version,
                "opened existing world"
            );
            storage.level = Some(level);
        } else {
            info!(world = %root.display(), "world has no level.dat yet; creating on first save");
        }
        Ok(storage)
    }

    /// Create a new world (or open it if it already exists), writing an initial
    /// `level.dat` when the directory is fresh.
    ///
    /// # Errors
    ///
    /// As for [`WorldStorage::open`], plus [`ServerError::Operational`] when the
    /// initial `level.dat` cannot be written.
    pub fn create_or_open(
        root: &Path,
        level_name: &str,
        autosave_ticks: u64,
    ) -> ServerResult<Self> {
        let fresh = !root.join(LEVEL_FILE).exists();
        let mut storage = Self::open(root, autosave_ticks)?;
        if fresh {
            let mut level = LevelDat::new(level_name, unix_millis(std::time::SystemTime::now()));
            level.initialized = false;
            storage.save_level(level)?;
        }
        Ok(storage)
    }

    /// World directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The cached level metadata, when the world has been initialised.
    ///
    /// The server owns gameplay changes; it mutates this through
    /// [`WorldStorage::level_mut`] and the next [`WorldStorage::flush`] persists
    /// it.
    #[must_use]
    pub const fn level(&self) -> Option<&LevelDat> {
        self.level.as_ref()
    }

    /// Mutable level metadata. Callers that change it must call
    /// [`WorldStorage::mark_level_dirty`].
    pub fn level_mut(&mut self) -> Option<&mut LevelDat> {
        self.level.as_mut()
    }

    /// Level name as recorded in `level.dat`.
    #[must_use]
    pub fn level_name(&self) -> &str {
        self.level
            .as_ref()
            .map_or("world", |level| level.level_name.as_str())
    }

    /// Flag the cached level metadata for the next flush.
    pub fn mark_level_dirty(&mut self) {
        self.dirty.mark_level();
    }

    /// Path of `level.dat`.
    #[must_use]
    pub fn level_path(&self) -> PathBuf {
        self.root.join(LEVEL_FILE)
    }

    /// Path of `level.dat_old`.
    #[must_use]
    pub fn level_backup_path(&self) -> PathBuf {
        backup_path(&self.level_path())
    }

    /// Compression used for chunks this world writes.
    #[must_use]
    pub const fn compression(&self) -> Compression {
        self.compression
    }

    /// Override the write codec (tests and future config).
    pub fn set_compression(&mut self, compression: Compression) {
        self.compression = compression;
    }

    /// Number of region handles currently open.
    #[must_use]
    pub fn open_region_count(&self) -> usize {
        self.regions.len()
    }

    /// Cap on simultaneously open region handles.
    #[must_use]
    pub const fn max_open_regions(&self) -> usize {
        self.max_open_regions
    }

    /// Set the region-handle cap (0 and 1 are clamped to 1).
    pub fn set_max_open_regions(&mut self, limit: usize) {
        self.max_open_regions = limit.max(1);
        self.evict_regions();
    }

    /// The autosave scheduler.
    #[must_use]
    pub const fn autosave(&self) -> &AutosaveScheduler {
        &self.autosave
    }

    /// Mutable access to the autosave scheduler (config reload).
    pub fn autosave_mut(&mut self) -> &mut AutosaveScheduler {
        &mut self.autosave
    }

    /// The dirty tracker.
    #[must_use]
    pub const fn dirty(&self) -> &DirtyTracker {
        &self.dirty
    }

    /// Chunks waiting to be written.
    #[must_use]
    pub fn pending_chunk_count(&self) -> usize {
        self.pending.len()
    }

    /// The layout (modern or legacy) that reads/writes use for a dimension.
    ///
    /// The modern `<world>/dimensions/<ns>/<value>` tree wins when it exists;
    /// otherwise a pre-26.1 layout is used and a deprecation warning is logged
    /// (ADR-0001 D-04).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a legacy-only dimension has no legacy
    /// folder (custom dimensions).
    pub fn layout_for(&mut self, dimension: &Dimension) -> ServerResult<DimensionLayout> {
        let modern = DimensionLayout::modern(&self.root, dimension);
        if modern.region_dir().is_dir() {
            return Ok(modern);
        }
        if let Ok(legacy) = DimensionLayout::legacy(&self.root, dimension)
            && legacy.region_dir().is_dir()
        {
            warn!(
                dimension = %dimension,
                path = %legacy.region_dir().display(),
                "reading a pre-26.1 world layout; the next save writes the 26.1 layout"
            );
            return Ok(legacy);
        }
        Ok(modern)
    }

    /// Region file path for a dimension and region coordinate.
    ///
    /// # Errors
    ///
    /// As for [`WorldStorage::layout_for`].
    pub fn region_path(
        &mut self,
        dimension: &Dimension,
        region: RegionCoord,
    ) -> ServerResult<PathBuf> {
        Ok(self.layout_for(dimension)?.region_path(region.0, region.1))
    }

    /// Read `level.dat`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file is missing or unreadable,
    /// [`ServerError::CorruptData`] when its contents are not a valid level.
    pub fn load_level(&self) -> ServerResult<LevelDat> {
        let path = self.level_path();
        let bytes = std::fs::read(&path).map_err(|e| {
            ServerError::Operational(format!("cannot read {}: {e}", path.display()))
        })?;
        LevelDat::from_nbt(&decode_gzip_nbt(&bytes)?)
    }

    /// Write `level.dat` atomically now, rotating the previous file to
    /// `level.dat_old`, and cache the level in memory.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when encoding or writing fails.
    pub fn save_level(&mut self, level: LevelDat) -> ServerResult<()> {
        self.write_level(&level)?;
        self.level = Some(level);
        Ok(())
    }

    fn write_level(&self, level: &LevelDat) -> ServerResult<()> {
        let bytes = encode_gzip_nbt("", &level.to_nbt())?;
        write_atomic(&self.level_path(), &bytes, true)?;
        debug!(path = %self.level_path().display(), "wrote level.dat");
        Ok(())
    }

    /// Queue a chunk for the next flush, encoding it now.
    ///
    /// Encoding happens at queue time so an encoding failure is reported to the
    /// caller immediately instead of surfacing in a background flush.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the chunk cannot be encoded.
    pub fn queue_chunk_save(
        &mut self,
        dimension: &Dimension,
        chunk: &ChunkData,
    ) -> ServerResult<()> {
        let bytes = chunk.to_nbt_bytes()?;
        let key = ChunkKey::new(dimension, chunk.pos);
        self.pending.insert(key, bytes);
        self.dirty.mark_chunk(dimension, chunk.pos);
        Ok(())
    }

    /// Write a chunk immediately (bypassing the queue).
    ///
    /// Used by tests and by `close`; the normal path is
    /// [`WorldStorage::queue_chunk_save`].
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] on encoding/IO failure,
    /// [`ServerError::CorruptData`] when the chunk's stored position disagrees
    /// with `pos`.
    pub fn write_chunk_now(
        &mut self,
        dimension: &Dimension,
        pos: ChunkPos,
        chunk: &ChunkData,
    ) -> ServerResult<()> {
        chunk.verify_position(pos)?;
        let bytes = chunk.to_nbt_bytes()?;
        let now = unix_seconds(std::time::SystemTime::now());
        let compression = self.compression;
        let region = (pos.region_x(), pos.region_z());
        self.with_region(dimension, region, |handle| {
            handle.write_chunk(pos, &bytes, compression, now)?;
            handle.sync()
        })?;
        self.pending.remove(&ChunkKey::new(dimension, pos));
        Ok(())
    }

    /// Whether a chunk exists on disk.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the region file cannot be opened.
    pub fn has_chunk(&mut self, dimension: &Dimension, pos: ChunkPos) -> ServerResult<bool> {
        if self.pending.contains_key(&ChunkKey::new(dimension, pos)) {
            return Ok(true);
        }
        let region = (pos.region_x(), pos.region_z());
        self.with_region(dimension, region, |handle| Ok(handle.has_chunk(pos)))
    }

    /// Read a chunk from disk.
    ///
    /// # Errors
    ///
    /// - [`ServerError::CorruptData`] for a structurally broken region file, a
    ///   chunk whose declared position disagrees with its slot, or malformed
    ///   chunk NBT;
    /// - [`ServerError::Operational`] for I/O failures and unsupported codecs;
    /// - [`ServerError::Protocol`] for truncated NBT.
    pub fn read_chunk(
        &mut self,
        dimension: &Dimension,
        pos: ChunkPos,
    ) -> ServerResult<Option<ChunkData>> {
        if let Some(bytes) = self.pending.get(&ChunkKey::new(dimension, pos)) {
            return ChunkData::from_nbt_bytes(bytes).map(Some);
        }
        let region = (pos.region_x(), pos.region_z());
        let stored = self.with_region(dimension, region, |handle| handle.read_chunk(pos))?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        let chunk = ChunkData::from_nbt_bytes(&stored.data).map_err(|e| match e {
            ServerError::CorruptData(message) => ServerError::CorruptData(format!(
                "chunk {pos:?} of {dimension} (region {region:?}, {} bytes): {message}",
                stored.data.len()
            )),
            other => other,
        })?;
        chunk.verify_position(pos)?;
        if !LevelDat::is_supported_data_version(chunk.data_version) {
            return Err(ServerError::CorruptData(format!(
                "chunk {pos:?} of {dimension} has DataVersion {}, outside the supported window",
                chunk.data_version
            )));
        }
        Ok(Some(chunk))
    }

    /// Remove a chunk from disk.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] for I/O failures.
    pub fn remove_chunk(&mut self, dimension: &Dimension, pos: ChunkPos) -> ServerResult<bool> {
        self.pending.remove(&ChunkKey::new(dimension, pos));
        let region = (pos.region_x(), pos.region_z());
        self.with_region(dimension, region, |handle| handle.remove_chunk(pos))
    }

    /// Advance the autosave clock; `Some(report)` when a save just happened.
    ///
    /// Returns `None` when no save was due.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when `level.dat` could not be written; chunk
    /// failures are reported inside the [`SaveReport`].
    pub fn on_tick(&mut self, tick: mc_core::tick::Tick) -> ServerResult<Option<SaveReport>> {
        if !self.autosave.on_tick(tick) {
            return Ok(None);
        }
        info!(tick, "autosave triggered");
        self.flush().map(Some)
    }

    /// Write everything pending and sync the touched region files.
    ///
    /// Ordering: chunks first, `level.dat` last, so a `level.dat` on disk never
    /// describes chunks that are not yet durable.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] only when `level.dat` cannot be written;
    /// per-chunk failures are reported in [`SaveReport::errors`] and the
    /// affected chunks stay queued for the next flush.
    pub fn flush(&mut self) -> ServerResult<SaveReport> {
        let snapshot = self.dirty.snapshot();
        let mut report = self.write_chunks(&snapshot);
        if snapshot.level() {
            let cached = self.level.clone();
            match cached {
                Some(level) => match self.write_level(&level) {
                    Ok(()) => report.level_written = true,
                    Err(e) => {
                        // Re-mark the level so the next flush retries it.
                        self.dirty.mark_level();
                        report.errors.push(format!("level.dat: {e}"));
                        return Err(e);
                    }
                },
                None => {
                    warn!("level.dat flagged dirty but no level is cached");
                }
            }
        }
        Ok(report)
    }

    /// Flush, then drop every open region handle.
    ///
    /// # Errors
    ///
    /// As for [`WorldStorage::flush`].
    pub fn close(mut self) -> ServerResult<SaveReport> {
        let report = self.flush()?;
        for (_, mut handle) in std::mem::take(&mut self.regions) {
            if let Err(e) = handle.sync() {
                warn!(path = %handle.path().display(), error = %e, "region sync failed during close");
            }
        }
        self.region_use.clear();
        Ok(report)
    }

    fn write_chunks(&mut self, snapshot: &DirtySnapshot) -> SaveReport {
        let mut report = SaveReport::default();
        let now = unix_seconds(std::time::SystemTime::now());
        let compression = self.compression;

        for ((dimension, region), positions) in snapshot.by_region() {
            // Collect this region's payloads first so the region handle can be
            // borrowed without also borrowing the queue.
            let batch: Vec<(ChunkPos, Vec<u8>)> = positions
                .iter()
                .filter_map(|pos| {
                    self.pending
                        .get(&ChunkKey::new(&dimension, *pos))
                        .map(|bytes| (*pos, bytes.clone()))
                })
                .collect();
            report.chunks_skipped += positions.len() - batch.len();

            let outcome = self.with_region(&dimension, region, |handle| {
                let mut written = 0usize;
                let mut failures: Vec<(ChunkPos, ServerError)> = Vec::new();
                let mut sync_error = None;
                for (pos, bytes) in &batch {
                    match handle.write_chunk(*pos, bytes, compression, now) {
                        Ok(()) => written += 1,
                        Err(e) => failures.push((*pos, e)),
                    }
                }
                match handle.sync() {
                    Ok(()) => {}
                    Err(e) => sync_error = Some(e),
                }
                Ok((written, failures, sync_error))
            });

            match outcome {
                Ok((written, failures, sync_error)) => {
                    report.chunks_written += written;
                    report.chunks_failed += failures.len();
                    let failed: Vec<ChunkPos> = failures.iter().map(|(pos, _)| *pos).collect();
                    for (pos, e) in failures {
                        report
                            .errors
                            .push(format!("cannot write chunk {pos:?} of {dimension}: {e}"));
                    }
                    if let Some(e) = sync_error {
                        report
                            .errors
                            .push(format!("cannot sync region {region:?} of {dimension}: {e}"));
                    } else {
                        report.regions_synced += 1;
                    }
                    for (pos, _) in &batch {
                        if failed.contains(pos) {
                            // Keep the payload queued *and* re-mark it dirty: the
                            // snapshot cleared the tracker, so without this the
                            // next flush would never look at this chunk again.
                            self.dirty.mark_chunk(&dimension, *pos);
                        } else {
                            self.pending.remove(&ChunkKey::new(&dimension, *pos));
                        }
                    }
                }
                Err(e) => {
                    // A region that cannot be opened fails all of its chunks, but
                    // the rest of the flush continues. Everything stays queued and
                    // dirty for the next attempt.
                    report.chunks_failed += batch.len();
                    for (pos, _) in &batch {
                        self.dirty.mark_chunk(&dimension, *pos);
                    }
                    report
                        .errors
                        .push(format!("cannot open region {region:?} of {dimension}: {e}"));
                }
            }
        }

        if report.chunks_failed > 0 {
            warn!(
                written = report.chunks_written,
                failed = report.chunks_failed,
                "some chunks could not be written; they stay queued"
            );
        } else if report.chunks_written > 0 {
            info!(
                written = report.chunks_written,
                regions = report.regions_synced,
                "chunk save complete"
            );
        }
        report
    }

    /// Run `f` against a region handle, keeping the handle cache consistent.
    ///
    /// The handle is temporarily taken out of the cache so that `f` cannot
    /// alias other `WorldStorage` state (the borrow checker enforces the
    /// single-owner contract).
    fn with_region<T>(
        &mut self,
        dimension: &Dimension,
        region: RegionCoord,
        f: impl FnOnce(&mut RegionFile) -> ServerResult<T>,
    ) -> ServerResult<T> {
        let key = (dimension.clone(), region);
        if !self.regions.contains_key(&key) {
            let path = self.region_path(dimension, region)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ServerError::Operational(format!(
                        "cannot create region dir {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            let handle = RegionFile::open(&path)?;
            self.regions.insert(key.clone(), handle);
            self.region_use.insert(key.clone(), self.use_counter);
            self.evict_regions();
        }
        self.use_counter = self.use_counter.saturating_add(1);
        let used = self.use_counter;
        self.region_use.insert(key.clone(), used);
        let mut handle = self.regions.remove(&key).ok_or_else(|| {
            ServerError::Invariant("region handle vanished from the cache".to_owned())
        })?;
        let result = f(&mut handle);
        // Put it back even when `f` failed: the handle is still usable and the
        // cache is the only owner.
        self.regions.insert(key, handle);
        self.evict_regions();
        result
    }

    /// Drop least-recently-used handles until the cap is respected.
    fn evict_regions(&mut self) {
        while self.regions.len() > self.max_open_regions {
            let Some(victim) = self
                .region_use
                .iter()
                .min_by_key(|(_, used)| **used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.region_use.remove(&victim);
            if let Some(mut handle) = self.regions.remove(&victim)
                && let Err(e) = handle.sync()
            {
                warn!(path = %handle.path().display(), error = %e, "sync on eviction failed");
            }
        }
    }
}
