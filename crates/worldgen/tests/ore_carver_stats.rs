//! P18-03 acceptance: 32×32-chunk ore counts, carved-air fraction, and the
//! neutralise pin.
//!
//! ## Tolerances — written **before** the run
//!
//! The task requires the tolerance numbers to exist before measurement so a
//! pretty result cannot be retro-fitted as a band. These constants were fixed
//! first; the test only asserts them.
//!
//! | Metric | Band | Why this band |
//! |---|---|---|
//! | coal ore blocks (any y) | `[COAL_MIN, COAL_MAX]` | `ore_coal` + `ore_coal_buried` from the pack; see the revision note beside the constants (the first draft's `y=136+` band is empty on our terrain). |
//! | diamond-band ore blocks, y -64..=15 | `[DIAMOND_MIN, DIAMOND_MAX]` | four diamond features, small sizes, several with `discard_chance_on_air_exposure ≥ 0.5`. |
//! | carved-air fraction over the region | `[CARVED_MIN, CARVED_MAX]` | cave 0.15 + extra 0.07 + canyon 0.01 per source chunk over a ±8-chunk neighbourhood. Zero means the carver is dead; a solid moon means the radius is wrong. |
//! | same seed, two runs | **exact** | AGENTS.md §3.6 — not a tolerance at all. |
//!
//! ## Vanilla differential (named, not claimed)
//!
//! PHASE-18 asks for a statistical check against a *vanilla-generated region of
//! the same seed*. No vanilla world region is checked in at this commit, and our
//! terrain model is still the P07 noise baseline rather than Vanilla's density
//! functions (P21-00b owns that decision), so a voxel-level vanilla diff is not
//! meaningful yet. What *is* asserted here: the **pack** counts and
//! distributions drive the generator, the numbers sit in a band derived from
//! those pack parameters, and the same seed is bit-stable. Closing the vanilla
//! region comparison is a named follow-up once P21-00b picks a terrain model.
//!
//! ## Running the acceptance measurement
//!
//! The 32×32 pins are `#[ignore]`d: a debug `cargo test --workspace` would
//! otherwise spend minutes in carver neighbourhood walks. Run the measurement
//! explicitly (release, because debug is ~10× slower):
//!
//! ```text
//! cargo test -p mc-worldgen --test ore_carver_stats --release -- --ignored --nocapture
//! ```
//!
//! `smoke_region` is the always-on 4×4 cut-down of the same pipeline, so a
//! plain `cargo test -p mc-worldgen` still loads the pack, carves, places ores
//! ## Pi single-chunk cost hook
//!
//! `pi_single_chunk_cost` (ignored) times `terrain + carvers + ores` over 64
//! chunks after an 8-chunk warmup and prints `ms/chunk`. On the Pi 5:
//!
//! ```text
//! cargo test -p mc-worldgen --test ore_carver_stats -- --ignored pi_single_chunk_cost --nocapture
//! ```
//!
//! Record hardware, toolchain, commit SHA and the printed `ms/chunk` beside the
//! §13 soak (P18-04). The hook is this test — there is no separate bench
//! binary, so the measurement cannot drift away from the code path it claims
//! to measure.

#![allow(clippy::cast_precision_loss)]

use mc_registry::Registries;
use mc_world::ChunkPos;
use mc_worldgen::seed::WorldgenContext;
use mc_worldgen::terrain::ChunkGenerator;
use mc_worldgen::{
    CarverSet, OreSet, TagTable, WorldSeed, carve_chunk, ore_state_ids, populate_ores,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

// ---------------------------------------------------------------- tolerances
// Written before the run. Do not widen one of these to make a red test green.
//
// Revision note (kept, not deleted): the first draft measured coal in y 136..=319
// because that is `ore_coal_upper`'s pack band. On *our* terrain the surface
// tops out near y = 112 (`TERRAIN_AMPLITUDE` 45 around base 67), so that band
// is empty by construction and the pin was measuring stone that does not exist.
// The metric is now **all coal** (the pack's `ore_coal` + `ore_coal_buried`
// states at any y), which is what `ore_coal_lower` (count 20, size 17,
// trapezoid 0..192) can actually replace. The band below is derived from that
// pack arithmetic before the confirming run.

/// Region edge, in chunks (32×32 = 1024 chunks).
const REGION_CHUNKS: i32 = 32;

/// Seed for every measurement in this file.
const SEED: i64 = 1_361_882_806;

/// Minimum coal ore blocks in the 32×32 region (tolerance floor).
///
/// `ore_coal_lower` draws `count=20` blobs of `size=17` per chunk. A blob
/// replaces on the order of 20–80 stone cells after clip/overlap; 20 × 25 ×
/// 1024 ≈ 500k as a naive mid-estimate. Floor is ~10% of that.
const COAL_MIN: usize = 30_000;

/// Maximum coal ore blocks in the 32×32 region (tolerance ceiling).
/// ~2× the naive mid-estimate, so blob overlap and carve losses cannot hide a
/// runaway writer.
const COAL_MAX: usize = 1_500_000;

/// Y band the diamond family owns after anchor clamp (`above_bottom ±80` → -64..=16).
const DIAMOND_Y: (i32, i32) = (-64, 16);

/// Minimum diamond-band ore blocks (small+medium+large+buried).
///
/// Four features, `7+2+~0.1+4` attempts of sizes `4+8+12+8` → order 150
/// blocks/chunk × 1024 ≈ 150k. Floor ~15% of that.
const DIAMOND_MIN: usize = 20_000;

/// Maximum diamond-band ore blocks (~2.5× the mid-estimate).
const DIAMOND_MAX: usize = 400_000;

/// Minimum carved-air fraction of the region's block volume.
///
/// cave 0.15 + extra 0.07 + canyon 0.01 per source over a ±8-chunk
/// neighbourhood, times a tunnel cross-section of tens of blocks. Zero means
/// the carver is dead; a solid moon means the radius or the replaceable set is
/// wrong.
const CARVED_MIN: f64 = 0.0002;

/// Maximum carved-air fraction of the region's block volume.
const CARVED_MAX: f64 = 0.12;

// ------------------------------------------------------------------ pack root

/// The extracted `data/minecraft` directory.
///
/// `MC_VANILLA_DATA` wins (same variable as `tests/structure_pack.rs`). The
/// fallback walks to the repository `target/` from `CARGO_MANIFEST_DIR`, which
/// is the form that works under `cargo test` (AUDIT-09 D-06: cwd is the crate
/// directory, not the repository root).
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

// ----------------------------------------------------------------- statistics

#[derive(Debug, Default, Clone, PartialEq)]
struct RegionStats {
    /// Ore blocks per 16-block y band (`(y - min_y) / 16`).
    ore_by_band: BTreeMap<i32, usize>,
    /// Coal ore blocks at any y.
    coal: usize,
    /// Diamond ore blocks whose y falls in `DIAMOND_Y`.
    diamond_band: usize,
    /// Blocks the carvers changed from non-air to air.
    carved_air: usize,
    /// Total block volume of the region.
    volume: u64,
    /// Every ore state id seen (for the neutralise pin's sanity).
    ore_ids: BTreeSet<i32>,
}

impl RegionStats {
    fn carved_fraction(&self) -> f64 {
        if self.volume == 0 {
            return 0.0;
        }
        self.carved_air as f64 / self.volume as f64
    }
}

/// Generate the region once and aggregate the P18-03 statistics.
///
/// `ores` / `carvers` are parameters so the neutralise pin can run the *same*
/// measurement with a neutralised assembly.
fn measure_region(ores: &OreSet, carvers: &CarverSet, seed: i64) -> RegionStats {
    let registries = Registries::vanilla().expect("registry fixture");
    let blocks = &registries.blocks;
    let context = WorldgenContext::overworld(WorldSeed::from_raw(seed));
    let generator =
        mc_worldgen::TerrainGenerator::new(context.clone(), blocks).expect("terrain generator");
    let ore_ids = ore_state_ids(ores);
    // Diamond states specifically: the diamond family writes two names.
    let diamond_ids: BTreeSet<i32> = ["minecraft:diamond_ore", "minecraft:deepslate_diamond_ore"]
        .iter()
        .filter_map(|n| blocks.default_state(n).ok())
        .collect();
    // Coal-upper writes coal_ore / deepslate_coal_ore (ore_coal + ore_coal_buried).
    let coal_ids: BTreeSet<i32> = ["minecraft:coal_ore", "minecraft:deepslate_coal_ore"]
        .iter()
        .filter_map(|n| blocks.default_state(n).ok())
        .collect();

    let mut stats = RegionStats {
        ore_ids: ore_ids.clone(),
        ..RegionStats::default()
    };

    for cz in 0..REGION_CHUNKS {
        for cx in 0..REGION_CHUNKS {
            let pos = ChunkPos::new(cx, cz);
            let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
            let carved = carve_chunk(&mut chunk, pos, &context, carvers, blocks);
            stats.carved_air += carved.blocks_carved;
            let _ore_stats = populate_ores(&mut chunk, pos, &context, ores, blocks);
            stats.volume += mc_worldgen::chunk_volume(&chunk);
            // One pass over the chunk's y range: band histogram + the two
            // acceptance-band counts. Three separate scans would triple the
            // `get_block` cost of a 32×32 region.
            for y in chunk.min_y()..chunk.max_y() {
                let band = (y - context.min_y).div_euclid(16);
                for z in 0..16 {
                    for x in 0..16 {
                        let id = chunk.get_block(x, y, z);
                        if coal_ids.contains(&id) {
                            stats.coal += 1;
                        }
                        if (DIAMOND_Y.0..=DIAMOND_Y.1).contains(&y) && diamond_ids.contains(&id) {
                            stats.diamond_band += 1;
                        }
                        if ore_ids.contains(&id) {
                            *stats.ore_by_band.entry(band).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }
    stats
}

// ------------------------------------------------------------------- the pins

/// Acceptance 1 (half A): coal-upper counts sit in the pre-written tolerance.
#[test]
#[ignore = "32×32 acceptance measurement; run --release -- --ignored"]
fn ore_counts_sit_inside_the_stated_tolerance() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let tags = TagTable::load_block_tags(&root, &registries.blocks);
    let (ores, ore_skipped) = OreSet::load_overworld(&root, &registries.blocks, &tags);
    let (carvers, carver_skipped) = CarverSet::load_overworld(&root, &tags);
    assert!(
        ore_skipped.is_empty(),
        "every overworld ore placed feature must load: {ore_skipped:?}"
    );
    assert!(
        carver_skipped.is_empty(),
        "every overworld carver must load: {carver_skipped:?}"
    );
    assert_eq!(ores.len(), 25, "the plains stage-6 ore set");
    assert_eq!(carvers.len(), 3, "cave + cave_extra_underground + canyon");

    let stats = measure_region(&ores, &carvers, SEED);
    eprintln!(
        "region {}×{}: coal={} diamond_band={} carved={} / {} ({:.5}) bands={:?}",
        REGION_CHUNKS,
        REGION_CHUNKS,
        stats.coal,
        stats.diamond_band,
        stats.carved_air,
        stats.volume,
        stats.carved_fraction(),
        stats.ore_by_band
    );
    assert!(
        (COAL_MIN..=COAL_MAX).contains(&stats.coal),
        "coal {} outside pre-written band [{}, {}]",
        stats.coal,
        COAL_MIN,
        COAL_MAX
    );
    assert!(
        (DIAMOND_MIN..=DIAMOND_MAX).contains(&stats.diamond_band),
        "diamond-band {} outside pre-written band [{}, {}]",
        stats.diamond_band,
        DIAMOND_MIN,
        DIAMOND_MAX
    );
}

/// Acceptance 1 (half B) + §3.6: the same seed twice is bit-identical.
#[test]
#[ignore = "32×32 acceptance measurement; run --release -- --ignored"]
fn the_same_seed_twice_gives_identical_statistics() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let tags = TagTable::load_block_tags(&root, &registries.blocks);
    let (ores, _) = OreSet::load_overworld(&root, &registries.blocks, &tags);
    let (carvers, _) = CarverSet::load_overworld(&root, &tags);
    let a = measure_region(&ores, &carvers, SEED);
    let b = measure_region(&ores, &carvers, SEED);
    assert_eq!(a, b, "same seed must give the same region");
}

/// Acceptance 2: carved-air fraction inside the pre-written band.
#[test]
#[ignore = "32×32 acceptance measurement; run --release -- --ignored"]
fn carved_air_fraction_sits_inside_the_stated_tolerance() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let tags = TagTable::load_block_tags(&root, &registries.blocks);
    let (ores, _) = OreSet::load_overworld(&root, &registries.blocks, &tags);
    let (carvers, _) = CarverSet::load_overworld(&root, &tags);
    let stats = measure_region(&ores, &carvers, SEED);
    let fraction = stats.carved_fraction();
    eprintln!("carved-air fraction {fraction:.6} (band [{CARVED_MIN}, {CARVED_MAX}])");
    assert!(
        (CARVED_MIN..=CARVED_MAX).contains(&fraction),
        "carved-air fraction {fraction} outside pre-written band [{CARVED_MIN}, {CARVED_MAX}]"
    );
    assert!(stats.carved_air > 0, "the carver must carve something");
}

/// Acceptance 3 (perturbation pin): neutralise the ore feature assembly and
/// the count that [`ore_counts_sit_inside_the_stated_tolerance`] asserts leaves
/// its band. This is the named red: if this ever *passes* alongside the real
/// count test, the count test is not measuring ores.
#[test]
#[ignore = "32×32 acceptance measurement; run --release -- --ignored"]
fn neutralising_the_ore_assembly_turns_the_count_red() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let tags = TagTable::load_block_tags(&root, &registries.blocks);
    let (ores, _) = OreSet::load_overworld(&root, &registries.blocks, &tags);
    let (carvers, _) = CarverSet::load_overworld(&root, &tags);

    // The real assembly is inside the band (this is the pin the other test
    // asserts; repeated here so the perturbation is a single comparison).
    let real = measure_region(&ores, &carvers, SEED);
    assert!(
        (COAL_MIN..=COAL_MAX).contains(&real.coal),
        "precondition: the real assembly is in band (got {})",
        real.coal
    );

    // Neutralised assembly: `OreSet::empty()` — no configured/placed feature
    // can write a block.
    let neutral = measure_region(&OreSet::empty(), &carvers, SEED);
    assert_eq!(neutral.coal, 0, "no ores, no coal");
    assert_eq!(neutral.diamond_band, 0, "no ores, no diamond");
    assert!(
        !(COAL_MIN..=COAL_MAX).contains(&neutral.coal),
        "the coal count test must go red when the ore assembly is neutralised"
    );
    // Carvers are untouched by neutralising ores: the carved-air fraction is
    // allowed to move only by the blocks ores no longer occupy, so we assert
    // the *count* pin (the acceptance wording) and not an exact carve equality.
    assert!(neutral.carved_air > 0, "carvers still run");
}

/// Pi hook: single-chunk `terrain + carvers + ores` cost.
///
/// Not a silent benchmark — it prints and is `#[ignore]`d so CI does not treat
/// a laptop number as a Pi number. See the module docs for the command line.
#[test]
#[ignore = "cost hook: run on the Pi and record ms/chunk (P18-04 / §13)"]
fn pi_single_chunk_cost() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let blocks = &registries.blocks;
    let tags = TagTable::load_block_tags(&root, blocks);
    let (ores, _) = OreSet::load_overworld(&root, blocks, &tags);
    let (carvers, _) = CarverSet::load_overworld(&root, &tags);
    let context = WorldgenContext::overworld(WorldSeed::from_raw(SEED));
    let generator = mc_worldgen::TerrainGenerator::new(context.clone(), blocks).expect("gen");

    // Warmup outside the timed window (allocator, page-in).
    for i in 0..8 {
        let pos = ChunkPos::new(i, i);
        let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
        let _ = carve_chunk(&mut chunk, pos, &context, &carvers, blocks);
        let _ = populate_ores(&mut chunk, pos, &context, &ores, blocks);
    }

    let samples = 64;
    let start = Instant::now();
    for i in 0..samples {
        let pos = ChunkPos::new(i, 1000);
        let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
        let _ = carve_chunk(&mut chunk, pos, &context, &carvers, blocks);
        let _ = populate_ores(&mut chunk, pos, &context, &ores, blocks);
    }
    let elapsed = start.elapsed();
    let ms = elapsed.as_secs_f64() * 1000.0 / f64::from(samples);
    // The print *is* the hook: copy this line into the §13 record.
    println!(
        "pi_single_chunk_cost: {samples} chunks in {:.3} ms → {ms:.3} ms/chunk \
         (terrain+carvers+ores, seed={SEED})",
        elapsed.as_secs_f64() * 1000.0
    );
}

/// Pack files the loader must see — a cheap structural pin so a renamed
/// extract root fails loudly instead of silently measuring an empty set.
#[test]
fn the_pack_ships_the_expected_ore_and_carver_files() {
    let root = pack_or_skip!();
    for name in mc_worldgen::OVERWORLD_ORE_PLACED_FEATURES {
        let path = root
            .join("worldgen")
            .join("placed_feature")
            .join(format!("{name}.json"));
        assert!(
            path.is_file(),
            "missing placed feature {name} at {}",
            path.display()
        );
    }
    for name in mc_worldgen::OVERWORLD_CARVERS {
        let path = root
            .join("worldgen")
            .join("configured_carver")
            .join(format!("{name}.json"));
        assert!(
            path.is_file(),
            "missing carver {name} at {}",
            path.display()
        );
    }
    // And the replaceable tags the features name.
    for tag in [
        "stone_ore_replaceables",
        "deepslate_ore_replaceables",
        "overworld_carver_replaceables",
    ] {
        let path = root.join("tags").join("block").join(format!("{tag}.json"));
        assert!(path.is_file(), "missing tag {tag} at {}", path.display());
    }
}

/// Always-on 4×4 smoke of the same pipeline the 32×32 pins measure.
///
/// Small enough for a debug `cargo test -p mc-worldgen`, and it still proves:
/// the pack assembles, carvers carve, ores place, the same seed is bit-stable,
/// and neutralising the ore assembly drops the coal count to zero (the
/// perturbation pin at smoke scale).
#[test]
fn smoke_region_assembles_carves_and_places() {
    let root = pack_or_skip!();
    let registries = Registries::vanilla().expect("registry");
    let blocks = &registries.blocks;
    let tags = TagTable::load_block_tags(&root, blocks);
    let (ores, ore_skipped) = OreSet::load_overworld(&root, blocks, &tags);
    let (carvers, carver_skipped) = CarverSet::load_overworld(&root, &tags);
    assert!(ore_skipped.is_empty(), "{ore_skipped:?}");
    assert!(carver_skipped.is_empty(), "{carver_skipped:?}");
    assert_eq!(ores.len(), 25);
    assert_eq!(carvers.len(), 3);

    let context = WorldgenContext::overworld(WorldSeed::from_raw(SEED));
    let generator = mc_worldgen::TerrainGenerator::new(context.clone(), blocks).expect("gen");
    let coal_ids: BTreeSet<i32> = ["minecraft:coal_ore", "minecraft:deepslate_coal_ore"]
        .iter()
        .filter_map(|n| blocks.default_state(n).ok())
        .collect();

    let mut coal = 0_usize;
    let mut carved = 0_usize;
    let mut coal_again = 0_usize;
    for cz in 0..4 {
        for cx in 0..4 {
            let pos = ChunkPos::new(cx, cz);
            let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
            carved += carve_chunk(&mut chunk, pos, &context, &carvers, blocks).blocks_carved;
            let _ = populate_ores(&mut chunk, pos, &context, &ores, blocks);
            for y in chunk.min_y()..chunk.max_y() {
                for z in 0..16 {
                    for x in 0..16 {
                        if coal_ids.contains(&chunk.get_block(x, y, z)) {
                            coal += 1;
                        }
                    }
                }
            }
            // Second generation of the same position must match block-for-block.
            let mut again = generator.generate_chunk(pos, blocks).expect("chunk");
            let _ = carve_chunk(&mut again, pos, &context, &carvers, blocks);
            let _ = populate_ores(&mut again, pos, &context, &ores, blocks);
            assert_eq!(chunk, again, "chunk {pos:?} must be bit-stable");
            for y in again.min_y()..again.max_y() {
                for z in 0..16 {
                    for x in 0..16 {
                        if coal_ids.contains(&again.get_block(x, y, z)) {
                            coal_again += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(coal, coal_again, "same seed twice");
    assert!(coal > 0, "the pack must place some coal");
    assert!(carved > 0, "the carver must carve something");

    // Neutralise pin at smoke scale.
    let mut neutral_coal = 0_usize;
    for cz in 0..2 {
        for cx in 0..2 {
            let pos = ChunkPos::new(cx, cz);
            let mut chunk = generator.generate_chunk(pos, blocks).expect("chunk");
            let _ = carve_chunk(&mut chunk, pos, &context, &carvers, blocks);
            let _ = populate_ores(&mut chunk, pos, &context, &OreSet::empty(), blocks);
            for y in chunk.min_y()..chunk.max_y() {
                for z in 0..16 {
                    for x in 0..16 {
                        if coal_ids.contains(&chunk.get_block(x, y, z)) {
                            neutral_coal += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(neutral_coal, 0, "neutralised assembly must place no coal");
}
