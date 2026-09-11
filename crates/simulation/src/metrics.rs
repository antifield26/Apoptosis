//! Tick timing and rate accounting (P05-01, feeds P08-02/P08-13).
//!
//! Two consumers: the server's own overrun logging, and the P08 benchmark
//! harness. Both need the same thing — per-phase cost and a distribution of whole
//! tick times — so the accounting lives here rather than in the benchmark.
//!
//! The window keeps a fixed number of recent tick durations, so a percentile is
//! computed over a bounded ring instead of an unbounded history. A window of 600
//! ticks is 30 seconds at 20 TPS: long enough to be stable, short enough to react.

use crate::phase::{PHASE_COUNT, PHASE_ORDER, TickPhase};
use std::time::Duration;

/// Recent tick durations kept for percentile estimates (30 s at 20 TPS).
pub const WINDOW: usize = 600;

/// One tick's measured cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickStats {
    /// Whole-tick duration.
    pub total_nanos: u64,
    /// Per-phase duration, indexed by [`TickPhase::index`].
    pub phase_nanos: [u64; PHASE_COUNT],
}

impl TickStats {
    /// Total in nanoseconds.
    #[must_use]
    pub const fn total(&self) -> Duration {
        Duration::from_nanos(self.total_nanos)
    }

    /// One phase's cost.
    #[must_use]
    pub fn phase(&self, phase: TickPhase) -> Duration {
        Duration::from_nanos(self.phase_nanos[phase.index()])
    }

    /// Sum of the per-phase costs.
    ///
    /// Slightly less than [`TickStats::total`] when the scheduler itself costs
    /// something, which is worth seeing rather than hiding.
    #[must_use]
    pub fn phase_sum(&self) -> Duration {
        Duration::from_nanos(self.phase_nanos.iter().sum())
    }
}

/// Rolling tick statistics.
#[derive(Debug, Clone)]
pub struct TickMetrics {
    /// Nominal budget for one tick (50 ms at 20 TPS).
    budget: Duration,
    window: [u64; WINDOW],
    window_len: usize,
    next_slot: usize,
    tick_count: u64,
    /// Ticks whose total exceeded the budget.
    overruns: u64,
    /// Worst whole-tick time since construction.
    worst: u64,
    /// Cumulative per-phase nanoseconds since construction.
    phase_total: [u64; PHASE_COUNT],
    started: bool,
}

impl TickMetrics {
    /// Create metrics with an explicit per-tick budget.
    #[must_use]
    pub const fn with_budget(budget: Duration) -> Self {
        Self {
            budget,
            window: [0; WINDOW],
            window_len: 0,
            next_slot: 0,
            tick_count: 0,
            overruns: 0,
            worst: 0,
            phase_total: [0; PHASE_COUNT],
            started: false,
        }
    }

    /// Create metrics for the nominal 20 TPS budget (50 ms).
    #[must_use]
    pub const fn nominal() -> Self {
        Self::with_budget(Duration::from_nanos(mc_core::tick::NANOS_PER_TICK))
    }

    /// Record one tick.
    pub fn record(&mut self, stats: &TickStats) {
        self.started = true;
        self.tick_count = self.tick_count.saturating_add(1);
        self.window[self.next_slot] = stats.total_nanos;
        self.next_slot = (self.next_slot + 1) % WINDOW;
        if self.window_len < WINDOW {
            self.window_len += 1;
        }
        if stats.total_nanos > self.worst {
            self.worst = stats.total_nanos;
        }
        if Duration::from_nanos(stats.total_nanos) > self.budget {
            self.overruns = self.overruns.saturating_add(1);
        }
        for (index, nanos) in stats.phase_nanos.iter().enumerate() {
            self.phase_total[index] = self.phase_total[index].saturating_add(*nanos);
        }
    }

    /// Ticks recorded since construction.
    #[must_use]
    pub const fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// Ticks that exceeded the budget.
    #[must_use]
    pub const fn overruns(&self) -> u64 {
        self.overruns
    }

    /// Whether any tick has been recorded.
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }

    /// The per-tick budget.
    #[must_use]
    pub const fn budget(&self) -> Duration {
        self.budget
    }

    /// Worst whole-tick time.
    #[must_use]
    pub const fn worst(&self) -> Duration {
        Duration::from_nanos(self.worst)
    }

    /// Ticks currently in the window.
    #[must_use]
    pub const fn window_len(&self) -> usize {
        self.window_len
    }

    /// Percentile of the recent window; `fraction` in `0.0..=1.0`.
    ///
    /// Returns zero when nothing has been recorded.
    #[must_use]
    pub fn percentile(&self, fraction: f64) -> Duration {
        if self.window_len == 0 {
            return Duration::ZERO;
        }
        let mut sorted: Vec<u64> = self.window[..self.window_len].to_vec();
        sorted.sort_unstable();
        let clamped = fraction.clamp(0.0, 1.0);
        // The result is clamped into `0..len`, so the narrowing cannot go wrong; the
        // window length is at most `WINDOW`.
        let index = ((sorted.len() - 1) as f64 * clamped).round().max(0.0) as usize;
        Duration::from_nanos(sorted[index.min(sorted.len() - 1)])
    }

    /// Mean whole-tick time over the window.
    #[must_use]
    pub fn mean(&self) -> Duration {
        if self.window_len == 0 {
            return Duration::ZERO;
        }
        let sum: u64 = self.window[..self.window_len].iter().sum();
        Duration::from_nanos(sum / self.window_len as u64)
    }

    /// Mean cost of one phase since construction.
    #[must_use]
    pub fn phase_mean(&self, phase: TickPhase) -> Duration {
        if self.tick_count == 0 {
            return Duration::ZERO;
        }
        Duration::from_nanos(self.phase_total[phase.index()] / self.tick_count)
    }

    /// The phase that has cost the most in total since construction.
    #[must_use]
    pub fn busiest_phase(&self) -> Option<(TickPhase, Duration)> {
        if self.tick_count == 0 {
            return None;
        }
        PHASE_ORDER
            .iter()
            .copied()
            .max_by_key(|phase| self.phase_total[phase.index()])
            .map(|phase| (phase, self.phase_mean(phase)))
    }

    /// Effective ticks per second over `elapsed` wall time.
    ///
    /// `None` when nothing has been recorded or `elapsed` is zero; this is a
    /// *derived* rate, not a measured one, so it is reported as an estimate.
    #[must_use]
    pub fn tps_estimate(&self, elapsed: Duration) -> Option<f64> {
        let seconds = elapsed.as_secs_f64();
        if self.tick_count == 0 || seconds <= 0.0 {
            return None;
        }
        Some(self.tick_count as f64 / seconds)
    }

    /// Reset the window and counters (keeps the budget).
    pub fn reset(&mut self) {
        *self = Self::with_budget(self.budget);
    }
}

impl Default for TickMetrics {
    fn default() -> Self {
        Self::nominal()
    }
}

#[cfg(test)]
mod tests {
    use super::{TickMetrics, TickStats, WINDOW};
    use crate::phase::TickPhase;
    use std::time::Duration;

    fn stats(total_ms: u64, network_us: u64) -> TickStats {
        let mut stats = TickStats {
            total_nanos: total_ms * 1_000_000,
            ..TickStats::default()
        };
        stats.phase_nanos[TickPhase::Network.index()] = network_us * 1_000;
        stats
    }

    #[test]
    fn counters_accumulate() {
        let mut metrics = TickMetrics::nominal();
        assert_eq!(metrics.tick_count(), 0);
        assert!(!metrics.started());
        assert_eq!(metrics.percentile(0.5), Duration::ZERO);
        assert_eq!(metrics.busiest_phase(), None);

        for _ in 0..3 {
            metrics.record(&stats(10, 1));
        }
        assert_eq!(metrics.tick_count(), 3);
        assert!(metrics.started());
        assert_eq!(metrics.overruns(), 0);
        assert_eq!(metrics.worst(), Duration::from_millis(10));
        assert_eq!(
            metrics.phase_mean(TickPhase::Network),
            Duration::from_micros(1)
        );
        assert_eq!(
            metrics.busiest_phase(),
            Some((TickPhase::Network, Duration::from_micros(1)))
        );
    }

    #[test]
    fn overruns_are_counted_against_the_budget() {
        let mut metrics = TickMetrics::with_budget(Duration::from_millis(50));
        metrics.record(&stats(49, 0));
        assert_eq!(metrics.overruns(), 0);
        metrics.record(&stats(50, 0)); // exactly the budget is not an overrun
        assert_eq!(metrics.overruns(), 0);
        metrics.record(&stats(51, 0));
        assert_eq!(metrics.overruns(), 1);
        assert_eq!(metrics.worst(), Duration::from_millis(51));
    }

    #[test]
    fn percentiles_come_from_a_bounded_window() {
        let mut metrics = TickMetrics::nominal();
        for tick in 1..=100u64 {
            metrics.record(&stats(tick, 0));
        }
        assert_eq!(metrics.window_len(), 100);
        let p50 = metrics.percentile(0.50);
        let p95 = metrics.percentile(0.95);
        let max = metrics.percentile(1.0);
        assert!(p50 <= p95 && p95 <= max, "{p50:?} {p95:?} {max:?}");
        assert!(p50 >= Duration::from_millis(49) && p50 <= Duration::from_millis(51));
        assert!(p95 >= Duration::from_millis(94));
        assert_eq!(max, Duration::from_millis(100));
        assert_eq!(metrics.mean(), Duration::from_micros(50_500));
    }

    #[test]
    fn the_window_does_not_grow_without_bound() {
        let mut metrics = TickMetrics::nominal();
        for _ in 0..(WINDOW * 3) {
            metrics.record(&stats(1, 0));
        }
        assert_eq!(metrics.window_len(), WINDOW, "the ring must stay bounded");
        assert_eq!(metrics.tick_count(), (WINDOW * 3) as u64);
    }

    #[test]
    fn tps_estimate_needs_elapsed_time() {
        let mut metrics = TickMetrics::nominal();
        assert_eq!(metrics.tps_estimate(Duration::from_secs(1)), None);
        for _ in 0..20 {
            metrics.record(&stats(1, 0));
        }
        let estimate = metrics.tps_estimate(Duration::from_secs(1)).expect("some");
        assert!((estimate - 20.0).abs() < 1e-9, "{estimate}");
        assert_eq!(metrics.tps_estimate(Duration::ZERO), None);
    }

    #[test]
    fn phase_sum_is_at_most_the_total() {
        let mut stats = TickStats {
            total_nanos: 5_000_000,
            ..TickStats::default()
        };
        stats.phase_nanos[TickPhase::Entities.index()] = 1_000_000;
        stats.phase_nanos[TickPhase::Players.index()] = 2_000_000;
        assert_eq!(stats.phase_sum(), Duration::from_millis(3));
        assert_eq!(stats.total(), Duration::from_millis(5));
        assert_eq!(stats.phase(TickPhase::Entities), Duration::from_millis(1));
    }

    #[test]
    fn reset_keeps_the_budget() {
        let mut metrics = TickMetrics::with_budget(Duration::from_millis(25));
        metrics.record(&stats(30, 0));
        assert_eq!(metrics.overruns(), 1);
        metrics.reset();
        assert_eq!(metrics.tick_count(), 0);
        assert_eq!(metrics.overruns(), 0);
        assert_eq!(metrics.budget(), Duration::from_millis(25));
    }
}
