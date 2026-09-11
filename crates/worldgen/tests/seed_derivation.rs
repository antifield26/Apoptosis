//! The `(seed, chunk_pos) → sub-seed` derivation (P07-13).
//!
//! The derivation is documented in `mc_worldgen::seed`'s module docs and is
//! injective by construction; these tests are the regression net for that claim,
//! plus the required collision sample over thousands of positions.

use mc_persistence::dimension::Dimension;
use mc_world::ChunkPos;
use mc_worldgen::seed::{WorldSeed, WorldgenContext, pack_chunk_pos, splitmix64_mix};
use mc_worldgen::{OVERWORLD_HEIGHT, OVERWORLD_MIN_Y, OVERWORLD_SEA_LEVEL};
use std::collections::BTreeSet;

#[test]
fn two_different_positions_never_collide_over_a_wide_sample() {
    // 40 000 positions in an irregular lattice (the strides 37 and 53 are coprime
    // with the 100-wide scan, so the sample covers both signs and both axes).
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut positions = 0_u32;
    for x in -100_i32..100 {
        for z in -100_i32..100 {
            let pos = ChunkPos::new(x * 37, z * 53);
            assert!(
                seen.insert(WorldSeed::from_raw(1).chunk_seed(pos).raw()),
                "{pos:?} collided"
            );
            positions += 1;
        }
    }
    assert_eq!(positions, 40_000);
    assert_eq!(seen.len(), 40_000);
}

#[test]
fn the_same_position_always_gives_the_same_sub_seed() {
    let seed = WorldSeed::from_raw(1_361_882_806);
    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(-1, 1),
        ChunkPos::new(i32::MAX, i32::MIN),
        ChunkPos::new(12_345, -67_890),
    ] {
        let first = seed.chunk_seed(pos).raw();
        // A fresh call, and a call after deriving thousands of others: the
        // derivation is stateless, so nothing can drift.
        for _ in 0..1_000 {
            let _ = seed.chunk_seed(ChunkPos::new(1, 1));
        }
        assert_eq!(first, seed.chunk_seed(pos).raw(), "{pos:?} drifted");
    }
}

#[test]
fn the_packing_is_a_bijection_on_hostile_coordinates() {
    // Every entry must be a *distinct* `ChunkPos`: `-0 == 0` for `i32`, so a
    // first version of this list contained `(0, 0)` twice and the set check below
    // correctly rejected it. The duplicate is gone rather than tolerated.
    let hostile = [
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(0, 1),
        ChunkPos::new(-1, 0),
        ChunkPos::new(0, -1),
        ChunkPos::new(i32::MAX, i32::MAX),
        ChunkPos::new(i32::MIN, i32::MIN),
        ChunkPos::new(i32::MIN, i32::MAX),
        ChunkPos::new(i32::MAX, i32::MIN),
        ChunkPos::new(1 << 16, 1 << 16),
        ChunkPos::new(-(1 << 16), 1 << 16),
        ChunkPos::new(0, i32::MIN),
        ChunkPos::new(i32::MIN, 0),
    ];
    let mut visited = BTreeSet::new();
    for pos in hostile {
        assert!(visited.insert(pack_chunk_pos(pos)), "{pos:?} packed twice");
    }
    assert_eq!(visited.len(), hostile.len());
}

#[test]
fn sub_seeds_differ_across_seeds_and_streams() {
    let pos = ChunkPos::new(17, -23);
    let a = WorldSeed::from_raw(0);
    let b = WorldSeed::from_raw(1);
    assert_ne!(a.chunk_seed(pos), b.chunk_seed(pos));
    assert_ne!(a.stream_seed(0), a.stream_seed(1));
    assert_ne!(a.stream_seed(0), b.stream_seed(0));

    // A seed of zero is the case the golden-gamma XOR exists for: `mix64(0)` is
    // zero, so without it `WorldSeed(0).chunk_seed((0, 0))` would be zero and
    // would collide with any other pair whose XOR also lands on zero.
    let zero = WorldSeed::from_raw(0);
    let zero_chunk = zero.chunk_seed(ChunkPos::new(0, 0)).raw();
    assert_ne!(
        zero_chunk, 0,
        "the zero seed must not produce a zero sub-seed"
    );
    assert_ne!(
        zero_chunk,
        zero.stream_seed(0),
        "and no stream of it either"
    );
    // The known collision case the XOR removes, stated explicitly.
    assert_eq!(splitmix64_mix(0), 0);
    assert_ne!(
        zero_chunk,
        splitmix64_mix(pack_chunk_pos(ChunkPos::new(0, 0))).cast_signed()
    );
}

#[test]
fn chunk_seeds_do_not_collide_with_stream_seeds() {
    // A stream seed and a chunk seed are different things; the same mixer is used
    // for both, so a caller that confused them would silently generate different
    // terrain. Over a sample, the two families must not overlap.
    let seed = WorldSeed::from_raw(1_361_882_806);
    let mut chunk_seeds = BTreeSet::new();
    for x in -30_i32..30 {
        for z in -30_i32..30 {
            chunk_seeds.insert(seed.chunk_seed(ChunkPos::new(x, z)).raw());
        }
    }
    assert_eq!(chunk_seeds.len(), 3_600);
    for stream in 0..64_u64 {
        assert!(
            !chunk_seeds.contains(&seed.stream_seed(stream)),
            "stream {stream} collided with a chunk seed"
        );
    }
}

#[test]
fn the_context_bounds_come_from_the_dimension() {
    let context = WorldgenContext::overworld(WorldSeed::from_raw(42));
    // These four values are the 26.1.2 overworld: -64..320, sea level 63.
    assert_eq!(context.min_y, OVERWORLD_MIN_Y);
    assert_eq!(context.height, OVERWORLD_HEIGHT);
    assert_eq!(context.sea_level, OVERWORLD_SEA_LEVEL);
    assert_eq!(context.max_y(), 320);
    assert_eq!(context.min_section_y(), -4);
    assert_eq!(context.section_count(), 24);
    assert_eq!(context.effective_max_y(), 320);
    assert_eq!(context.dimension, Dimension::Overworld);
}

#[test]
fn hostile_contexts_are_bounded_not_panicking() {
    for (min_y, height, sea_level) in [
        (i32::MIN, i32::MAX, i32::MIN),
        (i32::MAX, i32::MAX, i32::MAX),
        (i32::MIN, i32::MIN, i32::MIN),
        (0, 0, 0),
        (0, -1, 0),
        (-64, 384, 3_000_000),
    ] {
        let context = WorldgenContext::new(
            WorldSeed::from_raw(i64::MIN),
            Dimension::Overworld,
            min_y,
            height,
            sea_level,
        );
        // Every derived bound is finite and consistent, and both the section
        // count and the effective ceiling are bounded.
        assert!(context.section_count() >= 1 && context.section_count() <= 128);
        assert!(context.effective_max_y() >= context.min_y);
        // `min_section_y` is an `i8` by construction, so asserting it is "in
        // range" would be a tautology. What is worth asserting is that it agrees
        // with `min_y` whenever `min_y` really is representable as a section
        // index, and saturates when it is not.
        match i8::try_from(min_y.div_euclid(16)) {
            Ok(expected) => assert_eq!(context.min_section_y(), expected, "min_y {min_y}"),
            Err(_) => assert!(
                context.min_section_y() == i8::MIN || context.min_section_y() == i8::MAX,
                "an unrepresentable min_y {min_y} must saturate, got {}",
                context.min_section_y()
            ),
        }
        let seed = context.chunk_seed(ChunkPos::new(i32::MAX, i32::MIN));
        assert_eq!(
            seed,
            context.seed.chunk_seed(ChunkPos::new(i32::MAX, i32::MIN))
        );
    }
}

#[test]
fn a_world_gen_document_seed_is_read_by_name() {
    // The evidence world's seed is 1361882806; this asserts the reader finds it
    // through the shape a 26.1 world document has.
    use mc_nbt::NbtTag;
    let document = NbtTag::compound([(
        "Data".to_owned(),
        NbtTag::compound([
            ("version".to_owned(), NbtTag::Int(19133)),
            ("seed".to_owned(), NbtTag::Long(1_361_882_806)),
        ]),
    )]);
    assert_eq!(
        WorldSeed::from_level_dat(&document).expect("seed").raw(),
        1_361_882_806
    );
}
