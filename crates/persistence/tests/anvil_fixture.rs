//! Golden tests against files produced by the **real** vanilla 26.1.2 server
//! (P03-T07, evidence level L4/L6).
//!
//! Fixtures (see `crates/test-support/fixtures/anvil/MANIFEST.txt`):
//!
//! - `level_26_1_2.dat` — a `level.dat` written by vanilla 26.1.2;
//! - `region_26_1_2.mca` — a derived region file: vanilla's 8 KiB header with
//!   one slot rewritten to sector 2, followed by the verbatim sectors of one
//!   `minecraft:full` chunk (x=-37, z=-24, slot 283).
//!
//! These tests are the difference between "our format is self-consistent" and
//! "our format matches Vanilla": they decode real bytes and check that
//! re-encoding reproduces them.

// Binary-format tests narrow and widen integers (sector counts, bit widths,
// chunk coordinates) and keep long end-to-end scenario functions readable; the
// same exemption is documented in the crate roots.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines
)]

use mc_core::error::ServerError;
use mc_core::packing;
use mc_core::packing::{BIOME_ENTRIES, BLOCK_ENTRIES};
use mc_nbt::{Limits, TagReader};
use mc_persistence::chunk::{ChunkData, ChunkPos, LIGHT_BYTES};
use mc_persistence::compression::Compression;
use mc_persistence::dimension::Dimension;
use mc_persistence::level::{DATA_VERSION_26_1_2, Difficulty, LEVEL_VERSION_26_1_2, LevelDat};
use mc_persistence::region::RegionFile;
use mc_persistence::save::decode_gzip_nbt;
use mc_persistence::world::WorldStorage;
use mc_test_support::fixtures::{TempDir, read_fixture};

/// Chunk stored in the fixture region file.
const FIXTURE_CHUNK: ChunkPos = ChunkPos::new(-37, -24);
/// Slot that chunk occupies (verified by dumping the vanilla header).
const FIXTURE_SLOT: usize = 283;

fn level_bytes() -> Vec<u8> {
    read_fixture("anvil", "level_26_1_2.dat").expect("level fixture")
}

fn region_bytes() -> Vec<u8> {
    read_fixture("anvil", "region_26_1_2.mca").expect("region fixture")
}

/// Copy the region fixture into a scratch directory so tests can write to it.
fn region_in_temp(tag: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new(tag);
    let path = dir.path().join("r.-2.-1.mca");
    std::fs::write(&path, region_bytes()).expect("write fixture");
    (dir, path)
}

#[test]
fn vanilla_level_dat_decodes_to_the_measured_values() {
    let bytes = level_bytes();
    let level = LevelDat::from_bytes(&bytes).expect("decodes");

    // Values observed in the vanilla file and cross-checked against the 26.1.2
    // server jar (`DetectedVersion.createBuiltIn` pushes 4790; `level.dat`
    // carries `version: 19133`).
    assert_eq!(level.data_version, 4790);
    assert_eq!(level.data_version, DATA_VERSION_26_1_2);
    assert_eq!(level.level_version, 19133);
    assert_eq!(level.level_version, LEVEL_VERSION_26_1_2);
    let version = level.version.as_ref().expect("Version compound");
    assert_eq!(version.id, 4790);
    assert_eq!(version.name, "26.1.2");
    assert_eq!(version.series, "main");
    assert!(!version.snapshot);
    assert_eq!(level.level_name, "world");
    assert_eq!(level.difficulty, Difficulty::Easy);
    assert!(!level.hardcore);
    assert!(level.initialized);
    assert_eq!(level.server_brands, vec!["vanilla".to_owned()]);
    assert_eq!(level.enabled_packs, vec!["vanilla".to_owned()]);
    assert_eq!(
        level.spawn.dimension, "minecraft:overworld",
        "26.1 stores the spawn dimension inline"
    );
    assert_eq!(
        level.spawn_dimension().expect("dimension"),
        Dimension::Overworld
    );
    assert!(level.spawn.y > 0, "a real spawn sits above bedrock");
    // The 26.1.2 model covers every field the vanilla writer emits, so nothing
    // falls through to the preserved-unknown bucket. (Older or modded files can
    // still populate `extra`; that path is covered by unit tests.)
    assert!(
        level.extra.is_empty(),
        "unmodelled fields in a vanilla level.dat: {:?}",
        level.extra.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
}

#[test]
fn level_dat_round_trips_semantically() {
    let level = LevelDat::from_bytes(&level_bytes()).expect("decodes");
    let re_encoded = mc_persistence::save::encode_gzip_nbt("", &level.to_nbt()).expect("encodes");
    let decoded_again = LevelDat::from_bytes(&re_encoded).expect("decodes again");
    assert_eq!(
        decoded_again, level,
        "a vanilla level.dat must survive load -> save -> load unchanged"
    );
}

#[test]
fn vanilla_region_header_matches_our_decoder() {
    let (_dir, path) = region_in_temp("fixture-header");
    let mut region = RegionFile::open_readonly(&path).expect("opens");
    assert_eq!(region.header().occupied_count(), 1);
    let location = region
        .header()
        .location(FIXTURE_SLOT)
        .expect("fixture slot is populated");
    assert_eq!(location.sector, 2, "fixture payload starts at sector 2");
    assert!(location.sector_count >= 1);
    assert!(region.has_chunk(FIXTURE_CHUNK));
    let stored = region
        .read_chunk(FIXTURE_CHUNK)
        .expect("reads")
        .expect("present");
    assert_eq!(
        stored.compression,
        Compression::Zlib,
        "vanilla's default codec is deflate"
    );
    assert!(!stored.data.is_empty());
    let _ = region.sync();
}

#[test]
fn vanilla_chunk_decodes_to_the_measured_shape() {
    let chunk = read_fixture_chunk();
    assert_eq!(chunk.pos, FIXTURE_CHUNK);
    assert_eq!(chunk.data_version, DATA_VERSION_26_1_2);
    assert_eq!(chunk.status, "minecraft:full");
    assert_eq!(
        chunk.min_section_y, -4,
        "1.18+ overworld starts at section -4"
    );
    assert_eq!(chunk.sections.len(), 24);
    assert_eq!(chunk.sections[0].y, -4);
    assert_eq!(chunk.sections[23].y, 19);
    assert!(chunk.light_correct, "a full chunk has authoritative light");
    assert!(chunk.non_empty_section_count() > 0, "terrain is present");

    // Heightmaps: vanilla writes all four types for a full chunk.
    let names: Vec<&str> = chunk
        .heightmaps
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    for expected in [
        "MOTION_BLOCKING",
        "MOTION_BLOCKING_NO_LEAVES",
        "OCEAN_FLOOR",
        "WORLD_SURFACE",
    ] {
        assert!(
            names.contains(&expected),
            "missing heightmap {expected}: {names:?}"
        );
    }
    for (name, values) in &chunk.heightmaps {
        // 256 heightmap entries at 9 bits, 7 per long, never spanning:
        // ceil(256 / 7) = 37 longs.
        assert_eq!(values.len(), 37, "heightmap {name} packing");
    }

    // Light arrays are half a byte per block.
    let lit = chunk
        .sections
        .iter()
        .filter(|section| section.sky_light.is_some())
        .count();
    assert!(lit > 0, "expected sky light in a full overworld chunk");
    for section in &chunk.sections {
        if let Some(light) = &section.sky_light {
            assert_eq!(light.len(), LIGHT_BYTES);
        }
        if let Some(light) = &section.block_light {
            assert_eq!(light.len(), LIGHT_BYTES);
        }
    }

    // `entities` was absent from this chunk in the vanilla file; the reader must
    // not invent it.
    assert!(chunk.entities.is_empty());
    assert!(chunk.block_entities.is_empty());
}

#[test]
fn vanilla_palette_packing_round_trips_byte_identically() {
    // This is the decisive check for the bit-packing rules: unpack the real
    // vanilla long arrays and repack them; any deviation in bit width,
    // LSB-first ordering or long-boundary handling changes the bytes.
    let chunk = read_fixture_chunk();
    let mut checked_palettes = 0;
    for section in &chunk.sections {
        check_palette_round_trip(
            &section.block_states,
            section.y,
            "block_states",
            &mut checked_palettes,
        );
        check_palette_round_trip(&section.biomes, section.y, "biomes", &mut checked_palettes);
    }
    assert!(
        checked_palettes >= 10,
        "expected the fixture to exercise several packed palettes, saw {checked_palettes}"
    );

    // Palette sizes measured in the fixture chunk: up to 18 block states (5 bits).
    let max_block_palette = chunk
        .sections
        .iter()
        .map(|section| section.block_states.palette().len())
        .max()
        .expect("sections");
    assert!(
        max_block_palette > 16,
        "fixture should exercise a >4-bit block palette, saw {max_block_palette}"
    );
    assert!(max_block_palette <= BLOCK_ENTRIES);
    let max_biome_palette = chunk
        .sections
        .iter()
        .map(|section| section.biomes.palette().len())
        .max()
        .expect("sections");
    assert!((1..=BIOME_ENTRIES).contains(&max_biome_palette));
}

#[test]
fn vanilla_chunk_survives_a_write_read_cycle() {
    // Re-encode the vanilla chunk through our writer and read it back through
    // our reader: the schema boundary must be lossless for real data.
    let chunk = read_fixture_chunk();
    let encoded = chunk.to_nbt_bytes().expect("encodes");
    let decoded = ChunkData::from_nbt_bytes(&encoded).expect("decodes");
    assert_eq!(decoded, chunk);

    // And through a whole region file, which is the path a save actually takes.
    let (_dir, path) = region_in_temp("fixture-rewrite");
    {
        let mut region = RegionFile::open(&path).expect("opens");
        region
            .write_chunk(chunk.pos, &encoded, Compression::Zlib, 12_345)
            .expect("writes");
        region.sync().expect("syncs");
    }
    let mut region = RegionFile::open(&path).expect("reopens");
    let stored = region
        .read_chunk(chunk.pos)
        .expect("reads")
        .expect("present");
    assert_eq!(stored.timestamp, 12_345);
    assert_eq!(
        ChunkData::from_nbt_bytes(&stored.data).expect("decodes"),
        chunk
    );
    assert_eq!(
        region.header().occupied_count(),
        1,
        "the fixture holds exactly one chunk"
    );
}

#[test]
fn a_vanilla_world_directory_loads_through_world_storage() {
    // Lay out the fixture as a real 26.1 world directory and open it with the
    // public API, which is what Phase 04 will do.
    let dir = TempDir::new("fixture-world");
    let root = dir.path().join("world");
    let region_dir = root
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    std::fs::create_dir_all(&region_dir).expect("mkdir");
    std::fs::write(root.join("level.dat"), level_bytes()).expect("level");
    std::fs::write(region_dir.join("r.-2.-1.mca"), region_bytes()).expect("region");

    let mut storage = WorldStorage::open(&root, 6000).expect("opens the vanilla world");
    assert_eq!(storage.level_name(), "world");
    let chunk = storage
        .read_chunk(&Dimension::Overworld, FIXTURE_CHUNK)
        .expect("reads")
        .expect("chunk present");
    assert_eq!(chunk.status, "minecraft:full");
    assert!(
        !storage
            .has_chunk(&Dimension::Overworld, ChunkPos::new(1000, 1000))
            .expect("probe")
    );
    // Reading must not have created or modified anything.
    assert_eq!(storage.pending_chunk_count(), 0);
    assert!(storage.dirty().is_empty());
}

#[test]
fn a_newer_data_version_is_refused_instead_of_misread() {
    let dir = TempDir::new("fixture-newer");
    let root = dir.path().join("world");
    std::fs::create_dir_all(&root).expect("mkdir");
    let mut level = LevelDat::from_bytes(&level_bytes()).expect("decodes");
    level.data_version = 4903; // 26.2, which this build must not interpret
    let bytes = mc_persistence::save::encode_gzip_nbt("", &level.to_nbt()).expect("encodes");
    std::fs::write(root.join("level.dat"), bytes).expect("level");
    let err = WorldStorage::open(&root, 6000).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("4903"), "{err}");
}

#[test]
fn the_golden_fixtures_are_actually_valid_nbt() {
    // Guards the fixtures themselves: a corrupted fixture must fail loudly here
    // rather than silently weakening every other test in this file.
    let raw = Compression::Gzip
        .decompress(&level_bytes(), Limits::DISK.max_bytes)
        .expect("level fixture is gzip");
    let mut reader = TagReader::new(&raw, Limits::DISK).expect("reader");
    let (root_name, root) = reader.read_named().expect("level fixture is NBT");
    assert_eq!(root_name, "", "vanilla level.dat has an empty root name");
    assert_eq!(reader.remaining(), 0, "no trailing bytes in level.dat");
    assert!(root.get_compound("Data").is_some());

    let region_raw = region_bytes();
    assert_eq!(region_raw.len(), 16_384, "fixture is header + 2 sectors");
    assert_eq!(
        u32::from_be_bytes([
            region_raw[FIXTURE_SLOT * 4],
            region_raw[FIXTURE_SLOT * 4 + 1],
            region_raw[FIXTURE_SLOT * 4 + 2],
            region_raw[FIXTURE_SLOT * 4 + 3],
        ]) >> 8,
        2,
        "fixture chunk must start at sector 2"
    );
    assert!(
        decode_gzip_nbt(&region_raw).is_err(),
        "region is not gzip NBT"
    );
}

fn read_fixture_chunk() -> ChunkData {
    let (_dir, path) = region_in_temp("fixture-chunk");
    let mut region = RegionFile::open_readonly(&path).expect("opens");
    let stored = region
        .read_chunk(FIXTURE_CHUNK)
        .expect("reads")
        .expect("present");
    ChunkData::from_nbt_bytes(&stored.data).expect("decodes the vanilla chunk")
}

/// Unpack a real vanilla palette and repack it, asserting byte equality.
///
/// Any deviation in bit width, LSB-first ordering or long-boundary handling
/// changes the bytes, so this single check validates the whole packing rule set
/// against Vanilla's own output.
fn check_palette_round_trip<T: Clone + PartialEq + std::fmt::Debug>(
    container: &mc_persistence::chunk::PalettedContainer<T>,
    section_y: i8,
    what: &str,
    checked: &mut usize,
) {
    let Some(original) = container.data() else {
        assert_eq!(
            container.palette().len(),
            1,
            "a single-value palette stores no data; anything else must"
        );
        return;
    };
    let values = container.to_values().expect("unpacks");
    assert_eq!(values.len(), container.entries());
    let indices: Vec<u32> = values
        .iter()
        .map(|value| {
            container
                .palette()
                .iter()
                .position(|entry| entry == value)
                .expect("value is in the palette") as u32
        })
        .collect();
    let repacked = packing::pack(&indices, container.bits()).expect("packs");
    assert_eq!(
        repacked,
        original,
        "section Y={section_y} {what} palette ({} entries, {} bits) must repack byte-identically",
        container.palette().len(),
        container.bits()
    );
    *checked += 1;
}
