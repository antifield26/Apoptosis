//! P11-06: a serverbound `interact` attack damages what it names, once per hurt
//! window.
//!
//! The tests drive the decoded intent through the real channel, so what is
//! exercised is the whole path: `PlayIntent::Interact` -> `apply_intent` ->
//! `damage_entity` -> the entity store the packets are later built from. The
//! entity id used is the one `spawn_mob` returned, which is the same number
//! `AddEntity` puts on the wire (`id.get()`, no base offset), so this is also a
//! test of the id contract a client depends on.
//!
//! The rules pinned here, all of which are jar-derived:
//!
//! - a fist does **1.0** damage ([`mc_server::game`]'s `FIST_ATTACK_DAMAGE`);
//! - a hit lands at most once per **10-tick** window
//!   (`INVULNERABLE_TICKS`), which is vanilla's `invulnerableTime`;
//! - a **non-living** entity is immune rather than silently damaged.
//!
//! ## What these do not prove
//!
//! That a held item's damage is used (it is not: the fist figure is applied
//! whatever is held — a named gap), that a swing outside the interaction range
//! is refused (it is not — a named gap, see the parity matrix), and that a real
//! client's knockback or animation matches (P11-10).

// Health is compared exactly on purpose: every value here is reached by adding or
// subtracting 1.0 from a whole number, so each is exactly representable in `f32`
// and an approximate comparison would hide the off-by-one this suite exists to
// catch. `ai_wiring.rs` allows the same lint for the same reason.
#![allow(clippy::float_cmp)]

use mc_entity::MobKind;
use mc_entity::stack::ItemStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::PlayIntent;
use mc_server::game::{Game, INVULNERABLE_TICKS};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

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
        // Owned storage: a borrowing game never generates terrain, so the mob
        // would be standing in an all-air placeholder (see natural_spawn.rs).
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

    /// One attack swing at `entity`, the way the client sends it (`kind` 1).
    fn swing(&mut self, entity: i32) {
        self.intent(PlayIntent::Interact { entity, kind: 1 });
    }

    /// Spawn a mob two blocks from the player's feet, so a swing is a swing in
    /// any reading of the interaction range.
    fn summon_nearby(&mut self, kind: MobKind) -> mc_entity::EntityId {
        let player = self.game.player(self.id).expect("player").position;
        let at = mc_entity::player::Vec3::new(player.x + 2.0, player.y, player.z);
        self.game.spawn_mob(kind, at).expect("the mob spawns")
    }

    fn health(&self, entity: mc_entity::EntityId) -> Option<f32> {
        self.game.entity_store().get(entity).map(|e| e.health)
    }
}

#[test]
fn a_swing_takes_the_fist_damage_off_a_mob() {
    let mut harness = Harness::new("p11-attack");
    harness.join("Brawler");
    let chicken = harness.summon_nearby(MobKind::Chicken);

    let before = harness.health(chicken).expect("the chicken is alive");
    assert_eq!(before, 4.0, "a chicken spawns with its modelled health");
    harness.swing(chicken.get());

    let after = harness
        .health(chicken)
        .expect("the chicken survived one hit");
    assert_eq!(
        after,
        before - 1.0,
        "one fist swing is one point of damage; saw {before} -> {after}"
    );
}

#[test]
fn a_second_swing_inside_the_hurt_window_is_refused() {
    let mut harness = Harness::new("p11-attack-iframe");
    harness.join("Spammer");
    let chicken = harness.summon_nearby(MobKind::Chicken);

    harness.swing(chicken.get());
    let after_first = harness.health(chicken).expect("alive");
    assert_eq!(after_first, 3.0);

    // A client that swings every tick is what a held-down mouse button produces.
    // Without the window this would take the chicken from 4 to 0 in four ticks,
    // which is the defect Audit 03 found on the player side and P11-06 fixes for
    // mobs.
    for _ in 0..3 {
        harness.swing(chicken.get());
    }
    assert_eq!(
        harness.health(chicken),
        Some(after_first),
        "swings inside the 10-tick window change nothing"
    );

    // And the window really does expire: the same swings land again after it.
    harness.run(INVULNERABLE_TICKS as usize);
    harness.swing(chicken.get());
    assert_eq!(
        harness.health(chicken),
        Some(after_first - 1.0),
        "a swing after the window lands, so the refusal above is a window and not a broken attack"
    );
}

#[test]
fn a_hit_that_empties_the_health_removes_the_mob() {
    let mut harness = Harness::new("p11-attack-kill");
    harness.join("Executioner");
    let chicken = harness.summon_nearby(MobKind::Chicken);

    let mut swings = 0;
    for _ in 0..20 {
        if harness.health(chicken).is_none() {
            break;
        }
        harness.swing(chicken.get());
        swings += 1;
        harness.run(INVULNERABLE_TICKS as usize);
    }
    assert!(
        harness.health(chicken).is_none(),
        "the chicken is gone after {swings} spaced swings"
    );
    assert!(
        (4..=8).contains(&swings),
        "four health at one point per window is 4 swings; saw {swings}, which means either the \
         damage or the window moved"
    );
}

#[test]
fn a_non_living_entity_cannot_be_hit() {
    let mut harness = Harness::new("p11-attack-item");
    harness.join("Fighter");

    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    let player = harness.game.player(harness.id).expect("player").position;
    let item = harness
        .game
        .spawn_item(
            ItemStack::new(stone, 1).expect("one stone"),
            mc_world::Vec3::new(player.x + 2.0, player.y, player.z),
        )
        .expect("the drop spawns");

    harness.swing(item.get());
    assert!(
        harness.game.entity_store().get(item).is_some(),
        "a dropped item has no health to reduce, so the hit is refused rather than applied"
    );
}

#[test]
fn a_swing_at_an_id_that_names_no_entity_is_ignored() {
    let mut harness = Harness::new("p11-attack-ghost");
    harness.join("Ghostbuster");
    let chicken = harness.summon_nearby(MobKind::Chicken);

    // A hostile client can name any integer. The tick must survive it and the
    // real mob must be untouched -- an id that gets *reconstructed* rather than
    // validated is how a client would reach an entity it was never sent.
    harness.swing(999_999);
    harness.swing(0);
    harness.swing(-1);
    assert_eq!(
        harness.health(chicken),
        Some(4.0),
        "no invented id damaged the real mob"
    );
    assert_eq!(harness.game.player_count(), 1, "and the tick survived");
}
