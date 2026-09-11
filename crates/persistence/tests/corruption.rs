//! Corruption and failure-recovery tests (P03-15).
//!
//! A region file is untrusted input: it can be truncated by a power loss, edited
//! by a tool, or written by a buggy mod. The Phase 03 exit gate requires that
//! malformed data "fails safely and observably" — meaning:
//!
//! 1. an error is returned (never a panic, never a partial chunk);
//! 2. the error class distinguishes corruption from an unsupported feature;
//! 3. a healthy chunk in the same file stays readable, so one bad slot does not
//!    take the world down.
//!
//! Every case below is built by hand from the format description, not by
//! mutating a fixture, so the tests document the layout as much as the code.

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
use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::compression::Compression;
use mc_persistence::dimension::Dimension;
use mc_persistence::level::LevelDat;
use mc_persistence::region::{HEADER_BYTES, RegionFile, SECTOR_BYTES};
use mc_persistence::world::WorldStorage;
use mc_test_support::fixtures::{TempDir, read_fixture};

/// Chunk used by every synthetic region file.
const POS: ChunkPos = ChunkPos::new(2, 3);
/// A healthy chunk that must stay readable next to a corrupted one.
const HEALTHY: ChunkPos = ChunkPos::new(4, 4);

fn payload_for(bytes: &[u8]) -> Vec<u8> {
    let compressed = Compression::Zlib.compress(bytes).expect("compresses");
    let mut body = Vec::new();
    body.extend_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
    body.push(Compression::Zlib.id());
    body.extend_from_slice(&compressed);
    body
}

fn pad_to_sector(body: &[u8]) -> Vec<u8> {
    let sectors = body.len().div_ceil(SECTOR_BYTES).max(1);
    let mut out = body.to_vec();
    out.resize(sectors * SECTOR_BYTES, 0);
    out
}

/// Build a region file: header + payloads at the given sectors.
fn build_region(location: Option<(usize, u32, u32)>, bodies: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut file = vec![0u8; HEADER_BYTES];
    if let Some((slot, raw_offset, timestamp)) = location {
        file[slot * 4..slot * 4 + 4].copy_from_slice(&raw_offset.to_be_bytes());
        file[SECTOR_BYTES + slot * 4..SECTOR_BYTES + slot * 4 + 4]
            .copy_from_slice(&timestamp.to_be_bytes());
    } else {
        // Default: slot of POS at sector 2, one sector.
        let slot = POS.slot();
        let count = (bodies
            .first()
            .map_or(1, |(_, body)| body.len().div_ceil(SECTOR_BYTES)))
        .max(1) as u32;
        file[slot * 4..slot * 4 + 4].copy_from_slice(&((2u32 << 8) | count).to_be_bytes());
    }
    for (sector, body) in bodies {
        let start = *sector as usize * SECTOR_BYTES;
        if file.len() < start {
            file.resize(start, 0);
        }
        file.extend_from_slice(body);
        let padded = file.len().div_ceil(SECTOR_BYTES) * SECTOR_BYTES;
        file.resize(padded, 0);
    }
    file
}

fn write_temp(tag: &str, bytes: &[u8]) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new(tag);
    let path = dir.path().join("r.0.0.mca");
    std::fs::write(&path, bytes).expect("write region");
    (dir, path)
}

/// A chunk encoded the same way the writer would store it.
fn encoded_chunk() -> Vec<u8> {
    ChunkData::empty(POS, -4, 2)
        .to_nbt_bytes()
        .expect("encodes")
}

#[test]
fn a_truncated_header_is_corruption_not_a_panic() {
    for length in [1usize, 100, HEADER_BYTES - 1] {
        let (_dir, path) = write_temp("corrupt-header", &vec![0u8; length]);
        let err = RegionFile::open(&path).expect_err("must refuse a short header");
        assert!(
            matches!(err, ServerError::CorruptData(_)),
            "length {length}: {err:?}"
        );
        assert!(format!("{err}").contains("header"), "{err}");
    }
    // Exactly 8 KiB of zeros is a valid, empty region.
    let (_dir, path) = write_temp("corrupt-header-ok", &vec![0u8; HEADER_BYTES]);
    let mut region = RegionFile::open(&path).expect("opens");
    assert_eq!(region.header().occupied_count(), 0);
    assert!(region.read_chunk(POS).expect("reads").is_none());
}

#[test]
fn a_slot_pointing_into_the_header_is_rejected() {
    for sector in [0u32, 1] {
        let body = pad_to_sector(&payload_for(&encoded_chunk()));
        let bytes = build_region(Some((POS.slot(), (sector << 8) | 1, 7)), &[(sector, body)]);
        let (_dir, path) = write_temp("corrupt-overlap", &bytes);
        let mut region = RegionFile::open(&path).expect("header is fine");
        let err = region.read_chunk(POS).expect_err("must refuse");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
        assert!(
            format!("{err}").contains("overlaps the region header"),
            "{err}"
        );
    }
}

#[test]
fn a_zero_sector_count_is_rejected() {
    let bytes = build_region(Some((POS.slot(), 2 << 8, 7)), &[]);
    let (_dir, path) = write_temp("corrupt-zero-count", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("zero sectors"), "{err}");
}

#[test]
fn a_slot_pointing_past_the_end_of_the_file_is_rejected() {
    let body = pad_to_sector(&payload_for(&encoded_chunk()));
    // Claim a sector far past the end of a three-sector file.
    let bytes = build_region(Some((POS.slot(), (100u32 << 8) | 1, 7)), &[(2, body)]);
    let (_dir, path) = write_temp("corrupt-oob", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("past the end"), "{err}");
}

#[test]
fn a_short_final_sector_is_tolerated_like_vanilla() {
    // Regression test for a real finding: a save interrupted before
    // `RegionFile.close` leaves the region file unpadded, so the last chunk
    // starts inside a *partial* sector (observed on a vanilla 26.1.2 world:
    // r.-1.-1.mca was 141 sectors plus 348 bytes). Vanilla reads such a chunk
    // when the declared payload is present, so we must too — refusing it would
    // make us reject a world vanilla loads.
    let body = payload_for(&encoded_chunk());
    let allocation = body.len().div_ceil(SECTOR_BYTES).max(1) as u32;
    let mut file = vec![0u8; HEADER_BYTES];
    let slot = POS.slot();
    file[slot * 4..slot * 4 + 4].copy_from_slice(&((2u32 << 8) | allocation).to_be_bytes());
    file.extend_from_slice(&body); // deliberately NOT padded to a whole sector
    assert!(
        file.len() < HEADER_BYTES + allocation as usize * SECTOR_BYTES,
        "the file must be shorter than its allocation"
    );

    let (_dir, path) = write_temp("corrupt-short-sector", &file);
    let mut region = RegionFile::open(&path).expect("opens");
    let stored = region
        .read_chunk(POS)
        .expect("a short final sector is not corruption")
        .expect("present");
    assert_eq!(
        ChunkData::from_nbt_bytes(&stored.data).expect("decodes"),
        ChunkData::empty(POS, -4, 2)
    );
    // The partial sector still counts as allocated, so the writer must not hand
    // it out to a new chunk.
    assert!(region.allocated_sectors() >= 2 + allocation);
}

#[test]
fn a_length_field_larger_than_the_allocated_sectors_is_rejected() {
    let mut body = payload_for(&encoded_chunk());
    // Inflate the declared length; the payload stays the same size.
    let claimed = (body.len() + SECTOR_BYTES) as u32;
    body[..4].copy_from_slice(&claimed.to_be_bytes());
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-length", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("length field claims"), "{err}");
}

#[test]
fn a_zero_length_field_is_rejected() {
    let mut body = payload_for(&encoded_chunk());
    body[..4].copy_from_slice(&0u32.to_be_bytes());
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-zero-length", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("length field is 0"), "{err}");
}

#[test]
fn an_unknown_compression_id_is_rejected() {
    let mut body = payload_for(&encoded_chunk());
    body[4] = 9; // no codec with id 9
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-codec", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(
        format!("{err}").contains("unknown compression id 9"),
        "{err}"
    );
}

#[test]
fn unsupported_codecs_and_external_chunks_report_what_is_missing() {
    // id 4 = lz4: recognised, deliberately not implemented.
    let mut body = payload_for(&encoded_chunk());
    body[4] = Compression::Lz4.id();
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-lz4", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::Operational(_)), "{err:?}");
    assert!(format!("{err}").contains("not implemented"), "{err}");

    // 0x80 flag = payload lives in an external .mcc file.
    let mut body = payload_for(&encoded_chunk());
    body[4] = 0x80 | Compression::Zlib.id();
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-external", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::Operational(_)), "{err:?}");
    assert!(format!("{err}").contains(".mcc"), "{err}");
}

#[test]
fn a_truncated_payload_fails_without_publishing_a_chunk() {
    let body = pad_to_sector(&payload_for(&encoded_chunk()));
    let bytes = build_region(None, &[(2, body)]);
    // Keep the header plus six bytes of the payload sector: the length field
    // survives, the payload it promises does not.
    let truncated = &bytes[..HEADER_BYTES + 6];
    let (_dir, path) = write_temp("corrupt-truncated", truncated);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("available"), "{err}");

    // A file cut before the 5-byte prefix is rejected too.
    let truncated = &bytes[..HEADER_BYTES + 4];
    let (_dir, path) = write_temp("corrupt-truncated-prefix", truncated);
    let mut region = RegionFile::open(&path).expect("opens");
    let err = region.read_chunk(POS).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("need at least 5"), "{err}");
}

#[test]
fn a_bit_flip_inside_the_nbt_is_detected_downstream() {
    // zlib notices most bit flips itself; when it does not, the NBT reader must.
    // Either way the caller sees an error, never a half-decoded chunk.
    let chunk = ChunkData::empty(POS, -4, 4);
    let encoded = chunk.to_nbt_bytes().expect("encodes");
    let mut body = payload_for(&encoded);
    let flip_at = body.len() / 2;
    body[flip_at] ^= 0xFF;
    let bytes = build_region(None, &[(2, pad_to_sector(&body))]);
    let (_dir, path) = write_temp("corrupt-bitflip", &bytes);
    let mut region = RegionFile::open(&path).expect("opens");
    match region.read_chunk(POS) {
        Err(ServerError::CorruptData(_) | ServerError::Protocol(_)) => {}
        Ok(None) => panic!("corrupt chunk must not vanish silently"),
        Ok(Some(stored)) => {
            // Decompression succeeded; the NBT layer must reject it.
            let decoded = ChunkData::from_nbt_bytes(&stored.data);
            assert!(
                decoded.is_err(),
                "a bit flip must not decode to a valid chunk"
            );
        }
        Err(other) => panic!("unexpected error class: {other:?}"),
    }
}

#[test]
fn one_corrupt_slot_does_not_hide_a_healthy_one() {
    let good = pad_to_sector(&payload_for(&encoded_chunk()));
    let mut broken = payload_for(
        &ChunkData::empty(HEALTHY, -4, 1)
            .to_nbt_bytes()
            .expect("encodes"),
    );
    broken[..4].copy_from_slice(&u32::MAX.to_be_bytes()); // absurd length
    let broken = pad_to_sector(&broken);

    let mut file = vec![0u8; HEADER_BYTES];
    for (slot, sector, count) in [
        (POS.slot(), 2u32, good.len().div_ceil(SECTOR_BYTES) as u32),
        (
            HEALTHY.slot(),
            2 + good.len().div_ceil(SECTOR_BYTES) as u32,
            broken.len().div_ceil(SECTOR_BYTES) as u32,
        ),
    ] {
        file[slot * 4..slot * 4 + 4].copy_from_slice(&((sector << 8) | count).to_be_bytes());
    }
    file.extend_from_slice(&good);
    file.extend_from_slice(&broken);

    let (_dir, path) = write_temp("corrupt-isolated", &file);
    let mut region = RegionFile::open(&path).expect("opens");
    assert!(
        region.read_chunk(HEALTHY).is_err(),
        "the corrupt slot reports its own failure"
    );
    let stored = region.read_chunk(POS).expect("reads").expect("present");
    assert_eq!(
        ChunkData::from_nbt_bytes(&stored.data).expect("decodes"),
        ChunkData::empty(POS, -4, 2)
    );
}

#[test]
fn corrupt_level_dat_is_reported_and_never_silently_reset() {
    let dir = TempDir::new("corrupt-level");
    let root = dir.path().join("world");
    std::fs::create_dir_all(&root).expect("mkdir");

    // Not gzip at all.
    std::fs::write(root.join("level.dat"), b"garbage").expect("write");
    let err = WorldStorage::open(&root, 0).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");

    // Valid gzip, but truncated NBT inside.
    let level = LevelDat::new("world", 0);
    let mut bytes = mc_persistence::save::encode_gzip_nbt("", &level.to_nbt()).expect("encodes");
    bytes.truncate(bytes.len() / 2);
    std::fs::write(root.join("level.dat"), &bytes).expect("write");
    let err = WorldStorage::open(&root, 0).expect_err("must refuse");
    assert!(
        matches!(err, ServerError::CorruptData(_) | ServerError::Protocol(_)),
        "{err:?}"
    );

    // Valid gzip, valid NBT, but no `Data` compound.
    let bytes = mc_persistence::save::encode_gzip_nbt(
        "",
        &mc_nbt::NbtTag::Compound(vec![("NotData".to_owned(), mc_nbt::NbtTag::Int(1))]),
    )
    .expect("encodes");
    std::fs::write(root.join("level.dat"), bytes).expect("write");
    let err = WorldStorage::open(&root, 0).expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    assert!(format!("{err}").contains("Data"), "{err}");
}

#[test]
fn a_corrupt_chunk_is_reported_through_world_storage() {
    let dir = TempDir::new("corrupt-world");
    let root = dir.path().join("world");
    {
        let mut storage = WorldStorage::create_or_open(&root, "world", 0).expect("creates");
        storage
            .queue_chunk_save(&Dimension::Overworld, &ChunkData::empty(POS, -4, 2))
            .expect("queues");
        storage.flush().expect("flushes");
        storage.close().expect("closes");
    }
    let region_path = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    // Zero the location of the chunk's slot: the block becomes unreferenced.
    let mut bytes = std::fs::read(&region_path).expect("read region");
    let slot = POS.slot();
    bytes[slot * 4..slot * 4 + 4].copy_from_slice(&0u32.to_be_bytes());
    std::fs::write(&region_path, &bytes).expect("write region");

    let mut storage = WorldStorage::open(&root, 0).expect("world still opens");
    assert!(
        storage
            .read_chunk(&Dimension::Overworld, POS)
            .expect("reads")
            .is_none(),
        "a cleared slot is an empty chunk, not corruption"
    );

    // Now point the slot at a bogus sector: that is corruption.
    let mut bytes = std::fs::read(&region_path).expect("read region");
    bytes[slot * 4..slot * 4 + 4].copy_from_slice(&((9_999u32 << 8) | 1).to_be_bytes());
    std::fs::write(&region_path, &bytes).expect("write region");
    let mut storage = WorldStorage::open(&root, 0).expect("world still opens");
    let err = storage
        .read_chunk(&Dimension::Overworld, POS)
        .expect_err("must refuse");
    assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
}

#[test]
fn the_vanilla_fixture_survives_a_corrupt_neighbour() {
    // Real-world shape: a healthy vanilla region file plus an injected bad slot.
    let dir = TempDir::new("corrupt-next-to-vanilla");
    let path = dir.path().join("r.-2.-1.mca");
    let mut bytes = read_fixture("anvil", "region_26_1_2.mca").expect("fixture");
    let bad_slot = ChunkPos::new(-64, -32).slot(); // empty in the fixture
    bytes[bad_slot * 4..bad_slot * 4 + 4].copy_from_slice(&((3u32 << 8) | 2).to_be_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let mut region = RegionFile::open(&path).expect("opens");
    assert!(
        region.read_chunk(ChunkPos::new(-64, -32)).is_err(),
        "the injected slot must fail loudly"
    );
    // The vanilla chunk is untouched (its slot is 283, far from the injected one).
    let stored = region
        .read_chunk(ChunkPos::new(-37, -24))
        .expect("reads")
        .expect("present");
    let chunk = ChunkData::from_nbt_bytes(&stored.data).expect("decodes");
    assert_eq!(chunk.status, "minecraft:full");
    assert_eq!(chunk.sections.len(), 24);
}

#[test]
fn writes_to_a_read_only_directory_fail_cleanly_and_stay_queued() {
    // A save that cannot happen must keep the data queued and say so.
    let dir = TempDir::new("corrupt-readonly");
    let root = dir.path().join("world");
    let mut storage = WorldStorage::create_or_open(&root, "world", 0).expect("creates");
    let chunk = ChunkData::empty(ChunkPos::new(1, 1), -4, 1);
    storage
        .queue_chunk_save(&Dimension::Overworld, &chunk)
        .expect("queues");

    // Replace the region file with a directory so the open fails.
    let region_dir = root.join("dimensions/minecraft/overworld/region");
    std::fs::create_dir_all(&region_dir).expect("mkdir");
    std::fs::create_dir(region_dir.join("r.0.0.mca")).expect("block the region path");

    let report = storage.flush().expect("flush itself does not abort");
    assert!(!report.is_clean(), "{report:?}");
    assert_eq!(report.chunks_failed, 1);
    assert_eq!(report.chunks_written, 0);
    assert_eq!(
        storage.pending_chunk_count(),
        1,
        "an unwritten chunk must stay queued for the next attempt"
    );
    assert!(
        storage
            .dirty()
            .contains(&Dimension::Overworld, ChunkPos::new(1, 1))
    );

    // Removing the obstruction lets the next flush succeed.
    std::fs::remove_dir(region_dir.join("r.0.0.mca")).expect("unblock");
    let report = storage.flush().expect("second flush");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.chunks_written, 1);
    assert_eq!(storage.pending_chunk_count(), 0);
}
