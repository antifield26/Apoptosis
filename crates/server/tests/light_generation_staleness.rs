//! P18-05-C1: light computed against a missing neighbour must not survive
//! the neighbour's arrival.
//!
//! The walk screenshots show sharp lit/dark cave patches with no opening:
//! a chunk whose light computed while its neighbour was still ungenerated
//! reads that neighbour as air through the one-block margin, sky floods in,
//! and the result sits in the cache — which nothing invalidated on
//! generation, so holders kept the bright light forever. Generation now
//! drops the 3×3 cache and queues updates, and this test pins both halves:
//! without the drop the cache survives (first assertion), without the queue
//! nobody is told (second assertion).

use mc_network::bridge::game_channel;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

fn open_world(tag: &str) -> (Game, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = game_channel(256);
    let game =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    (game, dir)
}

/// Sky light at one cell of a cached chunk.
fn sky_at(game: &Game, pos: ChunkPos, x: i32, y: i32, z: i32) -> u8 {
    let chunk = game.world().chunk(pos).expect("loaded");
    let section = usize::try_from((y - chunk.min_y()) / 16).expect("in range");
    game.world().cached_light(pos).expect("light cached").sky[section].get(
        x.rem_euclid(16),
        y.rem_euclid(16),
        z.rem_euclid(16),
    )
}

#[test]
fn generation_invalidates_neighbour_light_and_queues_updates() {
    let (mut game, _dir) = open_world("light_generation_staleness");
    let center = ChunkPos::new(0, 0);
    assert!(game.load_chunk(center));
    assert_eq!(
        game.pending_light_len(),
        9,
        "the first generation queues its own 3×3"
    );

    // A deep-stone cell on the east border: far below any surface, so the
    // only sky that can reach it leaks sideways from the missing neighbour.
    // Plain terrain has no deep air, so hollow a 3×3×3 pocket against the
    // border first (the walk's cave in miniature).
    let (x, z) = (15, 8);
    let wx = center.x * 16 + x;
    let wz = center.z * 16 + z;
    let surface = (game.world().chunk(center).expect("loaded").min_y()..320)
        .rev()
        .find(|y| {
            game.world()
                .get_block_loaded(wx, *y, wz)
                .is_some_and(|state| !game.registries().blocks.is_empty(state))
        })
        .expect("a surface exists over the column");
    let y0 = surface - 30;
    let air_id = game.registries().blocks.air_id();
    for dy in 0..3 {
        for dz in -1..=1 {
            for dx in -2..=0 {
                game.world_mut()
                    .set_block(wx + dx, y0 + dy, wz + dz, air_id)
                    .expect("hollow applies");
            }
        }
    }

    let light_table = game.registries().light.clone();
    game.world_mut()
        .compute_light(center, &light_table)
        .expect("frontier light computes");
    // The pocket mouth reads the missing neighbour as air: sky floods in.
    let frontier = sky_at(&game, center, x, y0 + 1, z);
    assert!(
        frontier > 0,
        "the frontier must read the missing neighbour as air (got {frontier}) — \
         otherwise this test proves nothing"
    );

    // The neighbour arrives: the center's cached light must drop, and the
    // queue must grow so holders converge through light updates.
    let queued_before = game.pending_light_len();
    assert!(game.load_chunk(ChunkPos::new(1, 0)));
    assert!(
        game.world().cached_light(center).is_none(),
        "the center's light must be invalidated when its neighbour generates"
    );
    assert!(
        game.pending_light_len() > queued_before,
        "the arrival must queue updates for holders"
    );

    // Recomputing against the real neighbour converges to dark.
    game.world_mut()
        .compute_light(center, &light_table)
        .expect("re-light with the neighbour present");
    assert_eq!(
        sky_at(&game, center, x, y0 + 1, z),
        0,
        "an enclosed pocket is dark once the neighbour exists"
    );
}
