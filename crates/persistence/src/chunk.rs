//! Chunk schema: the disk boundary between persistence and world state (P03-10).
//!
//! [`ChunkData`] is *not* the runtime chunk. It is the serialization contract:
//! Phase 04's `mc-world` chunk converts to and from it, so the disk schema can
//! change (or a legacy field be tolerated) without touching gameplay code, and
//! gameplay refactors cannot silently change the file format
//! (`prompts/PHASE-03.md`: "do not tie persistence format directly to in-memory
//! structs").
//!
//! Field set measured on a real 26.1.2 `minecraft:full` chunk:
//!
//! ```text
//! { `DataVersion`, xPos, zPos, yPos, Status, LastUpdate, InhabitedTime,
//!   sections: [ { Y, block_states: {palette, data?}, biomes: {palette, data?},
//!                 BlockLight?, SkyLight? } ],
//!   Heightmaps: { MOTION_BLOCKING, MOTION_BLOCKING_NO_LEAVES, OCEAN_FLOOR,
//!                 WORLD_SURFACE },
//!   structures, block_entities, entities, block_ticks, fluid_ticks,
//!   PostProcessing, isLightOn }
//! ```
//!
//! Anything this model does not interpret (for example the transient
//! `carving_mask` of a partially generated chunk) is preserved in
//! [`ChunkData::extra`] and written back unchanged, so loading and re-saving a
//! vanilla chunk never silently drops data.

use crate::dimension::Dimension;
use mc_core::error::{ServerError, ServerResult};
use mc_core::packing::{
    BIOME_ENTRIES, BIOME_MIN_BITS, BLOCK_ENTRIES, BLOCK_MIN_BITS, bits_for, pack, unpack,
};
use mc_nbt::{Limits, NbtTag};
use tracing::warn;

/// Chunk coordinates (block coordinates divided by 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    /// Chunk x.
    pub x: i32,
    /// Chunk z.
    pub z: i32,
}

/// Chunks per region axis.
pub const REGION_CHUNKS: i32 = 32;

impl ChunkPos {
    /// Construct a chunk position.
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Region coordinate containing this chunk (`x >> 5`).
    #[must_use]
    pub const fn region_x(self) -> i32 {
        self.x >> 5
    }

    /// Region coordinate containing this chunk (`z >> 5`).
    #[must_use]
    pub const fn region_z(self) -> i32 {
        self.z >> 5
    }

    /// Slot inside the region file: `local_x + local_z * 32`, with negative
    /// coordinates wrapped (`-1` is local 31), matching `ChunkPos.getRegionLocalX`.
    #[must_use]
    pub const fn slot(self) -> usize {
        ((self.x & 31) + (self.z & 31) * REGION_CHUNKS) as usize
    }

    /// Whether the two coordinates live in the same region file.
    #[must_use]
    pub const fn same_region(self, other: Self) -> bool {
        self.region_x() == other.region_x() && self.region_z() == other.region_z()
    }
}

/// One block state: registry name plus optional properties.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockState {
    /// Registry name, e.g. `minecraft:oak_log`.
    pub name: String,
    /// Property key/value pairs, in file order.
    pub properties: Vec<(String, String)>,
}

impl BlockState {
    /// A property-less block state.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            properties: Vec::new(),
        }
    }

    /// Build from `{Name, Properties}` NBT.
    fn from_nbt(tag: &NbtTag) -> ServerResult<Self> {
        let name = tag
            .get_str("Name")
            .ok_or_else(|| ServerError::CorruptData("palette entry has no Name".to_owned()))?;
        let properties = tag
            .get_compound("Properties")
            .and_then(NbtTag::entries)
            .unwrap_or(&[])
            .iter()
            .map(|(key, value)| match value {
                NbtTag::String(text) => Ok((key.clone(), text.clone())),
                other => Err(ServerError::CorruptData(format!(
                    "block state property {key:?} is {}, expected String",
                    other.type_name()
                ))),
            })
            .collect::<ServerResult<Vec<_>>>()?;
        Ok(Self {
            name: name.to_owned(),
            properties,
        })
    }

    /// Encode as `{Name, Properties?}`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtTag {
        let mut entries = vec![("Name".to_owned(), NbtTag::String(self.name.clone()))];
        if !self.properties.is_empty() {
            entries.push((
                "Properties".to_owned(),
                NbtTag::Compound(
                    self.properties
                        .iter()
                        .map(|(key, value)| (key.clone(), NbtTag::String(value.clone())))
                        .collect(),
                ),
            ));
        }
        NbtTag::Compound(entries)
    }
}

/// A palette plus packed indices for a fixed number of container entries.
///
/// `bits` records the width the current `data` array is packed at, which is not
/// always the width the palette length implies: Vanilla writes the array with
/// the width that was current when the chunk was saved, and a reader must use
/// that same width to unpack it.
#[derive(Debug, Clone, PartialEq)]
pub struct PalettedContainer<T> {
    palette: Vec<T>,
    /// `None` while the palette holds a single value (Vanilla omits `data`).
    data: Option<Vec<i64>>,
    /// Width the packed array uses; 0 while there is no array.
    bits: u32,
    entries: usize,
    min_bits: u32,
}

impl<T: Clone + PartialEq> PalettedContainer<T> {
    /// A container filled with one value.
    #[must_use]
    pub fn single(value: T, entries: usize, min_bits: u32) -> Self {
        Self {
            palette: vec![value],
            data: None,
            bits: 0,
            entries,
            min_bits,
        }
    }

    /// Build from a decoded palette and packed array.
    ///
    /// `bits` is derived from the palette length, matching Vanilla: the width is
    /// not stored in the file, so a reader must recompute it. `data` may be
    /// longer than needed (Vanilla tolerates that), but never shorter.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the palette is empty or larger than the
    /// container, when the packed array is too short, or when an index points
    /// outside the palette.
    pub fn from_parts(
        palette: Vec<T>,
        data: Option<Vec<i64>>,
        entries: usize,
        min_bits: u32,
    ) -> ServerResult<Self> {
        if palette.is_empty() {
            return Err(ServerError::CorruptData(
                "paletted container has an empty palette".to_owned(),
            ));
        }
        if palette.len() > entries {
            return Err(ServerError::CorruptData(format!(
                "paletted container has {entries} entries but a palette of {}",
                palette.len()
            )));
        }
        let bits = bits_for(palette.len(), min_bits);
        if let Some(data) = &data {
            let values = unpack(data, bits, entries)?;
            if let Some(bad) = values.iter().find(|v| **v as usize >= palette.len()) {
                return Err(ServerError::CorruptData(format!(
                    "paletted container index {bad} is outside a palette of {}",
                    palette.len()
                )));
            }
        }
        Ok(Self {
            palette,
            bits: if data.is_some() { bits } else { 0 },
            data,
            entries,
            min_bits,
        })
    }

    /// Build a container from a full value list, packed the way Vanilla would: the
    /// palette holds the distinct values in first-appearance order and the bit
    /// width follows the palette length.
    ///
    /// This is the writer's entry point when the values came from gameplay rather
    /// than from a file. `values.len()` must equal the container's entry count
    /// (4096 for blocks, 64 for biomes).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when `values` is empty or its length is not the
    /// container's entry count, and [`ServerError::Invariant`] when the implied bit
    /// width is unusable.
    pub fn from_values(values: Vec<T>, entries: usize, min_bits: u32) -> ServerResult<Self> {
        if values.is_empty() {
            return Err(ServerError::CorruptData(
                "paletted container needs at least one value".to_owned(),
            ));
        }
        if values.len() != entries {
            return Err(ServerError::CorruptData(format!(
                "paletted container got {} values for a {entries}-entry container",
                values.len()
            )));
        }
        let mut palette: Vec<T> = Vec::new();
        let mut indices: Vec<u32> = Vec::with_capacity(values.len());
        for value in values {
            let index = if let Some(found) = palette.iter().position(|entry| *entry == value) {
                found
            } else {
                palette.push(value);
                palette.len() - 1
            };
            indices.push(index as u32);
        }
        // `palette` is non-empty because `values` is, so the single-value case can
        // hand the only entry straight to `single`.
        if let [only] = palette.as_slice() {
            return Ok(Self::single(only.clone(), entries, min_bits));
        }
        let bits = bits_for(palette.len(), min_bits);
        let data = pack(&indices, bits)?;
        Self::from_parts(palette, Some(data), entries, min_bits)
    }

    /// Bit width the values are packed at.
    ///
    /// For a container with no packed array this reports the width the palette
    /// *would* imply, which is what a writer needs.
    #[must_use]
    pub fn bits(&self) -> u32 {
        if self.data.is_some() {
            self.bits
        } else {
            bits_for(self.palette.len(), self.min_bits)
        }
    }

    /// Entry count of the container.
    #[must_use]
    pub const fn entries(&self) -> usize {
        self.entries
    }

    /// The palette.
    #[must_use]
    pub fn palette(&self) -> &[T] {
        &self.palette
    }

    /// The packed array, if the palette has more than one value.
    #[must_use]
    pub fn data(&self) -> Option<&[i64]> {
        self.data.as_deref()
    }

    /// Index at `index`, or `None` when out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.entries {
            return None;
        }
        let palette_index = match &self.data {
            None => 0,
            Some(data) => {
                let bits = self.bits;
                let per_long = mc_core::packing::values_per_long(bits);
                let slot = index / per_long;
                let offset = ((index % per_long) as u32) * bits;
                let mask = (1u64 << bits) - 1;
                let raw = (*data.get(slot)? as u64 >> offset) & mask;
                usize::try_from(raw).ok()?
            }
        };
        self.palette.get(palette_index)
    }

    /// Set the value at `index`, extending the palette when needed.
    ///
    /// Growing the palette can widen the bit width; when it does, the existing
    /// values are re-packed at the new width first. Skipping that step would
    /// silently mix two widths in one array — a corruption the restart tests
    /// catch (`paletted container index N is outside a palette of M`).
    ///
    /// Out-of-range indices are ignored (this is the persistence boundary, not a
    /// gameplay API; callers validate coordinates first).
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when re-packing fails, which cannot happen for
    /// a palette within the container's entry count.
    pub fn set(&mut self, index: usize, value: T) -> ServerResult<()> {
        if index >= self.entries {
            return Ok(());
        }
        let palette_index = if let Some(found) = self.palette.iter().position(|i| *i == value) {
            found
        } else {
            self.palette.push(value);
            self.palette.len() - 1
        };
        let target_bits = bits_for(self.palette.len(), self.min_bits);
        match self.data.take() {
            None => {
                // Materialise the array on the first write. Every existing entry
                // is palette index 0, so zeros are the correct initial content.
                self.bits = target_bits;
                let needed = mc_core::packing::longs_needed(self.entries, target_bits);
                self.data = Some(vec![0i64; needed]);
            }
            Some(data) => {
                if target_bits == self.bits {
                    self.data = Some(data);
                } else {
                    let values = unpack(&data, self.bits, self.entries)?;
                    self.bits = target_bits;
                    self.data = Some(pack(&values, target_bits)?);
                }
            }
        }
        let bits = self.bits;
        let per_long = mc_core::packing::values_per_long(bits);
        let needed = mc_core::packing::longs_needed(self.entries, bits);
        let data = self.data.get_or_insert_with(|| vec![0i64; needed]);
        if data.len() < needed {
            data.resize(needed, 0);
        }
        let slot = index / per_long;
        let offset = ((index % per_long) as u32) * bits;
        let mask = (1u64 << bits) - 1;
        let cleared = (data[slot] as u64) & !(mask << offset);
        data[slot] = (cleared | ((palette_index as u64 & mask) << offset)) as i64;
        Ok(())
    }

    /// All entries in order (used by tests and by the P04 world loader).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the packed array is short or an index
    /// is outside the palette (already validated by
    /// [`PalettedContainer::from_parts`]).
    pub fn to_values(&self) -> ServerResult<Vec<T>> {
        match &self.data {
            None => Ok(vec![self.palette[0].clone(); self.entries]),
            Some(data) => Ok(unpack(data, self.bits, self.entries)?
                .into_iter()
                .map(|index| self.palette[index as usize].clone())
                .collect()),
        }
    }

    /// Serializable form: `(palette, data)` with `data` omitted for a
    /// single-value palette, exactly as Vanilla writes it.
    #[must_use]
    pub fn parts(&self) -> (&[T], Option<&[i64]>) {
        (&self.palette, self.data.as_deref())
    }

    /// Normalise the packed array so a container built in memory serialises the
    /// same way a Vanilla one would (single entry → no `data`, width from the
    /// palette length).
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the palette length implies an unusable
    /// bit width.
    pub fn repack(&mut self) -> ServerResult<()> {
        if self.palette.len() <= 1 {
            self.data = None;
            self.bits = 0;
            return Ok(());
        }
        let target_bits = bits_for(self.palette.len(), self.min_bits);
        let values: Vec<u32> = match &self.data {
            Some(data) => unpack(data, self.bits, self.entries)?,
            None => vec![0; self.entries],
        };
        self.bits = target_bits;
        self.data = Some(pack(&values, target_bits)?);
        Ok(())
    }
}

/// One 16³ chunk section.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionData {
    /// Section index in the world (`y >> 4`); 26.1 overworld spans -4..=19.
    pub y: i8,
    /// Block states (4096 entries).
    pub block_states: PalettedContainer<BlockState>,
    /// Biomes (64 entries), stored as biome ids.
    pub biomes: PalettedContainer<String>,
    /// Nibble-packed block light (2048 bytes), when present.
    pub block_light: Option<Vec<u8>>,
    /// Nibble-packed sky light (2048 bytes), when present.
    pub sky_light: Option<Vec<u8>>,
}

/// Bytes in a nibble-packed light array for one section (4096 blocks / 2).
pub const LIGHT_BYTES: usize = 2048;

impl SectionData {
    /// A section full of one block state and one biome.
    #[must_use]
    pub fn filled(y: i8, block: BlockState, biome: &str) -> Self {
        Self {
            y,
            block_states: PalettedContainer::single(block, BLOCK_ENTRIES, BLOCK_MIN_BITS),
            biomes: PalettedContainer::single(biome.to_owned(), BIOME_ENTRIES, BIOME_MIN_BITS),
            block_light: None,
            sky_light: None,
        }
    }

    fn from_nbt(tag: &NbtTag) -> ServerResult<Self> {
        let y = tag
            .get_i8("Y")
            .ok_or_else(|| ServerError::CorruptData("chunk section has no Y".to_owned()))?;
        let block_states = tag.get_compound("block_states").ok_or_else(|| {
            ServerError::CorruptData(format!("chunk section at Y={y} has no block_states"))
        })?;
        let blocks = decode_palette(
            block_states,
            BLOCK_ENTRIES,
            BLOCK_MIN_BITS,
            "block_states",
            BlockState::from_nbt,
        )?;
        let biomes = if let Some(biome_tag) = tag.get_compound("biomes") {
            decode_palette(
                biome_tag,
                BIOME_ENTRIES,
                BIOME_MIN_BITS,
                "biomes",
                |entry| match entry {
                    NbtTag::String(value) => Ok(value.clone()),
                    other => Err(ServerError::CorruptData(format!(
                        "biome palette entry is {}, expected String",
                        other.type_name()
                    ))),
                },
            )?
        } else {
            warn!(
                section_y = y,
                "chunk section has no biomes; defaulting to plains"
            );
            PalettedContainer::single("minecraft:plains".to_owned(), BIOME_ENTRIES, BIOME_MIN_BITS)
        };
        Ok(Self {
            y,
            block_states: blocks,
            biomes,
            block_light: decode_light(tag, "BlockLight", y)?,
            sky_light: decode_light(tag, "SkyLight", y)?,
        })
    }

    fn to_nbt(&self) -> NbtTag {
        let (palette, data) = self.block_states.parts();
        let mut block_states = vec![(
            "palette".to_owned(),
            NbtTag::List(palette.iter().map(BlockState::to_nbt).collect()),
        )];
        if let Some(data) = data {
            block_states.push(("data".to_owned(), NbtTag::LongArray(data.to_vec())));
        }

        let (biome_palette, biome_data) = self.biomes.parts();
        let mut biomes = vec![(
            "palette".to_owned(),
            NbtTag::List(
                biome_palette
                    .iter()
                    .map(|id| NbtTag::String(id.clone()))
                    .collect(),
            ),
        )];
        if let Some(data) = biome_data {
            biomes.push(("data".to_owned(), NbtTag::LongArray(data.to_vec())));
        }

        let mut entries = vec![("Y".to_owned(), NbtTag::Byte(self.y))];
        entries.push(("block_states".to_owned(), NbtTag::Compound(block_states)));
        entries.push(("biomes".to_owned(), NbtTag::Compound(biomes)));
        if let Some(light) = &self.block_light {
            entries.push(("BlockLight".to_owned(), NbtTag::ByteArray(light.clone())));
        }
        if let Some(light) = &self.sky_light {
            entries.push(("SkyLight".to_owned(), NbtTag::ByteArray(light.clone())));
        }
        NbtTag::Compound(entries)
    }
}

fn decode_palette<T: Clone + PartialEq>(
    container: &NbtTag,
    entries: usize,
    min_bits: u32,
    what: &str,
    decode: impl Fn(&NbtTag) -> ServerResult<T>,
) -> ServerResult<PalettedContainer<T>> {
    let palette_entries = container
        .get_list("palette")
        .ok_or_else(|| ServerError::CorruptData(format!("{what} has no palette list")))?;
    let palette = palette_entries
        .iter()
        .map(&decode)
        .collect::<ServerResult<Vec<T>>>()?;
    let data = container.get_long_array("data").map(<[i64]>::to_vec);
    PalettedContainer::from_parts(palette, data, entries, min_bits)
}

fn decode_light(tag: &NbtTag, key: &str, y: i8) -> ServerResult<Option<Vec<u8>>> {
    let Some(bytes) = tag.get_byte_array(key) else {
        return Ok(None);
    };
    if bytes.len() != LIGHT_BYTES {
        return Err(ServerError::CorruptData(format!(
            "chunk section Y={y} {key} has {} bytes, expected {LIGHT_BYTES}",
            bytes.len()
        )));
    }
    Ok(Some(bytes.to_vec()))
}

/// The decoded chunk schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkData {
    /// Chunk coordinates.
    pub pos: ChunkPos,
    /// `DataVersion` of the stored data.
    pub data_version: i32,
    /// Generation status, e.g. `minecraft:full`.
    pub status: String,
    /// `yPos`: the lowest section index present.
    pub min_section_y: i32,
    /// Tick of the last update to this chunk.
    pub last_update: i64,
    /// Ticks players have spent in this chunk.
    pub inhabited_time: i64,
    /// Whether the stored light data is authoritative (`isLightOn`).
    pub light_correct: bool,
    /// Sections, ordered by `y` after decoding.
    pub sections: Vec<SectionData>,
    /// Heightmaps: `(type name, packed 256-entry long array)`.
    pub heightmaps: Vec<(String, Vec<i64>)>,
    /// Block entities.
    pub block_entities: Vec<NbtTag>,
    /// Entities (stored per chunk since 1.17).
    pub entities: Vec<NbtTag>,
    /// Scheduled block ticks.
    pub block_ticks: Vec<NbtTag>,
    /// Scheduled fluid ticks.
    pub fluid_ticks: Vec<NbtTag>,
    /// Post-processing positions per section (`PostProcessing`).
    pub post_processing: Vec<Vec<i16>>,
    /// Structure starts and references (`structures`).
    pub structures: Option<NbtTag>,
    /// Uninterpreted top-level entries, preserved verbatim.
    pub extra: Vec<(String, NbtTag)>,
}

/// Top-level chunk entries this model interprets.
const KNOWN_CHUNK_FIELDS: [&str; 16] = [
    "DataVersion",
    "xPos",
    "zPos",
    "yPos",
    "Status",
    "LastUpdate",
    "InhabitedTime",
    "isLightOn",
    "sections",
    "Heightmaps",
    "block_entities",
    "entities",
    "block_ticks",
    "fluid_ticks",
    "PostProcessing",
    "structures",
];

impl ChunkData {
    /// An empty chunk with the given section range, ready to be filled.
    #[must_use]
    pub fn empty(pos: ChunkPos, min_section_y: i8, section_count: usize) -> Self {
        let sections = (0..section_count)
            .map(|index| {
                SectionData::filled(
                    min_section_y.wrapping_add(index as i8),
                    BlockState::new("minecraft:air"),
                    "minecraft:plains",
                )
            })
            .collect();
        Self {
            pos,
            data_version: crate::level::DATA_VERSION_26_1_2,
            status: "minecraft:full".to_owned(),
            min_section_y: i32::from(min_section_y),
            last_update: 0,
            inhabited_time: 0,
            light_correct: false,
            sections,
            heightmaps: Vec::new(),
            block_entities: Vec::new(),
            entities: Vec::new(),
            block_ticks: Vec::new(),
            fluid_ticks: Vec::new(),
            post_processing: Vec::new(),
            structures: None,
            extra: Vec::new(),
        }
    }

    /// Index of a section by its `y` value.
    #[must_use]
    pub fn section_index(&self, y: i8) -> Option<usize> {
        self.sections.iter().position(|section| section.y == y)
    }

    /// Number of sections that are not a single air block state.
    ///
    /// A cheap integrity signal used by the restart tests to prove a chunk
    /// survived a save/load cycle.
    #[must_use]
    pub fn non_empty_section_count(&self) -> usize {
        self.sections
            .iter()
            .filter(|section| {
                section.block_states.palette().len() > 1
                    || section
                        .block_states
                        .palette()
                        .first()
                        .is_some_and(|state| state.name != "minecraft:air")
            })
            .count()
    }

    /// Decode from a chunk's root NBT tag.
    ///
    /// # Errors
    ///
    /// - [`ServerError::CorruptData`] for a missing/ill-typed field, a malformed
    ///   palette, a light array of the wrong size, or a **pre-1.18** chunk
    ///   (`Level` compound instead of `sections`), which this crate refuses
    ///   explicitly because it ships no datafixers;
    /// - [`ServerError::Protocol`] for structurally truncated NBT.
    pub fn from_nbt(root: &NbtTag) -> ServerResult<Self> {
        if root.get_compound("Level").is_some() || root.contains("Blocks") {
            return Err(ServerError::CorruptData(
                "chunk uses the pre-1.18 layout (Level/Blocks); this build only reads \
                 1.21.9..=26.1.2 chunks"
                    .to_owned(),
            ));
        }
        let sections_nbt = root
            .get_list("sections")
            .ok_or_else(|| ServerError::CorruptData("chunk has no sections list".to_owned()))?;
        let mut sections = sections_nbt
            .iter()
            .map(SectionData::from_nbt)
            .collect::<ServerResult<Vec<_>>>()?;
        sections.sort_by_key(|section| section.y);

        let pos = ChunkPos::new(
            root.get_i32("xPos").unwrap_or(0),
            root.get_i32("zPos").unwrap_or(0),
        );
        let min_section_y = root
            .get_i32("yPos")
            .or_else(|| sections.first().map(|s| i32::from(s.y)))
            .unwrap_or(0);

        let heightmaps = root
            .get_compound("Heightmaps")
            .and_then(NbtTag::entries)
            .unwrap_or(&[])
            .iter()
            .filter_map(|(name, value)| match value {
                NbtTag::LongArray(values) => Some((name.clone(), values.clone())),
                other => {
                    warn!(
                        heightmap = name.as_str(),
                        tag = other.type_name(),
                        "ignoring non-long-array heightmap"
                    );
                    None
                }
            })
            .collect();

        let extra = root
            .entries()
            .unwrap_or(&[])
            .iter()
            .filter(|(key, _)| !KNOWN_CHUNK_FIELDS.contains(&key.as_str()))
            .cloned()
            .collect();

        Ok(Self {
            pos,
            data_version: root
                .get_i32("DataVersion")
                .unwrap_or(crate::level::DATA_VERSION_26_1_2),
            status: root
                .get_str("Status")
                .unwrap_or("minecraft:full")
                .to_owned(),
            min_section_y,
            last_update: root.get_i64("LastUpdate").unwrap_or(0),
            inhabited_time: root.get_i64("InhabitedTime").unwrap_or(0),
            light_correct: root.get_bool("isLightOn").unwrap_or(false),
            sections,
            heightmaps,
            block_entities: compound_list(root, "block_entities"),
            entities: compound_list(root, "entities"),
            block_ticks: compound_list(root, "block_ticks"),
            fluid_ticks: compound_list(root, "fluid_ticks"),
            post_processing: post_processing(root),
            structures: root.get("structures").cloned(),
            extra,
        })
    }

    /// Encode as the 26.1.2 chunk structure.
    #[must_use]
    pub fn to_nbt(&self) -> NbtTag {
        let mut entries: Vec<(String, NbtTag)> = vec![
            ("DataVersion".to_owned(), NbtTag::Int(self.data_version)),
            ("xPos".to_owned(), NbtTag::Int(self.pos.x)),
            ("zPos".to_owned(), NbtTag::Int(self.pos.z)),
            ("yPos".to_owned(), NbtTag::Int(self.min_section_y)),
            ("Status".to_owned(), NbtTag::String(self.status.clone())),
            ("LastUpdate".to_owned(), NbtTag::Long(self.last_update)),
            (
                "InhabitedTime".to_owned(),
                NbtTag::Long(self.inhabited_time),
            ),
            (
                "sections".to_owned(),
                NbtTag::List(self.sections.iter().map(SectionData::to_nbt).collect()),
            ),
        ];
        if !self.heightmaps.is_empty() {
            entries.push((
                "Heightmaps".to_owned(),
                NbtTag::Compound(
                    self.heightmaps
                        .iter()
                        .map(|(name, values)| (name.clone(), NbtTag::LongArray(values.clone())))
                        .collect(),
                ),
            ));
        }
        if let Some(structures) = &self.structures {
            entries.push(("structures".to_owned(), structures.clone()));
        }
        entries.push((
            "block_entities".to_owned(),
            NbtTag::List(self.block_entities.clone()),
        ));
        entries.push(("entities".to_owned(), NbtTag::List(self.entities.clone())));
        entries.push((
            "block_ticks".to_owned(),
            NbtTag::List(self.block_ticks.clone()),
        ));
        entries.push((
            "fluid_ticks".to_owned(),
            NbtTag::List(self.fluid_ticks.clone()),
        ));
        entries.push((
            "PostProcessing".to_owned(),
            NbtTag::List(
                self.post_processing
                    .iter()
                    .map(|positions| {
                        NbtTag::List(positions.iter().map(|p| NbtTag::Short(*p)).collect())
                    })
                    .collect(),
            ),
        ));
        if self.light_correct {
            entries.push(("isLightOn".to_owned(), NbtTag::Byte(1)));
        }
        entries.extend(self.extra.iter().cloned());
        NbtTag::Compound(entries)
    }

    /// Encode and serialise to bytes (uncompressed NBT).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a string is too long to encode.
    pub fn to_nbt_bytes(&self) -> ServerResult<Vec<u8>> {
        let mut out = Vec::new();
        mc_nbt::write_named("", &self.to_nbt(), &mut out)?;
        Ok(out)
    }

    /// Decode from bytes (uncompressed NBT) with the standard disk budgets.
    ///
    /// # Errors
    ///
    /// As for [`ChunkData::from_nbt`], plus budget violations.
    pub fn from_nbt_bytes(bytes: &[u8]) -> ServerResult<Self> {
        let (_, root) = mc_nbt::read_named(bytes, Limits::DISK)?;
        Self::from_nbt(&root)
    }

    /// Check that the chunk's own coordinates match the region slot it was read
    /// from, the way Vanilla warns about "chunk file at ... is in the wrong
    /// location".
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] on a mismatch.
    pub fn verify_position(&self, expected: ChunkPos) -> ServerResult<()> {
        if self.pos == expected {
            return Ok(());
        }
        Err(ServerError::CorruptData(format!(
            "chunk stored at ({}, {}) declares position ({}, {})",
            expected.x, expected.z, self.pos.x, self.pos.z
        )))
    }
}

fn compound_list(root: &NbtTag, key: &str) -> Vec<NbtTag> {
    root.get_list(key)
        .unwrap_or(&[])
        .iter()
        .filter(|entry| matches!(entry, NbtTag::Compound(_)))
        .cloned()
        .collect()
}

fn post_processing(root: &NbtTag) -> Vec<Vec<i16>> {
    root.get_list("PostProcessing")
        .unwrap_or(&[])
        .iter()
        .map(|section| match section {
            NbtTag::List(positions) => positions
                .iter()
                .filter_map(NbtTag::as_i64)
                .filter_map(|value| i16::try_from(value).ok())
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// The dimension a chunk belongs to travels with it for save ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkKey {
    /// Dimension owning the chunk.
    pub dimension: Dimension,
    /// Chunk coordinates.
    pub pos: ChunkPos,
}

impl ChunkKey {
    /// Build a key.
    #[must_use]
    pub fn new(dimension: &Dimension, pos: ChunkPos) -> Self {
        Self {
            dimension: dimension.clone(),
            pos,
        }
    }

    /// Region file this chunk lives in.
    #[must_use]
    pub const fn region(&self) -> (i32, i32) {
        (self.pos.region_x(), self.pos.region_z())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BIOME_ENTRIES, BLOCK_ENTRIES, BlockState, ChunkData, ChunkPos, LIGHT_BYTES, SectionData,
    };
    use mc_core::error::ServerError;
    use mc_nbt::NbtTag;

    #[test]
    fn chunk_positions_follow_vanilla_region_math() {
        assert_eq!(ChunkPos::new(0, 0).slot(), 0);
        assert_eq!(ChunkPos::new(31, 31).slot(), 31 * 32 + 31);
        // Negative coordinates wrap: -1 is local 31 of region -1.
        assert_eq!(ChunkPos::new(-1, -1).region_x(), -1);
        assert_eq!(ChunkPos::new(-1, -1).slot(), 31 * 32 + 31);
        assert_eq!(ChunkPos::new(-32, -32).region_x(), -1);
        assert_eq!(ChunkPos::new(-33, -33).region_x(), -2);
        assert_eq!(ChunkPos::new(-33, -33).slot(), 31 * 32 + 31);
        // The fixture chunk from the vanilla world: (-37, -24) sits at local
        // (27, 8) of region (-2, -1), i.e. slot 27 + 8 * 32 = 283 —the slot the
        // vanilla region file reports for it.
        assert_eq!(ChunkPos::new(-37, -24).region_x(), -2);
        assert_eq!(ChunkPos::new(-37, -24).region_z(), -1);
        assert_eq!(ChunkPos::new(-37, -24).slot(), 283);
        assert!(ChunkPos::new(-37, -24).same_region(ChunkPos::new(-33, -1)));
        assert!(!ChunkPos::new(-37, -24).same_region(ChunkPos::new(-65, -24)));
    }

    #[test]
    fn block_state_round_trips_with_properties() {
        let state = BlockState {
            name: "minecraft:oak_log".to_owned(),
            properties: vec![("axis".to_owned(), "y".to_owned())],
        };
        assert_eq!(
            BlockState::from_nbt(&state.to_nbt()).expect("decodes"),
            state
        );
        let bare = BlockState::new("minecraft:air");
        assert_eq!(BlockState::from_nbt(&bare.to_nbt()).expect("decodes"), bare);
        // Missing Name is an error, not a panic.
        assert!(BlockState::from_nbt(&NbtTag::Compound(vec![])).is_err());
        // Non-string property value is rejected.
        let bad = NbtTag::Compound(vec![
            (
                "Name".to_owned(),
                NbtTag::String("minecraft:stone".to_owned()),
            ),
            (
                "Properties".to_owned(),
                NbtTag::Compound(vec![("x".to_owned(), NbtTag::Int(1))]),
            ),
        ]);
        assert!(BlockState::from_nbt(&bad).is_err());
    }

    #[test]
    fn empty_chunk_serialises_a_vanilla_shaped_section() {
        let chunk = ChunkData::empty(ChunkPos::new(3, -7), -4, 24);
        let root = chunk.to_nbt();
        assert_eq!(root.get_i32("xPos"), Some(3));
        assert_eq!(root.get_i32("zPos"), Some(-7));
        assert_eq!(root.get_i32("yPos"), Some(-4));
        assert_eq!(root.get_str("Status"), Some("minecraft:full"));
        let sections = root.get_list("sections").expect("sections");
        assert_eq!(sections.len(), 24);
        let first = &sections[0];
        assert_eq!(first.get_i8("Y"), Some(-4));
        let block_states = first.get_compound("block_states").expect("block_states");
        assert_eq!(
            block_states.get_list("palette").map(<[_]>::len),
            Some(1),
            "single-value palette"
        );
        assert!(
            block_states.get_long_array("data").is_none(),
            "data must be omitted for a single-value palette"
        );
        assert_eq!(chunk.sections[23].y, 19);
    }

    #[test]
    fn empty_chunk_round_trips() {
        let chunk = ChunkData::empty(ChunkPos::new(-37, -24), -4, 24);
        let decoded = ChunkData::from_nbt(&chunk.to_nbt()).expect("decodes");
        assert_eq!(decoded, chunk);
        let bytes = chunk.to_nbt_bytes().expect("encodes");
        assert_eq!(ChunkData::from_nbt_bytes(&bytes).expect("decodes"), chunk);
    }

    #[test]
    fn chunk_with_content_round_trips() {
        let mut chunk = ChunkData::empty(ChunkPos::new(5, 5), -4, 24);
        let section = &mut chunk.sections[4];
        for index in 0..BLOCK_ENTRIES {
            let name = match index % 3 {
                0 => "minecraft:stone",
                1 => "minecraft:dirt",
                _ => "minecraft:air",
            };
            section
                .block_states
                .set(index, BlockState::new(name))
                .expect("set");
        }
        for index in 0..BIOME_ENTRIES {
            section
                .biomes
                .set(index, format!("minecraft:biome_{}", index % 2))
                .expect("set");
        }
        section.block_light = Some(vec![0x12; LIGHT_BYTES]);
        section.sky_light = Some(vec![0xFF; LIGHT_BYTES]);
        chunk.heightmaps.push((
            "MOTION_BLOCKING".to_owned(),
            (0..36).map(|i| i as i64).collect(),
        ));
        chunk.block_entities.push(NbtTag::Compound(vec![(
            "id".to_owned(),
            NbtTag::String("minecraft:chest".to_owned()),
        )]));
        chunk.entities.push(NbtTag::Compound(vec![(
            "id".to_owned(),
            NbtTag::String("minecraft:pig".to_owned()),
        )]));
        chunk.block_ticks.push(NbtTag::Compound(vec![(
            "i".to_owned(),
            NbtTag::String("minecraft:water".to_owned()),
        )]));
        chunk.fluid_ticks.push(NbtTag::Compound(vec![(
            "i".to_owned(),
            NbtTag::String("minecraft:water".to_owned()),
        )]));
        chunk.post_processing = vec![vec![1, 2, 3], vec![]];
        chunk.structures = Some(NbtTag::Compound(vec![(
            "starts".to_owned(),
            NbtTag::Compound(vec![]),
        )]));
        chunk.light_correct = true;
        chunk.inhabited_time = 1234;
        chunk.last_update = 99;

        let decoded = ChunkData::from_nbt(&chunk.to_nbt()).expect("decodes");
        assert_eq!(decoded, chunk);
        let values = decoded.sections[4]
            .block_states
            .to_values()
            .expect("unpacks");
        assert_eq!(values.len(), BLOCK_ENTRIES);
        assert_eq!(values[0].name, "minecraft:stone");
        assert_eq!(values[1].name, "minecraft:dirt");
        assert_eq!(values[2].name, "minecraft:air");
    }

    #[test]
    fn palette_bit_width_grows_with_content() {
        let mut section =
            SectionData::filled(-4, BlockState::new("minecraft:air"), "minecraft:plains");
        assert_eq!(section.block_states.bits(), 4, "minimum width");
        let (_, data) = section.block_states.parts();
        assert!(data.is_none(), "single palette entry stores no data");
        // The palette starts with air, so 15 more distinct states fill it to 16
        // entries (4 bits) and the 16th new state pushes it to 17 (5 bits).
        for index in 0..15 {
            section
                .block_states
                .set(index, BlockState::new(&format!("minecraft:b{index}")))
                .expect("set");
        }
        assert_eq!(section.block_states.palette().len(), 16);
        assert_eq!(section.block_states.bits(), 4);
        section
            .block_states
            .set(15, BlockState::new("minecraft:b15"))
            .expect("set");
        assert_eq!(section.block_states.palette().len(), 17);
        assert_eq!(section.block_states.bits(), 5);
        let (palette, data) = section.block_states.parts();
        assert_eq!(palette.len(), 17);
        let data = data.expect("packed data once >1 palette entry");
        assert_eq!(
            data.len(),
            mc_core::packing::longs_needed(BLOCK_ENTRIES, 5),
            "repacked to the new width"
        );
    }

    #[test]
    fn legacy_and_malformed_chunks_are_rejected_without_panicking() {
        let legacy = NbtTag::Compound(vec![("Level".to_owned(), NbtTag::Compound(vec![]))]);
        let err = ChunkData::from_nbt(&legacy).expect_err("legacy layout");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
        assert!(format!("{err}").contains("pre-1.18"));

        // Missing sections.
        assert!(ChunkData::from_nbt(&NbtTag::Compound(vec![])).is_err());

        // Palette index outside the palette.
        let bad = NbtTag::Compound(vec![
            ("xPos".to_owned(), NbtTag::Int(0)),
            ("zPos".to_owned(), NbtTag::Int(0)),
            (
                "sections".to_owned(),
                NbtTag::List(vec![NbtTag::Compound(vec![
                    ("Y".to_owned(), NbtTag::Byte(0)),
                    (
                        "block_states".to_owned(),
                        NbtTag::Compound(vec![
                            (
                                "palette".to_owned(),
                                NbtTag::List(vec![NbtTag::Compound(vec![(
                                    "Name".to_owned(),
                                    NbtTag::String("minecraft:air".to_owned()),
                                )])]),
                            ),
                            ("data".to_owned(), NbtTag::LongArray(vec![i64::MAX; 512])),
                        ]),
                    ),
                ])]),
            ),
        ]);
        assert!(ChunkData::from_nbt(&bad).is_err(), "out-of-palette index");

        // Light array of the wrong size.
        let bad_light = NbtTag::Compound(vec![
            ("xPos".to_owned(), NbtTag::Int(0)),
            ("zPos".to_owned(), NbtTag::Int(0)),
            (
                "sections".to_owned(),
                NbtTag::List(vec![NbtTag::Compound(vec![
                    ("Y".to_owned(), NbtTag::Byte(0)),
                    (
                        "block_states".to_owned(),
                        NbtTag::Compound(vec![(
                            "palette".to_owned(),
                            NbtTag::List(vec![NbtTag::Compound(vec![(
                                "Name".to_owned(),
                                NbtTag::String("minecraft:air".to_owned()),
                            )])]),
                        )]),
                    ),
                    ("BlockLight".to_owned(), NbtTag::ByteArray(vec![0; 10])),
                ])]),
            ),
        ]);
        let err = ChunkData::from_nbt(&bad_light).expect_err("light size");
        assert!(format!("{err}").contains("2048"), "{err}");
    }

    #[test]
    fn unknown_fields_are_preserved() {
        let chunk = ChunkData::empty(ChunkPos::new(0, 0), -4, 1);
        let mut root = chunk.to_nbt();
        root.insert("carving_mask", NbtTag::LongArray(vec![1, 2, 3]));
        let decoded = ChunkData::from_nbt(&root).expect("decodes");
        assert_eq!(
            decoded
                .extra
                .iter()
                .find(|(k, _)| k == "carving_mask")
                .map(|(_, v)| v.clone()),
            Some(NbtTag::LongArray(vec![1, 2, 3]))
        );
        let re_encoded = decoded.to_nbt();
        assert!(re_encoded.contains("carving_mask"), "must survive a save");
    }

    #[test]
    fn position_mismatch_is_detected() {
        let chunk = ChunkData::empty(ChunkPos::new(1, 2), -4, 1);
        assert!(chunk.verify_position(ChunkPos::new(1, 2)).is_ok());
        let err = chunk
            .verify_position(ChunkPos::new(9, 9))
            .expect_err("mismatch");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    }

    #[test]
    fn chunk_data_is_ordered_for_deterministic_saves() {
        use super::ChunkKey;
        use crate::dimension::Dimension;
        let mut keys = [
            ChunkKey::new(&Dimension::Nether, ChunkPos::new(1, 1)),
            ChunkKey::new(&Dimension::Overworld, ChunkPos::new(5, 0)),
            ChunkKey::new(&Dimension::Overworld, ChunkPos::new(-1, -1)),
        ];
        keys.sort();
        assert_eq!(keys[0].dimension, Dimension::Overworld);
        assert_eq!(keys[0].pos, ChunkPos::new(-1, -1));
        assert_eq!(keys[2].dimension, Dimension::Nether);
    }
}
