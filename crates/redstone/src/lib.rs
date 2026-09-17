//! Redstone simulation foundation: a power model, a deterministic update queue and
//! an update-propagation algorithm (P06-09, P06-10, P06-11).
//!
//! This crate is the **foundation** for redstone, not a redstone implementation. It
//! answers three questions and refuses to pretend to answer the rest:
//!
//! 1. *What is a signal?* — [`power`]: a strength `0..=15`, split into a weak and a
//!    strong kind, produced by a small set of [`PowerSource`](power::PowerSource)s.
//! 2. *What has to be recomputed, in which order?* — [`update`]: a
//!    [`BlockPos`](update::BlockPos)-ordered queue of neighbour updates and scheduled
//!    ticks with documented size and per-tick bounds.
//! 3. *How does a change spread?* — [`propagation`]: recompute a block from its six
//!    neighbours, and if it changed, queue its neighbours.
//!
//! ## The vertical slice, and its edges
//!
//! The slice that actually works end to end is: **lever/torch/repeater/comparator →
//! redstone dust, attenuating 1 per block → a wire that reports its power**. Every
//! number in that path is either verified and cited or explicitly labelled as not
//! verified (see the evidence labels below). `tests/golden_circuits.rs` writes the
//! expected power at every position out by hand, and `tests/determinism.rs` proves the
//! processing order is reproducible.
//!
//! What is **not** implemented, and is not hidden behind a working-looking API:
//!
//! - **Mechanisms**: pistons, doors, trapdoors, fence gates, dispensers,
//!   droppers, note blocks, TNT, copper bulbs. A mechanism block is
//!   [`propagation::BlockRole::Passive`] — it never reacts — except the
//!   redstone lamp, which is [`propagation::BlockRole::Mechanism`] and lights
//!   when powered (P13-03).
//! - **Observers**, hoppers (and any container-driven comparator output), rails of every
//!   kind, sculk sensors, daylight detectors, target blocks, trapped chests, tripwire
//!   hooks, jukeboxes, lecterns.
//! - **Conductivity**: a solid block is never powered, so "lever attached to a block,
//!   dust on the far side" does not work. This is the largest single gap and it is
//!   called out in [`propagation`].
//! - **Quasi-connectivity.**
//! - **Timing**: a repeater's configured delay is stored and validated, not waited on.
//!   A torch changes state with no delay, and dust has no re-evaluation delay of its
//!   own. Only a wire's "ask again next tick" schedule exists.
//! - **Orientation**: a component has no facing, so its inputs and outputs are not
//!   directional. A comparator's side inputs are therefore not read.
//! - **Vanilla's update order**, and Vanilla's separation of block updates from shape
//!   updates and comparator updates.
//! - **Writing component states back**: the propagation loop writes redstone dust's
//!   `power` property and a lamp's `lit` property, and leaves every other block's
//!   state alone. Nothing here toggles a lever, because levers are driven by player
//!   actions in the server, not by the circuit.
//! - **Persistence**: nothing in this crate serialises the queue. A server restart loses
//!   pending updates, which is correct for a queue of same-tick work but would need an
//!   answer before scheduled ticks survive a save.
//!
//! ## Evidence labels
//!
//! AGENTS.md §3.1 forbids claiming Vanilla behaviour without evidence, and §3.3 forbids
//! hiding an unsupported case behind a completed-looking API. Redstone is full of
//! numbers that "everyone knows" and are subtly wrong, so every constant and rule in
//! this crate carries one of four labels, written next to it:
//!
//! | label | meaning |
//! |---|---|
//! | **verified** | stated by a source of truth, cited inline (usually minecraft.wiki, or the project's own registry fixture dumped from the 26.1.2 jar) |
//! | **derived** | follows by construction from something verified, with the derivation written out |
//! | **approximation** | deliberately simpler than Vanilla; the deviation is named |
//! | **product decision** | a bound or shape this project chose; not a Vanilla numeric value at all |
//!
//! **No part of this crate is claimed to be "Vanilla-accurate".** The strongest claim
//! made anywhere is that the wire-attenuation rule and the 0–15 strength range match
//! minecraft.wiki's description, and that block names and property names match the
//! registry dumped from the 26.1.2 server jar. Everything else is labelled.
//!
//! ## Determinism (AGENTS.md §3.6)
//!
//! There is no `HashMap` in this crate, no clock read, and no randomness. Every
//! container is a `BTreeMap`/`BTreeSet` or a `Vec` iterated in order, both queue halves
//! have a documented ordering rule, and the neighbour order is the fixed six-face
//! order of [`update::NeighbourSet`]. Two runs of the same scenario with the same
//! ordered inputs therefore produce the same sequence of changes, which
//! `tests/determinism.rs` asserts by comparing the whole change vector.
//!
//! ## Errors and hostile input (AGENTS.md §9, §10)
//!
//! There is no `unwrap`, `expect`, `panic!` or `unreachable!` in this crate outside
//! `#[cfg(test)]`. A power level above 15 is refused
//! ([`PowerLevel::new`](power::PowerLevel::new)) or clamped
//! ([`PowerLevel::from_raw`](power::PowerLevel::from_raw)); an absurd scheduled delay is
//! refused by [`UpdateQueue::schedule`](update::UpdateQueue::schedule) and clamped by
//! [`schedule_clamped`](update::UpdateQueue::schedule_clamped); queues are capped and
//! refuse rather than grow; block coordinates saturate instead of wrapping; and a
//! zero [`UpdateBudget`](update::UpdateBudget) processes nothing and reports it.

#![forbid(unsafe_code)]

pub mod blocks;
pub mod components;
pub mod power;
pub mod propagation;
pub mod update;

#[doc(inline)]
pub use blocks::{REDSTONE_LAMP, REDSTONE_WIRE, SOURCE_BLOCKS};
#[doc(inline)]
pub use components::{
    Comparator, ComparatorMode, ComponentState, FACING_COUNT, Lever, RedstoneTorch, Repeater,
};
#[doc(inline)]
pub use power::{MAX_POWER, PowerLevel, PowerSource, PowerState, SignalKind};
#[doc(inline)]
pub use propagation::{
    BlockInputs, BlockRole, BlockView, EmitterOutput, EmitterTable, FlatWorld, MIN_LIVE_WIRE_POWER,
    PropagatedChange, PropagationReport, WIRE_ATTENUATION_PER_BLOCK, WIRE_LIVE_BLOCKS,
};
#[doc(inline)]
pub use update::{
    BlockPos, Inserted, MAX_SCHEDULE_DELAY, NeighbourSet, UpdateBudget, UpdateKind, UpdateQueue,
};
