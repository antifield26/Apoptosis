//! Network → game bridge integration test (P04-03, P04-16).
//!
//! The survival E2E suite drives [`Game`] directly. This one closes the remaining
//! gap: a **real socket** connection that logs in through the actual protocol state
//! machine, enters play, and is then driven by the real game loop through the
//! bridge. It is the evidence that the two halves are wired together — which unit
//! tests on either side cannot show.

// The scenario reads coordinates back out of the world and narrows them to
// chunk indices; the crate root documents the same cast exemption.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::clientbound;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::time::Duration;

/// Everything a bridge test needs, kept alive for the test's duration.
struct Bridge {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    _storage: WorldService,
    _dir: TempDir,
}

impl Bridge {
    /// Start a server, log a real client in, and tick until the game sees it.
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

        let (client, join) = TestClient::login_join(addr, "Bridger")
            .await
            .expect("a real socket login completes");
        assert_eq!(join.login.name, "Bridger");

        // The connection task queued the join; tick until the game applies it.
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

    /// Run a few ticks, letting the connection task drain its socket in between.
    async fn settle(&mut self, ticks: usize) {
        for _ in 0..ticks {
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.game.tick().expect("tick");
        }
    }
}

#[tokio::test]
async fn a_real_socket_login_becomes_a_game_player() {
    let bridge = Bridge::start("p04-bridge").await;
    assert_eq!(
        bridge.game.player_count(),
        1,
        "the real client's login must surface as a game player"
    );
    let id = bridge.game.players().next().expect("one player");
    let player = bridge.game.player(id).expect("player exists");
    assert_eq!(player.profile.name, "Bridger");
    assert!(player.is_alive(), "a fresh player is alive");
    let chunk = mc_world::ChunkPos::new(
        (player.position.x.floor() as i32) >> 4,
        (player.position.z.floor() as i32) >> 4,
    );
    assert!(
        bridge.game.world().is_loaded(chunk),
        "the join must load the chunk the player stands in"
    );
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn the_client_receives_terrain_over_the_socket() {
    let mut bridge = Bridge::start("p04-bridge-chunks").await;
    bridge.settle(6).await;

    // Read what actually arrived on the wire for this player.
    let mut chunks = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && chunks < 4 {
        match tokio::time::timeout(Duration::from_millis(250), bridge.client.recv()).await {
            Ok(Ok(raw)) => {
                if raw.id == clientbound::play::LEVEL_CHUNK_WITH_LIGHT {
                    chunks += 1;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => {
                // Nothing yet: give the game another tick and keep waiting.
                bridge.game.tick().expect("tick");
            }
        }
    }
    assert!(
        chunks > 0,
        "a joined client must receive terrain packets over the socket"
    );
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_client_disconnect_removes_the_player() {
    let mut bridge = Bridge::start("p04-bridge-leave").await;
    assert_eq!(bridge.game.player_count(), 1);

    // Drop the socket: the connection task sees EOF and reports `Left`.
    bridge.service.shutdown().await;
    drop(bridge.client);
    for _ in 0..40 {
        bridge.game.tick().expect("tick");
        if bridge.game.player_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        bridge.game.player_count(),
        0,
        "a disconnected client must be removed from the simulation"
    );
}

#[tokio::test]
async fn a_movement_intent_from_the_socket_is_applied() {
    let mut bridge = Bridge::start("p04-bridge-move").await;
    let id = bridge.game.players().next().expect("one player");
    let (sx, sy, sz) = bridge.game.spawn();
    let before = bridge.game.player(id).expect("player").position;

    // Send exactly the bytes a real client sends for `move_player_pos`:
    // three f64 coordinates then a bool. `PlayIntent` is decode-only, so the test
    // encodes the packet itself — which is also what makes this a wire test.
    let mut payload = Vec::with_capacity(25);
    payload.extend_from_slice(&(f64::from(sx) + 1.5).to_be_bytes());
    payload.extend_from_slice(&f64::from(sy).to_be_bytes());
    payload.extend_from_slice(&(f64::from(sz) + 0.5).to_be_bytes());
    payload.push(1); // on_ground
    bridge
        .client
        .send_raw_packet(&RawPacket::new(
            mc_protocol::ids::serverbound::play::MOVE_PLAYER_POS,
            payload,
        ))
        .await
        .expect("movement sent");
    bridge.settle(8).await;

    let after = bridge.game.player(id).expect("player").position;
    assert!(
        after.x.is_finite() && after.y.is_finite() && after.z.is_finite(),
        "the position must stay finite"
    );
    assert!(
        after.y > -64.0 && after.y < 320.0,
        "the player must stay inside the world, got {after:?}"
    );

    // What this proves: the intent travelled socket → codec → bridge → game and
    // changed the server's *decision*. The requested step is a legal 1.5-block move
    // in a world with no collision geometry (generation is P07, so the default
    // world is air), so the server must either hold exactly the requested position
    // or have re-anchored the client with a `player_position` correction. The
    // previous phrasing — `after != before || game.player(id).is_some()` — was true
    // for any live player and therefore proved nothing about movement.
    let requested = (f64::from(sx) + 1.5, f64::from(sy), f64::from(sz) + 0.5);
    let accepted = (after.x - requested.0).abs() < 0.001
        && (after.y - requested.1).abs() < 0.001
        && (after.z - requested.2).abs() < 0.001;
    if !accepted {
        // Any correction was queued during `settle` and is on the socket by now.
        let mut corrected = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while !corrected && tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), bridge.client.recv()).await {
                Ok(Ok(raw)) => corrected = raw.id == clientbound::play::PLAYER_POSITION,
                _ => break,
            }
        }
        assert!(
            corrected,
            "the server neither accepted the requested step {requested:?} nor corrected \
             the client; it holds {after:?} (it started at {before:?})"
        );
    }
    bridge.service.shutdown().await;
}
