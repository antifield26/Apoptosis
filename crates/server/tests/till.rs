//! Trampling physics and the one click that reaches the hoe, slice 2a (P20-02).
//!
//! What is *here* is what a direct call cannot show. For trampling that is a
//! landing produced by the movement path: a 5-block drop tramples
//! deterministically (the roll is `nextFloat < fall − 0.5`, which a
//! ≥1.5-block fall always passes), standing still never lands at all, and a
//! falling mob tramples too. For tilling it is the dispatch itself: one
//! `UseItemOn` click must reach `apply_hoe`. Every mechanism behind that click —
//! the `TILLABLES` table, rooted dirt's drop, the three refusals, the wear each
//! till costs, the `fallOn` threshold — is a direct call in
//! `game::session::tests` / `game::growth::tests`, with no ticks and no
//! streaming (TEST-TIME-PLAN §2).
//!
//! `standing_still_never_tramples` runs ten ticks, not the 200 it used to: an
//! idle player sends no movement intent, so the landing path is never entered,
//! and a regression that trampled an idle player would be level-triggered —
//! firing on the first tick. The long run bought no extra coverage and cost 47 s
//! of this suite; the *threshold* it never actually reached (a fall under
//! `fall - 0.5` never passes the roll) is pinned directly by
//! `game::growth::tests::short_falls_do_not_trample`.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
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
    /// View distance 2 (TEST-TIME-PLAN §3): the physics under test is local to
    /// the landing cell, so the streaming ring is cost without coverage.
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
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

    /// Stand the player in the block at `(x, y, z)`.
    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    /// Right-click the block at `(x, y, z)` on `face` with the held item.
    fn click(&mut self, x: i32, y: i32, z: i32, face: i32) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
                    hand: 0,
                    position: block_position(x, y, z),
                    face,
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

    /// Fall straight down: one move intent `drop` blocks below the feet.
    fn drop(&mut self, x: f64, from_y: f64, z: f64, drop: f64) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::MovePlayerPos {
                    x,
                    y: from_y - drop,
                    z,
                    on_ground: false,
                }),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    /// Hover in place for a tick: clears a stale `on_ground` (teleporting via
    /// `stand` does not touch it, and a landing needs the airborne edge).
    fn hover(&mut self, x: f64, y: f64, z: f64) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::MovePlayerPos {
                    x,
                    y,
                    z,
                    on_ground: false,
                }),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
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

    /// Moist farmland at `(x, y, z)`.
    fn put_moist_farmland(&mut self, x: i32, y: i32, z: i32) {
        let soil = self
            .game
            .registries()
            .blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "7".to_owned())],
            )
            .expect("moist farmland");
        self.game
            .world_mut()
            .set_block(x, y, z, soil)
            .expect("soil placed");
    }
}

/// The click reaches the hoe handler (the phase proof for slice 2a): one
/// `UseItemOn` on dirt tills it.
///
/// Every mechanism behind the click is a direct call in
/// `game::session::tests`; this is the arm that proves `use_item_on` still
/// dispatches to `apply_hoe` — deleting that dispatch leaves all of those pins
/// green, which is why the migration keeps exactly one click.
#[test]
fn a_click_reaches_the_hoe() {
    let mut harness = Harness::new("till-wiring");
    harness.join("Tiller");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let dirt = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt");
    harness
        .game
        .world_mut()
        .set_block(8, 120, 8, dirt)
        .expect("soil placed");
    harness.give("minecraft:stone_hoe");
    harness.stand(8, 121, 6);
    harness.click(8, 120, 8, 1);
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:farmland",
        "the click reached apply_hoe and tilled the soil"
    );
}

/// A 5-block landing turns farmland to dirt (deterministic: the roll always
/// passes at that distance, jar and here).
#[test]
fn fall_tramples_farmland() {
    let mut harness = Harness::new("trample-fall");
    harness.join("Faller");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.put_moist_farmland(8, 120, 8);
    harness.stand(8, 125, 8);
    harness.hover(8.5, 125.0, 8.5);
    harness.drop(8.5, 125.0, 8.5, 5.0);
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:dirt",
        "a 5-block landing tramples"
    );
}

/// Standing still never lands, so it never tramples.
///
/// Ten ticks, not the two hundred this used to run: an idle player sends no
/// movement intent, so the landing path (`move_player`) is never entered at all,
/// and a regression that trampled an idle player would have to be level-
/// triggered — which fires on the first tick. The long run cost 47 s and caught
/// nothing extra; the *threshold* it never reached is pinned directly by
/// `game::growth::tests::short_falls_do_not_trample`.
#[test]
fn standing_still_never_tramples() {
    let mut harness = Harness::new("trample-still");
    harness.join("Standee");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.put_moist_farmland(8, 120, 8);
    harness.stand(8, 121, 8);
    for _ in 0..10 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:farmland",
        "no landing, no trampling"
    );
}

/// A falling mob tramples too (`mob_griefing` reads the stored rule, P20-05;
/// default true, which is what this pin runs under).
#[test]
fn mob_fall_tramples_farmland() {
    let mut harness = Harness::new("trample-mob");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 1)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 1)), "field loads");
    harness.put_moist_farmland(8, 120, 8);
    // A stone shaft around the fall column: mob steering works mid-air, so
    // an open drop wanders off the field (measured). The shaft forces the
    // fall straight down onto the soil.
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for y in 121..=150 {
        for (x, z) in [
            (7, 7),
            (7, 8),
            (7, 9),
            (8, 7),
            (8, 9),
            (9, 7),
            (9, 8),
            (9, 9),
        ] {
            harness
                .game
                .world_mut()
                .set_block(x, y, z, stone)
                .expect("shaft writes");
        }
    }
    // Eight zombies in a column: one landing passes the roll only about
    // half the time at these speeds (mob steering interferes with the fall,
    // so the landing step never reaches the always-passes range), but eight
    // independent landings on the same cell make a miss vanishingly unlikely
    // — and the seed fixes every draw, so this is a pin, not a flake.
    for k in 0..8 {
        harness
            .game
            .spawn_mob(
                mc_entity::mob::MobKind::Zombie,
                mc_world::Vec3::new(8.5, 150.0 + f64::from(k) * 2.0, 8.5),
            )
            .expect("zombie spawns");
    }
    for _ in 0..150 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:dirt",
        "a falling zombie tramples"
    );
}
