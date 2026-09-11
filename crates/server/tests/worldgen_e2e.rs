//! World generation end to end (P07-13..P07-17, P07-18).
//!
//! `mc-worldgen`'s own tests prove the generator. This file proves the **integration**,
//! which is where the phase's stated priority lives: *"prioritize loading existing worlds
//! before perfecting every generation feature"*.
//!
//! # The bug this file found
//!
//! `Game::read_stored_chunk` returns `Ok(None)` when the game has **no storage handle**, so a
//! borrowing game could not tell "no chunk is stored here" from "I cannot look". Wiring the
//! generator into the not-loaded path turned that ambiguity into data loss: the generator
//! ran, marked its chunk dirty and wrote it over the saved one. Opening an existing world
//! with a borrowing game would have replaced it with generated terrain.
//!
//! Generation is now gated on `Game::can_read_stored_chunks`, and
//! `a_borrowing_game_never_overwrites_stored_terrain` is the regression test.
//!
//! So the tests that save and reload use `with_seed_and_storage`, the **production**
//! constructor: a borrowing game cannot even generate, by design.

use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// The overworld's block-y range, which the generator works over.
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;

fn config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    }
}

/// A game that **owns** storage — the production path, and the only one that may generate.
fn owning_game(dir: &TempDir, seed: i64) -> Game {
    let service = WorldService::open(&config(dir)).expect("the world opens");
    Game::with_seed_and_storage(service, 3, mc_network::bridge::game_channel(64).1, seed)
        .expect("game builds")
}

/// A game that only **borrows** storage: cannot generate, and must not overwrite.
fn borrowing_game(service: &WorldService, seed: i64) -> Game {
    Game::with_seed(service, 3, mc_network::bridge::game_channel(64).1, seed).expect("game")
}

fn non_air_count(game: &Game, pos: ChunkPos) -> usize {
    let air = game.registries().blocks.air_id();
    let chunk = game.world().chunk(pos).expect("loaded");
    let mut count = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MAX_Y {
                if chunk.get_block(x, y, z) != air {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Place a marker block above the surface and return where it went.
///
/// The search runs **downward from the top of the world**, not upward from y=0: sea level is
/// 63 and terrain commonly sits above it, so a range like `0..64` can be entirely solid. The
/// first version of these tests searched there and failed looking for air inside the ground.
fn place_marker(game: &mut Game, pos: ChunkPos) -> (i32, i32, i32, i32) {
    let gold = game
        .registries()
        .blocks
        .default_state("minecraft:gold_block")
        .expect("gold_block resolves");
    let air = game.registries().blocks.air_id();
    let chunk = game.world().chunk(pos).expect("chunk");

    let (x, y, z) = (0..16)
        .flat_map(|x| (0..16).map(move |z| (x, z)))
        .find_map(|(x, z)| {
            (MIN_Y..MAX_Y)
                .rev()
                .find(|y| chunk.get_block(x, *y, z) == air)
                .map(|y| (x, y, z))
        })
        .expect("a generated chunk has air above its surface");

    game.world_mut()
        .set_block(pos.x * 16 + x, y, pos.z * 16 + z, gold)
        .expect("the marker is placed");
    (x, y, z, gold)
}

#[test]
fn a_new_chunk_is_generated_rather_than_left_as_air() {
    // The bug this exists for: `ensure_chunk` produced an all-air chunk, so a player fell
    // through the world. Phase 04 recorded that as a known placeholder; this asserts it is
    // gone.
    let dir = TempDir::new("p07-gen");
    let mut game = owning_game(&dir, 0);
    let pos = ChunkPos::new(0, 0);
    assert!(game.load_chunk(pos), "a chunk is loaded");

    let chunk = game.world().chunk(pos).expect("loaded");
    let bedrock = game
        .registries()
        .blocks
        .default_state("minecraft:bedrock")
        .expect("bedrock resolves");
    assert!(
        (0..16).all(|x| (0..16).all(|z| chunk.get_block(x, MIN_Y, z) == bedrock)),
        "the dimension floor must be bedrock"
    );

    let non_air = non_air_count(&game, pos);
    assert!(
        non_air > 16 * 16,
        "a generated chunk must have terrain above its floor, found {non_air} non-air blocks"
    );

    // Every column is standable: a column that is all air is a hole a player falls through,
    // which is exactly the failure this replaced.
    let air = game.registries().blocks.air_id();
    for x in 0..16 {
        for z in 0..16 {
            assert!(
                (MIN_Y..MAX_Y).any(|y| chunk.get_block(x, y, z) != air),
                "column ({x},{z}) is entirely air"
            );
        }
    }
    game.save_all_owned().expect("save");
}

#[test]
fn generation_is_deterministic_across_two_games_with_one_seed() {
    let dir_a = TempDir::new("p07-det-a");
    let dir_b = TempDir::new("p07-det-b");
    let mut a = owning_game(&dir_a, 4242);
    let mut b = owning_game(&dir_b, 4242);

    let pos = ChunkPos::new(3, -7);
    assert!(a.load_chunk(pos));
    assert!(b.load_chunk(pos));
    let chunk_a = a.world().chunk(pos).expect("a");
    let chunk_b = b.world().chunk(pos).expect("b");

    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MAX_Y {
                assert_eq!(
                    chunk_a.get_block(x, y, z),
                    chunk_b.get_block(x, y, z),
                    "two games with one seed must agree at ({x},{y},{z})"
                );
            }
        }
    }
    a.save_all_owned().expect("save a");
    b.save_all_owned().expect("save b");
}

#[test]
fn two_seeds_produce_different_terrain() {
    // Without this, "deterministic" could be satisfied by a constant.
    let dir_a = TempDir::new("p07-seed-a");
    let dir_b = TempDir::new("p07-seed-b");
    let mut a = owning_game(&dir_a, 1);
    let mut b = owning_game(&dir_b, 99);

    let pos = ChunkPos::new(0, 0);
    assert!(a.load_chunk(pos));
    assert!(b.load_chunk(pos));
    let chunk_a = a.world().chunk(pos).expect("a");
    let chunk_b = b.world().chunk(pos).expect("b");

    let differs = (0..16).any(|x| {
        (0..16).any(|z| {
            (MIN_Y..MAX_Y).any(|y| chunk_a.get_block(x, y, z) != chunk_b.get_block(x, y, z))
        })
    });
    assert!(differs, "two seeds must not produce identical terrain");
    a.save_all_owned().expect("save a");
    b.save_all_owned().expect("save b");
}

#[test]
fn a_generated_chunk_is_saved_and_a_stored_chunk_is_not_regenerated() {
    // **The phase's stated priority.** A generated chunk must be persisted so the next start
    // reads it, and a stored chunk must be read rather than generated. The proof is a
    // marker: if generation ran again, the marker would be gone.
    let dir = TempDir::new("p07-existing");
    let pos = ChunkPos::new(1, 2);

    let (x, y, z, gold) = {
        let mut game = owning_game(&dir, 7);
        assert!(game.load_chunk(pos));
        let marker = place_marker(&mut game, pos);
        game.save_all_owned().expect("save");
        marker
    };

    let mut game = owning_game(&dir, 7);
    assert!(game.load_chunk(pos));
    let chunk = game.world().chunk(pos).expect("loaded");
    assert_eq!(
        chunk.get_block(x, y, z),
        gold,
        "the stored chunk must be read rather than regenerated: the marker at ({x},{y},{z}) \
         is gone, which means generation overwrote a saved world"
    );
    game.save_all_owned().expect("save");
}

#[test]
fn a_borrowing_game_never_overwrites_stored_terrain() {
    // **The regression test for the data-loss bug described in the module docs.**
    //
    // A borrowing game cannot tell "no chunk stored" from "cannot look", so it must not
    // generate — and must not persist what it has. If it did, the marker below would be
    // replaced by generated terrain.
    let dir = TempDir::new("p07-borrow-safe");
    let pos = ChunkPos::new(0, 1);

    let (x, y, z, gold) = {
        let mut game = owning_game(&dir, 21);
        assert!(game.load_chunk(pos));
        let marker = place_marker(&mut game, pos);
        game.save_all_owned().expect("save");
        marker
    };

    // Open the world the way a *test or tool* would — borrowing.
    {
        let mut service = WorldService::open(&config(&dir)).expect("reopen");
        let mut game = borrowing_game(&service, 21);
        game.load_chunk(pos);
        // Whatever it did in memory, it must not write it back over the saved chunk.
        game.save_all(&mut service).expect("save");
    }

    // The marker must still be there.
    let mut game = owning_game(&dir, 21);
    assert!(game.load_chunk(pos));
    let chunk = game.world().chunk(pos).expect("loaded");
    assert_eq!(
        chunk.get_block(x, y, z),
        gold,
        "a borrowing game overwrote a saved world: the marker at ({x},{y},{z}) is gone"
    );
    game.save_all_owned().expect("save");
}

#[test]
fn an_existing_world_survives_repeated_reopen_cycles() {
    // The same property over several cycles, because a save that silently stopped marking
    // chunks dirty would pass a single-cycle test: the second game would regenerate
    // *identical* terrain from the same seed. The marker makes each cycle distinguishable.
    let dir = TempDir::new("p07-cycles");
    let pos = ChunkPos::new(-2, 3);

    let (x, y, z, gold) = {
        let mut game = owning_game(&dir, 11);
        assert!(game.load_chunk(pos));
        let marker = place_marker(&mut game, pos);
        game.save_all_owned().expect("save");
        marker
    };

    for cycle in 1..4 {
        let mut game = owning_game(&dir, 11);
        assert!(game.load_chunk(pos), "cycle {cycle}");
        let chunk = game.world().chunk(pos).expect("chunk");
        assert_eq!(
            chunk.get_block(x, y, z),
            gold,
            "cycle {cycle}: the marker placed in cycle 0 is gone, so the world was \
             regenerated over its own saved content"
        );
        game.save_all_owned().expect("save");
    }
}

#[test]
fn generation_does_not_panic_on_extreme_chunk_coordinates() {
    let dir = TempDir::new("p07-extreme");
    let mut game = owning_game(&dir, 0);

    // Far from the origin, and at the extremes of the coordinate space. The generator must
    // return a chunk or refuse; it must not panic or allocate absurdly.
    for pos in [
        ChunkPos::new(1_000_000, 1_000_000),
        ChunkPos::new(-1_000_000, -1_000_000),
        ChunkPos::new(i32::MAX / 16, i32::MIN / 16),
    ] {
        game.load_chunk(pos);
        assert!(
            game.world().chunk(pos).is_some(),
            "{pos:?} must produce a chunk or a placeholder, not nothing"
        );
    }
    game.save_all_owned().expect("save");
}
