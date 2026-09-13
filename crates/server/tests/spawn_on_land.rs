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

    // And the search moved it rather than the default having been lucky, so a run that passes because (0, 0)
    // happened to be dry cannot hide a search that never fires.
    assert_ne!(
        (sx, sz),
        (0, 0),
        "the default spawn at the origin is ocean at this seed, so a spawn still there means no search ran"
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
