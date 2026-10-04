//! Lightning strikes, slice 2: damage, announcement, removal, transience
//! (P20-03 slice 2).
//!
//! Strikes arrive through the public `Game::strike_lightning` seam — the same
//! entry the thunderstorm scheduler calls — so every pin below proves the
//! production path, not a test double. The scheduler's 1-in-100 000 roll is
//! pinned by rate in `game::weather::tests`; wiring the roll to the seam is
//! three reviewed lines in the `RandomTicks` sweep.
//!
//! Health is compared exactly: every value here is a whole number of
//! half-hearts reached by subtracting exactly 5.0 once, so an approximate
//! compare would hide the off-by-one this suite exists to catch (same
//! exemption as `pvp_authority.rs`).

#![allow(clippy::float_cmp)]

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::play::{AddEntity, RemoveEntities};
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            dir,
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

    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }
}

/// Drain every `add_entity` a client received.
fn announced(out: &mut InboundReceiver) -> Vec<AddEntity> {
    let mut seen = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::ADD_ENTITY
            && let Ok(add) = AddEntity::decode(&mut raw.reader())
        {
            seen.push(add);
        }
    }
    seen
}

/// Drain every `remove_entities` batch a client received, flattened.
fn removals(out: &mut InboundReceiver) -> Vec<i32> {
    let mut seen = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::REMOVE_ENTITIES
            && let Ok(remove) = RemoveEntities::decode(&mut raw.reader())
        {
            seen.extend(remove.entity_ids);
        }
    }
    seen
}

/// A strike hurts the living once each and leaves drops alone.
#[test]
fn strike_damages_the_living_once_and_spares_drops() {
    let mut harness = Harness::new("lightning-damage");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.stand(8, 121, 8);
    let zombie = harness
        .game
        .spawn_mob(MobKind::Zombie, mc_world::Vec3::new(10.5, 121.0, 8.5))
        .expect("zombie spawns");
    let diamond = harness
        .game
        .registries()
        .items
        .id("minecraft:diamond")
        .expect("diamond");
    harness
        .game
        .spawn_item(
            mc_entity::stack::ItemStack::new(diamond, 7).expect("seven diamonds"),
            mc_world::Vec3::new(6.5, 121.0, 8.5),
        )
        .expect("the drop spawns");
    let zombie_before = harness
        .game
        .entity_store()
        .get(zombie)
        .expect("zombie")
        .health;
    let player_before = harness.game.player(harness.id).expect("player").health;
    // One tick publishes the session position into the entity projection:
    // the strike reads store positions, and a projection still standing at
    // the spawn block is outside every box drawn here.
    harness.run(1);

    harness
        .game
        .strike_lightning(8, 121, 8)
        .expect("the strike lands");

    // 5.0 off every living body in the box (neutralise the damage loop and
    // nothing moves).
    let zombie_after = harness
        .game
        .entity_store()
        .get(zombie)
        .expect("zombie survives")
        .health;
    assert_eq!(
        zombie_before - zombie_after,
        5.0,
        "a zombie in the box takes exactly the strike"
    );
    assert_eq!(
        player_before - harness.game.player(harness.id).expect("player").health,
        5.0,
        "and so does the player standing in it"
    );
    // The drop is untouched: strikes, like vanilla, only bill the living.
    let drops = harness.game.dropped_items();
    assert_eq!(drops.len(), 1, "the drop survives the strike");
    assert_eq!(drops[0].0.count(), 7, "with its count intact");
    // And the bolt itself is in the store, non-living and harmless.
    let bolts = harness
        .game
        .entity_store()
        .iter()
        .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Bolt(_)))
        .count();
    assert_eq!(bolts, 1, "one bolt visual per strike");
}

/// A strike announces as `minecraft:lightning_bolt` and is removed after its
/// fuse.
#[test]
fn strike_announces_bolt_and_removes_it() {
    let mut harness = Harness::new("lightning-wire");
    let mut out = harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.stand(8, 121, 8);
    // Drain the join burst: everything asserted below arrives after this.
    announced(&mut out);

    let bolt = harness
        .game
        .strike_lightning(8, 121, 8)
        .expect("the strike lands");
    harness.run(1);
    let bolt_type = harness
        .game
        .registries()
        .entities
        .id("minecraft:lightning_bolt")
        .expect("lightning_bolt is registered");
    let adds = announced(&mut out);
    assert!(
        adds.iter()
            .any(|add| add.entity_id == bolt.get() && add.type_id == bolt_type),
        "the strike announces with the jar's bolt type id (neutralise the type arm and the client draws a drop)"
    );
    // The fuse burns for four ticks: 4→3→2→1→0, removed on the fourth.
    harness.run(2);
    assert!(
        harness
            .game
            .entity_store()
            .iter()
            .any(|entity| entity.id == bolt),
        "the bolt outlives its first ticks"
    );
    harness.run(2);
    assert!(
        harness
            .game
            .entity_store()
            .iter()
            .all(|entity| entity.id != bolt),
        "the fuse expiry sweeps the bolt"
    );
    assert!(
        removals(&mut out).contains(&bolt.get()),
        "and the sweep broadcasts the removal"
    );
}

/// Without thunder the scheduler never strikes, however often it is asked.
#[test]
fn no_thunder_means_no_strikes() {
    let mut harness = Harness::new("lightning-gate");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    // Rain without thunder: every gate past the thundering check passes, so
    // only that check stands between the roll and a strike.
    harness.game.set_weather(0, 100_000, 0, true, false);
    for _ in 0..1_000_000 {
        harness.game.maybe_strike_lightning(ChunkPos::new(0, 0));
    }
    assert_eq!(
        harness
            .game
            .entity_store()
            .iter()
            .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Bolt(_)))
            .count(),
        0,
        "a million rain-without-thunder rolls strike nothing (neutralise the gate and ~10 land)"
    );
}

/// Bolts are weather, not world state: a save never carries one.
#[test]
fn bolts_do_not_survive_a_restart() {
    let mut harness = Harness::new("lightning-persist");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.stand(8, 121, 8);
    harness
        .game
        .strike_lightning(8, 121, 8)
        .expect("the strike lands");
    harness.game.save_all_owned().expect("saves");
    harness.game.close_storage().expect("storage closes");
    let root = harness.dir.path().join("world");
    let Harness {
        game,
        dir: _keep_dir,
        ..
    } = harness;
    drop(game);

    let config = mc_server::config::StorageConfig {
        world_dir: root.clone(),
        autosave_ticks: 0,
        seed: None,
    };
    let service = WorldService::open(&config).expect("world reopens");
    let (_tx, rx) = game_channel(256);
    let mut second =
        Game::with_seed_and_storage(service, 2, rx, DEFAULT_RANDOM_SEED).expect("game rebuilds");
    assert!(
        second.load_chunk(ChunkPos::new(0, 0)),
        "the saved chunk loads"
    );
    assert_eq!(
        second
            .entity_store()
            .iter()
            .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Bolt(_)))
            .count(),
        0,
        "no bolt comes back (exclude them from the save and this fails)"
    );
}
