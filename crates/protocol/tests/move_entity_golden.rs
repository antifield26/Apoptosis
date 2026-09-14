//! The three relative-move packets, checked against bodies a real 26.1.2 server sent.
//!
//! # Where the layouts come from
//!
//! The capture holds **11,713** bodies for these three ids, and their lengths settle the field widths: every
//! length is one of two values, and the pair differs by **exactly one byte** — the `VarInt` entity id gaining a
//! byte past 127.
//!
//! ```text
//! 53  move_entity_pos     8 (2499) / 9 (2994)      1 + 6 + 1
//! 54  move_entity_pos_rot 10 (4249) / 11 (1774)    1 + 6 + 2 + 1
//! 56  move_entity_rot      4 (189) / 5 (8)         1 + 2 + 1
//! ```
//!
//! # Every assertion below is read off the bytes
//!
//! The first version of this file asserted that the ground flag was set, **having assumed it rather than looked**:
//! all three samples end in `00`. That is why each expectation here is written with its arithmetic beside it.

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{MoveEntityPos, MoveEntityPosRot, MoveEntityRot};

/// `16 | 00 00 | 00 00 | 00 00 | 00` — id 22, no deltas, **not** on the ground.
const POS: [u8; 8] = [0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

/// `16 | ff ea | 02 af | fe 67 | 45 | 00 | 00` — id 22, deltas -22 / 687 / -409, yaw 69, pitch 0, not on ground.
const POS_ROT: [u8; 10] = [0x16, 0xff, 0xea, 0x02, 0xaf, 0xfe, 0x67, 0x45, 0x00, 0x00];

/// `19 | 00 | 00 | 00` — id 25, no rotation, not on the ground.
const ROT: [u8; 4] = [0x19, 0x00, 0x00, 0x00];

#[test]
fn a_captured_move_entity_pos_decodes_and_re_encodes_to_the_same_bytes() {
    let packet = MoveEntityPos::decode(&POS).expect("decodes");
    assert_eq!(packet.entity_id, 22, "0x16");
    assert_eq!((packet.dx, packet.dy, packet.dz), (0, 0, 0));
    assert!(
        !packet.on_ground,
        "the body ends in 00, so the flag is clear"
    );
    assert_eq!(packet.encode().expect("encodes"), POS);
}

#[test]
fn a_captured_move_entity_pos_rot_decodes_and_re_encodes_to_the_same_bytes() {
    let packet = MoveEntityPosRot::decode(&POS_ROT).expect("decodes");
    assert_eq!(packet.entity_id, 22, "0x16");
    // Big-endian, as every multi-byte field in this protocol is -- the same reading that a hand-computed
    // expectation got wrong once already, in the entity-motion codec.
    assert_eq!(packet.dx, -22, "0xffea");
    assert_eq!(packet.dy, 687, "0x02af");
    assert_eq!(packet.dz, -409, "0xfe67");
    assert_eq!(packet.yaw, 69, "0x45");
    assert_eq!(packet.pitch, 0);
    assert!(!packet.on_ground, "the body ends in 00");
    assert_eq!(packet.encode().expect("encodes"), POS_ROT);
}

#[test]
fn a_captured_move_entity_rot_decodes_and_re_encodes_to_the_same_bytes() {
    let packet = MoveEntityRot::decode(&ROT).expect("decodes");
    assert_eq!(packet.entity_id, 25, "0x19");
    assert_eq!((packet.yaw, packet.pitch), (0, 0));
    assert!(!packet.on_ground, "the body ends in 00");
    assert_eq!(packet.encode().expect("encodes"), ROT);
}

#[test]
fn every_prefix_of_every_captured_body_is_refused() {
    // A prefix that stops inside a field must not decode into a packet claiming a shorter payload; every field
    // here is fixed-width or a VarInt, and both fail rather than pad.
    for cut in 0..POS.len() {
        assert!(
            MoveEntityPos::decode(&POS[..cut]).is_err(),
            "pos: {cut} bytes"
        );
    }
    for cut in 0..POS_ROT.len() {
        assert!(
            MoveEntityPosRot::decode(&POS_ROT[..cut]).is_err(),
            "pos_rot: {cut} bytes"
        );
    }
    for cut in 0..ROT.len() {
        assert!(
            MoveEntityRot::decode(&ROT[..cut]).is_err(),
            "rot: {cut} bytes"
        );
    }
}

#[test]
fn a_body_with_a_tail_is_refused_rather_than_truncated() {
    // The three share one helper that refuses trailing bytes, because a decoder that ignores a tail accepts a
    // longer packet as a shorter one -- which is how a field a future version added goes missing silently.
    let mut longer = POS.to_vec();
    longer.push(0x01);
    assert!(MoveEntityPos::decode(&longer).is_err());
}
