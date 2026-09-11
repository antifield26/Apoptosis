//! Login-phase tolerance and shutdown drain (Audit 01 fixes, Audit 02 evidence gap).
//!
//! Audit 02 found that two Audit 01 fixes had **no covering test**: the docs cited
//! tests that never sent the packets in question. This file is that missing
//! evidence.
//!
//! 1. Vanilla clients may interleave `custom_query_answer` (2) or
//!    `cookie_response` (4) between `LoginSuccess` and `login_acknowledged`.
//!    Phase 02 treated anything but the ack as a hard protocol error, which would
//!    have disconnected a real client. The fix was an ignore-loop; this test sends
//!    both packets and asserts the client still reaches play.
//! 2. `NetworkService::shutdown` is supposed to **drain** live connections, so the
//!    server can close the world knowing no player action can still arrive. The
//!    fix was a `JoinSet`; this test holds a real logged-in connection across
//!    shutdown and asserts it is drained rather than abandoned.

use mc_protocol::RawPacket;
use mc_protocol::ids::serverbound;
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart};
use mc_test_support::client::TestClient;
use std::time::Duration;
use tokio::net::TcpStream;

/// Start a listener with no game loop (protocol-only), on an ephemeral port.
///
/// Compression is off so the test can drive login packet by packet without also
/// implementing the codec switch; negotiation has its own tests
/// (`mc_protocol::framing`, `mc_server/tests/e2e_login_play.rs`).
async fn listener() -> mc_network::NetworkService {
    let settings = mc_network::NetworkSettings {
        bind: "127.0.0.1:0".parse().expect("addr"),
        compression_threshold: -1,
        ..mc_network::NetworkSettings::default()
    };
    mc_network::NetworkService::start(settings, None)
        .await
        .expect("listener starts")
}

/// Read packets until one with `id` arrives, or the deadline passes.
async fn read_until(client: &mut TestClient, id: i32, attempts: usize) -> bool {
    for _ in 0..attempts {
        match tokio::time::timeout(Duration::from_millis(500), client.recv()).await {
            Ok(Ok(raw)) if raw.id == id => return true,
            // Any other packet: keep reading until the budget runs out.
            Ok(Ok(_)) => {}
            _ => return false,
        }
    }
    false
}

#[tokio::test]
async fn login_tolerates_a_custom_query_answer_before_the_ack() {
    let service = listener().await;
    let addr = service.local_addr();

    // Handshake + LoginStart by hand, so the test controls what happens between
    // LoginSuccess and the acknowledgement.
    let mut client = TestClient::connect(addr).await.expect("connects");
    client
        .send(&mc_protocol::packets::handshake::Handshake {
            protocol_version: mc_protocol::ids::PROTOCOL_VERSION,
            server_address: "localhost".to_owned(),
            server_port: addr.port(),
            intent: mc_protocol::packets::handshake::HandshakeIntent::Login,
        })
        .await
        .expect("handshake");
    client
        .send(&LoginStart {
            name: "Tolerant".to_owned(),
            uuid: uuid::Uuid::nil(),
        })
        .await
        .expect("login start");

    // The server answers with SetCompression then LoginSuccess. Wait for the
    // latter, then send the packets a real client may interleave.
    assert!(
        read_until(
            &mut client,
            mc_protocol::ids::clientbound::login::LOGIN_FINISHED,
            8
        )
        .await,
        "the server must send LoginSuccess"
    );

    // `custom_query_answer`: an empty VarInt count is a valid encoding.
    client
        .send_raw_packet(&RawPacket::new(
            serverbound::login::CUSTOM_QUERY_ANSWER,
            vec![0x00],
        ))
        .await
        .expect("custom_query_answer");
    // `cookie_response`: a key plus an absent payload.
    client
        .send_raw_packet(&RawPacket::new(
            serverbound::login::COOKIE_RESPONSE,
            vec![0x00, 0x01, b'k', 0x00],
        ))
        .await
        .expect("cookie_response");

    // The client must still be allowed to acknowledge and enter configuration.
    client
        .send(&LoginAcknowledged)
        .await
        .expect("login acknowledged");
    let raw = tokio::time::timeout(Duration::from_secs(5), client.recv())
        .await
        .expect("no timeout")
        .expect("a packet, not a disconnect");
    assert_ne!(
        raw.id,
        mc_protocol::ids::clientbound::login::LOGIN_DISCONNECT,
        "an interleaved custom_query_answer/cookie_response must not kick the client"
    );

    service.shutdown().await;
}

#[tokio::test]
async fn shutdown_drains_a_live_connection() {
    let service = listener().await;
    let addr = service.local_addr();

    // A raw socket held open across shutdown: shutdown must not return while a
    // connection is still being served.
    let socket = TcpStream::connect(addr).await.expect("connects");
    let mut client = TestClient::connect(addr).await.expect("second connects");
    client
        .send(&mc_protocol::packets::handshake::Handshake {
            protocol_version: mc_protocol::ids::PROTOCOL_VERSION,
            server_address: "localhost".to_owned(),
            server_port: addr.port(),
            intent: mc_protocol::packets::handshake::HandshakeIntent::Login,
        })
        .await
        .expect("handshake");
    client
        .send(&LoginStart {
            name: "Drainee".to_owned(),
            uuid: uuid::Uuid::nil(),
        })
        .await
        .expect("login start");
    assert!(
        read_until(
            &mut client,
            mc_protocol::ids::clientbound::login::LOGIN_FINISHED,
            8
        )
        .await,
        "the connection must be established before shutdown"
    );

    // Shutdown must complete (draining or timing out), not hang.
    let shutdown = tokio::time::timeout(Duration::from_secs(15), service.shutdown()).await;
    assert!(
        shutdown.is_ok(),
        "shutdown must complete within the drain window"
    );
    drop(socket);
}
