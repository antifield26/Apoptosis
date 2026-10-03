//! Phase wiring for slice 1: the `RandomTicks` sweep draws its samples and
//! reaches the crop and farmland handlers (P20-02).
//!
//! Exactly one full-tick test lives here. It carries the two facts a direct
//! handler call cannot show: the sweep draws `sections × 3` slots per loaded
//! chunk in the radius, and a crop really does grow through that sweep. Every
//! mechanism (growth rate, max age, beetroot's pre-gate, the light gate,
//! farmland drying, dirting and wetting, the growth-speed arithmetic) is
//! pinned by direct calls in `game::growth::tests` — same handlers, no ticks,
//! ~0.6 s for all seventeen. The fixture age/moisture bands stay here as well
//! (a registry read, no ticks). See TEST-TIME-PLAN §2 for why the split
//! exists; `tests/spread.rs` is the same shape for slice 2b.
//!
//! ## Why the wiring farm is an aggregate
//!
//! One cell is sampled with probability `sections × 3 / (16 × 16 × 384)` per
//! tick — 7.3e-4 — so a single wheat needs thousands of ticks to grow. That is
//! Vanilla's own rate, not a test hook problem. The farm below therefore plants
//! 100 cells, runs a fixed tick count on the fixed `DEFAULT_RANDOM_SEED`, and
//! asserts a total far from zero with the arithmetic beside it. Because the
//! per-cell hit rate is invariant to the loaded-chunk count (samples and cells
//! scale together), the run happens at view distance 2: 1 800 samples per tick
//! instead of 5 832, with identical statistics (TEST-TIME-PLAN §3).
//!
//! A moist isolated wheat grows on one hit in three (`bound = (int)(25/10)+1`),
//! so 100 cells × 200 ticks expect `100 × 200 × 7.3e-4 / 3 ≈ 4.9` age steps.
//! The floor is 2 — less than half, and the seed makes the run deterministic,
//! so this is a regression pin and not a flake.

use std::collections::BTreeSet;

use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::{ChunkPos, Vec3};

/// Sky platform height: above the tallest P07 terrain (~112), so every plot
/// is open sky and the stale light the load computed (terrain was air here)
/// reads bright — the correct answer for an open field.
const FARM_Y: i32 = 120;

/// Ticks the wiring farm runs after its measured tick.
const FARM_TICKS: usize = 200;

/// Sweep radius, matching the harness's view distance.
const RADIUS: i32 = 2;

/// Chunks the spacing-2 farm grid below stays inside.
const FARM_CHUNKS: [(i32, i32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Owned-storage game with a live channel, mirroring `spread.rs`'s harness:
/// the sweep centres on players, so the test joins one.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<mc_network::bridge::ClientEvent>,
    id: mc_network::bridge::ConnectionId,
    _dir: TempDir,
}

impl Harness {
    /// View distance 2 (TEST-TIME-PLAN §3): the per-cell hit rate is invariant
    /// to the loaded-chunk count, so statistics are identical while the sweep
    /// covers 25 chunks instead of 81.
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
            Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
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

/// One wheat seedling at `(x, y+1, z)` on a 3×3 moist-farmland patch at `y`.
///
/// The patch is the point: `getGrowthSpeed` scores that cell 10.0, the rate the
/// floor below assumes. A lone soil column would score 4.0 and grow half as
/// often.
fn plant(game: &mut Game, x: i32, y: i32, z: i32) {
    let blocks = game.registries().blocks.clone();
    let soil = blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "7".to_owned())],
        )
        .expect("moist farmland");
    let seedling = blocks
        .state_id("minecraft:wheat", &[("age".to_owned(), "0".to_owned())])
        .expect("wheat seedling");
    for dx in -1..=1 {
        for dz in -1..=1 {
            game.world_mut()
                .set_block(x + dx, y, z + dz, soil)
                .expect("soil placed");
        }
    }
    game.world_mut()
        .set_block(x, y + 1, z, seedling)
        .expect("crop placed");
}

/// Let streaming settle, then recompute light over every loaded chunk (see
/// [`settle_light`]): hand-placed farm blocks must be lit as built, not as the
/// terrain was at load. Direct `world.set_block` writes bypass the edit path's
/// invalidate+queue; the live server converges by draining its light queue, and
/// this settles synchronously to the same state.
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

/// Sum of `age` over every plot cell, asserting each cell still holds the
/// seedling it started as and stays inside its age band.
fn crop_age_sum(game: &Game, plots: &[(i32, i32, i32)]) -> i64 {
    let blocks = &game.registries().blocks;
    let mut total = 0_i64;
    for &(x, y, z) in plots {
        let Some(id) = game.world().get_block_loaded(x, y, z) else {
            panic!("plot ({x}, {y}, {z}) unloaded mid-run");
        };
        assert_eq!(
            blocks.block_name(id).unwrap_or(""),
            "minecraft:wheat",
            "every seedling is still a crop"
        );
        let age = blocks
            .properties_of(id)
            .unwrap_or_default()
            .iter()
            .find(|(k, _)| k == "age")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .expect("age reads");
        assert!(
            (0..=7).contains(&age),
            "wheat never exceeds its max age: {age}"
        );
        total += age;
    }
    total
}

/// Moisture missing from `cells` since placement: a cell still at moisture `m`
/// lost `1 - m`, and a cell that dried through to dirt lost its one point.
fn moisture_lost(game: &Game, cells: &[(i32, i32, i32)]) -> i64 {
    let blocks = &game.registries().blocks;
    let mut lost = 0_i64;
    for &(x, y, z) in cells {
        let Some(id) = game.world().get_block_loaded(x, y, z) else {
            panic!("dry cell ({x}, {y}, {z}) unloaded mid-run");
        };
        match blocks.block_name(id).unwrap_or("") {
            "minecraft:dirt" => lost += 1,
            "minecraft:farmland" => {
                let moisture = blocks
                    .properties_of(id)
                    .unwrap_or_default()
                    .iter()
                    .find(|(k, _)| k == "moisture")
                    .and_then(|(_, v)| v.parse::<i64>().ok())
                    .expect("moisture reads");
                lost += 1 - moisture;
            }
            other => panic!("dry cell ({x}, {y}, {z}) became {other}"),
        }
    }
    lost
}

/// The sweep draws `sections × 3` slots per loaded chunk in the radius, and
/// those samples reach the crop handler (the phase proof; mechanisms are
/// unit-pinned).
#[test]
fn wheat_grows_through_the_phase() {
    let mut harness = Harness::new("growth-wiring");
    install_growth_tags(&mut harness.game);
    for (cx, cz) in FARM_CHUNKS {
        assert!(
            harness.game.load_chunk(ChunkPos::new(cx, cz)),
            "farm chunk ({cx}, {cz}) loads"
        );
    }
    // Spacing-2 grid: every crop keeps its own 3×3 patch (neighbouring patches
    // share their odd columns; the crop rows stay 2 apart, so no cell halves
    // its speed for same-crop rows).
    let mut plots = Vec::new();
    for gx in 0..10 {
        for gz in 0..10 {
            let (x, z) = (gx * 2, gz * 2);
            plant(&mut harness.game, x, FARM_Y, z);
            plots.push((x, FARM_Y + 1, z));
        }
    }
    // A dry patch in the same sweep: farmland is a dispatch arm of its own, and
    // this keeps the deleted tick farm's *phase* claim ("dry soils lose moisture
    // over the run") while the handler's own steps are unit-pinned. Placed clear
    // of the wheat field (x -1..19, z -1..19), inside chunk (1, 1).
    let mut dry_cells = Vec::new();
    let dry = harness
        .game
        .registries()
        .blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "1".to_owned())],
        )
        .expect("dry farmland");
    for i in 0..60 {
        let (x, z) = (22 + i % 10, 24 + i / 10);
        harness
            .game
            .world_mut()
            .set_block(x, FARM_Y, z, dry)
            .expect("dry soil placed");
        dry_cells.push((x, FARM_Y, z));
    }
    harness.join("Farmer");
    harness.stand(8, 121, 2);
    settle_after_streaming(&mut harness);

    // The rate is read off the live loaded set, not hardcoded: the pin is
    // *3 slots per section per chunk*, while streaming owns which chunks those
    // are. The player stands in chunk (0, 0) and physics never moves x/z out
    // of it, so the centre is a constant, not a float cast.
    let centre = ChunkPos::new(0, 0);
    let loaded_in_radius = harness
        .game
        .world()
        .chunk_positions()
        .filter(|pos| (pos.x - centre.x).abs() <= RADIUS && (pos.z - centre.z).abs() <= RADIUS)
        .count();
    assert!(loaded_in_radius > 0, "streaming must have loaded chunks");
    let sections = harness.game.world().section_count();
    let report = harness.game.tick().expect("tick");
    assert_eq!(
        report.random_tick_samples,
        loaded_in_radius * sections * 3,
        "every loaded chunk in the radius contributes sections × 3 samples"
    );

    for _ in 0..FARM_TICKS {
        harness.game.tick().expect("tick");
    }
    let total = crop_age_sum(&harness.game, &plots);
    assert!(
        total >= 2,
        "100 moist wheat over {FARM_TICKS} ticks should gain ≈4.9 ages, got {total}"
    );
    // 60 dry cells × 206 ticks × 7.3e-4 ≈ 9 sampled cells, each losing its one
    // point of moisture (a second hit turns the cell to dirt, still 0).
    let lost = moisture_lost(&harness.game, &dry_cells);
    assert!(
        lost >= 3,
        "60 dry soils over {FARM_TICKS} ticks should lose ≈9 moisture, lost {lost}"
    );
}

/// The age/moisture bands the growth code assumes, pinned against the fixture.
#[test]
fn max_ages_match_the_fixture() {
    let dir = TempDir::new("growth-pins");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = mc_network::bridge::game_channel(64);
    let game =
        Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
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
