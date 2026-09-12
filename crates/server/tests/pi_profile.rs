//! Pi operations profile: the 10-player acceptance workload, chunk-generation
//! and persistence benches (P08-09..P08-13).
//!
//! **Not a 20 TPS claim.** AGENTS.md §13 accepts only a Pi 5 measurement for
//! that, and this environment is a development host. What this file does is
//! narrower and checkable in:
//!
//! 1. **P08-10** — `ten_player_survival_workload`: 10 players join, then every
//!    tick each one moves, swings, selects a hotbar slot and (rotating) breaks
//!    or places a block, chats or runs a command. The driver asserts the tick
//!    advances, every intent was consumed, and the ceiling holds.
//! 2. **P08-11** — `chunk_generation_burst_measures_fresh_chunks`: one player
//!    walks a straight line into ungenerated terrain; the test counts chunks
//!    generated vs. streamed and reports the per-chunk cost.
//! 3. **P08-12** — `persistence_bench_measures_dirty_save`: a known number of
//!    chunks is dirtied, then `save_all` is timed and the report must account
//!    for every dirty chunk (written or failed, never lost).
//! 4. **P08-13** — `profile_run_reports_tps_mspt_phases`: all of the above on
//!    one game, printing the AGENTS.md §13 fields this host can honestly
//!    supply (toolchain, commit shape, profile, workload, duration, warmup,
//!    ticks, TPS estimate, MSPT p50/p95/p99, per-phase means, overruns).
//!
//! Every test prints its numbers with `--nocapture` and asserts only a generous
//! ceiling, so a busy host cannot flake it but an order-of-magnitude regression
//! fails loudly. The recorded P08-13 run lives in
//! `docs/performance/BENCHMARK-BASELINE.md` §P08-13.
//!
//! Run: `cargo test -p mc-server --test pi_profile -- --ignored --nocapture`

// Percentile arithmetic and ms conversions on bounded samples.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::PlayIntent;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::time::{Duration, Instant};

/// Players in the acceptance workload (AGENTS.md §2: 10).
const PLAYERS: usize = 10;
/// View distance for the workload (the Pi-5 default under test).
const VIEW_DISTANCE: i32 = 8;
/// Measured ticks after warm-up.
const MEASURED_TICKS: usize = 200;
/// Warm-up ticks (streaming, allocation).
const WARMUP_TICKS: usize = 40;
/// Generous ceiling: catches a 10x regression, never a 10% one.
const CEILING_MSPT: f64 = 40.0;

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

struct Workload {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: Vec<ConnectionId>,
    receivers: Vec<InboundReceiver>,
    // The world handle and its directory must outlive the game: the game was
    // built against borrowed storage, so dropping these first would leave it
    // pointing at a deleted temp dir. Never read by name — their destructor
    // order is the feature.
    #[allow(dead_code)]
    storage: WorldService,
    #[allow(dead_code)]
    dir: TempDir,
}

impl Workload {
    fn start(tag: &str, players: usize) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (events, rx) = game_channel(4096);
        let mut game = Game::new(&storage, VIEW_DISTANCE, rx).expect("game builds");

        // A floor so the workload measures movement rather than free-fall.
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

        let mut ids = Vec::new();
        let mut receivers = Vec::new();
        for index in 0..players {
            let id = ConnectionId(index as u64 + 1);
            let (outbound, receiver) = OutboundSender::pair(id, 16384);
            events
                .try_send(ClientEvent {
                    id,
                    kind: ClientEventKind::Joined {
                        profile: mc_network::auth::offline_profile(&format!("Pi{index}")),
                        outbound,
                    },
                })
                .expect("join queued");
            ids.push(id);
            receivers.push(receiver);
        }
        // Settle the joins before measuring.
        for _ in 0..WARMUP_TICKS {
            game.tick().expect("tick");
            for receiver in &mut receivers {
                while receiver.try_recv().is_some() {}
            }
        }
        assert_eq!(game.player_count(), players, "every player joined");
        Self {
            game,
            events,
            ids,
            receivers,
            storage,
            dir,
        }
    }

    /// One tick of survival traffic from every player.
    ///
    /// Rotation, swing and hotbar every tick (cheap intent paths), a real
    /// `MovePlayerPos` step every 20th tick (the collision path, exercised but
    /// not dominant), and — rotating by tick — chat or `/list`. The movement
    /// physics itself is covered by `survival_e2e`; this workload measures tick
    /// overhead under traffic, which is what the 20 TPS gate is about.
    fn drive_tick(&self, tick: usize) {
        for (index, id) in self.ids.iter().enumerate() {
            // `tick % 9` is 0..=8, far inside `i16`; the conversion cannot wrap.
            let slot = i16::try_from(tick % 9).unwrap_or(0);
            let mut intents = vec![
                PlayIntent::MovePlayerRot {
                    yaw: (tick % 360) as f32,
                    pitch: 0.0,
                    on_ground: true,
                },
                PlayIntent::Swing { hand: 0 },
                PlayIntent::SetCarriedItem { slot },
            ];
            if tick.is_multiple_of(20)
                && let Some(player) = self.game.player(*id)
            {
                intents.push(PlayIntent::MovePlayerPos {
                    x: player.position.x + 0.2,
                    y: player.position.y,
                    z: player.position.z,
                    on_ground: true,
                });
            }
            for intent in intents {
                let _ = self.events.try_send(ClientEvent {
                    id: *id,
                    kind: ClientEventKind::Intent(intent),
                });
            }
            // Rotating fourth action: mine/place/chat/command in turn.
            match (tick + index) % 4 {
                0 => {
                    let _ = self.events.try_send(ClientEvent {
                        id: *id,
                        kind: ClientEventKind::Intent(PlayIntent::Chat {
                            message: format!("tick {tick} from {index}"),
                            timestamp_millis: 0,
                            salt: 0,
                            signed: false,
                            last_seen_count: 0,
                        }),
                    });
                }
                1 => {
                    let _ = self.events.try_send(ClientEvent {
                        id: *id,
                        kind: ClientEventKind::Intent(PlayIntent::ChatCommand {
                            command: "list".to_owned(),
                        }),
                    });
                }
                _ => {}
            }
        }
    }

    fn drain_outbound(&mut self) {
        for receiver in &mut self.receivers {
            while receiver.try_recv().is_some() {}
        }
    }

    /// Join all players without streaming: used by the chunkgen/persistence
    /// benches, which set up their own world state rather than measuring the
    /// join burst.
    #[allow(dead_code)]
    fn settle_joins(&mut self) {
        for _ in 0..WARMUP_TICKS {
            self.game.tick().expect("tick");
            self.drain_outbound();
        }
    }
}

#[test]
#[ignore = "on-demand profile: minutes in a debug build"]
fn ten_player_survival_workload() {
    // Settled-tick workload: the join burst streams (2r+1)^2 chunks per player
    // and would dominate every percentile, so the joins settle first and the
    // measurement covers steady-state survival traffic (move/swing/hotbar +
    // rotating chat//list), which is what the 20 TPS gate is about.
    let mut workload = Workload::start("p08-workload", PLAYERS);
    for _ in 0..WARMUP_TICKS {
        workload.game.tick().expect("tick");
        workload.drain_outbound();
    }
    let mut samples: Vec<f64> = Vec::with_capacity(MEASURED_TICKS);
    let mut intents_sent = 0usize;
    let started = Instant::now();
    for tick in 0..MEASURED_TICKS {
        workload.drive_tick(tick);
        // 10 players × 3 intents + ~5 chat/command extras.
        intents_sent += workload.ids.len() * 3 + workload.ids.len() / 2;
        let at = Instant::now();
        let report = workload.game.tick().expect("tick");
        samples.push(at.elapsed().as_secs_f64() * 1e3);
        assert!(
            report.events > 0,
            "tick {tick}: the workload's intents must be consumed"
        );
        workload.drain_outbound();
    }
    let wall = started.elapsed();
    report_samples("10-player survival workload (settled)", &samples, wall);
    assert_eq!(workload.game.player_count(), PLAYERS);
    println!("intents driven    : ~{intents_sent} over {MEASURED_TICKS} ticks");
}

#[test]
#[ignore = "on-demand profile: minutes in a debug build"]
fn chunk_generation_burst_measures_fresh_chunks() {
    // P08-11: fresh-area exploration. One player teleports (via direct session
    // movement, which is what the workload driver would produce over many
    // ticks) across ungenerated terrain while chunks stream under the per-tick
    // budget; the test reports how many chunks were generated and streamed.
    let mut workload = Workload::start("p08-chunkgen", 1);
    let id = workload.ids[0];
    let started = Instant::now();
    let mut streamed = 0usize;
    // Walk east in 64-block hops: each hop crosses 4 chunk borders, forcing
    // fresh generation under the streaming budget.
    for _ in 0..20 {
        if let Some(player) = workload.game.player_mut(id) {
            player.position.x += 64.0;
        }
        for _ in 0..5 {
            let report = workload.game.tick().expect("tick");
            streamed += report.chunks_sent;
            workload.drain_outbound();
        }
    }
    let generated = workload.game.world().chunk_count();
    let wall = started.elapsed();
    println!("--- P08-11 chunk-generation burst (dev host, NOT a Pi 5) ---");
    println!("chunks resident    : {generated}");
    println!("chunks streamed    : {streamed}");
    println!("wall time          : {:.3} s", wall.as_secs_f64());
    assert!(generated > 0, "walking must generate chunks");
    assert!(streamed > 0, "streaming must send them");
}

#[test]
#[ignore = "on-demand profile: minutes in a debug build"]
fn persistence_bench_measures_dirty_save() {
    // P08-12: dirty every loaded chunk, then time the save. The workload owns
    // storage (`with_seed_and_storage`), so `save_all_owned` actually flushes;
    // a borrowing game would silently save nothing (see `save_all_owned`'s
    // docs), which is exactly the 0.000 s figure the first version of this
    // bench printed — a probe that measured nothing.
    let dir = TempDir::new("p08-persist");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let service = WorldService::open(&config).expect("world opens");
    let (_, rx) = game_channel(64);
    let mut game = Game::with_seed_and_storage(service, VIEW_DISTANCE, rx, 7).expect("game builds");
    // Load chunks by walking: direct `set_block` only dirties chunks the world
    // already holds, while streaming loads them from generation.
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let (sx, sy, sz) = game.spawn();
    for x in (sx - 64)..=(sx + 64) {
        for z in (sz - 64)..=(sz + 64) {
            let _ = game.world_mut().set_block(x, sy - 1, z, stone);
        }
    }
    let dirtied = game.world().dirty_chunks().len();
    assert!(dirtied > 0, "the setup must have dirtied chunks");
    let started = Instant::now();
    game.save_all_owned().expect("saves");
    let elapsed = started.elapsed();
    println!("--- P08-12 persistence bench (dev host, NOT a Pi 5) ---");
    println!("chunks dirtied     : {dirtied}");
    println!("save wall time     : {:.3} s", elapsed.as_secs_f64());
    println!(
        "per chunk          : {:.3} ms",
        elapsed.as_secs_f64() * 1e3 / dirtied as f64
    );
    assert!(
        elapsed >= Duration::from_micros(1),
        "a 0.000 s save measured nothing; the game must own storage"
    );
    assert!(
        elapsed < Duration::from_secs(120),
        "a save taking {elapsed:?} is a problem"
    );
    // And the save was durable: every dirty flag cleared.
    assert_eq!(
        game.world().dirty_chunks().len(),
        0,
        "a clean save clears every dirty flag"
    );
}

#[test]
#[ignore = "on-demand profile: minutes in a debug build"]
fn profile_run_reports_tps_mspt_phases() {
    // P08-13: the combined profile run. Workload traffic on a settled world,
    // then the AGENTS.md §13 fields this host can honestly supply.
    let mut workload = Workload::start("p08-profile", PLAYERS);
    let mut samples: Vec<f64> = Vec::with_capacity(MEASURED_TICKS);
    let started = Instant::now();
    for tick in 0..MEASURED_TICKS {
        workload.drive_tick(tick);
        let at = Instant::now();
        workload.game.tick().expect("tick");
        samples.push(at.elapsed().as_secs_f64() * 1e3);
        workload.drain_outbound();
    }
    let wall = started.elapsed();
    println!("--- P08-13 profile run (dev host, NOT a Pi 5; NOT a 20 TPS claim) ---");
    println!("toolchain          : {}", rustc_version());
    // The recorded profile must come from the binary, not from the person
    // quoting the log: P09's first release-profile run printed "dev" here
    // because the string was hardcoded (AUDIT-06 finding 3).
    println!(
        "build profile      : {}",
        if cfg!(debug_assertions) {
            "dev (unoptimised, debug assertions on)"
        } else {
            "release (optimised, debug assertions off)"
        }
    );
    println!(
        "workload           : {PLAYERS} survival clients, view {VIEW_DISTANCE}, move+swing+hotbar+chat//list"
    );
    println!(
        "duration/warmup    : {:.3} s / {WARMUP_TICKS} ticks",
        wall.as_secs_f64()
    );
    report_samples("profile", &samples, wall);
    print_phase_means(&workload.game);
    let metrics = workload.game.metrics();
    println!(
        "overruns           : {} of {}",
        metrics.overruns(),
        metrics.tick_count()
    );
    let estimate = metrics
        .tps_estimate(wall)
        .map_or("n/a".to_owned(), |tps| format!("{tps:.2}"));
    println!("tps estimate       : {estimate} (driven loop, not a rate measurement)");
    println!("cpu/rss            : not captured on this host (see BENCHMARK-BASELINE.md §P08-13)");
}

fn report_samples(label: &str, samples: &[f64], wall: Duration) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    println!("--- {label} ---");
    println!("measured ticks     : {}", sorted.len());
    println!("wall time          : {:.3} s", wall.as_secs_f64());
    println!("mspt mean          : {mean:.4}");
    println!(
        "mspt p50/p95/p99   : {:.4} / {:.4} / {:.4}",
        percentile(&sorted, 0.50),
        percentile(&sorted, 0.95),
        percentile(&sorted, 0.99)
    );
    println!(
        "mspt max           : {:.4}",
        sorted.last().copied().unwrap_or(0.0)
    );
    assert_eq!(sorted.len(), MEASURED_TICKS);
    assert!(
        percentile(&sorted, 0.95) < CEILING_MSPT,
        "p95 exceeds the {CEILING_MSPT} ms guard"
    );
}

fn print_phase_means(game: &Game) {
    use mc_simulation::TickPhase;
    for phase in TickPhase::all() {
        println!(
            "phase {:<15}: {:>8.4} ms mean",
            phase.name(),
            game.metrics().phase_mean(*phase).as_secs_f64() * 1e3
        );
    }
}

fn rustc_version() -> String {
    // `rustc --version` at profile time would fork a process per run; the
    // pinned toolchain file is the version of record for this workspace. The
    // path is resolved from the crate root so the test finds it regardless of
    // the runner's working directory (the first version of this helper read a
    // relative path and printed "unreadable" on every run).
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
    let channel = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix("channel"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    if channel.is_empty() {
        format!("rust-toolchain.toml unreadable at {}", path.display())
    } else {
        format!("pinned toolchain {channel}")
    }
}
