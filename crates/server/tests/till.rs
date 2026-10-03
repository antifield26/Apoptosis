//! Hoe tilling and trampling, slice 2a: `HoeItem.TILLABLES` and
//! `FarmlandBlock.fallOn` (P20-02).
//!
//! Tilling is an intent path (one click, immediate); trampling rides landings
//! (a 5-block drop tramples deterministically — the roll is
//! `nextFloat < fall − 0.5`, which a ≥1.5-block fall always passes — while
//! standing still never lands at all, which is the exact negative control).

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
            Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
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

/// A hoe tills grass, path, dirt and coarse dirt into farmland or dirt.
#[test]
fn hoe_tills_the_table() {
    let mut harness = Harness::new("till-table");
    harness.join("Tiller");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let cases = [
        ("minecraft:grass_block", "minecraft:farmland"),
        ("minecraft:dirt_path", "minecraft:farmland"),
        ("minecraft:dirt", "minecraft:farmland"),
        ("minecraft:coarse_dirt", "minecraft:dirt"),
    ];
    for (i, (soil, _)) in cases.iter().enumerate() {
        let x = i32::try_from(i).expect("row fits") * 3;
        let id = blocks.default_state(soil).expect("soil state");
        harness
            .game
            .world_mut()
            .set_block(x, 120, 8, id)
            .expect("soil placed");
    }
    harness.give("minecraft:stone_hoe");
    for (i, (_, expect)) in cases.iter().enumerate() {
        let x = i32::try_from(i).expect("row fits") * 3;
        harness.stand(x, 121, 6);
        harness.click(x, 120, 8, 1);
        assert_eq!(
            harness.block_name(x, 120, 8),
            *expect,
            "hoe on {} gives {expect}",
            cases[i].0
        );
    }
    // One wear per till (jar `hurtAndBreak(1)`), no Unbreaking involved.
    let damage = harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .selected_item()
        .damage();
    assert_eq!(damage, Some(4), "four tills wear four");
}

/// Rooted dirt tills to dirt and drops hanging roots.
#[test]
fn rooted_dirt_tills_and_drops_roots() {
    let mut harness = Harness::new("till-rooted");
    harness.join("Tiller");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let rooted = blocks
        .default_state("minecraft:rooted_dirt")
        .expect("rooted dirt");
    harness
        .game
        .world_mut()
        .set_block(8, 120, 8, rooted)
        .expect("soil placed");
    harness.give("minecraft:stone_hoe");
    harness.stand(8, 121, 6);
    harness.click(8, 120, 8, 1);
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:dirt",
        "rooted dirt tills to dirt"
    );
    let roots: Vec<_> = harness
        .game
        .dropped_items()
        .iter()
        .filter_map(|(stack, _)| {
            stack.item_id().and_then(|id| {
                harness
                    .game
                    .registries()
                    .items
                    .name(id)
                    .ok()
                    .map(str::to_owned)
            })
        })
        .collect();
    assert!(
        roots.iter().any(|name| name == "minecraft:hanging_roots"),
        "hanging roots drop, saw {roots:?}"
    );
}

/// The hoe refuses: DOWN face, covered soil, untillable block — without wear.
#[test]
fn hoe_refusals_change_nothing() {
    let mut harness = Harness::new("till-refuse");
    harness.join("Tiller");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    // Covered dirt at x=8 (stone above), open dirt at x=11, stone at x=14.
    for x in [8, 11] {
        harness
            .game
            .world_mut()
            .set_block(x, 120, 8, dirt)
            .expect("soil placed");
    }
    harness
        .game
        .world_mut()
        .set_block(8, 121, 8, stone)
        .expect("cover placed");
    harness
        .game
        .world_mut()
        .set_block(14, 120, 8, stone)
        .expect("stone placed");
    harness.give("minecraft:stone_hoe");
    // DOWN face on open dirt.
    harness.stand(11, 121, 6);
    harness.click(11, 120, 8, 0);
    // Top face on covered dirt.
    harness.stand(8, 121, 6);
    harness.click(8, 120, 8, 1);
    // Top face on stone.
    harness.stand(14, 121, 6);
    harness.click(14, 120, 8, 1);
    assert_eq!(
        harness.block_name(11, 120, 8),
        "minecraft:dirt",
        "DOWN face refuses"
    );
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:dirt",
        "cover refuses"
    );
    assert_eq!(
        harness.block_name(14, 120, 8),
        "minecraft:stone",
        "stone refuses"
    );
    let damage = harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .selected_item()
        .damage();
    assert_eq!(damage, None, "refusals wear nothing");
}

/// A 5-block landing turns farmland to dirt (deterministic: the roll always
/// passes at that distance, jar and here).
#[test]
fn fall_tramples_farmland() {
    let mut harness = Harness::new("trample-fall");
    harness.join("Faller");
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
#[test]
fn standing_still_never_tramples() {
    let mut harness = Harness::new("trample-still");
    harness.join("Standee");
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
    harness.stand(8, 121, 8);
    for _ in 0..200 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:farmland",
        "no landing, no trampling"
    );
}

/// A falling mob tramples too (mobGriefing reads true until P20-05).
#[test]
fn mob_fall_tramples_farmland() {
    let mut harness = Harness::new("trample-mob");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 1)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 1)), "field loads");
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
    // A stone shaft around the fall column: mob steering works mid-air, so
    // an open drop wanders off the field (measured). The shaft forces the
    // fall straight down onto the soil.
    let stone = blocks.default_state("minecraft:stone").expect("stone");
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
