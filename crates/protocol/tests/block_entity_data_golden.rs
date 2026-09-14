//! `block_entity_data`, checked against bytes a real 26.1.2 server sent — the
//! experiment that settled P10-09's open question.
//!
//! # The finding, stated first
//!
//! Three earlier sessions placed and filled a chest at the player and captured
//! **zero** `block_entity_data` packets, which is what sent this question to the
//! "unsolved" list. This experiment (tools/chat-capture/be_experiment.py) found
//! the trigger by capturing the packets that *do* exist:
//!
//! ```text
//! block_entity_data (6): 4 packets — sign placement, sign text edit,
//!                        campfire item change, spawner placed with SpawnData
//! block_event       (7): 0 packets
//! ```
//!
//! and the control group stayed silent: **a chest placed at the player, and the
//! same chest filled with five diamonds, produce nothing**. So the trigger is
//! **the block entity's client-visible NBT changing or first being needed for
//! rendering** — a sign is synced on placement (the client renders its text)
//! and again when its text is merged; a campfire syncs its items; a spawner
//! syncs its `SpawnData` — while a chest-type container does neither, because
//! container contents ride the container-menu channel. The task had been
//! worded "on placement and change"; the capture corrects the plan.
//!
//! # What each golden proves, and what it does not
//!
//! Through our own decoder, each captured body pins the field widths and the
//! payload shape. The type ids (`7` sign, `21` campfire, `9` spawner) are the
//! **captured values** — a block entity type is a *built-in* registry compiled
//! into the client jar, so a full id table needs a jar extraction that is still
//! open (P10-07's sibling); asserting the captured numbers is what the capture
//! itself licenses.
//!
//! # Provenance
//!
//! `crates/test-support/fixtures/protocol/block_entity_data_{sign,campfire,spawner}.hex`,
//! copied from `target/be-capture/bodies/`, captured by
//! `tools/chat-capture/be_experiment.py` against a vanilla 26.1.2 dedicated
//! server. The capture directory is not tracked.

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::BlockEntityData;

/// Decode a committed fixture, comment lines dropped.
fn captured(name: &str) -> BlockEntityData {
    let text = match name {
        "sign" => include_str!("../../test-support/fixtures/protocol/block_entity_data_sign.hex"),
        "campfire" => {
            include_str!("../../test-support/fixtures/protocol/block_entity_data_campfire.hex")
        }
        "spawner" => {
            include_str!("../../test-support/fixtures/protocol/block_entity_data_spawner.hex")
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
    BlockEntityData::decode(&bytes).expect("the captured body decodes")
}

#[test]
fn a_sign_text_edit_rides_block_entity_data() {
    let packet = captured("sign");
    assert_eq!(packet.type_id, 7, "the captured block entity type");
    let text = format!("{:?}", packet.data);
    assert!(
        text.contains("back_text") && text.contains("front_text"),
        "a sign's client-visible NBT carries both text faces: {text}"
    );
    assert!(
        text.contains("hello from the review"),
        "the merged line is in there: {text}"
    );
}

#[test]
fn a_campfire_item_change_rides_block_entity_data() {
    let packet = captured("campfire");
    assert_eq!(packet.type_id, 33, "the captured block entity type (0x21)");
    let text = format!("{:?}", packet.data);
    assert!(
        text.contains("Items"),
        "the campfire's synced NBT names its items: {text}"
    );
}

#[test]
fn a_spawner_placed_with_data_rides_block_entity_data() {
    let packet = captured("spawner");
    assert_eq!(packet.type_id, 9, "the captured block entity type");
    let text = format!("{:?}", packet.data);
    assert!(
        text.contains("MaxNearbyEntities") && text.contains("SpawnData"),
        "the spawner's synced NBT carries its spawn configuration: {text}"
    );
}
