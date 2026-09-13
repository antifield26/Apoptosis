//! What does the terrain actually look like? A distribution, not a spot check.
//!
//! The owner reports that the world has no trees, no grass and no biome variety — only stone, water and sand —
//! and the blocks confirm it: the chunks the server sent use exactly three states, `stone`, `water` and `sand`.
//!
//! The generator's own rule explains how that happens without any of it being a bug in the light or the
//! palette: **a column whose surface is below sea level is `Ocean`, and ocean's surface is sand over gravel**.
//! So one question decides whether this is a defect or an unlucky spawn: how much of the world is below sea
//! level, and how are the biomes distributed?
//!
//! A spot check cannot answer that — the spawn being an ocean is entirely possible in a working generator. This
//! samples a grid wide enough to see the shape of it.

use mc_registry::Registries;
use mc_worldgen::{OVERWORLD_SEA_LEVEL, TerrainGenerator, WorldSeed, WorldgenContext};

#[test]
#[ignore = "a diagnostic, not an assertion; run with --ignored --nocapture"]
fn the_terrain_distribution() {
    let registries = Registries::vanilla().expect("registry tables");
    let context = WorldgenContext::overworld(WorldSeed::from_raw(0));
    let generator = TerrainGenerator::new(context, &registries.blocks).expect("generator");

    let step = 32;
    let extent = 512;
    let mut samples = 0_u32;
    let mut below_sea = 0_u32;
    let (mut min_h, mut max_h) = (i32::MAX, i32::MIN);
    let mut total = 0_i64;
    let mut biomes: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();

    let mut x = -extent;
    while x <= extent {
        let mut z = -extent;
        while z <= extent {
            let height = generator.surface_height(x, z);
            let biome = generator.biome_source().biome_at_with_height(
                x,
                z,
                height,
                OVERWORLD_SEA_LEVEL,
                100,
            );
            samples += 1;
            if height < OVERWORLD_SEA_LEVEL {
                below_sea += 1;
            }
            min_h = min_h.min(height);
            max_h = max_h.max(height);
            total += i64::from(height);
            *biomes.entry(biome.id().to_owned()).or_default() += 1;
            z += step;
        }
        x += step;
    }

    println!(
        "=== terrain over a {}-block square, every {step} blocks ===",
        extent * 2
    );
    println!("  samples        : {samples}");
    println!("  sea level      : {OVERWORLD_SEA_LEVEL}");
    println!(
        "  height         : min {min_h}, max {max_h}, mean {}",
        total / i64::from(samples)
    );
    println!(
        "  below sea level: {below_sea} of {samples} ({:.1}%)",
        100.0 * f64::from(below_sea) / f64::from(samples)
    );
    println!("  biomes:");
    let mut entries: Vec<_> = biomes.into_iter().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for (name, count) in entries {
        println!(
            "    {name:<24} {count:>5}  ({:.1}%)",
            100.0 * f64::from(count) / f64::from(samples)
        );
    }
}

/// What the generator actually writes, over a strip of chunks on land.
///
/// See the module comment: the height and biome fields can both be perfect while the blocks are not, and the
/// blocks are what a player sees.
#[test]
#[ignore = "a diagnostic, not an assertion; run with --ignored --nocapture"]
fn the_blocks_a_chunk_actually_contains() {
    use mc_persistence::chunk::ChunkPos;
    use mc_worldgen::ChunkGenerator as _;
    use std::collections::BTreeMap;

    let registries = Registries::vanilla().expect("registry tables");
    let context = WorldgenContext::overworld(WorldSeed::from_raw(0));
    let generator = TerrainGenerator::new(context.clone(), &registries.blocks).expect("generator");
    let blocks = &registries.blocks;

    let name_of = |state: i32| -> String {
        for name in [
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:grass_block",
            "minecraft:sand",
            "minecraft:water",
            "minecraft:gravel",
            "minecraft:oak_log",
            "minecraft:oak_leaves",
            "minecraft:coarse_dirt",
            "minecraft:podzol",
            "minecraft:air",
        ] {
            if let Ok(id) = blocks.default_state(name)
                && id == state
            {
                return name.trim_start_matches("minecraft:").to_owned();
            }
        }
        format!("state#{state}")
    };

    let mut census: BTreeMap<String, u32> = BTreeMap::new();
    let mut surfaces: BTreeMap<String, u32> = BTreeMap::new();
    let mut printed = 0_u32;

    for chunk_x in -2..=2 {
        for chunk_z in -2..=2 {
            let pos = ChunkPos::new(chunk_x, chunk_z);
            let chunk = match generator.generate_chunk(pos, blocks) {
                Ok(chunk) => chunk,
                Err(error) => {
                    println!("  chunk {pos:?} failed: {error}");
                    continue;
                }
            };
            let air = blocks.air_id();
            for x in 0..16 {
                for z in 0..16 {
                    let bx = chunk_x * 16 + x;
                    let bz = chunk_z * 16 + z;
                    let mut top = None;
                    for y in (OVERWORLD_SEA_LEVEL - 40..=OVERWORLD_SEA_LEVEL + 60).rev() {
                        let state = chunk.get_block(x, y, z);
                        if state != air && top.is_none() {
                            top = Some((y, state));
                        }
                        if state != air {
                            *census.entry(name_of(state)).or_default() += 1;
                        }
                    }
                    if let Some((y, state)) = top {
                        *surfaces.entry(name_of(state)).or_default() += 1;
                        // A few columns in full, so the shape under the surface is visible too.
                        if printed < 8 && chunk_x == -1 && chunk_z == -1 && x % 5 == 0 && z % 5 == 0
                        {
                            printed += 1;
                            let biome = generator.biome_source().biome_at_with_height(
                                bx,
                                bz,
                                y,
                                OVERWORLD_SEA_LEVEL,
                                100,
                            );
                            println!("--- column ({bx}, {bz}) biome {} top y={y} ---", biome.id());
                            for yy in (y - 4..=y + 2).rev() {
                                println!("    y={yy:>4}  {}", name_of(chunk.get_block(x, yy, z)));
                            }
                        }
                    }
                }
            }
        }
    }

    println!("=== block census over a 5x5 of chunks ===");
    let mut entries: Vec<_> = census.into_iter().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for (name, count) in &entries {
        println!("  {name:<20} {count}");
    }
    println!("=== what is on top of each column ===");
    let mut tops: Vec<_> = surfaces.into_iter().collect();
    tops.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for (name, count) in &tops {
        println!("  {name:<20} {count}");
    }
}
