//! Tick-rate baseline for the Phase 04 slice (P04-18).
//!
//! **This is not a performance claim.** AGENTS.md section 13 requires a Pi 5,
//! 10-player workload before any 20 TPS statement, and that harness is P08-09/13.
//! What this measures is narrower and honest: how much wall time one tick of the
//! Phase 04 simulation costs on the development host with a fixed number of
//! players and a bounded view distance, so a regression is visible immediately and
//! so the Phase 04 report can quote a number with its conditions attached.
//!
//! The test asserts only a *generous* ceiling. Its purpose is to catch an
//! accidental order-of-magnitude regression (an O(n²) scan, a save inside the tick
//! loop), not to certify throughput.
//!
//! Run with output:
//! `cargo test -p mc-server --test tick_baseline -- --ignored --nocapture`
//!
//! It is `#[ignore]`d because a full run costs several minutes in a debug build
//! (the save alone serialises every dirty chunk), which is too slow for the
//! default suite.
//!
//! ## Recorded runs (development host: `x86_64` Windows, rustc 1.98.1, debug
//! profile, 10 players, view distance 8)
//!
//! These are **three separate runs of the same test**, quoted together because one
//! alone reads as more precise than it is: the tick is under a millisecond of real
//! work, so a busy desktop moves p95 more than a code change does. Treat them as
//! one order of magnitude, not as a number to regress against.
//!
//! ```text
//! run                                       mean    p50     p95     p99     max     full save
//! 2026-09-11, Phase 04 monolith              0.89    0.78    1.42    1.65    2.22    39.3 s
//! re-measured (audit, pre-P05 restructure)   --      0.75    1.33    2.04    3.93    40.8 s
//! 2026-09-11, after the P05 phase split      0.94    0.87    1.26    2.63    4.29    40.8 s
//! ```
//!
//! What those numbers cover: a **settled** 10-player server. The view is already
//! streamed when the measured ticks begin (`chunks streamed: 0` in the last run),
//! so a join burst, the world-generation pipeline (P07) and the disk chunk reads
//! the Broadcast phase now performs for a fresh view are *not* in the figure. What
//! they do not cover at all is the Pi 5, which is the only hardware AGENTS.md
//! section 13 accepts for a 20 TPS statement.
//!
//! Two things follow, both recorded rather than hidden: (a) the tick itself has
//! ~50x headroom against the 50 ms budget even unoptimised, and (b) the **save**
//! dominates everything at ~108 ms per chunk in debug, which is exactly what
//! P08-12/P08-14 must profile on the Pi.
//!
//! ## P05-18: the entity-heavy scenario
//!
//! `entity_heavy_ticks_within_the_frame_budget` is the workload this phase's exit
//! gate asks about: the same 10 players plus a populated entity store. It is
//! **still not a 20 TPS claim** (wrong hardware, debug build, no world generation),
//! and it is deliberately constructed so the reader can see exactly what it does
//! and does not cover:
//!
//! - mobs are inserted directly through `EntityStore::spawn`, because **nothing in
//!   the server spawns them yet** (P05-11 has no spawn rule). The scenario
//!   therefore measures entity bookkeeping and per-tick iteration, not a spawn
//!   cycle that does not exist;
//! - `Game::tick_entity_ai` is a documented no-op, so the mob AI cost measured here
//!   is the *store walk*, not goal decisions or pathfinding;
//! - items and mobs are never sent to clients (P05-15), so a real entity-heavy
//!   server would also pay packet encoding this figure excludes.
//!
//! Those three clauses are the difference between "the data structures hold up" and
//! "an entity-heavy server runs at 20 TPS", and only the first is claimed.

// Percentile arithmetic converts between lengths and floats; the sample count is
// bounded by MEASURED_TICKS, far inside f64's precision.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, OutboundSender, game_channel,
};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::time::{Duration, Instant};

/// Players in the measured workload.
const PLAYERS: usize = 10;

/// View distance used for the measurement (chunks).
const VIEW_DISTANCE: i32 = 8;

/// Ticks measured after warm-up.
const MEASURED_TICKS: usize = 400;

/// Ticks discarded before measuring (streaming, allocation warm-up).
const WARMUP_TICKS: usize = 60;

/// A single tick must stay well inside the 50 ms budget on any sane host. The
/// ceiling is deliberately loose so the test does not flake on a busy CI box; it
/// catches a 10x regression, not a 10% one.
const CEILING_MSPT: f64 = 40.0;

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Print the windowed per-phase means (P08-13), replacing the old lifetime
/// `busiest_phase` line: a lifetime mean reflects the join burst, while the
/// phase means show what a settled tick costs.
fn print_phase_means(game: &Game) {
    use mc_simulation::TickPhase;
    let metrics = game.metrics();
    for phase in TickPhase::all() {
        println!(
            "phase {:<15}: {:>8.4} ms mean",
            phase.name(),
            metrics.phase_mean(*phase).as_secs_f64() * 1e3
        );
    }
    println!(
        "overruns         : {} of {} ticks",
        metrics.overruns(),
        metrics.tick_count()
    );
}

#[test]
#[ignore = "on-demand measurement: a full run costs minutes in a debug build"]
fn ten_players_tick_within_the_frame_budget() {
    let dir = TempDir::new("p04-tick-baseline");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let mut storage = WorldService::open(&config).expect("world opens");
    let (tx, rx) = game_channel(4096);
    let mut game = Game::new(&storage, VIEW_DISTANCE, rx).expect("game builds");

    // A floor so players have ground (world generation is P07; without this every
    // player would free-fall, which is not the workload we want to measure).
    let (sx, sy, sz) = game.spawn();
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let radius = (VIEW_DISTANCE + 1) * 16;
    for x in (sx - radius)..=(sx + radius) {
        for z in (sz - radius)..=(sz + radius) {
            game.world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
        }
    }

    // Join `PLAYERS` clients, each with its own outbound queue that a test thread
    // drains, so a full queue never distorts the measurement.
    let mut receivers = Vec::new();
    for index in 0..PLAYERS {
        let (outbound, receiver) = OutboundSender::pair(ConnectionId(index as u64 + 1), 16384);
        tx.try_send(ClientEvent {
            id: ConnectionId(index as u64 + 1),
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile(&format!("Load{index}")),
                outbound,
            },
        })
        .expect("join queued");
        receivers.push(receiver);
    }

    let mut samples: Vec<f64> = Vec::with_capacity(MEASURED_TICKS);
    let mut chunks_streamed = 0usize;
    let started = Instant::now();
    for tick in 0..(WARMUP_TICKS + MEASURED_TICKS) {
        let at = Instant::now();
        let report = game.tick().expect("tick");
        let elapsed = at.elapsed();
        if tick >= WARMUP_TICKS {
            samples.push(elapsed.as_secs_f64() * 1000.0);
            chunks_streamed += report.chunks_sent;
        }
        // Drain like a socket would, so backpressure does not build up.
        for receiver in &mut receivers {
            while receiver.try_recv().is_some() {}
        }
    }
    let wall = started.elapsed();
    assert_eq!(game.player_count(), PLAYERS);

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let p99 = percentile(&samples, 0.99);
    let max = samples.last().copied().unwrap_or(0.0);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;

    println!("--- Phase 04 tick baseline (development host, not a Pi 5) ---");
    println!("players          : {PLAYERS}");
    println!("view distance    : {VIEW_DISTANCE} chunks");
    println!("measured ticks   : {MEASURED_TICKS} (after {WARMUP_TICKS} warm-up)");
    println!("wall time        : {:.3} s", wall.as_secs_f64());
    println!(
        "tps estimate     : {:.2}",
        MEASURED_TICKS as f64 / wall.as_secs_f64()
    );
    println!("mspt mean        : {mean:.4}");
    println!("mspt p50/p95/p99 : {p50:.4} / {p95:.4} / {p99:.4}");
    println!("mspt max         : {max:.4}");
    println!("chunks streamed  : {chunks_streamed} over {MEASURED_TICKS} ticks");
    print_phase_means(&game);
    println!("NOTE: this is a regression guard, not a 20 TPS claim. AGENTS.md");
    println!("      section 13 requires the Pi 5 harness (P08-09/P08-13) for that.");

    assert!(
        samples.len() == MEASURED_TICKS,
        "expected {MEASURED_TICKS} samples, got {}",
        samples.len()
    );
    assert!(
        p95 < CEILING_MSPT,
        "p95 tick time {p95:.3} ms exceeds the {CEILING_MSPT} ms guard"
    );
    assert!(
        max < CEILING_MSPT * 2.0,
        "worst tick {max:.3} ms suggests an unbounded stall"
    );

    // Saving is part of the lifecycle and must not be absurdly slow either.
    let save_started = Instant::now();
    game.save_all(&mut storage).expect("saves");
    let save_elapsed = save_started.elapsed();
    println!("full save        : {:.3} s", save_elapsed.as_secs_f64());
    assert!(
        save_elapsed < Duration::from_secs(60),
        "a save taking {save_elapsed:?} is a problem"
    );
}

/// Entities seeded into the entity-heavy scenario: mobs + items.
///
/// 1 000 is chosen to be ~10x a plausible Phase 05 population (10 players, a few
/// hundred mobs) while staying inside `MAX_ENTITIES`, so the figure has headroom
/// rather than sitting at the cap.
const HEAVY_MOBS: usize = 600;
const HEAVY_ITEMS: usize = 400;

#[test]
#[ignore = "on-demand measurement: a full run costs minutes in a debug build"]
// One linear scenario (build → populate → measure → save): splitting it would hide
// which stage a regression came from.
#[allow(clippy::too_many_lines)]
fn entity_heavy_ticks_within_the_frame_budget() {
    use mc_entity::entity::{EntityBody, EntityKind};
    use mc_entity::item_entity::ItemEntity;
    use mc_entity::mob::{Mob, MobKind};
    use mc_entity::player::Vec3 as EntityVec3;
    use mc_entity::stack::ItemStack;

    let dir = TempDir::new("p05-entity-baseline");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (tx, rx) = game_channel(4096);
    let mut game = Game::new(&storage, VIEW_DISTANCE, rx).expect("game builds");

    let (sx, sy, sz) = game.spawn();
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let radius = (VIEW_DISTANCE + 1) * 16;
    for x in (sx - radius)..=(sx + radius) {
        for z in (sz - radius)..=(sz + radius) {
            game.world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
        }
    }

    // Populate the entity store. Mobs are spread over the floor; items are dropped
    // slightly above it so their physics has something to do every tick.
    let mob_kinds = MobKind::ALL;
    for index in 0..HEAVY_MOBS {
        let kind = mob_kinds[index % mob_kinds.len()];
        let dx = (index % 40) as f64 - 20.0;
        let dz = (index / 40) as f64 - 7.0;
        game.entity_store_mut()
            .spawn(
                EntityBody::Mob(Mob::new(kind)),
                EntityVec3::new(f64::from(sx) + dx, f64::from(sy), f64::from(sz) + dz),
            )
            .expect("mob spawns");
    }
    let dirt = game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
    for index in 0..HEAVY_ITEMS {
        let dx = (index % 20) as f64 - 10.0;
        let dz = (index / 20) as f64 - 10.0;
        // `% 64` keeps the count inside a legal stack, and `try_from` keeps the
        // narrowing explicit rather than a wrapping cast.
        let count = i32::try_from(index % 64).unwrap_or(0) + 1;
        let stack = ItemStack::new(dirt, count).expect("stack");
        game.entity_store_mut()
            .spawn(
                EntityBody::Item(ItemEntity::new(stack, None)),
                EntityVec3::new(f64::from(sx) + dx, f64::from(sy) + 2.0, f64::from(sz) + dz),
            )
            .expect("item spawns");
    }
    assert_eq!(
        game.entity_store().of_kind(EntityKind::Mob).len(),
        HEAVY_MOBS
    );
    assert_eq!(
        game.entity_store().of_kind(EntityKind::Item).len(),
        HEAVY_ITEMS
    );

    // Join the same 10 players as the other scenario.
    let mut receivers = Vec::new();
    for index in 0..PLAYERS {
        let (outbound, receiver) = OutboundSender::pair(ConnectionId(index as u64 + 1), 16384);
        tx.try_send(ClientEvent {
            id: ConnectionId(index as u64 + 1),
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile(&format!("Heavy{index}")),
                outbound,
            },
        })
        .expect("join queued");
        receivers.push(receiver);
    }

    let mut samples: Vec<f64> = Vec::with_capacity(MEASURED_TICKS);
    let mut entities_ticked = 0usize;
    let started = Instant::now();
    for tick in 0..(WARMUP_TICKS + MEASURED_TICKS) {
        let at = Instant::now();
        let report = game.tick().expect("tick");
        let elapsed = at.elapsed();
        if tick >= WARMUP_TICKS {
            samples.push(elapsed.as_secs_f64() * 1000.0);
            entities_ticked += report.entities_ticked;
        }
        for receiver in &mut receivers {
            while receiver.try_recv().is_some() {}
        }
    }
    let wall = started.elapsed();

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let p99 = percentile(&samples, 0.99);
    let max = samples.last().copied().unwrap_or(0.0);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;

    println!("--- Phase 05 entity-heavy baseline (development host, NOT a Pi 5) ---");
    println!("players          : {PLAYERS}");
    println!("mobs / items     : {HEAVY_MOBS} / {HEAVY_ITEMS}");
    println!("view distance    : {VIEW_DISTANCE} chunks");
    println!("measured ticks   : {MEASURED_TICKS} (after {WARMUP_TICKS} warm-up)");
    println!("wall time        : {:.3} s", wall.as_secs_f64());
    println!(
        "tps estimate     : {:.2}",
        MEASURED_TICKS as f64 / wall.as_secs_f64()
    );
    println!("mspt mean        : {mean:.4}");
    println!("mspt p50/p95/p99 : {p50:.4} / {p95:.4} / {p99:.4}");
    println!("mspt max         : {max:.4}");
    print_phase_means(&game);
    println!(
        "entity-ticks     : {entities_ticked} over {MEASURED_TICKS} ticks ({:.1}/tick)",
        entities_ticked as f64 / MEASURED_TICKS as f64
    );
    println!("NOTE: mobs are seeded directly and the AI hook is a documented no-op,");
    println!("      and entities are not yet sent to clients (P05-15). This measures");
    println!("      entity bookkeeping and physics, NOT a spawning world at 20 TPS.");

    assert_eq!(samples.len(), MEASURED_TICKS);
    assert!(
        entities_ticked > 0,
        "the entity phase must actually tick entities"
    );
    assert!(
        p95 < CEILING_MSPT,
        "entity-heavy p95 {p95:.3} ms exceeds the {CEILING_MSPT} ms guard"
    );
    assert!(
        max < CEILING_MSPT * 2.0,
        "worst entity-heavy tick {max:.3} ms suggests an unbounded stall"
    );

    game.save_all_owned().expect("saves");
}
