//! Structure-template files: the on-disk format, its limits, and resolution to
//! block-state ids (P07-16).
//!
//! ## What a structure file is — measured, not recalled
//!
//! Every number in this section was measured from the 26.1.2 data pack extracted
//! from `target/vanilla-26.1.2/server-26.1.2.jar` into
//! `target/vanilla-26.1.2/extract/data/minecraft/structure/`, by
//! `target/vanilla-26.1.2/structure_census.py` and its two follow-ups. The
//! measurement is reproducible and the frozen results are asserted in
//! `tests/structure_pack.rs`.
//!
//! ```text
//! files                                 1202
//! gzip-compressed (magic 1f 8b)         1202   (not-gzip: 0)
//! root tag name                         ""     (all 1202)
//! DataVersion                          4790   (all 1202; TAG_Int, no other value)
//! root keys                             size, entities, DataVersion   (1202)
//!                                       blocks                       (1202)
//!                                       palette                      (1182)
//!                                       palettes  (PLURAL)           (  20)
//! size                                  TAG_List<TAG_Int> of 3, all > 0  (1202/1202)
//! largest size                          38 x 48 x 38 by volume
//!                                       (`bastion/treasure/big_air_full.nbt`);
//!                                       largest single axis 48
//! `blocks`                              TAG_List<TAG_Compound>        (1202/1202)
//! `blocks[i].pos`                       TAG_List<TAG_Int> of 3        (all 1 058 602)
//! `blocks[i].state`                     TAG_Int, an index into `palette`;
//!                                       0 out of range across all 1 058 602 blocks
//! `blocks[i].nbt`                       TAG_Compound; 4 848 blocks carry it
//! `entities`                            TAG_List<TAG_End> in 1147 files and
//!                                       TAG_List<TAG_Compound> in 55 (max 2);
//!                                       the key is present in all 1202
//! block positions outside `size`        0
//! ```
//!
//! And the same pack through **this reader and this project's registry**, i.e.
//! what the loader actually reports (frozen in `tests/structure_pack.rs`):
//!
//! ```text
//! loaded                                1182   (the 20 `palettes` files refused)
//! unique names                          1182   (the full relative path is the name,
//!                                               so `bastion/blocks/air` and
//!                                               `bastion/mobs/air` do not collide)
//! total blocks                          1 049 447
//! total palette entries                 15 558
//! total entities                        64
//! fit in one 16x16 chunk                1 028
//! palette entries with `Properties`     14 371  -- counting every alternative in
//!                                               the 20 plural files too
//! distinct palette block names          408    -- likewise; the 1 182 singular
//!                                               files alone use 403
//! palette entries per file              min 1, mean 13.2, max 124
//! blocks per file                       max 29 938
//! names that fail to resolve            0      -- all 15 558 resolve against
//!                                               `crates/test-support/fixtures/registry`
//! ```
//!
//! **The `size`/`pos` tag type is the single most important line above.** They are
//! `TAG_List<TAG_Int>`, **not** `TAG_IntArray`. An earlier version of this reader
//! accepted only `TAG_IntArray`, which meant every hand-built fixture test passed
//! while **all 1 202 real files were refused** with "no usable `size`". Both
//! encodings are accepted now, and `tests/structure_pack.rs` reads the real pack so
//! the mistake cannot come back.
//!
//! ## The two decisions this module makes, stated before the code
//!
//! **1. `palettes` (plural) is refused, by name.** 20 files — every
//! `shipwreck/*.nbt` — carry `palettes`: a *list of 8* alternative palettes that
//! Vanilla picks from at random to vary a wreck's wood type. They carry **no**
//! `palette` key at all, so a parser that only knows `palette` would silently
//! load an empty structure and place nothing. [`StructureError::UnsupportedPalettes`]
//! names the key instead. Implementing the random pick is deliberately deferred:
//! it needs a documented per-structure random draw and a decision about which
//! draw stream, and a wrong guess would produce a wreck of the wrong wood rather
//! than a visible refusal.
//!
//! **2. Block entities, entities and `DataVersion` are not modelled.** 4 848
//! blocks carry `nbt` (a chest's loot table, a spawner's contents). `nbt` is
//! **accepted and ignored**, and that is a real, recorded divergence: the block
//! is placed, its contents are not. [`StructureTemplate::entities`] is a **count**
//! as P07-16 specifies, not a model. See the *Not implemented* section of
//! [`crate::structures`] for the full list.
//!
//! ## Limits (AGENTS.md §10)
//!
//! A structure file is attacker-influenced data exactly like a region file, so
//! the reader is bounded on four axes — file bytes, decompressed bytes, block
//! count, palette count and dimension per axis — and, critically, a hostile
//! `size` is **refused before anything is allocated from it**. `blocks` is sized
//! by the length of the block list (bounded by both the NBT tag budget and
//! [`StructureLimits::max_blocks`]), never by `size[0] * size[1] * size[2]`:
//! a file claiming `[1_000_000, 1_000_000, 1_000_000]` with three blocks is
//! refused for the dimension and never asks for 10¹⁸ slots.
//!
//! ## No parity claim
//!
//! The *format* here is measured from the 26.1.2 jar and is therefore
//! **verified** for the files that were measured. What this module does **not**
//! reproduce is Vanilla's `StructureTemplate` semantics: no `StructureProcessor`
//! chain (no block-age randomisation, no gravity processing, no jigsaw
//! replacement), no mirror/rotation, no `ignoreAir` decision made *for* the
//! caller — [`crate::placement`] takes that as an explicit policy.

use mc_core::ids::ResourceId;
use mc_nbt::{Limits as NbtLimits, NbtTag, TagReader};
use mc_persistence::compression::Compression;
use mc_registry::BlockRegistry;
use std::fmt;
use std::path::Path;

// -------------------------------------------------------------------- limits

/// Resource budgets for one structure file.
///
/// Every field is a **product decision**, sized from the measured pack (see the
/// module docs) with headroom, not from a Vanilla limit — Vanilla has no
/// published structure-file limit this project could verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructureLimits {
    /// Maximum size of the compressed file on disk, in bytes.
    ///
    /// **derived**: the largest file in the 26.1.2 pack is 80 381 bytes, so this
    /// is about 52× headroom. It bounds the read before the decompressor runs.
    pub max_file_bytes: usize,
    /// Maximum size of the decompressed NBT payload, in bytes.
    ///
    /// **derived**: the largest decompressed payload in the pack is 1 078 393
    /// bytes (≈ 1.03 MiB; `bastion/units/air_base.nbt`), so this is about 3.9×
    /// headroom and is what stops a decompression bomb (the ratio in the pack is
    /// at most ≈ 350:1).
    pub max_decoded_bytes: usize,
    /// Maximum number of `blocks` entries.
    ///
    /// **derived**: the largest block list in the pack is 29 938, so this is
    /// about 3.3× headroom.
    pub max_blocks: usize,
    /// Maximum number of `palette` entries.
    ///
    /// **derived**: the largest palette in the pack holds 124 entries, so this is
    /// about 8× headroom.
    pub max_palette: usize,
    /// Maximum `size` along any single axis.
    ///
    /// **derived**: the largest axis in the pack is 48, so this is 4/3 of it.
    ///
    /// This is deliberately **tighter than a world**: Vanilla's jigsaw system
    /// assembles multi-piece structures far larger than one template, and a
    /// future jigsaw implementation must raise this rather than silently clip.
    /// It is also the guard that refuses a hostile `[1000000, 1000000, 1000000]`.
    pub max_axis: i32,
}

impl StructureLimits {
    /// The budgets used when a caller does not choose any.
    ///
    /// **derived** from the measured pack; see each field.
    pub const PACK: Self = Self {
        max_file_bytes: 4 * 1024 * 1024,
        max_decoded_bytes: 4 * 1024 * 1024,
        max_blocks: 100_000,
        max_palette: 1_024,
        max_axis: 64,
    };

    /// Limits with an explicit per-axis cap, keeping everything else.
    #[must_use]
    pub const fn with_max_axis(max_axis: i32) -> Self {
        Self {
            max_axis,
            ..Self::PACK
        }
    }

    /// NBT reader budgets matching [`StructureLimits::max_decoded_bytes`].
    ///
    /// The tag budget is `max_decoded_bytes / 4`: a real structure NBT needs at
    /// least four bytes per tag it creates (a tag id, a name length and a
    /// payload), so this cannot reject a file the byte budget would have
    /// accepted, and it stops a payload of 4 million one-byte tags from
    /// amplifying into millions of `NbtTag`s.
    #[must_use]
    pub const fn nbt_limits(self) -> NbtLimits {
        NbtLimits {
            max_depth: 32,
            max_bytes: self.max_decoded_bytes,
            max_elements: self.max_decoded_bytes / 4,
        }
    }
}

impl Default for StructureLimits {
    fn default() -> Self {
        Self::PACK
    }
}

// -------------------------------------------------------------------- errors

/// Why a structure file could not be read, or a palette could not be resolved.
///
/// Every variant names what failed. There is no "other": a refusal a caller
/// cannot act on is not a refusal (AGENTS.md §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureError {
    /// The file could not be read from disk.
    Io {
        /// The path that failed.
        path: String,
        /// The operating system's complaint.
        reason: String,
    },
    /// The file is larger than [`StructureLimits::max_file_bytes`].
    FileTooLarge {
        /// Actual size in bytes.
        size: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// The file is empty — not gzip, so it cannot be a structure at all.
    EmptyFile,
    /// The file does not start with the gzip magic `1f 8b`.
    NotGzip {
        /// The first two bytes, as read.
        magic: [u8; 2],
    },
    /// The gzip stream is truncated, corrupt, or expands past the byte budget.
    BadGzip {
        /// The decompressor's complaint.
        reason: String,
    },
    /// The decompressed payload is not NBT this reader accepts.
    BadNbt {
        /// The reader's complaint.
        reason: String,
    },
    /// The root tag is not a compound.
    RootNotCompound {
        /// The tag type that was found, e.g. `TAG_List`.
        found: &'static str,
    },
    /// A required root key is missing or has the wrong tag type.
    MissingField {
        /// The key, e.g. `size`.
        field: &'static str,
    },
    /// `size` is not exactly three integers.
    BadSizeLength {
        /// How many entries `size` held.
        found: usize,
    },
    /// `size` has a non-positive axis.
    ///
    /// A structure of zero or negative extent contains no position, so placing
    /// it is meaningless; refusing is more honest than placing nothing quietly.
    BadSizeValue {
        /// The offending `[x, y, z]`.
        size: [i32; 3],
    },
    /// `size` exceeds [`StructureLimits::max_axis`] on some axis.
    ///
    /// The hostile-input case: refused **before** any allocation proportional to
    /// the volume.
    SizeTooLarge {
        /// The declared `[x, y, z]`.
        size: [i32; 3],
        /// The limit that was exceeded.
        limit: i32,
    },
    /// The `palette` list is longer than [`StructureLimits::max_palette`].
    PaletteTooLarge {
        /// Declared palette length.
        size: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// The `blocks` list is longer than [`StructureLimits::max_blocks`].
    TooManyBlocks {
        /// Declared block count.
        size: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// A palette entry is not a compound, or has no string `Name`.
    BadPaletteEntry {
        /// Index of the entry in `palette`.
        index: usize,
    },
    /// A palette entry's `Name` is not a valid resource id.
    BadBlockName {
        /// Index of the entry in `palette`.
        index: usize,
        /// The name as it appeared in the file.
        name: String,
        /// The parser's complaint.
        reason: String,
    },
    /// A palette entry's `Properties` is not a compound of scalar values.
    BadProperties {
        /// Index of the entry in `palette`.
        index: usize,
        /// The property that failed.
        property: String,
    },
    /// A block entry is not a compound, has no `pos`, or has no integer `state`.
    BadBlockEntry {
        /// Index of the entry in `blocks`.
        index: usize,
    },
    /// A block's `pos` is not exactly three integers.
    ///
    /// (The value is carried as a string rather than a `[i32; 3]` because the
    /// offending array may have any length; `i32::MIN..=i32::MAX` is not a
    /// printable diagnostic for a three-element array that has two elements.)
    BadBlockPos {
        /// Index of the entry in `blocks`.
        index: usize,
        /// The position list as it appeared, rendered for the message.
        pos: String,
    },
    /// A block's `state` indexes outside the palette.
    ///
    /// Refused **by name of the index**, never clamped to the last palette entry
    /// (AGENTS.md §3.3: a wrong block is worse than a refused file). The index is
    /// a `u64` because a hostile file may store `usize::MAX`.
    PaletteIndexOutOfRange {
        /// Index of the entry in `blocks`.
        block: usize,
        /// The offending index.
        state: u64,
        /// The palette's length, so the reader can see the bound.
        palette_len: usize,
    },
    /// The file carries `palettes` (plural) instead of `palette`.
    ///
    /// Every `shipwreck/*.nbt` does (20 files, measured). See the module docs.
    UnsupportedPalettes {
        /// How many alternative palettes the file declares.
        count: usize,
    },
    /// A palette entry names a block the registry does not know.
    UnknownBlock {
        /// The block name, exactly as the palette spelled it.
        name: String,
        /// The registry's complaint, which distinguishes "no such block" from
        /// "the block exists but has no such state".
        reason: String,
    },
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => write!(formatter, "cannot read {path}: {reason}"),
            Self::FileTooLarge { size, limit } => write!(
                formatter,
                "structure file is {size} bytes, over the {limit} byte limit"
            ),
            Self::EmptyFile => write!(formatter, "structure file is empty"),
            Self::NotGzip { magic } => write!(
                formatter,
                "structure file is not gzip: expected 1f 8b, found {:02x} {:02x}",
                magic[0], magic[1]
            ),
            Self::BadGzip { reason } => write!(formatter, "structure gzip stream: {reason}"),
            Self::BadNbt { reason } => write!(formatter, "structure NBT: {reason}"),
            Self::RootNotCompound { found } => {
                write!(formatter, "structure root tag is {found}, not a compound")
            }
            Self::MissingField { field } => {
                write!(formatter, "structure has no usable `{field}`")
            }
            Self::BadSizeLength { found } => {
                write!(
                    formatter,
                    "structure `size` has {found} entries, expected 3"
                )
            }
            Self::BadSizeValue { size } => write!(
                formatter,
                "structure `size` [{}, {}, {}] must be positive on every axis",
                size[0], size[1], size[2]
            ),
            Self::SizeTooLarge { size, limit } => write!(
                formatter,
                "structure `size` [{}, {}, {}] exceeds the {limit} block per-axis limit",
                size[0], size[1], size[2]
            ),
            Self::PaletteTooLarge { size, limit } => write!(
                formatter,
                "structure palette holds {size} entries, over the {limit} limit"
            ),
            Self::TooManyBlocks { size, limit } => write!(
                formatter,
                "structure holds {size} blocks, over the {limit} limit"
            ),
            Self::BadPaletteEntry { index } => {
                write!(
                    formatter,
                    "palette entry {index} is not a compound with a `Name`"
                )
            }
            Self::BadBlockName {
                index,
                name,
                reason,
            } => write!(
                formatter,
                "palette entry {index} names {name:?}, not a resource id: {reason}"
            ),
            Self::BadProperties { index, property } => write!(
                formatter,
                "palette entry {index} property {property:?} is not a scalar value"
            ),
            Self::BadBlockEntry { index } => write!(
                formatter,
                "block entry {index} is not a compound with `pos` and an integer `state`"
            ),
            Self::BadBlockPos { index, pos } => write!(
                formatter,
                "block entry {index} has position {pos}, expected three integers"
            ),
            Self::PaletteIndexOutOfRange {
                block,
                state,
                palette_len,
            } => write!(
                formatter,
                "block entry {block} has state index {state}, outside the {palette_len} entry palette"
            ),
            Self::UnsupportedPalettes { count } => write!(
                formatter,
                "structure declares {count} alternative `palettes`, which this reader does not \
                 implement (only `palette` is supported)"
            ),
            Self::UnknownBlock { name, reason } => {
                write!(formatter, "structure palette names block {name}: {reason}")
            }
        }
    }
}

impl std::error::Error for StructureError {}

// --------------------------------------------------------------------- model

/// One entry of a structure's palette: a block name and its property values.
///
/// `properties` is stored **sorted by name**, because
/// [`mc_registry::BlockRegistry::state_id`] resolves by name and the sort makes
/// two palettes that differ only in property order compare and hash equal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaletteEntry {
    /// Block name, validated as a resource id when the file was read.
    pub name: ResourceId,
    /// Sorted `(property, value)` pairs; empty for a property-less block.
    pub properties: Vec<(String, String)>,
}

/// One block of a structure: where it is, and which palette entry it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructureBlock {
    /// Position inside the template, in `0..size` on every axis.
    ///
    /// A local origin, not a world position; [`crate::placement`] adds the
    /// placement origin.
    pub pos: [i32; 3],
    /// Index into [`StructureTemplate::palette`]. Always in range: a file whose
    /// index is out of range is refused while reading.
    pub state: usize,
}

/// A parsed structure file.
///
/// Still **unresolved**: the palette holds names, not block-state ids. Call
/// [`StructureTemplate::resolve`] to get the ids the chunk model writes. The two
/// steps are separate so loading 1 202 files does not require a registry, and so
/// an unknown block name is reported at a point where the file that caused it is
/// still known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureTemplate {
    /// Extent along `(x, y, z)`, all positive.
    pub size: [i32; 3],
    /// Block palette, in file order.
    pub palette: Vec<PaletteEntry>,
    /// Blocks, in file order.
    pub blocks: Vec<StructureBlock>,
    /// How many entities the file declares.
    ///
    /// A **count only** (P07-16): entity NBT is not modelled, so a structure's
    /// mobs are not spawned. Measured: 55 of the 1 202 pack files declare one or
    /// two entities, so this is non-zero for real files and the count is worth
    /// carrying rather than discarding.
    pub entities: usize,
    /// The file's `DataVersion`.
    ///
    /// Recorded, never checked: all 1 202 pack files say 4790, but refusing a
    /// file for a different version would reject a datapack written by a
    /// different (compatible) release, which is a datapack concern rather than a
    /// reader concern. `None` when the key was absent.
    pub data_version: Option<i32>,
}

impl StructureTemplate {
    /// Number of blocks the template declares.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the template fits entirely inside one chunk's 16×16 footprint.
    ///
    /// The question [`crate::placement`]'s single-chunk policy asks. **Measured
    /// on the 26.1.2 pack: 1 028 of the 1 182 single-palette files do**, and the
    /// 154 that do not are village houses, ancient-city pieces, bastion units,
    /// trial chambers and one woodland-mansion piece.
    #[must_use]
    pub fn fits_in_one_chunk(&self) -> bool {
        self.size[0] <= mc_world::SECTION_WIDTH && self.size[2] <= mc_world::SECTION_WIDTH
    }

    /// Whether any of the template's positions lies outside its own `size`.
    ///
    /// A property of the file, not of the model: [`read_structure`] accepts an
    /// out-of-range position (a datapack may legitimately author one, and
    /// Vanilla's own placer clips by bounds), so a caller that wants to know can
    /// ask. Measured: **zero** of the 1 058 602 pack blocks are outside `size`.
    #[must_use]
    pub fn blocks_outside_size(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| !inside(&self.size, block.pos))
            .count()
    }

    /// Resolve every palette entry to a numeric block-state id, **by name**.
    ///
    /// Nothing here is a hard-coded id: each entry goes through
    /// [`mc_registry::BlockRegistry::state_id`], so a registry refresh moves the
    /// ids and this code does not change. An unknown **block name** and an
    /// unknown **property combination** are different registry messages and both
    /// are carried through in [`StructureError::UnknownBlock::reason`].
    ///
    /// # Errors
    ///
    /// [`StructureError::UnknownBlock`] naming the block, for the first entry
    /// that fails. The first, not all: a report of 400 failures for one wrong
    /// registry is noise, and the caller fixes them one at a time anyway.
    pub fn resolve(&self, registry: &BlockRegistry) -> Result<ResolvedStructure, StructureError> {
        let mut states = Vec::with_capacity(self.palette.len());
        for entry in &self.palette {
            let name = entry.name.to_string();
            let id = registry
                .state_id(&name, &entry.properties)
                .map_err(|error| StructureError::UnknownBlock {
                    name,
                    reason: error.to_string(),
                })?;
            states.push(id);
        }
        Ok(ResolvedStructure {
            size: self.size,
            palette: states,
            blocks: self.blocks.clone(),
            entities: self.entities,
        })
    }
}

/// A structure whose palette has been resolved to block-state ids.
///
/// This is what [`crate::placement::place`] writes from. It no longer knows any
/// block *name*, which is the point: the name→id bridge is crossed once, in
/// [`StructureTemplate::resolve`], and a placement cannot accidentally re-derive
/// an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStructure {
    /// Extent along `(x, y, z)`, all positive.
    pub size: [i32; 3],
    /// Block-state ids, parallel to the template's palette.
    pub palette: Vec<i32>,
    /// Blocks, in file order.
    pub blocks: Vec<StructureBlock>,
    /// How many entities the file declared (not spawned).
    pub entities: usize,
}

impl ResolvedStructure {
    /// Number of blocks the structure declares.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Highest palette index used by any block, or `None` for an empty structure.
    ///
    /// Used by the pack test to assert the measured "no index out of range"
    /// property without re-reading the files.
    #[must_use]
    pub fn highest_palette_index(&self) -> Option<usize> {
        self.blocks.iter().map(|block| block.state).max()
    }
}

// -------------------------------------------------------------------- reader

/// Read and parse one structure file: gzip, then NBT, then this module's model.
///
/// Strict about the things that would otherwise corrupt a world, tolerant about
/// the things that would only lose a detail:
///
/// - **strict**: gzip magic, root compound, `size` present and positive and
///   within [`StructureLimits::max_axis`], `palette` present (or a named refusal
///   of `palettes`), every `state` index inside the palette;
/// - **tolerant**: `entities` and `DataVersion` may be missing;
/// - **ignored, and recorded as such**: `blocks[i].nbt` (block entities) is read
///   past and dropped.
///
/// # Errors
///
/// A [`StructureError`] naming what failed. Empty files, non-gzip files,
/// truncated gzip streams, absent fields, oversized `size` values, oversized
/// palettes and out-of-range palette indices are all typed refusals — none of
/// them panics, and none of them allocates from an attacker-supplied count.
pub fn read_structure(
    path: &Path,
    limits: StructureLimits,
) -> Result<StructureTemplate, StructureError> {
    let bytes = std::fs::read(path).map_err(|error| StructureError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    parse_gzip_structure(&bytes, limits)
}

/// [`read_structure`] for a payload already in memory.
///
/// Split out so the hostile-input tests can exercise the parser without writing
/// files, and so a datapack reader that has already loaded a blob can reuse it.
///
/// # Errors
///
/// As for [`read_structure`].
pub fn parse_gzip_structure(
    bytes: &[u8],
    limits: StructureLimits,
) -> Result<StructureTemplate, StructureError> {
    if bytes.len() > limits.max_file_bytes {
        return Err(StructureError::FileTooLarge {
            size: bytes.len(),
            limit: limits.max_file_bytes,
        });
    }
    if bytes.is_empty() {
        return Err(StructureError::EmptyFile);
    }
    if bytes.len() < 2 || bytes[0] != 0x1F || bytes[1] != 0x8B {
        // `[0, 0]` for a one-byte file, so the message is still printable.
        let magic = [
            bytes.first().copied().unwrap_or(0),
            bytes.get(1).copied().unwrap_or(0),
        ];
        return Err(StructureError::NotGzip { magic });
    }
    let decoded = Compression::Gzip
        .decompress(bytes, limits.max_decoded_bytes)
        .map_err(|error| StructureError::BadGzip {
            reason: error.to_string(),
        })?;
    parse_nbt_structure(&decoded, limits)
}

/// [`parse_gzip_structure`] with the decompression already done.
///
/// The lowest-level entry point: it takes a bare NBT byte slice (as a test can
/// build with [`mc_nbt::write_named`]) and applies the structure schema.
///
/// # Errors
///
/// [`StructureError::BadNbt`] or a schema refusal.
pub fn parse_nbt_structure(
    decoded: &[u8],
    limits: StructureLimits,
) -> Result<StructureTemplate, StructureError> {
    let mut reader =
        TagReader::new(decoded, limits.nbt_limits()).map_err(|error| bad_nbt(&error))?;
    let (_name, root) = reader.read_named().map_err(|error| bad_nbt(&error))?;
    // The root must be a compound: every field below is read by name, and name
    // lookup on a non-compound is how a malformed file would otherwise read as
    // "no `size`", which names the wrong problem.
    if root.entries().is_none() {
        return Err(StructureError::RootNotCompound {
            found: root.type_name(),
        });
    }

    let size = read_size(&root, limits)?;
    let palette = read_palette(&root, limits)?;
    let blocks = read_blocks(&root, limits, palette.len())?;
    let entities = root.get_list("entities").map_or(0, <[NbtTag]>::len);
    let data_version = root.get_i32("DataVersion");
    Ok(StructureTemplate {
        size,
        palette,
        blocks,
        entities,
        data_version,
    })
}

/// Wrap an NBT failure without losing the reader's message.
fn bad_nbt(error: &mc_core::error::ServerError) -> StructureError {
    StructureError::BadNbt {
        reason: error.to_string(),
    }
}

/// Whether a template-local position lies inside `size`.
///
/// The bounds are half-open on purpose: a structure of size `[1, 1, 1]` contains
/// exactly the position `[0, 0, 0]`, which matches Vanilla's
/// `StructureTemplate` (a template's bounding box is `size` blocks wide starting
/// at its origin) and matches the measured pack, where every one of the
/// 1 058 602 block positions satisfies `0 <= pos[i] < size[i]`.
#[must_use]
pub fn inside(size: &[i32; 3], pos: [i32; 3]) -> bool {
    (0..size[0]).contains(&pos[0])
        && (0..size[1]).contains(&pos[1])
        && (0..size[2]).contains(&pos[2])
}

/// Read a three-integer coordinate field, accepting both encodings.
///
/// ## Why two encodings are accepted, and which one is real
///
/// **Measured on all 1 202 pack files** (by
/// `target/vanilla-26.1.2/structure_tagids.py`): `size` is `LIST<INT>` in
/// **1 202 of 1 202** files, and `blocks[i].pos` is `LIST<INT>` in every sampled
/// block (77 553 of 77 553). Neither is a `TAG_IntArray`. Vanilla's
/// `StructureTemplate` reads them with `ListTag`-of-`IntTag` accessors, so this is
/// the format, not an artefact of the extraction.
///
/// `TAG_IntArray` is nonetheless **accepted**, because it is the obvious way to
/// write the same three numbers and a datapack generator may legitimately do so.
/// Getting this wrong in the other direction is exactly what an earlier version of
/// this reader did: it accepted *only* `TAG_IntArray`, so every hand-built fixture
/// test passed while **all 1 202 real files were refused**. That is the failure
/// mode a synthetic-only test suite cannot see, and it is why `StructureError`
/// carries the field name and why `tests/structure_pack.rs` reads the real pack.
///
/// Returns `None` when the field is absent or is not three integers; the caller
/// supplies the error, which differs between `size` and `pos`.
fn three_ints(tag: &NbtTag, field: &str) -> Option<[i32; 3]> {
    if let Some(array) = tag.get_int_array(field) {
        return <[i32; 3]>::try_from(array).ok();
    }
    let items = tag.get_list(field)?;
    if items.len() != 3 {
        return None;
    }
    let mut out = [0_i32; 3];
    for (slot, item) in items.iter().enumerate() {
        // `as_i64` accepts every integer width, which is the tolerant read
        // `mc-nbt` documents for a field a tool may have written narrowly.
        *out.get_mut(slot)? = i32::try_from(item.as_i64()?).ok()?;
    }
    Some(out)
}

/// A printable description of a mis-shaped coordinate field.
///
/// The value itself, not a length: "`pos` is `[0, 1]`" tells a datapack author
/// what to fix, and an array of three is short enough to print whole.
fn describe_list(tag: &NbtTag, field: &str) -> String {
    if let Some(items) = tag.get_list(field) {
        let rendered: Vec<String> = items
            .iter()
            .map(|item| {
                item.as_i64()
                    .map_or_else(|| item.type_name().to_owned(), |v| v.to_string())
            })
            .collect();
        return format!("[{}]", rendered.join(", "));
    }
    if let Some(array) = tag.get_int_array(field) {
        let rendered: Vec<String> = array.iter().map(i32::to_string).collect();
        return format!("[{}]", rendered.join(", "));
    }
    tag.get(field)
        .map_or_else(|| "absent".to_owned(), |value| value.type_name().to_owned())
}

/// Read and validate the `size` field.
fn read_size(root: &NbtTag, limits: StructureLimits) -> Result<[i32; 3], StructureError> {
    let Some(size) = three_ints(root, "size") else {
        // A `size` that exists but is not three integers is a *length* problem,
        // which is a different message from a missing field.
        if root.contains("size") {
            return Err(StructureError::BadSizeLength {
                found: root
                    .get_list("size")
                    .map_or(0, <[NbtTag]>::len)
                    .max(root.get_int_array("size").map_or(0, <[i32]>::len)),
            });
        }
        return Err(StructureError::MissingField { field: "size" });
    };
    if size.iter().any(|axis| *axis <= 0) {
        return Err(StructureError::BadSizeValue { size });
    }
    // The hostile case: refused here, before `palette` or `blocks` is read and
    // long before anything is allocated proportional to the declared volume.
    if size.iter().any(|axis| *axis > limits.max_axis) {
        return Err(StructureError::SizeTooLarge {
            size,
            limit: limits.max_axis,
        });
    }
    Ok(size)
}

/// Read and validate the `palette` field, refusing `palettes` by name.
fn read_palette(
    root: &NbtTag,
    limits: StructureLimits,
) -> Result<Vec<PaletteEntry>, StructureError> {
    if root.contains("palettes") {
        let count = root.get_list("palettes").map_or(0, <[NbtTag]>::len);
        return Err(StructureError::UnsupportedPalettes { count });
    }
    let raw = root
        .get_list("palette")
        .ok_or(StructureError::MissingField { field: "palette" })?;
    if raw.len() > limits.max_palette {
        return Err(StructureError::PaletteTooLarge {
            size: raw.len(),
            limit: limits.max_palette,
        });
    }
    let mut palette = Vec::with_capacity(raw.len());
    for (index, tag) in raw.iter().enumerate() {
        palette.push(read_palette_entry(index, tag)?);
    }
    Ok(palette)
}

/// One palette entry: `{Name: "minecraft:x", Properties: {...}}`.
fn read_palette_entry(index: usize, tag: &NbtTag) -> Result<PaletteEntry, StructureError> {
    let name = tag
        .get_str("Name")
        .ok_or(StructureError::BadPaletteEntry { index })?;
    let name = ResourceId::parse(name).map_err(|error| StructureError::BadBlockName {
        index,
        name: name.to_owned(),
        reason: error.to_string(),
    })?;
    let mut properties = Vec::new();
    if let Some(entries) = tag.get_compound("Properties").and_then(NbtTag::entries) {
        properties.reserve(entries.len());
        for (property, value) in entries {
            let text = scalar_to_string(value).ok_or_else(|| StructureError::BadProperties {
                index,
                property: property.clone(),
            })?;
            properties.push((property.clone(), text));
        }
        // Sorted, so `PaletteEntry` equality and hashing do not depend on the
        // order a datapack wrote its properties in. `BlockRegistry::state_id`
        // resolves by name and does not care about the order, but a caller
        // comparing two templates does.
        properties.sort();
    }
    Ok(PaletteEntry { name, properties })
}

/// A property value as a string, for the registry's `Vec<(String, String)>`.
///
/// Vanilla writes every property value as a `TAG_String` (measured: 10 747
/// `Properties` compounds, all string-valued), but a datapack-authored file may
/// use the natural numeric type. Both are accepted; a compound or a list is not,
/// because a property is a scalar and a non-scalar is a malformed file rather
/// than a missing feature.
fn scalar_to_string(tag: &NbtTag) -> Option<String> {
    match tag {
        NbtTag::String(text) => Some(text.clone()),
        NbtTag::Byte(value) => Some(value.to_string()),
        NbtTag::Short(value) => Some(value.to_string()),
        NbtTag::Int(value) => Some(value.to_string()),
        NbtTag::Long(value) => Some(value.to_string()),
        _ => None,
    }
}

/// Read and validate the `blocks` field.
fn read_blocks(
    root: &NbtTag,
    limits: StructureLimits,
    palette_len: usize,
) -> Result<Vec<StructureBlock>, StructureError> {
    let raw = root
        .get_list("blocks")
        .ok_or(StructureError::MissingField { field: "blocks" })?;
    if raw.len() > limits.max_blocks {
        return Err(StructureError::TooManyBlocks {
            size: raw.len(),
            limit: limits.max_blocks,
        });
    }
    let mut blocks = Vec::with_capacity(raw.len());
    for (index, tag) in raw.iter().enumerate() {
        blocks.push(read_block(index, tag, palette_len)?);
    }
    Ok(blocks)
}

/// One block entry: `{pos: [x, y, z], state: n, nbt: {...}}`.
///
/// The measured real shape: `pos` is `LIST<INT>` and `state` is `INT` (77 553 of
/// 77 553 sampled blocks). `pos` also accepts `TAG_IntArray`; see [`three_ints`].
/// `nbt` is deliberately **not read**: a block entity's contents are dropped, and
/// the block itself is still placed. That is a recorded divergence, not an
/// oversight — see the module docs.
fn read_block(
    index: usize,
    tag: &NbtTag,
    palette_len: usize,
) -> Result<StructureBlock, StructureError> {
    let Some(pos) = three_ints(tag, "pos") else {
        // Distinguish "no `pos` at all" from "a `pos` of the wrong shape": the
        // first is a malformed entry, the second is a malformed position, and a
        // reader that reported one as the other would send a datapack author to
        // the wrong line.
        if tag.contains("pos") {
            return Err(StructureError::BadBlockPos {
                index,
                pos: describe_list(tag, "pos"),
            });
        }
        return Err(StructureError::BadBlockEntry { index });
    };
    let state = tag
        .get_i64("state")
        .ok_or(StructureError::BadBlockEntry { index })?;
    // `u64`, so a hostile `usize::MAX` (or a negative value) is an out-of-range
    // refusal naming the index rather than a wrap into a valid-looking slot.
    let state_u64 = u64::try_from(state).unwrap_or(u64::MAX);
    let state = usize::try_from(state_u64)
        .ok()
        .filter(|value| *value < palette_len)
        .ok_or(StructureError::PaletteIndexOutOfRange {
            block: index,
            state: state_u64,
            palette_len,
        })?;
    Ok(StructureBlock { pos, state })
}

#[cfg(test)]
mod tests {
    use super::{
        StructureError, StructureLimits, parse_gzip_structure, parse_nbt_structure, read_structure,
    };
    use mc_nbt::{Limits as NbtLimits, NbtTag, write_named};
    use mc_registry::{BlockRegistry, Registries};
    use std::path::Path;

    fn registry() -> BlockRegistry {
        match Registries::vanilla() {
            Ok(registries) => registries.blocks,
            Err(error) => panic!("the registry fixture must load: {error}"),
        }
    }

    /// The byte-level builder the round-trip test uses.
    ///
    /// Deliberately hand-built from `NbtTag` values rather than read from a
    /// fixture file: a fixture would test "we can read what we wrote once", and
    /// this tests the schema in the module docs directly.
    fn sample_nbt() -> NbtTag {
        NbtTag::compound([
            ("DataVersion".to_owned(), NbtTag::Int(4790)),
            ("size".to_owned(), NbtTag::IntArray(vec![2, 2, 2])),
            (
                "palette".to_owned(),
                NbtTag::List(vec![
                    NbtTag::compound([("Name".to_owned(), NbtTag::String("minecraft:air".into()))]),
                    NbtTag::compound([
                        (
                            "Name".to_owned(),
                            NbtTag::String("minecraft:oak_log".into()),
                        ),
                        (
                            "Properties".to_owned(),
                            NbtTag::compound([("axis".to_owned(), NbtTag::String("y".into()))]),
                        ),
                    ]),
                    NbtTag::compound([(
                        "Name".to_owned(),
                        NbtTag::String("minecraft:stone_bricks".into()),
                    )]),
                ]),
            ),
            (
                "blocks".to_owned(),
                NbtTag::List(vec![
                    NbtTag::compound([
                        ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                        ("state".to_owned(), NbtTag::Int(1)),
                    ]),
                    NbtTag::compound([
                        ("pos".to_owned(), NbtTag::IntArray(vec![1, 0, 1])),
                        ("state".to_owned(), NbtTag::Int(2)),
                        // A block entity: accepted and dropped, by decision.
                        (
                            "nbt".to_owned(),
                            NbtTag::compound([(
                                "id".to_owned(),
                                NbtTag::String("minecraft:chest".into()),
                            )]),
                        ),
                    ]),
                ]),
            ),
            ("entities".to_owned(), NbtTag::List(Vec::new())),
        ])
    }

    fn encode(tag: &NbtTag) -> Vec<u8> {
        let mut raw = Vec::new();
        write_named("", tag, &mut raw).expect("NBT encodes");
        raw
    }

    #[test]
    fn a_hand_built_structure_round_trips_with_exact_palette_and_positions() {
        let template = parse_nbt_structure(&encode(&sample_nbt()), StructureLimits::PACK)
            .expect("the hand-built structure must parse");
        assert_eq!(template.size, [2, 2, 2]);
        assert_eq!(template.palette.len(), 3);
        assert_eq!(template.palette[0].name.to_string(), "minecraft:air");
        assert!(template.palette[0].properties.is_empty());
        assert_eq!(template.palette[1].name.to_string(), "minecraft:oak_log");
        assert_eq!(
            template.palette[1].properties,
            vec![("axis".to_owned(), "y".to_owned())]
        );
        assert_eq!(
            template.palette[2].name.to_string(),
            "minecraft:stone_bricks"
        );
        assert_eq!(template.blocks.len(), 2);
        assert_eq!(template.blocks[0].pos, [0, 0, 0]);
        assert_eq!(template.blocks[0].state, 1);
        assert_eq!(template.blocks[1].pos, [1, 0, 1]);
        assert_eq!(template.blocks[1].state, 2);
        assert_eq!(template.entities, 0);
        assert_eq!(template.data_version, Some(4790));
        assert!(template.fits_in_one_chunk());
        assert_eq!(template.block_count(), 2);
        // Every declared position is inside the declared size.
        assert_eq!(template.blocks_outside_size(), 0);
    }

    #[test]
    fn a_gzipped_structure_round_trips_through_the_file_reader() {
        let raw = encode(&sample_nbt());
        let gzipped = mc_persistence::compression::Compression::Gzip
            .compress(&raw)
            .expect("gzip encodes");
        assert_eq!(&gzipped[..2], &[0x1F, 0x8B], "gzip magic");
        let from_memory = parse_gzip_structure(&gzipped, StructureLimits::PACK).expect("parses");
        assert_eq!(from_memory.size, [2, 2, 2]);
        assert_eq!(from_memory.blocks.len(), 2);

        // And through a real file, which is the path the loader uses.
        let dir = std::env::temp_dir().join("mc-worldgen-structure-round-trip");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("sample.nbt");
        std::fs::write(&path, &gzipped).expect("write");
        let from_file = read_structure(&path, StructureLimits::PACK).expect("parses");
        assert_eq!(from_file, from_memory);
        let _ = std::fs::remove_file(&path);

        // A missing file is a typed refusal, not a panic.
        let error = read_structure(Path::new("no/such/structure.nbt"), StructureLimits::PACK)
            .expect_err("missing file");
        assert!(matches!(error, StructureError::Io { .. }), "{error:?}");
    }

    #[test]
    fn a_palette_entry_resolves_by_name_and_reports_unknown_blocks() {
        let blocks = registry();
        let template =
            parse_nbt_structure(&encode(&sample_nbt()), StructureLimits::PACK).expect("parses");
        let resolved = template
            .resolve(&blocks)
            .expect("every name is in the registry");
        assert_eq!(resolved.palette.len(), 3);
        // Resolved by name through the registry, never from a literal.
        assert_eq!(
            resolved.palette[0],
            blocks.default_state("minecraft:air").expect("air")
        );
        assert_eq!(
            resolved.palette[1],
            blocks
                .state_id("minecraft:oak_log", &[("axis".to_owned(), "y".to_owned())])
                .expect("oak_log axis=y")
        );
        assert_eq!(
            resolved.palette[2],
            blocks
                .default_state("minecraft:stone_bricks")
                .expect("stone_bricks")
        );
        // Properties were applied: `axis=y` is the log's **default**, and `axis=x` is a different state.
        //
        // The version of this assertion that stood here compared against
        // `default_state("minecraft:oak_log")` and required the two to **differ**, describing it as "the
        // property-less first state" — which is what `default_state` returned then, and is wrong for 642 of
        // 1168 blocks. The assertion was the bug written down as an expectation.
        assert_eq!(
            resolved.palette[1],
            blocks.default_state("minecraft:oak_log").expect("log"),
            "a log's default state is `axis=y`, so resolving `axis=y` must name it"
        );
        assert_ne!(
            blocks
                .state_id("minecraft:oak_log", &[("axis".to_owned(), "x".to_owned())])
                .expect("oak_log axis=x"),
            blocks.default_state("minecraft:oak_log").expect("log"),
            "`axis=x` must not be the default, which is what shows properties are applied"
        );
        assert_eq!(resolved.highest_palette_index(), Some(2));

        // An unknown block name is refused naming the block.
        let unknown = parse_nbt_structure(
            &encode(&NbtTag::compound([
                ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
                (
                    "palette".to_owned(),
                    NbtTag::List(vec![NbtTag::compound([(
                        "Name".to_owned(),
                        NbtTag::String("minecraft:not_a_block".into()),
                    )])]),
                ),
                ("blocks".to_owned(), NbtTag::List(Vec::new())),
            ])),
            StructureLimits::PACK,
        )
        .expect("parses: the name is a valid resource id");
        let error = unknown.resolve(&blocks).expect_err("must refuse");
        match &error {
            StructureError::UnknownBlock { name, reason } => {
                assert_eq!(name, "minecraft:not_a_block");
                assert!(!reason.is_empty(), "the registry's reason is carried");
            }
            other => panic!("expected UnknownBlock, got {other:?}"),
        }
        assert!(error.to_string().contains("minecraft:not_a_block"));

        // A name that exists but whose property combination does not.
        let bad_state = parse_nbt_structure(
            &encode(&NbtTag::compound([
                ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
                (
                    "palette".to_owned(),
                    NbtTag::List(vec![NbtTag::compound([
                        (
                            "Name".to_owned(),
                            NbtTag::String("minecraft:oak_log".into()),
                        ),
                        (
                            "Properties".to_owned(),
                            NbtTag::compound([(
                                "axis".to_owned(),
                                NbtTag::String("sideways".into()),
                            )]),
                        ),
                    ])]),
                ),
                ("blocks".to_owned(), NbtTag::List(Vec::new())),
            ])),
            StructureLimits::PACK,
        )
        .expect("parses: `sideways` is a syntactically fine value");
        let error = bad_state.resolve(&blocks).expect_err("must refuse");
        match &error {
            StructureError::UnknownBlock { name, reason } => {
                assert_eq!(name, "minecraft:oak_log");
                assert!(
                    reason.contains("sideways") || reason.contains("no state"),
                    "the reason must name the bad property: {reason}"
                );
            }
            other => panic!("expected UnknownBlock, got {other:?}"),
        }
    }
    #[test]
    fn an_out_of_range_palette_index_is_refused_naming_the_index() {
        // `i64` values that survive the `NBT TAG_Int` round trip: the reader asks
        // for an integer, and `Int(0xFFFF_FFFF)` is `-1` once read back as an
        // `i32`, so only in-range `i32` values can be expressed this way.
        for hostile in [3_i64, 999, i64::from(i32::MAX), -1] {
            let tag = NbtTag::compound([
                ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
                (
                    "palette".to_owned(),
                    NbtTag::List(vec![NbtTag::compound([(
                        "Name".to_owned(),
                        NbtTag::String("minecraft:stone".into()),
                    )])]),
                ),
                (
                    "blocks".to_owned(),
                    NbtTag::List(vec![NbtTag::compound([
                        ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                        ("state".to_owned(), NbtTag::Int(hostile as i32)),
                    ])]),
                ),
            ]);
            let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
                .expect_err("an out-of-range index must be refused");
            let reported = u64::try_from(i64::from(hostile as i32)).unwrap_or(u64::MAX);
            match &error {
                StructureError::PaletteIndexOutOfRange {
                    block,
                    state,
                    palette_len,
                } => {
                    assert_eq!(*block, 0);
                    assert_eq!(*palette_len, 1);
                    assert_eq!(
                        *state, reported,
                        "the *read-back* value is what the reader refuses"
                    );
                }
                other => panic!("expected PaletteIndexOutOfRange, got {other:?}"),
            }
            // The message names the index. It prints the `u64` the reader
            // derived, which for a negative `TAG_Int` is `u64::MAX` rather than
            // `-1` — that is the value the refusal is *about*, so the message and
            // the variant agree.
            assert!(error.to_string().contains(&reported.to_string()), "{error}");
        }
    }

    #[test]
    fn a_negative_palette_index_is_refused_as_the_largest_u64_not_wrapped() {
        // The hostile case a `state` stored as a `TAG_Long` can express and an
        // `i32` cannot: a negative value must be reported as an out-of-range
        // index, never as `usize`-wrapped into a valid-looking slot. `-1` mapped
        // through `usize` would be `usize::MAX`, which is *also* out of range, but
        // the point is that the reader must not rely on that coincidence — so the
        // value it reports is the sign-preserving `u64::MAX`.
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
            (
                "palette".to_owned(),
                NbtTag::List(vec![NbtTag::compound([(
                    "Name".to_owned(),
                    NbtTag::String("minecraft:stone".into()),
                )])]),
            ),
            (
                "blocks".to_owned(),
                NbtTag::List(vec![NbtTag::compound([
                    ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                    ("state".to_owned(), NbtTag::Long(-1)),
                ])]),
            ),
        ]);
        let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
            .expect_err("a negative index must be refused");
        match error {
            StructureError::PaletteIndexOutOfRange {
                state, palette_len, ..
            } => {
                assert_eq!(state, u64::MAX);
                assert_eq!(palette_len, 1);
            }
            other => panic!("expected PaletteIndexOutOfRange, got {other:?}"),
        }

        // And `i64::MAX` — far outside any palette — is refused with its own
        // value rather than truncated.
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
            (
                "palette".to_owned(),
                NbtTag::List(vec![NbtTag::compound([(
                    "Name".to_owned(),
                    NbtTag::String("minecraft:stone".into()),
                )])]),
            ),
            (
                "blocks".to_owned(),
                NbtTag::List(vec![NbtTag::compound([
                    ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                    ("state".to_owned(), NbtTag::Long(i64::MAX)),
                ])]),
            ),
        ]);
        let error =
            parse_nbt_structure(&encode(&tag), StructureLimits::PACK).expect_err("must be refused");
        match error {
            StructureError::PaletteIndexOutOfRange { state, .. } => {
                assert_eq!(state, i64::MAX as u64);
            }
            other => panic!("expected PaletteIndexOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn a_hostile_size_is_refused_before_anything_is_allocated() {
        let tag = NbtTag::compound([
            (
                "size".to_owned(),
                NbtTag::IntArray(vec![1_000_000, 1_000_000, 1_000_000]),
            ),
            ("palette".to_owned(), NbtTag::List(Vec::new())),
            ("blocks".to_owned(), NbtTag::List(Vec::new())),
        ]);
        let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
            .expect_err("a gigantic size must be refused");
        match error {
            StructureError::SizeTooLarge { size, limit } => {
                assert_eq!(size, [1_000_000, 1_000_000, 1_000_000]);
                assert_eq!(limit, StructureLimits::PACK.max_axis);
            }
            other => panic!("expected SizeTooLarge, got {other:?}"),
        }
        // A size of exactly the limit is accepted; one over is not.
        let ok = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![64, 1, 1])),
            ("palette".to_owned(), NbtTag::List(Vec::new())),
            ("blocks".to_owned(), NbtTag::List(Vec::new())),
        ]);
        assert!(parse_nbt_structure(&encode(&ok), StructureLimits::PACK).is_ok());
        let over = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![65, 1, 1])),
            ("palette".to_owned(), NbtTag::List(Vec::new())),
            ("blocks".to_owned(), NbtTag::List(Vec::new())),
        ]);
        assert!(parse_nbt_structure(&encode(&over), StructureLimits::PACK).is_err());
    }

    #[test]
    fn a_zero_or_negative_size_is_refused() {
        for size in [[0, 0, 0], [0, 1, 1], [1, -1, 1], [-5, 5, 5]] {
            let tag = NbtTag::compound([
                ("size".to_owned(), NbtTag::IntArray(size.to_vec())),
                ("palette".to_owned(), NbtTag::List(Vec::new())),
                ("blocks".to_owned(), NbtTag::List(Vec::new())),
            ]);
            let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
                .expect_err("a non-positive size must be refused");
            match error {
                StructureError::BadSizeValue { size: found } => assert_eq!(found, size),
                other => panic!("expected BadSizeValue for {size:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_plural_palettes_file_is_refused_by_name() {
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
            (
                "palettes".to_owned(),
                NbtTag::List(vec![
                    NbtTag::List(vec![NbtTag::compound([(
                        "Name".to_owned(),
                        NbtTag::String("minecraft:oak_planks".into()),
                    )])]);
                    8
                ]),
            ),
            ("blocks".to_owned(), NbtTag::List(Vec::new())),
        ]);
        let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
            .expect_err("the plural form is not implemented and must be refused");
        match error {
            StructureError::UnsupportedPalettes { count } => assert_eq!(count, 8),
            other => panic!("expected UnsupportedPalettes, got {other:?}"),
        }
    }

    #[test]
    fn a_file_with_no_palette_at_all_is_refused() {
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![1, 1, 1])),
            ("blocks".to_owned(), NbtTag::List(Vec::new())),
        ]);
        let error = parse_nbt_structure(&encode(&tag), StructureLimits::PACK)
            .expect_err("no palette is a refusal");
        assert!(matches!(
            error,
            StructureError::MissingField { field: "palette" }
        ));
    }

    #[test]
    fn the_nbt_budgets_are_enforced_not_merely_declared() {
        // A structure whose block list alone exceeds `max_blocks` is refused by
        // count, before any per-block work.
        let blocks: Vec<NbtTag> = (0..5)
            .map(|index| {
                NbtTag::compound([
                    ("pos".to_owned(), NbtTag::IntArray(vec![0, 0, 0])),
                    ("state".to_owned(), NbtTag::Int(index)),
                ])
            })
            .collect();
        let tag = NbtTag::compound([
            ("size".to_owned(), NbtTag::IntArray(vec![4, 4, 4])),
            (
                "palette".to_owned(),
                NbtTag::List(vec![NbtTag::compound([(
                    "Name".to_owned(),
                    NbtTag::String("minecraft:stone".into()),
                )])]),
            ),
            ("blocks".to_owned(), NbtTag::List(blocks)),
        ]);
        let tight = StructureLimits {
            max_blocks: 2,
            ..StructureLimits::PACK
        };
        let error = parse_nbt_structure(&encode(&tag), tight).expect_err("over the block limit");
        match error {
            StructureError::TooManyBlocks { size, limit } => {
                assert_eq!(size, 5);
                assert_eq!(limit, 2);
            }
            other => panic!("expected TooManyBlocks, got {other:?}"),
        }

        // And the NBT tag budget refuses a payload that exceeds it, which is the
        // decompression-bomb guard's second half.
        let huge = encode(&sample_nbt());
        let tiny = StructureLimits {
            max_decoded_bytes: 8,
            ..StructureLimits::PACK
        };
        let error = parse_nbt_structure(&huge, tiny).expect_err("over the byte budget");
        assert!(matches!(error, StructureError::BadNbt { .. }), "{error:?}");
    }

    #[test]
    fn the_reader_still_accepts_a_sixty_four_axis_size() {
        // The documented boundary, asserted so a future tightening is a test
        // failure rather than a silently dropped datapack.
        assert_eq!(StructureLimits::PACK.max_axis, 64);
        let limits = StructureLimits::with_max_axis(128);
        assert_eq!(limits.max_axis, 128);
        assert_eq!(
            limits.nbt_limits().max_bytes,
            StructureLimits::PACK.max_decoded_bytes
        );
        assert_eq!(limits.nbt_limits().max_depth, 32);
        let _ = NbtLimits::DISK;
    }
}
