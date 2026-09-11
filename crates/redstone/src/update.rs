//! The redstone update queue: what has to be recomputed, in which order, and how much of it one
//! tick may do (P06-10).
//!
//! ## Why a queue and not "just recompute the world"
//!
//! A redstone change propagates: toggling a lever changes the dust beside it, which changes the
//! dust beside that, which changes a lamp. Recomputing every block in the world each tick would be
//! correct and unusably slow; recomputing only the neighbours of what changed keeps the cost
//! proportional to the circuit ([ADR-0001 D-02](../../docs/adr/ADR-0001-system-architecture.md)).
//! That makes the *order* of recomputation observable, which in turn makes it a determinism
//! requirement (AGENTS.md §3.6): same initial state, same ordered inputs, same tick count must give
//! the same result.
//!
//! ## Determinism, by construction
//!
//! Every container here is ordered by construction, not by convention:
//!
//! | what | container | iteration order |
//! |---|---|---|
//! | this tick's neighbour updates | `BTreeSet<BlockPos>` | ascending `BlockPos` |
//! | updates raised for the next tick | `BTreeSet<BlockPos>` | ascending `BlockPos` |
//! | scheduled block ticks | `BTreeMap<Tick, BTreeSet<BlockPos>>` | ascending tick, then position |
//!
//! There is no `HashMap` in this module and no code path that depends on insertion order. Two runs
//! of the same scenario therefore produce the identical processing order, and
//! `tests/determinism.rs` compares the full change *sequence* rather than the final state to prove
//! it.
//!
//! ## Bounded, and durable: the property that matters most
//!
//! A redstone clock — a torch that reschedules itself, or a dust loop — is *legal*: it must keep
//! running forever. So the loop is **not** bounded by iteration count. It is bounded by two
//! independent resources, and **no work is ever dropped**:
//!
//! 1. **Queue size.** [`UpdateQueue::MAX_ENTRIES`] (a **product decision**) caps the number of
//!    pending entries. An insert that would exceed the cap is *refused* and counted in
//!    [`UpdateQueue::refused_total`]. The queue never grows past the cap, so a hostile circuit
//!    cannot allocate the server to death (AGENTS.md §10). Re-adding a position already pending is
//!    a no-op, which is what keeps a single clock from filling the queue with copies of itself.
//! 2. **Per-tick work.** [`UpdateQueue::MAX_UPDATES_PER_TICK`] (a **product decision**) caps how
//!    many entries one tick consumes. Work left over **stays queued** and is picked up next tick;
//!    [`UpdateQueue::budget_exhausted`] reports whether the last drain hit the cap so an operator
//!    can see a circuit starving the tick.
//!
//! The consequence, and the reason the split of the neighbour updates into three sets exists: **a
//! bounded run is slower than an unbounded one, never different.** A caller that keeps calling
//! [`UpdateQueue::drain_now`] / `crate::propagation::propagate` one tick at a time reaches exactly
//! the same final state as a caller with an unlimited budget. `tests/budget_exhaustion.rs` asserts
//! that directly, on a line long enough that the budget has to split the work.
//!
//! ## Delay semantics
//!
//! [`UpdateQueue::schedule`] stores `due = now + delay`, mirroring Vanilla's
//! `ServerLevel.scheduleTick(pos, block, delay)` → `LevelTicks.schedule` → `getGameTime() + delay`
//! (**derived** from the decompiled 26.1.2 tree; not verified against a live baseline). A delay of
//! 0 is therefore due *this* tick and a delay of 1 is due next tick. Only entries due at or before
//! `now` are consumed, so a delayed update can never be run early, and a tick that arrives late
//! still runs the work it owes rather than skipping it.

use mc_core::error::{ServerError, ServerResult};
use mc_core::tick::Tick;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A block position, ordered for deterministic iteration.
///
/// ## Why this crate defines its own
///
/// `mc-world` exposes block coordinates as loose `(i32, i32, i32)` parameters (`World::get_block`,
/// `World::set_block`, `BlockChange`) and `mc-persistence` defines `ChunkPos`, not a block
/// position; there is no `BlockPos` anywhere in the workspace to reuse (`crates/protocol` packs one
/// inline as a `long`). So this type exists here rather than being duplicated from somewhere else.
/// It is deliberately *not* wired into `mc-world`: replacing the world's coordinate parameters is a
/// refactor of a crate this task does not own.
///
/// Field order is `(x, y, z)` for readability. Nothing in this crate depends on the *specific*
/// derived order, only on it being total, fixed and independent of insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BlockPos {
    /// World block x.
    pub x: i32,
    /// World block y.
    pub y: i32,
    /// World block z.
    pub z: i32,
}

impl BlockPos {
    /// A position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The position `(dx, dy, dz)` away, saturating at the `i32` bounds.
    ///
    /// Saturating rather than wrapping is deliberate: wrapping `x = i32::MAX` by one would land
    /// inside the world at `x = i32::MIN` and let an update escape to a block the caller never
    /// named (AGENTS.md §10). `tests/hostile_input.rs` feeds in the extreme coordinates to prove
    /// this.
    #[must_use]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            z: self.z.saturating_add(dz),
        }
    }
}

impl fmt::Display for BlockPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

/// The fixed set of the six face-adjacent positions, in a canonical order.
///
/// Vanilla's `Direction` order is down, up, north, south, west, east; this uses the same order so a
/// trace from here can be read next to a Vanilla trace. The order is part of the determinism
/// contract: the algorithm processes neighbours in exactly this sequence, not in the order a set
/// happens to yield.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighbourSet {
    positions: [BlockPos; 6],
}

impl NeighbourSet {
    /// The six neighbours of `pos`, in `down, up, north(-z), south(+z), west(-x), east(+x)` order.
    #[must_use]
    pub const fn of(pos: BlockPos) -> Self {
        Self {
            positions: [
                pos.offset(0, -1, 0),
                pos.offset(0, 1, 0),
                pos.offset(0, 0, -1),
                pos.offset(0, 0, 1),
                pos.offset(-1, 0, 0),
                pos.offset(1, 0, 0),
            ],
        }
    }

    /// The positions.
    #[must_use]
    pub const fn positions(&self) -> &[BlockPos; 6] {
        &self.positions
    }

    /// Iterate the six positions in the canonical order.
    pub fn iter(&self) -> std::slice::Iter<'_, BlockPos> {
        self.positions.iter()
    }
}

impl<'a> IntoIterator for &'a NeighbourSet {
    type Item = &'a BlockPos;
    type IntoIter = std::slice::Iter<'a, BlockPos>;

    fn into_iter(self) -> Self::IntoIter {
        self.positions.iter()
    }
}

impl IntoIterator for NeighbourSet {
    type Item = BlockPos;
    type IntoIter = std::array::IntoIter<BlockPos, 6>;

    fn into_iter(self) -> Self::IntoIter {
        self.positions.into_iter()
    }
}

/// What kind of recomputation an update asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UpdateKind {
    /// A neighbour changed: re-read this position's inputs and recompute its output.
    ///
    /// This is the update a redstone change raises. It carries no delay because Vanilla's
    /// `neighborChanged` runs inside the same tick as the change that caused it; the *delay* in a
    /// repeater, a torch or a comparator is expressed by scheduling [`UpdateKind::ScheduledTick`]
    /// instead.
    NeighborChanged,
    /// A component's own timing elapsed: run its `tick` body after `delay` ticks.
    ///
    /// `delay` is in game ticks. A repeater's configured delay is 1–4 game ticks (**derived**: the
    /// `minecraft:repeater` block state is `delay=1|2|3|4` in the project's own registry fixture,
    /// dumped from the 26.1.2 data), so those values pass straight through.
    ScheduledTick {
        /// Ticks to wait before the update is due, counted from the tick it was scheduled on.
        delay: u32,
    },
    /// A comparator-facing block changed its output signal.
    ///
    /// **approximation**: Vanilla distinguishes a comparator update from a block update because
    /// only some blocks listen for it (`Level.updateNeighbourForOutputSignal`). This queue carries
    /// the kind so the distinction is *representable*, but the propagation algorithm treats it as
    /// an ordinary neighbour update — see `crate::propagation` for that deviation.
    ComparatorUpdate,
}

/// The largest scheduled delay this queue accepts, in ticks.
///
/// **product decision** — not a Vanilla limit. Vanilla's `LevelTicks` has no explicit maximum; this
/// project bounds it because an unbounded delay is an unbounded integer in the queue key, and
/// because a delay measured in real time (2^31 ticks is 3.4 years) can only come from corrupt data
/// or a bug. 32 768 ticks is 27.3 minutes at 20 TPS: comfortably longer than any legitimate redstone
/// timing, and short enough that `due` cannot overflow anywhere.
pub const MAX_SCHEDULE_DELAY: u32 = 32_768;

/// The outcome of trying to add one entry to the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    /// A new pending entry was created.
    Inserted,
    /// The position was already pending; nothing changed.
    ///
    /// Deduplication is not an optimisation here, it is part of what bounds a clock: a component
    /// that raises "update me next tick" every tick contributes one entry per tick, not one per
    /// raise.
    Deduplicated,
    /// The queue is at [`UpdateQueue::MAX_ENTRIES`] and refused the entry.
    ///
    /// **This is the one case where an update is dropped**, and it is deliberate: the alternative
    /// is unbounded memory (AGENTS.md §10). It is counted in
    /// [`UpdateQueue::refused_total`] so the loss is observable rather than silent, and the layer
    /// above can re-derive the dropped position from the world state if it needs to (a neighbour
    /// recomputation is idempotent and needs no history).
    Refused,
}

impl Inserted {
    /// Whether the queue changed.
    #[must_use]
    pub const fn was_inserted(self) -> bool {
        matches!(self, Self::Inserted)
    }

    /// Whether the queue refused the work.
    #[must_use]
    pub const fn was_refused(self) -> bool {
        matches!(self, Self::Refused)
    }
}

/// How much redstone work one tick may do.
///
/// Both fields are **product decisions**, chosen so redstone cannot consume the whole 50 ms tick
/// budget of the Raspberry Pi 5 target (AGENTS.md §13):
///
/// - `neighbour_updates_per_tick` defaults to [`UpdateQueue::MAX_UPDATES_PER_TICK`];
/// - `scheduled_ticks_per_tick` defaults to
///   [`UpdateBudget::DEFAULT_SCHEDULED_TICKS_PER_TICK`], because each due scheduled tick costs a
///   wire rescan, and 256 rescanning circuits is already a pathological situation worth surfacing
///   rather than absorbing silently.
///
/// A budget is a *work* bound, not a wall-clock bound. Nothing here reads a clock: counting
/// operations keeps the simulation deterministic, whereas a time-based cap would make a tick's
/// outcome depend on machine speed (AGENTS.md §3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateBudget {
    /// Neighbour updates the propagation loop may process in this tick.
    pub neighbour_updates_per_tick: usize,
    /// Scheduled ticks that may come due in this tick.
    pub scheduled_ticks_per_tick: usize,
}

impl UpdateBudget {
    /// Default scheduled ticks per tick; see the type docs for why 256.
    ///
    /// **product decision.**
    pub const DEFAULT_SCHEDULED_TICKS_PER_TICK: usize = 256;

    /// A budget with explicit caps.
    ///
    /// `0` is a legal, meaningful budget: it means "process nothing this tick", which is what a
    /// caller uses to defer redstone work during a lag spike. It is not clamped up, and a zero
    /// budget is tested.
    #[must_use]
    pub const fn new(neighbour_updates_per_tick: usize, scheduled_ticks_per_tick: usize) -> Self {
        Self {
            neighbour_updates_per_tick,
            scheduled_ticks_per_tick,
        }
    }

    /// The nominal budget for one server tick.
    #[must_use]
    pub const fn nominal() -> Self {
        Self::new(
            UpdateQueue::MAX_UPDATES_PER_TICK,
            Self::DEFAULT_SCHEDULED_TICKS_PER_TICK,
        )
    }

    /// Whether this budget permits no work at all.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.neighbour_updates_per_tick == 0 && self.scheduled_ticks_per_tick == 0
    }
}

impl Default for UpdateBudget {
    fn default() -> Self {
        Self::nominal()
    }
}

/// What one drain returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighbourDrain {
    /// The tick the drain ran for.
    pub now: Tick,
    /// The positions to recompute, in the order they must be processed.
    pub positions: Vec<BlockPos>,
    /// Whether the cap stopped the drain with more work pending for a later tick.
    pub budget_exhausted: bool,
}

impl NeighbourDrain {
    /// How many updates the caller may process.
    #[must_use]
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether nothing came out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Whether anything came out.
    #[must_use]
    pub fn did_work(&self) -> bool {
        !self.positions.is_empty()
    }
}

/// What one [`UpdateQueue::drain_due_block_ticks`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockTickDrain {
    /// The tick the drain ran for.
    pub now: Tick,
    /// The positions whose scheduled tick came due, ascending by `(due, position)`.
    pub positions: Vec<BlockPos>,
    /// Whether the cap stopped the drain with scheduled ticks still due.
    pub budget_exhausted: bool,
    /// Entries still queued for *later* ticks after this drain.
    pub scheduled_later: usize,
}

impl BlockTickDrain {
    /// How many scheduled ticks came due.
    #[must_use]
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether nothing came due.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Whether anything came due.
    #[must_use]
    pub fn did_work(&self) -> bool {
        !self.positions.is_empty()
    }
}

/// The deterministic redstone update queue.
///
/// ## The working set and the sweep cursor
///
/// - `current` — pending and serviceable now. A drain removes what it takes.
/// - `cursor` — where the sweep through `current` has reached.
///
/// Two properties matter, and each needs one of them.
///
/// **The sweep cursor is what makes a bounded tick live.** A drain that takes the first `cap`
/// positions of a sorted set looks correct and starves everything after them: a live circuit
/// re-raises the same positions continuously, so the set never empties and the same prefix would be
/// taken forever. A redstone line longer than `cap` positions would never light. Resuming **after**
/// the cursor and wrapping around walks the cap through the set instead, so every pending position is
/// reached within `ceil(len / cap)` drains.
///
/// **A position may be serviced more than once in a tick, and that is required for decreases to
/// propagate.** An earlier version serviced each position at most once per tick, which has a subtle
/// and serious consequence: a wire cites the *stored* level of a neighbouring wire, so when a source
/// is destroyed the line can only shed its power one hop per tick. Breaking a torch left the far end
/// of a line live for many ticks — the classic "redstone stays on after you break the source" defect.
/// Allowing a changed position's neighbours back into `current` lets the cascade reach its fixed point
/// in the same tick whenever the budget permits, which is what `tests/propagation.rs` asserts for a
/// five-block line decaying to zero.
///
/// Re-service does not reopen the starvation hole, because the cursor — not "have I seen this
/// already" — is what guarantees progress: a re-added position sorts into the rotation *after* the
/// cursor, so it waits its turn behind positions the drain has not reached. The per-tick *budget* is
/// what bounds the work, which is the honest place for that bound: a churning circuit is slow, not
/// stuck, and `budget_exhausted` says so.
///
/// `Default` is written out rather than derived: a derived `Default` would leave
/// `max_updates_per_tick` at zero, which is not the documented starting state.
///
/// **product decision**: this scheduling rule is this project's, not Vanilla's.
#[derive(Debug, Clone)]
pub struct UpdateQueue {
    /// Pending and serviceable now, ascending.
    current: BTreeSet<BlockPos>,
    /// Where the current sweep through `current` has reached, for the fairness rotation.
    cursor: Option<BlockPos>,
    /// Scheduled block ticks, keyed by due tick then position, both ascending.
    scheduled: BTreeMap<Tick, BTreeSet<BlockPos>>,
    /// Work one tick may consume.
    max_updates_per_tick: usize,
    /// Whether the last drain hit `max_updates_per_tick` with work still pending.
    budget_exhausted: bool,
    /// Entries refused because the queue was full, since construction.
    refused_total: u64,
    /// Entries refused as duplicates, since construction.
    deduplicated_total: u64,
    /// The tick the queue last drained for.
    last_drained_tick: Option<Tick>,
}

impl UpdateQueue {
    /// Maximum pending entries across every container.
    ///
    /// **product decision** — not a Vanilla limit. Vanilla's `LevelTicks` is unbounded; this project
    /// caps it because "unbounded" plus "client-triggerable circuit" is an allocation-amplification
    /// vector (AGENTS.md §10). 4096 distinct pending positions is far more than a 10-player survival
    /// world has live at once, and small enough that the memory ceiling is a few hundred KiB.
    pub const MAX_ENTRIES: usize = 4096;

    /// Default cap on neighbour updates processed per tick.
    ///
    /// **product decision** — not a Vanilla limit (Vanilla has no per-tick redstone cap at all; it
    /// processes the whole update list). At 1024 positions per tick a single tick stays in the low
    /// milliseconds even for the most expensive per-position work this crate does.
    pub const MAX_UPDATES_PER_TICK: usize = 1024;

    /// An empty queue with the default caps.
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: BTreeSet::new(),
            cursor: None,
            scheduled: BTreeMap::new(),
            max_updates_per_tick: Self::MAX_UPDATES_PER_TICK,
            budget_exhausted: false,
            refused_total: 0,
            deduplicated_total: 0,
            last_drained_tick: None,
        }
    }

    /// Total pending entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.current.len() + self.scheduled.values().map(BTreeSet::len).sum::<usize>()
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.current.is_empty() && self.scheduled.is_empty()
    }

    /// Neighbour updates pending for a recomputation.
    #[must_use]
    pub fn neighbour_len(&self) -> usize {
        self.current.len()
    }

    /// Number of distinct future ticks with at least one scheduled entry.
    #[must_use]
    pub fn scheduled_tick_count(&self) -> usize {
        self.scheduled.len()
    }

    /// Positions scheduled for exactly `due`.
    #[must_use]
    pub fn scheduled_at(&self, due: Tick) -> Option<&BTreeSet<BlockPos>> {
        self.scheduled.get(&due)
    }

    /// Entries refused because the queue was full, for backpressure observability.
    ///
    /// A non-zero value means updates were dropped; see [`Inserted::Refused`].
    #[must_use]
    pub const fn refused_total(&self) -> u64 {
        self.refused_total
    }

    /// Entries dropped as duplicates, for backpressure observability.
    #[must_use]
    pub const fn deduplicated_total(&self) -> u64 {
        self.deduplicated_total
    }

    /// Whether the last drain left work behind because it hit the per-tick cap.
    #[must_use]
    pub const fn budget_exhausted(&self) -> bool {
        self.budget_exhausted
    }

    /// The tick last drained, if any.
    #[must_use]
    pub const fn last_drained_tick(&self) -> Option<Tick> {
        self.last_drained_tick
    }

    /// The per-tick neighbour-update cap.
    #[must_use]
    pub const fn max_updates_per_tick(&self) -> usize {
        self.max_updates_per_tick
    }

    /// Change the per-tick cap.
    ///
    /// Clamped to `1..=MAX_ENTRIES`: a cap of 0 would mean "never process anything", which is
    /// indistinguishable from a broken queue, and a cap above the queue size is meaningless. A
    /// caller that wants to defer redstone work should pass a zero [`UpdateBudget`] to
    /// `crate::propagation::propagate` instead, which defers the work explicitly and reports it
    /// rather than silently deadlocking the queue.
    pub fn set_max_updates_per_tick(&mut self, max: usize) {
        self.max_updates_per_tick = max.clamp(1, Self::MAX_ENTRIES);
    }

    /// Every pending neighbour update, ascending, for diagnostics and tests.
    ///
    /// Returned as an owned `Vec` on purpose: a live iterator into the set would make it easy to hold
    /// a borrow across a mutation, and this is a diagnostic accessor, not a hot path.
    #[must_use]
    pub fn pending_neighbours(&self) -> Vec<BlockPos> {
        self.current.iter().copied().collect()
    }

    /// The next position that needs servicing, if any.
    ///
    /// The propagation loop uses this to decide whether to keep draining.
    #[must_use]
    pub fn next_pending(&self) -> Option<BlockPos> {
        self.current.iter().next().copied()
    }

    /// Queue a neighbour update for `pos`.
    ///
    /// A raise for a position that changed earlier in the same tick is **serviceable in the same
    /// tick**, so a cascade reaches its fixed point as far as the budget allows. See [`UpdateQueue`]
    /// for why that is required (a decrease has to be able to travel the whole line) and why it does
    /// not reopen the starvation hole (the sweep cursor, not a "seen it" filter, gives progress).
    pub fn push_neighbour(&mut self, pos: BlockPos) -> Inserted {
        if self.current.contains(&pos) {
            self.deduplicated_total = self.deduplicated_total.saturating_add(1);
            return Inserted::Deduplicated;
        }
        if self.len() >= Self::MAX_ENTRIES {
            self.refused_total = self.refused_total.saturating_add(1);
            return Inserted::Refused;
        }
        self.current.insert(pos);
        Inserted::Inserted
    }

    /// Queue neighbour updates for every position in `positions`.
    ///
    /// Returns how many were newly inserted. Refusals are counted in
    /// [`UpdateQueue::refused_total`] rather than returned one by one, because the caller's reaction
    /// to "the queue is full" is the same for all of them — and because a hostile circuit can
    /// produce thousands at once.
    pub fn push_neighbours(&mut self, positions: &[BlockPos]) -> usize {
        let mut inserted = 0;
        for pos in positions {
            if self.push_neighbour(*pos).was_inserted() {
                inserted += 1;
            }
        }
        inserted
    }

    /// Schedule a block tick for `pos`, `delay` ticks after `now`.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `delay > MAX_SCHEDULE_DELAY`, or when `now + delay` would
    /// overflow the tick counter. Both are refusals, not clamps — see
    /// [`UpdateQueue::schedule_clamped`] for the hostile-input path.
    pub fn schedule(&mut self, now: Tick, pos: BlockPos, delay: u32) -> ServerResult<Inserted> {
        if delay > MAX_SCHEDULE_DELAY {
            return Err(ServerError::InvalidAction(format!(
                "scheduled tick delay {delay} exceeds the {MAX_SCHEDULE_DELAY} tick maximum"
            )));
        }
        let due = now.checked_add(u64::from(delay)).ok_or_else(|| {
            ServerError::InvalidAction(format!("scheduled tick at tick {now} + {delay} overflows"))
        })?;
        Ok(self.schedule_at(due, pos))
    }

    /// Schedule a block tick from a possibly hostile delay, clamping into range.
    ///
    /// Never fails: an absurd delay becomes [`MAX_SCHEDULE_DELAY`]. Use this for values that came
    /// off the wire or out of a file.
    pub fn schedule_clamped(&mut self, now: Tick, pos: BlockPos, delay: u32) -> Inserted {
        let delay = delay.min(MAX_SCHEDULE_DELAY);
        let due = now.saturating_add(u64::from(delay));
        self.schedule_at(due, pos)
    }

    /// Schedule a block tick that is due at `due`.
    ///
    /// Deduplicated per `(due, position)`: scheduling the same position for the same tick twice is
    /// one entry, matching Vanilla's `LevelTicks` set semantics (**derived** from
    /// `LevelTicks.schedule` keeping a per-position collection).
    pub fn schedule_at(&mut self, due: Tick, pos: BlockPos) -> Inserted {
        if self
            .scheduled
            .get(&due)
            .is_some_and(|positions| positions.contains(&pos))
        {
            self.deduplicated_total = self.deduplicated_total.saturating_add(1);
            return Inserted::Deduplicated;
        }
        if self.len() >= Self::MAX_ENTRIES {
            self.refused_total = self.refused_total.saturating_add(1);
            return Inserted::Refused;
        }
        self.scheduled.entry(due).or_default().insert(pos);
        Inserted::Inserted
    }

    /// Queue a comparator update.
    ///
    /// Stored exactly like a neighbour update, because this crate's propagation treats the two
    /// identically; see [`UpdateKind::ComparatorUpdate`].
    pub fn push_comparator_update(&mut self, pos: BlockPos) -> Inserted {
        self.push_neighbour(pos)
    }

    /// Whether anything is scheduled for a tick later than `now`.
    #[must_use]
    pub fn has_pending_after(&self, now: Tick) -> bool {
        self.scheduled
            .range((std::ops::Bound::Excluded(now), std::ops::Bound::Unbounded))
            .next()
            .is_some()
    }

    /// Advance the queue's clock to `now`.
    ///
    /// Called by [`UpdateQueue::drain_due_block_ticks`] and [`UpdateQueue::drain_for_tick`], the two
    /// entry points that receive a tick number. It is deliberately *not* called by
    /// [`UpdateQueue::drain_now`], which the propagation loop calls repeatedly inside one tick.
    ///
    /// The sweep cursor is **kept** across the boundary. That is what makes the rotation fair beyond
    /// one tick: a live circuit re-raises the same positions every tick, so restarting each tick from
    /// the lowest position would hand the whole cap to the same prefix forever.
    fn begin_tick(&mut self, now: Tick) {
        self.last_drained_tick = Some(now);
    }

    /// Take up to the per-tick cap of pending neighbour updates, ascending.
    ///
    /// This is the drain a caller without a per-tick budget uses. It **does not change the current
    /// tick**. A caller that owns a tick number should call
    /// [`UpdateQueue::drain_due_block_ticks`] (or [`UpdateQueue::drain_for_tick`]) once at the start
    /// of the tick to advance the queue's clock.
    ///
    /// Work is *taken*: a caller that drains and then fails to process the result has lost it, which
    /// is why the only intended caller is `crate::propagation::propagate`, which drains only what its
    /// budget allows and processes all of it.
    pub fn drain_now(&mut self) -> NeighbourDrain {
        let now = self.last_drained_tick.unwrap_or(0);
        let cap = self.max_updates_per_tick;
        self.take_neighbours(now, cap)
    }

    /// Take at most `max` pending neighbour updates, ascending from the sweep cursor.
    ///
    /// This is the drain a caller with a per-tick budget should use: **the cap must be the same
    /// number the caller will actually process**, or the sweep cursor advances past work that was
    /// never done and the rotation stops being fair. That was a real bug in this crate — the drain
    /// took everything under `max_updates_per_tick` while `propagate` truncated to a smaller budget,
    /// so the cursor always landed at the end of the set and the same prefix was taken every tick.
    ///
    /// `max` is clamped to `1..=MAX_ENTRIES`; a drain of zero is meaningless and would look like an
    /// empty queue.
    ///
    /// Keeps the tick as it is, like [`UpdateQueue::drain_now`].
    pub fn drain_at_most(&mut self, max: usize) -> NeighbourDrain {
        let now = self.last_drained_tick.unwrap_or(0);
        self.take_neighbours(now, max.clamp(1, Self::MAX_ENTRIES))
    }

    /// Take up to the per-tick cap of pending neighbour updates for `now`, advancing the queue's
    /// clock to `now` first.
    ///
    /// `now` must not go backwards: a `now` earlier than [`UpdateQueue::last_drained_tick`] returns
    /// nothing and mutates nothing, because ticks only move forward and a backwards drain would
    /// resurrect consumed work and break the "one scheduled tick per tick" guarantee.
    pub fn drain_for_tick(&mut self, now: Tick) -> NeighbourDrain {
        if self.last_drained_tick.is_some_and(|last| now < last) {
            return NeighbourDrain {
                now,
                positions: Vec::new(),
                budget_exhausted: false,
            };
        }
        self.begin_tick(now);
        let cap = self.max_updates_per_tick;
        self.take_neighbours(now, cap)
    }

    /// The shared body of the drains: take up to `cap`, rotating through the pending set.
    ///
    /// Positions taken are **removed** from `current`, so the set shrinks as work is done and a
    /// settled circuit drains to empty. A position that changes re-enters through `push_neighbour`
    /// and is serviced again in the same tick if the rotation reaches it, which is what lets a
    /// decrease travel a whole line.
    ///
    /// The rotation is what keeps that live. A drain that takes the first `cap` positions of a sorted
    /// set starves everything after them, because a live circuit keeps re-raising the same positions
    /// and refilling the set: the same prefix would be taken forever. Resuming **after** the cursor
    /// and wrapping around walks the cap through the set instead, so every pending position is
    /// reached within `ceil(len / cap)` drains.
    fn take_neighbours(&mut self, now: Tick, cap: usize) -> NeighbourDrain {
        // Build the rotation order: everything after the cursor, then everything up to and including
        // it. `None` means "start from the beginning", which is the state of a fresh queue.
        let mut ordered: Vec<BlockPos> = Vec::with_capacity(self.current.len());
        match self.cursor {
            Some(cursor) => {
                ordered.extend(
                    self.current
                        .range((
                            std::ops::Bound::Excluded(cursor),
                            std::ops::Bound::Unbounded,
                        ))
                        .copied(),
                );
                ordered.extend(self.current.range(..=cursor).copied());
            }
            None => ordered.extend(self.current.iter().copied()),
        }

        let mut positions = Vec::with_capacity(cap.min(ordered.len()));
        for pos in ordered {
            if positions.len() >= cap {
                break;
            }
            if self.current.remove(&pos) {
                self.cursor = Some(pos);
                positions.push(pos);
            }
        }

        // Honest exhaustion: work this drain did not take, because the cap stopped it.
        self.budget_exhausted = self.neighbour_len() != 0;
        NeighbourDrain {
            now,
            positions,
            budget_exhausted: self.budget_exhausted,
        }
    }

    /// Consume the scheduled block ticks due at or before `now`, at most
    /// `budget.scheduled_ticks_per_tick` of them.
    ///
    /// Only entries due **at or before** `now` are consumed, so a delayed update is never run early,
    /// and a tick that arrives late (a lag spike) still runs the work it owes rather than skipping it
    /// — skipping would silently change a circuit's timing. Entries due strictly later are
    /// untouched, and `scheduled_later` counts exactly those.
    ///
    /// A `now` earlier than [`UpdateQueue::last_drained_tick`] is refused: no work is done and
    /// nothing is mutated.
    pub fn drain_due_block_ticks(&mut self, now: Tick, budget: UpdateBudget) -> BlockTickDrain {
        let mut positions = Vec::new();
        let mut budget_exhausted = false;
        if self.last_drained_tick.is_some_and(|last| now < last) {
            return BlockTickDrain {
                now,
                positions,
                budget_exhausted,
                scheduled_later: self.len(),
            };
        }
        // Register the tick even when nothing is scheduled: this is the call that advances the
        // queue's clock, and a tick with no scheduled work must still release the neighbour updates
        // held for it.
        self.begin_tick(now);

        // Collect the due ticks first so the map can be mutated while consuming.
        let due_ticks: Vec<Tick> = self.scheduled.range(..=now).map(|(due, _)| *due).collect();
        'outer: for due in due_ticks {
            let pending = self.scheduled.remove(&due).unwrap_or_default();
            let mut remaining = Vec::new();
            for pos in pending {
                if positions.len() >= budget.scheduled_ticks_per_tick {
                    budget_exhausted = true;
                    remaining.push(pos);
                } else {
                    positions.push(pos);
                }
            }
            if !remaining.is_empty() {
                // Put the remainder back on its original (already past) due tick so the next drain
                // picks it up immediately: nothing is dropped, and the order within the tick is
                // preserved.
                self.scheduled.entry(due).or_default().extend(remaining);
                break 'outer;
            }
        }
        BlockTickDrain {
            now,
            positions,
            budget_exhausted,
            scheduled_later: self.len(),
        }
    }

    /// Forget everything pending, keeping the caps and the counters.
    ///
    /// Used when a dimension unloads: the positions refer to chunks that no longer exist. This is
    /// the only operation that discards queued work, and it is explicit rather than a side effect.
    pub fn clear(&mut self) {
        self.current.clear();
        self.cursor = None;
        self.scheduled.clear();
        self.budget_exhausted = false;
    }
}

impl Default for UpdateQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockPos, Inserted, MAX_SCHEDULE_DELAY, NeighbourSet, UpdateBudget, UpdateKind, UpdateQueue,
    };

    fn pos(x: i32, z: i32) -> BlockPos {
        BlockPos::new(x, 64, z)
    }

    #[test]
    fn block_pos_offsets_and_saturates_at_the_coordinate_bounds() {
        let origin = BlockPos::new(0, 64, 0);
        assert_eq!(origin.offset(1, 0, 0), BlockPos::new(1, 64, 0));
        assert_eq!(origin.offset(0, -1, 0), BlockPos::new(0, 63, 0));
        // Saturating, not wrapping: a wrapping +1 on i32::MAX would land at i32::MIN.
        assert_eq!(
            BlockPos::new(i32::MAX, 0, 0).offset(1, 0, 0).x,
            i32::MAX,
            "must not wrap into the far side of the world"
        );
        assert_eq!(BlockPos::new(i32::MIN, 0, 0).offset(-1, 0, 0).x, i32::MIN);
        assert_eq!(
            BlockPos::new(i32::MAX, i32::MAX, i32::MAX).offset(1, 1, 1),
            BlockPos::new(i32::MAX, i32::MAX, i32::MAX)
        );
        assert_eq!(origin.to_string(), "(0, 64, 0)");
        assert_eq!(BlockPos::default(), BlockPos::new(0, 0, 0));
    }

    #[test]
    fn neighbours_are_the_six_faces_in_a_fixed_order() {
        let set = NeighbourSet::of(BlockPos::new(1, 2, 3));
        let expected = [
            BlockPos::new(1, 1, 3), // down
            BlockPos::new(1, 3, 3), // up
            BlockPos::new(1, 2, 2), // north
            BlockPos::new(1, 2, 4), // south
            BlockPos::new(0, 2, 3), // west
            BlockPos::new(2, 2, 3), // east
        ];
        assert_eq!(set.positions(), &expected);
        assert_eq!(set.iter().count(), 6);
        assert_eq!(set.into_iter().collect::<Vec<_>>(), expected.to_vec());
        // Every neighbour is one step away and the set contains no duplicates.
        let mut sorted = expected.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 6);
    }

    #[test]
    fn update_kinds_are_distinct_and_ordered() {
        assert_ne!(UpdateKind::NeighborChanged, UpdateKind::ComparatorUpdate);
        assert!(UpdateKind::NeighborChanged < UpdateKind::ScheduledTick { delay: 1 });
        assert_eq!(
            UpdateKind::ScheduledTick { delay: 4 },
            UpdateKind::ScheduledTick { delay: 4 }
        );
    }

    #[test]
    fn neighbour_updates_drain_in_ascending_position_order() {
        let mut queue = UpdateQueue::new();
        // Insert out of order on purpose: the queue must not preserve insertion order.
        for x in [7, -3, 0, 100, -100] {
            assert!(queue.push_neighbour(pos(x, 0)).was_inserted());
        }
        let drain = queue.drain_for_tick(1);
        let mut expected = vec![pos(-100, 0), pos(-3, 0), pos(0, 0), pos(7, 0), pos(100, 0)];
        expected.sort_unstable();
        assert_eq!(drain.positions, expected);
        assert!(!drain.budget_exhausted);
        assert!(queue.is_empty());
        assert_eq!(drain.len(), 5);
        assert!(drain.did_work());
        assert!(!drain.is_empty());
    }

    #[test]
    fn duplicates_are_deduplicated_not_queued_twice() {
        let mut queue = UpdateQueue::new();
        assert_eq!(queue.push_neighbour(pos(1, 1)), Inserted::Inserted);
        assert_eq!(queue.push_neighbour(pos(1, 1)), Inserted::Deduplicated);
        assert_eq!(queue.push_neighbour(pos(1, 1)), Inserted::Deduplicated);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.deduplicated_total(), 2);
        assert_eq!(queue.refused_total(), 0);
        // `push_neighbours` reports only the newly inserted ones.
        let inserted = queue.push_neighbours(&[pos(1, 1), pos(2, 2), pos(2, 2)]);
        assert_eq!(inserted, 1);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn the_queue_refuses_work_when_full_instead_of_growing() {
        let mut queue = UpdateQueue::new();
        for x in 0..i32::try_from(UpdateQueue::MAX_ENTRIES).expect("fits") {
            assert!(
                queue.push_neighbour(pos(x, 0)).was_inserted(),
                "entry {x} should fit"
            );
        }
        assert_eq!(queue.len(), UpdateQueue::MAX_ENTRIES);
        for x in 0..100 {
            assert_eq!(queue.push_neighbour(pos(10_000 + x, 0)), Inserted::Refused);
        }
        assert_eq!(
            queue.len(),
            UpdateQueue::MAX_ENTRIES,
            "the queue must not grow past its cap"
        );
        assert_eq!(queue.refused_total(), 100);
        // Scheduled entries share the cap.
        assert_eq!(queue.schedule_at(5, pos(0, 0)), Inserted::Refused);
    }

    #[test]
    fn scheduling_is_relative_to_the_current_tick() {
        let mut queue = UpdateQueue::new();
        let p = pos(4, 4);
        assert_eq!(
            queue.schedule(100, p, 2).expect("legal delay"),
            Inserted::Inserted
        );
        assert!(queue.scheduled_at(102).is_some_and(|set| set.contains(&p)));
        assert!(queue.has_pending_after(101));
        assert!(!queue.has_pending_after(102));
        // Re-scheduling the same (due, position) is a duplicate.
        assert_eq!(
            queue.schedule(100, p, 2).expect("legal"),
            Inserted::Deduplicated
        );
        assert_eq!(queue.scheduled_tick_count(), 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn absurd_delays_are_refused_by_schedule_and_clamped_by_schedule_clamped() {
        let mut queue = UpdateQueue::new();
        let p = pos(0, 0);
        assert!(queue.schedule(0, p, MAX_SCHEDULE_DELAY).is_ok());
        assert!(queue.schedule(0, p, MAX_SCHEDULE_DELAY + 1).is_err());
        assert!(queue.schedule(0, p, u32::MAX).is_err());
        assert!(matches!(
            queue.schedule(0, p, u32::MAX),
            Err(mc_core::error::ServerError::InvalidAction(_))
        ));
        // The tick counter itself can overflow, and that is refused too — on a fresh queue, because
        // the one above already holds p for a later tick and would deduplicate.
        let mut fresh = UpdateQueue::new();
        assert!(fresh.schedule(u64::MAX, p, 1).is_err());
        assert_eq!(
            fresh.schedule(u64::MAX, p, 0).ok(),
            Some(Inserted::Inserted)
        );
        assert_eq!(
            fresh.schedule(u64::MAX, p, 0).ok(),
            Some(Inserted::Deduplicated)
        );
        // The hostile-input path clamps instead of failing.
        let mut hostile = UpdateQueue::new();
        assert_eq!(hostile.schedule_clamped(0, p, u32::MAX), Inserted::Inserted);
        assert!(
            hostile.has_pending_after(0),
            "a clamped delay is still scheduled"
        );
        assert!(
            hostile
                .scheduled_at(u64::from(MAX_SCHEDULE_DELAY))
                .is_some()
        );
        assert_eq!(
            hostile.schedule_clamped(u64::MAX, p, u32::MAX),
            Inserted::Inserted,
            "a saturating due tick must not panic"
        );
    }

    #[test]
    fn scheduled_ticks_come_due_exactly_once_and_at_the_right_tick() {
        let mut queue = UpdateQueue::new();
        let p = pos(1, 1);
        queue.schedule(10, p, 3).expect("legal");
        // Not due yet.
        let early = queue.drain_due_block_ticks(12, UpdateBudget::nominal());
        assert!(early.is_empty());
        assert!(!early.did_work());
        assert_eq!(early.scheduled_later, 1);
        // Due exactly at 13.
        let on_time = queue.drain_due_block_ticks(13, UpdateBudget::nominal());
        assert_eq!(on_time.positions, vec![p]);
        assert!(!on_time.budget_exhausted);
        assert_eq!(on_time.scheduled_later, 0);
        // And it does not come due twice.
        let again = queue.drain_due_block_ticks(14, UpdateBudget::nominal());
        assert!(again.is_empty());
        assert!(queue.is_empty());
    }

    #[test]
    fn a_late_tick_still_runs_the_work_it_owes() {
        // A lag spike must not silently skip a scheduled tick: skipping would change the circuit's
        // timing, which is worse than running it late.
        let mut queue = UpdateQueue::new();
        queue.schedule(10, pos(1, 0), 1).expect("legal");
        queue.schedule(10, pos(2, 0), 2).expect("legal");
        let late = queue.drain_due_block_ticks(50, UpdateBudget::nominal());
        assert_eq!(late.len(), 2);
        assert_eq!(late.positions, vec![pos(1, 0), pos(2, 0)]);
        assert!(!late.budget_exhausted);
    }

    #[test]
    fn the_scheduled_budget_bounds_a_tick_and_leaves_the_rest_queued() {
        let mut queue = UpdateQueue::new();
        for x in 0..10 {
            queue.schedule_at(5, pos(x, 0));
        }
        let budget = UpdateBudget::new(1024, 4);
        let drain = queue.drain_due_block_ticks(5, budget);
        assert_eq!(drain.len(), 4);
        assert!(drain.budget_exhausted);
        assert_eq!(queue.len(), 6, "the rest stays queued");
        // The next drain continues where this one stopped, in the same order.
        let more = queue.drain_due_block_ticks(6, UpdateBudget::new(1024, 4));
        assert_eq!(more.len(), 4);
        assert_eq!(
            more.positions,
            vec![pos(4, 0), pos(5, 0), pos(6, 0), pos(7, 0)]
        );
        let last = queue.drain_due_block_ticks(7, UpdateBudget::new(1024, 4));
        assert_eq!(last.positions, vec![pos(8, 0), pos(9, 0)]);
        assert!(!last.budget_exhausted);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_zero_budget_defers_everything_and_a_small_cap_bounds_the_tick() {
        let mut queue = UpdateQueue::new();
        for x in 0..5 {
            queue.push_neighbour(pos(x, 0));
            queue.schedule_at(5, pos(x, 0));
        }
        // A zero budget is the documented way to defer redstone work during a lag spike: nothing is
        // consumed and nothing is lost.
        let deferred = queue.drain_due_block_ticks(5, UpdateBudget::new(0, 0));
        assert!(deferred.is_empty());
        assert_eq!(queue.len(), 10, "nothing may be consumed");
        // A small neighbour cap is what `set_max_updates_per_tick` is for; note the clamped minimum
        // of 1, so "0" cannot be expressed this way by accident.
        queue.set_max_updates_per_tick(3);
        let drain = queue.drain_for_tick(5);
        assert_eq!(drain.len(), 3);
        assert!(drain.budget_exhausted);
        assert_eq!(queue.neighbour_len(), 2);
    }

    #[test]
    fn the_per_tick_cap_is_clamped_into_a_usable_range() {
        let mut queue = UpdateQueue::new();
        queue.set_max_updates_per_tick(0);
        assert_eq!(
            queue.max_updates_per_tick(),
            1,
            "0 would deadlock the queue"
        );
        queue.set_max_updates_per_tick(usize::MAX);
        assert_eq!(queue.max_updates_per_tick(), UpdateQueue::MAX_ENTRIES);
    }

    #[test]
    fn time_never_runs_backwards() {
        let mut queue = UpdateQueue::new();
        queue.push_neighbour(pos(0, 0));
        queue.schedule_at(10, pos(1, 0));
        let first = queue.drain_for_tick(10);
        assert_eq!(first.len(), 1);
        assert_eq!(queue.last_drained_tick(), Some(10));
        // A backwards drain is refused entirely and mutates nothing.
        queue.push_neighbour(pos(2, 0));
        let backwards = queue.drain_for_tick(9);
        assert!(backwards.is_empty());
        assert_eq!(queue.neighbour_len(), 1, "the entry is still pending");
        assert_eq!(queue.last_drained_tick(), Some(10));
        let backwards_ticks = queue.drain_due_block_ticks(9, UpdateBudget::nominal());
        assert!(backwards_ticks.is_empty());
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn budget_exhausted_reports_the_last_drain_not_a_running_total() {
        let mut queue = UpdateQueue::new();
        queue.push_neighbour(pos(0, 0));
        queue.push_neighbour(pos(1, 0));
        queue.set_max_updates_per_tick(1);
        assert!(queue.drain_for_tick(1).budget_exhausted);
        assert!(queue.budget_exhausted());
        assert!(!queue.drain_for_tick(2).budget_exhausted);
        assert!(!queue.budget_exhausted(), "the flag tracks the last drain");
    }

    #[test]
    fn a_cap_limited_drain_loses_nothing_and_resumes_in_order() {
        // The durability property, at the queue level: cap 2 over 7 positions takes four ticks and
        // yields every position exactly once, in ascending order.
        let mut queue = UpdateQueue::new();
        for x in 0..7 {
            queue.push_neighbour(pos(x, 0));
        }
        queue.set_max_updates_per_tick(2);
        let mut seen = Vec::new();
        for tick in 1..=10 {
            let drain = queue.drain_for_tick(tick);
            seen.extend(drain.positions);
            if queue.is_empty() {
                break;
            }
        }
        assert_eq!(
            seen,
            (0..7).map(|x| pos(x, 0)).collect::<Vec<_>>(),
            "every position exactly once, ascending"
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn a_raised_position_is_serviceable_again_in_the_same_tick() {
        // A raise for a position that was already serviced this tick goes back into `current`. That
        // is required, not incidental: a wire cites a neighbouring wire's stored level, so if a
        // position could not be serviced twice in a tick a *decrease* could only travel one hop per
        // tick — a destroyed source would leave the far end of a line live for many ticks.
        let mut queue = UpdateQueue::new();
        let p = pos(0, 0);
        assert!(queue.push_neighbour(p).was_inserted());
        assert_eq!(queue.drain_for_tick(1).positions, vec![p]);
        assert!(queue.is_empty(), "the set shrank as the work was done");
        // Raising it again makes it serviceable immediately.
        assert_eq!(queue.push_neighbour(p), Inserted::Inserted);
        assert_eq!(queue.neighbour_len(), 1);
        assert_eq!(queue.next_pending(), Some(p));
        assert_eq!(
            queue.drain_now().positions,
            vec![p],
            "same tick, second service"
        );
        assert!(queue.is_empty());
        // A duplicate raise for a position already pending is still deduplicated, so a hot position
        // cannot multiply itself in the queue.
        queue.push_neighbour(p);
        assert_eq!(queue.push_neighbour(p), Inserted::Deduplicated);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn the_sweep_cursor_walks_the_whole_set_instead_of_retaking_the_prefix() {
        // The liveness property, isolated: with a raise after every drain (which is what a live
        // circuit does) a cap-limited drain must still reach every position. Without the cursor this
        // loops on the prefix forever and the positions above the cap are never serviced.
        let mut queue = UpdateQueue::new();
        let all: Vec<BlockPos> = (0..7).map(|x| pos(x, 0)).collect();
        for p in &all {
            queue.push_neighbour(*p);
        }
        queue.set_max_updates_per_tick(3);
        let mut serviced = std::collections::BTreeSet::new();
        for tick in 1..=10u64 {
            let drain = queue.drain_for_tick(tick);
            assert!(drain.len() <= 3, "the cap must bound the drain");
            for p in &drain.positions {
                serviced.insert(*p);
            }
            // Re-raise everything serviced, the way a live circuit does.
            for p in &drain.positions {
                queue.push_neighbour(*p);
            }
        }
        assert_eq!(
            serviced,
            all.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            "every position must be reached despite the cap and the constant re-raises"
        );
    }

    #[test]
    fn a_position_is_reachable_again_next_tick_after_the_set_empties() {
        // A live circuit re-raises itself every tick, so an empty queue must not mean "done".
        let mut queue = UpdateQueue::new();
        let p = pos(3, 3);
        queue.push_neighbour(p);
        let mut serviced = 0usize;
        for tick in 1..=5u64 {
            let drain = queue.drain_for_tick(tick);
            serviced += drain.len();
            if drain.did_work() {
                queue.push_neighbour(p);
            }
        }
        assert_eq!(serviced, 5, "one service per tick, every tick");
        assert!(!queue.is_empty(), "the clock is still armed");
    }

    #[test]
    fn a_drain_that_empties_the_set_clears_budget_exhausted() {
        let mut queue = UpdateQueue::new();
        for x in 0..5 {
            queue.push_neighbour(pos(x, 0));
        }
        queue.set_max_updates_per_tick(5);
        let exact = queue.drain_for_tick(1);
        assert_eq!(exact.len(), 5);
        assert!(
            !exact.budget_exhausted,
            "taking exactly the cap with nothing left is not exhaustion"
        );
    }

    #[test]
    fn clear_drops_everything_pending() {
        let mut queue = UpdateQueue::new();
        queue.push_neighbour(pos(0, 0));
        queue.schedule_at(3, pos(1, 0));
        queue.drain_for_tick(1);
        queue.push_neighbour(pos(0, 0));
        assert!(!queue.is_empty());
        queue.clear();
        assert!(queue.is_empty());
        assert!(!queue.budget_exhausted());
        assert_eq!(queue.scheduled_tick_count(), 0);
        assert_eq!(queue.neighbour_len(), 0);
        assert!(queue.next_pending().is_none());
    }

    #[test]
    fn pending_neighbours_reports_the_pending_set() {
        let mut queue = UpdateQueue::new();
        queue.push_neighbour(pos(5, 0));
        queue.drain_for_tick(1);
        assert!(queue.is_empty(), "the drain consumed it");
        queue.push_neighbour(pos(5, 0));
        queue.push_neighbour(pos(9, 0));
        assert_eq!(queue.neighbour_len(), 2);
        let mut pending = queue.pending_neighbours();
        pending.sort_unstable();
        assert_eq!(pending, vec![pos(5, 0), pos(9, 0)]);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.next_pending(), Some(pos(5, 0)));
        // Both are serviceable; the sweep cursor resumes after pos(5), where the previous drain
        // stopped, so the order is the rotation rather than ascending from the start. The set is what
        // matters here.
        let next = queue.drain_for_tick(2).positions;
        let mut sorted = next.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![pos(5, 0), pos(9, 0)], "both are serviced");
        assert!(queue.is_empty());
    }

    #[test]
    fn a_default_queue_is_empty_and_uses_the_documented_caps() {
        let queue = UpdateQueue::default();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(
            queue.max_updates_per_tick(),
            UpdateQueue::MAX_UPDATES_PER_TICK
        );
        assert_eq!(queue.refused_total(), 0);
        assert_eq!(queue.deduplicated_total(), 0);
        assert_eq!(queue.last_drained_tick(), None);
        assert!(!queue.budget_exhausted());
        assert!(queue.next_pending().is_none());
        assert_eq!(UpdateBudget::default(), UpdateBudget::nominal());
        assert!(!UpdateBudget::nominal().is_empty());
        assert!(UpdateBudget::new(0, 0).is_empty());
    }
}
