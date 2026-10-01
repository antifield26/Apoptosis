//! The in-tick phase order (P05-02, extended by P20-01).
//!
//! See the crate docs for why this order and not another. The important property
//! for tests is that [`PHASE_ORDER`] is a `const` array: the order cannot be
//! changed at runtime, so a future refactor that reorders phases has to change
//! this one visible place and will break the ordering test.
//!
//! The two fluid/random-tick phases and their position between the block queue
//! and the entities are fixed by
//! [ADR-0009 §2.1](../../../docs/adr/ADR-0009-world-ticking.md), which mirrors
//! the order the 26.1.2 jar's `ServerLevel.tick` calls its subsystems in
//! (`tickTime` → `blockTicks.tick` → `fluidTicks.tick` → `tickChunk` →
//! `entityTickList.forEach` → `tickBlockEntities`), verified with `javap` on
//! 2026-10-01.

/// One step of a tick.
///
/// Declared in execution order, and [`PHASE_ORDER`] is the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TickPhase {
    /// Drain inbound network events and apply player intents.
    ///
    /// First: every later phase must observe this tick's input.
    Network,
    /// Advance the world clock, then run due **block** scheduled ticks.
    ///
    /// The world time and weather timers move here because the jar puts
    /// `tickTime()` before both tick queues (ADR-0009 §2.2). Fluid ticks are
    /// **not** drained here since P20-01: they have their own queue and their
    /// own phase, exactly as the jar's `ServerLevel` owns a separate
    /// `LevelTicks<Fluid>`.
    ScheduledTicks,
    /// Run due **fluid** scheduled ticks (P20-01).
    ///
    /// After the block queue and before the random-tick sweep, which is the
    /// jar's verified order (ADR-0009 §2.1).
    FluidTicks,
    /// Sweep the ticking radius for random ticks (P20-01 skeleton, P20-02 growth).
    ///
    /// A **sweep**, not a queue: the jar's `ServerLevel.tickChunk(LevelChunk, int)`
    /// samples each section at the `randomTickSpeed` game rule's rate. P20-01
    /// installs the phase, the radius and the counters; the per-block growth
    /// handlers are P20-02's deliverable, so this phase currently samples and
    /// applies nothing (see `crates/server/src/game/tick.rs::phase_random_ticks`).
    RandomTicks,
    /// Entity AI and physics, in ascending entity id.
    Entities,
    /// Player physics and actions.
    Players,
    /// Block-entity behaviour (containers, furnaces, hoppers).
    BlockEntities,
    /// Flush outbound packets.
    ///
    /// Last: a client must see a tick's complete result, never a partial one.
    Broadcast,
}

/// Number of phases, for fixed-size timing arrays.
pub const PHASE_COUNT: usize = 8;

/// The authoritative execution order.
pub const PHASE_ORDER: [TickPhase; PHASE_COUNT] = [
    TickPhase::Network,
    TickPhase::ScheduledTicks,
    TickPhase::FluidTicks,
    TickPhase::RandomTicks,
    TickPhase::Entities,
    TickPhase::Players,
    TickPhase::BlockEntities,
    TickPhase::Broadcast,
];

impl TickPhase {
    /// Index of this phase in [`PHASE_ORDER`], for timing arrays.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Network => 0,
            Self::ScheduledTicks => 1,
            Self::FluidTicks => 2,
            Self::RandomTicks => 3,
            Self::Entities => 4,
            Self::Players => 5,
            Self::BlockEntities => 6,
            Self::Broadcast => 7,
        }
    }

    /// Stable name for logs and metric labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::ScheduledTicks => "scheduled_ticks",
            Self::FluidTicks => "fluid_ticks",
            Self::RandomTicks => "random_ticks",
            Self::Entities => "entities",
            Self::Players => "players",
            Self::BlockEntities => "block_entities",
            Self::Broadcast => "broadcast",
        }
    }

    /// All phases in execution order.
    #[must_use]
    pub const fn all() -> &'static [TickPhase; PHASE_COUNT] {
        &PHASE_ORDER
    }
}

impl std::fmt::Display for TickPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::{PHASE_COUNT, PHASE_ORDER, TickPhase};

    #[test]
    fn the_order_is_the_documented_one() {
        // Changing this list changes observable behaviour; the assertion exists so
        // that change is deliberate and reviewed rather than incidental. This is
        // the ADR-0009 §2.1 order, widened from six phases to eight by P20-01.
        assert_eq!(
            PHASE_ORDER,
            [
                TickPhase::Network,
                TickPhase::ScheduledTicks,
                TickPhase::FluidTicks,
                TickPhase::RandomTicks,
                TickPhase::Entities,
                TickPhase::Players,
                TickPhase::BlockEntities,
                TickPhase::Broadcast,
            ]
        );
    }

    #[test]
    fn the_fluid_and_random_tick_phases_sit_where_adr_0009_puts_them() {
        // The jar's order is time → block ticks → fluid ticks → random ticks →
        // entities → block entities (ADR-0009 §1). Pinning it as positions, not
        // just membership, is what makes a reorder a red test rather than a
        // silent behaviour change.
        assert_eq!(TickPhase::FluidTicks.index(), 2);
        assert_eq!(TickPhase::RandomTicks.index(), 3);
        assert!(TickPhase::ScheduledTicks < TickPhase::FluidTicks);
        assert!(TickPhase::FluidTicks < TickPhase::RandomTicks);
        assert!(TickPhase::RandomTicks < TickPhase::Entities);
        assert_eq!(TickPhase::FluidTicks.name(), "fluid_ticks");
        assert_eq!(TickPhase::RandomTicks.name(), "random_ticks");
    }

    #[test]
    fn indices_match_positions_and_cover_every_phase() {
        for (position, phase) in PHASE_ORDER.iter().enumerate() {
            assert_eq!(phase.index(), position, "{phase} index");
        }
        assert_eq!(PHASE_ORDER.len(), PHASE_COUNT);
        // Every variant appears exactly once.
        let mut seen = [0usize; PHASE_COUNT];
        for phase in PHASE_ORDER {
            seen[phase.index()] += 1;
        }
        assert!(seen.iter().all(|count| *count == 1), "{seen:?}");
    }

    #[test]
    fn network_is_first_and_broadcast_is_last() {
        assert_eq!(PHASE_ORDER[0], TickPhase::Network);
        assert_eq!(PHASE_ORDER[PHASE_COUNT - 1], TickPhase::Broadcast);
        assert_eq!(TickPhase::all().len(), PHASE_COUNT);
    }

    #[test]
    fn names_are_unique_and_non_empty() {
        let mut names: Vec<&str> = PHASE_ORDER.iter().map(|phase| phase.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PHASE_COUNT, "phase names must be unique");
        assert!(names.iter().all(|name| !name.is_empty()));
        assert_eq!(TickPhase::Network.to_string(), "network");
    }
}
