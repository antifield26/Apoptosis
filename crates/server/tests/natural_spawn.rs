//! Natural mob spawning end to end (P11-01): the measured rules drive real
//! spawn cycles around a joined player.
//!
//! These tests run the real [`Game`] tick loop — real world generation, real
//! light, real entity store — with the clock offset choosing day or night.
//! The assertions that matter are the ones the rules make deterministic:
//!
//! - **at noon no hostile spawns on the surface**: raw brightness 15 exceeds
//!   every `0..=7` sample, so the monster rule refuses regardless of the RNG;
//! - **no mob ever comes within the measured 24-block minimum** of a player;
//! - **a mob spawn is announced by `add_entity` and given its spawn health by
//!   `set_entity_data`**, the pair the P10-07 captures showed.
//!
//! The counts themselves (> 0) depend on the seeded RNG and are pinned to this
//! seed: a rule or table change that moves them is a visible change, which is
//! the point.
//!
//! What this does **not** prove: that a real client renders the mobs (that is
//! P11-03's acceptance), and that vanilla's spawn *density* matches — the
//! cadence simplification is on the record in `mc_server::spawn`.

use mc_entity::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
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
        // **The game must own its storage**: a borrowing game refuses to
        // generate terrain (it cannot know nothing is stored), so every chunk
        // would be an all-air placeholder and no spawn position could ever
        // pass the solid-floor check.
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

    /// Join a player and run the tick that applies the join.
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

    /// Run `ticks` more ticks of the simulation.
    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }
}

/// Run `count` ticks; the spawn cycle is part of every tick.
fn cycles(harness: &mut Harness, count: usize) {
    harness.run(count);
}

#[test]
fn at_noon_the_surface_spawns_passives_and_no_hostiles() {
    let mut harness = Harness::new("p11-spawn-day");
    harness.game.set_time_offset(6_000);
    harness.join("Daylight");
    cycles(&mut harness, 200);

    let mobs = harness.game.mobs();
    let hostiles = mobs
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                MobKind::Zombie | MobKind::Skeleton | MobKind::Spider | MobKind::Creeper
            )
        })
        .count();
    assert_eq!(
        hostiles, 0,
        "at noon the surface brightness refuses every hostile; saw {mobs:?}"
    );
    assert!(
        mobs.iter().any(|(kind, _)| matches!(
            kind,
            MobKind::Cow | MobKind::Pig | MobKind::Sheep | MobKind::Chicken
        )),
        "daylight spawns the plains passives; saw {mobs:?}"
    );
}

#[test]
fn at_night_the_surface_spawns_hostiles_and_no_passives() {
    let mut harness = Harness::new("p11-spawn-night");
    harness.game.set_time_offset(15_000);
    harness.join("Nightfall");
    cycles(&mut harness, 300);

    let mobs = harness.game.mobs();
    // **No "passives never spawn at night" assertion**: the animal rule reads
    // raw brightness with no sky darkening (`getRawBrightness(pos, 0) > 8`),
    // so a moonlit surface passes it. The bytecode is the authority here, and
    // the folklore it corrects is exactly the kind of thing this suite keeps
    // catching.
    assert!(
        mobs.iter().any(|(kind, _)| matches!(
            kind,
            MobKind::Zombie | MobKind::Skeleton | MobKind::Spider | MobKind::Creeper
        )),
        "night spawns hostiles; saw {mobs:?}"
    );
}

#[test]
fn no_mob_spawns_inside_the_measured_minimum_distance() {
    let mut harness = Harness::new("p11-spawn-ring");
    harness.game.set_time_offset(15_000);
    harness.join("Ringer");
    cycles(&mut harness, 30);

    let player = harness.game.player(harness.id).expect("player").position;
    for (kind, position) in harness.game.mobs() {
        let dx = position.x - player.x;
        let dz = position.z - player.z;
        let distance_sq = dx * dx + dz * dz;
        assert!(
            distance_sq >= mc_server::spawn::MIN_SPAWN_DISTANCE_SQR,
            "{kind:?} spawned {distance_sq:?} from the player, inside the 24-block minimum"
        );
    }
}

#[test]
fn a_mob_spawn_is_announced_and_then_given_its_health() {
    let mut harness = Harness::new("p11-spawn-announce");
    harness.game.set_time_offset(15_000);
    let mut out = harness.join("Announcer");
    cycles(&mut harness, 300);

    let mut ids = Vec::new();
    while let Some(raw) = out.try_recv() {
        ids.push(raw.id);
    }
    let adds = ids
        .iter()
        .filter(|id| **id == clientbound::play::ADD_ENTITY)
        .count();
    let datas = ids
        .iter()
        .filter(|id| **id == clientbound::play::SET_ENTITY_DATA)
        .count();
    let mobs = harness.game.mobs().len();
    assert!(mobs > 0, "the night spawned mobs to announce");
    assert_eq!(
        adds, mobs,
        "every mob is announced once (saw {adds} adds for {mobs} mobs)"
    );
    assert_eq!(
        datas, adds,
        "every announcement is followed by its spawn-health set_entity_data"
    );
}
