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
