//! Doors, trapdoors and fence gates end to end (P17-01 Step A).
//!
//! Through the real tick loop: placement orients (doors two halves with
//! hinge, trapdoors by face, gates by look), right-click toggles both
//! halves (iron refuses), redstone opens and closes, breaking one half
//! removes the other.
//!
//! Falsification shape: stop flipping the far half and the two-halves
//! assertions fail; stop feeding redstone on toggle and the observer
//! half (P17-01 Step B groundwork) goes dark — covered here by the
//! lever-driven open/close.

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

    fn empty_hand(&mut self) {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::EMPTY)
            .expect("hand emptied");
    }

    fn stand(&mut self) {
        self.game.player_mut(self.id).expect("player").on_ground = true;
    }

    fn look(&mut self, yaw: f32) {
        self.game.player_mut(self.id).expect("player").yaw = yaw;
    }

    /// Right-click the block at `(x, y, z)` on `face` with the cursor.
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

    /// Props of the block at `(x, y, z)` as (key, value) pairs.
    fn props(&self, x: i32, y: i32, z: i32) -> Vec<(String, String)> {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .properties_of(id)
            .expect("props read")
    }

    fn prop(&self, x: i32, y: i32, z: i32, key: &str) -> String {
        self.props(x, y, z)
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
}

/// A stone floor row under the work area so placements have ground.
fn floor(harness: &mut Harness, sx: i32, sy: i32, sz: i32) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for x in (sx - 4)..=(sx + 6) {
        harness.game.load_chunk(ChunkPos::new(x >> 4, sz >> 4));
        harness
            .game
            .world_mut()
            .set_block(x, sy - 1, sz, stone)
            .expect("floor");
    }
}

#[test]
fn placing_a_door_sets_both_oriented_halves() {
    let mut harness = Harness::new("p17-door-place");
    harness.join("Carpenter");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    // Look south: the door front faces the clicker (north).
    harness.look(0.0);
    harness.give("minecraft:oak_door");
    // Click the floor top under the target cell.
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    assert_eq!(
        harness.name(at.0, at.1, at.2),
        "minecraft:oak_door",
        "the lower half lands"
    );
    assert_eq!(
        harness.name(at.0, at.1 + 1, at.2),
        "minecraft:oak_door",
        "and the upper half with it"
    );
    assert_eq!(harness.prop(at.0, at.1, at.2, "facing"), "north");
    assert_eq!(harness.prop(at.0, at.1, at.2, "half"), "lower");
    assert_eq!(harness.prop(at.0, at.1 + 1, at.2, "half"), "upper");
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "false");
    // Symmetric stone floor, centred cursor: the hinge tiebreak lands left.
    assert_eq!(harness.prop(at.0, at.1, at.2, "hinge"), "right");
}

#[test]
fn right_click_toggles_both_halves_and_iron_refuses() {
    let mut harness = Harness::new("p17-door-toggle");
    harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:oak_door");
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    harness.empty_hand();
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "true");
    assert_eq!(
        harness.prop(at.0, at.1 + 1, at.2, "open"),
        "true",
        "the far half follows"
    );
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "false");
    assert_eq!(harness.prop(at.0, at.1 + 1, at.2, "open"), "false");
    // Iron is redstone-only: the hand does nothing.
    harness.give("minecraft:iron_door");
    harness.click(sx + 3, sy - 1, sz, 1);
    let iron = (sx + 3, sy, sz);
    assert_eq!(harness.name(iron.0, iron.1, iron.2), "minecraft:iron_door");
    harness.empty_hand();
    harness.click(iron.0, iron.1, iron.2, 1);
    assert_eq!(
        harness.prop(iron.0, iron.1, iron.2, "open"),
        "false",
        "an iron door ignores the hand"
    );
}

#[test]
fn redstone_opens_a_door_and_its_loss_closes_it() {
    let mut harness = Harness::new("p17-door-power");
    harness.join("Sparky");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:oak_door");
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    // A lever on the floor beside the door: flip on, the door opens.
    harness.give("minecraft:lever");
    harness.click(sx + 2, sy - 1, sz, 1);
    harness.empty_hand();
    harness.click(sx + 2, sy, sz, 1);
    harness.run(5);
    assert_eq!(
        harness.prop(at.0, at.1, at.2, "open"),
        "true",
        "a live neighbour opens the door"
    );
    assert_eq!(harness.prop(at.0, at.1, at.2, "powered"), "true");
    harness.click(sx + 2, sy, sz, 1);
    harness.run(5);
    assert_eq!(
        harness.prop(at.0, at.1, at.2, "open"),
        "false",
        "killing the power closes it again"
    );
}

#[test]
fn breaking_one_door_half_removes_the_other() {
    let mut harness = Harness::new("p17-door-break");
    harness.join("Wrecker");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:oak_door");
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    // Dig the lower half with an axe (doors are axe work, 3.0 hard).
    harness.give("minecraft:diamond_axe");
    harness.stand();
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(at.0, at.1, at.2),
        facing: 1,
        sequence: 0,
    });
    harness.run(15);
    let air = harness.game.registries().blocks.air_id();
    assert_eq!(
        harness.game.world().get_block_loaded(at.0, at.1, at.2),
        Some(air),
        "the dug half is gone"
    );
    assert_eq!(
        harness.game.world().get_block_loaded(at.0, at.1 + 1, at.2),
        Some(air),
        "and its partner with it"
    );
}

#[test]
fn trapdoors_orient_by_face_and_toggle() {
    let mut harness = Harness::new("p17-trapdoor");
    harness.join("Hatch");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:oak_trapdoor");
    // Top face clicked: bottom half, facing the clicker (south).
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    assert_eq!(harness.name(at.0, at.1, at.2), "minecraft:oak_trapdoor");
    assert_eq!(harness.prop(at.0, at.1, at.2, "half"), "bottom");
    assert_eq!(harness.prop(at.0, at.1, at.2, "facing"), "south");
    harness.empty_hand();
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "true");
}

#[test]
fn fence_gates_face_the_clicker_and_swing_back_on_open() {
    let mut harness = Harness::new("p17-gate");
    harness.join("Gatekeeper");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    harness.give("minecraft:oak_fence_gate");
    harness.click(sx + 1, sy - 1, sz, 1);
    let at = (sx + 1, sy, sz);
    assert_eq!(harness.prop(at.0, at.1, at.2, "facing"), "south");
    harness.empty_hand();
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "true");
    // Turn west, close, reopen: the gate swings to face the opener.
    harness.look(90.0);
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "false");
    harness.click(at.0, at.1, at.2, 1);
    assert_eq!(harness.prop(at.0, at.1, at.2, "open"), "true");
    assert_eq!(
        harness.prop(at.0, at.1, at.2, "facing"),
        "west",
        "an opening gate re-orients to its opener"
    );
}
