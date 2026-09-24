//! Terrain generation: a noise→height baseline and a flat generator (P07-14).
//!
//! ## What this generates
//!
//! [`TerrainGenerator`] fills a chunk column by column:
//!
//! ```text
//! min_y ..= min_y+bedrock-1        bedrock   (thickness varies per column)
//! bedrock_top+1 ..= stone_top      stone
//! stone_top+1 ..= surface-1        biome filler  (dirt / sand / gravel / podzol)
//! y = surface                      biome top     (grass_block / sand / stone / coarse_dirt)
//! surface+1 ..= sea_level          water         (only where surface < sea_level)
//! surface+1 ..= max_y-1            air           (plus an optional snow cap on high mountains)
//! ```
//!
//! Every block **name** goes through [`mc_registry::BlockRegistry`] once, at
//! generator construction ([`BlockPalette`]); no numeric block-state id is
//! written down anywhere in this crate.
//!
//! ## The height mapping, in full
//!
//! ```text
//! base   = sea_level + BASE_HEIGHT_OFFSET                     (63 + 4 = 67)
//! coarse = terrain_noise(x, 1024, z)                          ([-1, 1], 5 octaves @ TERRAIN_FREQUENCY)
//! fine   = detail_noise(x, 1024, z)                           ([-1, 1], 3 octaves @ DETAIL_FREQUENCY)
//! height = round(base + coarse * TERRAIN_AMPLITUDE + fine * DETAIL_AMPLITUDE)
//! height = clamp(height, min_y + 1, max_y - 1)
//! ```
//!
//! Both noise fields are the crate's own [`FractalNoise`]; the offsets, the two
//! amplitudes, the frequencies and the octave counts are **approximations / our
//! own decisions**. They are *not* Vanilla's density functions, and terrain
//! generated here will not look like a Vanilla world. They were chosen to satisfy
//! the properties this crate actually tests: land above sea level more often than
//! not, oceans below it, mountains above [`MOUNTAIN_HEIGHT`], and nothing outside
//! the dimension.
//!
//! ## What is not implemented
//!
//! Caves, ravines and ores are **not this module's job** — they are separate
//! passes ([`crate::carver`], [`crate::ore`], P18-03) applied after the fill.
//! Still missing here: lakes, aquifers, biome height modifiers, and Vanilla's
//! density-function stack. [`crate::features`] lists the decorations that *are*
//! implemented (oak trees, chunk-local) and is the single place that list lives.

use crate::biome::{Biome, BiomeSource};
use crate::features::{self, TreeDensity};
use crate::noise::{DEFAULT_LACUNARITY, DEFAULT_PERSISTENCE, FractalNoise};
use crate::seed::{WorldSeed, WorldgenContext, splitmix64_mix};
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos, SECTION_HEIGHT, SECTION_WIDTH};
use thiserror::Error;

// ---------------------------------------------------------------- parameters

/// Vertical offset of the noise's zero point above sea level, in blocks.
///
/// **approximation / product decision.** Vanilla's continentalness/erosion stack
/// puts "average" terrain somewhere above sea level; `+4` here is our own choice,
/// picked so the mean surface sits a few blocks above the water and a useful
/// fraction of a large sample falls below it (which is what makes `ocean`
/// reachable — `tests/biome_reachability.rs` measures it).
pub const BASE_HEIGHT_OFFSET: i32 = 4;

/// Peak-to-peak amplitude of the coarse terrain noise, in blocks.
///
/// **approximation / product decision.** `45` is ours, and it is stated against a
/// *measured* field rather than a nominal one. [`FractalNoise::value`] realizes
/// roughly ±0.36 with the default five-octave shaping (its nominal band is ±1,
/// and the octaves rarely reinforce — `tests/golden.rs` freezes the measurement),
/// so [`HeightMap`] multiplies it by [`COARSE_NOISE_SCALE`] first. `45` then puts
/// the surface near `67 ± 45`: down to about 22, which is well below sea level,
/// and up to about 112, which is above [`MOUNTAIN_HEIGHT`]. A nominal-band
/// amplitude of `30` was tried first and produced **no mountains at all**, which
/// is why the sizing is documented in terms of the measured range.
pub const TERRAIN_AMPLITUDE: f64 = 45.0;

/// Multiplier that turns the measured coarse-field range into `[-1, 1]`.
///
/// **derived**: `1 / 0.36 ≈ 2.8`; `2.5` is chosen just below that so the field
/// clips the band extremely rarely rather than never reaching it. The realized
/// range after scaling is asserted in `tests/golden.rs`, so this constant cannot
/// drift unnoticed.
pub const COARSE_NOISE_SCALE: f64 = 2.5;

/// Amplitude of the fine detail noise, in blocks.
///
/// **approximation / product decision.** `6.0` is ours.
pub const DETAIL_AMPLITUDE: f64 = 6.0;

/// Base frequency of the coarse terrain field.
///
/// **approximation.** `0.006` (≈ one feature per 165 blocks) is ours.
pub const TERRAIN_FREQUENCY: f64 = 0.006;

/// Base frequency of the fine detail field.
///
/// **approximation.** `0.05` (≈ one feature per 20 blocks) is ours.
pub const DETAIL_FREQUENCY: f64 = 0.05;

/// Octaves of the coarse terrain field.
///
/// **approximation.** `5` is ours, bounded by [`crate::noise::MAX_OCTAVES`].
pub const TERRAIN_OCTAVES: u32 = 5;

/// Octaves of the fine detail field.
///
/// **approximation.** `3` is ours.
pub const DETAIL_OCTAVES: u32 = 3;

/// Blocks of biome filler under the surface block (dirt under grass, etc.).
///
/// **approximation.** Vanilla's surface rule uses a depth of a few blocks for most
/// biomes; `3` is a fixed value here, not a measured one.
pub const FILLER_DEPTH: i32 = 3;

/// Minimum bedrock thickness at the dimension floor.
///
/// **approximation**: Vanilla's floor is a bedrock band with an irregular top.
pub const BEDROCK_MIN_THICKNESS: i32 = 1;

/// Maximum bedrock thickness at the dimension floor.
///
/// **approximation**: see [`BEDROCK_MIN_THICKNESS`]. Vanilla's exact distribution
/// has not been measured here.
pub const BEDROCK_MAX_THICKNESS: i32 = 4;

/// Frequency of the bedrock-thickness field.
///
/// **approximation / product decision.** `0.35` makes the floor vary over a few
/// blocks, which is enough to look irregular without punching through the world.
pub const BEDROCK_FREQUENCY: f64 = 0.35;

/// Surface height (inclusive) at or above which a column is `mountains`.
///
/// **approximation / product decision.** `95` is 32 blocks above sea level: high
/// enough that mountains are a minority of a sampled area and low enough to be
/// reachable with the amplitude above.
pub const MOUNTAIN_HEIGHT: i32 = 95;

/// Surface height (inclusive) at or above which a mountain column gets snow.
///
/// **approximation / product decision.** `110` is ours; Vanilla's snow line
/// depends on temperature and biome.
pub const SNOW_HEIGHT: i32 = 110;

/// Y plane the climate-independent terrain fields are sampled on.
///
/// **product decision**: sampling the terrain noise on the plane `y = 1024`
/// rather than `y = 0` keeps terrain and climate from sharing a lattice plane.
/// `1024` is 4 × 256, an exact period of the noise table, so it costs nothing in
/// quality.
pub const TERRAIN_NOISE_Y: f64 = 1024.0;

// -------------------------------------------------------------------- errors

/// Why a generator could not produce a chunk.
///
/// Only one variant today, and deliberately so: a generator that cannot resolve
/// its block palette refuses at construction (see [`BlockPalette::resolve`]),
/// because a chunk that silently changed its surface block would be a
/// data-corruption bug rather than a recoverable failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GenerationError {
    /// A block name the generator needs is not in the registry.
    #[error("worldgen needs block {name}: {reason}")]
    UnknownBlock {
        /// The name that failed.
        name: String,
        /// The registry's complaint.
        reason: String,
    },
}

// ------------------------------------------------------------------ palette

/// Every block id a generator needs, resolved from names once.
///
/// `Default` is deliberately **not** derived: an all-zero palette would read as
/// air, and a generator built from it would produce an empty world. The only way
/// to obtain one is [`BlockPalette::resolve`], which fails on an unknown name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPalette {
    air: i32,
    stone: i32,
    bedrock: i32,
    water: i32,
    sand: i32,
    snow: i32,
    /// Per-biome surface names, indexed by [`Biome::index`].
    surface: [ResolvedSurface; 6],
    /// Tree blocks, used by [`crate::features`].
    oak_log: i32,
    oak_leaves: i32,
}

/// A biome's surface composition resolved to ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedSurface {
    top: i32,
    filler: i32,
    underwater: i32,
}

impl BlockPalette {
    /// Resolve every name the generators need.
    ///
    /// # Errors
    ///
    /// [`GenerationError::UnknownBlock`] naming the first missing entry. This is
    /// the *only* place a lookup failure can surface, which is what lets
    /// [`ChunkGenerator::generate_chunk`] be total in practice.
    pub fn resolve(registry: &BlockRegistry) -> Result<Self, GenerationError> {
        let lookup = |name: &str| -> Result<i32, GenerationError> {
            registry
                .default_state(name)
                .map_err(|error| GenerationError::UnknownBlock {
                    name: name.to_owned(),
                    reason: error.to_string(),
                })
        };
        let mut surface = [ResolvedSurface {
            top: 0,
            filler: 0,
            underwater: 0,
        }; 6];
        for biome in crate::biome::BIOMES {
            let blocks = biome.surface();
            surface[biome.index()] = ResolvedSurface {
                top: lookup(blocks.top)?,
                filler: lookup(blocks.filler)?,
                underwater: lookup(blocks.underwater)?,
            };
        }
        Ok(Self {
            // Air by id rather than by name: `BlockRegistry::air_id` is the
            // registry's own answer to "what is air", so this cannot disagree
            // with the empty-block bookkeeping the chunk maintains.
            air: registry.air_id(),
            stone: lookup("minecraft:stone")?,
            bedrock: lookup("minecraft:bedrock")?,
            // The default `level=0` state is a full water source block; every
            // other level value is a flowing block, which a generator must not
            // place as terrain.
            water: lookup("minecraft:water")?,
            sand: lookup("minecraft:sand")?,
            snow: lookup("minecraft:snow_block")?,
            surface,
            oak_log: lookup("minecraft:oak_log")?,
            oak_leaves: lookup("minecraft:oak_leaves")?,
        })
    }

    /// `minecraft:air`.
    #[must_use]
    pub const fn air(&self) -> i32 {
        self.air
    }

    /// `minecraft:stone`.
    #[must_use]
    pub const fn stone(&self) -> i32 {
        self.stone
    }

    /// `minecraft:bedrock`.
    #[must_use]
    pub const fn bedrock(&self) -> i32 {
        self.bedrock
    }

    /// `minecraft:water` (default state: a source block).
    #[must_use]
    pub const fn water(&self) -> i32 {
        self.water
    }

    /// `minecraft:sand`.
    #[must_use]
    pub const fn sand(&self) -> i32 {
        self.sand
    }

    /// `minecraft:snow_block`.
    #[must_use]
    pub const fn snow(&self) -> i32 {
        self.snow
    }

    /// `minecraft:oak_log`.
    #[must_use]
    pub const fn oak_log(&self) -> i32 {
        self.oak_log
    }

    /// `minecraft:oak_leaves`.
    #[must_use]
    pub const fn oak_leaves(&self) -> i32 {
        self.oak_leaves
    }

    /// The surface block of a biome.
    #[must_use]
    pub const fn top_of(&self, biome: Biome) -> i32 {
        self.surface[biome.index()].top
    }

    /// The filler block of a biome.
    #[must_use]
    pub const fn filler_of(&self, biome: Biome) -> i32 {
        self.surface[biome.index()].filler
    }

    /// The block a biome keeps on a submerged surface.
    #[must_use]
    pub const fn underwater_of(&self, biome: Biome) -> i32 {
        self.surface[biome.index()].underwater
    }

    /// Whether a block id is this palette's air (used by the tests).
    #[must_use]
    pub const fn is_air(&self, id: i32) -> bool {
        id == self.air
    }
}

// --------------------------------------------------------------------- trait

/// A thing that produces chunks.
///
/// `Send + Sync` because a real server generates on worker threads; generation is
/// a pure function of `(generator, pos)`, so sharing one across threads cannot
/// introduce nondeterminism (AGENTS.md §8, §3.6).
///
/// # Errors
///
/// The trait is fallible so a future generator can report a configuration or
/// storage failure. Both generators in this crate refuse at *construction*
/// ([`BlockPalette::resolve`]) and therefore do not return `Err` in practice.
pub trait ChunkGenerator: Send + Sync {
    /// Generate one chunk, deterministically.
    ///
    /// # Errors
    ///
    /// [`GenerationError`] when the generator cannot produce a chunk. Neither
    /// generator in this crate does, for a context that was accepted at
    /// construction.
    fn generate_chunk(
        &self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Chunk, GenerationError>;

    /// The biome at a block column, as this generator would generate it.
    fn biome_at(&self, block_x: i32, block_z: i32) -> Biome;

    /// The generator's context (seed and vertical bounds).
    fn context(&self) -> &WorldgenContext;
}

// --------------------------------------------------------------- height field

/// The noise → height mapping used by [`TerrainGenerator`].
#[derive(Debug, Clone, PartialEq)]
struct HeightMap {
    terrain: FractalNoise,
    detail: FractalNoise,
    /// Per-world offsets applied to the sampled coordinates so two seeds do not
    /// share a lattice origin.
    offset_x: f64,
    offset_z: f64,
    base: f64,
    sea_level: i32,
    min_height: i32,
    max_height: i32,
}

impl HeightMap {
    fn new(context: &WorldgenContext) -> Self {
        let seed = context.seed;
        let terrain = FractalNoise::new(
            seed.stream_seed(0x20),
            TERRAIN_OCTAVES,
            DEFAULT_PERSISTENCE,
            DEFAULT_LACUNARITY,
        )
        .at_frequency(TERRAIN_FREQUENCY);
        let detail = FractalNoise::new(
            seed.stream_seed(0x21),
            DETAIL_OCTAVES,
            DEFAULT_PERSISTENCE,
            DEFAULT_LACUNARITY,
        )
        .at_frequency(DETAIL_FREQUENCY);
        // An offset that is a whole number of noise periods (multiples of 256)
        // from the seed, so different worlds see different terrain without
        // changing the field's statistics.
        let offset = |stream: u64| -> f64 {
            let mixed = splitmix64_mix(seed.raw() as u64 ^ stream);
            f64::from((mixed >> 40) as i32 % 4096) * 256.0
        };
        // One block of headroom at each end keeps bedrock and snow inside the
        // dimension even at the extremes of the noise. `effective_max_y`, not
        // `max_y`: the chunk only exists up to the capped section count, and a
        // hostile `height` must not turn into a two-billion-iteration loop.
        let min_height = context.min_y.saturating_add(1);
        let max_height = context
            .effective_max_y()
            .saturating_sub(1)
            .max(min_height.saturating_add(1));
        Self {
            terrain,
            detail,
            offset_x: offset(0x30),
            offset_z: offset(0x31),
            base: f64::from(context.sea_level.saturating_add(BASE_HEIGHT_OFFSET)),
            sea_level: context.sea_level,
            min_height,
            max_height,
        }
    }

    /// The surface y of a column. Always within `(min_y, max_y)`.
    fn height_at(&self, block_x: i32, block_z: i32) -> i32 {
        let x = f64::from(block_x) + self.offset_x;
        let z = f64::from(block_z) + self.offset_z;
        // Scaled from the *measured* field range into `[-1, 1]`; see
        // `COARSE_NOISE_SCALE`.
        let coarse = self.terrain.value(x, TERRAIN_NOISE_Y, z) * COARSE_NOISE_SCALE;
        let fine = self.detail.value(x, TERRAIN_NOISE_Y, z) * COARSE_NOISE_SCALE;
        let height = self.base + coarse * TERRAIN_AMPLITUDE + fine * DETAIL_AMPLITUDE;
        if height.is_nan() {
            return self.sea_level.clamp(self.min_height, self.max_height);
        }
        (height.round() as i32).clamp(self.min_height, self.max_height)
    }

    /// Thickness of the bedrock band at a column.
    fn bedrock_thickness(&self, block_x: i32, block_z: i32) -> i32 {
        let noise = self.detail.value_2d(
            f64::from(block_x) * BEDROCK_FREQUENCY,
            f64::from(block_z) * BEDROCK_FREQUENCY,
        );
        // Map `[-1, 1]` onto `[0, span]` and round; the clamp below makes any
        // out-of-band noise harmless.
        let span = BEDROCK_MAX_THICKNESS - BEDROCK_MIN_THICKNESS;
        // `midpoint` rather than `(noise + 1.0) * 0.5`: the latter overflows for a
        // large `noise`, and clippy is right that the safe spelling costs nothing.
        let extra = (f64::midpoint(noise, 1.0) * f64::from(span)).round() as i32;
        (BEDROCK_MIN_THICKNESS + extra).clamp(BEDROCK_MIN_THICKNESS, BEDROCK_MAX_THICKNESS)
    }
}

// ----------------------------------------------------------- terrain generator

/// Noise terrain with a biome-driven surface.
#[derive(Debug, Clone)]
pub struct TerrainGenerator {
    context: WorldgenContext,
    height_map: HeightMap,
    biomes: BiomeSource,
    palette: BlockPalette,
    trees: TreeDensity,
}

impl TerrainGenerator {
    /// Build a terrain generator for a context.
    ///
    /// # Errors
    ///
    /// [`GenerationError::UnknownBlock`] when the registry is missing a block
    /// this generator needs, including `minecraft:air` itself.
    pub fn new(
        context: WorldgenContext,
        registry: &BlockRegistry,
    ) -> Result<Self, GenerationError> {
        let palette = BlockPalette::resolve(registry)?;
        let biomes = BiomeSource::new(context.seed);
        let height_map = HeightMap::new(&context);
        Ok(Self {
            context,
            height_map,
            biomes,
            palette,
            trees: TreeDensity::default(),
        })
    }

    /// Override the tree density used by [`TerrainGenerator::decorate`].
    #[must_use]
    pub fn with_tree_density(mut self, trees: TreeDensity) -> Self {
        self.trees = trees;
        self
    }

    /// The resolved block palette.
    #[must_use]
    pub const fn palette(&self) -> &BlockPalette {
        &self.palette
    }

    /// The biome field.
    #[must_use]
    pub const fn biome_source(&self) -> &BiomeSource {
        &self.biomes
    }

    /// The surface height of a column, as the generator would shape it.
    #[must_use]
    pub fn surface_height(&self, block_x: i32, block_z: i32) -> i32 {
        self.height_map.height_at(block_x, block_z)
    }

    /// The terrain surface height that decides a column's biome.
    ///
    /// The same value [`ChunkGenerator::generate_chunk`] used for the column, so
    /// a feature pass can classify a column exactly as the terrain pass did —
    /// including under water, where a chunk's own top solid block is the
    /// seafloor but the height the biome was chosen from is the land height.
    #[must_use]
    pub fn terrain_height_at(&self, block_x: i32, block_z: i32) -> i32 {
        self.height_map.height_at(block_x, block_z)
    }

    /// Apply the oak-tree feature to an already-generated chunk.
    ///
    /// Separate from [`ChunkGenerator::generate_chunk`] on purpose: terrain and
    /// decoration are two passes in Vanilla too, and a caller that wants bare
    /// terrain (or its own features) can skip this. **Chunk-local**; see
    /// [`crate::features::populate_oak_trees`] for the straddle rule.
    pub fn decorate(
        &self,
        chunk: &mut Chunk,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> features::TreeStats {
        let height_map = &self.height_map;
        features::populate_oak_trees(
            chunk,
            pos,
            &self.biomes,
            &self.palette,
            self.trees,
            |block_x, block_z| height_map.height_at(block_x, block_z),
            registry,
        )
    }

    /// Fill one column's solid body and surface blocks.
    fn fill_solid(
        &self,
        chunk: &mut Chunk,
        registry: &BlockRegistry,
        block_x: i32,
        block_z: i32,
        height: i32,
        biome: Biome,
    ) {
        let bedrock_top = self
            .context
            .min_y
            .saturating_add(self.height_map.bedrock_thickness(block_x, block_z) - 1);

        // Stone from the top of the bedrock band up to the top of the filler.
        let stone_top = (height - FILLER_DEPTH).max(bedrock_top);
        for y in (bedrock_top + 1)..=stone_top {
            let _ = chunk.set_block(block_x, y, block_z, self.palette.stone, registry);
        }

        // Submerged columns get the biome's underwater block; dry ones its top.
        let submerged = height < self.height_map.sea_level;
        let (top, filler) = if submerged {
            (
                self.palette.underwater_of(biome),
                self.palette.underwater_of(biome),
            )
        } else {
            (self.palette.top_of(biome), self.palette.filler_of(biome))
        };
        for y in (stone_top + 1)..=height {
            let block = if y == height { top } else { filler };
            let _ = chunk.set_block(block_x, y, block_z, block, registry);
        }
    }

    /// Fill one column's bedrock band.
    fn fill_bedrock(
        &self,
        chunk: &mut Chunk,
        registry: &BlockRegistry,
        block_x: i32,
        block_z: i32,
    ) {
        let thickness = self.height_map.bedrock_thickness(block_x, block_z);
        for offset in 0..thickness {
            let y = self.context.min_y.saturating_add(offset);
            let _ = chunk.set_block(block_x, y, block_z, self.palette.bedrock, registry);
        }
    }
}

// ------------------------------------------------------------- flat generator

/// One layer of a [`FlatGenerator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatLayer {
    /// First block y of the layer (inclusive).
    pub from_y: i32,
    /// Last block y of the layer (inclusive).
    pub to_y: i32,
    /// Block name, resolved through the registry.
    pub block: &'static str,
}

impl FlatLayer {
    /// A layer covering `from_y..=to_y` with one block name.
    #[must_use]
    pub const fn new(from_y: i32, to_y: i32, block: &'static str) -> Self {
        Self {
            from_y,
            to_y,
            block,
        }
    }
}

/// A generator that emits a fixed stack of layers in every column.
///
/// This exists for two concrete reasons:
///
/// 1. it is **testable against hand-written expectations** — a golden test can
///    assert the exact block id at exact coordinates without knowing anything
///    about noise (`tests/flat_generator.rs`);
/// 2. it is genuinely useful as a server option (Vanilla's "superflat" world
///    type, and the fastest way to get a deterministic arena for tests).
///
/// It is **not** Vanilla's superflat preset system: the layer stack is a constant
/// of this crate, and Vanilla's generator-settings JSON is not implemented.
#[derive(Debug, Clone)]
pub struct FlatGenerator {
    context: WorldgenContext,
    /// Resolved layers: `(from_y, to_y, id)`, sorted by `(from_y, to_y)`.
    layers: Vec<(i32, i32, i32)>,
    biome: Biome,
}

/// The classic three-band stack on an overworld floor at `y = -64`: bedrock,
/// two dirt, two grass.
///
/// **approximation / product decision**: this is the familiar "superflat" shape,
/// but it is a constant of *this* crate rather than a verified Vanilla preset.
/// Vanilla's default superflat preset is documented as bedrock at the floor, two
/// dirt and one grass; the extra grass layer here is ours, so that a spawned
/// player stands on grass at `y = -60` with two blocks of clearance.
pub const SUPERFLAT_LAYERS: [FlatLayer; 4] = [
    FlatLayer::new(-64, -64, "minecraft:bedrock"),
    FlatLayer::new(-63, -62, "minecraft:dirt"),
    FlatLayer::new(-61, -61, "minecraft:grass_block"),
    FlatLayer::new(-60, -60, "minecraft:grass_block"),
];

impl FlatGenerator {
    /// Build from explicit layers.
    ///
    /// Layers outside the context's vertical range, or with `to_y < from_y`, are
    /// **dropped** — and [`FlatGenerator::dropped_layers`] reports how many —
    /// rather than silently clipped into another layer's band. Overlapping layers
    /// resolve *last writer wins* in `(from_y, to_y)` order, which is
    /// deterministic and independent of the caller's slice order.
    ///
    /// # Errors
    ///
    /// [`GenerationError::UnknownBlock`] when a layer names a block the registry
    /// does not know.
    pub fn new(
        context: WorldgenContext,
        layers: &[FlatLayer],
        biome: Biome,
        registry: &BlockRegistry,
    ) -> Result<Self, GenerationError> {
        let mut resolved: Vec<(i32, i32, i32)> = Vec::with_capacity(layers.len());
        // `effective_max_y`: the chunk cannot hold anything above its capped
        // section range, so a layer there must be dropped rather than written
        // into a section that does not exist.
        let (min_y, max_y) = (context.min_y, context.effective_max_y());
        let mut ordered: Vec<&FlatLayer> = layers.iter().collect();
        ordered.sort_by_key(|layer| (layer.from_y, layer.to_y));
        for layer in ordered {
            if layer.to_y < layer.from_y || layer.from_y < min_y || layer.to_y >= max_y {
                continue;
            }
            let id = registry.default_state(layer.block).map_err(|error| {
                GenerationError::UnknownBlock {
                    name: layer.block.to_owned(),
                    reason: error.to_string(),
                }
            })?;
            resolved.push((layer.from_y, layer.to_y, id));
        }
        Ok(Self {
            context,
            layers: resolved,
            biome,
        })
    }

    /// Build the [`SUPERFLAT_LAYERS`] stack for an overworld context.
    ///
    /// # Errors
    ///
    /// As for [`FlatGenerator::new`]; with the shipped registry it cannot fail
    /// (`tests/palette.rs` asserts every name resolves).
    pub fn superflat(
        context: WorldgenContext,
        registry: &BlockRegistry,
    ) -> Result<Self, GenerationError> {
        Self::new(context, &SUPERFLAT_LAYERS, Biome::Plains, registry)
    }

    /// How many of `declared` layers were dropped for falling outside the world.
    #[must_use]
    pub fn dropped_layers(&self, declared: usize) -> usize {
        declared.saturating_sub(self.layers.len())
    }

    /// The layers in effect.
    #[must_use]
    pub fn layers(&self) -> &[(i32, i32, i32)] {
        &self.layers
    }

    /// The biome every column reports.
    #[must_use]
    pub const fn biome(&self) -> Biome {
        self.biome
    }

    /// Apply the oak-tree feature to an already-generated chunk.
    ///
    /// A flat world has one biome everywhere, so unlike
    /// [`TerrainGenerator::decorate`] this does not consult the climate field.
    pub fn decorate(
        &self,
        chunk: &mut Chunk,
        pos: ChunkPos,
        palette: &BlockPalette,
        density: TreeDensity,
        registry: &BlockRegistry,
    ) -> features::TreeStats {
        features::populate_oak_trees_flat(chunk, pos, self.biome, palette, density, registry)
    }
}

// -------------------------------------------------------------- trait impls

impl ChunkGenerator for TerrainGenerator {
    fn generate_chunk(
        &self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Chunk, GenerationError> {
        let mut chunk = Chunk::air(
            pos,
            self.context.min_section_y(),
            self.context.section_count(),
            registry,
        );
        // A dimension with no vertical room (a degenerate `height`) has nowhere to
        // put a floor, so the chunk stays empty rather than writing a block at a y
        // the caller never asked for.
        if self.context.height <= 0 {
            return Ok(chunk);
        }
        let base_x = pos.x.saturating_mul(SECTION_WIDTH);
        let base_z = pos.z.saturating_mul(SECTION_WIDTH);
        let sea_level = self.height_map.sea_level;
        let water_top = sea_level.min(self.context.effective_max_y().saturating_sub(1));
        for local_z in 0..SECTION_WIDTH {
            for local_x in 0..SECTION_WIDTH {
                let block_x = base_x.saturating_add(local_x);
                let block_z = base_z.saturating_add(local_z);
                let height = self.height_map.height_at(block_x, block_z);
                let biome = self.biomes.biome_at_with_height(
                    block_x,
                    block_z,
                    height,
                    sea_level,
                    MOUNTAIN_HEIGHT,
                );
                self.fill_bedrock(&mut chunk, registry, block_x, block_z);
                self.fill_solid(&mut chunk, registry, block_x, block_z, height, biome);
                // Water above a submerged surface, up to sea level.
                if height < sea_level {
                    for y in (height + 1)..=water_top {
                        let _ = chunk.set_block(block_x, y, block_z, self.palette.water, registry);
                    }
                }
                // A snow cap on the highest mountains.
                if biome == Biome::Mountains && height >= SNOW_HEIGHT {
                    let cap = height.saturating_add(1);
                    if cap < self.context.effective_max_y() {
                        let _ = chunk.set_block(block_x, cap, block_z, self.palette.snow, registry);
                    }
                }
            }
        }
        Ok(chunk)
    }

    fn biome_at(&self, block_x: i32, block_z: i32) -> Biome {
        let height = self.height_map.height_at(block_x, block_z);
        self.biomes.biome_at_with_height(
            block_x,
            block_z,
            height,
            self.height_map.sea_level,
            MOUNTAIN_HEIGHT,
        )
    }

    fn context(&self) -> &WorldgenContext {
        &self.context
    }
}

impl ChunkGenerator for FlatGenerator {
    fn generate_chunk(
        &self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Chunk, GenerationError> {
        let mut chunk = Chunk::air(
            pos,
            self.context.min_section_y(),
            self.context.section_count(),
            registry,
        );
        let base_x = pos.x.saturating_mul(SECTION_WIDTH);
        let base_z = pos.z.saturating_mul(SECTION_WIDTH);
        for local_z in 0..SECTION_WIDTH {
            for local_x in 0..SECTION_WIDTH {
                let block_x = base_x.saturating_add(local_x);
                let block_z = base_z.saturating_add(local_z);
                for (from_y, to_y, id) in &self.layers {
                    for y in *from_y..=*to_y {
                        let _ = chunk.set_block(block_x, y, block_z, *id, registry);
                    }
                }
            }
        }
        Ok(chunk)
    }

    fn biome_at(&self, _block_x: i32, _block_z: i32) -> Biome {
        self.biome
    }

    fn context(&self) -> &WorldgenContext {
        &self.context
    }
}

/// A fixed seed used by tests, documentation and [`crate::existing`] examples.
///
/// **product decision**: a constant so examples are reproducible. It is the seed
/// of the repository's evidence world (`crates/test-support/fixtures/anvil`), and
/// it is *not* a Vanilla default — Vanilla has no fixed default seed.
pub const EXAMPLE_SEED: WorldSeed = WorldSeed::from_raw(1_361_882_806);

/// Section height in blocks, re-exported so callers sizing buffers do not need
/// `mc-world`.
pub const SECTION_HEIGHT_BLOCKS: i32 = SECTION_HEIGHT;

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    /// The shortest period any terrain-scale frequency may imply, in blocks.
    ///
    /// A player crossing a few thousand blocks would notice terrain repeating, so the guard
    /// is generous: it only fires on a frequency that would tile within a short walk.
    const MIN_ACCEPTABLE_PERIOD: f64 = 4_000.0;

    #[test]
    fn the_frequency_constants_do_not_tile_terrain() {
        // The noise lattice is periodic (`crate::noise::LATTICE_PERIOD`), so every
        // frequency constant implies a world-space period of `LATTICE_PERIOD / frequency`.
        // Nothing enforced that before, and a plausible-looking edit — raising
        // `TERRAIN_FREQUENCY` to make terrain more dramatic — would have made the world tile
        // every few hundred blocks with no test failing.
        for (name, frequency) in [
            ("TERRAIN_FREQUENCY", super::TERRAIN_FREQUENCY),
            ("DETAIL_FREQUENCY", super::DETAIL_FREQUENCY),
        ] {
            assert!(
                frequency > 0.0 && frequency.is_finite(),
                "{name} must be a positive finite frequency, got {frequency}"
            );
            let period = crate::noise::LATTICE_PERIOD / frequency;
            assert!(
                period >= MIN_ACCEPTABLE_PERIOD,
                "{name} = {frequency} implies a period of {period:.0} blocks, which tiles \
                 visibly; keep the period at or above {MIN_ACCEPTABLE_PERIOD:.0} blocks"
            );
        }

        // The bedrock-thickness field is deliberately allowed a shorter period: it varies over
        // one or two blocks, so a repeat every few hundred blocks is invisible. Asserted so
        // the exemption is a decision rather than an oversight.
        let bedrock_period = crate::noise::LATTICE_PERIOD / super::BEDROCK_FREQUENCY;
        assert!(
            bedrock_period > 500.0,
            "the bedrock field repeats every {bedrock_period:.0} blocks"
        );
    }

    use super::{
        BlockPalette, ChunkGenerator, EXAMPLE_SEED, FlatGenerator, FlatLayer, GenerationError,
        HeightMap, SUPERFLAT_LAYERS, TerrainGenerator,
    };
    use crate::biome::{BIOMES, Biome};
    use crate::seed::WorldgenContext;
    use mc_persistence::dimension::Dimension;
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::ChunkPos;

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    #[test]
    fn the_palette_resolves_every_block_the_crate_uses() {
        let blocks = registry();
        let palette = BlockPalette::resolve(&blocks).expect("palette");
        assert_eq!(
            palette.air(),
            blocks.default_state("minecraft:air").expect("air")
        );
        assert!(!palette.is_air(palette.stone()));
        assert_ne!(palette.bedrock(), palette.stone());
        assert!(!palette.is_air(palette.water()));
        assert!(!palette.is_air(palette.oak_log()));
        assert!(!palette.is_air(palette.oak_leaves()));
        assert!(!palette.is_air(palette.snow()));
        for biome in BIOMES {
            assert!(!palette.is_air(palette.top_of(biome)), "{biome:?} top");
            assert!(
                !palette.is_air(palette.filler_of(biome)),
                "{biome:?} filler"
            );
            assert!(
                !palette.is_air(palette.underwater_of(biome)),
                "{biome:?} underwater"
            );
        }
        // Water must be a source block (level=0), never a flowing state.
        let source = blocks
            .state_id("minecraft:water", &[("level".to_owned(), "0".to_owned())])
            .expect("water level 0");
        assert_eq!(palette.water(), source);
    }

    #[test]
    fn a_missing_block_is_refused_at_construction() {
        let empty = BlockRegistry::parse("minecraft:air\t0\t1\t-\n").expect("minimal table");
        let error = BlockPalette::resolve(&empty).expect_err("stone is missing");
        // The failure names the block and carries the registry's reason, and it
        // renders without panicking.
        match &error {
            GenerationError::UnknownBlock { name, reason } => {
                assert!(name.starts_with("minecraft:"), "{name}");
                assert!(
                    !reason.is_empty(),
                    "the registry's reason is carried through"
                );
            }
        }
        assert!(TerrainGenerator::new(WorldgenContext::overworld(EXAMPLE_SEED), &empty).is_err());
        assert!(
            FlatGenerator::new(
                WorldgenContext::overworld(EXAMPLE_SEED),
                &SUPERFLAT_LAYERS,
                Biome::Plains,
                &empty
            )
            .is_err()
        );
        // The error renders without panicking.
        assert!(error.to_string().contains("worldgen needs block"));
    }

    #[test]
    fn flat_layers_outside_the_world_are_dropped_and_counted() {
        let blocks = registry();
        let layers = [
            FlatLayer::new(-100, -100, "minecraft:stone"),
            FlatLayer::new(-64, -60, "minecraft:stone"),
            FlatLayer::new(400, 410, "minecraft:stone"),
            FlatLayer::new(10, 5, "minecraft:stone"),
        ];
        let generator = FlatGenerator::new(
            WorldgenContext::overworld(EXAMPLE_SEED),
            &layers,
            Biome::Plains,
            &blocks,
        )
        .expect("generator");
        assert_eq!(
            generator.layers().len(),
            1,
            "only the in-range layer survives"
        );
        assert_eq!(generator.dropped_layers(layers.len()), 3);
        assert_eq!(generator.biome(), Biome::Plains);
        // Layer order in the slice does not change the result.
        let reordered = [
            FlatLayer::new(400, 410, "minecraft:stone"),
            FlatLayer::new(-64, -60, "minecraft:stone"),
            FlatLayer::new(-100, -100, "minecraft:stone"),
            FlatLayer::new(10, 5, "minecraft:stone"),
        ];
        let other = FlatGenerator::new(
            WorldgenContext::overworld(EXAMPLE_SEED),
            &reordered,
            Biome::Plains,
            &blocks,
        )
        .expect("generator");
        assert_eq!(generator.layers(), other.layers());
    }

    #[test]
    fn a_degenerate_context_does_not_panic() {
        let blocks = registry();
        // A zero or negative height: exactly one section, and the superflat stack
        // no longer fits, so every layer is dropped and the chunk stays empty.
        for height in [0, -50] {
            let context = WorldgenContext::new(EXAMPLE_SEED, Dimension::Overworld, 0, height, 0);
            let generator = TerrainGenerator::new(context.clone(), &blocks).expect("generator");
            let chunk = generator
                .generate_chunk(ChunkPos::new(0, 0), &blocks)
                .expect("chunk");
            assert_eq!(chunk.section_count(), 1);
            assert!(
                chunk
                    .sections
                    .iter()
                    .all(|section| section.non_empty_block_count == 0),
                "a world with no height has nothing in it"
            );
            let flat = FlatGenerator::new(context, &SUPERFLAT_LAYERS, Biome::Plains, &blocks)
                .expect("flat");
            assert_eq!(
                flat.dropped_layers(SUPERFLAT_LAYERS.len()),
                SUPERFLAT_LAYERS.len(),
                "every superflat layer is outside a zero-height world"
            );
            let chunk = flat
                .generate_chunk(ChunkPos::new(0, 0), &blocks)
                .expect("chunk");
            assert_eq!(chunk.section_count(), 1);
            assert!(chunk.sections.iter().all(|s| s.non_empty_block_count == 0));
        }
        // A hostile vertical range is clamped, not looped over for ever. `min_y`
        // is the lowest section index the chunk model can represent, so the floor
        // below really exists: a `min_y` of `i32::MIN` would name a section
        // outside the `i8` range and every write would be refused.
        let hostile = WorldgenContext::new(EXAMPLE_SEED, Dimension::Overworld, -2048, i32::MAX, 0);
        assert_eq!(hostile.section_count(), 128, "the section count is bounded");
        assert_eq!(hostile.effective_max_y(), 0, "the chunk really ends at 0");
        let generator = TerrainGenerator::new(hostile, &blocks).expect("generator");
        let chunk = generator
            .generate_chunk(ChunkPos::new(i32::MAX, i32::MIN), &blocks)
            .expect("chunk");
        assert_eq!(chunk.section_count(), 128);
        assert!(chunk.min_y() < chunk.max_y());
        // The floor still exists, so the world is still standable.
        assert!(
            chunk.non_empty_block_count(0) > 0,
            "the floor section is not empty"
        );
        // And nothing escaped the section range.
        for (index, section) in chunk.sections.iter().enumerate() {
            assert!(
                section.blocks.iter().all(|id| *id >= 0),
                "section {index} holds a negative id"
            );
        }
    }

    #[test]
    fn the_height_map_stays_inside_the_dimension() {
        let context = WorldgenContext::overworld(EXAMPLE_SEED);
        let map = HeightMap::new(&context);
        let (mut minimum, mut maximum) = (i32::MAX, i32::MIN);
        for x in (-1_000..1_000).step_by(7) {
            for z in (-1_000..1_000).step_by(11) {
                let height = map.height_at(x, z);
                assert!(
                    height > context.min_y && height < context.max_y(),
                    "height {height} at ({x}, {z}) outside the dimension"
                );
                minimum = minimum.min(height);
                maximum = maximum.max(height);
            }
        }
        assert!(
            minimum < context.sea_level,
            "some terrain is below sea level (min {minimum})"
        );
        assert!(
            maximum > context.sea_level,
            "some terrain is above sea level (max {maximum})"
        );
    }

    #[test]
    fn extreme_chunk_positions_generate_without_panicking() {
        let blocks = registry();
        let generator = TerrainGenerator::new(WorldgenContext::overworld(EXAMPLE_SEED), &blocks)
            .expect("generator");
        for pos in [
            ChunkPos::new(i32::MAX, i32::MAX),
            ChunkPos::new(i32::MIN, i32::MIN),
            ChunkPos::new(i32::MAX, i32::MIN),
            ChunkPos::new(0, i32::MIN),
        ] {
            let chunk = generator.generate_chunk(pos, &blocks).expect("chunk");
            assert_eq!(chunk.pos, pos);
            assert_eq!(chunk.section_count(), 24);
            // The floor section always has bedrock, so it is never empty.
            assert!(
                chunk.non_empty_block_count(0) > 0,
                "{pos:?} must have a floor"
            );
        }
    }

    #[test]
    fn generation_error_messages_are_stable() {
        // P15-06: operator-visible strings; the thiserror migration must not reword them.
        let error = GenerationError::UnknownBlock {
            name: "minecraft:x".to_owned(),
            reason: "nope".to_owned(),
        };
        assert_eq!(error.to_string(), "worldgen needs block minecraft:x: nope");
    }
}
