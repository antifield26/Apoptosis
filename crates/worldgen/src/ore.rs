//! Pack-driven overworld ore veins (P18-03).
//!
//! ## What is implemented
//!
//! Ore features are assembled from the pack's two JSON layers, exactly as
//! Vanilla assembles them:
//!
//! ```text
//! configured_feature/ore_*.json   →  OreConfig   (size, discard_chance_on_air_exposure, targets)
//! placed_feature/ore_*.json       →  attempts + HeightDist + a reference to the config
//! together                        →  OreFeature  (what the generator runs)
//! ```
//!
//! The overworld set is the feature stage that `worldgen/biome/plains.json`
//! lists — **measured** from the extracted pack (25 placed features: stone
//! variants, coal×2, iron×3, gold×2, redstone×2, diamond×4, lapis×2, copper,
//! dirt/gravel/tuff). Nether ores and emerald (mountains-only) are **not** in
//! that list and are not generated here; P21-02 owns the Nether.
//!
//! ## Placement rule, in full
//!
//! For each [`OreFeature`] in list order, for each of `attempts` draws:
//!
//! 1. a per-`(feature, chunk, attempt)` [`RandomSource`] (never a shared stream)
//!    draws the origin: `in_square` puts `(x, z)` anywhere in the chunk's 16×16,
//!    and [`HeightDist`] picks the y;
//! 2. the blob is an ellipsoid chain between two random endpoints around the
//!    origin, with the pack's `size` controlling both length and radius —
//!    **approximation** of Vanilla's `OreFeature` geometry (see
//!    [`place_blob`]);
//! 3. each cell replaces a block whose state is in one of the feature's
//!    *replaceable* sets (`stone_ore_replaceables`, …), using that target's ore
//!    state (so stone → `coal_ore` and deepslate → `deepslate_coal_ore`);
//! 4. when `discard_chance_on_air_exposure > 0`, a cell that touches air is
//!    skipped with that probability — the pack's *buried* diamond/gold/coal/lapis
//!    variants use `0.5`–`1.0` so veins exposed in a cave thin out.
//!
//! ## Cross-chunk rule
//!
//! A blob that would leave the chunk is **clipped** (the in-chunk cells still
//! place) and the overflow is counted in [`OreStats::blocks_outside`]. This is
//! the same honesty rule as [`crate::placement`]: `Chunk::set_block` wraps
//! horizontally, so a "write into the neighbour" placer would silently corrupt
//! the chunk it was given. The consequence is that veins are slightly thinner
//! at chunk borders than the `count` suggests.
//!
//! ## What is **not** implemented
//!
//! - **Bit-identical `OreFeature` geometry.** Vanilla's exact ellipsoid
//!   sequence and its `java.util.Random` draw order are not reproduced; the
//!   *distributions* (count, y band, size, air-exposure discard) come from the
//!   pack, the shape is our documented approximation.
//! - **`scattered_ore`** (ancient debris): a different feature type, not loaded.
//! - **Biome-filtered ores** outside the plains set (emerald in mountains).
//! - **Deepslate as a terrain layer.** Our terrain is stone, so only the
//!   `stone_ore_replaceables` target fires; the deepslate targets are parsed
//!   and ready for when terrain places deepslate.

use crate::pack_json::{Attempts, HeightDist, TagTable, feature_random, read_worldgen_json};
use crate::seed::WorldgenContext;
use mc_core::random::RandomSource;
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos, SECTION_WIDTH};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Hard ceiling on blob attempts per feature per chunk.
///
/// **product decision.** 64 is above every `count` in the 26.1.2 overworld set
/// (max 30) and still bounds a hostile `IntProvider` (AGENTS.md §10).
pub const MAX_ATTEMPTS_PER_FEATURE: i32 = 64;

/// The overworld ore placed features, in the order `plains.json` lists them.
///
/// **verified**: the stage-6 (underground decoration) feature list of
/// `data/minecraft/worldgen/biome/plains.json` in the 26.1.2 pack. Changing
/// this list is a pack change, not a taste change.
pub const OVERWORLD_ORE_PLACED_FEATURES: &[&str] = &[
    "ore_dirt",
    "ore_gravel",
    "ore_granite_upper",
    "ore_granite_lower",
    "ore_diorite_upper",
    "ore_diorite_lower",
    "ore_andesite_upper",
    "ore_andesite_lower",
    "ore_tuff",
    "ore_coal_upper",
    "ore_coal_lower",
    "ore_iron_upper",
    "ore_iron_middle",
    "ore_iron_small",
    "ore_gold",
    "ore_gold_lower",
    "ore_redstone",
    "ore_redstone_lower",
    "ore_diamond",
    "ore_diamond_medium",
    "ore_diamond_large",
    "ore_diamond_buried",
    "ore_lapis",
    "ore_lapis_buried",
    "ore_copper",
];

/// One target rule: "replace a block in `replaceable` with `ore`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OreTarget {
    /// Default state id of the block written (e.g. `minecraft:coal_ore`).
    pub ore: i32,
    /// Tag the replaced block must belong to (e.g. `minecraft:stone_ore_replaceables`).
    pub replaceable_tag: String,
    /// Resolved replaceable state ids (empty when the tag is unknown).
    pub replaceable: std::collections::BTreeSet<i32>,
}

/// A configured ore feature (`type: minecraft:ore`).
#[derive(Debug, Clone, PartialEq)]
pub struct OreConfig {
    /// Pack file stem, for diagnostics and RNG keys.
    pub name: String,
    /// Blob size parameter (Vanilla's `size`).
    pub size: i32,
    /// Probability of discarding a cell that touches air.
    pub discard_chance_on_air_exposure: f64,
    /// Replacement rules, in pack order.
    pub targets: Vec<OreTarget>,
}

/// An assembled ore feature: what to place, how often, and where in y.
#[derive(Debug, Clone, PartialEq)]
pub struct OreFeature {
    /// Placed-feature name (`ore_coal_upper`, …) — the RNG key.
    pub name: String,
    /// The configured blob.
    pub config: OreConfig,
    /// Attempts per chunk.
    pub attempts: Attempts,
    /// Y distribution of the blob origin.
    pub height: HeightDist,
}

/// What one chunk's ore pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OreStats {
    /// Blob attempts drawn (sum of `attempts.sample` over features).
    pub attempts: usize,
    /// Blob cells that actually replaced a block.
    pub blocks_placed: usize,
    /// Blob cells skipped by the air-exposure discard roll.
    pub discarded_air_exposure: usize,
    /// Blob cells that did not match any replaceable target.
    pub skipped_not_replaceable: usize,
    /// Blob cells that fell outside this chunk (the clip rule).
    pub blocks_outside: usize,
    /// Blob cells whose y fell outside the dimension.
    pub blocks_out_of_world: usize,
}

impl OreStats {
    /// Add one feature-chunk's numbers into a running total.
    pub const fn record(&mut self, other: Self) {
        self.attempts += other.attempts;
        self.blocks_placed += other.blocks_placed;
        self.discarded_air_exposure += other.discarded_air_exposure;
        self.skipped_not_replaceable += other.skipped_not_replaceable;
        self.blocks_outside += other.blocks_outside;
        self.blocks_out_of_world += other.blocks_out_of_world;
    }
}

/// An ordered ore set: the generator runs features in this order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OreSet {
    features: Vec<OreFeature>,
}

impl OreSet {
    /// An empty set — the **neutralised** assembly used by the perturbation pin.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            features: Vec::new(),
        }
    }

    /// Whether nothing would be placed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Number of assembled features.
    #[must_use]
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// The assembled features, in run order.
    #[must_use]
    pub fn features(&self) -> &[OreFeature] {
        &self.features
    }

    /// Mean ore attempts per chunk over the whole set (for statistics).
    #[must_use]
    pub fn mean_attempts_per_chunk(&self) -> f64 {
        self.features.iter().map(|f| f.attempts.mean()).sum()
    }

    /// Load the overworld ore set from a pack namespace root.
    ///
    /// `namespace_root` is `data/minecraft` (the directory holding `worldgen/`).
    /// Each name in [`OVERWORLD_ORE_PLACED_FEATURES`] is assembled from its
    /// placed + configured file; a missing or unparsable name is reported in
    /// `skipped` rather than aborting the load (AGENTS.md §9).
    #[must_use]
    pub fn load_overworld(
        namespace_root: &Path,
        registry: &BlockRegistry,
        tags: &TagTable,
    ) -> (Self, Vec<(String, String)>) {
        let mut set = Self::empty();
        let mut skipped = Vec::new();
        for name in OVERWORLD_ORE_PLACED_FEATURES {
            match load_one(namespace_root, name, registry, tags) {
                Ok(feature) => set.features.push(feature),
                Err(reason) => skipped.push(((*name).to_owned(), reason)),
            }
        }
        (set, skipped)
    }

    /// Build a set from hand-assembled features (tests, neutralised assembly).
    #[must_use]
    pub fn from_features(features: Vec<OreFeature>) -> Self {
        Self { features }
    }
}

/// Load and assemble one placed ore feature.
fn load_one(
    namespace_root: &Path,
    name: &str,
    registry: &BlockRegistry,
    tags: &TagTable,
) -> Result<OreFeature, String> {
    let placed_path = namespace_root
        .join("worldgen")
        .join("placed_feature")
        .join(format!("{name}.json"));
    let placed = read_worldgen_json(&placed_path).map_err(|e| e.to_string())?;
    let configured_id = placed
        .get("feature")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "placed feature has no `feature` id".to_owned())?;
    let config_name = configured_id
        .rsplit(':')
        .next()
        .ok_or_else(|| format!("bad feature id {configured_id:?}"))?;
    let attempts =
        Attempts::parse_placement(placed.get("placement").unwrap_or(&serde_json::Value::Null))
            .ok_or_else(|| format!("{name}: no count/rarity_filter placement"))?;
    let height = placed
        .get("placement")
        .and_then(serde_json::Value::as_array)
        .and_then(|list| {
            list.iter().find(|entry| {
                entry
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|t| t.ends_with("height_range"))
            })
        })
        .and_then(|entry| entry.get("height"))
        .and_then(HeightDist::parse)
        .ok_or_else(|| format!("{name}: no parsable height_range"))?;
    let config = load_config(namespace_root, config_name, registry, tags)?;
    Ok(OreFeature {
        name: name.to_owned(),
        config,
        attempts,
        height,
    })
}

fn load_config(
    namespace_root: &Path,
    name: &str,
    registry: &BlockRegistry,
    tags: &TagTable,
) -> Result<OreConfig, String> {
    let path = namespace_root
        .join("worldgen")
        .join("configured_feature")
        .join(format!("{name}.json"));
    let value = read_worldgen_json(&path).map_err(|e| e.to_string())?;
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !kind.ends_with(":ore") && kind != "ore" {
        return Err(format!("{name}: type {kind:?} is not minecraft:ore"));
    }
    let config = value
        .get("config")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("{name}: missing config"))?;
    let size = config
        .get("size")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0)
        .max(0);
    let discard = config
        .get("discard_chance_on_air_exposure")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let targets_json = config
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{name}: missing targets"))?;
    let mut targets = Vec::new();
    for entry in targets_json {
        let ore_name = entry
            .pointer("/state/Name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{name}: target without state.Name"))?;
        let ore = registry
            .default_state(ore_name)
            .map_err(|e| format!("{name}: {e}"))?;
        let tag = entry
            .pointer("/target/tag")
            .and_then(serde_json::Value::as_str)
            .map(std::borrow::ToOwned::to_owned);
        let block = entry
            .pointer("/target/block")
            .and_then(serde_json::Value::as_str)
            .map(std::borrow::ToOwned::to_owned);
        let (replaceable_tag, replaceable) = if let Some(tag) = tag {
            let members = tags.members(&tag).cloned().unwrap_or_default();
            (tag, members)
        } else if let Some(block) = block {
            let id = registry
                .default_state(&block)
                .map_err(|e| format!("{name}: {e}"))?;
            let mut set = std::collections::BTreeSet::new();
            set.insert(id);
            (block, set)
        } else {
            return Err(format!("{name}: target has neither tag nor block"));
        };
        targets.push(OreTarget {
            ore,
            replaceable_tag,
            replaceable,
        });
    }
    Ok(OreConfig {
        name: name.to_owned(),
        size,
        discard_chance_on_air_exposure: discard,
        targets,
    })
}

/// Place every ore feature into one already-generated (and already-carved) chunk.
///
/// Pure function of `(OreSet, world seed, chunk position)` — per-attempt RNG is
/// derived from the feature name and attempt index, never from a shared stream
/// (AGENTS.md §3.6).
///
/// `Attempts` is sampled **once per feature per chunk** (Vanilla's
/// `CountPlacement` draws one count and then that many positions). A hostile
/// count is clamped to [`MAX_ATTEMPTS_PER_FEATURE`] so a pack cannot hang a
/// chunk generate (AGENTS.md §10).
#[must_use]
pub fn populate_ores(
    chunk: &mut Chunk,
    pos: ChunkPos,
    context: &WorldgenContext,
    ores: &OreSet,
    registry: &BlockRegistry,
) -> OreStats {
    let mut total = OreStats::default();
    for feature in ores.features() {
        let mut count_random = feature_random(context.seed.raw(), &feature.name, pos, 0);
        let count = feature
            .attempts
            .sample(&mut count_random)
            .clamp(0, MAX_ATTEMPTS_PER_FEATURE);
        for roll in 0..count {
            let mut blob_random = feature_random(
                context.seed.raw(),
                &feature.name,
                pos,
                (roll as u32).wrapping_add(1),
            );
            total.record(place_one(
                chunk,
                pos,
                context,
                feature,
                &mut blob_random,
                registry,
            ));
        }
    }
    total
}

fn place_one(
    chunk: &mut Chunk,
    pos: ChunkPos,
    context: &WorldgenContext,
    feature: &OreFeature,
    random: &mut RandomSource,
    registry: &BlockRegistry,
) -> OreStats {
    let mut stats = OreStats {
        attempts: 1,
        ..OreStats::default()
    };
    let base_x = pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = pos.z.saturating_mul(SECTION_WIDTH);
    // `in_square`: origin anywhere in the chunk's 16×16.
    let origin_x = base_x.saturating_add(random.next_i32_bounded(SECTION_WIDTH));
    let origin_z = base_z.saturating_add(random.next_i32_bounded(SECTION_WIDTH));
    let origin_y = feature.height.sample(random, context);
    place_blob(
        chunk,
        context,
        &feature.config,
        origin_x,
        origin_y,
        origin_z,
        random,
        registry,
        &mut stats,
    );
    stats
}

/// Write one blob into `chunk`, clipping to the chunk's footprint.
///
/// Geometry (**approximation** of Vanilla's `OreFeature`): two endpoints are
/// drawn on opposite sides of the origin along a random horizontal angle, each
/// with a y jitter of ±2; `size` ellipsoids of shrinking radius are stamped
/// along the segment. A cell is a candidate when
/// `dx²/rx² + dy²/ry² + dz²/rz² ≤ 1`.
#[allow(clippy::too_many_arguments)]
fn place_blob(
    chunk: &mut Chunk,
    context: &WorldgenContext,
    config: &OreConfig,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    random: &mut RandomSource,
    registry: &BlockRegistry,
    stats: &mut OreStats,
) {
    let size = config.size.max(1);
    let angle = random.next_f64() * std::f64::consts::PI;
    let half = f64::from(size) / 8.0;
    let (sin, cos) = (angle.sin(), angle.cos());
    let x1 = f64::from(origin_x + 8) + sin * half;
    let x2 = f64::from(origin_x + 8) - sin * half;
    let z1 = f64::from(origin_z + 8) + cos * half;
    let z2 = f64::from(origin_z + 8) - cos * half;
    let y1 = f64::from(origin_y) + f64::from(random.next_i32_inclusive(-2, 2));
    let y2 = f64::from(origin_y) + f64::from(random.next_i32_inclusive(-2, 2));

    let min_y = context.min_y;
    let max_y = context.effective_max_y();
    let base_x = chunk.pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = chunk.pos.z.saturating_mul(SECTION_WIDTH);
    // Clip each step to the chunk footprint before iterating its cells (the
    // same O(1) miss rule as `crate::carver::carve_ellipsoid`).
    let chunk_max_x = base_x + SECTION_WIDTH - 1;
    let chunk_max_z = base_z + SECTION_WIDTH - 1;

    for step in 0..size {
        let t = f64::from(step) / f64::from(size);
        let cx = x1 + (x2 - x1) * t;
        let cy = y1 + (y2 - y1) * t;
        let cz = z1 + (z2 - z1) * t;
        // Spindle radius: widest in the middle of the segment.
        let bulge = (std::f64::consts::PI * t).sin().max(0.15);
        let rx = (f64::from(size) / 2.0 * bulge).max(0.5);
        let ry = rx * 0.7;
        let rz = rx;
        let x_lo = ((cx - rx).floor() as i32).max(base_x);
        let x_hi = ((cx + rx).ceil() as i32).min(chunk_max_x);
        let y_lo = ((cy - ry).floor() as i32).max(min_y);
        let y_hi = ((cy + ry).ceil() as i32).min(max_y - 1);
        let z_lo = ((cz - rz).floor() as i32).max(base_z);
        let z_hi = ((cz + rz).ceil() as i32).min(chunk_max_z);
        if x_lo > x_hi || z_lo > z_hi {
            stats.blocks_outside += 1;
            continue;
        }
        for y in y_lo..=y_hi {
            for x in x_lo..=x_hi {
                for z in z_lo..=z_hi {
                    let dx = (f64::from(x) + 0.5 - cx) / rx;
                    let dy = (f64::from(y) + 0.5 - cy) / ry;
                    let dz = (f64::from(z) + 0.5 - cz) / rz;
                    if dx * dx + dy * dy + dz * dz > 1.0 {
                        continue;
                    }
                    write_ore_cell(chunk, config, x, y, z, random, registry, stats);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_ore_cell(
    chunk: &mut Chunk,
    config: &OreConfig,
    x: i32,
    y: i32,
    z: i32,
    random: &mut RandomSource,
    registry: &BlockRegistry,
    stats: &mut OreStats,
) {
    let current = chunk.get_block(x, y, z);
    let Some(target) = config
        .targets
        .iter()
        .find(|t| t.replaceable.contains(&current))
    else {
        stats.skipped_not_replaceable += 1;
        return;
    };
    if config.discard_chance_on_air_exposure > 0.0
        && touches_air(chunk, registry.air_id(), x, y, z)
        && random.next_f64() < config.discard_chance_on_air_exposure
    {
        stats.discarded_air_exposure += 1;
        return;
    }
    if matches!(chunk.set_block(x, y, z, target.ore, registry), Ok(Some(_))) {
        stats.blocks_placed += 1;
    }
}

/// Whether any of the six face neighbours is air.
fn touches_air(chunk: &Chunk, air: i32, x: i32, y: i32, z: i32) -> bool {
    const OFFSETS: [(i32, i32, i32); 6] = [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    OFFSETS
        .iter()
        .any(|(dx, dy, dz)| chunk.get_block(x + dx, y + dy, z + dz) == air)
}

/// Count ore blocks in a chunk, grouped by 16-block y band.
///
/// Band index is `(y - min_y) / 16`. This is the statistic the 32×32 acceptance
/// test aggregates; it is a pure read of the chunk.
#[must_use]
pub fn ore_blocks_by_band(
    chunk: &Chunk,
    context: &WorldgenContext,
    ore_ids: &std::collections::BTreeSet<i32>,
) -> BTreeMap<i32, usize> {
    let mut out = BTreeMap::new();
    let min_y = context.min_y;
    for y in chunk.min_y()..chunk.max_y() {
        for z in 0..SECTION_WIDTH {
            for x in 0..SECTION_WIDTH {
                if ore_ids.contains(&chunk.get_block(x, y, z)) {
                    let band = (y - min_y).div_euclid(16);
                    *out.entry(band).or_insert(0_usize) += 1;
                }
            }
        }
    }
    out
}

/// Every state id this ore set can write, for [`ore_blocks_by_band`].
#[must_use]
pub fn ore_state_ids(ores: &OreSet) -> std::collections::BTreeSet<i32> {
    let mut out = std::collections::BTreeSet::new();
    for feature in ores.features() {
        for target in &feature.config.targets {
            out.insert(target.ore);
        }
    }
    out
}

/// Path helper shared with tests: `data/minecraft` inside an extract root.
#[must_use]
pub fn namespace_root_from_extract(extract_data: &Path) -> PathBuf {
    let candidate = extract_data.join("minecraft");
    if candidate.is_dir() {
        candidate
    } else {
        extract_data.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::{OreConfig, OreFeature, OreSet, OreStats, OreTarget, place_blob, populate_ores};
    use crate::pack_json::Attempts;
    use crate::pack_json::HeightDist;
    use crate::pack_json::YAnchor;
    use crate::seed::{WorldSeed, WorldgenContext};
    use mc_core::random::RandomSource;
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::{Chunk, ChunkPos};

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(r) => r.blocks,
            Err(e) => panic!("registry: {e}"),
        }
    }

    fn context() -> WorldgenContext {
        WorldgenContext::overworld(WorldSeed::from_raw(9))
    }

    fn solid_stone_chunk(blocks: &BlockRegistry) -> Chunk {
        let ctx = context();
        let mut chunk = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            blocks,
        );
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        for y in ctx.min_y..ctx.min_y + 64 {
            for z in 0..16 {
                for x in 0..16 {
                    let _ = chunk.set_block(x, y, z, stone, blocks);
                }
            }
        }
        chunk
    }

    fn coal_feature(blocks: &BlockRegistry) -> OreFeature {
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        let coal = blocks.default_state("minecraft:coal_ore").expect("coal");
        OreFeature {
            name: "test:coal".to_owned(),
            config: OreConfig {
                name: "test:ore_coal".to_owned(),
                size: 8,
                discard_chance_on_air_exposure: 0.0,
                targets: vec![OreTarget {
                    ore: coal,
                    replaceable_tag: "minecraft:stone".to_owned(),
                    replaceable: [stone].into_iter().collect(),
                }],
            },
            attempts: Attempts::Fixed(4),
            height: HeightDist::Uniform {
                min: YAnchor::Absolute(-64),
                max: YAnchor::Absolute(-20),
            },
        }
    }

    #[test]
    fn a_blob_replaces_stone_with_ore_and_is_deterministic() {
        let blocks = registry();
        let ctx = context();
        let feature = coal_feature(&blocks);
        let mut chunk = solid_stone_chunk(&blocks);
        let mut random = RandomSource::new(1);
        let mut stats = OreStats::default();
        place_blob(
            &mut chunk,
            &ctx,
            &feature.config,
            0,
            -40,
            0,
            &mut random,
            &blocks,
            &mut stats,
        );
        assert!(stats.blocks_placed > 0, "{stats:?}");
        let coal = blocks.default_state("minecraft:coal_ore").expect("coal");
        let mut seen = 0;
        for y in -64..0 {
            for z in 0..16 {
                for x in 0..16 {
                    if chunk.get_block(x, y, z) == coal {
                        seen += 1;
                    }
                }
            }
        }
        assert_eq!(seen, stats.blocks_placed);

        // Same inputs → same chunk.
        let mut again = solid_stone_chunk(&blocks);
        let mut random2 = RandomSource::new(1);
        let mut stats2 = OreStats::default();
        place_blob(
            &mut again,
            &ctx,
            &feature.config,
            0,
            -40,
            0,
            &mut random2,
            &blocks,
            &mut stats2,
        );
        assert_eq!(chunk, again);
        assert_eq!(stats, stats2);
    }

    #[test]
    fn air_exposure_discard_thins_exposed_veins() {
        let blocks = registry();
        let ctx = context();
        let mut buried = coal_feature(&blocks);
        buried.config.discard_chance_on_air_exposure = 1.0;
        let air = blocks.air_id();
        let stone = blocks.default_state("minecraft:stone").expect("stone");

        // A single stone layer with air above and below: every cell of a blob
        // here touches air, so `discard = 1.0` must discard all of them. An
        // interior cell of a fat blob would *not* touch air and would still
        // place — that is Vanilla's rule, and this fixture is the exposed case.
        let mut chunk = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            &blocks,
        );
        for z in 0..16 {
            for x in 0..16 {
                let _ = chunk.set_block(x, -40, z, stone, &blocks);
            }
        }
        let mut random = RandomSource::new(2);
        let mut stats = OreStats::default();
        place_blob(
            &mut chunk,
            &ctx,
            &buried.config,
            0,
            -40,
            0,
            &mut random,
            &blocks,
            &mut stats,
        );
        assert_eq!(
            stats.blocks_placed, 0,
            "discard=1.0 must not place an exposed cell: {stats:?}"
        );
        assert!(stats.discarded_air_exposure > 0, "{stats:?}");
        // And the same blob with discard=0.0 does place (the rule is the roll,
        // not a blanket refusal).
        let mut open = coal_feature(&blocks);
        open.config.discard_chance_on_air_exposure = 0.0;
        let mut chunk2 = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            &blocks,
        );
        for z in 0..16 {
            for x in 0..16 {
                let _ = chunk2.set_block(x, -40, z, stone, &blocks);
            }
        }
        let mut random2 = RandomSource::new(2);
        let mut stats2 = OreStats::default();
        place_blob(
            &mut chunk2,
            &ctx,
            &open.config,
            0,
            -40,
            0,
            &mut random2,
            &blocks,
            &mut stats2,
        );
        assert!(stats2.blocks_placed > 0, "{stats2:?}");
        let _ = air;
    }

    #[test]
    fn populate_ores_is_stable_across_calls() {
        let blocks = registry();
        let ctx = context();
        let set = OreSet::from_features(vec![coal_feature(&blocks)]);
        let mut a = solid_stone_chunk(&blocks);
        let mut b = solid_stone_chunk(&blocks);
        let sa = populate_ores(&mut a, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        let sb = populate_ores(&mut b, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        assert_eq!(sa, sb);
        assert_eq!(a, b);
        assert!(sa.attempts >= 1);
    }

    #[test]
    fn neutralising_the_assembly_places_nothing() {
        let blocks = registry();
        let ctx = context();
        let mut chunk = solid_stone_chunk(&blocks);
        let before = chunk.clone();
        let stats = populate_ores(
            &mut chunk,
            ChunkPos::new(0, 0),
            &ctx,
            &OreSet::empty(),
            &blocks,
        );
        assert_eq!(stats, OreStats::default());
        assert_eq!(chunk, before);
        assert!(OreSet::empty().is_empty());
    }
}
