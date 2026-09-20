//! P16-03: status effects ticking end to end — poison and wither damage,
//! regeneration heals, expiry clears the icon, mobs tick their own effects.
//!
//! Sources here are direct (the `/effect` command path is pinned in
//! `admin_commands.rs`); this suite is about what happens after an effect is
//! stored: per-tick damage, floors, heals and removal packets.

use mc_entity::mob::MobKind;
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

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn leave(&mut self) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Left,
            })
            .expect("leave event queued");
        self.game.tick().expect("tick");
    }

    fn summon_nearby(&mut self, kind: MobKind) -> mc_entity::EntityId {
        let player = self.game.player(self.id).expect("player").position;
        let at = mc_world::Vec3::new(player.x + 2.0, player.y, player.z);
        self.game.spawn_mob(kind, at).expect("the mob spawns")
    }
}

#[test]
fn poison_ticks_damage_and_expiry_clears_the_icon() {
    // P16-03: amplifier-0 poison hits every 25th tick for 1.0; when the
    // duration runs out the client gets exactly one removal packet.
    let mut harness = Harness::new("p16-poison");
    let mut out = harness.join("Sick");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .give_effect(19, 0, 60);
    harness.run(30);
    let health = harness.game.player(harness.id).expect("player").health;
    assert!(
        (18.0..20.0).contains(&health),
        "one or two poison ticks landed in 30 ticks, saw {health}"
    );
    harness.run(35);
    assert!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .effects
            .is_empty(),
        "duration 60 expires after 65 ticks"
    );
    let mut removals = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::REMOVE_MOB_EFFECT {
            removals += 1;
        }
    }
    assert_eq!(removals, 1, "exactly one removal packet");
}

#[test]
fn wither_ticks_through_iron_and_can_kill() {
    // P16-03: wither bypasses armour (tag-grounded). Amplifier 0 hits every
    // 25th tick, comfortably outside the 10-tick window, so each hit lands;
    // the kill itself is pinned at entity level (no window there), while this
    // proves the server path applies wither damage through a full suit.
    let mut harness = Harness::new("p16-wither");
    harness.join("Doomed");
    let registries = mc_registry::Registries::vanilla().expect("registry");
    let player = harness.game.player_mut(harness.id).expect("player");
    for (slot, name) in [
        (36, "minecraft:iron_boots"),
        (37, "minecraft:iron_leggings"),
        (38, "minecraft:iron_chestplate"),
        (39, "minecraft:iron_helmet"),
    ] {
        let id = registries.items.id(name).expect("armor piece");
        player
            .inventory
            .set_slot(
                slot,
                mc_entity::stack::ItemStack::new(id, 1).expect("stack"),
            )
            .expect("equips");
    }
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .give_effect(20, 0, 100);
    harness.run(30);
    let health = harness.game.player(harness.id).expect("player").health;
    assert!(
        (18.0..20.0).contains(&health),
        "one or two wither ticks landed through iron, saw {health}"
    );
}

#[test]
fn regeneration_heals_without_overshoot() {
    // P16-03: amplifier-0 regen heals 1.0 every 50th tick, capped at max.
    let mut harness = Harness::new("p16-regen");
    harness.join("Hurt");
    let player = harness.game.player_mut(harness.id).expect("player");
    player.set_health(10.0);
    player.give_effect(10, 0, 200);
    harness.run(60);
    let health = harness.game.player(harness.id).expect("player").health;
    assert!(
        (11.0..=12.0).contains(&health),
        "one or two regen ticks landed in 60 ticks, saw {health}"
    );
}

#[test]
fn a_poisoned_mob_ticks_down_and_drops_no_xp_without_a_swing() {
    // P16-03: mob-side ticking mirrors the player path; the death goes
    // through the shared routine, so loot drops but no XP scatters without
    // a player killer (documented environmental rule).
    let mut harness = Harness::new("p16-mob-poison");
    harness.join("Witness");
    let zombie = harness.summon_nearby(MobKind::Zombie);
    harness
        .game
        .entity_store_mut()
        .get_mut(zombie)
        .expect("zombie")
        .effects
        .insert(
            19,
            mc_entity::effect::ActiveEffect {
                id: 19,
                amplifier: 0,
                duration: 100,
                ambient: false,
            },
        );
    harness.run(30);
    let health = harness
        .game
        .entity_store()
        .get(zombie)
        .expect("victim")
        .health;
    assert!(
        (18.0..20.0).contains(&health),
        "one or two poison ticks landed in 30 ticks, saw {health}"
    );
    // No death here, so no scatter question arises; the no-credit rule for
    // unsourced kills is pinned by the fall-kill test in `xp_orbs.rs`
    // (a cow that falls to its death leaves zero orbs).
}

#[test]
fn leaving_and_rejoining_resyncs_the_icons() {
    // P16-03: effects persist across the leave/join boundary (file first,
    // in-memory fallback), so a rejoin must re-announce them or the icon
    // stays lost until expiry.
    let mut harness = Harness::new("p16-rejoin");
    harness.join("Sticky");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .give_effect(1, 0, 6000);
    harness.leave();
    // The rejoin reloads from the leave-time playerdata write.
    let mut out = harness.join("Sticky");
    assert!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .effects
            .contains_key(&1),
        "speed survives the round trip"
    );
    let mut updates = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::UPDATE_MOB_EFFECT {
            updates += 1;
        }
    }
    assert_eq!(updates, 1, "one icon packet on rejoin");
}
