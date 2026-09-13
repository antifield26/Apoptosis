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
//! ## Deliberate simplifications (P04 scope)
//!
//! - **No world generation.** A chunk that is loaded but absent becomes all air
//!   with `minecraft:full` status. That is a placeholder so the player can stand
//!   somewhere, not generation; P07 owns terrain. It is marked in the API name
//!   ([`World::ensure_chunk`]) so no caller mistakes it for content.
//! - **Collision is full-cube.** A block is solid unless it is air-like or in the
//!   explicit [`collision::NON_SOLID`] list. Slabs, stairs and fences therefore
//!   collide as full cubes — recorded in the parity matrix as a known gap.
//! - **No lighting engine.** Light arrays are carried through from disk and (once
//!   sent) left zero; see `docs/protocol/chunk-wire-format.md` section 6.
//! - **No fluid, redstone or scheduled-tick simulation.** Those are P05/P06.

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
pub use collision::{Aabb, BlockHit, Vec3, is_solid, is_solid_or_unknown, look_vector};
pub use ray::{BlockSampler, RegionSampler, ray_cast};
pub use world::{BlockChange, MoveResult, World};

/// Y range of the Vanilla overworld: sections -4..=19 cover y = -64..=319.
pub const OVERWORLD_MIN_SECTION_Y: i8 = -4;
/// Number of sections in the Vanilla overworld.
pub const OVERWORLD_SECTION_COUNT: usize = 24;
