//! `add_entity`, checked against bytes a real 26.1.2 server sent.
//!
//! # What this proves, and what it does not
//!
//! A golden test through **our own decoder** proves the layout round-trips and that every field width is right —
//! but if the fixture and the decoder came from one reading of the format, comparing them only confirms that
//! reading. **The claim that is independent of me is narrower and is asserted first**:
//!
//! ```text
//! 1 (VarInt id) + 16 (UUID) + 1 (VarInt type) + 3*8 (f64) + 3*1 (i8) + 1 (VarInt data) + 3*2 (i16) = 52
//! ```
//!
//! and the captured body **is** 52 bytes. That is arithmetic on somebody else's output.
//!
//! # Provenance
//!
//! `crates/test-support/fixtures/protocol/add_entity_slime.hex`, copied from
//! `target/vanilla-capture/bodies-lit/000139_s2c_play_1.bin` — a vanilla 26.1.2 dedicated server, through the
//! rig in `tools/surface-capture/`.

use mc_protocol::packets::play::AddEntity;
use mc_protocol::wire::{PacketReader, PacketWriter};

/// The captured body, from the committed fixture.
fn captured() -> Vec<u8> {
    let text = include_str!("../../test-support/fixtures/protocol/add_entity_slime.hex");
    let hex: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .collect();
    assert!(!hex.is_empty(), "the fixture parsed as empty");
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn the_field_widths_add_up_to_what_the_server_sent() {
    // **The claim that does not depend on my reading of the format.** A real server produced these bytes; the
    // widths the packet's shape implies sum to a number; the two agree. Written as a sum of parts rather than
    // with `3 * 1`, which clippy correctly calls an operation that has no effect.
    let implied = 1 + 16 + 1 + 24 + 3 + 1 + 6;
    assert_eq!(implied, 52, "the layout's own arithmetic changed");
    assert_eq!(
        captured().len(),
        implied,
        "the captured body is not the size this layout implies"
    );
}

#[test]
fn a_real_add_entity_decodes_to_a_slime_somewhere_a_player_could_stand() {
    let bytes = captured();
    let mut reader = PacketReader::new(&bytes);
    let entity = AddEntity::decode(&mut reader).expect("decodes");
    assert_eq!(
        reader.remaining(),
        0,
        "the decoder must consume the whole body"
    );

    assert_eq!(entity.entity_id, 78, "the VarInt entity id");
    assert_eq!(
        entity.type_id, 117,
        "entity_types.tsv says 117 is minecraft:slime, and the fixture's header says so too"
    );

    // A decoder that read the doubles at the wrong offset would produce a denormal, a NaN or something no one
    // could stand at. The captured entity was spawned in a real world, so its coordinates have to look like one.
    for (axis, value) in [("x", entity.x), ("y", entity.y), ("z", entity.z)] {
        assert!(
            value.is_finite() && value.abs() < 30_000_000.0,
            "{axis} decoded as {value}, which is not a world coordinate"
        );
    }
    assert!(
        (-64.0..320.0).contains(&entity.y),
        "y = {} is outside the world height",
        entity.y
    );
}

#[test]
fn add_entity_round_trips_through_our_own_codec() {
    let entity = AddEntity {
        entity_id: 42,
        uuid: uuid::Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
        type_id: 71,
        x: -8.5,
        y: 66.0,
        z: 12.25,
        pitch: 10,
        yaw: -20,
        head_yaw: 30,
        data: 0,
        velocity_x: 0,
        velocity_y: -100,
        velocity_z: 250,
    };
    let mut writer = PacketWriter::new();
    entity.encode_into(&mut writer).expect("encodes");
    let out = writer.finish();
    let mut reader = PacketReader::new(&out);
    assert_eq!(AddEntity::decode(&mut reader).expect("decodes"), entity);
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn a_truncated_body_is_refused_rather_than_padded() {
    // Every prefix of a real body is short of a field, so none of them may decode. A lenient decoder that
    // defaulted the missing fields would send a client an entity at the origin.
    let bytes = captured();
    for cut in 0..bytes.len() {
        let mut reader = PacketReader::new(&bytes[..cut]);
        assert!(
            AddEntity::decode(&mut reader).is_err(),
            "a {cut}-byte prefix decoded, so the decoder is padding what the wire did not send"
        );
    }
}
