//! AUDIT-09 B-01's regression test: a chunk that is **stored but unreadable**
//! must never be generated over.
//!
//! The defect: a failed chunk read left `loaded == false`, and the generation
//! gate only asked `can_read_stored_chunks()` (which is "does this game own
//! storage"), so an owning game generated terrain over a file it could not
//! read. The generated chunk is clean, so the file survived until the first
//! edit made the chunk dirty and the next autosave replaced real terrain with
//! generated blocks — unrecoverable, and exactly the class
//! `load_or_create_chunk`'s own docs forbid.
//!
//! The test plants a chunk whose `DataVersion` is outside the readable window
//! (4435..=4790, `LevelDat::is_supported_data_version`), which is one of the
//! reachable read failures, then:
//!
//! 1. loads it in an owning game and asserts **no generated terrain** appears
//!    (a placeholder is all air; the generator would have put solid rock far
//!    below sea level in the same column);
//! 2. edits a block in that placeholder chunk (which marks it dirty), saves,
//!    and reopens the world — asserting the planted file is still there, still
//!    unreadable. The file surviving is the difference between "the guard
//!    works" and "the guard worked until somebody walked past".
//!
//! It is the test the remediation document named and the landing never wrote.

use mc_nbt::NbtTag;
use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::dimension::Dimension;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// A chunk that will refuse to read: the right position, a `DataVersion` the
/// window rejects, and one section so the file is non-trivial.
fn planted_chunk(pos: ChunkPos) -> ChunkData {
    ChunkData {
        pos,
        data_version: 5000, // above `DATA_VERSION_26_1_2` (4790)
        status: "minecraft:full".to_owned(),
        min_section_y: -4,
        last_update: 0,
        inhabited_time: 0,
        light_correct: false,
        sections: vec![mc_persistence::chunk::SectionData::filled(
            4,
            mc_persistence::chunk::BlockState::new("minecraft:stone"),
            "minecraft:plains",
        )],
        heightmaps: Vec::new(),
        block_entities: Vec::new(),
        entities: Vec::new(),
        block_ticks: Vec::new(),
        fluid_ticks: Vec::new(),
        post_processing: Vec::new(),
        structures: None,
        extra: vec![("_audit".to_owned(), NbtTag::Byte(1))],
    }
}

#[test]
fn an_unreadable_stored_chunk_is_never_generated_over_nor_replaced() {
    let dir = TempDir::new("p11-unreadable-chunk");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };

    // ---- find the spawn chunk and plant the unreadable file ---------------
    let mut storage = WorldService::open(&config).expect("world opens");
    let (sx, _sy, sz) = {
        let probe = mc_server::game::Game::new(&storage, 2, mc_network::bridge::game_channel(4).1)
            .expect("game builds");
        probe.spawn()
    };
    let pos = ChunkPos::new(sx >> 4, sz >> 4);
    storage
        .storage_mut()
        .queue_chunk_save(&Dimension::Overworld, &planted_chunk(pos))
        .expect("the planted chunk queues");
    storage
        .storage_mut()
        .flush()
        .expect("the planted chunk is on disk");
    storage.close().expect("the handle closes before reopening");

    // ---- the owning game loads over it ------------------------------------
    let storage = WorldService::open(&config).expect("the world reopens");
    let mut game = Game::with_seed_and_storage(
        storage,
        3,
        mc_network::bridge::game_channel(64).1,
        mc_server::game::DEFAULT_RANDOM_SEED,
    )
    .expect("game builds");
    assert!(
        game.load_chunk(pos),
        "the chunk loads -- as a placeholder, not as generated terrain"
    );
    // Generated terrain would have solid blocks well below sea level in this
    // column; a placeholder is air all the way. Assert a deep block, where the
    // generator's stone is unambiguous.
    let deep = -20;
    assert_eq!(
        game.world().get_block_loaded(sx, deep, sz),
        Some(0),
        "no terrain was generated over a file the server could not read"
    );

    // ---- the edit, the save, and the file's survival ----------------------
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    game.world_mut()
        .set_block(sx, deep + 1, sz, stone)
        .expect("the edit lands");
    game.save_all_owned().expect("the world saves");
    game.close_storage().expect("the handle closes");
    drop(game);

    // A third boot re-reads the same file. If the save had replaced the
    // unreadable chunk with a readable generated one, this read would now
    // *succeed* -- which is the data loss the guard exists to prevent.
    let mut storage = WorldService::open(&config).expect("the world reopens again");
    let read = storage
        .storage_mut()
        .read_chunk(&Dimension::Overworld, pos)
        .map(|option| option.is_some());
    assert!(
        !matches!(read, Ok(true)),
        "the planted chunk must still be unreadable after a dirty edit and a save; a readable \
         file here means the save replaced it (the data loss the guard exists to prevent)"
    );
    storage.close().expect("the handle closes");
}
