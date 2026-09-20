//! AUDIT-11 N-1: the reach rule, against the jar's own arithmetic.
//!
//! ## What N-1 said, and what is actually true
//!
//! AUDIT-11 reported that "the dig path has **no reach check**: any client can break
//! any block anywhere", on the evidence that the owner's session dug
//! `BlockPos.ZERO` and the server broke deep-underground (0,0,0) twelve times.
//!
//! **Both halves are refuted**, and the refutation is not a reading — it is on disk:
//!
//! * no trace under `target/` contains a zero-position dig.
//!   `target/verify/trace_summary.py` counts them: the preserved owner session
//!   (`target/visual-check-p11-acceptance-2026-09-15/`) has **24 digs at 12 distinct
//!   real positions**, zero of them `BlockPos.ZERO`, each matched by a `block_update`
//!   at exactly that position; the later 41 MB session has **258 digs, zero at
//!   `BlockPos.ZERO`**, and 154 `block_changed_ack` packets — the M-2 fix working
//!   live;
//! * the check exists and has since Phase 04: `Game::within_reach` is called on the
//!   dig path at `apply_player_action` and on the place path, and `survival_e2e`
//!   has pinned a 40-block refusal since P04-09.
//!
//! What N-1 was *right* about is the class of gap, and underneath its evidence there
//! was a real one, in the opposite direction: the server used the bare attribute
//! (4.5) with no verification buffer, so it **refused digs vanilla accepts** between
//! 4.5 and 5.5 blocks from the eye. Separately, the entity/attack path really did
//! have no check at all — a recorded open divergence — so a swing could land from
//! any distance. Both are fixed here.
//!
//! ## The rule (bytecode, Mojang-mapped 26.1.2 server jar)
//!
//! ```text
//! Player.DEFAULT_BLOCK_INTERACTION_RANGE                  = 4.5f
//! ServerPlayer.CREATIVE_BLOCK_INTERACTION_RANGE_MODIFIER   = +0.5 (ADD_VALUE)
//! ServerPlayer.BLOCK_INTERACTION_DISTANCE_VERIFICATION_BUFFER = 1.0d
//! Player.isWithinBlockInteractionRange(pos, buffer):
//!     AABB(pos).distanceToSqr(getEyePosition()) < (blockInteractionRange() + buffer)^2
//! ServerPlayerGameMode.handleBlockBreakAction        -> buffer 1.0 (dconst_1)
//! ServerGamePacketListenerImpl.handleUseItemOn       -> buffer 1.0 (dconst_1)
//! Player.DEFAULT_ENTITY_INTERACTION_RANGE            = 3.0f
//! ServerPlayer.ENTITY_INTERACTION_DISTANCE_VERIFICATION_BUFFER = 3.0d
//! ServerGamePacketListenerImpl.handleInteract -> isWithinEntityInteractionRange(aabb, 3.0)
//! ```
//!
//! ## How these tests compute distances
//!
//! Through [`eye_distance_sq`], which calls the **same public primitive the server
//! calls**. The first version of this file did the arithmetic by hand, placed the
//! target at the player's feet level, and called the horizontal offset "the
//! distance" — but the eye sits 0.62 blocks *above* such a block's top face, so
//! every figure was wrong. Two of the tests were consequently not load-bearing at
//! all, which `target/m_probes.py` probes N-1c and N-1e reported as
//! `*** PASSED -- TEST NOT LOAD-BEARING ***`. The geometries are rebuilt so that a
//! distance in a test is a distance the server would compute, and the two
//! discriminating cases place the target at the eye's level (block at feet + 1) so
//! the vertical term is exactly zero.
//!
//! ## What these do not prove
//!
//! That a real client's aim and this server's idea of "the aimed block" agree — the
//! acceptance round is what would. That the *entity* buffer of 3.0 is the right one
//! for a held weapon in 26.1.2 (vanilla also has `isWithinAttackRange` over the
//! item's `AttackRange` component, which this build does not model); the check here
//! is the interaction-range gate vanilla applies before branching on the action.

#![allow(clippy::cast_possible_truncation)]

use mc_entity::mob::MobKind;
use mc_entity::player::GameMode;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::{Game, block_reach, entity_reach};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// The player's eye height, repeated here so the tests compute distances the way
/// the server does rather than importing its constant and testing it against
/// itself.
const EYE: f64 = 1.62;

/// The exact squared distance from a player's eye to a block's box, computed with
/// the same public primitive the server uses.
fn eye_distance_sq(feet: (f64, f64, f64), block: (i32, i32, i32)) -> f64 {
    mc_world::Aabb::block(block.0, block.1, block.2).distance_to_sqr(mc_world::Vec3::new(
        feet.0,
        feet.1 + EYE,
        feet.2,
    ))
}

/// Squared distance from the **feet** to a block's box, for the one test that has
/// to show the two readings disagree.
fn feet_distance_sq(feet: (f64, f64, f64), block: (i32, i32, i32)) -> f64 {
    mc_world::Aabb::block(block.0, block.1, block.2)
        .distance_to_sqr(mc_world::Vec3::new(feet.0, feet.1, feet.2))
}

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

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn set_game_mode(&mut self, mode: GameMode) {
        self.game.player_mut(self.id).expect("player").game_mode = mode;
    }

    /// Put the player's **feet** at `(x, y, z)` exactly, so the eye lands at
    /// `y + EYE` and the distance to a block is arithmetic rather than guesswork.
    fn place_player(&mut self, x: f64, y: f64, z: f64) {
        self.game.player_mut(self.id).expect("player").position = mc_world::Vec3::new(x, y, z);
    }

    /// Lay a course of stone at height `y` along `+x`, with the space above cleared,
    /// so a dig at any tested column is a real block at a known distance.
    fn lay_course(&mut self, from_x: i32, to_x: i32, y: i32, z: i32) {
        for x in from_x..=to_x {
            assert!(
                self.game.load_chunk(ChunkPos::new(x >> 4, z >> 4)),
                "the chunk at ({x}, {z}) loads"
            );
        }
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        let air = self.game.registries().blocks.air_id();
        for x in from_x..=to_x {
            self.game
                .world_mut()
                .set_block(x, y, z, stone)
                .expect("the target block is set");
            self.game
                .world_mut()
                .set_block(x, y + 1, z, air)
                .expect("the space above it is cleared");
        }
    }

    /// Dig `(x, y, z)` on status 0 (start) and report whether the block broke.
    fn dig_breaks(&mut self, x: i32, y: i32, z: i32) -> bool {
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        // Restore the block first, so each case is independent of the last.
        self.game
            .world_mut()
            .set_block(x, y, z, stone)
            .expect("the target is restored");
        self.intent(PlayIntent::PlayerAction {
            status: 0,
            position: block_position(x, y, z),
            facing: 1,
            sequence: 1,
        });
        self.game.world().get_block_loaded(x, y, z) != Some(stone)
    }

    /// Summon a mob and return its wire id.
    fn summon(&mut self, kind: MobKind, x: f64, y: f64, z: f64) -> i32 {
        let id = self
            .game
            .spawn_mob(kind, mc_world::Vec3::new(x, y, z))
            .expect("the mob spawns");
        id.get()
    }

    /// Swing at `entity` (interact kind 1) and report whether it took damage.
    fn swing_hurts(&mut self, entity: i32) -> bool {
        let target = mc_entity::EntityId::new(entity).expect("a valid entity id");
        let health_before = self.health(target).expect("the target exists");
        self.intent(PlayIntent::Interact { entity, kind: 1 });
        let health_after = self.health(target).expect("the target still exists");
        health_after < health_before
    }

    /// The entity store's own health, which is what `damage_entity` writes.
    fn health(&self, entity: mc_entity::EntityId) -> Option<f32> {
        self.game.entity_store().get(entity).map(|e| e.health)
    }
}

#[test]
fn the_reach_constants_are_the_jars() {
    assert!(
        (block_reach(false) - 5.5).abs() < 1.0e-9,
        "survival block reach = 4.5 attribute + 1.0 buffer"
    );
    assert!(
        (block_reach(true) - 6.0).abs() < 1.0e-9,
        "creative block reach = 5.0 + 1.0"
    );
    assert!(
        (entity_reach() - 6.0).abs() < 1.0e-9,
        "entity reach = 3.0 + 3.0"
    );
}

/// **The case that separates the fix from the old rule.**
///
/// A survival dig between 4.5 and 5.5 blocks from the eye: the jar accepts it, the
/// old bare-attribute check refused it. If someone reverts `within_reach` to the
/// attribute alone, this test fails.
#[test]
fn a_survival_dig_between_the_attribute_and_the_buffer_is_accepted() {
    let mut harness = Harness::new("n1-buffer");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    // The target sits one above the feet, so the eye is inside the block's y span
    // and the distance is purely horizontal — and therefore exact.
    let target_y = sy + 1;
    harness.lay_course(sx, sx + 8, target_y, sz);
    let feet = (f64::from(sx) + 0.9, f64::from(sy), f64::from(sz) + 0.5);
    harness.place_player(feet.0, feet.1, feet.2);

    let column = sx + 6;
    let d2 = eye_distance_sq(feet, (column, target_y, sz));
    assert!(
        d2 > 4.5 * 4.5 && d2 < block_reach(false).powi(2),
        "the case must sit between the old 4.5 and the jar's 5.5; d={:.4}",
        d2.sqrt()
    );
    assert!(
        harness.dig_breaks(column, target_y, sz),
        "a survival dig at d={:.4} must break: inside the jar's 5.5",
        d2.sqrt()
    );
}

/// The other side: beyond the buffered range the dig is refused, including the
/// hostile case of an aimed block on the far side of the map.
#[test]
fn a_dig_beyond_the_buffered_reach_is_refused() {
    let mut harness = Harness::new("n1-refuse");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    let target_y = sy + 1;
    harness.lay_course(sx, sx + 10, target_y, sz);
    let feet = (f64::from(sx) + 0.9, f64::from(sy), f64::from(sz) + 0.5);
    harness.place_player(feet.0, feet.1, feet.2);

    let column = sx + 7;
    let d2 = eye_distance_sq(feet, (column, target_y, sz));
    assert!(
        d2 > block_reach(false).powi(2),
        "the case must be out of reach; d={:.4}",
        d2.sqrt()
    );
    assert!(
        !harness.dig_breaks(column, target_y, sz),
        "a survival dig at d={:.4} must be refused",
        d2.sqrt()
    );

    harness.place_player(feet.0, feet.1, f64::from(sz) + 40.5);
    assert!(
        !harness.dig_breaks(column, target_y, sz),
        "a dig 40 blocks away in z must be refused"
    );
}

/// The creative modifier is applied, and is the only difference: a distance
/// between 5.5 and 6.0 is out of a survival player's reach and inside a creative
/// player's.
#[test]
fn creative_reach_is_half_a_block_longer_than_survival() {
    let mut harness = Harness::new("n1-creative");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    let target_y = sy + 1;
    harness.lay_course(sx, sx + 10, target_y, sz);
    let feet = (f64::from(sx) + 1.4, f64::from(sy), f64::from(sz) + 0.5);
    harness.place_player(feet.0, feet.1, feet.2);

    let column = sx + 7;
    let d2 = eye_distance_sq(feet, (column, target_y, sz));
    assert!(
        d2 > block_reach(false).powi(2) && d2 < block_reach(true).powi(2),
        "the case must separate survival from creative; d={:.4}",
        d2.sqrt()
    );

    harness.set_game_mode(GameMode::Survival);
    assert!(
        !harness.dig_breaks(column, target_y, sz),
        "survival must refuse d={:.4}",
        d2.sqrt()
    );
    harness.set_game_mode(GameMode::Creative);
    assert!(
        harness.dig_breaks(column, target_y, sz),
        "creative must accept d={:.4} (5.0 + 1.0)",
        d2.sqrt()
    );
}

/// Exactly at the buffered range is refused, because the jar compares with a strict
/// `<`.
///
/// This is the one place `<` and `<=` differ, and both server-side call sites would
/// otherwise look right. The distance is exactly 5.5 by construction, which is why
/// the target has to sit at the eye's level: at the feet's level the eye is 0.62
/// above the block's top face, the distance is 5.53, and the case is outside the
/// range under **either** comparison — which made the first version of this test
/// vacuous (probe N-1c).
#[test]
fn the_boundary_itself_is_refused() {
    let mut harness = Harness::new("n1-boundary");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    let target_y = sy + 1;
    harness.lay_course(sx, sx + 10, target_y, sz);
    let feet = (f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
    harness.place_player(feet.0, feet.1, feet.2);

    let column = sx + 6;
    let d2 = eye_distance_sq(feet, (column, target_y, sz));
    assert!(
        (d2 - block_reach(false).powi(2)).abs() < 1.0e-9,
        "the case must sit exactly on the boundary; d={:.6} want={}",
        d2.sqrt(),
        block_reach(false)
    );
    assert!(
        !harness.dig_breaks(column, target_y, sz),
        "vanilla compares with a strict <, so exactly {} blocks is refused",
        block_reach(false)
    );
}

/// The entity rule: a swing inside 6.0 from the eye lands, one beyond does not.
///
/// This is the half of N-1's *concern* that was real. `PlayIntent::Interact` never
/// checked reach at all, so a swing damaged a mob from any distance — a recorded
/// open divergence.
#[test]
fn a_swing_outside_the_entity_reach_is_refused() {
    let mut harness = Harness::new("n1-entity");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    harness.lay_course(sx, sx + 20, sy, sz);

    harness.place_player(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
    let near = harness.summon(
        MobKind::Zombie,
        f64::from(sx) + 2.5,
        f64::from(sy),
        f64::from(sz) + 0.5,
    );
    assert!(
        harness.swing_hurts(near),
        "a swing at a mob one block away must land"
    );

    let far = harness.summon(
        MobKind::Zombie,
        f64::from(sx) + 0.5,
        f64::from(sy),
        f64::from(sz) + 20.5,
    );
    assert!(
        20.0 > entity_reach(),
        "the case must be out of entity reach"
    );
    assert!(
        !harness.swing_hurts(far),
        "a swing 20 blocks away must be refused, not damage the mob"
    );
}

/// The eye offset is part of the rule, not a detail.
///
/// A block **two above the feet** is 0.38 away from the eye vertically and 2.0 from
/// the feet, so a check measured from the feet refuses what the jar accepts. The
/// geometry is chosen so the two readings disagree in that direction; the first
/// version used a high block, where **both** readings refuse and the test proved
/// nothing (probe N-1e).
#[test]
fn reach_is_measured_from_the_eye_not_the_feet() {
    let mut harness = Harness::new("n1-eye");
    let _ = harness.join();
    let (sx, sy, sz) = harness.game.spawn();
    let target_y = sy + 2;
    harness.lay_course(sx, sx + 10, target_y, sz);
    // dx = 5.3 is the value that makes the two readings disagree: vertically the
    // block is 0.38 from the eye but 2.00 from the feet, so
    //   from the eye:  5.3^2 + 0.38^2 = 28.23  -> 5.314, inside 5.5
    //   from the feet: 5.3^2 + 2.00^2 = 32.09  -> 5.665, outside
    // dx = 5.1 (the first attempt) leaves the feet reading at 5.478, inside the
    // range, and the test then proves nothing.
    let feet = (f64::from(sx) + 0.7, f64::from(sy), f64::from(sz) + 0.5);
    harness.place_player(feet.0, feet.1, feet.2);

    let column = sx + 6;
    let from_eye = eye_distance_sq(feet, (column, target_y, sz));
    let from_feet = feet_distance_sq(feet, (column, target_y, sz));
    assert!(
        from_eye < block_reach(false).powi(2),
        "the jar's eye-based check must accept it; d={:.4}",
        from_eye.sqrt()
    );
    assert!(
        from_feet > block_reach(false).powi(2),
        "and a feet-based check must refuse it, or this test proves nothing; d={:.4}",
        from_feet.sqrt()
    );
    assert!(
        harness.dig_breaks(column, target_y, sz),
        "reach is measured from the eye (feet + {EYE}), so this dig must break"
    );
}
