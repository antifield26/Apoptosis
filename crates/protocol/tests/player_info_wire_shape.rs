//! Wire shape of `minecraft:player_info_update` / `player_info_remove`
//! (AUDIT-19 A-04), against the jar **and** against real captured bytes.
//!
//! **Oracle 1: the 26.1.2 client jar's bytecode.**
//! `javap -p -c -classpath target/vanilla-26.1.2/client.jar
//! net.minecraft.network.protocol.game.ClientboundPlayerInfoUpdatePacket`
//! (and its `$Entry`, `$Action`, `$EntryBuilder` types) shows:
//!
//! ```text
//! private void write(RegistryFriendlyByteBuf);          // the packet
//!   1: getfield actions     → writeEnumSet(actions, Action.class)
//!  15: getfield entries     → writeCollection(entries, this::lambda$write$0)
//!
//! public <E extends Enum<E>> void writeEnumSet(EnumSet<E>, Class<E>);  // FriendlyByteBuf
//!   8: new BitSet(values.length)
//!  38: EnumSet.contains(values[i]) → BitSet.set(i, …)     // ordinal order
//!  55: writeFixedBitSet(bits, values.length)
//!
//! public void writeFixedBitSet(BitSet, int);
//!  26: BitSet.toByteArray() → Arrays.copyOf(bytes, positiveCeilDiv(len, 8)) → writeBytes
//!
//! private void lambda$write$0(FriendlyByteBuf, Entry);
//!   2: Entry.profileId() → writeUUID      // two big-endian longs, most first
//!  13: actions.iterator() → action.writer.write(buf, entry)   // ordinal order
//!
//! private static void lambda$static$1(FriendlyByteBuf, Entry);   // ADD_PLAYER's writer
//!  11: ByteBufCodecs.PLAYER_NAME.encode(buf, profile.name())
//!  24: ByteBufCodecs.GAME_PROFILE_PROPERTIES.encode(buf, profile.properties())
//!
//! ByteBufCodecs$32 (GAME_PROFILE_PROPERTIES).encode(ByteBuf, PropertyMap);
//!   2: PropertyMap.size() → writeCount(buf, size, 16)      // Vanilla's cap: 16
//!  40: name    → Utf8String.write(buf, name, 64)
//!  51: value   → Utf8String.write(buf, value, 32767)
//!  63: signature → FriendlyByteBuf.writeNullable(buf, signature, …(1024))
//! ```
//!
//! and the `Action` static initialiser fixes the ordinals the bitset indexes:
//! `ADD_PLAYER` 0, `INITIALIZE_CHAT` 1, `UPDATE_GAME_MODE` 2, `UPDATE_LISTED` 3,
//! `UPDATE_LATENCY` 4, `UPDATE_DISPLAY_NAME` 5, `UPDATE_LIST_ORDER` 6,
//! `UPDATE_HAT` 7 — **not** the 1.20.x order. `EntryBuilder`'s constructor
//! initialises `gameMode` (`GameType.DEFAULT_MODE`) and nothing else, so
//! `listed` and `showHat` start **false**: a client that is never sent
//! `UPDATE_LISTED` creates the entry unlisted (an invisible tab entry) and one
//! never sent `UPDATE_HAT` hides the hat layer.
//!
//! **Oracle 2: three real captured server bodies** under
//! `target/vanilla-capture/bodies/` (the corpus `capture_sweep.rs` sweeps; the
//! directory is not committed, so the bodies are inlined here as constants with
//! their file names). They are the strongest pin available for the bitset, the
//! count, the UUID and the `ADD_PLAYER` fields: one is a complete vanilla
//! `createPlayerInitializing` packet with **all eight** actions set.
//!
//! A round trip alone cannot catch a wrong *order* here (our decoder follows the
//! bitset, so it agrees with any order our encoder chose); the byte-equality
//! assertions below are what pin the order, and `capture_sweep` re-checks the same
//! bodies from the decode side.

use mc_protocol::packets::Packet;
use mc_protocol::packets::login::ProfileProperty;
use mc_protocol::packets::play::{
    MAX_PROFILE_PROPERTIES, PlayerInfoAction, PlayerInfoActions, PlayerInfoEntry, PlayerInfoField,
    PlayerInfoProfile, PlayerInfoRemove, PlayerInfoUpdate,
};

/// `000054_s2c_play_70.bin` — `ff 00`: every action, **no** entries.
///
/// Vanilla sends this shape when a player joins an empty server
/// (`createPlayerInitializing` over an empty collection never happens — it is the
/// broadcast of one player to nobody that a capture still records as a body), and
/// it is the minimal proof that the bitset is a whole byte and that the count is a
/// `VarInt` after it.
const CAPTURED_ALL_ACTIONS_NO_ENTRIES: [u8; 2] = [0xFF, 0x00];

/// `000055_s2c_play_70.bin` — one entry, all eight actions:
/// `ff` (bitset) `01` (count) `53c15a8b15403cc3bfd2d04d748a0a86` (uuid)
/// `0a` `"RealClient"` (`ADD_PLAYER` name) `00` (0 properties)
/// `00` (`INITIALIZE_CHAT` null) `01` (`UPDATE_GAME_MODE`: creative)
/// `01` (`UPDATE_LISTED`: true) `00` (`UPDATE_LATENCY`) `00`
/// (`UPDATE_DISPLAY_NAME` null) `00` (`UPDATE_LIST_ORDER`) `01` (`UPDATE_HAT`).
///
/// Field by field, that the action slots are written in **ordinal** order.
const CAPTURED_ALL_ACTIONS_ONE_ENTRY: [u8; 37] = [
    0xFF, 0x01, // bitset: all eight actions; entry count
    0x53, 0xC1, 0x5A, 0x8B, 0x15, 0x40, 0x3C, 0xC3, 0xBF, 0xD2, 0xD0, 0x4D, 0x74, 0x8A, 0x0A,
    0x86, // profileId
    0x0A, b'R', b'e', b'a', b'l', b'C', b'l', b'i', b'e', b'n', b't', // ADD_PLAYER: name
    0x00, // ADD_PLAYER: 0 properties
    0x00, // INITIALIZE_CHAT: null
    0x01, // UPDATE_GAME_MODE: 1
    0x01, // UPDATE_LISTED: true
    0x00, // UPDATE_LATENCY: 0
    0x00, // UPDATE_DISPLAY_NAME: null
    0x00, // UPDATE_LIST_ORDER: 0
    0x01, // UPDATE_HAT: true
];

/// `004934_s2c_play_70.bin` — one entry, `UPDATE_LATENCY` only: `10` (bit 4)
/// `01` (count) uuid `00` (latency 0).
///
/// This is the body that identifies the bit assignment without ambiguity: bit 4
/// alone is `0x10`, which is `UPDATE_LATENCY` only under the jar's ordinal table.
const CAPTURED_LATENCY_ONLY: [u8; 19] = [
    0x10, 0x01, // bitset: UPDATE_LATENCY (bit 4); entry count
    0x53, 0xC1, 0x5A, 0x8B, 0x15, 0x40, 0x3C, 0xC3, 0xBF, 0xD2, 0xD0, 0x4D, 0x74, 0x8A, 0x0A,
    0x86, // profileId
    0x00, // UPDATE_LATENCY: 0
];

/// The UUID both `000055` and `004934` carry.
fn captured_uuid() -> uuid::Uuid {
    uuid::Uuid::from_bytes([
        0x53, 0xC1, 0x5A, 0x8B, 0x15, 0x40, 0x3C, 0xC3, 0xBF, 0xD2, 0xD0, 0x4D, 0x74, 0x8A, 0x0A,
        0x86,
    ])
}

/// The body this server sends for one joining player, built by hand from the
/// jar's field order: `java -jar …` is not available, so every byte is written
/// out and the encoder is asserted against it.
///
/// Actions `ADD_PLAYER | UPDATE_GAME_MODE | UPDATE_LISTED | UPDATE_HAT` =
/// bits 0, 2, 3, 7 = `0b1000_1101` = `0x8D`.
///
/// Fields: `01` (count) then, for
/// `uuid 00112233-4455-6677-8899-aabbccddeeff`,
/// `07 "Skinner" 01` (name + one property)
/// `08 "textures" 08 "c2tpbg==" 01 03 "sig"` (name/value/signed value)
/// `01` (`UPDATE_GAME_MODE`: creative) `01` (`UPDATE_LISTED`: true)
/// `01` (`UPDATE_HAT`: true).
const JAR_ORDERED_SEND_BODY: [u8; 53] = [
    0x8D, 0x01, // bitset; entry count
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
    0xFF, // profileId
    0x07, b'S', b'k', b'i', b'n', b'n', b'e', b'r', // ADD_PLAYER: name
    0x01, // ADD_PLAYER: one property
    0x08, b't', b'e', b'x', b't', b'u', b'r', b'e', b's', // property name
    0x08, b'c', b'2', b't', b'p', b'b', b'g', b'=', b'=', // property value
    0x01, 0x03, b's', b'i', b'g', // property signature (writeNullable: present)
    0x01, // UPDATE_GAME_MODE: creative
    0x01, // UPDATE_LISTED: true
    0x01, // UPDATE_HAT: true
];

/// The fixture behind [`JAR_ORDERED_SEND_BODY`] — the entry a joining player is
/// announced with.
fn send_fixture() -> PlayerInfoUpdate {
    PlayerInfoUpdate {
        actions: PlayerInfoActions::from(PlayerInfoAction::AddPlayer)
            | PlayerInfoActions::from(PlayerInfoAction::UpdateGameMode)
            | PlayerInfoActions::from(PlayerInfoAction::UpdateListed)
            | PlayerInfoActions::from(PlayerInfoAction::UpdateHat),
        entries: vec![PlayerInfoEntry {
            uuid: uuid::Uuid::from_u128(0x0011_2233_4455_6677_8899_AABB_CCDD_EEFF),
            profile: PlayerInfoField::Value(PlayerInfoProfile {
                name: "Skinner".to_owned(),
                properties: vec![ProfileProperty {
                    name: "textures".to_owned(),
                    value: "c2tpbg==".to_owned(),
                    signature: Some("sig".to_owned()),
                }],
            }),
            game_mode: PlayerInfoField::Value(1),
            listed: PlayerInfoField::Value(true),
            show_hat: PlayerInfoField::Value(true),
            ..PlayerInfoEntry::default()
        }],
    }
}

/// The named pin for the field order this server's join announcement depends on:
/// bitset, count, UUID, then the action fields in **ordinal** order with
/// `ADD_PLAYER`'s name/properties first. Moving any field (or setting the actions
/// from a 1.20.x-shaped table) fails on the byte vector.
#[test]
fn the_send_shape_matches_the_jars_field_order_byte_for_byte() {
    let body = send_fixture().encode().expect("encodes");
    assert_eq!(
        body, JAR_ORDERED_SEND_BODY,
        "the body must be bitset → count → uuid → action fields in ordinal order"
    );
    assert_eq!(body.len(), JAR_ORDERED_SEND_BODY.len());
}

/// The same body, hand-built, decodes back into the fixture: the decoder reads
/// each slot at the position the bitset says, not at a fixed offset.
#[test]
fn the_send_shape_decodes_from_the_hand_built_jar_ordered_body() {
    let decoded = PlayerInfoUpdate::decode(&JAR_ORDERED_SEND_BODY).expect("decodes");
    assert_eq!(decoded, send_fixture());
    // And the actions bits *are* the ordinals: 0x8D = 0b1000_1101.
    assert_eq!(
        decoded.actions.bits(),
        0b1000_1101,
        "bits 0 (ADD_PLAYER), 2 (UPDATE_GAME_MODE), 3 (UPDATE_LISTED), 7 (UPDATE_HAT)"
    );
    assert!(!decoded.actions.contains(PlayerInfoAction::UpdateLatency));
}

/// Oracle 2, decode side: a real vanilla `createPlayerInitializing` body, field
/// by field. This is what fixes the *meaning* of every slot (a round trip cannot:
/// our encoder and decoder would agree on a wrong order too).
#[test]
fn a_captured_vanilla_body_decodes_field_by_field() {
    let decoded = PlayerInfoUpdate::decode(&CAPTURED_ALL_ACTIONS_ONE_ENTRY).expect("decodes");
    assert_eq!(decoded.actions.bits(), 0xFF, "all eight actions");
    assert_eq!(decoded.entries.len(), 1);
    let entry = &decoded.entries[0];
    assert_eq!(entry.uuid, captured_uuid());
    assert_eq!(
        entry.profile,
        PlayerInfoField::Value(PlayerInfoProfile {
            name: "RealClient".to_owned(),
            properties: Vec::new(),
        }),
        "ADD_PLAYER writes the name and then a writeCount-capped property list"
    );
    assert_eq!(entry.chat_session, PlayerInfoField::Null);
    assert_eq!(
        entry.game_mode,
        PlayerInfoField::Value(1),
        "UPDATE_GAME_MODE is a VarInt: the byte after the null chat session"
    );
    assert_eq!(entry.listed, PlayerInfoField::Value(true));
    assert_eq!(entry.latency, PlayerInfoField::Value(0));
    assert_eq!(entry.display_name, PlayerInfoField::Null);
    assert_eq!(entry.list_order, PlayerInfoField::Value(0));
    assert_eq!(entry.show_hat, PlayerInfoField::Value(true));
}

/// Oracle 2, encode side: re-encoding the captured body reproduces it exactly, so
/// the encoder speaks the same shape the real server did — bitset, count, UUID and
/// all eight action slots.
#[test]
fn captured_vanilla_bodies_re_encode_byte_for_byte() {
    for (name, body) in [
        (
            "000054_s2c_play_70.bin",
            &CAPTURED_ALL_ACTIONS_NO_ENTRIES[..],
        ),
        (
            "000055_s2c_play_70.bin",
            &CAPTURED_ALL_ACTIONS_ONE_ENTRY[..],
        ),
        ("004934_s2c_play_70.bin", &CAPTURED_LATENCY_ONLY[..]),
    ] {
        let decoded = PlayerInfoUpdate::decode(body).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            decoded.encode().expect("encodes"),
            body,
            "{name} must round-trip byte for byte"
        );
    }
}

/// The single-action capture: bit 4 alone is `UPDATE_LATENCY`, which is only true
/// under the jar's ordinal order (`ADD_PLAYER` 0 … `UPDATE_HAT` 7).
#[test]
fn the_captured_latency_only_body_names_bit_four_as_update_latency() {
    let decoded = PlayerInfoUpdate::decode(&CAPTURED_LATENCY_ONLY).expect("decodes");
    assert_eq!(
        decoded.actions.bits(),
        PlayerInfoAction::UpdateLatency.mask()
    );
    assert!(decoded.actions.contains(PlayerInfoAction::UpdateLatency));
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].latency, PlayerInfoField::Value(0));
    assert_eq!(decoded.entries[0].profile, PlayerInfoField::Absent);

    // An empty entry list is a legal body and stays two bytes: the count is a
    // VarInt right after the bitset.
    let empty = PlayerInfoUpdate::decode(&CAPTURED_ALL_ACTIONS_NO_ENTRIES).expect("decodes");
    assert!(empty.entries.is_empty());
    assert_eq!(empty.actions.bits(), 0xFF);
    assert_eq!(
        empty.encode().expect("encodes"),
        CAPTURED_ALL_ACTIONS_NO_ENTRIES
    );
}

/// The bitset is **one** byte for eight actions (`positiveCeilDiv(8, 8)`), so a
/// body with a longer header is not this packet — and trailing bytes are refused
/// rather than silently ignored.
#[test]
fn a_body_that_is_not_exactly_this_packet_is_refused() {
    let mut trailing = CAPTURED_ALL_ACTIONS_NO_ENTRIES.to_vec();
    trailing.push(0x00);
    assert!(
        PlayerInfoUpdate::decode(&trailing).is_err(),
        "a byte after the last entry must be refused"
    );

    // Bitset says ADD_PLAYER but the entry stops after its uuid.
    let truncated = [
        0x01, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC,
        0xDD, 0xEE, 0xFF,
    ];
    assert!(
        PlayerInfoUpdate::decode(&truncated).is_err(),
        "a body shorter than its own bitset must be refused"
    );

    // A count with no entries behind it.
    assert!(PlayerInfoUpdate::decode(&[0x01, 0x02]).is_err());
    assert!(PlayerInfoUpdate::decode(&[]).is_err());
}

/// A bitset and an entry that disagree are our own bug and are refused at encode
/// time: emitting the packet anyway would produce a body shorter than its own
/// bitset, which is what a real client rejects (worse than a smaller packet it
/// accepts).
#[test]
fn an_action_without_its_value_is_refused_at_encode_time() {
    let mut update = send_fixture();
    update.entries[0].profile = PlayerInfoField::Absent;
    assert!(
        update.encode().is_err(),
        "ADD_PLAYER in the bitset with an Absent profile must not encode"
    );

    // `Null` is a wire state for the two `writeNullable` actions only.
    let mut update = send_fixture();
    update.entries[0].game_mode = PlayerInfoField::Null;
    assert!(
        update.encode().is_err(),
        "UPDATE_GAME_MODE is not nullable, so Null is not a wire state"
    );

    // A null chat session and a null display name *are* wire states (that is how
    // the captured all-actions body carries them).
    let mut update = send_fixture();
    update.actions.insert(PlayerInfoAction::InitializeChat);
    update.actions.insert(PlayerInfoAction::UpdateDisplayName);
    update.entries[0].chat_session = PlayerInfoField::Null;
    update.entries[0].display_name = PlayerInfoField::Null;
    let body = update.encode().expect("nulls encode");
    let decoded = PlayerInfoUpdate::decode(&body).expect("decodes");
    assert_eq!(decoded.entries[0].chat_session, PlayerInfoField::Null);
    assert_eq!(decoded.entries[0].display_name, PlayerInfoField::Null);
}

/// Vanilla's own cap: `GAME_PROFILE_PROPERTIES` is written with
/// `writeCount(size, 16)`, which throws above 16 — so 17 properties are a packet
/// no client would read, and this encoder refuses them instead of emitting it.
#[test]
fn more_profile_properties_than_vanilla_allows_are_refused() {
    let mut update = send_fixture();
    let property = ProfileProperty {
        name: "textures".to_owned(),
        value: "x".to_owned(),
        signature: None,
    };
    let replace_profile = |update: &mut PlayerInfoUpdate, count: usize| {
        let PlayerInfoField::Value(profile) = &mut update.entries[0].profile else {
            panic!("fixture carries a profile");
        };
        profile.properties = vec![property.clone(); count];
    };

    replace_profile(&mut update, MAX_PROFILE_PROPERTIES);
    assert!(update.encode().is_ok(), "16 properties still encode");

    replace_profile(&mut update, MAX_PROFILE_PROPERTIES + 1);
    assert!(
        update.encode().is_err(),
        "17 properties must be refused: the client's readCount(16) would throw"
    );
}

/// `player_info_remove` (id 69) is `writeCollection(profileIds)`: a `VarInt` count
/// and 16 bytes per id, nothing else — the other half of the tab list.
#[test]
fn a_removal_is_a_counted_list_of_uuids() {
    let packet = PlayerInfoRemove {
        profile_ids: vec![captured_uuid()],
    };
    let body = packet.encode().expect("encodes");
    let mut expected = vec![0x01];
    expected.extend_from_slice(captured_uuid().as_bytes());
    assert_eq!(body, expected);
    assert_eq!(PlayerInfoRemove::decode(&body).expect("decodes"), packet);

    let empty = PlayerInfoRemove {
        profile_ids: Vec::new(),
    };
    assert_eq!(empty.encode().expect("encodes"), [0x00]);
    assert!(PlayerInfoRemove::decode(&[0x00, 0x00]).is_err());
}

/// Guard the fixtures themselves: the captured lengths agree with the layout the
/// comments claim, and the hand-built body's only `0x8D` is its bitset, so none of
/// the byte assertions above can pass by accident.
#[test]
fn the_fixtures_are_well_formed() {
    assert_eq!(CAPTURED_ALL_ACTIONS_ONE_ENTRY.len(), 37);
    assert_eq!(CAPTURED_LATENCY_ONLY.len(), 19);
    assert!(
        !CAPTURED_ALL_ACTIONS_ONE_ENTRY.contains(&0x8D),
        "0x8D is the send fixture's own bitset; it must not appear in the capture"
    );
    assert_eq!(JAR_ORDERED_SEND_BODY.len(), 53);
    let bitset_positions: Vec<usize> = JAR_ORDERED_SEND_BODY
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == 0x8D).then_some(index))
        .collect();
    assert_eq!(
        bitset_positions,
        vec![0],
        "the bitset byte is the only 0x8D in the send fixture, and it leads"
    );

    // The property value is the one place a length could be silently wrong: the
    // value's 8 characters sit behind an 8-byte VarInt length prefix.
    let name_end = 2 + 16 + 1 + 7;
    assert_eq!(JAR_ORDERED_SEND_BODY[name_end], 0x01, "one property");
    assert_eq!(
        JAR_ORDERED_SEND_BODY[name_end + 1],
        0x08,
        "property name len"
    );
    assert_eq!(
        JAR_ORDERED_SEND_BODY[name_end + 1 + 1 + 8],
        0x08,
        "property value len"
    );
}
