//! `set_entity_data` slot table for the mob kinds Phase 11 spawns, checked
//! against bodies a real 26.1.2 server sent — the close-out of P10-07.
//!
//! # The finding, stated first
//!
//! A console `summon` per kind, with one client in the world, makes the vanilla
//! server enumerate each kind's metadata slots; the capture reads them back
//! through this repo's own decoder, which refuses a body it cannot walk to a
//! clean terminator. Two runs (`tools/chat-capture/entity_experiment.py` and
//! `chicken_experiment.py`) plus the earlier natural-spawn capture give every
//! Phase 11 kind the same two load-bearing slots:
//!
//! - **health rides index 9 with the float serializer** (now
//!   [`METADATA_INDEX_HEALTH`]);
//! - **everything else is optional at spawn** — variants, wool color and the
//!   slime size are per-type slots, and a spawn of a default-variant mob omits
//!   them, which is why a server that sends only health still matches vanilla's
//!   spawn shape.
//!
//! # What the goldens prove, and what they do not
//!
//! The two slime bodies are the only captured mob bodies our decoder can walk
//! end to end, because slime spawns carry no slot whose width the capture has
//! not solved (the other kinds add variant slots — serializers 23, 28, 30, 31 —
//! whose value widths the walk records and stops at). So the goldens pin the
//! decoder's byte-level behavior on mob metadata: index order, the float
//! health encoding, the slime size slot (`NBT Size + 1`, health its square at
//! both measured sizes) and the terminator. The full per-type table, including
//! the pairs our walk could not value-decode, lives in
//! `crates/test-support/fixtures/registry/entity_metadata.tsv` and is pinned
//! row by row below.
//!
//! # Provenance
//!
//! `crates/test-support/fixtures/protocol/set_entity_data_slime_{small,medium}.hex`,
//! copied from `target/chicken-capture/bodies/`, captured by
//! `tools/chat-capture/chicken_experiment.py` against a vanilla 26.1.2
//! dedicated server (`summon minecraft:slime ~ ~3 ~ {Size:1b}` and `{Size:2b}`).

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    METADATA_INDEX_HEALTH, METADATA_TYPE_FLOAT, METADATA_TYPE_ITEM_STACK, SetEntityData,
};

/// Decode a committed fixture, comment lines dropped.
fn captured(name: &str) -> (Vec<u8>, SetEntityData) {
    let text = match name {
        "small" => {
            include_str!("../../test-support/fixtures/protocol/set_entity_data_slime_small.hex")
        }
        "medium" => {
            include_str!("../../test-support/fixtures/protocol/set_entity_data_slime_medium.hex")
        }
        other => panic!("unknown fixture {other}"),
    };
    let hex: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .collect();
    assert!(!hex.is_empty(), "the fixture parsed as empty");
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect();
    let packet = SetEntityData::decode(&bytes).expect("the captured body decodes");
    (bytes, packet)
}

#[test]
fn a_summoned_slime_carries_health_and_its_size_slot() {
    let (bytes, packet) = captured("small");
    assert_eq!(
        packet.entity_id, 1148,
        "the captured entity id (VarInt fc 08)"
    );
    assert_eq!(
        packet.entries,
        vec![
            (9, mc_protocol::packets::play::MetadataValue::Float(4.0)),
            (16, mc_protocol::packets::play::MetadataValue::VarInt(2)),
        ],
        "health at index 9, size slot at 16; nothing else on a fresh spawn"
    );
    assert_eq!(
        bytes[bytes.len() - 1],
        mc_protocol::packets::play::METADATA_TERMINATOR
    );
}

#[test]
fn the_medium_slime_scales_health_with_the_size_slot() {
    let (_bytes, packet) = captured("medium");
    assert_eq!(packet.entity_id, 1191);
    assert_eq!(
        packet.entries,
        vec![
            (9, mc_protocol::packets::play::MetadataValue::Float(9.0)),
            (16, mc_protocol::packets::play::MetadataValue::VarInt(3)),
        ],
        "NBT Size 2 sends size slot 3 and health 9.0: slot = Size + 1, health its square"
    );
}

#[test]
fn a_captured_slime_re_encodes_to_the_same_bytes() {
    for name in ["small", "medium"] {
        let (bytes, packet) = captured(name);
        assert_eq!(
            packet.encode().expect("encodes"),
            bytes,
            "{name} round-trips"
        );
    }
}

#[test]
fn the_health_value_is_read_from_those_exact_bytes() {
    // Perturb the float's leading byte (40 -> 41, 4.0 -> 16.0) and the decoder
    // must say 16.0 — proof the health value comes from these bytes and not
    // from a default.
    let text = include_str!("../../test-support/fixtures/protocol/set_entity_data_slime_small.hex");
    let hex: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .collect();
    let mut bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect();
    assert_eq!(
        &bytes[4..8],
        &[0x40, 0x80, 0x00, 0x00],
        "the float is where the comment says"
    );
    bytes[4] = 0x41;
    let packet = SetEntityData::decode(&bytes).expect("still decodes");
    assert_eq!(
        packet.entries[0],
        (9, mc_protocol::packets::play::MetadataValue::Float(16.0)),
        "the perturbed byte is the health value"
    );
}

#[test]
fn a_truncated_terminator_is_an_error_even_on_captured_data() {
    let (mut bytes, _packet) = captured("small");
    bytes.pop();
    assert!(
        SetEntityData::decode(&bytes).is_err(),
        "a body that runs out without 0xFF must be refused, not half-read"
    );
}

/// One parsed row of `entity_metadata.tsv`.
struct Row {
    entity: String,
    index: u8,
    serializer: i32,
}

fn table() -> Vec<Row> {
    let text = include_str!("../../test-support/fixtures/registry/entity_metadata.tsv");
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            assert!(
                fields.len() >= 3,
                "a row has entity, index, serializer: {line}"
            );
            Row {
                entity: fields[0].to_owned(),
                index: fields[1].parse().expect("index parses"),
                serializer: fields[2].parse().expect("serializer parses"),
            }
        })
        .collect()
}

/// Every kind Phase 11 will spawn, against `entity_metadata.tsv`.
const PHASE_ELEVEN_KINDS: &[&str] = &[
    "minecraft:zombie",
    "minecraft:skeleton",
    "minecraft:creeper",
    "minecraft:spider",
    "minecraft:pig",
    "minecraft:chicken",
    "minecraft:sheep",
    "minecraft:cow",
    "minecraft:slime",
];

#[test]
fn every_phase_eleven_kind_has_the_health_slot_at_index_nine() {
    let rows = table();
    for kind in PHASE_ELEVEN_KINDS {
        let hit = rows
            .iter()
            .find(|row| row.entity == *kind && row.index == METADATA_INDEX_HEALTH)
            .unwrap_or_else(|| panic!("{kind} has a health row"));
        assert_eq!(
            hit.serializer, METADATA_TYPE_FLOAT,
            "{kind} health rides the float serializer"
        );
    }
}

#[test]
fn the_per_type_rows_the_capture_measured_are_pinned() {
    let rows = table();
    let has = |entity: &str, index: u8, serializer: i32| {
        rows.iter()
            .any(|row| row.entity == entity && row.index == index && row.serializer == serializer)
    };
    // The dropped item this server already sends (P10-08, verified three ways).
    assert!(has("minecraft:item", 8, METADATA_TYPE_ITEM_STACK));
    // Slime's size slot: the only other slot this build decodes end to end.
    assert!(has("minecraft:slime", 16, 1));
    // Per-type rows whose value widths stay unsolved — pinned as pairs.
    assert!(has("minecraft:sheep", 18, 0), "sheep wool byte");
    assert!(has("minecraft:cow", 18, 23), "cow variant");
    assert!(has("minecraft:pig", 19, 28), "pig variant");
    assert!(has("minecraft:chicken", 18, 30), "chicken variant");
    assert!(has("minecraft:creeper", 16, 1), "creeper fuse");
}

#[test]
fn the_health_constant_agrees_with_every_captured_kind() {
    // The constant the send path will use, held against the table it was
    // measured from — the check that keeps it from drifting back to a guess.
    assert_eq!(METADATA_INDEX_HEALTH, 9);
    let rows = table();
    for kind in PHASE_ELEVEN_KINDS {
        assert!(
            rows.iter()
                .any(|row| row.entity == *kind && row.index == METADATA_INDEX_HEALTH),
            "{kind} pins index {METADATA_INDEX_HEALTH} as its health slot"
        );
    }
}
