//! P16-04 Step B: skeleton bows and creeper fuses end to end.
//!
//! Through the real tick loop with summoned mobs: a skeleton in range with
//! line of sight looses an arrow that hurts the player; the same skeleton
//! behind a stone wall holds fire; a creeper next to the player lights its
//! fuse and detonates within ~30 ticks; a creeper thirty blocks away never
//! lights. The falsification shape is contact: removing the damage call (or
//! the fuse increment) leaves health at 20 and the creeper alive, failing
//! every assertion below.

#![allow(clippy::float_cmp)]

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
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

    /// Spawn `kind` at a block offset from world spawn, loading the chunk.
    fn summon(&mut self, kind: MobKind, dx: i32, dy: i32, dz: i32) -> mc_entity::EntityId {
        let (sx, sy, sz) = self.game.spawn();
        self.game
            .load_chunk(ChunkPos::new((sx + dx) >> 4, (sz + dz) >> 4));
        let position = mc_world::Vec3::new(
            f64::from(sx) + 0.5 + f64::from(dx),
            f64::from(sy) + f64::from(dy),
            f64::from(sz) + 0.5 + f64::from(dz),
        );
        self.game.spawn_mob(kind, position).expect("mob spawns")
    }

    fn put_stone(&mut self, x: i32, y: i32, z: i32) {
        self.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone is known");
        self.game
            .world_mut()
            .set_block(x, y, z, stone)
            .expect("wall block sets");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn health(&self) -> f32 {
        self.game.player(self.id).expect("player").health
    }

    fn arrows(&self) -> usize {
        self.game
            .entity_store()
            .iter()
            .filter(|entity| {
                matches!(
                    &entity.body,
                    mc_entity::EntityBody::Projectile(p)
                        if p.kind == mc_entity::ProjectileKind::Arrow
                )
            })
            .count()
    }

    fn creeper_alive(&self) -> bool {
        self.game.entity_store().iter().any(|entity| {
            matches!(&entity.body, mc_entity::EntityBody::Mob(mob) if mob.kind == MobKind::Creeper)
                && !entity.removed
        })
    }
}

#[test]
fn a_skeleton_in_range_shoots_and_the_arrow_lands() {
    let mut harness = Harness::new("p16-bow");
    let _ = harness.join();
    // Eight blocks east on the same level: inside the 15-block bow range,
    // outside melee, open sky between.
    harness.summon(MobKind::Skeleton, 8, 0, 0);
    harness.run(60);
    assert!(
        harness.health() < 20.0,
        "an arrow must have landed within 60 ticks, health={}",
        harness.health()
    );
}

#[test]
fn a_skeleton_behind_a_wall_holds_fire() {
    let mut harness = Harness::new("p16-bow-wall");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    harness.summon(MobKind::Skeleton, 8, 0, 0);
    // A three-high wall halfway between: the eye-to-torso ray must cross it.
    for dy in 0..=2 {
        harness.put_stone(sx + 4, sy + dy, sz);
    }
    harness.run(60);
    assert_eq!(
        harness.health(),
        20.0,
        "no line of sight means no arrows and no damage"
    );
    assert_eq!(
        harness.arrows(),
        0,
        "a blind skeleton holds fire rather than wasting arrows"
    );
}

#[test]
fn a_creeper_next_to_the_player_lights_and_detonates() {
    let mut harness = Harness::new("p16-fuse");
    let _ = harness.join();
    // Two blocks east: inside the 3-block ignite radius on the first tick.
    harness.summon(MobKind::Creeper, 2, 0, 0);
    harness.run(45);
    assert!(
        !harness.creeper_alive(),
        "the fuse burns 30 ticks: the creeper must be gone after 45"
    );
    assert!(
        harness.health() < 20.0,
        "point-blank detonation must hurt, health={}",
        harness.health()
    );
}

#[test]
fn a_creeper_thirty_blocks_away_never_lights() {
    let mut harness = Harness::new("p16-fuse-far");
    let _ = harness.join();
    // Thirty blocks east: past the 16-block follow range, so the creeper
    // wanders around its spawn and never comes within the 7-block fuse
    // radius (wander radius is 10, so 30 - 10 stays clear).
    let creeper = harness.summon(MobKind::Creeper, 30, 0, 0);
    harness.run(60);
    assert!(
        harness.creeper_alive(),
        "far outside every radius the fuse never lights"
    );
    let fuse = harness
        .game
        .entity_store()
        .get(creeper)
        .expect("creeper still around");
    let mc_entity::EntityBody::Mob(mob) = &fuse.body else {
        panic!("the creeper id still points at a mob");
    };
    assert_eq!(mob.ai.fuse, 0, "the fuse counter never moved");
    assert_eq!(harness.health(), 20.0, "no blast, no damage");
}
