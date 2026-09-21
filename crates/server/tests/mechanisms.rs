//! Observers and dispensers end to end (P17-01 Step B).
//!
//! Through the real tick loop: an observer pulses dust behind it when its
//! watched block changes (and ignores changes elsewhere); a dispenser
//! fires exactly one item on a rising edge, re-arms on the fall, and opens
//! a nine-slot window on right-click.
//!
//! Falsification shape: never power the observer and the dust stays dark;
//! never spawn on dispense and the ground stays empty while the slot still
//! drains.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{OpenScreen, PlayIntent, block_position};
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

    fn empty_hand(&mut self) {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::EMPTY)
            .expect("hand emptied");
    }

    fn look(&mut self, yaw: f32) {
        self.game.player_mut(self.id).expect("player").yaw = yaw;
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
            sequence: 0,
        });
    }

    fn prop(&self, x: i32, y: i32, z: i32, key: &str) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .properties_of(id)
            .expect("props read")
            .iter()
            .find(|(k, _)| k == key)
            .map_or_else(|| panic!("{key} present"), |(_, v)| v.clone())
    }

    /// Dust power at `(x, y, z)` (0 when the cell holds no dust).
    fn dust_power(&self, x: i32, y: i32, z: i32) -> u8 {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        let name = self.game.registries().blocks.block_name(id).expect("named");
        if name != "minecraft:redstone_wire" {
            return 0;
        }
        self.prop(x, y, z, "power").parse().unwrap_or(0)
    }
}

/// A stone floor strip under the work area so placements have ground.
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
            // Clear the work volume to air: natural terrain here is
            // uncontrolled, and a hill inside a target cell would refuse
            // the placement the test is about (staged, like the floor).
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
fn an_observer_pulses_dust_behind_it_when_its_watched_block_changes() {
    let mut harness = Harness::new("p17-observer");
    harness.join("Watcher");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Look north: the observer front watches (sx+1, sy, sz-1).
    harness.look(180.0);
    harness.give("minecraft:observer");
    harness.click(sx + 1, sy - 1, sz, 1);
    assert_eq!(
        harness.prop(sx + 1, sy, sz, "facing"),
        "north",
        "the front faces the clicker"
    );
    // Dust on the back cell reads the pulse.
    harness.give("minecraft:redstone");
    harness.click(sx + 1, sy - 1, sz + 1, 1);
    harness.run(3);
    assert_eq!(
        harness.dust_power(sx + 1, sy, sz + 1),
        0,
        "dark observer, dark dust"
    );
    // Change the watched block: stone goes in front.
    harness.give("minecraft:stone");
    harness.click(sx + 1, sy - 1, sz - 1, 1);
    // One tick for the neighbour queue to propagate: the click tick latches
    // the observer in the Players phase (after ScheduledTicks), so the dust
    // lights on the *next* tick. Reading after three ticks sees only the
    // dark after the 2-tick pulse.
    harness.run(1);
    assert_eq!(
        harness.dust_power(sx + 1, sy, sz + 1),
        15,
        "the pulse lights the back dust"
    );
    assert_eq!(
        harness.prop(sx + 1, sy, sz, "powered"),
        "true",
        "and the observer flag is up"
    );
    harness.run(5);
    assert_eq!(
        harness.dust_power(sx + 1, sy, sz + 1),
        0,
        "two ticks later the pulse is over"
    );
    assert_eq!(harness.prop(sx + 1, sy, sz, "powered"), "false");
}

#[test]
fn an_observer_ignores_changes_it_does_not_face() {
    let mut harness = Harness::new("p17-observer-deaf");
    harness.join("Watcher");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(180.0);
    harness.give("minecraft:observer");
    harness.click(sx + 1, sy - 1, sz, 1);
    harness.give("minecraft:redstone");
    harness.click(sx + 1, sy - 1, sz + 1, 1);
    harness.run(3);
    // Change the block EAST of the observer (not faced): silence.
    harness.give("minecraft:stone");
    harness.click(sx + 2, sy - 1, sz, 1);
    harness.run(5);
    assert_eq!(harness.dust_power(sx + 1, sy, sz + 1), 0);
    assert_eq!(harness.prop(sx + 1, sy, sz, "powered"), "false");
}

#[test]
fn a_dispenser_fires_one_item_on_a_rising_edge() {
    let mut harness = Harness::new("p17-dispense");
    harness.join("Trigger");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Look south: the head points north, front cell (sx+1, sy, sz-1).
    harness.look(0.0);
    harness.give("minecraft:dispenser");
    harness.click(sx + 1, sy - 1, sz, 1);
    assert_eq!(harness.prop(sx + 1, sy, sz, "facing"), "north");
    // Fill white-box (the chest tests fill the same way): 10 stones.
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let pos = mc_container::BlockPos::new(sx + 1, sy, sz);
        let entity = harness
            .game
            .block_entities_mut()
            .get_mut(pos)
            .expect("dispenser entity exists from placement");
        let items = entity.data.items_mut().expect("dispenser items");
        items[0] = mc_entity::stack::ItemStack::new(stone, 10).expect("stack");
    }
    // Lever on the floor east of the dispenser; flip it on.
    harness.give("minecraft:lever");
    harness.click(sx + 2, sy - 1, sz, 1);
    harness.empty_hand();
    harness.click(sx + 2, sy, sz, 1);
    harness.run(10);
    let total: i32 = harness
        .game
        .dropped_items()
        .into_iter()
        .filter(|(stack, _)| stack.item_id() == Some(stone))
        .map(|(stack, _)| stack.count())
        .sum();
    assert_eq!(total, 1, "exactly one item left");
    let left: i32 = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(sx + 1, sy, sz))
        .and_then(|e| e.data.items())
        .map_or(-1, |items| {
            items.iter().map(mc_entity::stack::ItemStack::count).sum()
        });
    assert_eq!(left, 9, "one of ten dispensed");
    // Held power fires nothing more: the latch spent itself.
    harness.run(20);
    let held_total: i32 = harness
        .game
        .dropped_items()
        .into_iter()
        .filter(|(stack, _)| stack.item_id() == Some(stone))
        .map(|(stack, _)| stack.count())
        .sum();
    assert_eq!(held_total, 1, "no second shot without a falling edge");
    // Fall and rise again: the second shot leaves.
    harness.click(sx + 2, sy, sz, 1);
    harness.run(3);
    assert_eq!(
        harness.prop(sx + 1, sy, sz, "triggered"),
        "false",
        "the fall unlatches"
    );
    harness.click(sx + 2, sy, sz, 1);
    harness.run(10);
    // Sum counts, not entities: the two shots land on the same cell and the
    // ground-merge pass (P11-09) folds them into one stack of two.
    let rearmed_total: i32 = harness
        .game
        .dropped_items()
        .into_iter()
        .filter(|(stack, _)| stack.item_id() == Some(stone))
        .map(|(stack, _)| stack.count())
        .sum();
    assert_eq!(rearmed_total, 2, "re-armed, it fires again");
}

#[test]
fn a_dropper_drops_and_a_dispenser_window_opens_nine_slots() {
    let mut harness = Harness::new("p17-dropper-ui");
    let mut out = harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:dropper");
    harness.click(sx + 1, sy - 1, sz, 1);
    assert_eq!(
        harness.prop(sx + 1, sy, sz, "facing"),
        "north",
        "droppers head away like dispensers"
    );
    // Right-click with an empty hand opens the nine-slot window.
    harness.empty_hand();
    harness.click(sx + 1, sy, sz, 1);
    let mut types = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            types.push(OpenScreen::decode(&raw.payload).expect("decodes").menu_type);
        }
    }
    assert_eq!(
        types,
        vec![mc_protocol::packets::play::MENU_GENERIC_3X3],
        "one 3x3 window, saw {types:?}"
    );

    // The drop half, quick: dirt in, lever on, dirt out front.
    let dirt = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt");
    {
        let pos = mc_container::BlockPos::new(sx + 1, sy, sz);
        let entity = harness
            .game
            .block_entities_mut()
            .get_mut(pos)
            .expect("dropper entity");
        let items = entity.data.items_mut().expect("dropper items");
        items[4] = mc_entity::stack::ItemStack::new(dirt, 3).expect("stack");
    }
    harness.give("minecraft:lever");
    harness.click(sx + 2, sy - 1, sz, 1);
    harness.empty_hand();
    harness.click(sx + 2, sy, sz, 1);
    harness.run(10);
    assert!(
        harness
            .game
            .dropped_items()
            .iter()
            .any(|(stack, _)| stack.item_id() == Some(dirt)),
        "the dropper dropped its dirt"
    );
}
