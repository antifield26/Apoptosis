//! Command E2E tests over a real socket (P07-05, P07-18).
//!
//! The dispatcher's unit tests prove the grammar. This file proves the *integration*: a
//! `chat_command` sent by a client is decoded by `mc-protocol`, routed through the game
//! loop, parsed against the tree and answered with a `disguised_chat` the client receives.
//! That path is what makes commands reachable, and nothing else tests it.

use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::ClientInformation;
use mc_protocol::packets::play::SetChunkCacheRadius;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::time::Duration;

/// A server with a real socket and a logged-in client, plus the game loop.
struct Harness {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    _storage: WorldService,
    _dir: TempDir,
}

impl Harness {
    async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (event_tx, event_rx) = game_channel(256);
        let mut game = Game::new(&storage, 3, event_rx).expect("game builds");

        let settings = mc_network::NetworkSettings {
            bind: "127.0.0.1:0".parse().expect("addr"),
            view_distance: 3,
            ..mc_network::NetworkSettings::default()
        };
        let link = mc_network::listener::GameLink::new(event_tx, 4096);
        let service = mc_network::NetworkService::start_with_game(settings, None, Some(link))
            .await
            .expect("listener starts");
        let addr = service.local_addr();
        let (client, _join) = TestClient::login_join(addr, "Commander")
            .await
            .expect("login completes");

        for _ in 0..40 {
            game.tick().expect("tick");
            if game.player_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Self {
            client,
            game,
            service,
            _storage: storage,
            _dir: dir,
        }
    }

    /// Send a `chat_command` exactly as a client would.
    async fn command(&mut self, text: &str) {
        self.client
            .send_raw_packet(&RawPacket::new(
                serverbound::play::CHAT_COMMAND,
                string_field(text),
            ))
            .await
            .expect("command sent");
        self.settle(3).await;
    }

    /// Acknowledge the next teleport the server sends, as a real client does.
    ///
    /// The server keeps a teleport **pending** until this arrives, so commands that resolve `~` against the
    /// player's position see the old one until then. Reading the packet and replying with its own teleport id
    /// is exactly the client's side of the handshake.
    async fn acknowledge_teleport(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(60), self.client.recv()).await {
                Ok(Ok(raw)) if raw.id == clientbound::play::PLAYER_POSITION => {
                    // `player_position` leads with a `VarInt` teleport id.
                    let payload = &raw.payload;
                    let id = payload.first().copied().unwrap_or(0);
                    self.client
                        .send_raw_packet(&RawPacket::new(
                            serverbound::play::ACCEPT_TELEPORTATION,
                            vec![id],
                        ))
                        .await
                        .expect("teleport acknowledged");
                    self.settle(2).await;
                    return;
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    self.game.tick().expect("tick");
                }
            }
        }
        panic!("the server sent no teleport to acknowledge");
    }

    async fn settle(&mut self, ticks: usize) {
        for _ in 0..ticks {
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.game.tick().expect("tick");
        }
    }

    /// Read every packet that arrives within the window, returning their ids.
    async fn drain_ids(&mut self, millis: u64) -> Vec<i32> {
        let mut ids = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(millis);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(60), self.client.recv()).await {
                Ok(Ok(raw)) => ids.push(raw.id),
                Ok(Err(_)) => break,
                // Nothing arrived: give the game a tick and keep waiting, so a slow
                // reply is not mistaken for no reply.
                Err(_) => {
                    self.game.tick().expect("tick");
                }
            }
        }
        ids
    }

    fn id(&self) -> mc_network::bridge::ConnectionId {
        self.game.players().next().expect("a player")
    }
}

/// Encode a string the way a client writes it: a `VarInt` length then UTF-8 bytes.
///
/// One helper rather than three call sites, so the narrowing the *format* requires is
/// visible in one place.
fn string_field(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    // A `VarInt` length is a 32-bit field, and no test string approaches that.
    let length = i32::try_from(bytes.len()).expect("a test string fits a VarInt length");
    let mut payload = varint(length);
    payload.extend_from_slice(bytes);
    payload
}

/// Minimal `VarInt` encoder for the test's own packets.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn varint(value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut value = value as u32;
    loop {
        if value & !0x7F == 0 {
            out.push(value as u8);
            return out;
        }
        out.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
}

#[tokio::test]
async fn a_help_command_is_answered_over_the_socket() {
    let mut harness = Harness::start("p07-help").await;
    harness.command("help").await;
    let ids = harness.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::DISGUISED_CHAT),
        "a command must be answered with system_chat, saw {ids:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn every_declared_command_is_reachable_from_a_client() {
    // The point is not what each returns but that none of them panics, disconnects the
    // client, or is unreachable — which is the integration claim.
    let mut harness = Harness::start("p07-reach").await;
    for command in [
        "help",
        "list",
        "say hello from a test",
        "time",
        "time 1000",
        "tp Commander 10 70 -5",
        "op",
        "gamemode creative",
        "give Commander stone 5",
        "kill",
        "seed",
        "difficulty",
        "difficulty peaceful",
        "deop Commander",
        // A player is level 0, so the operator commands below are denied — but
        // each denial must be a *message*, not a disconnect, and must not
        // reveal the command's grammar.
        "stop",
    ] {
        harness.command(command).await;
        assert_eq!(
            harness.game.player_count(),
            1,
            "{command:?} must not disconnect the client"
        );
    }
    // The `stop` command is console-only, so it must not have stopped the server.
    assert!(
        !harness.game.shutdown_requested(),
        "a player must not be able to stop the server"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_unknown_command_is_reported_rather_than_ignored() {
    let mut harness = Harness::start("p07-unknown").await;
    harness.command("definitely_not_a_command").await;
    let ids = harness.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::DISGUISED_CHAT),
        "an unknown command must be reported, saw {ids:?}"
    );
    assert_eq!(harness.game.player_count(), 1, "and must not disconnect");
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_malformed_command_is_refused_without_disconnecting() {
    let mut harness = Harness::start("p07-malformed").await;
    for command in [
        "",
        " ",
        "/",
        "\"",
        "say \"",
        "time 99999999999999999999",
        "time -1",
        "time 1.5",
        "tp Commander 1 2",
        "\u{0}",
        "help extra arguments here",
    ] {
        harness.command(command).await;
        assert_eq!(
            harness.game.player_count(),
            1,
            "{command:?} must not disconnect the client"
        );
    }
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_over_long_command_ends_only_that_connection() {
    // A command past `COMMAND_MAX_CHARS` (32 500, Vanilla's own cap) is a decode failure,
    // and a decode failure closes the connection — the same deliberate behaviour as a
    // truncated `container_click`. Note what is *not* asserted: that the offending client
    // survives. A client sending an over-long field is misbehaving.
    //
    // The property that matters is that the **server** survives and keeps serving, which
    // is what the fresh login below demonstrates.
    let mut harness = Harness::start("p07-long").await;
    let long = format!("say {}", "x".repeat(40_000));
    // The send may itself fail once the server closes the connection, which is an
    // acceptable outcome rather than a test failure.
    let _ = harness
        .client
        .send_raw_packet(&RawPacket::new(
            serverbound::play::CHAT_COMMAND,
            string_field(&long),
        ))
        .await;
    for _ in 0..20 {
        harness.game.tick().expect("tick");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The listener is still accepting: a fresh client logs in and gets an answer.
    let addr = harness.service.local_addr();
    let (mut fresh, _join) = TestClient::login_join(addr, "Survivor")
        .await
        .expect("the server must still accept logins after an over-long command");
    fresh
        .send_raw_packet(&RawPacket::new(
            serverbound::play::CHAT_COMMAND,
            string_field("help"),
        ))
        .await
        .expect("a fresh client can still send a command");

    // Tick until the fresh client's reply arrives, or give up.
    //
    // The deadline is generous because this client's login makes the server send it a whole view, and since
    // P10-05 every one of those chunk packets is computed with a light engine — 205 chunks, each three passes
    // over 124 320 cells. In a debug build on a slower machine that legitimately takes tens of seconds, and a
    // five-second deadline began failing in CI while passing locally.
    //
    // The deadline's job is to fail when the server is **wedged**, not to measure throughput, so it is set
    // well above the honest cost rather than tuned to it. The cost itself is a recorded limitation with a
    // named fix: light is recomputed on every chunk send with no cache, and caching it per chunk — invalidated
    // on block change — is the same work as the incremental relighting P10-04 still owes.
    let mut saw_reply = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        harness.game.tick().expect("tick");
        match tokio::time::timeout(Duration::from_millis(100), fresh.recv()).await {
            Ok(Ok(raw)) if raw.id == clientbound::play::DISGUISED_CHAT => {
                saw_reply = true;
                break;
            }
            // Another packet, or nothing yet: keep ticking until the deadline.
            Ok(Ok(_)) | Err(_) => {}
            // The connection ended without a reply.
            Ok(Err(_)) => break,
        }
    }
    assert!(
        saw_reply,
        "the server must still answer commands after refusing an over-long one"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_command_flood_from_one_client_does_not_starve_the_tick() {
    // P08-08: one client sending 300 commands in one tick must not own the
    // tick. The Network phase drains at most PENDING_INTENT_BUDGET events per
    // tick; the excess stays queued for the next tick, in order. The evidence
    // is the drain bound, not the absence of a crash: every command is still
    // answered, and the tick count advances normally.
    let mut harness = Harness::start("p08-flood").await;
    for _ in 0..300 {
        harness
            .client
            .send_raw_packet(&RawPacket::new(
                serverbound::play::CHAT_COMMAND,
                string_field("help"),
            ))
            .await
            .expect("command sent");
    }
    // One tick drains at most the per-tick budget (256); the rest stays queued
    // for the next ticks rather than owning this one. The player survives.
    harness.game.tick().expect("tick");
    assert_eq!(
        harness.game.player_count(),
        1,
        "a flood must not disconnect the sender"
    );
    // Drain the answers over the next ticks; every command gets one.
    let mut chats = 0usize;
    for _ in 0..12 {
        harness.game.tick().expect("tick");
        for id in harness.drain_ids(200).await {
            if id == clientbound::play::DISGUISED_CHAT {
                chats += 1;
            }
        }
    }
    assert!(
        chats >= 300,
        "every flooded command is answered over the next ticks, saw {chats}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_client_information_update_resets_the_streaming_radius() {
    // The P14-04 forwarding path over a real socket: the config-phase
    // settings arrive before play (and stay silent here, 8 clamped to the
    // server's 3 changes nothing observable), then a play-phase update to 2
    // must come back as a radius-2 confirm.
    let mut harness = Harness::start("p14-client-info").await;
    let info = ClientInformation {
        locale: "en_us".to_owned(),
        view_distance: 2,
        chat_mode: 0,
        chat_colors: true,
        skin_parts: 0x7f,
        main_hand: 1,
        text_filtering: false,
        server_listing: true,
    };
    harness
        .client
        .send_raw_packet(&RawPacket::new(
            serverbound::play::CLIENT_INFORMATION,
            info.encode_body().expect("encodes"),
        ))
        .await
        .expect("settings sent");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut radii = Vec::new();
    while tokio::time::Instant::now() < deadline {
        harness.game.tick().expect("tick");
        match tokio::time::timeout(Duration::from_millis(100), harness.client.recv()).await {
            Ok(Ok(raw)) if raw.id == clientbound::play::SET_CHUNK_CACHE_RADIUS => {
                radii.push(
                    SetChunkCacheRadius::decode(&raw.payload)
                        .expect("a radius decodes")
                        .radius,
                );
            }
            Ok(Ok(_)) | Err(_) => {}
            Ok(Err(_)) => break,
        }
        if radii.contains(&2) {
            break;
        }
    }
    // enter_play announces the server radius first; the update confirms 2
    // after it, exactly once.
    assert_eq!(
        radii.last(),
        Some(&2),
        "the update must confirm radius 2 last; saw {radii:?}"
    );
    assert_eq!(
        radii.iter().filter(|radius| **radius == 2).count(),
        1,
        "exactly one radius-2 confirm; saw {radii:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_time_command_changes_the_broadcast_time() {
    // The property that makes `/time` stick: the per-second `SetTime` broadcast computes
    // the time from the tick counter, so a command must record an *offset* or the next
    // broadcast would immediately overwrite it.
    let mut harness = Harness::start("p07-time").await;
    assert_eq!(harness.game.time_offset(), 0, "no offset initially");

    harness.command("time 6000").await;
    assert_ne!(
        harness.game.time_offset(),
        0,
        "setting the time must record an offset"
    );

    // The next broadcasts must still carry the offset, not revert to the raw tick.
    harness.settle(30).await;
    assert_ne!(
        harness.game.time_offset(),
        0,
        "the offset must survive later ticks"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_time_query_does_not_change_the_offset() {
    let mut harness = Harness::start("p07-time-query").await;
    harness.command("time").await;
    assert_eq!(
        harness.game.time_offset(),
        0,
        "a query must not set anything"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn time_set_and_presets_reach_the_client_clock() {
    // P14-09 walk: `/time set` was rejected by the grammar and the old packet
    // shape decoded as an empty clock map, so the server's mobs moved to
    // night while the client's sky never did.
    let mut harness = Harness::start("p07-time-set").await;
    // Effective time reads two counters, and a tick may pass between the
    // command and the read — so assert the value the command aimed at, plus
    // only forward drift from later ticks.
    harness.command("time set night").await;
    let drift = (harness.game.tick_count().cast_signed() + harness.game.time_offset() - 13_000)
        .rem_euclid(24_000);
    assert!(drift < 5, "night must land on 13000, drifted {drift}");
    harness.command("time set 20000").await;
    let drift = (harness.game.tick_count().cast_signed() + harness.game.time_offset() - 20_000)
        .rem_euclid(24_000);
    assert!(
        drift < 5,
        "an integer set must land exactly, drifted {drift}"
    );
    let ids = harness.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::SET_TIME),
        "a set must broadcast the clock immediately, saw {ids:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn teleport_moves_the_invoking_player() {
    let mut harness = Harness::start("p07-tp").await;
    let id = harness.id();
    let before = harness.game.player(id).expect("player").position;

    harness.command("tp Commander 30 80 -20").await;
    let after = harness.game.player(id).expect("player").position;
    assert!(
        (after.x - before.x).abs() > 0.5 || (after.z - before.z).abs() > 0.5,
        "the teleport must move the player ({before:?} -> {after:?})"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn teleporting_someone_else_is_refused_with_a_reason() {
    // The honest limitation: there is no cross-player teleport authority model, so the
    // target must be the source. It must say so rather than silently do nothing.
    let mut harness = Harness::start("p07-tp-other").await;
    let id = harness.id();
    let before = harness.game.player(id).expect("player").position;

    harness.command("tp SomeoneElse 30 80 -20").await;
    let after = harness.game.player(id).expect("player").position;
    assert!(
        (after.x - before.x).abs() < 0.001 && (after.z - before.z).abs() < 0.001,
        "an unknown target must not move the source"
    );
    let ids = harness.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::DISGUISED_CHAT),
        "the refusal must be explained"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_relative_teleport_resolves_against_the_player() {
    let mut harness = Harness::start("p07-tp-relative").await;
    let id = harness.id();
    let home = harness.game.spawn();

    // Move to a known absolute position, then step one block with `~`.
    harness
        .command(&format!("tp Commander {} {} {}", home.0, home.1, home.2))
        .await;
    // **The teleport has to be acknowledged before it counts.** Without this the server still holds the
    // pending position, `~1` resolves against where the player *was*, and the assertion below passes only when
    // moving to the spawn happened to be a no-op — which is what it was while the spawn was the origin.
    harness.acknowledge_teleport().await;
    let absolute = harness.game.player(id).expect("player").position;
    harness.command("tp Commander ~1 ~ ~").await;
    let stepped = harness.game.player(id).expect("player").position;
    assert!(
        (stepped.x - (absolute.x + 1.0)).abs() < 0.6,
        "`~1` must be one block east of the player ({} -> {})",
        absolute.x,
        stepped.x
    );
    harness.service.shutdown().await;
}
