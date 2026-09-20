//! Vanilla-compatible world persistence (Phase 03, ADR-0001 D-04).
//!
//! Scope: the Anvil region format, `level.dat`, dimension/world metadata, the
//! chunk serialization boundary, dirty tracking, autosave scheduling and
//! ordered/atomic save semantics. Gameplay world state (chunk runtime objects,
//! block updates) belongs to `mc-world` in Phase 04 — this crate defines the
//! **disk schema** and the boundary that converts to and from it.
//!
//! ## Verified against a real 26.1.2 world
//!
//! Every format constant here was measured on a world generated and saved by the
//! vanilla 26.1.2 server (`target/vanilla-26.1.2`, evidence recorded in
//! `docs/research/protocol-baseline.md` section 2 and
//! the Phase 03 report (git history, tag `phase-09-final`)):
//!
//! | Fact | Value | How it was measured |
//! |---|---|---|
//! | write `DataVersion` | 4790 | `level.dat` `Data.DataVersion`; also every chunk and `data/minecraft/*.dat` |
//! | level `version` | 19133 | `level.dat` `Data.version` |
//! | region geometry | 32×32 slots, 4096-byte sectors, 8 KiB header | `RegionFile` bytecode constants + file inspection |
//! | default compression id | 2 (`deflate`) | `RegionFileVersion.DEFAULT = VERSION_DEFLATE` + 320/320 chunks with id 2 |
//! | chunk root name | `""` | NBT dump of a real chunk |
//! | dimension layout | `<world>/dimensions/<ns>/<value>/{region,entities,poi,data}` | directory listing of the generated world |
//!
//! Unsupported inputs are refused with an explicit error, never guessed at
//! (AGENTS.md section 3.3): LZ4/custom region compression, external `.mcc`
//! chunks, pre-1.18 chunk layouts and `DataVersion`s outside the readable window.
//!
//! ## Ownership and threading (AGENTS.md section 8)
//!
//! [`world::WorldStorage`] is deliberately **single-owner**: it owns open file
//! handles and a write-behind queue, so it is `Send` but not `Sync` and takes
//! `&mut self` for every operation.
//!
//! - **Owner**: the save worker (Phase 04/P08 wires it to a dedicated thread).
//! - **Synchronization**: none inside the crate; the tick thread hands over
//!   [`chunk::ChunkData`] values over a bounded channel, which is the only
//!   cross-thread boundary.
//! - **Ordering**: chunk writes are applied in `BTreeMap` (dimension, position)
//!   order so a given dirty set always produces the same byte layout, keeping
//!   saves reproducible (AGENTS.md section 3.6).
//! - **Backpressure**: the channel bound is the caller's; [`world::WorldStorage::queue_chunk_save`]
//!   never blocks on I/O.
//! - **Failure**: a failed chunk write stays queued and is reported in
//!   [`save::SaveReport::errors`], so a later flush retries it instead of
//!   silently dropping the change.
//! - **Shutdown**: [`world::WorldStorage::close`] flushes, syncs every open
//!   region file and writes `level.dat` last (ADR-0001 D-04 ordering).

#![forbid(unsafe_code)]
// Binary format work narrows and widens integers constantly (sector indices,
// bit widths, chunk coordinates). Every site is bounds-checked first or is
// lossless by construction; the pedantic cast lints would only add noise, as
// already documented for `mc-protocol` and `mc-nbt`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod autosave;
pub mod chunk;
pub mod compression;
pub mod dimension;
pub mod dirty;
pub mod level;
pub mod region;
pub mod save;
pub mod world;

pub use chunk::{BlockState, ChunkData, ChunkPos, PalettedContainer, SectionData};
pub use compression::Compression;
pub use dimension::Dimension;
pub use dirty::DirtyTracker;
pub use level::{Difficulty, LevelDat, SpawnPoint};
pub use region::RegionFile;
pub use save::SaveReport;
pub use world::WorldStorage;
