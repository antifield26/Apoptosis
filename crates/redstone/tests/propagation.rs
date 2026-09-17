//! Propagation acceptance: attenuation over a line, the conductivity matrix, and
//! exhaustion from a real circuit.
//!
//! ## What is being tested, and what is *not*
//!
//! `mc-redstone` does **not** claim Vanilla's update order, its block-update/shape-update
//! split, or its component timing (see `propagation`'s module docs). What it
//! does claim is:
//!
//! - the wire attenuation rule (verified: minecraft.wiki, *Redstone mechanics* §"Signal
//!   transmission" — "the signal strength decreases by 1 for every block of redstone dust
//!   that the signal travels … up to 15 blocks by itself"), and
//! - the conductivity matrix (measured on a real 26.1.2 server, P13-06): which
//!   neighbour powers a solid, in which direction, strongly or weakly — and
//!   which neighbours a lamp reads.
//!
//! The tests whose subject is the *implemented* rule rather than Vanilla's are named
//! `our_…`, so nothing here can be mistaken for a parity claim.

mod common;

use common::{block, component, flat, is_torch, level, lever, registry, table, torch, wire};
use mc_redstone::components::{
    Comparator, ComparatorMode, ComponentState, Lever, RedstoneTorch, Repeater,
    clamp_repeater_delay,
};
use mc_redstone::propagation::{
    BlockRole, prepare, prepare_self, propagate, run_block_tick, schedule_wire_recheck,
};
use mc_redstone::{
    BlockPos, EmitterOutput, MAX_POWER, PowerLevel, PowerSource, UpdateBudget, UpdateQueue,
    WIRE_ATTENUATION_PER_BLOCK, WIRE_LIVE_BLOCKS,
};

/// A horizontal line of `count` wire blocks starting at `start`, one per block eastwards.
fn wire_line(count: i32) -> Vec<BlockPos> {
    (0..count)
        .map(|step| BlockPos::new(1 + step, 0, 0))
        .collect()
}

/// The wire emission at `pos`: what a neighbour of `pos` reads from it.
///
/// This is the model's answer for a wire — the level stored in its `power` property — read
/// through the same classification an update uses, so this is not a second implementation of
/// the rule.
fn wire_emission(
    world: &mc_redstone::FlatWorld,
    registry: &mc_registry::BlockRegistry,
    pos: BlockPos,
) -> EmitterOutput {
    let table = table(registry);
    match world.get(pos).map(|id| table.classify(id)) {
        Some(BlockRole::Wire { stored }) => {
            EmitterOutput::of(mc_redstone::PowerState::weak_only(stored))
        }
        other => panic!("{pos} is not redstone wire in this scenario: {other:?}"),
    }
}

/// Build the 15-wire line with a `source` at `(0, 0, 0)` and propagate once.
fn line_with_source(
    source: i32,
    count: i32,
) -> (
    mc_redstone::FlatWorld,
    mc_registry::BlockRegistry,
    Vec<BlockPos>,
) {
    let registry = registry();
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), source);
    for pos in wire_line(count) {
        world.set(pos, wire(&registry, PowerLevel::ZERO));
    }
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(
        &mut world,
        &mut queue,
        table(&registry),
        UpdateBudget::nominal(),
    );
    assert!(
        !report.budget_exhausted,
        "a 15-block line must fit the nominal budget"
    );
    (world, registry, wire_line(count))
}

#[test]
fn the_attenuation_rule_is_the_documented_one() {
    assert_eq!(WIRE_ATTENUATION_PER_BLOCK, 1);
    // The *live* reach is 15 / 1 = 15: the first dust off a source carries the
    // full strength (P13-05, measured on a real 26.1.2 server), then one per
    // block. `WIRE_LIVE_BLOCKS` documents the measurement.
    assert_eq!(WIRE_LIVE_BLOCKS, 15);
    assert_eq!(
        WIRE_LIVE_BLOCKS,
        u32::from(MAX_POWER) / u32::from(WIRE_ATTENUATION_PER_BLOCK)
    );
}

#[test]
fn power_attenuates_by_exactly_one_per_block_from_a_strong_source() {
    // A block of redstone at (0,0,0), wire at x = 1..=15. Hand-computed expectation:
    //   wire(x) = 16 - x   for x in 1..=15
    // so wire(1) = 15 and wire(15) = 1. The value at x = 0 is the source itself.
    let registry = registry();
    let (world, _, positions) = line_with_source(block(&registry, "minecraft:redstone_block"), 15);
    let expected: Vec<(BlockPos, u8)> = (0..15)
        .map(|index| {
            let x = index + 1;
            (
                BlockPos::new(x, 0, 0),
                16 - u8::try_from(x).expect("x fits in u8"),
            )
        })
        .collect();
    common::assert_wire_powers(&world, &registry, &expected);
    assert_eq!(positions.len(), 15);
    // Spelled out per position as well, so a reader can check the arithmetic without
    // replaying the closure.
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
            .effective()
            .get(),
        15
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(2, 0, 0))
            .effective()
            .get(),
        14
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(7, 0, 0))
            .effective()
            .get(),
        9
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(14, 0, 0))
            .effective()
            .get(),
        2
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(15, 0, 0))
            .effective()
            .get(),
        1,
        "15 blocks from the source is the measured end of the signal (P13-05)"
    );
    // The source itself is unaffected: a redstone block is not a wire.
    assert!(matches!(
        table(&registry).classify(world.get(BlockPos::new(0, 0, 0)).expect("source")),
        BlockRole::Emitter {
            source: PowerSource::RedstoneBlock
        }
    ));
}

#[test]
fn the_signal_reaches_exactly_fifteen_live_wire_blocks_and_no_further() {
    // The reach is a property of the *chain*, so it is tested on a line longer than the
    // reach. Hand-computed expectation for a 15-strength source at (0,0,0):
    //   wire(x) = 16 - x, so wire(15) = 1 is the last live block and wire(16) = 0.
    // 15 live blocks, which is `WIRE_LIVE_BLOCKS`, matching the wiki's sentence
    // and the P13-05 vanilla measurement.
    let registry = registry();
    let (world, _, _) = line_with_source(block(&registry, "minecraft:redstone_block"), 20);
    let live: Vec<i32> = wire_line(20)
        .into_iter()
        .filter(|pos| {
            wire_emission(&world, &registry, *pos)
                .effective()
                .is_powered()
        })
        .map(|pos| pos.x)
        .collect();
    assert_eq!(
        live,
        (1..=15).collect::<Vec<i32>>(),
        "exactly the first 15 wire blocks carry a signal"
    );
    assert_eq!(u32::try_from(live.len()).expect("fits"), WIRE_LIVE_BLOCKS);
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(15, 0, 0))
            .effective()
            .get(),
        1,
        "the last live block carries the weakest possible signal"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(16, 0, 0)).effective(),
        PowerLevel::ZERO,
        "and the next one is off"
    );
}

/// Propagate one prepared position on the nominal budget, refusing exhaustion.
fn settle(
    world: &mut mc_redstone::FlatWorld,
    registry: &mc_registry::BlockRegistry,
    pos: BlockPos,
) {
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, pos);
    let report = propagate(world, &mut queue, table(registry), UpdateBudget::nominal());
    assert!(
        !report.budget_exhausted,
        "this circuit must fit the nominal budget"
    );
}

/// Whether the lamp at `pos` is lit, read through the registry.
fn lit_at(
    world: &mc_redstone::FlatWorld,
    registry: &mc_registry::BlockRegistry,
    pos: BlockPos,
) -> bool {
    let id = world.get(pos).expect("lamp must be in the box");
    registry
        .properties_of(id)
        .expect("lamp state resolves")
        .into_iter()
        .any(|(name, value)| name == "lit" && value == "true")
}

/// A wall torch facing `facing`, lit or not.
fn wall_torch(registry: &mc_registry::BlockRegistry, facing: &str, lit: bool) -> i32 {
    registry
        .state_id(
            "minecraft:redstone_wall_torch",
            &[
                ("facing".to_owned(), facing.to_owned()),
                ("lit".to_owned(), lit.to_string()),
            ],
        )
        .unwrap_or_else(|error| panic!("wall torch {facing}/{lit} must resolve: {error}"))
}

#[test]
fn our_direct_emission_kind_is_recorded_but_solids_follow_position() {
    // **The honest state of the strong/weak split in this pass**, asserted rather than
    // implied:
    //
    // - A block of redstone emits strongly to adjacent dust and a lever weakly,
    //   and both put 15 on the wire next to them (the wire rule takes the
    //   maximum either way).
    // - Which kind a *solid* carries is positional (P13-06, measured), not a
    //   property of the source kind: a block of redstone never powers an
    //   adjacent solid, and a lever only powers its mount — both checked
    //   below and in the matrix tests.
    let registry = registry();
    let table = table(&registry);

    let powers_for = |source: i32| {
        let mut world = flat();
        world.set(BlockPos::new(0, 0, 0), source);
        world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
        let mut queue = UpdateQueue::new();
        prepare(&mut queue, BlockPos::new(0, 0, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        assert!(!report.budget_exhausted);
        let emission = wire_emission(&world, &registry, BlockPos::new(1, 0, 0));
        let source_kind = table.emitted_for_id(source, &mc_redstone::BlockInputs::NONE);
        assert!(
            matches!(table.classify(source), BlockRole::Emitter { .. }),
            "the source must be an emitter"
        );
        (emission.effective().get(), source_kind.is_strong())
    };

    let (from_redstone_block, redstone_block_is_strong) =
        powers_for(block(&registry, "minecraft:redstone_block"));
    let (from_lever, lever_is_strong) = powers_for(component(
        &registry,
        ComponentState::Lever(Lever::new(true)),
    ));

    assert!(
        redstone_block_is_strong,
        "a block of redstone is the strong source"
    );
    assert!(!lever_is_strong, "a lever is a weak source in this model");
    assert_eq!(
        from_redstone_block, 15,
        "dust beside a redstone block carries 15"
    );
    assert_eq!(
        from_lever, 15,
        "dust beside a lever also carries 15: the wire rule takes the maximum \
         of weak and strong"
    );
    // And neither powers the stone beside it: that follows position, and the
    // matrix tests pin every cell.
    for source in [
        block(&registry, "minecraft:redstone_block"),
        lever(&registry, "wall", "north", true),
    ] {
        let mut world = flat();
        world.set(BlockPos::new(0, 0, 0), source);
        world.set(BlockPos::new(1, 0, 0), block(&registry, "minecraft:stone"));
        settle(&mut world, &registry, BlockPos::new(0, 0, 0));
        assert_eq!(
            mc_redstone::propagation::solid_power(&world, table, BlockPos::new(1, 0, 0), None),
            None,
            "no solid power beside a block or a non-mount lever"
        );
    }
}

#[test]
fn torch_powers_only_the_stone_above_it() {
    // P13-06, measured rows D2/R1/T1/T2 (hot) and R6/Q-b/z14 (cold). Lanes
    // are 4 apart so no lane reads another; every probe is diagonal-or-far
    // from every source but its stone.
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    // D2: dust on top of a torch-powered stone reads 15.
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 1, 0), stone);
    world.set(BlockPos::new(0, 2, 0), wire(&registry, PowerLevel::ZERO));
    // R1: dust beside a torch-powered stone reads 15.
    world.set(BlockPos::new(4, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(4, 1, 0), stone);
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(5, 1, 0), wire(&registry, PowerLevel::ZERO));
    // T1: a lamp on a torch-powered stone lights.
    world.set(BlockPos::new(8, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(8, 1, 0), stone);
    world.set(
        BlockPos::new(8, 2, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    // T2: a torch on a torch-powered stone goes out.
    world.set(BlockPos::new(12, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(12, 1, 0), stone);
    world.set(BlockPos::new(12, 2, 0), torch(&registry, true));
    // R6: a torch above a stone does not power it (side probe dark).
    world.set(BlockPos::new(16, 0, 0), stone);
    world.set(BlockPos::new(16, 1, 0), torch(&registry, true));
    world.set(BlockPos::new(17, 0, 0), wire(&registry, PowerLevel::ZERO));
    // Q-b: a torch beside a stone does not power it (top probe dark).
    world.set(BlockPos::new(20, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(21, 0, 0), stone);
    world.set(BlockPos::new(21, 1, 0), wire(&registry, PowerLevel::ZERO));
    // z14: a wall torch attached to a stone does not power it, and stays lit.
    world.set(BlockPos::new(24, 1, 0), stone);
    world.set(BlockPos::new(25, 1, 0), wall_torch(&registry, "east", true));
    world.set(BlockPos::new(23, 1, 0), wire(&registry, PowerLevel::ZERO));
    for x in [0, 4, 8, 12, 16, 20, 24] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
    }
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(0, 2, 0))
            .effective()
            .get(),
        15,
        "D2: dust on top of torch-powered stone reads 15"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(5, 1, 0))
            .effective()
            .get(),
        15,
        "R1: dust beside torch-powered stone reads 15"
    );
    assert!(
        lit_at(&world, &registry, BlockPos::new(8, 2, 0)),
        "T1: lamp on torch-powered stone lights"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(12, 2, 0)),
        "T2: torch on torch-powered stone goes out"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(17, 0, 0)).effective(),
        PowerLevel::ZERO,
        "R6: torch above does not power the stone"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(21, 1, 0)).effective(),
        PowerLevel::ZERO,
        "Q-b: torch beside does not power the stone"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(23, 1, 0)).effective(),
        PowerLevel::ZERO,
        "z14: attached torch does not power the stone"
    );
    assert!(
        lit_at(&world, &registry, BlockPos::new(25, 1, 0)),
        "z14: torch on a cold stone stays lit"
    );
}

#[test]
fn lever_powers_only_its_mount() {
    // P13-06, measured rows R9 (floor), R10 (wall mount) and B' (side).
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    // R9: a floor lever powers the stone below it, strongly.
    world.set(BlockPos::new(0, 0, 0), stone);
    world.set(
        BlockPos::new(0, 1, 0),
        lever(&registry, "floor", "north", true),
    );
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    // R10: a wall lever powers the stone it is mounted on, strongly.
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(
        BlockPos::new(4, 0, 0),
        lever(&registry, "wall", "west", true),
    );
    world.set(BlockPos::new(5, 0, 1), wire(&registry, PowerLevel::ZERO));
    // B': a lever beside a stone it is not mounted on powers nothing; a
    // torch on that stone stays lit.
    world.set(
        BlockPos::new(8, 0, 0),
        lever(&registry, "wall", "north", true),
    );
    world.set(BlockPos::new(9, 0, 0), stone);
    world.set(BlockPos::new(10, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(9, 1, 0), torch(&registry, true));
    // An off lever powers nothing even through its mount.
    world.set(BlockPos::new(12, 0, 0), stone);
    world.set(
        BlockPos::new(12, 1, 0),
        lever(&registry, "floor", "north", false),
    );
    world.set(BlockPos::new(13, 0, 0), wire(&registry, PowerLevel::ZERO));
    for x in [0, 4, 8, 12] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
    }
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
            .effective()
            .get(),
        15,
        "R9: dust beside a lever-powered mount reads the full level"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(5, 0, 1))
            .effective()
            .get(),
        15,
        "R10: dust beside a wall-lever mount reads the full level"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(10, 0, 0)).effective(),
        PowerLevel::ZERO,
        "B': dust beside a non-mount stone stays dark"
    );
    assert!(
        lit_at(&world, &registry, BlockPos::new(9, 1, 0)),
        "B': torch on a non-mount stone stays lit"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(13, 0, 0)).effective(),
        PowerLevel::ZERO,
        "an off lever powers nothing"
    );
}

#[test]
fn a_redstone_block_never_powers_stone() {
    // P13-06, measured rows C2 (above), A' (side) and R8 (below): all dark.
    // The block still drives adjacent dust and torches directly (S, M1).
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    let redstone_block = block(&registry, "minecraft:redstone_block");
    // C2: dust on top of a block-topped stone reads 0.
    world.set(BlockPos::new(0, 0, 0), redstone_block);
    world.set(BlockPos::new(0, 1, 0), stone);
    world.set(BlockPos::new(0, 2, 0), wire(&registry, PowerLevel::ZERO));
    // A': a torch on a block-beside stone stays lit.
    world.set(BlockPos::new(4, 0, 0), redstone_block);
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(5, 1, 0), torch(&registry, true));
    // R8: dust beside a block-based stone reads 0.
    world.set(BlockPos::new(8, 0, 0), stone);
    world.set(BlockPos::new(8, 1, 0), redstone_block);
    world.set(BlockPos::new(9, 0, 0), wire(&registry, PowerLevel::ZERO));
    // S: dust directly on top of the block reads 15.
    world.set(BlockPos::new(12, 0, 0), redstone_block);
    world.set(BlockPos::new(12, 1, 0), wire(&registry, PowerLevel::ZERO));
    // M1: a torch directly on top of the block goes out.
    world.set(BlockPos::new(16, 0, 0), redstone_block);
    world.set(BlockPos::new(16, 1, 0), torch(&registry, true));
    for x in [0, 4, 8, 12, 16] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
    }
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(0, 2, 0)).effective(),
        PowerLevel::ZERO,
        "C2: block below does not power the stone"
    );
    assert!(
        lit_at(&world, &registry, BlockPos::new(5, 1, 0)),
        "A': block beside does not power the stone"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(9, 0, 0)).effective(),
        PowerLevel::ZERO,
        "R8: block above does not power the stone"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(12, 1, 0))
            .effective()
            .get(),
        15,
        "S: dust directly on the block reads 15"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(16, 1, 0)),
        "M1: torch directly on the block goes out"
    );
}

#[test]
fn dust_powers_stone_beneath_and_beside_it_but_not_from_below() {
    // P13-06, measured rows Q-c (top feeds, side probe 14), z=20 (side feeds,
    // top probe 14) and z=22 (dust below feeds nothing). Dust-fed stone is
    // weak: the probe steps down one more.
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    // Q-c: live dust on a stone, side probe reads 14.
    world.set(BlockPos::new(4, 1, 0), stone);
    world.set(BlockPos::new(4, 2, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(5, 1, 0), stone);
    world.set(BlockPos::new(5, 2, 0), torch(&registry, true));
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(3, 0, 0), stone);
    world.set(BlockPos::new(3, 1, 0), wire(&registry, PowerLevel::ZERO));
    // z=20: live dust beside a stone, top probe reads 14.
    world.set(BlockPos::new(10, 0, 0), stone);
    world.set(BlockPos::new(11, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(
        BlockPos::new(12, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(10, 1, 0), wire(&registry, PowerLevel::ZERO));
    // z=22: live dust below a stone, side probe reads 0.
    world.set(BlockPos::new(16, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(
        BlockPos::new(17, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(16, 1, 0), stone);
    world.set(BlockPos::new(15, 0, 0), stone);
    world.set(BlockPos::new(15, 1, 0), wire(&registry, PowerLevel::ZERO));
    // W3: the z=20 shape with a cover stone on the dust — the cover
    // suppresses the sideways powering, so the top probe reads 0.
    world.set(BlockPos::new(20, 0, 0), stone);
    world.set(BlockPos::new(21, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(
        BlockPos::new(22, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(20, 1, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(21, 1, 0), stone);
    for x in [4, 5, 10, 12, 16, 17, 20, 21, 22] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
        settle(&mut world, &registry, BlockPos::new(x, 2, 0));
    }
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(4, 2, 0))
            .effective()
            .get(),
        15,
        "Q-c sanity: the top dust is live"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(3, 1, 0))
            .effective()
            .get(),
        14,
        "Q-c: dust beside a dust-fed stone reads 14"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(10, 1, 0))
            .effective()
            .get(),
        14,
        "z=20: dust on top of a dust-fed stone reads 14"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(15, 1, 0)).effective(),
        PowerLevel::ZERO,
        "z=22: dust below does not power the stone"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(20, 1, 0)).effective(),
        PowerLevel::ZERO,
        "W3: covered dust does not power the stone beside it"
    );
}

#[test]
fn dust_reads_powered_stone_beside_and_below_but_not_above() {
    // P13-06, measured: R1 (side, 15 off strong stone), D2 (below, 15 off
    // strong stone) and Q-a (above, 0 under a powered stone).
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    // Q-a: a certainly-powered stone (dust-fed from above, side probe 14)
    // with dust directly beneath it: the under-dust reads 0.
    world.set(BlockPos::new(4, 1, 0), stone);
    world.set(BlockPos::new(4, 2, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(5, 1, 0), stone);
    world.set(BlockPos::new(5, 2, 0), torch(&registry, true));
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(4, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(3, 0, 0), stone);
    world.set(BlockPos::new(3, 1, 0), wire(&registry, PowerLevel::ZERO));
    for x in [3, 4, 5] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
        settle(&mut world, &registry, BlockPos::new(x, 2, 0));
    }
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(3, 1, 0))
            .effective()
            .get(),
        14,
        "validator: the stone is powered (dust-fed)"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(4, 0, 0)).effective(),
        PowerLevel::ZERO,
        "Q-a: dust under a powered stone reads 0"
    );
}

#[test]
fn lamp_reads_only_the_solids_above_and_below_it() {
    // P13-06, measured rows T1 (below, lit), R2 (side, dark), Q-d (above,
    // lit), K1/I/L1/L2 (sources, dark).
    let registry = registry();
    let mut world = flat();
    let stone = block(&registry, "minecraft:stone");
    let lamp = block(&registry, "minecraft:redstone_lamp");
    // T1: lamp on a torch-powered stone lights.
    world.set(BlockPos::new(0, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(0, 1, 0), stone);
    world.set(BlockPos::new(0, 2, 0), lamp);
    // R2: lamp beside a torch-powered stone stays dark.
    world.set(BlockPos::new(4, 0, 0), torch(&registry, true));
    world.set(BlockPos::new(4, 1, 0), stone);
    world.set(BlockPos::new(5, 0, 0), stone);
    world.set(BlockPos::new(5, 1, 0), lamp);
    // Q-d: lamp under a dust-powered stone lights.
    world.set(BlockPos::new(8, 0, 0), lamp);
    world.set(BlockPos::new(8, 1, 0), stone);
    world.set(BlockPos::new(8, 2, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(9, 1, 0), stone);
    world.set(BlockPos::new(9, 2, 0), torch(&registry, true));
    world.set(BlockPos::new(9, 0, 0), stone);
    // K1: lamp beside live dust stays dark.
    world.set(
        BlockPos::new(12, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(13, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(13, 0, 1), lamp);
    // I: lamp beside an on lever stays dark.
    world.set(
        BlockPos::new(16, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(17, 0, 0), lamp);
    // L1: lamp on top of a redstone block stays dark.
    world.set(
        BlockPos::new(20, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(20, 1, 0), lamp);
    for x in [0, 4, 8, 9, 12, 13, 16, 17, 20] {
        settle(&mut world, &registry, BlockPos::new(x, 0, 0));
        settle(&mut world, &registry, BlockPos::new(x, 1, 0));
        settle(&mut world, &registry, BlockPos::new(x, 2, 0));
    }
    assert!(
        lit_at(&world, &registry, BlockPos::new(0, 2, 0)),
        "T1: lamp on powered stone lights"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(5, 1, 0)),
        "R2: lamp beside powered stone stays dark"
    );
    assert!(
        lit_at(&world, &registry, BlockPos::new(8, 0, 0)),
        "Q-d: lamp under powered stone lights"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(13, 0, 0))
            .effective()
            .get(),
        15,
        "K1 sanity: the dust beside the lamp is live"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(13, 0, 1)),
        "K1: lamp beside live dust stays dark"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(17, 0, 0)),
        "I: lamp beside an on lever stays dark"
    );
    assert!(
        !lit_at(&world, &registry, BlockPos::new(20, 1, 0)),
        "L1: lamp on a redstone block stays dark"
    );
}

#[test]
fn a_solid_cannot_keep_dust_lit_through_itself() {
    // The exclusion half of `solid_power`: dust must not cite itself through
    // the block. A floor lever on a stone lights the dust beside the mount;
    // flipping the lever off must darken it again. Without excluding the
    // reader, the stone would keep citing the lit dust and the line would
    // never go dark.
    let registry = registry();
    let mut world = flat();
    world.set(BlockPos::new(1, 0, 0), block(&registry, "minecraft:stone"));
    world.set(
        BlockPos::new(1, 1, 0),
        lever(&registry, "floor", "north", true),
    );
    world.set(BlockPos::new(2, 0, 0), wire(&registry, PowerLevel::ZERO));
    settle(&mut world, &registry, BlockPos::new(1, 1, 0));
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(2, 0, 0))
            .effective()
            .get(),
        15,
        "precondition: dust lit through the lever's mount"
    );
    // Flip the lever off: the whole path must go dark in one pass.
    world.set(
        BlockPos::new(1, 1, 0),
        lever(&registry, "floor", "north", false),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(1, 1, 0));
    let report = propagate(
        &mut world,
        &mut queue,
        table(&registry),
        UpdateBudget::nominal(),
    );
    assert!(
        report.blocks_changed >= 1,
        "the dust must change, got {}",
        report.blocks_changed
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(2, 0, 0)).effective(),
        PowerLevel::ZERO,
        "no self-sustaining loop through the stone"
    );
}

#[test]
fn our_wire_is_never_a_strong_source() {
    // The half of the strong/weak rule this model does implement literally: dust powers a
    // block *weakly*, so a wire never emits a strong signal, whatever its strength.
    let registry = registry();
    let (world, _, _) = line_with_source(block(&registry, "minecraft:redstone_block"), 4);
    for x in 1..=3 {
        let emission = wire_emission(&world, &registry, BlockPos::new(x, 0, 0));
        assert!(
            !emission.is_strong(),
            "wire at x={x} must not emit strongly"
        );
        assert_eq!(emission.state.strong, PowerLevel::ZERO);
    }
}

#[test]
fn a_powered_repeater_outputs_fifteen_whatever_the_input_strength() {
    // Verified rule: minecraft.wiki, *Redstone mechanics* §"Signal transmission" — "When a
    // redstone repeater receives a redstone signal of any strength, it outputs a signal of
    // strength 15."
    //
    // The repeater's *emission* follows its stored `powered` state, so this test drives that
    // state and then shows the input strength is not observable in the output. (The input is
    // what turns `powered` on in the first place; that latch is the delay, and the delay is
    // not enforced in this pass.)
    let registry = registry();
    let table = table(&registry);
    for delay in 1..=Repeater::MAX_DELAY {
        for powered in [false, true] {
            let repeater = Repeater {
                delay,
                powered,
                locked: false,
            };
            for input in 0..=MAX_POWER {
                let out = ComponentState::Repeater(repeater)
                    .output_power(mc_redstone::PowerState::weak_only(level(input)));
                let expected = if powered {
                    mc_redstone::PowerState::weak_only(PowerLevel::MAX)
                } else {
                    mc_redstone::PowerState::OFF
                };
                assert_eq!(
                    out, expected,
                    "delay {delay}, powered {powered}, input {input}"
                );
            }
        }
    }
    // Downstream of a repeater, dust restarts its own count from 15.
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(
            &registry,
            ComponentState::Repeater(Repeater {
                delay: 2,
                powered: true,
                locked: false,
            }),
        ),
    );
    for x in 1..=4 {
        world.set(BlockPos::new(x, 0, 0), wire(&registry, PowerLevel::ZERO));
    }
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    let expected = [
        (BlockPos::new(1, 0, 0), 15u8),
        (BlockPos::new(2, 0, 0), 14),
        (BlockPos::new(3, 0, 0), 13),
        (BlockPos::new(4, 0, 0), 12),
    ];
    common::assert_wire_powers(&world, &registry, &expected);
}

#[test]
fn a_torch_inverts_its_input_and_powers_the_dust_it_controls() {
    // A torch takes its input from the block it is attached to. Lit → 15 out; a powered
    // attachment → dark → 0 out. The delay and burn-out Vanilla has are **not** modelled.
    let registry = registry();
    let table = table(&registry);
    let torch_lit = ComponentState::Torch(RedstoneTorch::LIT);
    let torch_dark = ComponentState::Torch(RedstoneTorch::new(false));
    assert_eq!(
        torch_lit.output_power(mc_redstone::PowerState::OFF),
        mc_redstone::PowerState::weak_only(PowerLevel::MAX)
    );
    assert_eq!(
        torch_lit.output_power(mc_redstone::PowerState::weak_only(level(1))),
        mc_redstone::PowerState::weak_only(PowerLevel::MAX),
        "a lit torch emits 15 while lit"
    );
    assert_eq!(
        torch_dark.output_power(mc_redstone::PowerState::OFF),
        mc_redstone::PowerState::OFF,
        "an unlit torch emits nothing, whatever its input"
    );
    // A dark torch next to dust leaves the dust at 0 once the dust is recomputed. The wire
    // starts claiming 15 — the level a live torch *would* give it — so the recomputation has
    // something real to correct. The torch stays dark because its attachment (the lever
    // below) is powered; without that lever the torch would relight, since a torch with
    // a dead attachment is lit.
    let mut world = flat();
    world.set(BlockPos::new(0, 1, 0), component(&registry, torch_dark));
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(1, 1, 0), wire(&registry, level(15)));
    assert!(is_torch(&world, &registry, BlockPos::new(0, 1, 0)));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 1, 0));
    prepare_self(&mut queue, BlockPos::new(0, 1, 0));
    prepare(&mut queue, BlockPos::new(1, 1, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.blocks_changed, 1, "exactly the wire changes");
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 1, 0)).effective(),
        PowerLevel::ZERO,
        "a dark torch lets the dust beside it fall to 0"
    );

    // The same scenario with the torch lit drives the dust to 15, which is the inversion seen
    // from the dust's side: lit → 15, dark → 0.
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), component(&registry, torch_lit));
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
            .effective()
            .get(),
        15
    );
}

#[test]
fn our_comparator_combines_a_back_and_a_side_input_by_the_wiki_modes() {
    // Compare: output the back input if it is at least the side, else off.
    // Subtract: back minus side, floored at 0.
    // Both sentences are on `ComparatorMode`, quoted from minecraft.wiki.
    let compare = Comparator::new(ComparatorMode::Compare);
    assert_eq!(compare.level(level(10), level(10)), level(10));
    assert_eq!(compare.level(level(10), level(9)), level(10));
    assert_eq!(compare.level(level(10), level(11)), PowerLevel::ZERO);
    assert_eq!(compare.level(level(1), level(2)), PowerLevel::ZERO);
    let subtract = Comparator::new(ComparatorMode::Subtract);
    assert_eq!(subtract.level(level(10), level(3)), level(7));
    assert_eq!(subtract.level(level(10), level(10)), PowerLevel::ZERO);
    assert_eq!(subtract.level(level(3), level(10)), PowerLevel::ZERO);
    // Exhaustive, because the input space is 17 x 17 and that is small enough to be total.
    for back in 0..=MAX_POWER {
        for side in 0..=MAX_POWER {
            let expected = if back >= side { back } else { 0 };
            assert_eq!(
                compare.level(level(back), level(side)),
                level(expected),
                "compare back={back} side={side}"
            );
            assert_eq!(
                subtract.level(level(back), level(side)),
                level(back.saturating_sub(side)),
                "subtract back={back} side={side}"
            );
        }
    }
}

#[test]
fn a_repeater_delay_is_clamped_and_a_shortest_delay_is_one_tick() {
    // The delay property is `delay=1|2|3|4` in the registry fixture (dumped from the 26.1.2
    // data), so anything else is out of range and clamps rather than panicking.
    assert_eq!(Repeater::MIN_DELAY, 1);
    assert_eq!(Repeater::MAX_DELAY, 4);
    assert_eq!(clamp_repeater_delay(0), 1);
    assert_eq!(clamp_repeater_delay(u32::MAX), 4);
    for delay in [0, 1, 2, 3, 4, 5, 1_000, u32::MAX] {
        let repeater = Repeater::new(delay);
        assert!((1..=4).contains(&repeater.delay()), "{delay}");
    }
}

#[test]
fn a_self_rescheduling_clock_is_bounded_by_the_per_tick_cap_not_by_iteration() {
    // A clock is legal, so the work must not be bounded by an iteration count. It is bounded by
    // the queue's entry cap, its per-tick update cap, and the budget. This test runs a clock
    // for 200 ticks under a deliberately tiny budget and asserts the invariants that matter:
    // per-tick work never exceeds the cap, the queue never grows past its cap, the clock keeps
    // waking up, and the circuit's answer does not drift.
    let registry = registry();
    let mut world = flat();
    // A live wire next to its source, written out rather than computed so the loop starts from
    // the state under test.
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(1, 0, 0), wire(&registry, level(15)));
    let table = table(&registry);
    let mut queue = UpdateQueue::new();
    // Seed the clock: the wire is due on tick 1.
    assert!(queue.schedule(0, BlockPos::new(1, 0, 0), 1).is_ok());

    let mut tick = 0u64;
    let mut total_processed = 0usize;
    let mut ticks_with_work = 0usize;
    for _ in 0..200 {
        tick += 1;
        // A deliberately tiny budget, so the cap is what stops each tick.
        let budget = UpdateBudget::new(4, 2);
        let report = run_block_tick(&mut world, &mut queue, table, tick, budget);
        total_processed += report.updates_processed;
        assert!(
            report.updates_processed <= budget.neighbour_updates_per_tick,
            "tick {tick} processed {} updates with a cap of {}",
            report.updates_processed,
            budget.neighbour_updates_per_tick
        );
        if report.did_work() {
            ticks_with_work += 1;
        }
        // The hard invariant: the queue never grows past its cap.
        assert!(
            queue.len() <= UpdateQueue::MAX_ENTRIES,
            "tick {tick}: queue grew to {}",
            queue.len()
        );
        // And the clock never runs itself early or late: exactly the next tick is armed.
        assert!(
            queue
                .scheduled_at(tick + 1)
                .is_some_and(|set| set.contains(&BlockPos::new(1, 0, 0))),
            "tick {tick}: the wire must be armed for tick {}",
            tick + 1
        );
        assert!(
            !queue.has_pending_after(tick + 1),
            "tick {tick}: nothing may be armed beyond the next tick"
        );
    }
    assert!(
        total_processed > 0,
        "the clock must have done some work across 200 ticks"
    );
    assert!(
        ticks_with_work > 0,
        "a self-rescheduling wire must keep waking up"
    );
    // And the wire is still exactly where the rule puts it: bounded work, not drift.
    // P13-05: dust next to a source carries the full strength.
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
            .effective()
            .get(),
        15
    );
}

#[test]
fn a_zero_budget_processes_nothing_reports_it_and_loses_nothing() {
    let registry = registry();
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    let table = table(&registry);
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let queued_before = queue.len();

    let report = propagate(&mut world, &mut queue, table, UpdateBudget::new(0, 0));
    assert_eq!(report.updates_processed, 0);
    assert_eq!(report.blocks_changed, 0);
    assert!(
        report.budget_exhausted,
        "there was work to do and none was done"
    );
    assert_eq!(queue.len(), queued_before, "nothing may be consumed");
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0)).effective(),
        PowerLevel::ZERO,
        "the wire must not have been updated"
    );

    // The next tick with a real budget finishes the job.
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(report.updates_processed > 0);
    assert!(!report.budget_exhausted);
    assert!(queue.is_empty());
}

#[test]
fn a_dust_loop_terminates_because_a_wire_cites_state_rather_than_recursing() {
    // Four wires in a ring: every wire is adjacent to two wires and nothing else, so the
    // whole ring is off and must settle in one pass. Without the "cite the stored level
    // rather than recursing" rule in `gather_inputs` this would recurse forever.
    let registry = registry();
    let mut world = flat();
    let ring = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(1, 0, 0),
        BlockPos::new(1, 0, 1),
        BlockPos::new(0, 0, 1),
    ];
    for pos in ring {
        world.set(pos, wire(&registry, PowerLevel::ZERO));
    }
    let table = table(&registry);
    let mut queue = UpdateQueue::new();
    for pos in ring {
        prepare(&mut queue, pos);
    }
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(
        report.blocks_changed, 0,
        "an unpowered ring changes nothing"
    );
    assert!(!report.budget_exhausted);
    assert!(queue.is_empty(), "the queue must settle, not spin");
    for pos in ring {
        assert_eq!(
            wire_emission(&world, &registry, pos).effective(),
            PowerLevel::ZERO
        );
    }
}

#[test]
fn removing_the_source_drops_every_wire_in_the_line_to_zero_in_one_pass() {
    // The classic defect this guards against: a redstone model that propagates *increases* but not
    // *decreases*, so breaking the source leaves the line lit. It is the case where a model can pass
    // every golden test and still be wrong, so it is asserted over a whole line rather than one hop.
    //
    // A single-hop decrease can pass by accident (the first wire is the one the removal touches
    // directly), so this checks **every** position of a five-block line: x = 1..=5 all reach 0.
    let registry = registry();
    let air = registry.air_id();
    let (mut world, _, positions) =
        line_with_source(block(&registry, "minecraft:redstone_block"), 5);
    let table = table(&registry);

    // Precondition: the line is fully lit, 15 down to 11, so the assertions below are not vacuous.
    common::assert_wire_powers(
        &world,
        &registry,
        &[
            (BlockPos::new(1, 0, 0), 15),
            (BlockPos::new(2, 0, 0), 14),
            (BlockPos::new(3, 0, 0), 13),
            (BlockPos::new(4, 0, 0), 12),
            (BlockPos::new(5, 0, 0), 11),
        ],
    );

    // Break the source and announce the change at its position, which is what the code that removes a
    // block must do: `prepare` queues the six neighbours of the position that changed, regardless of
    // whether the new state is itself an emitter.
    assert!(world.set(BlockPos::new(0, 0, 0), air));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());

    assert!(
        report.blocks_changed >= positions.len(),
        "every wire must change, got {}",
        report.blocks_changed
    );
    // Every position, not just the first.
    for pos in &positions {
        assert_eq!(
            wire_emission(&world, &registry, *pos).effective(),
            PowerLevel::ZERO,
            "{pos} must be off once the source is gone"
        );
    }
    assert!(queue.is_empty(), "the cascade must settle, not linger");
    // And the transcript records the decreases, one per position.
    let decreased: Vec<BlockPos> = report
        .changes
        .iter()
        .filter(|change| change.emitted.effective() == PowerLevel::ZERO)
        .map(|change| change.pos)
        .collect();
    for pos in &positions {
        assert!(
            decreased.contains(pos),
            "{pos} must appear in the transcript as a decrease"
        );
    }
}

#[test]
fn switching_a_lever_off_drops_the_line_and_on_lights_it_again() {
    // The same property for a *component* rather than a removed block: a lever with `powered=false`
    // emits nothing, so the queue must carry that decrease too. Both directions are checked, because a
    // model that only ever turns things on would pass the increase half alone.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    let line: Vec<BlockPos> = (1..=5).map(|x| BlockPos::new(x, 0, 0)).collect();
    for pos in &line {
        world.set(*pos, wire(&registry, PowerLevel::ZERO));
    }

    // Off -> on: the line lights, 15 down to 11.
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    for (index, pos) in line.iter().enumerate() {
        let expected = 15 - u8::try_from(index).expect("fits");
        assert_eq!(
            wire_emission(&world, &registry, *pos).effective().get(),
            expected,
            "{pos} after switching on"
        );
    }

    // On -> off: the whole line must go dark.
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::OFF)),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(report.blocks_changed >= line.len());
    for pos in &line {
        assert_eq!(
            wire_emission(&world, &registry, *pos).effective(),
            PowerLevel::ZERO,
            "{pos} after switching off"
        );
    }
}

#[test]
fn only_powered_wires_are_rescheduled() {
    // `schedule_wire_recheck` is the "ask again next tick" hook. It must schedule live wires
    // and leave dark ones alone, or every wire in the world would wake up every tick.
    let registry = registry();
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), wire(&registry, PowerLevel::MAX));
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(2, 0, 0), block(&registry, "minecraft:stone"));
    let mut queue = UpdateQueue::new();
    let scheduled = schedule_wire_recheck(
        &world,
        table(&registry),
        &mut queue,
        10,
        &[
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 0, 0),
            BlockPos::new(2, 0, 0),
        ],
    );
    assert_eq!(scheduled, 1, "only the live wire is rescheduled");
    assert!(
        queue
            .scheduled_at(11)
            .is_some_and(|set| set.contains(&BlockPos::new(0, 0, 0)))
    );
    assert_eq!(queue.len(), 1);
}

#[test]
fn prepare_and_prepare_self_queue_what_the_algorithm_needs() {
    let mut queue = UpdateQueue::new();
    let pos = BlockPos::new(3, 0, 3);
    assert_eq!(prepare(&mut queue, pos), 6, "six face neighbours");
    assert_eq!(queue.neighbour_len(), 6);
    assert!(prepare_self(&mut queue, pos).was_inserted());
    assert_eq!(queue.neighbour_len(), 7);
    // All six in one field: the caller gets one working set, not a helper per shape.
    assert!(matches!(
        prepare_self(&mut queue, pos),
        mc_redstone::Inserted::Deduplicated
    ));
}
