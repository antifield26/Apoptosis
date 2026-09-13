//! A generated world must contain the features its biomes call for.
//!
//! ## Why this exists
//!
//! `TerrainGenerator::generate_chunk` produces **terrain only**; trees are `TerrainGenerator::decorate`, a
//! separate pass. The server called `decorate_with_structures` — structures — and never `decorate`, so **every
//! world it generated had no trees in it at all**, and nothing anywhere said so.
//!
//! The block census over a 5x5 of chunks was unambiguous: grass over dirt over stone, podzol and coarse dirt
//! for taiga, sand and water for ocean, and **not one `oak_log` or `oak_leaves`**. Every block was the right
//! block for its biome; the features that make a biome recognisable were simply absent.
//!
//! That is a failure no unit test of terrain, biomes, blocks or light can see, because none of them is wrong.
//! It takes generating a world the way the server does and asking whether anything grew in it.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, OutboundSender, game_channel,
};
use mc_server::config::StorageConfig;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

#[tokio::test]
async fn a_generated_world_contains_trees() {
    let dir = TempDir::new("p10-features");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    let mut game =
        Game::with_seed_and_storage(storage, 6, rx, mc_server::game::DEFAULT_RANDOM_SEED)
            .expect("game builds");

    // Joining is what loads chunks, and loading one is what generates and decorates it.
    let id = ConnectionId(1);
    let (outbound, _receiver) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Forester"),
                outbound,
            },
        })
        .expect("join queued");
    for _ in 0..200 {
        game.tick().expect("tick");
    }

    let stats = game.tree_stats();
    assert!(
        stats.columns_considered > 0,
        "no column was ever offered to the tree pass, so the feature pass is still not being called: {stats:?}"
    );
    assert!(
        stats.trees_placed > 0,
        "the tree pass ran over {} columns and placed nothing: {stats:?}",
        stats.columns_considered
    );

    // And the blocks are really in the world, not merely counted: an oak log or a leaf somewhere.
    let blocks = game.registries().blocks.clone();
    let log = blocks.default_state("minecraft:oak_log").expect("oak_log");
    let leaves = blocks
        .default_state("minecraft:oak_leaves")
        .expect("oak_leaves");
    let min_y = i32::from(game.world().min_section_y()) * mc_world::SECTION_HEIGHT;
    let top_y = min_y
        + i32::try_from(game.world().section_count()).expect("section count fits")
            * mc_world::SECTION_HEIGHT;

    // A range around the spawn, which is what the join loaded. World has no position iterator, and adding
    // one for a test would be a public API for a diagnostic's convenience.
    let (sx, _, sz) = game.spawn();
    let (cx, cz) = (sx >> 4, sz >> 4);
    let mut found = 0_u32;
    for dx in -6..=6 {
        for dz in -6..=6 {
            let Some(chunk) = game
                .world()
                .chunk(mc_persistence::chunk::ChunkPos::new(cx + dx, cz + dz))
            else {
                continue;
            };
            for x in 0..16 {
                for z in 0..16 {
                    for y in (min_y..top_y).rev() {
                        let block = chunk.get_block(x, y, z);
                        if block == log || block == leaves {
                            found += 1;
                            break;
                        }
                    }
                }
            }
        }
    }
    assert!(
        found > 0,
        "the counters say {} trees were placed but no log or leaf is in any loaded chunk",
        stats.trees_placed
    );
}
