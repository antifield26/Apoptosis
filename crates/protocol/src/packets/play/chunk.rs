//! chunk packets: palettes, sections, heightmaps, full chunk bodies (P15-04).
//!
//! Mechanical split of `super::play`: every item here moved
//! byte-identical. No logic changed.

use crate::ids::clientbound;
use crate::nbt::Nbt;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};
use mc_core::packing;

use super::super::Packet;
use super::{
    ChunkBlockEntity, packed_len, packing_error, read_count, read_light_data, write_light_data,
};

/// Largest palette accepted in a network paletted container.
///
/// A block-state palette cannot exceed one entry per block state in the
/// registry, and `bits` is capped at [`mc_core::packing::MAX_BITS`] = 32
/// anyway; the explicit cap keeps a hostile length from being used as an
/// allocation hint before the bits check rejects it.
pub const MAX_PALETTE_LEN: usize = 1 << 16;

/// Largest section count accepted in a `chunk data` blob.
///
/// The 26.1 overworld is 24 sections tall with room for expansion; the cap is
/// generous but keeps the section loop bounded independently of the frame cap.
pub const MAX_CHUNK_SECTIONS: usize = 1024;

/// Largest heightmap count accepted in one chunk.
pub const MAX_HEIGHTMAPS: usize = 16;

/// Largest long count accepted in one heightmap.
///
/// A heightmap holds one column per horizontal position, packed 9 entries to a
/// `long` in vanilla; the cap is far above that but still bounds the loop.
pub const MAX_HEIGHTMAP_LONGS: usize = 4096;

/// Largest block-entity count accepted in one chunk.
pub const MAX_BLOCK_ENTITIES: usize = 4096;

// ---------------------------------------------------------------------------
// Block positions
// ---------------------------------------------------------------------------

/// Block states in one chunk section (16×16×16).
pub const BLOCKS_PER_SECTION: usize = packing::BLOCK_ENTRIES;

/// Biomes in one chunk section (4×4×4).
pub const BIOMES_PER_SECTION: usize = packing::BIOME_ENTRIES;

/// Minimum bit width for a **network** biome palette.
///
/// The disk form uses 1 ([`mc_core::packing::BIOME_MIN_BITS`]) but the
/// network `Strategy.createForBiomes` uses **2**
/// (`docs/protocol/chunk-wire-format.md` section 7), so a disk container's width
/// must not be reused verbatim on the wire. Block states agree at 4 in both
/// forms, which is why only the biome side needs its own constant.
pub const NETWORK_BIOME_MIN_BITS: u32 = 2;

/// A paletted container in its **network** form.
///
/// ```text
/// u8   bits_per_entry
/// bits == 0:  VarInt single global palette id      (the whole container)
/// bits != 0:  VarInt palette length
///             VarInt global palette id   × length
///             i64    packed indices      × longs_needed(entries, bits)
/// ```
///
/// Verified from the jar as `PalettedContainer$Data.write`
/// (`docs/protocol/chunk-wire-format.md` section 5). Two details matter and are
/// easy to get wrong:
///
/// - the storage array is `writeFixedSizeLongArray`: **raw longs with no**
///   `VarInt` length prefix. The count is derived from `bits` and the container's
///   cell count (4096 block states, 64 biomes). Only a heightmap's long array
///   carries a length prefix.
/// - `bits == 0` is the single-value form: one global palette id and **no**
///   palette and **no** array. It is not "the minimum width for a 1-entry
///   palette" — `bits_for` would answer 4 there, and a 4-bit container does
///   carry an array.
///
/// `palette` and `values` are the **decompressed** form: exactly
/// [`BLOCKS_PER_SECTION`] (or [`BIOMES_PER_SECTION`]) palette indices, one per
/// cell, which is what the world layer works with. Packing is delegated to
/// [`mc_core::packing`] — LSB-first, values never spanning a `long`
/// boundary — rather than reimplemented here, so the disk and network forms
/// cannot drift apart (P03-10).
///
/// [`PalettedContainer::decode`] re-derives the bit width from the palette and
/// rejects a mismatch: nothing in this version lets a peer widen a container.
///
/// The biome minimum on the network is [`NETWORK_BIOME_MIN_BITS`] (2), **not**
/// the disk constant — see that constant's docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedContainer {
    /// Global palette ids present in this container.
    pub palette: Vec<u32>,
    /// One palette index per cell, in vanilla scan order.
    pub values: Vec<u32>,
    /// Bit width used on the wire (`0` selects the single-value form).
    pub bits: u32,
}

impl PalettedContainer {
    /// Wrap an existing `(palette, values)` pair, deriving the bit width.
    ///
    /// A container that is one value repeated — the common case for air, and for
    /// a biome section — is encoded as the **single-value** form: `bits = 0` with
    /// the one global palette id and **no** index array. That form is not a
    /// minimum-width choice, it is a different layout, which is why
    /// [`mc_core::packing::bits_for`]'s `max(min_bits, …)` result is
    /// ignored for it: a palette of one still yields a 4-bit width there, and a
    /// 4-bit width *does* carry an array.
    ///
    /// `min_bits` is [`mc_core::packing::BLOCK_MIN_BITS`] for block
    /// states and `BIOME_MIN_BITS` for biomes.
    #[must_use]
    pub fn new(palette: Vec<u32>, values: Vec<u32>, min_bits: u32) -> Self {
        let bits = Self::canonical_bits(&palette, min_bits);
        Self {
            palette,
            values,
            bits,
        }
    }

    /// The wire bit width for a palette: `0` for the single-value form,
    /// otherwise the minimum width for the palette length.
    fn canonical_bits(palette: &[u32], min_bits: u32) -> u32 {
        if palette.len() <= 1 {
            0
        } else {
            packing::bits_for(palette.len(), min_bits)
        }
    }

    /// Encode the container, returning the number of bytes written.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the palette or width is not encodable
    /// (server-authored data, so this is a bug rather than bad input).
    pub fn encode(&self, writer: &mut PacketWriter) -> ServerResult<usize> {
        if self.palette.len() > MAX_PALETTE_LEN {
            return Err(ServerError::Invariant(format!(
                "palette of {} entries exceeds the {MAX_PALETTE_LEN}-entry limit",
                self.palette.len()
            )));
        }
        if self.bits == 0 && self.palette.len() > 1 {
            return Err(ServerError::Invariant(format!(
                "single-value container with a {}-entry palette",
                self.palette.len()
            )));
        }
        let before = writer.len();
        writer.write_u8(self.bits as u8);
        if self.bits == 0 {
            // Single-value form: the whole container is one palette id, written
            // once, with no data array at all.
            let first = self.palette.first().copied().unwrap_or(0);
            writer.write_varint(first as i32);
            return Ok(writer.len() - before);
        }
        if self.palette.is_empty() {
            return Err(ServerError::Invariant(
                "indexed container with an empty palette".to_owned(),
            ));
        }
        writer.write_varint(packed_len(self.palette.len())?);
        for entry in &self.palette {
            writer.write_varint(*entry as i32);
        }
        // `writeFixedSizeLongArray`: raw longs, no length prefix.
        let packed = packing::pack(&self.values, self.bits)?;
        for long in &packed {
            writer.write_i64(*long);
        }
        Ok(writer.len() - before)
    }

    /// Decode a container with `entries` cells and `min_bits` minimum width.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for a truncated payload, an out-of-range
    /// palette length or bit width, a bit width that disagrees with the palette,
    /// or an index outside the palette.
    pub fn decode(
        reader: &mut PacketReader<'_>,
        entries: usize,
        min_bits: u32,
    ) -> ServerResult<Self> {
        let bits = u32::from(reader.read_u8()?);
        if bits == 0 {
            // Single-value form: one global palette id, no palette and no index array. `entries` is the whole
            // container, so every slot indexes the one palette entry — **index `0`, not the id it holds**. The
            // two coincide for air, whose id is `0`, which is why handing back the id went unnoticed until a
            // uniform section of something else (`palette=[86]`, `values=[86, ..]`) made `palette[values[i]]`
            // an out-of-range read.
            let value = reader.read_varint()?;
            if value < 0 {
                return Err(ServerError::Protocol(format!(
                    "single-value palette id {value} is negative"
                )));
            }
            return Ok(Self {
                palette: vec![value as u32],
                values: vec![0; entries],
                bits,
            });
        }
        if !(1..=packing::MAX_BITS).contains(&bits) {
            return Err(ServerError::Protocol(format!(
                "paletted container declares {bits} bits per entry"
            )));
        }
        let palette_len = read_count(reader, "palette", MAX_PALETTE_LEN)?;
        let mut palette = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            let id = reader.read_varint()?;
            if id < 0 {
                return Err(ServerError::Protocol(format!(
                    "palette entry {id} is negative"
                )));
            }
            palette.push(id as u32);
        }
        if palette.is_empty() {
            return Err(ServerError::Protocol(
                "paletted container has an empty palette but stores indices".to_owned(),
            ));
        }
        // Any indexed container has a palette of at least two, and its width must
        // be exactly the minimum for that length: `bits` is not an independent
        // field that a peer may choose to widen.
        let derived = Self::canonical_bits(&palette, min_bits);
        if bits != derived {
            return Err(ServerError::Protocol(format!(
                "paletted container declares {bits} bits for a {}-entry palette (expected {derived})",
                palette.len()
            )));
        }
        // `writeFixedSizeLongArray`: the count is not on the wire, it is derived
        // from the width and the container's cell count.
        let longs = packing::longs_needed(entries, bits);
        let mut data = Vec::with_capacity(longs);
        for _ in 0..longs {
            data.push(reader.read_i64()?);
        }
        let values = packing::unpack(&data, bits, entries).map_err(packing_error)?;
        for value in &values {
            if *value as usize >= palette.len() {
                return Err(ServerError::Protocol(format!(
                    "palette index {value} is outside a {}-entry palette",
                    palette.len()
                )));
            }
        }
        Ok(Self {
            palette,
            values,
            bits,
        })
    }
}

/// One 16×16×16 chunk section as it appears inside `chunk data`.
///
/// `LevelChunkSection.write`, verified from the jar
/// (`docs/protocol/chunk-wire-format.md` section 4):
///
/// ```text
/// i16  non-empty block count
/// i16  fluid count
/// <PalettedContainer> block states  (4096 cells)
/// <PalettedContainer> biomes        (64 cells)
/// ```
///
/// The **fluid count** is easy to miss: an earlier brief omitted it, and a
/// section without it desynchronises every following byte of the chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSection {
    /// Non-empty (non-air) block count, `0..=4096`.
    pub block_count: i16,
    /// Non-empty fluid count, `0..=4096`.
    pub fluid_count: i16,
    /// Block states, [`BLOCKS_PER_SECTION`] cells.
    pub block_states: PalettedContainer,
    /// Biomes, [`BIOMES_PER_SECTION`] cells.
    pub biomes: PalettedContainer,
}

/// One heightmap: a `Heightmap.Types` id plus its packed columns.
///
/// The wire carries an integer id, not a name
/// (`ByteBufCodecs.idMapper(BY_ID, Types::id)` → `VarInt.write(buf, Types::id)`),
/// so the constants below are the ids the client resolves. They come from
/// `docs/protocol/heightmap-types.tsv`, read from `Heightmap$Types.class` in
/// `server-26.1.2.jar` (sha1 `83eb1106…`; the `97ccd4c0…` hash in the task
/// header is the *outer* bundler jar, and the TSV records that the inner jar is
/// byte-identical to the bundler's own copy).
///
/// Note there are exactly six types and **no** `LIGHT_BLOCKING`: that identifier
/// exists only as a legacy on-disk key written by `HeightmapRenamingFix`.
/// `kind` is kept as a raw `i32` so an unknown id round-trips instead of being
/// dropped — the client itself decodes out-of-range ids leniently to id 0
/// (`ByIdMap.OutOfBoundsStrategy.ZERO`), so a peer may legally send one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heightmap {
    /// `Heightmap.Types` id, see [`HEIGHTMAP_WORLD_SURFACE`] and friends.
    pub kind: i32,
    /// Packed height columns. The count **is** on the wire for heightmaps
    /// (`ByteBufCodecs.LONG_ARRAY` → `FriendlyByteBuf.writeLongArray`), unlike a
    /// paletted container's storage array.
    pub data: Vec<i64>,
}

/// `Heightmap.Types` id for `WORLD_SURFACE_WG`.
pub const HEIGHTMAP_WORLD_SURFACE_WG: i32 = 0;

/// `Heightmap.Types` id for `WORLD_SURFACE` — the type Phase 04 sends.
pub const HEIGHTMAP_WORLD_SURFACE: i32 = 1;

/// `Heightmap.Types` id for `OCEAN_FLOOR_WG`.
pub const HEIGHTMAP_OCEAN_FLOOR_WG: i32 = 2;

/// `Heightmap.Types` id for `OCEAN_FLOOR`.
pub const HEIGHTMAP_OCEAN_FLOOR: i32 = 3;

/// `Heightmap.Types` id for `MOTION_BLOCKING`.
pub const HEIGHTMAP_MOTION_BLOCKING: i32 = 4;

/// `Heightmap.Types` id for `MOTION_BLOCKING_NO_LEAVES`.
pub const HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES: i32 = 5;

/// Sections in an overworld chunk column (384 blocks / 16).
///
/// The section blob has **no** count prefix, so this is what a decoder needs to
/// know how many sections to read; a taller or shorter dimension supplies its
/// own value to [`LevelChunkWithLight::decode_chunk_data`].
pub const OVERWORLD_SECTIONS: usize = 24;

/// A `LevelChunkWithLight` payload (clientbound, play).
///
/// ```text
/// i32     chunk_x
/// i32     chunk_z
/// VarInt  heightmap count
///           per entry: VarInt Heightmap.Types id
///                      VarInt long count
///                      i64    × count              (raw)
/// VarInt  data size (bytes)
/// byte[]  chunk data:
///           per section (fixed count, no prefix):
///             i16 block_count, i16 fluid_count,
///             block states container, biomes container
/// VarInt  block_entity count
///           per entry: u16 packed xz, u16 y, VarInt type id, NBT data
/// VarInt  sky light mask          \
/// VarInt  block light mask         |  bit i ↔ light section i - 1
/// VarInt  empty sky light mask     |
/// VarInt  empty block light mask  /
/// VarInt  sky light array count
///           per array: VarInt 2048, byte[2048]
/// VarInt  block light array count
///           per array: VarInt 2048, byte[2048]
/// ```
///
/// Verified from the jar — see `docs/protocol/chunk-wire-format.md` for the
/// class and method behind every line. Three things the earlier description got
/// wrong and this implementation follows the jar on: the section blob has no
/// count prefix, each section carries a fluid count after the block count, and
/// the heightmaps are a numeric map rather than an NBT compound.
///
/// The light *masks* are not derivable from the arrays (a set bit with no array
/// means "fully lit", a set bit in an *empty* mask means "fully dark"), so they
/// are carried verbatim and checked for consistency against the array counts:
/// the arrays carry no section index of their own, so a mismatch is
/// unrecoverable rather than cosmetic. Phase 04 sends all four masks and both
/// array counts as `0`, which leaves the chunk dark until a `light_update`
/// arrives — recorded as a known limitation, not hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    /// Chunk x (in chunks).
    pub chunk_x: i32,
    /// Chunk z (in chunks).
    pub chunk_z: i32,
    /// Heightmaps in wire order. A `Map` on the wire, so a repeated id is
    /// possible and preserved here rather than collapsed.
    pub heightmaps: Vec<Heightmap>,
    /// Sections bottom-to-top, including empty ones.
    pub sections: Vec<ChunkSection>,
    /// Block entities in this chunk.
    pub block_entities: Vec<ChunkBlockEntity>,
    /// Light sections whose sky light an array follows for, as **set indices**.
    ///
    /// These four are `java.util.BitSet` on the wire (P10-03, KD-44): a `VarInt` count of longs followed by
    /// that many `i64`s. They were read as `VarInt`s, which survives an empty mask — `0` in both encodings —
    /// and silently misreads any mask with content, which is why every captured vanilla chunk failed to parse.
    ///
    /// Index `i` is light section `i`, which is world section `i - 1`. Sorted, deduplicated.
    pub sky_light_mask: Vec<u32>,
    /// Light sections whose block light an array follows for. See [`Self::sky_light_mask`].
    pub block_light_mask: Vec<u32>,
    /// Light sections that are uniformly sky-lit, so need no array.
    pub empty_sky_light_mask: Vec<u32>,
    /// Light sections that are uniformly dark, so need no array.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

/// Encode a heightmap list (`VarInt` count, then id + length + longs each).
///
/// # Errors
///
/// [`ServerError::Invariant`] when a list length does not fit a `VarInt`.
fn encode_heightmaps(writer: &mut PacketWriter, heightmaps: &[Heightmap]) -> ServerResult<()> {
    writer.write_varint(packed_len(heightmaps.len())?);
    for heightmap in heightmaps {
        writer.write_varint(heightmap.kind);
        writer.write_varint(packed_len(heightmap.data.len())?);
        for long in &heightmap.data {
            writer.write_i64(*long);
        }
    }
    Ok(())
}

/// Decode a heightmap list.
///
/// # Errors
///
/// [`ServerError::Protocol`] on a hostile entry or long count, or truncation.
fn decode_heightmaps(reader: &mut PacketReader<'_>) -> ServerResult<Vec<Heightmap>> {
    let count = read_count(reader, "heightmap", MAX_HEIGHTMAPS)?;
    let mut heightmaps = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = reader.read_varint()?;
        let long_count = read_count(reader, "heightmap long", MAX_HEIGHTMAP_LONGS)?;
        let mut data = Vec::with_capacity(long_count);
        for _ in 0..long_count {
            data.push(reader.read_i64()?);
        }
        heightmaps.push(Heightmap { kind, data });
    }
    Ok(heightmaps)
}

impl LevelChunkWithLight {
    /// Encode the `chunk data` blob (sections only), returning the bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when a section's containers do not have the
    /// exact cell count the format requires, or a count is out of `0..=4096`.
    pub fn encode_chunk_data(sections: &[ChunkSection]) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        for section in sections {
            // A bad count here is server-authored data, so the shared check's
            // `Protocol` verdict is re-labelled as the invariant violation it is.
            validate_section_counts(section)
                .map_err(|error| ServerError::Invariant(error.to_string()))?;
            writer.write_i16(section.block_count);
            writer.write_i16(section.fluid_count);
            section.block_states.encode(&mut writer)?;
            section.biomes.encode(&mut writer)?;
        }
        Ok(writer.finish())
    }

    /// Decode a `chunk data` blob holding exactly `sections_per_chunk` sections.
    ///
    /// The count is a parameter because the blob does not carry one: it is
    /// implied by the dimension's height ([`OVERWORLD_SECTIONS`] for the
    /// overworld). A blob that does not end exactly at the last section is
    /// rejected rather than partially accepted.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on a hostile block/fluid count or container, a
    /// truncated blob, or trailing bytes.
    pub fn decode_chunk_data(
        bytes: &[u8],
        sections_per_chunk: usize,
    ) -> ServerResult<Vec<ChunkSection>> {
        if sections_per_chunk > MAX_CHUNK_SECTIONS {
            return Err(ServerError::Protocol(format!(
                "chunk declares {sections_per_chunk} sections, above the \
                 {MAX_CHUNK_SECTIONS}-section limit"
            )));
        }
        let mut reader = PacketReader::new(bytes);
        let mut sections = Vec::with_capacity(sections_per_chunk);
        for _ in 0..sections_per_chunk {
            let block_count = reader.read_i16()?;
            let fluid_count = reader.read_i16()?;
            let section = ChunkSection {
                block_count,
                fluid_count,
                block_states: PalettedContainer::decode(
                    &mut reader,
                    BLOCKS_PER_SECTION,
                    packing::BLOCK_MIN_BITS,
                )?,
                biomes: PalettedContainer::decode(
                    &mut reader,
                    BIOMES_PER_SECTION,
                    NETWORK_BIOME_MIN_BITS,
                )?,
            };
            validate_section_counts(&section)
                .map_err(|error| ServerError::Protocol(error.to_string()))?;
            sections.push(section);
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "chunk data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(sections)
    }
}

/// Check the two section counters and the container cell counts.
///
/// Used by both directions: the counters are `i16` on the wire but describe a
/// 4096-block volume, so out-of-range values are malformed in either direction.
fn validate_section_counts(section: &ChunkSection) -> ServerResult<()> {
    let max = BLOCKS_PER_SECTION as i16;
    if !(0..=max).contains(&section.block_count) {
        return Err(ServerError::Protocol(format!(
            "section block count {} is outside 0..={max}",
            section.block_count
        )));
    }
    if !(0..=max).contains(&section.fluid_count) {
        return Err(ServerError::Protocol(format!(
            "section fluid count {} is outside 0..={max}",
            section.fluid_count
        )));
    }
    if section.block_states.values.len() != BLOCKS_PER_SECTION {
        return Err(ServerError::Protocol(format!(
            "block states hold {} cells, expected {BLOCKS_PER_SECTION}",
            section.block_states.values.len()
        )));
    }
    if section.biomes.values.len() != BIOMES_PER_SECTION {
        return Err(ServerError::Protocol(format!(
            "biomes hold {} cells, expected {BIOMES_PER_SECTION}",
            section.biomes.values.len()
        )));
    }
    Ok(())
}

impl Packet for LevelChunkWithLight {
    const ID: i32 = clientbound::play::LEVEL_CHUNK_WITH_LIGHT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let chunk_x = reader.read_i32()?;
        let chunk_z = reader.read_i32()?;
        let heightmaps = decode_heightmaps(&mut reader)?;
        let size = reader.read_varint()?;
        if size < 0 {
            return Err(ServerError::Protocol(format!(
                "chunk data size {size} is negative"
            )));
        }
        let data = reader.read_bytes(size as usize)?;
        // The blob has no section count of its own; the dimension supplies it.
        let sections = Self::decode_chunk_data(data, OVERWORLD_SECTIONS)?;
        let block_entity_count = read_count(&mut reader, "block entity", MAX_BLOCK_ENTITIES)?;
        let mut block_entities = Vec::with_capacity(block_entity_count);
        for _ in 0..block_entity_count {
            let packed_xz = reader.read_u8()?;
            let y = reader.read_u16()?;
            let type_id = reader.read_varint()?;
            let mut rest = reader.remaining_slice();
            let data = Nbt::read_network(&mut rest)?;
            let consumed = reader.remaining() - rest.len();
            reader.advance(consumed)?;
            block_entities.push(ChunkBlockEntity {
                packed_xz,
                y,
                type_id,
                data,
            });
        }
        let light = read_light_data(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "level_chunk_with_light has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            heightmaps,
            sections,
            block_entities,
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
        writer.write_i32(self.chunk_x);
        writer.write_i32(self.chunk_z);
        encode_heightmaps(&mut writer, &self.heightmaps)?;
        let data = Self::encode_chunk_data(&self.sections)?;
        writer.write_varint(packed_len(data.len())?);
        writer.write_bytes(&data);
        encode_block_entities(&mut writer, &self.block_entities)?;
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

/// Append the block-entity list (`count`, then `u16 xz`, `u16 y`, `VarInt` type
/// id and nameless NBT per entry).
fn encode_block_entities(
    writer: &mut PacketWriter,
    block_entities: &[ChunkBlockEntity],
) -> ServerResult<()> {
    writer.write_varint(packed_len(block_entities.len())?);
    let mut nbt_bytes = Vec::new();
    for entity in block_entities {
        writer.write_u8(entity.packed_xz);
        writer.write_u16(entity.y);
        writer.write_varint(entity.type_id);
        nbt_bytes.clear();
        entity.data.write_network(&mut nbt_bytes)?;
        writer.write_bytes(&nbt_bytes);
    }
    Ok(())
}

/// Acknowledge the client's predicted block changes up to a sequence
/// (`minecraft:block_changed_ack`, clientbound play 4).
///
/// # Why this packet exists, and what its absence cost
///
/// A 26.x client does not apply a `block_update` straight to its level. Mining
/// opens a **prediction**: `MultiPlayerGameMode.startDestroyBlock` calls
/// `startPrediction`, which raises `BlockStatePredictionHandler.currentSequenceNr`
/// and sends `player_action` carrying that number, having first retained the
/// server-known state for the position.
///
/// The client then routes every server block change through
/// `ClientLevel.setServerVerifiedBlockState`, which is (bytecode, client jar):
///
/// ```text
/// if (!this.blockStatePredictionHandler.updateKnownServerState(pos, state)) {
///     super.setBlock(pos, state, flags, 512);
/// }
/// ```
///
/// `updateKnownServerState` **returns true whenever a prediction is pending at
/// that position** — and then the state is only *stored*, never applied. The
/// pending entry is cleared by exactly one thing: `endPredictionsUpTo(sequence)`,
/// which `ClientPacketListener.handleBlockChangedAck` calls with the sequence
/// from this packet.
///
/// So a server that never sends `block_changed_ack` never lets the client finish a
/// prediction. The block the player mined stays stone on screen, and **every later
/// server change at that position is swallowed too** — a permanent, per-position
/// freeze. That was the real cause of the owner's "survival mining does not break
/// blocks" report; the earlier reading of the capture (that the dig packets were
/// malformed) was an artefact of counting the packet-id byte as part of the body.
///
/// The vanilla server's rule, from `javap -c -p` on
/// `net.minecraft.server.network.ServerGamePacketListenerImpl`:
///
/// ```text
/// ackBlockChangesUpTo(int sequence):
///     if (sequence < 0) throw new IllegalArgumentException("Expected packet sequence nr >= 0");
///     this.ackBlockChangesUpTo = Math.max(sequence, this.ackBlockChangesUpTo);
/// tick():
///     if (this.ackBlockChangesUpTo > -1) {
///         send(new ClientboundBlockChangedAckPacket(this.ackBlockChangesUpTo));
///         this.ackBlockChangesUpTo = -1;
///     }
/// ```
///
/// and it is fed from `handlePlayerAction`, `handleUseItemOn` and `handleUseItem` —
/// the three serverbound packets that carry a sequence. Hence the high-water mark
/// rather than "ack each": the client only ever needs to know how far the server
/// has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChangedAck {
    /// Highest prediction sequence the server has processed for this connection.
    pub sequence: i32,
}

impl Packet for BlockChangedAck {
    /// The clientbound play id, cross-checked against the shipped table: the
    /// client jar's registration order and `docs/protocol/packet-ids-775.tsv`
    /// both put `block_changed_ack` at **4**.
    const ID: i32 = clientbound::play::BLOCK_CHANGED_ACK;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let sequence = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_changed_ack has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { sequence })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        // A single VarInt, and nothing else: the packet is one field wide.
        writer.write_varint(self.sequence);
        Ok(writer.finish())
    }
}

/// A single block change (`minecraft:block_update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpdate {
    /// Packed block position, see [`block_position`].
    pub position: i64,
    /// Global block state id.
    pub block_state: i32,
}

impl Packet for BlockUpdate {
    const ID: i32 = clientbound::play::BLOCK_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let position = reader.read_i64()?;
        let block_state = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            position,
            block_state,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.position);
        writer.write_varint(self.block_state);
        Ok(writer.finish())
    }
}

/// A block-crack overlay stage (`minecraft:block_destruction`, P16-05).
///
/// Wire shape corroborated by pumpkin's `CSetBlockDestroyStage`: `VarInt`
/// entity id (usually the miner's), packed block position, `i8` stage.
/// Stages run 0–9 with progress; any other value (vanilla sends -1) clears
/// the overlay. Id 5 in both the shipped table and pumpkin's packet enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDestruction {
    /// Miner's entity id: overlays are keyed per miner, so two players can
    /// crack the same block independently.
    pub entity_id: i32,
    /// Packed block position, see [`block_position`].
    pub position: i64,
    /// Crack stage 0–9, or anything else to clear.
    pub stage: i8,
}

impl Packet for BlockDestruction {
    const ID: i32 = clientbound::play::BLOCK_DESTRUCTION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            position: reader.read_i64()?,
            stage: reader.read_i8()?,
        };
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_destruction has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i64(self.position);
        writer.write_i8(self.stage);
        Ok(writer.finish())
    }
}

/// A batch of block changes in one section (`minecraft:section_blocks_update`).
///
/// Vanilla packs many block changes into one packet, which maps cleanly onto a
/// tick's worth of edits to a single section. The leading count is the only
/// structure: every entry has the same two fields as [`BlockUpdate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionBlocksUpdate {
    /// Changes, each as a packed position plus a global block state id.
    pub updates: Vec<BlockUpdate>,
}

/// Largest number of block changes accepted in one [`SectionBlocksUpdate`].
///
/// A section holds [`BLOCKS_PER_SECTION`] blocks, so a larger batch cannot
/// describe a real change set; the cap also bounds the decode loop before the
/// frame-size cap would.
pub const MAX_SECTION_UPDATES: usize = BLOCKS_PER_SECTION;

impl Packet for SectionBlocksUpdate {
    const ID: i32 = clientbound::play::SECTION_BLOCKS_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let count = read_count(&mut reader, "section update", MAX_SECTION_UPDATES)?;
        let mut updates = Vec::with_capacity(count);
        for _ in 0..count {
            updates.push(BlockUpdate {
                position: reader.read_i64()?,
                block_state: reader.read_varint()?,
            });
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "section_blocks_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { updates })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(packed_len(self.updates.len())?);
        for update in &self.updates {
            writer.write_i64(update.position);
            writer.write_varint(update.block_state);
        }
        Ok(writer.finish())
    }
}

/// `minecraft:chunk_batch_finished` (clientbound 11): the number of chunks
/// in the batch that just finished.
///
/// Shape corroborated three ways: twelve 1-byte capture bodies reading 5..13
/// (plausible batch sizes, and a multi-byte `VarInt` would show longer
/// bodies), pumpkin's `CChunkBatchEnd.batch_size: VarInt`, and the jar id
/// table. This server sends no batches, so the type is decode-side only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkBatchFinished {
    /// Chunks in the finished batch.
    pub batch_size: i32,
}

impl Packet for ChunkBatchFinished {
    const ID: i32 = clientbound::play::CHUNK_BATCH_FINISHED;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let batch_size = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "chunk_batch_finished has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { batch_size })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.batch_size);
        Ok(writer.finish())
    }
}

/// `minecraft:chunk_batch_start` (clientbound 12): an empty signal opening a
/// chunk batch. Eight captured bodies, every one empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChunkBatchStart;

impl Packet for ChunkBatchStart {
    const ID: i32 = clientbound::play::CHUNK_BATCH_START;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if !payload.is_empty() {
            return Err(ServerError::Protocol(format!(
                "chunk_batch_start must be empty, got {} bytes",
                payload.len()
            )));
        }
        Ok(Self)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(Vec::new())
    }
}
