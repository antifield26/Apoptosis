//! Trailing-byte behaviour of the configuration decoders that really are reached
//! from the socket (AUDIT-19 C M-5).
//!
//! The module header of `mc_protocol::packets::config` used to claim these
//! decoders "run in tests and capture tools, never on hostile socket bytes",
//! which is false: `connection.rs` decodes `select_known_packs`,
//! `client_information`, `finish_configuration` and `keep_alive` straight off the
//! wire. The rule is now per-decoder, and both halves of it are pinned here —
//! a decision that is not pinned is the accident this audit keeps finding.

use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{ConfigKeepAlive, ConfigPong, SelectKnownPacks};

/// A fixed-width body has no unmodelled field a real client could send, so a
/// tail is malformed input.
#[test]
fn config_keepalive_refuses_a_trailing_byte() {
    let body = ConfigKeepAlive { id: 7 }.encode().expect("encodes");
    assert!(
        ConfigKeepAlive::decode(&body).is_ok(),
        "the clean body decodes"
    );
    let mut bad = body;
    bad.push(0x00);
    assert!(
        ConfigKeepAlive::decode(&bad).is_err(),
        "one trailing byte must be refused, not tolerated"
    );
}

/// The same rule for the other fixed-width configuration packet.
#[test]
fn config_pong_refuses_a_trailing_byte() {
    let body = ConfigPong { id: 3 }.encode().expect("encodes");
    assert!(ConfigPong::decode(&body).is_ok(), "the clean body decodes");
    let mut bad = body;
    bad.push(0x00);
    assert!(
        ConfigPong::decode(&bad).is_err(),
        "one trailing byte must be refused, not tolerated"
    );
}

/// The variable body keeps its tolerance — the capture is the only evidence for
/// its shape, and refusing an unmodelled field could lock a real client out of
/// configuration. Pinned so the tolerance is a decision with a test rather than
/// a silence.
#[test]
fn select_known_packs_tolerates_a_tail_it_does_not_model() {
    // count = 0, then one byte this build does not model.
    assert!(
        SelectKnownPacks::decode(&[0x00, 0xab]).is_ok(),
        "the tolerated tail must still decode — see the module header for why"
    );
    // And a clean body round-trips, so the tolerance is not hiding a broken read.
    let clean = SelectKnownPacks { packs: Vec::new() }
        .encode()
        .expect("encodes");
    assert_eq!(clean, vec![0x00], "an empty pack list is one varint");
    assert!(SelectKnownPacks::decode(&clean).is_ok());
}
