//! World generation: seeds, noise, biomes, terrain, features, structures,
//! pack-driven ores and carvers, and the existing-world-first chunk provider
//! (P07-13..P07-17, P18-03).
//!
//! ## Why this crate exists, in the phase prompt's own order
//!
//! PHASE-07 says: *"Prioritize loading existing worlds before perfecting every
//! generation feature, then expand toward 26.1.2 parity."* That sentence orders
//! this crate:
//!
//! 1. [`existing::ChunkProvider::chunk_at`] returns a **stored** chunk whenever
//!    one exists and only falls back to a generator when nothing is stored. The
//!    data-loss rule from `Game::load_or_create_chunk` — never write a
//!    placeholder over real terrain — is the reason this is the first thing
//!    built rather than the last;
//! 2. [`seed`] makes generation a pure function of `(world seed, chunk position)`
//!    so a generated chunk is reproducible for ever, in any order (AGENTS.md
//!    §3.6);
//! 3. [`noise`], [`terrain`], [`biome`], [`features`], [`structures`], and now
//!    [`ore`] + [`carver`] then produce terrain that is *labelled as ours* at
//!    every step — see "no parity claim" below.
//!
//! ## No parity claim — what this is and is not
//!
//! **Nothing in this crate reproduces Vanilla world generation.** It is a
//! documented, deterministic baseline that can be replaced field by field:
//!
//! | Piece | Status |
//! |---|---|
//! | `(seed, chunk_pos) → chunk` determinism | **ours**, proven by construction and by test |
//! | Perlin gradient noise | Perlin's published 2002 algorithm; **not** Vanilla's `NormalNoise` |
//! | Octave tables / amplitudes | **approximation**: geometric persistence, not Vanilla's tables |
//! | Biome selection | **ours**: a two-field threshold rule over noise, **not** Vanilla's multi-noise parameter table |
//! | Terrain shape | **ours**: a documented noise → y mapping; **not** Vanilla's density functions |
//! | Biome ids | names copied from Vanilla's `minecraft:` ids; **no biome registry is loaded**, so this is a label, not a lookup |
//! | Block palette | all names resolved through [`mc_registry::BlockRegistry`] — **never** a hard-coded numeric state id |
//! | Trees | oak only, chunk-local, documented density; **not** Vanilla's placed-feature system |
//! | Structure *files* | the 26.1.2 `.nbt` format, **measured** (1 202 files, `DataVersion` 4790); `palettes` (plural, 20 shipwreck files) and block-entity `nbt` are **not** modelled |
//! | Structure *placement* | one whole template per selected chunk on a documented grid; **not** Vanilla's `RandomSpreadStructurePlacement`, and **not** jigsaw assembly |
//! | Structure *types* | templates a data pack ships; mineshafts, strongholds and fortresses are procedural in Vanilla and **are not implemented** |
//! | **Ores** | **pack-driven** counts, y bands, size and air-exposure discard from `configured_feature`/`placed_feature` (P18-03); blob geometry is our approximation of `OreFeature` |
//! | **Caves / canyons** | **pack-driven** cave + canyon carvers (P18-03), dry air only — lakes and water-filled carvers are P20-01b |
//! | Lakes, springs, aquifers, water in carvers | **not implemented** (P20-01b) |
//!
//! Every constant in this crate carries one of four labels: **verified** (with a
//! source), **derived** (from something verified, derivation written out),
//! **approximation** (deliberately simpler, deviation named) or **product
//! decision** (our choice, no Vanilla claim). Terrain is the easiest place in
//! this project to present a guess as a fact, so the labels are next to the
//! numbers, and the crate's doc comments repeat the important ones.
//!
//! ## Where a structure is placed from
//!
//! [`structures::generate_structures`] runs **after** terrain, because a structure
//! sits on the surface and needs the same height field the terrain pass used. It
//! writes only the chunk it is given: [`placement`] documents at length why
//! `mc_world::Chunk` cannot write into a neighbour (it wraps horizontally rather
//! than erroring), why the default policy therefore refuses a structure that does
//! not fit in one chunk, and exactly how many of the real templates that affects.
//!
//! ## Generation order for one chunk (P18-03)
//!
//! ```text
//! terrain fill  →  carvers (cave/canyon, dry)  →  ore veins  →  trees / structures
//! ```
//!
//! Carvers run before ores so a vein can be exposed to cave air and hit its
//! `discard_chance_on_air_exposure`, which is the buried-vein behaviour the
//! pack encodes. [`ore`] and [`carver`] are separate passes so a caller can
//! skip either; `tests/ore_carver_stats.rs` measures both over a 32×32-chunk
//! region with tolerances written before the run.

#![forbid(unsafe_code)]
// Terrain, noise and features narrow and widen constantly: block coordinates are
// `i32`, lattice indices are `usize`, and heights are `f64`. Every site is
// range-checked first, clamped by a documented constant, or lossless by
// construction (`noise::COORDINATE_LIMIT`, `seed::WorldgenContext::section_count`).
// The same exemption is documented in the other binary-format crates.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::cast_precision_loss
)]

pub mod biome;
pub mod carver;
pub mod existing;
pub mod features;
pub mod noise;
pub mod ore;
pub mod pack_json;
pub mod placement;
pub mod seed;
pub mod structure;
pub mod structures;
pub mod terrain;

pub use biome::{BIOMES, Biome, BiomeSource, SurfaceBlocks};
pub use carver::{
    CARVER_SOURCE_RADIUS_CHUNKS, CarverConfig, CarverKind, CarverSet, CarverStats,
    OVERWORLD_CARVERS, carve_chunk, carved_air_fraction, chunk_volume, with_no_replaceables,
};
pub use existing::{
    ChunkLookup, ChunkProvider, FnLookup, MapLookup, NoStorage, ProviderError, ProviderStats,
    StoredChunk,
};
pub use features::{
    OakPlacement, TreeDensity, TreeStats, place_oak_at, populate_oak_trees,
    populate_oak_trees_flat, tree_fits_in_chunk,
};
pub use noise::{FractalNoise, MAX_OCTAVES, PerlinNoise};
pub use ore::{
    MAX_ATTEMPTS_PER_FEATURE, OVERWORLD_ORE_PLACED_FEATURES, OreConfig, OreFeature, OreSet,
    OreStats, OreTarget, ore_blocks_by_band, ore_state_ids, populate_ores,
};
pub use pack_json::{Attempts, FloatRange, HeightDist, TagTable, YAnchor};
pub use placement::{
    AirPolicy, Anchor, CrossChunk, PlacementReport, blocks_outside_chunk, fits_in_chunk, place,
};
pub use seed::{
    ChunkSeed, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y, OVERWORLD_SEA_LEVEL, WorldSeed, WorldgenContext,
};
pub use structure::{
    PaletteEntry, ResolvedStructure, StructureBlock, StructureError, StructureLimits,
    StructureTemplate, parse_gzip_structure, parse_nbt_structure, read_structure,
};
pub use structures::{
    MAX_SELECTABLE_STRUCTURES, STRUCTURE_CHANCE_PER_CELL, STRUCTURE_MAX_ATTEMPTS,
    STRUCTURE_SEPARATION_CHUNKS, STRUCTURE_SPACING_CHUNKS, StructureBuild, StructureGenReport,
    StructureGenRequest, StructureGrid, StructureLoadReport, StructureRegistry, StructureSelection,
    StructureSet, generate_structures, load_structures,
};
pub use terrain::{
    BlockPalette, ChunkGenerator, EXAMPLE_SEED, FlatGenerator, FlatLayer, GenerationError,
    TerrainGenerator,
};
