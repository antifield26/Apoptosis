//! Oak trees: the only feature this crate places, and the full list of what it
//! does not (P07-15/P07-16).
//!
//! ## What is implemented
//!
//! Exactly one thing: **oak trees on grass/dirt surfaces in `plains`, `forest`
//! and `taiga`**, at a documented per-biome density, with a documented trunk
//! height range and canopy shape. That is a deliberate vertical slice
//! (AGENTS.md §3.4): a player can see, walk around and cut down a tree, which
//! makes the whole path — biome → surface → feature → block write — observable.
//!
//! ## What is **not** implemented (the honest list)
//!
//! - **Structures**: villages, temples, mineshafts, strongholds, ruined portals,
//!   ocean monuments, shipwrecks, igloos, pillager outposts, ancient cities.
//! - **Ores**: coal, iron, copper, gold, redstone, lapis, diamond, emerald,
//!   quartz, deepslate variants, and every other ore vein or blob.
//! - **Caves and ravines**: no carver of any kind, so the world is solid.
//! - **Lakes, springs, aquifers, water and lava pockets**: water exists only as
//!   sea-level filling from [`crate::terrain`]; there is no lake or spring
//!   feature, and no aquifer can carve a cave full of water.
//! - **Other trees**: birch, spruce, jungle, acacia, dark oak, mangrove, cherry,
//!   pale oak, azalea; and for oak itself no big oak, no branches, no vines, no
//!   bee nests.
//! - **Other plants**: grass, ferns, flowers, mushrooms, cacti, sugar cane,
//!   pumpkins, melons, berries, saplings, bamboo, kelp, coral, and every other
//!   "decoration" block Vanilla's vegetation features place.
//! - **Snow and ice features**: a mountain snow *cap* is written by
//!   [`crate::terrain`], but there is no snowfall, no ice and no freezing.
//! - **Fossils, geodes, amethyst, dripstone, sculk, tuff blobs, and the
//!   dirt/gravel/granite/diorite/andesite discs.**
//! - **Vanilla's placed-feature system**: no `count`/`rarity_filter`/`in_square`/
//!   `heightmap` placement modifiers, no feature ordering, no `PlacedFeature`
//!   JSON, no datapack features. The density rule below is a direct per-column
//!   probability instead.
//! - **Block *states***: logs are placed in their default (`axis=y`) state and
//!   leaves in their default (`distance=7`, `persistent=false`,
//!   `waterlogged=false`) state. Vanilla sets leaf `distance` from the trunk and
//!   a log's `axis` from its neighbours; the default log state happens to be
//!   correct for a vertical trunk, and the leaf `distance` is deliberately left
//!   at the default rather than computed.
//!
//! ## Placement rule, in full
//!
//! For each of the 256 columns of a chunk, in `(local_x, local_z)` order:
//!
//! 1. the column's biome must be one of `plains`/`forest`/`taiga`
//!    ([`Biome::has_trees`]); otherwise the column is skipped and **not** counted
//!    as an attempt;
//! 2. one draw from a [`RandomSource`] seeded by the *column*, not by a shared
//!    stream — `RandomSource::new(splitmix64(chunk_pos ^ block_x << 32 ^ block_z))`
//!    — decides the tree: `roll < chance`;
//! 3. if accepted, a second draw from the same per-column source picks the trunk
//!    height in `MIN_TRUNK_HEIGHT..=MAX_TRUNK_HEIGHT`;
//! 4. the column's top solid block must be the biome's own surface block, with
//!    air (not water) for the trunk's headroom; otherwise the column is skipped;
//! 5. the tree is placed only if its whole canopy fits inside the chunk (the
//!    straddle rule below) and inside the dimension.
//!
//! Per-column seeding is what makes placement independent of iteration order, of
//! how many earlier columns were skipped, and of whether a neighbouring chunk has
//! been generated at all. `tests/determinism.rs` checks that two chunks generated
//! in opposite orders come out identical.
//!
//! ## The straddle rule — decided, and asserted
//!
//! A tree at a chunk's edge has a canopy that reaches into the neighbour. This
//! crate takes the **simple, documented option**: it **refuses** to place a tree
//! whose canopy would leave the chunk, and reports the refusal in
//! [`TreeStats::refused_straddling`]. Nothing is ever written into a chunk other
//! than the one passed in, so generation stays a pure function of one position —
//! which is what keeps it order-independent and free of cross-chunk write
//! hazards.
//!
//! The consequence, stated rather than hidden: **trees are slightly rarer near
//! chunk borders than the density suggests** (a trunk needs to be at least
//! [`CANOPY_RADIUS`] blocks inside the chunk on both axes), so there is a
//! visible tree-free frame at every chunk boundary. Vanilla places features from
//! the owning chunk and writes across the border; a later phase can replace this
//! rule by reusing [`place_oak_at`] and writing into the neighbour.
//! `tests/tree_features.rs` asserts both halves: a straddling tree is refused and
//! writes nothing, and a placed tree touches no block outside its chunk.

use crate::biome::Biome;
use crate::seed::{OVERWORLD_SEA_LEVEL, pack_chunk_pos, splitmix64_mix};
use crate::terrain::{BlockPalette, MOUNTAIN_HEIGHT};
use mc_registry::BlockRegistry;
use mc_simulation::RandomSource;
use mc_world::{Chunk, ChunkPos, SECTION_WIDTH};

/// Smallest trunk height, in blocks.
///
/// **approximation**: Vanilla's oak trunk is commonly documented as 4–6 blocks;
/// `4` is the low end. Not measured from the 26.1.2 jar here.
pub const MIN_TRUNK_HEIGHT: i32 = 4;

/// Largest trunk height, in blocks.
///
/// **approximation**: see [`MIN_TRUNK_HEIGHT`]; `6` is the high end.
pub const MAX_TRUNK_HEIGHT: i32 = 6;

/// Horizontal radius of the widest canopy layer, in blocks.
///
/// **approximation**: Vanilla's oak canopy is nominally a 5×5 blob on its lower
/// layers (radius 2), which this reproduces; the exact per-layer shape is a
/// simplification (see [`place_oak_at`]).
pub const CANOPY_RADIUS: i32 = 2;

/// Number of canopy layers above the trunk's top block.
///
/// **approximation**: `2` puts the canopy top at `surface + trunk_height + 2`.
/// With [`CANOPY_RADIUS`] the layers have radii `2`, `1` and `0`, so the canopy
/// is 5×5, then 3×3, then a single block — 32 blocks including a five-log trunk.
pub const CANOPY_LAYERS: i32 = 2;

/// Mean trees per 16×16 chunk in a forest.
///
/// **approximation / product decision.** Vanilla's oak count in a forest is
/// commonly quoted as around ten per chunk (community documentation, **not**
/// measured here); `6` is our value, chosen so a forest reads as wooded without
/// the chunk-local rule making canopies overlap everywhere.
pub const FOREST_TREES_PER_CHUNK: f64 = 6.0;

/// Mean trees per 16×16 chunk in plains.
///
/// **approximation / product decision.** `0.5` — roughly one tree every two
/// chunks, which is the sparse look a plains biome has.
pub const PLAINS_TREES_PER_CHUNK: f64 = 0.5;

/// Mean trees per 16×16 chunk in taiga.
///
/// **approximation / product decision.** `3.0` — wooded, less dense than a
/// forest in this baseline.
pub const TAIGA_TREES_PER_CHUNK: f64 = 3.0;

/// Tree density for one biome, in mean trees per 16×16 chunk.
///
/// The value is converted to a per-column probability as
/// `chance = mean_trees_per_chunk / 256`, which is the documented rule. It is
/// **not** Vanilla's density: Vanilla uses a `count` per chunk with a
/// `rarity_filter` and an `in_square` offset, and its real numbers live in
/// worldgen JSON this project has not loaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeDensity {
    /// Mean trees per chunk in `plains`.
    pub plains_per_chunk: f64,
    /// Mean trees per chunk in `forest`.
    pub forest_per_chunk: f64,
    /// Mean trees per chunk in `taiga`.
    pub taiga_per_chunk: f64,
}

impl TreeDensity {
    /// The documented density used when a caller does not choose one.
    #[must_use]
    pub const fn default() -> Self {
        Self {
            plains_per_chunk: PLAINS_TREES_PER_CHUNK,
            forest_per_chunk: FOREST_TREES_PER_CHUNK,
            taiga_per_chunk: TAIGA_TREES_PER_CHUNK,
        }
    }

    /// No trees anywhere.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            plains_per_chunk: 0.0,
            forest_per_chunk: 0.0,
            taiga_per_chunk: 0.0,
        }
    }

    /// Mean trees per chunk for a biome (`0.0` for a biome that grows none).
    #[must_use]
    pub const fn per_chunk(&self, biome: Biome) -> f64 {
        match biome {
            Biome::Plains => self.plains_per_chunk,
            Biome::Forest => self.forest_per_chunk,
            Biome::Taiga => self.taiga_per_chunk,
            Biome::Desert | Biome::Ocean | Biome::Mountains => 0.0,
        }
    }

    /// Per-column probability for a biome, clamped into `[0, 1]`.
    ///
    /// The documented conversion from "mean trees per chunk" to "chance per
    /// column". Columns per chunk is 256 by construction ([`SECTION_WIDTH`]²).
    /// A NaN or negative density yields `0.0`, and an infinite one yields `1.0`,
    /// so a hostile config cannot produce a probability outside the range.
    #[must_use]
    pub fn chance_per_column(&self, biome: Biome) -> f64 {
        let columns = f64::from(SECTION_WIDTH * SECTION_WIDTH);
        let chance = self.per_chunk(biome) / columns;
        if chance.is_nan() {
            return 0.0;
        }
        chance.clamp(0.0, 1.0)
    }
}

impl Default for TreeDensity {
    fn default() -> Self {
        Self::default()
    }
}

/// What one chunk's decoration pass did.
///
/// Returned rather than logged so a caller — and a test — can see the refusals,
/// not just the successes: "no trees here" and "trees refused" are different
/// outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeStats {
    /// Columns whose biome grows trees (the attempts the density applies to).
    pub columns_considered: usize,
    /// Trees whose trunk and canopy were written into the chunk.
    pub trees_placed: usize,
    /// Trees refused because part of the canopy would have left the chunk.
    ///
    /// The straddle rule in action; see the module docs. Non-zero is expected
    /// near chunk borders, not an error.
    pub refused_straddling: usize,
    /// Trees refused because they would not fit vertically in the dimension.
    pub refused_vertical: usize,
    /// Blocks written by the feature (trunk logs plus leaves).
    pub blocks_written: usize,
}

impl TreeStats {
    /// Add one chunk's decoration into a running total.
    ///
    /// `TreeStats` is per chunk because that is what the pass produces; a caller decorating a world needs a
    /// total, and putting the addition here keeps it next to the fields it adds.
    pub const fn record(&mut self, other: Self) {
        self.columns_considered += other.columns_considered;
        self.trees_placed += other.trees_placed;
        self.refused_straddling += other.refused_straddling;
        self.refused_vertical += other.refused_vertical;
        self.blocks_written += other.blocks_written;
    }
}

/// The outcome of one tree placement attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OakPlacement {
    /// The tree was written; `blocks` counts the blocks that changed.
    Placed {
        /// Block writes that actually changed a block.
        blocks: usize,
    },
    /// The canopy would have crossed the chunk border, so nothing was written.
    RefusedStraddling,
    /// The trunk or canopy would not fit inside the dimension, so nothing was
    /// written.
    RefusedVertical,
}

/// Place oaks across a generated chunk, using the biome at each column.
///
/// `terrain_height` is the generator's own surface height for a column (see
/// [`crate::terrain::TerrainGenerator::terrain_height_at`]); it decides whether a
/// column is `ocean`/`mountains` exactly as the terrain pass did, so the feature
/// cannot disagree with the terrain under it. A caller that has only a chunk can
/// pass the height of its top solid block instead, which agrees everywhere except
/// under water.
///
/// **Chunk-local**: only `chunk` is written. See the module docs for the straddle
/// rule and the full placement rule.
#[must_use]
pub fn populate_oak_trees(
    chunk: &mut Chunk,
    pos: ChunkPos,
    biomes: &crate::biome::BiomeSource,
    palette: &BlockPalette,
    density: TreeDensity,
    terrain_height: impl Fn(i32, i32) -> i32,
    registry: &BlockRegistry,
) -> TreeStats {
    let mut stats = TreeStats::default();
    let base_x = pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = pos.z.saturating_mul(SECTION_WIDTH);
    for local_z in 0..SECTION_WIDTH {
        for local_x in 0..SECTION_WIDTH {
            let block_x = base_x.saturating_add(local_x);
            let block_z = base_z.saturating_add(local_z);
            let biome = biomes.biome_at_with_height(
                block_x,
                block_z,
                terrain_height(block_x, block_z),
                OVERWORLD_SEA_LEVEL,
                MOUNTAIN_HEIGHT,
            );
            stats = attempt(
                stats, chunk, pos, block_x, block_z, biome, density, palette, registry,
            );
        }
    }
    stats
}

/// Place oaks across a chunk of a [`crate::terrain::FlatGenerator`] world.
///
/// The same rule as [`populate_oak_trees`], except that a flat world has one
/// biome everywhere instead of a climate field, so the biome is passed in
/// directly.
#[must_use]
pub fn populate_oak_trees_flat(
    chunk: &mut Chunk,
    pos: ChunkPos,
    biome: Biome,
    palette: &BlockPalette,
    density: TreeDensity,
    registry: &BlockRegistry,
) -> TreeStats {
    let mut stats = TreeStats::default();
    let base_x = pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = pos.z.saturating_mul(SECTION_WIDTH);
    for local_z in 0..SECTION_WIDTH {
        for local_x in 0..SECTION_WIDTH {
            let block_x = base_x.saturating_add(local_x);
            let block_z = base_z.saturating_add(local_z);
            stats = attempt(
                stats, chunk, pos, block_x, block_z, biome, density, palette, registry,
            );
        }
    }
    stats
}

/// One column's tree attempt, shared by both population functions.
#[allow(clippy::too_many_arguments)]
fn attempt(
    mut stats: TreeStats,
    chunk: &mut Chunk,
    pos: ChunkPos,
    block_x: i32,
    block_z: i32,
    biome: Biome,
    density: TreeDensity,
    palette: &BlockPalette,
    registry: &BlockRegistry,
) -> TreeStats {
    if !biome.has_trees() {
        return stats;
    }
    let chance = density.chance_per_column(biome);
    if chance <= 0.0 {
        return stats;
    }
    stats.columns_considered += 1;
    let mut random = column_random(pos, block_x, block_z);
    if random.next_f64() >= chance {
        return stats;
    }
    let trunk_height = random.next_i32_inclusive(MIN_TRUNK_HEIGHT, MAX_TRUNK_HEIGHT);
    let Some(surface) = tree_surface(chunk, palette, block_x, block_z) else {
        return stats;
    };
    match place_oak_at(
        chunk,
        registry,
        palette,
        block_x,
        surface,
        block_z,
        trunk_height,
    ) {
        OakPlacement::Placed { blocks } => {
            stats.trees_placed += 1;
            stats.blocks_written += blocks;
        }
        OakPlacement::RefusedStraddling => stats.refused_straddling += 1,
        OakPlacement::RefusedVertical => stats.refused_vertical += 1,
    }
    stats
}

/// Write one oak whose trunk base sits on top of `surface_y`.
///
/// The shape, documented because it is a choice rather than a measurement:
///
/// - trunk: `trunk_height` logs in a vertical line from `surface_y + 1`, so the
///   topmost log is at `surface_y + trunk_height`;
/// - canopy: layers at `trunk_top + 1`, `trunk_top + 2` and `trunk_top + 3`, of
///   radius [`CANOPY_RADIUS`], `CANOPY_RADIUS - 1` and `0` respectively, as full
///   squares except that the widest layer's four corners are omitted;
/// - the trunk's own column is skipped where the canopy passes over it;
/// - a leaf write is skipped where a log already exists, and nothing else is ever
///   overwritten, so a tree never destroys terrain.
///
/// With the default five-block trunk the tree is 36 blocks: 5 logs, 21 leaves
/// (5×5 minus four corners), 9 leaves (3×3) and 1 leaf tip.
/// This is **not** Vanilla's oak shape: Vanilla's tree decorators randomise the
/// canopy per tree.
#[must_use]
pub fn place_oak_at(
    chunk: &mut Chunk,
    registry: &BlockRegistry,
    palette: &BlockPalette,
    block_x: i32,
    surface_y: i32,
    block_z: i32,
    trunk_height: i32,
) -> OakPlacement {
    let trunk_height = trunk_height.clamp(MIN_TRUNK_HEIGHT, MAX_TRUNK_HEIGHT);
    let trunk_top = surface_y.saturating_add(trunk_height);

    // Chunk-local rule: the widest canopy layer must fit inside this chunk.
    if !tree_fits_in_chunk(block_x, block_z) {
        return OakPlacement::RefusedStraddling;
    }
    // Vertical rule: the whole tree must fit inside the dimension.
    let canopy_top = trunk_top.saturating_add(1).saturating_add(CANOPY_LAYERS);
    if canopy_top >= chunk.max_y() || surface_y < chunk.min_y() {
        return OakPlacement::RefusedVertical;
    }

    let mut written = 0_usize;
    // Trunk first, so the canopy never has to replace a log with a leaf.
    for y in (surface_y.saturating_add(1))..=trunk_top {
        if write(chunk, registry, block_x, y, block_z, palette.oak_log()) {
            written += 1;
        }
    }
    for layer in 0..=CANOPY_LAYERS {
        let radius = (CANOPY_RADIUS - layer).max(0);
        // The canopy sits *above* the top log, so a trunk never pokes through its
        // own leaves; the topmost log is at `trunk_top` and the widest leaf layer
        // at `trunk_top + 1`.
        let y = trunk_top.saturating_add(1).saturating_add(layer);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                // The widest layer's four corners are omitted, which is what
                // gives an oak canopy its rounded silhouette.
                if layer == 0 && dx.abs() == radius && dz.abs() == radius {
                    continue;
                }
                let x = block_x.saturating_add(dx);
                let z = block_z.saturating_add(dz);
                if chunk.get_block(x, y, z) == palette.oak_log() {
                    continue;
                }
                if write(chunk, registry, x, y, z, palette.oak_leaves()) {
                    written += 1;
                }
            }
        }
    }
    OakPlacement::Placed { blocks: written }
}

/// Whether a tree with a trunk at `(block_x, block_z)` fits in its chunk.
///
/// Exposed so a caller — or a test — can ask the straddle question without
/// generating anything: this is exactly the predicate [`place_oak_at`] applies
/// before it writes a block.
#[must_use]
pub fn tree_fits_in_chunk(block_x: i32, block_z: i32) -> bool {
    let local_x = block_x.rem_euclid(SECTION_WIDTH);
    let local_z = block_z.rem_euclid(SECTION_WIDTH);
    (CANOPY_RADIUS..SECTION_WIDTH - CANOPY_RADIUS).contains(&local_x)
        && (CANOPY_RADIUS..SECTION_WIDTH - CANOPY_RADIUS).contains(&local_z)
}

/// Write a block, reporting whether anything changed.
fn write(chunk: &mut Chunk, registry: &BlockRegistry, x: i32, y: i32, z: i32, id: i32) -> bool {
    // `set_block` refuses a `y` outside the chunk, which the vertical check above
    // already excludes; a refusal is not a panic and counts as "not written".
    matches!(chunk.set_block(x, y, z, id, registry), Ok(Some(_)))
}

/// The per-column random source.
///
/// Seeded from `(chunk position, block position)` only — never from a shared
/// stream and never from iteration order — so a column's tree decision is the
/// same however the chunk is reached. The chunk seed already mixes the world
/// seed, and the mixer is the crate's splitmix64.
fn column_random(pos: ChunkPos, block_x: i32, block_z: i32) -> RandomSource {
    let mixed = splitmix64_mix(
        ((block_x as i64 as u64) << 32) ^ (block_z as u32 as u64) ^ pack_chunk_pos(pos),
    );
    RandomSource::new(mixed as i64)
}

/// The y a tree may stand on, or `None` when the column is not plantable.
///
/// "Plantable" means: the top non-air block is one of the palette's biome top
/// blocks (so a tree never grows out of sand, stone or gravel), it is not water,
/// and the [`MIN_TRUNK_HEIGHT`] positions above it are air.
fn tree_surface(chunk: &Chunk, palette: &BlockPalette, block_x: i32, block_z: i32) -> Option<i32> {
    let mut surface = None;
    for y in (chunk.min_y()..chunk.max_y()).rev() {
        let id = chunk.get_block(block_x, y, block_z);
        if id != palette.air() {
            surface = Some(y);
            break;
        }
    }
    let surface = surface?;
    let block = chunk.get_block(block_x, surface, block_z);
    if block == palette.water() || !is_plantable_top(palette, block) {
        return None;
    }
    for offset in 1..=MIN_TRUNK_HEIGHT {
        if surface.saturating_add(offset) >= chunk.max_y() {
            return None;
        }
        if chunk.get_block(block_x, surface + offset, block_z) != palette.air() {
            return None;
        }
    }
    // The canopy needs headroom too; without this a column that looks plantable
    // would be refused by `place_oak_at` for a reason the caller cannot see.
    if surface.saturating_add(MIN_TRUNK_HEIGHT + CANOPY_LAYERS + 1) >= chunk.max_y() {
        return None;
    }
    Some(surface)
}

/// Whether a block id is the surface block of a biome that grows trees.
fn is_plantable_top(palette: &BlockPalette, id: i32) -> bool {
    crate::biome::BIOMES
        .iter()
        .any(|biome| biome.has_trees() && palette.top_of(*biome) == id)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        CANOPY_RADIUS, FOREST_TREES_PER_CHUNK, OakPlacement, TreeDensity, TreeStats, place_oak_at,
        populate_oak_trees_flat, tree_fits_in_chunk,
    };
    use crate::biome::Biome;
    use crate::seed::{WorldSeed, WorldgenContext};
    use crate::terrain::{BlockPalette, ChunkGenerator, FlatGenerator};
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::ChunkPos;

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    /// A flat superflat chunk at `(0, 0)`: grass at `y = -60`.
    fn flat(registry: &BlockRegistry) -> (mc_world::Chunk, BlockPalette) {
        let generator =
            FlatGenerator::superflat(WorldgenContext::overworld(WorldSeed::from_raw(5)), registry)
                .expect("flat generator");
        (
            generator
                .generate_chunk(ChunkPos::new(0, 0), registry)
                .expect("chunk"),
            BlockPalette::resolve(registry).expect("palette"),
        )
    }

    #[test]
    fn the_density_rule_is_documented_and_clamped() {
        let density = TreeDensity::default();
        assert_eq!(density.per_chunk(Biome::Desert), 0.0);
        assert_eq!(density.per_chunk(Biome::Ocean), 0.0);
        assert_eq!(density.per_chunk(Biome::Mountains), 0.0);
        assert_eq!(density.per_chunk(Biome::Forest), FOREST_TREES_PER_CHUNK);
        let chance = density.chance_per_column(Biome::Forest);
        assert!((chance - FOREST_TREES_PER_CHUNK / 256.0).abs() < 1e-12);
        assert!(chance > 0.0 && chance < 1.0);
        assert_eq!(TreeDensity::none().chance_per_column(Biome::Forest), 0.0);
        // A hostile density cannot produce a probability outside [0, 1].
        let silly = TreeDensity {
            plains_per_chunk: f64::INFINITY,
            forest_per_chunk: -5.0,
            taiga_per_chunk: f64::NAN,
        };
        assert_eq!(silly.chance_per_column(Biome::Plains), 1.0);
        assert_eq!(silly.chance_per_column(Biome::Forest), 0.0);
        assert_eq!(silly.chance_per_column(Biome::Taiga), 0.0);
    }

    #[test]
    fn the_straddle_rule_refuses_border_columns() {
        // Columns 0 and 1 are too close to the border for radius 2; 2 is the
        // first column that fits.
        assert!(!tree_fits_in_chunk(0, 8));
        assert!(!tree_fits_in_chunk(1, 8));
        assert!(tree_fits_in_chunk(2, 8));
        assert!(tree_fits_in_chunk(13, 8));
        assert!(!tree_fits_in_chunk(14, 8));
        assert!(!tree_fits_in_chunk(15, 8));
        assert!(
            tree_fits_in_chunk(2 + 16, 8),
            "the next chunk has its own frame"
        );
        // Negative world coordinates behave the same way: -1 is local 15.
        assert!(!tree_fits_in_chunk(-1, 8));
        assert!(!tree_fits_in_chunk(-2, 8));
        assert!(tree_fits_in_chunk(-3, 8), "column -3 is local 13");
    }

    #[test]
    fn a_placed_tree_writes_only_inside_its_chunk() {
        let blocks = registry();
        let (mut chunk, palette) = flat(&blocks);
        let before = chunk.clone();
        let outcome = place_oak_at(&mut chunk, &blocks, &palette, 8, -60, 8, 5);
        let written = match outcome {
            OakPlacement::Placed { blocks } => blocks,
            other => panic!("expected a placement, got {other:?}"),
        };
        // 5 trunk logs + 21 leaves (5×5 minus four corners) + 9 leaves (3×3) +
        // 1 leaf tip = 36 blocks. Every one is air beforehand, so the count of
        // changed blocks equals the count of writes.
        assert_eq!(written, 36, "the documented shape has 36 blocks");
        // Nothing outside the chunk's own 16×16 footprint changed.
        let mut differences = 0;
        for x in -2..18 {
            for z in -2..18 {
                for y in -64..320 {
                    if before.get_block(x, y, z) != chunk.get_block(x, y, z) {
                        assert!(
                            (0..16).contains(&x) && (0..16).contains(&z),
                            "wrote outside the chunk at ({x}, {y}, {z})"
                        );
                        differences += 1;
                    }
                }
            }
        }
        assert_eq!(
            differences, written,
            "every counted block is inside the chunk"
        );

        // The trunk runs from `surface + 1` to `surface + trunk_height`; the
        // surface block itself is untouched. The canopy sits above the top log.
        for y in -59..=-55 {
            assert_eq!(chunk.get_block(8, y, 8), palette.oak_log(), "trunk at {y}");
        }
        assert_eq!(
            chunk.get_block(8, -54, 8),
            palette.oak_leaves(),
            "widest layer"
        );
        assert_eq!(
            chunk.get_block(8, -53, 8),
            palette.oak_leaves(),
            "middle layer"
        );
        assert_eq!(chunk.get_block(8, -52, 8), palette.oak_leaves(), "leaf tip");
        assert_eq!(
            chunk.get_block(8, -51, 8),
            palette.air(),
            "nothing above the tree"
        );
        assert_eq!(chunk.get_block(8, -60, 8), palette.top_of(Biome::Plains));
        assert_eq!(chunk.get_block(9, -54, 8), palette.oak_leaves(), "canopy");
        assert_eq!(chunk.get_block(7, -54, 8), palette.oak_leaves());
        assert_eq!(
            chunk.get_block(9, -53, 8),
            palette.oak_leaves(),
            "middle layer"
        );
        assert_eq!(chunk.get_block(6, -54, 6), palette.air(), "corner omitted");
        assert_eq!(
            chunk.get_block(10, -54, 10),
            palette.air(),
            "corner omitted"
        );
    }

    #[test]
    fn a_straddling_tree_is_refused_and_writes_nothing() {
        let blocks = registry();
        let (mut chunk, palette) = flat(&blocks);
        let before = chunk.clone();
        for (x, z) in [(0, 8), (15, 8), (8, 0), (8, 15), (0, 0), (1, 14), (14, 1)] {
            let outcome = place_oak_at(&mut chunk, &blocks, &palette, x, -60, z, 5);
            assert_eq!(
                outcome,
                OakPlacement::RefusedStraddling,
                "column ({x}, {z})"
            );
        }
        assert_eq!(chunk, before, "a refused tree must not write a block");
    }

    #[test]
    fn a_tree_that_does_not_fit_vertically_is_refused() {
        let blocks = registry();
        let (mut chunk, palette) = flat(&blocks);
        let before = chunk.clone();
        let ceiling = chunk.max_y();
        let outcome = place_oak_at(&mut chunk, &blocks, &palette, 8, ceiling - 1, 8, 5);
        assert_eq!(outcome, OakPlacement::RefusedVertical);
        assert_eq!(chunk, before);
        // An absurd trunk height is clamped, not trusted.
        let outcome = place_oak_at(&mut chunk, &blocks, &palette, 8, -60, 8, i32::MAX);
        assert!(matches!(outcome, OakPlacement::Placed { .. }));
    }

    #[test]
    fn a_forest_chunk_grows_trees_and_reports_refusals() {
        let blocks = registry();
        let generator =
            FlatGenerator::superflat(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
                .expect("flat");
        let mut chunk = generator
            .generate_chunk(ChunkPos::new(0, 0), &blocks)
            .expect("chunk");
        let palette = BlockPalette::resolve(&blocks).expect("palette");
        let before = chunk.clone();
        let stats = populate_oak_trees_flat(
            &mut chunk,
            ChunkPos::new(0, 0),
            Biome::Forest,
            &palette,
            TreeDensity::default(),
            &blocks,
        );
        assert_eq!(stats.columns_considered, 256);
        assert!(stats.trees_placed > 0, "a forest chunk must grow a tree");
        assert!(stats.blocks_written > 0);
        // At most the border frame can be refused: 256 - 12² = 112 columns.
        let frame = usize::try_from(256 - (16 - 2 * CANOPY_RADIUS).pow(2)).expect("112 fits");
        assert_eq!(frame, 112);
        assert!(
            stats.refused_straddling <= frame,
            "refusals {} exceed the {frame} border columns",
            stats.refused_straddling
        );
        assert_eq!(stats.refused_vertical, 0);
        assert!(
            stats.trees_placed + stats.refused_straddling <= stats.columns_considered,
            "placements {} + refusals {} cannot exceed the {} accepted rolls",
            stats.trees_placed,
            stats.refused_straddling,
            stats.columns_considered
        );
        // The neighbours stayed untouched. The scan is over *local* coordinates,
        // because `Chunk::get_block` wraps horizontal coordinates into the same
        // chunk (it is a query, not a bounds check), so a world-coordinate probe
        // at x = 16 would read column 0 of this chunk rather than the neighbour's.
        let mut differences = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in -64..320 {
                    if before.get_block(x, y, z) != chunk.get_block(x, y, z) {
                        differences += 1;
                    }
                }
            }
        }
        assert_eq!(
            differences, stats.blocks_written,
            "every changed block belongs to one of the {} trees",
            stats.trees_placed
        );
        // Nothing outside the chunk's own footprint can have changed either:
        // every write goes through `Chunk::set_block`, whose local coordinates are
        // `rem_euclid(16)`, so a write can never leave the chunk it was given.
        for (x, z) in [(0, 8), (15, 8), (8, 0), (8, 15)] {
            for y in -64..320 {
                assert_eq!(
                    before.get_block(x, y, z),
                    chunk.get_block(x, y, z),
                    "a border column ({x}, {y}, {z}) must be untouched"
                );
            }
        }
        // The same input gives the same output, twice.
        let mut again = generator
            .generate_chunk(ChunkPos::new(0, 0), &blocks)
            .expect("chunk");
        let stats_again = populate_oak_trees_flat(
            &mut again,
            ChunkPos::new(0, 0),
            Biome::Forest,
            &palette,
            TreeDensity::default(),
            &blocks,
        );
        assert_eq!(stats, stats_again);
        assert_eq!(chunk, again);
    }

    #[test]
    fn a_desert_column_never_grows_a_tree() {
        let blocks = registry();
        let generator =
            FlatGenerator::superflat(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
                .expect("flat");
        let mut chunk = generator
            .generate_chunk(ChunkPos::new(0, 0), &blocks)
            .expect("chunk");
        let palette = BlockPalette::resolve(&blocks).expect("palette");
        let before = chunk.clone();
        let stats = populate_oak_trees_flat(
            &mut chunk,
            ChunkPos::new(0, 0),
            Biome::Desert,
            &palette,
            TreeDensity::default(),
            &blocks,
        );
        assert_eq!(stats, TreeStats::default(), "a desert grows nothing");
        assert_eq!(chunk, before);
    }

    #[test]
    fn a_water_surface_is_never_planted() {
        // A flooded column replays the ocean case without needing the climate
        // field: the feature must not plant on water even when it is asked to.
        let blocks = registry();
        let generator =
            FlatGenerator::superflat(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
                .expect("flat");
        let mut chunk = generator
            .generate_chunk(ChunkPos::new(0, 0), &blocks)
            .expect("chunk");
        let palette = BlockPalette::resolve(&blocks).expect("palette");
        // Every column is water at and above the top solid block, so no column
        // is plantable and the whole chunk is left alone.
        for x in 0..16 {
            for z in 0..16 {
                for y in -60..-40 {
                    chunk
                        .set_block(x, y, z, palette.water(), &blocks)
                        .expect("inside the chunk");
                }
            }
        }
        let before = chunk.clone();
        let stats = populate_oak_trees_flat(
            &mut chunk,
            ChunkPos::new(0, 0),
            Biome::Plains,
            &palette,
            TreeDensity {
                plains_per_chunk: 256.0,
                forest_per_chunk: 0.0,
                taiga_per_chunk: 0.0,
            },
            &blocks,
        );
        assert_eq!(
            stats.columns_considered, 256,
            "a certainty roll every column"
        );
        assert_eq!(stats.trees_placed, 0, "nothing may grow out of water");
        assert_eq!(
            stats.refused_straddling, 0,
            "border columns are refused earlier"
        );
        assert_eq!(chunk, before, "a flooded chunk is left untouched");
    }
}
