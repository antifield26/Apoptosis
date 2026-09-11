//! Fixed-step simulation: phase ordering, tick scheduling and deterministic
//! randomness (P05-01, P05-02).
//!
//! `mc-simulation` owns the **shape of a tick**, not the content of any phase.
//! [ADR-0001 D-02](../../docs/adr/ADR-0001-system-architecture.md) fixes the
//! in-tick order as
//! `net-drain → tasks → worlds → entities → block-entities → packet-flush`, and
//! that order is what makes the simulation reproducible: the same state plus the
//! same ordered inputs plus the same tick count must produce the same normalized
//! state (AGENTS.md §3.6).
//!
//! ## Why the order is a contract
//!
//! Phase order is observable. Moving entity AI before scheduled block ticks, or
//! flushing packets before entities move, changes what a client sees on a given
//! tick even when the final state is identical. So the order is:
//!
//! 1. [`TickPhase::Network`] — drain inbound events and apply player intents.
//!    First, because everything else must see this tick's input.
//! 2. [`TickPhase::ScheduledTicks`] — block/fluid/entity ticks that fell due.
//!    Before entities, because a scheduled tick can move or destroy a block an
//!    entity is about to collide with.
//! 3. [`TickPhase::Entities`] — entity AI and physics, in ascending entity id.
//! 4. [`TickPhase::Players`] — player physics and actions. After entities, so a
//!    player resolves against the world this tick's entities left behind.
//! 5. [`TickPhase::BlockEntities`] — block-entity behaviour (furnaces, hoppers).
//! 6. [`TickPhase::Broadcast`] — flush outbound packets. Last, so a client sees a
//!    tick's complete result rather than a partial one.
//!
//! Determinism also needs a controllable source of randomness, since mob spawning
//! and AI make random choices. [`RandomSource`] is a seeded `java.util.Random`
//! clone, so a scenario replays identically and the sequence is the same one
//! Vanilla would draw (useful for later differential work).
//!
//! ## Threading (AGENTS.md section 8)
//!
//! Everything here is synchronous and single-threaded by design: the tick thread
//! owns the simulation. I/O stays on Tokio and hands work over bounded channels.
//! [`Scheduler`] does not spawn, await, or touch a socket.

#![forbid(unsafe_code)]
// Tick accounting converts between durations and integers constantly; every site is
// a saturating conversion of a monotonic clock reading, bounded by `WINDOW`.
//
// The `RandomSource` casts are not incidental: a 48-bit LCG produces *bit
// patterns*, and Java's `nextInt` reinterprets 32 bits as a signed int while
// `nextLong` sign-extends each half. The sign reinterpretation is the algorithm
// being reproduced, which is why it is allowed here with that justification
// rather than hidden behind a helper that would obscure the correspondence.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod metrics;
pub mod phase;
pub mod random;
pub mod scheduler;

pub use metrics::{TickMetrics, TickStats};
pub use phase::{PHASE_COUNT, PHASE_ORDER, TickPhase};
pub use random::RandomSource;
pub use scheduler::{PhaseRunner, Scheduler, TickOutcome};
