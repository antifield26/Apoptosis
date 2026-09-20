//! light packets: masks, arrays, live updates (P15-04).
//!
//! Mechanical split of `super::play`: every item here moved
//! byte-identical. No logic changed.

use crate::ids::clientbound;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

use super::super::Packet;
use super::{packed_len, read_count};

/// Bytes in one light array (2048 = 16³ nibbles).
pub const LIGHT_ARRAY_BYTES: usize = 2048;

/// Highest light-section bit a mask may use.
///
/// Bit `i` of a mask corresponds to light section `i - 1` — section 0 is bit 1
/// and bit 0 covers the layer below the world
/// (`docs/protocol/chunk-wire-format.md` section 6). The value is one below the
/// `i32` sign bit so `1 << (MAX_LIGHT_SECTIONS + 1)` is still a defined shift
/// while a full positive `i32` mask cannot be silently truncated.
pub const MAX_LIGHT_SECTIONS: u32 = 30;

/// The light half of both chunk packets: four masks and two array lists.
///
/// Returned owned by `read_light_data`; `write_light_data` borrows the fields instead, so encoding a packet
/// does not clone a single 2 048-byte array.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightData {
    /// Sections an array follows for, by index. See [`LevelChunkWithLight::sky_light_mask`].
    pub sky_light_mask: Vec<u32>,
    /// Sections a block-light array follows for.
    pub block_light_mask: Vec<u32>,
    /// Sections that are uniformly sky-lit.
    pub empty_sky_light_mask: Vec<u32>,
    /// Sections that are uniformly dark.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

/// Write the four masks and two array lists that `level_chunk_with_light` and `light_update` share.
///
/// # Errors
///
/// [`ServerError::Invariant`] for an array that is not `LIGHT_ARRAY_BYTES`.
#[allow(clippy::too_many_arguments)]
pub fn write_light_data(
    writer: &mut PacketWriter,
    sky_light_mask: &[u32],
    block_light_mask: &[u32],
    empty_sky_light_mask: &[u32],
    empty_block_light_mask: &[u32],
    sky_light: &[Vec<u8>],
    block_light: &[Vec<u8>],
) -> ServerResult<()> {
    write_light_mask(writer, sky_light_mask);
    write_light_mask(writer, block_light_mask);
    write_light_mask(writer, empty_sky_light_mask);
    write_light_mask(writer, empty_block_light_mask);
    encode_light_arrays(writer, sky_light)?;
    encode_light_arrays(writer, block_light)
}

/// Read the four masks and two array lists that both chunk packets share.
///
/// # Errors
///
/// [`ServerError::Protocol`] on a malformed mask, a count that disagrees with its mask, or an array that is not
/// `LIGHT_ARRAY_BYTES`.
pub fn read_light_data(reader: &mut PacketReader<'_>) -> ServerResult<LightData> {
    let sky_light_mask = read_light_mask(reader, "sky light")?;
    let block_light_mask = read_light_mask(reader, "block light")?;
    let empty_sky_light_mask = read_light_mask(reader, "empty sky light")?;
    let empty_block_light_mask = read_light_mask(reader, "empty block light")?;
    let sky_light = decode_light_arrays(reader, &sky_light_mask)?;
    let block_light = decode_light_arrays(reader, &block_light_mask)?;
    Ok(LightData {
        sky_light_mask,
        block_light_mask,
        empty_sky_light_mask,
        empty_block_light_mask,
        sky_light,
        block_light,
    })
}

/// Read a light-array section and check the count against the mask that indexes
/// it (the arrays carry no section index of their own).
pub(crate) fn decode_light_arrays(
    reader: &mut PacketReader<'_>,
    mask: &[u32],
) -> ServerResult<Vec<Vec<u8>>> {
    let count = read_count(reader, "light array", MAX_LIGHT_SECTIONS as usize + 1)?;
    let expected = mask.len();
    if count != expected {
        return Err(ServerError::Protocol(format!(
            "light mask has {expected} sections set but {count} arrays follow"
        )));
    }
    let mut arrays = Vec::with_capacity(count);
    for _ in 0..count {
        let len = reader.read_varint()?;
        if len != LIGHT_ARRAY_BYTES as i32 {
            return Err(ServerError::Protocol(format!(
                "light array declares {len} bytes, expected {LIGHT_ARRAY_BYTES}"
            )));
        }
        let bytes = reader.read_bytes(LIGHT_ARRAY_BYTES)?;
        arrays.push(bytes.to_vec());
    }
    Ok(arrays)
}

/// Append a light-array section (`VarInt` count, then `VarInt 2048` + the bytes).
///
/// The `2048` length prefix is redundant but mandatory, and is written
/// explicitly rather than derived from `array.len()` so a caller cannot
/// accidentally emit an array the client will slice wrongly.
fn encode_light_arrays(writer: &mut PacketWriter, arrays: &[Vec<u8>]) -> ServerResult<()> {
    writer.write_varint(packed_len(arrays.len())?);
    for array in arrays {
        if array.len() != LIGHT_ARRAY_BYTES {
            return Err(ServerError::Invariant(format!(
                "light array of {} bytes, expected {LIGHT_ARRAY_BYTES}",
                array.len()
            )));
        }
        writer.write_varint(LIGHT_ARRAY_BYTES as i32);
        writer.write_bytes(array);
    }
    Ok(())
}

/// Upper bound on the longs a light-mask `BitSet` may claim.
///
/// `MAX_LIGHT_SECTIONS + 1` bits need `(MAX_LIGHT_SECTIONS + 1).div_ceil(64)` longs; one more is allowed so a
/// server that pads is still readable. The bound exists so one hostile `VarInt` cannot drive a large
/// allocation, as with every other count in this packet.
const MAX_LIGHT_MASK_LONGS: i32 = 2;

/// Read one light mask: vanilla's `BitSet` form, a `VarInt` count of longs then that many `i64`s (P10-03,
/// KD-44). Returns the **set indices**, sorted.
pub(crate) fn read_light_mask(reader: &mut PacketReader<'_>, what: &str) -> ServerResult<Vec<u32>> {
    let longs = reader.read_varint()?;
    if longs < 0 {
        return Err(ServerError::Protocol(format!(
            "{what} mask claims {longs} longs"
        )));
    }
    if longs > MAX_LIGHT_MASK_LONGS {
        return Err(ServerError::Protocol(format!(
            "{what} mask claims {longs} longs, above the {MAX_LIGHT_MASK_LONGS} a light section range needs"
        )));
    }
    let mut indices = Vec::new();
    for index in 0..longs {
        // The bit pattern is what matters, so the signed read is reinterpreted rather than range-checked.
        #[allow(clippy::cast_sign_loss)]
        let value = reader.read_i64()? as u64;
        for bit in 0..64u32 {
            if value >> bit & 1 == 1 {
                let section = index as u32 * 64 + bit;
                if section > MAX_LIGHT_SECTIONS {
                    return Err(ServerError::Protocol(format!(
                        "{what} mask sets bit {section}, beyond section {MAX_LIGHT_SECTIONS}"
                    )));
                }
                indices.push(section);
            }
        }
    }
    Ok(indices)
}

/// Write one light mask as a `BitSet`: a `VarInt` count of longs then that many `i64`s.
///
/// Trailing zero longs are trimmed, which is what vanilla's `BitSet.toLongArray()` does. Writing a fixed-width
/// integer instead would emit bytes no real server produces — and the client reads a count, so it would read
/// the wrong number of them.
pub(crate) fn write_light_mask(writer: &mut PacketWriter, mask: &[u32]) {
    let Some(highest) = mask.iter().copied().max() else {
        writer.write_varint(0);
        return;
    };
    let longs = highest / 64 + 1;
    #[allow(clippy::cast_possible_wrap)]
    writer.write_varint(longs as i32);
    for index in 0..longs {
        let mut value = 0u64;
        for bit in 0..64 {
            if mask.contains(&(index * 64 + bit)) {
                value |= 1u64 << bit;
            }
        }
        #[allow(clippy::cast_possible_wrap)]
        writer.write_i64(value as i64);
    }
}

/// `minecraft:light_update` (clientbound play 48).
///
/// Sent when a chunk's light changes without its blocks being re-sent — placing a torch, breaking a block that
/// was casting a shadow. It carries **the same light data** as the tail of [`LevelChunkWithLight`], which is
/// why both go through [`write_light_data`] and [`read_light_data`].
///
/// **The coordinates are `VarInt`s.** The chunk packet writes the same two numbers as `i32`; this one does
/// not, and reading them the other way consumes two extra bytes per coordinate and misparses the light data
/// that follows. That difference is the whole reason this packet is not a struct re-use of the chunk one, and
/// it is pinned by a test.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightUpdate {
    /// Chunk x.
    pub chunk_x: i32,
    /// Chunk z.
    pub chunk_z: i32,
    /// Sections an array follows for, by index.
    pub sky_light_mask: Vec<u32>,
    /// Sections a block-light array follows for.
    pub block_light_mask: Vec<u32>,
    /// Sections that are uniformly sky-lit.
    pub empty_sky_light_mask: Vec<u32>,
    /// Sections that are uniformly dark.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

impl LightUpdate {
    /// Build one from a computed chunk, taking only what changed.
    ///
    /// Every light section is accounted for, the same way [`LevelChunkWithLight`] does it: a section that is
    /// uniformly the layer's default goes in the matching `empty_*` mask and needs no array, and bit `i` is
    /// light section `i`, i.e. world section `i - 1`.
    #[must_use]
    pub fn from_chunk_light(
        chunk_x: i32,
        chunk_z: i32,
        light: &crate::packets::play::LightData,
    ) -> Self {
        Self {
            chunk_x,
            chunk_z,
            sky_light_mask: light.sky_light_mask.clone(),
            block_light_mask: light.block_light_mask.clone(),
            empty_sky_light_mask: light.empty_sky_light_mask.clone(),
            empty_block_light_mask: light.empty_block_light_mask.clone(),
            sky_light: light.sky_light.clone(),
            block_light: light.block_light.clone(),
        }
    }
}

impl Packet for LightUpdate {
    const ID: i32 = clientbound::play::LIGHT_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        // **`VarInt`, not `i32`** — see the type's documentation. The chunk packet beside it uses `i32` for
        // the same two numbers.
        let chunk_x = reader.read_varint()?;
        let chunk_z = reader.read_varint()?;
        let light = read_light_data(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "light_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            sky_light_mask: light.sky_light_mask,
            block_light_mask: light.block_light_mask,
            empty_sky_light_mask: light.empty_sky_light_mask,
            empty_block_light_mask: light.empty_block_light_mask,
            sky_light: light.sky_light,
            block_light: light.block_light,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.chunk_x);
        writer.write_varint(self.chunk_z);
        write_light_data(
            &mut writer,
            &self.sky_light_mask,
            &self.block_light_mask,
            &self.empty_sky_light_mask,
            &self.empty_block_light_mask,
            &self.sky_light,
            &self.block_light,
        )?;
        Ok(writer.finish())
    }
}
