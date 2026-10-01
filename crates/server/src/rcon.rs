//! RCON admin protocol: packets, auth and request plumbing (P19-04).
//!
//! Vanilla's RCON is little-endian framing around three types: `LOGIN` (3),
//! `COMMAND` (2) and `RESPONSE` (0). A body is
//! `[length i32LE][id i32LE][type i32LE][payload][0x00 0x00]` where `length`
//! covers everything after itself. Auth answers with the request id on
//! success and id `-1` on failure; commands answer with one or more
//! `RESPONSE` packets (long output is chunked — a single packet is capped
//! like a request).
//!
//! What this module owns: the codec (with a hard length cap — a hostile
//! length prefix drops the connection before any allocation), the password
//! check (fixed-time comparison, never early-out), the request channel
//! into the tick loop, and the listener's admission budgets (AUDIT-19
//! A-05/C19-M1/C19-M2: concurrent sockets reuse the game listener's
//! [`ConnectionGate`], bad logins accumulate per address so reconnecting
//! cannot reset the throttle). What it does not own: sockets and the
//! accept loop (the binary's listener, `apps/server/src/rcon.rs`), and the
//! world itself (commands run through `Game::dispatch_console`).

use mc_core::error::{ServerError, ServerResult};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The game listener's admission gate and its refusal reason, re-exported so
/// the RCON listener in the binary can reuse the one limiter instead of
/// growing a second one.
///
/// AUDIT-19 A-05/C19-M1 found the admin port accepting unbounded sockets: the
/// game listener had a gate, RCON had none. Re-exporting the types (the binary
/// does not depend on `mc-network` directly) keeps a single implementation of
/// "how many sockets may this address hold", so the two listeners cannot drift.
pub use mc_network::limits::{ConnectionGate, LimitError};

/// Login packet type.
pub const TYPE_LOGIN: i32 = 3;
/// Command / auth-answer packet type.
pub const TYPE_COMMAND: i32 = 2;
/// Command-output packet type.
pub const TYPE_RESPONSE: i32 = 0;

/// Id the server answers a failed login with.
pub const AUTH_FAILURE_ID: i32 = -1;

/// Largest framed body accepted, header included.
///
/// Vanilla caps requests at 4 KiB; anything declaring more is hostile
/// (the classic overflow is a huge length followed by a short read), so
/// the connection dies before a single payload byte is allocated.
pub const MAX_PACKET_LEN: usize = 4096;

/// Whether a declared length may be read (P19-04).
///
/// Checked before allocating or reading the body: a hostile prefix fails
/// here, never in an allocator.
#[must_use]
pub const fn length_accepted(declared: usize) -> bool {
    declared <= MAX_PACKET_LEN
}

/// Largest reply text carried in one `RESPONSE` packet body.
pub const MAX_RESPONSE_BODY: usize = 4000;

/// One decoded packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RconPacket {
    /// Request id (echoed in the answer; `-1` names an auth failure).
    pub id: i32,
    /// One of `TYPE_LOGIN`, `TYPE_COMMAND`, `TYPE_RESPONSE`.
    pub kind: i32,
    /// Payload without the two trailing zero bytes.
    pub payload: Vec<u8>,
}

/// Decode one framed body (without the length prefix).
///
/// Refuses trailing garbage, missing padding and unknown types: a decoder
/// that guessed would turn a hostile probe into a command-shaped object.
///
/// # Errors
///
/// [`ServerError::Protocol`] naming the violation.
///
/// # Panics
///
/// Never in practice: the fixed-width reads sit behind the 10-byte length
/// guard above, and the crate's own tests fail the build if that changes.
pub fn decode_body(body: &[u8]) -> ServerResult<RconPacket> {
    if body.len() < 10 {
        return Err(ServerError::Protocol(format!(
            "rcon body is {} bytes, shorter than id + type + padding",
            body.len()
        )));
    }
    let id = i32::from_le_bytes(body[0..4].try_into().expect("sliced"));
    let kind = i32::from_le_bytes(body[4..8].try_into().expect("sliced"));
    if !matches!(kind, TYPE_LOGIN | TYPE_COMMAND | TYPE_RESPONSE) {
        return Err(ServerError::Protocol(format!(
            "rcon packet has unknown type {kind}"
        )));
    }
    let payload = &body[8..];
    if payload.len() < 2 || payload[payload.len() - 2..] != [0, 0] {
        return Err(ServerError::Protocol(
            "rcon payload misses its two trailing zero bytes".to_owned(),
        ));
    }
    Ok(RconPacket {
        id,
        kind,
        payload: payload[..payload.len() - 2].to_vec(),
    })
}

/// Encode one packet, returning the length-prefixed bytes.
#[must_use]
pub fn encode_packet(id: i32, kind: i32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    let length = i32::try_from(10 + payload.len()).unwrap_or(i32::MAX);
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&[0, 0]);
    out
}

/// Encode a login: the password as the payload.
#[must_use]
pub fn encode_login(id: i32, password: &str) -> Vec<u8> {
    encode_packet(id, TYPE_LOGIN, password.as_bytes())
}

/// Encode command output as one or more `RESPONSE` packets.
///
/// Output longer than [`MAX_RESPONSE_BODY`] bytes is chunked on byte
/// boundaries (the payload is UTF-8 answers built from chat lines; a chunk
/// may split mid-character and the stock reassembly concatenates raw bytes
/// first, so splitting is safe). Empty output still sends one packet —
/// silence is an answer, not a hang.
#[must_use]
pub fn encode_response(id: i32, text: &str) -> Vec<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return vec![encode_packet(id, TYPE_RESPONSE, &[])];
    }
    bytes
        .chunks(MAX_RESPONSE_BODY)
        .map(|chunk| encode_packet(id, TYPE_RESPONSE, chunk))
        .collect()
}

/// Whether `candidate` is the configured password.
///
/// Fixed-time over the password length: no early-out on the first wrong
/// byte, so a timing probe learns nothing per byte. Different lengths
/// still differ in time (unavoidably — the length is one comparison), but
/// content never short-circuits.
#[must_use]
pub fn check_password(candidate: &[u8], password: &[u8]) -> bool {
    if candidate.len() != password.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in candidate.iter().zip(password.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Backoff after `failures` consecutive bad logins.
#[must_use]
pub fn auth_delay(failures: u32) -> std::time::Duration {
    let shift = failures.min(5);
    std::time::Duration::from_millis(100 * (1 << shift))
}

/// Concurrent RCON sockets admitted across every client (AUDIT-19 A-05).
///
/// RCON is an admin surface, not a game port: a normal load is one operator
/// session plus the odd reconnect, so the socket budget is single-digit. The
/// cap is a budget, not a promise — the surplus is refused at `accept` instead
/// of becoming an unbounded task.
pub const LISTENER_MAX_CONNECTIONS: u32 = 8;

/// Most concurrent RCON sockets one address may hold (AUDIT-19 A-05).
///
/// Four matches an operator running a handful of admin tools from one host
/// while stopping one address from owning the whole global budget.
pub const LISTENER_MAX_CONNECTIONS_PER_IP: u32 = 4;

/// Time to regain one reconnect token after the per-address burst is spent.
///
/// Deliberately slower than the game listener's 250 ms: RCON clients have no
/// legitimate reconnect churn (a stock client opens one socket, runs its
/// commands and closes), so after the burst an address is throttled to one
/// socket per second instead of four.
pub const LISTENER_RECONNECT_REFILL_INTERVAL: Duration = Duration::from_secs(1);

/// Bad logins one address may accumulate before the listener refuses it.
///
/// The per-connection budget (`MAX_AUTH_FAILURES` in the listener) closes a
/// socket after five wrong passwords, but a reconnecting client starts from
/// zero — AUDIT-19 C19-M2. This budget is the shared one: twenty failures per
/// address per [`AUTH_BLOCK_WINDOW`], regardless of how many sockets they were
/// spread over. With the backoff schedule that is a few guesses per minute,
/// not fifty.
pub const AUTH_FAILURE_BUDGET: u32 = 20;

/// How long a spent [`AUTH_FAILURE_BUDGET`] keeps an address refused.
///
/// Measured from the first failure of the run, so a blocked address cannot
/// extend its own block by reconnecting; once the window elapses the count
/// resets and the address is served again.
pub const AUTH_BLOCK_WINDOW: Duration = Duration::from_secs(300);

/// Build the RCON listener's admission gate with the numbers above.
///
/// This is the game listener's [`ConnectionGate`], not a second limiter: same
/// global semaphore, same per-IP token bucket, same release-on-drop guard. The
/// constructor lives here because this crate owns RCON's rules and already
/// depends on `mc-network`, while the binary owns only the sockets.
#[must_use]
pub fn listener_gate() -> Arc<ConnectionGate> {
    Arc::new(ConnectionGate::new(
        LISTENER_MAX_CONNECTIONS,
        LISTENER_MAX_CONNECTIONS_PER_IP,
        LISTENER_RECONNECT_REFILL_INTERVAL,
    ))
}

/// Bad-login budget shared by every connection from one address (AUDIT-19
/// C19-M2).
///
/// A per-connection counter is not a throttle: the attacker reconnects and it
/// is back to zero. This one outlives connections — failures accumulate per
/// source address, and once `budget` of them land inside `window` the address
/// is refused at `accept` until the window that started at its first failure
/// has elapsed. Time is injected so tests are deterministic; production passes
/// [`Instant::now`].
#[derive(Debug)]
pub struct AuthBudget {
    budget: u32,
    window: Duration,
    failures: Mutex<HashMap<IpAddr, FailureWindow>>,
}

/// Failures counted for one address inside the current window.
#[derive(Debug, Clone, Copy)]
struct FailureWindow {
    /// When the current window started (the first failure of the run).
    first: Instant,
    /// Failures recorded since `first`.
    count: u32,
}

impl AuthBudget {
    /// A budget of `budget` failures (at least one) per `window`.
    #[must_use]
    pub fn new(budget: u32, window: Duration) -> Self {
        Self {
            budget: budget.max(1),
            window,
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Record one bad login from `ip`; returns the address's running count.
    pub fn record_failure(&self, ip: IpAddr, now: Instant) -> u32 {
        let mut failures = self.lock();
        let entry = failures.entry(ip).or_insert(FailureWindow {
            first: now,
            count: 0,
        });
        if now.saturating_duration_since(entry.first) >= self.window {
            entry.first = now;
            entry.count = 0;
        }
        entry.count += 1;
        let count = entry.count;
        // Bound map growth under address spraying, like the game gate does:
        // once the table is large, forget windows that have already expired.
        if failures.len() > 4096 {
            let window = self.window;
            failures.retain(|_, entry| now.saturating_duration_since(entry.first) < window);
        }
        count
    }

    /// Whether `ip` has spent its budget and is still inside the window.
    #[must_use]
    pub fn is_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        let mut failures = self.lock();
        let expired = failures
            .get(&ip)
            .is_some_and(|entry| now.saturating_duration_since(entry.first) >= self.window);
        if expired {
            failures.remove(&ip);
            return false;
        }
        failures
            .get(&ip)
            .is_some_and(|entry| entry.count >= self.budget)
    }

    /// Forget an address's failures (called after a successful login).
    pub fn clear(&self, ip: IpAddr) {
        self.lock().remove(&ip);
    }

    /// Addresses currently tracked (diagnostics/tests).
    #[must_use]
    pub fn tracked_ips(&self) -> usize {
        self.lock().len()
    }

    /// Lock the table, recovering from a poisoned mutex.
    ///
    /// The guarded state is a counter map, so a recovered lock is still
    /// consistent enough to decide an admission; a panic elsewhere must not
    /// turn every later login into a process-wide panic (same rule as
    /// [`ConnectionGate`]).
    fn lock(&self) -> MutexGuard<'_, HashMap<IpAddr, FailureWindow>> {
        self.failures.lock().unwrap_or_else(|poisoned| {
            tracing::error!("rcon auth budget mutex was poisoned; continuing with recovered state");
            poisoned.into_inner()
        })
    }
}

impl Default for AuthBudget {
    fn default() -> Self {
        Self::new(AUTH_FAILURE_BUDGET, AUTH_BLOCK_WINDOW)
    }
}

/// A command for the tick loop, with its way back.
pub struct RconRequest {
    /// Command text, without a leading slash.
    pub command: String,
    /// Where the joined reply lines go.
    pub reply: tokio::sync::oneshot::Sender<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        AUTH_BLOCK_WINDOW, AUTH_FAILURE_BUDGET, AUTH_FAILURE_ID, AuthBudget,
        LISTENER_MAX_CONNECTIONS, LISTENER_MAX_CONNECTIONS_PER_IP, MAX_PACKET_LEN,
        MAX_RESPONSE_BODY, TYPE_COMMAND, TYPE_LOGIN, TYPE_RESPONSE, auth_delay, check_password,
        decode_body, encode_login, encode_packet, encode_response, listener_gate,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn login_round_trips_with_padding() {
        let bytes = encode_login(7, "s3cret");
        let body = &bytes[4..];
        let packet = decode_body(body).expect("decodes");
        assert_eq!(packet.id, 7);
        assert_eq!(packet.kind, TYPE_LOGIN);
        assert_eq!(packet.payload, b"s3cret");
        assert_eq!(AUTH_FAILURE_ID, -1);
        const {
            assert!(MAX_PACKET_LEN >= 4096);
        }
    }

    #[test]
    fn hostile_bodies_are_refused() {
        // Truncated, missing padding, unknown type, empty.
        assert!(decode_body(&[]).is_err());
        assert!(decode_body(&[1, 2, 3]).is_err());
        let mut no_pad = vec![7, 0, 0, 0, 2, 0, 0, 0, b'x', b'y'];
        assert!(decode_body(&no_pad).is_err());
        no_pad.extend_from_slice(&[0, 0]);
        assert!(
            decode_body(&no_pad).is_ok(),
            "exactly two zeros is the padding, nothing more is required"
        );
        // Same length, padding replaced by payload: refused.
        let mut bad_pad = vec![7, 0, 0, 0, 2, 0, 0, 0, b'x', b'y', b'z', b'w'];
        assert!(decode_body(&bad_pad).is_err());
        bad_pad[10] = 0;
        bad_pad[11] = 0;
        assert!(decode_body(&bad_pad).is_ok());
        let mut bad_kind = vec![7, 0, 0, 0, 9, 0, 0, 0, 0, 0];
        assert!(decode_body(&bad_kind).is_err());
        bad_kind.clear();
        assert!(decode_body(&bad_kind).is_err());
    }

    #[test]
    fn long_output_chunks_and_reassembles() {
        let text = "x".repeat(MAX_RESPONSE_BODY * 2 + 17);
        let packets = encode_response(3, &text);
        assert_eq!(packets.len(), 3, "two full chunks and a tail");
        let mut joined = Vec::new();
        for bytes in &packets {
            let packet = decode_body(&bytes[4..]).expect("each chunk decodes");
            assert_eq!(packet.id, 3);
            assert_eq!(packet.kind, TYPE_RESPONSE);
            joined.extend_from_slice(&packet.payload);
        }
        assert_eq!(joined, text.as_bytes());
        // Silence is one empty packet, not zero packets.
        assert_eq!(encode_response(3, "").len(), 1);
    }

    #[test]
    fn passwords_compare_without_early_out() {
        assert!(check_password(b"s3cret", b"s3cret"));
        assert!(!check_password(b"s3creu", b"s3cret"), "last byte differs");
        assert!(!check_password(b"s3cre", b"s3cret"), "length differs");
        assert!(!check_password(b"x", b""));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(auth_delay(0), std::time::Duration::from_millis(100));
        assert_eq!(auth_delay(1), std::time::Duration::from_millis(200));
        assert_eq!(auth_delay(5), std::time::Duration::from_millis(3200));
        assert_eq!(auth_delay(99), std::time::Duration::from_millis(3200));
    }

    #[test]
    fn command_packet_shape_matches_login() {
        // `TYPE_COMMAND` reuses the login framing; a wrong-kind body with
        // the right length still fails closed.
        let bytes = encode_packet(11, TYPE_COMMAND, b"list");
        let packet = decode_body(&bytes[4..]).expect("decodes");
        assert_eq!((packet.id, packet.kind), (11, TYPE_COMMAND));
        assert_eq!(packet.payload, b"list");
    }

    #[test]
    fn bad_logins_accumulate_per_address_across_connections() {
        // AUDIT-19 C19-M2: the budget must be a property of the *address*,
        // not of a connection, so N failures spread over N "connections" trip
        // it exactly like N failures on one.
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let other = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
        let budget = AuthBudget::new(3, Duration::from_secs(60));
        let start = Instant::now();
        assert_eq!(budget.record_failure(ip, start), 1);
        // Another address keeps its own count from the same instant.
        assert_eq!(budget.record_failure(other, start), 1);
        assert!(!budget.is_blocked(ip, start));
        // A second connection does not reset the count.
        assert_eq!(budget.record_failure(ip, start + Duration::from_secs(1)), 2);
        assert!(!budget.is_blocked(ip, start + Duration::from_secs(1)));
        assert_eq!(budget.record_failure(ip, start + Duration::from_secs(2)), 3);
        assert!(budget.is_blocked(ip, start + Duration::from_secs(2)));
        // Other addresses keep their own budget.
        assert!(!budget.is_blocked(other, start));
        // A successful login forgets the address's failures.
        budget.clear(ip);
        assert!(!budget.is_blocked(ip, start + Duration::from_secs(3)));
        assert_eq!(budget.tracked_ips(), 1, "only the culprit was forgotten");
        // The block is a window, not a permanent ban.
        let late = start + Duration::from_secs(61);
        budget.record_failure(ip, late);
        budget.record_failure(ip, late);
        budget.record_failure(ip, late);
        assert!(budget.is_blocked(ip, late));
        assert!(
            !budget.is_blocked(ip, late + Duration::from_secs(60)),
            "an expired window releases the address"
        );
    }

    #[test]
    fn listener_gate_uses_the_documented_budgets() {
        let gate = listener_gate();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Instant::now();
        // Exactly the per-IP cap is admitted; the next one is refused.
        let held: Vec<_> = (0..LISTENER_MAX_CONNECTIONS_PER_IP)
            .map(|_| {
                Arc::clone(&gate)
                    .try_acquire(ip, now)
                    .expect("within the per-IP cap")
            })
            .collect();
        assert_eq!(gate.concurrent_for(ip), LISTENER_MAX_CONNECTIONS_PER_IP);
        assert_eq!(
            Arc::clone(&gate).try_acquire(ip, now).err(),
            Some(super::LimitError::PerIpConcurrent)
        );
        drop(held);
        assert_eq!(gate.concurrent_for(ip), 0);
        // The global budget is the documented one and never below the per-IP
        // one, or the per-IP cap would be unreachable.
        const { assert!(LISTENER_MAX_CONNECTIONS >= LISTENER_MAX_CONNECTIONS_PER_IP) };
        assert_eq!(AUTH_FAILURE_BUDGET, 20);
        assert_eq!(AUTH_BLOCK_WINDOW, Duration::from_secs(300));
    }
}
