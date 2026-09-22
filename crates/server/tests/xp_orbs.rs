//! P16-02: experience orbs end to end — death scatter, pickup into levels,
//! merge, and the client's `SetExperience` announcement.
//!
//! The harness mirrors `player_attack.rs`: a live game, a joined player, and
//! direct swings. Orb assertions read the entity store (values are
//! server-side; the client only ever sees sizes).

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::play::PlayIntent;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// Everything a test needs: a live game and a channel to act as the client.
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

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    /// One attack swing at `entity`, standing two blocks west first.
    fn swing(&mut self, entity: i32) {
        if let Ok(target) = mc_entity::EntityId::new(entity)
            && let Some(at) = self.game.entity_store().get(target).map(|e| e.position)
            && let Some(player) = self.game.player_mut(self.id)
        {
            player.position = mc_world::Vec3::new(at.x - 2.0, at.y, at.z);
        }
        self.intent(PlayIntent::Interact { entity, kind: 1 });
    }

    fn summon_nearby(&mut self, kind: MobKind) -> mc_entity::EntityId {
        let player = self.game.player(self.id).expect("player").position;
        let at = mc_world::Vec3::new(player.x + 2.0, player.y, player.z);
        self.game.spawn_mob(kind, at).expect("the mob spawns")
    }

    /// (count, summed value) of the orbs on the ground.
    fn orbs(&self) -> (usize, i32) {
        let mut count = 0;
        let mut total = 0;
        for entity in self.game.entity_store().iter() {
            if let mc_entity::EntityBody::Orb(orb) = &entity.body {
                count += 1;
                total += orb.value;
            }
        }
        (count, total)
    }
}

#[test]
fn a_sword_kill_scatters_the_cows_three_xp() {
    // P16-02: a player kill scatters the kind's reward (cow: 3) as orbs.
    let mut harness = Harness::new("p16-scatter");
    harness.join("Butcher");
    let registries = mc_registry::Registries::vanilla().expect("registry");
    let sword = registries
        .items
        .id("minecraft:diamond_sword")
        .expect("sword");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(
            0,
            mc_entity::stack::ItemStack::new(sword, 1).expect("sword"),
        )
        .expect("sword equipped");
    let cow = harness.summon_nearby(MobKind::Cow);

    // Diamond sword deals 7: two swings (second past the hurt window).
    harness.swing(cow.get());
    harness.run(10);
    harness.swing(cow.get());

    assert_eq!(
        harness.orbs(),
        (1, 3),
        "one 3-point orb for the cow, split top-down as [3]"
    );
}

#[test]
fn an_explosion_kill_scatters_nothing() {
    // P16-02: vanilla awards XP for player kills only. A cow finished by a
    // creeper blast (attacker None) leaves no orbs.
    //
    // History: this was a fall-kill test, but the fall geometry cannot kill —
    // per-tick fall segments floor at the 3-block threshold (the P05 gap the
    // matrix records), so a 40-block drop lands a cow at full health and the
    // test passed with or without the attribution gate (verified by probe
    // during the P16/P17 acceptance: the cow stood at 10.0 HP after 120
    // ticks). The gate is proven by blast instead, through the real fuse.
    let mut harness = Harness::new("p16-blast-kill");
    harness.join("Witness");
    let (sx, sy, sz) = harness.game.spawn();
    // Stone floor under the rig so both mobs stand at sy (no falling, no
    // wandering surprises beyond the pens).
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    for x in (sx - 2)..=(sx + 5) {
        for z in (sz - 2)..=(sz + 2) {
            harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
            harness
                .game
                .world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
            harness
                .game
                .world_mut()
                .set_block(x, sy, z, air)
                .expect("clear");
            harness
                .game
                .world_mut()
                .set_block(x, sy + 1, z, air)
                .expect("clear head");
        }
    }
    let wall = |harness: &mut Harness, x: i32, y: i32, z: i32| {
        harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
        harness
            .game
            .world_mut()
            .set_block(x, y, z, stone)
            .expect("pen wall sets");
    };
    // Creeper cell (sx+2) penned on the player side and flanks; cow cell
    // (sx+3) penned on the far side and flanks. The shared edge stays open
    // (a wall there would block the blast LOS gate); the player-side wall
    // also shields the player, so only the cow can die.
    wall(&mut harness, sx + 1, sy, sz);
    wall(&mut harness, sx + 2, sy, sz - 1);
    wall(&mut harness, sx + 2, sy, sz + 1);
    wall(&mut harness, sx + 4, sy, sz);
    wall(&mut harness, sx + 3, sy, sz - 1);
    wall(&mut harness, sx + 3, sy, sz + 1);
    let at =
        |dx: i32| mc_world::Vec3::new(f64::from(sx + dx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
    let creeper = harness
        .game
        .spawn_mob(MobKind::Creeper, at(2))
        .expect("creeper spawns");
    let cow = harness
        .game
        .spawn_mob(MobKind::Cow, at(3))
        .expect("cow spawns");
    // Player within the 3-block ignite radius; 30-tick fuse plus margin.
    harness.run(45);
    assert!(
        harness
            .game
            .entity_store()
            .get(creeper)
            .is_none_or(|e| e.removed),
        "the creeper detonated within 45 ticks"
    );
    assert!(
        harness
            .game
            .entity_store()
            .get(cow)
            .is_none_or(|e| e.removed),
        "the blast next door killed the cow"
    );
    assert_eq!(harness.orbs(), (0, 0), "no swing, no orbs");
}

#[test]
fn an_orb_pickup_levels_and_announces() {
    // P16-02: 7 points at level 0 is exactly level 1, and the client hears
    // about it through SetExperience in the same tick.
    let mut harness = Harness::new("p16-pickup");
    let mut out = harness.join("Pupil");
    let at = harness.game.player(harness.id).expect("player").position;
    harness.game.spawn_orb(7, at).expect("orb spawns");
    harness.run(1);

    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(player.total_experience, 7);
    assert_eq!(player.level, 1);
    let mut ids = Vec::new();
    while let Some(raw) = out.try_recv() {
        ids.push(raw.id);
    }
    assert!(
        ids.contains(&clientbound::play::SET_EXPERIENCE),
        "pickup announces SetExperience, saw {ids:?}"
    );
    assert_eq!(harness.orbs(), (0, 0), "picked orb leaves the ground");
}

#[test]
fn one_orb_per_two_ticks_throttle() {
    // P16-02: vanilla's per-player pickup throttle — the second orb waits.
    let mut harness = Harness::new("p16-throttle");
    harness.join("Slow");
    let at = harness.game.player(harness.id).expect("player").position;
    // 0.8 apart (no merge) but each within the 1.0 pickup reach.
    harness
        .game
        .spawn_orb(1, mc_world::Vec3::new(at.x + 0.4, at.y, at.z))
        .expect("first");
    harness
        .game
        .spawn_orb(1, mc_world::Vec3::new(at.x - 0.4, at.y, at.z))
        .expect("second");
    harness.run(1);
    assert_eq!(
        harness.orbs().0,
        1,
        "one orb picked, one still grounded after one tick"
    );
    harness.run(2);
    assert_eq!(
        harness.orbs(),
        (0, 0),
        "throttle expires, second orb picked"
    );
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .total_experience,
        2
    );
}

#[test]
fn adjacent_orbs_merge_values_into_the_older() {
    // P16-02: same contact scale as drops; values sum, no cap.
    let mut harness = Harness::new("p16-merge");
    harness.join("Far");
    let far = mc_world::Vec3::new(200.0, 70.0, 200.0);
    harness.game.spawn_orb(3, far).expect("first");
    harness.game.spawn_orb(7, far).expect("second");
    harness.run(1);
    assert_eq!(harness.orbs(), (1, 10), "merged into one 10-point orb");
}

#[test]
fn a_player_death_scatters_capped_xp() {
    // P16-02: dying at level 3 (30 banked points) scatters min(7*3,100) = 21
    // as orbs; the bar still resets at respawn.
    let mut harness = Harness::new("p16-death");
    harness.join("Doomed");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .add_experience(30);
    assert_eq!(harness.game.player(harness.id).expect("player").level, 3);
    // Four climb-8/drop-8 cycles (the per-packet jump cap refuses more
    // than 8 per move): each landing deals 8-3 = 5, four landings kill from
    // full health. Falls measure per tick segment (tick_start_y resets each
    // tick), so multi-tick falls deal segment-wise rather than vanilla's
    // accumulated total — a P05 physics gap, not touched here.
    let at = harness.game.player(harness.id).expect("player").position;
    for _ in 0..4 {
        let mut y = harness.game.player(harness.id).expect("player").position.y;
        for _ in 0..8 {
            y += 1.0;
            harness.intent(PlayIntent::MovePlayerPos {
                x: at.x,
                y,
                z: at.z,
                on_ground: false,
            });
        }
        harness.intent(PlayIntent::MovePlayerPos {
            x: at.x,
            y: y - 8.0,
            z: at.z,
            on_ground: false,
        });
    }
    assert!(
        !harness.game.player(harness.id).expect("player").is_alive(),
        "the fall is lethal"
    );
    assert_eq!(harness.orbs().1, 21, "min(7 * level, 100) on the ground");
}

#[test]
fn an_orb_five_blocks_out_is_magnetised_into_pickup() {
    // Owner session: orbs that stopped outside the 1-block reach sat
    // forever — drag-asymptote micro-creep never crosses the boundary,
    // and the client reads it as orbs circling at the feet. Vanilla
    // homing (jar `ExperienceOrb.followNearbyPlayer`: nearest player
    // within 8 blocks, `(1 - dist/8)^2 * 0.1` toward the eye midpoint)
    // guarantees contact. Flat test ground (seeded hills would measure
    // terrain, not homing); the orb starts 5 blocks east with no
    // velocity.
    let mut harness = Harness::new("p17-orb-homing");
    harness.join("Magnet");
    let at = harness.game.player(harness.id).expect("player").position;
    flat_strip(&mut harness, &at);
    harness
        .game
        .spawn_orb(3, mc_world::Vec3::new(at.x + 5.0, at.y, at.z))
        .expect("orb spawns");
    harness.run(400);
    assert_eq!(harness.orbs(), (0, 0), "the magnetism rode the orb in");
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .total_experience,
        3,
        "and the points landed"
    );
}

#[test]
fn a_nearly_still_orb_snaps_to_rest() {
    // Drag multiplies without ever reaching zero, so a settled orb kept
    // a nonzero velocity and drifted (and re-announced) forever. Thirty
    // blocks out no homing interferes, so this pins the snap alone.
    let mut harness = Harness::new("p17-orb-rest");
    harness.join("Watcher");
    let at = harness.game.player(harness.id).expect("player").position;
    flat_strip(&mut harness, &at);
    let orb = harness
        .game
        .spawn_orb(3, mc_world::Vec3::new(at.x + 30.0, at.y, at.z))
        .expect("orb spawns");
    harness
        .game
        .entity_store_mut()
        .get_mut(orb)
        .expect("orb")
        .velocity = mc_world::Vec3::new(1e-5, 0.0, 1e-5);
    harness.run(5);
    assert_eq!(
        harness.game.entity_store().get(orb).expect("orb").velocity,
        mc_world::Vec3::ZERO,
        "micro-drift ends instead of creeping forever"
    );
}

/// A flat stone strip with cleared headroom under the player's feet, so
/// orb motion measures physics — not the seeded hills (inside a hill an
/// orb could not move at all; over a pit it would fall away, and either
/// failure would read as a homing/rest defect).
///
/// Values are floored before narrowing, so the truncation lint's
/// complaint does not apply; the crate root documents the same
/// exemption.
#[allow(clippy::cast_possible_truncation)]
fn flat_strip(harness: &mut Harness, at: &mc_world::Vec3) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    let (cx, cy, cz) = (
        at.x.floor() as i32,
        at.y.floor() as i32,
        at.z.floor() as i32,
    );
    for x in (cx - 32)..=(cx + 32) {
        for z in (cz - 2)..=(cz + 2) {
            harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
            harness
                .game
                .world_mut()
                .set_block(x, cy - 1, z, stone)
                .expect("floor");
            for y in [cy, cy + 1, cy + 2] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, air)
                    .expect("cleared");
            }
        }
    }
}
