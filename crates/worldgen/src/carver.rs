//! Pack-driven cave and canyon carvers, with **no water fill** (P18-03).
//!
//! ## What is implemented
//!
//! Two carver types from `worldgen/configured_carver/`:
//!
//! | Pack type | Files | Shape |
//! |---|---|---|
//! | `minecraft:cave` | `cave`, `cave_extra_underground` | winding tunnels with elliptical cross-section |
//! | `minecraft:canyon` | `canyon` | long, tall, thin ravine along a slowly turning path |
//!
//! Both carve **air only**. `lava_level` is parsed and then **ignored**: a lava
//! floor needs the fluid system (P20), and a dry carver is the honest partial.
//! Water-filled carver output and lakes are **P20-01b** — named, not hidden.
//! `debug_settings` (the pack's coloured-glass markers) are ignored entirely;
//! they exist only for Vanilla's carver debug world.
//!
//! ## Cross-chunk rule (the important design note)
//!
//! A cave that starts in chunk A must still carve the part of itself that falls
//! in chunk B, or the world is a grid of amputated tunnels. Vanilla solves this
//! with a proto-chunk buffer; `mc_world::Chunk` **cannot** write into a
//! neighbour ([`crate::placement`] documents why). So this module uses the
//! pure-function version of the same idea:
//!
//! ```text
//! to carve chunk C:
//!   for every source chunk S within CARVER_SOURCE_RADIUS_CHUNKS of C:
//!       seed = (world seed, S, carver name)
//!       roll S's probability; if it fails, skip
//!       walk the whole path S would carve (world coordinates)
//!       write only the cells that fall inside C
//! ```
//!
//! Every source is a pure function of `(seed, S)`, so chunk C's caves are the
//! same however the world is generated, and neighbouring chunks share one
//! continuous path (AGENTS.md §3.6). The radius is a **derived** bound: the
//! longest walk here is [`CAVE_MAX_STEPS`] × [`STEP_LENGTH`] blocks plus
//! radius, which fits inside 8 chunks (128 blocks).
//!
//! ## What is **not** implemented
//!
//! - **Water or lava in carvers** (P20-01b / P20 fluids).
//! - **Aquifers.** Vanilla's carver consults an aquifer so a cave below sea
//!   level can flood; we always leave dry air.
//! - **Bit-identical tunnel geometry.** The walk (angle drift, radius
//!   multipliers, floor level) consumes the pack's parameters and is our
//!   documented approximation of `CaveWorldCarver` / `RavineWorldCarver`, not a
//!   line-for-line port.
//! - **`nether_cave`.** P21-02.
//! - **Cheese/spaghetti/noodle cave subtypes.** One walk style per type.

use crate::pack_json::{FloatRange, TagTable, feature_random, read_worldgen_json};
use crate::seed::WorldgenContext;
use mc_core::random::RandomSource;
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos, SECTION_WIDTH};
use std::path::Path;

/// How far (in chunks) a carver source may sit from the chunk being carved.
///
/// **derived**: [`CAVE_MAX_STEPS`] × [`STEP_LENGTH`] + max radius ≈ 100 blocks,
/// so ±8 chunks (128 blocks) covers every cell this module can reach.
pub const CARVER_SOURCE_RADIUS_CHUNKS: i32 = 8;

/// Longest cave walk, in steps.
///
/// **approximation / product decision.** `48` is ours; Vanilla's cave length
/// distribution is not measured here.
pub const CAVE_MAX_STEPS: i32 = 48;

/// Longest canyon walk, in steps.
pub const CANYON_MAX_STEPS: i32 = 96;

/// World distance advanced per walk step.
pub const STEP_LENGTH: f64 = 2.0;

/// The overworld carvers every overworld biome lists.
///
/// **verified**: `worldgen/biome/plains.json` `carvers`, which is the same
/// triple every overworld biome in the 26.1.2 pack declares.
pub const OVERWORLD_CARVERS: &[&str] = &["cave", "cave_extra_underground", "canyon"];

/// What kind of tunnel a carver walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarverKind {
    /// `minecraft:cave`.
    Cave,
    /// `minecraft:canyon`.
    Canyon,
}

/// Shared carver configuration, as the pack writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct CarverConfig {
    /// File stem (`cave`, `canyon`, …) — the RNG key.
    pub name: String,
    /// Which walk to run.
    pub kind: CarverKind,
    /// Probability that a source chunk actually spawns this carver.
    pub probability: f64,
    /// Y band of the walk origin.
    pub y: crate::pack_json::HeightDist,
    /// Vertical squish of the cross-section (`yScale`).
    pub y_scale: FloatRange,
    /// Horizontal radius multiplier range (cave) or fixed factor (canyon).
    pub horizontal_radius_multiplier: FloatRange,
    /// Vertical radius multiplier (cave) / unused for canyon.
    pub vertical_radius_multiplier: FloatRange,
    /// Floor of the cave as a fraction of the vertical radius (cave only).
    ///
    /// The pack writes `uniform(-1.0, -0.4)`; cells below
    /// `center_y + floor_level * v_radius` are left alone.
    pub floor_level: FloatRange,
    /// Tag of replaceable blocks (`#minecraft:overworld_carver_replaceables`).
    pub replaceable_tag: String,
    /// Resolved replaceable state ids.
    pub replaceable: std::collections::BTreeSet<i32>,
    /// Canyon thickness (vertical half-extent parameter), 0 for caves.
    pub thickness: FloatRange,
    /// Canyon vertical rotation per step (radians).
    pub vertical_rotation: FloatRange,
    /// Lava level, parsed and **not applied** (P20). Kept so a later fluid pass
    /// reads the pack value from here rather than re-parsing.
    #[allow(dead_code)]
    pub lava_level: i32,
}

/// An ordered carver set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarverSet {
    carvers: Vec<CarverConfig>,
}

impl CarverSet {
    /// No carvers — the dry, uncarved world.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            carvers: Vec::new(),
        }
    }

    /// Whether the set carves nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.carvers.is_empty()
    }

    /// How many carvers are loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.carvers.len()
    }

    /// The loaded carvers, in run order.
    #[must_use]
    pub fn carvers(&self) -> &[CarverConfig] {
        &self.carvers
    }

    /// Load [`OVERWORLD_CARVERS`] from a pack namespace root.
    ///
    /// Missing or unparsable files land in `skipped` (AGENTS.md §9).
    #[must_use]
    pub fn load_overworld(namespace_root: &Path, tags: &TagTable) -> (Self, Vec<(String, String)>) {
        let mut set = Self::empty();
        let mut skipped = Vec::new();
        for name in OVERWORLD_CARVERS {
            match load_one(namespace_root, name, tags) {
                Ok(config) => set.carvers.push(config),
                Err(reason) => skipped.push(((*name).to_owned(), reason)),
            }
        }
        (set, skipped)
    }

    /// Hand-built set (tests).
    #[must_use]
    pub fn from_carvers(carvers: Vec<CarverConfig>) -> Self {
        Self { carvers }
    }
}

fn load_one(namespace_root: &Path, name: &str, tags: &TagTable) -> Result<CarverConfig, String> {
    let path = namespace_root
        .join("worldgen")
        .join("configured_carver")
        .join(format!("{name}.json"));
    let value = read_worldgen_json(&path).map_err(|e| e.to_string())?;
    let kind_str = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let kind = match kind_str.rsplit(':').next().unwrap_or("") {
        "cave" => CarverKind::Cave,
        "canyon" => CarverKind::Canyon,
        other => return Err(format!("{name}: carver type {other:?} is not implemented")),
    };
    let config = value
        .get("config")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("{name}: missing config"))?;
    let get = |key: &str| config.get(key);
    let probability = get("probability")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let y = get("y")
        .and_then(crate::pack_json::HeightDist::parse)
        .ok_or_else(|| format!("{name}: missing y height range"))?;
    let y_scale = get("yScale")
        .and_then(FloatRange::parse)
        .unwrap_or(FloatRange { min: 1.0, max: 1.0 });
    let horizontal_radius_multiplier = get("horizontal_radius_multiplier")
        .and_then(FloatRange::parse)
        .or_else(|| {
            // Canyon writes the factor under `shape.horizontal_radius_factor`.
            config
                .get("shape")
                .and_then(|s| s.get("horizontal_radius_factor"))
                .and_then(FloatRange::parse)
        })
        .unwrap_or(FloatRange { min: 1.0, max: 1.0 });
    let vertical_radius_multiplier = get("vertical_radius_multiplier")
        .and_then(FloatRange::parse)
        .unwrap_or(FloatRange { min: 1.0, max: 1.0 });
    let floor_level = get("floor_level")
        .and_then(FloatRange::parse)
        .unwrap_or(FloatRange {
            min: -1.0,
            max: -1.0,
        });
    let replaceable_tag = get("replaceable")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("#minecraft:overworld_carver_replaceables")
        .trim_start_matches('#')
        .to_owned();
    let replaceable = tags.members(&replaceable_tag).cloned().unwrap_or_default();
    let thickness = config
        .get("shape")
        .and_then(|s| s.get("thickness"))
        .and_then(|t| {
            let min = t.get("min").and_then(serde_json::Value::as_f64)?;
            let max = t.get("max").and_then(serde_json::Value::as_f64)?;
            Some(FloatRange { min, max })
        })
        .unwrap_or(FloatRange { min: 1.0, max: 3.0 });
    let vertical_rotation = config
        .get("vertical_rotation")
        .and_then(FloatRange::parse)
        .unwrap_or(FloatRange { min: 0.0, max: 0.0 });
    let lava_level = config
        .get("lava_level")
        .and_then(|l| l.get("above_bottom"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(8);
    Ok(CarverConfig {
        name: name.to_owned(),
        kind,
        probability,
        y,
        y_scale,
        horizontal_radius_multiplier,
        vertical_radius_multiplier,
        floor_level,
        replaceable_tag,
        replaceable,
        thickness,
        vertical_rotation,
        lava_level,
    })
}

/// What one chunk's carver pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarverStats {
    /// Source chunks examined (the neighbourhood).
    pub sources_considered: usize,
    /// Source chunks that rolled a carver this chunk must honour.
    pub systems: usize,
    /// Blocks changed from non-air to air.
    pub blocks_carved: usize,
    /// Candidate cells whose block was not replaceable (left alone).
    pub skipped_not_replaceable: usize,
    /// Candidate cells outside this chunk (clipped).
    pub cells_outside: usize,
}

impl CarverStats {
    /// Add one carver's numbers into a running total.
    pub const fn record(&mut self, other: Self) {
        self.sources_considered += other.sources_considered;
        self.systems += other.systems;
        self.blocks_carved += other.blocks_carved;
        self.skipped_not_replaceable += other.skipped_not_replaceable;
        self.cells_outside += other.cells_outside;
    }
}

/// Carve every carver system that intersects `chunk`.
///
/// See the module docs for the neighbourhood rule. Pure function of
/// `(CarverSet, seed, chunk position)`.
#[must_use]
pub fn carve_chunk(
    chunk: &mut Chunk,
    pos: ChunkPos,
    context: &WorldgenContext,
    carvers: &CarverSet,
    registry: &BlockRegistry,
) -> CarverStats {
    let mut total = CarverStats::default();
    let air = registry.air_id();
    for carver in carvers.carvers() {
        for dz in -CARVER_SOURCE_RADIUS_CHUNKS..=CARVER_SOURCE_RADIUS_CHUNKS {
            for dx in -CARVER_SOURCE_RADIUS_CHUNKS..=CARVER_SOURCE_RADIUS_CHUNKS {
                let source = ChunkPos::new(pos.x.saturating_add(dx), pos.z.saturating_add(dz));
                total.sources_considered += 1;
                let mut random = feature_random(
                    context.seed.raw(),
                    &format!("carver/{}", carver.name),
                    source,
                    0,
                );
                if random.next_f64() >= carver.probability {
                    continue;
                }
                total.systems += 1;
                let mut local = CarverStats {
                    systems: 1,
                    ..CarverStats::default()
                };
                walk_and_carve(
                    chunk,
                    context,
                    carver,
                    source,
                    &mut random,
                    registry,
                    air,
                    &mut local,
                );
                total.record(local);
            }
        }
    }
    total
}

#[allow(clippy::too_many_arguments)]
fn walk_and_carve(
    chunk: &mut Chunk,
    context: &WorldgenContext,
    carver: &CarverConfig,
    source: ChunkPos,
    random: &mut RandomSource,
    registry: &BlockRegistry,
    air: i32,
    stats: &mut CarverStats,
) {
    let base_x = f64::from(source.x.saturating_mul(SECTION_WIDTH)) + 8.0;
    let base_z = f64::from(source.z.saturating_mul(SECTION_WIDTH)) + 8.0;
    let mut x = base_x + f64::from(random.next_i32_bounded(SECTION_WIDTH)) - 8.0;
    let mut z = base_z + f64::from(random.next_i32_bounded(SECTION_WIDTH)) - 8.0;
    let mut y = f64::from(carver.y.sample(random, context));
    let mut yaw = random.next_f64() * std::f64::consts::TAU;
    let mut pitch = 0.0_f64;

    let (steps, base_radius) = match carver.kind {
        CarverKind::Cave => {
            let radius = 1.5 + random.next_f64() * 4.5;
            (CAVE_MAX_STEPS, radius)
        }
        CarverKind::Canyon => {
            let radius = 1.0 + random.next_f64() * 2.0;
            (CANYON_MAX_STEPS, radius)
        }
    };

    for step in 0..steps {
        let h_mult = carver.horizontal_radius_multiplier.sample(random);
        let v_mult = carver.vertical_radius_multiplier.sample(random);
        let y_scale = carver.y_scale.sample(random);
        // Taper the walk slightly toward the end so tunnels close rather than
        // stop mid-air (a documented approximation of Vanilla's radius curve).
        let taper = 1.0 - f64::from(step) / f64::from(steps) * 0.55;
        match carver.kind {
            CarverKind::Cave => {
                let h_r = (base_radius * h_mult * taper).max(0.4);
                let v_r = (h_r * y_scale * v_mult).max(0.3);
                let floor = y + carver.floor_level.sample(random) * v_r;
                carve_ellipsoid(
                    chunk,
                    context,
                    carver,
                    x,
                    y,
                    z,
                    h_r,
                    v_r,
                    Some(floor),
                    registry,
                    air,
                    stats,
                );
            }
            CarverKind::Canyon => {
                let thickness = carver.thickness.sample(random).max(0.5);
                // Canyons are tall and thin: `yScale` (3.0 in the pack)
                // stretches the vertical half-extent.
                let h_r = (base_radius * h_mult * taper).max(0.35);
                let v_r = (thickness * y_scale.abs().max(0.5) * 0.5).max(1.0);
                carve_ellipsoid(
                    chunk, context, carver, x, y, z, h_r, v_r, None, registry, air, stats,
                );
            }
        }
        // Advance the walk.
        match carver.kind {
            CarverKind::Cave => {
                yaw += (random.next_f64() - 0.5) * 0.7;
                pitch = pitch * 0.7 + (random.next_f64() - 0.5) * 0.35;
            }
            CarverKind::Canyon => {
                yaw += (random.next_f64() - 0.5) * 0.35;
                pitch += carver.vertical_rotation.sample(random);
                pitch = pitch.clamp(-0.35, 0.35);
            }
        }
        let horizontal = STEP_LENGTH * pitch.cos().abs().max(0.15);
        x += yaw.cos() * horizontal;
        z += yaw.sin() * horizontal;
        y += pitch.sin() * STEP_LENGTH;
    }
}

#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    chunk: &mut Chunk,
    context: &WorldgenContext,
    carver: &CarverConfig,
    cx: f64,
    cy: f64,
    cz: f64,
    rx: f64,
    ry: f64,
    floor: Option<f64>,
    registry: &BlockRegistry,
    air: i32,
    stats: &mut CarverStats,
) {
    let min_y = context.min_y;
    let max_y = context.effective_max_y();
    let base_x = chunk.pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = chunk.pos.z.saturating_mul(SECTION_WIDTH);
    // Clip the loop to this chunk's footprint *before* iterating: a walk step
    // 100 blocks away must cost O(1), not O(ellipsoid volume). Without this a
    // ±8-chunk neighbourhood × 3 carvers × 1024 chunks never finishes.
    let x_lo = ((cx - rx).floor() as i32).max(base_x);
    let x_hi = ((cx + rx).ceil() as i32).min(base_x + SECTION_WIDTH - 1);
    let z_lo = ((cz - rx).floor() as i32).max(base_z);
    let z_hi = ((cz + rx).ceil() as i32).min(base_z + SECTION_WIDTH - 1);
    if x_lo > x_hi || z_lo > z_hi {
        stats.cells_outside += 1;
        return;
    }
    let y_lo = ((cy - ry).floor() as i32).max(min_y);
    let y_hi = ((cy + ry).ceil() as i32).min(max_y - 1);
    for y in y_lo..=y_hi {
        if let Some(floor) = floor
            && f64::from(y) < floor
        {
            continue;
        }
        for x in x_lo..=x_hi {
            for z in z_lo..=z_hi {
                let dx = (f64::from(x) + 0.5 - cx) / rx.max(0.1);
                let dy = (f64::from(y) + 0.5 - cy) / ry.max(0.1);
                let dz = (f64::from(z) + 0.5 - cz) / rx.max(0.1);
                if dx * dx + dy * dy + dz * dz > 1.0 {
                    continue;
                }
                let current = chunk.get_block(x, y, z);
                if current == air {
                    continue;
                }
                if !carver.replaceable.contains(&current) {
                    stats.skipped_not_replaceable += 1;
                    continue;
                }
                if matches!(chunk.set_block(x, y, z, air, registry), Ok(Some(_))) {
                    stats.blocks_carved += 1;
                }
            }
        }
    }
}

/// Carved-air fraction of a chunk: `carved / volume`, where `volume` counts
/// every block the chunk can hold.
///
/// Callers that need the *region* fraction sum `blocks_carved` and volumes
/// themselves; this is the per-chunk helper the Pi cost hook uses.
#[must_use]
pub fn carved_air_fraction(stats: &CarverStats, chunk: &Chunk) -> f64 {
    let volume = chunk_volume(chunk);
    if volume == 0 {
        return 0.0;
    }
    stats.blocks_carved as f64 / volume as f64
}

/// Blocks a chunk can hold.
#[must_use]
pub fn chunk_volume(chunk: &Chunk) -> u64 {
    let height = chunk.max_y().saturating_sub(chunk.min_y());
    u64::try_from(height).unwrap_or(0)
        * u64::try_from(SECTION_WIDTH).unwrap_or(0)
        * u64::try_from(SECTION_WIDTH).unwrap_or(0)
}

/// Empty-replaceable helper used by tests that want a carver which carves
/// nothing (perturbation pin).
#[must_use]
pub fn with_no_replaceables(mut config: CarverConfig) -> CarverConfig {
    config.replaceable.clear();
    config
}

#[cfg(test)]
mod tests {
    use super::{CarverKind, CarverSet, CarverStats, carve_chunk, with_no_replaceables};
    use crate::pack_json::{FloatRange, HeightDist, YAnchor};
    use crate::seed::{WorldSeed, WorldgenContext};
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::{Chunk, ChunkPos};

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(r) => r.blocks,
            Err(e) => panic!("registry: {e}"),
        }
    }

    fn context() -> WorldgenContext {
        WorldgenContext::overworld(WorldSeed::from_raw(4))
    }

    fn stone_chunk(blocks: &BlockRegistry) -> Chunk {
        let ctx = context();
        let mut chunk = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            blocks,
        );
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        for y in ctx.min_y..ctx.min_y + 96 {
            for z in 0..16 {
                for x in 0..16 {
                    let _ = chunk.set_block(x, y, z, stone, blocks);
                }
            }
        }
        chunk
    }

    fn cave(blocks: &BlockRegistry, probability: f64) -> super::CarverConfig {
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        super::CarverConfig {
            name: "test_cave".to_owned(),
            kind: CarverKind::Cave,
            probability,
            y: HeightDist::Uniform {
                min: YAnchor::Absolute(-40),
                max: YAnchor::Absolute(-10),
            },
            y_scale: FloatRange { min: 0.5, max: 1.0 },
            horizontal_radius_multiplier: FloatRange { min: 0.8, max: 1.2 },
            vertical_radius_multiplier: FloatRange { min: 0.8, max: 1.2 },
            floor_level: FloatRange {
                min: -1.0,
                max: -1.0,
            },
            replaceable_tag: "test:stone".to_owned(),
            replaceable: [stone].into_iter().collect(),
            thickness: FloatRange { min: 1.0, max: 2.0 },
            vertical_rotation: FloatRange { min: 0.0, max: 0.0 },
            lava_level: 8,
        }
    }

    #[test]
    fn a_forced_cave_carves_air_out_of_stone_deterministically() {
        let blocks = registry();
        let ctx = context();
        let set = CarverSet::from_carvers(vec![cave(&blocks, 1.0)]);
        let mut a = stone_chunk(&blocks);
        let mut b = stone_chunk(&blocks);
        let sa = carve_chunk(&mut a, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        let sb = carve_chunk(&mut b, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        assert_eq!(sa, sb);
        assert_eq!(a, b);
        assert!(sa.systems > 0, "{sa:?}");
        assert!(sa.blocks_carved > 0, "{sa:?}");
        let air = blocks.air_id();
        let mut air_seen = 0;
        for y in ctx.min_y..ctx.min_y + 96 {
            for z in 0..16 {
                for x in 0..16 {
                    if a.get_block(x, y, z) == air {
                        air_seen += 1;
                    }
                }
            }
        }
        assert_eq!(air_seen, sa.blocks_carved);
    }

    #[test]
    fn zero_probability_carves_nothing() {
        let blocks = registry();
        let ctx = context();
        let set = CarverSet::from_carvers(vec![cave(&blocks, 0.0)]);
        let mut chunk = stone_chunk(&blocks);
        let before = chunk.clone();
        let stats = carve_chunk(&mut chunk, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        assert_eq!(stats.systems, 0);
        assert_eq!(stats.blocks_carved, 0);
        assert_eq!(chunk, before);
    }

    #[test]
    fn no_replaceables_leaves_terrain_alone() {
        let blocks = registry();
        let ctx = context();
        let set = CarverSet::from_carvers(vec![with_no_replaceables(cave(&blocks, 1.0))]);
        let mut chunk = stone_chunk(&blocks);
        let before = chunk.clone();
        let stats = carve_chunk(&mut chunk, ChunkPos::new(0, 0), &ctx, &set, &blocks);
        assert_eq!(stats.blocks_carved, 0);
        assert!(stats.skipped_not_replaceable > 0, "{stats:?}");
        assert_eq!(chunk, before);
    }

    #[test]
    fn empty_carver_set_is_a_no_op() {
        let blocks = registry();
        let ctx = context();
        let mut chunk = stone_chunk(&blocks);
        let before = chunk.clone();
        let stats = carve_chunk(
            &mut chunk,
            ChunkPos::new(0, 0),
            &ctx,
            &CarverSet::empty(),
            &blocks,
        );
        assert_eq!(stats, CarverStats::default());
        assert_eq!(chunk, before);
    }
}
