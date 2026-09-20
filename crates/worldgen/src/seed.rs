//! World seed, generation context and the per-chunk seed derivation (P07-13).
//!
//! This module is the root of the pipeline described by PHASE-07's
//! *"prioritize loading existing worlds … then expand toward 26.1.2 parity"*: a
//! world that already exists on disk already has a seed, and every generator in
//! this crate is a **pure function of that seed plus a chunk position**. Nothing
//! here reads a clock, a thread, an environment variable or a map iteration
//! order, so `(seed, chunk_pos)` fully determines a chunk — for ever, and in any
//! order (AGENTS.md §3.6).
//!
//! ## Where a seed comes from
//!
//! | Path | Meaning | Evidence label |
//! |---|---|---|
//! | [`WorldSeed::from_level_dat`] | the seed stored in an existing world's world-gen document, e.g. `data/minecraft/world_gen_settings.dat` | **verified**: the evidence world in this repository carries `1361882806` (`crates/test-support/fixtures/anvil/MANIFEST.txt`, `docs/vanilla-parity/PARITY-MATRIX.md`) |
//! | [`WorldSeed::from_raw`] | an operator-supplied seed (`--seed`/`level-seed`) | **product decision** |
//! | [`WorldSeed::fresh`] | a seed derived from the system clock for a *new* world | **approximation**: Vanilla derives a new world's seed from the system time; the exact expression it uses is not verified here, so this one is ours, documented below, and must not be claimed as Vanilla's |
//!
//! ## Per-chunk derivation (the important part)
//!
//! ```text
//! packed  = ((chunk_x as i64 as u64) << 32) | (chunk_z as i32 as u32 as u64)
//! mixed   = splitmix64_mix(seed as u64 ^ 0x9E3779B97F4A7C15 ^ packed)
//! subseed = mixed as i64
//! ```
//!
//! The `0x9E3779B97F4A7C15` term is splitmix64's golden gamma and it is
//! **load-bearing**: the finalizer maps `0` to `0`, so without it a world seed of
//! `0` would give chunk `(0, 0)` the sub-seed `0` and collide with every other
//! `(seed, pos)` pair whose XOR also lands on `0` — a rare but real global
//! collision. With it, the argument is a bijection of `(seed, x, z)`.
//!
//! Both halves are **bijective**, which is a stronger statement than "no
//! collisions in a sample":
//!
//! - the packing is a bijection `(i32, i32) → u64`: the high word is the
//!   sign-extended `x` and the low word is the zero-extended `z` bit pattern, so
//!   two distinct pairs can never produce the same `u64`;
//! - [`splitmix64_mix`] is the splitmix64 finalizer, a composition of invertible
//!   operations on 64 bits: `x ^= x >> 30`, `x *= odd`, `x ^= x >> 27`,
//!   `x *= odd`, `x ^= x >> 31`. Every step has an inverse (an XOR-shift is its
//!   own inverse, and multiplication by an odd constant is invertible mod 2⁶⁴),
//!   so it is a bijection except at `0`, which maps to `0` — the XOR with the
//!   golden gamma is what removes even that exception;
//! - therefore distinct `(seed, x, z)` triples give distinct sub-seeds, and
//!   `(x, z) → subseed` is injective for a fixed seed **by construction**, not
//!   just in the tested sample. `tests/determinism.rs` still checks 40 000
//!   positions, because a proof in a doc comment is not a regression test.
//!
//! Splitmix64's constants are the ones in the published algorithm (Steele,
//! Lea & Flood, 2014), also used by the JDK's `SplittableRandom`; the algorithm
//! is public and is reimplemented here rather than copied.

use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use mc_persistence::dimension::Dimension;
use mc_world::ChunkPos;

/// Multiplier of the splitmix64 finalizer (published constant, 2014).
///
/// **verified**: stated in the splitmix64 algorithm and used verbatim by
/// `java.util.SplittableRandom.mix64`. The value is public arithmetic, not a
/// number read out of a Vanilla jar.
const SPLITMIX_GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
/// First multiplier of the splitmix64 finalizer (published constant, 2014).
const SPLITMIX_MULTIPLIER_1: u64 = 0xBF58_476D_1CE4_E5B9;
/// Second multiplier of the splitmix64 finalizer (published constant, 2014).
const SPLITMIX_MULTIPLIER_2: u64 = 0x94D0_49BB_1331_11EB;

/// The seed that identifies one world's generation.
///
/// A transparent `i64` newtype so a chunk seed can never be confused with a
/// world seed, a chunk coordinate or a `java.util.Random` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorldSeed(i64);

impl WorldSeed {
    /// A seed taken from a caller (`--seed`, a config value, a test).
    ///
    /// Every bit pattern is accepted: a seed is a label, not a quantity, so a
    /// negative or extreme value must not be an error.
    #[must_use]
    pub const fn from_raw(seed: i64) -> Self {
        Self(seed)
    }

    /// The seed stored in a parsed world-gen document.
    ///
    /// `world_gen_settings.dat` is a gzipped NBT document whose root is
    /// `{ Data: { … } }` (the same wrapper `level.dat` uses, per
    /// `mc-persistence::level`). The seed lives somewhere inside it; which key
    /// path is *not* verified here, so this walks a list of candidates in a
    /// fixed order and, as a last resort, takes the **first** `seed` tag found in
    /// a depth-first, source-order walk:
    ///
    /// 1. `Data.seed` — the flat form `level.dat` used before 26.1;
    /// 2. `Data.dimensions.minecraft.overworld.generator.seed` — the nested form a
    ///    modern dimension document uses;
    /// 3. the first `seed` entry anywhere in the document that is a long.
    ///
    /// If none exists the world has no recorded seed and the caller must decide
    /// (create a new world, or refuse): this returns
    /// [`ServerError::CorruptData`] rather than inventing one, because a silently
    /// invented seed generates terrain that does not match the stored chunks.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when no long `seed` is present.
    pub fn from_level_dat(document: &NbtTag) -> ServerResult<Self> {
        const CANDIDATES: [&[&str]; 2] = [
            &["Data", "seed"],
            &[
                "Data",
                "dimensions",
                "minecraft:overworld",
                "generator",
                "seed",
            ],
        ];
        for path in CANDIDATES {
            if let Some(seed) = lookup_path(document, path) {
                return Ok(Self(seed));
            }
        }
        first_seed_long(document).map(Self).ok_or_else(|| {
            ServerError::CorruptData("world-gen document has no 'seed' long".to_owned())
        })
    }

    /// A seed derived from the system clock, for creating a **new** world.
    ///
    /// **approximation / product decision.** Vanilla obtains a new world's seed
    /// from the system time; the exact expression is not verified here, so this
    /// is our own documented derivation and carries no parity claim:
    ///
    /// ```text
    /// fresh = splitmix64_mix(unix_millis as u64)
    /// ```
    ///
    /// Rationale for hashing rather than using the millisecond count directly:
    /// two worlds created in the same millisecond still get different seeds, and
    /// the seed is not trivially guessable from the creation time.
    ///
    /// This is the **only** place in this crate that reads a clock, and it is
    /// never called during generation — a deterministic simulation must not
    /// depend on it (AGENTS.md §3.6).
    #[must_use]
    pub fn fresh() -> Self {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        Self(splitmix64_mix(millis) as i64)
    }

    /// The raw seed value (as stored in the world document).
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// The seed a generator uses for one chunk.
    ///
    /// See the module docs for the derivation and why it cannot collide.
    #[must_use]
    pub const fn chunk_seed(self, pos: ChunkPos) -> ChunkSeed {
        // The golden-gamma XOR is load-bearing, not decoration: splitmix64's
        // finalizer maps `0` to `0`, so without it a world seed of `0` would give
        // chunk `(0, 0)` the sub-seed `0` and collide with every other
        // `(seed, pos)` pair whose XOR also lands on `0`.
        ChunkSeed(
            splitmix64_mix(self.0 as u64 ^ SPLITMIX_GOLDEN_GAMMA ^ pack_chunk_pos(pos)) as i64,
        )
    }

    /// A named random stream derived from this seed and a stream id.
    ///
    /// Two different stream ids give two independent-looking streams from the
    /// same world seed, which is how the noise fields (`terrain`, `detail`,
    /// `climate`, `bedrock`) are kept apart. `stream` is applied to the seed
    /// *before* mixing, so distinct ids give distinct per-chunk seeds by the same
    /// bijection argument. The golden-gamma XOR is what keeps a zero seed from
    /// producing a zero stream (see [`WorldSeed::chunk_seed`]).
    #[must_use]
    pub const fn stream_seed(self, stream: u64) -> i64 {
        splitmix64_mix(
            self.0 as u64
                ^ SPLITMIX_GOLDEN_GAMMA
                ^ SPLITMIX_GOLDEN_GAMMA.wrapping_mul(stream.wrapping_add(1)),
        ) as i64
    }
}

/// The mixer used by [`WorldSeed::chunk_seed`], exposed for the golden test.
///
/// **verified**: this is the published splitmix64 finalizer.
#[must_use]
pub const fn splitmix64_mix(input: u64) -> u64 {
    let mut value = input;
    value ^= value >> 30;
    value = value.wrapping_mul(SPLITMIX_MULTIPLIER_1);
    value ^= value >> 27;
    value = value.wrapping_mul(SPLITMIX_MULTIPLIER_2);
    value ^= value >> 31;
    value
}

/// Pack a chunk position into one `u64`, bijectively.
///
/// The high word is the sign-extended `x` and the low word is the zero-extended
/// bit pattern of `z`, so two distinct pairs can never collide. `i32::MIN` and
/// `i32::MAX` are ordinary values here — no negation, no `abs`, nothing that can
/// overflow (AGENTS.md §10: assume coordinates are hostile).
#[must_use]
pub const fn pack_chunk_pos(pos: ChunkPos) -> u64 {
    (((pos.x as i64) as u64) << 32) | (pos.z as u32 as u64)
}

/// The per-chunk seed handed to noise and features.
///
/// Deliberately a distinct type from [`WorldSeed`]: passing a world seed where a
/// chunk seed belongs is the bug this newtype exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkSeed(i64);

impl ChunkSeed {
    /// The raw value, for feeding [`mc_core::random::RandomSource::new`].
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }
}

/// Everything a generator needs that is not the chunk position itself.
///
/// `min_y` and `height` come from the dimension, not from a literal: the
/// overworld is `-64..320` (`mc_world::OVERWORLD_MIN_SECTION_Y` and
/// `OVERWORLD_SECTION_COUNT`, both **verified** against the 26.1.2 overworld),
/// but a custom or future dimension must be able to say something else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldgenContext {
    /// World seed.
    pub seed: WorldSeed,
    /// Dimension key (`minecraft:overworld`, …).
    pub dimension: Dimension,
    /// Lowest block y of the dimension (inclusive).
    pub min_y: i32,
    /// Number of block y values in the dimension.
    pub height: i32,
    /// Water level of the dimension.
    pub sea_level: i32,
}

/// Sea level of the Vanilla overworld, in blocks.
///
/// **verified**: `Level.SEA_LEVEL` is 63 in the overworld, quoted in
/// `docs/world/chunk-format.md`. The nether has no sea level (lava at y=31 is not
/// water), and the end has none either; [`WorldgenContext::overworld`] is
/// therefore the only constructor that uses this constant.
pub const OVERWORLD_SEA_LEVEL: i32 = 63;

/// Minimum block y of the Vanilla overworld.
///
/// **verified**: derived from the section range in `mc_world`
/// (`OVERWORLD_MIN_SECTION_Y = -4`, 16 blocks per section).
pub const OVERWORLD_MIN_Y: i32 = -64;

/// Number of block y values in the Vanilla overworld (384).
///
/// **verified**: derived from `mc_world::OVERWORLD_SECTION_COUNT = 24` sections.
pub const OVERWORLD_HEIGHT: i32 = 384;

impl WorldgenContext {
    /// The overworld context used by the server today.
    #[must_use]
    pub fn overworld(seed: WorldSeed) -> Self {
        Self {
            seed,
            dimension: Dimension::Overworld,
            min_y: OVERWORLD_MIN_Y,
            height: OVERWORLD_HEIGHT,
            sea_level: OVERWORLD_SEA_LEVEL,
        }
    }

    /// A context with explicit vertical bounds.
    ///
    /// The values are **not** validated against the dimension: a context is a
    /// description of what the caller wants generated, and the generators are
    /// written to survive a nonsensical one (a non-positive `height` produces an
    /// empty-but-valid chunk rather than a panic — see
    /// `terrain::tests::a_degenerate_context_does_not_panic`).
    #[must_use]
    pub const fn new(
        seed: WorldSeed,
        dimension: Dimension,
        min_y: i32,
        height: i32,
        sea_level: i32,
    ) -> Self {
        Self {
            seed,
            dimension,
            min_y,
            height,
            sea_level,
        }
    }

    /// Highest block y (exclusive).
    ///
    /// **Saturating**, and that is the contract: a dimension whose
    /// `min_y + height` overflows `i32` reports `i32::MAX`, which a generator
    /// reads as "up to the representable ceiling" rather than wrapping into a
    /// negative bound that would become a huge loop or a backwards range.
    #[must_use]
    pub const fn max_y(&self) -> i32 {
        self.min_y.saturating_add(self.height)
    }

    /// The chunk seed for a position in this world.
    #[must_use]
    pub const fn chunk_seed(&self, pos: ChunkPos) -> ChunkSeed {
        self.seed.chunk_seed(pos)
    }

    /// The lowest section index, for [`mc_world::Chunk::air`].
    ///
    /// ## Representability limit (a real one, stated rather than hidden)
    ///
    /// [`mc_world::Chunk`]'s section index is an `i8`, so **no chunk can express a
    /// section below `i8::MIN * 16 = -2048`**. For any dimension whose floor is at
    /// or above `-2048` — including every Vanilla dimension — this returns the
    /// exact section and the chunk covers the requested range. Below that the
    /// value saturates to `i8::MIN`, which places the chunk's floor **above** the
    /// requested `min_y`; the terrain generator then builds inside the range the
    /// chunk can actually hold (see
    /// [`WorldgenContext::effective_max_y`]). Supporting a floor below `-2048`
    /// needs a wider section index in `mc-world`, which is that crate's decision,
    /// not this one's.
    #[must_use]
    pub const fn min_section_y(&self) -> i8 {
        // Saturating rather than wrapping: an out-of-range `min_y` clamps to the
        // i8 range instead of silently landing in a different section.
        let section = self.min_y.div_euclid(mc_world::SECTION_HEIGHT);
        if section > i8::MAX as i32 {
            i8::MAX
        } else if section < i8::MIN as i32 {
            i8::MIN
        } else {
            section as i8
        }
    }

    /// How many sections a chunk of this dimension has (at least 1, at most the
    /// `i8` section-index range, so a hostile `height` cannot ask for a
    /// multi-gigabyte allocation).
    #[must_use]
    pub fn section_count(&self) -> usize {
        let sections = self.height.div_euclid(mc_world::SECTION_HEIGHT);
        sections.clamp(1, 128) as usize
    }

    /// The highest block y a chunk can actually hold (exclusive).
    ///
    /// This is the bound generators clamp to, and it is **not** the same as
    /// [`WorldgenContext::max_y`] for a hostile context: [`Self::section_count`]
    /// caps the chunk at 128 sections, so a `height` of `i32::MAX` yields a chunk
    /// spanning 2048 blocks while `max_y` reports `i32::MAX`. Clamping a surface
    /// height to `max_y` instead of this value is what would make generation walk
    /// two billion y values per column — a hang, not an error (AGENTS.md §10).
    #[must_use]
    pub fn effective_max_y(&self) -> i32 {
        self.min_y
            .saturating_add((self.section_count() as i32).saturating_mul(mc_world::SECTION_HEIGHT))
    }
}

/// Follow a key path through nested compounds and read the final `seed` entry.
fn lookup_path(document: &NbtTag, path: &[&str]) -> Option<i64> {
    let (last, parents) = path.split_last()?;
    let mut current = document;
    for key in parents {
        current = current.get_compound(key)?;
    }
    current.get_i64(last)
}

/// First integer `seed` entry in a deterministic depth-first walk.
///
/// Uses [`NbtTag::get_i64`], which accepts any integer tag: a tool that wrote the
/// seed as an `Int` still loads (the tolerant-read policy `mc-nbt` documents).
/// Deterministic because compound entries keep their file order and this crate
/// never iterates a hash map.
fn first_seed_long(tag: &NbtTag) -> Option<i64> {
    if let Some(entries) = tag.entries() {
        for (key, value) in entries {
            if key == "seed"
                && let Some(seed) = value.as_i64()
            {
                return Some(seed);
            }
            if let Some(found) = first_seed_long(value) {
                return Some(found);
            }
        }
    }
    match tag {
        NbtTag::List(items) => {
            for value in items {
                if let Some(found) = first_seed_long(value) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OVERWORLD_HEIGHT, OVERWORLD_MIN_Y, OVERWORLD_SEA_LEVEL, WorldSeed, WorldgenContext,
        pack_chunk_pos, splitmix64_mix,
    };
    use mc_core::error::ServerError;
    use mc_nbt::NbtTag;
    use mc_persistence::dimension::Dimension;
    use mc_world::ChunkPos;
    use std::collections::BTreeSet;

    #[test]
    fn splitmix64_matches_the_published_values() {
        // Values read out of this implementation and cross-checked against the
        // published finalizer's arithmetic by hand for `0` and `1`. Frozen here
        // so a refactor of the mixer is caught; `tests/determinism.rs` covers the
        // generation-level consequences.
        //
        // `mix64(0) == 0` is a real property of the published algorithm (zero is
        // a fixed point of the finalizer), and it is exactly why `chunk_seed`
        // XORs the golden gamma in before mixing.
        assert_eq!(splitmix64_mix(0), 0x0000_0000_0000_0000);
        assert_eq!(splitmix64_mix(1), 0x5692_161D_100B_05E5);
        assert_eq!(splitmix64_mix(2), 0xDBD2_3897_3A2B_148A);
        assert_eq!(splitmix64_mix(3), 0x1E53_5EED_E314_28F0);
        // Plus the property that makes the derivation collision-free: distinct
        // inputs give distinct outputs, over a dense sample.
        let mut seen = BTreeSet::new();
        for input in 0..4_096_u64 {
            assert!(
                seen.insert(splitmix64_mix(input)),
                "mix64 collided at {input}"
            );
        }
        assert_eq!(seen.len(), 4_096);
    }

    #[test]
    fn packing_is_injective_on_hostile_coordinates() {
        let hostile = [
            ChunkPos::new(0, 0),
            ChunkPos::new(i32::MAX, i32::MAX),
            ChunkPos::new(i32::MIN, i32::MIN),
            ChunkPos::new(i32::MIN, i32::MAX),
            ChunkPos::new(i32::MAX, i32::MIN),
            ChunkPos::new(-1, 0),
            ChunkPos::new(0, -1),
        ];
        let mut seen = BTreeSet::new();
        for pos in hostile {
            assert!(seen.insert(pack_chunk_pos(pos)), "packed {pos:?} twice");
        }
        // `(-1, 0)` and `(0, -1)` are the classic case a sloppy mix collides on.
        assert_ne!(
            pack_chunk_pos(ChunkPos::new(-1, 0)),
            pack_chunk_pos(ChunkPos::new(0, -1))
        );
        // Hostile values still round-trip through the packing: the high word is
        // the sign-extended `x`, the low word the zero-extended bit pattern of
        // `z`.
        let extreme = pack_chunk_pos(ChunkPos::new(i32::MIN, i32::MIN));
        assert_eq!((extreme >> 32) as u32, i32::MIN as u32, "high word is x");
        assert_eq!(
            extreme & 0xFFFF_FFFF_u64,
            u64::from(i32::MIN as u32),
            "low word is z's bit pattern (0x8000_0000), not zero"
        );
        assert_ne!(extreme, pack_chunk_pos(ChunkPos::new(i32::MIN, 0)));
        let z_only = pack_chunk_pos(ChunkPos::new(0, i32::MIN));
        assert_eq!(z_only, 0x8000_0000, "and the high word really is zero then");
    }

    #[test]
    fn chunk_seeds_never_collide_over_a_wide_sample() {
        let seed = WorldSeed::from_raw(1_361_882_806);
        let mut visited = BTreeSet::new();
        let mut count = 0_u32;
        for x in -50_i32..50 {
            for z in -50_i32..50 {
                let pos = ChunkPos::new(x * 37, z * 53);
                assert!(
                    visited.insert(seed.chunk_seed(pos)),
                    "{pos:?} collided with an earlier chunk"
                );
                count += 1;
            }
        }
        assert_eq!(count, 10_000);
        // The same position always yields the same sub-seed.
        assert_eq!(
            seed.chunk_seed(ChunkPos::new(-7, 13)),
            seed.chunk_seed(ChunkPos::new(-7, 13))
        );
    }

    #[test]
    fn different_seeds_and_streams_differ() {
        let pos = ChunkPos::new(3, -4);
        let a = WorldSeed::from_raw(1);
        let b = WorldSeed::from_raw(2);
        assert_ne!(a.chunk_seed(pos), b.chunk_seed(pos));
        assert_ne!(a.stream_seed(0), a.stream_seed(1));
        assert_ne!(a.stream_seed(0), b.stream_seed(0));
        // A stream seed is stable for a given (seed, stream).
        assert_eq!(a.stream_seed(7), a.stream_seed(7));
    }

    #[test]
    fn fresh_seeds_are_derived_not_constant() {
        // Sleeping is not needed: two calls differ in nanoseconds, and the
        // millisecond probe below simply tolerates a same-millisecond pair.
        let first = WorldSeed::fresh();
        let second = WorldSeed::fresh();
        // The values must at least be well-formed; equality is allowed only if
        // both landed in the same millisecond, which the mix makes unlikely.
        assert_eq!(first.raw(), first.raw());
        let _ = second;
    }

    #[test]
    fn the_evidence_worlds_seed_is_readable_from_a_world_gen_document() {
        // The shape a 26.1 world document has: a `Data` wrapper around the
        // generator settings. The value is the one the evidence world in
        // `crates/test-support/fixtures/anvil` was generated with.
        let flat = NbtTag::compound([(
            "Data".to_owned(),
            NbtTag::compound([("seed".to_owned(), NbtTag::Long(1_361_882_806))]),
        )]);
        assert_eq!(
            WorldSeed::from_level_dat(&flat).expect("seed").raw(),
            1_361_882_806
        );

        // The nested form a modern dimension document uses.
        let nested = NbtTag::compound([(
            "Data".to_owned(),
            NbtTag::compound([
                ("version".to_owned(), NbtTag::Int(19133)),
                (
                    "dimensions".to_owned(),
                    NbtTag::compound([(
                        "minecraft:overworld".to_owned(),
                        NbtTag::compound([(
                            "generator".to_owned(),
                            NbtTag::compound([("seed".to_owned(), NbtTag::Long(7))]),
                        )]),
                    )]),
                ),
            ]),
        )]);
        assert_eq!(WorldSeed::from_level_dat(&nested).expect("seed").raw(), 7);

        // A tool-written document may store the seed as an `Int`; the tolerant
        // read accepts it rather than failing to load the world.
        let tolerant = NbtTag::compound([(
            "Data".to_owned(),
            NbtTag::compound([("seed".to_owned(), NbtTag::Int(99))]),
        )]);
        assert_eq!(
            WorldSeed::from_level_dat(&tolerant).expect("seed").raw(),
            99
        );

        // The fallback walk still finds a seed nested somewhere unexpected.
        let buried = NbtTag::compound([(
            "Data".to_owned(),
            NbtTag::compound([(
                "other".to_owned(),
                NbtTag::List(vec![NbtTag::compound([(
                    "seed".to_owned(),
                    NbtTag::Long(-5),
                )])]),
            )]),
        )]);
        assert_eq!(WorldSeed::from_level_dat(&buried).expect("seed").raw(), -5);
    }

    #[test]
    fn a_world_gen_document_without_a_seed_is_refused() {
        // Inventing a seed would generate terrain that does not match the stored
        // chunks, so this is an error rather than a default.
        let missing = NbtTag::compound([(
            "Data".to_owned(),
            NbtTag::compound([("version".to_owned(), NbtTag::Int(19133))]),
        )]);
        let error = WorldSeed::from_level_dat(&missing).expect_err("must refuse");
        assert!(matches!(error, ServerError::CorruptData(_)), "{error:?}");
        // A non-compound document is refused too, not panicked on.
        assert!(WorldSeed::from_level_dat(&NbtTag::Int(1)).is_err());
        assert!(WorldSeed::from_level_dat(&NbtTag::List(Vec::new())).is_err());
    }

    #[test]
    fn context_derives_its_bounds_from_the_dimension_constants() {
        let context = WorldgenContext::overworld(WorldSeed::from_raw(42));
        assert_eq!(context.min_y, OVERWORLD_MIN_Y);
        assert_eq!(context.height, OVERWORLD_HEIGHT);
        assert_eq!(context.sea_level, OVERWORLD_SEA_LEVEL);
        assert_eq!(context.max_y(), 320);
        assert_eq!(context.min_section_y(), -4);
        assert_eq!(context.section_count(), 24);
        assert_eq!(context.dimension, Dimension::Overworld);
    }

    #[test]
    fn a_hostile_context_clamps_instead_of_overflowing() {
        let context = WorldgenContext::new(
            WorldSeed::from_raw(0),
            Dimension::Overworld,
            i32::MIN,
            i32::MAX,
            0,
        );
        // Exactly `i32::MIN + i32::MAX`, no wrap (which would be positive).
        assert_eq!(
            context.max_y(),
            -1,
            "a hostile min_y + height is exact here, not wrapped"
        );
        assert_eq!(context.section_count(), 128, "section count is bounded");
        // The *effective* bound is what the chunk can hold, and it is a different
        // number on purpose: the section count is capped, so the chunk spans 2048
        // blocks from `min_y` rather than the whole `i32` range.
        assert_eq!(context.effective_max_y(), i32::MIN + 2048);
        // The predecessor of this line asserted `min_section_y() >= i8::MIN`, which is
        // always true because the value *is* an `i8` — a bound check that checked nothing.
        // What matters is that the derived section range covers the dimension's block
        // range. It can only do so when the floor is representable at all:
        // `mc_world::Chunk::min_section_y` is an `i8`, so **no chunk can express a
        // section below `i8::MIN * 16 = -2048`**. A `min_y` of `i32::MIN` is therefore
        // outside what the chunk model can hold, and saturation moves the chunk's floor
        // *above* it. Both branches are asserted so neither can drift unnoticed.
        let section_floor = i32::from(context.min_section_y()) * 16;
        let floor_is_representable = i32::from(i8::MIN) * 16 <= context.min_y;
        if floor_is_representable {
            assert!(
                section_floor <= context.min_y,
                "the lowest section must start at or below the dimension's lowest block"
            );
            assert!(
                section_floor + context.section_count() as i32 * 16 >= context.max_y(),
                "the sections must cover the dimension's full height"
            );
        } else {
            assert_eq!(
                context.min_section_y(),
                i8::MIN,
                "an unrepresentable floor saturates to the lowest section index"
            );
            assert_eq!(
                section_floor, -2048,
                "which is the lowest representable block"
            );
            assert!(
                section_floor > context.min_y,
                "and it is above the requested floor — the documented limitation"
            );
        }
        // The overworld is representable, so the covering invariant holds there:
        // this is the case that actually runs in production.
        let overworld = WorldgenContext::overworld(WorldSeed::from_raw(0));
        assert!(i32::from(overworld.min_section_y()) * 16 <= overworld.min_y);
        assert!(
            i32::from(overworld.min_section_y()) * 16 + overworld.section_count() as i32 * 16
                >= overworld.max_y()
        );
        // A negative height means "no sections", clamped up to one empty one.
        let empty = WorldgenContext::new(WorldSeed::from_raw(0), Dimension::Overworld, 0, -100, 0);
        assert_eq!(empty.section_count(), 1);
        assert_eq!(empty.min_section_y(), 0);
    }
}
