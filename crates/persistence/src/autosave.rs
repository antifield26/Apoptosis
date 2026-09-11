//! Autosave scheduling (P03-12).
//!
//! The scheduler is a pure function of the tick counter: given the same tick
//! sequence it always fires on the same ticks, so a save cannot perturb
//! simulation determinism beyond the save itself (AGENTS.md section 3.6).
//!
//! Semantics:
//!
//! - `interval_ticks == 0` disables the timer (explicit saves only, matching
//!   the `storage.autosave_ticks = 0` config option);
//! - the first save is due one interval after the clock starts;
//! - after a due save the deadline moves to `tick + interval`, so a stalled or
//!   overrun tick loop produces **one** save, never a burst of catch-up saves
//!   (same anti-death-spiral reasoning as [`mc_core::tick::TickClock`]);
//! - the scheduler never performs I/O; it only answers "is a save due".

use mc_core::tick::Tick;

/// Default interval: 6000 ticks = 5 minutes at 20 TPS, Vanilla's autosave
/// cadence.
pub const DEFAULT_INTERVAL_TICKS: u64 = 6000;

/// Decides when the next periodic save happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutosaveScheduler {
    interval_ticks: u64,
    next_due: Option<Tick>,
    saves: u64,
}

impl AutosaveScheduler {
    /// Create a scheduler; `interval_ticks == 0` disables periodic saving.
    #[must_use]
    pub fn new(interval_ticks: u64) -> Self {
        Self {
            interval_ticks,
            next_due: None,
            saves: 0,
        }
    }

    /// The configured interval in ticks (0 when disabled).
    #[must_use]
    pub const fn interval_ticks(&self) -> u64 {
        self.interval_ticks
    }

    /// Whether periodic saving is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.interval_ticks > 0
    }

    /// Number of saves this scheduler has fired.
    #[must_use]
    pub const fn saves(&self) -> u64 {
        self.saves
    }

    /// The tick at which the next save becomes due.
    #[must_use]
    pub const fn next_due_tick(&self) -> Option<Tick> {
        self.next_due
    }

    /// Advance to `tick` and report whether a save is due now.
    ///
    /// Calling this twice for the same tick reports `true` at most once.
    pub fn on_tick(&mut self, tick: Tick) -> bool {
        if !self.is_enabled() {
            return false;
        }
        match self.next_due {
            None => {
                self.next_due = Some(tick.saturating_add(self.interval_ticks));
                false
            }
            Some(due) if tick >= due => {
                // Re-arm from *now*, not from the missed deadline.
                self.next_due = Some(tick.saturating_add(self.interval_ticks));
                self.saves = self.saves.saturating_add(1);
                true
            }
            Some(_) => false,
        }
    }

    /// Force the next save to be due `interval_ticks` after `tick`.
    pub fn defer_from(&mut self, tick: Tick) {
        if self.is_enabled() {
            self.next_due = Some(tick.saturating_add(self.interval_ticks));
        }
    }

    /// Change the interval at runtime (config reload); re-arms from `tick`.
    pub fn set_interval(&mut self, interval_ticks: u64, tick: Tick) {
        self.interval_ticks = interval_ticks;
        self.next_due = if interval_ticks == 0 {
            None
        } else {
            Some(tick.saturating_add(interval_ticks))
        };
    }
}

impl Default for AutosaveScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL_TICKS)
    }
}

#[cfg(test)]
mod tests {
    use super::{AutosaveScheduler, DEFAULT_INTERVAL_TICKS};

    #[test]
    fn default_interval_is_vanilla_five_minutes() {
        assert_eq!(DEFAULT_INTERVAL_TICKS, 6000);
        let scheduler = AutosaveScheduler::default();
        assert!(scheduler.is_enabled());
        assert_eq!(scheduler.interval_ticks(), 6000);
    }

    #[test]
    fn zero_interval_disables_saving() {
        let mut scheduler = AutosaveScheduler::new(0);
        assert!(!scheduler.is_enabled());
        for tick in 0..100_000 {
            assert!(
                !scheduler.on_tick(tick),
                "disabled scheduler must never fire"
            );
        }
        assert_eq!(scheduler.saves(), 0);
        assert_eq!(scheduler.next_due_tick(), None);
    }

    #[test]
    fn fires_once_per_interval() {
        let mut scheduler = AutosaveScheduler::new(100);
        assert!(!scheduler.on_tick(0), "arming tick is not a save");
        assert_eq!(scheduler.next_due_tick(), Some(100));
        for tick in 1..100 {
            assert!(!scheduler.on_tick(tick), "tick {tick} is early");
        }
        assert!(scheduler.on_tick(100));
        assert!(!scheduler.on_tick(100), "the same tick fires once");
        assert_eq!(scheduler.next_due_tick(), Some(200));
        assert!(scheduler.on_tick(200));
        assert_eq!(scheduler.saves(), 2);
    }

    #[test]
    fn overrun_produces_one_save_not_a_burst() {
        let mut scheduler = AutosaveScheduler::new(100);
        let _ = scheduler.on_tick(0);
        // The tick loop stalled for 10 intervals.
        assert!(scheduler.on_tick(1000));
        assert!(!scheduler.on_tick(1000));
        assert_eq!(scheduler.saves(), 1, "catch-up saves must not burst");
        assert_eq!(scheduler.next_due_tick(), Some(1100));
    }

    #[test]
    fn sequence_is_deterministic() {
        fn run(ticks: &[u64]) -> Vec<u64> {
            let mut scheduler = AutosaveScheduler::new(64);
            let mut fired = Vec::new();
            for tick in ticks {
                if scheduler.on_tick(*tick) {
                    fired.push(*tick);
                }
            }
            fired
        }
        let ticks: Vec<u64> = (0..500).collect();
        let first = run(&ticks);
        assert_eq!(first, run(&ticks));
        assert_eq!(
            first,
            (64..500).step_by(64).collect::<Vec<u64>>(),
            "fires on exact multiples of the interval"
        );
    }

    #[test]
    fn interval_can_be_changed_and_deferred() {
        let mut scheduler = AutosaveScheduler::new(100);
        let _ = scheduler.on_tick(0);
        assert_eq!(scheduler.next_due_tick(), Some(100));
        scheduler.defer_from(50);
        assert_eq!(scheduler.next_due_tick(), Some(150));
        scheduler.set_interval(10, 150);
        assert_eq!(scheduler.next_due_tick(), Some(160));
        assert!(scheduler.on_tick(160));
        scheduler.set_interval(0, 200);
        assert!(!scheduler.is_enabled());
        assert_eq!(scheduler.next_due_tick(), None);
    }
}
