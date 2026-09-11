//! Keepalive behaviour over a live listener (P02-10/P02-15 regression).
//!
//! Uses short intervals so the suite stays fast; the production defaults are
//! 15 s interval / 30 s timeout (`NetworkSettings::default`).

use mc_network::{NetworkService, NetworkSettings};
use mc_protocol::ids::clientbound;
use mc_test_support::client::TestClient;
use std::time::Duration;

fn fast_settings() -> NetworkSettings {
    NetworkSettings {
        bind: "127.0.0.1:0".parse().expect("addr"),
        keepalive_interval: Duration::from_millis(150),
        keepalive_timeout: Duration::from_millis(400),
        compression_threshold: -1, // compression is covered by protocol tests
        ..NetworkSettings::default()
    }
}

#[tokio::test]
async fn responsive_client_survives_and_silent_client_is_kicked() {
    let service = NetworkService::start(fast_settings(), None)
        .await
        .expect("starts");
    let addr = service.local_addr();

    // Responsive client: answers keepalives and stays connected.
    let (mut alive, _join) = TestClient::login_join(addr, "Keeper").await.expect("join");
    let answered = alive
        .stay_alive(Duration::from_millis(600))
        .await
        .expect("stays alive");
    assert!(
        answered >= 1,
        "expected at least one keepalive, got {answered}"
    );

    // Silent client: completes login but never answers and must be kicked.
    let (mut silent, _join) = TestClient::login_join(addr, "Silent").await.expect("join");
    tokio::time::sleep(Duration::from_millis(900)).await;
    let mut saw_disconnect = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match silent.recv().await {
            Ok(packet) if packet.id == clientbound::play::DISCONNECT => {
                saw_disconnect = true;
                break;
            }
            Ok(_) => {}      // keepalives buffered before the kick
            Err(_) => break, // EOF also manifests the kick
        }
    }
    assert!(
        saw_disconnect,
        "silent client should receive a play disconnect"
    );
    drop(alive);

    service.shutdown().await;
}
