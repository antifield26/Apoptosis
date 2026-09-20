//! Shared primitives for the Minecraft server workspace.
//!
//! `core` owns identifiers, math helpers, timing/tick types and the error
//! taxonomy contract (AGENTS.md sections 7-9, ADR-0001 D-01). It must stay free
//! of networking, world state and gameplay semantics so every other crate can
//! depend on it without cycles.

#![forbid(unsafe_code)]
// The paletted-container packing in `packing` narrows and widens integers
// constantly (bit widths, slots, offsets), `random` reinterprets LCG bits
// the way `java.util.Random` does (`nextInt`/`nextLong`), and its float draws
// divide by `2^24`/`2^53` exactly as the JDK specifies — the precision change
// is the specified formula, not a defect. Every other site either
// bounds-checks the value first or is lossless by construction. The pedantic
// cast lints would add noise without catching real defects, matching the
// exemption already documented in `mc-protocol`, `mc-nbt` and `mc-persistence`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

pub mod error;
pub mod ids;
pub mod packing;
pub mod random;
pub mod tick;
