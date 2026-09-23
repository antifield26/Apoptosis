//! Crafting-table window end to end (P17-02 Step C).
//!
//! Through the real tick loop with real clicks: right-click opens the
//! 3×3 window, grid clicks recompute, taking consumes, shift-click takes
//! consume exactly once (no minting), and closing returns leftovers.
//! A 1×3 slab row proves the width-3 path (it cannot match 2-wide).
//!
//! Falsification shape: a hardcoded width 2 leaves the slab row empty;
//! skipping the shift-click consume mints a second batch of sticks.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{MENU_CRAFTING, OpenScreen, PlayIntent, block_position};
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

    fn item(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }

    fn give_count(&mut self, item: &str, count: i32) {
        let id = self.item(item);
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(id, count).expect("stack"),
            )
            .expect("hotbar 0 takes it");
    }

    fn click_slot(&mut self, slot: i16, button: i8, click_type: i32) {
        let window = self.game.menu_window_id(self.id).expect("window");
        let state = self.game.menu_state_id(self.id).expect("state");
        self.intent(PlayIntent::ContainerClick {
            window_id: i32::from(window),
            state_id: state,
            slot,
            button,
            click_type,
        });
    }

    fn close(&mut self) {
        let window = self.game.menu_window_id(self.id).expect("window");
        self.intent(PlayIntent::ContainerClose {
            window_id: i32::from(window),
        });
    }

    fn menu(&self, slot: usize) -> mc_entity::stack::ItemStack {
        self.game.menu_slot(self.id, slot).expect("menu slot")
    }

    fn inventory_count(&self, item: &str) -> i32 {
        let id = self.item(item);
        let player = self.game.player(self.id).expect("player");
        (0..player.inventory.stored_slots())
            .map(|i| {
                let s = player.inventory.slot(i);
                if s.item_id() == Some(id) {
                    s.count()
                } else {
                    0
                }
            })
            .sum()
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

/// Place a crafting table one east of spawn and open it with an empty hand.
fn open_table(harness: &mut Harness, out: &mut InboundReceiver) -> (i32, i32, i32) {
    let (sx, sy, sz) = harness.game.spawn();
    floor(harness, sx, sy, sz);
    let (tx, ty, tz) = (sx + 1, sy, sz);
    harness.give_count("minecraft:crafting_table", 1);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(tx, ty - 1, tz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 0.5,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: 0,
    });
    // Empty the hand, then click the table itself.
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(0, mc_entity::stack::ItemStack::EMPTY)
        .expect("hand emptied");
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(tx, ty, tz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 0.5,
        cursor_z: 0.5,
        inside_block: false,
        world_border_hit: false,
        sequence: 0,
    });
    let mut types = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            types.push(OpenScreen::decode(&raw.payload).expect("decodes").menu_type);
        }
    }
    assert_eq!(types, vec![MENU_CRAFTING], "one crafting window");
    (tx, ty, tz)
}

#[test]
fn right_click_opens_the_crafting_window() {
    let mut harness = Harness::new("p17-craft-open");
    let mut out = harness.join("Crafter");
    open_table(&mut harness, &mut out);
    assert_eq!(
        harness.game.menu_slot_count(harness.id),
        Some(46),
        "result plus 9 grid plus 36 player"
    );
}

#[test]
fn two_planks_craft_sticks_and_taking_consumes() {
    let mut harness = Harness::new("p17-craft-sticks");
    let mut out = harness.join("Crafter");
    open_table(&mut harness, &mut out);
    let sticks = harness.item("minecraft:stick");
    // Hotbar 0 is menu slot 37 in the crafting window. Pick the stack up,
    // right-click one plank into grid cells 2 and 5 (middle column), leaving
    // the cursor empty for the take.
    harness.give_count("minecraft:oak_planks", 2);
    harness.click_slot(37, 0, 0);
    harness.click_slot(2, 1, 0);
    harness.click_slot(5, 1, 0);
    let result = harness.menu(0);
    assert_eq!(result.item_id(), Some(sticks));
    assert_eq!(result.count(), 4);
    // Take: the cursor holds sticks and the grid is empty.
    harness.click_slot(0, 0, 0);
    let cursor = harness.game.menu_cursor(harness.id).expect("cursor");
    assert_eq!(cursor.item_id(), Some(sticks));
    assert_eq!(cursor.count(), 4);
    assert!(harness.menu(2).is_empty() && harness.menu(5).is_empty());
}

#[test]
fn a_plank_row_crafts_slabs_only_three_wide() {
    let mut harness = Harness::new("p17-craft-slabs");
    let mut out = harness.join("Crafter");
    open_table(&mut harness, &mut out);
    // Top row of the grid: menu slots 1, 2, 3.
    harness.give_count("minecraft:oak_planks", 4);
    harness.click_slot(37, 0, 0);
    harness.click_slot(1, 1, 0);
    harness.click_slot(2, 1, 0);
    harness.click_slot(3, 1, 0);
    let result = harness.menu(0);
    assert_eq!(
        result.item_id(),
        Some(harness.item("minecraft:oak_slab")),
        "the 1x3 row matches at width 3, got {result:?}"
    );
    assert_eq!(result.count(), 6);
}

#[test]
fn shift_clicking_the_result_consumes_exactly_once() {
    let mut harness = Harness::new("p17-craft-shift");
    let mut out = harness.join("Crafter");
    open_table(&mut harness, &mut out);
    harness.give_count("minecraft:oak_planks", 4);
    harness.click_slot(37, 0, 0);
    harness.click_slot(2, 1, 0);
    harness.click_slot(5, 1, 0);
    assert_eq!(harness.menu(0).count(), 4);
    // Shift-click the result (click_type 1): one batch moves, one craft consumed.
    harness.click_slot(0, 0, 1);
    assert!(harness.menu(2).is_empty() && harness.menu(5).is_empty());
    // One batch moved into the inventory, exactly one craft's worth.
    assert_eq!(harness.inventory_count("minecraft:stick"), 4);
    // A second shift-click finds an empty result and mints nothing.
    harness.click_slot(0, 0, 1);
    assert_eq!(harness.inventory_count("minecraft:stick"), 4);
}

#[test]
fn closing_returns_leftovers_to_the_inventory() {
    let mut harness = Harness::new("p17-craft-close");
    let mut out = harness.join("Crafter");
    open_table(&mut harness, &mut out);
    harness.give_count("minecraft:oak_planks", 4);
    harness.click_slot(37, 0, 0);
    // Park two planks in the grid, keep two on the cursor, then close:
    // cursor returns via the shared path, grid via the table return.
    harness.click_slot(1, 1, 0);
    harness.click_slot(2, 1, 0);
    harness.close();
    assert_eq!(
        harness.inventory_count("minecraft:oak_planks"),
        4,
        "grid plus cursor come home"
    );
}

#[test]
fn closing_the_player_inventory_returns_the_2x2_grid() {
    let mut harness = Harness::new("p17-player-grid-close");
    harness.join("Pockets");
    // Window 0: hotbar slot 0 is menu slot 36; the 2×2 grid is menu 1..=4.
    harness.give_count("minecraft:oak_planks", 4);
    harness.click_slot(36, 0, 0);
    harness.click_slot(1, 1, 0);
    harness.click_slot(2, 1, 0);
    harness.close();
    assert_eq!(
        harness.inventory_count("minecraft:oak_planks"),
        4,
        "closing window 0 returns the 2x2 leftovers"
    );
}
