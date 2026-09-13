//! End-to-end client/server tests for the Phase-02 protocol slice (P02-14).
//!
//! These drive a real TCP listener through the full offline
//! handshake → login → configuration → play conversation using the
//! `mc-test-support` client, and prove hostile input only drops connections.

use mc_core::error::ServerError;
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
use mc_protocol::packets::login::LoginDisconnect;
use mc_server::config::ServerConfig;
use mc_server::lifecycle::Server;
use mc_test_support::client::{EXPECTED_PROTOCOL, TestClient};
use std::net::SocketAddr;

/// Start a server on an ephemeral port and return it with its address.
async fn start_server(motd: &str) -> (Server, SocketAddr, mc_server::lifecycle::ShutdownHandle) {
    let mut config = ServerConfig::default();
    "127.0.0.1:0".clone_into(&mut config.network.bind);
    config.network.motd = motd.to_owned();
    let mut server = Server::new(config);
    let addr = server.start_network().await.expect("network binds");
    let handle = server.shutdown_handle();
    (server, addr, handle)
}

async fn run_until_shutdown(mut server: Server) -> ServerError {
    server
        .run()
        .await
        .expect_err("run returns Shutdown on request")
}

#[tokio::test]
async fn status_ping_reports_protocol_and_motd() {
    let (server, addr, handle) = start_server("E2E status").await;
    let task = tokio::spawn(run_until_shutdown(server));

    let status = TestClient::status(addr).await.expect("status flow");
    assert_eq!(status.protocol, EXPECTED_PROTOCOL);
    assert_eq!(status.version_name, "26.1.2");
    assert_eq!(status.max_players, 10);
    assert_eq!(status.motd, "E2E status");

    handle.request();
    assert!(matches!(task.await.expect("join"), ServerError::Shutdown));
}

#[tokio::test]
async fn offline_login_reaches_play_with_registry_payload() {
    let (server, addr, handle) = start_server("E2E login").await;
    let task = tokio::spawn(run_until_shutdown(server));

    let (_client, joined) = TestClient::login_join(addr, "TestBot")
        .await
        .expect("login flow");
    assert_eq!(joined.login.name, "TestBot");
    assert!(
        joined.login.properties.is_empty(),
        "offline profiles carry no properties"
    );
    // The count is derived, not pinned: the server sends the two hand-authored registries plus every
    // registry the extracted fixture holds, and that set grew when a real client named what it required
    // (P10-03). What matters is the rule below, not the number.
    assert!(
        joined.registries.len() >= 2,
        "at least dimension_type and worldgen/biome are expected, got {}",
        joined.registries.len()
    );
    for registry in &joined.registries {
        assert!(
            !registry.entries.is_empty(),
            "{} was sent empty; a real 26.1.2 client refuses an empty synced registry (P10-03)",
            registry.registry
        );
    }
    for expected in [
        "minecraft:dimension_type",
        "minecraft:worldgen/biome",
        // The two the overworld dimension *references*. Absent, a real client refuses the session in its
        // registry loader — that was KD-39.
        "minecraft:world_clock",
        "minecraft:timeline",
    ] {
        assert!(
            joined
                .registries
                .iter()
                .any(|registry| registry.registry == expected),
            "{expected} must be sent; got {:?}",
            joined
                .registries
                .iter()
                .map(|registry| &registry.registry)
                .collect::<Vec<_>>()
        );
    }
    let dimension = joined
        .registries
        .iter()
        .find(|registry| registry.registry == "minecraft:dimension_type")
        .expect("dimension registry");
    // Entry **0** is the contract, not the count: `join_game` references `dimension_type_id: 0`, so the
    // overworld has to be first. The payload is now the captured vanilla set (4 dimensions) rather than the
    // single hand-authored entry this test used to expect.
    assert_eq!(dimension.entries[0].id, "minecraft:overworld");
    assert!(
        !dimension.entries.is_empty(),
        "the dimension registry must not be empty: a real client refuses an empty synced registry"
    );
    // Ids only, and that is deliberate rather than a regression: a 26.1.2 client declared
    // `minecraft:core = 26.1.2` under `select_known_packs`, so vanilla sends the registry shape without
    // element data, and the client reads the content from its own jar (P10-03, captured).
    assert!(
        dimension.entries[0].data.is_none(),
        "elements are ids only; sending data the protocol does not ask for is what broke enchantment"
    );

    assert_eq!(joined.join.dimension_name, "minecraft:overworld");
    assert_eq!(joined.join.max_players, 10);
    assert_eq!(joined.join.view_distance, 8);

    handle.request();
    assert!(matches!(task.await.expect("join"), ServerError::Shutdown));
}

#[tokio::test]
async fn offline_uuid_matches_derivation_rule() {
    let (server, addr, handle) = start_server("E2E uuid").await;
    let task = tokio::spawn(run_until_shutdown(server));

    let (_client, joined) = TestClient::login_join(addr, "Notch")
        .await
        .expect("login flow");
    assert_eq!(
        joined.login.uuid.to_string(),
        "b50ad385-829d-3141-a216-7e7d7539ba7f"
    );

    handle.request();
    assert!(matches!(task.await.expect("join"), ServerError::Shutdown));
}

#[tokio::test]
async fn wrong_protocol_is_kicked_with_disconnect() {
    let (server, addr, handle) = start_server("E2E version").await;
    let task = tokio::spawn(run_until_shutdown(server));

    let mut client = TestClient::connect(addr).await.expect("connect");
    client
        .send(&Handshake {
            protocol_version: EXPECTED_PROTOCOL - 1,
            server_address: addr.ip().to_string(),
            server_port: addr.port(),
            intent: HandshakeIntent::Login,
        })
        .await
        .expect("handshake");
    let packet = client.recv().await.expect("disconnect packet");
    assert_eq!(packet.id, clientbound::login::LOGIN_DISCONNECT);
    let disconnect = LoginDisconnect::decode(&packet.payload).expect("decodes");
    assert!(
        disconnect.json.contains("Outdated client"),
        "{}",
        disconnect.json
    );

    // Server is still healthy for other clients.
    let status = TestClient::status(addr).await.expect("status after kick");
    assert_eq!(status.protocol, EXPECTED_PROTOCOL);

    handle.request();
    assert!(matches!(task.await.expect("join"), ServerError::Shutdown));
}

#[tokio::test]
async fn malformed_input_drops_connection_but_not_process() {
    let (server, addr, handle) = start_server("E2E hostile").await;
    let task = tokio::spawn(run_until_shutdown(server));

    // Non-terminating VarInt frame length then garbage.
    let mut client = TestClient::connect(addr).await.expect("connect");
    client
        .send_bytes(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
        .await
        .expect("write garbage");
    // The server must close this connection; reading yields EOF or an error.
    let outcome = client.recv().await;
    assert!(
        outcome.is_err(),
        "expected the hostile connection to be dropped"
    );

    // Socket-level fuzz: a deterministic corpus of random byte strings, each
    // on its own connection. None may crash or wedge the process.
    let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
    for round in 0..12u32 {
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let length = (next() % 96) as usize;
        let bytes: Vec<u8> = (0..length).map(|_| (next() & 0xFF) as u8).collect();
        let Ok(mut fuzz_client) = TestClient::connect(addr).await else {
            continue; // per-IP budget may refuse; that is acceptable
        };
        let _ = fuzz_client.send_bytes(&bytes).await;
        let _ =
            tokio::time::timeout(std::time::Duration::from_millis(200), fuzz_client.recv()).await;
        let _ = round;
    }

    // A fresh client still completes login: the process survived. The token
    // bucket deliberately throttled the burst above, so wait for refill first.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let (_client, joined) = TestClient::login_join(addr, "Survivor")
        .await
        .expect("login flow");
    assert_eq!(joined.join.dimension_name, "minecraft:overworld");

    handle.request();
    assert!(matches!(task.await.expect("join"), ServerError::Shutdown));
}

#[tokio::test]
async fn online_mode_refuses_to_start_without_provider() {
    let mut config = ServerConfig::default();
    config.network.online_mode = true;
    let mut server = Server::new(config);
    let error = server
        .start_network()
        .await
        .expect_err("online mode must fail fast in Phase 02");
    assert!(matches!(error, ServerError::Operational(_)));
}
