//! Integration: a real client through the rig to a real server (P10-01).
//!
//! The unit tests prove the trace is well formed; they cannot prove the rig is **transparent**. That needs
//! a real socket, so this starts an actual server, points the rig at it, and drives the `TestClient` through
//! the rig. If any byte were altered, the join would fail — which is what makes "the join succeeded" a
//! meaningful statement about the relay rather than about the trace.

use mc_capture_rig::serve;
use mc_server::config::ServerConfig;
use mc_server::lifecycle::Server;
use mc_test_support::client::TestClient;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

/// A sink the test can read back after the rig has written to it.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("trace buffer").clone()).expect("trace is utf-8")
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("trace buffer").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn start_server() -> (Server, SocketAddr, mc_server::lifecycle::ShutdownHandle) {
    let mut config = ServerConfig::default();
    "127.0.0.1:0".clone_into(&mut config.network.bind);
    "rig-test".clone_into(&mut config.network.motd);
    let mut server = Server::new(config);
    let addr = server.start_network().await.expect("network binds");
    let handle = server.shutdown_handle();
    (server, addr, handle)
}

/// Run the rig in front of `upstream` for exactly one connection, returning its address and trace buffer.
async fn start_rig(upstream: SocketAddr) -> (SocketAddr, SharedBuf) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("rig binds");
    let addr = listener.local_addr().expect("rig addr");
    let buf = SharedBuf::default();
    let sink = buf.clone();
    tokio::spawn(async move {
        let _ = serve(listener, upstream, Some(1), None, move || {
            Box::new(sink.clone()) as Box<dyn Write + Send>
        })
        .await;
    });
    (addr, buf)
}

/// Wait until the trace contains an event of `kind`, or the deadline passes.
///
/// `session_end` is written once both relay directions finish, which includes the server noticing the
/// client's EOF. That is not instantaneous, and a fixed sleep would be a guess about how fast it is.
async fn wait_for_event(trace: &SharedBuf, kind: &str) -> bool {
    for _ in 0..100 {
        if trace.text().contains(&format!("\"kind\":\"{kind}\"")) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn events(trace: &str) -> Vec<serde_json::Value> {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

#[tokio::test]
async fn a_client_reaches_play_through_the_rig_and_the_trace_records_the_join() {
    let (mut server, server_addr, shutdown) = start_server().await;
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (rig_addr, trace) = start_rig(server_addr).await;

    // The client talks to the **rig**, never to the server.
    let (mut client, joined) = TestClient::login_join(rig_addr, "RigTest")
        .await
        .expect("the join must succeed through the rig: any altered byte would fail it here");
    // `JoinResult` carries the whole `LoginSuccess`, so the name lives inside it.
    assert_eq!(joined.login.name, "RigTest");

    // Give the rig a moment to observe the last frames before the session closes.
    client.stay_alive(Duration::from_millis(200)).await.ok();
    drop(client);
    assert!(
        wait_for_event(&trace, "session_end").await,
        "the rig must close the session once both directions finish"
    );

    let text = trace.text();
    assert!(!text.is_empty(), "the rig must write a trace");

    let all = events(&text);
    let packets: Vec<&serde_json::Value> = all
        .iter()
        .filter(|event| event["kind"] == "packet")
        .collect();
    assert!(
        packets.len() > 4,
        "a join is more than four packets: {text}"
    );

    // The state machine must have walked handshake -> login -> config -> play, which is only possible if
    // the observer understood every transition.
    let states: Vec<&str> = packets.iter().filter_map(|p| p["state"].as_str()).collect();
    for expected in ["handshake", "login", "config", "play"] {
        assert!(
            states.contains(&expected),
            "the trace must reach {expected}: {states:?}"
        );
    }

    // Direction labels must be the documented short ones, not the enum spelling.
    assert!(
        packets.iter().any(|p| p["dir"] == "c2s") && packets.iter().any(|p| p["dir"] == "s2c"),
        "both directions must appear with the documented labels"
    );

    // The join's own packets must be named, which is what makes a trace readable.
    assert!(
        packets.iter().any(|p| p["name"] == "join_game"),
        "the server's first play-state packet should be named: {text}"
    );

    // Nothing may have degraded: an uninterpreted session is the one failure this tool must not hide.
    assert!(
        !text.contains("observer_error"),
        "no direction may degrade on a clean join: {text}"
    );
    let end = all
        .iter()
        .find(|event| event["kind"] == "session_end")
        .expect("the rig must close the session");
    assert_eq!(end["degraded"].as_array().map(Vec::len), Some(0));
    assert!(end["c2s_frames"].as_u64().unwrap_or(0) > 0);
    assert!(end["s2c_frames"].as_u64().unwrap_or(0) > 0);

    shutdown.request();
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;
}

#[tokio::test]
async fn the_rig_relays_a_status_ping_and_records_the_status_state() {
    let (mut server, server_addr, shutdown) = start_server().await;
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (rig_addr, trace) = start_rig(server_addr).await;

    let status = TestClient::status(rig_addr)
        .await
        .expect("a status ping must survive the rig");
    assert_eq!(status.motd, "rig-test");

    // The status path closes as soon as the response is sent, so waiting for the end also guarantees the
    // response has been traced.
    let _ = wait_for_event(&trace, "session_end").await;

    let text = trace.text();
    let packets: Vec<serde_json::Value> = events(&text)
        .into_iter()
        .filter(|event| event["kind"] == "packet")
        .collect();

    // A status intention must put the session in the status state, not in login by default. This is the
    // state machine doing real work: id 0 means `intention` in handshake and `status_request` in status.
    assert!(
        packets.iter().any(|p| p["state"] == "status"),
        "the ping must be traced in the status state: {text}"
    );
    assert!(
        packets.iter().any(|p| p["name"] == "status_response"),
        "and the response must be named: {text}"
    );
    assert!(
        !text.contains("observer_error"),
        "the ping must not degrade anything: {text}"
    );

    shutdown.request();
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;
}
