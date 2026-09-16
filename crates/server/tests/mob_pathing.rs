//! M-4: a mob's step is checked against the next cell before it is taken.
//!
//! ## What the owner saw
//!
//! "Pathing is direct steering (recorded): mobs walk into water and walls."
//! Direct steering was a named simplification of P11-02, and it is still direct
//! steering — there is no path, no A*, and a wall between a mob and its target
//! still stops the mob rather than routing it. What changed is that a step into a
//! cell the mob cannot occupy is now **refused** by the AI before the velocity is
//! written, instead of being taken and left to the collision pass.
//!
//! That distinction only has teeth for blocks the collision pass *allows*: water
//! and lava are in `mc_world::collision::NON_SOLID`, so a mob walking into them
//! was not a collision failure at all — it was a mob that had decided to swim.
//! That is the observable these tests are built on.
//!
//! ## Why the negative control is the important test
//!
//! `a_mob_with_a_clear_path_still_reaches_the_player` exists because the two
//! refusal tests are both satisfied by a lookahead that refuses **everything**.
//! A check that always says "blocked" would freeze every mob in the world and pass
//! both of them; the control is what makes them mean something. This is the
//! project's own rule — a test that cannot fail proves nothing — applied to the
//! test that was easiest to write vacuously.
//!
//! ## What these do not prove
//!
//! That a real client sees mobs avoid water (that is the acceptance round), that
//! mobs route *around* an obstacle, that they refuse ledges or falls (they do not
//! — see `MOB_LOOKAHEAD_BLOCKS`'s neighbours in `game.rs`), or that the lookahead
//! is what a vanilla mob does. Vanilla has real navigation; this is a one-cell
//! reflex.

#![allow(clippy::cast_possible_truncation)]

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// How far west of the player the mob starts, in blocks.
const START_OFFSET: i32 = 6;
/// The course is this many ticks long. At the measured zombie walk (~0.35
/// blocks/tick) six blocks is under twenty ticks; sixty leaves room for the
/// walk's decision boundaries without letting a passing mob drift arbitrarily.
const COURSE_TICKS: usize = 60;

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

    fn join(&mut self) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile("Tester"),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        out
    }

    /// Lay a stone course along `z = z0` and clear the two air levels above it, so
    /// the mob is walking on a floor rather than negotiating generated terrain.
    ///
    /// The floor is continuous under every cell of the course, including the ones
    /// that will hold a fluid: a fluid in a *hole* would stop the mob by gravity
    /// and prove nothing about the lookahead.
    fn lay_course(&mut self, from_x: i32, to_x: i32, y: i32, z0: i32) {
        for x in from_x..=to_x {
            assert!(
                self.game.load_chunk(ChunkPos::new(x >> 4, z0 >> 4)),
                "the chunk at ({x}, {z0}) loads"
            );
        }
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone is a known block");
        let air = self.game.registries().blocks.air_id();
        for x in from_x..=to_x {
            for z in (z0 - 1)..=(z0 + 1) {
                self.game
                    .world_mut()
                    .set_block(x, y - 1, z, stone)
                    .expect("the floor is set");
                for dy in 0..=2 {
                    self.game
                        .world_mut()
                        .set_block(x, y + dy, z, air)
                        .expect("the course is cleared");
                }
            }
        }
    }

    /// Put one `block` at `(x, y, z)`.
    fn put(&mut self, x: i32, y: i32, z: i32, block: &str) {
        let id = self
            .game
            .registries()
            .blocks
            .default_state(block)
            .unwrap_or_else(|_| panic!("{block} is a known block"));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("the block is set");
    }

    fn place_player(&mut self, x: f64, y: f64, z: f64) {
        self.game
            .player_mut(self.id)
            .expect("a joined player")
            .position = mc_entity::player::Vec3::new(x, y, z);
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    /// The mob's feet cell, or `None` when no mob of that kind exists.
    fn mob_cell(&self, kind: MobKind) -> Option<(i32, i32, i32)> {
        self.game
            .mobs()
            .into_iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, p)| (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32))
    }

    fn mob_x(&self, kind: MobKind) -> Option<f64> {
        self.game
            .mobs()
            .into_iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, p)| p.x)
    }

    /// Every feet cell the mob occupied this tick sequence, as `(x, z)` pairs.
    fn mob_track(&mut self, kind: MobKind, ticks: usize) -> Vec<(i32, i32)> {
        let mut track = Vec::with_capacity(ticks);
        for _ in 0..ticks {
            self.game.tick().expect("tick");
            let (x, _y, z) = self.mob_cell(kind).expect("the mob is alive");
            track.push((x, z));
        }
        track
    }
}

/// Summon a zombie `START_OFFSET` blocks west of a player at `(px, py, pz)`,
/// both on the course. Returns the player's block x, for the callers that assert
/// on the arrival distance.
fn stage_chase(harness: &mut Harness, px: i32, py: i32, pz: i32) -> i32 {
    harness.join();
    harness.lay_course(px - START_OFFSET - 2, px + 2, py, pz);
    harness.place_player(f64::from(px) + 0.5, f64::from(py), f64::from(pz) + 0.5);
    harness
        .game
        .spawn_mob(
            MobKind::Zombie,
            mc_entity::player::Vec3::new(
                f64::from(px - START_OFFSET) + 0.5,
                f64::from(py),
                f64::from(pz) + 0.5,
            ),
        )
        .expect("the zombie spawns");
    px
}

/// **The negative control.** With nothing in the way the zombie must reach the
/// player — otherwise a lookahead that refuses every step would pass the two
/// tests below.
#[test]
fn a_mob_with_a_clear_path_still_reaches_the_player() {
    let mut harness = Harness::new("m4-clear");
    let (sx, sy, sz) = harness.game.spawn();
    let px = stage_chase(&mut harness, sx, sy, sz);

    let start_x = harness.mob_x(MobKind::Zombie).expect("the zombie exists");
    harness.run(COURSE_TICKS);
    let end_x = harness.mob_x(MobKind::Zombie).expect("the zombie exists");

    // It moved in the player's direction, and it got close.
    assert!(
        end_x > start_x + 2.0,
        "a zombie with a clear course must walk east towards the player: \
         started at x={start_x:.3}, ended at x={end_x:.3}"
    );
    let player_x = f64::from(px) + 0.5;
    assert!(
        (player_x - end_x).abs() <= mc_entity::mob::ATTACK_RANGE + 0.75,
        "and must arrive within melee range of it: player at x={player_x:.3}, \
         zombie at x={end_x:.3}"
    );
}

/// A zombie chasing a player across a one-cell water gap must stop at the edge.
///
/// Water is **non-solid**, so this is not collision: without the lookahead the
/// zombie walks straight in and stands in it — which is exactly what the owner
/// saw. The test therefore fails by *entering the water cell*, and the assertion
/// is on the cell rather than a distance so it cannot be satisfied by a mob that
/// merely slowed down.
#[test]
fn a_chasing_mob_stops_at_the_edge_of_water() {
    let mut harness = Harness::new("m4-water");
    let (sx, sy, sz) = harness.game.spawn();
    let _ = stage_chase(&mut harness, sx, sy, sz);

    // The water sits between the two, and the mob starts west of it.
    let water_x = sx - 2;
    harness.put(water_x, sy, sz, "minecraft:water");
    assert!(
        harness
            .game
            .world()
            .get_block_loaded(water_x, sy, sz)
            .is_some_and(|id| mc_world::collision::is_liquid(
                &harness.game.registries().blocks,
                id
            )),
        "the gap really holds a fluid, or this test proves nothing"
    );
    let start_x = harness.mob_x(MobKind::Zombie).expect("the zombie exists");
    assert!(
        f64::from(water_x) > start_x,
        "the zombie must start west of the water"
    );

    let track = harness.mob_track(MobKind::Zombie, COURSE_TICKS);

    // It walked up to the water: a mob that never moved would satisfy "did not
    // enter the water" for the wrong reason.
    let furthest = track.iter().map(|(x, _z)| *x).max().expect("a track");
    assert!(
        furthest >= water_x - 1,
        "the zombie must have walked up to the water's edge, not stopped short: \
         reached x={furthest}, water at x={water_x}"
    );
    // And it never occupied the fluid cell.
    let entered: Vec<(i32, i32)> = track
        .iter()
        .copied()
        .filter(|(x, _z)| *x == water_x)
        .collect();
    assert!(
        entered.is_empty(),
        "the zombie entered the water cell {entered:?}; track={track:?}"
    );
    // The stop is a stop: it does not creep forward over the remaining ticks.
    let last_ten: Vec<i32> = track[track.len() - 10..].iter().map(|(x, _z)| *x).collect();
    assert!(
        last_ten.iter().all(|x| *x == last_ten[0]),
        "a blocked mob halts rather than grinding forward: {last_ten:?}"
    );
}

/// The same for lava, because the predicate is a list and a list can be half-wired.
#[test]
fn a_chasing_mob_stops_at_the_edge_of_lava() {
    let mut harness = Harness::new("m4-lava");
    let (sx, sy, sz) = harness.game.spawn();
    let _ = stage_chase(&mut harness, sx, sy, sz);

    let lava_x = sx - 2;
    harness.put(lava_x, sy, sz, "minecraft:lava");

    let track = harness.mob_track(MobKind::Zombie, COURSE_TICKS);
    let furthest = track.iter().map(|(x, _z)| *x).max().expect("a track");
    assert!(
        furthest >= lava_x - 1,
        "the zombie must reach the lava's edge: reached x={furthest}, lava at x={lava_x}"
    );
    assert!(
        track.iter().all(|(x, _z)| *x != lava_x),
        "the zombie walked into lava; track={track:?}"
    );
}

/// The lookahead must not be confused with the thing it replaced: a **solid** wall
/// between the two still stops the mob, and now the mob also does not keep
/// pressing into it. The wall half is weaker evidence than the fluid half — the
/// collision pass already refused the cell — so this test asserts the part that is
/// new: the mob stays put rather than sliding along the face.
#[test]
fn a_wall_stops_a_chasing_mob_without_it_sliding_along() {
    let mut harness = Harness::new("m4-wall");
    let (sx, sy, sz) = harness.game.spawn();
    let _ = stage_chase(&mut harness, sx, sy, sz);

    let wall_x = sx - 2;
    for dy in 0..=1 {
        harness.put(wall_x, sy + dy, sz, "minecraft:stone");
    }

    let track = harness.mob_track(MobKind::Zombie, COURSE_TICKS);
    assert!(
        track.iter().all(|(x, _z)| *x != wall_x),
        "the zombie must never occupy the wall's cell; track={track:?}"
    );
    let last_ten: Vec<(i32, i32)> = track[track.len() - 10..].to_vec();
    assert!(
        last_ten.windows(2).all(|w| w[0] == w[1]),
        "a blocked mob halts where it is rather than sliding along the wall: {last_ten:?}"
    );
}
