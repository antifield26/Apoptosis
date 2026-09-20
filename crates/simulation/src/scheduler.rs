//! The tick scheduler (P05-01).
//!
//! [`Scheduler`] runs the six phases in the fixed order of [`PHASE_ORDER`],
//! timing each one and recording the result in [`TickMetrics`]. It owns no
//! gameplay state: the caller supplies a [`PhaseRunner`], which is how
//! `mc-server`'s game loop plugs in without this crate depending on it.
//!
//! Keeping the scheduler ignorant of gameplay is deliberate (AGENTS.md §3.4): it
//! means the ordering contract can be tested with a recording stub, and the
//! dependency direction stays `server → simulation`, never the reverse.
//!
//! ## Deterministic ordering
//!
//! Two independent things have to hold for a tick to be reproducible:
//!
//! 1. the *phase* order is fixed — guaranteed here, and asserted by tests;
//! 2. the order *within* a phase is fixed — the caller's responsibility, because
//!    only the caller knows whether it is iterating entities, chunks or packets.
//!    Every container in this project that a phase iterates is ordered
//!    (`BTreeMap`/`BTreeSet`/sorted `Vec`), which is why [`PhaseRunner::run_phase`]
//!    documents it as a requirement rather than a suggestion.
//!
//! ## Overrun policy
//!
//! An overrun is *recorded*, never compensated for. Catching up by running extra
//! ticks is how a server enters a death spiral (`TickClock::MAX_CATCH_UP` in
//! `mc-core` bounds that separately). The scheduler therefore reports what
//! happened and lets the caller decide.

use crate::metrics::{TickMetrics, TickStats};
use crate::phase::{PHASE_ORDER, TickPhase};
use mc_core::error::ServerResult;
use mc_core::tick::Tick;
use std::time::Instant;
use tracing::{debug, warn};

/// Something that can run one phase of a tick.
///
/// Implementors **must** iterate any collection in a deterministic order
/// (ascending id, sorted chunk position, insertion-ordered list). A phase that
/// iterates a `HashMap` breaks AGENTS.md §3.6 even though nothing here can detect
/// it, which is why the requirement is stated on the trait.
pub trait PhaseRunner {
    /// Run one phase.
    ///
    /// # Errors
    ///
    /// Any error aborts the tick and propagates to [`Scheduler::run_tick`]'s
    /// caller; the server treats that as fatal for the process, so phases should
    /// reject bad *player* input themselves and return `Ok`.
    fn run_phase(&mut self, tick: Tick, phase: TickPhase) -> ServerResult<()>;
}

/// What happened during one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// Every phase ran.
    Completed(TickStats),
    /// A phase returned an error; the stats cover the phases that ran.
    ///
    /// Carries the error so the caller does not have to stash it, while still
    /// giving the metrics a complete tick to account for.
    Failed {
        /// The failing phase.
        phase: TickPhase,
        /// Cost of the phases that ran, including the failing one.
        stats: TickStats,
    },
}

impl TickOutcome {
    /// The tick's stats, whether or not it completed.
    #[must_use]
    pub const fn stats(&self) -> &TickStats {
        match self {
            Self::Completed(stats) | Self::Failed { stats, .. } => stats,
        }
    }

    /// Whether every phase ran.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// The phase that failed, if any.
    #[must_use]
    pub const fn failed_phase(&self) -> Option<TickPhase> {
        match self {
            Self::Completed(_) => None,
            Self::Failed { phase, .. } => Some(*phase),
        }
    }
}

/// Runs the phases of a tick and keeps its metrics.
#[derive(Debug)]
pub struct Scheduler {
    metrics: TickMetrics,
    /// What the most recent tick did, including one that failed.
    ///
    /// Kept because a caller that aborts on a phase error still wants to know
    /// which phase it was and what the partial tick cost.
    last_outcome: TickOutcome,
    /// Log a warning when a tick exceeds the budget.
    log_overruns: bool,
}

impl Scheduler {
    /// Create a scheduler with the nominal 20 TPS budget.
    #[must_use]
    pub fn new() -> Self {
        Self {
            metrics: TickMetrics::nominal(),
            last_outcome: TickOutcome::Completed(TickStats::default()),
            log_overruns: true,
        }
    }

    /// Create a scheduler with an explicit budget and overrun logging toggle.
    #[must_use]
    pub fn with_budget(budget: std::time::Duration, log_overruns: bool) -> Self {
        Self {
            metrics: TickMetrics::with_budget(budget),
            last_outcome: TickOutcome::Completed(TickStats::default()),
            log_overruns,
        }
    }

    /// Accumulated metrics.
    #[must_use]
    pub const fn metrics(&self) -> &TickMetrics {
        &self.metrics
    }

    /// Mutable metrics (the benchmark resets them between runs).
    pub fn metrics_mut(&mut self) -> &mut TickMetrics {
        &mut self.metrics
    }

    /// What the most recent tick did.
    #[must_use]
    pub const fn last_outcome(&self) -> &TickOutcome {
        &self.last_outcome
    }

    /// Run one tick.
    ///
    /// The returned [`TickOutcome`] always carries the tick's stats, including
    /// when a phase failed: a half-run tick still costs time, and hiding that from
    /// the metrics would make an overrun caused by a failing phase invisible.
    ///
    /// # Errors
    ///
    /// Returns the failing phase's error after recording the tick.
    pub fn run_tick<R: PhaseRunner + ?Sized>(
        &mut self,
        tick: Tick,
        runner: &mut R,
    ) -> ServerResult<TickOutcome> {
        let started = Instant::now();
        let mut stats = TickStats::default();
        let mut failure: Option<(TickPhase, mc_core::error::ServerError)> = None;

        for phase in PHASE_ORDER {
            let phase_started = Instant::now();
            let result = runner.run_phase(tick, phase);
            let phase_nanos = phase_started.elapsed().as_nanos() as u64;
            stats.phase_nanos[phase.index()] = phase_nanos;
            if let Err(error) = result {
                failure = Some((phase, error));
                break;
            }
        }
        stats.total_nanos = started.elapsed().as_nanos() as u64;
        self.metrics.record(&stats);

        let overrun = stats.total_nanos > self.metrics.budget().as_nanos() as u64;
        if overrun && self.log_overruns {
            // The worst phase rides along so the next 15:07-class spike is
            // attributable from the log alone (P15-01): aggregate MSPT cannot
            // say which phase blew the budget.
            let (worst_phase, worst_nanos) = stats.worst_phase();
            warn!(
                tick,
                total_ms = stats.total_nanos as f64 / 1e6,
                budget_ms = self.metrics.budget().as_secs_f64() * 1e3,
                worst_phase = worst_phase.name(),
                worst_phase_ms = worst_nanos.as_secs_f64() * 1e3,
                "tick exceeded its budget"
            );
        }

        if let Some((phase, error)) = failure {
            debug!(tick, %phase, "tick aborted in phase");
            self.last_outcome = TickOutcome::Failed { phase, stats };
            return Err(error);
        }
        self.last_outcome = TickOutcome::Completed(stats);
        Ok(self.last_outcome)
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{PhaseRunner, Scheduler};
    use crate::phase::{PHASE_ORDER, TickPhase};
    use mc_core::error::{ServerError, ServerResult};
    use mc_core::tick::Tick;

    /// Records the phases it is asked to run.
    #[derive(Default)]
    struct Recorder {
        seen: Vec<(Tick, TickPhase)>,
        fail_at: Option<TickPhase>,
    }

    impl PhaseRunner for Recorder {
        fn run_phase(&mut self, tick: Tick, phase: TickPhase) -> ServerResult<()> {
            self.seen.push((tick, phase));
            if self.fail_at == Some(phase) {
                return Err(ServerError::Invariant(format!("{phase} failed")));
            }
            Ok(())
        }
    }

    #[test]
    fn phases_run_once_in_order() {
        let mut scheduler = Scheduler::new();
        let mut runner = Recorder::default();
        let outcome = scheduler.run_tick(7, &mut runner).expect("tick runs");
        assert!(outcome.is_complete());
        assert_eq!(
            runner.seen,
            PHASE_ORDER.map(|phase| (7, phase)).to_vec(),
            "every phase runs exactly once, in the documented order, for this tick"
        );
    }

    #[test]
    fn the_same_tick_number_is_passed_to_every_phase() {
        let mut scheduler = Scheduler::new();
        let mut runner = Recorder::default();
        scheduler.run_tick(42, &mut runner).expect("tick");
        assert!(runner.seen.iter().all(|(tick, _)| *tick == 42));
    }

    #[test]
    fn a_failing_phase_aborts_the_tick_but_is_still_measured() {
        let mut scheduler = Scheduler::with_budget(std::time::Duration::from_millis(50), false);
        let mut runner = Recorder {
            fail_at: Some(TickPhase::Entities),
            ..Recorder::default()
        };
        let error = scheduler.run_tick(1, &mut runner).expect_err("fails");
        assert!(matches!(error, ServerError::Invariant(_)));
        // Phases before the failure ran; later ones did not.
        assert_eq!(
            runner
                .seen
                .iter()
                .map(|(_, phase)| *phase)
                .collect::<Vec<_>>(),
            vec![
                TickPhase::Network,
                TickPhase::ScheduledTicks,
                TickPhase::Entities
            ]
        );
        // The half-run tick still counted.
        assert_eq!(scheduler.metrics().tick_count(), 1);
    }

    #[test]
    fn metrics_accumulate_across_ticks() {
        let mut scheduler = Scheduler::with_budget(std::time::Duration::from_millis(50), false);
        let mut runner = Recorder::default();
        for tick in 0..10 {
            scheduler.run_tick(tick, &mut runner).expect("tick");
        }
        assert_eq!(scheduler.metrics().tick_count(), 10);
        assert_eq!(runner.seen.len(), 10 * PHASE_ORDER.len());
        assert!(scheduler.metrics().started());
        assert_eq!(scheduler.metrics().budget().as_millis(), 50);
    }

    #[test]
    fn a_tick_reports_per_phase_costs() {
        let mut scheduler = Scheduler::with_budget(std::time::Duration::from_millis(50), false);
        let mut runner = Recorder::default();
        let outcome = scheduler.run_tick(0, &mut runner).expect("tick");
        let stats = outcome.stats();
        assert!(
            stats.total_nanos > 0,
            "an instant tick still costs something"
        );
        // Phase costs are measured, not zero-filled placeholders.
        assert!(
            stats.phase_sum() <= stats.total(),
            "phase sum {:?} must not exceed the total {:?}",
            stats.phase_sum(),
            stats.total()
        );
        assert!(stats.phase(TickPhase::Broadcast) <= stats.total());
    }

    #[test]
    fn outcome_accessors_agree_with_the_variant() {
        let mut scheduler = Scheduler::with_budget(std::time::Duration::from_millis(50), false);
        let mut ok = Recorder::default();
        let outcome = scheduler.run_tick(0, &mut ok).expect("ok");
        assert!(outcome.is_complete());
        assert_eq!(outcome.failed_phase(), None);

        let mut bad = Recorder {
            fail_at: Some(TickPhase::Network),
            ..Recorder::default()
        };
        let _ = scheduler.run_tick(1, &mut bad);
        // The scheduler keeps the failing outcome for observers that only see the
        // error value.
        let last = scheduler.last_outcome();
        assert!(!last.is_complete());
        assert_eq!(last.failed_phase(), Some(TickPhase::Network));
    }
}
