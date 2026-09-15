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
fn a_change_on_a_corner_also_drops_the_diagonal_neighbour() {
    // AUDIT-09 B-05. The margin is one block on *all four* sides, so a block at
    // local (0, 0) is inside the margin of the chunk at (x - 1, z - 1) as well as
    // of the two axis neighbours. The rule was written as four independent `if`s
    // over the four edges, which cannot express the diagonal: a torch in a corner
    // left the diagonal chunk's cached light stale, and (in the server's copy of
    // the same rule) left the client drawing it.
    let stone = Registries::vanilla()
        .expect("registries")
        .blocks
        .default_state("minecraft:stone")
        .expect("stone exists");
    let pos = ChunkPos::new(0, 0);
    let (mut world, registries) = world_with_chunk(pos);
    let diagonals = [ChunkPos::new(-1, -1), ChunkPos::new(-1, 1)];
    for diagonal in diagonals {
        world.ensure_chunk(diagonal);
    }
    for chunk in [pos, diagonals[0], diagonals[1]] {
        world
            .compute_light(chunk, &registries.light)
            .expect("computes");
    }
    assert_eq!(world.light_cache_len(), 3, "three chunks are cached");

    // Local (0, 0) of chunk (0, 0) is world (0, 0): the north-west corner shared
    // with chunk (-1, -1).
    world.set_block(0, 0, 0, stone).expect("sets");
    for chunk in [pos, diagonals[0]] {
        assert!(
            world.cached_light(chunk).is_none(),
            "the corner change drops {chunk:?}, whose margin reads it"
        );
    }
}

/// The chunks whose cached light a block at world `(x, z)` can change, derived
/// from the margin as an **interval** rather than from the case analysis the
/// implementation uses.
///
/// A chunk `cx` computes light from blocks with `x` in
/// `[cx * 16 - 1, cx * 16 + 16]`, so the block belongs to every chunk satisfying
/// `cx * 16 + 16 >= x` and `cx * 16 - 1 <= x`, i.e.
/// `cx` in `[(x - 1) / 16, (x + 1) / 16]` with floor division. The two
/// derivations share only the statement of the margin, so agreeing is evidence
/// and not a tautology.
fn chunks_from_the_margin_interval(x: i32, z: i32) -> Vec<ChunkPos> {
    // Named for the axis each end belongs to, rather than `cx_min`/`cz_min`, so the
    // four names cannot be confused for one another at a glance.
    let first_chunk_x = (x - 1).div_euclid(16);
    let last_chunk_x = (x + 1).div_euclid(16);
    let first_chunk_z = (z - 1).div_euclid(16);
    let last_chunk_z = (z + 1).div_euclid(16);
    let mut out = Vec::new();
    for chunk_x in first_chunk_x..=last_chunk_x {
        for chunk_z in first_chunk_z..=last_chunk_z {
            out.push(ChunkPos::new(chunk_x, chunk_z));
        }
    }
    out
}

#[test]
fn the_affected_chunk_rule_agrees_with_the_margin_derived_from_its_definition() {
    // Exhaustive over every local position of two chunks, one of them negative in
    // both axes so a sign or `rem_euclid` mistake is visible. The function takes
    // the block's **own** chunk, which is how the callers use it, so the domain is
    // local 0..16 rather than the wider range a mistaken reading might assume.
    for pos in [ChunkPos::new(0, 0), ChunkPos::new(-3, 5)] {
        let origin_x = pos.x * 16;
        let origin_z = pos.z * 16;
        let mut checked = 0usize;
        for local_x in 0..16 {
            for local_z in 0..16 {
                let (x, z) = (origin_x + local_x, origin_z + local_z);
                let mut ours = mc_world::world::chunks_a_block_can_light(pos, x, z);
                let mut expected = chunks_from_the_margin_interval(x, z);
                ours.sort_unstable();
                expected.sort_unstable();
                assert_eq!(
                    ours, expected,
                    "block ({x}, {z}) relative to {pos:?}: the rule and the margin interval disagree"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 16 * 16, "every local position was compared");
    }
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
