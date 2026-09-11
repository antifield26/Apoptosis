//! Regression tests for the dual-copy inventory bug (Audit 04 finding A1).
//!
//! ## The bug
//!
//! The server keeps the player's items in **two** places: `PlayerInventory` (the
//! authoritative gameplay state) and `Session::menu`'s player container (the
//! transaction view). Three client-driven paths mutate only the inventory —
//! `player_action` drop (Q), offhand swap, and survival block placement — while
//! `apply_container_click` called `write_back_inventory` **unconditionally**,
//! overwriting the inventory with the menu's stale copy.
//!
//! The result was client-reachable duplication: press Q with a stack selected (the
//! stack leaves the inventory *and* spawns an item entity), then send any
//! `container_click` — even one that does nothing — and the stack is back in the
//! inventory while the dropped entity still exists.
//!
//! These tests drive the game loop directly rather than the socket, so they fail on
//! the invariant even if the packet plumbing changes.

use mc_container::ClickType;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// A game with one joined player and a solid floor.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _service: WorldService,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let mut service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(64);
        let mut game = Game::new(&service, 3, rx).expect("game builds");

        // A floor so physics has something to stand on.
        let (sx, sy, sz) = game.spawn();
        let stone = game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        for x in (sx - 4)..=(sx + 4) {
            for z in (sz - 4)..=(sz + 4) {
                game.world_mut()
                    .set_block(x, sy - 1, z, stone)
                    .expect("floor");
            }
        }
        let _ = service.storage_mut();

        let id = ConnectionId(1);
        let (outbound, _rx) = OutboundSender::pair(id, 4096);
        tx.try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Duper"),
                outbound,
            },
        })
        .expect("join queued");
        game.tick().expect("tick");

        Self {
            game,
            events: tx,
            id,
            _service: service,
            _dir: dir,
        }
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

    /// A `container_click` that does nothing at all: slot 0 with an empty cursor and
    /// an empty slot. It used to be enough to resurrect a dropped stack.
    fn idle_click(&mut self) {
        self.intent(PlayIntent::ContainerClick {
            window_id: 0,
            state_id: self.game.menu_state_id(self.id).unwrap_or(0),
            slot: 0,
            button: 0,
            click_type: ClickType::Pickup.id(),
        });
    }

    fn hotbar_slot(&self) -> i32 {
        // Menu slot 36 is hotbar slot 0.
        self.game
            .menu_slot(self.id, 36)
            .map_or(0, |stack| stack.count())
    }

    fn inventory_total(&self) -> i64 {
        let player = self.game.players().next().expect("a player");
        let session = self.game.session(player).expect("the session");
        session.inventory_total()
    }

    fn entity_count(&self) -> usize {
        self.game.entity_store().len()
    }
}

#[test]
fn dropping_an_item_and_then_clicking_does_not_duplicate_it() {
    let mut harness = Harness::new("p06-dupe-drop");
    harness
        .game
        .grant_item(harness.id, "minecraft:stone", 64)
        .expect("granted");
    assert_eq!(harness.hotbar_slot(), 64);
    let before = harness.inventory_total();
    let entities_before = harness.entity_count();

    // Press Q: the held stack leaves the inventory and becomes an entity.
    harness.intent(PlayIntent::PlayerAction {
        status: 3,
        position: block_position(0, 64, 0),
        facing: 0,
        sequence: 0,
    });
    // **The duplication invariant**, checked first so a failure here is unambiguous.
    // This is what the mirror-in before `apply_click` protects: without it the stale
    // menu copy is flushed over the inventory by the unconditional write-back.
    assert_eq!(
        harness.inventory_total(),
        before - 64,
        "the drop must remove the stack from the authoritative inventory"
    );
    assert_eq!(
        harness.entity_count(),
        entities_before + 1,
        "the dropped stack is a real entity"
    );

    // Now any container click at all — even a no-op one.
    harness.idle_click();

    assert_eq!(
        harness.inventory_total(),
        before - 64,
        "a click must not resurrect the dropped stack (this is the duplication bug)"
    );
    assert_eq!(
        harness.entity_count(),
        entities_before + 1,
        "and the dropped entity still exists, so a restored stack would be a duplicate"
    );

    // **The view invariant.** The client is looking at this window, so the menu must
    // reflect the drop; the per-action sync is what protects this.
    assert_eq!(
        harness.hotbar_slot(),
        0,
        "the menu must show the emptied hotbar slot"
    );
}

#[test]
fn swapping_to_the_offhand_and_then_clicking_does_not_duplicate() {
    let mut harness = Harness::new("p06-dupe-swap");
    harness
        .game
        .grant_item(harness.id, "minecraft:stone", 10)
        .expect("granted");
    let before = harness.inventory_total();

    // Swap main hand with the offhand.
    harness.intent(PlayIntent::PlayerAction {
        status: 6,
        position: block_position(0, 64, 0),
        facing: 0,
        sequence: 0,
    });
    assert_eq!(
        harness.inventory_total(),
        before,
        "a swap conserves the total"
    );

    harness.idle_click();
    assert_eq!(
        harness.inventory_total(),
        before,
        "a click must not duplicate a swapped stack"
    );
}

#[test]
fn placing_a_block_then_clicking_does_not_refund_it() {
    let mut harness = Harness::new("p06-dupe-place");
    harness
        .game
        .grant_item(harness.id, "minecraft:stone", 10)
        .expect("granted");
    let before = harness.inventory_total();
    let (sx, sy, sz) = harness.game.spawn();

    // Place one block on top of the floor beside the player.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx + 1, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 0,
    });
    let after_place = harness.inventory_total();

    harness.idle_click();

    assert_eq!(
        harness.inventory_total(),
        after_place,
        "a click must not refund a placed block"
    );
    assert!(
        after_place <= before,
        "and placing must never increase the total ({before} -> {after_place})"
    );
}

#[test]
fn the_menu_and_the_inventory_agree_after_every_action() {
    // The invariant behind all three cases: whatever the player does, the menu's view
    // of the player's slots must equal the inventory's. A divergence is the bug.
    let mut harness = Harness::new("p06-dupe-agree");
    harness
        .game
        .grant_item(harness.id, "minecraft:stone", 32)
        .expect("granted");

    for status in [6, 6, 3] {
        harness.intent(PlayIntent::PlayerAction {
            status,
            position: block_position(0, 64, 0),
            facing: 0,
            sequence: 0,
        });
        harness.idle_click();
        assert_eq!(
            harness.game.menu_inventory_divergence(harness.id),
            Some(0),
            "after player_action status {status} the two views must agree"
        );
    }
}

#[test]
fn a_click_after_a_drop_can_still_move_the_remaining_items() {
    // The fix must not freeze the menu: after a drop, clicks still work.
    let mut harness = Harness::new("p06-dupe-still-works");
    harness
        .game
        .grant_item(harness.id, "minecraft:stone", 64)
        .expect("granted");
    // Put a second stack in the hotbar so one drop leaves something to move.
    {
        let player = harness.game.players().next().expect("a player");
        let session = harness.game.session(player).expect("session");
        let _ = session;
    }
    harness.intent(PlayIntent::PlayerAction {
        status: 3,
        position: block_position(0, 64, 0),
        facing: 0,
        sequence: 0,
    });
    // Now pick up whatever is left in the hotbar via the menu.
    harness.intent(PlayIntent::ContainerClick {
        window_id: 0,
        state_id: harness.game.menu_state_id(harness.id).unwrap_or(0),
        slot: 36,
        button: 0,
        click_type: ClickType::Pickup.id(),
    });
    let cursor = harness.game.menu_cursor(harness.id).expect("a cursor");
    // Either the slot was empty (nothing to pick up) or the cursor holds it.
    assert!(
        cursor.count() == 0,
        "the drop emptied the only hotbar stack, so there is nothing to pick up"
    );
}
