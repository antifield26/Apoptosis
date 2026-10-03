//! Stalk growth and grass spread, slice 2b (P20-02).
//!
//! All farms run on the fixed `DEFAULT_RANDOM_SEED`, so every total below is
//! a deterministic pin, not a flake: thresholds were written **before** the
//! run from the per-cell hit rate (`sections × 3 / (16 × 16 × 384) ≈
//! 7.3e-4`/tick, invariant to the loaded-chunk count), then confirmed by it.
//!
//! | Farm | Ticks | Expected events | Assert |
//! |---|---|---|
//! | 100 cane at age 15 + 30 at age 0 | 400 | `≈ 29` + `≈ 9` hits | `≥ 12` stalks reach height 2, none above 3, `≥ 3` age-0 climb |
//! | 20 cane triple-stacks | 400 | `≈ 6` top hits | every stack still height 3 |
//! | 60 cactus at age 15 + 20 at age 0 | 400 | `≈ 17` + `≈ 6` hits | `≥ 8` reach height 2, none above 3, `≥ 2` age-0 climb |
//! | 64 grass in 12×12 dirt + 40 roofed + 20/20 water checker | 400 | spread attempts on dirt, hits on roofed | bed grows `≥ 3`, `≥ 4`/40 starve, 20/20 water-dirt hold |
//! | 64 mycelium in 12×12 dirt | 400 | `≈ 19` hits × 4 attempts | bed grows `≥ 3` past 64 |

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
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
}

/// Recompute light over every loaded chunk: hand-placed farm blocks bypass
/// the edit path's light queue, so load-time light is stale over them (the
/// same settle the growth suite documents).
fn settle_after_streaming(harness: &mut Harness) {
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let table = harness.game.registries().light.clone();
    let loaded: Vec<ChunkPos> = harness.game.world().chunk_positions().collect();
    for pos in loaded {
        harness.game.world_mut().invalidate_light_3x3(pos);
        harness
            .game
            .world_mut()
            .compute_light(pos, &table)
            .expect("light computes");
    }
}

/// Height of the stalk of `name` standing on `(x, y, z)` (walk up while the
/// cell matches).
fn stalk_height(harness: &Harness, name: &str, x: i32, y: i32, z: i32) -> usize {
    let blocks = &harness.game.registries().blocks;
    let mut height = 0_usize;
    for dy in 0..8 {
        let Some(id) = harness.game.world().get_block_loaded(x, y + dy, z) else {
            break;
        };
        if blocks.block_name(id).unwrap_or("") != name {
            break;
        }
        height += 1;
    }
    height
}

/// A stalk of `name` at `(x, y, z)` with `age`, on a `support` block.
fn plant_stalk(harness: &mut Harness, name: &str, support: &str, x: i32, y: i32, z: i32, age: i32) {
    let blocks = harness.game.registries().blocks.clone();
    let ground = blocks.default_state(support).expect("support state");
    harness
        .game
        .world_mut()
        .set_block(x, y, z, ground)
        .expect("support placed");
    let stalk = blocks
        .state_id(name, &[("age".to_owned(), age.to_string())])
        .expect("aged stalk");
    harness
        .game
        .world_mut()
        .set_block(x, y + 1, z, stalk)
        .expect("stalk placed");
}

/// Cane climbs to three and stops.
///
/// Two plantings: age-15 stalks (one hit grows — the grow arm) and age-0
/// stalks (hits climb the age — the climb arm). The deterministic seed fixes
/// every draw, so the floors below are pins, not statistics.
#[test]
fn cane_grows_to_three_and_stops() {
    let mut harness = Harness::new("spread-cane");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    for i in 0..100 {
        let x = (i % 25) - 4;
        let z = (i / 25) - 4;
        plant_stalk(
            &mut harness,
            "minecraft:sugar_cane",
            "minecraft:sand",
            x,
            120,
            z,
            15,
        );
    }
    for i in 0..30 {
        plant_stalk(
            &mut harness,
            "minecraft:sugar_cane",
            "minecraft:sand",
            20 + (i % 10),
            120,
            4 + (i / 10),
            0,
        );
    }
    harness.stand(8, 121, -2);
    settle_after_streaming(&mut harness);
    for _ in 0..400 {
        harness.game.tick().expect("tick");
    }
    let mut tall = 0_usize;
    let mut max = 0_usize;
    for i in 0..100 {
        let x = (i % 25) - 4;
        let z = (i / 25) - 4;
        let height = stalk_height(&harness, "minecraft:sugar_cane", x, 121, z);
        assert!(height >= 1, "a planted cane is never eaten");
        if height >= 2 {
            tall += 1;
        }
        max = max.max(height);
    }
    assert!(
        tall >= 12,
        "age-15 cane grows on one hit (≈29 first-hits over the run), only {tall}/100 reached height 2"
    );
    assert!(max <= 3, "no stalk passes three, tallest is {max}");
    let mut climbed = 0_usize;
    for i in 0..30 {
        let x = 20 + (i % 10);
        let z = 4 + (i / 10);
        let Some(id) = harness.game.world().get_block_loaded(x, 121, z) else {
            continue;
        };
        let props = harness
            .game
            .registries()
            .blocks
            .properties_of(id)
            .unwrap_or_default();
        if props.iter().any(|(k, v)| k == "age" && v != "0") {
            climbed += 1;
        }
    }
    assert!(
        climbed >= 3,
        "age-0 cane must climb (≈9 hits over the run), only {climbed}/30 moved"
    );
}

/// A full triple-stack never grows (every top hit returns early on height).
#[test]
fn full_cane_stacks_never_grow() {
    let mut harness = Harness::new("spread-cane-cap");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let sand = blocks.default_state("minecraft:sand").expect("sand");
    let top = blocks
        .state_id(
            "minecraft:sugar_cane",
            &[("age".to_owned(), "15".to_owned())],
        )
        .expect("max-age cane");
    let mid = blocks
        .state_id(
            "minecraft:sugar_cane",
            &[("age".to_owned(), "0".to_owned())],
        )
        .expect("fresh cane");
    for i in 0..20 {
        let x = (i % 10) - 2;
        let z = (i / 10) - 2;
        harness
            .game
            .world_mut()
            .set_block(x, 120, z, sand)
            .expect("sand placed");
        for (dy, state) in [(1, mid), (2, mid), (3, top)] {
            harness
                .game
                .world_mut()
                .set_block(x, 120 + dy, z, state)
                .expect("cane placed");
        }
    }
    harness.stand(2, 124, -2);
    settle_after_streaming(&mut harness);
    for _ in 0..400 {
        harness.game.tick().expect("tick");
    }
    for i in 0..20 {
        let x = (i % 10) - 2;
        let z = (i / 10) - 2;
        assert_eq!(
            stalk_height(&harness, "minecraft:sugar_cane", x, 121, z),
            3,
            "a full stack stays full"
        );
    }
}

/// Cactus climbs to three and stops, flowerless (named gap).
#[test]
fn cactus_grows_to_three_and_stops() {
    let mut harness = Harness::new("spread-cactus");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    for i in 0..60 {
        let x = (i % 15) - 2;
        let z = (i / 15) - 2;
        plant_stalk(
            &mut harness,
            "minecraft:cactus",
            "minecraft:sand",
            x,
            120,
            z,
            15,
        );
    }
    for i in 0..20 {
        plant_stalk(
            &mut harness,
            "minecraft:cactus",
            "minecraft:sand",
            16 + (i % 10),
            120,
            4 + (i / 10),
            0,
        );
    }
    harness.stand(5, 121, -2);
    settle_after_streaming(&mut harness);
    for _ in 0..400 {
        harness.game.tick().expect("tick");
    }
    let mut tall = 0_usize;
    let mut max = 0_usize;
    for i in 0..60 {
        let x = (i % 15) - 2;
        let z = (i / 15) - 2;
        let height = stalk_height(&harness, "minecraft:cactus", x, 121, z);
        assert!(height >= 1, "a planted cactus is never eaten");
        if height >= 2 {
            tall += 1;
        }
        max = max.max(height);
    }
    assert!(
        tall >= 8,
        "age-15 cactus grows on one hit (≈17 first-hits over the run), only {tall}/60 reached height 2"
    );
    assert!(max <= 3, "no cactus passes three, tallest is {max}");
    let mut climbed = 0_usize;
    for i in 0..20 {
        let x = 16 + (i % 10);
        let z = 4 + (i / 10);
        let Some(id) = harness.game.world().get_block_loaded(x, 121, z) else {
            continue;
        };
        let props = harness
            .game
            .registries()
            .blocks
            .properties_of(id)
            .unwrap_or_default();
        if props.iter().any(|(k, v)| k == "age" && v != "0") {
            climbed += 1;
        }
    }
    assert!(
        climbed >= 2,
        "age-0 cactus must climb, only {climbed}/20 moved"
    );
}

/// Grass spreads over open dirt and starves under cover.
///
/// Three fields: a 8×8 grass patch in a dirt bed (spread), 40 roofed grass
/// cells (starvation needs the hit on the cell itself), and a 20/20
/// grass/dirt checkerboard under water roofs (propagation refusal — every
/// water-dirt has grass neighbours attempting spread onto it, so zero
/// conversions is a real refusal, not an unsampled vacuum).
#[test]
fn grass_spreads_and_starves() {
    let mut harness = Harness::new("spread-grass");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(2, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let grass = blocks
        .default_state("minecraft:grass_block")
        .expect("grass");
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    let water = blocks.default_state("minecraft:water").expect("water");
    // Spread bed: 8×8 grass in a 12×12 dirt bed, all open sky.
    for dx in -2..=9 {
        for dz in -2..=9 {
            let cell = if (0..8).contains(&dx) && (0..8).contains(&dz) {
                grass
            } else {
                dirt
            };
            harness
                .game
                .world_mut()
                .set_block(dx, 120, dz, cell)
                .expect("bed placed");
        }
    }
    // Starvation row: 40 grass cells with a stone roof directly above.
    for i in 0..40 {
        harness
            .game
            .world_mut()
            .set_block(20 + i, 120, 0, grass)
            .expect("grass placed");
        harness
            .game
            .world_mut()
            .set_block(20 + i, 121, 0, stone)
            .expect("roof placed");
    }
    // Refusal checkerboard: alternating grass and dirt, every dirt cell
    // water-roofed, every dirt cell neighbouring grass on all sides.
    for i in 0..40 {
        let x = 20 + (i % 10);
        let z = 10 + (i / 10);
        let cell = if (i % 2) == 0 { grass } else { dirt };
        harness
            .game
            .world_mut()
            .set_block(x, 120, z, cell)
            .expect("cell placed");
        if (i % 2) == 1 {
            harness
                .game
                .world_mut()
                .set_block(x, 121, z, water)
                .expect("water placed");
        }
    }
    harness.stand(4, 121, 4);
    settle_after_streaming(&mut harness);
    // Count per region: the bed must grow while the roofed row dies — a
    // global count mixes the two (the first draft did, and a healthy spread
    // hid inside a larger starvation).
    let bed_before = grass_region(&harness, -2, 10, -2, 10);
    for _ in 0..400 {
        harness.game.tick().expect("tick");
    }
    let bed_after = grass_region(&harness, -2, 10, -2, 10);
    assert!(
        bed_after >= bed_before + 3,
        "open grass must spread (bed started at {bed_before}, ended at {bed_after})"
    );
    let mut starved = 0_usize;
    for i in 0..40 {
        if harness.block_name(20 + i, 120, 0) == "minecraft:dirt" {
            starved += 1;
        }
    }
    assert!(
        starved >= 4,
        "covered grass starves (≈12 hits over the run), only {starved}/40 turned"
    );
    for i in 0..40 {
        if (i % 2) == 1 {
            let x = 20 + (i % 10);
            let z = 10 + (i / 10);
            assert_eq!(
                harness.block_name(x, 120, z),
                "minecraft:dirt",
                "water-roofed dirt never converts"
            );
        }
    }
}

/// Count grass blocks in a region (inclusive) at y 120.
fn grass_region(harness: &Harness, x0: i32, x1: i32, z0: i32, z1: i32) -> usize {
    let mut count = 0_usize;
    for x in x0..=x1 {
        for z in z0..=z1 {
            if harness.block_name(x, 120, z) == "minecraft:grass_block" {
                count += 1;
            }
        }
    }
    count
}

impl Harness {
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

/// Mycelium spreads onto dirt the same way.
///
/// An 8×8 seed patch in a 12×12 dirt bed (the single-cell draft sampled
/// nothing — one cell in eight million is ~0.3 hits per run, and the
/// instrumented re-run proved the only starves anywhere were natural grass
/// under cover, i.e. the rule working, not the farm being seen).
#[test]
fn mycelium_spreads() {
    let mut harness = Harness::new("spread-mycelium");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let mycelium = blocks
        .default_state("minecraft:mycelium")
        .expect("mycelium");
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    for dx in -2..=9 {
        for dz in -2..=9 {
            let cell = if (0..8).contains(&dx) && (0..8).contains(&dz) {
                mycelium
            } else {
                dirt
            };
            harness
                .game
                .world_mut()
                .set_block(dx, 120, dz, cell)
                .expect("bed placed");
        }
    }
    harness.stand(4, 121, 4);
    settle_after_streaming(&mut harness);
    let before = mycelium_region(&harness, -2, 10, -2, 10);
    assert_eq!(before, 64, "the seed patch is planted");
    for _ in 0..400 {
        harness.game.tick().expect("tick");
    }
    let after = mycelium_region(&harness, -2, 10, -2, 10);
    assert!(
        after >= before + 3,
        "mycelium must spread (bed started at {before}, ended at {after})"
    );
}

/// Count mycelium blocks in a region (inclusive) at y 120.
fn mycelium_region(harness: &Harness, x0: i32, x1: i32, z0: i32, z1: i32) -> usize {
    let mut count = 0_usize;
    for x in x0..=x1 {
        for z in z0..=z1 {
            if harness.block_name(x, 120, z) == "minecraft:mycelium" {
                count += 1;
            }
        }
    }
    count
}
