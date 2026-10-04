//! Negative-coordinate fills land correctly and count changed cells.
//!
//! The P20-07 soak setup once misread `Filled 9 out of 15` replies as a
//! minus-sign parsing defect; the probe showed the parser is exact and the
//! short counts are shared-corner overlaps. This file keeps that lesson as
//! a pin: negatives resolve absolutely, and a second fill over half-built
//! ground counts only what it changes.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let uuid = mc_network::auth::offline_profile("Chief").id;
        let text = format!(r#"[{{"uuid": "{uuid}", "name": "Chief", "level": 4}}]"#);
        let ops = OperatorList::parse(&text, Path::new("ops.json")).expect("ops");
        let game =
            Game::build_with_operators(None, Some(storage), 2, rx, DEFAULT_RANDOM_SEED, ops)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile("Chief"),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        out
    }

    fn command(&mut self, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let mut report = mc_server::game::TickReport::default();
        self.game
            .dispatch_command(self.id, text, &mut report)
            .expect("answered");
        let mut lines = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT
                && let Ok(chat) = mc_protocol::packets::play::SystemChat::decode(&raw.payload)
            {
                lines.push(chat.content.as_plain().to_owned());
            }
        }
        lines
    }

    fn block_name(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("registered")
            .to_owned()
    }
}

/// Negative coordinates resolve absolutely, and overlapping fills count
/// only changed cells.
#[test]
fn negative_fills_land_where_named_and_count_changes() {
    let mut harness = Harness::new("fill-negative");
    let mut out = harness.join();
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    // Two walls sharing their corner columns, like the soak tank.
    let lines = harness.command(&mut out, "fill 8 100 -14 8 102 -10 minecraft:stone");
    assert!(
        lines.iter().any(|line| line.contains("15 block(s) out of 15")),
        "a fresh wall fills whole, saw {lines:?}"
    );
    let lines = harness.command(&mut out, "fill 8 100 -14 12 102 -14 minecraft:stone");
    assert!(
        lines.iter().any(|line| line.contains("12 block(s) out of 15")),
        "the one shared corner is not double-counted, saw {lines:?}"
    );
    // ...at the named (negative) coordinates, not mirrored positive.
    for (x, z) in [(8, -14), (12, -14), (8, -10)] {
        assert_eq!(
            harness.block_name(x, 101, z),
            "minecraft:stone",
            "wall stone lands at ({x}, 101, {z})"
        );
    }
    assert_eq!(
        harness.block_name(8, 101, 14),
        "minecraft:air",
        "the mirrored-positive column stays air (a dropped minus would build here)"
    );
}
