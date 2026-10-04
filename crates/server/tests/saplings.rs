//! Saplings, leaf decay and bone meal, slice 2c: click paths and the sweep
//! wiring (P20-02 slice 2c).
//!
//! The mechanisms themselves are pinned by direct calls in
//! `game::growth::tests`; this file proves the two paths that reach them —
//! `UseItemOn` with bone meal, and the `RandomTicks` sweep over sapling and
//! leaf cells. One full-phase wiring test, fast harnesses (view distance 2,
//! TEST-TIME-PLAN §3).

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
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
        Self::with_operators(tag, ops_empty())
    }

    fn with_operators(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::build_with_operators(None, Some(storage), 2, rx, DEFAULT_RANDOM_SEED, operators)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        out
    }

    fn command(&mut self, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        use mc_protocol::ids::clientbound;
        use mc_protocol::packets::Packet;
        let mut report = mc_server::game::TickReport::default();
        self.game
            .dispatch_command(self.id, text, &mut report)
            .expect("a command is answered");
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

    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    /// Right-click the block at `(x, y, z)` on its top face with the held item.
    fn click(&mut self, x: i32, y: i32, z: i32) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
                    hand: 0,
                    position: block_position(x, y, z),
                    face: 1,
                    cursor_x: 0.5,
                    cursor_y: 0.5,
                    cursor_z: 0.5,
                    inside_block: false,
                    world_border_hit: false,
                    sequence: 0,
                }),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    /// Put one `item` in the held slot (hotbar slot 0).
    fn give(&mut self, item: &str) {
        let item_id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(item_id, 1).expect("stack"),
            )
            .expect("slot 0 takes it");
    }

    fn held_count(&mut self) -> i32 {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .selected_item()
            .count()
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

    fn int_prop(&self, x: i32, y: i32, z: i32, key: &str) -> Option<i32> {
        let id = self.game.world().get_block_loaded(x, y, z)?;
        self.game
            .registries()
            .blocks
            .properties_of(id)
            .ok()?
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.parse().ok())
    }
}

/// Bone meal grows a wheat crop one stage and is eaten.
#[test]
fn bone_meal_grows_a_crop_and_is_eaten() {
    let mut harness = Harness::new("bonemeal-crop");
    harness.join("Gardener");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let soil = blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "7".to_owned())],
        )
        .expect("moist farmland");
    harness
        .game
        .world_mut()
        .set_block(8, 120, 8, soil)
        .expect("soil placed");
    let seedling = blocks
        .state_id("minecraft:wheat", &[("age".to_owned(), "0".to_owned())])
        .expect("seedling");
    harness
        .game
        .world_mut()
        .set_block(8, 121, 8, seedling)
        .expect("crop placed");
    harness.give("minecraft:bone_meal");
    harness.stand(8, 121, 6);
    harness.click(8, 121, 8);
    assert_eq!(
        harness.int_prop(8, 121, 8, "age"),
        Some(1),
        "one meal grows one stage (neutralise the meal arm and the age stays 0)"
    );
    assert_eq!(harness.held_count(), 0, "the meal is eaten");
}

/// Bone meal refuses finished crops and stone without eating.
#[test]
fn bone_meal_refuses_finished_crops_and_stone() {
    let mut harness = Harness::new("bonemeal-miss");
    harness.join("Gardener");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let ripe = blocks
        .state_id("minecraft:wheat", &[("age".to_owned(), "7".to_owned())])
        .expect("ripe wheat");
    harness
        .game
        .world_mut()
        .set_block(8, 121, 8, ripe)
        .expect("crop placed");
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    harness
        .game
        .world_mut()
        .set_block(9, 121, 8, stone)
        .expect("stone placed");
    harness.give("minecraft:bone_meal");
    harness.stand(8, 121, 6);
    harness.click(8, 121, 8);
    assert_eq!(
        harness.int_prop(8, 121, 8, "age"),
        Some(7),
        "a ripe crop stays ripe"
    );
    assert_eq!(harness.held_count(), 1, "a refused meal is kept");
    harness.click(9, 121, 8);
    assert_eq!(harness.held_count(), 1, "stone keeps the meal too");
    assert_eq!(
        harness.block_name(9, 121, 8),
        "minecraft:stone",
        "and places nothing: the meal is not a block"
    );
}

/// Bone meal advances a sapling, then grows its tree.
#[test]
fn bone_meal_grows_a_sapling_then_a_tree() {
    let mut harness = Harness::new("bonemeal-tree");
    harness.join("Gardener");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    harness
        .game
        .world_mut()
        .set_block(8, 119, 8, dirt)
        .expect("pad placed");
    let seedling = blocks
        .state_id(
            "minecraft:oak_sapling",
            &[("stage".to_owned(), "0".to_owned())],
        )
        .expect("stage-0 sapling");
    harness
        .game
        .world_mut()
        .set_block(8, 120, 8, seedling)
        .expect("sapling placed");
    harness.give("minecraft:bone_meal");
    harness.stand(8, 120, 6);
    harness.click(8, 120, 8);
    assert_eq!(
        harness.int_prop(8, 120, 8, "stage"),
        Some(1),
        "the first meal advances the stage"
    );
    assert_eq!(harness.held_count(), 0, "the meal is eaten");
    harness.give("minecraft:bone_meal");
    harness.click(8, 120, 8);
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:oak_log",
        "the second meal grows the tree (neutralise the meal arm and the sapling stands)"
    );
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

fn ops_empty() -> OperatorList {
    OperatorList::parse("[]", Path::new("ops.json")).expect("empty parses")
}

/// Stage-1 saplings grow through the `RandomTicks` sweep.
#[test]
fn saplings_grow_through_the_phase() {
    let mut harness = Harness::with_operators("sapling-phase", ops_for("Chief", 4));
    let mut out = harness.join("Chief");
    for chunk in [
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(0, 1),
        ChunkPos::new(1, 1),
    ] {
        assert!(harness.game.load_chunk(chunk), "orchard chunk loads");
    }
    let blocks = harness.game.registries().blocks.clone();
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    let grown = blocks
        .state_id(
            "minecraft:oak_sapling",
            &[("stage".to_owned(), "1".to_owned())],
        )
        .expect("stage-1 sapling");
    // Chunk-local 4..12 in every axis: every canopy fits its chunk, so a
    // missing trunk is the sweep's verdict, never the straddle rule.
    let mut plots = Vec::new();
    for &x in &[4, 6, 10, 12, 20, 22, 26, 28] {
        for &z in &[4, 8, 12, 20, 24, 28] {
            for &y in &[120, 130] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y - 1, z, dirt)
                    .expect("pad placed");
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, grown)
                    .expect("sapling placed");
                plots.push((x, y, z));
            }
        }
    }
    assert_eq!(plots.len(), 96, "the orchard counts what was planted");
    harness.stand(8, 121, 8);
    // No natural spawns: mob AI (A* pathfinding over a growing crowd) is the
    // dominant per-tick cost here, and spawns are not what this pin measures.
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    for _ in 0..600 {
        harness.game.tick().expect("tick");
    }
    let trunks = plots
        .iter()
        .filter(|&&(x, y, z)| harness.block_name(x, y, z) == "minecraft:oak_log")
        .count();
    assert!(
        trunks >= 2,
        "the sweep grows trees through the random-tick arm (saw {trunks}; neutralise the dispatch and none grow)"
    );
    // And the sweep reaches the leaf arm too: some grown leaf near a trunk
    // carries a repaired (sub-7) distance rather than the fixture default.
    let mut repaired = 0;
    for &(x, y, z) in &plots {
        if harness.block_name(x, y, z) != "minecraft:oak_log" {
            continue;
        }
        for dy in 1..=10 {
            for dx in -3..=3 {
                for dz in -3..=3 {
                    if harness.block_name(x + dx, y + dy, z + dz) == "minecraft:oak_leaves"
                        && harness
                            .int_prop(x + dx, y + dy, z + dz, "distance")
                            .is_some_and(|d| d < 7)
                    {
                        repaired += 1;
                    }
                }
            }
        }
    }
    assert!(
        repaired > 0,
        "grown canopies repair their leaf distances through the sweep"
    );
}
