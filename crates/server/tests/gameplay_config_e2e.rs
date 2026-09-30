//! Gameplay properties end to end (P19-06).
//!
//! Config + Game plumbing only: every key reads, writes and round-trips,
//! with enforcement P20-owned per key (named in `GameplayConfig` docs, not
//! stubbed here). The boot path installs the config values into the live
//! game.
//!
//! Falsification shape: drop a field from the install call and the boot
//! test finds the default where the config value belongs; break a setter
//! and its read-back goes red.

use mc_network::bridge::game_channel;
use mc_server::config::{DefaultGameMode, GameplayConfig};
use mc_server::game::Game;
use mc_server::lifecycle::Server;
use mc_test_support::fixtures::TempDir;

fn bare_game() -> Game {
    let (_tx, rx) = game_channel(64);
    Game::build_with_operators(
        None,
        None,
        4,
        rx,
        mc_server::game::DEFAULT_RANDOM_SEED,
        mc_server::ops::OperatorList::new(),
    )
    .expect("game builds")
}

#[test]
fn fresh_game_holds_the_documented_defaults() {
    let game = bare_game();
    assert_eq!(game.spawn_protection(), 16);
    assert!(game.pvp());
    assert_eq!(game.idle_timeout_minutes(), 0);
    assert_eq!(game.simulation_distance(), 8);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Survival);
    assert!(!game.hide_online_players());
}

#[test]
fn every_key_reads_back_what_was_written() {
    let mut game = bare_game();
    game.set_spawn_protection(0);
    game.set_pvp(false);
    game.set_idle_timeout_minutes(30);
    game.set_simulation_distance(10);
    game.set_default_gamemode(DefaultGameMode::Creative);
    game.set_hide_online_players(true);
    assert_eq!(game.spawn_protection(), 0);
    assert!(!game.pvp());
    assert_eq!(game.idle_timeout_minutes(), 30);
    assert_eq!(game.simulation_distance(), 10);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Creative);
    assert!(game.hide_online_players());
    // Whole-struct install is what the boot uses.
    game.set_gameplay_config(GameplayConfig::default());
    assert_eq!(game.spawn_protection(), 16);
    assert_eq!(game.gameplay().simulation_distance, 8);
}

#[test]
fn boot_installs_the_config_values() {
    let dir = TempDir::new("gameplay-boot");
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.gameplay.spawn_protection = 0;
    config.gameplay.pvp = false;
    config.gameplay.simulation_distance = 10;
    config.gameplay.default_gamemode = DefaultGameMode::Adventure;
    let mut server = Server::new(config);
    server.open_world().expect("world opens");
    let game = server.game().expect("game built");
    assert_eq!(game.spawn_protection(), 0);
    assert!(!game.pvp());
    assert_eq!(game.simulation_distance(), 10);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Adventure);
}
