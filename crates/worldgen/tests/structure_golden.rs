//! Frozen selection decisions for the structure rule (P07-16).
//!
//! These values were **printed by this implementation and then frozen**. That is
//! what makes them useful: they are not a claim about correctness, they are a
//! tripwire. Any change to the rule — a different salt, a different attempt count,
//! a different modulo, a reordered hash chain — moves at least one of them, and the
//! failure names the `(seed, chunk_pos)` pair.
//!
//! Compare with `docs/vanilla-parity/PARITY-MATRIX.md`: **none of these is a
//! Vanilla value.** Vanilla selects structures with
//! `RandomSpreadStructurePlacement` over per-structure `StructureSet` JSON this
//! project has not loaded, so a Vanilla world's structures are elsewhere. This is
//! the honest position, and the constants the rule uses are labelled
//! `approximation` / `product decision` in `mc_worldgen::structures`.
//!
//! The companion `tests/golden.rs` freezes the noise pipeline the same way.
//!
//! ```text
//! cargo test -p mc-worldgen --test structure_golden -- --nocapture
//! ```

// Exact comparison is the *point* of this file: the constants and the frozen values are exact
// literals, and a tolerance would let a changed constant through. Same reasoning as `terrain.rs`.
//
// The integer/float casts below are **sample arithmetic**: cell counts of at most a few thousand and
// coupon-collector bounds derived from them, all exactly representable. Rewriting them to satisfy a
// lint about 64-bit pointer widths would obscure the arithmetic the file documents at length.
// These must precede every item in the file to be legal inner attributes.
#![allow(clippy::float_cmp)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use mc_core::ids::ResourceId;
use mc_world::ChunkPos;
use mc_worldgen::WorldSeed;
use mc_worldgen::structure::{PaletteEntry, StructureBlock, StructureTemplate};
use mc_worldgen::structures::{
    MAX_SELECTABLE_STRUCTURES, STRUCTURE_CHANCE_PER_CELL, STRUCTURE_MAX_ATTEMPTS,
    STRUCTURE_SEPARATION_CHUNKS, STRUCTURE_SPACING_CHUNKS, StructureGrid, StructureRegistry,
    StructureSet, spacing_for,
};

/// The evidence world's seed, shared with `tests/golden.rs`.
const SEED: i64 = 1_361_882_806;

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid resource id")
}

/// A one-block template of `minecraft:stone`, so a registry can be built without
/// touching the pack or the block registry.
fn one_block_template() -> StructureTemplate {
    StructureTemplate {
        size: [1, 1, 1],
        palette: vec![PaletteEntry {
            name: id("minecraft:stone"),
            properties: Vec::new(),
        }],
        blocks: vec![StructureBlock {
            pos: [0, 0, 0],
            state: 0,
        }],
        entities: 0,
        data_version: Some(4790),
    }
}

/// A set of `count` synthetic names, built through the **production** derivation.
///
/// `StructureSet::from_registry` is called for real rather than reimplemented here:
/// an earlier version of this helper recomputed the grid itself and disagreed with
/// the production `separation_for`, so the frozen values would have described a rule
/// nothing else used.
fn set(count: usize) -> StructureSet {
    let mut registry = StructureRegistry::new();
    for index in 0..count {
        registry.insert(
            id(&format!("minecraft:structure/golden_{index}")),
            one_block_template(),
        );
    }
    StructureSet::from_registry(&registry)
}

#[test]
fn the_shipped_grid_constants_are_what_the_rule_documents() {
    // The documented constant set, frozen. A change here changes which chunks hold
    // structures for every world.
    assert_eq!(STRUCTURE_SPACING_CHUNKS, 8);
    assert_eq!(STRUCTURE_SEPARATION_CHUNKS, 2);
    assert_eq!(STRUCTURE_MAX_ATTEMPTS, 1);
    assert_eq!(STRUCTURE_CHANCE_PER_CELL, 0.5);
    // The default grid is built from them.
    let grid = StructureGrid::default();
    assert_eq!(grid.spacing_chunks(), 8);
    assert_eq!(grid.separation_chunks(), 2);
    assert_eq!(grid.max_attempts(), 1);
    assert_eq!(grid.drawable_chunks(), 6);
    // The derived spacing for a registry of `n` names is `ceil(sqrt(2n))`.
    let frozen: [(usize, i32); 6] = [(0, 1), (1, 2), (2, 2), (8, 4), (50, 10), (1_182, 49)];
    for (count, expected) in frozen {
        assert_eq!(spacing_for(count), expected, "spacing_for({count}) moved");
    }
}

#[test]
fn the_uniform_float_construction_is_frozen_through_its_only_caller() {
    // `unit_float` is private and stays private: exposing it for a test would add
    // public API to freeze an implementation detail. Its *consequences* are frozen
    // instead, by `the_selection_decision_is_frozen_for_a_handful_of_positions`
    // below, which is the caller that depends on it.
    //
    // What is asserted here is the property the construction is chosen for: the
    // value can never reach `1.0`, so a chance of `1.0` is a certainty and a chance
    // of `0.0` is impossible — the boundary the whole rule rests on.
    let certain = StructureSet::with_names(
        [id("minecraft:structure/probe")],
        StructureGrid::new(1, 0, 1),
        1.0,
    );
    let impossible = StructureSet::with_names(
        [id("minecraft:structure/probe")],
        StructureGrid::new(1, 0, 1),
        f64::MIN_POSITIVE,
    );
    let seed = WorldSeed::from_raw(0);
    let mut certain_selected = 0_usize;
    let mut impossible_selected = 0_usize;
    for x in 0..64 {
        let pos = ChunkPos::new(x, 0);
        if certain.select(seed, pos).is_some() {
            certain_selected += 1;
        }
        if impossible.select(seed, pos).is_some() {
            impossible_selected += 1;
        }
    }
    println!(
        "MEASURED chance 1.0 selected {certain_selected}/64, \
         chance f64::MIN_POSITIVE selected {impossible_selected}/64"
    );
    assert_eq!(certain_selected, 64, "chance 1.0 must be a certainty");
    assert!(
        impossible_selected < 64,
        "a sub-representable chance must not be a certainty"
    );
    // A seed of zero is the classic degenerate case (splitmix64's finalizer maps
    // zero to zero), and it must still select — see `WorldSeed::chunk_seed`'s
    // golden-gamma note. A 1x1 certain grid at seed 0 covers it.
    assert!(
        certain
            .select(WorldSeed::from_raw(0), ChunkPos::new(0, 0))
            .is_some(),
        "the zero seed must not produce an empty stream"
    );
}

#[test]
fn the_selection_decision_is_frozen_for_a_handful_of_positions() {
    // A registry of 12 names: the derived grid is `ceil(sqrt(24)) = 5` chunks, with
    // `min(5/4, 4) = 1` chunk of separation, so 4 drawable chunks per axis.
    let set = set(12);
    assert_eq!(set.names().len(), 12);
    assert_eq!(set.grid().spacing_chunks(), 5);
    assert_eq!(set.grid().separation_chunks(), 1);
    assert_eq!(set.grid().drawable_chunks(), 4);
    assert_eq!(set.grid().max_attempts(), 1);

    let seed = WorldSeed::from_raw(SEED);
    // (chunk_x, chunk_z, the name this rule selects, or "" for no structure).
    //
    // **Measured, not adjusted.** Eight positions select and six do not, and the `selected` assertion
    // below pins the count of eight. An earlier version of this array held twelve empty strings: its
    // sample happened to contain no structures, so it pinned nothing, and the `selected == 3`
    // assertion that would have caught that was edited to `""` instead of the sample being fixed.
    let frozen: [(i32, i32, &str); 14] = [
        (-12, -4, "minecraft:structure/golden_0"),
        (-7, -3, "minecraft:structure/golden_6"),
        (-5, -18, "minecraft:structure/golden_7"),
        (-13, 10, "minecraft:structure/golden_4"),
        (-17, -2, "minecraft:structure/golden_10"),
        (-9, 20, "minecraft:structure/golden_4"),
        (-8, -19, "minecraft:structure/golden_1"),
        (-15, 37, "minecraft:structure/golden_2"),
        (0, 0, ""),
        (1, 0, ""),
        (100, 200, ""),
        (1_000, -2_000, ""),
        (-7, 13, ""),
        (4, 4, ""),
    ];
    let mut selected = 0_usize;
    let mut mismatches: Vec<String> = Vec::new();
    println!("FROZEN structure selection for seed {SEED} (12 names, 4-chunk cells):");
    for (x, z, expected) in frozen {
        let pos = ChunkPos::new(x, z);
        let found = set.select(seed, pos);
        let actual = found
            .as_ref()
            .map_or_else(String::new, |s| s.name.to_string());
        println!("  ({x:>5}, {z:>5}, {actual:?}),");
        if actual != expected {
            mismatches.push(format!("({x}, {z}): got {actual:?}, expected {expected:?}"));
        }
        if !expected.is_empty() {
            selected += 1;
        }
        if let Some(selection) = found {
            assert_eq!(selection.origin, pos);
            assert_eq!(selection.chance, STRUCTURE_CHANCE_PER_CELL);
            assert_eq!(selection.attempt, 0, "one attempt per cell by default");
            assert_eq!(selection.cell, set.grid().cell_of(pos));
            // The ordinal really is the name's position in the sorted list.
            assert_eq!(
                set.names()[selection.ordinal],
                selection.name,
                "ordinal must index the sorted name list"
            );
        }
    }
    assert!(
        mismatches.is_empty(),
        "the structure distribution changed:\n{}",
        mismatches.join("\n")
    );
    assert_eq!(
        selected, 8,
        "the sample must contain exactly the eight measured selections; a sample that emptied would \
         otherwise let this whole array be rewritten to match"
    );
    // The sample is sparse: six of the fourteen positions hold nothing, which is asserted by the
    // eight measured selections above.
}

#[test]
fn the_selection_is_reproducible_and_order_independent() {
    let set = set(12);
    let seed = WorldSeed::from_raw(SEED);
    let positions: Vec<ChunkPos> = (0..24)
        .map(|index| ChunkPos::new(index % 8, index / 8))
        .collect();

    // Forward, then in reverse: the same decision for every position, because the
    // rule is a pure function of `(seed, pos)` and consults no neighbour.
    let forward: Vec<Option<String>> = positions
        .iter()
        .map(|pos| set.select(seed, *pos).map(|s| s.name.to_string()))
        .collect();
    let backward: Vec<Option<String>> = positions
        .iter()
        .rev()
        .map(|pos| set.select(seed, *pos).map(|s| s.name.to_string()))
        .collect();
    let mut expected: Vec<Option<String>> = backward;
    expected.reverse();
    assert_eq!(forward, expected, "selection depends on evaluation order");
}

/// How many cells a reachability sample needs, for `names` distinct templates.
///
/// **This is a measured derivation, not a guess.** Selection picks a name with
/// `h3 % names.len()`, a uniform draw, so reaching all `names` templates is the
/// coupon-collector problem: after `m` draws the expected number of *unseen* names
/// is `names · e^(−m/names)`. Requiring that to be at most `names⁻³` — so the
/// probability that even one template is missed is below `1e-6` for every size this
/// test exercises — gives `m ≥ 3 · names · ln(names)`. At `chance = 1.0` every cell
/// yields exactly one draw, so `m` cells are needed, and the sample is
/// `ceil(√m)` cells on each axis.
///
/// The first version of this test used `2 · names · ln(names)` draws sampled over a
/// fixed 256-cell grid and **failed at 64 names** (6 templates unreachable), which
/// is what the arithmetic above predicts: 256 cells over 64 names leaves
/// `64·e^(−4) ≈ 1.2` expected misses, and the observed 6 is an ordinary fluctuation
/// of that. The bug was in the test's sample size, not in the rule — and it is
/// exactly the kind of "looks reachable" claim that a smaller sample would have
/// let through.
fn cells_for_full_coverage(names: usize) -> usize {
    if names <= 1 {
        return 4;
    }
    let names_f = names as f64;
    let draws = (3.0 * names_f * names_f.ln()).ceil() as usize;
    // `ceil(sqrt(draws))` cells per axis, so the square sample holds `>= draws`.
    let mut side = 1_usize;
    while side * side < draws {
        side += 1;
    }
    side
}

/// The selectable name list is capped, so one chunk's cost does not depend on the pack size.
///
/// A registry may hold all 1 182 loadable pack templates, but `StructureSet::from_registry` takes a
/// bounded, sorted slice — so this is the one place the selection's name list is *not* "everything
/// registered", and the cap is what keeps a chunk's cost independent of the installed data pack.
#[test]
fn the_selectable_name_list_is_capped_at_a_documented_size() {
    // `MAX_SELECTABLE_STRUCTURES` is a documented product decision: a registry may
    // hold all 1 182 loadable pack templates, but a single chunk's cost must not
    // depend on the size of the installed data pack, so `from_registry` takes a
    // bounded, sorted slice. Asserted here because it is the one place the
    // selection's name list is *not* "everything registered".
    assert_eq!(MAX_SELECTABLE_STRUCTURES, 64);
    let mut huge = StructureRegistry::new();
    for index in 0..(MAX_SELECTABLE_STRUCTURES + 10) {
        huge.insert(
            id(&format!("minecraft:structure/golden_{index:03}")),
            one_block_template(),
        );
    }
    let capped = StructureSet::from_registry(&huge);
    assert_eq!(huge.len(), MAX_SELECTABLE_STRUCTURES + 10);
    assert_eq!(capped.names().len(), MAX_SELECTABLE_STRUCTURES);
    // The slice is the *first* names in sorted order, so it is deterministic
    // rather than dependent on insertion order.
    assert_eq!(
        capped.names()[0].to_string(),
        "minecraft:structure/golden_000"
    );
    assert_eq!(
        capped.names()[63].to_string(),
        "minecraft:structure/golden_063"
    );
    assert!(
        !capped
            .names()
            .contains(&id("minecraft:structure/golden_064")),
        "the cap must exclude the names past the slice"
    );
}

/// Reachability is asserted only up to the selectable cap; this records that the
/// test knows why the loop below stops where it does.
#[test]
fn every_registered_template_is_reachable_at_a_documented_sample_size() {
    // The "reachable" half of P07-16's requirement, at `chance = 1.0` so this
    // measures the **name draw** alone. Sparsity — the other half — is measured
    // separately below at the shipped chance, because folding the two together
    // would make a failure impossible to attribute.
    for count in [1_usize, 2, 5, 24, 64, 128] {
        let set = set(count);
        let seed = WorldSeed::from_raw(SEED);
        let spacing = set.grid().spacing_chunks();
        let side = cells_for_full_coverage(count);
        let span = spacing * side as i32;
        let mut reached: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut draws = 0_usize;
        for x in 0..span {
            for z in 0..span {
                if let Some(selection) = set.select_for(seed, ChunkPos::new(x, z), 1.0) {
                    reached.insert(selection.name.to_string());
                    draws += 1;
                }
            }
        }
        println!(
            "MEASURED reachability: {count} names, spacing {spacing}, {side}x{side} cells \
             ({span}x{span} chunks), {draws} draws, {} names reached",
            reached.len()
        );
        // The expectation is the **selectable** count, not `count`: `from_registry` caps the name
        // list at `MAX_SELECTABLE_STRUCTURES`, so a registry larger than the cap can never have every
        // template reached — the cap is what keeps a chunk's cost independent of the installed pack
        // size. An earlier version asserted `reached == count` and failed at count=128 with "64
        // templates were never selected", which is the cap working, not a selection bug.
        let selectable = count.min(MAX_SELECTABLE_STRUCTURES);
        let missing: Vec<String> = set
            .names()
            .iter()
            .take(selectable)
            .map(ToString::to_string)
            .filter(|name| !reached.contains(name))
            .collect();
        assert_eq!(
            reached.len(),
            selectable,
            "count={count} (selectable {selectable}): {} template(s) unreachable over {draws} draws:              {missing:?}",
            missing.len()
        );
        // Every draw lands on a registered name, and the sample really is one draw
        // per cell (so the coupon-collector arithmetic above applies).
        assert_eq!(draws, (span as usize / spacing as usize).pow(2));
    }
}

#[test]
fn the_shipped_chance_keeps_the_world_sparse() {
    // The "sparse" half: at the shipped chance, not every chunk may hold a
    // structure — and the density is documented rather than incidental. Measured
    // over one grid cell per sample, which is the unit the rule works in.
    for count in [1_usize, 5, 24, 64] {
        let set = set(count);
        let seed = WorldSeed::from_raw(SEED);
        let spacing = set.grid().spacing_chunks();
        let side = 12_i32;
        let span = spacing * side;
        let mut sampled = 0_usize;
        let mut selected = 0_usize;
        for x in 0..span {
            for z in 0..span {
                sampled += 1;
                if set.select(seed, ChunkPos::new(x, z)).is_some() {
                    selected += 1;
                }
            }
        }
        let density = selected as f64 / sampled as f64;
        println!(
            "MEASURED sparsity: {count} names, spacing {spacing}: {selected} of {sampled} \
             chunks selected ({:.3}%)",
            density * 100.0
        );
        assert!(selected > 0, "count={count}: nothing was ever selected");
        assert!(
            selected < sampled,
            "count={count}: every chunk was selected ({selected} of {sampled})"
        );
        // Well under a quarter of chunks: the world must read as mostly empty of
        // structures, which is the property the constants are sized for.
        assert!(
            density < 0.25,
            "count={count}: density {density} is not sparse"
        );
        // And it really is the shipped chance doing the work: with certainty the
        // same grid selects about twice as many chunks.
        let mut certain = 0_usize;
        for x in 0..span {
            for z in 0..span {
                if set.select_for(seed, ChunkPos::new(x, z), 1.0).is_some() {
                    certain += 1;
                }
            }
        }
        println!("MEASURED   at chance 1.0 the same area selects {certain}");
        assert!(
            certain > selected,
            "count={count}: the chance draw is not thinning anything ({certain} vs {selected})"
        );
    }
}

#[test]
fn the_advance_attempt_count_never_skips_a_cell() {
    // The bug this test exists for: `h0 % (max_attempts + 1)` draws **zero**
    // attempts half the time, so a 1x1 grid could never select anything and every
    // other grid was half as dense as documented. With `1 + h0 % max_attempts` a
    // certain-chance set must select *every* chunk of a 1-chunk grid.
    let certain = StructureSet::with_names(
        [id("minecraft:structure/always")],
        StructureGrid::new(1, 0, 1),
        1.0,
    );
    let seed = WorldSeed::from_raw(SEED);
    for x in -4..4 {
        for z in -4..4 {
            let pos = ChunkPos::new(x, z);
            assert!(
                certain.select(seed, pos).is_some(),
                "a 1x1 certain grid must select every chunk, ({x}, {z}) was skipped"
            );
        }
    }
    // And with `max_attempts` raised, a certain chance still selects every chunk
    // (the count varies, the outcome does not).
    let multi = StructureSet::with_names(
        [id("minecraft:structure/always")],
        StructureGrid::new(2, 1, 4),
        1.0,
    );
    let mut selected = 0_usize;
    for x in 0..16 {
        for z in 0..16 {
            if multi.select(seed, ChunkPos::new(x, z)).is_some() {
                selected += 1;
            }
        }
    }
    println!("MEASURED 2x2 cell, 4 attempts, certainty: {selected} of 256 chunks selected");
    assert_eq!(
        selected, 64,
        "a 2x2 cell has 4 chunks and one origin each, so exactly 4 per 16 chunks"
    );
}

#[test]
fn a_zero_or_certain_chance_selects_nothing_or_everything() {
    let set = set(6);
    let seed = WorldSeed::from_raw(SEED);
    let pos = ChunkPos::new(0, 0);
    // Chance 0 is `None` everywhere, and never an empty name.
    for x in 0..32 {
        for z in 0..32 {
            assert_eq!(
                set.select_for(seed, ChunkPos::new(x, z), 0.0),
                None,
                "chance 0 must select nothing"
            );
        }
    }
    // Chance 1 always selects *something* at the cell origin, and never writes a
    // name outside the registered set.
    let mut names = std::collections::BTreeSet::new();
    for x in 0..32 {
        for z in 0..32 {
            if let Some(selection) = set.select_for(seed, ChunkPos::new(x, z), 1.0) {
                assert!(set.names().contains(&selection.name));
                names.insert(selection.name.to_string());
            }
        }
    }
    assert!(!names.is_empty());
    assert!(
        names.len() <= set.names().len(),
        "names came from outside the set"
    );
    assert_eq!(pos, set.grid().origin_in_cell(ChunkPos::new(0, 0), 0));
}
