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
    /// window.
    pub fn open(config: &StorageConfig) -> ServerResult<Self> {
        let root = config.world_dir.as_path();
        let mut storage = WorldStorage::open(root, config.autosave_ticks)?;
        if storage.level().is_none() {
            let name = root
                .file_name()
                .map_or_else(|| "world".to_owned(), |n| n.to_string_lossy().into_owned());
            let level = mc_persistence::level::LevelDat::new(
                &name,
                mc_persistence::save::unix_millis(std::time::SystemTime::now()),
            );
            storage.save_level(level)?;
            tracing::info!(world = %root.display(), "created a new world");
        }
        tracing::info!(
            world = %root.display(),
            level_name = storage.level_name(),
            autosave_ticks = config.autosave_ticks,
            "world storage ready"
        );
        Ok(Self { storage })
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
    use super::WorldService;
    use crate::config::StorageConfig;
    use mc_persistence::level::DATA_VERSION_26_1_2;
    use mc_test_support::fixtures::TempDir;

    fn config_for(dir: &TempDir, autosave_ticks: u64) -> StorageConfig {
        StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks,
        }
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
