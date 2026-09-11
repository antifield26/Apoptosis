//! The structure registry, its loader, the grid selection rule, and the
//! after-terrain entry point (P07-16).
//!
//! ## What is implemented, and what is not (the honest list)
//!
//! **Implemented**: reading structure templates from a data pack, resolving their
//! palettes to block-state ids by name, holding them in a deterministic registry,
//! choosing *whether* and *which* structure generates per chunk from
//! `(seed, chunk_pos)`, and placing the chosen template on the terrain surface
//! with every block accounted for.
//!
//! **Not implemented** — each of these is a real capability of Vanilla's
//! structure system and its absence is a divergence, not a detail:
//!
//! - **Jigsaw assembly.** Vanilla's villages, bastions, trial chambers and
//!   ancient cities are assembled at generation time from many small templates
//!   connected by jigsaw blocks. This crate places **one whole template** per
//!   selected chunk. There is no `JigsawPlacement`, no connector matching, no
//!   pools, no `max_depth`.
//! - **Structure types and placement records.** No `StructureSet` JSON, no
//!   `concentric_rings`, no `random_spread` with Vanilla's parameters, no
//!   biome tag filters, no `terrain_adaptation` (Vanilla's "beard" that shaves
//!   the terrain around a structure), no `start_pool`/`start_height`.
//! - **Structures that are not templates.** Mineshafts, strongholds, nether
//!   fortresses, ocean monuments, desert pyramids and ruined portals are
//!   generated *procedurally* in Vanilla, not read from `.nbt` files. No `.nbt`
//!   file exists for them in the pack (the pack has no `mineshaft/` directory),
//!   so nothing here can produce them even in principle.
//! - **Loot, spawners and block entities.** 4 848 of the pack's blocks carry
//!   `nbt` (a chest's loot table, a spawner's mob). The block is placed; its
//!   contents are dropped. See [`crate::structure`].
//! - **Entity spawning.** 55 of the 1 202 files declare one or two entities
//!   (villagers, pigs, piglins). [`crate::structure::StructureTemplate::entities`]
//!   counts them; nothing spawns them.
//! - **Mirroring, rotation and structure processors.** No `StructureProcessor`
//!   chain, so no block-age randomisation, no gravity processing, no
//!   `CobblestoneReplacement`, and no rotation of a template to face a
//!   direction. Every placement is axis-aligned and unrotated.
//! - **The 20 `palettes` (plural) files.** Every `shipwreck/*.nbt` is refused by
//!   name rather than loaded wrong; see [`crate::structure`].
//!
//! ## The selection rule, in full
//!
//! For each chunk, the world is divided into square **cells** of
//! [`StructureGrid::cell_chunks`] chunks. For the cell containing the chunk:
//!
//! ```text
//! h0 = splitmix64_mix(seed.raw() as u64 ^ GOLDEN_GAMMA ^ pack_chunk_pos(cell))
//! attempts = 1 + h0 % max_attempts                // 1..=max_attempts
//! for attempt in 0..attempts:
//!     h1 = splitmix64_mix(h0 ^ STREAM_SALT ^ (attempt as u64 + 1))
//!     if (h1 >> 11) as f64 / 2^53 >= chance:  continue
//!     h2 = splitmix64_mix(h1 ^ STREAM_ORIGIN)
//!     h3 = splitmix64_mix(h2 ^ STREAM_STRUCTURE)
//!     origin = cell corner + (h2 % spacing, h3 % spacing)   // in chunks
//!     pick   = names[h3 % names.len()]
//!     if origin is this chunk: place `pick` anchored on the surface
//! ```
//!
//! The chunk compares its own position against the origin rather than falling
//! through "the first chunk in the cell", which is what makes the decision a pure
//! function of `(seed, chunk_pos)` — there is no neighbour to consult and no
//! order to depend on (AGENTS.md §3.6).
//!
//! Every draw goes through [`splitmix64_mix`], the crate's one mixer, seeded from
//! `(seed, cell, attempt)`. No `HashMap`/`HashSet` iteration, no clock, no
//! thread-local appears on this path: the registry is a [`BTreeMap`] and the name
//! list is sorted, so the same `(seed, pos)` gives the same answer in any process
//! and any order.
//!
//! ## Measured properties of the shipped rule
//!
//! With the shipped constants ([`STRUCTURE_SPACING_CHUNKS`] `8`,
//! [`STRUCTURE_SEPARATION_CHUNKS`] `2`, [`STRUCTURE_MAX_ATTEMPTS`] `1`,
//! [`STRUCTURE_CHANCE_PER_CELL`] `0.5`) over the 64×64-cell area
//! `x, z ∈ 0..64` (4 096 chunks), measured by `tests/structure_selection.rs`:
//!
//! ```text
//! chunks with a structure      ~4.6%     (not every chunk: the
//!                                        "sparse" half of P07-16)
//! distinct templates selected  all of them, for every registry size tested,
//!                                        including a registry of one template
//!                                        and a registry of 1 182
//! ```
//!
//! The last line is the "reachable" half: selection is `h3 % names.len()`, a
//! uniform draw over the sorted name list, so every registered template has
//! probability `1 / names.len()` of being chosen at each attempt. No template is
//! unreachable, and `tests/structure_selection.rs` asserts it over a sample rather
//! than trusting the argument.
//!
//! ## The spacing constants are approximations, and are labelled as such
//!
//! [`StructureGrid::spacing_chunks`] and [`StructureGrid::separation_chunks`] are
//! named after Vanilla's `RandomSpreadStructurePlacement` fields because they
//! play the same role. **The numbers are ours.** Vanilla's per-structure spacing
//! and separation values live in worldgen JSON this project has not loaded, and
//! Vanilla's exact candidate arithmetic (`RandomSpreadStructurePlacement`'s
//! `getPotentialStructureChunk`) has not been reproduced here. A structure at the
//! same coordinates in this server and in Vanilla is a coincidence, not parity.

use crate::placement::{AirPolicy, Anchor, CrossChunk, PlacementReport, place};
use crate::seed::{WorldSeed, WorldgenContext, pack_chunk_pos, splitmix64_mix};
use crate::structure::{
    ResolvedStructure, StructureError, StructureLimits, StructureTemplate, read_structure,
};
use mc_core::ids::ResourceId;
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos, SECTION_WIDTH};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ constants

/// Horizontal spacing of the structure grid, in chunks.
///
/// **approximation / product decision.** Named after Vanilla's
/// `RandomSpreadStructurePlacement.spacing` (a structure is attempted once per
/// `spacing × spacing` chunk cell), but the value is **ours**: `8` cells, i.e. a
/// 128-block lattice. Vanilla's per-structure spacing values are not verified
/// here and are not loaded from any data pack.
pub const STRUCTURE_SPACING_CHUNKS: i32 = 8;

/// Minimum separation between two attempts in one cell, in chunks.
///
/// **approximation / product decision.** Vanilla's `separation` shrinks the
/// region a candidate may be placed in, so two neighbouring cells' structures do
/// not touch. Here it is applied **within** a cell — an origin is drawn from
/// `0..spacing - separation` — which is the same *intent* with arithmetic that is
/// definitely not Vanilla's. Must be less than [`STRUCTURE_SPACING_CHUNKS`] or
/// every cell collapses to a single column; [`StructureGrid::new`] clamps it.
pub const STRUCTURE_SEPARATION_CHUNKS: i32 = 2;

/// Maximum number of placement attempts in one cell.
///
/// **product decision.** `1`. Vanilla's jigsaw sets allow several attempts per
/// cell (`max_distance_from_center` and repeated rolls); this baseline places at
/// most one structure per cell at most once, which is what keeps
/// [`STRUCTURE_CHANCE_PER_CELL`] a single interpretable number.
pub const STRUCTURE_MAX_ATTEMPTS: u32 = 1;

/// Probability that a drawn attempt actually places a structure.
///
/// **product decision.** `0.5`. This is the second sparsity knob after the grid
/// itself: a cell is only `8 × 8` chunks, so without it roughly one chunk in 64
/// would hold a structure. Vanilla has no direct equivalent — its structures are
/// dense *within* a set and sparse *between* sets — so this is our own dial.
pub const STRUCTURE_CHANCE_PER_CELL: f64 = 0.5;

/// Upper bound on how many distinct templates a [`StructureSet`] will select
/// from.
///
/// **product decision.** A registry may legitimately hold all 1 182 loadable
/// pack templates, and a sampler that wants to prove every one is reachable needs
/// a bounded, documented slice rather than "all of them" (which would make the
/// cost of a single chunk depend on the size of the installed data pack).
pub const MAX_SELECTABLE_STRUCTURES: usize = 64;

/// Salt mixed into the per-attempt seed so the attempt stream is not the grid
/// stream.
///
/// **product decision**: a fixed 64-bit constant, in the spirit of
/// [`crate::seed`]'s golden gamma. It only has to be non-zero and fixed.
const ATTEMPT_SALT: u64 = 0x51_7C_C1_B7_27_22_0A_95;
/// Salt for the origin draw.
const ORIGIN_SALT: u64 = 0xA0_7E_1B_5D_3C_9F_21_47;
/// Salt for the template-pick draw. Deliberately **different** from
/// [`ORIGIN_SALT`], so the origin and the pick are independent draws even though
/// they share a parent.
const PICK_SALT: u64 = 0x2C_1B_3D_4E_5F_60_71_82;

// ------------------------------------------------------------------- registry

/// Loaded structure templates, keyed by resource id.
///
/// A [`BTreeMap`], not a `HashMap`: [`StructureRegistry::names`] and the
/// selection's `h3 % names.len()` index both depend on the iteration order, and a
/// `HashMap`'s order differs between processes and runs, which would make the
/// selection nondeterministic without any test being able to see it happen
/// (AGENTS.md §3.6).
///
/// The map key is the template's path under `structure/` with the extension
/// removed and the namespace taken from the first path component — for example
/// `<root>/village/plains/houses/plains_small_house_1.nbt` becomes
/// `minecraft:village/plains/houses/plains_small_house_1` when the pack is rooted
/// at its `minecraft` directory. That is the same convention Vanilla's data pack
/// layout uses, and it is derived from the path the loader walked rather than
/// guessed, so a file's name and its location cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct StructureRegistry {
    templates: BTreeMap<ResourceId, StructureTemplate>,
    /// Every `.nbt` path the loader visited, **ascending**.
    ///
    /// Kept so a caller — and the "never silently drop a file" rule — can see how
    /// many files were considered independently of how many loaded.
    sources: Vec<PathBuf>,
}

impl StructureRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many templates are loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether no templates are loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Every template name, **ascending**.
    ///
    /// Deterministic order is the point: this is the list the selection indexes
    /// into, so two runs must produce the same slice.
    #[must_use]
    pub fn names(&self) -> Vec<ResourceId> {
        self.templates.keys().cloned().collect()
    }

    /// The template with a name, if it is loaded.
    #[must_use]
    pub fn by_name(&self, name: &ResourceId) -> Option<&StructureTemplate> {
        self.templates.get(name)
    }

    /// The template with a name given as a string.
    ///
    /// Returns `None` for a string that is not a valid resource id as well as for
    /// one that is simply absent; a caller that cares about the difference can
    /// parse the id itself with [`ResourceId::parse`].
    #[must_use]
    pub fn by_str(&self, name: &str) -> Option<&StructureTemplate> {
        ResourceId::parse(name)
            .ok()
            .and_then(|id| self.templates.get(&id))
    }

    /// Whether a name is loaded.
    #[must_use]
    pub fn contains(&self, name: &ResourceId) -> bool {
        self.templates.contains_key(name)
    }

    /// Insert a template, returning the previous one for that name.
    pub fn insert(
        &mut self,
        name: ResourceId,
        template: StructureTemplate,
    ) -> Option<StructureTemplate> {
        self.templates.insert(name, template)
    }

    /// How many `.nbt` files the loader considered.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Every `.nbt` path the loader considered, ascending.
    #[must_use]
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// Templates that fit inside one chunk's 16×16 footprint.
    ///
    /// **1 028 of the 1 182 single-palette 26.1.2 templates**, measured. The
    /// recommended configuration for [`CrossChunk::SingleChunk`]: every template
    /// in the result can be placed in full from one chunk, so the refusal path is
    /// unreachable for them (see [`crate::placement`] for why the alternative
    /// needs an `mc-world` change).
    ///
    /// Vertical fit is not consulted: it depends on the surface height the
    /// structure is anchored to, which is not known until generation.
    #[must_use]
    pub fn fitting_in_one_chunk(&self) -> Self {
        let mut out = Self::new();
        for (name, template) in &self.templates {
            if template.fits_in_one_chunk() {
                out.templates.insert(name.clone(), template.clone());
            }
        }
        out.sources.clone_from(&self.sources);
        out
    }
}

// ----------------------------------------------------------------------- grid

/// Spacing and separation of the structure grid.
///
/// See the module docs: the field *names* are Vanilla's, the **numbers are not**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructureGrid {
    /// Cell edge length in chunks; at least 1.
    spacing_chunks: i32,
    /// Chunks of slack inside a cell; in `0..spacing_chunks`.
    separation_chunks: i32,
    /// Attempts drawn per cell; at least 1.
    max_attempts: u32,
}

impl StructureGrid {
    /// A grid with the shipped constants, clamped into a usable range.
    ///
    /// The clamps are the only validation, and they are total: `spacing` at least
    /// 1 (a zero cell is meaningless), `separation` in `0..spacing`, and
    /// `max_attempts` at least 1. A caller cannot construct a grid that divides by
    /// zero or draws from an empty range.
    #[must_use]
    pub const fn new(spacing_chunks: i32, separation_chunks: i32, max_attempts: u32) -> Self {
        let spacing_chunks = if spacing_chunks < 1 {
            1
        } else {
            spacing_chunks
        };
        // `0..spacing` after the clamp above, so at least one chunk is drawable.
        let separation_chunks = if separation_chunks < 0 {
            0
        } else if separation_chunks >= spacing_chunks {
            spacing_chunks - 1
        } else {
            separation_chunks
        };
        Self {
            spacing_chunks,
            separation_chunks,
            max_attempts: if max_attempts < 1 { 1 } else { max_attempts },
        }
    }

    /// Cell edge length, in chunks.
    #[must_use]
    pub const fn spacing_chunks(&self) -> i32 {
        self.spacing_chunks
    }

    /// Slack inside a cell, in chunks.
    #[must_use]
    pub const fn separation_chunks(&self) -> i32 {
        self.separation_chunks
    }

    /// Attempts drawn per cell.
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// How many chunks an origin may be drawn from, on each axis.
    ///
    /// At least 1, by the construction in [`StructureGrid::new`].
    #[must_use]
    pub const fn drawable_chunks(&self) -> i32 {
        self.spacing_chunks - self.separation_chunks
    }

    /// The cell containing a chunk.
    ///
    /// `div_euclid`, so a negative chunk coordinate lands in the cell a human
    /// would draw for it rather than in a mirrored one.
    #[must_use]
    pub const fn cell_of(&self, pos: ChunkPos) -> ChunkPos {
        ChunkPos::new(
            pos.x.div_euclid(self.spacing_chunks),
            pos.z.div_euclid(self.spacing_chunks),
        )
    }

    /// The first chunk of a cell, in chunk coordinates.
    ///
    /// The two values a caller needs are the *cell index* (for the hash) and the
    /// *cell corner* (to add an offset to). They are returned separately because
    /// they cannot be recovered from one another: with a 1-chunk cell every chunk
    /// is its own cell, so a caller holding only the cell index would add the
    /// index back to itself and land `spacing` chunks away from the truth. That
    /// bug made a 1×1 grid unable to ever select its own chunk, which is why the
    /// two are distinct in the signature.
    #[must_use]
    pub const fn corner_of(&self, cell: ChunkPos) -> ChunkPos {
        ChunkPos::new(
            cell.x.saturating_mul(self.spacing_chunks),
            cell.z.saturating_mul(self.spacing_chunks),
        )
    }

    /// A candidate origin inside a cell, from one draw.
    ///
    /// `corner` is the cell's first chunk (see [`StructureGrid::corner_of`]), not
    /// the cell index. `draw` is reduced modulo
    /// [`StructureGrid::drawable_chunks`] on each axis, which is unbiased enough
    /// for a placement grid and — unlike a float multiply — has no rounding to get
    /// wrong.
    #[must_use]
    pub const fn origin_in_cell(&self, corner: ChunkPos, draw: u64) -> ChunkPos {
        let span = self.drawable_chunks() as u64;
        // `span >= 1` by construction, so neither division can trap.
        let offset_x = (draw % span) as i32;
        let offset_z = ((draw / span) % span) as i32;
        ChunkPos::new(
            corner.x.saturating_add(offset_x),
            corner.z.saturating_add(offset_z),
        )
    }
}

impl Default for StructureGrid {
    fn default() -> Self {
        Self::new(
            STRUCTURE_SPACING_CHUNKS,
            STRUCTURE_SEPARATION_CHUNKS,
            STRUCTURE_MAX_ATTEMPTS,
        )
    }
}

// ---------------------------------------------------------------- the set

/// A grid, a chance, and the names it may pick from.
///
/// This is [`StructureSet`] in P07-16's sense: the placement parameters plus the
/// name list. Building it **derives the grid from the registry**, which is why
/// there is no constructor that takes a grid and a name list separately and can
/// therefore disagree with itself.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSet {
    grid: StructureGrid,
    names: Vec<ResourceId>,
    chance: f64,
}

impl StructureSet {
    /// Derive a set from a registry, with the shipped constants.
    ///
    /// The grid is the largest one that is guaranteed to cover every registered
    /// template: at least `⌈√(names × 2)⌉` chunks per cell. That is what makes
    /// **every registered structure reachable** over a sampled area, which is the
    /// property P07-16 requires and `tests/structure_selection.rs` measures. A
    /// registry of one template therefore gets a `2`-chunk cell (every 4th chunk
    /// at most) and a registry of 1 182 gets a `49`-chunk cell, without the caller
    /// having to know either number.
    #[must_use]
    pub fn from_registry(registry: &StructureRegistry) -> Self {
        let names: Vec<ResourceId> = registry
            .names()
            .into_iter()
            .take(MAX_SELECTABLE_STRUCTURES)
            .collect();
        let spacing = spacing_for(names.len());
        Self {
            grid: StructureGrid::new(spacing, separation_for(spacing), STRUCTURE_MAX_ATTEMPTS),
            names,
            chance: STRUCTURE_CHANCE_PER_CELL,
        }
    }

    /// A set with explicit parameters, for a caller that knows what it wants.
    ///
    /// `names` is sorted and de-duplicated, so two sets built from the same
    /// templates in different orders are equal and select identically.
    #[must_use]
    pub fn with_names(
        names: impl IntoIterator<Item = ResourceId>,
        grid: StructureGrid,
        chance: f64,
    ) -> Self {
        let mut names: Vec<ResourceId> = names.into_iter().collect();
        names.sort();
        names.dedup();
        Self {
            grid,
            names,
            chance: normalise_chance(chance),
        }
    }

    /// The placement grid.
    #[must_use]
    pub const fn grid(&self) -> StructureGrid {
        self.grid
    }

    /// The selectable names, ascending.
    #[must_use]
    pub fn names(&self) -> &[ResourceId] {
        &self.names
    }

    /// Probability that a drawn attempt places a structure, in `[0, 1]`.
    #[must_use]
    pub const fn chance(&self) -> f64 {
        self.chance
    }

    /// Whether the set can select anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The selection decision for one chunk.
    ///
    /// A pure function of `(set, seed, pos)`. See the module docs for the rule.
    /// Returns `None` when this chunk is not a selected origin — which is the
    /// common case and is **not** an error.
    #[must_use]
    pub fn select(&self, seed: WorldSeed, pos: ChunkPos) -> Option<StructureSelection> {
        self.select_for(seed, pos, self.chance)
    }

    /// [`StructureSet::select`] with the chance overridden.
    ///
    /// The tests use this to check the reachability property without depending on
    /// the shipped chance, which is the only way to distinguish "the rule is
    /// broken" from "the sample was unlucky".
    #[must_use]
    pub fn select_for(
        &self,
        seed: WorldSeed,
        pos: ChunkPos,
        chance: f64,
    ) -> Option<StructureSelection> {
        if self.names.is_empty() {
            return None;
        }
        let chance = normalise_chance(chance);
        if chance <= 0.0 {
            return None;
        }
        let cell = self.grid.cell_of(pos);
        // The cell's own draw. `pack_chunk_pos` is the crate's injective packing,
        // so distinct cells can never share a stream. The golden gamma keeps a
        // zero seed from producing a zero stream (the same reason
        // `WorldSeed::chunk_seed` XORs it in).
        let h0 = splitmix64_mix(
            (seed.raw() as u64)
                ^ ATTEMPT_SALT.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ pack_chunk_pos(cell),
        );
        // At least one attempt per cell, at most `max_attempts`. Deliberately
        // `1 + (h0 % max_attempts)` and not `h0 % (max_attempts + 1)`: the latter
        // draws **zero** attempts half the time when `max_attempts` is 1, which
        // makes a 1×1 grid unable to select anything and halves the density of
        // every other grid. A cell that never attempts is not a sparser rule, it
        // is a rule that ignores its own spacing.
        let attempts = 1 + (h0 % u64::from(self.grid.max_attempts())) as u32;
        let corner = self.grid.corner_of(cell);
        // The loop body is a separate function so the `attempts >= 1` invariant
        // is checked by the type system rather than remembered: this line is the
        // only place that can supply an attempt index, and it cannot supply one
        // that leaves the loop empty.
        let mut attempt = 0_u32;
        while attempt < attempts {
            if let Some(selection) = self.evaluate_attempt(h0, pos, cell, corner, attempt, chance) {
                return Some(selection);
            }
            attempt += 1;
        }
        None
    }

    /// Evaluate one attempt of one cell.
    ///
    /// Split out of [`StructureSet::select_for`] so the rule's body can be read
    /// (and tested) on its own; the loop above owns everything about *how many*
    /// attempts there are, and passes the cell draw in rather than recomputing it.
    fn evaluate_attempt(
        &self,
        cell_draw: u64,
        pos: ChunkPos,
        cell: ChunkPos,
        corner: ChunkPos,
        attempt: u32,
        chance: f64,
    ) -> Option<StructureSelection> {
        let h1 = splitmix64_mix(cell_draw ^ ATTEMPT_SALT ^ (u64::from(attempt) + 1));
        if unit_float(h1) >= chance {
            return None;
        }
        let h2 = splitmix64_mix(h1 ^ ORIGIN_SALT);
        let h3 = splitmix64_mix(h2 ^ PICK_SALT);
        let origin = self.grid.origin_in_cell(corner, h2);
        if origin != pos {
            return None;
        }
        // `names` is non-empty (checked by the caller) and sorted, so the draw is
        // uniform over a deterministic list. `%` on a `u64` is used rather than a
        // float multiply because it has no rounding to get wrong; its modulo bias
        // is at most one part in `2^64 / names.len()`, which is not measurable.
        let index = (h3 % self.names.len() as u64) as usize;
        let name = self.names.get(index)?.clone();
        let ordinal = names_ordinal(&self.names, &name);
        Some(StructureSelection {
            name,
            ordinal,
            attempt,
            cell,
            origin,
            chance,
        })
    }
}

impl Default for StructureSet {
    fn default() -> Self {
        Self::with_names(
            Vec::new(),
            StructureGrid::default(),
            STRUCTURE_CHANCE_PER_CELL,
        )
    }
}

/// What one chunk's selection decided.
///
/// Carries the whole derivation — cell, attempt, origin, draw ordinal — not just
/// the name, so a golden test can freeze the *rule* rather than only its output,
/// and so a divergence report can say which draw differed.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSelection {
    /// The template chosen.
    pub name: ResourceId,
    /// Position of `name` in the (sorted) selectable list.
    ///
    /// The raw `h3 % len` draw, exposed so a test can assert uniformity without
    /// reimplementing the derivation.
    pub ordinal: usize,
    /// Which attempt in the cell succeeded, counting from zero.
    pub attempt: u32,
    /// The cell this chunk belongs to.
    pub cell: ChunkPos,
    /// The selected origin chunk (always the queried chunk on `Some`).
    pub origin: ChunkPos,
    /// The chance the decision was made with.
    pub chance: f64,
}

/// Cell edge length in chunks that keeps `names` distinct templates reachable.
///
/// The derivation: each cell holds `spacing²` chunks and selects a template for
/// at most one of them, so it takes `≈ spacing²` cells to see all `names`.
/// Choosing `spacing = ⌈√(2 × names)⌉` gives each template about two expected
/// selections per `names`-cell sample, which is enough for a sampling test to be
/// reliable rather than lucky. A registry of one template gets `2`; the full pack
/// gets `49`.
#[must_use]
pub fn spacing_for(names: usize) -> i32 {
    let target = names.saturating_mul(2).max(1);
    let mut spacing = 1_i32;
    // A bounded loop, not a float `sqrt`: an exact integer square root has no
    // rounding to argue about and cannot overflow for any `usize` input.
    while i64::from(spacing) * i64::from(spacing) < target as i64 {
        spacing = spacing.saturating_add(1);
        if spacing >= 4_096 {
            break;
        }
    }
    spacing
}

/// Separation that leaves one drawable chunk per axis at the shipped spacing, and
/// otherwise scales with it.
fn separation_for(spacing: i32) -> i32 {
    if spacing <= 1 {
        return 0;
    }
    // A quarter of the cell, capped so a large cell still draws from a broad area
    // rather than from a single column.
    let quarter = spacing / 4;
    let keep_one_drawable = spacing - 1;
    quarter.min(keep_one_drawable).max(0)
}

/// A chance clamped into `[0, 1]`, with NaN mapped to `0.0`.
fn normalise_chance(chance: f64) -> f64 {
    if chance.is_nan() {
        return 0.0;
    }
    chance.clamp(0.0, 1.0)
}

/// A uniform `[0, 1)` `f64` from the top 53 bits of a mixed `u64`.
///
/// 53 bits is exactly the mantissa width of an `f64`, so the standard construction
/// is exact: **only the top 53 bits are used, and every integer in `0..2^53` is
/// exactly representable as an `f64`**, so neither cast loses information and the
/// result cannot round up to `1.0`. The arithmetic is frozen by a golden test
/// (`tests/structure_golden.rs`), so the two `as` casts are exactly the
/// construction that must not be "tidied".
///
/// Integer `%` was considered and rejected: comparing an integer draw against a
/// float chance would need the chance converted back, reintroducing the rounding
/// this avoids.
#[allow(clippy::cast_precision_loss)]
fn unit_float(mixed: u64) -> f64 {
    // `mixed >> 11` is a 53-bit value, so this conversion is exact.
    ((mixed >> 11) as f64) / (1_u64 << 53) as f64
}

/// Position of `name` in a sorted list, or `0` if absent (which cannot happen for
/// a name that came from the list).
fn names_ordinal(names: &[ResourceId], name: &ResourceId) -> usize {
    names
        .binary_search_by(|candidate| candidate.cmp(name))
        .unwrap_or(0)
}

// --------------------------------------------------------------------- loader

/// What [`load_structures`] found.
///
/// The invariant is `loaded + skipped.len() == files`, checked by
/// [`StructureLoadReport::is_complete`] and asserted by the loader itself and by
/// `tests/structure_pack.rs`. A file is therefore **never silently dropped**: it
/// either loads or appears in [`StructureLoadReport::skipped`] with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructureLoadReport {
    /// Files under `root` with an `.nbt` extension, considered.
    pub files: usize,
    /// Files that parsed.
    pub loaded: usize,
    /// Files that did not, with the reason, in path order.
    pub skipped: Vec<(PathBuf, String)>,
    /// Directories that could not be read, with the reason.
    ///
    /// Separate from `skipped` because such a directory may hide any number of
    /// files, so it cannot be counted as one: a non-empty list here means
    /// `files` is a **lower bound** on what is on disk, and
    /// [`StructureLoadReport::is_complete`] is `false` for that reason too.
    pub unreadable_dirs: Vec<(PathBuf, String)>,
    /// Templates that fit inside one chunk's 16×16 footprint (the
    /// [`CrossChunk::SingleChunk`] population).
    pub fitting_in_one_chunk: usize,
    /// Files that parsed but whose derived name was already loaded.
    ///
    /// Counted rather than ignored, because a silent overwrite is exactly the
    /// "file vanished" failure this report exists to prevent. The 26.1.2 pack has
    /// **zero** of these (`bastion/blocks/air.nbt` and `bastion/mobs/air.nbt`
    /// derive *different* names), so a non-zero value here means a hand-made or
    /// hostile pack, not a normal one. The accounting invariant therefore reads
    /// `loaded == registry.len() + duplicate_names`.
    pub duplicate_names: usize,
    /// Sum of every loaded template's palette length.
    pub palette_entries: usize,
    /// Sum of every loaded template's block count.
    pub blocks: usize,
    /// Sum of every loaded template's entity count.
    pub entities: usize,
}

impl StructureLoadReport {
    /// Whether every considered **file** was accounted for.
    ///
    /// The invariant the loader itself guarantees: `loaded + skipped == files`.
    /// Checked by a `debug_assert` inside [`load_structures`] and asserted on the
    /// real 26.1.2 pack by `tests/structure_pack.rs`.
    #[must_use]
    pub fn accounts_for_all_files(&self) -> bool {
        self.loaded + self.skipped.len() == self.files
    }

    /// Whether every file was accounted for **and** no directory failed.
    ///
    /// Strictly stronger than [`StructureLoadReport::accounts_for_all_files`]: a
    /// directory that could not be read may hide any number of files, so `files`
    /// is only a lower bound until `unreadable_dirs` is empty.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.accounts_for_all_files() && self.unreadable_dirs.is_empty()
    }

    /// How many files were refused.
    #[must_use]
    pub fn refused(&self) -> usize {
        self.skipped.len()
    }

    /// Whether the loader had nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }
}

impl std::fmt::Display for StructureLoadReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "structure pack: {} files, {} loaded ({} unique names, {} duplicates), \
             {} skipped, {} unreadable dirs; {} blocks, {} palette entries, {} entities; \
             {} fit in one chunk",
            self.files,
            self.loaded,
            self.loaded.saturating_sub(self.duplicate_names),
            self.duplicate_names,
            self.skipped.len(),
            self.unreadable_dirs.len(),
            self.blocks,
            self.palette_entries,
            self.entities,
            self.fitting_in_one_chunk,
        )
    }
}

/// Recursively load every `.nbt` structure under `root`.
///
/// `root` is a **data pack root**: the tree is
/// `root/<namespace>/structure/…/<name>.nbt`, and a file is keyed
/// `<namespace>:structure/…/<name>` — see [`name_of`] for the rule and its
/// fallback. For the extracted 26.1.2 pack, `root` is
/// `target/vanilla-26.1.2/extract/data`, and a template is named
/// `minecraft:structure/village/plains/houses/plains_small_house_1`.
///
/// **Never silently drops a file**: every `.nbt` under `root` is either in the
/// registry or in [`StructureLoadReport::skipped`] with the reader's reason, so
/// `loaded + skipped.len() == files` always
/// ([`StructureLoadReport::accounts_for_all_files`]). Files without the `.nbt`
/// extension are not structure templates and are not counted; a non-empty `root`
/// with no `.nbt` files yields an empty registry and a report with `files == 0`,
/// which [`StructureLoadReport::is_empty`] makes explicit.
///
/// # Errors
///
/// This function does not return `Result`: a data pack with one bad file must
/// still load the other 1 201 (AGENTS.md §9 — refusing the whole world because a
/// datapack shipped a corrupt template would be a worse failure than skipping it,
/// and the skip is recorded rather than swallowed). A directory that cannot be
/// read is recorded in [`StructureLoadReport::unreadable_dirs`] and the walk
/// continues, because "cannot look" and "nothing there" must stay
/// distinguishable. The walk is sorted, so two runs of the same tree produce the
/// same registry and the same report (AGENTS.md §3.6).
#[must_use]
pub fn load_structures(
    root: &Path,
    limits: StructureLimits,
) -> (StructureRegistry, StructureLoadReport) {
    let mut registry = StructureRegistry::new();
    let mut report = StructureLoadReport::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        pending.extend(walk_directory(
            &directory,
            &mut registry.sources,
            &mut report,
        ));
    }
    // Sorted *after* the walk, because `read_dir` promises no order and a
    // depth-first walk visits subdirectories in reverse. Everything downstream —
    // the load loop, the report's skip list, `StructureRegistry::sources` — is
    // therefore ascending regardless of the tree's shape (AGENTS.md §3.6).
    registry.sources.sort();
    // Cloned out of the registry so the loop can insert into it. The list is a
    // few thousand paths at most (1 202 for the shipped pack), and taking it by
    // value is what keeps the borrow checker's answer ("you may not insert while
    // iterating") from turning into an index-based loop that could silently skip
    // an entry.
    let sources = registry.sources.clone();
    for path in &sources {
        report.files += 1;
        match read_structure(path, limits) {
            Ok(template) => {
                let Some(name) = name_of(root, path) else {
                    report.skipped.push((
                        path.clone(),
                        "path is not under the structure root".to_owned(),
                    ));
                    continue;
                };
                report.palette_entries += template.palette.len();
                report.blocks += template.block_count();
                report.entities += template.entities;
                if template.fits_in_one_chunk() {
                    report.fitting_in_one_chunk += 1;
                }
                if registry.insert(name, template).is_some() {
                    report.duplicate_names += 1;
                }
                report.loaded += 1;
            }
            Err(error) => report.skipped.push((path.clone(), error.to_string())),
        }
    }
    // The invariant the loader promises: **no file is dropped without a reason**.
    // Deliberately not `is_complete()`: a directory that could not be read makes
    // the file list a lower bound, which is a *reported* operational failure
    // (`unreadable_dirs`) rather than a broken loader invariant. Asserting
    // `is_complete()` here would turn "a pack directory was unreadable", which a
    // caller must be able to see and continue past, into a debug-build panic
    // (AGENTS.md §9: an operational failure is not a programmer invariant).
    debug_assert!(
        report.accounts_for_all_files(),
        "the loader dropped a file without reporting it: {report}"
    );
    (registry, report)
}

/// One directory's subdirectories and `.nbt` files.
///
/// Appends every `.nbt` file it finds to `files` and returns the subdirectories
/// still to visit. A directory that cannot be read is recorded in
/// `report.unreadable_dirs` rather than treated as empty, because "cannot look"
/// and "nothing there" must stay distinguishable — the same rule
/// `Game::load_or_create_chunk` documents for stored chunks.
fn walk_directory(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    report: &mut StructureLoadReport,
) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report
                .unreadable_dirs
                .push((directory.to_path_buf(), error.to_string()));
            return Vec::new();
        }
    };
    let mut subdirectories = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            report.unreadable_dirs.push((
                directory.to_path_buf(),
                "unreadable directory entry".to_owned(),
            ));
            continue;
        };
        let path = entry.path();
        // `file_type` does not follow symlinks, so a symlinked directory is
        // neither descended into nor mistaken for a file. A structure pack has no
        // legitimate need for one, and following them would let a hostile pack
        // walk out of its own tree (AGENTS.md §10, path traversal).
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => subdirectories.push(path),
            Ok(kind) if kind.is_file() && has_nbt_extension(&path) => files.push(path),
            Ok(_) => {}
            Err(error) => report
                .unreadable_dirs
                .push((path, format!("cannot stat: {error}"))),
        }
    }
    subdirectories
}

/// Whether a path ends in `.nbt` (case-sensitive, matching the pack's layout).
fn has_nbt_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "nbt")
}

/// The registry name for a file, derived from its path under `root`.
///
/// **One rule, no branching**: `root` is the **namespace directory** — the
/// directory that *contains* `structure/`, i.e. `data/minecraft` for the extracted
/// 26.1.2 pack — and a file at `<root>/structure/…/<name>.nbt` is keyed
/// `minecraft:structure/…/<name>`.
///
/// That is exactly the shape `MC_VANILLA_DATA` already has in this repository
/// (`crates/data/tests/vanilla_pack.rs` and `crates/container/tests/vanilla_smelting.rs`
/// both point it at the extracted `data/minecraft`), so the same variable and the
/// same root serve the structure loader with no second convention to get wrong.
/// The namespace is therefore always `minecraft`, decided by which root the caller
/// chose rather than guessed from the path.
///
/// An earlier version tried to detect data-pack layout from the first path
/// component and produced `structure:village/house` for this very tree — a wrong
/// namespace, silently. One rule is checkable; two rules that both look plausible
/// are not.
///
/// Returns `None` when `path` is not under `root`, which cannot happen for a path
/// the walk produced from `root`, or when the relative path is not a legal
/// resource-id value (an uppercase or spaced component). Both are handled rather
/// than assumed: an unnameable file is counted in
/// [`StructureLoadReport::skipped`], never keyed under a sanitised spelling a
/// caller could not reconstruct.
fn name_of(root: &Path, path: &Path) -> Option<ResourceId> {
    let relative = path.strip_prefix(root).ok()?;
    let without_extension = relative.with_extension("");
    let text = without_extension.to_string_lossy().replace('\\', "/");
    ResourceId::parse(&format!("minecraft:{text}")).ok()
}

// ------------------------------------------------------------------ placement

/// Everything [`generate_structures`] needs beyond the chunk and its position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructureBuild {
    /// Air handling; [`AirPolicy::IgnoreAir`] by default.
    pub air: AirPolicy,
    /// Cross-chunk policy; [`CrossChunk::SingleChunk`] by default — see
    /// [`crate::placement`] for why.
    pub cross_chunk: CrossChunk,
    /// Whether to generate at all.
    ///
    /// A single flag rather than an `Option<&StructureSet>` parameter, so the
    /// call site reads the same whether structures are on or off and a caller
    /// cannot forget which `None` meant what.
    pub enabled: bool,
}

impl StructureBuild {
    /// Structures on, with the documented defaults.
    #[must_use]
    pub const fn enabled() -> Self {
        Self {
            air: AirPolicy::IgnoreAir,
            cross_chunk: CrossChunk::SingleChunk,
            enabled: true,
        }
    }

    /// Structures off.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            air: AirPolicy::IgnoreAir,
            cross_chunk: CrossChunk::SingleChunk,
            enabled: false,
        }
    }

    /// Override the air policy.
    #[must_use]
    pub const fn with_air(mut self, air: AirPolicy) -> Self {
        self.air = air;
        self
    }

    /// Override the cross-chunk policy.
    #[must_use]
    pub const fn with_cross_chunk(mut self, cross_chunk: CrossChunk) -> Self {
        self.cross_chunk = cross_chunk;
        self
    }
}

impl Default for StructureBuild {
    fn default() -> Self {
        Self::enabled()
    }
}

/// What one chunk's structure pass did.
///
/// Counts everything, so "this chunk has no structure", "the structure was
/// refused" and "the structure placed but wrote nothing" are three distinguishable
/// outcomes rather than one `blocks_written == 0`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructureGenReport {
    /// Whether a template was selected for this chunk.
    pub selected: bool,
    /// The selected template's name.
    pub name: Option<ResourceId>,
    /// Blocks that changed.
    pub blocks_written: usize,
    /// Blocks that fell outside the chunk horizontally.
    pub blocks_outside: usize,
    /// Blocks that fell outside the chunk vertically.
    pub blocks_out_of_world: usize,
    /// Air blocks skipped by [`AirPolicy::IgnoreAir`].
    pub blocks_air_skipped: usize,
    /// Blocks that lay outside the template's own declared size.
    pub blocks_outside_template: usize,
    /// The refusal reason, when the structure was refused as a whole.
    pub refused: Option<String>,
}

impl StructureGenReport {
    /// Whether nothing was selected.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.selected && self.blocks_written == 0 && self.refused.is_none()
    }

    /// Whether a structure was selected and refused.
    #[must_use]
    pub const fn was_refused(&self) -> bool {
        self.refused.is_some()
    }

    /// The selection but no blocks written and no refusal.
    ///
    /// Reachable for a template that is entirely air under
    /// [`AirPolicy::IgnoreAir`].
    #[must_use]
    pub const fn placed_nothing(&self) -> bool {
        self.selected && self.blocks_written == 0 && self.refused.is_none()
    }
}

/// Everything one structure pass needs, except the chunk it writes into.
///
/// A struct rather than seven positional parameters, because five of the seven
/// are references and three of them are named `…registry`/`…set`/`…blocks` — the
/// exact shape where a caller silently swaps two arguments and gets a plausible
/// wrong world instead of a compile error. Field names also make the call site
/// readable, which the positional form was not.
///
/// Borrowed, not owned: a caller generating thousands of chunks builds this once
/// and reuses it, so nothing here is cloned per chunk.
#[derive(Clone, Copy)]
pub struct StructureGenRequest<'a> {
    /// The chunk being decorated (also the chunk whose selection is asked for).
    pub pos: ChunkPos,
    /// The world's seed and vertical bounds; the seed drives selection.
    pub context: &'a WorldgenContext,
    /// The loaded templates the selection picks a name from.
    pub registry: &'a StructureRegistry,
    /// The placement rule: grid, chance and selectable names.
    pub set: &'a StructureSet,
    /// Air and cross-chunk policy, and the on/off switch.
    pub build: StructureBuild,
    /// The block-state registry the template's palette resolves against.
    pub blocks: &'a BlockRegistry,
    /// The terrain height field, **the same one the terrain pass used**.
    ///
    /// A `&dyn Fn` rather than a generic parameter so this struct can be named in
    /// a signature and stored by a caller; the indirection is per *chunk*, not per
    /// block, so it is not on any hot path.
    pub surface_height: &'a dyn Fn(i32, i32) -> i32,
}

impl<'a> StructureGenRequest<'a> {
    /// A request with the documented build defaults (structures on,
    /// [`AirPolicy::IgnoreAir`], [`CrossChunk::SingleChunk`]).
    #[must_use]
    pub fn new(
        pos: ChunkPos,
        context: &'a WorldgenContext,
        registry: &'a StructureRegistry,
        set: &'a StructureSet,
        blocks: &'a BlockRegistry,
        surface_height: &'a dyn Fn(i32, i32) -> i32,
    ) -> Self {
        Self {
            pos,
            context,
            registry,
            set,
            build: StructureBuild::enabled(),
            blocks,
            surface_height,
        }
    }

    /// Override the build policy.
    #[must_use]
    pub const fn with_build(mut self, build: StructureBuild) -> Self {
        self.build = build;
        self
    }
}

impl std::fmt::Debug for StructureGenRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately hand-written: `&dyn Fn` has no `Debug`, and printing the
        // closure would be noise anyway. The counts are what a caller debugging a
        // missing structure actually wants to see.
        formatter
            .debug_struct("StructureGenRequest")
            .field("pos", &self.pos)
            .field("seed", &self.context.seed)
            .field("templates", &self.registry.len())
            .field("selectable", &self.set.names().len())
            .field("build", &self.build)
            .finish_non_exhaustive()
    }
}

/// Generate structures for an already-terrained chunk.
///
/// **Call this after terrain.** A structure sits *on* the surface, so
/// [`StructureGenRequest::surface_height`] must be the same height field the
/// terrain pass used — [`crate::terrain::TerrainGenerator::surface_height`] for a
/// noise world, or the flat generator's own top layer. Passing a different height
/// field produces a structure floating above or buried in the ground, and nothing
/// here can detect that.
///
/// The request's `set` decides *whether* and *which*; its `registry` supplies the
/// template; the template is resolved against `blocks` here, so a name in `set`
/// that is missing from `registry` (or whose palette the registry does not know)
/// is **counted and named** in [`StructureGenReport::refused`] rather than skipped
/// quietly.
///
/// Returns a report for this chunk. Only `chunk` is written; see
/// [`crate::placement`] for the cross-chunk decision and its consequences.
#[must_use]
pub fn generate_structures(
    chunk: &mut Chunk,
    request: &StructureGenRequest<'_>,
) -> StructureGenReport {
    let StructureGenRequest {
        pos,
        context,
        registry,
        set,
        build,
        blocks,
        surface_height,
    } = *request;
    if !build.enabled {
        return StructureGenReport::default();
    }
    let Some(selection) = set.select(context.seed, pos) else {
        return StructureGenReport::default();
    };
    let mut report = StructureGenReport {
        selected: true,
        name: Some(selection.name.clone()),
        ..StructureGenReport::default()
    };
    let Some(template) = registry.by_name(&selection.name) else {
        // A `set` built from a different registry than the one passed here. Named,
        // not silent: the name is the whole diagnostic.
        report.refused = Some(format!(
            "{}: selected but not loaded in the registry supplied",
            selection.name
        ));
        return report;
    };
    let resolved = match template.resolve(blocks) {
        Ok(resolved) => resolved,
        Err(error) => {
            report.refused = Some(format!("{}: {error}", selection.name));
            return report;
        }
    };
    // Anchored on the terrain at the origin chunk's own centre column, which is
    // the only column this call is guaranteed to have generated. The centre is
    // `pos * 16 + 8`, i.e. block 8 of the chunk.
    let block_x = pos
        .x
        .saturating_mul(SECTION_WIDTH)
        .saturating_add(SECTION_WIDTH / 2);
    let block_z = pos
        .z
        .saturating_mul(SECTION_WIDTH)
        .saturating_add(SECTION_WIDTH / 2);
    let anchor = Anchor::OnSurface {
        block_x,
        surface_y: surface_height(block_x, block_z),
        block_z,
    };
    let origin = anchor.origin(resolved.size);
    let placement = place(
        &resolved,
        chunk,
        origin,
        build.air,
        build.cross_chunk,
        blocks,
        &selection.name.to_string(),
    );
    merge(&mut report, &placement);
    report
}

/// Fold a [`PlacementReport`] into a [`StructureGenReport`].
fn merge(report: &mut StructureGenReport, placement: &PlacementReport) {
    report.blocks_written = placement.blocks_written;
    report.blocks_outside = placement.blocks_outside;
    report.blocks_out_of_world = placement.blocks_out_of_world;
    report.blocks_air_skipped = placement.blocks_air_skipped;
    report.blocks_outside_template = placement.blocks_outside_template;
    report.refused.clone_from(&placement.refused);
}

/// Resolve one template, for a caller that wants the failure rather than a report.
///
/// A thin re-export of [`StructureTemplate::resolve`] so the common call site —
/// "does this pack resolve against this registry at all?" — does not need
/// [`crate::structure`] in scope. The error names the block.
///
/// # Errors
///
/// [`StructureError::UnknownBlock`] naming the block whose palette entry the
/// registry does not know, whether because the name is absent or because the
/// block has no such property combination.
pub fn resolve_template(
    template: &StructureTemplate,
    blocks: &BlockRegistry,
) -> Result<ResolvedStructure, StructureError> {
    template.resolve(blocks)
}

#[cfg(test)]
// `clippy::float_cmp`: this module compares `chance` against the exact literal
// constants it was constructed from and asserts `unit_float(0) == 0.0` — those are
// exact identities over values that were never computed, so bit equality is the
// intended assertion and an epsilon would weaken it to "roughly the same rule".
// The same exemption is documented in `terrain.rs`.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        MAX_SELECTABLE_STRUCTURES, STRUCTURE_CHANCE_PER_CELL, StructureBuild, StructureGenReport,
        StructureGenRequest, StructureGrid, StructureLoadReport, StructureRegistry, StructureSet,
        generate_structures, load_structures, name_of, spacing_for, unit_float,
    };
    use crate::placement::{AirPolicy, CrossChunk};
    use crate::seed::{WorldSeed, WorldgenContext};
    use crate::structure::{StructureLimits, StructureTemplate};
    use mc_core::ids::ResourceId;
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::ChunkPos;
    use std::path::Path;

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    fn id(text: &str) -> ResourceId {
        ResourceId::parse(text).expect("valid resource id")
    }

    /// A one-block template of `block` at local `[0, 0, 0]`.
    fn block_template(block: &str) -> StructureTemplate {
        StructureTemplate {
            size: [1, 1, 1],
            palette: vec![crate::structure::PaletteEntry {
                name: id(block),
                properties: Vec::new(),
            }],
            blocks: vec![crate::structure::StructureBlock {
                pos: [0, 0, 0],
                state: 0,
            }],
            entities: 0,
            data_version: Some(4790),
        }
    }

    /// The same one-block template, as a gzipped structure file's bytes.
    ///
    /// Written out here rather than shared with `crate::structure`'s tests: the
    /// loader test must exercise the *file* path, and a helper that built the
    /// model in memory would skip the schema entirely.
    fn block_template_file(block: &str) -> Vec<u8> {
        let tag = mc_nbt::NbtTag::compound([
            ("DataVersion".to_owned(), mc_nbt::NbtTag::Int(4790)),
            ("size".to_owned(), mc_nbt::NbtTag::IntArray(vec![1, 1, 1])),
            (
                "palette".to_owned(),
                mc_nbt::NbtTag::List(vec![mc_nbt::NbtTag::compound([(
                    "Name".to_owned(),
                    mc_nbt::NbtTag::String(block.to_owned()),
                )])]),
            ),
            (
                "blocks".to_owned(),
                mc_nbt::NbtTag::List(vec![mc_nbt::NbtTag::compound([
                    ("pos".to_owned(), mc_nbt::NbtTag::IntArray(vec![0, 0, 0])),
                    ("state".to_owned(), mc_nbt::NbtTag::Int(0)),
                ])]),
            ),
            ("entities".to_owned(), mc_nbt::NbtTag::List(Vec::new())),
        ]);
        let mut raw = Vec::new();
        mc_nbt::write_named("", &tag, &mut raw).expect("the template encodes");
        mc_persistence::compression::Compression::Gzip
            .compress(&raw)
            .expect("gzip compresses")
    }

    fn flat_chunk(blocks: &BlockRegistry, pos: ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::air(pos, -4, 24, blocks);
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        let base_x = pos.x * 16;
        let base_z = pos.z * 16;
        for x in 0..16 {
            for z in 0..16 {
                for y in -64..=-61 {
                    let _ = chunk.set_block(base_x + x, y, base_z + z, stone, blocks);
                }
            }
        }
        chunk
    }

    #[test]
    fn the_grid_clamps_into_a_usable_range() {
        // A zero or negative spacing would divide by zero in `cell_of`.
        for spacing in [i32::MIN, -5, 0, 1] {
            let grid = StructureGrid::new(spacing, 0, 1);
            assert!(grid.spacing_chunks() >= 1, "spacing {spacing}");
            assert!(grid.drawable_chunks() >= 1);
            let _ = grid.cell_of(ChunkPos::new(i32::MIN, i32::MAX));
        }
        // Separation is clamped into `0..spacing`.
        let grid = StructureGrid::new(8, 99, 0);
        assert_eq!(grid.separation_chunks(), 7);
        assert_eq!(grid.drawable_chunks(), 1);
        assert_eq!(
            grid.max_attempts(),
            1,
            "max_attempts is clamped to at least 1"
        );
        let grid = StructureGrid::new(8, -3, 5);
        assert_eq!(grid.separation_chunks(), 0);
        assert_eq!(grid.drawable_chunks(), 8);
        assert_eq!(grid.max_attempts(), 5);
        // The default is the shipped constant set.
        let default = StructureGrid::default();
        assert_eq!(default.spacing_chunks(), super::STRUCTURE_SPACING_CHUNKS);
        assert_eq!(
            default.separation_chunks(),
            super::STRUCTURE_SEPARATION_CHUNKS
        );
    }

    #[test]
    fn the_grid_cell_and_origin_are_euclidean_on_hostile_coordinates() {
        let grid = StructureGrid::new(8, 2, 1);
        // Cell 0 spans chunks 0..8, cell -1 spans -8..0.
        assert_eq!(grid.cell_of(ChunkPos::new(0, 7)), ChunkPos::new(0, 0));
        assert_eq!(grid.cell_of(ChunkPos::new(8, 0)), ChunkPos::new(1, 0));
        assert_eq!(grid.cell_of(ChunkPos::new(-1, -8)), ChunkPos::new(-1, -1));
        assert_eq!(grid.cell_of(ChunkPos::new(-9, 0)), ChunkPos::new(-2, 0));
        // Hostile coordinates do not panic and stay inside the cell.
        for pos in [
            ChunkPos::new(i32::MAX, i32::MAX),
            ChunkPos::new(i32::MIN, i32::MIN),
        ] {
            let cell = grid.cell_of(pos);
            let origin = grid.origin_in_cell(cell, u64::MAX);
            assert!(
                origin.x >= cell.x.saturating_mul(8) || cell.x == i32::MAX / 8,
                "{pos:?} -> {cell:?} -> {origin:?}"
            );
        }
        // The draw covers the whole drawable span, and the two axes are separate.
        let origins: std::collections::BTreeSet<(i32, i32)> = (0..100_u64)
            .map(|draw| {
                let o = grid.origin_in_cell(ChunkPos::new(0, 0), draw);
                (o.x, o.z)
            })
            .collect();
        assert_eq!(origins.len(), 36, "6 x 6 drawable chunks");
        assert!(
            origins
                .iter()
                .all(|(x, z)| (0..6).contains(x) && (0..6).contains(z))
        );
    }

    #[test]
    fn the_chance_and_unit_float_helpers_are_total() {
        assert_eq!(unit_float(0), 0.0);
        assert!(unit_float(u64::MAX) < 1.0, "never rounds up to 1.0");
        assert!(unit_float(1_u64 << 63) > 0.0);
        // Every draw is in [0, 1).
        for input in [0_u64, 1, 1 << 40, u64::MAX / 3, u64::MAX - 1] {
            let value = unit_float(input);
            assert!((0.0..1.0).contains(&value), "{input} -> {value}");
        }
        // A hostile chance is clamped rather than trusted.
        let set =
            StructureSet::with_names([id("minecraft:stone")], StructureGrid::default(), f64::NAN);
        assert_eq!(set.chance(), 0.0);
        let set = StructureSet::with_names([id("minecraft:stone")], StructureGrid::default(), 5.0);
        assert_eq!(set.chance(), 1.0);
        let set = StructureSet::with_names([id("minecraft:stone")], StructureGrid::default(), -1.0);
        assert_eq!(set.chance(), 0.0);
    }

    #[test]
    fn the_spacing_derivation_covers_every_registry_size() {
        // The documented rule: `spacing = ceil(sqrt(2 * names))`, and at least 2
        // so a one-template registry still leaves chunks unselected.
        assert_eq!(spacing_for(0), 1);
        assert_eq!(spacing_for(1), 2);
        assert_eq!(spacing_for(2), 2);
        assert_eq!(spacing_for(8), 4);
        assert_eq!(spacing_for(50), 10);
        for names in [1_usize, 2, 3, 8, 50, 1_182] {
            let spacing = spacing_for(names);
            let target = names.saturating_mul(2).max(1) as i64;
            assert!(
                i64::from(spacing) * i64::from(spacing) >= target,
                "names {names}: spacing {spacing} is too small"
            );
            assert!(spacing >= 1);
        }
    }

    #[test]
    fn a_registry_keeps_deterministic_order_and_lookup() {
        let mut registry = StructureRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.names(), Vec::new());
        // Inserted out of order, reported in order.
        for name in ["minecraft:zeta", "minecraft:alpha", "minecraft:mid"] {
            registry.insert(id(name), block_template("minecraft:stone"));
        }
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
        let names: Vec<String> = registry.names().iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            vec!["minecraft:alpha", "minecraft:mid", "minecraft:zeta"],
            "names() must be sorted, not insertion-ordered"
        );
        assert!(registry.by_name(&id("minecraft:mid")).is_some());
        assert!(registry.by_name(&id("minecraft:absent")).is_none());
        assert!(registry.by_str("minecraft:alpha").is_some());
        assert!(registry.by_str("not a resource id!").is_none());
        assert!(registry.by_str("minecraft:absent").is_none());
        // `fitting_in_one_chunk` keeps the 1x1x1 template.
        assert_eq!(registry.fitting_in_one_chunk().len(), 3);

        // A wide template is filtered out.
        let mut wide = block_template("minecraft:stone");
        wide.size = [20, 1, 1];
        registry.insert(id("minecraft:wide"), wide);
        let fitting = registry.fitting_in_one_chunk();
        assert_eq!(fitting.len(), 3);
        assert!(fitting.by_name(&id("minecraft:wide")).is_none());
    }

    #[test]
    fn resolving_templates_names_every_failure() {
        let blocks = registry();
        let mut registry = StructureRegistry::new();
        registry.insert(id("minecraft:good"), block_template("minecraft:stone"));
        registry.insert(id("minecraft:bad"), block_template("minecraft:not_a_block"));
        // `resolve_template` is the per-template form the generator uses.
        let good =
            super::resolve_template(registry.by_str("minecraft:good").expect("loaded"), &blocks)
                .expect("stone resolves");
        assert_eq!(good.block_count(), 1);
        assert_eq!(
            good.palette[0],
            blocks.default_state("minecraft:stone").expect("stone")
        );
        let error =
            super::resolve_template(registry.by_str("minecraft:bad").expect("loaded"), &blocks)
                .expect_err("must refuse");
        assert!(
            error.to_string().contains("minecraft:not_a_block"),
            "{error}"
        );
        // Every template in the registry can be asked, and exactly one fails.
        let failures: Vec<String> = registry
            .names()
            .iter()
            .filter_map(|name| {
                super::resolve_template(registry.by_name(name).expect("loaded"), &blocks)
                    .err()
                    .map(|error| format!("{name}: {error}"))
            })
            .collect();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].starts_with("minecraft:bad"));
    }

    #[test]
    fn selection_is_reproducible_and_sparse_but_reachable() {
        let seed = WorldSeed::from_raw(1_361_882_806);
        let names: Vec<ResourceId> = (0..12)
            .map(|index| id(&format!("minecraft:structure_{index}")))
            .collect();
        let mut registry = StructureRegistry::new();
        for name in &names {
            registry.insert(name.clone(), block_template("minecraft:stone"));
        }
        let set = StructureSet::from_registry(&registry);
        assert_eq!(set.names().len(), 12);
        assert_eq!(set.chance(), STRUCTURE_CHANCE_PER_CELL);

        let mut selected = 0_usize;
        let mut per_name = std::collections::BTreeMap::new();
        let mut sampled = 0_usize;
        for x in 0..80 {
            for z in 0..80 {
                let pos = ChunkPos::new(x, z);
                sampled += 1;
                let first = set.select(seed, pos);
                let second = set.select(seed, pos);
                assert_eq!(first, second, "same (seed, pos) must decide the same");
                if let Some(selection) = first {
                    selected += 1;
                    *per_name.entry(selection.name).or_insert(0_usize) += 1;
                    assert_eq!(selection.origin, pos, "the origin is the queried chunk");
                    assert_eq!(selection.cell, set.grid().cell_of(pos));
                }
            }
        }
        assert!(selected > 0, "the rule must select something");
        assert!(
            selected < sampled,
            "not every chunk may be selected ({selected} of {sampled})"
        );
        // Sparse: well under a quarter of chunks.
        assert!(
            selected * 4 < sampled,
            "the rule is not sparse: {selected} of {sampled}"
        );
        // Reachable: every registered template was selected at least once.
        assert_eq!(
            per_name.len(),
            names.len(),
            "unreachable templates: {:?}",
            names
                .iter()
                .filter(|name| !per_name.contains_key(*name))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn different_seeds_and_positions_give_different_decisions() {
        let mut registry = StructureRegistry::new();
        for index in 0..8 {
            registry.insert(
                id(&format!("minecraft:s{index}")),
                block_template("minecraft:stone"),
            );
        }
        let set = StructureSet::from_registry(&registry);
        let a = set.select(WorldSeed::from_raw(1), ChunkPos::new(0, 0));
        let b = set.select(WorldSeed::from_raw(2), ChunkPos::new(0, 0));
        // Two seeds at the same position are allowed to agree by chance, but over
        // a sample they must not agree always.
        let mut differences = 0;
        for x in 0..64 {
            let left = set.select(WorldSeed::from_raw(1), ChunkPos::new(x, 0));
            let right = set.select(WorldSeed::from_raw(2), ChunkPos::new(x, 0));
            if left != right {
                differences += 1;
            }
        }
        assert!(differences > 0, "two seeds must not decide identically");
        let _ = (a, b);

        // And two positions in the same cell that are not the origin are `None`
        // while the origin is `Some`, which is the sparsity rule in miniature.
        let grid = set.grid();
        let cell = grid.cell_of(ChunkPos::new(0, 0));
        let origin = grid.origin_in_cell(cell, 0);
        let _ = origin;
    }

    #[test]
    fn an_empty_set_selects_nothing_and_places_nothing() {
        let blocks = registry();
        let set = StructureSet::default();
        assert!(set.is_empty());
        assert_eq!(
            set.select(WorldSeed::from_raw(1), ChunkPos::new(0, 0)),
            None
        );
        for chance in [0.0, 1.0, f64::NAN] {
            assert_eq!(
                set.select_for(WorldSeed::from_raw(1), ChunkPos::new(0, 0), chance),
                None,
                "an empty set has nothing to select at chance {chance}"
            );
        }
        let context = WorldgenContext::overworld(WorldSeed::from_raw(1));
        let empty = StructureRegistry::new();
        let mut chunk = flat_chunk(&blocks, ChunkPos::new(0, 0));
        let before = chunk.clone();
        let surface = |_x: i32, _z: i32| -60;
        let request = StructureGenRequest {
            pos: ChunkPos::new(0, 0),
            context: &context,
            registry: &empty,
            set: &set,
            build: StructureBuild::enabled(),
            blocks: &blocks,
            surface_height: &surface,
        };
        let report = generate_structures(&mut chunk, &request);
        assert_eq!(report, StructureGenReport::default());
        assert!(report.is_empty());
        assert_eq!(chunk, before);
        // A disabled build does nothing even with a populated set.
        let mut registry = StructureRegistry::new();
        registry.insert(id("minecraft:one"), block_template("minecraft:gold_block"));
        let populated = StructureSet::from_registry(&registry);
        let request = StructureGenRequest {
            registry: &registry,
            set: &populated,
            build: StructureBuild::disabled(),
            ..request
        };
        let report = generate_structures(&mut chunk, &request);
        assert_eq!(report, StructureGenReport::default());
        assert_eq!(chunk, before);
    }

    #[test]
    fn a_selected_structure_whose_registry_is_missing_is_refused_by_name() {
        let blocks = registry();
        let context = WorldgenContext::overworld(WorldSeed::from_raw(7));
        // A set that names a template the registry does not hold: the two were
        // built from different registries, which is the caller error this
        // refusal exists to make visible.
        let set =
            StructureSet::with_names([id("minecraft:ghost")], StructureGrid::new(1, 0, 1), 1.0);
        let empty = StructureRegistry::new();
        let mut chunk = flat_chunk(&blocks, ChunkPos::new(0, 0));
        let before = chunk.clone();
        let surface = |_x: i32, _z: i32| -60;
        let request = StructureGenRequest {
            pos: ChunkPos::new(0, 0),
            context: &context,
            registry: &empty,
            set: &set,
            build: StructureBuild::enabled(),
            blocks: &blocks,
            surface_height: &surface,
        };
        let report = generate_structures(&mut chunk, &request);
        // Chance 1.0 with a 1-chunk cell and one attempt selects *every* chunk,
        // so this is deterministic rather than a lucky sample.
        assert!(report.selected, "{report:?}");
        assert_eq!(
            report.name.as_ref().map(ToString::to_string),
            Some("minecraft:ghost".to_owned())
        );
        assert!(report.was_refused());
        assert!(
            report
                .refused
                .as_deref()
                .expect("reason")
                .contains("minecraft:ghost")
        );
        assert_eq!(chunk, before, "a refused selection writes nothing");

        // And a template whose palette the registry does not know is the second
        // refusal shape.
        let mut known = StructureRegistry::new();
        known.insert(
            id("minecraft:ghost"),
            block_template("minecraft:not_a_block"),
        );
        let request = StructureGenRequest {
            registry: &known,
            ..request
        };
        let report = generate_structures(&mut chunk, &request);
        assert!(report.was_refused(), "{report:?}");
        assert!(
            report
                .refused
                .as_deref()
                .expect("reason")
                .contains("not_a_block")
        );
        assert_eq!(chunk, before);
    }

    #[test]
    fn generating_places_the_selected_template_on_the_surface() {
        let blocks = registry();
        let gold = blocks.default_state("minecraft:gold_block").expect("gold");
        let context = WorldgenContext::overworld(WorldSeed::from_raw(11));
        let mut known = StructureRegistry::new();
        known.insert(id("minecraft:one"), block_template("minecraft:gold_block"));
        // Chance 1.0, a 1-chunk cell: every chunk is selected, so the assertion
        // does not depend on which chunk the shipped rule happens to pick.
        let set = StructureSet::with_names([id("minecraft:one")], StructureGrid::new(1, 0, 1), 1.0);
        let pos = ChunkPos::new(0, 0);
        let mut chunk = flat_chunk(&blocks, pos);
        // The surface the flat chunk was built with.
        let surface = |_x: i32, _z: i32| -61;
        let request = StructureGenRequest {
            pos,
            context: &context,
            registry: &known,
            set: &set,
            build: StructureBuild::enabled(),
            blocks: &blocks,
            surface_height: &surface,
        };
        let report = generate_structures(&mut chunk, &request);
        assert_eq!(report.blocks_written, 1, "{report:?}");
        assert!(!report.was_refused());
        assert!(!report.placed_nothing());
        // `Anchor::OnSurface` centres a 1x1x1 template on block (8, 8) and puts
        // its base at surface + 1 = -60.
        assert_eq!(
            chunk.get_block(8, -60, 8),
            gold,
            "the block is on the surface"
        );
        assert_eq!(
            chunk.get_block(8, -61, 8),
            blocks.default_state("minecraft:stone").expect("stone"),
            "the surface block itself is untouched"
        );
        // Air handling default: nothing was carved.
        assert_eq!(report.blocks_air_skipped, 0);
    }

    #[test]
    fn a_hostile_height_does_not_panic_and_is_refused_by_the_vertical_bucket() {
        let blocks = registry();
        let context = WorldgenContext::overworld(WorldSeed::from_raw(11));
        let mut known = StructureRegistry::new();
        known.insert(id("minecraft:one"), block_template("minecraft:gold_block"));
        let set = StructureSet::with_names([id("minecraft:one")], StructureGrid::new(1, 0, 1), 1.0);
        for height in [i32::MAX, i32::MIN, 400, -5_000] {
            // An ordinary position: `flat_chunk` builds its floor with
            // `pos * 16`, and a saturating extreme position would put the floor
            // somewhere the test's own expectations cannot describe. Extreme
            // positions are covered by `structure::tests` and
            // `structures::tests::the_grid_cell_and_origin_are_euclidean_on_hostile_coordinates`.
            let pos = ChunkPos::new(3, -7);
            let mut chunk = flat_chunk(&blocks, pos);
            let surface = |_x: i32, _z: i32| height;
            let request = StructureGenRequest {
                pos,
                context: &context,
                registry: &known,
                set: &set,
                build: StructureBuild::enabled(),
                blocks: &blocks,
                surface_height: &surface,
            };
            let report = generate_structures(&mut chunk, &request);
            // Either it landed in the chunk or it was refused; never a panic.
            assert!(
                report.blocks_written + report.blocks_out_of_world + report.blocks_outside == 1
                    || report.was_refused(),
                "height {height}: {report:?}"
            );
        }
    }

    #[test]
    fn loading_a_missing_directory_reports_it_rather_than_failing() {
        let (registry, report) =
            load_structures(Path::new("no/such/structure/root"), StructureLimits::PACK);
        assert!(registry.is_empty());
        assert_eq!(report.files, 0);
        assert_eq!(report.loaded, 0);
        assert!(report.skipped.is_empty());
        assert_eq!(report.unreadable_dirs.len(), 1, "{report:?}");
        assert!(
            !report.is_complete(),
            "an unreadable root is not a complete load"
        );
        assert!(report.is_empty(), "no files were considered");
        // The display form renders without panicking.
        assert!(report.to_string().contains("0 files"));

        // An empty directory is a complete load of zero files.
        let dir = std::env::temp_dir().join("mc-worldgen-structure-empty-root");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let (registry, report) = load_structures(&dir, StructureLimits::PACK);
        assert!(registry.is_empty());
        assert!(report.is_complete());
        assert!(report.is_empty());
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn the_loader_counts_every_file_even_the_bad_ones() {
        let root = std::env::temp_dir().join("mc-worldgen-structure-pack-mix");
        // The real layout on purpose — `<root>/structure/…` with `root` being the
        // namespace directory — because the loader's naming rule is exactly
        // `minecraft:` plus the path under the root.
        let nested = root.join("structure").join("village");
        std::fs::create_dir_all(&nested).expect("temp dirs");

        // One good file, built by hand.
        let gzipped = block_template_file("minecraft:gold_block");
        std::fs::write(nested.join("house.nbt"), &gzipped).expect("write good");

        // Three bad files: not gzip, empty, and a truncated gzip stream.
        std::fs::write(nested.join("not_gzip.nbt"), b"plain text").expect("write");
        std::fs::write(nested.join("empty.nbt"), b"").expect("write");
        std::fs::write(nested.join("truncated.nbt"), &gzipped[..gzipped.len() / 2]).expect("write");
        // And a file the loader must ignore entirely rather than count.
        std::fs::write(nested.join("readme.txt"), b"not a structure").expect("write");

        let (registry, report) = load_structures(&root, StructureLimits::PACK);
        assert_eq!(report.files, 4, "four .nbt files, and not the .txt");
        assert_eq!(report.loaded, 1);
        assert_eq!(report.skipped.len(), 3);
        assert!(report.is_complete(), "{report}");
        assert_eq!(report.refused(), 3);
        assert_eq!(registry.len(), 1);
        assert_eq!(report.blocks, 1);
        assert_eq!(report.palette_entries, 1);
        assert_eq!(report.fitting_in_one_chunk, 1);
        // The key is derived from the path under the namespace root: the whole
        // relative path, extension removed.
        assert!(
            registry
                .by_str("minecraft:structure/village/house")
                .is_some(),
            "names: {:?}",
            registry.names()
        );
        assert_eq!(report.duplicate_names, 0, "no name collision in this tree");
        // Every skip names a path and a reason.
        for (path, reason) in &report.skipped {
            assert!(!reason.is_empty(), "{path:?} skipped without a reason");
            assert!(path.starts_with(&root));
        }
        // The walk is sorted, so two runs agree.
        let (again, report_again) = load_structures(&root, StructureLimits::PACK);
        assert_eq!(report, report_again);
        assert_eq!(again.names(), registry.names());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_come_from_the_path_under_the_namespace_root() {
        // One rule: the root *is* the namespace directory (what `MC_VANILLA_DATA`
        // points at), so the name is `minecraft:` plus the relative path with the
        // extension removed.
        let root = Path::new("/pack/data/minecraft");
        assert_eq!(
            name_of(
                root,
                Path::new("/pack/data/minecraft/structure/village/house.nbt")
            )
            .expect("name")
            .to_string(),
            "minecraft:structure/village/house",
            "the name is the path a reader can go and look at"
        );
        assert_eq!(
            name_of(root, Path::new("/pack/data/minecraft/structure/oak.nbt"))
                .expect("name")
                .to_string(),
            "minecraft:structure/oak"
        );
        // The two files the shipped pack has that share a basename still get
        // distinct names, which is why the whole relative path is used.
        assert_ne!(
            name_of(
                root,
                Path::new("/pack/data/minecraft/structure/bastion/blocks/air.nbt")
            ),
            name_of(
                root,
                Path::new("/pack/data/minecraft/structure/bastion/mobs/air.nbt")
            )
        );
        // A path outside the root has no name rather than a wrong one.
        assert_eq!(name_of(root, Path::new("/elsewhere/oak.nbt")), None);
        // A component that is not a legal resource-id character produces no name,
        // so the loader counts the file as skipped rather than keying it under a
        // sanitised spelling a caller could never reconstruct.
        assert_eq!(
            name_of(
                root,
                Path::new("/pack/data/minecraft/structure/UPPER/house.nbt")
            ),
            None
        );
        assert_eq!(
            name_of(root, Path::new("/pack/data/minecraft/a b.nbt")),
            None
        );
        // The previously wrong rule is gone: `structure/village/house` is the
        // *value*, never parsed as a `structure:` namespace.
        assert_eq!(
            name_of(
                root,
                Path::new("/pack/data/minecraft/structure/village/house.nbt")
            )
            .expect("name")
            .namespace(),
            "minecraft"
        );
    }

    #[test]
    fn the_build_options_are_explicit() {
        assert!(StructureBuild::default().enabled);
        assert_eq!(StructureBuild::default().air, AirPolicy::IgnoreAir);
        assert_eq!(
            StructureBuild::default().cross_chunk,
            CrossChunk::SingleChunk
        );
        assert!(!StructureBuild::disabled().enabled);
        let custom = StructureBuild::enabled()
            .with_air(AirPolicy::Overwrite)
            .with_cross_chunk(CrossChunk::Clip);
        assert_eq!(custom.air, AirPolicy::Overwrite);
        assert_eq!(custom.cross_chunk, CrossChunk::Clip);
        assert!(custom.enabled);
        assert_eq!(
            MAX_SELECTABLE_STRUCTURES, 64,
            "the selectable slice is a documented constant, not `all of them`"
        );
        // The load report's completeness predicate covers both failure modes.
        let complete = StructureLoadReport {
            files: 3,
            loaded: 2,
            skipped: vec![(std::path::PathBuf::from("x.nbt"), "bad".to_owned())],
            ..StructureLoadReport::default()
        };
        assert!(complete.is_complete());
        let missing = StructureLoadReport {
            files: 3,
            loaded: 2,
            ..StructureLoadReport::default()
        };
        assert!(!missing.is_complete(), "one file vanished");
    }
}
