//! Seeded surface lakes: small water basins carved into terrain (P20-01b).
//!
//! ## What is implemented
//!
//! One lake per chunk at most, rolled from `feature_random(seed, "lake", pos)`:
//! a shallow ellipsoid bowl dug at the surface and filled with water up to one
//! below the rim. Lakes only form on land above sea level (ocean columns are
//! already water) and below `sea_level + 32` (a documented approximation —
//! Vanilla's aquifer-driven lakes have no such ceiling, but without a density
//! field a high-mountain lake would read as a floating error rather than a
//! feature).
//!
//! ## What is **not** implemented (named, not hidden)
//!
//! - **Lava lakes.** Vanilla decorates deserts and badlands with lava pools;
//!   every lake here is water. Lava still appears as carver floors
//!   ([`crate::carver`]).
//! - **Pack-driven placement.** Vanilla's lake placement is driven by the
//!   noise router and aquifer; this module uses one seeded chance per chunk
//!   with fixed radius/depth bands, exactly like the oak-tree pass it sits
//!   next to. Same-seed determinism holds; Vanilla-identity does not.
//! - **Multi-chunk lakes.** A lake whose bowl crosses the chunk border is
//!   clipped, for the same reason structures refuse to cross
//!   ([`crate::placement`]): `mc_world::Chunk` cannot write into a neighbour.
//! - **Springs and aquifers.** No outflow, no water table; a dry carver below
//!   sea level floods from [`crate::carver`]'s fill rule instead.
//!
//! ## Constants and their labels
//!
//! | Constant | Value | Label |
//! |---|---|---|
//! | `LAKE_CHANCE_PER_CHUNK` | 1/64 | **product decision** (ours; sets lake density) |
//! | `LAKE_RADIUS_MIN/MAX` | 3/6 | **product decision** (single-chunk fit) |
//! | `LAKE_DEPTH_MIN/MAX` | 2/3 | **product decision** (shallow basins only) |
//! | `LAKE_MAX_ELEVATION_ABOVE_SEA` | 32 | **approximation** (see above) |
//! | Water state | `minecraft:water` default | **verified** (source block, via registry) |

use crate::pack_json::feature_random;
use crate::seed::WorldgenContext;
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos, SECTION_WIDTH};

/// Chance that a chunk holds a lake (per-chunk roll, seeded).
///
/// **product decision**: 1/64 keeps lakes visible on a walk without flooding
/// lowlands; the number is ours, not Vanilla's.
pub const LAKE_CHANCE_PER_CHUNK: f64 = 1.0 / 64.0;

/// Horizontal half-extent band of a lake bowl, in blocks.
///
/// **product decision**: capped so a centered bowl usually fits one chunk.
pub const LAKE_RADIUS_MIN: i32 = 3;
/// See [`LAKE_RADIUS_MIN`].
pub const LAKE_RADIUS_MAX: i32 = 6;

/// Vertical half-extent band of a lake bowl, in blocks.
///
/// **product decision**: shallow basins only; nothing here models a deep lake.
pub const LAKE_DEPTH_MIN: i32 = 2;
/// See [`LAKE_DEPTH_MIN`].
pub const LAKE_DEPTH_MAX: i32 = 3;

/// How far above sea level a lake may still form.
///
/// **approximation**: Vanilla has no such ceiling; this bounds the feature to
/// lowlands where a bowl reads as a lake rather than a mountaintop error.
pub const LAKE_MAX_ELEVATION_ABOVE_SEA: i32 = 32;

/// What one chunk's lake pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LakeStats {
    /// Chunks examined (always 1 for a single-chunk call).
    pub chunks_considered: usize,
    /// Lakes actually dug (0 or 1).
    pub lakes_placed: usize,
    /// Terrain blocks removed for the bowl.
    pub blocks_dug: usize,
    /// Water blocks placed in the bowl.
    pub blocks_water: usize,
}

/// Dig this chunk's lake, if any.
///
/// Pure function of `(seed, chunk position)` plus the chunk's post-carver
/// state (the surface is read from the blocks, not from a height field), so a
/// chunk's lake is the same however the world is generated (AGENTS.md §3.6).
#[must_use]
pub fn place_lakes(
    chunk: &mut Chunk,
    pos: ChunkPos,
    context: &WorldgenContext,
    registry: &BlockRegistry,
) -> LakeStats {
    place_lakes_with_chance(chunk, pos, context, registry, LAKE_CHANCE_PER_CHUNK)
}

/// [`place_lakes`] with an explicit chance (tests and perturbation pins).
#[must_use]
pub fn place_lakes_with_chance(
    chunk: &mut Chunk,
    pos: ChunkPos,
    context: &WorldgenContext,
    registry: &BlockRegistry,
    chance: f64,
) -> LakeStats {
    let mut stats = LakeStats {
        chunks_considered: 1,
        ..LakeStats::default()
    };
    let mut random = feature_random(context.seed.raw(), "lake", pos, 0);
    if random.next_f64() >= chance {
        return stats;
    }
    let Ok(water) = registry.default_state("minecraft:water") else {
        return stats;
    };
    let bedrock = registry.default_state("minecraft:bedrock").ok();
    let air = registry.air_id();
    let base_x = pos.x.saturating_mul(SECTION_WIDTH);
    let base_z = pos.z.saturating_mul(SECTION_WIDTH);
    let cx = base_x + random.next_i32_bounded(SECTION_WIDTH);
    let cz = base_z + random.next_i32_bounded(SECTION_WIDTH);
    let radius = LAKE_RADIUS_MIN + random.next_i32_bounded(LAKE_RADIUS_MAX - LAKE_RADIUS_MIN + 1);
    let depth = LAKE_DEPTH_MIN + random.next_i32_bounded(LAKE_DEPTH_MAX - LAKE_DEPTH_MIN + 1);
    let top = context.effective_max_y() - 1;
    // The rim: the highest non-air block of the center column.
    let Some(rim) = surface_y(chunk, cx, cz, context.min_y, top, air) else {
        return stats;
    };
    if rim <= context.sea_level {
        return stats;
    }
    if rim > context.sea_level + LAKE_MAX_ELEVATION_ABOVE_SEA {
        return stats;
    }
    let center_y = rim - 1;
    let waterline = center_y;
    let rx = f64::from(radius);
    let ry = f64::from(depth);
    let x_lo = (cx - radius).max(base_x);
    let x_hi = (cx + radius).min(base_x + SECTION_WIDTH - 1);
    let z_lo = (cz - radius).max(base_z);
    let z_hi = (cz + radius).min(base_z + SECTION_WIDTH - 1);
    let y_lo = (center_y - depth).max(context.min_y);
    let y_hi = (center_y + depth).min(top);
    let mut dug = 0_usize;
    let mut watered = 0_usize;
    for y in y_lo..=y_hi {
        for x in x_lo..=x_hi {
            for z in z_lo..=z_hi {
                let dx = (f64::from(x) + 0.5 - (f64::from(cx) + 0.5)) / rx.max(0.1);
                let dy = (f64::from(y) + 0.5 - (f64::from(center_y) + 0.5)) / ry.max(0.1);
                let dz = (f64::from(z) + 0.5 - (f64::from(cz) + 0.5)) / rx.max(0.1);
                if dx * dx + dy * dy + dz * dz > 1.0 {
                    continue;
                }
                let current = chunk.get_block(x, y, z);
                if Some(current) == bedrock {
                    continue;
                }
                if current == air && y > waterline {
                    continue;
                }
                let fill = if y <= waterline { water } else { air };
                if current == fill {
                    continue;
                }
                if matches!(chunk.set_block(x, y, z, fill, registry), Ok(Some(_))) {
                    if fill == water {
                        watered += 1;
                    } else {
                        dug += 1;
                    }
                }
            }
        }
    }
    if dug + watered == 0 {
        return stats;
    }
    stats.lakes_placed = 1;
    stats.blocks_dug = dug;
    stats.blocks_water = watered;
    stats
}

/// Highest non-air block y of a column, or `None` for an all-air column.
fn surface_y(chunk: &Chunk, x: i32, z: i32, min_y: i32, top: i32, air: i32) -> Option<i32> {
    (min_y..=top)
        .rev()
        .find(|&y| chunk.get_block(x, y, z) != air)
}

#[cfg(test)]
mod tests {
    use super::{LAKE_CHANCE_PER_CHUNK, place_lakes, place_lakes_with_chance};
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

    /// Land chunk: stone up to y=70 (above sea 63), air above.
    fn land_chunk(blocks: &BlockRegistry) -> Chunk {
        let ctx = context();
        let mut chunk = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            blocks,
        );
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        for y in ctx.min_y..70 {
            for z in 0..16 {
                for x in 0..16 {
                    let _ = chunk.set_block(x, y, z, stone, blocks);
                }
            }
        }
        chunk
    }

    #[test]
    fn zero_chance_places_nothing() {
        let blocks = registry();
        let ctx = context();
        let mut chunk = land_chunk(&blocks);
        let before = chunk.clone();
        let stats = place_lakes_with_chance(&mut chunk, ChunkPos::new(0, 0), &ctx, &blocks, 0.0);
        assert_eq!(stats.lakes_placed, 0);
        assert_eq!(stats.blocks_water, 0);
        assert_eq!(chunk, before);
    }

    #[test]
    fn a_forced_lake_digs_a_water_basin_deterministically() {
        let blocks = registry();
        let ctx = context();
        let water = blocks.default_state("minecraft:water").expect("water");
        let mut a = land_chunk(&blocks);
        let mut b = land_chunk(&blocks);
        let sa = place_lakes_with_chance(&mut a, ChunkPos::new(0, 0), &ctx, &blocks, 1.0);
        let sb = place_lakes_with_chance(&mut b, ChunkPos::new(0, 0), &ctx, &blocks, 1.0);
        assert_eq!(sa, sb, "same seed and chunk repeat exactly");
        assert_eq!(a, b);
        assert_eq!(sa.lakes_placed, 1, "{sa:?}");
        assert!(sa.blocks_water > 0, "{sa:?}");
        let mut water_seen = 0;
        for y in ctx.min_y..ctx.effective_max_y() {
            for z in 0..16 {
                for x in 0..16 {
                    if a.get_block(x, y, z) == water {
                        water_seen += 1;
                    }
                }
            }
        }
        assert_eq!(water_seen, sa.blocks_water, "every placed water is counted");
    }

    #[test]
    fn ocean_columns_are_skipped_because_the_sea_is_already_there() {
        let blocks = registry();
        let ctx = context();
        let mut chunk = Chunk::air(
            ChunkPos::new(0, 0),
            ctx.min_section_y(),
            ctx.section_count(),
            &blocks,
        );
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        // Seabed at y=50, below sea level 63: an ocean column.
        for y in ctx.min_y..50 {
            for z in 0..16 {
                for x in 0..16 {
                    let _ = chunk.set_block(x, y, z, stone, &blocks);
                }
            }
        }
        let stats = place_lakes_with_chance(&mut chunk, ChunkPos::new(0, 0), &ctx, &blocks, 1.0);
        assert_eq!(stats.lakes_placed, 0, "{stats:?}");
        assert_eq!(stats.blocks_water, 0, "{stats:?}");
    }

    #[test]
    fn the_product_chance_is_a_real_probability() {
        assert!(
            (0.0..1.0).contains(&LAKE_CHANCE_PER_CHUNK),
            "{LAKE_CHANCE_PER_CHUNK}"
        );
    }

    #[test]
    fn public_entry_uses_the_product_chance() {
        // Smoke: the public entry runs without panic on land; the outcome
        // (lake or not) is the seeded roll, not asserted here.
        let blocks = registry();
        let ctx = context();
        let mut chunk = land_chunk(&blocks);
        let _ = place_lakes(&mut chunk, ChunkPos::new(5, -3), &ctx, &blocks);
    }
}
