//! The spawn the server hands a player must not be open water.
//!
//! ## Why this is a test and not a preference
//!
//! A fresh world's `level.dat` names `(0, 64, 0)` and about **two columns in five** of this generator are ocean,
//! so the default put players in open sea. Everything the owner reported follows from that one point: the only
//! blocks in view were `stone`, `water` and `sand` — the three an ocean is made of — there were no trees and no
//! grass because an ocean has none, and the sea bed was **correctly** dark because water attenuates sky light.
//!
//! A world that starts a player in the middle of an ocean looks broken even though every subsystem in it is
//! working, and no unit test of terrain, light or blocks would say so. This is the assertion that would have.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, OutboundSender, game_channel,
};
use mc_server::config::StorageConfig;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_worldgen::OVERWORLD_SEA_LEVEL;

#[tokio::test]
async fn a_fresh_world_spawns_the_player_on_land() {
    let dir = TempDir::new("p10-spawn-land");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    // The production seed: the default spawn is water at this seed, which is the case being fixed.
    let mut game =
        Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
            .expect("game builds");

    let (sx, sy, sz) = game.spawn();
    let generator = game
        .terrain_generator()
        .expect("a fresh world has a generator");
    let surface = generator.surface_height(sx, sz);
    assert!(
        surface >= OVERWORLD_SEA_LEVEL,
        "spawn ({sx}, {sy}, {sz}) stands over y={surface}, below the sea level of {OVERWORLD_SEA_LEVEL}: \
         the player would start in open water"
    );

    // A joined player is placed at the spawn, and `set_default_spawn_position` is what the client is told.
    let id = ConnectionId(1);
    let (outbound, _receiver) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Spawner"),
                outbound,
            },
        })
        .expect("join queued");
    game.tick().expect("tick");
}

/// **The search itself**, with the one seed this test needs named explicitly.
///
/// The production-seed test above asserts the property — a fresh world starts a player on land — and that holds
/// whatever the seed. This one asserts that the land search *runs*, which can only be shown at a seed whose
/// origin is water, and it says which: **seed 0**, where the origin is ocean. The run's own log records the move:
/// "the stored spawn is under water; moved to the nearest land from_x=0 from_z=0 to_x=-8 to_z=-8".
///
/// The two were one test, and its assertion carried the seed dependence in its message while the body used
/// whatever the production seed happened to be. Changing the seed for an unrelated reason — a perturbation
/// meant to find exactly this class of defect — failed it, having found no defect at all.
#[tokio::test]
async fn the_land_search_moves_a_spawn_that_is_under_water() {
    /// The seed whose origin is ocean. Named here rather than inherited from production, because the test is
    /// *about* that precondition.
    const WATER_AT_ORIGIN_SEED: i64 = 0;

    let dir = TempDir::new("p10-spawn-search");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    let game =
        Game::with_seed_and_storage(storage, 4, rx, WATER_AT_ORIGIN_SEED).expect("game builds");

    let generator = game
        .terrain_generator()
        .expect("a fresh world has a generator");
    assert!(
        generator.surface_height(0, 0) < OVERWORLD_SEA_LEVEL,
        "this test needs a seed whose origin is under water, and {WATER_AT_ORIGIN_SEED} is supposed to be one"
    );

    let (sx, sy, sz) = game.spawn();
    assert_ne!(
        (sx, sz),
        (0, 0),
        "the origin is under water at this seed, so a spawn still there means the search never ran"
    );
    assert!(
        generator.surface_height(sx, sz) >= OVERWORLD_SEA_LEVEL,
        "the search moved the spawn to ({sx}, {sy}, {sz}), which is still under water"
    );
    let _ = events;
}
