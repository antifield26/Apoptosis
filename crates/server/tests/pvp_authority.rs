//! `PvP`: the damage is the session's, and the players can see each other
//! (AUDIT-19 A-01 and A-02).
//!
//! Two real sessions, driven through the channel the network layer uses. The
//! defects these tests were written against:
//!
//! * **A-01** — a swing at a player wrote the *projection*
//!   (`entity.health`, `entity.invulnerable_ticks`) while the authoritative
//!   `session.player.health` was never touched and no `SetHealth` went out,
//!   and `tick_entity` early-returned for players, so the projection's hurt
//!   window never decayed: the first hit left the player permanently immune.
//! * **A-02** — a player projection was never announced (`pending_entity_spawns`
//!   excluded it and the chunk-stream resident path filtered `EntityKind::Player`
//!   out), so no client ever received `add_entity` for another player and `PvP` was
//!   untestable from a real client.
//!
//! Falsification shape, one per mechanism:
//!
//! * drop the session write and the victim's `Player::health` stops moving
//!   (`a_swing_takes_the_authoritative_health_of_a_player_victim`);
//! * drop the vitals push and no `SetHealth` reaches the victim's client (same
//!   test);
//! * drop the window decay and the second spaced swing does not land
//!   (`the_hurt_window_decays_for_a_player_victim`);
//! * drop the death hand-off and a lethal hit leaves the player at 0 HP with no
//!   death and no respawn (`a_lethal_player_hit_reaches_the_death_path`);
//! * restore the `EntityKind::Player` filters and no `add_entity` for a player
//!   reaches the other client, and a departure is never announced
//!   (`players_are_announced_to_each_other_as_players`,
//!   `a_leaving_player_is_removed_for_the_others`).
//!
//! Health is compared exactly: every value here is a whole number of half-hearts
//! reached by adding or subtracting `1.0`, so an approximate compare would hide
//! the off-by-one this suite exists to catch (same exemption as `player_attack.rs`).

#![allow(clippy::float_cmp)]

use mc_entity::EntityId;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{AddEntity, PlayIntent, RemoveEntities, SetHealth};
use mc_server::game::{Game, INVULNERABLE_TICKS};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// `minecraft:player` in the entity-type registry the client is sent
/// (`entity_types.tsv`, extracted from the 26.1.2 jar). Pinned again here
/// because it is the number on the wire, and `registry_ids.rs` owns the table.
const PLAYER_TYPE_ID: i32 = 155;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    _dir: TempDir,
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
        // Owned storage: a borrowing game never generates terrain, so both
        // players would stand in an all-air placeholder chunk (natural_spawn.rs
        // records the same trap).
        let game =
            Game::with_seed_and_storage(storage, 2, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, out) = OutboundSender::pair(id, 8192);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        (id, out)
    }

    fn leave(&mut self, id: ConnectionId) {
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Left,
            })
            .expect("leave event queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    /// Move `victim` two blocks east of `attacker` and let the projection
    /// follow, so the swing is inside vanilla's entity interaction range
    /// (`within_entity_reach`, 6.0 blocks from the eye) by a wide margin. Both
    /// players otherwise spawn on the same block, where a hit would pass for the
    /// degenerate reason.
    fn stand_apart(&mut self, attacker: ConnectionId, victim: ConnectionId) {
        let at = self.game.player(attacker).expect("attacker").position;
        let victim = self.game.player_mut(victim).expect("victim");
        victim.position = mc_world::Vec3::new(at.x + 2.0, at.y, at.z);
        self.run(1);
    }

    /// One attack swing at `entity`, the way the client sends it (`kind` 1).
    fn swing(&mut self, attacker: ConnectionId, entity: EntityId) {
        self.events
            .try_send(ClientEvent {
                id: attacker,
                kind: ClientEventKind::Intent(PlayIntent::Interact {
                    entity: entity.get(),
                    kind: 1,
                }),
            })
            .expect("swing queued");
        self.game.tick().expect("tick");
    }

    /// The authoritative health the HUD is fed from.
    fn health(&self, id: ConnectionId) -> f32 {
        self.game.player(id).expect("session").health
    }

    /// The health a reader of the entity store sees — the projection.
    fn projection_health(&self, entity: EntityId) -> Option<f32> {
        self.game.entity_store().get(entity).map(|e| e.health)
    }

    fn entity_of(&self, id: ConnectionId) -> EntityId {
        self.game
            .entity_store()
            .iter()
            .find(|entity| {
                self.game.player(id).map(|player| player.entity_id) == Some(entity.id.get())
            })
            .map(|entity| entity.id)
            .expect("the session's projection is in the store")
    }
}

/// Drain every `SetHealth` a client received.
fn health_packets(out: &mut InboundReceiver) -> Vec<f32> {
    let mut seen = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::SET_HEALTH
            && let Ok(health) = SetHealth::decode(&raw.payload)
        {
            seen.push(health.health);
        }
    }
    seen
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

#[test]
fn a_swing_takes_the_authoritative_health_of_a_player_victim() {
    let mut harness = Harness::new("pvp-authority");
    let (attacker, _attacker_out) = harness.join("Brawler");
    let (victim, mut victim_out) = harness.join("Target");
    harness.stand_apart(attacker, victim);
    let target = harness.entity_of(victim);
    // The join burst's own vitals are not what this test measures.
    let _ = health_packets(&mut victim_out);

    assert_eq!(
        harness.health(victim),
        20.0,
        "the victim joins at full health"
    );
    harness.swing(attacker, target);

    assert_eq!(
        harness.health(victim),
        19.0,
        "one fist swing takes 1.0 off the *session's* health, not the projection's"
    );
    assert_eq!(
        harness.projection_health(target),
        Some(19.0),
        "and the projection is mirrored, so a same-tick reader sees the truth"
    );
    let vitals = health_packets(&mut victim_out);
    assert!(
        vitals.contains(&19.0),
        "the victim's client must be told with set_health, saw {vitals:?}"
    );
}

#[test]
fn the_hurt_window_decays_for_a_player_victim() {
    // AUDIT-19 A-01: `tick_entity` early-returned for players, so the
    // projection's window never decayed and the first hit left the player
    // permanently immune — every later swing was refused by the `connects`
    // gate before any damage was computed.
    let mut harness = Harness::new("pvp-hurt-window");
    let (attacker, _attacker_out) = harness.join("Brawler");
    let (victim, _victim_out) = harness.join("Target");
    harness.stand_apart(attacker, victim);
    let target = harness.entity_of(victim);

    harness.swing(attacker, target);
    let after_first = harness.health(victim);
    assert_eq!(after_first, 19.0, "the first swing lands");

    for _ in 0..3 {
        harness.swing(attacker, target);
    }
    assert_eq!(
        harness.health(victim),
        after_first,
        "swings inside the 10-tick window change nothing"
    );

    harness.run(INVULNERABLE_TICKS as usize + 1);
    harness.swing(attacker, target);
    assert_eq!(
        harness.health(victim),
        after_first - 1.0,
        "the window expires for a player victim exactly as it does for a mob"
    );
}

#[test]
fn a_lethal_player_hit_reaches_the_death_path() {
    let mut harness = Harness::new("pvp-death");
    let (attacker, _attacker_out) = harness.join("Killer");
    let (victim, mut victim_out) = harness.join("Doomed");
    harness.stand_apart(attacker, victim);
    let target = harness.entity_of(victim);
    let _ = health_packets(&mut victim_out);
    // One point from death, so a single fist swing is lethal.
    harness.game.player_mut(victim).expect("victim").health = 1.0;

    harness.swing(attacker, target);

    assert_eq!(harness.health(victim), 0.0, "the hit is lethal");
    assert_eq!(
        harness.projection_health(target),
        Some(0.0),
        "the projection reports the death rather than being deleted"
    );
    assert!(
        harness.game.entity_store().get(target).is_some(),
        "a dead player is still an entity: the client keeps the body until it respawns"
    );
    let vitals = health_packets(&mut victim_out);
    assert!(
        vitals.contains(&0.0),
        "the death reaches the client as set_health 0, saw {vitals:?}"
    );

    // The existing respawn path still works and restores the maximum.
    harness
        .events
        .try_send(ClientEvent {
            id: victim,
            kind: ClientEventKind::Intent(PlayIntent::ClientCommand { action: 0 }),
        })
        .expect("respawn queued");
    harness.game.tick().expect("tick");
    assert_eq!(
        harness.health(victim),
        20.0,
        "the respawn request restores the player, so death was a state and not a removal"
    );
}

#[test]
fn players_are_announced_to_each_other_as_players() {
    let mut harness = Harness::new("pvp-visible");
    let (alpha, mut alpha_out) = harness.join("Alpha");
    let (beta, mut beta_out) = harness.join("Beta");
    let alpha_entity = harness.entity_of(alpha);
    let beta_entity = harness.entity_of(beta);
    assert_ne!(alpha_entity, beta_entity, "two players, two entities");

    let beta_sees = announced(&mut beta_out);
    let alpha_sees = announced(&mut alpha_out);
    let for_alpha = beta_sees
        .iter()
        .find(|add| add.entity_id == alpha_entity.get())
        .unwrap_or_else(|| panic!("Beta's client must be told about Alpha, saw {beta_sees:?}"));
    assert_eq!(
        for_alpha.type_id, PLAYER_TYPE_ID,
        "the announced type id is minecraft:player (155), not the item fallback"
    );
    // The identity on the wire is the entity store's own derivation
    // (`entity_uuid(seed, id)`), which is what every other announcement
    // carries. The *profile* uuid reaches a client through
    // `player_info_update`, which since AUDIT-19 A-04 the join sends to every
    // client (`player_info_tab_list.rs` pins that); the two are different
    // identities on purpose, so this assert stays about the store's.
    assert_eq!(
        for_alpha.uuid,
        harness
            .game
            .entity_store()
            .uuid(alpha_entity)
            .expect("the announced entity is live"),
        "the announcement carries the identity the store answers with"
    );
    let for_beta = alpha_sees
        .iter()
        .find(|add| add.entity_id == beta_entity.get())
        .unwrap_or_else(|| panic!("Alpha's client must be told about Beta, saw {alpha_sees:?}"));
    assert_eq!(for_beta.type_id, PLAYER_TYPE_ID);

    // Neither client is told a second identity for its own entity: that id came
    // with `JoinGame` and a second `add_entity` would contradict it.
    assert!(
        !beta_sees
            .iter()
            .any(|add| add.entity_id == beta_entity.get()),
        "Beta must not be announced to itself"
    );
    assert!(
        !alpha_sees
            .iter()
            .any(|add| add.entity_id == alpha_entity.get()),
        "Alpha must not be announced to itself"
    );
    assert_eq!(
        harness
            .game
            .registries()
            .entities
            .id(mc_registry::entities::PLAYER)
            .expect("the table has minecraft:player"),
        PLAYER_TYPE_ID,
        "the id sent is the registry's, not a literal"
    );
}

#[test]
fn a_leaving_player_is_removed_for_the_others() {
    let mut harness = Harness::new("pvp-despawn");
    let (alpha, mut alpha_out) = harness.join("Alpha");
    let (beta, _beta_out) = harness.join("Beta");
    let beta_entity = harness.entity_of(beta);
    // The announcement is the precondition for the removal meaning anything.
    assert!(
        announced(&mut alpha_out)
            .iter()
            .any(|add| add.entity_id == beta_entity.get()),
        "Beta was announced before leaving"
    );

    harness.leave(beta);
    harness.run(2);

    let gone = removals(&mut alpha_out);
    assert!(
        gone.contains(&beta_entity.get()),
        "Alpha's client must be told Beta's entity is gone, saw {gone:?}"
    );
    assert_eq!(harness.game.player_count(), 1, "and the session is gone");
    let _ = alpha;
}
