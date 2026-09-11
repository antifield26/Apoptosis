//! Packet-id conformance against the official 26.1.2 jar (P02-04, P04-01).
//!
//! `docs/protocol/packet-ids-775.tsv` is machine-extracted from the vanilla
//! server jar: vanilla registers packets with `ProtocolInfoBuilder.addPacket`
//! from a static initialiser, and each call takes the next index, so the order of
//! the `getstatic PacketTypes.<NAME>` instructions in that initialiser **is** the
//! id table. (Extraction method and jar hash: `docs/research/provenance.md`.)
//!
//! This test is what caught a real defect: the Phase 02 table had
//! `serverbound::play::CHAT_COMMAND = 8`, but the jar says **7** — a real
//! 26.1.2 client would have had `/commands` decoded as a chat message. That
//! constant came from a reference snapshot whose table was wrong for protocol 775.
//!
//! Every constant this crate actually sends or decodes is asserted here, so a
//! typo can no longer reach the wire unnoticed.

// The assertion list is deliberately flat and exhaustive: one line per constant is
// easier to audit against the TSV than a generated loop, so the length is the point.
#![allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use mc_protocol::ids::{clientbound, serverbound};

/// Parse `docs/protocol/packet-ids-775.tsv` into `(state, direction, id, name)`.
fn authoritative_table() -> Vec<(String, String, i32, String)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/protocol/packet-ids-775.tsv");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split(' ');
        let (Some(state), Some(direction), Some(id), Some(name)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            panic!("malformed row in {}: {line:?}", path.display());
        };
        rows.push((
            state.to_owned(),
            direction.to_owned(),
            id.parse().expect("id is an integer"),
            name.to_owned(),
        ));
    }
    assert!(
        rows.len() > 200,
        "table looks truncated: {} rows",
        rows.len()
    );
    rows
}

/// Look up an id by state, direction and logical name.
fn id_of(rows: &[(String, String, i32, String)], state: &str, direction: &str, name: &str) -> i32 {
    rows.iter()
        .find(|(s, d, _, n)| s == state && d == direction && n == name)
        .unwrap_or_else(|| panic!("{state}/{direction}/{name} missing from the table"))
        .2
}

#[test]
fn the_table_covers_every_state_and_direction() {
    let rows = authoritative_table();
    for (state, direction, expected) in [
        ("handshake", "serverbound", 1),
        ("status", "serverbound", 2),
        ("status", "clientbound", 2),
        ("login", "serverbound", 5),
        ("login", "clientbound", 6),
        ("configuration", "serverbound", 10),
        ("configuration", "clientbound", 20),
        ("game", "serverbound", 69),
        ("game", "clientbound", 141),
    ] {
        let found = rows
            .iter()
            .filter(|(s, d, _, _)| s == state && d == direction)
            .count();
        assert_eq!(found, expected, "{state}/{direction} packet count");
    }
}

#[test]
fn ids_are_contiguous_from_zero() {
    // Registration order is the id, so every direction must be 0..n with no gap.
    let rows = authoritative_table();
    let mut keys: Vec<(String, String)> = rows
        .iter()
        .map(|(s, d, _, _)| (s.clone(), d.clone()))
        .collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let mut ids: Vec<i32> = rows
            .iter()
            .filter(|(s, d, _, _)| *s == key.0 && *d == key.1)
            .map(|(_, _, id, _)| *id)
            .collect();
        ids.sort_unstable();
        let expected: Vec<i32> = (0..ids.len() as i32).collect();
        assert_eq!(ids, expected, "{}/{} ids must be contiguous", key.0, key.1);
    }
}

#[test]
fn every_constant_we_use_matches_the_vanilla_jar() {
    let rows = authoritative_table();
    let check = |state: &str, direction: &str, name: &str, ours: i32| {
        let expected = id_of(&rows, state, direction, name);
        assert_eq!(
            ours, expected,
            "{state}/{direction}/{name}: ids.rs says {ours}, the 26.1.2 jar says {expected}"
        );
    };

    check(
        "handshake",
        "serverbound",
        "intention",
        serverbound::handshake::INTENTION,
    );

    check(
        "status",
        "serverbound",
        "status_request",
        serverbound::status::STATUS_REQUEST,
    );
    check(
        "status",
        "serverbound",
        "ping_request",
        serverbound::status::PING_REQUEST,
    );
    check(
        "status",
        "clientbound",
        "status_response",
        clientbound::status::STATUS_RESPONSE,
    );
    check(
        "status",
        "clientbound",
        "pong_response",
        clientbound::status::PONG_RESPONSE,
    );

    check("login", "serverbound", "hello", serverbound::login::HELLO);
    check("login", "serverbound", "key", serverbound::login::KEY);
    check(
        "login",
        "serverbound",
        "custom_query_answer",
        serverbound::login::CUSTOM_QUERY_ANSWER,
    );
    check(
        "login",
        "serverbound",
        "login_acknowledged",
        serverbound::login::LOGIN_ACKNOWLEDGED,
    );
    check(
        "login",
        "serverbound",
        "cookie_response",
        serverbound::login::COOKIE_RESPONSE,
    );
    check(
        "login",
        "clientbound",
        "login_disconnect",
        clientbound::login::LOGIN_DISCONNECT,
    );
    check("login", "clientbound", "hello", clientbound::login::HELLO);
    check(
        "login",
        "clientbound",
        "login_finished",
        clientbound::login::LOGIN_FINISHED,
    );
    check(
        "login",
        "clientbound",
        "login_compression",
        clientbound::login::LOGIN_COMPRESSION,
    );
    check(
        "login",
        "clientbound",
        "custom_query",
        clientbound::login::CUSTOM_QUERY,
    );
    check(
        "login",
        "clientbound",
        "cookie_request",
        clientbound::login::COOKIE_REQUEST,
    );

    check(
        "configuration",
        "serverbound",
        "client_information",
        serverbound::config::CLIENT_INFORMATION,
    );
    check(
        "configuration",
        "serverbound",
        "cookie_response",
        serverbound::config::COOKIE_RESPONSE,
    );
    check(
        "configuration",
        "serverbound",
        "custom_payload",
        serverbound::config::CUSTOM_PAYLOAD,
    );
    check(
        "configuration",
        "serverbound",
        "finish_configuration",
        serverbound::config::FINISH_CONFIGURATION,
    );
    check(
        "configuration",
        "serverbound",
        "keep_alive",
        serverbound::config::KEEP_ALIVE,
    );
    check(
        "configuration",
        "serverbound",
        "pong",
        serverbound::config::PONG,
    );
    check(
        "configuration",
        "serverbound",
        "resource_pack",
        serverbound::config::RESOURCE_PACK,
    );
    check(
        "configuration",
        "serverbound",
        "select_known_packs",
        serverbound::config::SELECT_KNOWN_PACKS,
    );
    check(
        "configuration",
        "serverbound",
        "custom_click_action",
        serverbound::config::CUSTOM_CLICK_ACTION,
    );
    check(
        "configuration",
        "serverbound",
        "accept_code_of_conduct",
        serverbound::config::ACCEPT_CODE_OF_CONDUCT,
    );

    check(
        "configuration",
        "clientbound",
        "cookie_request",
        clientbound::config::COOKIE_REQUEST,
    );
    check(
        "configuration",
        "clientbound",
        "custom_payload",
        clientbound::config::CUSTOM_PAYLOAD,
    );
    check(
        "configuration",
        "clientbound",
        "disconnect",
        clientbound::config::DISCONNECT,
    );
    check(
        "configuration",
        "clientbound",
        "finish_configuration",
        clientbound::config::FINISH_CONFIGURATION,
    );
    check(
        "configuration",
        "clientbound",
        "keep_alive",
        clientbound::config::KEEP_ALIVE,
    );
    check(
        "configuration",
        "clientbound",
        "ping",
        clientbound::config::PING,
    );
    check(
        "configuration",
        "clientbound",
        "reset_chat",
        clientbound::config::RESET_CHAT,
    );
    check(
        "configuration",
        "clientbound",
        "registry_data",
        clientbound::config::REGISTRY_DATA,
    );
    check(
        "configuration",
        "clientbound",
        "transfer",
        clientbound::config::TRANSFER,
    );
    check(
        "configuration",
        "clientbound",
        "update_enabled_features",
        clientbound::config::UPDATE_ENABLED_FEATURES,
    );
    check(
        "configuration",
        "clientbound",
        "update_tags",
        clientbound::config::UPDATE_TAGS,
    );
    check(
        "configuration",
        "clientbound",
        "select_known_packs",
        clientbound::config::SELECT_KNOWN_PACKS,
    );
    check(
        "configuration",
        "clientbound",
        "code_of_conduct",
        clientbound::config::CODE_OF_CONDUCT,
    );

    // Play: serverbound.
    check(
        "game",
        "serverbound",
        "accept_teleportation",
        serverbound::play::ACCEPT_TELEPORTATION,
    );
    check(
        "game",
        "serverbound",
        "chat_command",
        serverbound::play::CHAT_COMMAND,
    );
    check("game", "serverbound", "chat", serverbound::play::CHAT);
    check(
        "game",
        "serverbound",
        "client_command",
        serverbound::play::CLIENT_COMMAND,
    );
    check(
        "game",
        "serverbound",
        "client_information",
        serverbound::play::CLIENT_INFORMATION,
    );
    check(
        "game",
        "serverbound",
        "configuration_acknowledged",
        serverbound::play::CONFIGURATION_ACKNOWLEDGED,
    );
    check(
        "game",
        "serverbound",
        "interact",
        serverbound::play::INTERACT,
    );
    check(
        "game",
        "serverbound",
        "keep_alive",
        serverbound::play::KEEP_ALIVE,
    );
    check(
        "game",
        "serverbound",
        "move_player_pos",
        serverbound::play::MOVE_PLAYER_POS,
    );
    check(
        "game",
        "serverbound",
        "move_player_pos_rot",
        serverbound::play::MOVE_PLAYER_POS_ROT,
    );
    check(
        "game",
        "serverbound",
        "move_player_rot",
        serverbound::play::MOVE_PLAYER_ROT,
    );
    check(
        "game",
        "serverbound",
        "move_player_status_only",
        serverbound::play::MOVE_PLAYER_STATUS_ONLY,
    );
    check(
        "game",
        "serverbound",
        "player_action",
        serverbound::play::PLAYER_ACTION,
    );
    check(
        "game",
        "serverbound",
        "player_command",
        serverbound::play::PLAYER_COMMAND,
    );
    check(
        "game",
        "serverbound",
        "player_input",
        serverbound::play::PLAYER_INPUT,
    );
    check(
        "game",
        "serverbound",
        "player_loaded",
        serverbound::play::PLAYER_LOADED,
    );
    check(
        "game",
        "serverbound",
        "ping_request",
        serverbound::play::PING_REQUEST,
    );
    check("game", "serverbound", "pong", serverbound::play::PONG);
    // Phase 04 packats.
    check("game", "serverbound", "swing", serverbound::play::SWING);
    check(
        "game",
        "serverbound",
        "use_item_on",
        serverbound::play::USE_ITEM_ON,
    );
    check(
        "game",
        "serverbound",
        "use_item",
        serverbound::play::USE_ITEM,
    );
    check(
        "game",
        "serverbound",
        "set_carried_item",
        serverbound::play::SET_CARRIED_ITEM,
    );
    check(
        "game",
        "serverbound",
        "container_click",
        serverbound::play::CONTAINER_CLICK,
    );
    check(
        "game",
        "serverbound",
        "container_close",
        serverbound::play::CONTAINER_CLOSE,
    );
    check(
        "game",
        "serverbound",
        "set_creative_mode_slot",
        serverbound::play::SET_CREATIVE_MODE_SLOT,
    );
    check("game", "serverbound", "attack", serverbound::play::ATTACK);
    check(
        "game",
        "serverbound",
        "command_suggestion",
        serverbound::play::COMMAND_SUGGESTION,
    );

    // Play: clientbound.
    check(
        "game",
        "clientbound",
        "chunk_batch_finished",
        clientbound::play::CHUNK_BATCH_FINISHED,
    );
    check(
        "game",
        "clientbound",
        "chunk_batch_start",
        clientbound::play::CHUNK_BATCH_START,
    );
    check(
        "game",
        "clientbound",
        "disconnect",
        clientbound::play::DISCONNECT,
    );
    check(
        "game",
        "clientbound",
        "game_event",
        clientbound::play::GAME_EVENT,
    );
    check(
        "game",
        "clientbound",
        "keep_alive",
        clientbound::play::KEEP_ALIVE,
    );
    check(
        "game",
        "clientbound",
        "level_chunk_with_light",
        clientbound::play::LEVEL_CHUNK_WITH_LIGHT,
    );
    check("game", "clientbound", "login", clientbound::play::LOGIN);
    check("game", "clientbound", "ping", clientbound::play::PING);
    check(
        "game",
        "clientbound",
        "pong_response",
        clientbound::play::PONG_RESPONSE,
    );
    check(
        "game",
        "clientbound",
        "player_position",
        clientbound::play::PLAYER_POSITION,
    );
    check(
        "game",
        "clientbound",
        "set_chunk_cache_center",
        clientbound::play::SET_CHUNK_CACHE_CENTER,
    );
    check(
        "game",
        "clientbound",
        "set_chunk_cache_radius",
        clientbound::play::SET_CHUNK_CACHE_RADIUS,
    );
    check(
        "game",
        "clientbound",
        "start_configuration",
        clientbound::play::START_CONFIGURATION,
    );
    // Phase 04 packets.
    check(
        "game",
        "clientbound",
        "block_update",
        clientbound::play::BLOCK_UPDATE,
    );
    check(
        "game",
        "clientbound",
        "section_blocks_update",
        clientbound::play::SECTION_BLOCKS_UPDATE,
    );
    check(
        "game",
        "clientbound",
        "set_health",
        clientbound::play::SET_HEALTH,
    );
    check(
        "game",
        "clientbound",
        "set_experience",
        clientbound::play::SET_EXPERIENCE,
    );
    check("game", "clientbound", "respawn", clientbound::play::RESPAWN);
    check(
        "game",
        "clientbound",
        "set_default_spawn_position",
        clientbound::play::SET_DEFAULT_SPAWN_POSITION,
    );
    check(
        "game",
        "clientbound",
        "set_time",
        clientbound::play::SET_TIME,
    );
    check(
        "game",
        "clientbound",
        "set_entity_data",
        clientbound::play::SET_ENTITY_DATA,
    );
    check(
        "game",
        "clientbound",
        "system_chat",
        clientbound::play::SYSTEM_CHAT,
    );
    check(
        "game",
        "clientbound",
        "set_held_slot",
        clientbound::play::SET_HELD_SLOT,
    );
    check(
        "game",
        "clientbound",
        "container_set_content",
        clientbound::play::CONTAINER_SET_CONTENT,
    );
    check(
        "game",
        "clientbound",
        "container_set_slot",
        clientbound::play::CONTAINER_SET_SLOT,
    );
    check(
        "game",
        "clientbound",
        "set_player_inventory",
        clientbound::play::SET_PLAYER_INVENTORY,
    );
}

#[test]
fn the_regression_that_motivated_this_test_stays_fixed() {
    // Phase 02 shipped CHAT_COMMAND = 8; the 26.1.2 jar registers it at 7.
    // Documented in PHASE-03/AUDIT notes; asserted here so it cannot regress.
    let rows = authoritative_table();
    assert_eq!(id_of(&rows, "game", "serverbound", "chat_command"), 7);
    assert_eq!(serverbound::play::CHAT_COMMAND, 7);
    assert_eq!(id_of(&rows, "game", "serverbound", "chat"), 9);
    assert_eq!(serverbound::play::CHAT, 9);
    // `chat_command_signed` sits between them and is deliberately unmodelled.
    assert_eq!(
        id_of(&rows, "game", "serverbound", "chat_command_signed"),
        8
    );
}
