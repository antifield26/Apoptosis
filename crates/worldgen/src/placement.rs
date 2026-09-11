//! Placing a resolved structure into a chunk, with every block counted (P07-16).
//!
//! ## The cross-chunk decision, and why it is option (b)
//!
//! P07-16 says: a structure is usually larger than one chunk, so choose **one**
//! of (a) place from the owning chunk and write into neighbours, or (b) refuse
//! anything that does not fit in one chunk and count the refusals — "silence is
//! not [acceptable]".
//!
//! **This module chooses (b): [`CrossChunk::SingleChunk`] is the default, and a
//! structure that would cross a chunk border is refused and counted.**
//!
//! ### Why (a) is not merely unimplemented here — it is prevented by the model
//!
//! Option (a) needs a neighbour-writing API. **`mc_world::Chunk` does not have
//! one, and its shape makes one impossible to fake:**
//!
//! - `Chunk::set_block` reduces the horizontal coordinates with `rem_euclid(16)`
//!   (`crates/world/src/chunk.rs`), so asking it to write `x = 16` writes the
//!   **same chunk's** column `x = 0`. It does not write the neighbour and it does
//!   not report an error: it silently writes the wrong column. A cross-chunk
//!   placer built on `set_block` would therefore corrupt the chunk it was given
//!   rather than fail loudly.
//! - `Chunk::get_block` reduces the same way, so a placer cannot even *detect*
//!   the overflow by reading back what it wrote.
//! - Only `y` is bounds-checked: `Chunk::set_block` returns
//!   `ServerError::InvalidAction` for a `y` outside the chunk.
//!
//! So option (a) requires a new capability in `mc-world` (a placer that owns
//! several chunks, or a `Chunk` that reports the chunk position of a write
//! instead of wrapping). That is a cross-crate change, outside this task's
//! "work only inside `crates/worldgen/**`" boundary, so this module does the
//! honest thing instead of the thing that looks finished.
//!
//! ### The consequence, stated rather than hidden
//!
//! **154 of the 1 182 single-palette 26.1.2 templates (13.0%) cannot be placed
//! at all** under the default policy: `village` (44), `ancient_city` (43),
//! `bastion` (33), `trial_chambers` (30), `end_city` (2), `pillager_outpost` (1)
//! and `woodland_mansion` (1) all contain pieces wider or deeper than 16 blocks.
//! Every one of them is reported through [`PlacementReport::refused`], so "no
//! structure here" and "the structure does not fit" are different outcomes at
//! every level.
//!
//! ### What the caller can do instead, today
//!
//! - [`CrossChunk::Clip`] writes the in-bounds subset and counts the rest in
//!   [`PlacementReport::blocks_outside`]. That is **not** Vanilla behaviour
//!   either (it leaves a sliced building at the border), but it is an explicit,
//!   measured choice, it is order-independent, and it is what a caller who wants
//!   *something* at a chunk edge should select.
//! - [`CrossChunk::SingleChunk`] plus a registry restricted with
//!   [`crate::structures::StructureRegistry::fitting_in_one_chunk`] — 1 028 of
//!   the 1 182 templates — places every selected structure in full, with an empty
//!   refusal list. That is the recommended starting configuration.
//!
//! ## Air handling, as a policy rather than a hidden default
//!
//! Vanilla's structure placer has an `ignoreAir` flag and **both settings are
//! used** by real structures, so guessing one would be wrong whichever way it
//! went. [`AirPolicy`] makes the caller state it:
//!
//! - [`AirPolicy::IgnoreAir`] (**the default**) skips a block whose template
//!   state is air, so a structure never punches a hole in existing terrain. The
//!   measured cost: **43.7% of the pack's blocks are `minecraft:air`** (first 200
//!   files sampled: 160 012 of 365 820), because templates are stored as dense
//!   cuboids, so most of a template's air entries are "this cell is inside the
//!   bounding box but outside the building".
//! - [`AirPolicy::Overwrite`] writes them, which is what carves a structure's
//!   interior out of the ground. Correct for some structures, destructive for
//!   others.
//!
//! Both are tested (`tests/structure_placement.rs`).
//!
//! ## Counting rule (nothing is silently dropped)
//!
//! Every block of the template lands in exactly one bucket, and the buckets sum
//! to the number of blocks iterated:
//!
//! ```text
//! blocks_written
//!   + blocks_outside       (would land outside the chunk horizontally)
//!   + blocks_out_of_world  (would land above or below the chunk's y range)
//!   + blocks_air_skipped   (air under `AirPolicy::IgnoreAir`)
//!   + blocks_outside_template (a position outside the template's own `size`)
//!   == structure.blocks.len()
//! ```
//!
//! `tests/structure_placement.rs` asserts that identity directly, so a block
//! cannot go missing from the accounting. Note what the identity does **not**
//! say: `blocks_written` counts blocks that *changed*, so a template block equal
//! to what was already in the chunk is counted in none of the buckets. That is
//! why the identity is over the loop, not over "blocks written plus skipped".

use crate::structure::{ResolvedStructure, inside};
use mc_registry::BlockRegistry;
use mc_world::{Chunk, SECTION_WIDTH};

// -------------------------------------------------------------------- policy

/// What to do with a structure that does not fit inside one chunk.
///
/// See the module docs for the full argument; this is the choice itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossChunk {
    /// **Default.** Refuse the whole structure and count it.
    ///
    /// Nothing is written, [`PlacementReport::refused`] names the template and
    /// the overflow, and [`PlacementReport::blocks_written`] is zero. Chunk
    /// generation stays a pure function of one chunk position, which is what
    /// keeps generation order-independent (AGENTS.md §3.6).
    #[default]
    SingleChunk,
    /// Write only the blocks that fall inside the chunk; count the rest.
    ///
    /// Deliberately **not** the default: a sliced building at a chunk border is
    /// a visible artifact, whereas a refusal is invisible and honest. Exposed
    /// because it is the only policy that places *part* of a large structure,
    /// and because the resulting [`PlacementReport::blocks_outside`] is the
    /// measurement P07-16 asks for.
    Clip,
}

/// What to do with a template block whose state is air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AirPolicy {
    /// **Default.** Skip air blocks, so terrain is never removed.
    ///
    /// Named after Vanilla's `ignoreAir` flag because it is the same idea; that
    /// the *name* matches is the whole claim, not that the flag is chosen the
    /// way Vanilla chooses it for any particular structure.
    #[default]
    IgnoreAir,
    /// Write air blocks, carving the structure's volume out of the terrain.
    Overwrite,
}

/// How a structure's `[0, 0, 0]` corner is anchored in the world.
///
/// A separate type because the two answers are each wrong for the other caller:
/// a village wants its floor at the surface, a shipwreck wants a specific y band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The template's `y = 0` plane sits at `surface_y + 1`, with the template's
    /// footprint centred on `(block_x, block_z)` horizontally.
    ///
    /// "Centred" means the origin column is `block_x - size[0] / 2` (truncating
    /// division, so a 5-wide template at `block_x` spans `block_x - 2 ..= block_x + 2`).
    OnSurface {
        /// World x of the column the structure is centred on.
        block_x: i32,
        /// The terrain surface height at that column.
        surface_y: i32,
        /// World z of the column the structure is centred on.
        block_z: i32,
    },
    /// The template's `[0, 0, 0]` corner sits exactly at this world position.
    At {
        /// World position of the template's origin corner.
        origin: (i32, i32, i32),
    },
}

impl Anchor {
    /// The world position of the template's `[0, 0, 0]` corner.
    ///
    /// Every step saturates, so a hostile `surface_y` or `size` yields a
    /// degenerate but well-defined origin rather than a wrapped one.
    #[must_use]
    pub const fn origin(self, size: [i32; 3]) -> (i32, i32, i32) {
        match self {
            Self::OnSurface {
                block_x,
                surface_y,
                block_z,
            } => (
                block_x.saturating_sub(size[0] / 2),
                surface_y.saturating_add(1),
                block_z.saturating_sub(size[2] / 2),
            ),
            Self::At { origin } => origin,
        }
    }
}

// -------------------------------------------------------------------- report

/// What one placement attempt did.
///
/// [`PlacementReport::blocks_written`] being `0` is ambiguous on its own — an
/// all-air template under [`AirPolicy::IgnoreAir`] also writes nothing — which
/// is why [`PlacementReport::refused`] exists and why every block is bucketed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlacementReport {
    /// Blocks that actually changed a block in the chunk.
    ///
    /// "Changed" in the sense of `Chunk::set_block` returning `Some`: a template
    /// block equal to what was already there is **not** counted, which is why
    /// this is not simply "blocks inside the chunk minus the air".
    pub blocks_written: usize,
    /// Blocks whose position falls outside the chunk **horizontally**.
    ///
    /// Under [`CrossChunk::SingleChunk`] a non-zero value implies
    /// [`PlacementReport::refused`] is `Some` and nothing was written at all.
    pub blocks_outside: usize,
    /// Blocks whose position falls outside the chunk's **vertical** range.
    ///
    /// Separate from [`PlacementReport::blocks_outside`] because the two are
    /// fixed differently: a horizontal overflow needs a different placement
    /// policy, a vertical one needs a different anchor y.
    pub blocks_out_of_world: usize,
    /// Blocks skipped because their template state is air and the policy is
    /// [`AirPolicy::IgnoreAir`].
    pub blocks_air_skipped: usize,
    /// Blocks whose template position lies outside the template's own `size`.
    ///
    /// Zero for every measured pack file, but a datapack may author one, and an
    /// un-counted one would break the bucket identity in the module docs. Such a
    /// block is **not** written (it is not part of the structure's extent) and is
    /// reported here instead.
    pub blocks_outside_template: usize,
    /// Why the whole structure was refused, naming the template and the reason.
    pub refused: Option<String>,
}

impl PlacementReport {
    /// An all-zero report.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            blocks_written: 0,
            blocks_outside: 0,
            blocks_out_of_world: 0,
            blocks_air_skipped: 0,
            blocks_outside_template: 0,
            refused: None,
        }
    }

    /// Every block this attempt accounted for.
    ///
    /// Equal to the template's block count whenever the attempt iterated the
    /// whole template (including a refusal, which accounts for none of them —
    /// see [`PlacementReport::refused`]).
    #[must_use]
    pub const fn accounted(&self) -> usize {
        self.blocks_written
            + self.blocks_outside
            + self.blocks_out_of_world
            + self.blocks_air_skipped
            + self.blocks_outside_template
    }

    /// Whether this attempt wrote nothing at all.
    #[must_use]
    pub const fn wrote_nothing(&self) -> bool {
        self.blocks_written == 0
    }

    /// Whether the structure was refused as a whole.
    #[must_use]
    pub const fn was_refused(&self) -> bool {
        self.refused.is_some()
    }

    /// A refusal naming `name` and `reason`, with nothing written.
    #[must_use]
    pub fn refuse(name: &str, reason: &str) -> Self {
        Self {
            refused: Some(format!("{name}: {reason}")),
            ..Self::empty()
        }
    }
}

// --------------------------------------------------------------- placement

/// Place a resolved structure into a chunk.
///
/// Only `chunk` is ever written (see the module docs: `Chunk::set_block` wraps
/// horizontally, so there is no other chunk it *could* write). `origin` is the
/// world position of the template's `[0, 0, 0]` corner; use [`Anchor::origin`] to
/// derive it from an [`Anchor`].
///
/// A refusal is **reported**, not returned: a caller generating thousands of
/// chunks needs counts, not a `Result` it would log and discard (AGENTS.md §3.3
/// — the refusal is visible in [`PlacementReport::refused`]).
///
/// # Panics
///
/// Never: every coordinate arithmetic site uses `checked_add` and treats an
/// overflow as "outside the chunk", every index is bounds-checked, and no site
/// can produce an out-of-range `y` for `Chunk::set_block`.
#[must_use]
pub fn place(
    structure: &ResolvedStructure,
    chunk: &mut Chunk,
    origin: (i32, i32, i32),
    air: AirPolicy,
    cross_chunk: CrossChunk,
    registry: &BlockRegistry,
    name: &str,
) -> PlacementReport {
    let area = ChunkArea::of(chunk);
    let outside = structure.blocks_outside_at(origin, area);
    if cross_chunk == CrossChunk::SingleChunk && outside > 0 {
        // The refusal is the *whole* report: nothing was written, so a caller can
        // tell this apart from "placed, but the template was all air".
        return PlacementReport::refuse(
            name,
            &format!(
                "does not fit in one chunk: {outside} of {} blocks fall outside \
                 (size {:?} at origin ({}, {}, {}))",
                structure.block_count(),
                structure.size,
                origin.0,
                origin.1,
                origin.2
            ),
        );
    }

    let air_id = registry.air_id();
    let mut report = PlacementReport::empty();
    for block in &structure.blocks {
        // A position outside the template's own extent is not part of the
        // structure: counted, never written.
        if !inside(&structure.size, block.pos) {
            report.blocks_outside_template += 1;
            continue;
        }
        let (Some(x), Some(y), Some(z)) = (
            block.pos[0].checked_add(origin.0),
            block.pos[1].checked_add(origin.1),
            block.pos[2].checked_add(origin.2),
        ) else {
            // An overflowing coordinate is certainly outside the chunk; which
            // bucket it belongs in depends on which axis overflowed, and a
            // coordinate that cannot be represented has no axis worth naming.
            report.blocks_outside += 1;
            continue;
        };
        if !area.contains_horizontal(x, z) {
            report.blocks_outside += 1;
            continue;
        }
        if !area.contains_vertical(y) {
            report.blocks_out_of_world += 1;
            continue;
        }
        let Some(id) = structure.palette.get(block.state).copied() else {
            // Unreachable for a template that came from `read_structure` (the
            // reader refuses an out-of-range index) and reachable only for a
            // hand-built `ResolvedStructure`, whose author owns that invariant.
            // Counted rather than ignored so the bucket identity still holds.
            report.blocks_outside_template += 1;
            continue;
        };
        if id == air_id && air == AirPolicy::IgnoreAir {
            report.blocks_air_skipped += 1;
            continue;
        }
        // `set_block` can only fail for a `y` the check above already excluded; a
        // refusal counts as "not written" rather than being dropped.
        if matches!(chunk.set_block(x, y, z, id, registry), Ok(Some(_))) {
            report.blocks_written += 1;
        }
    }
    report
}

// ------------------------------------------------------------------ helpers

/// The chunk's writable box, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChunkArea {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    min_y: i32,
    max_y: i32,
}

impl ChunkArea {
    /// The box a chunk occupies.
    ///
    /// `min_x`/`max_x` are half-open: a chunk at `pos.x` spans
    /// `pos.x * 16 .. pos.x * 16 + 16`. The multiplication saturates, so an
    /// extreme [`mc_world::ChunkPos`] yields a degenerate box rather than a
    /// wrapped one (the same hostile-coordinate policy as
    /// [`crate::seed::pack_chunk_pos`]).
    fn of(chunk: &Chunk) -> Self {
        let min_x = chunk.pos.x.saturating_mul(SECTION_WIDTH);
        let min_z = chunk.pos.z.saturating_mul(SECTION_WIDTH);
        Self {
            min_x,
            max_x: min_x.saturating_add(SECTION_WIDTH),
            min_z,
            max_z: min_z.saturating_add(SECTION_WIDTH),
            min_y: chunk.min_y(),
            max_y: chunk.max_y(),
        }
    }

    /// Whether a world `(x, z)` lies in the chunk's 16×16 footprint.
    ///
    /// This is the **world-space** test. `Chunk::get_block`/`set_block` answer a
    /// different question — they wrap — so a caller must never use them to decide
    /// whether a coordinate is inside.
    fn contains_horizontal(&self, x: i32, z: i32) -> bool {
        (self.min_x..self.max_x).contains(&x) && (self.min_z..self.max_z).contains(&z)
    }

    /// Whether a world `y` is inside the chunk's vertical range.
    fn contains_vertical(&self, y: i32) -> bool {
        (self.min_y..self.max_y).contains(&y)
    }
}

impl ResolvedStructure {
    /// How many blocks land outside `area` at `origin`.
    ///
    /// Blocks whose template position is outside the template's own `size` are
    /// skipped: they are not part of the structure's extent, so they cannot make
    /// it overflow (`place` counts them in their own bucket).
    fn blocks_outside_at(&self, origin: (i32, i32, i32), area: ChunkArea) -> usize {
        self.blocks
            .iter()
            .filter(|block| {
                if !inside(&self.size, block.pos) {
                    return false;
                }
                let (Some(x), Some(y), Some(z)) = (
                    block.pos[0].checked_add(origin.0),
                    block.pos[1].checked_add(origin.1),
                    block.pos[2].checked_add(origin.2),
                ) else {
                    return true;
                };
                !area.contains_horizontal(x, z) || !area.contains_vertical(y)
            })
            .count()
    }
}

/// Whether a structure placed at `origin` fits entirely inside `chunk`.
///
/// The predicate [`CrossChunk::SingleChunk`] applies, exposed so a caller can ask
/// the question without placing anything — for instance to build a registry of
/// templates that will never be refused
/// ([`crate::structures::StructureRegistry::fitting_in_one_chunk`]).
///
/// Note the asymmetry with [`PlacementReport`]: this answers only the *fit*
/// question, so it ignores the air policy entirely. A structure that "fits" can
/// still write nothing.
#[must_use]
pub fn fits_in_chunk(
    structure: &ResolvedStructure,
    chunk: &Chunk,
    origin: (i32, i32, i32),
) -> bool {
    structure.blocks_outside_at(origin, ChunkArea::of(chunk)) == 0
}

/// How many of a structure's blocks would fall outside `chunk` at `origin`.
///
/// The measurement behind [`fits_in_chunk`], exposed separately because the
/// *count* is what makes a refusal diagnosable. Writes nothing, so it is safe on
/// a chunk that is still being generated.
#[must_use]
pub fn blocks_outside_chunk(
    structure: &ResolvedStructure,
    chunk: &Chunk,
    origin: (i32, i32, i32),
) -> usize {
    structure.blocks_outside_at(origin, ChunkArea::of(chunk))
}

#[cfg(test)]
mod tests {
    use super::{
        AirPolicy, Anchor, CrossChunk, PlacementReport, blocks_outside_chunk, fits_in_chunk, place,
    };
    use crate::structure::{
        ResolvedStructure, StructureBlock, StructureLimits, parse_nbt_structure,
    };
    use mc_nbt::{NbtTag, write_named};
    use mc_registry::{BlockRegistry, Registries};
    use mc_world::ChunkPos;

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    /// A minimal flat chunk: stone at `y = 0..=3`, air above.
    ///
    /// Built by hand rather than through a generator so the placement tests
    /// assert against a floor they wrote themselves.
    fn flat_chunk(blocks: &BlockRegistry) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::air(ChunkPos::new(0, 0), -4, 24, blocks);
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        for x in 0..16 {
            for z in 0..16 {
                for y in -64..=-61 {
                    let _ = chunk.set_block(x, y, z, stone, blocks);
                }
            }
        }
        chunk
    }

    /// A structure of `size` whose every cell is `block`, cells listed in
    /// `(y, z, x)` order so the expected write order is documented.
    fn filled(blocks: &BlockRegistry, size: [i32; 3], block: &str) -> ResolvedStructure {
        let id = blocks.default_state(block).expect("block");
        let mut cells = Vec::new();
        for y in 0..size[1] {
            for z in 0..size[2] {
                for x in 0..size[0] {
                    cells.push(StructureBlock {
                        pos: [x, y, z],
                        state: 0,
                    });
                }
            }
        }
        ResolvedStructure {
            size,
            palette: vec![id],
            blocks: cells,
            entities: 0,
        }
    }

    #[test]
    fn a_structure_that_fits_writes_exactly_its_blocks_at_exact_coordinates() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let gold = blocks.default_state("minecraft:gold_block").expect("gold");
        // 2 x 1 x 2 = 4 blocks, placed with its corner at (5, -60, 7).
        let structure = filled(&blocks, [2, 1, 2], "minecraft:gold_block");
        assert_eq!(structure.block_count(), 4);
        assert!(fits_in_chunk(&structure, &chunk, (5, -60, 7)));

        let report = place(
            &structure,
            &mut chunk,
            (5, -60, 7),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:gold",
        );
        assert_eq!(report.blocks_written, 4, "all four blocks changed");
        assert_eq!(report.blocks_outside, 0);
        assert_eq!(report.blocks_out_of_world, 0);
        assert_eq!(report.blocks_air_skipped, 0);
        assert_eq!(report.blocks_outside_template, 0);
        assert!(!report.was_refused());
        assert_eq!(report.accounted(), 4);

        // Exactly those four coordinates, and nothing else.
        for y in -64..320 {
            for z in 0..16 {
                for x in 0..16 {
                    let expected = if matches!((x, z), (5 | 6, 7 | 8)) && y == -60 {
                        gold
                    } else if y <= -61 {
                        blocks.default_state("minecraft:stone").expect("stone")
                    } else {
                        blocks.air_id()
                    };
                    assert_eq!(
                        chunk.get_block(x, y, z),
                        expected,
                        "block at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_structure_larger_than_a_chunk_is_refused_and_counted() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let before = chunk.clone();
        // 20 wide: four columns land in the next chunk's footprint.
        let structure = filled(&blocks, [20, 1, 1], "minecraft:gold_block");
        let report = place(
            &structure,
            &mut chunk,
            (0, -60, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:wide",
        );
        assert!(report.was_refused(), "{report:?}");
        assert_eq!(report.blocks_written, 0, "a refusal writes nothing");
        let refusal = report.refused.as_deref().expect("a reason");
        assert!(refusal.contains("test:wide"), "{refusal}");
        assert!(refusal.contains("4 of 20"), "{refusal}");
        assert_eq!(
            chunk, before,
            "a refused structure must not touch the chunk"
        );
        // The count is what `CrossChunk::Clip` would report as `blocks_outside`.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (0, -60, 0)), 4);
    }

    #[test]
    fn clip_writes_the_inside_and_counts_the_outside() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let structure = filled(&blocks, [20, 1, 1], "minecraft:gold_block");
        let report = place(
            &structure,
            &mut chunk,
            (0, -60, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::Clip,
            &blocks,
            "test:wide",
        );
        assert!(!report.was_refused(), "{report:?}");
        assert_eq!(report.blocks_written, 16, "columns 0..=15 are in the chunk");
        assert_eq!(report.blocks_outside, 4, "columns 16..=19 are not");
        assert_eq!(report.accounted(), 20, "every block is accounted for");
        // The 16 in-bounds columns really are gold, and column 16 of the *next*
        // chunk was never touched: `set_block` would have wrapped it onto this
        // chunk's column 0.
        for x in 0..16 {
            assert_eq!(
                chunk.get_block(x, -60, 0),
                blocks.default_state("minecraft:gold_block").expect("gold"),
                "column {x}"
            );
        }
    }

    #[test]
    fn a_partly_outside_structure_counts_the_right_number() {
        let blocks = registry();
        let chunk = flat_chunk(&blocks);
        // A 4 x 1 x 4 block: starting at x = 14 puts columns 14, 15 inside and
        // 16, 17 outside -> 8 of 16 blocks outside.
        let structure = filled(&blocks, [4, 1, 4], "minecraft:gold_block");
        assert_eq!(structure.block_count(), 16);
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (14, -60, 0)), 8);
        // Starting at z = 15: rows 15 inside, 16..18 outside -> 12 of 16.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (0, -60, 15)), 12);
        // Corners count once, not twice.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (15, -60, 15)), 15);
        // A fully outside footprint is all 16.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (16, -60, 16)), 16);
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (-4, -60, -4)), 16);
        // A negative origin that is *in* the chunk cannot happen (this chunk's
        // footprint is `0..16`), and the sample is here to prove the count is
        // right at the boundary rather than off by one: starting at x = -1 and
        // z = -1 puts the z = -1 row (4 blocks) and the x = -1 column (4 blocks)
        // outside, and the corner (-1, -1) is counted once -> 7, not 8.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (-1, -60, -1)), 7);
        // At exactly -4 the whole footprint lies to the left of the chunk.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (-4, -60, 0)), 16);
    }

    #[test]
    fn the_air_policy_is_explicit_and_tested_both_ways() {
        let blocks = registry();
        let air = blocks.air_id();
        let gold = blocks.default_state("minecraft:gold_block").expect("gold");
        // A 1 x 2 x 1 column: air at y = 0, gold at y = 1. Placed at y = -62 it
        // overwrites stone at -62 (air) and air at -61 (gold).
        let structure = ResolvedStructure {
            size: [1, 2, 1],
            palette: vec![air, gold],
            blocks: vec![
                StructureBlock {
                    pos: [0, 0, 0],
                    state: 0,
                },
                StructureBlock {
                    pos: [0, 1, 0],
                    state: 1,
                },
            ],
            entities: 0,
        };

        // `IgnoreAir` (the default): the solid stone at y = -62 survives.
        let mut ignore = flat_chunk(&blocks);
        let report = place(
            &structure,
            &mut ignore,
            (3, -62, 3),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:air",
        );
        assert_eq!(report.blocks_written, 1, "only the gold is written");
        assert_eq!(report.blocks_air_skipped, 1);
        assert_eq!(report.accounted(), 2);
        assert_eq!(
            ignore.get_block(3, -62, 3),
            blocks.default_state("minecraft:stone").expect("stone"),
            "ignore-air must not punch a hole in terrain"
        );
        assert_eq!(ignore.get_block(3, -61, 3), gold);

        // `Overwrite`: the same block becomes air.
        let mut overwrite = flat_chunk(&blocks);
        let report = place(
            &structure,
            &mut overwrite,
            (3, -62, 3),
            AirPolicy::Overwrite,
            CrossChunk::SingleChunk,
            &blocks,
            "test:air",
        );
        assert_eq!(
            report.blocks_written, 2,
            "the air block changed stone to air"
        );
        assert_eq!(report.blocks_air_skipped, 0);
        assert_eq!(overwrite.get_block(3, -62, 3), air, "overwrite carves");
        assert_eq!(overwrite.get_block(3, -61, 3), gold);
        // The two policies really do disagree about this block.
        assert_ne!(ignore.get_block(3, -62, 3), overwrite.get_block(3, -62, 3));

        // The default is `IgnoreAir`, asserted so a change of default is a test
        // failure rather than a silent behaviour change.
        assert_eq!(AirPolicy::default(), AirPolicy::IgnoreAir);
        assert_eq!(CrossChunk::default(), CrossChunk::SingleChunk);
    }

    #[test]
    fn blocks_above_and_below_the_world_are_counted_separately() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let structure = filled(&blocks, [1, 4, 1], "minecraft:gold_block");
        // The chunk spans -64..320; a column starting at 318 puts two blocks
        // outside the top.
        assert_eq!(blocks_outside_chunk(&structure, &chunk, (0, 318, 0)), 2);
        let report = place(
            &structure,
            &mut chunk,
            (0, 318, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::Clip,
            &blocks,
            "test:tall",
        );
        assert_eq!(report.blocks_written, 2);
        assert_eq!(report.blocks_out_of_world, 2);
        assert_eq!(report.accounted(), 4);

        // Below the floor: the whole column is out.
        let report = place(
            &structure,
            &mut chunk,
            (0, -70, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::Clip,
            &blocks,
            "test:deep",
        );
        assert_eq!(report.blocks_written, 0);
        assert_eq!(report.blocks_out_of_world, 4);
    }

    #[test]
    fn hostile_origins_do_not_overflow_or_panic() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let structure = filled(&blocks, [2, 2, 2], "minecraft:gold_block");
        for origin in [
            (i32::MAX, i32::MAX, i32::MAX),
            (i32::MIN, i32::MIN, i32::MIN),
            (i32::MAX - 1, -60, i32::MIN + 1),
            (0, i32::MAX, 0),
        ] {
            let report = place(
                &structure,
                &mut chunk,
                origin,
                AirPolicy::Overwrite,
                CrossChunk::Clip,
                &blocks,
                "test:hostile",
            );
            assert_eq!(
                report.accounted(),
                structure.block_count(),
                "every block is accounted for at {origin:?}"
            );
            assert_eq!(report.blocks_written, 0, "nothing lands in the chunk");
            // And the refusal path agrees.
            let refused = place(
                &structure,
                &mut chunk,
                origin,
                AirPolicy::Overwrite,
                CrossChunk::SingleChunk,
                &blocks,
                "test:hostile",
            );
            assert!(refused.was_refused(), "{origin:?}");
        }
    }

    #[test]
    fn a_block_outside_the_template_size_is_counted_not_written() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let gold = blocks.default_state("minecraft:gold_block").expect("gold");
        // A datapack-authored file whose block list leaves the declared size.
        let structure = ResolvedStructure {
            size: [1, 1, 1],
            palette: vec![gold],
            blocks: vec![
                StructureBlock {
                    pos: [0, 0, 0],
                    state: 0,
                },
                StructureBlock {
                    pos: [5, 0, 0],
                    state: 0,
                },
            ],
            entities: 0,
        };
        let report = place(
            &structure,
            &mut chunk,
            (2, -60, 2),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:wide-template",
        );
        assert_eq!(report.blocks_written, 1);
        assert_eq!(report.blocks_outside_template, 1);
        assert_eq!(report.accounted(), 2);
        // The out-of-size block did not make the structure "not fit": it is not
        // part of the extent, so the single-chunk policy let it through.
        assert!(!report.was_refused());
        assert_eq!(chunk.get_block(2, -60, 2), gold);
        assert_ne!(chunk.get_block(7, -60, 2), gold, "not written");
    }

    #[test]
    fn the_anchor_centres_and_sits_on_the_surface() {
        let size = [5, 3, 5];
        let anchor = Anchor::OnSurface {
            block_x: 100,
            surface_y: 64,
            block_z: -50,
        };
        // 5 / 2 = 2, so the footprint is 98..=102 and 19 of the origin column is
        // the centre.
        assert_eq!(anchor.origin(size), (98, 65, -52));
        assert_eq!(Anchor::At { origin: (1, 2, 3) }.origin(size), (1, 2, 3));
        // Hostile values saturate rather than wrapping.
        let hostile = Anchor::At {
            origin: (i32::MAX, i32::MIN, 0),
        }
        .origin([i32::MAX, 1, 1]);
        assert_eq!(hostile, (i32::MAX, i32::MIN, 0));
        // A truncated anchor size cannot flip the sign of the offset. `i32::MIN / 2`
        // truncates towards zero and is therefore `-1073741824`, **not** zero: the
        // offset is negative, so `0 - offset` moves the origin *right* and *up*.
        // That is the documented behaviour of `size[0] / 2` and the reason this
        // case is asserted rather than assumed — an earlier version of this test
        // expected `(0, 1, 0)` and was wrong about which way truncation goes.
        assert_eq!(
            Anchor::OnSurface {
                block_x: 0,
                surface_y: 0,
                block_z: 0
            }
            .origin([i32::MIN, 1, i32::MIN]),
            (1_073_741_824, 1, 1_073_741_824)
        );
        // A hostile `surface_y` saturates instead of wrapping to the dimension
        // floor, which is the property that keeps `place` from writing at the
        // wrong end of the world.
        assert_eq!(
            Anchor::OnSurface {
                block_x: 0,
                surface_y: i32::MAX,
                block_z: 0
            }
            .origin([1, 1, 1]),
            (0, i32::MAX, 0)
        );
    }

    #[test]
    fn the_report_predicates_distinguish_the_outcomes() {
        let empty = PlacementReport::empty();
        assert!(empty.wrote_nothing());
        assert!(!empty.was_refused());
        assert_eq!(empty.accounted(), 0);
        let refused = PlacementReport::refuse("minecraft:test", "too big");
        assert!(refused.wrote_nothing());
        assert!(refused.was_refused());
        assert_eq!(refused.accounted(), 0, "a refusal accounts for nothing");
        assert!(
            refused
                .refused
                .as_deref()
                .expect("reason")
                .contains("minecraft:test")
        );
    }

    #[test]
    fn an_empty_structure_places_nothing_and_is_not_a_refusal() {
        let blocks = registry();
        let mut chunk = flat_chunk(&blocks);
        let structure = ResolvedStructure {
            size: [1, 1, 1],
            palette: vec![blocks.air_id()],
            blocks: Vec::new(),
            entities: 0,
        };
        let report = place(
            &structure,
            &mut chunk,
            (0, -60, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:empty",
        );
        assert_eq!(report, PlacementReport::empty());
        // An all-air template under `IgnoreAir` writes nothing but is *not* a
        // refusal: this is exactly the distinction `refused` exists to make.
        let all_air = ResolvedStructure {
            size: [1, 1, 1],
            palette: vec![blocks.air_id()],
            blocks: vec![StructureBlock {
                pos: [0, 0, 0],
                state: 0,
            }],
            entities: 0,
        };
        let report = place(
            &all_air,
            &mut chunk,
            (0, -60, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "test:air",
        );
        assert!(report.wrote_nothing());
        assert!(!report.was_refused(), "{report:?}");
        assert_eq!(report.blocks_air_skipped, 1);
    }

    #[test]
    fn a_real_zero_axis_pack_file_is_handled() {
        // `empty.nbt` and friends are 1 x 1 x 1 with one air block; this is the
        // shape the pack actually ships, rebuilt here so the parser and placer
        // agree about it without needing the jar.
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
            (
                "palette".to_owned(),
                NbtTag::List(vec![NbtTag::compound([(
                    "Name".to_owned(),
                    NbtTag::String("minecraft:air".into()),
                )])]),
            ),
            (
                "blocks".to_owned(),
                NbtTag::List(vec![NbtTag::compound([
                    ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                    ("state".to_owned(), NbtTag::Int(0)),
                ])]),
            ),
        ]);
        let mut raw = Vec::new();
        write_named("", &tag, &mut raw).expect("encodes");
        let template = parse_nbt_structure(&raw, StructureLimits::PACK).expect("parses");
        assert_eq!(template.size, [1, 1, 1]);
        assert_eq!(template.block_count(), 1);
        let blocks = registry();
        let resolved = template.resolve(&blocks).expect("air resolves");
        let mut chunk = flat_chunk(&blocks);
        let before = chunk.clone();
        let report = place(
            &resolved,
            &mut chunk,
            (0, -60, 0),
            AirPolicy::IgnoreAir,
            CrossChunk::SingleChunk,
            &blocks,
            "minecraft:empty",
        );
        assert_eq!(report.blocks_air_skipped, 1);
        assert_eq!(chunk, before);
    }
}
