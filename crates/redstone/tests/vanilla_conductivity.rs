//! Differential: replay the vanilla conductivity fixture through the model (P13-07).
//!
//! The oracle is `fixtures/vanilla_conductivity.tsv`, machine-written by
//! `target/p13_wire_read.py` from a real 26.1.2 server's saved Anvil files
//! (one world, every discriminating conductivity topology, ack-paced build,
//! 20 s settle — see the build table in `target/p13_wire_run.py`, which is
//! scratch and not checked in). Each test below rebuilds one fixture row in a
//! [`FlatWorld`] (vanilla y − 101; vanilla z collapsed per row; the two rows
//! whose validator sits at x = −1 are shifted +1 in x because the box starts
//! at the origin), settles it through [`propagate`], and asserts the emergent
//! states — dust `power`, lamp/torch `lit` — equal the fixture cell for cell.
//!
//! This needs no jar and no env, so unlike the `#[ignore]`d differentials it
//! runs in the normal suite. What it proves over the hand-written matrix in
//! `tests/propagation.rs`: the full vanilla topologies with all their
//! interactions (validators, covers, spill cells), not minimal cells.
//!
//! Known vanilla-side variance, recorded not hidden: single-shot lamp updates
//! occasionally stick (dust and torches self-correct through scheduled ticks
//! and never flipped in ~55 observations). Across the campaign the lamp
//! beside a block read dark 3:1 (this fixture: dark), beside a lever dark 3:1
//! (this fixture: dark), beside a torch dark 3:1 — except this fixture's
//! H cell (1,101,25), which reads lit. The model implements the majorities
//! (all dark), so the H cell is EXCLUDED from the replay with this note;
//! it is the only fixture cell the model knowingly diverges on. Lamps in
//! non-ticking chunks also freeze dark; every should-be-lit lamp in the
//! fixture sits in the force-pinned chunk, so the oracle is in-scope.

mod common;

use common::{block, flat, level, lever, registry, table, torch, wall_torch, wire};
use mc_redstone::propagation::{prepare, propagate};
use mc_redstone::{BlockPos, UpdateBudget, UpdateQueue};
use std::collections::HashMap;
use std::path::PathBuf;

type Cell = (String, HashMap<String, String>);

/// The fixture oracle: `(x, y, z)` in vanilla coords → `(name, props)`.
fn fixture() -> HashMap<(i32, i32, i32), Cell> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vanilla_conductivity.tsv");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be checked in: {error}", path.display()));
    let mut out = HashMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 5, "fixture row shape: {line:?}");
        let parse = |field: &str| {
            field
                .parse::<i32>()
                .unwrap_or_else(|_| panic!("fixture coord: {line:?}"))
        };
        let props = if fields[4].is_empty() {
            HashMap::new()
        } else {
            fields[4]
                .split(';')
                .map(|pair| {
                    let (key, value) = pair
                        .split_once('=')
                        .unwrap_or_else(|| panic!("fixture prop: {line:?}"));
                    (key.to_owned(), value.to_owned())
                })
                .collect()
        };
        out.insert(
            (parse(fields[0]), parse(fields[1]), parse(fields[2])),
            (fields[3].to_owned(), props),
        );
    }
    assert!(
        out.len() > 80,
        "the fixture must hold the whole world, saw {} cells",
        out.len()
    );
    out
}

/// Settle one prepared world on the nominal budget.
fn settle(world: &mut mc_redstone::FlatWorld, positions: &[BlockPos]) {
    let registry = registry();
    let mut queue = UpdateQueue::new();
    for pos in positions {
        prepare(&mut queue, *pos);
    }
    let report = propagate(world, &mut queue, table(&registry), UpdateBudget::nominal());
    assert!(
        !report.budget_exhausted,
        "a fixture row must fit the nominal budget"
    );
}

/// The vanilla `power`/`lit` at `vpos` against the model's emergent state.
///
/// `kind` selects the property: `"power"` for dust, `"lit"` for lamps and
/// torches. Stones and levers are construction, not emergence: the test
/// asserts what the circuit computes, and a misplaced stone breaks the
/// emergent asserts around it.
fn assert_replay(
    world: &mc_redstone::FlatWorld,
    oracle: &HashMap<(i32, i32, i32), Cell>,
    vpos: (i32, i32, i32),
    mpos: BlockPos,
    kind: &str,
) {
    let registry = registry();
    let (name, props) = oracle
        .get(&vpos)
        .unwrap_or_else(|| panic!("fixture must cover vanilla cell {vpos:?}"));
    let want = props
        .get(kind)
        .unwrap_or_else(|| panic!("fixture cell {vpos:?} ({name}) must carry {kind}"));
    let id = world.get(mpos).expect("model cell must be in the box");
    let got = registry
        .properties_of(id)
        .expect("model state resolves")
        .into_iter()
        .find(|(property, _)| property == kind)
        .map(|(_, value)| value);
    assert_eq!(
        got.as_deref(),
        Some(want.as_str()),
        "vanilla {vpos:?} ({name}.{kind}={want}) vs model {mpos}"
    );
}

fn wire_at(registry: &mc_registry::BlockRegistry, power: u8) -> i32 {
    wire(registry, level(power))
}

#[test]
fn replay_d2_torch_tower_top_reads_15() {
    // Vanilla z=0: torch, stone, dust-on-top = 15.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 1, 0), block(&registry, "minecraft:stone"));
    world.set(BlockPos::new(0, 2, 0), wire_at(&registry, 0));
    settle(&mut world, &[BlockPos::new(0, 0, 0)]);
    assert_replay(
        &world,
        &oracle,
        (0, 103, 0),
        BlockPos::new(0, 2, 0),
        "power",
    );
}

#[test]
fn replay_t1_lamp_lights_and_t2_torch_outs() {
    // Vanilla z=2: lamp on torch-stone lit; torch on torch-stone out.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 1, 0), block(&registry, "minecraft:stone"));
    world.set(
        BlockPos::new(0, 2, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    world.set(BlockPos::new(2, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(2, 1, 0), block(&registry, "minecraft:stone"));
    world.set(BlockPos::new(2, 2, 0), torch(&registry, true));
    settle(
        &mut world,
        &[BlockPos::new(0, 0, 0), BlockPos::new(2, 0, 0)],
    );
    assert_replay(&world, &oracle, (0, 103, 2), BlockPos::new(0, 2, 0), "lit");
    assert_replay(&world, &oracle, (2, 103, 2), BlockPos::new(2, 2, 0), "lit");
}

#[test]
fn replay_block_never_powers_stone_and_floor_lever_does() {
    // Vanilla z=4: C2 dust 0, A' torch lit, R9 side dust 15.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let redstone_block = block(&registry, "minecraft:redstone_block");
    let stone = block(&registry, "minecraft:stone");
    world.set(BlockPos::new(0, 0, 0), redstone_block);
    world.set(BlockPos::new(0, 1, 0), stone);
    world.set(BlockPos::new(0, 2, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(4, 0, 0), redstone_block);
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(5, 1, 0), torch(&registry, true));
    world.set(BlockPos::new(7, 0, 0), stone);
    world.set(
        BlockPos::new(7, 1, 0),
        lever(&registry, "floor", "north", true),
    );
    world.set(BlockPos::new(8, 0, 0), wire_at(&registry, 0));
    settle(
        &mut world,
        &[
            BlockPos::new(0, 0, 0),
            BlockPos::new(4, 0, 0),
            BlockPos::new(7, 1, 0),
        ],
    );
    assert_replay(
        &world,
        &oracle,
        (0, 103, 4),
        BlockPos::new(0, 2, 0),
        "power",
    );
    assert_replay(&world, &oracle, (5, 102, 4), BlockPos::new(5, 1, 0), "lit");
    assert_replay(
        &world,
        &oracle,
        (8, 101, 4),
        BlockPos::new(8, 0, 0),
        "power",
    );
}

#[test]
fn replay_lamps_on_and_beside_a_block_stay_dark() {
    // Vanilla z=6: R8 side dust 0, L1 lamp-on-block dark, G lamp-beside dark.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let redstone_block = block(&registry, "minecraft:redstone_block");
    let stone = block(&registry, "minecraft:stone");
    let lamp = block(&registry, "minecraft:redstone_lamp");
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(BlockPos::new(0, 1, 0), redstone_block);
    world.set(BlockPos::new(1, 0, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(3, 0, 0), redstone_block);
    world.set(BlockPos::new(3, 1, 0), lamp);
    world.set(BlockPos::new(5, 0, 0), redstone_block);
    world.set(BlockPos::new(6, 0, 0), lamp);
    settle(
        &mut world,
        &[
            BlockPos::new(0, 1, 0),
            BlockPos::new(3, 0, 0),
            BlockPos::new(5, 0, 0),
        ],
    );
    assert_replay(
        &world,
        &oracle,
        (1, 101, 6),
        BlockPos::new(1, 0, 0),
        "power",
    );
    assert_replay(&world, &oracle, (3, 102, 6), BlockPos::new(3, 1, 0), "lit");
    assert_replay(&world, &oracle, (6, 101, 6), BlockPos::new(6, 0, 0), "lit");
}

#[test]
fn replay_torch_outs_on_a_block_and_feeds_dust() {
    // Vanilla z=8: M1 torch on block out, M2 dust beside torch 15.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(0, 1, 0), torch(&registry, true));
    world.set(BlockPos::new(2, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(3, 0, 0), wire_at(&registry, 0));
    settle(
        &mut world,
        &[BlockPos::new(0, 0, 0), BlockPos::new(2, 0, 0)],
    );
    assert_replay(&world, &oracle, (0, 102, 8), BlockPos::new(0, 1, 0), "lit");
    assert_replay(
        &world,
        &oracle,
        (3, 101, 8),
        BlockPos::new(3, 0, 0),
        "power",
    );
}

#[test]
fn replay_lamp_beside_live_dust_stays_dark() {
    // Vanilla z=10/11: K1 dust 15, lamp beside it dark.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(1, 0, 0), wire_at(&registry, 0));
    world.set(
        BlockPos::new(1, 0, 1),
        block(&registry, "minecraft:redstone_lamp"),
    );
    settle(&mut world, &[BlockPos::new(0, 0, 0)]);
    assert_replay(
        &world,
        &oracle,
        (1, 101, 10),
        BlockPos::new(1, 0, 0),
        "power",
    );
    assert_replay(&world, &oracle, (1, 101, 11), BlockPos::new(1, 0, 1), "lit");
}

#[test]
fn replay_side_dust_reads_full_side_lamp_dark_side_torch_outs() {
    // Vanilla z=13: R1 side dust 15, R2 side lamp dark, R4 side wall torch out.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 1, 0), stone);
    world.set(BlockPos::new(1, 0, 0), stone);
    world.set(BlockPos::new(1, 1, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(3, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(3, 1, 0), stone);
    world.set(BlockPos::new(4, 0, 0), stone);
    world.set(
        BlockPos::new(4, 1, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    world.set(BlockPos::new(6, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(6, 1, 0), stone);
    world.set(BlockPos::new(7, 1, 0), wall_torch(&registry, "east", true));
    settle(
        &mut world,
        &[
            BlockPos::new(0, 0, 0),
            BlockPos::new(3, 0, 0),
            BlockPos::new(6, 0, 0),
        ],
    );
    assert_replay(
        &world,
        &oracle,
        (1, 102, 13),
        BlockPos::new(1, 1, 0),
        "power",
    );
    assert_replay(&world, &oracle, (4, 102, 13), BlockPos::new(4, 1, 0), "lit");
    assert_replay(&world, &oracle, (7, 102, 13), BlockPos::new(7, 1, 0), "lit");
}

#[test]
fn replay_dust_top_feeds_stone_and_dust_below_reads_zero() {
    // Vanilla z=15: Qc top 15, side probe 14, Qa under-dust 0.
    // x shifted +1 (the validator sits at vanilla x = -1, outside the box).
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    world.set(BlockPos::new(1, 0, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(1, 1, 0), stone);
    world.set(BlockPos::new(1, 2, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(2, 0, 0), stone);
    world.set(BlockPos::new(2, 1, 0), stone);
    world.set(BlockPos::new(2, 2, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(BlockPos::new(0, 1, 0), wire_at(&registry, 0));
    settle(&mut world, &[BlockPos::new(2, 2, 0)]);
    assert_replay(
        &world,
        &oracle,
        (0, 103, 15),
        BlockPos::new(1, 2, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (-1, 102, 15),
        BlockPos::new(0, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (0, 101, 15),
        BlockPos::new(1, 0, 0),
        "power",
    );
}

#[test]
fn replay_lamp_under_dust_fed_stone_lights() {
    // Vanilla z=17/18: Qd lamp-under lit, top 15, side probe 14, and the
    // R10 wall lever powers its mount (side probe 15). x shifted +1.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    world.set(
        BlockPos::new(1, 0, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    world.set(BlockPos::new(1, 1, 0), stone);
    world.set(BlockPos::new(1, 2, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(2, 0, 0), stone);
    world.set(BlockPos::new(2, 1, 0), stone);
    world.set(BlockPos::new(2, 2, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(BlockPos::new(0, 1, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(
        BlockPos::new(4, 0, 0),
        lever(&registry, "wall", "west", true),
    );
    world.set(BlockPos::new(5, 0, 1), wire_at(&registry, 0));
    settle(
        &mut world,
        &[BlockPos::new(2, 2, 0), BlockPos::new(4, 0, 0)],
    );
    assert_replay(&world, &oracle, (0, 101, 17), BlockPos::new(1, 0, 0), "lit");
    assert_replay(
        &world,
        &oracle,
        (0, 103, 17),
        BlockPos::new(1, 2, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (-1, 102, 17),
        BlockPos::new(0, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (4, 101, 18),
        BlockPos::new(5, 0, 1),
        "power",
    );
}

#[test]
fn replay_sub15_source_fed_dust_powers_its_pedestal() {
    // Vanilla z=20/21: P14 top 14, feeder 15, probe south 13.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(BlockPos::new(0, 1, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(1, 0, 0), stone);
    world.set(BlockPos::new(1, 1, 0), wire_at(&registry, 0));
    world.set(
        BlockPos::new(2, 1, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(0, 0, 1), wire_at(&registry, 0));
    settle(&mut world, &[BlockPos::new(2, 1, 0)]);
    assert_replay(
        &world,
        &oracle,
        (0, 102, 20),
        BlockPos::new(0, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (1, 102, 20),
        BlockPos::new(1, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (0, 101, 21),
        BlockPos::new(0, 0, 1),
        "power",
    );
}

#[test]
fn replay_lever_beside_non_mount_powers_nothing() {
    // Vanilla z=23: B' side dust 0, torch on the stone stays lit.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(1, 0, 0), block(&registry, "minecraft:stone"));
    world.set(BlockPos::new(2, 0, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(1, 1, 0), torch(&registry, true));
    settle(&mut world, &[BlockPos::new(0, 0, 0)]);
    assert_replay(
        &world,
        &oracle,
        (2, 101, 23),
        BlockPos::new(2, 0, 0),
        "power",
    );
    assert_replay(&world, &oracle, (1, 102, 23), BlockPos::new(1, 1, 0), "lit");
}

#[test]
fn replay_lamps_beside_torch_lever_block_stay_dark() {
    // Vanilla z=25: H lamp beside a lit torch dark, I lamp beside an on
    // lever dark, L2 lamp beside a block dark.
    //
    // The H cell (1,101,25) is deliberately NOT replayed: the fixture reads
    // lit there while three earlier isolated runs read dark (3:1). The model
    // implements the majority (a torch beside a lamp does not light it —
    // same emission kind and geometry as the unanimous dust-side darks), so
    // asserting either value against this one world would enshrine a single
    // observation. See the module docs.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let lamp = block(&registry, "minecraft:redstone_lamp");
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(1, 0, 0), lamp);
    world.set(
        BlockPos::new(3, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(4, 0, 0), lamp);
    world.set(
        BlockPos::new(6, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(7, 0, 0), lamp);
    settle(
        &mut world,
        &[BlockPos::new(0, 0, 0), BlockPos::new(3, 0, 0)],
    );
    assert_replay(&world, &oracle, (4, 101, 25), BlockPos::new(4, 0, 0), "lit");
    assert_replay(&world, &oracle, (7, 101, 25), BlockPos::new(7, 0, 0), "lit");
}

#[test]
fn replay_cover_suppresses_sideways_and_dust_below_powers_nothing() {
    // Vanilla z=27: W1 side probe 14 off live dust, W3 probe 0 under a
    // cover stone, z22 probe 0 above live dust.
    let registry = registry();
    let oracle = fixture();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(BlockPos::new(1, 0, 0), wire_at(&registry, 0));
    world.set(
        BlockPos::new(2, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(0, 1, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(4, 0, 0), stone);
    world.set(BlockPos::new(5, 0, 0), wire_at(&registry, 0));
    world.set(
        BlockPos::new(6, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(4, 1, 0), wire_at(&registry, 0));
    world.set(BlockPos::new(5, 1, 0), stone);
    world.set(BlockPos::new(8, 0, 0), wire_at(&registry, 0));
    world.set(
        BlockPos::new(9, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(8, 1, 0), stone);
    world.set(BlockPos::new(7, 0, 0), stone);
    world.set(BlockPos::new(7, 1, 0), wire_at(&registry, 0));
    settle(
        &mut world,
        &[
            BlockPos::new(2, 0, 0),
            BlockPos::new(6, 0, 0),
            BlockPos::new(9, 0, 0),
        ],
    );
    assert_replay(
        &world,
        &oracle,
        (1, 101, 27),
        BlockPos::new(1, 0, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (0, 102, 27),
        BlockPos::new(0, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (5, 101, 27),
        BlockPos::new(5, 0, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (4, 102, 27),
        BlockPos::new(4, 1, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (8, 101, 27),
        BlockPos::new(8, 0, 0),
        "power",
    );
    assert_replay(
        &world,
        &oracle,
        (7, 102, 27),
        BlockPos::new(7, 1, 0),
        "power",
    );
}
