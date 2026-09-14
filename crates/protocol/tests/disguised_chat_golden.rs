//! `disguised_chat`, checked against a payload a real 26.1.2 server sent.
//!
//! ```text
//! 08 00 03 62 79 65 | 05 | 08 00 06 53 65 72 76 65 72 | 00
//!    6 bytes          1        9 bytes                    1   = 17
//! ```
//!
//! **Seventeen bytes in, four fields out**, every byte accounted for.
//!
//! # A divergence this test found and records rather than hides
//!
//! The decode is exact; the **encode is not byte-identical**. A real server sends a plain message as a bare
//! `TAG_String`, where `TextComponent::to_nbt` writes `TAG_Compound { text: ... }`:
//!
//! ```text
//! ours:  10 08 00 04 't','e','x','t' 00 03 'b','y','e' 00
//! real:  08 00 03 'b','y','e'
//! ```
//!
//! A client reads both as the same component, so this is a divergence in **what is sent** rather than in what is
//! understood — and this project's standard for a wire artefact is byte-identical, so it is written down here. The
//! test asserts the divergence **exists**, so that fixing the encoder has to fail this test and remove the note
//! rather than quietly leaving a stale comment behind.

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::DisguisedChat;

const CAPTURED: [u8; 17] = [
    0x08, 0x00, 0x03, 0x62, 0x79, 0x65, 0x05, 0x08, 0x00, 0x06, 0x53, 0x65, 0x72, 0x76, 0x65, 0x72,
    0x00,
];

#[test]
fn a_real_disguised_chat_decodes_field_by_field() {
    let packet = DisguisedChat::decode(&CAPTURED).expect("decodes");
    assert_eq!(
        packet.chat_type, 5,
        "the chat type is the byte after the message"
    );
    assert_eq!(
        packet.target_name, None,
        "0x00 is an absent tag, and reading it consumes it"
    );
}

#[test]
fn the_terminator_is_consumed_and_not_left_for_the_trailing_check() {
    // The defect this test exists for: recognising `0x00` without consuming it left one byte behind, and the
    // decoder then rejected a payload that was correct.
    assert!(
        DisguisedChat::decode(&CAPTURED[..CAPTURED.len() - 1]).is_err(),
        "without its terminator the packet is truncated, and must not read as 'no target name'"
    );
}

#[test]
fn every_prefix_of_the_captured_body_is_refused() {
    for cut in 0..CAPTURED.len() {
        assert!(
            DisguisedChat::decode(&CAPTURED[..cut]).is_err(),
            "a {cut}-byte prefix decoded, so the decoder is padding what the wire did not send"
        );
    }
}

#[test]
fn the_encoder_writes_a_compound_where_the_server_wrote_a_bare_string() {
    // **Asserting the divergence, not accepting it.** If the encoder is ever made byte-identical, this fails and
    // the note above has to go with it -- which is the outcome the assertion is for.
    let packet = DisguisedChat::decode(&CAPTURED).expect("decodes");
    let ours = packet.encode().expect("encodes");
    assert_ne!(
        ours, CAPTURED,
        "the encoder is now byte-identical to the server: delete this test and the note it documents"
    );
    assert_eq!(
        ours.first().copied(),
        Some(0x0A),
        "ours opens a TAG_Compound where the server sent a bare string"
    );
    let again = DisguisedChat::decode(&ours).expect("our own form decodes");
    assert_eq!(again.chat_type, packet.chat_type);
    assert_eq!(again.target_name, packet.target_name);
}
