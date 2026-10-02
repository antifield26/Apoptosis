//! Random-tick growth, slice 1: crops and farmland moisture (P20-02).
//!
//! ## Why aggregates, not single plants
//!
//! One cell is sampled with probability `sections × 3 / (16 × 16 × 384)` per
//! tick — about 7.3e-4 — so a single wheat needs thousands of ticks to grow.
//! That is Vanilla's own rate, not a test hook problem, and the phase's
//! acceptance says exactly this: growth probability is tested
//! *statistically* with a seeded source. Every farm below therefore plants a
//! field (50–200 cells), runs a fixed tick count on the fixed
//! `DEFAULT_RANDOM_SEED`, and asserts a total far from zero with the
//! calculation beside it. Same seed twice is bit-identical (the game owns one
//! seeded source), so these are regression pins, not flakes.
//!
//! ## Thresholds — written **before** the run
//!
//! Per-cell hit rate `p ≈ 7.3e-4`/tick is invariant to the loaded-chunk count
//! (samples and cells scale together). Moist isolated wheat grows per hit with
//! `P ≈ 1/3` (bound `(int)(25/10)+1 = 3`); dry farmland ticks down 1 per hit.
//!
//! | Farm | Ticks | Expected events | Assert |
//! |---|---|---|
//! | 200 moist wheat | 300 | `200×300×7.3e-4/3 ≈ 14.6` age steps | `≥ 5` total gains |
//! | 100 wheat + 100 beetroot, moist | 300 | wheat ≈ 7.3, beet ≈ 2.4 | wheat sum `>` beet sum |
//! | 50 dark wheat (y=0, moist) | 300 | 0 (light gate) | exactly 0 gains, moisture sum unchanged |
//! | 100 dry farmland (moisture 1) | 300 | `100×300×7.3e-4 ≈ 22` decrements | moisture sum drop `≥ 8`, dirt `> 0` |
//!
//! If a measured total lands below its floor, the farm or the rule is wrong —
//! the floor is not widened to fit.

use std::collections::BTreeSet;

use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::{ChunkPos, Vec3};

/// Sky platform height: above the tallest P07 terrain (~112), so every plot
/// is open sky and the stale light the load computed (terrain was air here)
/// reads bright — the correct answer for an open field.
const FARM_Y: i32 = 120;

/// Underground band for the dark farm: solid stone on generated terrain.
const DARK_Y: i32 = 0;

/// Ticks every farm runs.
const FARM_TICKS: usize = 300;

fn open_world(tag: &str) -> (Game, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = mc_network::bridge::game_channel(256);
    let game =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    (game, dir)
}

/// Owned-storage game with a live channel, mirroring `fluid_core`'s harness:
/// the sweep centres on players, so sampling tests join one.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<mc_network::bridge::ClientEvent>,
    id: mc_network::bridge::ConnectionId,
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
        let (tx, rx) = mc_network::bridge::game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        Self {
            game,
            events: tx,
            id: mc_network::bridge::ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) {
        let (outbound, _out) = mc_network::bridge::OutboundSender::pair(self.id, 4096);
        self.events
            .try_send(mc_network::bridge::ClientEvent {
                id: self.id,
                kind: mc_network::bridge::ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
    }

    /// Stand the player in the block at `(x, y, z)`.
    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }
}

/// Hand-built growth tags (the test seam; production resolves the pack tags
/// in `load_packs`): every farmland moisture state grows crops, every wheat
/// age state holds dry soil.
fn install_growth_tags(game: &mut Game) {
    let blocks = &game.registries().blocks;
    let mut grows = BTreeSet::new();
    let mut maintains = BTreeSet::new();
    for level in 0..8 {
        grows.insert(
            blocks
                .state_id(
                    "minecraft:farmland",
                    &[("moisture".to_owned(), level.to_string())],
                )
                .expect("farmland moisture state"),
        );
    }
    for age in 0..8 {
        maintains.insert(
            blocks
                .state_id("minecraft:wheat", &[("age".to_owned(), age.to_string())])
                .expect("wheat age state"),
        );
    }
    game.set_growth_tags(grows, maintains);
}

/// One farm plot: moist farmland with a seedling crop above.
fn plant(game: &mut Game, x: i32, y: i32, z: i32, crop: &str) {
    let blocks = game.registries().blocks.clone();
    let soil = blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "7".to_owned())],
        )
        .expect("moist farmland");
    let seedling = blocks
        .state_id(crop, &[("age".to_owned(), "0".to_owned())])
        .expect("seedling crop");
    game.world_mut()
        .set_block(x, y, z, soil)
        .expect("soil placed");
    game.world_mut()
        .set_block(x, y + 1, z, seedling)
        .expect("crop placed");
}

/// Standing water at the plot (hydration source for the moisture rule).
fn pour(game: &mut Game, x: i32, y: i32, z: i32) {
    let water = game
        .registries()
        .blocks
        .default_state("minecraft:water")
        .expect("water");
    game.world_mut()
        .set_block(x, y, z, water)
        .expect("water placed");
}

/// Sum of `age` over every slice-1 crop cell in the region, plus the count of
/// cells holding each crop.
fn crop_totals(game: &Game, plots: &[(i32, i32, i32)]) -> (i64, i64, usize) {
    let blocks = &game.registries().blocks;
    let mut wheat = 0_i64;
    let mut beet = 0_i64;
    let mut cells = 0_usize;
    for &(x, y, z) in plots {
        let Some(id) = game.world().get_block_loaded(x, y, z) else {
            continue;
        };
        let Ok(name) = blocks.block_name(id) else {
            continue;
        };
        if name != "minecraft:wheat" && name != "minecraft:beetroots" {
            continue;
        }
        let props = blocks.properties_of(id).unwrap_or_default();
        let age = props
            .iter()
            .find(|(k, _)| k == "age")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0);
        assert!(age >= 0, "a crop age is never negative: {age}");
        if name == "minecraft:wheat" {
            assert!(age <= 7, "wheat never exceeds its max age: {age}");
            wheat += age;
        } else {
            assert!(age <= 3, "beetroot never exceeds its max age: {age}");
            beet += age;
        }
        cells += 1;
    }
    (wheat, beet, cells)
}

/// The sweep draws `sections × 3` slots per loaded chunk in the radius.
#[test]
fn the_sweep_samples_three_slots_per_section() {
    let mut harness = Harness::new("growth-samples");
    install_growth_tags(&mut harness.game);
    harness.join("Sampler");
    harness.stand(24, 121, 24);
    // Settle streaming: the measured tick then sees a stable loaded set.
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let report = harness.game.tick().expect("tick");
    // The expected count is read off the live loaded set, not hardcoded: the
    // pin is the *rate* (3 slots per section per chunk), while streaming owns
    // which chunks those are.
    // The player stands at (24, *, 24) and physics never moves x/z out of
    // chunk (1, 1), so the centre is a constant, not a float cast.
    let centre = ChunkPos::new(1, 1);
    let radius = 4;
    let loaded_in_radius = harness
        .game
        .world()
        .chunk_positions()
        .filter(|pos| (pos.x - centre.x).abs() <= radius && (pos.z - centre.z).abs() <= radius)
        .count();
    assert!(loaded_in_radius > 0, "streaming must have loaded chunks");
    let sections = harness.game.world().section_count();
    assert_eq!(
        report.random_tick_samples,
        loaded_in_radius * sections * 3,
        "every loaded chunk in the radius contributes sections × 3 samples"
    );
}

/// Moist wheat grows over a run of ticks (aggregate; see the module docs).
#[test]
fn wheat_grows_on_moist_farmland() {
    let mut harness = Harness::new("growth-wheat");
    install_growth_tags(&mut harness.game);
    let game = &mut harness.game;
    assert!(game.load_chunk(ChunkPos::new(0, 0)), "farm chunk loads");
    assert!(game.load_chunk(ChunkPos::new(1, 0)), "farm chunk loads");
    let mut plots = Vec::new();
    for (i, (x, z)) in (0..20)
        .flat_map(|dx| (0..10).map(move |dz| (dx, dz)))
        .enumerate()
    {
        let (wx, wz) = (x - 2, z - 2);
        if i % 4 == 3 {
            pour(game, wx, FARM_Y, wz);
            continue;
        }
        plant(game, wx, FARM_Y, wz, "minecraft:wheat");
        plots.push((wx, FARM_Y + 1, wz));
    }
    harness.join("Farmer");
    harness.stand(8, 121, 2);
    settle_after_streaming(&mut harness);
    for _ in 0..FARM_TICKS {
        harness.game.tick().expect("tick");
    }
    let (wheat, _, cells) = crop_totals(&harness.game, &plots);
    assert_eq!(cells, plots.len(), "every seedling is still a crop");
    assert!(
        wheat >= 5,
        "200 moist wheat over 300 ticks should gain ≈14 ages, got {wheat}"
    );
}

/// Beetroot's `nextInt(3)` pre-gate makes it grow slower than wheat.
#[test]
fn beetroot_grows_slower_than_wheat() {
    let mut harness = Harness::new("growth-beet");
    install_growth_tags(&mut harness.game);
    let game = &mut harness.game;
    assert!(game.load_chunk(ChunkPos::new(0, 0)), "farm chunk loads");
    let mut plots = Vec::new();
    for i in 0..200 {
        let x = i % 20 - 2;
        let z = i / 20 - 2;
        let crop = if i % 2 == 0 {
            "minecraft:wheat"
        } else {
            "minecraft:beetroots"
        };
        if i % 5 == 4 {
            pour(game, x, FARM_Y, z);
            continue;
        }
        plant(game, x, FARM_Y, z, crop);
        plots.push((x, FARM_Y + 1, z));
    }
    harness.join("Farmer");
    harness.stand(8, 121, 2);
    settle_after_streaming(&mut harness);
    for _ in 0..FARM_TICKS {
        harness.game.tick().expect("tick");
    }
    let (wheat, beet, _) = crop_totals(&harness.game, &plots);
    assert!(
        wheat > beet,
        "wheat ({wheat}) must outgrow beetroot ({beet}) under identical conditions"
    );
    assert!(beet > 0, "beetroot still grows, only slower: {beet}");
}

/// No light, no growth — and moisture stays put in the dark.
///
/// The dark room is a hand-sealed stone box (opaque in every direction, so
/// the settled light inside is 0 by construction), not "somewhere deep":
/// the first draft planted into raw terrain, where two plots sat in a
/// skylit flooded cave and grew — an honest bright reading, not a gate
/// failure, and the reason the room is built rather than found.
#[test]
fn crops_do_not_grow_in_the_dark() {
    let mut harness = Harness::new("growth-dark");
    install_growth_tags(&mut harness.game);
    let game = &mut harness.game;
    assert!(game.load_chunk(ChunkPos::new(-1, -1)), "farm chunk loads");
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for y in -3..=5 {
        for z in -14..=-7 {
            for x in -14..=-1 {
                game.world_mut()
                    .set_block(x, y, z, stone)
                    .expect("box sealed");
            }
        }
    }
    let mut plots = Vec::new();
    // Spacing-2 grid: plots on even x, water on the odd x beside each plot.
    // (Pouring at `x + 1` on a contiguous grid overwrites the neighbour's
    // water with soil — the first draft's dry-out.)
    for rz in 0..5 {
        for rx in 0..5 {
            let x = -12 + rx * 2;
            let z = -12 + rz;
            plant(game, x, DARK_Y, z, "minecraft:wheat");
            plots.push((x, DARK_Y + 1, z));
            pour(game, x + 1, DARK_Y, z);
        }
    }
    let moisture_before = soil_moisture_sum(game, &plots);
    harness.join("Miner");
    harness.stand(-7, 121, -9);
    settle_after_streaming(&mut harness);
    for _ in 0..FARM_TICKS {
        harness.game.tick().expect("tick");
    }
    let (wheat, _, _) = crop_totals(&harness.game, &plots);
    assert_eq!(wheat, 0, "nothing grows without light");
    assert_eq!(
        soil_moisture_sum(&harness.game, &plots),
        moisture_before,
        "dark soil is still watered, not dried"
    );
}

/// Dry soil ticks down and, uncovered, turns to dirt.
#[test]
fn dry_farmland_dries_then_dirts() {
    let mut harness = Harness::new("growth-dry");
    install_growth_tags(&mut harness.game);
    let game = &mut harness.game;
    assert!(game.load_chunk(ChunkPos::new(1, 1)), "farm chunk loads");
    let blocks = game.registries().blocks.clone();
    let mut plots = Vec::new();
    for i in 0..100 {
        let x = i % 10 + 20;
        let z = i / 10 + 20;
        let dry = blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "1".to_owned())],
            )
            .expect("dry farmland");
        game.world_mut()
            .set_block(x, FARM_Y, z, dry)
            .expect("soil placed");
        plots.push((x, FARM_Y, z));
    }
    harness.join("Farmer");
    harness.stand(24, 121, 24);
    settle_after_streaming(&mut harness);
    for _ in 0..FARM_TICKS {
        harness.game.tick().expect("tick");
    }
    // Moisture 1 over 100 cells starts at 100; every decrement is a hit.
    let mut moisture = 0_i64;
    let mut dirt = 0_usize;
    for &(x, y, z) in &plots {
        let Some(id) = harness.game.world().get_block_loaded(x, y, z) else {
            continue;
        };
        let Ok(name) = blocks.block_name(id) else {
            continue;
        };
        if name == "minecraft:dirt" {
            dirt += 1;
        } else if name == "minecraft:farmland" {
            let props = blocks.properties_of(id).unwrap_or_default();
            moisture += props
                .iter()
                .find(|(k, _)| k == "moisture")
                .and_then(|(_, v)| v.parse::<i64>().ok())
                .unwrap_or(0);
        }
    }
    assert!(
        100 - moisture >= 8,
        "100 dry soils over 300 ticks should lose ≈22 moisture, lost {}",
        100 - moisture
    );
    assert!(dirt > 0, "some dry uncovered soil became dirt");
}

/// Recompute light over the farm chunks after hand-placing blocks.
///
/// Direct `world.set_block` writes bypass the edit path's invalidate+queue,
/// so the load-time light is stale over every touched cell (in a flooded
/// cave that reads bright — exactly what the dark test first tripped on).
/// The live server converges here by draining its light queue; the test
/// settles synchronously instead, which is the same state a few idle ticks
/// later.
/// Let streaming settle, then recompute light over every loaded chunk (see
/// [`settle_light`]): hand-placed farm blocks must be lit as built, not as
/// the terrain was at load.
fn settle_after_streaming(harness: &mut Harness) {
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let loaded: Vec<ChunkPos> = harness.game.world().chunk_positions().collect();
    settle_light(&mut harness.game, &loaded);
}

fn settle_light(game: &mut Game, chunks: &[ChunkPos]) {
    let table = game.registries().light.clone();
    for &pos in chunks {
        game.world_mut().invalidate_light_3x3(pos);
        game.world_mut()
            .compute_light(pos, &table)
            .expect("light computes");
    }
}

/// Sum of `moisture` over the soils under `plots` (each plot's crop sits one
/// above its soil).
fn soil_moisture_sum(game: &Game, plots: &[(i32, i32, i32)]) -> i64 {
    let blocks = &game.registries().blocks;
    let mut sum = 0_i64;
    for &(x, y, z) in plots {
        let Some(id) = game.world().get_block_loaded(x, y - 1, z) else {
            continue;
        };
        if blocks.block_name(id).unwrap_or("") != "minecraft:farmland" {
            continue;
        }
        sum += blocks
            .properties_of(id)
            .unwrap_or_default()
            .iter()
            .find(|(k, _)| k == "moisture")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0);
    }
    sum
}

/// The age/moisture bands the growth code assumes, pinned against the fixture.
#[test]
fn max_ages_match_the_fixture() {
    let (game, _dir) = open_world("growth-pins");
    let blocks = &game.registries().blocks;
    for (name, max) in [
        ("minecraft:wheat", 7),
        ("minecraft:carrots", 7),
        ("minecraft:potatoes", 7),
        ("minecraft:beetroots", 3),
    ] {
        let mut seen = BTreeSet::new();
        let states = i32::try_from(blocks.state_count()).expect("state ids fit in i32");
        for id in 0..states {
            if blocks.block_name(id).unwrap_or("") != name {
                continue;
            }
            let props = blocks.properties_of(id).unwrap_or_default();
            if let Some(age) = props
                .iter()
                .find(|(k, _)| k == "age")
                .and_then(|(_, v)| v.parse::<i32>().ok())
            {
                seen.insert(age);
            }
        }
        assert_eq!(
            seen.into_iter().collect::<Vec<_>>(),
            (0..=max).collect::<Vec<_>>(),
            "{name} age band"
        );
    }
    let mut moisture = BTreeSet::new();
    let states = i32::try_from(blocks.state_count()).expect("state ids fit in i32");
    for id in 0..states {
        if blocks.block_name(id).unwrap_or("") != "minecraft:farmland" {
            continue;
        }
        let props = blocks.properties_of(id).unwrap_or_default();
        if let Some(level) = props
            .iter()
            .find(|(k, _)| k == "moisture")
            .and_then(|(_, v)| v.parse::<i32>().ok())
        {
            moisture.insert(level);
        }
    }
    assert_eq!(
        moisture.into_iter().collect::<Vec<_>>(),
        (0..=7).collect::<Vec<_>>(),
        "farmland moisture band"
    );
}
