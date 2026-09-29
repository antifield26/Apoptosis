//! Console dispatch and save controls end to end (P19-03).
//!
//! `dispatch_console` runs strings at console level without a connection;
//! `save-off` holds the autosave writes the lifecycle would otherwise do
//! every few ticks, while an explicit `save-all` still writes through.
//!
//! Falsification shape: drop the hold check in the autosave block and the
//! held test finds a region file; route console through a level-0 source
//! and `stop`/`save-all` come back permission-denied; make `save_all_owned`
//! respect the hold and the bypass test finds no file.

use mc_network::bridge::{ClientEvent, ClientEventKind, ConnectionIds, OutboundSender};
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

fn game_with_storage(tag: &str, autosave_ticks: u64) -> (Game, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = mc_network::bridge::game_channel(64);
    let game = Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
        .expect("game builds");
    (game, dir)
}

/// Region files under the world dir, if any.
fn region_files(dir: &TempDir) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.path().join("world")];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "mca") {
                out.push(path);
            }
        }
    }
    out
}

/// Total region payload bytes on disk.
fn region_bytes(dir: &TempDir) -> u64 {
    region_files(dir)
        .iter()
        .map(|path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        .sum()
}

fn console(game: &mut Game, text: &str) -> Vec<String> {
    let mut report = TickReport::default();
    game.dispatch_console(text, &mut report)
        .expect("console input is answered")
}

#[test]
fn console_runs_at_console_level() {
    let (mut game, _dir) = game_with_storage("console-level", 0);
    // `stop` is console-only: reaching it proves the source level.
    let lines = console(&mut game, "stop");
    assert!(
        lines.iter().any(|line| line.contains("Stopping")),
        "console must run console-only commands: {lines:?}"
    );
    assert!(
        game.shutdown_requested(),
        "console `stop` requests the shutdown"
    );
}

#[test]
fn console_reports_unknown_commands_as_lines() {
    let (mut game, _dir) = game_with_storage("console-unknown", 0);
    let lines = console(&mut game, "frobnicate");
    assert!(
        lines.iter().any(|line| line.contains("Unknown command")),
        "refusals come back as lines, not silence: {lines:?}"
    );
    assert!(
        !game.shutdown_requested(),
        "a refusal must not disturb the server"
    );
}

#[test]
fn save_off_holds_autosave_writes_and_save_all_writes_through() {
    let (mut game, dir) = game_with_storage("save-hold", 5);
    // Dirty a generated chunk through the block path (marks dirty).
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    game.load_chunk(mc_world::ChunkPos::new(0, 0));
    game.world_mut()
        .set_block(1, 70, 1, stone)
        .expect("block set");
    // Loading touches the region file (the handle is created to even check
    // for a stored chunk), so the observable is payload bytes, not file
    // presence.
    let before = region_bytes(&dir);
    // Hold automatic writes, then run past several autosave deadlines the
    // way the lifecycle's autosave block does.
    let lines = console(&mut game, "save-off");
    assert!(
        lines.iter().any(|line| line.contains("held")),
        "save-off confirms: {lines:?}"
    );
    assert!(!game.saving_enabled());
    for _ in 0..30 {
        game.tick().expect("tick");
        // The lifecycle autosave block, through the same `autosave_due`
        // helper (P19-03 owns both halves).
        if game.autosave_due(game.tick_count()) {
            game.save_all_owned().expect("saves");
        }
    }
    assert_eq!(
        region_bytes(&dir),
        before,
        "held automatic writes must not grow the region payload"
    );
    // An explicit save-all writes through the hold...
    let lines = console(&mut game, "save-all flush");
    assert!(
        lines.iter().any(|line| line.contains("Saved")),
        "save-all confirms: {lines:?}"
    );
    assert!(
        region_bytes(&dir) > before,
        "an explicit save-all writes despite the hold"
    );
    // ...and save-on resumes the timer.
    let lines = console(&mut game, "save-on");
    assert!(
        lines.iter().any(|line| line.contains("resumed")),
        "save-on confirms: {lines:?}"
    );
    assert!(game.saving_enabled());
}

#[test]
fn save_all_refuses_modes_other_than_flush() {
    let (mut game, _dir) = game_with_storage("save-modes", 0);
    let lines = console(&mut game, "save-all everything");
    assert!(
        lines.iter().any(|line| line.contains("flush")),
        "only `flush` is a mode: {lines:?}"
    );
}

#[test]
fn plain_player_cannot_hold_saves() {
    let dir = TempDir::new("save-denied");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let service = WorldService::open(&config).expect("world opens");
    let (events_tx, events_rx) = mc_network::bridge::game_channel(64);
    let mut game =
        Game::build_with_operators(None, Some(service), 3, events_rx, 7, OperatorList::new())
            .expect("game builds");
    let ids = ConnectionIds::new();
    let id = ids.next_id();
    let (outbound, _) = OutboundSender::pair(id, 64);
    events_tx
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Plain"),
                outbound,
            },
        })
        .expect("join queued");
    game.tick().expect("tick");
    let mut report = TickReport::default();
    game.dispatch_command(id, "save-off", &mut report)
        .expect("a command is answered");
    assert!(
        game.saving_enabled(),
        "a level-0 `save-off` must not hold anything"
    );
}
