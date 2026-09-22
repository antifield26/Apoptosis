//! Container transaction tests over a real socket (P06-01, P06-02, P06-15).
//!
//! `mc-container`'s unit tests prove the transaction rules against a `Menu`. This
//! file proves the *wire* half: a `container_click` sent by a client is decoded by
//! `mc-protocol`, validated against the session's menu, applied, and the result is
//! sent back as `container_set_slot`/`container_set_content`. That path is what
//! makes the rules reachable by a real client, and nothing else tests it.

// A VarInt is emitted seven bits at a time from the value's unsigned
// reinterpretation, so these casts are the encoding rather than incidental
// narrowing.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use mc_container::ClickType;
use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::time::Duration;

/// A server with a real socket and a logged-in client, plus the game loop.
struct Bridge {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    _storage: WorldService,
    _dir: TempDir,
}

impl Bridge {
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
        let (client, _join) = TestClient::login_join_tick(addr, "Trader", || {
            game.tick().expect("tick");
        })
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

    /// Encode a `container_click` exactly as the client would.
    fn click_packet(
        window_id: i32,
        state_id: i32,
        slot: i16,
        button: i8,
        click_type: ClickType,
    ) -> RawPacket {
        let mut payload = Vec::new();
        // VarInt window id.
        payload.extend(varint(window_id));
        // VarInt state id.
        payload.extend(varint(state_id));
        // i16 slot, i8 button, VarInt click type.
        payload.extend(slot.to_be_bytes());
        payload.push(button as u8);
        payload.extend(varint(click_type.id()));
        RawPacket::new(serverbound::play::CONTAINER_CLICK, payload)
    }

    async fn send_click(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        slot: i16,
        button: i8,
        click_type: ClickType,
    ) {
        let state = self.game.menu_state_id(id).unwrap_or(0);
        let packet = Self::click_packet(0, state, slot, button, click_type);
        self.client
            .send_raw_packet(&packet)
            .await
            .expect("click sent");
        self.settle(3).await;
    }

    async fn settle(&mut self, ticks: usize) {
        for _ in 0..ticks {
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.game.tick().expect("tick");
        }
    }

    /// Read packets until the deadline, returning the ids seen.
    async fn drain_ids(&mut self, millis: u64) -> Vec<i32> {
        let mut ids = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(millis);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(80), self.client.recv()).await {
                Ok(Ok(raw)) => ids.push(raw.id),
                Ok(Err(_)) => break,
                Err(_) => {
                    self.game.tick().expect("tick");
                }
            }
        }
        ids
    }
}

/// Minimal `VarInt` encoder for the test's own packets.
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
async fn a_player_joins_with_a_menu_holding_their_inventory() {
    let bridge = Bridge::start("p06-menu").await;
    assert_eq!(bridge.game.player_count(), 1);
    let player = bridge.game.players().next().expect("a player");
    let session = bridge.game.session(player).expect("the session");
    // Menu slot 0 is the crafting result, 45 the offhand: the jar-verified layout.
    assert_eq!(session.menu_slot_count(), 46);
    assert_eq!(session.menu_window_id(), 0);
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_container_click_over_the_socket_moves_items_and_is_acknowledged() {
    let mut bridge = Bridge::start("p06-click").await;
    let player = bridge.game.players().next().expect("a player");

    // Seed the player's hotbar the way the server would (a game-mode grant).
    bridge
        .game
        .grant_item(player, "minecraft:stone", 64)
        .expect("granted");
    bridge.settle(2).await;

    // The client picks up menu slot 36, which is hotbar slot 0.
    let before = bridge.game.menu_total_items(player).expect("a menu");
    bridge
        .client
        .send_raw_packet(&Bridge::click_packet(
            0,
            bridge.game.menu_state_id(player).unwrap_or(0),
            36,
            0,
            ClickType::Pickup,
        ))
        .await
        .expect("click");
    bridge.settle(3).await;

    let after = bridge.game.menu_total_items(player).expect("a menu");
    assert_eq!(
        after, before,
        "a pickup must conserve items (cursor plus containers)"
    );
    assert!(
        bridge.game.menu_cursor(player).map(|stack| stack.count()) == Some(64),
        "the whole stack should be on the cursor, got {:?}",
        bridge.game.menu_cursor(player)
    );

    // The server must have told the client about it.
    let ids = bridge.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_SLOT)
            || ids.contains(&clientbound::play::CONTAINER_SET_CONTENT),
        "the server must acknowledge the click, saw {ids:?}"
    );
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_stale_state_id_gets_a_full_resync_instead_of_moving_items() {
    let mut bridge = Bridge::start("p06-stale").await;
    let player = bridge.game.players().next().expect("a player");
    bridge
        .game
        .grant_item(player, "minecraft:stone", 64)
        .expect("granted");
    bridge.settle(2).await;
    let before = bridge.game.menu_total_items(player).expect("a menu");

    // Deliberately wrong state id: the client is out of sync.
    let packet = Bridge::click_packet(0, 9_999, 36, 0, ClickType::Pickup);
    bridge
        .client
        .send_raw_packet(&packet)
        .await
        .expect("click sent");
    bridge.settle(3).await;

    assert_eq!(
        bridge.game.menu_total_items(player),
        Some(before),
        "a stale click must not move anything"
    );
    let ids = bridge.drain_ids(600).await;
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_CONTENT),
        "a stale state id must trigger a full resync, saw {ids:?}"
    );
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_malformed_click_is_ignored_and_never_kicks() {
    let mut bridge = Bridge::start("p06-malformed").await;
    let player = bridge.game.players().next().expect("a player");
    let before = bridge.game.menu_total_items(player).expect("a menu");

    // An unknown click type, a slot far out of range, and a negative state id.
    for packet in [
        RawPacket::new(serverbound::play::CONTAINER_CLICK, {
            let mut v = varint(0);
            v.extend(varint(0));
            v.extend(0i16.to_be_bytes());
            v.push(0);
            v.extend(varint(99));
            v
        }),
        RawPacket::new(serverbound::play::CONTAINER_CLICK, {
            let mut v = varint(0);
            v.extend(varint(0));
            v.extend(i16::MAX.to_be_bytes());
            v.push(0);
            v.extend(varint(0));
            v
        }),
        RawPacket::new(serverbound::play::CONTAINER_CLICK, {
            let mut v = varint(0);
            v.extend(varint(-1));
            v.extend(0i16.to_be_bytes());
            v.push(0);
            v.extend(varint(0));
            v
        }),
    ] {
        bridge.client.send_raw_packet(&packet).await.expect("sent");
        bridge.settle(2).await;
    }

    assert_eq!(
        bridge.game.menu_total_items(player),
        Some(before),
        "malformed clicks must change nothing"
    );
    assert_eq!(
        bridge.game.player_count(),
        1,
        "malformed clicks must never disconnect the client"
    );
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_truncated_click_payload_ends_only_that_connection() {
    // A *truncated* payload is a real decode failure, unlike an unknown click type
    // (which is refused and ignored). Refusing it by closing the connection is
    // deliberate and matches Vanilla, whose `PacketDecoder` does the same. The
    // property this test pins down is therefore the one that matters for security:
    // **the server survives it and other players are unaffected**, not that the
    // offending client is kept.
    let mut bridge = Bridge::start("p06-truncated").await;
    assert_eq!(bridge.game.player_count(), 1);

    let full = {
        let mut v = varint(0);
        v.extend(varint(0));
        v.extend(36i16.to_be_bytes());
        v.push(0);
        v.extend(varint(0));
        v
    };
    // Send every strict prefix of a valid click; the first one that cannot decode
    // closes this connection.
    let mut closed = false;
    for length in 0..full.len() {
        let packet = RawPacket::new(serverbound::play::CONTAINER_CLICK, full[..length].to_vec());
        if bridge.client.send_raw_packet(&packet).await.is_err() {
            closed = true;
            break;
        }
        bridge.settle(1).await;
    }

    // Give the server time to notice and reap the connection.
    for _ in 0..20 {
        bridge.game.tick().expect("tick");
        if bridge.game.player_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    assert!(
        closed || bridge.game.player_count() == 0,
        "a truncated payload must be refused (connection closed) rather than \
         silently accepted"
    );
    // The listener is still serving: a fresh client can log in.
    let addr = bridge.service.local_addr();
    let (_fresh, _join) = TestClient::login_join_tick(addr, "Survivor", || {
        bridge.game.tick().expect("tick");
    })
    .await
    .expect("the server must still accept logins after a malformed packet");
    bridge.service.shutdown().await;
}

#[tokio::test]
async fn a_throw_over_the_socket_removes_exactly_one_item() {
    let mut bridge = Bridge::start("p06-throw").await;
    let player = bridge.game.players().next().expect("a player");
    bridge
        .game
        .grant_item(player, "minecraft:stone", 10)
        .expect("granted");
    bridge.settle(2).await;
    let before = bridge.game.menu_total_items(player).expect("a menu");
    let entities_before = bridge.game.entity_store().len();

    bridge.send_click(player, 36, 0, ClickType::Throw).await;

    assert_eq!(
        bridge.game.menu_total_items(player),
        Some(before - 1),
        "a throw removes exactly one item from the window"
    );
    assert_eq!(
        bridge.game.entity_store().len(),
        entities_before + 1,
        "and the thrown item becomes a real entity, not a silent deletion"
    );
    bridge.service.shutdown().await;
}
