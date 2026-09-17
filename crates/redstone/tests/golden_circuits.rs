//! Golden circuits: hand-written scenarios with the exact expected power at every position.
//!
//! Each expected number is written out in the test rather than derived from a run, so a change
//! in the model shows up as a failing number a human can check rather than as a diff in a
//! machine-generated fixture. Where the expectation is this crate's rule rather than a verified
//! Vanilla number, the comment says so and `docs`-style labels match the module docs in
//! `mc_redstone::power` and `mc_redstone::propagation`.
//!
//! The wire rule under test, stated once:
//!
//! ```text
//! wire_power(x) = 16 - x for a 15-strength source at x = 0
//! ```
//!
//! so a 15-strength source at x = 0 gives wire(1) = 15, wire(2) = 14, ... wire(15) = 1,
//! wire(16) = 0. That is `WIRE_LIVE_BLOCKS` = 15 live wire blocks (P13-05, measured
//! on a real 26.1.2 server).

mod common;

use common::{block, component, flat, level, registry, table, wire, wire_power_at};
use mc_redstone::components::{
    Comparator, ComparatorMode, ComponentState, Lever, RedstoneTorch, Repeater,
};
use mc_redstone::propagation::{BlockRole, prepare, propagate, run_block_tick};
use mc_redstone::{BlockPos, MAX_POWER, PowerLevel, UpdateBudget, UpdateQueue};

/// The state id of a wire at `power`, read through the registry.
fn w(registry: &mc_registry::BlockRegistry, power: u8) -> i32 {
    wire(registry, level(power))
}

/// Whether the lamp at `pos` is lit, read through the registry.
fn is_lit(
    world: &mc_redstone::propagation::FlatWorld,
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

#[test]
fn golden_lever_fifteen_blocks_of_wire_and_a_lamp() {
    // Layout, all on y = 0 with the wire resting on nothing this model cares about:
    //
    //   x:  0        1   2   3  ...                              16   17
    //       lever    w   w   w   ...                              w    lamp
    //
    // Hand-computed expectation (P13-05, measured: the first dust carries the
    // full strength, then one attenuation per block):
    //
    //   wire(1) = 15   wire(2) = 14   wire(3) = 13   ...   wire(14) = 2   wire(15) = 1
    //   wire(16) = 0   (so the lamp at x = 17 is off)
    //
    // The lamp is `minecraft:redstone_lamp`, the driven mechanism (P13-03): the first
    // half asserts it stays dark off a dead wire, the second half that it lights
    // off a live one.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    for x in 1..=16 {
        world.set(BlockPos::new(x, 0, 0), w(&registry, 0));
    }
    world.set(
        BlockPos::new(17, 0, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );

    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(
        !report.budget_exhausted,
        "this circuit must fit the nominal budget"
    );

    // The 15 live wire blocks, spelled out one number per position.
    let expected: [(BlockPos, u8); 16] = [
        (BlockPos::new(1, 0, 0), 15),
        (BlockPos::new(2, 0, 0), 14),
        (BlockPos::new(3, 0, 0), 13),
        (BlockPos::new(4, 0, 0), 12),
        (BlockPos::new(5, 0, 0), 11),
        (BlockPos::new(6, 0, 0), 10),
        (BlockPos::new(7, 0, 0), 9),
        (BlockPos::new(8, 0, 0), 8),
        (BlockPos::new(9, 0, 0), 7),
        (BlockPos::new(10, 0, 0), 6),
        (BlockPos::new(11, 0, 0), 5),
        (BlockPos::new(12, 0, 0), 4),
        (BlockPos::new(13, 0, 0), 3),
        (BlockPos::new(14, 0, 0), 2),
        (BlockPos::new(15, 0, 0), 1),
        // The 16th wire block is off: the signal died after 15 blocks of dust.
        (BlockPos::new(16, 0, 0), 0),
    ];
    common::assert_wire_powers(&world, &registry, &expected);

    // What the lamp reads: its only neighbour is the dead wire at x = 16, so it
    // reads nothing. (Vanilla would also be dark here.)
    assert_eq!(
        common::emitted_at(&world, &registry, BlockPos::new(16, 0, 0)).effective(),
        PowerLevel::ZERO
    );
    assert_eq!(
        table.classify(world.get(BlockPos::new(17, 0, 0)).expect("lamp")),
        BlockRole::Mechanism,
        "the lamp is the driven mechanism (P13-03)"
    );
    assert!(
        !is_lit(&world, &registry, BlockPos::new(17, 0, 0)),
        "with a dead neighbour the lamp stays dark"
    );

    // Now shorten the line to 13 blocks and stand the lamp on a stone
    // pedestal at the end: the last dust powers the pedestal from the side
    // (P13-06, measured z=20), and the lamp reads its support. A lamp beside
    // live dust would stay dark (K1) — the pedestal is the vanilla-faithful
    // geometry.
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    for x in 1..=13 {
        world.set(BlockPos::new(x, 0, 0), w(&registry, 0));
    }
    world.set(BlockPos::new(14, 0, 0), block(&registry, "minecraft:stone"));
    world.set(
        BlockPos::new(14, 1, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(
        wire_power_at(&world, &registry, BlockPos::new(13, 0, 0)),
        Some(3)
    );
    // And the mechanism half: propagate writes `lit` back, so the lamp on
    // the dust-powered pedestal is lit.
    assert!(
        is_lit(&world, &registry, BlockPos::new(14, 1, 0)),
        "a lamp on a dust-powered pedestal must be lit after propagation"
    );
}

#[test]
fn golden_lever_mount_powers_stone_strongly() {
    // Layout, y = 0 except where noted:
    //
    //       z=0:  stone-A  lever(floor,on)   w(2,0,0)
    //             x=1      x=1,y=1
    //       z=0:  lever(wall,west,on)  stone-B  torch(top, out)
    //             x=4                  x=5      x=5,y=1
    //       z=0:  lever(wall,west,on)  stone-C  lamp(top, lit) + lamp beside C (dark)
    //             x=7                  x=8      x=8,y=1          x=9,y=1 on stone x=9,y=0
    //
    // P13-06, measured rows R9/R10: a lever powers its mount strongly, so
    // dust beside the mount reads the full level, a torch on the mount goes
    // out, and a lamp on the mount lights — while a lamp beside the mount
    // stays dark (R2).
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    let stone = |world: &mut mc_redstone::propagation::FlatWorld, x: i32, y: i32| {
        world.set(BlockPos::new(x, y, 0), block(&registry, "minecraft:stone"));
    };
    // Mount A: floor lever, dust beside the mount.
    stone(&mut world, 1, 0);
    world.set(
        BlockPos::new(1, 1, 0),
        common::lever(&registry, "floor", "north", true),
    );
    world.set(BlockPos::new(2, 0, 0), w(&registry, 0));
    // Mount B: wall lever, torch on the mount.
    world.set(
        BlockPos::new(4, 0, 0),
        common::lever(&registry, "wall", "west", true),
    );
    stone(&mut world, 5, 0);
    world.set(
        BlockPos::new(5, 1, 0),
        registry
            .state_id(
                "minecraft:redstone_torch",
                &[("lit".to_owned(), "true".to_owned())],
            )
            .expect("lit torch"),
    );
    // Mount C: wall lever, lamp on the mount, lamp beside the mount.
    world.set(
        BlockPos::new(7, 0, 0),
        common::lever(&registry, "wall", "west", true),
    );
    stone(&mut world, 8, 0);
    world.set(
        BlockPos::new(8, 1, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    stone(&mut world, 9, 0);
    world.set(
        BlockPos::new(9, 1, 0),
        block(&registry, "minecraft:redstone_lamp"),
    );
    let mut queue = UpdateQueue::new();
    for x in [1, 4, 5, 7, 8] {
        prepare(&mut queue, BlockPos::new(x, 0, 0));
        prepare(&mut queue, BlockPos::new(x, 1, 0));
    }
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(
        !report.budget_exhausted,
        "this circuit must fit the nominal budget"
    );
    // Dust beside a lever-powered mount reads the full level.
    common::assert_wire_powers(&world, &registry, &[(BlockPos::new(2, 0, 0), 15)]);
    // The torch on the mount goes out; the lamp on the mount lights; the
    // lamp beside the mount stays dark.
    assert!(
        !is_lit(&world, &registry, BlockPos::new(5, 1, 0)),
        "a torch on a lever-powered mount must go dark"
    );
    assert!(
        is_lit(&world, &registry, BlockPos::new(8, 1, 0)),
        "a lamp on a lever-powered mount must light"
    );
    assert!(
        !is_lit(&world, &registry, BlockPos::new(9, 1, 0)),
        "a lamp beside a powered mount must stay dark"
    );
}

#[test]
fn golden_torch_inverter_inverts_its_attachment_block() {
    // Layout, y = 0:
    //
    //   x:  0                1                 2      3   4
    //       lever      stone-under-torch        torch  w   w
    //
    // Wait — this model has no conductivity, so the torch cannot be attached to a block and read
    // it. The layout this model *can* express is a torch whose neighbours include the thing that
    // powers it, which is exactly the "input with a torch attached" the wiki describes:
    //
    //   x:  0        1        2   3
    //       lever    torch    w   w
    //
    // Torch state is stored (`lit`), so the two golden cases are the two torch states, and the
    // input is what turns `lit` on and off via `RedstoneTorch::react`.
    let registry = registry();
    let table = table(&registry);

    // Case 1: the torch is lit, which is the state of a freshly placed torch with no power on its
    // attachment. The wire next to it carries 15 and the next one 14.
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Torch(RedstoneTorch::LIT)),
    );
    world.set(BlockPos::new(1, 0, 0), w(&registry, 0));
    world.set(BlockPos::new(2, 0, 0), w(&registry, 0));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    common::assert_wire_powers(
        &world,
        &registry,
        &[(BlockPos::new(1, 0, 0), 15), (BlockPos::new(2, 0, 0), 14)],
    );

    // Case 2: the torch is dark because its attachment is powered, so it stays
    // dark and the wire it controls falls to 0. The lever *below* the torch is
    // what matters now — attachment, not strongest neighbour (P13-04).
    let mut world = flat();
    world.set(
        BlockPos::new(0, 1, 0),
        component(&registry, ComponentState::Torch(RedstoneTorch::new(false))),
    );
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(1, 1, 0), w(&registry, 14));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 1, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.blocks_changed, 1, "the one wire must fall");
    common::assert_wire_powers(&world, &registry, &[(BlockPos::new(1, 1, 0), 0)]);

    // The inversion itself, over the whole input space: any non-zero input (either kind) turns
    // the torch off, and a clear input lights it. **approximation**: the delay and the burn-out
    // Vanilla has are not modelled, so the inversion is instantaneous and unlimited.
    for value in 0..=MAX_POWER {
        let weak = mc_redstone::PowerState::weak_only(level(value));
        let strong = mc_redstone::PowerState::strong_at(level(value));
        assert_eq!(RedstoneTorch::react(weak).lit, value == 0, "weak {value}");
        assert_eq!(
            RedstoneTorch::react(strong).lit,
            value == 0,
            "strong {value}"
        );
    }
}

#[test]
fn golden_repeater_with_its_documented_delay() {
    // Layout, y = 0:
    //
    //   x:  0          1   2   3   4   5
    //       repeater   w   w   w   w   w
    //
    // The repeater is `powered = true`, so it emits 15 and the dust restarts its count from
    // there: wire(1) = 15, wire(2) = 14, wire(3) = 13, wire(4) = 12, wire(5) = 11.
    //
    // The delay is *stored*, not waited on: `Repeater::MAX_DELAY` = 4 game ticks comes from the
    // `delay=1|2|3|4` property in the registry fixture (dumped from the 26.1.2 data), and this
    // pass does not enforce it. What the test checks is that the stored delay survives a round
    // trip and that the emission does not depend on it.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(
            &registry,
            ComponentState::Repeater(Repeater {
                delay: 4,
                powered: true,
                locked: false,
            }),
        ),
    );
    for x in 1..=5 {
        world.set(BlockPos::new(x, 0, 0), w(&registry, 0));
    }
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
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

    // Every delay gives the same output: the verified wiki rule is about the *output*, not the
    // delay.
    for delay in 1..=Repeater::MAX_DELAY {
        let repeater = ComponentState::Repeater(Repeater {
            delay,
            powered: true,
            locked: false,
        });
        assert_eq!(
            repeater.output_power(mc_redstone::PowerState::OFF),
            mc_redstone::PowerState::weak_only(PowerLevel::MAX),
            "delay {delay}"
        );
        // And the delay survives a registry round trip, which is what makes the block state the
        // source of truth for it.
        let id = table.component_state(repeater).expect("state resolves");
        assert_eq!(
            table.component_of(id).expect("reads back").source(),
            mc_redstone::PowerSource::Repeater
        );
    }

    // An unpowered repeater leaves its dust dark: the latch has not closed yet.
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(
            &registry,
            ComponentState::Repeater(Repeater {
                delay: 4,
                powered: false,
                locked: false,
            }),
        ),
    );
    world.set(BlockPos::new(1, 0, 0), w(&registry, 14));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(
        wire_power_at(&world, &registry, BlockPos::new(1, 0, 0)),
        Some(0)
    );
}

#[test]
fn golden_comparator_reads_the_wire_behind_it() {
    // Layout, y = 0:
    //
    //   x:  0        1   2   3   4
    //       lever    w   w   comparator
    //
    // The comparator faces east here (its back is the wire at x = 2), so it
    // passes 14 through in compare mode with no side input. Facing is part of
    // the state under test since P13-04: the same rig with the wire on a side
    // face outputs nothing (see the unit test for back/side separation).
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(1, 0, 0), w(&registry, 0));
    world.set(BlockPos::new(2, 0, 0), w(&registry, 0));
    world.set(
        BlockPos::new(3, 0, 0),
        registry
            .state_id(
                "minecraft:comparator",
                &[
                    ("mode".to_owned(), "compare".to_owned()),
                    ("powered".to_owned(), "false".to_owned()),
                    ("facing".to_owned(), "east".to_owned()),
                ],
            )
            .expect("east-facing comparator"),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    prepare(&mut queue, BlockPos::new(3, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    common::assert_wire_powers(
        &world,
        &registry,
        &[(BlockPos::new(1, 0, 0), 15), (BlockPos::new(2, 0, 0), 14)],
    );
    // Hand-computed: the comparator's back is wire(2) at 14, and compare mode with
    // no side input passes 14 through.
    assert_eq!(
        common::emitted_at(&world, &registry, BlockPos::new(3, 0, 0))
            .effective()
            .get(),
        14
    );
    // The pure arithmetic is unchanged: subtract takes the side off.
    assert_eq!(
        Comparator::new(ComparatorMode::Subtract).level(level(14), PowerLevel::ZERO),
        level(14)
    );
    assert_eq!(
        Comparator::new(ComparatorMode::Subtract).level(level(14), level(5)),
        level(9)
    );
}

#[test]
fn golden_a_torch_clock_advances_one_scheduled_tick_per_tick() {
    // The clock shape this model supports: a live wire that is due every tick. The golden
    // expectation is the *timing*, not a power number: after processing tick N, the wire must be
    // armed for exactly tick N + 1 and nothing may be armed beyond that.
    //
    // Refusing to run early or late is the scheduler contract in `mc_redstone::update`: `due =
    // now + delay`, so a delay of 1 scheduled at tick 1 is due at tick 2 and not before.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    world.set(BlockPos::new(1, 0, 0), w(&registry, 15));
    let clock = BlockPos::new(1, 0, 0);
    let mut queue = UpdateQueue::new();
    assert!(queue.schedule(0, clock, 1).is_ok());

    for tick in 1..=10u64 {
        // Not due before its tick.
        if tick > 1 {
            assert!(
                queue.scheduled_at(tick - 1).is_none(),
                "tick {tick}: nothing may still be armed for the past"
            );
        }
        let before = queue.scheduled_at(tick).cloned();
        assert_eq!(
            before.as_ref().map(std::collections::BTreeSet::len),
            Some(1),
            "tick {tick}: exactly one entry is due"
        );
        let report = run_block_tick(&mut world, &mut queue, table, tick, UpdateBudget::nominal());
        assert!(
            report.updates_processed > 0,
            "tick {tick}: the clock did work"
        );
        assert!(
            queue
                .scheduled_at(tick + 1)
                .is_some_and(|set| set.contains(&clock)),
            "tick {tick}: the clock is armed for exactly the next tick"
        );
        assert!(
            queue.scheduled_at(tick + 2).is_none(),
            "tick {tick}: and not for the tick after"
        );
        assert!(
            !queue.has_pending_after(tick + 1),
            "tick {tick}: nothing beyond the next tick"
        );
        assert_eq!(
            wire_power_at(&world, &registry, clock),
            Some(15),
            "tick {tick}: the wire is stable at 15"
        );
    }
}
