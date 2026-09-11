//! Frozen golden values for the noise pipeline (P07-14).
//!
//! These numbers were **printed by this implementation and then frozen**. That is
//! what makes them useful: they are not a claim about correctness, they are a
//! tripwire. Any refactor that changes the noise field — a different permutation
//! seeding, a different fade curve, a reordered lerp, a changed normalization —
//! moves at least one of them, and the failure names the coordinate.
//!
//! Compare with `docs/vanilla-parity/PARITY-MATRIX.md`: **none of these values is
//! a Vanilla value.** Vanilla's `NormalNoise` has a different gradient set,
//! different tables and different amplitude tables, so a Vanilla world's terrain
//! fails these assertions on the first coordinate — which is exactly the honest
//! position.
//!
//! ## What is *also* frozen here: measured ranges
//!
//! AGENTS.md §3.2 says to measure a baseline rather than reason from intuition.
//! The realized range of [`FractalNoise`] is far narrower than its nominal
//! `[-1, 1]`, and two constants in `mc_worldgen::terrain`/`mc_worldgen::biome`
//! are sized from the measurement. The ranges below are part of that evidence,
//! and they are asserted with a tolerance because a range is an extremum over a
//! sample, not an identity.

use mc_world::ChunkPos;
use mc_worldgen::noise::{FractalNoise, MAX_OCTAVES, PerlinNoise};
use mc_worldgen::seed::{WorldSeed, splitmix64_mix};

/// Compare two `f64` bit patterns exactly.
///
/// Exact equality is the right assertion for a *golden* value: the computation is
/// deterministic IEEE-754 arithmetic in a fixed order, so a refactor that
/// perturbs the last bit is a real change and must be reported, not rounded away.
/// `to_bits` is used rather than `==` so that `-0.0` and `0.0` compare different
/// here and a NaN would fail loudly rather than compare unequal silently.
#[track_caller]
fn assert_bits(actual: f64, expected_bits: u64, label: &str) {
    assert_eq!(
        actual.to_bits(),
        expected_bits,
        "{label}: got {actual:?} ({}), expected {:?} ({expected_bits})",
        actual.to_bits(),
        f64::from_bits(expected_bits)
    );
}

#[test]
fn perlin_noise_matches_five_frozen_values() {
    // `PerlinNoise::new(1_361_882_806)` — the seed of the repository's evidence
    // world (`crates/test-support/fixtures/anvil/MANIFEST.txt`).
    let noise = PerlinNoise::new(1_361_882_806);
    let frozen: [(f64, f64, f64, u64); 5] = [
        (0.5, 0.5, 0.5, 0x3FD0_F876_CCDF_6CDA),
        (1.25, -2.75, 3.5, 0x3F80_CC72_58BC_0950),
        (-9.875, 3.625, 100.0625, 0xBFAA_2AC4_28B3_B597),
        (123.456, 78.9, -0.5, 0xBF5F_6AAA_651E_0200),
        (
            1_000.333_333_333_333_4,
            2_000.666_666_666_666_7,
            -3000.5,
            0x3FBB_5B0E_6534_C566,
        ),
    ];
    for (x, y, z, bits) in frozen {
        let value = noise.value(x, y, z);
        assert_bits(value, bits, &format!("PerlinNoise::value({x}, {y}, {z})"));
    }
    // Printed so a deliberate change can be re-frozen from the log:
    // `cargo test -p mc-worldgen --test golden -- --nocapture`
    println!("FROZEN PerlinNoise::new(1361882806):");
    for (x, y, z, bits) in frozen {
        println!(
            "  ({x}, {y}, {z}) = {:#018X}",
            noise.value(x, y, z).to_bits()
        );
        assert_eq!(bits, noise.value(x, y, z).to_bits());
    }
}

#[test]
fn fractal_noise_matches_five_frozen_values() {
    // Four octaves, the crate's default persistence (0.5) and lacunarity (2.0).
    let noise = FractalNoise::with_octaves(42, 4);
    let frozen: [(f64, f64, f64, u64); 3] = [
        (0.5, 0.0, 0.5, 0x3FA8_22CB_17FF_2EB9),
        (-17.25, 0.0, 8.75, 0xBF94_5EAD_435A_9F5A),
        (1000.5, 0.0, -2000.5, 0xBFA8_22CB_17FF_2EB9),
    ];
    for (x, y, z, bits) in frozen {
        assert_bits(
            noise.value(x, y, z),
            bits,
            &format!("FractalNoise::value({x}, {y}, {z})"),
        );
    }
    // The fourth: the same field off the `y = 0` plane, so the three-dimensional
    // path is frozen as well as the two-dimensional one.
    assert_bits(
        noise.value(1.5, 2.5, 3.5),
        0x3FB8_22CB_17FF_2EB9,
        "FractalNoise::value(1.5, 2.5, 3.5)",
    );
    // The fifth: a scaled base frequency, which exercises the octave frequency
    // chain rather than just the octave sum.
    let scaled = FractalNoise::new(42, 4, 0.5, 2.0).at_frequency(0.01);
    assert_bits(
        scaled.value_2d(0.5, 0.5),
        0xBF6F_6415_86D9_D53E,
        "FractalNoise::at_frequency(0.01).value_2d(0.5, 0.5)",
    );
    println!("FROZEN FractalNoise::with_octaves(42, 4):");
    for (x, y, z, bits) in frozen {
        println!("  value({x}, {y}, {z}) = {bits:#018X}");
    }
    println!(
        "  value(1.5, 2.5, 3.5) = {:#018X}",
        noise.value(1.5, 2.5, 3.5).to_bits()
    );
    println!(
        "  at_frequency(0.01).value_2d(0.5, 0.5) = {:#018X}",
        scaled.value_2d(0.5, 0.5).to_bits()
    );
}

#[test]
fn the_realized_noise_range_is_measured_not_assumed() {
    // The nominal band is [-1, 1]; the realized range is much narrower, and this
    // is the measurement the terrain and climate amplitudes are sized from.
    let noise = FractalNoise::with_octaves(42, 4);
    let (mut minimum, mut maximum) = (f64::MAX, f64::MIN);
    let mut samples = 0_u32;
    for x in -250_i32..250 {
        for z in -250_i32..250 {
            let value = noise.value_2d(f64::from(x) * 0.25, f64::from(z) * 0.25);
            minimum = minimum.min(value);
            maximum = maximum.max(value);
            samples += 1;
        }
    }
    println!("MEASURED FractalNoise 4-octave range over {samples} samples: {minimum} .. {maximum}");
    // Measured: about -0.377 .. +0.387 (asserted with slack, because an extremum
    // over a finite sample is not an identity).
    assert!(
        (-0.45..=-0.30).contains(&minimum),
        "realized minimum {minimum} moved"
    );
    assert!(
        (0.30..=0.45).contains(&maximum),
        "realized maximum {maximum} moved"
    );
    assert!(
        minimum >= -1.0 && maximum <= 1.0,
        "still inside the nominal band"
    );
    // The single-octave field is much wider, which is *why* the fractal is
    // divided by its amplitude sum: without that, octaves would pile up.
    let single = FractalNoise::with_octaves(42, 1);
    let mut single_max = f64::MIN;
    for x in -250_i32..250 {
        for z in -250_i32..250 {
            single_max = single_max.max(single.value_2d(f64::from(x) * 0.31, f64::from(z) * 0.17));
        }
    }
    println!("MEASURED FractalNoise 1-octave maximum: {single_max}");
    assert!(
        single_max > maximum,
        "one octave ({single_max}) should reach further than four ({maximum})"
    );
}

#[test]
fn the_octave_count_narrows_and_converges_the_realized_range() {
    // The consequence of the `1/Σaᵢ` normalization, **measured** rather than
    // asserted from theory. The first version of this test claimed the realized
    // maximum decreases *strictly* with every added octave; the measurement says
    // otherwise — at 12 octaves it rose by 7.5e-7 over 11. That is not a bug: the
    // octaves are independent, so an extra octave adds a little energy at the
    // extremum while shrinking the normalizer, and the sequence is asymptotically
    // decreasing, not monotone. The honest assertions are the coarse ones below.
    let maxima: Vec<(u32, f64)> = (1..=MAX_OCTAVES)
        .map(|octaves| {
            let noise = FractalNoise::with_octaves(42, octaves);
            let mut maximum = f64::MIN;
            for x in -120_i32..120 {
                for z in -120_i32..120 {
                    maximum = maximum.max(noise.value_2d(f64::from(x) * 0.31, f64::from(z) * 0.17));
                }
            }
            (octaves, maximum)
        })
        .collect();
    println!("MEASURED realized maximum by octave count:");
    for (octaves, maximum) in &maxima {
        println!("  {octaves:>2}: {maximum}");
    }

    // 1. Every field is alive and inside the nominal band.
    for (octaves, maximum) in &maxima {
        assert!(*maximum > 0.0, "octaves={octaves} is a dead field");
        assert!(*maximum <= 1.0, "octaves={octaves} left the nominal band");
    }
    // 2. The range narrows substantially from one octave to the maximum.
    let first = maxima.first().expect("at least one octave").1;
    let last = maxima.last().expect("at least one octave").1;
    assert!(
        last < first * 0.75,
        "the normalized range should narrow markedly: {first} -> {last}"
    );
    // 3. It narrows across the powers of two, which is the shape the normalizer
    //    produces.
    for pair in [(1_usize, 2_usize), (2, 4), (4, 8), (8, 16)] {
        let (low, high) = (maxima[pair.0 - 1].1, maxima[pair.1 - 1].1);
        assert!(
            high < low,
            "octaves {} ({low}) should not exceed octaves {} ({high})",
            pair.0,
            pair.1
        );
    }
    // 4. And it converges: the last few steps move by far less than the first.
    let early_step = maxima[0].1 - maxima[1].1;
    let late_step = (maxima[MAX_OCTAVES as usize - 2].1 - maxima[MAX_OCTAVES as usize - 1].1).abs();
    assert!(
        late_step < early_step * 0.1,
        "the sequence should converge: early step {early_step}, late step {late_step}"
    );
}

#[test]
fn chunk_seeds_match_four_frozen_values() {
    // The `(seed, chunk_pos) → sub-seed` derivation, frozen. A change here means
    // every world generated by this crate changes, so it must be deliberate.
    let seed = WorldSeed::from_raw(1_361_882_806);
    let frozen: [(ChunkPos, i64); 4] = [
        (ChunkPos::new(0, 0), -1_300_892_167_044_901_178),
        (ChunkPos::new(1, 0), 7_101_120_202_906_763_263),
        (ChunkPos::new(-1, -1), 6_296_336_343_046_550_972),
        (ChunkPos::new(1000, -2000), 5_313_152_569_276_729_419),
    ];
    for (pos, expected) in frozen {
        assert_eq!(
            seed.chunk_seed(pos).raw(),
            expected,
            "chunk seed for {pos:?} moved"
        );
    }
    println!("FROZEN chunk seeds for world seed 1361882806:");
    for (pos, expected) in frozen {
        println!("  {pos:?} = {expected}");
    }
    // The mixer itself, at the published algorithm's own first values.
    assert_eq!(
        splitmix64_mix(0),
        0,
        "zero is a fixed point of the finalizer"
    );
    assert_eq!(splitmix64_mix(1), 0x5692_161D_100B_05E5);
    assert_eq!(splitmix64_mix(2), 0xDBD2_3897_3A2B_148A);
}

#[test]
fn surface_heights_match_frozen_values() {
    // The end-to-end consequence of the two golden tests above: the terrain a
    // column actually gets. If the noise moves, this moves too, and the diff
    // names the column.
    use mc_registry::Registries;
    use mc_worldgen::WorldgenContext;
    use mc_worldgen::terrain::TerrainGenerator;

    let blocks = Registries::vanilla().expect("registry").blocks;
    let generator =
        TerrainGenerator::new(WorldgenContext::overworld(WorldSeed::from_raw(1)), &blocks)
            .expect("generator");
    let frozen: [((i32, i32), i32); 6] = [
        ((0, 0), 49),
        ((1, 1), 49),
        ((100, 100), 83),
        ((-100, -100), 65),
        ((1000, -2000), 61),
        ((12_345, 6_789), 78),
    ];
    for ((x, z), expected) in frozen {
        assert_eq!(
            generator.surface_height(x, z),
            expected,
            "surface height at ({x}, {z}) moved"
        );
    }
    // The heights are land and sea, not a flat plane: this is the property the
    // biome rule depends on, and it is asserted here so a constant that flattens
    // the world cannot pass the golden values above by accident.
    let heights: Vec<i32> = frozen
        .iter()
        .map(|((x, z), _)| generator.surface_height(*x, *z))
        .collect();
    let above = heights.iter().filter(|height| **height >= 63).count();
    assert!(above > 0, "some sampled columns must be land");
    println!("FROZEN surface heights for world seed 1:");
    for ((x, z), expected) in frozen {
        println!("  ({x}, {z}) = {expected}");
    }
}
