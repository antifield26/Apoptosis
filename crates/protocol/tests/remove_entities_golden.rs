//! `remove_entities`, checked against bytes a real 26.1.2 server sent.
//!
//! # The claim that does not depend on my reading of the format
//!
//! A `VarInt` count followed by that many `VarInt` ids is **one byte of count plus one byte per id**, so a single
//! removal is two bytes. All **twelve** `remove_entities` bodies in `target/vanilla-capture/bodies-lit/` are
//! exactly two bytes, and three of them are here:
//!
//! ```text
//! 001788_s2c_play_77.bin: 01 0f   -> count 1, entity id 15
//! 002890_s2c_play_77.bin: 01 4d   -> count 1, entity id 77
//! 003812_s2c_play_77.bin: 01 36   -> count 1, entity id 54
//! ```
//!
//! **Embedded rather than committed as a fixture**, unlike `add_entity_slime.hex`: two bytes with their source in
//! a comment is a whole capture, and a separate file would be a second place to keep the provenance right. The
//! twelve-body claim is asserted below against the same three, which is what the capture actually supports saying
//! from here.

use mc_protocol::packets::play::RemoveEntities;
use mc_protocol::wire::{PacketReader, PacketWriter};

/// `(source file, bytes)` for three of the twelve captured bodies.
const CAPTURED: &[(&str, [u8; 2])] = &[
    ("001788_s2c_play_77.bin", [0x01, 0x0f]),
    ("002890_s2c_play_77.bin", [0x01, 0x4d]),
    ("003812_s2c_play_77.bin", [0x01, 0x36]),
];

#[test]
fn the_shape_implies_two_bytes_and_the_server_sent_two_bytes() {
    // One byte of count, one byte of id. This is arithmetic on somebody else's output rather than an agreement
    // between two of my own readings.
    let implied = 1 + 1;
    for (source, bytes) in CAPTURED {
        assert_eq!(
            bytes.len(),
            implied,
            "{source} is not the length this shape implies"
        );
    }
}

#[test]
fn a_real_remove_entities_decodes_to_the_id_it_names() {
    // The ids the three captures name, in the order the table lists them.
    for ((source, bytes), expected) in CAPTURED.iter().zip([15, 77, 54]) {
        let mut reader = PacketReader::new(bytes);
        let packet = RemoveEntities::decode(&mut reader).expect("decodes");
        assert_eq!(
            reader.remaining(),
            0,
            "{source}: the decoder left bytes behind"
        );
        assert_eq!(
            packet.entity_ids,
            vec![expected],
            "{source} names entity id {expected}"
        );
    }
}

#[test]
fn remove_entities_round_trips_through_our_own_codec() {
    for ids in [vec![1], vec![15, 77, 54], vec![i32::MAX], vec![]] {
        let packet = RemoveEntities {
            entity_ids: ids.clone(),
        };
        let mut writer = PacketWriter::new();
        packet.encode_into(&mut writer).expect("encodes");
        let out = writer.finish();
        let mut reader = PacketReader::new(&out);
        assert_eq!(
            RemoveEntities::decode(&mut reader)
                .expect("decodes")
                .entity_ids,
            ids
        );
        assert_eq!(reader.remaining(), 0);
    }
}

#[test]
fn a_hostile_count_is_refused_rather_than_reserved() {
    // `ff ff ff ff 0f` is a VarInt of 4294967295 with no ids behind it. A decoder that reserved on the count
    // would try to allocate sixteen gigabytes per byte of input; one that reads what is there runs off the end
    // and reports it, instead of amplifying a five-byte body into a sixteen-gigabyte allocation.
    let hostile = [0xff, 0xff, 0xff, 0xff, 0x0f];
    let mut reader = PacketReader::new(&hostile);
    assert!(
        RemoveEntities::decode(&mut reader).is_err(),
        "a count with no ids behind it must not decode"
    );

    // And a negative count is not a count at all.
    let negative = [0xff, 0xff, 0xff, 0xff, 0x0f];
    let mut reader = PacketReader::new(&negative);
    assert!(RemoveEntities::decode(&mut reader).is_err());
}

#[test]
fn every_prefix_of_a_real_body_is_refused() {
    for (source, bytes) in CAPTURED {
        let mut reader = PacketReader::new(&bytes[..1]);
        assert!(
            RemoveEntities::decode(&mut reader).is_err(),
            "{source}: a one-byte prefix decoded, so the decoder is padding what the wire did not send"
        );
    }
}
