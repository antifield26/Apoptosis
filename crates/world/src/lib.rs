//! World runtime: chunk block storage, collision and block targeting (P04-02/07/08).
//!
//! Boundary (ADR-0001 D-01, refined by ADR-0002): `mc-persistence` owns the
//! **disk schema** (`ChunkData`, block states by name), `mc-registry` owns the
//! **name ↔ numeric id** tables, and this crate owns the **runtime** form a tick
//! touches: blocks by id, per-section counts, collision shapes and ray casting.
//! Nothing here does I/O; conversion to and from [`mc_persistence::ChunkData`] is
//! explicit at the edges.
//!
//! ## What "block" means here
//!
//! Every block reference is a **global block-state id** (0..29 873 in Vanilla
//! 26.1.2), the same number the client receives in chunk palettes and
//! `block_update`. The ids come from the official jar's own registry, via
//! [`mc_registry::BlockRegistry`].
//!
//! ## Scope: what this crate does and does not model
//!
//! - **No generation.** [`World::ensure_chunk`] creates an all-air chunk when one
//!   is missing, so a caller always has something to write into. It is a
//!   placeholder, not terrain: generation lives in `mc-worldgen` and is driven by
//!   `mc-server`, which fills a chunk before it can be seen. Nothing in this
//!   crate claims generated terrain.
//! - **Collision is a per-state lookup, not a cube model.** A solid cell collides
//!   as the boxes the registry's shape table gives it, with the full cube as the
//!   default for a state with no entry (P16-06); see [`collision`].
//! - **Light is computed, not merely carried.** [`light`] implements the sky
//!   flood, the block-light BFS and the jar-derived emission/dampening tables;
//!   the per-chunk cache and its invalidation live in [`World`].
//! - **No fluid, redstone or scheduled-tick simulation.** The power model is
//!   `mc-redstone`, the tick phases are `mc-simulation`/`mc-server`, and fluids
//!   are out of scope (water and lava are non-solid here).

#![forbid(unsafe_code)]
// Runtime block code narrows and widens constantly (section indices, y ranges,
// bit widths, packed positions). Every site is bounds-checked first or lossless by
// construction; the same exemption is documented in the other binary-format crates.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod chunk;
pub mod collision;
pub mod light;
pub mod ray;
pub mod world;

pub use chunk::{Chunk, ChunkPos, SECTION_HEIGHT, SECTION_WIDTH};
pub use collision::{
    Aabb, BlockHit, NO_STEP_UP, STEP_HEIGHT, Vec3, is_full_cube, is_solid, is_solid_or_unknown,
    look_vector,
};
pub use ray::{BlockSampler, RegionSampler, ray_cast};
pub use world::{BlockChange, MoveResult, World, chunks_a_block_can_light};

/// Y range of the Vanilla overworld: sections -4..=19 cover y = -64..=319.
pub const OVERWORLD_MIN_SECTION_Y: i8 = -4;
/// Number of sections in the Vanilla overworld.
pub const OVERWORLD_SECTION_COUNT: usize = 24;
