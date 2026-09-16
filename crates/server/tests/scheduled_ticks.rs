//! Scheduled-tick queue wiring (P13-01).
//!
//! `mc-redstone`'s `UpdateQueue` proves its own ordering, dedup and budget
//! rules against a bare map. This file proves the *integration*: the
//! `ScheduledTicks` phase drains the game-owned queue every tick, under the
//! nominal budget, and the report says what fired and what is still queued.
//! Nothing in production schedules yet (the world feed is P13-02), so the
//! tests schedule directly through the same public call the feed will use.

use mc_network::bridge::game_channel;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

fn game(tag: &str) -> (Game, WorldService, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = game_channel(64);
    let game = Game::new(&storage, 3, rx).expect("game builds");
    (game, storage, dir)
}

#[test]
fn a_scheduled_tick_fires_exactly_once_at_its_due_tick() {
    let (mut game, _storage, _dir) = game("p13-due");
    game.schedule_block_tick(4, 64, 6, 3).expect("schedules");

    let first = game.tick().expect("tick 1");
    assert_eq!(first.scheduled_ticks_fired, 0, "due in 3, not 1");
    assert_eq!(first.scheduled_ticks_pending, 1);
    let second = game.tick().expect("tick 2");
    assert_eq!(second.scheduled_ticks_fired, 0, "due in 3, not 2");
    assert_eq!(second.scheduled_ticks_pending, 1);
    let third = game.tick().expect("tick 3");
    assert_eq!(third.scheduled_ticks_fired, 1, "due at 3");
    assert_eq!(third.scheduled_ticks_pending, 0);
    let fourth = game.tick().expect("tick 4");
    assert_eq!(
        fourth.scheduled_ticks_fired, 0,
        "a due tick must not fire twice"
    );
}

#[test]
fn the_per_tick_budget_defers_a_burst_rather_than_dropping_it() {
    let (mut game, _storage, _dir) = game("p13-budget");
    for x in 0..300 {
        game.schedule_block_tick(x, 64, 0, 0).expect("schedules");
    }
    let report = game.tick().expect("tick");
    assert_eq!(
        report.scheduled_ticks_fired,
        mc_redstone::UpdateBudget::DEFAULT_SCHEDULED_TICKS_PER_TICK,
        "the nominal budget bounds one tick"
    );
    assert_eq!(
        report.scheduled_ticks_fired + report.scheduled_ticks_pending,
        300,
        "whatever did not fire must still be queued, not lost"
    );
}
