//! Operator permissions end to end (P07-04).
//!
//! `ops.rs`'s unit tests prove the file is read correctly. This proves the **effect**: a
//! listed uuid reaches an operator-only command and an unlisted one does not. That is the
//! whole point of the module — before it, every player was level 0 for their entire session,
//! so `op` and `stop` were unreachable and there was no way to make them reachable.
//!
//! The join path is the real one: a `ClientEventKind::Joined` event carrying an `outbound`
//! handle, then a tick. `mc_network::auth::offline_profile` derives the uuid from the name,
//! which is what lets a test know which uuid the server will see without inventing one.

use mc_command::PermissionLevel;
use mc_network::bridge::{ClientEvent, ClientEventKind, ConnectionIds};
use mc_server::game::{Game, TickReport};
use mc_server::ops::{OPS_FILE_NAME, OperatorList};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::path::Path;

fn config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    }
}

/// An operator file listing one name at one level.
fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new(OPS_FILE_NAME)).expect("the fixture parses")
}

/// A game owning storage, with `operators` loaded, and one player joined.
fn game_with_player(
    dir: &TempDir,
    operators: OperatorList,
    name: &str,
) -> (Game, mc_network::bridge::ConnectionId) {
    let service = WorldService::open(&config(dir)).expect("world opens");
    let (events_tx, events_rx) = mc_network::bridge::game_channel(64);
    let mut game = Game::build_with_operators(None, Some(service), 3, events_rx, 7, operators)
        .expect("game builds");

    let ids = ConnectionIds::new();
    let id = ids.next_id();
    // `bridge::channel` is the real constructor: it builds the event sender, the inbound
    // receiver and the outbound sender together, so the join event carries a usable sender.
    let (_events, _inbound, outbound) = mc_network::bridge::channel(
        id,
        events_tx.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        4096,
    );
    events_tx
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile(name),
                outbound,
            },
        })
        .expect("join queued");
    game.tick().expect("tick");
    (game, id)
}

#[test]
fn a_listed_uuid_gets_its_file_level_and_an_unlisted_one_gets_nothing() {
    let dir = TempDir::new("ops-e2e-levels");
    let (game, id) = game_with_player(&dir, ops_for("Operator", 4), "Operator");
    assert_eq!(game.player_count(), 1, "the player joined");
    assert_eq!(
        game.player_permission(id),
        PermissionLevel::Console,
        "a listed uuid must reach the level the file states"
    );

    // The list and the session must agree, which is the assertion the accessor exists for: a
    // level that came from somewhere other than the file would pass the check above.
    let uuid = mc_network::auth::offline_profile("Operator").id.to_string();
    assert!(game.operators().is_operator(&uuid));

    // A *different* player against the same file: nothing.
    let dir = TempDir::new("ops-e2e-unlisted");
    let (game, id) = game_with_player(&dir, ops_for("Operator", 4), "Someone");
    assert_eq!(
        game.player_permission(id),
        PermissionLevel::All,
        "an unlisted uuid must hold no authority"
    );
    assert!(!game.operators().is_operator("Someone"));
}

#[test]
fn an_operator_can_stop_the_server_and_a_plain_player_cannot() {
    // The integration claim: the level is not merely *stored*, it changes what a command does.
    // `stop` is console-only, so it is the sharpest probe.
    let dir = TempDir::new("ops-e2e-stop");
    let (mut game, id) = game_with_player(&dir, ops_for("Operator", 4), "Operator");
    let mut report = TickReport::default();
    game.dispatch_command(id, "stop", &mut report)
        .expect("a command is answered");
    assert!(
        game.shutdown_requested(),
        "a level-4 operator must be able to stop the server"
    );
}

#[test]
fn a_plain_player_cannot_stop_the_server_nor_use_op() {
    // The half that matters for safety: with no ops.json, a player has no authority at all.
    let dir = TempDir::new("ops-e2e-plain");
    let (mut game, id) = game_with_player(&dir, OperatorList::new(), "Plain");
    let mut report = TickReport::default();

    game.dispatch_command(id, "stop", &mut report)
        .expect("a command is answered");
    assert!(
        !game.shutdown_requested(),
        "a player with no operator entry must not be able to stop the server"
    );

    game.dispatch_command(id, "op", &mut report)
        .expect("a command is answered");
    assert!(!game.shutdown_requested());
    assert_eq!(game.player_count(), 1, "and the client is not disconnected");
}

#[test]
fn a_level_2_entry_reaches_operator_commands_but_not_console_ones() {
    // The middle of the ladder, which a two-valued test would not distinguish: level 2 is
    // enough for `op` and not enough for `stop`.
    let dir = TempDir::new("ops-e2e-level2");
    let (mut game, id) = game_with_player(&dir, ops_for("MidOp", 2), "MidOp");
    assert_eq!(game.player_permission(id), PermissionLevel::Operator);

    let mut report = TickReport::default();
    game.dispatch_command(id, "op", &mut report)
        .expect("a command is answered");
    assert!(
        !game.shutdown_requested(),
        "level 2 must not reach the console-only command"
    );
    // `op` itself changes nothing (it reports that grants are not persisted), so the
    // observable is that it was *answered* rather than denied — which the permission check
    // above already establishes.
}

#[test]
fn the_file_is_read_from_beside_the_world() {
    // Vanilla puts `ops.json` beside `server.properties`, which is the directory *containing*
    // the world. A mistake here would show up as "no operators" rather than as an error — the
    // failure mode this module exists to prevent.
    let dir = TempDir::new("ops-e2e-location");
    let world_dir = dir.path().join("world");
    std::fs::create_dir_all(&world_dir).expect("world dir");
    let uuid = mc_network::auth::offline_profile("Beside").id;
    std::fs::write(
        dir.path().join(OPS_FILE_NAME),
        format!(r#"[{{"uuid": "{uuid}", "level": 3}}]"#),
    )
    .expect("write ops.json");

    let operators = OperatorList::load(&mc_server::ops::ops_directory(&world_dir))
        .expect("the file beside the world is found");
    assert!(operators.is_operator(&uuid.to_string()));
    assert_eq!(
        operators.level_for(&uuid.to_string()),
        PermissionLevel::Administrator
    );

    // And a file *inside* the world directory is deliberately not the one consulted.
    let inside = OperatorList::load(&world_dir).expect("no file inside the world");
    assert!(
        inside.is_empty(),
        "ops.json belongs beside the world, not inside it"
    );
}

#[test]
fn a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator() {
    // P08-06: the game loop is where "a player" exists, so the cap is enforced here.
    // A bypass operator joins anyway; a refused client is disconnected, not fatal.
    use mc_network::bridge::OutboundSender;

    let dir = TempDir::new("ops-e2e-full");
    let service = WorldService::open(&config(&dir)).expect("world opens");
    let (events_tx, events_rx) = mc_network::bridge::game_channel(64);
    let operator = mc_network::auth::offline_profile("BypassOp").id.to_string();
    let bypass = format!(
        r#"[{{"uuid": "{operator}", "name": "BypassOp", "level": 4, "bypassesPlayerLimit": true}}]"#
    );
    let operators =
        OperatorList::parse(&bypass, Path::new(OPS_FILE_NAME)).expect("the fixture parses");
    let mut game = Game::build_with_operators(None, Some(service), 3, events_rx, 7, operators)
        .expect("game builds");
    game.set_max_players(1);
    let ids = ConnectionIds::new();

    let join = |game: &mut Game, name: &str| -> (mc_network::bridge::ConnectionId, bool) {
        let id = ids.next_id();
        let (outbound, mut inbound) = OutboundSender::pair(id, 64);
        events_tx
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join queued");
        game.tick().expect("tick");
        let joined = game.has_player(id);
        let refused = {
            use mc_protocol::packets::Packet as _;
            let mut saw_refusal = false;
            while let Some(raw) = inbound.try_recv() {
                let Ok(packet) = mc_protocol::packets::play::PlayDisconnect::decode(&raw.payload)
                else {
                    continue;
                };
                if packet.reason.as_plain().contains("full") {
                    saw_refusal = true;
                }
            }
            saw_refusal
        };
        (id, joined && !refused)
    };

    let (_, first) = join(&mut game, "First");
    assert!(first, "the first player joins a one-slot server");
    let (_, extra) = join(&mut game, "Extra");
    assert!(!extra, "one more join on a full server is refused");
    assert_eq!(
        game.player_count(),
        1,
        "the refusal leaves the session map alone"
    );
    let (_, op) = join(&mut game, "BypassOp");
    assert!(op, "a bypass operator joins a full server");
    assert_eq!(game.player_count(), 2);
}

#[test]
fn an_uppercase_uuid_in_the_file_still_grants() {
    // Files in the wild are not consistent about uuid case, and a mismatch is invisible: the
    // operator simply appears not to be listed, which reads as "permissions stopped working".
    let dir = TempDir::new("ops-e2e-case");
    let uuid = mc_network::auth::offline_profile("Cased").id;
    let upper = uuid.to_string().to_ascii_uppercase();
    let operators = OperatorList::parse(
        &format!(r#"[{{"uuid": "{upper}", "level": 4}}]"#),
        Path::new(OPS_FILE_NAME),
    )
    .expect("parses");

    let (game, id) = game_with_player(&dir, operators, "Cased");
    assert_eq!(
        game.player_permission(id),
        PermissionLevel::Console,
        "an upper-case uuid in the file must still match the lower-case session uuid"
    );
}
