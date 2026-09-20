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
fn a_fall_kill_scatters_nothing() {
    // P16-02: vanilla awards XP for player kills only. A mob that falls to
    // its death without a swing leaves no orbs (documented: no hurt-credit
    // tracking, so environmental finishes do not credit anyone).
    let mut harness = Harness::new("p16-fall-kill");
    harness.join("Witness");
    let cow = harness.summon_nearby(MobKind::Cow);
    harness
        .game
        .entity_store_mut()
        .get_mut(cow)
        .expect("cow")
        .position
        .y += 40.0;
    harness.run(120);
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
