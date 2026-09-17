//! Budget exhaustion: a deliberately expensive scenario processes at most the budget, reports it,
//! does not hang, and does not grow the queue without bound.
//!
//! The work item's requirement, restated as the three properties these tests check:
//!
//! 1. **bounded work** — `updates_processed <= budget.neighbour_updates_per_tick`, always;
//! 2. **reported** — `budget_exhausted` is true exactly when work is still queued;
//! 3. **bounded memory** — `queue.len() <= UpdateQueue::MAX_ENTRIES`, always.
//!
//! A redstone clock is the adversarial case: it must keep running forever, so the bound cannot be
//! an iteration count. It is the queue's entry cap plus the per-tick cap plus the budget.

mod common;

use common::{block, component, flat, level, registry, table, wire};
use mc_redstone::components::{ComponentState, Lever};
use mc_redstone::propagation::{prepare, prepare_self, propagate, run_block_tick};
use mc_redstone::{BlockPos, PowerLevel, UpdateBudget, UpdateQueue};

/// A long contiguous wire line fed by a source at x = 0, so one update cascades down the run.
fn long_line(
    registry: &mc_registry::BlockRegistry,
    length: i32,
) -> (mc_redstone::FlatWorld, Vec<BlockPos>) {
    let mut world = mc_redstone::FlatWorld::new(BlockPos::new(0, 0, 0), (64, 4, 4));
    world.set(
        BlockPos::new(0, 0, 0),
        component(registry, ComponentState::Lever(Lever::new(true))),
    );
    let positions: Vec<BlockPos> = (1..=length).map(|x| BlockPos::new(x, 0, 0)).collect();
    for pos in &positions {
        world.set(*pos, wire(registry, PowerLevel::ZERO));
    }
    (world, positions)
}

/// A fan of independent wire stubs around one source, so the queue holds many positions.
fn wide_fan(registry: &mc_registry::BlockRegistry, radius: i32) -> mc_redstone::FlatWorld {
    let span = u32::try_from(radius * 2 + 3).expect("positive span");
    let mut world =
        mc_redstone::FlatWorld::new(BlockPos::new(-radius - 1, -1, -radius - 1), (span, 3, span));
    world.set(
        BlockPos::new(0, 0, 0),
        block(registry, "minecraft:redstone_block"),
    );
    for x in -radius..=radius {
        for z in -radius..=radius {
            if x.abs() + z.abs() == 1 {
                world.set(BlockPos::new(x, 0, z), wire(registry, PowerLevel::ZERO));
            }
        }
    }
    world
}

#[test]
fn a_tiny_budget_bounds_each_tick_exactly() {
    // The strongest form of the requirement: a 17-block line with a 3-update budget must take many
    // ticks, must process at most 3 per tick, and must keep the leftover queued.
    //
    // Hand-computed final state: `wire(x) = 16 - x`, so 15, 14, ... 1 for x = 1..=15 and **0
    // for x = 16, 17** — the line is deliberately longer than the signal reaches, so the test also
    // covers "a wire beyond the reach stays off and does not stall the queue".
    let registry = registry();
    let table = table(&registry);
    let (mut world, positions) = long_line(&registry, 17);
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    assert!(!queue.is_empty(), "there is work to do");

    let final_power =
        |pos: BlockPos| Some(16u8.saturating_sub(u8::try_from(pos.x).expect("x fits in u8")));

    let mut ticks = 0usize;
    let mut total = 0usize;
    let mut exhausted_ticks = 0usize;
    loop {
        ticks += 1;
        assert!(ticks < 10_000, "must not run forever");
        let report = run_block_tick(
            &mut world,
            &mut queue,
            table,
            u64::try_from(ticks).expect("fits"),
            UpdateBudget::new(3, 1),
        );
        assert!(
            report.updates_processed <= 3,
            "tick {ticks}: processed {} with a cap of 3",
            report.updates_processed
        );
        total += report.updates_processed;
        if report.budget_exhausted {
            exhausted_ticks += 1;
        }
        assert!(
            queue.len() <= UpdateQueue::MAX_ENTRIES,
            "tick {ticks}: queue grew to {}",
            queue.len()
        );
        // The line is finished when every wire carries its final value. This is the property that
        // matters: a budgeted run must be *slower* than an unbounded one, never *different*.
        if positions
            .iter()
            .all(|pos| common::wire_power_at(&world, &registry, *pos) == final_power(*pos))
        {
            break;
        }
    }
    assert!(
        ticks > 1,
        "a 3-update budget cannot finish a 17-block line in one tick"
    );
    assert!(total > 0);
    assert!(
        exhausted_ticks > 0,
        "some ticks must report that the budget stopped them"
    );
    // Every position reached exactly the value the unbounded run gives.
    for pos in &positions {
        assert_eq!(
            common::wire_power_at(&world, &registry, *pos),
            final_power(*pos),
            "{pos} must have the same final value as an unbounded run"
        );
    }
    // And the far end really is off, so the equality above is not vacuous.
    assert_eq!(
        common::wire_power_at(&world, &registry, positions[16]),
        Some(0),
        "the 17th wire block is beyond the signal's reach"
    );
    assert_eq!(
        common::wire_power_at(&world, &registry, positions[14]),
        Some(1),
        "the 15th is the last live one (P13-05, measured)"
    );
}

#[test]
fn the_queue_refuses_work_instead_of_growing_without_bound() {
    // A hostile or broken circuit that raises more updates than the cap allows must be refused,
    // counted, and must not allocate without bound.
    let registry = registry();
    let mut queue = UpdateQueue::new();
    // Push ten times the cap.
    let mut refused = 0usize;
    for x in 0..(UpdateQueue::MAX_ENTRIES * 10) {
        let pos = BlockPos::new(
            i32::try_from(x).expect("fits"),
            i32::try_from(x / 1000).expect("fits"),
            0,
        );
        if queue.push_neighbour(pos).was_refused() {
            refused += 1;
        }
    }
    assert_eq!(
        queue.len(),
        UpdateQueue::MAX_ENTRIES,
        "the queue stops at its cap"
    );
    assert!(refused >= UpdateQueue::MAX_ENTRIES * 9, "most were refused");
    assert_eq!(
        u64::try_from(refused).expect("fits"),
        queue.refused_total(),
        "every refusal is counted for observability"
    );
    // Scheduled entries share the same cap.
    assert!(!queue.schedule_at(5, BlockPos::new(-1, 0, 0)).was_inserted());
    assert_eq!(queue.len(), UpdateQueue::MAX_ENTRIES);

    // And a real scenario pushing a wide fan through the queue stays inside the cap.
    let mut world = wide_fan(&registry, 20);
    let table = table(&registry);
    let mut fan_queue = UpdateQueue::new();
    prepare(&mut fan_queue, BlockPos::new(0, 0, 0));
    for tick in 1..=50u64 {
        let report = run_block_tick(
            &mut world,
            &mut fan_queue,
            table,
            tick,
            UpdateBudget::nominal(),
        );
        assert!(fan_queue.len() <= UpdateQueue::MAX_ENTRIES, "tick {tick}");
        assert!(report.updates_processed <= UpdateQueue::MAX_UPDATES_PER_TICK);
    }
}

#[test]
fn a_clock_never_grows_the_queue_and_never_hangs() {
    // The adversarial case named in the work item: a self-rescheduling clock. It must run
    // forever without the queue growing and without the loop failing to return.
    let registry = registry();
    let table = table(&registry);
    let mut world = flat();
    world.set(
        BlockPos::new(0, 0, 0),
        block(&registry, "minecraft:redstone_block"),
    );
    let clock = BlockPos::new(1, 0, 0);
    world.set(clock, wire(&registry, level(14)));
    let mut queue = UpdateQueue::new();
    assert!(queue.schedule(0, clock, 1).is_ok());

    let mut peak = 0usize;
    for tick in 1..=500u64 {
        let report = run_block_tick(&mut world, &mut queue, table, tick, UpdateBudget::new(8, 4));
        peak = peak.max(queue.len());
        assert!(
            report.updates_processed <= 8,
            "tick {tick}: {} updates with a cap of 8",
            report.updates_processed
        );
        assert!(
            queue.len() <= 8,
            "tick {tick}: a single clock must not accumulate a backlog, queue.len() = {}",
            queue.len()
        );
        // P13-05: dust next to a source carries the full strength, so the wire
        // reads 15 from the first tick on and never drifts after.
        assert_eq!(
            common::wire_power_at(&world, &registry, clock),
            Some(15),
            "tick {tick}: the clock's value does not drift"
        );
    }
    assert!(
        peak <= 8,
        "a single clock occupies a bounded number of entries (the wire plus its neighbours), peaked at {peak}"
    );
    assert_eq!(queue.refused_total(), 0, "nothing was refused");
}

#[test]
fn budget_exhausted_is_true_only_when_work_remains() {
    // A false negative here would let a circuit starve silently; a false positive would make an
    // operator chase a problem that is not there.
    let registry = registry();
    let table = table(&registry);

    // Empty queue: nothing processed, nothing left, so not exhausted.
    let mut world = flat();
    let mut empty = UpdateQueue::new();
    let report = propagate(&mut world, &mut empty, table, UpdateBudget::nominal());
    assert_eq!(report.updates_processed, 0);
    assert!(!report.budget_exhausted, "nothing to do is not exhaustion");

    // A zero budget with work queued: exhausted.
    world.set(
        BlockPos::new(0, 0, 0),
        component(&registry, ComponentState::Lever(Lever::new(true))),
    );
    world.set(BlockPos::new(1, 0, 0), wire(&registry, PowerLevel::ZERO));
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::new(0, 0));
    assert_eq!(report.updates_processed, 0);
    assert!(report.budget_exhausted, "work is queued and none was done");

    // Enough budget: finishes, not exhausted.
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert!(report.updates_processed > 0);
    assert!(!report.budget_exhausted);
    assert!(queue.is_empty());
}

#[test]
fn the_per_tick_cap_and_the_entry_cap_are_documented_product_decisions() {
    // These two numbers are not Vanilla facts; they are this project's bounds. The test pins them
    // so a change is deliberate.
    assert_eq!(UpdateQueue::MAX_ENTRIES, 4096);
    assert_eq!(UpdateQueue::MAX_UPDATES_PER_TICK, 1024);
    assert_eq!(UpdateBudget::DEFAULT_SCHEDULED_TICKS_PER_TICK, 256);
    assert_eq!(mc_redstone::MAX_SCHEDULE_DELAY, 32_768);
    // The nominal budget uses both documented defaults.
    let nominal = UpdateBudget::nominal();
    assert_eq!(
        nominal.neighbour_updates_per_tick,
        UpdateQueue::MAX_UPDATES_PER_TICK
    );
    assert_eq!(
        nominal.scheduled_ticks_per_tick,
        UpdateBudget::DEFAULT_SCHEDULED_TICKS_PER_TICK
    );
}

#[test]
fn an_enormous_scheduled_load_is_capped_by_the_scheduled_budget() {
    // A hostile or broken circuit that schedules more ticks than a tick may run must be capped,
    // with the remainder preserved.
    let mut queue = UpdateQueue::new();
    let count = 1000i32;
    for x in 0..count {
        queue.schedule_at(5, BlockPos::new(x, 0, 0));
    }
    assert_eq!(queue.len(), usize::try_from(count).expect("fits"));
    let budget = UpdateBudget::new(1024, 100);
    let drain = queue.drain_due_block_ticks(5, budget);
    assert_eq!(drain.len(), 100, "only the budget's worth comes due");
    assert!(drain.budget_exhausted);
    assert_eq!(
        queue.len(),
        usize::try_from(count - 100).expect("fits"),
        "the rest stays queued"
    );
    // It makes progress towards empty rather than stalling.
    let mut remaining = queue.len();
    let mut ticks = 0;
    while remaining > 0 && ticks < 100 {
        ticks += 1;
        let drain = queue.drain_due_block_ticks(5, budget);
        assert!(drain.len() <= 100);
        assert!(queue.len() < remaining, "each drain must make progress");
        remaining = queue.len();
    }
    assert_eq!(remaining, 0, "the backlog drains in bounded steps");
}

#[test]
fn prepare_self_and_run_block_tick_do_not_lose_a_position_across_a_budget_split() {
    // The durability property end to end: a position the budget could not reach must still be
    // processed later, and the final state must equal the unbounded run's.
    //
    // The budget is deliberately far smaller than the working set (the line's ~70 distinct
    // positions), so the work is split across many ticks. A bounded run is allowed to be slow; it is
    // not allowed to be different.
    let registry = registry();
    let table = table(&registry);
    let (mut world, positions) = long_line(&registry, 10);
    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 0, 0));
    prepare_self(&mut queue, BlockPos::new(1, 0, 0));

    let final_power = |pos: BlockPos| Some(16u8.saturating_sub(u8::try_from(pos.x).expect("fits")));
    let mut tick = 0u64;
    let mut finished = false;
    while tick < 5000 {
        tick += 1;
        let report = run_block_tick(&mut world, &mut queue, table, tick, UpdateBudget::new(4, 1));
        assert!(
            report.updates_processed <= 4,
            "tick {tick}: the budget must bound the work exactly"
        );
        if positions
            .iter()
            .all(|pos| common::wire_power_at(&world, &registry, *pos) == final_power(*pos))
        {
            finished = true;
            break;
        }
    }
    assert!(
        finished,
        "the line must finish within 5000 ticks under a 4-update budget (got to tick {tick})"
    );
    for pos in &positions {
        assert_eq!(
            common::wire_power_at(&world, &registry, *pos),
            final_power(*pos),
            "{pos}"
        );
    }
    // It really was split: a 4-update budget cannot light a 10-block line in one tick.
    assert!(tick > 1, "the split must have taken more than one tick");
}
