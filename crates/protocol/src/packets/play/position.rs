//! position packets: packing, teleports, entity moves (P15-04).
//!
//! Mechanical split of `super::play`: every item here moved
//! byte-identical. No logic changed.

use crate::ids::clientbound;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

use super::super::Packet;

/// Bits of the y coordinate in a packed block position.
const POSITION_Y_BITS: u32 = 12;

/// Bits of an x/z coordinate in a packed block position.
const POSITION_HORIZONTAL_BITS: u32 = 26;

/// Bit offset of the x field in a packed block position.
const POSITION_X_SHIFT: u32 = 38;

/// Bit offset of the z field in a packed block position.
const POSITION_Z_SHIFT: u32 = 12;

/// Mask selecting an x/z field before it is shifted into place.
const POSITION_HORIZONTAL_MASK: i64 = (1 << POSITION_HORIZONTAL_BITS) - 1;

/// Mask selecting the y field in place.
const POSITION_Y_MASK: i64 = (1 << POSITION_Y_BITS) - 1;

/// Pack a block position the way `net.minecraft.core.BlockPos.asLong` does
/// (`((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`).
///
/// The coordinate ranges are ±33 554 431 horizontally and ±2048 vertically;
/// values outside a field's width are truncated into it, exactly as the vanilla
/// bit operations do. Callers validate gameplay-coordinate bounds separately —
/// this is the wire encoding, not a range check.
#[must_use]
pub const fn block_position(x: i32, y: i32, z: i32) -> i64 {
    let x = (x as i64 & POSITION_HORIZONTAL_MASK) << POSITION_X_SHIFT;
    let z = (z as i64 & POSITION_HORIZONTAL_MASK) << POSITION_Z_SHIFT;
    let y = y as i64 & POSITION_Y_MASK;
    x | z | y
}

/// Inverse of [`block_position`], sign-extending each field.
///
/// Shifting a field up to the top of the 64-bit word and back down with an
/// arithmetic right shift *is* the sign extension, so a negative x, y or z
/// comes back negative instead of picking up the neighbouring field's bits.
#[must_use]
pub const fn unpack_block_position(packed: i64) -> (i32, i32, i32) {
    let x = (packed << (64 - POSITION_HORIZONTAL_BITS - POSITION_X_SHIFT))
        >> (64 - POSITION_HORIZONTAL_BITS);
    let z = (packed << (64 - POSITION_HORIZONTAL_BITS - POSITION_Z_SHIFT))
        >> (64 - POSITION_HORIZONTAL_BITS);
    let y = (packed << (64 - POSITION_Y_BITS)) >> (64 - POSITION_Y_BITS);
    (x as i32, y as i32, z as i32)
}

/// `minecraft:player_position` / teleport (clientbound 72).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPosition {
    /// Absolute x.
    pub x: f64,
    /// Absolute y.
    pub y: f64,
    /// Absolute z.
    pub z: f64,
    /// Velocity x.
    pub velocity_x: f64,
    /// Velocity y.
    pub velocity_y: f64,
    /// Velocity z.
    pub velocity_z: f64,
    /// Yaw degrees.
    pub yaw: f32,
    /// Pitch degrees.
    pub pitch: f32,
    /// Relative position/rotation flags.
    pub flags: i32,
    /// Teleport id to acknowledge.
    pub teleport_id: i32,
}

impl Packet for PlayerPosition {
    const ID: i32 = clientbound::play::PLAYER_POSITION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        // The teleport id **leads**. Captured from a vanilla 26.1.2 server: `01` then six `f64`s, two `f32`s
        // and an `i32` of flags, which is exactly the 61 bytes the wire reports (P10-03).
        Ok(Self {
            teleport_id: reader.read_varint()?,
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            velocity_x: reader.read_f64()?,
            velocity_y: reader.read_f64()?,
            velocity_z: reader.read_f64()?,
            yaw: reader.read_f32()?,
            pitch: reader.read_f32()?,
            flags: reader.read_i32()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        // Leading, and it matters: a client reads this first. Writing it last made the client consume our
        // first byte of `x` as the id and then report one byte left over.
        writer.write_varint(self.teleport_id);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_f64(self.velocity_x);
        writer.write_f64(self.velocity_y);
        writer.write_f64(self.velocity_z);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        writer.write_i32(self.flags);
        Ok(writer.finish())
    }
}

/// The three relative-move packets, whose layouts a capture settled.
///
/// A real 26.1.2 server sent **11,713** bodies for these three ids and every one is a length the field widths
/// imply, with the two values per packet differing by exactly the one byte a `VarInt` id gains past 127:
///
/// ```text
/// 53  move_entity_pos      id, dx, dy, dz (i16), on_ground                 = 8 / 9
/// 54  move_entity_pos_rot  id, dx, dy, dz (i16), yaw, pitch (i8), on_ground = 10 / 11
/// 56  move_entity_rot      id, yaw, pitch (i8), on_ground                  = 4 / 5
/// ```
///
/// **Deltas, not positions**: vanilla sends these for small movements and `teleport_entity` for large ones, so the
/// units are 1/4096 of a block per axis and the caller decides which packet a movement deserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPos {
    /// Entity id.
    pub entity_id: i32,
    /// X delta, in 1/4096 of a block.
    pub dx: i16,
    /// Y delta, in 1/4096 of a block.
    pub dy: i16,
    /// Z delta, in 1/4096 of a block.
    pub dz: i16,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

/// A relative move with rotation; see [`MoveEntityPos`] for the layout's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPosRot {
    /// Entity id.
    pub entity_id: i32,
    /// X delta, in 1/4096 of a block.
    pub dx: i16,
    /// Y delta, in 1/4096 of a block.
    pub dy: i16,
    /// Z delta, in 1/4096 of a block.
    pub dz: i16,
    /// Yaw, in 1/256 of a degree.
    pub yaw: i8,
    /// Pitch, in 1/256 of a degree.
    pub pitch: i8,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

/// Rotation only; see [`MoveEntityPos`] for the layout's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityRot {
    /// Entity id.
    pub entity_id: i32,
    /// Yaw, in 1/256 of a degree.
    pub yaw: i8,
    /// Pitch, in 1/256 of a degree.
    pub pitch: i8,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

impl Packet for MoveEntityPos {
    const ID: i32 = clientbound::play::MOVE_ENTITY_POS;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_pos")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

impl Packet for MoveEntityPosRot {
    const ID: i32 = clientbound::play::MOVE_ENTITY_POS_ROT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            yaw: reader.read_i8()?,
            pitch: reader.read_i8()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_pos_rot")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

impl Packet for MoveEntityRot {
    const ID: i32 = clientbound::play::MOVE_ENTITY_ROT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            yaw: reader.read_i8()?,
            pitch: reader.read_i8()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_rot")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

/// Refuse a body with bytes left over, naming the packet.
///
/// Shared by the three above because a decoder that ignores a tail would accept a longer packet as a shorter one
/// -- which is how a field a future version added goes missing without anything reporting it.
fn require_exhausted(reader: &PacketReader<'_>, packet: &str) -> ServerResult<()> {
    if reader.is_empty() {
        return Ok(());
    }
    Err(ServerError::Protocol(format!(
        "{packet} has {} trailing bytes",
        reader.remaining()
    )))
}
