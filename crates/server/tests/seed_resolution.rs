//! World-seed plumbing end to end (AUDIT-18 F-H1): a configured seed reaches
//! the live game on a fresh world, and the recorded seed survives a reboot
//! with the config key removed (stored wins over configured).
//!
//! Falsification: hardcode seed 0 back into `open_world` and the first
//! assertion goes red; drop the write-back in `WorldService::open` and the
//! second boot falls back to 0.

use mc_server::lifecycle::Server;
use mc_test_support::fixtures::TempDir;

fn config_for(dir: &TempDir, seed: Option<i64>) -> mc_server::config::ServerConfig {
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.storage.seed = seed;
    config
}

fn game_seed(server: &Server<mc_server::lifecycle::NoopHook>) -> i64 {
    server
        .game()
        .expect("game built by open_world")
        .random_seed()
}

#[test]
fn configured_seed_reaches_the_game_and_survives_unset() {
    let dir = TempDir::new("seed-plumbing");
    let world_dir = dir.path().join("world");

    // Fresh world + configured seed: the game generates from it, and the
    // seed is recorded in level.dat.
    let mut server = Server::new(config_for(&dir, Some(1_361_882_806)));
    server.open_world().expect("world opens");
    assert_eq!(game_seed(&server), 1_361_882_806);
    drop(server);

    // Same directory, config key removed: the recorded seed still wins over
    // the historical seed-0 default.
    let mut plain = mc_server::config::ServerConfig::default();
    plain.storage.world_dir = world_dir;
    plain.storage.autosave_ticks = 0;
    let mut server = Server::new(plain);
    server.open_world().expect("world reopens");
    assert_eq!(
        game_seed(&server),
        1_361_882_806,
        "the stored seed must survive dropping the config key"
    );
}
