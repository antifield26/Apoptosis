//! Regression pins for the P17 owner-session fixes (AUDIT-17 Lane E).
//!
//! Each test is the named detector for one historical fix: creative slot
//! writes, double-door pairing from the lower half, button pulse (no sticky
//! hold), and sneak-place via `player_input` bit 5. Falsification: neutralise
//! the matching arm in `session.rs` / `mod.rs` and exactly one of these goes
//! red.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_entity::GameMode;
use mc_entity::stack::ItemStack as EntityStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::ItemStack as WireStack;
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

    fn item_id(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }

    fn give(&mut self, item: &str) {
        let id = self.item_id(item);
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, EntityStack::new(id, 1).expect("stack"))
            .expect("hotbar 0 takes it");
    }

    fn empty_hand(&mut self) {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, EntityStack::EMPTY)
            .expect("hand emptied");
    }

    fn look(&mut self, yaw: f32) {
        self.game.player_mut(self.id).expect("player").yaw = yaw;
    }

    fn set_game_mode(&mut self, mode: GameMode) {
        self.game.player_mut(self.id).expect("player").game_mode = mode;
    }

    fn inv_count(&self, name: &str) -> i32 {
        let id = self.item_id(name);
        let inv = &self.game.player(self.id).expect("player").inventory;
        (0..41)
            .map(|slot| {
                let stack = inv.slot(slot);
                if stack.item_id() == Some(id) {
                    stack.count()
                } else {
                    0
                }
            })
            .sum()
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

    /// Click with an explicit cursor (door hinge tiebreak).
    fn click_cursor(&mut self, x: i32, y: i32, z: i32, face: i32, cursor: (f32, f32, f32)) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: cursor.0,
            cursor_y: cursor.1,
            cursor_z: cursor.2,
            inside_block: false,
            world_border_hit: false,
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

    fn name(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("named")
            .to_owned()
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
        let id = self
            .game
            .registries()
            .blocks
            .default_state(name)
            .unwrap_or_else(|_| panic!("{name} is a known block"));
        self.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("block set");
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
        for z in (sz - 3)..=(sz + 3) {
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

/// c66f1d2: creative takes land in the inventory; survival writes are refused.
#[test]
fn creative_take_writes_hotbar_and_survival_is_refused() {
    let mut harness = Harness::new("p17-pin-creative");
    harness.join("Architect");
    harness.set_game_mode(GameMode::Creative);
    let stick = harness.item_id("minecraft:stick");
    // Window slot 36 is hotbar 0.
    harness.intent(PlayIntent::SetCreativeModeSlot {
        slot: 36,
        item: WireStack {
            item_id: stick,
            count: 5,
            components: Vec::new(),
        },
    });
    assert_eq!(
        harness.inv_count("minecraft:stick"),
        5,
        "creative take lands"
    );
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .inventory
            .slot(0)
            .count(),
        5
    );

    // Survival must refuse the same write.
    harness.set_game_mode(GameMode::Survival);
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(0, EntityStack::EMPTY)
        .expect("clear");
    harness.intent(PlayIntent::SetCreativeModeSlot {
        slot: 36,
        item: WireStack {
            item_id: stick,
            count: 7,
            components: Vec::new(),
        },
    });
    assert_eq!(
        harness.inv_count("minecraft:stick"),
        0,
        "survival never mints from set_creative_mode_slot"
    );
}

/// d95ce2a / 80d4558: an opposite-hinge pair flips together, including from
/// the upper half (the `lower_y` remap).
#[test]
fn opposite_hinge_double_doors_open_together_from_either_half() {
    let mut harness = Harness::new("p17-pin-double-door");
    harness.join("Carpenter");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0); // look south → facing north
    let (x0, y0, z0) = (sx + 1, sy, sz);
    let (x1, _, z1) = (sx + 2, sy, sz);

    // Left leaf (cursor west → hinge left) then right leaf (cursor east).
    harness.give("minecraft:oak_door");
    harness.click_cursor(x0, y0 - 1, z0, 1, (0.2, 0.5, 0.5));
    harness.give("minecraft:oak_door");
    harness.click_cursor(x1, y1_floor(y0), z1, 1, (0.8, 0.5, 0.5));
    assert_eq!(harness.name(x0, y0, z0), "minecraft:oak_door");
    assert_eq!(harness.name(x1, y0, z1), "minecraft:oak_door");
    let h0 = harness.prop(x0, y0, z0, "hinge");
    let h1 = harness.prop(x1, y0, z1, "hinge");
    assert_ne!(h0, h1, "opposite hinges form a pair, got {h0}/{h1}");

    // Click the **upper** half of the first leaf: both leaves must open.
    harness.empty_hand();
    harness.click(x0, y0 + 1, z0, 1);
    assert_eq!(
        harness.prop(x0, y0, z0, "open"),
        "true",
        "clicked leaf opens"
    );
    assert_eq!(
        harness.prop(x1, y0, z1, "open"),
        "true",
        "paired leaf follows"
    );
    assert_eq!(harness.prop(x0, y0 + 1, z0, "open"), "true");
    assert_eq!(harness.prop(x1, y0 + 1, z1, "open"), "true");

    // Close from the lower half of the second leaf: both close again.
    harness.click(x1, y0, z1, 1);
    assert_eq!(harness.prop(x0, y0, z0, "open"), "false");
    assert_eq!(harness.prop(x1, y0, z1, "open"), "false");
}

fn y1_floor(y0: i32) -> i32 {
    y0 - 1
}

/// ae94f4b / f16b25a: a button is a pulse — press schedules unpress and a
/// second press inside the hold does not stick it on.
#[test]
fn stone_button_unpresses_after_the_hold_and_never_sticks() {
    let mut harness = Harness::new("p17-pin-button");
    harness.join("Stoner");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (x, y, z) = (sx + 1, sy, sz);
    // Place the button on the floor's top face.
    harness.give("minecraft:stone_button");
    harness.click(x, y - 1, z, 1);
    assert_eq!(harness.name(x, y, z), "minecraft:stone_button");
    assert_eq!(
        harness.prop(x, y, z, "powered"),
        "false",
        "placement must not feed a pulse (owner A2)"
    );

    harness.empty_hand();
    harness.click(x, y, z, 1);
    assert_eq!(harness.prop(x, y, z, "powered"), "true", "press powers");
    // Two `intent` ticks already ran (press + re-press). The hold is 20 from
    // the first press, so 17 more ticks sit inside it and 20 clear it.
    harness.click(x, y, z, 1);
    harness.run(17);
    assert_eq!(harness.prop(x, y, z, "powered"), "true", "still held at 19");
    harness.run(4);
    assert_eq!(
        harness.prop(x, y, z, "powered"),
        "false",
        "stone button releases by tick 21"
    );
}

/// a9b1c63: sneak (`player_input` bit 5) places onto a container instead of
/// opening it — the real client's path (not `player_command` 0/1).
#[test]
fn player_input_bit5_sneak_places_onto_a_chest_instead_of_opening() {
    let mut harness = Harness::new("p17-pin-sneak");
    harness.join("Builder");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (x, y, z) = (sx + 1, sy, sz);
    harness.set_block(x, y, z, "minecraft:chest");

    // Sneak on via player_input bit 5 (32).
    harness.intent(PlayIntent::PlayerInput { input: 32 });
    harness.give("minecraft:stone");
    harness.click(x, y, z, 1); // top face → stone lands above
    assert_eq!(
        harness.name(x, y + 1, z),
        "minecraft:stone",
        "sneak places against the chest"
    );
    assert_eq!(harness.name(x, y, z), "minecraft:chest", "chest stays");

    // Clear sneak and click again: the chest opens (no second stone).
    harness.intent(PlayIntent::PlayerInput { input: 0 });
    harness.empty_hand();
    harness.click(x, y, z, 1);
    assert_eq!(
        harness.name(x, y + 1, z),
        "minecraft:stone",
        "without sneak nothing new lands on the chest"
    );
}
