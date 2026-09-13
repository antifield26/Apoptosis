//! The light cache: that it is used, and that it is dropped when it should be.
//!
//! A cache that is silently bypassed is worse than no cache — the code reads as if it saves work and does not
//! — and a cache that is not invalidated serves light that no longer matches the world. Both failures are
//! invisible: the packet stays well-formed and the client renders whatever it is told. So the two properties
//! are asserted rather than assumed.

use mc_persistence::chunk::ChunkPos;
use mc_persistence::dimension::Dimension;
use mc_registry::Registries;
use mc_world::World;
use mc_world::light::MAX_LIGHT;

/// A world from the shipped registry tables, with one air chunk loaded.
fn world_with_chunk(pos: ChunkPos) -> (World, Registries) {
    let registries = Registries::vanilla().expect("the shipped registry tables load");
    let mut world = World::new(Dimension::Overworld, registries.blocks.clone());
    world.ensure_chunk(pos);
    (world, registries)
}

#[test]
fn computing_light_puts_it_in_the_cache_and_a_second_call_is_a_hit() {
    let pos = ChunkPos::new(0, 0);
    let (mut world, registries) = world_with_chunk(pos);

    assert_eq!(
        world.light_cache_len(),
        0,
        "nothing is cached before asking"
    );
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    assert_eq!(
        world.light_cache_len(),
        1,
        "the cache is filled, not bypassed"
    );

    // A second call must not add anything: same chunk, same entry.
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    assert_eq!(world.light_cache_len(), 1);
}

#[test]
fn the_cached_light_is_what_a_direct_computation_produces() {
    // The cache is only worth having if it holds the *same* answer. An air chunk is uniformly lit, which is a
    // weak shape for this — but it is enough to catch a cache keyed wrongly or storing a stale entry.
    let pos = ChunkPos::new(3, -7);
    let (mut world, registries) = world_with_chunk(pos);
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    let cached = world.cached_light(pos).expect("cached").clone();

    let direct = mc_world::light::compute_chunk_light(
        &registries.light,
        pos.x,
        pos.z,
        world.chunk(pos).expect("chunk").min_y(),
        world.chunk(pos).expect("chunk").sections.len(),
        |x, y, z| world.get_block_loaded(x, y, z),
    )
    .expect("computes");

    assert_eq!(
        cached, direct,
        "the cache must hold what a direct computation gives"
    );
    assert_eq!(
        cached
            .sky
            .last()
            .and_then(mc_world::light::LightArray::uniform),
        Some(MAX_LIGHT),
        "an air chunk is fully sky-lit, so this is not comparing two empty results"
    );
}

#[test]
fn a_block_change_drops_the_changed_chunk() {
    let pos = ChunkPos::new(0, 0);
    let (mut world, registries) = world_with_chunk(pos);
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    assert_eq!(world.light_cache_len(), 1);

    let stone = registries
        .blocks
        .default_state("minecraft:stone")
        .expect("stone exists");
    world.set_block(8, 0, 8, stone).expect("sets");
    assert_eq!(
        world.light_cache_len(),
        0,
        "a change in the middle of a chunk invalidates that chunk"
    );
}

#[test]
fn a_change_on_a_border_also_drops_the_neighbour_whose_margin_reads_it() {
    let stone = Registries::vanilla()
        .expect("registries")
        .blocks
        .default_state("minecraft:stone")
        .expect("stone exists");

    // The margin is one block, so a block at local x = 0 is read by the chunk at x - 1, and one at local
    // x = 15 by the chunk at x + 1. Getting this wrong leaves a neighbour's edge lit for a block that is no
    // longer there, which is exactly the kind of wrong that produces no error.
    for (local_x, neighbour_dx, label) in [(0, -1, "west"), (15, 1, "east")] {
        let pos = ChunkPos::new(0, 0);
        let (mut world, registries) = world_with_chunk(pos);
        world.ensure_chunk(ChunkPos::new(neighbour_dx, 0));
        world
            .compute_light(pos, &registries.light)
            .expect("computes");
        world
            .compute_light(ChunkPos::new(neighbour_dx, 0), &registries.light)
            .expect("computes");
        assert_eq!(world.light_cache_len(), 2);

        world.set_block(local_x, 0, 8, stone).expect("sets");
        assert!(
            world.cached_light(pos).is_none(),
            "the changed chunk is dropped ({label} case)"
        );
        assert!(
            world.cached_light(ChunkPos::new(neighbour_dx, 0)).is_none(),
            "and so is the {label} neighbour, whose margin reads that border block"
        );
    }

    // A change in the middle must NOT drop the neighbours, or the invalidation is simply "drop everything"
    // wearing a condition.
    let pos = ChunkPos::new(0, 0);
    let (mut world, registries) = world_with_chunk(pos);
    world.ensure_chunk(ChunkPos::new(1, 0));
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    world
        .compute_light(ChunkPos::new(1, 0), &registries.light)
        .expect("computes");
    world.set_block(8, 0, 8, stone).expect("sets");
    assert!(
        world.cached_light(ChunkPos::new(1, 0)).is_some(),
        "an interior change must leave the neighbours alone"
    );
}

#[test]
fn unloading_a_chunk_drops_its_light() {
    let pos = ChunkPos::new(0, 0);
    let (mut world, registries) = world_with_chunk(pos);
    world
        .compute_light(pos, &registries.light)
        .expect("computes");
    assert_eq!(world.light_cache_len(), 1);
    world.unload_chunk(pos);
    assert_eq!(
        world.light_cache_len(),
        0,
        "an unloaded chunk keeps no light"
    );
}

#[test]
fn clearing_drops_everything() {
    let (mut world, registries) = world_with_chunk(ChunkPos::new(0, 0));
    world.ensure_chunk(ChunkPos::new(1, 0));
    world
        .compute_light(ChunkPos::new(0, 0), &registries.light)
        .expect("computes");
    world
        .compute_light(ChunkPos::new(1, 0), &registries.light)
        .expect("computes");
    assert_eq!(world.light_cache_len(), 2);
    world.clear_light();
    assert_eq!(world.light_cache_len(), 0);
}
