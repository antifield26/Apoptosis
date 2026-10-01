//! Wire shape of `minecraft:hello` / `EncryptionRequest` against the jar.
//!
//! **Oracle: the 26.1.2 client-jar bytecode, not our decoder.** In
//! `net.minecraft.network.protocol.login.ClientboundHelloPacket` the private
//! `write(FriendlyByteBuf)` method emits
//! `writeUtf(serverId)` → `writeByteArray(publicKey)` →
//! `writeByteArray(challenge)` → `writeBoolean(shouldAuthenticate)`, and the
//! private `ClientboundHelloPacket(FriendlyByteBuf)` constructor (also called
//! from the `STREAM_CODEC` decoder) reads `readUtf(20)` → `readByteArray` →
//! `readByteArray` → `readBoolean`. The boolean is therefore the **last**
//! field on the wire, and the 4-argument constructor is
//! `(String, byte[], byte[], boolean)`.
//! (Read with `javap -p -c -classpath <client jar>
//! net.minecraft.network.protocol.login.ClientboundHelloPacket`; the
//! server side's only construction site,
//! `net.minecraft.server.network.ServerLoginPacketListenerImpl.handleHello`,
//! passes `true`.)
//!
//! A pure encode/decode round trip cannot catch a missing field here: our
//! decoder rejects trailing bytes, so dropping the fourth field keeps the
//! round trip green while every real client rejects the packet. These tests
//! assert the byte sequence and the hand-built body instead.

use mc_protocol::packets::Packet as _;
use mc_protocol::packets::login::EncryptionRequest;
use mc_protocol::wire::{PacketReader, PacketWriter};

/// The exact body the jar's `write` method would produce for the fixture
/// below: `writeUtf("")`, `writeByteArray([0x30,0x82,0x02,0x03])`,
/// `writeByteArray([0xDE,0xAD,0xBE,0xEF])`, `writeBoolean(true)`.
///
/// The fixture bytes are chosen so that the trailing boolean is the only
/// `0x01` byte in the body: a body missing it cannot be mistaken for a
/// `true` one, and neither can a body where it moved.
const JAR_ORDERED_BODY: [u8; 12] = [
    0x00, // serverId: VarInt length 0
    0x04, 0x30, 0x82, 0x02, 0x03, // publicKey: VarInt length 4 + DER stub
    0x04, 0xDE, 0xAD, 0xBE, 0xEF, // verifyToken: VarInt length 4
    0x01, // shouldAuthenticate: writeBoolean(true)
];

fn fixture(should_authenticate: bool) -> EncryptionRequest {
    EncryptionRequest {
        server_id: String::new(),
        public_key: vec![0x30, 0x82, 0x02, 0x03],
        verify_token: vec![0xDE, 0xAD, 0xBE, 0xEF],
        should_authenticate,
    }
}

/// The named pin for AUDIT-19 C19-H1: `shouldAuthenticate` is written
/// **last**, as `ClientboundHelloPacket.write` does. Deleting the
/// `write_bool` from `EncryptionRequest::encode` (or moving it before the
/// byte arrays) fails this test on the byte vector and on the final-byte
/// assertion below.
#[test]
fn encryption_request_wire_shape_puts_should_authenticate_last() {
    let body = fixture(true).encode().expect("encodes");
    assert_eq!(
        body, JAR_ORDERED_BODY,
        "body must match the jar's writeUtf → writeByteArray → writeByteArray → writeBoolean order"
    );
    assert_eq!(
        body.last(),
        Some(&0x01),
        "the jar reads/writes shouldAuthenticate as the final boolean"
    );

    // The other polarity is a wire value too: `false` also ends up last.
    let body = fixture(false).encode().expect("encodes");
    let mut expected = JAR_ORDERED_BODY;
    expected[11] = 0x00;
    assert_eq!(body, expected);
}

/// A hand-built body (not produced by `encode`) in the jar's field order
/// decodes into the four fields — the decoder reads the boolean from the
/// trailing byte rather than assuming it.
#[test]
fn encryption_request_decodes_a_hand_built_jar_ordered_body() {
    let decoded = EncryptionRequest::decode(&JAR_ORDERED_BODY).expect("decodes");
    assert_eq!(decoded, fixture(true));
    assert!(decoded.should_authenticate);

    let mut false_body = JAR_ORDERED_BODY;
    false_body[11] = 0x00;
    let decoded = EncryptionRequest::decode(&false_body).expect("decodes");
    assert_eq!(decoded, fixture(false));
    assert!(!decoded.should_authenticate);

    // And the same body built through the writer, for the string/blob
    // encodings, still ends with the boolean.
    let mut writer = PacketWriter::new();
    writer.write_string("").expect("writes");
    writer.write_varint(4);
    writer.write_bytes(&[0x30, 0x82, 0x02, 0x03]);
    writer.write_varint(4);
    writer.write_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]);
    writer.write_bool(true);
    assert_eq!(writer.finish(), JAR_ORDERED_BODY);
}

/// The trailing-byte refusal stays: a body that carries a fourth field our
/// decoder does not read is rejected rather than silently truncated (this is
/// the assertion that made the old three-field encoder look self-consistent,
/// so it is pinned deliberately).
#[test]
fn encryption_request_refuses_a_body_without_the_boolean() {
    let three_field = &JAR_ORDERED_BODY[..11];
    assert!(
        EncryptionRequest::decode(three_field).is_err(),
        "a 3-field body (no shouldAuthenticate) must not decode"
    );

    let mut with_trailing = JAR_ORDERED_BODY.to_vec();
    with_trailing.push(0x00);
    assert!(
        EncryptionRequest::decode(&with_trailing).is_err(),
        "bytes after the boolean are still refused"
    );
}

/// Guard the fixture itself: the trailing boolean is the only `0x01` byte and
/// the blob lengths agree with the reader's expectations, so the byte
/// assertions above cannot pass by accident.
#[test]
fn jar_ordered_body_fixture_is_well_formed() {
    let mut reader = PacketReader::new(&JAR_ORDERED_BODY);
    assert_eq!(reader.read_string(20).expect("serverId"), "");
    assert_eq!(reader.read_varint().expect("key len"), 4);
    assert_eq!(
        reader.read_bytes(4).expect("key"),
        &[0x30, 0x82, 0x02, 0x03]
    );
    assert_eq!(reader.read_varint().expect("token len"), 4);
    assert_eq!(
        reader.read_bytes(4).expect("token"),
        &[0xDE, 0xAD, 0xBE, 0xEF]
    );
    assert!(reader.read_bool().expect("flag"));
    assert!(reader.is_empty());
    assert_eq!(
        JAR_ORDERED_BODY.iter().position(|b| *b == 0x01),
        Some(11),
        "the trailing boolean is the only 0x01 byte in this fixture"
    );
}
