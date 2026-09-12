//! Operational metrics snapshot (P08-02).
//!
//! [`TickMetrics`] already accounts everything the phase gate needs — per-phase
//! cost, a bounded window of whole-tick times, overruns — but it lives in
//! `mc-simulation` and knows nothing about players, entities or chunks. This
//! module is the join: [`OperationalSnapshot`] reads the live [`Game`] once and
//! renders one structured log line, so an operator (and the P08-13 harness)
//! sees the same fields in the same order every 30 s.
//!
//! No new accounting is introduced here. Every field is a read of state the
//! server already keeps; the snapshot exists so the *shape* is tested in one
//! place rather than re-derived by each consumer.

use crate::game::{Game, METRICS_LOG_INTERVAL_TICKS};

/// One line of operational telemetry: tick health plus population.
///
/// Built with [`OperationalSnapshot::of`]; rendered with
/// [`OperationalSnapshot::log`]. The struct is `Copy`-cheap and holds no
/// references, so a test can build one, drop the game, and still assert on it.
///
/// Deliberately **no CPU/RSS fields**: both need a platform API (`getrusage`
/// on Linux, `GetProcessMemoryInfo` on Windows) behind a new dependency or a
/// `cfg`-split import, and the P08-13 profile run reports wall time, TPS and
/// MSPT — the quantities the 20 TPS gate actually checks — without them. RSS
/// on the Pi comes from `systemd-cgtop`/`journalctl` during that run, which the
/// runbook records. Adding a field here without a reader would be the exact
/// fabricated-measurement shape Phase 07 kept finding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperationalSnapshot {
    /// Ticks recorded since the scheduler started.
    pub ticks: u64,
    /// Whole-tick mean over the window, in milliseconds.
    pub mean_ms: f64,
    /// Whole-tick p50 over the window, in milliseconds.
    pub p50_ms: f64,
    /// Whole-tick p95 over the window, in milliseconds.
    pub p95_ms: f64,
    /// Whole-tick p99 over the window, in milliseconds.
    pub p99_ms: f64,
    /// Worst whole-tick time since construction, in milliseconds.
    pub worst_ms: f64,
    /// Ticks that exceeded the 50 ms budget.
    pub overruns: u64,
    /// Connected players.
    pub players: usize,
    /// Live entities in the store.
    pub entities: usize,
    /// Loaded chunks in the world.
    pub chunks: usize,
    /// Dirty chunks not yet on disk.
    pub dirty_chunks: usize,
    /// The interval this line covers, in ticks (30 s at 20 TPS).
    pub interval_ticks: u64,
}

impl OperationalSnapshot {
    /// Read the live game once.
    #[must_use]
    pub fn of(game: &Game) -> Self {
        let metrics = game.metrics();
        let as_ms = |duration: std::time::Duration| duration.as_secs_f64() * 1e3;
        Self {
            ticks: metrics.tick_count(),
            mean_ms: as_ms(metrics.mean()),
            p50_ms: as_ms(metrics.percentile(0.50)),
            p95_ms: as_ms(metrics.percentile(0.95)),
            p99_ms: as_ms(metrics.percentile(0.99)),
            worst_ms: as_ms(metrics.worst()),
            overruns: metrics.overruns(),
            players: game.player_count(),
            entities: game.entity_store().len(),
            chunks: game.world().chunk_count(),
            dirty_chunks: game.world().dirty_chunks().len(),
            interval_ticks: METRICS_LOG_INTERVAL_TICKS,
        }
    }

    /// Emit the one structured summary line the lifecycle logs every 30 s.
    pub fn log(self, tick: u64) {
        tracing::info!(
            tick,
            ticks = self.ticks,
            mean_ms = self.mean_ms,
            p50_ms = self.p50_ms,
            p95_ms = self.p95_ms,
            p99_ms = self.p99_ms,
            worst_ms = self.worst_ms,
            overruns = self.overruns,
            players = self.players,
            entities = self.entities,
            chunks = self.chunks,
            dirty_chunks = self.dirty_chunks,
            "tick metrics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::OperationalSnapshot;
    use crate::game::Game;
    use mc_network::bridge::game_channel;
    use mc_test_support::fixtures::TempDir;

    // The tag must differ per test: TempDir uniqueness is pid + nanos, and two
    // same-tag constructions in one process have observed the same nanos tick
    // under parallel I/O, so one test's level.dat rename raced the other's
    // drop-time remove_dir_all (the P08 flaky record; see CHANGELOG.md).
    fn game(tag: &str) -> (Game, TempDir) {
        let dir = TempDir::new(tag);
        let config = crate::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = crate::storage::WorldService::open(&config).expect("world opens");
        let (_, rx) = game_channel(64);
        let game = Game::new(&storage, 3, rx).expect("game builds");
        (game, dir)
    }

    #[test]
    // Zero is the exact rendering of an empty window, not an epsilon comparison.
    #[allow(clippy::float_cmp)]
    fn a_fresh_game_reports_zeroes_not_garbage() {
        let (game, _dir) = game("ops-metrics-fresh");
        let snapshot = OperationalSnapshot::of(&game);
        assert_eq!(snapshot.ticks, 0);
        assert_eq!(snapshot.overruns, 0);
        assert_eq!(snapshot.players, 0);
        assert_eq!(snapshot.entities, 0);
        assert_eq!(snapshot.chunks, 0);
        assert_eq!(snapshot.dirty_chunks, 0);
        assert_eq!(snapshot.mean_ms, 0.0);
        assert_eq!(snapshot.p95_ms, 0.0);
    }

    #[test]
    // Durations are exact `Duration` reads rendered to `f64` milliseconds; the
    // comparisons below are ordering checks on measured data, not epsilon work.
    #[allow(clippy::float_cmp)]
    fn after_ticks_the_snapshot_counts_what_the_scheduler_counted() {
        let (mut game, _dir) = game("ops-metrics-counted");
        for _ in 0..5 {
            game.tick().expect("tick");
        }
        let snapshot = OperationalSnapshot::of(&game);
        assert_eq!(snapshot.ticks, 5);
        assert_eq!(snapshot.ticks, game.metrics().tick_count());
        assert_eq!(snapshot.overruns, game.metrics().overruns());
        // Ordering must hold on real data, not just in the metrics unit tests.
        assert!(
            snapshot.p50_ms <= snapshot.p95_ms && snapshot.p95_ms <= snapshot.p99_ms,
            "{snapshot:?}"
        );
        assert!(snapshot.mean_ms >= 0.0);
        assert!(snapshot.worst_ms >= snapshot.p99_ms, "{snapshot:?}");
    }

    #[test]
    fn the_snapshot_does_not_move_the_game() {
        // Reading metrics must not tick, join or drop anything.
        let (mut game, _dir) = game("ops-metrics-readonly");
        game.tick().expect("tick");
        let before = game.metrics().tick_count();
        let _ = OperationalSnapshot::of(&game);
        let _ = OperationalSnapshot::of(&game);
        assert_eq!(game.metrics().tick_count(), before);
        assert_eq!(game.player_count(), 0);
    }
}
