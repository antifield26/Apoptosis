//! The in-tick phase order (P05-02).
//!
//! See the crate docs for why this order and not another. The important property
//! for tests is that [`PHASE_ORDER`] is a `const` array: the order cannot be
//! changed at runtime, so a future refactor that reorders phases has to change
//! this one visible place and will break the ordering test.

/// One step of a tick.
///
/// Declared in execution order, and [`PHASE_ORDER`] is the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TickPhase {
    /// Drain inbound network events and apply player intents.
    ///
    /// First: every later phase must observe this tick's input.
    Network,
    /// Run scheduled block, fluid and entity ticks that came due.
    ScheduledTicks,
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
pub const PHASE_COUNT: usize = 6;

/// The authoritative execution order.
pub const PHASE_ORDER: [TickPhase; PHASE_COUNT] = [
    TickPhase::Network,
    TickPhase::ScheduledTicks,
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
            Self::Entities => 2,
            Self::Players => 3,
            Self::BlockEntities => 4,
            Self::Broadcast => 5,
        }
    }

    /// Stable name for logs and metric labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::ScheduledTicks => "scheduled_ticks",
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
        // that change is deliberate and reviewed rather than incidental.
        assert_eq!(
            PHASE_ORDER,
            [
                TickPhase::Network,
                TickPhase::ScheduledTicks,
                TickPhase::Entities,
                TickPhase::Players,
                TickPhase::BlockEntities,
                TickPhase::Broadcast,
            ]
        );
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
