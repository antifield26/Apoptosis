//! Deterministic clock and fixed-step tick types (P01-09, ADR-0001 D-02/D-06).
//!
//! The simulation advances in fixed 50 ms steps at 20 ticks per second.
//! Two clocks exist on purpose:
//!
//! - [`TickClock`] is the authoritative simulation clock: it counts ticks and
//!   decides when the next tick is due. It owns no wall-clock I/O itself; the
//!   caller supplies `now`, so tests can drive time deterministically.
//! - [`WallClock`] is a thin injectable wrapper around real time for the
//!   operational edges (metrics, shutdown deadlines). Production passes the
//!   real clock, tests pass a fake.
//!
//! Deadline math saturates instead of wrapping so a hostile or absurd `now`
//! can delay a tick but never panic the scheduler.

/// Ticks per second. Fixed by the product contract (AGENTS.md section 2).
pub const TICKS_PER_SECOND: u64 = 20;

/// Nanoseconds per tick at the nominal rate (50 ms).
pub const NANOS_PER_TICK: u64 = 1_000_000_000 / TICKS_PER_SECOND;

/// Monotonic tick counter. `u64` will not wrap in practice
/// (20 TPS needs ~29 billion years to overflow).
pub type Tick = u64;

/// Authoritative fixed-step simulation clock.
///
/// The tick thread calls [`TickClock::poll`] with the current time; it returns
/// how many ticks are due. Overruns are **clamped** to [`TickClock::MAX_CATCH_UP`]
/// (anti-death-spiral, cf. Pumpkin `ticker.rs:100-106` and Minestom
/// `TickSchedulerThread.java:36-45`): a stalled server skips ahead instead of
/// grinding through backlog.
#[derive(Debug, Clone)]
pub struct TickClock {
    next_tick_nanos: u64,
    tick: Tick,
    tick_nanos: u64,
    started: bool,
}

impl TickClock {
    /// Maximum ticks a single [`TickClock::poll`] may report.
    pub const MAX_CATCH_UP: u64 = 5;

    /// Create a clock running at the nominal 20 TPS.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tick_nanos(NANOS_PER_TICK)
    }

    /// Create a clock with an explicit step, for tests and the vanilla
    /// `/tick rate` future (Pumpkin `tick_rate_manager.rs` shape, P05).
    #[must_use]
    pub fn with_tick_nanos(tick_nanos: u64) -> Self {
        Self {
            next_tick_nanos: 0,
            tick: 0,
            tick_nanos: tick_nanos.max(1),
            started: false,
        }
    }

    /// Current tick count (ticks already released).
    #[must_use]
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Nanoseconds between ticks for this clock.
    #[must_use]
    pub fn tick_nanos(&self) -> u64 {
        self.tick_nanos
    }

    /// Change the step length (future `/tick rate`; frozen/sprint in P05).
    /// The next deadline is recomputed from `now` so the change takes effect
    /// on the next poll without a time jump.
    pub fn set_tick_nanos(&mut self, tick_nanos: u64, now_nanos: u64) {
        self.tick_nanos = tick_nanos.max(1);
        self.next_tick_nanos = now_nanos.saturating_add(self.tick_nanos);
    }

    /// Report how many ticks are due at `now_nanos`, advancing the deadline.
    ///
    /// The first call anchors the deadline to `now + step` and reports 0, so
    /// construction time never counts as missed ticks.
    #[must_use]
    pub fn poll(&mut self, now_nanos: u64) -> u64 {
        if !self.started {
            self.started = true;
            self.next_tick_nanos = now_nanos.saturating_add(self.tick_nanos);
            return 0;
        }
        if now_nanos < self.next_tick_nanos {
            return 0;
        }
        let elapsed = now_nanos - self.next_tick_nanos;
        let mut due = elapsed / self.tick_nanos + 1;
        if due > Self::MAX_CATCH_UP {
            // Clamp: skip backlog, resume from now (anti death-spiral).
            due = Self::MAX_CATCH_UP;
            self.next_tick_nanos = now_nanos.saturating_add(self.tick_nanos);
        } else {
            self.next_tick_nanos = self.next_tick_nanos.saturating_add(due * self.tick_nanos);
        }
        self.tick = self.tick.saturating_add(due);
        due
    }

    /// Nanoseconds until the next tick (0 when overdue). Used to sleep the
    /// tick thread without busy-waiting.
    #[must_use]
    pub fn nanos_until_next(&self, now_nanos: u64) -> u64 {
        if !self.started {
            return self.tick_nanos;
        }
        self.next_tick_nanos.saturating_sub(now_nanos)
    }
}

impl Default for TickClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Injectable wall-clock for operational edges (metrics, deadlines).
///
/// Production uses [`WallClock::real`]; tests use [`WallClock::manual`] and
/// advance it by hand, keeping simulation I/O-free per AGENTS.md section 3.5.
#[derive(Debug, Clone)]
pub struct WallClock {
    now_nanos: Option<u64>,
}

impl WallClock {
    /// Real time via [`std::time::Instant`] (monotonic, process-local origin).
    #[must_use]
    pub fn real() -> Self {
        Self { now_nanos: None }
    }

    /// Manual clock starting at `start_nanos` for deterministic tests.
    #[must_use]
    pub fn manual(start_nanos: u64) -> Self {
        Self {
            now_nanos: Some(start_nanos),
        }
    }

    /// Current time in nanoseconds (arbitrary monotonic origin).
    #[must_use]
    pub fn now_nanos(&self) -> u64 {
        match self.now_nanos {
            Some(t) => t,
            None => Self::monotonic_nanos(),
        }
    }

    /// Advance a manual clock. Panics in debug builds when called on a real
    /// clock — a programmer error, caught by tests, never by players.
    pub fn advance(&mut self, delta_nanos: u64) {
        match &mut self.now_nanos {
            Some(t) => *t = t.saturating_add(delta_nanos),
            None => debug_assert!(false, "cannot advance a real WallClock"),
        }
    }

    fn monotonic_nanos() -> u64 {
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(std::time::Instant::now);
        std::time::Instant::now()
            .saturating_duration_since(*origin)
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::{NANOS_PER_TICK, TICKS_PER_SECOND, TickClock, WallClock};

    #[test]
    fn nominal_rate_is_20tps() {
        assert_eq!(TICKS_PER_SECOND, 20);
        assert_eq!(NANOS_PER_TICK, 50_000_000);
    }

    #[test]
    fn first_poll_anchors_without_releasing_ticks() {
        let mut clock = TickClock::new();
        assert_eq!(clock.poll(1_000), 0);
        assert_eq!(clock.tick(), 0);
        assert_eq!(clock.poll(1_000), 0);
    }

    #[test]
    fn releases_one_tick_per_step() {
        let mut clock = TickClock::new();
        let _ = clock.poll(0);
        assert_eq!(clock.poll(NANOS_PER_TICK), 1);
        assert_eq!(clock.poll(2 * NANOS_PER_TICK), 1);
        assert_eq!(clock.tick(), 2);
    }

    #[test]
    fn overrun_is_clamped_not_replayed() {
        let mut clock = TickClock::new();
        let _ = clock.poll(0);
        // 1000 missed ticks collapse to MAX_CATCH_UP; the server skips ahead.
        assert_eq!(clock.poll(1_000 * NANOS_PER_TICK), TickClock::MAX_CATCH_UP);
        assert_eq!(clock.tick(), TickClock::MAX_CATCH_UP);
        // And the deadline resumed from now instead of backlog-chasing.
        assert_eq!(clock.poll(1_000 * NANOS_PER_TICK), 0);
    }

    #[test]
    fn same_inputs_same_tick_count_is_deterministic() {
        // AGENTS.md section 3.6: identical time series must give identical ticks.
        fn run(times: &[u64]) -> u64 {
            let mut clock = TickClock::new();
            for t in times {
                let _ = clock.poll(*t);
            }
            clock.tick()
        }
        let series: Vec<u64> = (0..500)
            .map(|i| i * NANOS_PER_TICK + (i % 7) * 1_000_000)
            .collect();
        assert_eq!(run(&series), run(&series));
    }

    #[test]
    fn manual_wall_clock_advances_deterministically() {
        let mut clock = WallClock::manual(0);
        clock.advance(NANOS_PER_TICK);
        clock.advance(NANOS_PER_TICK);
        assert_eq!(clock.now_nanos(), 2 * NANOS_PER_TICK);
    }

    #[test]
    fn sleep_duration_counts_down_to_tick() {
        let mut clock = TickClock::new();
        let _ = clock.poll(0);
        assert_eq!(clock.nanos_until_next(0), NANOS_PER_TICK);
        assert_eq!(clock.nanos_until_next(NANOS_PER_TICK), 0);
    }
}
