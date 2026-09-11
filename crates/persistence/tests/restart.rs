//! Restart/load integration tests (P03-14).
//!
//! The Phase 03 exit gate is "create/load/save/restart preserves a verified
//! world slice". These tests build a world through the public API, drop every
//! handle, reopen the directory from scratch, and compare the semantic content
//! — not the bytes — so an implementation change cannot quietly lose state.

// Binary-format tests narrow and widen integers (sector counts, bit widths,
// chunk coordinates) and keep long end-to-end scenario functions readable; the
// same exemption is documented in the crate roots.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines
)]

use mc_persistence::autosave::AutosaveScheduler;
use mc_persistence::chunk::{BlockState, ChunkData, ChunkPos, LIGHT_BYTES};
use mc_persistence::dimension::Dimension;
use mc_persistence::level::{DATA_VERSION_26_1_2, Difficulty, LevelDat};
use mc_persistence::world::WorldStorage;
use mc_test_support::fixtures::TempDir;

/// A chunk with content in several sections, so a round trip has something to
/// lose: palettes above the 4-bit minimum, both light arrays, a heightmap,
/// block/fluid ticks and both entity lists.
fn rich_chunk(pos: ChunkPos) -> ChunkData {
    let mut chunk = ChunkData::empty(pos, -4, 24);
    let section = &mut chunk.sections[4]; // y = 0
    for index in 0..16 * 16 * 16 {
        let name = match index % 5 {
            0 => "minecraft:stone",
            1 => "minecraft:dirt",
            2 => "minecraft:grass_block",
            3 => "minecraft:oak_log",
            _ => "minecraft:air",
        };
        section
            .block_states
            .set(index, BlockState::new(name))
            .expect("sets a block state");
    }
    for index in 0..16 {
        section
            .biomes
            .set(index, format!("minecraft:biome_{index}"))
            .expect("sets a biome");
    }
    section.sky_light = Some(vec![0xAB; LIGHT_BYTES]);
    section.block_light = Some(vec![0x0F; LIGHT_BYTES]);
    chunk.sections[5]
        .block_states
        .set(
            1234,
            BlockState {
                name: "minecraft:oak_log".to_owned(),
                properties: vec![("axis".to_owned(), "y".to_owned())],
            },
        )
        .expect("sets an oriented block state");
    chunk.heightmaps.push((
        "MOTION_BLOCKING".to_owned(),
        (0..37).map(|i| i64::from(i) * 3).collect(),
    ));
    chunk.inhabited_time = 4242;
    chunk.last_update = 77;
    chunk.light_correct = true;
    chunk.block_entities.push(mc_nbt::NbtTag::Compound(vec![
        (
            "id".to_owned(),
            mc_nbt::NbtTag::String("minecraft:chest".to_owned()),
        ),
        ("x".to_owned(), mc_nbt::NbtTag::Int(pos.x * 16)),
    ]));
    chunk.entities.push(mc_nbt::NbtTag::Compound(vec![(
        "id".to_owned(),
        mc_nbt::NbtTag::String("minecraft:pig".to_owned()),
    )]));
    chunk.block_ticks.push(mc_nbt::NbtTag::Compound(vec![(
        "i".to_owned(),
        mc_nbt::NbtTag::String("minecraft:water".to_owned()),
    )]));
    chunk.fluid_ticks.push(mc_nbt::NbtTag::Compound(vec![(
        "i".to_owned(),
        mc_nbt::NbtTag::String("minecraft:lava".to_owned()),
    )]));
    chunk.post_processing = vec![vec![11, 22], vec![]];
    chunk
}

#[test]
fn save_then_reopen_preserves_the_world_slice() {
    let dir = TempDir::new("restart-slice");
    let root = dir.path().join("world");

    // --- session 1: create the world and write two dimensions --------------
    let (level_snapshot, chunks) = {
        let mut storage =
            WorldStorage::create_or_open(&root, "restart world", 6000).expect("creates the world");
        let mut level = storage.level().cloned().expect("level cached");
        level.difficulty = Difficulty::Normal;
        level.initialized = true;
        level.spawn.x = -592;
        level.spawn.y = 67;
        level.spawn.z = -384;
        level.time = 24_000;
        storage.save_level(level.clone()).expect("saves level");

        let chunks = vec![
            (Dimension::Overworld, rich_chunk(ChunkPos::new(-37, -24))),
            (Dimension::Overworld, rich_chunk(ChunkPos::new(-1, -1))),
            (Dimension::Overworld, rich_chunk(ChunkPos::new(40, 3))),
            (Dimension::Nether, rich_chunk(ChunkPos::new(0, 0))),
        ];
        for (dimension, chunk) in &chunks {
            storage
                .queue_chunk_save(dimension, chunk)
                .expect("queues the chunk");
        }
        assert_eq!(storage.pending_chunk_count(), chunks.len());
        assert_eq!(storage.dirty().chunk_count(), chunks.len());

        let report = storage.flush().expect("flush succeeds");
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.chunks_written, chunks.len());
        assert_eq!(storage.pending_chunk_count(), 0, "queue drains on success");
        assert!(storage.dirty().is_empty(), "dirty set drains on success");

        let report = storage.close().expect("clean close");
        assert!(report.is_clean(), "{report:?}");
        (level, chunks)
    };

    // Files must exist in the 26.1 layout.
    assert!(root.join("level.dat").is_file());
    assert!(
        root.join("dimensions/minecraft/overworld/region/r.-2.-1.mca")
            .is_file(),
        "overworld region file"
    );
    assert!(
        root.join("dimensions/minecraft/overworld/region/r.1.0.mca")
            .is_file(),
        "second overworld region"
    );
    assert!(
        root.join("dimensions/minecraft/the_nether/region/r.0.0.mca")
            .is_file(),
        "nether region file"
    );

    // --- session 2: reopen from scratch ------------------------------------
    let mut storage = WorldStorage::open(&root, 6000).expect("reopens");
    let level = storage.level().cloned().expect("level loaded");
    assert_eq!(level.data_version, DATA_VERSION_26_1_2);
    assert_eq!(level.level_name, "restart world");
    assert_eq!(level.difficulty, Difficulty::Normal);
    assert!(level.initialized);
    assert_eq!(
        (level.spawn.x, level.spawn.y, level.spawn.z),
        (-592, 67, -384)
    );
    assert_eq!(level.time, 24_000);
    assert_eq!(level, level_snapshot, "level.dat survived the restart");
    // The startup `create_or_open` writes an initial level.dat, so a second
    // save makes the backup meaningful.
    assert!(
        root.join("level.dat_old").is_file(),
        "the previous level.dat is kept as level.dat_old"
    );

    for (dimension, expected) in &chunks {
        let loaded = storage
            .read_chunk(dimension, expected.pos)
            .expect("reads")
            .expect("present");
        assert_eq!(
            loaded, *expected,
            "chunk {:?} of {dimension} must survive the restart",
            expected.pos
        );
        // Spot-check the content that a naive implementation would drop.
        assert_eq!(
            loaded.non_empty_section_count(),
            expected.non_empty_section_count()
        );
        assert_eq!(loaded.heightmaps, expected.heightmaps);
        assert_eq!(loaded.block_entities, expected.block_entities);
        assert_eq!(loaded.entities, expected.entities);
        assert_eq!(loaded.block_ticks, expected.block_ticks);
        assert_eq!(loaded.fluid_ticks, expected.fluid_ticks);
        assert_eq!(loaded.post_processing, expected.post_processing);
        assert_eq!(
            loaded.sections[4].sky_light.as_deref(),
            expected.sections[4].sky_light.as_deref()
        );
        assert_eq!(loaded.sections[5].block_states.palette().len(), 2);
        let prop = loaded.sections[5]
            .block_states
            .to_values()
            .expect("unpacks");
        assert_eq!(
            prop[1234].properties,
            vec![("axis".to_owned(), "y".to_owned())]
        );
    }
    assert!(storage.dirty().is_empty());
}

#[test]
fn a_chunk_written_by_one_session_is_visible_to_a_second_writer() {
    let dir = TempDir::new("restart-append");
    let root = dir.path().join("world");
    let pos = ChunkPos::new(5, 5);

    {
        let mut storage = WorldStorage::create_or_open(&root, "world", 0).expect("creates");
        storage
            .queue_chunk_save(&Dimension::Overworld, &rich_chunk(pos))
            .expect("queues");
        storage.flush().expect("flushes");
        storage.close().expect("closes");
    }
    {
        let mut storage = WorldStorage::open(&root, 0).expect("reopens");
        assert!(
            storage
                .has_chunk(&Dimension::Overworld, pos)
                .expect("probe")
        );
        // Add a second chunk to the same region file and re-read both.
        let other = ChunkPos::new(6, 5);
        storage
            .queue_chunk_save(&Dimension::Overworld, &rich_chunk(other))
            .expect("queues");
        storage.flush().expect("flushes");
        assert!(
            storage
                .read_chunk(&Dimension::Overworld, pos)
                .expect("reads")
                .is_some()
        );
        assert!(
            storage
                .read_chunk(&Dimension::Overworld, other)
                .expect("reads")
                .is_some()
        );
        let removed = storage
            .remove_chunk(&Dimension::Overworld, pos)
            .expect("removes");
        assert!(removed);
        assert!(
            storage
                .read_chunk(&Dimension::Overworld, pos)
                .expect("reads")
                .is_none()
        );
        storage.close().expect("closes");
    }
    let mut storage = WorldStorage::open(&root, 0).expect("reopens again");
    assert!(
        storage
            .read_chunk(&Dimension::Overworld, pos)
            .expect("reads")
            .is_none()
    );
    assert!(
        storage
            .read_chunk(&Dimension::Overworld, ChunkPos::new(6, 5))
            .expect("reads")
            .is_some()
    );
}

#[test]
fn level_dat_backup_holds_the_previous_version() {
    let dir = TempDir::new("restart-backup");
    let root = dir.path().join("world");
    let mut storage = WorldStorage::create_or_open(&root, "world", 0).expect("creates");

    let first = storage.level().cloned().expect("level");
    let mut second = first.clone();
    second.time = 12_345;
    second.level_name = "renamed".to_owned();
    storage.save_level(second.clone()).expect("second save");

    let from_disk =
        LevelDat::from_bytes(&std::fs::read(root.join("level.dat")).expect("read level"))
            .expect("decodes");
    let from_backup =
        LevelDat::from_bytes(&std::fs::read(root.join("level.dat_old")).expect("read backup"))
            .expect("decodes backup");
    assert_eq!(from_disk, second);
    assert_eq!(from_backup, first, "level.dat_old is the previous document");
    assert!(!root.join("level.dat.tmp").exists());
    storage.close().expect("closes");
}

#[test]
fn queued_chunks_are_written_by_the_autosave_scheduler() {
    let dir = TempDir::new("restart-autosave");
    let root = dir.path().join("world");
    let mut storage = WorldStorage::create_or_open(&root, "world", 100).expect("creates");
    assert!(storage.autosave().is_enabled());

    storage
        .queue_chunk_save(&Dimension::Overworld, &rich_chunk(ChunkPos::new(2, 2)))
        .expect("queues");

    // Nothing is due before the interval elapses.
    for tick in 0..100 {
        assert!(
            storage.on_tick(tick).expect("tick").is_none(),
            "tick {tick} must not save"
        );
    }
    assert_eq!(storage.pending_chunk_count(), 1, "still queued");
    let report = storage
        .on_tick(100)
        .expect("tick ok")
        .expect("save is due at tick 100");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.chunks_written, 1);
    assert_eq!(storage.pending_chunk_count(), 0);
    assert!(storage.on_tick(100).expect("tick").is_none(), "fires once");
    assert!(storage.on_tick(199).expect("tick").is_none());
    assert!(
        storage.on_tick(200).expect("tick").is_some(),
        "next interval"
    );

    // A disabled scheduler never fires.
    storage.autosave_mut().set_interval(0, 200);
    storage
        .queue_chunk_save(&Dimension::Overworld, &rich_chunk(ChunkPos::new(3, 3)))
        .expect("queues");
    for tick in 200..10_000 {
        assert!(storage.on_tick(tick).expect("tick").is_none());
    }
    assert_eq!(storage.pending_chunk_count(), 1);
    // Explicit flush still works with autosave disabled.
    let report = storage.flush().expect("explicit flush");
    assert_eq!(report.chunks_written, 1);
    storage.close().expect("closes");
}

#[test]
fn region_handle_cache_is_bounded_and_eviction_is_safe() {
    let dir = TempDir::new("restart-handles");
    let root = dir.path().join("world");
    let mut storage = WorldStorage::create_or_open(&root, "world", 0).expect("creates");
    storage.set_max_open_regions(2);
    assert_eq!(storage.max_open_regions(), 2);

    // Five distinct regions across two dimensions.
    let positions = [
        ChunkPos::new(0, 0),
        ChunkPos::new(32, 0),
        ChunkPos::new(64, 0),
        ChunkPos::new(0, 32),
        ChunkPos::new(-32, -32),
    ];
    for pos in positions {
        storage
            .queue_chunk_save(&Dimension::Overworld, &rich_chunk(pos))
            .expect("queues");
    }
    storage.flush().expect("flushes");
    assert!(
        storage.open_region_count() <= 2,
        "cache must respect its cap, has {}",
        storage.open_region_count()
    );
    // Everything still readable: eviction must not lose anything.
    for pos in positions {
        assert!(
            storage
                .read_chunk(&Dimension::Overworld, pos)
                .expect("reads")
                .is_some(),
            "{pos:?} must survive handle eviction"
        );
    }
    assert!(!storage.dirty().is_level_dirty());
    storage.close().expect("closes");
}

#[test]
fn autosave_scheduler_matches_the_configured_interval() {
    // The scheduler is part of the persistence contract: same ticks, same saves.
    let mut scheduler = AutosaveScheduler::new(6000);
    assert_eq!(scheduler.interval_ticks(), 6000);
    let mut fired = Vec::new();
    for tick in 0..18_001 {
        if scheduler.on_tick(tick) {
            fired.push(tick);
        }
    }
    assert_eq!(fired, vec![6000, 12_000, 18_000]);
}

#[test]
fn a_world_without_level_dat_is_created_cleanly() {
    let dir = TempDir::new("restart-fresh");
    let root = dir.path().join("brand-new-world");
    let storage = WorldStorage::open(&root, 6000).expect("opens a missing directory");
    assert!(storage.level().is_none(), "no level yet");
    assert_eq!(
        storage.level_name(),
        "world",
        "fallback name before creation"
    );
    assert!(root.is_dir(), "the world directory is created");
    drop(storage);

    let storage = WorldStorage::create_or_open(&root, "fresh", 6000).expect("creates");
    let level = storage.level().expect("level created");
    assert_eq!(level.level_name, "fresh");
    assert_eq!(level.data_version, DATA_VERSION_26_1_2);
    assert!(!level.initialized);
    assert_eq!(level.difficulty, Difficulty::Easy);
    assert!(root.join("level.dat").is_file());
    storage.close().expect("closes");
}
