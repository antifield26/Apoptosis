//! Existing-world-first: a stored chunk always wins over a generated one
//! (P07-17).
//!
//! ## Why this module is first, not last
//!
//! PHASE-07: *"Prioritize loading existing worlds before perfecting every
//! generation feature, then expand toward 26.1.2 parity."* This module is that
//! sentence made executable. It is the reason the crate exists:
//!
//! > **A stored chunk is returned unchanged; a generator is consulted only when
//! > nothing is stored.**
//!
//! `Game::load_or_create_chunk` (`crates/server/src/game.rs`) already has the
//! right *shape* — it tries disk, then falls back to a placeholder, and it marks
//! the fallback clean so it is never written back over real terrain. What it
//! lacks is the second fallback: when nothing is stored, there is no generator to
//! ask. [`ChunkProvider`] is that missing layer, with the data-loss rule built in:
//!
//! - **stored** → return it exactly as the caller supplied it, and never touch
//!   the generator (asserted by `tests/existing_world_first.rs`, which counts
//!   generator calls through a spy lookup);
//! - **missing** → ask the generator;
//! - **a lookup that fails** → return the error. It is *not* silently treated as
//!   "nothing is stored", because that substitution is what once let a
//!   placeholder be written over real terrain. The caller decides whether to
//!   degrade.
//!
//! ## Determinism and ownership
//!
//! [`ChunkProvider::chunk_at`] takes `&mut self` and keeps **no cache**: the
//! caller owns the world map (`mc_world::World`), so a chunk lives in exactly one
//! place. The only mutable state is the statistics counter ([`ProviderStats`]),
//! which is a diagnostic and never feeds generation, so two providers over the
//! same world produce identical chunks whatever order they are asked in
//! (AGENTS.md §3.6).
//!
//! ## The registry question
//!
//! Generation needs a [`BlockRegistry`] because every block id comes from it.
//! [`ChunkProvider::chunk_at`] takes it as a parameter rather than caching its own
//! copy: the server already owns exactly one registry, and a second copy inside a
//! provider would be a second source of truth for block ids. It is consulted
//! **only** on the generation path, so a stored chunk is returned even when the
//! caller's table would not accept it.

use crate::seed::WorldgenContext;
use crate::terrain::{ChunkGenerator, GenerationError};
use mc_registry::BlockRegistry;
use mc_world::{Chunk, ChunkPos};

/// A chunk read from somewhere the provider does not own (disk, a cache, a test).
///
/// A trait rather than a direct `mc-persistence` call because this crate must not
/// own the storage schema: the server already has a `WorldService` that knows
/// about regions and `level.dat`, and duplicating that knowledge here would be a
/// second source of truth for what is on disk.
pub trait ChunkLookup {
    /// Read a stored chunk.
    ///
    /// `Ok(None)` means **"nothing is stored at this position"**, which is a
    /// different answer from an error and is what licenses generation. An error
    /// means the caller *cannot know*, and must not be turned into "generate over
    /// it".
    ///
    /// # Errors
    ///
    /// Whatever the underlying storage reports; the provider forwards it.
    fn stored_chunk(&mut self, pos: ChunkPos) -> Result<Option<Chunk>, String>;
}

/// A [`ChunkLookup`] built from a closure.
///
/// Exists so a caller can pass a small function (`|pos| storage.read(pos)`)
/// without defining a type; the server's hook is what will wrap.
pub struct FnLookup<F> {
    read: F,
}

impl<F> FnLookup<F> {
    /// Wrap a closure.
    pub const fn new(read: F) -> Self {
        Self { read }
    }
}

impl<F> ChunkLookup for FnLookup<F>
where
    F: FnMut(ChunkPos) -> Result<Option<Chunk>, String>,
{
    fn stored_chunk(&mut self, pos: ChunkPos) -> Result<Option<Chunk>, String> {
        (self.read)(pos)
    }
}

/// A lookup that never has anything stored.
///
/// Useful for a brand-new world and for tests that want generation only. It is
/// **not** the same as an unreadable region file: it answers `Ok(None)` ("I know
/// there is nothing here"), which licenses generation, whereas a failed read
/// answers `Err`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStorage;

impl ChunkLookup for NoStorage {
    fn stored_chunk(&mut self, _pos: ChunkPos) -> Result<Option<Chunk>, String> {
        Ok(None)
    }
}

/// A lookup that answers from a fixed, ordered map of chunks.
///
/// Present for tests and for a future in-memory cache, and deliberately backed by
/// a `BTreeMap`: iteration order is deterministic, per AGENTS.md §3.6.
#[derive(Debug, Clone, Default)]
pub struct MapLookup {
    chunks: std::collections::BTreeMap<ChunkPos, Chunk>,
}

impl MapLookup {
    /// An empty lookup.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a stored chunk.
    pub fn insert(&mut self, chunk: Chunk) {
        self.chunks.insert(chunk.pos, chunk);
    }

    /// How many chunks are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Positions in deterministic order.
    pub fn positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }
}

impl ChunkLookup for MapLookup {
    fn stored_chunk(&mut self, pos: ChunkPos) -> Result<Option<Chunk>, String> {
        Ok(self.chunks.get(&pos).cloned())
    }
}

/// What one provider call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderStats {
    /// Calls that returned a stored chunk.
    pub stored: u64,
    /// Calls that generated a chunk because nothing was stored.
    pub generated: u64,
    /// Calls whose lookup failed and were reported as an error.
    pub failed: u64,
}

impl ProviderStats {
    /// Total calls.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.stored
            .saturating_add(self.generated)
            .saturating_add(self.failed)
    }

    /// Whether the generator was consulted at all.
    ///
    /// This is the counter the existing-world-first rule is asserted with: after
    /// reading only stored chunks it must be `false`.
    #[must_use]
    pub const fn generator_was_consulted(&self) -> bool {
        self.generated > 0
    }
}

/// A chunk plus where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredChunk {
    /// The chunk itself.
    pub chunk: Chunk,
    /// `true` when it came from the lookup, `false` when it was generated.
    pub from_storage: bool,
}

/// A provider that prefers stored chunks and falls back to a generator.
///
/// See the module docs for the ordering rule and why it matters.
pub struct ChunkProvider<G: ChunkGenerator, L: ChunkLookup> {
    generator: G,
    lookup: L,
    stats: ProviderStats,
}

impl<G: ChunkGenerator, L: ChunkLookup> ChunkProvider<G, L> {
    /// Build a provider from a generator and a lookup.
    #[must_use]
    pub const fn new(generator: G, lookup: L) -> Self {
        Self {
            generator,
            lookup,
            stats: ProviderStats {
                stored: 0,
                generated: 0,
                failed: 0,
            },
        }
    }

    /// The generator (for its context, palette and biome field).
    #[must_use]
    pub const fn generator(&self) -> &G {
        &self.generator
    }

    /// The lookup.
    #[must_use]
    pub const fn lookup(&self) -> &L {
        &self.lookup
    }

    /// The lookup, mutably (a caller may need to point it at another region).
    pub fn lookup_mut(&mut self) -> &mut L {
        &mut self.lookup
    }

    /// What this provider has done so far.
    #[must_use]
    pub const fn stats(&self) -> ProviderStats {
        self.stats
    }

    /// The generation context.
    #[must_use]
    pub fn context(&self) -> &WorldgenContext {
        self.generator.context()
    }

    /// The chunk at a position: **stored when one is stored, generated only when
    /// nothing is**.
    ///
    /// A stored chunk is returned exactly as the lookup produced it — not
    /// re-generated, not decorated, not marked dirty, not re-validated. A stored
    /// chunk is somebody else's data, and the moment this function "improves" it,
    /// the existing world has stopped being the source of truth. `registry` is
    /// consulted **only** on the generation path.
    ///
    /// # Errors
    ///
    /// - [`ProviderError::Lookup`] when the storage read fails, so the caller can
    ///   choose to refuse rather than generate over unknown ground;
    /// - [`ProviderError::Generate`] when the generator cannot produce the chunk
    ///   (neither generator in this crate does, for a context accepted at
    ///   construction).
    pub fn chunk_at(
        &mut self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Chunk, ProviderError> {
        match self.lookup.stored_chunk(pos) {
            Ok(Some(stored)) => {
                self.stats.stored += 1;
                Ok(stored)
            }
            Ok(None) => match self.generator.generate_chunk(pos, registry) {
                Ok(chunk) => {
                    self.stats.generated += 1;
                    Ok(chunk)
                }
                Err(error) => {
                    self.stats.failed += 1;
                    Err(ProviderError::Generate { pos, error })
                }
            },
            Err(message) => {
                self.stats.failed += 1;
                Err(ProviderError::Lookup { pos, message })
            }
        }
    }

    /// As [`ChunkProvider::chunk_at`], but says where the chunk came from.
    ///
    /// # Errors
    ///
    /// As for [`ChunkProvider::chunk_at`].
    pub fn chunk_at_with_source(
        &mut self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<StoredChunk, ProviderError> {
        let before = self.stats.stored;
        let chunk = self.chunk_at(pos, registry)?;
        // `stored` only advances on the storage path, so this comparison is the
        // provenance without a second lookup.
        let from_storage = self.stats.stored > before;
        Ok(StoredChunk {
            chunk,
            from_storage,
        })
    }

    /// As [`ChunkProvider::chunk_at`], but marks a **generated** chunk dirty.
    ///
    /// The dirty flag is the save system's "this was not read from disk and must
    /// be written back". A stored chunk keeps whatever flag it arrived with, so a
    /// chunk that was loaded and not edited is never rewritten.
    ///
    /// # Errors
    ///
    /// As for [`ChunkProvider::chunk_at`].
    pub fn chunk_at_marked(
        &mut self,
        pos: ChunkPos,
        registry: &BlockRegistry,
    ) -> Result<Chunk, ProviderError> {
        let before = self.stats.stored;
        let mut chunk = self.chunk_at(pos, registry)?;
        if self.stats.stored == before {
            chunk.dirty = true;
        }
        Ok(chunk)
    }
}

/// A provider call that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The storage lookup failed. **Never** treated as "nothing stored".
    Lookup {
        /// The position asked for.
        pos: ChunkPos,
        /// The storage layer's message.
        message: String,
    },
    /// The generator could not produce the chunk.
    Generate {
        /// The position asked for.
        pos: ChunkPos,
        /// The generator's error.
        error: GenerationError,
    },
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lookup { pos, message } => write!(
                formatter,
                "chunk ({}, {}) could not be read from storage: {message}",
                pos.x, pos.z
            ),
            Self::Generate { pos, error } => write!(
                formatter,
                "chunk ({}, {}) could not be generated: {error}",
                pos.x, pos.z
            ),
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{ChunkLookup, ChunkProvider, MapLookup, NoStorage, ProviderError, ProviderStats};
    use crate::seed::{WorldSeed, WorldgenContext};
    use crate::terrain::{ChunkGenerator, FlatGenerator, TerrainGenerator};
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::{Chunk, ChunkPos};

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    fn terrain(blocks: &BlockRegistry) -> TerrainGenerator {
        TerrainGenerator::new(WorldgenContext::overworld(WorldSeed::from_raw(7)), blocks)
            .expect("generator")
    }

    /// A lookup that fails, to prove a failed read is never "nothing stored".
    struct FailingLookup;

    impl ChunkLookup for FailingLookup {
        fn stored_chunk(&mut self, _pos: ChunkPos) -> Result<Option<Chunk>, String> {
            Err("region file is unreadable".to_owned())
        }
    }

    #[test]
    fn a_stored_chunk_wins_and_the_generator_is_not_consulted() {
        let blocks = registry();
        let pos = ChunkPos::new(2, -3);
        // A stored chunk that is deliberately *not* what the generator would
        // produce: all air, with a marker block the generator never writes there.
        let mut stored = Chunk::air(pos, -4, 24, &blocks);
        let marker = blocks.default_state("minecraft:bedrock").expect("bedrock");
        stored
            .set_block(pos.x * 16 + 5, 200, pos.z * 16 + 5, marker, &blocks)
            .expect("inside the chunk");
        let mut lookup = MapLookup::new();
        lookup.insert(stored.clone());
        assert_eq!(lookup.len(), 1);
        assert!(!lookup.is_empty());
        assert_eq!(lookup.positions().count(), 1);

        let mut provider = ChunkProvider::new(terrain(&blocks), lookup);
        let returned = provider.chunk_at(pos, &blocks).expect("stored chunk");
        assert_eq!(returned, stored, "returned exactly as stored");
        assert_eq!(provider.stats().stored, 1);
        assert_eq!(provider.stats().generated, 0);
        assert!(
            !provider.stats().generator_was_consulted(),
            "the generator must not run when a chunk is stored"
        );
        // The marker proves nothing re-generated it.
        assert_eq!(
            returned.get_block(pos.x * 16 + 5, 200, pos.z * 16 + 5),
            marker
        );
        assert_eq!(provider.context().seed, WorldSeed::from_raw(7));
    }

    #[test]
    fn a_missing_chunk_is_generated() {
        let blocks = registry();
        let pos = ChunkPos::new(-1, 4);
        let mut provider = ChunkProvider::new(terrain(&blocks), NoStorage);
        let generated = provider.chunk_at(pos, &blocks).expect("generated");
        assert_eq!(generated.pos, pos);
        assert_eq!(provider.stats().generated, 1);
        assert_eq!(provider.stats().stored, 0);
        assert!(provider.stats().generator_was_consulted());
        assert_eq!(provider.stats().total(), 1);
        // It is real terrain: bedrock at the floor, solid rock above it.
        let floor = blocks.default_state("minecraft:bedrock").expect("bedrock");
        assert_eq!(generated.get_block(pos.x * 16, -64, pos.z * 16), floor);
        // Asking twice generates twice (no cache) and gives identical chunks.
        let again = provider.chunk_at(pos, &blocks).expect("generated");
        assert_eq!(generated, again);
        assert_eq!(provider.stats().generated, 2);
    }

    #[test]
    fn a_failed_lookup_is_an_error_not_a_generation() {
        let blocks = registry();
        let pos = ChunkPos::new(0, 0);
        let mut provider = ChunkProvider::new(terrain(&blocks), FailingLookup);
        let error = provider.chunk_at(pos, &blocks).expect_err("must refuse");
        match &error {
            ProviderError::Lookup { pos: at, message } => {
                assert_eq!(*at, pos);
                assert!(message.contains("unreadable"));
            }
            // The other variant is enumerated rather than wildcarded, so adding a
            // `ProviderError` variant later breaks this test instead of silently
            // passing it.
            ProviderError::Generate { error, .. } => {
                panic!("a failing lookup must not reach the generator (got {error})")
            }
        }
        assert_eq!(
            provider.stats().generated,
            0,
            "a failed read must never license generation"
        );
        assert_eq!(provider.stats().failed, 1);
        assert!(!provider.stats().generator_was_consulted());
        assert!(error.to_string().contains("could not be read"));
    }

    #[test]
    fn provenance_and_the_dirty_flag_follow_the_source() {
        let blocks = registry();
        let pos = ChunkPos::new(1, 1);
        let mut lookup = MapLookup::new();
        let mut stored = Chunk::air(pos, -4, 24, &blocks);
        stored.mark_clean();
        lookup.insert(stored.clone());
        let mut provider = ChunkProvider::new(terrain(&blocks), lookup);

        let from_storage = provider.chunk_at_with_source(pos, &blocks).expect("stored");
        assert!(from_storage.from_storage);
        assert_eq!(from_storage.chunk, stored);
        assert!(!from_storage.chunk.dirty, "a stored chunk keeps its flag");

        let elsewhere = provider
            .chunk_at_with_source(ChunkPos::new(9, 9), &blocks)
            .expect("generated");
        assert!(!elsewhere.from_storage);

        // `chunk_at_marked` dirties a generated chunk and leaves a stored one.
        let marked = provider
            .chunk_at_marked(ChunkPos::new(9, 9), &blocks)
            .expect("generated");
        assert!(marked.dirty);
        let untouched = provider.chunk_at_marked(pos, &blocks).expect("stored");
        assert!(!untouched.dirty);
    }

    #[test]
    fn a_flat_generator_works_as_the_fallback_too() {
        let blocks = registry();
        let flat =
            FlatGenerator::superflat(WorldgenContext::overworld(WorldSeed::from_raw(5)), &blocks)
                .expect("flat");
        let mut provider = ChunkProvider::new(flat, NoStorage);
        let chunk = provider
            .chunk_at(ChunkPos::new(0, 0), &blocks)
            .expect("chunk");
        let grass = blocks
            .default_state("minecraft:grass_block")
            .expect("grass");
        assert_eq!(chunk.get_block(0, -60, 0), grass);
        assert_eq!(provider.generator().biome(), crate::biome::Biome::Plains);
        // The lookup can be reached mutably while the provider lives.
        let _ = provider.lookup_mut();
        assert_eq!(provider.generator().context().min_y, -64);
    }

    #[test]
    fn statistics_add_up_and_default_to_empty() {
        let stats = ProviderStats::default();
        assert_eq!(stats.total(), 0);
        assert!(!stats.generator_was_consulted());
        let blocks = registry();
        let mut provider = ChunkProvider::new(terrain(&blocks), FailingLookup);
        let _ = provider.chunk_at(ChunkPos::new(0, 0), &blocks);
        let _ = provider.chunk_at(ChunkPos::new(1, 0), &blocks);
        assert_eq!(provider.stats().total(), 2);
        assert_eq!(provider.stats().failed, 2);
        assert_eq!(
            ProviderStats {
                stored: 1,
                generated: 2,
                failed: 3
            }
            .total(),
            6
        );
    }
}
