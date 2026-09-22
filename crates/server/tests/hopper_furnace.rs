//! Hopper-to-furnace routing end to end (P17-02 Step A).
//!
//! Through the real tick loop: a hopper below a furnace pulls the output
//! slot only (input/fuel stay put); a hopper above feeds smeltables into
//! the input and holds fuel back; a side hopper feeds fuel into the fuel
//! slot and holds smeltables back; the full chain smelts and collects.
//!
//! Falsification shape: all-Storage source roles let the pull steal the
//! input (test 1 fails); skipping the item check lets coal drop into the
//! input from above (test 2 fails).

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

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
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
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

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn give(&mut self, item: &str) {
        let id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(id, 1).expect("stack"))
            .expect("slot 0 takes it");
    }

    fn click(&mut self, x: i32, y: i32, z: i32, face: i32) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }

    /// Give one item and click: survival consumes, so every placement needs
    /// its own give (a second click on one give is an empty-hand no-op).
    fn place(&mut self, item: &str, x: i32, y: i32, z: i32, face: i32) {
        self.give(item);
        self.click(x, y, z, face);
    }

    fn item(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }

    fn name_at(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("named")
            .to_owned()
    }

    /// White-box fill of a block entity slot (the chest tests fill the same way).
    fn fill(&mut self, x: i32, y: i32, z: i32, slot: usize, item: &str, count: i32) {
        let id = self.item(item);
        let pos = mc_container::BlockPos::new(x, y, z);
        let entity = self
            .game
            .block_entities_mut()
            .get_mut(pos)
            .unwrap_or_else(|| panic!("block entity exists at {x},{y},{z}"));
        let items = entity.data.items_mut().expect("inventory");
        items[slot] = mc_entity::stack::ItemStack::new(id, count).expect("stack");
    }

    fn entity_total(&self, x: i32, y: i32, z: i32) -> i32 {
        self.game
            .block_entities()
            .get(mc_container::BlockPos::new(x, y, z))
            .and_then(|e| e.data.items())
            .map_or(-1, |items| {
                items.iter().map(mc_entity::stack::ItemStack::count).sum()
            })
    }

    fn furnace_slot(&self, x: i32, y: i32, z: i32, slot: usize) -> (Option<i32>, i32) {
        let items = self
            .game
            .block_entities()
            .get(mc_container::BlockPos::new(x, y, z))
            .and_then(|e| e.data.items())
            .expect("furnace inventory");
        let stack = items[slot];
        (stack.item_id(), stack.count())
    }

    /// White-box facing surgery (hopper placement orientation is unmodelled —
    /// first state wins — so sideways hoppers are staged, like the floor).
    fn set_facing(&mut self, x: i32, y: i32, z: i32, facing: &str) {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        let name = self
            .game
            .registries()
            .blocks
            .block_name(id)
            .expect("named")
            .to_owned();
        let default = self
            .game
            .registries()
            .blocks
            .default_state(&name)
            .expect("default");
        let mut props = self
            .game
            .registries()
            .blocks
            .properties_of(default)
            .expect("props");
        let mut found = false;
        for (key, value) in &mut props {
            if key == "facing" {
                facing.clone_into(value);
                found = true;
            }
        }
        assert!(found, "hopper has a facing property");
        let state = self
            .game
            .registries()
            .blocks
            .state_id(&name, &props)
            .expect("facing state resolves");
        self.game
            .world_mut()
            .set_block(x, y, z, state)
            .expect("facing set");
    }
}

/// A stone floor strip with the work volume cleared to air.
fn floor(harness: &mut Harness, sx: i32, sy: i32, sz: i32) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    for x in (sx - 4)..=(sx + 6) {
        for z in (sz - 2)..=(sz + 2) {
            harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
            harness
                .game
                .world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
            for y in [sy, sy + 1, sy + 2] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, air)
                    .expect("cleared");
            }
        }
    }
}

#[test]
fn hopper_below_pulls_only_the_output() {
    let mut harness = Harness::new("p17-pull-output");
    let _out = harness.join("Stoker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Work one block east of spawn: the spawn cell itself would refuse
    // placement (inside the player).
    let (fx, fy, fz) = (sx + 1, sy, sz);
    harness.place("minecraft:furnace", fx, fy - 1, fz, 1);
    assert_eq!(harness.name_at(fx, fy, fz), "minecraft:furnace");
    // Hopper in a dug hole directly beneath: click the north face of the
    // south neighbour.
    let air = harness.game.registries().blocks.air_id();
    harness
        .game
        .world_mut()
        .set_block(fx, fy - 1, fz, air)
        .expect("dig the hole");
    harness.place("minecraft:hopper", fx, fy - 1, fz + 1, 2);
    assert_eq!(harness.name_at(fx, fy - 1, fz), "minecraft:hopper");
    harness.fill(fx, fy, fz, 0, "minecraft:cobblestone", 5);
    harness.fill(fx, fy, fz, 1, "minecraft:coal", 3);
    harness.fill(fx, fy, fz, 2, "minecraft:stone", 2);
    harness.run(20);
    // Two pulls happened (8-tick cadence); the input was never touched.
    assert_eq!(harness.entity_total(fx, fy - 1, fz), 2);
    assert_eq!(harness.furnace_slot(fx, fy, fz, 0).1, 5);
    assert_eq!(harness.furnace_slot(fx, fy, fz, 2).1, 0);
}

#[test]
fn hopper_above_feeds_smeltables_and_holds_fuel_back() {
    let mut harness = Harness::new("p17-top-feed");
    let _out = harness.join("Stoker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    let (fx, fy, fz) = (sx + 1, sy, sz);
    harness.place("minecraft:furnace", fx, fy - 1, fz, 1);
    // Scaffold pillar beside the target cell, then click its side face.
    harness.place("minecraft:dirt", fx + 1, fy - 1, fz, 1);
    harness.place("minecraft:dirt", fx + 1, fy, fz, 1);
    harness.place("minecraft:hopper", fx + 1, fy + 1, fz, 4);
    assert_eq!(harness.name_at(fx, fy + 1, fz), "minecraft:hopper");
    harness.fill(fx, fy + 1, fz, 0, "minecraft:cobblestone", 5);
    harness.run(10);
    let (item, count) = harness.furnace_slot(fx, fy, fz, 0);
    assert_eq!(item, Some(harness.item("minecraft:cobblestone")));
    assert!(count >= 1, "smeltables land in the input, saw {count}");
    assert_eq!(harness.entity_total(fx, fy + 1, fz), 5 - count);

    // Same geometry with coal only: top entry refuses fuel, everything stays.
    if let Some(items) = harness
        .game
        .block_entities_mut()
        .get_mut(mc_container::BlockPos::new(fx, fy, fz))
        .and_then(|e| e.data.items_mut())
    {
        items[0] = mc_entity::stack::ItemStack::EMPTY;
    }
    for slot in 0..5 {
        harness.fill(fx, fy + 1, fz, slot, "minecraft:coal", 5);
    }
    harness.run(10);
    assert_eq!(
        harness.entity_total(fx, fy + 1, fz),
        25,
        "coal from above stays in the hopper"
    );
    assert_eq!(
        harness.furnace_slot(fx, fy, fz, 0).1,
        0,
        "and none of it reaches the input"
    );
}

#[test]
fn side_hopper_feeds_fuel_and_holds_smeltables_back() {
    let mut harness = Harness::new("p17-side-feed");
    let _out = harness.join("Stoker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Two blocks east of spawn: the west hopper's cell would otherwise sit
    // inside the player and placement refuses it.
    let (fx, fy, fz) = (sx + 3, sy, sz);
    harness.place("minecraft:furnace", fx, fy - 1, fz, 1);
    // East hopper (coal) facing west, west hopper (cobble) facing east.
    harness.place("minecraft:dirt", fx + 2, fy - 1, fz, 1);
    harness.place("minecraft:dirt", fx - 2, fy - 1, fz, 1);
    harness.place("minecraft:hopper", fx + 2, fy, fz, 4);
    harness.place("minecraft:hopper", fx - 2, fy, fz, 5);
    assert_eq!(harness.name_at(fx + 1, fy, fz), "minecraft:hopper");
    assert_eq!(harness.name_at(fx - 1, fy, fz), "minecraft:hopper");
    harness.set_facing(fx + 1, fy, fz, "west");
    harness.set_facing(fx - 1, fy, fz, "east");
    harness.fill(fx + 1, fy, fz, 0, "minecraft:coal", 5);
    harness.fill(fx - 1, fy, fz, 0, "minecraft:cobblestone", 5);
    harness.run(10);
    let (fuel_item, fuel_count) = harness.furnace_slot(fx, fy, fz, 1);
    assert_eq!(fuel_item, Some(harness.item("minecraft:coal")));
    assert!(
        fuel_count >= 1,
        "fuel lands in the fuel slot, saw {fuel_count}"
    );
    assert_eq!(
        harness.furnace_slot(fx, fy, fz, 0).1,
        0,
        "smeltables from the side stay out of the input"
    );
    assert_eq!(
        harness.entity_total(fx - 1, fy, fz),
        5,
        "the cobble hopper keeps everything"
    );
}

#[test]
fn a_fed_furnace_cooks_and_the_hopper_below_collects() {
    let mut harness = Harness::new("p17-full-chain");
    let _out = harness.join("Stoker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Same eastward shift as the side-feed test (spawn column is occupied).
    let (fx, fy, fz) = (sx + 3, sy, sz);
    harness.place("minecraft:furnace", fx, fy - 1, fz, 1);
    // Hopper above via a scaffold pillar; side hopper west of the furnace
    // facing east; bottom hopper in a dug hole.
    harness.place("minecraft:dirt", fx + 1, fy - 1, fz, 1);
    harness.place("minecraft:dirt", fx + 1, fy, fz, 1);
    harness.place("minecraft:dirt", fx - 2, fy - 1, fz, 1);
    harness.place("minecraft:hopper", fx + 1, fy + 1, fz, 4);
    harness.place("minecraft:hopper", fx - 2, fy, fz, 5);
    harness.set_facing(fx - 1, fy, fz, "east");
    let air = harness.game.registries().blocks.air_id();
    harness
        .game
        .world_mut()
        .set_block(fx, fy - 1, fz, air)
        .expect("dig the hole");
    harness.place("minecraft:hopper", fx, fy - 1, fz + 1, 2);
    assert_eq!(harness.name_at(fx, fy + 1, fz), "minecraft:hopper");
    assert_eq!(harness.name_at(fx - 1, fy, fz), "minecraft:hopper");
    assert_eq!(harness.name_at(fx, fy - 1, fz), "minecraft:hopper");
    harness.fill(fx, fy + 1, fz, 0, "minecraft:cobblestone", 5);
    harness.fill(fx - 1, fy, fz, 0, "minecraft:coal", 5);
    // One cobble cooks in 200 ticks; the pull below collects on an 8-tick
    // cadence. 260 ticks covers feed (~40) + cook (200) + collect.
    harness.run(260);
    assert_eq!(
        harness.entity_total(fx, fy - 1, fz),
        1,
        "exactly one smelted stone collected below"
    );
}
