//! World storage wiring for the server lifecycle (P03-09/13 slice).
//!
//! This module is the single place where the binary touches the filesystem
//! layout: it resolves [`crate::config::StorageConfig`] into a
//! [`WorldStorage`], creates the world on first start, and closes it with a
//! flush on shutdown.
//!
//! Scope note: gameplay (chunk generation, block updates, players) is Phase 04.
//! What exists here is the *operational* slice — the world directory is opened
//! at startup and saved at shutdown — so the persistence layer is exercised by
//! the real binary rather than only by tests, and so Phase 04 has one obvious
//! place to attach its chunk queue.
//!
//! Threading (AGENTS.md section 8): `WorldStorage` is single-owner. Today the
//! tick thread owns it through this struct; Phase 04 moves it to a save worker
//! and hands chunks over a bounded channel. Until then, saves happen inline on
//! the tick thread (which is why autosave is off by default in tests).

use crate::config::StorageConfig;
use mc_core::error::ServerResult;
use mc_persistence::world::WorldStorage;
use std::path::Path;

/// Resolve the world-generation seed for a boot (AUDIT-18 F-H1).
///
/// A seed the world **records** always wins: opening a world that names one
/// (vanilla, or a previous boot here) must generate from it, or new chunks fork
/// the terrain. Otherwise the configured seed applies (fresh worlds), else the
/// historical seed-0 default, stated rather than hidden.
///
/// A world with no recorded seed is the only case this value can change the
/// outcome for, and [`WorldService::open`] + [`WorldService::record_seed`]
/// close that window: every world this build creates or opens records the seed
/// it generated from (AUDIT-19 D-19-M1).
#[must_use]
pub fn resolve_seed(configured: Option<i64>, stored: Option<i64>) -> i64 {
    stored
        .or(configured)
        .unwrap_or(crate::game::DEFAULT_RANDOM_SEED)
}

/// The seed recorded in the world's 26.1 world-gen settings document, if any
/// (AUDIT-19 D-19-H1).
///
/// `mc-persistence` reads the file — which lives outside `level.dat`, where a
/// real 26.1.2 world keeps its world generation — and `mc-worldgen` interprets
/// the document's key paths. A document that parses but names no seed is the
/// "no recorded seed" case, not a failure.
///
/// # Errors
///
/// [`ServerError::Operational`] / [`ServerError::CorruptData`] when the document
/// exists but cannot be read or understood: see
/// [`mc_persistence::level::read_world_gen_settings`] for why that is an error
/// rather than a silent fallback.
fn world_gen_settings_seed(root: &Path) -> ServerResult<Option<i64>> {
    let Some(document) = mc_persistence::level::read_world_gen_settings(root)? else {
        return Ok(None);
    };
    // `from_level_dat` reports "this document has no seed" as `CorruptData`;
    // here that is exactly the answer `Ok(None)` means, and the caller decides
    // the fallback.
    Ok(mc_worldgen::seed::WorldSeed::from_level_dat(&document)
        .ok()
        .map(mc_worldgen::seed::WorldSeed::raw))
}

/// Owns the open world for the lifetime of the server process.
#[derive(Debug)]
pub struct WorldService {
    storage: WorldStorage,
}

impl WorldService {
    /// Open (or create) the world described by `config`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the directory cannot be created or a
    /// region file cannot be opened; [`ServerError::CorruptData`] when
    /// `level.dat` is unreadable or its `DataVersion` is outside the supported
    /// window, or when the world's 26.1 world-gen settings document exists but
    /// cannot be read (AUDIT-19 D-19-H1).
    pub fn open(config: &StorageConfig) -> ServerResult<Self> {
        let root = config.world_dir.as_path();
        let mut storage = WorldStorage::open(root, config.autosave_ticks)?;
        if storage.level().is_none() {
            let name = root
                .file_name()
                .map_or_else(|| "world".to_owned(), |n| n.to_string_lossy().into_owned());
            let mut level = mc_persistence::level::LevelDat::new(
                &name,
                mc_persistence::save::unix_millis(std::time::SystemTime::now()),
            );
            // AUDIT-19 D-19-M1: record the seed this world is about to generate
            // from, so adding `seed = N` to the config later cannot fork it and
            // removing one cannot flip it back. A directory can hold world-gen
            // settings without a `level.dat` (a vanilla world whose metadata was
            // lost), and that recorded seed still wins over the configured one.
            level.seed = Some(resolve_seed(config.seed, world_gen_settings_seed(root)?));
            let seeded = level.seed;
            storage.save_level(level)?;
            tracing::info!(world = %root.display(), seed = ?seeded, "created a new world");
        }
        tracing::info!(
            world = %root.display(),
            level_name = storage.level_name(),
            autosave_ticks = config.autosave_ticks,
            "world storage ready"
        );
        Ok(Self { storage })
    }

    /// The seed this world records, if it records one (AUDIT-19 D-19-H1).
    ///
    /// `level.dat` first — the flat key this build writes and pre-26.1 worlds
    /// carry — then the 26.1 world-gen settings document, which is where a real
    /// 26.1.2 world keeps it and whose `level.dat` has **no** seed key at all
    /// (`mc_persistence::level` module docs). Without that second read, every
    /// newly generated chunk of a vanilla world was seed-0 terrain, which is the
    /// case AUDIT-18 F-H1's fix was written for and did not reach.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] / [`ServerError::CorruptData`] when the
    /// world-gen settings document exists but cannot be read: silently falling
    /// back to another seed is the fork this function exists to prevent.
    pub fn recorded_seed(&self) -> ServerResult<Option<i64>> {
        if let Some(seed) = self.storage.level().and_then(|level| level.seed) {
            return Ok(Some(seed));
        }
        world_gen_settings_seed(self.root())
    }

    /// Record `seed` in `level.dat`, so a later config change cannot re-seed
    /// this world (AUDIT-19 D-19-M1).
    ///
    /// Written eagerly rather than marked dirty: the record is what makes a
    /// world's terrain reproducible across boots, so it must not depend on a
    /// later flush happening. A no-op when the level already records `seed`,
    /// which is the steady state for every world this build has opened once.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when `level.dat` cannot be written.
    pub fn record_seed(&mut self, seed: i64) -> ServerResult<()> {
        let Some(mut level) = self.storage.level().cloned() else {
            // No level yet: `WorldService::open` records the seed when it
            // creates one, so there is nothing to attach the record to here.
            return Ok(());
        };
        if level.seed == Some(seed) {
            return Ok(());
        }
        level.seed = Some(seed);
        self.storage.save_level(level)?;
        tracing::info!(seed, "recorded the world's generation seed in level.dat");
        Ok(())
    }

    /// The underlying storage handle (Phase 04 attaches the chunk queue here).
    #[must_use]
    pub const fn storage(&self) -> &WorldStorage {
        &self.storage
    }

    /// Mutable access to the storage handle.
    pub fn storage_mut(&mut self) -> &mut WorldStorage {
        &mut self.storage
    }

    /// World directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.storage.root()
    }

    /// Advance the autosave clock and save when due.
    ///
    /// Returns `true` when a save ran (cleanly or not); a failed save is logged
    /// with its error list and stays queued for the next attempt.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when `level.dat` could not be written.
    pub fn on_tick(&mut self, tick: mc_core::tick::Tick) -> ServerResult<bool> {
        let Some(report) = self.storage.on_tick(tick)? else {
            return Ok(false);
        };
        if !report.is_clean() {
            for error in &report.errors {
                tracing::error!(error = %error, "autosave reported a failure");
            }
        }
        tracing::info!(
            chunks = report.chunks_written,
            failed = report.chunks_failed,
            level = report.level_written,
            "autosave finished"
        );
        Ok(true)
    }

    /// Flush and close the world (shutdown path).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when `level.dat` could not be written; chunk
    /// failures are logged and reported in the returned [`mc_persistence::SaveReport`].
    pub fn close(self) -> ServerResult<mc_persistence::SaveReport> {
        let root = self.storage.root().to_path_buf();
        let report = self.storage.close()?;
        if report.is_clean() {
            tracing::info!(
                world = %root.display(),
                chunks = report.chunks_written,
                "world saved and closed"
            );
        } else {
            for error in &report.errors {
                tracing::error!(error = %error, "save failure during shutdown");
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::{WorldService, resolve_seed};
    use crate::config::StorageConfig;
    use mc_persistence::level::{DATA_VERSION_26_1_2, LevelDat};
    use mc_test_support::fixtures::TempDir;

    fn config_for(dir: &TempDir, autosave_ticks: u64) -> StorageConfig {
        StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks,
            seed: None,
        }
    }

    #[test]
    fn seed_resolution_prefers_stored_then_configured_then_zero() {
        // A world that records a seed keeps it no matter the config: opening
        // a vanilla world must not fork its terrain (AUDIT-18 F-H1).
        assert_eq!(resolve_seed(Some(7), Some(1_361_882_806)), 1_361_882_806);
        assert_eq!(resolve_seed(None, Some(1_361_882_806)), 1_361_882_806);
        // Fresh worlds take the configured seed; unset means the
        // historical seed-0 default, stated not hidden.
        assert_eq!(resolve_seed(Some(42), None), 42);
        assert_eq!(resolve_seed(None, None), crate::game::DEFAULT_RANDOM_SEED);
    }

    #[test]
    fn opening_a_fresh_world_records_the_seed_it_will_generate_from() {
        // AUDIT-19 D-19-M1: with no configured seed the world generates from
        // the historical default, and that is what it must record — otherwise
        // adding `seed = N` later silently forks its terrain.
        let dir = TempDir::new("world-service-seed");
        let service = WorldService::open(&config_for(&dir, 0)).expect("opens");
        assert_eq!(
            service.storage().level().and_then(|level| level.seed),
            Some(crate::game::DEFAULT_RANDOM_SEED)
        );
        assert_eq!(service.recorded_seed().expect("recorded"), Some(0));
        // And it is durable, not merely cached: a fresh open of the same
        // directory reads it back off disk.
        service.close().expect("closes");
        let level = LevelDat::from_bytes(
            &std::fs::read(dir.path().join("world").join("level.dat")).expect("level.dat"),
        )
        .expect("decodes");
        assert_eq!(level.seed, Some(crate::game::DEFAULT_RANDOM_SEED));

        // A configured seed is recorded too, and a later boot without it keeps
        // generating from it (the existing F-H1 behaviour, now via the record).
        let dir = TempDir::new("world-service-seed-configured");
        let mut config = config_for(&dir, 0);
        config.seed = Some(1_361_882_806);
        let service = WorldService::open(&config).expect("opens");
        assert_eq!(
            service.storage().level().and_then(|level| level.seed),
            Some(1_361_882_806)
        );
        service.close().expect("closes");
    }

    #[test]
    fn record_seed_is_idempotent_and_updates_the_level() {
        let dir = TempDir::new("world-service-record-seed");
        let mut service = WorldService::open(&config_for(&dir, 0)).expect("opens");
        service.record_seed(4242).expect("records");
        assert_eq!(service.recorded_seed().expect("recorded"), Some(4242));
        // Recording the same value again writes nothing and stays correct.
        service.record_seed(4242).expect("records again");
        assert_eq!(service.recorded_seed().expect("recorded"), Some(4242));
        service.close().expect("closes");
        let level = LevelDat::from_bytes(
            &std::fs::read(dir.path().join("world").join("level.dat")).expect("level.dat"),
        )
        .expect("decodes");
        assert_eq!(level.seed, Some(4242));
    }

    #[test]
    fn opening_a_fresh_world_creates_level_dat() {
        let dir = TempDir::new("world-service");
        let config = config_for(&dir, 6000);
        let service = WorldService::open(&config).expect("opens");
        assert_eq!(service.root(), config.world_dir.as_path());
        let level = service.storage().level().expect("level created");
        assert_eq!(level.data_version, DATA_VERSION_26_1_2);
        assert_eq!(level.level_name, "world", "named after the folder");
        assert!(config.world_dir.join("level.dat").is_file());
        let report = service.close().expect("closes");
        assert!(report.is_clean(), "{report:?}");
    }

    #[test]
    fn reopening_keeps_the_existing_level() {
        let dir = TempDir::new("world-service-reopen");
        let config = config_for(&dir, 0);
        {
            let mut service = WorldService::open(&config).expect("opens");
            let mut level = service.storage().level().cloned().expect("level");
            level.time = 999;
            level.difficulty = mc_persistence::Difficulty::Hard;
            service
                .storage_mut()
                .save_level(level)
                .expect("saves level");
            service.close().expect("closes");
        }
        let service = WorldService::open(&config).expect("reopens");
        let level = service.storage().level().expect("level");
        assert_eq!(level.time, 999);
        assert_eq!(level.difficulty, mc_persistence::Difficulty::Hard);
        service.close().expect("closes");
    }

    #[test]
    fn autosave_ticks_from_config_drive_the_scheduler() {
        let dir = TempDir::new("world-service-autosave");
        let config = config_for(&dir, 20);
        let mut service = WorldService::open(&config).expect("opens");
        for tick in 0..20 {
            assert!(!service.on_tick(tick).expect("tick"), "tick {tick}");
        }
        assert!(service.on_tick(20).expect("tick"), "save due at tick 20");
        assert!(!service.on_tick(20).expect("tick"), "fires once");
        service.close().expect("closes");

        // A zero interval disables periodic saves entirely.
        let config = config_for(&dir, 0);
        let mut service = WorldService::open(&config).expect("opens");
        for tick in 0..10_000 {
            assert!(!service.on_tick(tick).expect("tick"));
        }
        service.close().expect("closes");
    }
}
