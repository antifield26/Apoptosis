//! P20-01b acceptance: lakes appear in generated terrain, deterministically.
//!
//! ## Tolerances — written **before** the run
//!
//! | Metric | Band | Why this band |
//! |---|---|---|
//! | lakes in a 32×32 region (chance 1/64) | `[LAKES_MIN, LAKES_MAX]` | mean 1024/64 = 16; the floor (~1/8 of mean) catches a dead feature, the ceiling (4× mean) catches a runaway writer. Both are regression guards, not Vanilla numbers — our terrain is still the P07 noise baseline (P21-00b owns the terrain model), so no vanilla-region count is asserted here. |
//! | same seed, two runs | **exact** | AGENTS.md §3.6 — not a tolerance at all. |
//!
//! ## Running the acceptance measurement
//!
//! ```text
//! cargo test -p mc-worldgen --test lake_stats --release -- --ignored --nocapture
//! ```
//!
//! `smoke_lakes_are_deterministic_and_watery` is the always-on 4×4 cut-down
//! (forced chance, so no flake): a plain `cargo test -p mc-worldgen` proves
//! lakes dig basins, place source water, repeat bit-exactly, and go dry when
//! neutralised.

use mc_registry::Registries;
use mc_world::ChunkPos;
use mc_worldgen::seed::WorldgenContext;
use mc_worldgen::terrain::ChunkGenerator;
use mc_worldgen::{CarverSet, LakeStats, TagTable, WorldSeed, carve_chunk, place_lakes};
use std::path::PathBuf;

/// Region edge for the ignored acceptance (32×32 = 1024 chunks).
const REGION_CHUNKS: i32 = 32;

/// Seed for every measurement in this file (the evidence-world seed).
const SEED: i64 = 1_361_882_806;

/// Minimum lakes in the 32×32 region (~1/8 of the 16-lake mean).
const LAKES_MIN: usize = 2;

/// Maximum lakes in the 32×32 region (4× the 16-lake mean).
const LAKES_MAX: usize = 64;

/// The extracted `data/minecraft` directory (same rule as
/// `tests/ore_carver_stats.rs`: `MC_VANILLA_DATA` wins, else the
/// `target/vanilla-26.1.2/extract` fallback).
fn pack_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("MC_VANILLA_DATA") {
        let root = PathBuf::from(root);
        if root.is_dir() {
            return Some(root);
        }
    }
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = crate_dir.join("../../target/vanilla-26.1.2/extract/data/minecraft");
    if candidate.is_dir() && candidate.join("worldgen").is_dir() {
        return Some(candidate);
    }
    None
}

macro_rules! pack_or_skip {
    () => {
        match pack_root() {
            Some(root) => root,
            None => {
                eprintln!(
                    "MC_VANILLA_DATA is not set and target/vanilla-26.1.2/extract is missing; skipping"
                );
                return;
            }
        }
    };
}

/// Generate `edge × edge` chunks through the production pipeline
/// (terrain → carvers → lakes) and sum the lake stats.
fn measure_region(edge: i32, seed: i64, with_lakes: bool) -> (LakeStats, usize) {
    let root = pack_root().expect("vanilla data for the lake acceptance");
    let registries = Registries::vanilla().expect("registry fixture");
    let blocks = &registries.blocks;
    let tags = TagTable::load_block_tags(&root, blocks);
    let (carvers, carver_skipped) = CarverSet::load_overworld(&root, &tags);
    assert!(carver_skipped.is_empty(), "{carver_skipped:?}");
    let context = WorldgenContext::overworld(WorldSeed::from_raw(seed));
    let generator = mc_worldgen::TerrainGenerator::new(context.clone(), blocks).expect("gen");
    let mut total = LakeStats::default();
    let mut chunks = 0_usize;
    for cz in 0..edge {
        for cx in 0..edge {
            let pos = ChunkPos::new(cx, cz);
            let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
            let _ = carve_chunk(&mut chunk, pos, &context, &carvers, blocks);
            if with_lakes {
                let stats = place_lakes(&mut chunk, pos, &context, blocks);
                total.chunks_considered += stats.chunks_considered;
                total.lakes_placed += stats.lakes_placed;
                total.blocks_dug += stats.blocks_dug;
                total.blocks_water += stats.blocks_water;
            }
            chunks += 1;
        }
    }
    (total, chunks)
}

/// Always-on 4×4 smoke at forced chance: basins are dug, water is source
/// blocks, the same seed repeats bit-exactly, and neutralising the pass
/// leaves the terrain dry.
#[test]
fn smoke_lakes_are_deterministic_and_watery() {
    use mc_worldgen::lake::place_lakes_with_chance;
    let registries = Registries::vanilla().expect("registry fixture");
    let blocks = &registries.blocks;
    let context = WorldgenContext::overworld(WorldSeed::from_raw(SEED));
    let generator = mc_worldgen::TerrainGenerator::new(context.clone(), blocks).expect("gen");
    let water = blocks.default_state("minecraft:water").expect("water");

    let mut lakes = 0_usize;
    let mut water_cells = 0_usize;
    for cz in 0..4 {
        for cx in 0..4 {
            let pos = ChunkPos::new(cx, cz);
            let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
            let stats = place_lakes_with_chance(&mut chunk, pos, &context, blocks, 1.0);
            lakes += stats.lakes_placed;
            water_cells += stats.blocks_water;
            // Bit-stability: the same position generates the same lake.
            let mut again = generator.generate_chunk(pos, blocks).expect("chunk");
            let again_stats = place_lakes_with_chance(&mut again, pos, &context, blocks, 1.0);
            assert_eq!(chunk, again, "chunk {pos:?} must be bit-stable");
            assert_eq!(stats, again_stats, "lake stats {pos:?} must repeat");
            // Every counted water block is really water in the chunk.
            let mut seen = 0_usize;
            for y in chunk.min_y()..chunk.max_y() {
                for z in 0..16 {
                    for x in 0..16 {
                        if chunk.get_block(x, y, z) == water {
                            seen += 1;
                        }
                    }
                }
            }
            assert!(
                seen >= stats.blocks_water,
                "counted water must be in the chunk ({seen} < {})",
                stats.blocks_water
            );
        }
    }
    assert!(lakes > 0, "forced chance must place lakes on land");
    assert!(water_cells > 0, "lakes must hold water");

    // Neutralise pin at smoke scale: chance 0.0 digs nothing.
    let pos = ChunkPos::new(0, 0);
    let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
    let before = chunk.clone();
    let stats = place_lakes_with_chance(&mut chunk, pos, &context, blocks, 0.0);
    assert_eq!(stats.lakes_placed, 0);
    assert_eq!(stats.blocks_water, 0);
    assert_eq!(chunk, before);
}

/// Acceptance (ignored): lakes appear across a lived region inside the
/// pre-written band.
#[test]
#[ignore = "32×32 lake acceptance; run --release -- --ignored"]
fn lakes_appear_inside_the_stated_tolerance() {
    let _root = pack_or_skip!();
    let (stats, chunks) = measure_region(REGION_CHUNKS, SEED, true);
    eprintln!(
        "region {}×{} ({} chunks): lakes={} dug={} water={}",
        REGION_CHUNKS,
        REGION_CHUNKS,
        chunks,
        stats.lakes_placed,
        stats.blocks_dug,
        stats.blocks_water
    );
    assert_eq!(chunks, 1024);
    assert!(
        (LAKES_MIN..=LAKES_MAX).contains(&stats.lakes_placed),
        "lakes {} outside pre-written band [{}, {}]",
        stats.lakes_placed,
        LAKES_MIN,
        LAKES_MAX
    );
    assert!(stats.blocks_water > 0, "lakes must hold water");
}

/// Acceptance (ignored): the same seed twice gives identical lake stats.
#[test]
#[ignore = "32×32 lake acceptance; run --release -- --ignored"]
fn the_same_seed_twice_gives_identical_lakes() {
    let _root = pack_or_skip!();
    let (a, _) = measure_region(8, SEED, true);
    let (b, _) = measure_region(8, SEED, true);
    assert_eq!(a, b, "same seed must give the same lakes");
}

/// Perturbation pin (ignored): skipping the lake pass leaves no lake water.
#[test]
#[ignore = "32×32 lake acceptance; run --release -- --ignored"]
fn neutralising_the_lake_pass_turns_the_count_red() {
    let _root = pack_or_skip!();
    let (real, _) = measure_region(8, SEED, true);
    assert!(
        real.lakes_placed > 0 && real.blocks_water > 0,
        "precondition: the live pass places lakes ({real:?})"
    );
    let (neutral, _) = measure_region(8, SEED, false);
    assert_eq!(neutral.lakes_placed, 0, "no pass, no lakes");
    assert_eq!(neutral.blocks_water, 0, "no pass, no lake water");
}
