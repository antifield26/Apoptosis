//! Seeded gradient noise and fractal composition (P07-14).
//!
//! ## What this is, exactly
//!
//! **Ken Perlin's "improved" gradient noise** (2002) over a seeded 256-entry
//! permutation table, plus a plain fractal (fBm) octave sum. That is the whole
//! algorithm; it is public, and it is reimplemented here from the published
//! description rather than copied from a reference implementation.
//!
//! ## What this is **not** — read this before using it for anything
//!
//! It is **not** Vanilla's `NormalNoise`/`PerlinNoise`, and no terrain generated
//! from it will match a Vanilla world:
//!
//! - Vanilla's `PerlinNoise` uses several independent permutation tables and the
//!   classic (1985) twelve-gradient `grad` function; this uses one table and the
//!   2002 gradient set. Different gradients ⇒ a different value at the same
//!   coordinate.
//! - Vanilla's `NormalNoise` sums `firstOctave..=octaves` noise instances with
//!   **amplitude tables** that this project has not extracted from the 26.1.2
//!   jar. No amplitude table here is claimed to be Vanilla's; the octaves use a
//!   geometric persistence instead (see [`FractalNoise::new`]).
//! - Vanilla documents its `NormalNoise` range as `[-1, 1]`. Ours is normalized
//!   into the same nominal band by construction, but that is a *range* claim, not
//!   a *shape* claim.
//! - Vanilla seeds its noise from a `XoroshiroRandomSource` chain; here each
//!   field gets a [`crate::seed::WorldSeed::stream_seed`] value, which is our own
//!   documented scheme.
//!
//! In short: **this noise is a labelled approximation.** It is deterministic,
//! bounded, cheap and good enough to build a world on; it carries no parity
//! claim, and the parity matrix must keep saying so until someone measures the
//! real amplitude tables.
//!
//! ## Determinism
//!
//! The permutation table is the only state, it is built once from
//! [`mc_simulation::RandomSource`] (the JDK-verified `java.util.Random` clone)
//! and never mutated afterwards, so [`PerlinNoise`] is immutable after
//! construction and `Send + Sync`. The same seed and coordinate always give the
//! same `f64` — bit for bit — because every operation is plain IEEE-754
//! arithmetic in a fixed order, with no fused multiply-add, no transcendental
//! functions and no platform-dependent rounding. `tests/golden.rs` freezes five
//! values so a refactor that changes the field is caught.
//!
//! ## Bounds (AGENTS.md §9, §10)
//!
//! - [`MAX_OCTAVES`] caps the octave count; a larger request is clamped.
//! - `octaves = 0` degrades to a flat `0.0` instead of dividing by zero.
//! - Coordinates are clamped to [`COORDINATE_LIMIT`] before any cast, so
//!   `i32::MAX`, `i32::MIN`, ±∞ and NaN are all safe inputs.
//! - Nothing here allocates per sample: the tables are built once.

use mc_simulation::RandomSource;

/// Largest octave count any configuration may use.
///
/// **product decision.** Vanilla terrain uses a handful of octaves per field; 16
/// is far above anything this crate needs and is the cost bound, because a
/// sample costs one table lookup and a few multiplies *per octave*: an unbounded
/// octave count from a config file would be a denial-of-service lever
/// (AGENTS.md §10). A larger request is clamped, and
/// [`FractalNoise::octaves`] reports the clamped value.
pub const MAX_OCTAVES: u32 = 16;

/// Largest absolute coordinate handed to the noise functions.
///
/// **derived.** Vanilla's build limit is ±30 000 000 blocks (documented community
/// fact, not measured here), and an argument beyond 2²⁴ loses all precision in a
/// 32-bit lattice anyway. Clamping here keeps every `f64 → i32` cast in range for
/// *any* input, including `i32::MIN`/`i32::MAX`.
pub const COORDINATE_LIMIT: f64 = 30_000_000.0;

/// Octave amplitude decay used when a caller does not choose one.
///
/// **approximation / product decision.** `0.5` is the textbook fBm persistence.
/// Vanilla's per-octave amplitudes come from a table, not a geometric series, so
/// this is explicitly *not* Vanilla's.
pub const DEFAULT_PERSISTENCE: f64 = 0.5;

/// Frequency multiplier between octaves used when a caller does not choose one.
///
/// **approximation.** `2.0` is the textbook fBm lacunarity. Vanilla's octave
/// spacing is part of its amplitude tables and is not verified here.
pub const DEFAULT_LACUNARITY: f64 = 2.0;

/// Number of entries in the permutation table (Perlin's original size).
const TABLE_SIZE: usize = 256;
/// Mask for `index & TABLE_MASK`, valid because [`TABLE_SIZE`] is a power of two.
const TABLE_MASK: usize = TABLE_SIZE - 1;
/// Mask for an index into the doubled (512-entry) table.
const TABLE_MASK_2X: usize = TABLE_SIZE * 2 - 1;
/// Scale applied to every gradient so the interpolated result stays in `[-1, 1]`.
///
/// **derived.** Perlin's gradient set contains vectors of length `√2`
/// (`(1,1,0)`) and of length `1` (`(1,0,1)`). A displacement component is at
/// most `1` inside a unit cell, so a raw dot product reaches `√2 · √2 = 2`, and
/// the trilinear blend of eight such values can exceed `1`. Scaling every
/// gradient by `√2/2 ≈ 0.7071` bounds a raw dot product by `1.5` and the blend by
/// `1.5` as well — inside the nominal band Vanilla documents for its own noise.
/// This is a *derived* normalization of our own field, not a Vanilla constant;
/// `value_is_bounded` measures the realized range, which is much narrower.
const GRADIENT_SCALE: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Perlin's improved-noise gradient set, normalized to length `≤ 1`.
///
/// **verified**: the direction set is the published 2002 set of 16 entries (four
/// of them repeated from the 1985 set: indices 0, 4, 8 and 12 are the repeats,
/// which is what makes the `hash & 15` selection usable). The published set lists
/// 12 distinct directions; the classic selection was `& 15`, so the set below
/// keeps the repeats in the first four slots to preserve that distribution.
///
/// These are **not** Vanilla's gradients.
const GRADIENTS: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
];

/// Seeded improved-Perlin gradient noise in three dimensions.
///
/// Immutable after construction: clone it freely and share it across threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerlinNoise {
    /// A shuffled `0..256`, doubled to 512.
    permutation: [u8; TABLE_SIZE * 2],
}

impl PerlinNoise {
    /// Build the noise from a seed.
    ///
    /// The table is a Fisher–Yates shuffle of `0..=255` driven by
    /// [`RandomSource`]. This is **our own** seeding scheme; Vanilla's is not
    /// reproduced. The golden test freezes the resulting field.
    #[must_use]
    pub fn new(seed: i64) -> Self {
        let mut permutation = [0_u8; TABLE_SIZE * 2];
        for (index, slot) in permutation.iter_mut().take(TABLE_SIZE).enumerate() {
            // 256 entries fit a u8 exactly; the truncation is lossless.
            *slot = index as u8;
        }
        let mut random = RandomSource::new(seed);
        // Fisher–Yates, descending: swap `index` with a uniform draw in `0..=index`.
        for index in (1..TABLE_SIZE).rev() {
            let bound = index + 1;
            let swap_with = random.next_i32_bounded(bound as i32) as usize;
            permutation.swap(index, swap_with);
        }
        for index in 0..TABLE_SIZE {
            permutation[index + TABLE_SIZE] = permutation[index];
        }
        Self { permutation }
    }

    /// The shuffled permutation table (the first 256 entries).
    ///
    /// Exposed for the golden test and for diagnostics; a permutation is public
    /// information, not internal state a caller could corrupt (the field is not
    /// reachable mutably).
    #[must_use]
    pub fn permutation(&self) -> &[u8] {
        &self.permutation[..TABLE_SIZE]
    }

    /// The value at a point, in `[-1, 1]`.
    ///
    /// `0.0` at every integer lattice point (the gradient dot products vanish
    /// there), which `values_are_bounded_and_repeat_on_the_lattice` asserts.
    // The single-letter names are the *algorithm's*: `x`/`y`/`z` are a mathematical
    // function's arguments, and `xi`/`yi`/`zi`, `xf`/`yf`/`zf` and `u`/`v`/`w` are the
    // canonical names from Perlin's formulation. Renaming them to satisfy a style lint
    // would make this harder to check against the algorithm, so the lint is allowed here
    // and nowhere else in the crate.
    #[must_use]
    #[allow(clippy::many_single_char_names)]
    pub fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        // `x`/`y`/`z` are the domain's notation and stay in the signature; the locals are
        // named for what they are, so the eight single-character bindings the lint objected
        // to do not exist.
        let (sample_x, sample_y, sample_z) = (
            clamp_coordinate(x),
            clamp_coordinate(y),
            clamp_coordinate(z),
        );
        let (floor_x, floor_y, floor_z) = (sample_x.floor(), sample_y.floor(), sample_z.floor());
        // The lattice cell, wrapped into the table period.
        let xi = to_lattice(floor_x);
        let yi = to_lattice(floor_y);
        let zi = to_lattice(floor_z);
        let (xf, yf, zf) = (sample_x - floor_x, sample_y - floor_y, sample_z - floor_z);
        let (u, v, w) = (fade(xf), fade(yf), fade(zf));

        // `perm` is periodic with period 256, so masking an index that would run
        // past the doubled table is exact, not a wrap-around bug. Every index
        // below is masked, which is what makes the array accesses total.
        let perm = |index: usize| self.permutation[index & TABLE_MASK_2X] as usize;
        let a = perm(xi) + yi;
        let b = perm(xi + 1) + yi;
        let aa = perm(a) + zi;
        let ab = perm(a + 1) + zi;
        let ba = perm(b) + zi;
        let bb = perm(b + 1) + zi;

        let x1 = lerp(
            u,
            grad(perm(aa), xf, yf, zf),
            grad(perm(ba), xf - 1.0, yf, zf),
        );
        let x2 = lerp(
            u,
            grad(perm(ab), xf, yf - 1.0, zf),
            grad(perm(bb), xf - 1.0, yf - 1.0, zf),
        );
        let x3 = lerp(
            u,
            grad(perm(aa + 1), xf, yf, zf - 1.0),
            grad(perm(ba + 1), xf - 1.0, yf, zf - 1.0),
        );
        let x4 = lerp(
            u,
            grad(perm(ab + 1), xf, yf - 1.0, zf - 1.0),
            grad(perm(bb + 1), xf - 1.0, yf - 1.0, zf - 1.0),
        );
        lerp(w, lerp(v, x1, x2), lerp(v, x3, x4))
    }

    /// Two-dimensional convenience wrapper, sampling the `y = 0` plane.
    #[must_use]
    pub fn value_2d(&self, x: f64, z: f64) -> f64 {
        self.value(x, 0.0, z)
    }
}

/// Wrap a floored coordinate into `0..TABLE_SIZE`, for any finite input.
///
/// Inputs are already clamped to [`COORDINATE_LIMIT`], so the `i64` conversion is
/// lossless; `bitand` rather than `%` keeps the wrap correct for negative values
/// without relying on the remainder's sign.
fn to_lattice(floored: f64) -> usize {
    let value = floored as i64;
    (value & TABLE_MASK as i64) as usize
}

/// The lattice period of a [`PerlinNoise`], in that noise's own coordinates.
///
/// `to_lattice` masks the integer coordinate with `TABLE_MASK`, so the field is periodic with
/// this period **by construction**. The consequence is a real constraint on every frequency
/// constant: a field sampled at frequency `f` repeats every `LATTICE_PERIOD / f` blocks, so a
/// frequency near `1.0` would tile terrain visibly. The terrain's own frequencies are chosen
/// with that in mind and `the_frequency_constants_do_not_tile_terrain` guards it.
///
/// (A probe of mine once reported this as a bug. It is not: it is what a 256-entry
/// permutation table *is*, and Vanilla's `ImprovedNoise` behaves the same way — its terrain
/// avoids tiling by sampling at a frequency far below 1, not by widening the table.)
pub const LATTICE_PERIOD: f64 = {
    // TABLE_SIZE is 256, so this is exact; the u16 intermediate makes the narrowing
    // explicit rather than relying on a usize -> f64 cast that could lose precision on a
    // hypothetical wider table.
    const SIZE: u16 = TABLE_SIZE as u16;
    SIZE as f64
};

/// Clamp a coordinate into the range the lattice casts can represent.
fn clamp_coordinate(value: f64) -> f64 {
    // NaN compares false against everything, so `clamp` would propagate it; the
    // explicit check turns it into 0.0 instead of letting it poison the result.
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-COORDINATE_LIMIT, COORDINATE_LIMIT)
    }
}

/// Perlin's improved fade curve `6t⁵ - 15t⁴ + 10t³`.
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Linear interpolation.
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    // Written as `a + t*(b-a)`, not `(1-t)*a + t*b`: the first form is exact at
    // `t = 0` and `t = 1`, which keeps lattice values reproducible at exactly 0.
    a + t * (b - a)
}

/// Dot product of the gradient selected by the low four bits of `hash` with a
/// displacement vector.
///
/// `hash` is the already-masked table index (0..512); only its low four bits
/// select a gradient, which is the published selection rule.
fn grad(hash: usize, x: f64, y: f64, z: f64) -> f64 {
    let gradient = GRADIENTS[hash & 15];
    (gradient[0] * x + gradient[1] * y + gradient[2] * z) * GRADIENT_SCALE
}

/// A fractal (fBm) sum of [`PerlinNoise`] octaves.
///
/// The value is `Σ aᵢ · noise(fᵢ · p) / Σ aᵢ` with `aᵢ = persistenceⁱ` and
/// `fᵢ = lacunarityⁱ`. Dividing by the amplitude sum is the *normalization*
/// Vanilla's `NormalNoise` also performs; the octaves themselves are ours.
#[derive(Debug, Clone, PartialEq)]
pub struct FractalNoise {
    octaves: Vec<PerlinNoise>,
    amplitudes: Vec<f64>,
    /// Sum of `amplitudes`, precomputed and floored at 1.0 so it is never zero.
    ///
    /// Each octave is divided by this sum. That is the `1/fBm` normalization
    /// Vanilla's `NormalNoise` also applies; it keeps an 8-octave field in the
    /// same band as a 1-octave one instead of letting the octaves pile up.
    amplitude_sum: f64,
    frequency: f64,
    lacunarity: f64,
}

impl FractalNoise {
    /// Build a fractal noise field.
    ///
    /// - `octaves` is clamped into `0..=MAX_OCTAVES`; `0` yields a constant `0.0`.
    /// - `persistence` and `lacunarity` are clamped to a positive, finite range;
    ///   a non-finite or non-positive value falls back to the documented default
    ///   rather than producing NaNs or an unbounded frequency chain.
    #[must_use]
    pub fn new(seed: i64, octaves: u32, persistence: f64, lacunarity: f64) -> Self {
        let octaves = octaves.min(MAX_OCTAVES);
        let persistence = sane(persistence, DEFAULT_PERSISTENCE);
        let lacunarity = sane(lacunarity, DEFAULT_LACUNARITY);

        let mut sources = Vec::with_capacity(octaves as usize);
        let mut amplitudes = Vec::with_capacity(octaves as usize);
        let mut amplitude = 1.0_f64;
        for octave in 0..octaves {
            // Each octave gets its own table, derived from the field seed so the
            // whole field is reproducible from one number. The golden test
            // freezes the consequence.
            let octave_seed = crate::seed::splitmix64_mix(
                seed as u64 ^ 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(u64::from(octave) + 1),
            ) as i64;
            sources.push(PerlinNoise::new(octave_seed));
            amplitudes.push(amplitude);
            amplitude *= persistence;
        }
        let amplitude_sum = amplitudes.iter().sum::<f64>().max(1.0);
        Self {
            octaves: sources,
            amplitudes,
            amplitude_sum,
            frequency: 1.0,
            lacunarity,
        }
    }

    /// A fractal with the documented defaults ([`DEFAULT_PERSISTENCE`],
    /// [`DEFAULT_LACUNARITY`]).
    #[must_use]
    pub fn with_octaves(seed: i64, octaves: u32) -> Self {
        Self::new(seed, octaves, DEFAULT_PERSISTENCE, DEFAULT_LACUNARITY)
    }

    /// The octave count actually in use (after clamping).
    #[must_use]
    pub fn octaves(&self) -> u32 {
        self.octaves.len() as u32
    }

    /// The base frequency applied before the first octave.
    #[must_use]
    pub const fn frequency(&self) -> f64 {
        self.frequency
    }

    /// Set the base frequency (builder style). A non-finite or non-positive value
    /// falls back to `1.0`.
    #[must_use]
    pub fn at_frequency(mut self, frequency: f64) -> Self {
        self.frequency = sane(frequency, 1.0);
        self
    }

    /// The value at a point, normalized into the nominal `[-1, 1]` band.
    ///
    /// Exactly `0.0` when the field has no octaves.
    ///
    /// **The realized range is much narrower than `[-1, 1]`, and that is a
    /// measured fact, not a bug.** Dividing by the amplitude sum means the value
    /// can only reach 1 if every octave reaches its own maximum at the same point
    /// with the same sign, which independent octaves do not do. The measured
    /// range for the defaults is recorded in `tests/golden.rs`; callers that need
    /// a *guaranteed* range must clamp, and callers that want to use the whole
    /// band must scale by the inverse of the measured range.
    #[must_use]
    pub fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        if self.octaves.is_empty() {
            return 0.0;
        }
        let mut total = 0.0;
        let mut frequency = self.frequency;
        for (source, amplitude) in self.octaves.iter().zip(self.amplitudes.iter()) {
            total += amplitude * source.value(x * frequency, y * frequency, z * frequency);
            frequency *= self.lacunarity;
        }
        total / self.amplitude_sum
    }

    /// The value in the `y = 0` plane.
    #[must_use]
    pub fn value_2d(&self, x: f64, z: f64) -> f64 {
        self.value(x, 0.0, z)
    }
}

/// Replace a non-finite or non-positive parameter with its documented default.
fn sane(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // Determinism assertions compare exact bits on purpose.
mod tests {
    use super::{DEFAULT_LACUNARITY, DEFAULT_PERSISTENCE, FractalNoise, MAX_OCTAVES, PerlinNoise};

    #[test]
    fn the_same_seed_and_coordinate_give_the_same_value() {
        let first = PerlinNoise::new(1_361_882_806);
        let second = PerlinNoise::new(1_361_882_806);
        for index in 0..256 {
            let x = f64::from(index) * 0.37;
            let z = f64::from(index) * -0.11;
            assert_eq!(first.value_2d(x, z), second.value_2d(x, z));
            assert_eq!(first.value(x, 2.5, z), second.value(x, 2.5, z));
        }
        // A different seed gives a different table, hence a different field.
        let other = PerlinNoise::new(1_361_882_807);
        let differs = (0..64).any(|index| {
            let x = f64::from(index) * 0.37;
            first.value_2d(x, x) != other.value_2d(x, x)
        });
        assert!(differs, "a different seed must give a different field");
        // The table really is a permutation of 0..=255.
        let mut table = first.permutation().to_vec();
        table.sort_unstable();
        assert!(
            table
                .iter()
                .enumerate()
                .all(|(slot, value)| slot == *value as usize),
            "the permutation table must contain every value exactly once"
        );
    }

    #[test]
    fn values_are_bounded_and_repeat_on_the_lattice() {
        let noise = PerlinNoise::new(99);
        let mut minimum = f64::MAX;
        let mut maximum = f64::MIN;
        for x in -60..60 {
            for z in -60..60 {
                let value = noise.value_2d(f64::from(x) * 0.5, f64::from(z) * 0.5);
                assert!(value.is_finite(), "value at ({x}, {z}) is not finite");
                assert!(
                    (-1.0..=1.0).contains(&value),
                    "noise out of the nominal band: {value} at ({x}, {z})"
                );
                minimum = minimum.min(value);
                maximum = maximum.max(value);
            }
        }
        // A real field, not a constant and not all zero.
        assert!(
            maximum > 0.2 && minimum < -0.2,
            "range {minimum}..{maximum}"
        );
        // Exactly zero at every integer lattice point.
        for x in -3..3 {
            for z in -3..3 {
                assert_eq!(
                    noise.value_2d(f64::from(x), f64::from(z)),
                    0.0,
                    "lattice point ({x}, {z})"
                );
            }
        }
    }

    #[test]
    fn hostile_coordinates_are_clamped_not_panicking() {
        let noise = PerlinNoise::new(7);
        for value in [
            f64::MAX,
            f64::MIN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            f64::from(i32::MAX),
            f64::from(i32::MIN),
            -30_000_000.0,
            30_000_000.0,
        ] {
            let sampled = noise.value(value, value, value);
            assert!(sampled.is_finite(), "value({value}) = {sampled}");
            assert!(sampled.abs() <= 1.0, "value({value}) = {sampled}");
        }
    }

    #[test]
    fn octave_counts_are_clamped_and_zero_is_flat() {
        let none = FractalNoise::with_octaves(5, 0);
        assert_eq!(none.octaves(), 0);
        assert_eq!(none.value(1.0, 2.0, 3.0), 0.0, "no octaves is a flat zero");
        assert_eq!(none.value_2d(-99.0, 1234.0), 0.0);

        let huge = FractalNoise::with_octaves(5, u32::MAX);
        assert_eq!(huge.octaves(), MAX_OCTAVES, "clamped, not refused");
        let value = huge.value(0.5, 0.5, 0.5);
        assert!(value.is_finite());
        assert!(value.abs() <= 1.0, "normalized: {value}");

        // Nonsensical shaping parameters fall back to the documented defaults.
        let silly = FractalNoise::new(5, 4, f64::NAN, -3.0);
        let value = silly.value(1.0, 1.0, 1.0);
        assert!(value.is_finite() && value.abs() <= 1.0, "{value}");
        let reference = FractalNoise::new(5, 4, DEFAULT_PERSISTENCE, DEFAULT_LACUNARITY);
        assert_eq!(value, reference.value(1.0, 1.0, 1.0));
    }

    #[test]
    fn fractal_values_stay_in_the_nominal_band() {
        for octaves in 1..=MAX_OCTAVES {
            let noise = FractalNoise::with_octaves(31, octaves).at_frequency(0.01);
            for step in -120..120 {
                let value = noise.value_2d(f64::from(step) * 8.0, f64::from(step) * -5.0);
                assert!(value.is_finite(), "octaves={octaves} step={step}");
                assert!(
                    (-1.0..=1.0).contains(&value),
                    "octaves={octaves} step={step} value={value}"
                );
            }
        }
    }

    #[test]
    fn a_frequency_shift_changes_the_field() {
        let slow = FractalNoise::with_octaves(3, 4).at_frequency(0.01);
        let fast = FractalNoise::with_octaves(3, 4).at_frequency(0.5);
        // A half-integer coordinate, so neither field is sampled on a lattice
        // point (where every Perlin value is exactly zero, whatever the seed).
        assert_ne!(slow.value_2d(100.5, 100.5), fast.value_2d(100.5, 100.5));
        assert!(slow.value_2d(100.5, 100.5).abs() > 1e-9);
        // A non-finite frequency falls back rather than poisoning the field.
        let broken = FractalNoise::with_octaves(3, 4).at_frequency(f64::NAN);
        assert_eq!(broken.frequency(), 1.0);
        assert!(broken.value_2d(1.0, 1.0).is_finite());
    }
}
