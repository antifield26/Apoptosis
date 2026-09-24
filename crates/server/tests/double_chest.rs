//! Double chests end to end (P17-02 Step B).
//!
//! Through the real tick loop: adjacent placements join left/right,
//! opening shows the 54-slot window right-half-first, transactions split
//! back on close, breaking singles the survivor, hoppers see the merged
//! pair, cover refuses the open, and copper joins copper only.
//!
//! Falsification shape: never joining (always single) fails the type
//! assertions; right-first flipped to left-first fails the order read.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    MENU_GENERIC_9X3, MENU_GENERIC_9X6, OpenScreen, PlayIntent, block_position,
};
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

    fn stand(&mut self) {
        self.game.player_mut(self.id).expect("player").on_ground = true;
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

    /// Give plus click: survival consumes, so each placement needs its own give.
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
}

fn floor(harness: &mut Harness, sx: i32, sy: i32, sz: i32) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    for x in (sx - 4)..=(sx + 8) {
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
fn placing_adjacent_chests_forms_left_and_right() {
    let mut harness = Harness::new("p17-double-place");
    harness.join("Carpenter");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Look south: chests face north. The east chest joins on its
    // counter-clockwise (west) side, becoming RIGHT.
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    let (bx, by, bz) = (sx + 2, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    assert_eq!(harness.prop(ax, ay, az, "type"), "single");
    harness.place("minecraft:chest", bx, by - 1, bz, 1);
    assert_eq!(harness.prop(ax, ay, az, "facing"), "north");
    assert_eq!(harness.prop(ax, ay, az, "type"), "left");
    assert_eq!(harness.prop(bx, by, bz, "type"), "right");
    // A third chest past the pair stays single (no triple chests).
    harness.place("minecraft:chest", bx + 1, by - 1, bz, 1);
    assert_eq!(harness.prop(bx + 1, by, bz, "type"), "single");
    assert_eq!(harness.prop(bx, by, bz, "type"), "right");
}

#[test]
fn opening_a_double_shows_fifty_four_slots_right_first() {
    let mut harness = Harness::new("p17-double-open");
    let mut out = harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    let (bx, by, bz) = (sx + 2, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    harness.place("minecraft:chest", bx, by - 1, bz, 1);
    // East half (right) holds stone, west half (left) holds dirt.
    harness.fill(bx, by, bz, 0, "minecraft:stone", 5);
    harness.fill(ax, ay, az, 0, "minecraft:dirt", 7);
    harness.empty_hand();
    harness.click(ax, ay, az, 1);
    let mut screens = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            screens.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert_eq!(screens.len(), 1, "one window opens");
    assert_eq!(screens[0].menu_type, MENU_GENERIC_9X6);
    assert_eq!(
        screens[0].title,
        mc_protocol::text::TextComponent::literal("Large Chest")
    );
    // Pumpkin-mirrored order: slots 0..27 read the RIGHT half.
    let slot0 = harness.game.menu_slot(harness.id, 0).expect("menu slot 0");
    assert_eq!(slot0.item_id(), Some(harness.item("minecraft:stone")));
    assert_eq!(slot0.count(), 5);
    let slot27 = harness
        .game
        .menu_slot(harness.id, 27)
        .expect("menu slot 27");
    assert_eq!(slot27.item_id(), Some(harness.item("minecraft:dirt")));
    assert_eq!(slot27.count(), 7);
    // A lone chest still opens 27 with the plain title.
    harness.place("minecraft:chest", ax + 3, ay - 1, az, 1);
    harness.click(ax + 3, ay, az, 1);
    let mut single = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            single.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].menu_type, MENU_GENERIC_9X3);
}

#[test]
fn transacting_then_closing_splits_back_into_halves() {
    let mut harness = Harness::new("p17-double-transact");
    harness.join("Trader");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    let (bx, by, bz) = (sx + 2, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    harness.place("minecraft:chest", bx, by - 1, bz, 1);
    harness.empty_hand();
    harness.click(ax, ay, az, 1);
    let window = harness.game.menu_window_id(harness.id).expect("window");
    // Hotbar slot 0 of a 54-window is menu slot 81. Give stone, pick it up,
    // put it into menu slot 30 (left half, slot 3).
    harness.give("minecraft:stone");
    // Give overwrote hotbar 0 with 1 stone; top it to a visible stack.
    {
        let stone = harness.item("minecraft:stone");
        harness
            .game
            .player_mut(harness.id)
            .expect("player")
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(stone, 4).expect("stack"),
            )
            .expect("stones");
    }
    let mut state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: i32::from(window),
        state_id: state,
        slot: 81,
        button: 0,
        click_type: 0,
    });
    state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: i32::from(window),
        state_id: state,
        slot: 30,
        button: 0,
        click_type: 0,
    });
    harness.intent(PlayIntent::ContainerClose {
        window_id: i32::from(window),
    });
    // Menu slot 30 is the left half's slot 3: the west entity holds it, the
    // east entity is untouched.
    assert_eq!(harness.entity_total(ax, ay, az), 4);
    assert_eq!(harness.entity_total(bx, by, bz), 0);
    let west = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(ax, ay, az))
        .and_then(|e| e.data.items())
        .expect("west items")[3]
        .clone();
    assert_eq!(west.item_id(), Some(harness.item("minecraft:stone")));
}

#[test]
fn breaking_one_half_singles_the_other_and_drops() {
    let mut harness = Harness::new("p17-double-break");
    harness.join("Wrecker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    let (bx, by, bz) = (sx + 2, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    harness.place("minecraft:chest", bx, by - 1, bz, 1);
    harness.fill(ax, ay, az, 0, "minecraft:dirt", 6);
    harness.fill(bx, by, bz, 0, "minecraft:stone", 5);
    // Diamond axe, grounded: chest 2.5 breaks in ~10 ticks.
    let axe = harness.item("minecraft:diamond_axe");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(0, mc_entity::stack::ItemStack::new(axe, 1).expect("stack"))
        .expect("axe");
    harness.stand();
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(ax, ay, az),
        facing: 1,
        sequence: 0,
    });
    harness.run(15);
    let air = harness.game.registries().blocks.air_id();
    assert_eq!(
        harness.game.world().get_block_loaded(ax, ay, az),
        Some(air),
        "the dug half is gone"
    );
    assert_eq!(
        harness.prop(bx, by, bz, "type"),
        "single",
        "the survivor singles"
    );
    // The survivor keeps its own 5 stones; the broken half's 6 dirt drops.
    assert_eq!(harness.entity_total(bx, by, bz), 5);
    let dirt_drops: i32 = harness
        .game
        .dropped_items()
        .iter()
        .filter(|(stack, _)| stack.item_id() == Some(harness.item("minecraft:dirt")))
        .map(|(stack, _)| stack.count())
        .sum();
    assert!(dirt_drops >= 6, "the broken half drops, saw {dirt_drops}");
}

#[test]
fn hopper_below_a_double_pulls_across_halves() {
    let mut harness = Harness::new("p17-double-hopper");
    harness.join("Stoker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    let (bx, by, bz) = (sx + 2, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    harness.place("minecraft:chest", bx, by - 1, bz, 1);
    // Only the WEST half holds anything; the hopper sits below the EAST half.
    harness.fill(ax, ay, az, 0, "minecraft:dirt", 5);
    let air = harness.game.registries().blocks.air_id();
    harness
        .game
        .world_mut()
        .set_block(bx, ay - 1, bz, air)
        .expect("dig the hole");
    harness.place("minecraft:hopper", bx, ay - 1, bz + 1, 2);
    harness.run(10);
    assert!(
        harness.entity_total(bx, ay - 1, bz) >= 1,
        "the hopper under the empty half pulls the pair's dirt"
    );
}

#[test]
fn covered_chest_refuses_but_covered_barrel_opens() {
    let mut harness = Harness::new("p17-chest-cover");
    let mut out = harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    // Stone above via a scaffold pillar (clicking the chest would open it).
    harness.place("minecraft:dirt", ax - 1, ay - 1, az, 1);
    harness.place("minecraft:dirt", ax - 1, ay, az, 1);
    harness.place("minecraft:stone", ax - 1, ay + 1, az, 5);
    harness.empty_hand();
    harness.click(ax, ay, az, 1);
    let mut screens = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            screens.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert!(screens.is_empty(), "a covered chest stays closed");

    // Same cover over a barrel: barrels ignore cover like vanilla.
    harness.place("minecraft:barrel", ax + 2, ay - 1, az, 1);
    harness.place("minecraft:dirt", ax + 3, ay - 1, az, 1);
    harness.place("minecraft:dirt", ax + 3, ay, az, 1);
    harness.place("minecraft:stone", ax + 3, ay + 1, az, 4);
    harness.empty_hand();
    harness.click(ax + 2, ay, az, 1);
    let mut barrel_screens = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            barrel_screens.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert_eq!(barrel_screens.len(), 1, "a covered barrel still opens");
}

#[test]
fn copper_joins_copper_not_plain() {
    let mut harness = Harness::new("p17-copper-join");
    let mut out = harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (ax, ay, az) = (sx + 1, sy, sz);
    harness.place("minecraft:chest", ax, ay - 1, az, 1);
    harness.place("minecraft:copper_chest", ax + 1, ay - 1, az, 1);
    assert_eq!(harness.prop(ax, ay, az, "type"), "single");
    assert_eq!(harness.prop(ax + 1, ay, az, "type"), "single");
    harness.place("minecraft:copper_chest", ax + 2, ay - 1, az, 1);
    assert_eq!(harness.prop(ax + 1, ay, az, "type"), "left");
    assert_eq!(harness.prop(ax + 2, ay, az, "type"), "right");
    // Copper opens with its own title on the chest window.
    harness.empty_hand();
    harness.click(ax + 1, ay, az, 1);
    let mut screens = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            screens.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert_eq!(screens.len(), 1);
    assert_eq!(screens[0].menu_type, MENU_GENERIC_9X6);
    assert_eq!(
        screens[0].title,
        mc_protocol::text::TextComponent::literal("Large Copper Chest")
    );
}
