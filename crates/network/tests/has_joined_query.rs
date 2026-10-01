//! `hasJoined` query shape (AUDIT-19 C19-M3).
//!
//! The session check must carry the joining address: vanilla (and authlib)
//! always append `ip=`, and without it the session answer is not bound to the
//! address the player joins from. The stub below is a raw TCP listener that
//! captures the request line, so what is asserted is the bytes that would
//! reach Mojang — not our own round-trip.
//!
//! Run: `cargo test -p mc-network --test has_joined_query`.

use mc_network::online::MojangClient;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::time::Duration;

/// A profile body the session server could return.
const PROFILE: &str = r#"{"id":"069a79f444e94726a5bef4a7b64ac909","name":"Notch"}"#;

/// Serve one canned HTTP 200 and report the request line the stub saw.
fn stub_once(body: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("stub binds");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("one connection");
        let mut request = [0u8; 4096];
        let read = stream.read(&mut request).expect("read");
        let head = String::from_utf8_lossy(&request[..read]).into_owned();
        let line = head.lines().next().unwrap_or_default().to_owned();
        let _ = tx.send(line);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (base, rx)
}

/// The request target the stub saw, for one `has_joined` call.
fn target_for(ip: Option<IpAddr>) -> String {
    let (base, request_rx) = stub_once(PROFILE);
    MojangClient::at_base(&base)
        .has_joined("Notch", "hash", ip)
        .expect("200 parses");
    let request = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the stub saw a request");
    request
        .split_whitespace()
        .nth(1)
        .expect("a request target")
        .to_owned()
}

#[test]
fn has_joined_sends_the_joining_address() {
    let target = target_for(Some("203.0.113.7".parse().expect("test address")));
    assert_eq!(
        target, "/session/minecraft/hasJoined?username=Notch&serverId=hash&ip=203.0.113.7",
        "vanilla's parameter order, with the joining address last"
    );
}

#[test]
fn has_joined_sends_an_unbracketed_ipv6_address() {
    // `InetAddress.getHostAddress()` — the shape vanilla appends — has no
    // brackets, and neither does `IpAddr`'s Display.
    let target = target_for(Some("2001:db8::1".parse().expect("test address")));
    assert_eq!(
        target, "/session/minecraft/hasJoined?username=Notch&serverId=hash&ip=2001:db8::1",
        "an IPv6 join address keeps the host-address shape"
    );
}

#[test]
fn has_joined_without_an_address_omits_the_parameter() {
    // The `Option` is the caller's: no address means the pre-fix URL shape,
    // never a guessed one.
    let target = target_for(None);
    assert_eq!(
        target, "/session/minecraft/hasJoined?username=Notch&serverId=hash",
        "no address, no ip= parameter"
    );
}

/// The **wiring**, not just the URL builder: the provider the login path calls
/// must carry the peer address into the request. AUDIT-19 C19-M3's first half
/// was a builder that could take an address while every caller passed `None` —
/// so this test goes through the trait method the connection uses.
#[tokio::test]
async fn the_provider_binds_the_session_to_the_peer_address() {
    use mc_network::auth::OnlineAuthProvider as _;
    use mc_network::online::MojangSessionAuth;

    let (base, request_rx) = stub_once(PROFILE);
    let auth = MojangSessionAuth::at_base(&base);
    let profile = auth
        .authenticate_from(
            "Notch",
            "hash",
            Some("203.0.113.7".parse().expect("test address")),
        )
        .await
        .expect("the stub's 200 authenticates");
    assert_eq!(profile.name, "Notch", "the profile is the stub's");

    let request = request_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the stub saw a request");
    let target = request.split_whitespace().nth(1).expect("a request target");
    assert!(
        target.contains("&ip=203.0.113.7"),
        "the address must reach the session server through the provider, saw {target}"
    );
}
