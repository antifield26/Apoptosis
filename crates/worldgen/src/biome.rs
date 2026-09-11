//! A six-biome baseline selected from noise, with biome-driven surface blocks
//! (P07-15).
//!
//! ## What this is, and what it is not
//!
//! **Not Vanilla.** Vanilla selects biomes with a six-parameter climate table
//! (temperature, humidity, continentalness, erosion, depth, weirdness) fed by
//! multi-noise, then resolves the result against a registry of 60+ biomes with
//! datapack-defined parameters. None of that is reproduced here, and none of
//! those parameters have been measured from the 26.1.2 jar.
//!
//! This is a **two-field threshold rule** over the crate's own
//! [`FractalNoise`]:
//!
//! ```text
//! temperature = temperature_noise(block_x, block_z)   // [-1, 1] nominal
//! humidity    = humidity_noise(block_x, block_z)      // [-1, 1] nominal
//! ```
//!
//! plus one rule that is *not* noise-driven: a column whose terrain surface is
//! below the dimension's sea level is **ocean** (see
//! [`BiomeSource::biome_at_with_height`]). That rule is what makes `ocean`
//! appear where there is actually water instead of in a climate band.
//!
//! ## Biome ids
//!
//! Each [`Biome::id`] is a vanilla resource-id **name** (`minecraft:plains`, …).
//! **No biome registry is loaded by this project**, so these are labels this
//! crate chose for a documented set — `mc-registry` loads blocks and items only,
//! and `Chunk::to_chunk_data` currently writes `minecraft:plains` for every
//! section. They are spelled the way Vanilla spells them so that a later
//! biome-registry loader can match them by name, and that is the whole claim.
//!
//! ## Why the biome is more than a label
//!
//! [`Biome::surface`] is what makes it real: the terrain generator reads it to
//! decide the top block (grass and dirt in plains, sand in desert, stone in
//! mountains, snow on a high mountain), so a wrong biome produces visibly wrong
//! terrain rather than a wrong string in a data structure.

use crate::noise::{DEFAULT_LACUNARITY, DEFAULT_PERSISTENCE, FractalNoise};
use crate::seed::{ChunkSeed, WorldSeed};
use mc_registry::BlockRegistry;

/// Every biome in this baseline, in a fixed order for deterministic iteration.
///
/// **product decision**: six biomes, chosen because each one changes the
/// *surface blocks* a player can see. A biome that only changed its name would
/// not have earned a slot (AGENTS.md §3.4).
pub const BIOMES: [Biome; 6] = [
    Biome::Plains,
    Biome::Desert,
    Biome::Forest,
    Biome::Ocean,
    Biome::Mountains,
    Biome::Taiga,
];

/// A biome of the baseline set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Biome {
    /// Temperate grassland: grass over dirt, scattered oaks.
    Plains,
    /// Hot and dry: sand over sand over stone, no trees.
    Desert,
    /// Temperate and damp: grass over dirt, dense oaks.
    Forest,
    /// Surface below sea level: water, with sand or gravel on the floor.
    Ocean,
    /// High terrain: stone and gravel, snow above the snow line.
    Mountains,
    /// Cold: coarse dirt and podzol over dirt, sparse oaks.
    Taiga,
}

impl Biome {
    /// The biome's resource-id name.
    ///
    /// **verified as a *name***: `minecraft:plains`, `minecraft:desert`,
    /// `minecraft:forest`, `minecraft:ocean`, `minecraft:mountains` and
    /// `minecraft:taiga` are all real Vanilla biome keys (the id list is public
    /// and stable). **Not verified as a mapping**: nothing here claims our
    /// `Desert` covers the same area as Vanilla's desert, and no biome registry
    /// lookup backs this string.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Plains => "minecraft:plains",
            Self::Desert => "minecraft:desert",
            Self::Forest => "minecraft:forest",
            Self::Ocean => "minecraft:ocean",
            Self::Mountains => "minecraft:mountains",
            Self::Taiga => "minecraft:taiga",
        }
    }

    /// Stable index in [`BIOMES`], for deterministic array storage.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Plains => 0,
            Self::Desert => 1,
            Self::Forest => 2,
            Self::Ocean => 3,
            Self::Mountains => 4,
            Self::Taiga => 5,
        }
    }

    /// The biome with a given [`Biome::index`], when in range.
    #[must_use]
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Plains),
            1 => Some(Self::Desert),
            2 => Some(Self::Forest),
            3 => Some(Self::Ocean),
            4 => Some(Self::Mountains),
            5 => Some(Self::Taiga),
            _ => None,
        }
    }

    /// Whether oaks may be placed on this biome's surface.
    #[must_use]
    pub const fn has_trees(self) -> bool {
        matches!(self, Self::Plains | Self::Forest | Self::Taiga)
    }

    /// The surface composition this biome asks the terrain generator for.
    #[must_use]
    pub const fn surface(self) -> SurfaceBlocks {
        match self {
            // Plains and forest share a surface composition while remaining distinct
            // biomes; the merge states that rather than duplicating it.
            Self::Plains | Self::Forest => {
                SurfaceBlocks::soil("minecraft:grass_block", "minecraft:dirt")
            }
            Self::Desert => SurfaceBlocks::all_sand("minecraft:sand"),
            Self::Ocean => SurfaceBlocks::soil("minecraft:sand", "minecraft:gravel"),
            Self::Mountains => SurfaceBlocks::soil("minecraft:stone", "minecraft:gravel"),
            Self::Taiga => SurfaceBlocks::soil("minecraft:coarse_dirt", "minecraft:podzol"),
        }
    }
}

/// The block *names* one biome puts on a column's top and below it.
///
/// Names, not ids: the generator resolves them through
/// [`mc_registry::BlockRegistry`] exactly once, at construction, so no numeric
/// block-state id is ever written down in this crate (the crate contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceBlocks {
    /// The block placed on the topmost solid position of a column.
    pub top: &'static str,
    /// The block filling the few positions under [`SurfaceBlocks::top`].
    pub filler: &'static str,
    /// The block used under water when this biome is submerged.
    pub underwater: &'static str,
}

impl SurfaceBlocks {
    /// A soil biome: `top` over `filler`, `top` kept when submerged.
    #[must_use]
    pub const fn soil(top: &'static str, filler: &'static str) -> Self {
        Self {
            top,
            filler,
            underwater: top,
        }
    }

    /// A biome whose surface is the same block all the way down (desert).
    #[must_use]
    pub const fn all_sand(sand: &'static str) -> Self {
        Self {
            top: sand,
            filler: sand,
            underwater: sand,
        }
    }
}

/// Climate thresholds of the biome selection rule.
///
/// **approximation / product decision.** These four numbers are ours. They are
/// not Vanilla's biome parameters (whose real values live in datapack JSON and
/// were not extracted), and they are chosen only so that all six biomes of
/// [`BIOMES`] are reachable — which `tests/biome_reachability.rs` proves by
/// sampling a large area and counting.
pub mod thresholds {
    /// Below this temperature a column is cold (`plains` or `taiga`).
    ///
    /// **approximation / product decision**: `-0.25` is ours. With
    /// [`CLIMATE_SCALE`] the measured temperature range is about ±0.80, so this
    /// band covers roughly the coldest 25 % of the world.
    pub const COLD: f64 = -0.25;
    /// Above this temperature a column is hot (`desert`, or `forest` when damp).
    ///
    /// **approximation / product decision**: `0.25` is ours, symmetric with
    /// [`COLD`].
    pub const HOT: f64 = 0.25;
    /// Above this humidity a cold column is `taiga` and a temperate one `forest`.
    ///
    /// **approximation / product decision**: `0.0` is ours — exactly the field's
    /// midpoint, so humidity splits the world into damp and dry halves.
    pub const HUMID: f64 = 0.0;
    /// Above this humidity a hot column is `forest` rather than `desert`.
    ///
    /// **approximation / product decision**: `0.15` is ours, higher than
    /// [`HUMID`] so a hot, barely-damp column is still a desert.
    pub const FOREST_HUMIDITY: f64 = 0.15;
}

/// Noise-driven biome field.
///
/// Two [`FractalNoise`] fields keyed off the world seed, sampled at a documented
/// frequency. Immutable after construction, `Send + Sync`.
#[derive(Debug, Clone, PartialEq)]
pub struct BiomeSource {
    temperature: FractalNoise,
    humidity: FractalNoise,
    /// Blocks per climate feature, converted to a frequency at construction.
    scale: f64,
}

/// Base climate frequency: one climate feature per ~200 blocks.
///
/// **approximation / product decision.** Vanilla's climate noise works on a
/// quarter-block lattice with its own scales; this is ours. At `0.005` a
/// temperature or humidity band is a few hundred blocks across, which is wide
/// enough that a player sees a biome rather than noise, and narrow enough that a
/// 500 × 500-block sample reaches all six (asserted by test).
pub const CLIMATE_FREQUENCY: f64 = 0.005;

/// Octaves of the climate fields.
///
/// **product decision**: four octaves give a coastline with some detail without
/// making the bands fractal-noisy at the scale a player crosses them.
pub const CLIMATE_OCTAVES: u32 = 4;

/// Multiplier that turns the measured climate-field range into roughly `[-1, 1]`.
///
/// **derived**: [`FractalNoise::value`] realizes about ±0.40 for the default
/// shaping with four octaves (its nominal band is ±1 but the octaves rarely
/// reinforce), so `2.0` maps it to about ±0.80. Without it the climate almost
/// never leaves the middle band, which is exactly what happened on the first
/// measurement: 75 % of a 12 000-block-wide sample came out `plains`, and
/// `desert` and `taiga` were effectively unreachable. `tests/biome_reachability.rs`
/// freezes the resulting distribution.
pub const CLIMATE_SCALE: f64 = 2.0;

impl BiomeSource {
    /// Build the field for a world seed.
    #[must_use]
    pub fn new(seed: WorldSeed) -> Self {
        // Distinct stream ids keep the two fields independent.
        let temperature = FractalNoise::new(
            seed.stream_seed(0x10),
            CLIMATE_OCTAVES,
            DEFAULT_PERSISTENCE,
            DEFAULT_LACUNARITY,
        )
        .at_frequency(CLIMATE_FREQUENCY);
        let humidity = FractalNoise::new(
            seed.stream_seed(0x11),
            CLIMATE_OCTAVES,
            DEFAULT_PERSISTENCE,
            DEFAULT_LACUNARITY,
        )
        .at_frequency(CLIMATE_FREQUENCY);
        Self {
            temperature,
            humidity,
            scale: CLIMATE_FREQUENCY,
        }
    }

    /// The climate frequency in use.
    #[must_use]
    pub const fn frequency(&self) -> f64 {
        self.scale
    }

    /// The temperature field value at a block position, scaled to roughly
    /// `[-1, 1]` (see [`CLIMATE_SCALE`]).
    #[must_use]
    pub fn temperature_at(&self, block_x: i32, block_z: i32) -> f64 {
        self.temperature
            .value_2d(f64::from(block_x), f64::from(block_z))
            * CLIMATE_SCALE
    }

    /// The humidity field value at a block position, scaled to roughly `[-1, 1]`.
    #[must_use]
    pub fn humidity_at(&self, block_x: i32, block_z: i32) -> f64 {
        self.humidity
            .value_2d(f64::from(block_x), f64::from(block_z))
            * CLIMATE_SCALE
    }

    /// The biome for a column, given only its climate.
    ///
    /// This is the entire selection rule, as a pure function of two numbers, so
    /// it can be tested without generating any terrain:
    ///
    /// | temperature | humidity | biome |
    /// |---|---|---|
    /// | `< COLD` | any | `plains` when dry, `taiga` when humid |
    /// | `COLD..=HOT` | any | `plains` when dry, `forest` when humid |
    /// | `> HOT` | `< FOREST_HUMIDITY` | `desert` |
    /// | `> HOT` | `>= FOREST_HUMIDITY` | `forest` |
    ///
    /// `mountains` and `ocean` are **not** selectable here: both depend on
    /// terrain height, so they come from
    /// [`BiomeSource::biome_at_with_height`]. A NaN input selects `plains`,
    /// because a NaN temperature is a bug in the caller and a deterministic
    /// answer beats a panic (AGENTS.md §9).
    #[must_use]
    pub fn climate_biome(temperature: f64, humidity: f64) -> Biome {
        if temperature.is_nan() || humidity.is_nan() {
            return Biome::Plains;
        }
        if temperature < thresholds::COLD {
            return if humidity >= thresholds::HUMID {
                Biome::Taiga
            } else {
                Biome::Plains
            };
        }
        if temperature > thresholds::HOT {
            return if humidity >= thresholds::FOREST_HUMIDITY {
                Biome::Forest
            } else {
                Biome::Desert
            };
        }
        if humidity >= thresholds::HUMID {
            Biome::Forest
        } else {
            Biome::Plains
        }
    }

    /// The biome for a column, given its terrain surface height.
    ///
    /// Rule order matters and is documented: a submerged column is `ocean`
    /// whatever its climate says; a column at or above `mountain_height` is
    /// `mountains`; everything else falls through to
    /// [`BiomeSource::climate_biome`].
    #[must_use]
    pub fn biome_at_with_height(
        &self,
        block_x: i32,
        block_z: i32,
        surface_height: i32,
        sea_level: i32,
        mountain_height: i32,
    ) -> Biome {
        if surface_height < sea_level {
            return Biome::Ocean;
        }
        if surface_height >= mountain_height {
            return Biome::Mountains;
        }
        Self::climate_biome(
            self.temperature_at(block_x, block_z),
            self.humidity_at(block_x, block_z),
        )
    }

    /// The biome for a column, ignoring height (so: never `ocean`/`mountains`).
    ///
    /// Convenience for callers that only have a position; the terrain generator
    /// uses [`BiomeSource::biome_at_with_height`].
    #[must_use]
    pub fn biome_at(&self, block_x: i32, block_z: i32) -> Biome {
        Self::climate_biome(
            self.temperature_at(block_x, block_z),
            self.humidity_at(block_x, block_z),
        )
    }

    /// The seed a chunk uses for this field, so a caller can key a cache by it.
    #[must_use]
    pub const fn chunk_seed(seed: WorldSeed, pos: mc_world::ChunkPos) -> ChunkSeed {
        seed.chunk_seed(pos)
    }
}

/// Resolve a block name, falling back to air with a logged warning.
///
/// **Nothing in the generation path uses this.** Both generators resolve their
/// block palette once, fallibly, in `terrain::BlockPalette::resolve`, and a
/// missing entry there is an error rather than a substitution. This helper exists
/// for callers outside the pipeline that want a name→id conversion which cannot
/// fail, and it logs rather than substituting silently (AGENTS.md §3.3).
#[must_use]
pub fn resolve_or_air(registry: &BlockRegistry, name: &str) -> i32 {
    registry.default_state(name).unwrap_or_else(|error| {
        tracing::warn!(block = name, %error, "block name is missing from the registry; using air");
        registry.air_id()
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // The rule is exact by construction; comparisons are exact.
mod tests {
    use super::{BIOMES, Biome, BiomeSource, CLIMATE_FREQUENCY, SurfaceBlocks, thresholds};
    use crate::seed::WorldSeed;

    #[test]
    fn the_biome_set_is_closed_and_ordered() {
        for (slot, biome) in BIOMES.iter().enumerate() {
            assert_eq!(biome.index(), slot, "{biome:?} has a stable slot");
            assert_eq!(Biome::from_index(slot), Some(*biome));
            assert!(biome.id().starts_with("minecraft:"), "{biome:?}");
        }
        assert_eq!(Biome::from_index(BIOMES.len()), None);
        // Ids are distinct, so a set of biomes is a set of names.
        let mut ids: Vec<&str> = BIOMES.iter().map(|biome| biome.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BIOMES.len());
        // Only the soil biomes grow trees.
        assert!(Biome::Forest.has_trees() && Biome::Plains.has_trees());
        assert!(!Biome::Desert.has_trees() && !Biome::Ocean.has_trees());
        assert!(!Biome::Mountains.has_trees());
    }

    #[test]
    fn surface_blocks_are_biome_specific() {
        assert_eq!(Biome::Plains.surface().top, "minecraft:grass_block");
        assert_eq!(Biome::Desert.surface().top, "minecraft:sand");
        assert_eq!(Biome::Desert.surface().filler, "minecraft:sand");
        assert_eq!(Biome::Mountains.surface().top, "minecraft:stone");
        assert_eq!(Biome::Ocean.surface().underwater, "minecraft:sand");
        assert_eq!(Biome::Taiga.surface().top, "minecraft:coarse_dirt");
        assert_eq!(
            SurfaceBlocks::soil("minecraft:grass_block", "minecraft:dirt").underwater,
            "minecraft:grass_block"
        );
    }

    #[test]
    fn the_climate_rule_covers_every_band() {
        let case =
            |temperature: f64, humidity: f64| BiomeSource::climate_biome(temperature, humidity);
        assert_eq!(case(0.0, -0.9), Biome::Plains, "temperate and dry");
        assert_eq!(case(0.0, 0.9), Biome::Forest, "temperate and humid");
        assert_eq!(case(-0.9, -0.9), Biome::Plains, "cold and dry");
        assert_eq!(case(-0.9, 0.9), Biome::Taiga, "cold and humid");
        assert_eq!(case(0.9, -0.9), Biome::Desert, "hot and dry");
        assert_eq!(case(0.9, 0.9), Biome::Forest, "hot and humid");
        assert_eq!(
            case(thresholds::COLD, 0.5),
            Biome::Forest,
            "cold bound is inclusive"
        );
        assert_eq!(
            case(thresholds::HOT, -0.5),
            Biome::Plains,
            "hot bound is inclusive"
        );
        // A NaN is answered deterministically rather than panicking.
        assert_eq!(case(f64::NAN, 0.0), Biome::Plains);
        assert_eq!(case(0.0, f64::NAN), Biome::Plains);
    }

    #[test]
    fn height_overrides_climate_for_ocean_and_mountains() {
        let source = BiomeSource::new(WorldSeed::from_raw(1_361_882_806));
        assert_eq!(
            source.biome_at_with_height(0, 0, 40, 63, 100),
            Biome::Ocean,
            "below sea level is ocean whatever the climate says"
        );
        assert_eq!(
            source.biome_at_with_height(0, 0, 120, 63, 100),
            Biome::Mountains,
            "at the mountain height is mountains"
        );
        // At sea level the climate decides, and the height-free helper agrees.
        let at_sea = source.biome_at_with_height(0, 0, 63, 63, 100);
        assert_eq!(at_sea, source.biome_at(0, 0));
    }

    #[test]
    fn the_climate_fields_are_deterministic() {
        let first = BiomeSource::new(WorldSeed::from_raw(7));
        let second = BiomeSource::new(WorldSeed::from_raw(7));
        assert_eq!(first, second);
        assert_eq!(first.frequency(), CLIMATE_FREQUENCY);
        for step in -50..50 {
            let x = step * 32;
            let z = step * -17;
            assert_eq!(first.temperature_at(x, z), second.temperature_at(x, z));
            assert_eq!(first.humidity_at(x, z), second.humidity_at(x, z));
            assert_eq!(first.biome_at(x, z), second.biome_at(x, z));
        }
        // A different seed is a different climate. Sampled at a half-integer
        // coordinate: an integer block position is a lattice point of the climate
        // field, where every Perlin value is exactly zero whatever the seed.
        let other = BiomeSource::new(WorldSeed::from_raw(8));
        assert_ne!(
            first.temperature_at(1, 1),
            other.temperature_at(1, 1),
            "a different world seed must give a different climate"
        );
    }

    #[test]
    fn climate_values_stay_in_the_nominal_band() {
        let source = BiomeSource::new(WorldSeed::from_raw(99));
        for x in (-2_000..2_000).step_by(97) {
            for z in (-2_000..2_000).step_by(131) {
                let temperature = source.temperature_at(x, z);
                let humidity = source.humidity_at(x, z);
                assert!((-1.0..=1.0).contains(&temperature), "{temperature}");
                assert!((-1.0..=1.0).contains(&humidity), "{humidity}");
            }
        }
    }
}
