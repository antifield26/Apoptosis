//! Differential light test: run **our** engine on **vanilla's** blocks and compare with **vanilla's** light.
//!
//! ## What makes this the strongest check available
//!
//! The captured chunk packets carry both halves: the block states of a real vanilla world, and the light
//! arrays that vanilla computed for them. So the two can be compared directly, with no modelling in between —
//! our engine is handed vanilla's own blocks and its answer is checked against vanilla's.
//!
//! Every other check so far has been weaker than this. The unit tests use synthetic worlds and assert rules I
//! wrote down; the shape checks confirm the payload is well-formed; the real client proves a client *accepts*
//! it. None of them can catch a light engine that is self-consistent and wrong, because a client does not
//! validate light levels — it renders whatever it is told. This test can.
//!
//! ## What it found
//!
//! It found one, immediately. The light table's `block` rows — the 738 blocks whose states share one triple,
//! which covers **stone, dirt and every common terrain block** — were parsed into a vector that nothing read,
//! so those states kept the default transparent air and the world was lit straight through its terrain. The
//! agreement was **87.5%, exactly 7/8**: fourteen of sixteen arrays matched perfectly and the terrain section
//! was uniformly 15 where vanilla's was mixed.
//!
//! After fixing the parser the sky agreement is **100.0000%** — 262 144 of 262 144 cells across 32 chunks,
//! worst difference zero, with the world stitched from all 117 captured chunks so light crosses borders
//! through real neighbouring blocks.
//!
//! ## What a second capture added, and what it costs
//!
//! The first capture had no light sources, so it carried no block-light arrays and could not test that layer
//! at all. The second places eight glowstone blocks through the server console and then captures. Block light
//! agrees on **40 939 of 40 960 cells** — 99.95%, worst difference 3, the disagreements confined to the two
//! chunks adjacent to the glowstone.
//!
//! That gap is the one-block margin, and it is a **design approximation rather than a bug**: the margin lets
//! light enter a chunk across its border but not travel several blocks outside it first. Exactness would cost
//! a 15-block margin, which is 6.5x the work per chunk — see the assertion below, where the trade is named.
//!
//! **Block light is not covered**: a superflat world has no light sources, so vanilla sent no block-light
//! arrays and there is nothing to compare. That is asserted below rather than left implicit.
//!
//! ## The two approximations, stated
//!
//! * **The margin.** Our engine reads one block outside the chunk so light crosses borders, and the capture
//!   only contains this chunk. Rather than treat the neighbour as air — which would brighten the edge — the
//!   edge column is *replicated*. The captured world is superflat, so the neighbouring chunk really is
//!   identical, and replication is the honest approximation rather than a convenient one.
//! * **`min_y`.** The packet carries a section count but not the world floor. The overworld's is `-64`, which
//!   the capture's 24 sections confirm.
//!
//! ## Running it
//!
//! ```text
//! MC_VANILLA_CAPTURE=target/vanilla-capture/bodies \
//!   cargo test -p mc-server --test vanilla_light_differential -- --ignored --nocapture
//! ```

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::LevelChunkWithLight;
use mc_registry::Registries;
use mc_world::light::{MAX_LIGHT, compute_chunk_light};
use std::collections::HashMap;
use std::path::PathBuf;

/// The overworld floor. The capture's chunks hold 24 sections, which is `384 / 16`.
const MIN_Y: i32 = -64;

fn captured_chunks() -> Vec<PathBuf> {
    let root = std::env::var("MC_VANILLA_CAPTURE")
        .unwrap_or_else(|_| "target/vanilla-capture/bodies".to_owned());
    let raw = PathBuf::from(&root);
    let directory = if raw.is_absolute() {
        raw
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(raw)
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_s2c_play_45.bin"))
        })
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no captured chunks in {}",
        directory.display()
    );
    files
}

/// The light level in one cell of a wire array: low nibble for even cells, high for odd.
fn nibble(array: &[u8], cell: usize) -> u8 {
    let byte = array[cell / 2];
    if cell.is_multiple_of(2) {
        byte & 0x0F
    } else {
        byte >> 4
    }
}

/// Per-cell global block-state ids for one section, in vanilla scan order.
fn section_cells(section: &mc_protocol::packets::play::ChunkSection) -> Vec<i32> {
    let container = &section.block_states;
    // A one-value container is the *single-value* form: one palette entry and no index array at all, so the
    // cells are not in `values` and reading it as if they were yields an empty section.
    if container.values.is_empty() {
        let only = container.palette.first().copied().unwrap_or(0);
        return vec![only.cast_signed(); 4096];
    }
    let mut cells = vec![0i32; 4096];
    for (cell, index) in container.values.iter().enumerate() {
        let Some(global) = container.palette.get(*index as usize) else {
            continue;
        };
        cells[cell] = (*global).cast_signed();
    }
    cells
}

/// The block-state id at a world position, answered from the **stitched world**.
///
/// A position whose chunk is not among the captured ones reads as `None`, which the engine treats as an
/// unloaded neighbour. An earlier version replicated the chunk's own edge column instead — correct for a
/// superflat world, and wrong the moment a light source sat in the next chunk along, because replicating the
/// edge replicates the absence of that source.
fn state_at(world: &HashMap<(i32, i32), Vec<Vec<i32>>>, x: i32, y: i32, z: i32) -> Option<i32> {
    let local_y = y - MIN_Y;
    let sections = world.get(&(x.div_euclid(16), z.div_euclid(16)))?;
    let height = i32::try_from(sections.len()).ok()? * 16;
    if local_y < 0 || local_y >= height {
        return None;
    }
    let local_x = x.rem_euclid(16);
    let local_z = z.rem_euclid(16);
    let section = usize::try_from(local_y / 16).ok()?;
    let cell = usize::try_from(local_y % 16 * 256 + local_z * 16 + local_x).ok()?;
    sections.get(section)?.get(cell).copied()
}

/// Compare one layer, returning `(matching, total, worst difference, histogram of differences)`.
fn compare(
    ours: &[mc_world::light::LightArray],
    theirs: &[Vec<u8>],
    mask: &[u32],
) -> (usize, usize, u8) {
    let mut matching = 0usize;
    let mut total = 0usize;
    let mut worst = 0u8;
    for (array, section) in theirs.iter().zip(mask.iter()) {
        // Light section `i` is world section `i - 1`.
        let Some(world_section) = section.checked_sub(1).map(|index| index as usize) else {
            continue;
        };
        let Some(ours) = ours.get(world_section) else {
            continue;
        };
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    let cell = usize::try_from(x + z * 16 + y * 256).expect("within a section");
                    let mine = ours.get(x, y, z);
                    let vanilla = {
                        let byte = array[cell / 2];
                        if cell % 2 == 0 {
                            byte & 0x0F
                        } else {
                            byte >> 4
                        }
                    };
                    total += 1;
                    if mine == vanilla {
                        matching += 1;
                    } else {
                        worst = worst.max(mine.abs_diff(vanilla));
                    }
                }
            }
        }
    }
    (matching, total, worst)
}

/// Print, per array, how one layer compares — because an aggregate hides *which* section differs, and
/// "which" is the diagnosis: a whole-array difference in one section is a different bug from scattered wrong
/// levels, and it was a whole-array difference that led to the unapplied `block` rows.
fn report_arrays(
    layer: &str,
    arrays: &[Vec<u8>],
    mask: &[u32],
    ours: &[mc_world::light::LightArray],
) {
    for (array, section) in arrays.iter().zip(mask.iter()) {
        let Some(world_section) = section.checked_sub(1).map(|index| index as usize) else {
            println!("    {layer} light section {section}: outside the world, skipped");
            continue;
        };
        let Some(theirs) = ours.get(world_section) else {
            continue;
        };
        let mut matching = 0usize;
        let mut mine_uniform: Option<u8> = None;
        let mut theirs_uniform: Option<u8> = None;
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    let cell = usize::try_from(x + z * 16 + y * 256).expect("within a section");
                    let mine = theirs.get(x, y, z);
                    let vanilla = nibble(array, cell);
                    if mine == vanilla {
                        matching += 1;
                    }
                    mine_uniform = Some(merge(mine_uniform, mine));
                    theirs_uniform = Some(merge(theirs_uniform, vanilla));
                }
            }
        }
        println!(
            "    {layer} light section {section} -> world section {world_section}: \
             {matching}/4096 match; ours uniform {mine_uniform:?}, vanilla uniform {theirs_uniform:?}"
        );
    }
}

/// `Some(level)` while every value seen equals `level`, then `Some(255)` once one differs.
fn merge(seen: Option<u8>, value: u8) -> u8 {
    match seen {
        None => value,
        Some(previous) if previous == value => previous,
        Some(_) => 255,
    }
}

#[test]
#[ignore = "needs the vanilla capture under target/, which is not committed"]
fn our_light_matches_vanillas_for_vanillas_own_world() {
    let registries = Registries::vanilla().expect("the shipped registry tables load");
    let files = captured_chunks();

    // Decode every captured chunk into one world, so the margin is answered by real neighbours rather than
    // approximated. The chunks under test are a prefix only to bound the run; the world map is all of them.
    let mut world: HashMap<(i32, i32), Vec<Vec<i32>>> = HashMap::new();
    let mut decoded: Vec<(PathBuf, LevelChunkWithLight)> = Vec::new();
    for path in &files {
        let body = std::fs::read(path).expect("read");
        let Ok(packet) = LevelChunkWithLight::decode(&body) else {
            continue;
        };
        let sections: Vec<Vec<i32>> = packet.sections.iter().map(section_cells).collect();
        world.insert((packet.chunk_x, packet.chunk_z), sections);
        decoded.push((path.clone(), packet));
    }
    println!("stitched world: {} chunks", world.len());

    let mut sky_matching = 0usize;
    let mut sky_total = 0usize;
    let mut sky_worst = 0u8;
    let mut block_matching = 0usize;
    let mut block_total = 0usize;
    let mut block_worst = 0u8;
    let mut chunks_compared = 0usize;

    for (path, packet) in decoded.iter().take(32) {
        let ours = compute_chunk_light(
            &registries.light,
            packet.chunk_x,
            packet.chunk_z,
            MIN_Y,
            packet.sections.len(),
            |x, y, z| state_at(&world, x, y, z),
        )
        .expect("computes");

        report_arrays("sky", &packet.sky_light, &packet.sky_light_mask, &ours.sky);
        report_arrays(
            "block",
            &packet.block_light,
            &packet.block_light_mask,
            &ours.block,
        );

        let (m, t, w) = compare(&ours.sky, &packet.sky_light, &packet.sky_light_mask);
        sky_matching += m;
        sky_total += t;
        sky_worst = sky_worst.max(w);
        let (m, t, w) = compare(&ours.block, &packet.block_light, &packet.block_light_mask);
        block_matching += m;
        block_total += t;
        block_worst = block_worst.max(w);
        chunks_compared += 1;

        println!(
            "{}: chunk ({}, {}) sections {} sky arrays {} block arrays {}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            packet.chunk_x,
            packet.chunk_z,
            packet.sections.len(),
            packet.sky_light.len(),
            packet.block_light.len()
        );
    }

    println!("\n=== sky light ===");
    println!("  cells compared : {sky_total}");
    println!("  matching       : {sky_matching}");
    println!("  worst difference: {sky_worst} (of a 0..={MAX_LIGHT} range)");

    println!("\n=== block light ===");
    println!("  cells compared : {block_total}");
    println!("  matching       : {block_matching}");
    println!("  worst difference: {block_worst}");

    println!("\nchunks compared: {chunks_compared}");

    // Exact, because that is what the run shows. A weaker bound would claim less than the evidence.
    assert!(
        sky_total > 0,
        "no sky arrays were compared, so the test proves nothing"
    );
    assert_eq!(
        sky_matching, sky_total,
        "our sky light must reproduce vanilla's for vanilla's own blocks, cell for cell"
    );
    assert_eq!(sky_worst, 0, "and no cell may differ by even one level");

    // The block layer was not exercised: vanilla sends no block-light arrays for a world with no light
    // sources, so there is nothing to compare against. Asserting this keeps the gap visible rather than
    // letting a reader take the sky result as covering both layers.
    // Block light is comparable **only** when the capture has light sources in it. Requiring it is what
    // stops a capture without them from passing while having tested nothing — which is exactly how the sky
    // result could otherwise be mistaken for covering both layers.
    assert!(
        block_total > 0,
        "this capture carries no block-light arrays, so block light is not being verified; capture a world \
         with light sources in it (see tools/vanilla-probe or target/p10_capture_lit.py)"
    );
    // Not exact, and the reason is named rather than the bound quietly widened. `compute_chunk_light`
    // reads a **one-block margin**, which lets light enter the region across a border but not travel several
    // blocks outside it first — so a source a few blocks into the next chunk arrives attenuated. Exactness
    // would need a margin of 15 (light loses at least one level per block, so nothing further can matter),
    // which is 6.5x the work per chunk against an engine that already recomputes everything on every send
    // (KD-45). The fix is to compute light over the **loaded world** rather than per chunk, which is the same
    // work as the caching and incremental relighting that gap already calls for.
    assert!(
        block_matching * 1000 >= block_total * 999,
        "block light must agree to within a tenth of a percent: {block_matching}/{block_total}"
    );
    assert!(
        block_worst <= 3,
        "disagreements must stay within the margin's reach, found {block_worst}"
    );
}
