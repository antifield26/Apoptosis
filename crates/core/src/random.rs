//! Seeded random source matching `java.util.Random` (P05-02, P05-11..P05-14).
//!
//! Mob spawning and AI make random choices, so an uncontrolled RNG would make the
//! simulation unreproducible and break AGENTS.md §3.6. This is a seeded 48-bit
//! linear congruential generator implementing the algorithm the JDK documents:
//!
//! ```text
//! next(bits): seed = (seed * 0x5DEECE66D + 0xB) mod 2^48; return seed >>> (48 - bits)
//! ```
//!
//! Matching `java.util.Random` rather than using a Rust PRNG is deliberate: it is
//! the generator Vanilla's `LegacyRandomSource` wraps, so the *same seed draws the
//! same sequence*. That makes a differential comparison against a vanilla server
//! meaningful later, and it costs nothing now.
//!
//! ## Caveats, stated rather than implied
//!
//! - This is **not** cryptographic and must never be used for anything security
//!   related (tokens, salts, session ids).
//! - Vanilla's `Level.random` is seeded from the world seed *and* advanced by an
//!   amount that depends on tick count and dimension; we expose a plain seeded
//!   sequence and let the caller define the seeding policy. Anything that claims
//!   parity with a specific vanilla random draw would need that policy verified
//!   first, which is why no such claim is made here.

/// Multiplier of the `java.util.Random` LCG.
const MULTIPLIER: u64 = 0x0005_DEEC_E66D;
/// Addend of the `java.util.Random` LCG.
const ADDEND: u64 = 0xB;
/// The generator's state is 48 bits.
const MASK: u64 = (1 << 48) - 1;

/// A reproducible random source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomSource {
    seed: u64,
    /// Draws taken since construction, for determinism diagnostics.
    draws: u64,
}

impl RandomSource {
    /// Create a source from a seed, scrambling it exactly as
    /// `new java.util.Random(seed)` does (`seed ^ 0x5DEECE66D`).
    #[must_use]
    pub const fn new(seed: i64) -> Self {
        // `Java Random(long seed)`: `this.seed = (seed ^ 0x5DEECE66DL) & ((1L << 48) - 1)`.
        // The XOR is on the two's-complement bit pattern, so the reinterpretation is
        // the algorithm.
        Self {
            seed: (seed.cast_unsigned() ^ MULTIPLIER) & MASK,
            draws: 0,
        }
    }

    /// The current internal state (the scrambled seed).
    #[must_use]
    pub const fn state(&self) -> u64 {
        self.seed
    }

    /// How many draws this source has taken.
    #[must_use]
    pub const fn draws(&self) -> u64 {
        self.draws
    }

    /// Next `bits` (1..=32) of randomness.
    fn next(&mut self, bits: u32) -> u32 {
        let bits = bits.clamp(1, 32);
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        self.draws = self.draws.saturating_add(1);
        (self.seed >> (48 - bits)) as u32
    }

    /// `java.util.Random.nextInt()`.
    #[must_use]
    pub fn next_i32(&mut self) -> i32 {
        // Java returns the 32 bits as a signed `int`.
        self.next(32).cast_signed()
    }

    /// `java.util.Random.nextInt(bound)`: uniform in `0..bound`.
    ///
    /// A non-positive `bound` returns 0 rather than panicking: the value can come
    /// from a config or a data file, and a wrong bound must not take the server
    /// down (AGENTS.md §9).
    #[must_use]
    pub fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        // Rejection sampling keeps the distribution uniform, matching the JDK.
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound.cast_unsigned();
            // Java: `for (int u = r; u - (r = u % bound) + m < 0; u = next(31));`
            // The comparison is signed 32-bit, so it must wrap: evaluating it in a
            // wider type would never be negative and the rejection would never fire.
            let test = bits
                .wrapping_sub(value)
                .wrapping_add((bound - 1).cast_unsigned());
            if test.cast_signed() >= 0 {
                return value.cast_signed();
            }
        }
    }

    /// `java.util.Random.nextLong()`.
    #[must_use]
    pub fn next_i64(&mut self) -> i64 {
        // Java: `((long)(next(32)) << 32) + next(32)`. `next(32)` is an `int`, so
        // both halves are **sign-extended**; treating them as unsigned would give a
        // different answer whenever the high bit is set (a bug this test caught).
        let high = i64::from(self.next(32).cast_signed());
        let low = i64::from(self.next(32).cast_signed());
        (high << 32).wrapping_add(low)
    }

    /// `java.util.Random.nextFloat()`: uniform in `[0, 1)`.
    #[must_use]
    pub fn next_f32(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }

    /// `java.util.Random.nextDouble()`: uniform in `[0, 1)`.
    #[must_use]
    pub fn next_f64(&mut self) -> f64 {
        let high = i64::from(self.next(26)) << 27;
        let low = i64::from(self.next(27));
        (high + low) as f64 / (1i64 << 53) as f64
    }

    /// `java.util.Random.nextBoolean()`.
    #[must_use]
    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    /// Uniform `f32` in `[min, max)`; returns `min` for a degenerate range.
    #[must_use]
    pub fn next_f32_between(&mut self, min: f32, max: f32) -> f32 {
        // `max <= min` (including a NaN bound, where the comparison is false) yields
        // `min` rather than an unbounded or NaN result.
        if max.partial_cmp(&min).is_none_or(std::cmp::Ordering::is_le) {
            return min;
        }
        min + self.next_f32() * (max - min)
    }

    /// Uniform `f64` in `[min, max)`; returns `min` for a degenerate range.
    #[must_use]
    pub fn next_f64_between(&mut self, min: f64, max: f64) -> f64 {
        // See `next_f32_between`: a NaN bound collapses to `min`.
        if max.partial_cmp(&min).is_none_or(std::cmp::Ordering::is_le) {
            return min;
        }
        min + self.next_f64() * (max - min)
    }

    /// Random offset in `[-bound, bound]` on each axis, as Vanilla's
    /// `RandomSource.triangle`-adjacent helpers do for wander targets.
    #[must_use]
    pub fn next_i32_inclusive(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + self.next_i32_bounded(max - min + 1)
    }

    /// Whether a `1 in n` chance succeeds; `false` for `n <= 0`.
    #[must_use]
    pub fn one_in(&mut self, n: i32) -> bool {
        n > 0 && self.next_i32_bounded(n) == 0
    }

    /// Pick an index in `0..len`; `None` when `len` is zero.
    #[must_use]
    pub fn index(&mut self, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        // `len` is a real collection length, so it fits in `i32` for any plausible
        // collection; `next_i32_bounded` returns a value in `0..bound`, which is
        // therefore non-negative.
        let bound = i32::try_from(len).unwrap_or(i32::MAX);
        Some(self.next_i32_bounded(bound).cast_unsigned() as usize)
    }
}

#[cfg(test)]
// The golden vectors compare against the exact values the JDK printed, and the
// degenerate-range cases return their input unchanged: these are exact
// comparisons by construction, not approximate ones.
#[allow(clippy::float_cmp)]
mod tests {
    use super::RandomSource;

    #[test]
    fn matches_the_jdk_for_known_seeds() {
        // Measured with `java.util.Random` on JDK 25 (`target/vanilla-26.1.2/RandomProbe.java`),
        // the same runtime the 26.1.2 server uses. Order per seed:
        // nextInt, nextInt, nextLong, nextDouble, nextFloat, nextBoolean.
        // Integers are compared as integers: going through `f64` would silently
        // lose the low bits of a `nextLong`.
        struct Expect {
            seed: i64,
            ints: [i32; 2],
            long: i64,
            double: f64,
            float: f32,
            boolean: bool,
        }
        let expected = [
            Expect {
                seed: 42,
                ints: [-1_170_105_035, 234_785_527],
                long: -5_843_495_416_241_995_736,
                double: 0.308_719_455_332_659_76,
                float: 0.277_078_45,
                boolean: true,
            },
            Expect {
                seed: 0,
                ints: [-1_155_484_576, -723_955_400],
                long: 4_437_113_781_045_784_766,
                double: 0.637_417_425_350_108_3,
                float: 0.550_437,
                boolean: false,
            },
            Expect {
                seed: 12345,
                ints: [1_553_932_502, -2_090_749_135],
                long: -1_236_052_134_575_208_584,
                double: 0.833_091_348_971_023_7,
                float: 0.326_475_74,
                boolean: false,
            },
        ];
        for case in expected {
            let seed = case.seed;
            let mut source = RandomSource::new(seed);
            assert_eq!(source.next_i32(), case.ints[0], "seed {seed} nextInt");
            assert_eq!(source.next_i32(), case.ints[1], "seed {seed} nextInt");
            assert_eq!(source.next_i64(), case.long, "seed {seed} nextLong");
            assert_eq!(source.next_f64(), case.double, "seed {seed} nextDouble");
            assert_eq!(source.next_f32(), case.float, "seed {seed} nextFloat");
            assert_eq!(source.next_bool(), case.boolean, "seed {seed} nextBoolean");
        }
    }

    #[test]
    fn bounded_draws_match_the_jdk_including_the_power_of_two_branch() {
        // `new Random(7).nextInt(37)` eight times, from a fresh generator…
        let mut source = RandomSource::new(7);
        let expected_37 = [8, 8, 36, 2, 7, 23, 7, 16];
        for (index, want) in expected_37.iter().enumerate() {
            assert_eq!(
                source.next_i32_bounded(37),
                *want,
                "nextInt(37) draw {index}"
            );
        }
        // …and `new Random(7).nextInt(64)` eight times, also from a fresh one: the
        // bounded sequence above consumes the same state as Java's, so this is a
        // check on the power-of-two path in isolation.
        let mut power_of_two = RandomSource::new(7);
        let expected_64 = [46, 40, 47, 0, 22, 31, 57, 47];
        for (index, want) in expected_64.iter().enumerate() {
            assert_eq!(
                power_of_two.next_i32_bounded(64),
                *want,
                "nextInt(64) draw {index}"
            );
        }
    }

    #[test]
    fn the_same_seed_replays_exactly() {
        let draws = |seed: i64| {
            let mut source = RandomSource::new(seed);
            (0..64)
                .map(|_| {
                    (
                        source.next_i32(),
                        source.next_f64_between(-10.0, 10.0),
                        source.next_i32_bounded(37),
                        source.next_bool(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(draws(12345), draws(12345), "same seed, same sequence");
        assert_ne!(
            draws(12345),
            draws(12346),
            "different seed, different sequence"
        );
    }

    #[test]
    fn bounded_draws_stay_in_range() {
        let mut source = RandomSource::new(7);
        for _ in 0..10_000 {
            let value = source.next_i32_bounded(37);
            assert!((0..37).contains(&value), "{value}");
        }
        // Non-positive bounds are refused, not panicked on.
        assert_eq!(source.next_i32_bounded(0), 0);
        assert_eq!(source.next_i32_bounded(-5), 0);
        // A power-of-two bound goes through the other branch.
        for _ in 0..1_000 {
            let value = source.next_i32_bounded(64);
            assert!((0..64).contains(&value), "{value}");
        }
    }

    #[test]
    fn bounded_draws_are_roughly_uniform() {
        // A coarse sanity check that the rejection loop is not systematically
        // biased; 10 000 draws over 10 buckets should be within 20% of even.
        let mut source = RandomSource::new(99);
        let mut buckets = [0usize; 10];
        for _ in 0..10_000 {
            buckets[source.next_i32_bounded(10) as usize] += 1;
        }
        for (index, count) in buckets.iter().enumerate() {
            assert!(
                (800..1200).contains(count),
                "bucket {index} got {count} of 10000"
            );
        }
    }

    #[test]
    fn floats_stay_in_their_ranges() {
        let mut source = RandomSource::new(5);
        for _ in 0..10_000 {
            let unit = source.next_f32();
            assert!((0.0..1.0).contains(&unit), "{unit}");
            let unit64 = source.next_f64();
            assert!((0.0..1.0).contains(&unit64), "{unit64}");
            let ranged = source.next_f32_between(-3.0, 3.0);
            assert!((-3.0..3.0).contains(&ranged), "{ranged}");
            let ranged64 = source.next_f64_between(1.0, 2.0);
            assert!((1.0..2.0).contains(&ranged64), "{ranged64}");
        }
        assert_eq!(source.next_f32_between(5.0, 5.0), 5.0);
        assert_eq!(source.next_f64_between(5.0, 1.0), 5.0, "degenerate range");
    }

    #[test]
    fn inclusive_and_one_in_helpers_behave() {
        let mut source = RandomSource::new(11);
        for _ in 0..1_000 {
            let value = source.next_i32_inclusive(-4, 4);
            assert!((-4..=4).contains(&value), "{value}");
        }
        assert_eq!(source.next_i32_inclusive(3, 3), 3);
        assert_eq!(source.next_i32_inclusive(5, 1), 5, "inverted range");

        // `one_in(1)` always succeeds; `one_in(0)` never does.
        assert!(source.one_in(1));
        assert!(!source.one_in(0));
        assert!(!source.one_in(-1));
        let hits = (0..1_000).filter(|_| source.one_in(4)).count();
        assert!(
            (150..350).contains(&hits),
            "one_in(4) fired {hits} times in 1000"
        );
    }

    #[test]
    fn index_handles_empty_and_bounded_input() {
        let mut source = RandomSource::new(3);
        assert_eq!(source.index(0), None);
        for _ in 0..1_000 {
            let value = source.index(7).expect("some");
            assert!(value < 7);
        }
    }

    #[test]
    fn draw_count_tracks_calls_and_state_advances() {
        let mut source = RandomSource::new(1);
        let initial = source.state();
        assert_eq!(source.draws(), 0);
        let _ = source.next_i32();
        assert_eq!(source.draws(), 1);
        assert_ne!(source.state(), initial);
        // `next_i64` takes two draws, matching the JDK.
        let before = source.draws();
        let _ = source.next_i64();
        assert_eq!(source.draws(), before + 2);
    }

    #[test]
    fn long_draws_are_full_width() {
        let mut source = RandomSource::new(2024);
        let mut saw_negative = false;
        let mut saw_positive = false;
        for _ in 0..256 {
            let value = source.next_i64();
            saw_negative |= value < 0;
            saw_positive |= value > 0;
        }
        assert!(
            saw_negative && saw_positive,
            "nextLong must use the full range"
        );
    }
}
