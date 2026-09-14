//! The acceptance finding, reproduced in-process: the owner dug a staircase in
//! the live world and saw **dead-black surface patches in the dug region**.
//!
//! This test drives the same sequence the live session drove -- chunks around
//! spawn, dig a shaft down from the surface -- and reads the light the server
//! would send afterwards. If the recomputed light for the dug column is not
//! sky-lit, the engine's recompute-after-dig is the bug; if it IS sky-lit, the
//! bug is in the `light_update` wire encoding instead. Either way the test
//! names the layer.

use mc_server::game::Game;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

fn game() -> Game {
    let dir = TempDir::new("p10-light-repro");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let service = mc_server::storage::WorldService::open(&config).expect("world opens");
    Game::with_seed_and_storage(service, 3, mc_network::bridge::game_channel(64).1, 7)
        .expect("game builds")
}

#[test]
fn digging_a_shaft_lights_the_dug_column() {
    let mut game = game();

    // Load a block of chunks around the dig site so it has loaded neighbours,
    // the way the live session had.
    let (cx, cz) = (-1, 0);
    for dx in -2..=2 {
        for dz in -2..=2 {
            assert!(game.load_chunk(ChunkPos::new(cx + dx, cz + dz)));
        }
    }

    // Find the surface at the dig column, then open a four-block shaft down
    // from just above it -- the shape the owner dug.
    let (x, z) = (cx * 16 + 5, cz * 16 + 9);
    let mut surface = i32::MAX;
    for y in (-64..320).rev() {
        if game
            .world()
            .get_block_loaded(x, y, z)
            .is_some_and(|state| !game.registries().blocks.is_empty(state))
        {
            surface = y;
            break;
        }
    }
    assert_ne!(surface, i32::MAX, "a surface exists at the dig column");

    let chunk = ChunkPos::new(cx, cz);
    let chunk_min_y = game.world().chunk(chunk).expect("loaded").min_y();
    let light_table = game.registries().light.clone();
    let air_id = game.registries().blocks.air_id();
    game.world_mut()
        .compute_light(chunk, &light_table)
        .expect("initial light computes");
    let before = {
        let light = game
            .world()
            .cached_light(chunk)
            .expect("the initial light is cached");
        light.sky[usize::try_from((surface - 1 - chunk_min_y) / 16)
            .expect("the surface section is in range")]
        .get(
            x.rem_euclid(16),
            (surface - 1).rem_euclid(16),
            z.rem_euclid(16),
        )
    };

    // The dig: four blocks of the column become air, the way the acceptance
    // session's staircase went.
    for dy in 0..4 {
        game.world_mut()
            .set_block(x, surface - dy, z, air_id)
            .expect("the dig applies");
    }

    // set_block invalidates the cached light; the next compute must see the
    // shaft. This is the light the server would now send.
    game.world_mut()
        .compute_light(chunk, &light_table)
        .expect("re-light after the dig");
    let after = {
        let light = game
            .world()
            .cached_light(chunk)
            .expect("the recomputed light is cached");
        light.sky[usize::try_from((surface - 1 - chunk_min_y) / 16)
            .expect("the surface section is in range")]
        .get(
            x.rem_euclid(16),
            (surface - 1).rem_euclid(16),
            z.rem_euclid(16),
        )
    };

    assert_eq!(
        before, 0,
        "below the intact surface the sky light was dark before the dig (got {before})"
    );
    assert_eq!(
        after, 15,
        "the dug shaft must be sky-lit at full strength after the dig (got {after} -- the dead-black finding)"
    );

    // The wire layer (the light_update packet the server would send) is pinned
    // separately, inside the crate, where `light_fields` is reachable -- see
    // the light_fields test module in game.rs. The engine layer above is what
    // this in-process test can settle.
}
