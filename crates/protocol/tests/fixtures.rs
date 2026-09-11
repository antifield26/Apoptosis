//! Golden/fixture tests for the protocol codec (P02-11).
//!
//! Fixtures live in `crates/test-support/fixtures/protocol/*.hex` and are
//! checked in as hex text. They pin wire bytes that were hand-assembled from
//! the specification, independent of the encoder under test.

use mc_protocol::RawPacket;
use mc_protocol::framing::FrameCodec;
use mc_protocol::nbt::Nbt;
use mc_protocol::packets::Packet;
use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
use mc_protocol::text::TextComponent;
use mc_test_support::fixtures::{assert_bytes_eq, read_hex_fixture};

#[test]
fn handshake_login_matches_golden_bytes() {
    let golden = read_hex_fixture("protocol", "handshake_login.hex").expect("fixture");
    let decoded = Handshake::decode(&golden).expect("decodes golden");
    assert_eq!(decoded.protocol_version, 775);
    assert_eq!(decoded.server_address, "localhost");
    assert_eq!(decoded.server_port, 25565);
    assert_eq!(decoded.intent, HandshakeIntent::Login);

    let re_encoded = decoded.encode().expect("encodes");
    assert_bytes_eq(&re_encoded, &golden);
}

#[test]
fn uncompressed_frame_matches_golden_bytes() {
    let golden = read_hex_fixture("protocol", "frame_uncompressed.hex").expect("fixture");
    let mut codec = FrameCodec::new();
    codec.feed(&golden).expect("feeds");
    let packet = codec.try_next().expect("decodes").expect("one packet");
    assert_eq!(packet, RawPacket::new(0x2A, vec![0x01, 0x02]));
    assert_eq!(codec.try_next().expect("drained"), None);

    let re_encoded = FrameCodec::encode(&packet, None).expect("encodes");
    assert_bytes_eq(&re_encoded, &golden);
}

#[test]
fn network_nbt_literal_text_matches_golden_bytes() {
    let golden = read_hex_fixture("protocol", "nbt_literal_text.hex").expect("fixture");
    let mut slice = &golden[..];
    let decoded = Nbt::read_network(&mut slice).expect("decodes golden");
    assert!(slice.is_empty(), "golden must be fully consumed");

    let mut re_encoded = Vec::new();
    decoded.write_network(&mut re_encoded).expect("encodes");
    assert_bytes_eq(&re_encoded, &golden);

    assert_eq!(decoded, TextComponent::literal("bye").to_nbt());
}

#[test]
fn decode_reencode_is_stable_for_packet_corpus() {
    // A small corpus exercising every integer width and list bounds; decoding
    // then re-encoding must be byte-stable.
    let packets: Vec<RawPacket> = vec![
        RawPacket::new(0, vec![0x00]),
        RawPacket::new(1, vec![0xFF; 5]),
        RawPacket::new(127, (0..=127).collect()),
        RawPacket::new(128, vec![0x80, 0x01, 0x7F]),
        RawPacket::new(300, vec![]),
    ];
    for packet in packets {
        let bytes = FrameCodec::encode(&packet, None).expect("encode");
        let mut codec = FrameCodec::new();
        codec.feed(&bytes).expect("feed");
        let decoded = codec.try_next().expect("decode").expect("packet");
        assert_eq!(decoded, packet, "round trip must preserve the packet");
    }
}
