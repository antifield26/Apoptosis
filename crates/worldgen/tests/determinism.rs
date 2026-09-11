//! Determinism of the generation pipeline (AGENTS.md §3.6).
//!
//! Covers the required properties:
//!
//! 1. the same `(seed, pos)` gives an identical chunk, generated twice;
//! 2. two chunks generated in **opposite orders** come out identical;
//! 3. different seeds give different terrain;
//! 4. every block id of every section is compared, not just the surface.

use mc_registry::{BlockRegistry, Registries};
use mc_world::ChunkPos;
use mc_worldgen::terrain::{ChunkGenerator, EXAMPLE_SEED, TerrainGenerator};
use mc_worldgen::{WorldSeed, WorldgenContext};

fn registry() -> BlockRegistry {
    Registries::vanilla().expect("registry fixture").blocks
}

fn generator(seed: i64) -> TerrainGenerator {
    TerrainGenerator::new(
        WorldgenContext::overworld(WorldSeed::from_raw(seed)),
        &registry(),
    )
    .expect("generator")
}

/// Every block id of every section, in index order.
fn all_blocks(chunk: &mc_world::Chunk) -> Vec<i32> {
    chunk
        .sections
        .iter()
        .flat_map(|section| section.blocks.iter().copied())
        .collect()
}

/// The non-air block ids of every section, which is what a client would see as
/// terrain (the air is not interesting and dominates the vector).
fn solid_blocks(chunk: &mc_world::Chunk, air: i32) -> Vec<(usize, i32)> {
    let mut out = Vec::new();
    for (index, section) in chunk.sections.iter().enumerate() {
        for (slot, id) in section.blocks.iter().enumerate() {
            if *id != air {
                out.push((index * 4096 + slot, *id));
            }
        }
    }
    out
}

#[test]
fn the_same_seed_and_position_give_an_identical_chunk() {
    let blocks = registry();
    let generator = generator(EXAMPLE_SEED.raw());
    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(-7, 13),
        ChunkPos::new(1_000, -2_000),
    ] {
        let first = generator.generate_chunk(pos, &blocks).expect("chunk");
        let second = generator.generate_chunk(pos, &blocks).expect("chunk");
        // The derived `PartialEq` on `Chunk` compares every field, but the
        // per-block comparison below is the assertion the contract asks for and
        // is deliberately explicit: a `PartialEq` that silently ignored a field
        // would hide a regression.
        assert_eq!(first, second, "{pos:?} must be byte-identical");
        let (left, right) = (all_blocks(&first), all_blocks(&second));
        assert_eq!(left.len(), right.len(), "{pos:?} section sizes");
        assert_eq!(left.len(), 24 * 4096, "every section was compared");
        for (index, (a, b)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(a, b, "{pos:?} differs at flat index {index}");
        }
        assert!(
            left.iter().any(|id| *id != 0),
            "{pos:?} must have some terrain in it"
        );
    }
}

#[test]
fn two_chunks_generated_in_opposite_orders_are_identical() {
    let blocks = registry();
    let generator = generator(EXAMPLE_SEED.raw());
    let left = ChunkPos::new(-3, 5);
    let right = ChunkPos::new(4, -6);

    // Order A: left then right, keeping the results.
    let a_left = generator.generate_chunk(left, &blocks).expect("chunk");
    let a_right = generator.generate_chunk(right, &blocks).expect("chunk");
    // Order B: right then left.
    let b_right = generator.generate_chunk(right, &blocks).expect("chunk");
    let b_left = generator.generate_chunk(left, &blocks).expect("chunk");

    assert_eq!(
        a_left, b_left,
        "the left chunk must not depend on what was generated before it"
    );
    assert_eq!(
        a_right, b_right,
        "the right chunk must not depend on what was generated before it"
    );
    // And the two chunks really are different from each other, so the comparison
    // above is not passing because everything is identical.
    assert_ne!(a_left, a_right);
}

#[test]
fn generation_does_not_depend_on_how_many_chunks_were_generated_first() {
    let blocks = registry();
    let generator = generator(EXAMPLE_SEED.raw());
    let target = ChunkPos::new(11, -11);
    let alone = generator.generate_chunk(target, &blocks).expect("chunk");

    // Generate a hundred other chunks (in a deterministic order) and then the
    // target again: the result must be unchanged.
    for index in 0..100_i32 {
        let _ = generator
            .generate_chunk(ChunkPos::new(index * 7 - 50, index * -3 + 20), &blocks)
            .expect("chunk");
    }
    let after = generator.generate_chunk(target, &blocks).expect("chunk");
    assert_eq!(alone, after, "no cross-chunk state may exist");
}

#[test]
fn different_seeds_give_different_terrain() {
    let blocks = registry();
    let first = generator(1);
    let second = generator(2);
    let air = blocks.air_id();
    let positions: Vec<ChunkPos> = (0..8)
        .map(|index| ChunkPos::new(index * 13 - 40, index * -7 + 30))
        .collect();

    for pos in &positions {
        let a = first.generate_chunk(*pos, &blocks).expect("chunk");
        let b = second.generate_chunk(*pos, &blocks).expect("chunk");
        assert_ne!(
            solid_blocks(&a, air),
            solid_blocks(&b, air),
            "seed 1 and seed 2 produced identical terrain at {pos:?}"
        );
    }
    // Every column of both is still solid at the floor, so the difference is
    // terrain and not one world being empty.
    for pos in &positions {
        for chunk in [
            first.generate_chunk(*pos, &blocks).expect("chunk"),
            second.generate_chunk(*pos, &blocks).expect("chunk"),
        ] {
            assert!(chunk.non_empty_block_count(0) > 0);
        }
    }
}

#[test]
fn decoration_is_deterministic_too() {
    let blocks = registry();
    let generator =
        TerrainGenerator::new(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
            .expect("generator");
    let pos = ChunkPos::new(6, 6);
    let mut first = generator.generate_chunk(pos, &blocks).expect("chunk");
    let mut second = generator.generate_chunk(pos, &blocks).expect("chunk");
    let stats_first = generator.decorate(&mut first, pos, &blocks);
    let stats_second = generator.decorate(&mut second, pos, &blocks);
    assert_eq!(stats_first, stats_second, "same decoration statistics");
    assert_eq!(first, second, "same decorated chunk");
}

#[test]
fn decoration_never_touches_a_neighbouring_chunk() {
    // The straddle rule, at the generator level: a chunk's decoration is a pure
    // function of that chunk, so generating the neighbour first cannot change it.
    let blocks = registry();
    let generator =
        TerrainGenerator::new(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
            .expect("generator");
    let pos = ChunkPos::new(-2, 3);
    let mut alone = generator.generate_chunk(pos, &blocks).expect("chunk");
    let _ = generator.decorate(&mut alone, pos, &blocks);

    let _ = generator
        .generate_chunk(ChunkPos::new(-1, 3), &blocks)
        .expect("neighbour");
    let _ = generator
        .generate_chunk(ChunkPos::new(-2, 4), &blocks)
        .expect("neighbour");
    let mut after = generator.generate_chunk(pos, &blocks).expect("chunk");
    let _ = generator.decorate(&mut after, pos, &blocks);
    assert_eq!(alone, after);
}

#[test]
fn generation_throughput_is_recorded() {
    // Not a threshold, a *measurement*: AGENTS.md §13 forbids hard-coding a
    // performance number before a representative workload exists, and this host
    // is not the Pi 5. The number is printed so the baseline is reproducible
    // (`cargo test -p mc-worldgen -- --nocapture generation_throughput`).
    let blocks = registry();
    let generator = generator(EXAMPLE_SEED.raw());
    let started = std::time::Instant::now();
    let chunks = 64;
    let mut solid = 0_usize;
    for index in 0..chunks {
        let chunk = generator
            .generate_chunk(ChunkPos::new(index, -index), &blocks)
            .expect("chunk");
        solid += chunk
            .sections
            .iter()
            .map(|section| usize::from(section.non_empty_block_count.unsigned_abs()))
            .sum::<usize>();
    }
    let elapsed = started.elapsed();
    println!(
        "BASELINE worldgen: {chunks} chunks in {elapsed:?} ({:.2} ms/chunk, debug build, {solid} solid blocks total)",
        elapsed.as_secs_f64() * 1000.0 / f64::from(chunks)
    );
    assert!(solid > 0);
}
