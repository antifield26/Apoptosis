//! Propagation acceptance: attenuation over a line, the strong/weak distinction, and
//! exhaustion from a real circuit.
//!
//! ## What is being tested, and what is *not*
//!
//! `mc-redstone` does **not** claim Vanilla's update order, its block-update/shape-update
//! split, conductivity, or its component timing (see `propagation`'s module docs). What it
//! does claim is:
//!
//! - the wire attenuation rule (verified: minecraft.wiki, *Redstone mechanics* §"Signal
//!   transmission" — "the signal strength decreases by 1 for every block of redstone dust
//!   that the signal travels … up to 15 blocks by itself"), and
//! - the strong/weak distinction (verified for the rule; the redstone block's strength is
//!   the one place the implemented table is an interpretation, and `power.rs` says so).
//!
//! The tests whose subject is the *implemented* rule rather than Vanilla's are named
//! `our_…`, so nothing here can be mistaken for a parity claim.

mod common;

use common::{block, component, flat, is_torch, level, registry, table, wire};
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
    // The *live* reach is 15 / 1 - 1 = 14, because the first wire already takes one step off
    // the source. `WIRE_LIVE_BLOCKS` documents why that is one less than the wiki's "15
    // blocks" phrasing; the test below asserts the 14 empirically.
    assert_eq!(WIRE_LIVE_BLOCKS, 14);
    assert_eq!(
        WIRE_LIVE_BLOCKS,
        u32::from(MAX_POWER) / u32::from(WIRE_ATTENUATION_PER_BLOCK) - 1
    );
}

#[test]
fn power_attenuates_by_exactly_one_per_block_from_a_strong_source() {
    // A block of redstone at (0,0,0), wire at x = 1..=15. Hand-computed expectation:
    //   wire(x) = 15 - x   for x in 1..=15
    // so wire(1) = 14 and wire(15) = 0. The value at x = 0 is the source itself.
    let registry = registry();
    let (world, _, positions) = line_with_source(block(&registry, "minecraft:redstone_block"), 15);
    let expected: Vec<(BlockPos, u8)> = (0..15)
        .map(|index| {
            let x = index + 1;
            (
                BlockPos::new(x, 0, 0),
                MAX_POWER - u8::try_from(x).expect("x fits in u8"),
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
        14
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(2, 0, 0))
            .effective()
            .get(),
        13
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(7, 0, 0))
            .effective()
            .get(),
        8
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(14, 0, 0))
            .effective()
            .get(),
        1
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(15, 0, 0))
            .effective()
            .get(),
        0,
        "15 blocks from the source is the documented end of the signal"
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
fn the_signal_reaches_exactly_fourteen_live_wire_blocks_and_no_further() {
    // The reach is a property of the *chain*, so it is tested on a line longer than the
    // reach. Hand-computed expectation for a 15-strength source at (0,0,0):
    //   wire(x) = 15 - x, so wire(14) = 1 is the last live block and wire(15) = 0.
    // 14 live blocks, which is `WIRE_LIVE_BLOCKS`. The wiki's sentence says "up to 15 blocks";
    // the difference is documented on that constant rather than papered over.
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
        (1..=14).collect::<Vec<i32>>(),
        "exactly the first 14 wire blocks carry a signal"
    );
    assert_eq!(u32::try_from(live.len()).expect("fits"), WIRE_LIVE_BLOCKS);
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(14, 0, 0))
            .effective()
            .get(),
        1,
        "the last live block carries the weakest possible signal"
    );
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(15, 0, 0)).effective(),
        PowerLevel::ZERO,
        "and the next one is off"
    );
}

#[test]
fn our_strong_and_weak_sources_are_distinguishable_only_through_the_recorded_kind() {
    // **The honest state of the strong/weak split in this pass**, asserted rather than
    // implied:
    //
    // - The *rule* ("strong powers adjacent dust, weak does not") is verified against
    //   minecraft.wiki and is what `SignalKind` documents.
    // - Which sources are strong is **not** verified, and this model marks exactly one
    //   (`RedstoneBlock`) as strong.
    // - Nothing in the propagation arithmetic reads the kind yet: `EmitterOutput::effective`
    //   takes the maximum of the two fields, so a strong redstone block and a weak lever
    //   produce the *same* wire power when placed next to dust.
    //
    // So the distinction is carried and recorded, and is **not yet observable** in a circuit
    // without conductivity. That is the gap, and this test is where it is written down.
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
        from_redstone_block, 14,
        "dust beside a redstone block carries 14"
    );
    assert_eq!(
        from_lever, 14,
        "GAP: dust beside a lever also carries 14, because the wire rule takes the maximum \
         of weak and strong and has no conductivity to distinguish them. The kind is recorded \
         and unused."
    );
}

#[test]
fn our_a_block_between_a_source_and_dust_stops_the_signal() {
    // The consequence of having no conductivity table: a solid block in the path is the end
    // of the circuit, whether the source is strong or weak. Vanilla would strongly power the
    // stone and light dust on its far side.
    let registry = registry();
    let table = table(&registry);
    for source in [
        block(&registry, "minecraft:redstone_block"),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    ] {
        let mut world = flat();
        world.set(BlockPos::new(0, 0, 0), source);
        world.set(BlockPos::new(1, 0, 0), block(&registry, "minecraft:stone"));
        world.set(BlockPos::new(2, 0, 0), wire(&registry, PowerLevel::ZERO));
        let mut queue = UpdateQueue::new();
        prepare(&mut queue, BlockPos::new(0, 0, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        assert!(!report.budget_exhausted);
        assert_eq!(
            wire_emission(&world, &registry, BlockPos::new(2, 0, 0)).effective(),
            PowerLevel::ZERO,
            "no conductivity: the stone blocks the signal instead of passing it through"
        );
        // With the block removed, the same source does power dust one step away.
        world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
        let mut queue = UpdateQueue::new();
        prepare(&mut queue, BlockPos::new(1, 0, 0));
        propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        assert_eq!(
            wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
                .effective()
                .get(),
            14
        );
    }
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
        (BlockPos::new(1, 0, 0), 14u8),
        (BlockPos::new(2, 0, 0), 13),
        (BlockPos::new(3, 0, 0), 12),
        (BlockPos::new(4, 0, 0), 11),
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
    // starts claiming 14 — the level a live torch *would* give it — so the recomputation has
    // something real to correct.
    let mut world = flat();
    world.set(BlockPos::new(0, 0, 0), component(&registry, torch_dark));
    world.set(BlockPos::new(1, 0, 0), wire(&registry, level(14)));
    assert!(is_torch(&world, &registry, BlockPos::new(0, 0, 0)));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.blocks_changed, 1, "exactly the wire changes");
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0)).effective(),
        PowerLevel::ZERO,
        "a dark torch lets the dust beside it fall to 0"
    );

    // The same scenario with the torch lit drives the dust to 14, which is the inversion seen
    // from the dust's side: lit → 14, dark → 0.
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
        14
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
    world.set(BlockPos::new(1, 0, 0), wire(&registry, level(14)));
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
    assert_eq!(
        wire_emission(&world, &registry, BlockPos::new(1, 0, 0))
            .effective()
            .get(),
        14
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
    // whole ring is off and must settle in one pass. Without the "cite the stored level"
    // rule in `gather_wire_inputs` this would recurse forever.
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

    // Precondition: the line is fully lit, 14 down to 10, so the assertions below are not vacuous.
    common::assert_wire_powers(
        &world,
        &registry,
        &[
            (BlockPos::new(1, 0, 0), 14),
            (BlockPos::new(2, 0, 0), 13),
            (BlockPos::new(3, 0, 0), 12),
            (BlockPos::new(4, 0, 0), 11),
            (BlockPos::new(5, 0, 0), 10),
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

    // Off -> on: the line lights, 14 down to 10.
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    for (index, pos) in line.iter().enumerate() {
        let expected = 14 - u8::try_from(index).expect("fits");
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
