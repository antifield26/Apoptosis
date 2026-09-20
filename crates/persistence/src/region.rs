//! Anvil region files (P03-06 read path, P03-07 write path).
//!
//! Layout (verified against the 26.1.2 `RegionFile` implementation and against
//! a region file written by vanilla):
//!
//! ```text
//! sector 0..1    header
//!   [0x0000, 0x1000)  1024 × u32 location:  (sector << 8) | sector_count
//!   [0x1000, 0x2000)  1024 × i32 timestamp (seconds since epoch)
//! sector 2..     chunk payloads, each padded to a whole number of 4096-byte
//!                sectors:
//!   u32 length            = 1 + payload bytes (so it counts the codec byte)
//!   u8  compression id    (see `compression`; 128 flag = external `.mcc`)
//!   [payload]
//! ```
//!
//! Slot index is `(x & 31) + (z & 31) * 32`, so region `r.-2.-1.mca` holds
//! chunks -64..-33 in x and -32..-1 in z, and chunk (-37, -24) sits in slot
//! `27 * 32 + 8 = 872`.
//!
//! ## Failure behaviour
//!
//! A region file is untrusted input. Every field is validated before use: a
//! slot must point at sector ≥ 2 (never into the header), the declared sector
//! count must be non-zero and inside the file, the payload length must fit both
//! the allocated sectors and the remaining file, and the compression id must be
//! one this build can decode. Violations become
//! [`ServerError::CorruptData`] — never a panic and never a partially published
//! chunk (AGENTS.md sections 9-10).
//!
//! ## Write ordering
//!
//! `write_chunk` appends the payload first, then the timestamp, and writes the
//! **location entry last**: until that final write lands, readers still see the
//! previous chunk, so an interrupted save can lose the new data but never
//! corrupt the old. [`RegionFile::sync`] then flushes the file to stable
//! storage; the save barrier calls it once per flush rather than per chunk.

use crate::chunk::ChunkPos;
use crate::compression::{Compression, DEFAULT_PAYLOAD_LIMIT};
use mc_core::error::{ServerError, ServerResult};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Bytes per sector. Vanilla: `RegionFile.SECTOR_BYTES = 4096`.
pub const SECTOR_BYTES: usize = 4096;

/// Header size: 1024 locations + 1024 timestamps. Vanilla: `HEADER_OFFSET` +
/// `SECTOR_INTS`.
pub const HEADER_BYTES: usize = SECTOR_BYTES * 2;

/// Chunk slots per region file (32×32).
pub const CHUNK_SLOTS: usize = 1024;

/// Highest sector count a location entry can express (low byte of the u32).
pub const MAX_SECTORS_PER_CHUNK: u32 = 0xFF;

/// The first sector a chunk may use; 0 and 1 are the header.
pub const FIRST_CHUNK_SECTOR: u32 = 2;

/// Compression id flag marking a chunk stored in an external `.mcc` file.
const EXTERNAL_STREAM_FLAG: u8 = 128;

/// Location entry of one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLocation {
    /// First sector of the payload.
    pub sector: u32,
    /// Number of sectors allocated to the payload.
    pub sector_count: u32,
}

impl ChunkLocation {
    /// Decode the packed u32; `None` for an empty slot.
    #[must_use]
    pub const fn decode(raw: u32) -> Option<Self> {
        if raw == 0 {
            return None;
        }
        Some(Self {
            sector: raw >> 8,
            sector_count: raw & 0xFF,
        })
    }

    /// Encode back into the packed u32.
    #[must_use]
    pub const fn encode(self) -> u32 {
        (self.sector << 8) | (self.sector_count & 0xFF)
    }

    /// Byte offset of the payload start.
    #[must_use]
    pub const fn byte_offset(self) -> u64 {
        self.sector as u64 * SECTOR_BYTES as u64
    }
}

/// The 8 KiB region header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionHeader {
    locations: [u32; CHUNK_SLOTS],
    timestamps: [i32; CHUNK_SLOTS],
}

impl RegionHeader {
    /// An all-empty header.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            locations: [0; CHUNK_SLOTS],
            timestamps: [0; CHUNK_SLOTS],
        }
    }

    /// Parse a header from the first 8 KiB of a region file.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when fewer than [`HEADER_BYTES`] are given.
    pub fn parse(bytes: &[u8]) -> ServerResult<Self> {
        if bytes.len() < HEADER_BYTES {
            return Err(ServerError::CorruptData(format!(
                "region header truncated: {} bytes, expected at least {HEADER_BYTES}",
                bytes.len()
            )));
        }
        let mut header = Self::empty();
        for slot in 0..CHUNK_SLOTS {
            let at = slot * 4;
            header.locations[slot] =
                u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            let at = SECTOR_BYTES + slot * 4;
            header.timestamps[slot] =
                i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        }
        Ok(header)
    }

    /// Location of a slot, if populated.
    #[must_use]
    pub fn location(&self, slot: usize) -> Option<ChunkLocation> {
        self.locations
            .get(slot)
            .copied()
            .and_then(ChunkLocation::decode)
    }

    /// Raw location word of a slot.
    #[must_use]
    pub fn raw_location(&self, slot: usize) -> u32 {
        self.locations.get(slot).copied().unwrap_or(0)
    }

    /// Timestamp of a slot (seconds since the epoch, as stored).
    #[must_use]
    pub fn timestamp(&self, slot: usize) -> i32 {
        self.timestamps.get(slot).copied().unwrap_or(0)
    }

    /// Slots that currently hold a chunk.
    pub fn occupied_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.locations
            .iter()
            .enumerate()
            .filter(|(_, raw)| **raw != 0)
            .map(|(slot, _)| slot)
    }

    /// Number of populated slots.
    #[must_use]
    pub fn occupied_count(&self) -> usize {
        self.occupied_slots().count()
    }

    fn set_location(&mut self, slot: usize, location: Option<ChunkLocation>, now: i32) {
        if slot >= CHUNK_SLOTS {
            return;
        }
        self.locations[slot] = location.map_or(0, ChunkLocation::encode);
        self.timestamps[slot] = now;
    }
}

/// Tracks which sectors of a region file are in use, for allocation.
#[derive(Debug, Clone)]
struct SectorBitmap {
    used: Vec<bool>,
}

impl SectorBitmap {
    fn from_header(header: &RegionHeader, sectors_in_file: u32) -> Self {
        let mut bitmap = Self {
            used: vec![false; sectors_in_file.max(FIRST_CHUNK_SECTOR) as usize],
        };
        // The header itself is always in use.
        bitmap.mark(0, FIRST_CHUNK_SECTOR);
        for slot in header.occupied_slots() {
            if let Some(location) = header.location(slot)
                && location.sector >= FIRST_CHUNK_SECTOR
            {
                bitmap.mark(location.sector, location.sector_count);
            }
        }
        bitmap
    }

    fn mark(&mut self, sector: u32, count: u32) {
        for index in sector..sector.saturating_add(count) {
            if let Some(entry) = self.used.get_mut(index as usize) {
                *entry = true;
            }
        }
    }

    fn free(&mut self, sector: u32, count: u32) {
        for index in sector..sector.saturating_add(count) {
            if let Some(entry) = self.used.get_mut(index as usize) {
                *entry = false;
            }
        }
    }

    /// First run of `count` free sectors, growing the file when needed.
    fn allocate(&mut self, count: u32) -> u32 {
        let needed = count as usize;
        let mut run_start = FIRST_CHUNK_SECTOR as usize;
        let mut run_len = 0usize;
        for index in FIRST_CHUNK_SECTOR as usize..self.used.len() {
            if self.used[index] {
                run_start = index + 1;
                run_len = 0;
            } else {
                run_len += 1;
                if run_len == needed {
                    self.mark(run_start as u32, count);
                    return run_start as u32;
                }
            }
        }
        // No run big enough: append at the end of the file.
        let sector = self.used.len().max(FIRST_CHUNK_SECTOR as usize) as u32;
        self.used.resize(sector as usize + needed, false);
        self.mark(sector, count);
        sector
    }

    fn sectors(&self) -> u32 {
        self.used.len() as u32
    }
}

/// A chunk read out of a region file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChunk {
    /// Compression the chunk was stored with.
    pub compression: Compression,
    /// Decompressed chunk NBT.
    pub data: Vec<u8>,
    /// Stored timestamp (seconds since the epoch).
    pub timestamp: i32,
}

/// An open Anvil region file (`r.X.Z.mca`).
#[derive(Debug)]
pub struct RegionFile {
    path: PathBuf,
    file: File,
    header: RegionHeader,
    bitmap: SectorBitmap,
    readonly: bool,
    /// Test-only write journal: every `(offset, len)` passed to `write_all_at`,
    /// in order. Compiled out of production builds; it exists so the
    /// commit-point ordering test can observe *sequence*, which no end-state
    /// read can see (AUDIT-09 B-02).
    #[cfg(test)]
    journal: Vec<(u64, usize)>,
}

impl RegionFile {
    /// Open (creating if needed) a region file for reading and writing.
    ///
    /// A zero-length file — which vanilla creates for `entities`/`poi` folders
    /// of an unused region — is treated as an empty region rather than as
    /// corruption.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file cannot be opened or created,
    /// [`ServerError::CorruptData`] when an existing file has a truncated header.
    pub fn open(path: &Path) -> ServerResult<Self> {
        Self::open_with(path, false)
    }

    /// Open an existing region file read-only.
    ///
    /// # Errors
    ///
    /// As for [`RegionFile::open`], plus [`ServerError::Operational`] when the
    /// file does not exist.
    pub fn open_readonly(path: &Path) -> ServerResult<Self> {
        Self::open_with(path, true)
    }

    fn open_with(path: &Path, readonly: bool) -> ServerResult<Self> {
        let file = if readonly {
            File::open(path)
        } else {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
        }
        .map_err(|e| {
            ServerError::Operational(format!("cannot open region file {}: {e}", path.display()))
        })?;

        let mut this = Self {
            path: path.to_path_buf(),
            file,
            header: RegionHeader::empty(),
            bitmap: SectorBitmap::from_header(&RegionHeader::empty(), FIRST_CHUNK_SECTOR),
            readonly,
            #[cfg(test)]
            journal: Vec::new(),
        };
        let len = this.length()?;
        if len == 0 {
            debug!(path = %path.display(), "region file is empty; treating as unused region");
            return Ok(this);
        }
        if (len as usize) < HEADER_BYTES {
            return Err(ServerError::CorruptData(format!(
                "region file {} is {len} bytes, shorter than the {HEADER_BYTES}-byte header",
                path.display()
            )));
        }
        let mut header_bytes = vec![0u8; HEADER_BYTES];
        this.read_exact_at(0, &mut header_bytes)?;
        this.header = RegionHeader::parse(&header_bytes)?;
        this.bitmap = SectorBitmap::from_header(&this.header, this.sector_count());
        Ok(this)
    }

    /// Path of this region file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current file length in bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file metadata cannot be read.
    pub fn length(&self) -> ServerResult<u64> {
        self.file
            .metadata()
            .map(|meta| meta.len())
            .map_err(|e| ServerError::Operational(format!("stat {}: {e}", self.path.display())))
    }

    /// Number of sectors the file covers, rounding a partial trailing sector up.
    ///
    /// Vanilla does not always pad a region file to a whole sector: a save
    /// interrupted before `RegionFile.close` leaves a short final sector (found
    /// on a real 26.1.2 world: `r.-1.-1.mca` is 577 884 bytes = 141 sectors plus
    /// 348 bytes, with the last chunk starting at sector 141). Rounding up keeps
    /// that partial sector marked as used so the writer never overwrites a chunk
    /// vanilla can still read.
    fn sector_count(&self) -> u32 {
        let Ok(len) = self.length() else {
            return 0;
        };
        len.div_ceil(SECTOR_BYTES as u64) as u32
    }

    /// The parsed header.
    #[must_use]
    pub const fn header(&self) -> &RegionHeader {
        &self.header
    }

    /// Whether a chunk is present in the given slot.
    #[must_use]
    pub fn has_chunk_at(&self, slot: usize) -> bool {
        self.header.location(slot).is_some()
    }

    /// Whether a chunk is present at the given chunk coordinates.
    #[must_use]
    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        self.has_chunk_at(pos.slot())
    }

    /// Read and decompress a chunk.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] for any structural violation (see the module
    /// docs), [`ServerError::Operational`] for I/O failures and unsupported
    /// codecs, [`ServerError::Protocol`] when the decompressed NBT is malformed.
    pub fn read_chunk(&mut self, pos: ChunkPos) -> ServerResult<Option<StoredChunk>> {
        self.read_chunk_at(pos.slot(), DEFAULT_PAYLOAD_LIMIT)
    }

    /// [`RegionFile::read_chunk`] with an explicit decompressed-size limit.
    ///
    /// # Errors
    ///
    /// As for [`RegionFile::read_chunk`].
    pub fn read_chunk_at(
        &mut self,
        slot: usize,
        limit: usize,
    ) -> ServerResult<Option<StoredChunk>> {
        let Some(location) = self.header.location(slot) else {
            return Ok(None);
        };
        let where_ = format!("{} slot {slot}", self.path.display());
        if location.sector < FIRST_CHUNK_SECTOR {
            return Err(ServerError::CorruptData(format!(
                "{where_}: sector {} overlaps the region header",
                location.sector
            )));
        }
        if location.sector_count == 0 {
            return Err(ServerError::CorruptData(format!(
                "{where_}: location declares zero sectors"
            )));
        }
        let file_len = self.length()?;
        let start = location.byte_offset();
        if start >= file_len {
            return Err(ServerError::CorruptData(format!(
                "{where_}: sector {} starts past the end of the {file_len}-byte file",
                location.sector
            )));
        }
        // The header allocates `sector_count` sectors, but the file may hold
        // fewer bytes than that allocation (an interrupted save). Vanilla reads
        // whatever is present and only fails if the payload does not fit, so the
        // effective capacity is the smaller of the two.
        let allocated = location.sector_count as usize * SECTOR_BYTES;
        let present = (file_len - start) as usize;
        let usable = allocated.min(present);
        if usable < 5 {
            return Err(ServerError::CorruptData(format!(
                "{where_}: only {usable} bytes present at sector {}, need at least 5",
                location.sector
            )));
        }

        let mut prefix = [0u8; 5];
        self.read_exact_at(start, &mut prefix)?;
        let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        let codec_byte = prefix[4];
        if length < 1 {
            return Err(ServerError::CorruptData(format!(
                "{where_}: length field is {length}"
            )));
        }
        let payload_len = (length - 1) as usize;
        let capacity = usable - 4;
        if payload_len > capacity {
            return Err(ServerError::CorruptData(format!(
                "{where_}: length field claims {payload_len} payload bytes but only {capacity} \
                 are available ({} sectors allocated, {present} bytes present)",
                location.sector_count
            )));
        }

        if codec_byte & EXTERNAL_STREAM_FLAG != 0 {
            return Err(ServerError::Operational(format!(
                "{where_}: chunk is stored in an external .mcc file (codec byte {codec_byte:#04x}), \
                 which this build does not read"
            )));
        }
        let compression = Compression::from_id(codec_byte).ok_or_else(|| {
            ServerError::CorruptData(format!("{where_}: unknown compression id {codec_byte}"))
        })?;

        let mut payload = vec![0u8; payload_len];
        self.read_exact_at(location.byte_offset() + 5, &mut payload)?;
        let data = compression.decompress(&payload, limit)?;
        Ok(Some(StoredChunk {
            compression,
            data,
            timestamp: self.header.timestamp(slot),
        }))
    }

    /// Write (or replace) a chunk, compressing with [`Compression::Zlib`].
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the encoded chunk cannot fit in
    /// [`MAX_SECTORS_PER_CHUNK`] sectors, [`ServerError::Operational`] for I/O
    /// failures or a read-only handle, [`ServerError::CorruptData`] when `slot`
    /// is out of range.
    pub fn write_chunk(
        &mut self,
        pos: ChunkPos,
        data: &[u8],
        compression: Compression,
        now: i32,
    ) -> ServerResult<()> {
        self.write_chunk_at(pos.slot(), data, compression, now)
    }

    /// [`RegionFile::write_chunk`] addressed by raw slot.
    ///
    /// # Errors
    ///
    /// As for [`RegionFile::write_chunk`].
    pub fn write_chunk_at(
        &mut self,
        slot: usize,
        data: &[u8],
        compression: Compression,
        now: i32,
    ) -> ServerResult<()> {
        if self.readonly {
            return Err(ServerError::Operational(format!(
                "region file {} is open read-only",
                self.path.display()
            )));
        }
        if slot >= CHUNK_SLOTS {
            return Err(ServerError::CorruptData(format!(
                "chunk slot {slot} is outside the {CHUNK_SLOTS}-slot region file"
            )));
        }
        let payload = compression.compress(data)?;
        let body_len = payload.len() + 1;
        if body_len + 4 > MAX_SECTORS_PER_CHUNK as usize * SECTOR_BYTES {
            return Err(ServerError::Invariant(format!(
                "chunk of {body_len} bytes cannot be stored: the location field allows at most \
                 {MAX_SECTORS_PER_CHUNK} sectors"
            )));
        }
        let sector_count = (body_len + 4).div_ceil(SECTOR_BYTES) as u32;

        // Ensure the header exists before the first payload write.
        if self.length()? == 0 {
            self.write_header()?;
        }

        let previous = self.header.location(slot);
        let sector = self.bitmap.allocate(sector_count);

        let mut body = Vec::with_capacity(sector_count as usize * SECTOR_BYTES);
        body.extend_from_slice(&(body_len as u32).to_be_bytes());
        body.push(compression.id());
        body.extend_from_slice(&payload);
        body.resize(sector_count as usize * SECTOR_BYTES, 0);
        self.write_all_at(sector as u64 * SECTOR_BYTES as u64, &body)?;

        if let Some(previous) = previous {
            let same_run = previous.sector == sector && previous.sector_count == sector_count;
            if !same_run {
                self.bitmap.free(previous.sector, previous.sector_count);
            }
        }
        self.header.set_location(
            slot,
            Some(ChunkLocation {
                sector,
                sector_count,
            }),
            now,
        );
        // Timestamp first, location last: the location word is the commit point.
        self.write_i32_at((SECTOR_BYTES + slot * 4) as u64, now)?;
        self.write_u32_at((slot * 4) as u64, self.header.raw_location(slot))?;
        Ok(())
    }

    /// Remove a chunk, freeing its sectors.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] for a read-only handle or an I/O failure.
    pub fn remove_chunk(&mut self, pos: ChunkPos) -> ServerResult<bool> {
        let slot = pos.slot();
        let Some(location) = self.header.location(slot) else {
            return Ok(false);
        };
        if self.readonly {
            return Err(ServerError::Operational(format!(
                "region file {} is open read-only",
                self.path.display()
            )));
        }
        self.bitmap.free(location.sector, location.sector_count);
        self.header.set_location(slot, None, 0);
        self.write_u32_at((slot * 4) as u64, 0)?;
        self.write_i32_at((SECTOR_BYTES + slot * 4) as u64, 0)?;
        Ok(true)
    }

    /// Flush pending writes to stable storage.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the flush fails.
    pub fn sync(&mut self) -> ServerResult<()> {
        if self.readonly {
            return Ok(());
        }
        self.file
            .sync_data()
            .map_err(|e| ServerError::Operational(format!("fsync {}: {e}", self.path.display())))
    }

    /// Write the full 8 KiB header (used when initialising a new file).
    fn write_header(&mut self) -> ServerResult<()> {
        let mut bytes = vec![0u8; HEADER_BYTES];
        for slot in 0..CHUNK_SLOTS {
            let at = slot * 4;
            bytes[at..at + 4].copy_from_slice(&self.header.locations[slot].to_be_bytes());
            let at = SECTOR_BYTES + slot * 4;
            bytes[at..at + 4].copy_from_slice(&self.header.timestamps[slot].to_be_bytes());
        }
        self.write_all_at(0, &bytes)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> ServerResult<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ServerError::Operational(format!("seek {}: {e}", self.path.display())))?;
        self.file.read_exact(out).map_err(|e| {
            ServerError::CorruptData(format!(
                "short read at offset {offset} in {}: {e}",
                self.path.display()
            ))
        })
    }

    fn write_all_at(&mut self, offset: u64, bytes: &[u8]) -> ServerResult<()> {
        #[cfg(test)]
        self.journal.push((offset, bytes.len()));
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ServerError::Operational(format!("seek {}: {e}", self.path.display())))?;
        self.file
            .write_all(bytes)
            .map_err(|e| ServerError::Operational(format!("write {}: {e}", self.path.display())))
    }

    fn write_u32_at(&mut self, offset: u64, value: u32) -> ServerResult<()> {
        self.write_all_at(offset, &value.to_be_bytes())
    }

    fn write_i32_at(&mut self, offset: u64, value: i32) -> ServerResult<()> {
        self.write_all_at(offset, &value.to_be_bytes())
    }

    /// Snapshot the test-only write journal (order of `write_all_at` calls).
    #[cfg(test)]
    fn write_journal(&self) -> Vec<(u64, usize)> {
        self.journal.clone()
    }

    /// Clear the test-only write journal.
    #[cfg(test)]
    fn clear_write_journal(&mut self) {
        self.journal.clear();
    }

    /// Sectors currently allocated (including the 2-sector header).
    #[must_use]
    pub fn allocated_sectors(&self) -> u32 {
        self.bitmap.sectors()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHUNK_SLOTS, ChunkLocation, FIRST_CHUNK_SECTOR, HEADER_BYTES, RegionFile, RegionHeader,
        SECTOR_BYTES, SectorBitmap,
    };
    use crate::chunk::ChunkPos;
    use crate::compression::Compression;
    use mc_core::error::ServerError;
    use mc_test_support::fixtures::TempDir;

    #[test]
    fn location_encoding_is_the_vanilla_packing() {
        assert_eq!(ChunkLocation::decode(0), None);
        let location = ChunkLocation {
            sector: 0x0000_ABCD,
            sector_count: 0xEF,
        };
        assert_eq!(location.encode(), 0x00AB_CDEF);
        assert_eq!(ChunkLocation::decode(0x00AB_CDEF), Some(location));
        // A sector count of 0 is still a "present" slot on the wire; the reader
        // rejects it, but decoding keeps the value so the error can name it.
        assert_eq!(
            ChunkLocation::decode(0x0000_0400),
            Some(ChunkLocation {
                sector: 4,
                sector_count: 0
            })
        );
        assert_eq!(location.byte_offset(), 0x0000_ABCD * 4096);
        assert_eq!(FIRST_CHUNK_SECTOR, 2);
    }

    #[test]
    fn header_round_trip_and_truncation() {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[0..4].copy_from_slice(&((5u32 << 8) | 2).to_be_bytes());
        bytes[SECTOR_BYTES..SECTOR_BYTES + 4].copy_from_slice(&1234i32.to_be_bytes());
        let header = RegionHeader::parse(&bytes).expect("parses");
        assert_eq!(
            header.location(0),
            Some(ChunkLocation {
                sector: 5,
                sector_count: 2
            })
        );
        assert_eq!(header.timestamp(0), 1234);
        assert_eq!(header.occupied_count(), 1);
        assert_eq!(header.occupied_slots().collect::<Vec<_>>(), vec![0]);
        assert!(RegionHeader::parse(&bytes[..HEADER_BYTES - 1]).is_err());
    }

    #[test]
    fn sector_bitmap_allocates_the_first_free_run_and_appends() {
        let mut header = RegionHeader::empty();
        header.set_location(
            0,
            Some(ChunkLocation {
                sector: 2,
                sector_count: 2,
            }),
            0,
        );
        let mut bitmap = SectorBitmap::from_header(&header, 8);
        // Sectors 0-1 header, 2-3 used → first free run is 4.
        assert_eq!(bitmap.allocate(2), 4);
        // 4-5 now used → next is 6.
        assert_eq!(bitmap.allocate(1), 6);
        // Only sector 7 free: a 2-sector request must append at the end.
        assert_eq!(bitmap.allocate(2), 8);
        bitmap.free(4, 2);
        assert_eq!(bitmap.allocate(2), 4);
    }

    fn temp_region(tag: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new(tag);
        let path = dir.path().join("r.0.0.mca");
        (dir, path)
    }

    #[test]
    fn empty_region_file_is_not_corruption() {
        let dir = TempDir::new("region-empty");
        let path = dir.path().join("r.0.0.mca");
        std::fs::write(&path, []).expect("create empty file");
        let mut region = RegionFile::open(&path).expect("opens");
        assert_eq!(region.length().expect("len"), 0);
        assert!(!region.has_chunk(ChunkPos::new(0, 0)));
        assert!(
            region
                .read_chunk(ChunkPos::new(0, 0))
                .expect("reads")
                .is_none()
        );
    }

    #[test]
    fn write_then_read_round_trip() {
        let (_dir, path) = temp_region("region-rt");
        let mut region = RegionFile::open(&path).expect("opens");
        let payload = b"chunk nbt bytes".repeat(50);
        region
            .write_chunk(ChunkPos::new(3, 4), &payload, Compression::Zlib, 77)
            .expect("writes");
        assert!(region.has_chunk(ChunkPos::new(3, 4)));
        let stored = region
            .read_chunk(ChunkPos::new(3, 4))
            .expect("reads")
            .expect("present");
        assert_eq!(stored.data, payload);
        assert_eq!(stored.compression, Compression::Zlib);
        assert_eq!(stored.timestamp, 77);
        // Re-open from disk: the on-disk header must be self-consistent.
        drop(region);
        let mut reopened = RegionFile::open(&path).expect("reopens");
        assert_eq!(
            reopened
                .read_chunk(ChunkPos::new(3, 4))
                .expect("reads")
                .expect("present")
                .data,
            payload
        );
        assert_eq!(reopened.header().occupied_count(), 1);
    }

    #[test]
    fn slots_are_independent_and_negative_coordinates_work() {
        let (_dir, path) = temp_region("region-slots");
        let mut region = RegionFile::open(&path).expect("opens");
        // All four chunks live in region (-2, -1); their slots are the corners
        // of that region's 32×32 grid.
        let positions = [
            ChunkPos::new(-64, -32), // local (0, 0)    -> slot 0
            ChunkPos::new(-33, -1),  // local (31, 31)  -> slot 1023
            ChunkPos::new(-64, -1),  // local (0, 31)   -> slot 992
            ChunkPos::new(-33, -32), // local (31, 0)   -> slot 31
        ];
        for (index, pos) in positions.into_iter().enumerate() {
            assert_eq!(pos.region_x(), -2, "{pos:?}");
            assert_eq!(pos.region_z(), -1, "{pos:?}");
            region
                .write_chunk(
                    pos,
                    format!("chunk-{index}").as_bytes(),
                    Compression::Zlib,
                    1,
                )
                .expect("writes");
        }
        assert_eq!(region.header().occupied_count(), 4);
        assert_eq!(positions[1].slot(), 1023);
        assert_eq!(positions[3].slot(), 31);
        for (index, pos) in positions.into_iter().enumerate() {
            let stored = region.read_chunk(pos).expect("reads").expect("present");
            assert_eq!(stored.data, format!("chunk-{index}").into_bytes());
        }
        assert!(
            region
                .read_chunk(ChunkPos::new(-40, -20))
                .expect("reads")
                .is_none(),
            "unwritten slot stays empty"
        );
    }

    #[test]
    fn rewriting_a_chunk_reuses_or_frees_sectors() {
        let (_dir, path) = temp_region("region-rewrite");
        let mut region = RegionFile::open(&path).expect("opens");
        let big = vec![7u8; SECTOR_BYTES * 3];
        region
            .write_chunk(ChunkPos::new(0, 0), &big, Compression::None, 1)
            .expect("writes big");
        let after_big = region.length().expect("len");
        assert!(after_big >= (FIRST_CHUNK_SECTOR as u64 + 3) * SECTOR_BYTES as u64);
        // Overwrite with something tiny. Vanilla (and this implementation)
        // allocates the new run *before* freeing the old one, so the file may
        // grow once here; what matters is that the freed run is reused after.
        region
            .write_chunk(ChunkPos::new(0, 0), b"small", Compression::None, 2)
            .expect("writes small");
        let stored = region
            .read_chunk(ChunkPos::new(0, 0))
            .expect("reads")
            .expect("present");
        assert_eq!(stored.data, b"small");
        assert_eq!(stored.timestamp, 2);
        let grew = region.length().expect("len");
        assert!(
            grew <= after_big + SECTOR_BYTES as u64,
            "one replacement may add a single sector run, found {grew} vs {after_big}"
        );
        // ... and the sectors freed by the shrink are handed back out, so a
        // second same-size chunk does not grow the file at all.
        region
            .write_chunk(ChunkPos::new(1, 1), &big, Compression::None, 3)
            .expect("writes another big chunk");
        assert_eq!(
            region.length().expect("len"),
            grew,
            "the freed 3-sector run must be reused"
        );
    }

    #[test]
    fn the_location_word_is_written_last() {
        // P08-07: the parity matrix claimed "header word last" with no test
        // behind it (Audit 05). This is that test: after writing a chunk over
        // an existing one, the on-disk location word must point at a payload
        // whose length field fits the allocated sectors — i.e. the commit point
        // landed after the payload, not before it.
        use std::io::{Read, Seek, SeekFrom};
        let (_dir, path) = temp_region("region-ordering");
        let mut region = RegionFile::open(&path).expect("opens");
        region
            .write_chunk(ChunkPos::new(0, 0), b"first", Compression::Zlib, 1)
            .expect("writes");
        region
            .write_chunk(ChunkPos::new(0, 0), b"second-payload", Compression::Zlib, 2)
            .expect("rewrites");
        region.sync().expect("syncs");
        drop(region);

        let mut file = std::fs::File::open(&path).expect("opens");
        let slot = ChunkPos::new(0, 0).slot();
        let mut word = [0u8; 4];
        file.seek(SeekFrom::Start((slot * 4) as u64))
            .expect("seeks");
        file.read_exact(&mut word).expect("reads");
        let raw = u32::from_be_bytes(word);
        let location = ChunkLocation::decode(raw).expect("slot is occupied");
        assert!(location.sector >= FIRST_CHUNK_SECTOR);
        assert!(location.sector_count >= 1);
        // The payload at that sector must be self-consistent.
        let mut prefix = [0u8; 5];
        file.seek(SeekFrom::Start(location.byte_offset()))
            .expect("seeks");
        file.read_exact(&mut prefix).expect("reads");
        let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        assert!(
            length >= 1 && (length as usize) + 4 <= location.sector_count as usize * SECTOR_BYTES,
            "location word points at a fitting payload (len {length}, {} sectors)",
            location.sector_count
        );
        // And the committed payload is the second write, not the first.
        let mut reopened = RegionFile::open(&path).expect("reopens");
        let stored = reopened
            .read_chunk(ChunkPos::new(0, 0))
            .expect("reads")
            .expect("present");
        assert_eq!(stored.data, b"second-payload");
        assert_eq!(stored.timestamp, 2);
    }

    #[test]
    fn location_word_is_written_after_payload() {
        // AUDIT-09 B-02: the end-state test above passes under a reordered
        // write (both orders reach the same bytes when nothing crashes
        // between them). This one observes the *sequence* through the
        // test-only journal: the location word (offset `slot * 4`, 4 bytes)
        // must land after the payload write. A crash between payload and
        // commit then leaves the old word pointing at the old payload —
        // never a new word pointing at a torn one.
        let (_dir, path) = temp_region("region-commit-order");
        let mut region = RegionFile::open(&path).expect("opens");
        region
            .write_chunk(ChunkPos::new(0, 0), b"first", Compression::Zlib, 1)
            .expect("writes");
        region.clear_write_journal();
        region
            .write_chunk(ChunkPos::new(0, 0), b"second-payload", Compression::Zlib, 2)
            .expect("rewrites");
        let journal = region.write_journal();
        let slot_offset = (ChunkPos::new(0, 0).slot() * 4) as u64;
        let location = journal
            .iter()
            .position(|&(offset, len)| offset == slot_offset && len == 4)
            .expect("a location-word write is journalled");
        let payload = journal
            .iter()
            .position(|&(offset, len)| offset != 0 && len >= SECTOR_BYTES)
            .expect("a payload write is journalled");
        assert!(
            location > payload,
            "location word at journal index {location} must follow the payload write at {payload}: {journal:?}"
        );
    }

    #[test]
    fn removal_frees_the_slot() {
        let (_dir, path) = temp_region("region-remove");
        let mut region = RegionFile::open(&path).expect("opens");
        region
            .write_chunk(ChunkPos::new(1, 1), b"data", Compression::Zlib, 5)
            .expect("writes");
        assert!(region.remove_chunk(ChunkPos::new(1, 1)).expect("removes"));
        assert!(
            !region
                .remove_chunk(ChunkPos::new(1, 1))
                .expect("idempotent")
        );
        assert!(
            region
                .read_chunk(ChunkPos::new(1, 1))
                .expect("reads")
                .is_none()
        );
        drop(region);
        let reopened = RegionFile::open(&path).expect("reopens");
        assert_eq!(reopened.header().occupied_count(), 0);
    }

    #[test]
    fn oversized_chunks_are_refused_not_truncated() {
        let (_dir, path) = temp_region("region-oversize");
        let mut region = RegionFile::open(&path).expect("opens");
        // Uncompressed, so the stored size equals the input size.
        let huge = vec![0u8; (super::MAX_SECTORS_PER_CHUNK as usize) * SECTOR_BYTES];
        let err = region
            .write_chunk(ChunkPos::new(0, 0), &huge, Compression::None, 1)
            .expect_err("too big");
        assert!(matches!(err, ServerError::Invariant(_)), "{err:?}");
    }

    #[test]
    fn read_only_files_reject_writes() {
        let (_dir, path) = temp_region("region-ro");
        let mut region = RegionFile::open(&path).expect("opens");
        region
            .write_chunk(ChunkPos::new(0, 0), b"x", Compression::Zlib, 1)
            .expect("writes");
        drop(region);
        let mut readonly = RegionFile::open_readonly(&path).expect("opens ro");
        assert!(
            readonly
                .read_chunk(ChunkPos::new(0, 0))
                .expect("reads")
                .is_some()
        );
        assert!(
            readonly
                .write_chunk(ChunkPos::new(0, 0), b"y", Compression::Zlib, 2)
                .is_err()
        );
        assert!(readonly.remove_chunk(ChunkPos::new(0, 0)).is_err());
        assert!(readonly.sync().is_ok(), "sync is a no-op when read-only");
    }

    #[test]
    fn every_chunk_slot_is_addressable() {
        let (_dir, path) = temp_region("region-slots-all");
        let mut region = RegionFile::open(&path).expect("opens");
        for x in 0..32 {
            for z in 0..32 {
                let pos = ChunkPos::new(x, z);
                assert!(pos.slot() < CHUNK_SLOTS);
                region
                    .write_chunk(pos, &[x as u8, z as u8], Compression::None, 1)
                    .expect("writes");
            }
        }
        assert_eq!(region.header().occupied_count(), CHUNK_SLOTS);
        for x in 0..32 {
            for z in 0..32 {
                let stored = region
                    .read_chunk(ChunkPos::new(x, z))
                    .expect("reads")
                    .expect("present");
                assert_eq!(stored.data, vec![x as u8, z as u8]);
            }
        }
    }
}
