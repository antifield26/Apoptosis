//! The item-stack metadata value, checked against bytes a real 26.1.2 server sent.
//!
//! # Where these bytes come from
//!
//! `tools/chat-capture/run.py` injects `summon minecraft:item ...` into a real server's console while a client is
//! connected, and the capture holds the four `set_entity_data` bodies that produced. Two of them, stripped of
//! their entity id, index and terminator:
//!
//! ```text
//! 03 01 00 00       count 3, item   1
//! 07 83 07 00 00    count 7, item 899
//! ```
//!
//! **And the values match the commands that made them**: the injections were `{id:"minecraft:stone",count:3}` and
//! `{id:"minecraft:diamond",count:7}`, and `items.tsv` -- verified row for row against `ItemProbe` -- gives item 1
//! as `minecraft:stone` and item 899 as `minecraft:diamond`. Two fields, two packets, both agreeing with the
//! commands and with the registry table at once.

use mc_protocol::packets::play::{METADATA_TYPE_ITEM_STACK, MetadataValue};
use mc_protocol::wire::{PacketReader, PacketWriter};

/// `(payload, count, item id)` for two of the four captured stacks.
const CAPTURED: &[(&[u8], i32, i32)] = &[
    (&[0x03, 0x01, 0x00, 0x00], 3, 1),
    (&[0x07, 0x83, 0x07, 0x00, 0x00], 7, 899),
];

#[test]
fn a_captured_item_stack_decodes_and_re_encodes_to_the_same_bytes() {
    for (payload, count, item_id) in CAPTURED {
        let mut reader = PacketReader::new(payload);
        let value = MetadataValue::decode(&mut reader, METADATA_TYPE_ITEM_STACK).expect("decodes");
        assert_eq!(
            reader.remaining(),
            0,
            "the decoder must consume the whole value"
        );
        assert_eq!(
            value,
            MetadataValue::ItemStack {
                count: *count,
                item_id: *item_id
            },
            "the stack the capture carries"
        );

        // **Byte-identical, unlike the `disguised_chat` encoder.** The component patch is written as the two
        // zeroes the capture has, so this codec reproduces a real server's bytes exactly.
        let mut writer = PacketWriter::new();
        value.encode(&mut writer);
        assert_eq!(
            writer.finish(),
            *payload,
            "the codec must reproduce the server's bytes"
        );
    }
}

#[test]
fn a_stack_with_components_is_refused_rather_than_sent_as_though_it_had_none() {
    // `03 01 01 00` says one component was added. This build does not model components, so encoding such a stack
    // would send a different item than the caller asked for -- which is the failure this refusal exists for.
    let with_components = [0x03, 0x01, 0x01, 0x00];
    let mut reader = PacketReader::new(&with_components);
    assert!(
        MetadataValue::decode(&mut reader, METADATA_TYPE_ITEM_STACK).is_err(),
        "a stack carrying components must not decode as one carrying none"
    );
}

#[test]
fn a_truncated_stack_value_is_refused() {
    for cut in 0..4 {
        let mut reader = PacketReader::new(&[0x03, 0x01, 0x00, 0x00][..cut]);
        assert!(
            MetadataValue::decode(&mut reader, METADATA_TYPE_ITEM_STACK).is_err(),
            "a {cut}-byte prefix decoded, so the decoder is padding what the wire did not send"
        );
    }
}
