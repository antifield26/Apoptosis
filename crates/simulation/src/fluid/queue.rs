//! The fluid scheduled-tick queue (P20-01).
//!
//! ## Why fluids get their own queue
//!
//! [ADR-0009 §1](../../../../docs/adr/ADR-0009-world-ticking.md) verified with
//! `javap` that the 26.1.2 `ServerLevel` owns **two independent objects**,
//! `LevelTicks<Block> blockTicks` and `LevelTicks<Fluid> fluidTicks`. They are
//! not one queue with two kinds of entry, and the tick calls them in order
//! (`blockTicks.tick` at offset 110, `fluidTicks.tick` at 120). This module is
//! the second of those two, and it lives in `mc-simulation` rather than beside
//! `mc_redstone::UpdateQueue` because the *algorithm* it feeds
//! ([`crate::fluid::flow`]) lives here too: a queue and the consumer it exists
//! for belong in one crate, and `mc-simulation` is the crate that owns a tick's
//! phases. `mc-redstone`'s queue is the **precedent for the discipline**
//! (caps, dedup, a budget, a total order), which is why the two types read alike.
//!
//! ## The discipline, and where each part comes from
//!
//! | property | rule | source |
//! |---|---|---|
//! | order | ascending `(due tick, position)` | ours; the jar walks chunk containers in their own tick order, which a global `(due, pos)` order approximates deterministically (AGENTS.md §3.6) |
//! | dedup | one entry per `(due, position)` | mirrors `UpdateQueue::schedule_at`; the jar's `LevelChunkTicks` holds one entry per position |
//! | per-tick cap | [`MAX_FLUID_TICKS_PER_TICK`] = 4096 | **product decision**, ADR-0009 §2.3 |
//! | spill | work over the cap **stays queued** in queue order | `LevelTicks.tick(long, int, BiConsumer)` collects at most `maxTicks` (`canScheduleMoreTicks(i)` is `toRunThisTick.size() < i`) and then `rescheduleLeftoverContainers()` puts the rest back — spill, never drop |
//! | lateness counter | [`FluidQueue::pending`] | ADR-0009 §5: `fluid_ticks_pending`, in the style of `scheduled_ticks_pending`; the jar's `LevelTicks.count()` is the same number |
//! | queue cap | [`MAX_FLUID_QUEUE_ENTRIES`] = 16 384 (four ticks of work) | **product decision**: unbounded plus client-triggerable is an allocation-amplification vector (AGENTS.md §10), and it is sized so the *spill* path is the one a burst meets, not the refusal |
//!
//! The one place this queue **refuses** rather than spills is a full queue,
//! exactly like the redstone queue's [`Inserted::Refused`]; it is counted in
//! [`FluidQueue::refused_total`] so the loss is observable. Everything the cap
//! (rather than the queue size) stops is spilled, which is the property
//! ADR-0009 §2.3 asks for: "a queue that overflows must lose nothing, only
//! lateness".
//!
//! [`Inserted::Refused`]: https://docs.rs/mc-redstone

use mc_core::tick::Tick;
use std::collections::{BTreeMap, BTreeSet};

use super::world::Pos;

/// Due fluid updates one tick may run (ADR-0009 §2.3).
///
/// **product decision** — the ADR's number, chosen against the P20-07 workload
/// estimate (4096 updates at ≈120 ns each ≈ 0.5 ms/tick). Work beyond it is
/// spilled to the next tick, never dropped.
pub const MAX_FLUID_TICKS_PER_TICK: usize = 4_096;

/// Largest number of pending fluid entries.
///
/// **product decision** — not a Vanilla limit; the jar's `LevelTicks` is
/// unbounded. Four ticks' worth of the per-tick cap
/// ([`MAX_FLUID_TICKS_PER_TICK`]), because the two caps do different jobs: the
/// per-tick cap bounds *work*, and work over it **spills**; this cap bounds
/// *memory*, and an insert over it is **refused** (AGENTS.md §10). Sizing it at
/// the per-tick cap would make the spill path unreachable — a burst would be
/// refused before it could ever be late — which is why it is larger.
pub const MAX_FLUID_QUEUE_ENTRIES: usize = 4 * MAX_FLUID_TICKS_PER_TICK;

/// Largest delay this queue accepts, in ticks.
///
/// Every delay this engine schedules is jar-sourced and small (5 for water, 10
/// or 30 for lava, up to 4× that for lava's `getSpreadDelay`), so this only
/// exists so a corrupt or hostile value cannot put an unbounded integer in a map
/// key. 32 768 ticks is 27.3 minutes at 20 TPS.
pub const MAX_FLUID_SCHEDULE_DELAY: u32 = 32_768;

/// What one `schedule` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidScheduled {
    /// A new pending entry was created.
    Scheduled,
    /// The position was already queued for that tick; nothing changed.
    Deduplicated,
    /// The queue was at [`MAX_FLUID_QUEUE_ENTRIES`] and refused the entry.
    Refused,
}

impl FluidScheduled {
    /// Whether the queue gained an entry.
    #[must_use]
    pub const fn was_scheduled(self) -> bool {
        matches!(self, Self::Scheduled)
    }
}

/// How much fluid work one tick may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidBudget {
    /// Fluid ticks the drain may take.
    pub fluid_ticks_per_tick: usize,
}

impl FluidBudget {
    /// A budget with an explicit cap.
    #[must_use]
    pub const fn new(fluid_ticks_per_tick: usize) -> Self {
        Self {
            fluid_ticks_per_tick,
        }
    }

    /// The nominal budget: [`MAX_FLUID_TICKS_PER_TICK`].
    #[must_use]
    pub const fn nominal() -> Self {
        Self::new(MAX_FLUID_TICKS_PER_TICK)
    }

    /// Whether this budget permits no work at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.fluid_ticks_per_tick == 0
    }
}

impl Default for FluidBudget {
    fn default() -> Self {
        Self::nominal()
    }
}

/// What one [`FluidQueue::drain_due`] call returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidDrain {
    /// The tick the drain ran for.
    pub now: Tick,
    /// Positions whose fluid tick came due, ascending by `(due, position)`.
    pub due: Vec<Pos>,
    /// Whether the cap stopped the drain with due work still queued.
    pub budget_exhausted: bool,
    /// Entries still queued after this drain, for *any* tick: the lateness
    /// counter ADR-0009 §5 calls `fluid_ticks_pending`.
    pub pending: usize,
}

impl FluidDrain {
    /// How many fluid ticks came due.
    #[must_use]
    pub fn len(&self) -> usize {
        self.due.len()
    }

    /// Whether nothing came due.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }

    /// Whether anything came due.
    #[must_use]
    pub fn did_work(&self) -> bool {
        !self.due.is_empty()
    }
}

/// The deterministic fluid scheduled-tick queue.
///
/// Ordered by construction: a `BTreeMap` keyed by due tick, each holding a
/// `BTreeSet` of positions. There is no `HashMap` in this module and no code
/// path that depends on insertion order, so two runs of the same scenario drain
/// the same positions in the same sequence.
#[derive(Debug, Clone)]
pub struct FluidQueue {
    /// Scheduled fluid ticks, keyed by due tick then position, both ascending.
    scheduled: BTreeMap<Tick, BTreeSet<Pos>>,
    /// Running total of pending entries, so [`FluidQueue::pending`] is O(1).
    entries: usize,
    /// Work one tick may take.
    max_per_tick: usize,
    /// The tick the queue last drained for.
    last_drained_tick: Option<Tick>,
    /// Entries that had to wait because the per-tick cap was reached.
    spilled_total: u64,
    /// Entries deduplicated instead of queued, since construction.
    deduplicated_total: u64,
    /// Entries refused because the queue was full, since construction.
    refused_total: u64,
}

impl FluidQueue {
    /// An empty queue with the default caps.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scheduled: BTreeMap::new(),
            entries: 0,
            max_per_tick: MAX_FLUID_TICKS_PER_TICK,
            last_drained_tick: None,
            spilled_total: 0,
            deduplicated_total: 0,
            refused_total: 0,
        }
    }

    /// Total pending entries, all due ticks included.
    ///
    /// This is the number the tick report publishes as `fluid_ticks_pending`:
    /// work the server owes and has not run yet (ADR-0009 §5).
    #[must_use]
    pub const fn pending(&self) -> usize {
        self.entries
    }

    /// Whether nothing is queued.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// Total pending entries; the `len`/`is_empty` pair clippy expects.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries
    }

    /// Number of distinct future ticks with at least one entry.
    #[must_use]
    pub fn scheduled_tick_count(&self) -> usize {
        self.scheduled.len()
    }

    /// Positions scheduled for exactly `due`.
    #[must_use]
    pub fn scheduled_at(&self, due: Tick) -> Option<&BTreeSet<Pos>> {
        self.scheduled.get(&due)
    }

    /// Whether anything is scheduled for a tick later than `now`.
    #[must_use]
    pub fn has_pending_after(&self, now: Tick) -> bool {
        self.scheduled
            .range((std::ops::Bound::Excluded(now), std::ops::Bound::Unbounded))
            .next()
            .is_some()
    }

    /// Entries left behind by a capped drain, summed over drains, since
    /// construction.
    ///
    /// This counts **entry-spills, not distinct entries**: an entry that waits
    /// three ticks is counted three times, because each drain that leaves it
    /// behind is one more tick of lateness. It is a pressure signal, not a size.
    #[must_use]
    pub const fn spilled_total(&self) -> u64 {
        self.spilled_total
    }

    /// Entries deduplicated instead of queued, since construction.
    #[must_use]
    pub const fn deduplicated_total(&self) -> u64 {
        self.deduplicated_total
    }

    /// Entries refused because the queue was full, since construction.
    #[must_use]
    pub const fn refused_total(&self) -> u64 {
        self.refused_total
    }

    /// The tick last drained, if any.
    #[must_use]
    pub const fn last_drained_tick(&self) -> Option<Tick> {
        self.last_drained_tick
    }

    /// The per-tick cap.
    #[must_use]
    pub const fn max_per_tick(&self) -> usize {
        self.max_per_tick
    }

    /// Change the per-tick cap.
    ///
    /// Clamped to `1..=MAX_FLUID_QUEUE_ENTRIES`: a cap of zero would look like
    /// an empty queue forever, and a cap above the queue size is meaningless.
    pub fn set_max_per_tick(&mut self, max: usize) {
        self.max_per_tick = max.clamp(1, MAX_FLUID_QUEUE_ENTRIES);
    }

    /// Queue a fluid tick for `pos`, `delay` ticks after `now`.
    ///
    /// The delay is clamped to [`MAX_FLUID_SCHEDULE_DELAY`] rather than refused:
    /// every delay in this engine is jar-sourced, so an out-of-range one is a
    /// corrupt value on a path that must not fail the tick (AGENTS.md §9).
    /// Deduplicated per `(due, position)`.
    pub fn schedule(&mut self, now: Tick, pos: Pos, delay: u32) -> FluidScheduled {
        let due = now.saturating_add(u64::from(delay.min(MAX_FLUID_SCHEDULE_DELAY)));
        self.schedule_at(due, pos)
    }

    /// Queue a fluid tick that is due at `due`.
    pub fn schedule_at(&mut self, due: Tick, pos: Pos) -> FluidScheduled {
        if self
            .scheduled
            .get(&due)
            .is_some_and(|positions| positions.contains(&pos))
        {
            self.deduplicated_total = self.deduplicated_total.saturating_add(1);
            return FluidScheduled::Deduplicated;
        }
        if self.entries >= MAX_FLUID_QUEUE_ENTRIES {
            self.refused_total = self.refused_total.saturating_add(1);
            return FluidScheduled::Refused;
        }
        self.scheduled.entry(due).or_default().insert(pos);
        self.entries += 1;
        FluidScheduled::Scheduled
    }

    /// Take the fluid ticks due at or before `now`, at most
    /// `budget.fluid_ticks_per_tick` of them.
    ///
    /// Only entries due **at or before** `now` are taken, so a delayed update is
    /// never run early and a tick that arrives late (a lag spike) still runs the
    /// work it owes. Entries beyond the cap go **back on their original due
    /// tick** — an already-past tick — so the next drain picks them up first,
    /// in the same order: nothing is dropped, only delayed.
    ///
    /// A `now` earlier than [`FluidQueue::last_drained_tick`] returns nothing and
    /// mutates nothing: ticks only move forward, and a backwards drain would
    /// resurrect consumed work.
    pub fn drain_due(&mut self, now: Tick, budget: FluidBudget) -> FluidDrain {
        if self.last_drained_tick.is_some_and(|last| now < last) {
            return FluidDrain {
                now,
                due: Vec::new(),
                budget_exhausted: false,
                pending: self.entries,
            };
        }
        self.last_drained_tick = Some(now);

        let cap = budget.fluid_ticks_per_tick;
        let mut due = Vec::new();
        let mut budget_exhausted = false;
        // Collect first so the map can be mutated while consuming.
        let due_ticks: Vec<Tick> = self.scheduled.range(..=now).map(|(due, _)| *due).collect();
        'outer: for tick in due_ticks {
            let pending = self.scheduled.remove(&tick).unwrap_or_default();
            let mut remaining = BTreeSet::new();
            for pos in pending {
                if due.len() >= cap {
                    budget_exhausted = true;
                    remaining.insert(pos);
                } else {
                    due.push(pos);
                }
            }
            if !remaining.is_empty() {
                self.spilled_total = self
                    .spilled_total
                    .saturating_add(u64::try_from(remaining.len()).unwrap_or(u64::MAX));
                self.scheduled.entry(tick).or_default().extend(remaining);
                break 'outer;
            }
        }
        // Anything left behind was moved between buckets, never created or lost.
        self.entries = self.scheduled.values().map(BTreeSet::len).sum();
        FluidDrain {
            now,
            due,
            budget_exhausted,
            pending: self.entries,
        }
    }

    /// Forget everything pending, keeping the caps and the counters.
    ///
    /// Used when a dimension unloads: the positions refer to chunks that no
    /// longer exist. This is the only operation that discards queued work, and
    /// it is explicit rather than a side effect.
    pub fn clear(&mut self) {
        self.scheduled.clear();
        self.entries = 0;
    }
}

impl Default for FluidQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FluidBudget, FluidQueue, FluidScheduled, MAX_FLUID_QUEUE_ENTRIES, MAX_FLUID_SCHEDULE_DELAY,
    };
    use crate::fluid::world::Pos;

    fn pos(x: i32, z: i32) -> Pos {
        Pos::new(x, 64, z)
    }

    #[test]
    fn due_ticks_drain_in_ascending_tick_then_position_order() {
        let mut queue = FluidQueue::new();
        // Inserted out of order on purpose: the queue must not preserve it.
        queue.schedule_at(7, pos(3, 0));
        queue.schedule_at(7, pos(-1, 0));
        queue.schedule_at(5, pos(99, 0));
        queue.schedule_at(6, pos(0, 0));
        let drain = queue.drain_due(7, FluidBudget::nominal());
        assert_eq!(
            drain.due,
            vec![pos(99, 0), pos(0, 0), pos(-1, 0), pos(3, 0)],
            "ascending by due tick, then by position"
        );
        assert!(!drain.budget_exhausted);
        assert_eq!(drain.pending, 0);
        assert!(queue.is_empty());
        assert_eq!(drain.len(), 4);
        assert!(drain.did_work());
    }

    #[test]
    fn a_fluid_tick_is_due_once_and_never_early() {
        let mut queue = FluidQueue::new();
        let p = pos(1, 1);
        assert_eq!(
            queue.schedule(10, p, 5),
            FluidScheduled::Scheduled,
            "water's getTickDelay is 5"
        );
        assert_eq!(queue.pending(), 1);
        // Not due yet, and the entry is untouched.
        let early = queue.drain_due(14, FluidBudget::nominal());
        assert!(early.is_empty());
        assert!(!early.did_work());
        assert_eq!(early.pending, 1, "pending is the lateness counter");
        // Due exactly at 15.
        let on_time = queue.drain_due(15, FluidBudget::nominal());
        assert_eq!(on_time.due, vec![p]);
        assert_eq!(on_time.pending, 0);
        // And not twice.
        assert!(queue.drain_due(16, FluidBudget::nominal()).is_empty());
        assert!(queue.is_empty());
    }

    #[test]
    fn a_late_tick_still_runs_the_work_it_owes() {
        // A lag spike must not silently skip a fluid tick: skipping changes when
        // water arrives, which is a visible divergence.
        let mut queue = FluidQueue::new();
        queue.schedule(10, pos(1, 0), 1);
        queue.schedule(10, pos(2, 0), 2);
        let late = queue.drain_due(500, FluidBudget::nominal());
        assert_eq!(late.due, vec![pos(1, 0), pos(2, 0)]);
        assert!(!late.budget_exhausted);
    }

    #[test]
    fn the_per_tick_cap_spills_in_queue_order_and_loses_nothing() {
        // The property ADR-0009 §2.3 asks for, at the queue level: cap 2 over 7
        // positions takes four ticks and yields every position exactly once, in
        // the same order an uncapped drain would have used.
        let mut queue = FluidQueue::new();
        for x in 0..7 {
            queue.schedule_at(5, pos(x, 0));
        }
        let mut seen = Vec::new();
        let mut ticks_used = 0;
        for tick in 5..=10 {
            let drain = queue.drain_due(tick, FluidBudget::new(2));
            ticks_used += 1;
            seen.extend(drain.due.clone());
            if drain.pending == 0 {
                break;
            }
            assert!(drain.budget_exhausted, "work left means the cap stopped us");
        }
        assert_eq!(
            seen,
            (0..7).map(|x| pos(x, 0)).collect::<Vec<_>>(),
            "every position exactly once, in queue order"
        );
        assert_eq!(ticks_used, 4);
        assert!(queue.is_empty());
        // 5 waited after the first drain, 3 after the second and 1 after the
        // third: entry-spills, as the accessor documents.
        assert_eq!(queue.spilled_total(), 5 + 3 + 1);
        assert_eq!(queue.refused_total(), 0, "spill is not refusal");
    }

    #[test]
    fn a_zero_budget_defers_everything_and_a_small_cap_bounds_the_tick() {
        let mut queue = FluidQueue::new();
        for x in 0..5 {
            queue.schedule_at(5, pos(x, 0));
        }
        let deferred = queue.drain_due(5, FluidBudget::new(0));
        assert!(deferred.is_empty());
        assert_eq!(
            deferred.pending, 5,
            "a zero budget is how a caller defers fluid work during a lag spike"
        );
        queue.set_max_per_tick(3);
        let drain = queue.drain_due(6, FluidBudget::new(queue.max_per_tick()));
        assert_eq!(drain.len(), 3);
        assert!(drain.budget_exhausted);
        assert_eq!(drain.pending, 2);
    }

    #[test]
    fn duplicates_are_deduplicated_and_the_queue_refuses_when_full() {
        let mut queue = FluidQueue::new();
        assert_eq!(queue.schedule_at(3, pos(1, 1)), FluidScheduled::Scheduled);
        assert_eq!(
            queue.schedule_at(3, pos(1, 1)),
            FluidScheduled::Deduplicated
        );
        assert_eq!(queue.pending(), 1);
        assert_eq!(queue.deduplicated_total(), 1);
        // A different due tick is a different entry, as in `UpdateQueue`.
        assert_eq!(queue.schedule_at(4, pos(1, 1)), FluidScheduled::Scheduled);
        assert_eq!(queue.pending(), 2);

        let mut full = FluidQueue::new();
        for x in 0..i32::try_from(MAX_FLUID_QUEUE_ENTRIES).expect("fits") {
            assert!(full.schedule_at(1, pos(x, 0)).was_scheduled(), "entry {x}");
        }
        assert_eq!(full.pending(), MAX_FLUID_QUEUE_ENTRIES);
        assert_eq!(full.schedule_at(9, pos(10_000, 0)), FluidScheduled::Refused);
        assert_eq!(
            full.pending(),
            MAX_FLUID_QUEUE_ENTRIES,
            "the queue must not grow past its cap"
        );
        assert_eq!(full.refused_total(), 1);
    }

    #[test]
    fn an_absurd_delay_is_clamped_not_wrapped() {
        let mut queue = FluidQueue::new();
        let p = pos(0, 0);
        assert_eq!(queue.schedule(0, p, u32::MAX), FluidScheduled::Scheduled);
        assert!(
            queue
                .scheduled_at(u64::from(MAX_FLUID_SCHEDULE_DELAY))
                .is_some()
        );
        assert!(queue.has_pending_after(0));
        // The tick counter itself saturates rather than overflowing.
        let mut fresh = FluidQueue::new();
        assert_eq!(
            fresh.schedule(u64::MAX, p, MAX_FLUID_SCHEDULE_DELAY),
            FluidScheduled::Scheduled
        );
        assert!(fresh.scheduled_at(u64::MAX).is_some());
    }

    #[test]
    fn time_never_runs_backwards() {
        let mut queue = FluidQueue::new();
        queue.schedule_at(10, pos(0, 0));
        assert_eq!(queue.drain_due(10, FluidBudget::nominal()).len(), 1);
        assert_eq!(queue.last_drained_tick(), Some(10));
        queue.schedule_at(10, pos(1, 0));
        let backwards = queue.drain_due(9, FluidBudget::nominal());
        assert!(backwards.is_empty());
        assert_eq!(backwards.pending, 1, "the entry is still pending");
        assert_eq!(queue.last_drained_tick(), Some(10), "and the clock held");
    }

    #[test]
    fn clearing_drops_everything_and_keeps_the_counters() {
        let mut queue = FluidQueue::new();
        for x in 0..4 {
            queue.schedule_at(3, pos(x, 0));
        }
        queue.schedule_at(3, pos(0, 0));
        assert_eq!(queue.scheduled_tick_count(), 1);
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.pending(), 0);
        assert_eq!(queue.deduplicated_total(), 1, "counters survive a clear");
        assert!(queue.drain_due(4, FluidBudget::nominal()).is_empty());
    }

    #[test]
    fn the_per_tick_cap_is_clamped_into_a_usable_range() {
        let mut queue = FluidQueue::new();
        queue.set_max_per_tick(0);
        assert_eq!(queue.max_per_tick(), 1, "0 would look like an empty queue");
        queue.set_max_per_tick(usize::MAX);
        assert_eq!(queue.max_per_tick(), MAX_FLUID_QUEUE_ENTRIES);
    }
}
