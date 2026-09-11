//! The runtime chunk: blocks by id, per-section counts, light arrays (P04-02/08).
//!
//! Block coordinates are **world** coordinates; the section math lives here so no
//! caller has to know that section `y >> 4` holds `y & 15`.

use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use mc_persistence::chunk::{
    BlockState as DiskBlockState, ChunkData, LIGHT_BYTES, PalettedContainer, SectionData,
};
use mc_persistence::packing::{BIOME_ENTRIES, BIOME_MIN_BITS, BLOCK_ENTRIES, BLOCK_MIN_BITS};
use mc_registry::BlockRegistry;

/// Re-export so callers do not need `mc_persistence` for the coordinate type.
pub use mc_persistence::chunk::ChunkPos;

/// Section edge length in blocks.
pub const SECTION_WIDTH: i32 = 16;
/// Section height in blocks.
pub const SECTION_HEIGHT: i32 = 16;
/// Blocks in one section (16³).
pub const BLOCKS_PER_SECTION: usize = 4096;

/// Bits per entry in a Vanilla heightmap (world height 384 needs 9).
///
/// 64 / 9 = 7 entries per long, so the tail of every long is padding and 256
/// entries occupy 37 longs.
pub const HEIGHTMAP_BITS: u32 = 9;

/// One 16³ section: block ids plus optional light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Section index in the world (`block_y >> 4`).
    pub y: i8,
    /// Block-state ids, indexed `x + z*16 + y*256` (matching Vanilla's
    /// `Strategy.getIndex`, where x varies fastest).
    pub blocks: Vec<i32>,
    /// Number of non-air blocks, maintained on every write (Vanilla's
    /// `nonEmptyBlockCount`, sent in the chunk packet).
    pub non_empty_block_count: i16,
    /// Nibble-packed block light, when the chunk carries it.
    pub block_light: Option<Vec<u8>>,
    /// Nibble-packed sky light, when the chunk carries it.
    pub sky_light: Option<Vec<u8>>,
}

impl Section {
    /// An all-air section.
    #[must_use]
    pub fn air(y: i8, air_id: i32) -> Self {
        Self {
            y,
            blocks: vec![air_id; BLOCKS_PER_SECTION],
            non_empty_block_count: 0,
            block_light: None,
            sky_light: None,
        }
    }

    /// In-section index of a local coordinate.
    #[must_use]
    pub const fn index(local_x: i32, local_y: i32, local_z: i32) -> usize {
        (local_x + local_z * SECTION_WIDTH + local_y * SECTION_WIDTH * SECTION_WIDTH) as usize
    }

    /// Block id at a local coordinate, or `None` when out of range.
    #[must_use]
    pub fn get(&self, local_x: i32, local_y: i32, local_z: i32) -> Option<i32> {
        if !(0..SECTION_WIDTH).contains(&local_x)
            || !(0..SECTION_HEIGHT).contains(&local_y)
            || !(0..SECTION_WIDTH).contains(&local_z)
        {
            return None;
        }
        self.blocks
            .get(Self::index(local_x, local_y, local_z))
            .copied()
    }
}

/// A loaded chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Chunk coordinates.
    pub pos: ChunkPos,
    /// Sections ordered by `y`.
    pub sections: Vec<Section>,
    /// Lowest section index (inclusive).
    pub min_section_y: i8,
    /// Generation status (`minecraft:full`, …).
    pub status: String,
    /// `DataVersion` this chunk was read at (or the current one when created).
    pub data_version: i32,
    /// Tick of the last content change.
    pub last_update: i64,
    /// Ticks players have spent here.
    pub inhabited_time: i64,
    /// Whether the stored light data is authoritative.
    pub light_correct: bool,
    /// Something changed since the last save.
    pub dirty: bool,
    /// Top-level NBT entries this model does not interpret, preserved verbatim so
    /// a load/save cycle cannot silently drop them.
    pub extra: Vec<(String, NbtTag)>,
}

impl Chunk {
    /// An all-air chunk spanning `section_count` sections from `min_section_y`.
    #[must_use]
    pub fn air(
        pos: ChunkPos,
        min_section_y: i8,
        section_count: usize,
        registry: &BlockRegistry,
    ) -> Self {
        let air = registry.air_id();
        let sections = (0..section_count)
            .map(|offset| Section::air(min_section_y.wrapping_add(offset as i8), air))
            .collect();
        Self {
            pos,
            sections,
            min_section_y,
            status: "minecraft:full".to_owned(),
            data_version: mc_persistence::level::DATA_VERSION_26_1_2,
            last_update: 0,
            inhabited_time: 0,
            light_correct: false,
            dirty: true,
            extra: Vec::new(),
        }
    }

    /// Number of sections.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Highest block y covered (exclusive).
    #[must_use]
    pub fn max_y(&self) -> i32 {
        (i32::from(self.min_section_y) + self.sections.len() as i32) * SECTION_HEIGHT
    }

    /// Lowest block y covered (inclusive).
    #[must_use]
    pub fn min_y(&self) -> i32 {
        i32::from(self.min_section_y) * SECTION_HEIGHT
    }

    /// Index of the section holding `block_y`.
    fn section_index_for(&self, block_y: i32) -> Option<usize> {
        let section_y = block_y.div_euclid(SECTION_HEIGHT);
        let offset = section_y - i32::from(self.min_section_y);
        usize::try_from(offset)
            .ok()
            .filter(|index| *index < self.sections.len())
    }

    /// Block-state id at a world position.
    ///
    /// Out-of-range `y` reads as air rather than erroring: callers ask "is this
    /// solid?" about positions above the world all the time, and air is the
    /// truthful answer (Vanilla does the same).
    #[must_use]
    pub fn get_block(&self, block_x: i32, block_y: i32, block_z: i32) -> i32 {
        let Some(index) = self.section_index_for(block_y) else {
            return 0; // air: id 0 is minecraft:air in every Vanilla version
        };
        let local_y = block_y.rem_euclid(SECTION_HEIGHT);
        let local_x = block_x.rem_euclid(SECTION_WIDTH);
        let local_z = block_z.rem_euclid(SECTION_WIDTH);
        self.sections[index]
            .get(local_x, local_y, local_z)
            .unwrap_or(0)
    }

    /// Set a block-state id at a world position.
    ///
    /// Returns the previous id and whether anything changed. A `y` outside the
    /// chunk's range is refused with an error instead of silently wrapping into
    /// another section — that is a bug in the caller (a coordinate validation
    /// failure), not player input.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `block_y` is outside this chunk.
    pub fn set_block(
        &mut self,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        id: i32,
        registry: &BlockRegistry,
    ) -> ServerResult<Option<(i32, i32)>> {
        let Some(index) = self.section_index_for(block_y) else {
            return Err(ServerError::InvalidAction(format!(
                "y={block_y} is outside chunk ({}, {}) which spans {}..{}",
                self.pos.x,
                self.pos.z,
                self.min_y(),
                self.max_y() - 1
            )));
        };
        let local_y = block_y.rem_euclid(SECTION_HEIGHT);
        let local_x = block_x.rem_euclid(SECTION_WIDTH);
        let local_z = block_z.rem_euclid(SECTION_WIDTH);
        let slot = Section::index(local_x, local_y, local_z);
        let section = &mut self.sections[index];
        let previous = section.blocks[slot];
        if previous == id {
            return Ok(None);
        }
        let was_empty = registry.is_empty(previous);
        let is_empty = registry.is_empty(id);
        match (was_empty, is_empty) {
            (true, false) => section.non_empty_block_count += 1,
            (false, true) => section.non_empty_block_count -= 1,
            _ => {}
        }
        section.blocks[slot] = id;
        self.dirty = true;
        Ok(Some((previous, id)))
    }

    /// Non-air block count of a section.
    #[must_use]
    pub fn non_empty_block_count(&self, section_index: usize) -> i16 {
        self.sections
            .get(section_index)
            .map_or(0, |section| section.non_empty_block_count)
    }

    /// Convert a disk chunk into the runtime form.
    ///
    /// # Errors
    ///
    /// - [`ServerError::CorruptData`] when the disk chunk names a block the
    ///   registry does not know, or carries a malformed palette. Substituting air
    ///   would silently corrupt a world, so it is refused instead.
    /// - [`ServerError::Operational`] when a palette cannot be re-packed.
    pub fn from_chunk_data(data: &ChunkData, registry: &BlockRegistry) -> ServerResult<Self> {
        let mut sections = Vec::with_capacity(data.sections.len());
        let mut first_section_y = None;
        for section in &data.sections {
            let entries = section
                .block_states
                .to_values()
                .map_err(|e| decorate(e, data.pos, section.y))?;
            let mut blocks = Vec::with_capacity(BLOCKS_PER_SECTION);
            let mut non_empty: i16 = 0;
            for (slot, state) in entries.iter().enumerate() {
                let properties: Vec<(String, String)> = state.properties.clone();
                let id = registry.state_id(&state.name, &properties).map_err(|e| {
                    ServerError::CorruptData(format!(
                        "chunk ({}, {}) section y={} slot {slot} names {}: {e}",
                        data.pos.x, data.pos.z, section.y, state.name
                    ))
                })?;
                if !registry.is_empty(id) {
                    non_empty += 1;
                }
                blocks.push(id);
            }
            sections.push(Section {
                y: section.y,
                blocks,
                non_empty_block_count: non_empty,
                block_light: section.block_light.clone(),
                sky_light: section.sky_light.clone(),
            });
            first_section_y.get_or_insert(section.y);
        }
        let Some(first_section_y) = first_section_y else {
            return Err(ServerError::CorruptData(format!(
                "chunk ({}, {}) has no sections",
                data.pos.x, data.pos.z
            )));
        };
        Ok(Self {
            pos: data.pos,
            sections,
            min_section_y: i8::try_from(data.min_section_y).unwrap_or(first_section_y),
            status: data.status.clone(),
            data_version: data.data_version,
            last_update: data.last_update,
            inhabited_time: data.inhabited_time,
            light_correct: data.light_correct,
            dirty: false,
            extra: data.extra.clone(),
        })
    }

    /// Convert back to the disk schema.
    ///
    /// # Errors
    ///
    /// - [`ServerError::CorruptData`] when a block id is outside the registry.
    /// - [`ServerError::Operational`] when a palette cannot be packed.
    pub fn to_chunk_data(&self, registry: &BlockRegistry) -> ServerResult<ChunkData> {
        let mut sections = Vec::with_capacity(self.sections.len());
        for section in &self.sections {
            let mut blocks: Vec<DiskBlockState> = Vec::with_capacity(section.blocks.len());
            for (slot, id) in section.blocks.iter().enumerate() {
                let state = registry.state_ref_of(*id).map_err(|e| {
                    ServerError::CorruptData(format!(
                        "chunk ({}, {}) section y={} slot {slot} holds id {id}: {e}",
                        self.pos.x, self.pos.z, section.y
                    ))
                })?;
                blocks.push(DiskBlockState {
                    name: state.name,
                    properties: state.properties,
                });
            }
            let block_states =
                PalettedContainer::from_values(blocks, BLOCK_ENTRIES, BLOCK_MIN_BITS)?;
            // Biomes are not simulated in P04; persist the plains biome so the
            // written chunk stays valid and round-trips.
            let biomes = PalettedContainer::single(
                "minecraft:plains".to_owned(),
                BIOME_ENTRIES,
                BIOME_MIN_BITS,
            );
            sections.push(SectionData {
                y: section.y,
                block_states,
                biomes,
                block_light: section.block_light.clone(),
                sky_light: section.sky_light.clone(),
            });
        }
        // Carry the heightmap through so a re-saved chunk keeps it.
        let heightmaps = vec![("WORLD_SURFACE".to_owned(), self.heightmap_long_array())];
        Ok(ChunkData {
            pos: self.pos,
            data_version: self.data_version,
            status: self.status.clone(),
            min_section_y: i32::from(self.min_section_y),
            last_update: self.last_update,
            inhabited_time: self.inhabited_time,
            light_correct: self.light_correct,
            sections,
            heightmaps,
            block_entities: Vec::new(),
            entities: Vec::new(),
            block_ticks: Vec::new(),
            fluid_ticks: Vec::new(),
            post_processing: Vec::new(),
            structures: None,
            extra: self.extra.clone(),
        })
    }

    /// Highest non-air block per column, packed the way Vanilla sends
    /// `WORLD_SURFACE`: 256 entries of 9 bits, 7 per long, never spanning.
    ///
    /// # Panics
    ///
    /// Cannot panic in practice: the packing width is the constant 9, which the
    /// shared packer accepts. The `expect` documents that invariant rather than
    /// returning a fallible signature every caller would have to unwrap.
    #[must_use]
    pub fn heightmap_long_array(&self) -> Vec<i64> {
        let mut values = vec![0u32; 256];
        for local_z in 0..SECTION_WIDTH {
            for local_x in 0..SECTION_WIDTH {
                let world_x = self.pos.x * SECTION_WIDTH + local_x;
                let world_z = self.pos.z * SECTION_WIDTH + local_z;
                let mut height = self.min_y();
                for y in (self.min_y()..self.max_y()).rev() {
                    if self.get_block(world_x, y, world_z) != 0 {
                        height = y + 1;
                        break;
                    }
                }
                values[(local_z * SECTION_WIDTH + local_x) as usize] = height as u32;
            }
        }
        // Vanilla's heightmap packing: 9 bits per entry, 7 per long, never spanning
        // a long boundary (64 / 9 = 7), so 256 entries need 37 longs. Reuse the
        // shared, fixture-verified packer rather than repeating the arithmetic here.
        mc_persistence::packing::pack(&values, HEIGHTMAP_BITS)
            .expect("9 bits is a valid packing width")
    }

    /// Mark the chunk clean (after a successful save).
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

fn decorate(error: ServerError, pos: ChunkPos, section_y: i8) -> ServerError {
    match error {
        ServerError::CorruptData(message) => ServerError::CorruptData(format!(
            "chunk ({}, {}) section y={section_y}: {message}",
            pos.x, pos.z
        )),
        other => other,
    }
}

/// Light arrays are a fixed size; re-exported for callers building sections.
pub const LIGHT_ARRAY_BYTES: usize = LIGHT_BYTES;
