//! Determinism: the same scenario run twice produces the identical change *sequence*.
//!
//! AGENTS.md §3.6 requires that "the same initial state + same ordered inputs + same tick count
//! should produce the same normalized simulation state". Comparing only the final state would
//! pass for an implementation that processes updates in a hash order and happens to converge, so
//! these tests compare the full `Vec<PropagatedChange>` — position, cause, old id, new id and
//! emission, in order.
//!
//! The crate has no `HashMap`, no clock read and no randomness, so this should be trivially
//! true. That is exactly why it is worth a test: the property is what makes the rest of the
//! crate's behaviour reproducible, and a future change that reaches for `HashMap` or for
//! insertion order would break it here rather than silently in a save file.

mod common;

use common::{block, component, flat, level, registry, table, wire};
use mc_redstone::components::{ComponentState, Lever, Repeater};
use mc_redstone::propagation::{PropagatedChange, prepare, propagate, run_block_tick};
use mc_redstone::{BlockPos, PowerLevel, UpdateBudget, UpdateQueue};

/// One scenario's script: the same steps applied to a fresh world each time.
///
/// `steps` are the ticks to run, so the seed positions are identical between runs.
fn run_scenario() -> (Vec<PropagatedChange>, Vec<(BlockPos, Option<u8>)>) {
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();

    // A shape with several independent branches, so the processing order is actually exercised:
    // a lever feeding a wire line east, a repeater feeding another line south, and a torch.
    let lever = component(&registry, ComponentState::Lever(Lever::new(true)));
    world.set(BlockPos::new(0, 0, 0), lever);
    for x in 1..=6 {
        world.set(BlockPos::new(x, 0, 0), wire(&registry, PowerLevel::ZERO));
    }
    let repeater = component(
        &registry,
        ComponentState::Repeater(Repeater {
            delay: 3,
            powered: true,
            locked: false,
        }),
    );
    world.set(BlockPos::new(10, 0, 0), repeater);
    for z in 1..=4 {
        world.set(BlockPos::new(10, 0, z), wire(&registry, PowerLevel::ZERO));
    }
    // A cross-connection so the two branches meet and the order matters.
    world.set(BlockPos::new(6, 0, 1), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(6, 0, 2), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(6, 0, 3), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(7, 0, 3), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(8, 0, 3), wire(&registry, PowerLevel::ZERO));
    world.set(BlockPos::new(9, 0, 3), wire(&registry, PowerLevel::ZERO));

    let mut queue = UpdateQueue::new();
    let mut all_changes = Vec::new();
    // Announce both sources, then run a few ticks at the nominal budget, then a couple at a
    // smaller one so the drain-splitting path is included in the transcript.
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    prepare(&mut queue, BlockPos::new(10, 0, 0));
    for tick in 1..=4u64 {
        let budget = if tick % 2 == 0 {
            UpdateBudget::nominal()
        } else {
            UpdateBudget::new(3, 2)
        };
        let report = run_block_tick(&mut world, &mut queue, table, tick, budget);
        all_changes.extend(report.changes);
    }

    // The observable end state: every wire's power, in position order.
    let mut states = Vec::new();
    for x in 0..16 {
        for z in 0..8 {
            let pos = BlockPos::new(x, 0, z);
            let id = world.get(pos).expect("inside the box");
            if let mc_redstone::propagation::BlockRole::Wire { stored } = table.classify(id) {
                states.push((pos, Some(stored.get())));
            }
        }
    }
    (all_changes, states)
}

#[test]
fn the_same_scenario_twice_produces_the_identical_change_sequence() {
    let (first, first_states) = run_scenario();
    let (second, second_states) = run_scenario();
    assert!(
        !first.is_empty(),
        "the scenario must produce changes, or this test proves nothing"
    );
    assert_eq!(
        first, second,
        "the full change sequence must be identical between runs, not just the end state"
    );
    assert_eq!(
        first_states, second_states,
        "and the end state must agree too"
    );
}

#[test]
fn the_processing_order_is_the_documented_one_not_insertion_order() {
    // Two queues fed the same positions in different orders must drain identically, because the
    // queue is a `BTreeSet`. If it were a `Vec` or a `HashMap`, this would fail.
    let positions = [
        BlockPos::new(5, 0, 5),
        BlockPos::new(-3, 0, 0),
        BlockPos::new(0, 0, 0),
        BlockPos::new(5, 0, -5),
        BlockPos::new(1, 0, 1),
    ];
    let mut ascending = UpdateQueue::new();
    let mut shuffled = UpdateQueue::new();
    for pos in positions {
        ascending.push_neighbour(pos);
    }
    for pos in positions.iter().rev() {
        shuffled.push_neighbour(*pos);
    }
    let a = ascending.drain_for_tick(1).positions;
    let b = shuffled.drain_for_tick(1).positions;
    assert_eq!(a, b, "insertion order must not be observable");
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    assert_eq!(a, sorted);
}

#[test]
fn a_full_tick_replays_identically_from_the_same_inputs() {
    // The same scenario as above but taken to completion, so the comparison covers the
    // propagation reaching a fixed point rather than a bounded number of ticks.
    let run = || {
        let registry = registry();
        let table = table(&registry);
        let mut world = flat();
        world.set(
            BlockPos::new(0, 0, 0),
            block(&registry, "minecraft:redstone_block"),
        );
        for x in 1..=12 {
            world.set(BlockPos::new(x, 0, 0), wire(&registry, PowerLevel::ZERO));
        }
        let mut queue = UpdateQueue::new();
        prepare(&mut queue, BlockPos::new(0, 0, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        let powers: Vec<Option<u8>> = (0..12)
            .map(|index| common::wire_power_at(&world, &registry, BlockPos::new(index + 1, 0, 0)))
            .collect();
        (report.changes, powers)
    };
    let (first_changes, first_powers) = run();
    let (second_changes, second_powers) = run();
    assert_eq!(first_changes.len(), 12, "12 wires change");
    assert_eq!(first_changes, second_changes);
    assert_eq!(first_powers, second_powers);
    // And the transcript is in ascending position order, which is the queue's order.
    let positions: Vec<BlockPos> = first_changes.iter().map(|change| change.pos).collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(
        positions, sorted,
        "changes are reported in processing order"
    );
    assert!(
        first_changes
            .iter()
            .all(|change| change.cause == mc_redstone::UpdateKind::NeighborChanged),
        "every propagated change is a neighbour update in this pass"
    );
}

#[test]
fn the_change_transcript_records_the_ids_and_the_emission() {
    // The transcript is the evidence, so its contents must be checkable, not just its equality
    // between runs.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.changes.len(), 1);
    let change = report.changes[0];
    assert_eq!(change.pos, BlockPos::new(1, 0, 0));
    assert_eq!(change.old_state, wire(&registry, PowerLevel::ZERO));
    assert_eq!(change.new_state, wire(&registry, level(14)));
    assert_eq!(change.emitted.effective(), level(14));
    assert_eq!(change.cause, mc_redstone::UpdateKind::NeighborChanged);
}

#[test]
fn splitting_the_work_across_ticks_keeps_the_order_and_the_end_state() {
    // **The contract, stated precisely.** The queue's order is by position, so the *relative*
    // order of the changes does not depend on how the work was chunked — but a split run does
    // not report the same transcript, because an entry that was not reached this tick is
    // recomputed under different neighbouring values next tick. The weak run therefore produces
    // a prefix of the strong run's changes (it stops earlier), and the same values for whatever
    // it did reach.
    //
    // A test that claimed full sequence equality here would be asserting something the algorithm
    // does not promise. What the algorithm does promise is what is checked: order is preserved,
    // and the fixed point is the same.
    let registry = registry();
    let build = || {
        let mut world = flat();
        // Contiguous wire: the source at x = 0 touches the wire at x = 1, and each wire touches
        // the next. Power does not jump a gap, so the chain has to be solid.
        world.set(
            BlockPos::new(0, 0, 0),
            component(&registry, ComponentState::Lever(Lever::new(true))),
        );
        for x in 1..=10 {
            world.set(BlockPos::new(x, 0, 0), wire(&registry, PowerLevel::ZERO));
        }
        world
    };
    let table = table(&registry);

    let mut big_world = build();
    let mut big_queue = UpdateQueue::new();
    prepare(&mut big_queue, BlockPos::new(0, 0, 0));
    let whole = propagate(
        &mut big_world,
        &mut big_queue,
        table,
        UpdateBudget::nominal(),
    );

    let mut small_world = build();
    let mut small_queue = UpdateQueue::new();
    prepare(&mut small_queue, BlockPos::new(0, 0, 0));

    let powers = |world: &mc_redstone::FlatWorld| -> Vec<Option<u8>> {
        (1..=10)
            .map(|x| common::wire_power_at(world, &registry, BlockPos::new(x, 0, 0)))
            .collect()
    };
    let target = powers(&big_world);

    // Run the split until it reaches the *same state* the unbounded run reached. A generous but
    // still-binding per-tick budget (16) is used rather than a tiny one: a hard budget does change
    // the observed sequence, because an entry not reached this tick is recomputed under different
    // neighbouring values next tick. That is a real and documented property, so what this test
    // asserts is what the algorithm actually promises — the same relative order and the same end
    // state. The bounding itself is covered by `a_tiny_budget_bounds_each_tick_exactly`.
    let mut pieces = Vec::new();
    let mut tick = 0u64;
    while tick < 2000 && powers(&small_world) != target {
        tick += 1;
        let report = run_block_tick(
            &mut small_world,
            &mut small_queue,
            table,
            tick,
            UpdateBudget::new(16, 8),
        );
        assert!(
            report.updates_processed <= 16,
            "tick {tick}: the budget must bound the work exactly"
        );
        pieces.extend(report.changes);
    }
    assert!(
        tick < 2000,
        "the split run must reach the unbounded run's state (got to tick {tick})"
    );

    assert_eq!(whole.blocks_changed, 10, "ten wires change in one pass");
    assert_eq!(
        pieces.len(),
        whole.changes.len(),
        "both runs report the same number of changes in total"
    );
    // Same order and the same values, one entry at a time.
    let whole_positions: Vec<BlockPos> = whole.changes.iter().map(|c| c.pos).collect();
    let piece_positions: Vec<BlockPos> = pieces.iter().map(|c| c.pos).collect();
    assert_eq!(
        whole_positions, piece_positions,
        "the relative order of the changes must not depend on the chunking"
    );
    for (one, other) in whole.changes.iter().zip(pieces.iter()) {
        assert_eq!(one, other, "the same change must be reported the same way");
    }

    // And the final state agrees, which is the property the work item asks for by name.
    assert_eq!(powers(&small_world), target);
    for x in 1..=10 {
        let pos = BlockPos::new(x, 0, 0);
        assert_eq!(
            common::wire_power_at(&big_world, &registry, pos),
            common::wire_power_at(&small_world, &registry, pos),
            "{pos}"
        );
    }
}

#[test]
fn a_budget_of_zero_leaves_the_queue_exactly_as_it_was() {
    // The "do nothing this tick" path must not consume, reorder or drop anything, or a lag spike
    // would silently change a circuit's timing.
    let registry = registry();
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    for x in 1..=4 {
        world.set(BlockPos::new(x, 0, 0), wire(&registry, PowerLevel::ZERO));
    }
    let table = table(&registry);
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let before: Vec<BlockPos> = queue.pending_neighbours();
    assert!(!before.is_empty(), "there is work to defer");

    let report = propagate(&mut world, &mut queue, table, UpdateBudget::new(0, 0));
    assert_eq!(
        report.updates_processed, 0,
        "a zero budget processes nothing"
    );
    assert_eq!(report.blocks_changed, 0);
    assert!(
        report.budget_exhausted,
        "the budget stopped the call and work was waiting"
    );
    assert_eq!(
        queue.pending_neighbours(),
        before,
        "nothing consumed, nothing dropped, order unchanged"
    );
    assert_eq!(queue.len(), before.len());

    // The next call with a real budget finishes the job, so nothing was lost.
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(report.updates_processed > 0);
    assert!(
        !report.budget_exhausted,
        "the nominal budget is enough here"
    );
    for x in 1..=4 {
        assert_eq!(
            common::wire_power_at(&world, &registry, BlockPos::new(x, 0, 0)),
            Some(15 - u8::try_from(x).expect("fits")),
            "wire at x = {x}"
        );
    }
}
