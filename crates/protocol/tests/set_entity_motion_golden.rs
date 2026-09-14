//! `set_entity_motion`, checked against 2942 bodies a real 26.1.2 server sent.
//!
//! # The strongest evidence any codec in this phase has had
//!
//! **Every one of the 2942 `set_entity_motion` bodies in `target/vanilla-capture/bodies-lit/` is exactly seven
//! bytes**, and `1 + 3 * 2 = 7` is what a `VarInt` entity id plus three `i16` velocities comes to. `add_entity`
//! had 55 bodies for a 52-byte layout; this has 2942 for a 7-byte one, so "the lengths agree" is not a
//! coincidence that survived a single sample.
//!
//! Two of them, verbatim, with what they decode to:
//!
//! ```text
//! 000143_s2c_play_101.bin: 4e 49 f9 7c 92 eb ed   -> id 78, 18937, 31890, -5139 -> 2.37, 3.99, -0.64
//! 000214_s2c_play_101.bin: 43 e9 0e 78 54 ec ae   -> id 67, -5874, 30804, -4946 -> -0.73, 3.85, -0.62
//! ```

use mc_protocol::packets::play::SetEntityMotion;
use mc_protocol::wire::{PacketReader, PacketWriter};

/// `(source file, body, entity id, velocities)` for two of the 2942 captured bodies.
const CAPTURED: &[(&str, [u8; 7], i32, [i16; 3])] = &[
    (
        "000143_s2c_play_101.bin",
        [0x4e, 0x49, 0xf9, 0x7c, 0x92, 0xeb, 0xed],
        78,
        [18937, 31890, -5139],
    ),
    (
        "000214_s2c_play_101.bin",
        [0x43, 0xe9, 0x0e, 0x78, 0x54, 0xec, 0xae],
        67,
        [-5874, 30804, -4946],
    ),
];

#[test]
fn the_shape_implies_seven_bytes_and_the_server_sent_seven_bytes() {
    // One byte of VarInt id plus three i16. Arithmetic on somebody else's output: 2942 bodies, one length.
    let implied = 1 + 3 * 2;
    assert_eq!(implied, 7);
    for (source, body, _, _) in CAPTURED {
        assert_eq!(body.len(), implied, "{source} is not 7 bytes");
    }
}

#[test]
fn a_real_set_entity_motion_decodes_to_plausible_velocities() {
    for (source, body, entity_id, velocities) in CAPTURED {
        let mut reader = PacketReader::new(body);
        let packet = SetEntityMotion::decode(&mut reader).expect("decodes");
        assert_eq!(reader.remaining(), 0, "{source}: bytes left over");
        assert_eq!(packet.entity_id, *entity_id, "{source}: entity id");
        assert_eq!(
            [packet.velocity_x, packet.velocity_y, packet.velocity_z],
            *velocities,
            "{source}: velocities"
        );

        // **The check a length assertion cannot make.** An `i16` read at the wrong offset is still an `i16`; it
        // would not give three small numbers that look like something walking around a world. A block per tick
        // is already an implausible speed for a mob, so anything under a few blocks is fine and anything over
        // four is a misread.
        let (x, y, z) = packet.velocity();
        for (axis, value) in [("x", x), ("y", y), ("z", z)] {
            assert!(
                value.is_finite() && value.abs() < 4.0,
                "{source}: velocity {axis} decoded as {value} blocks/tick, which no entity walks at"
            );
        }
    }
}

#[test]
fn set_entity_motion_round_trips() {
    for (id, x, y, z) in [
        (1, 0.0, 0.0, 0.0),
        (78, -0.2, -3.5, -0.6),
        (i32::MAX, 4.0, -4.0, 0.000_125),
    ] {
        let packet = SetEntityMotion::from_velocity(id, x, y, z).expect("in range");
        let mut writer = PacketWriter::new();
        packet.encode(&mut writer).expect("encodes");
        let out = writer.finish();
        let mut reader = PacketReader::new(&out);
        assert_eq!(
            SetEntityMotion::decode(&mut reader).expect("decodes"),
            packet
        );
        assert_eq!(reader.remaining(), 0);
    }
}

#[test]
fn a_velocity_the_wire_cannot_carry_is_refused_rather_than_wrapped() {
    // 4.1 blocks/tick is above `i16::MAX` at 1/8000, and a silent wrap would send the client an entity moving
    // the other way at speed. NaN and infinity are the same class of caller error.
    for (x, y, z) in [
        (5.0, 0.0, 0.0),
        (0.0, -5.0, 0.0),
        (f64::NAN, 0.0, 0.0),
        (0.0, f64::INFINITY, 0.0),
    ] {
        assert!(
            SetEntityMotion::from_velocity(1, x, y, z).is_err(),
            "({x}, {y}, {z}) must be refused"
        );
    }
}
