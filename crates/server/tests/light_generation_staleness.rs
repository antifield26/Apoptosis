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

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::{ChunkPos, Vec3};

fn open_world(tag: &str) -> (Game, tokio::sync::mpsc::Sender<ClientEvent>, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (tx, rx) = game_channel(256);
    let game =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    (game, tx, dir)
}

fn join(
    game: &mut Game,
    events: &tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    name: &str,
) -> InboundReceiver {
    let (outbound, out) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile(name),
                outbound,
            },
        })
        .expect("join event queued");
    game.tick().expect("tick applies the join");
    out
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
    let (mut game, _events, _dir) = open_world("light_generation_staleness");
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

/// F-M4 end to end: unloading a chunk recomputes its surviving neighbours
/// and the holders actually receive the `LightUpdate`.
///
/// Setup settles first (60 ticks drain every setup queue, then the outbound
/// is drained), so a post-teleport update for the survivor can only come
/// from the unload path: no arrivals happen near it anymore and nothing is
/// edited. The exact queued set is pinned at unit level
/// (`unload_queues_loaded_neighbours_only` in `light_cache.rs`); this pins
/// the queue → broadcast → send chain.
#[test]
fn unloading_a_chunk_sends_light_updates_to_holders() {
    let (mut game, events, _dir) = open_world("light_unload_e2e");
    let id = ConnectionId(1);
    let mut out = join(&mut game, &events, id, "Watcher");
    // Light the origin 3x3 the player stands in.
    for dx in -1..=1 {
        for dz in -1..=1 {
            assert!(game.load_chunk(ChunkPos::new(dx, dz)));
        }
    }
    let light_table = game.registries().light.clone();
    for dx in -1..=1 {
        for dz in -1..=1 {
            game.world_mut()
                .compute_light(ChunkPos::new(dx, dz), &light_table)
                .expect("light computes");
        }
    }
    // Settle: drain every setup queue and every setup packet.
    for _ in 0..60 {
        game.tick().expect("tick settles");
    }
    // Persist the settle: random-tick growth writes blocks (leaf repairs
    // since P20-02 slice 2c, crops before it), and a written chunk is dirty
    // — which the unload path keeps resident until a save clears it.
    // Production autosaves every 6000 ticks, so dirtiness only ever pins a
    // chunk briefly; the test saves here for the same reason, so the
    // teleport below measures unloading rather than dirtiness.
    game.save_all_owned().expect("settle saves");
    while out.try_recv().is_some() {}
    assert_eq!(
        game.pending_light_len(),
        0,
        "setup queues must drain before the teleport"
    );

    // Walk away: chunk (0, 0) unloads (view 4 + margin 2 = 6 < 7) while
    // (1, 0) stays held.
    game.player_mut(id).expect("player").position = Vec3::new(120.0, 70.0, 8.0);
    // The sweep centres on the entity projection, which the Players phase
    // publishes one tick late — so the teleport tick still sweeps the origin
    // ring, and a sampled leaf there repairs (P20-02 slice 2c) and re-dirties
    // the chunk. Production would autosave that away within minutes; the
    // test saves after the lag tick for the same reason, then measures.
    for _ in 0..2 {
        game.tick().expect("tick publishes the teleport");
    }
    game.save_all_owned().expect("post-teleport save");
    for _ in 0..38 {
        game.tick().expect("tick unloads and redrains");
    }
    assert!(
        game.world().chunk(ChunkPos::new(0, 0)).is_none(),
        "the origin chunk must unload once far away"
    );
    // Scan the outbound for the survivor's recompute.
    let mut seen = false;
    while let Some(raw) = out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::LIGHT_UPDATE {
            let mut reader = mc_protocol::wire::PacketReader::new(&raw.payload);
            let x = reader.read_varint().expect("chunk x");
            let z = reader.read_varint().expect("chunk z");
            if x == 1 && z == 0 {
                seen = true;
            }
        }
    }
    assert!(
        seen,
        "a holder of surviving (1, 0) must receive its LightUpdate after (0, 0) unloads"
    );
}
