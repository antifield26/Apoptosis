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
//! check (fixed-time comparison, never early-out), and the request channel
//! into the tick loop. What it does not own: sockets and auth throttle
//! (the binary's listener, `apps/server/src/rcon.rs`), and the world
//! itself (commands run through `Game::dispatch_console`).

use mc_core::error::{ServerError, ServerResult};

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
        AUTH_FAILURE_ID, MAX_PACKET_LEN, MAX_RESPONSE_BODY, TYPE_COMMAND, TYPE_LOGIN,
        TYPE_RESPONSE, auth_delay, check_password, decode_body, encode_login, encode_packet,
        encode_response,
    };

    #[test]
    fn login_round_trips_with_padding() {
        let bytes = encode_login(7, "s3cret");
        let body = &bytes[4..];
        let packet = decode_body(body).expect("decodes");
        assert_eq!(packet.id, 7);
        assert_eq!(packet.kind, TYPE_LOGIN);
        assert_eq!(packet.payload, b"s3cret");
        assert_eq!(AUTH_FAILURE_ID, -1);
        assert!(MAX_PACKET_LEN >= 4096);
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
}
