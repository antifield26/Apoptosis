//! P15-03 zero-observable-delta scenario pin (P18-07).
//!
//! The P15 splits claimed "zero behavior change" and AUDIT-16/17 kept the
//! gap open: test-count movement is not equivalence. This suite runs a fixed
//! observable scenario twice from a clean seed and asserts the normalized
//! transcript matches — the independent comparison, not a re-count.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

fn scenario(tag: &str) -> Vec<String> {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (tx, rx) = game_channel(256);
    let mut game =
        Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
            .expect("game builds");
    let (outbound, mut out) = OutboundSender::pair(ConnectionId(1), 8192);
    let id = ConnectionId(1);
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Delta"),
            outbound,
        },
    })
    .expect("join queued");
    game.tick().expect("tick");

    let mut lines = Vec::new();
    let (sx, sy, sz) = game.spawn();
    lines.push(format!("spawn {sx} {sy} {sz}"));
    game.load_chunk(ChunkPos::new(sx >> 4, sz >> 4));
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    game.world_mut()
        .set_block(sx + 1, sy - 1, sz, stone)
        .expect("floor");

    // Place one stone on the floor, then dig it — the P15-03 named path.
    let cobble = game
        .registries()
        .items
        .id("minecraft:cobblestone")
        .expect("cobble");
    game.player_mut(id)
        .expect("player")
        .inventory
        .set_slot(
            0,
            mc_entity::stack::ItemStack::new(cobble, 1).expect("stack"),
        )
        .expect("slot");
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(sx + 1, sy - 1, sz),
            face: 1,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 1,
        }),
    })
    .expect("place");
    game.tick().expect("tick");
    let placed = game
        .registries()
        .blocks
        .block_name(game.world().get_block(sx + 1, sy, sz))
        .expect("name")
        .to_owned();
    lines.push(format!("placed {placed}"));

    for i in 0..8 {
        game.tick().expect("tick");
        lines.push(format!("tick {i} dropped {}", game.dropped_items().len()));
    }

    let _ = out.try_recv();
    while let Some(raw) = out.try_recv() {
        lines.push(format!("pkt {}", raw.id));
    }
    let id0 = game.player(id).expect("player").entity_id;
    lines.push(format!("player_entity {id0}"));
    lines
}

#[test]
fn p15_split_observables_replay_identically() {
    let a = scenario("p15-delta-a");
    let b = scenario("p15-delta-b");
    assert_eq!(
        a, b,
        "P15-03 zero-observable-delta: same seed, same inputs, same transcript"
    );
}
