//! `disguised_chat`, checked against a payload a real 26.1.2 server sent.
//!
//! ```text
//! 08 00 03 62 79 65 | 05 | 08 00 06 53 65 72 76 65 72 | 00
//!    6 bytes          1        9 bytes                    1   = 17
//! ```
//!
//! **Seventeen bytes in, four fields out**, every byte accounted for.
//!
//! # The divergence this test found, and its fix
//!
//! The decode was always exact; the **encode used not to be byte-identical**. A real server sends a plain
//! message as a bare `TAG_String` (`08 00 03 'b','y','e'`), while `TextComponent::to_nbt` wrote
//! `TAG_Compound { text: ... }` (`10 08 00 04 't','e','x','t' 00 ...`). A client reads both as the same component
//! in *older* protocols, but the 26.1.2 client's component decoder **rejects the compound form on the wire**:
//! a real client that joined, received our `disguised_chat` welcome, and failed with
//! `DecoderException: Failed to decode packet 'clientbound/minecraft:disguised_chat'` — which is how the
//! encoder finally got fixed to the bare-string form, after the divergence had been asserted here first.

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
fn the_encoder_is_now_byte_identical_to_the_server() {
    // The divergence the earlier version of this suite documented has been fixed:
    // a plain literal encodes as a bare TAG_String, exactly what the server sent,
    // so a re-encode of the captured packet is byte-identical.
    let packet = DisguisedChat::decode(&CAPTURED).expect("decodes");
    let ours = packet.encode().expect("encodes");
    assert_eq!(
        ours, CAPTURED,
        "the encoder must stay byte-identical to the server's shape"
    );
}
