//! The `mc_world::World` implementation of [`BlockView`].
//!
//! The rest of the suite runs against [`mc_redstone::FlatWorld`], which is the right harness for
//! testing the *rule*. This file is the evidence that the rule also works against the **real** world
//! type the server uses, and that the two integration decisions in that `impl` hold:
//!
//! 1. a read of an **unloaded** chunk returns `None` rather than air, so the algorithm knows the
//!    difference between "no signal here" and "this crate must not reason about this position";
//! 2. an update can never **create** a chunk, because a redstone change must not be able to make the
//!    server generate terrain (AGENTS.md §10);
//! 3. a write goes through `World::set_block`, so the change is recorded in the world's change list
//!    for broadcast and saving rather than being applied behind the world's back.

mod common;

use common::{component, level, registry, table, wire};
use mc_persistence::dimension::Dimension;
use mc_redstone::components::{ComponentState, Lever};
use mc_redstone::propagation::{BlockRole, BlockView, prepare, propagate, read_block};
use mc_redstone::{BlockPos, PowerLevel, UpdateBudget, UpdateQueue};
use mc_world::World;
use mc_world::chunk::ChunkPos;

/// A world with one loaded chunk covering `(0, 0)`, so `x, z` in `0..16` are readable.
///
/// `World::new` takes a `Dimension`, which lives in `mc-persistence` and is not re-exported by
/// `mc-world`, hence the `mc-persistence` dev-dependency. It is a dev-dependency only: the library
/// needs nothing from persistence, and the world type is still reached through `mc-world`.
fn loaded_world(registry: &mc_registry::BlockRegistry) -> World {
    let mut world = World::new(Dimension::Overworld, registry.clone());
    // `ensure_chunk` is the documented all-air placeholder; the test loads exactly what it needs and
    // asserts the rest is unloaded.
    let _ = world.ensure_chunk(ChunkPos::new(0, 0));
    world
}

#[test]
fn reads_come_from_the_world_and_writes_are_recorded_in_its_change_list() {
    let registry = registry();
    let table = table(&registry);
    let mut world = loaded_world(&registry);

    // A lever and a wire, written through the world's own API.
    let lever = component(&registry, ComponentState::Lever(Lever::new(true)));
    let mut world = {
        world.set_block(0, 64, 0, lever).expect("lever writes");
        for x in 1..=4 {
            world
                .set_block(x, 64, 0, wire(&registry, PowerLevel::ZERO))
                .expect("wire writes");
        }
        world
    };
    // The setup writes are noise for this assertion, so drain them.
    let _ = world.take_block_changes();

    let mut queue = UpdateQueue::new();
    prepare(&mut queue, BlockPos::new(0, 64, 0));
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());

    // The four wires changed, and the power is the hand-computed 15, 14, 13, 12
    // (P13-05, measured: the first dust off a source carries the full strength).
    assert_eq!(report.blocks_changed, 4, "four wires change");
    for (index, x) in (1..=4).enumerate() {
        let expected = 15 - u8::try_from(index).expect("fits");
        let id = world.get_block(x, 64, 0);
        assert_eq!(
            table.classify(id),
            BlockRole::Wire {
                stored: level(expected)
            },
            "wire at x = {x}"
        );
    }

    // Integration point 3: every write is in the world's change list, in order, with both ids.
    let changes = world.take_block_changes();
    assert_eq!(changes.len(), 4, "one recorded change per wire");
    assert!(
        world.block_changes().is_empty(),
        "take_block_changes drains the list"
    );
    for (change, x) in changes.iter().zip(1..=4) {
        assert_eq!((change.x, change.y, change.z), (x, 64, 0));
        assert_eq!(change.pos, ChunkPos::new(0, 0));
        assert_eq!(change.old_id, wire(&registry, PowerLevel::ZERO));
        assert_ne!(
            change.new_id, change.old_id,
            "a recorded change is a real one"
        );
    }
    // And the wires changed in ascending position order, which is the queue's order.
    let xs: Vec<i32> = changes.iter().map(|change| change.x).collect();
    assert_eq!(xs, vec![1, 2, 3, 4]);
}

#[test]
fn an_unloaded_chunk_reads_as_none_and_is_never_created_by_an_update() {
    let registry = registry();
    let table = table(&registry);
    let mut world = loaded_world(&registry);
    // A live wire inside the loaded chunk, and a queued update for a position in a chunk that is not
    // loaded at all.
    world
        .set_block(1, 64, 0, wire(&registry, PowerLevel::MAX))
        .expect("wire writes");
    // The setup write is noise for the assertions below, so drain it.
    let _ = world.take_block_changes();
    let far = BlockPos::new(1000, 64, 1000);
    assert!(!world.is_loaded(ChunkPos::new(62, 62)));

    // Integration point 1: the read distinguishes "unloaded" from "air".
    assert_eq!(
        world.get_state(far),
        None,
        "an unloaded chunk reads as None"
    );
    assert_eq!(
        world.get_block(far.x, far.y, far.z),
        0,
        "and `get_block` still answers air for callers that want a block"
    );

    // The unloaded position is not updatable: `read_block` refuses it.
    assert!(read_block(&world, table, far).is_none());

    // Integration point 2: an update for it neither panics nor creates the chunk.
    let mut queue = UpdateQueue::new();
    queue.push_neighbour(far);
    prepare(&mut queue, far);
    let before = world.chunk_count();
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.blocks_changed, 0);
    assert_eq!(
        world.chunk_count(),
        before,
        "a redstone update must never generate terrain"
    );
    assert!(!world.is_loaded(ChunkPos::new(62, 62)));
    assert!(
        world.block_changes().is_empty(),
        "nothing was written, so nothing is recorded"
    );
}

#[test]
fn a_position_outside_the_world_height_is_refused_without_panicking() {
    // `World::set_block` refuses a `y` outside the section range. The `BlockView` impl must turn that
    // into `false`, not into a panic (AGENTS.md §9), and the propagation loop must treat it as "not
    // updatable" rather than assuming the write landed.
    let registry = registry();
    let table = table(&registry);
    let mut world = loaded_world(&registry);
    let high = BlockPos::new(1, 10_000, 1);
    assert_eq!(
        world.get_state(high),
        Some(0),
        "a read above the world is air, like Vanilla"
    );
    assert!(
        !BlockView::set_state(&mut world, high, wire(&registry, PowerLevel::MAX)),
        "an out-of-range write must be refused, not panic"
    );
    assert!(!BlockView::set_state(
        &mut world,
        BlockPos::new(1, i32::MIN, 1),
        wire(&registry, PowerLevel::MAX)
    ));

    // And a queued update at that position is a no-op rather than an abort.
    let mut queue = UpdateQueue::new();
    queue.push_neighbour(high);
    let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
    assert_eq!(report.blocks_changed, 0);
}

#[test]
fn the_same_scenario_against_the_real_world_is_deterministic() {
    // The determinism property, against `mc_world::World` rather than the flat array: the change
    // *sequence* must be identical between runs.
    let run = || {
        let registry = registry();
        let table = table(&registry);
        let mut world = loaded_world(&registry);
        world
            .set_block(
                0,
                64,
                0,
                component(&registry, ComponentState::Lever(Lever::new(true))),
            )
            .expect("lever");
        for x in 1..=8 {
            world
                .set_block(x, 64, 0, wire(&registry, PowerLevel::ZERO))
                .expect("wire");
        }
        let _ = world.take_block_changes();
        let mut queue = UpdateQueue::new();
        prepare(&mut queue, BlockPos::new(0, 64, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        let recorded: Vec<(i32, i32, i32)> = world
            .take_block_changes()
            .iter()
            .map(|change| (change.x, change.y, change.z))
            .collect();
        (report.changes, recorded)
    };
    let (first, first_recorded) = run();
    let (second, second_recorded) = run();
    assert_eq!(first.len(), 8, "eight wires change");
    assert_eq!(first, second, "the change sequence must replay identically");
    assert_eq!(first_recorded, second_recorded);
    assert_eq!(
        first_recorded,
        (1..=8).map(|x| (x, 64, 0)).collect::<Vec<_>>(),
        "and in ascending position order"
    );
}
