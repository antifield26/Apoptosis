//! Game rules: storage, `/gamerule`, and wiring (P20-05).
//!
//! Every pin here is behaviour through the public surface: `/gamerule`
//! answers, tick counters, the death path, the skip-night quorum, spawn
//! counts, the clock, and `game_rules.dat` on disk. Nothing reads `Game`'s
//! private rule state — the command and the world *are* the state as far as
//! any operator is concerned.
//!
//! Falsification shape, one per mechanism (`DoD` item 13): neutralise the
//! wiring (hard-code the old default at the call site) and the named test
//! goes red; the file is restored byte-exact afterwards.
//!
//! Health is compared exactly: every value here is a whole number of
//! half-hearts, so an approximate compare would hide the off-by-one these
//! pins exist to catch (same exemption as `pvp_authority.rs`).

#![allow(clippy::float_cmp)]

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{ContainerSetContent, GameEvent, PlayIntent, block_position};
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        // View distance 2 (TEST-TIME-PLAN §3): every pin below reads local
        // behaviour — command answers, a counter, one cell — so the ring is
        // cost, not coverage.
        let mut game =
            Game::build_with_operators(None, Some(storage), 2, rx, DEFAULT_RANDOM_SEED, operators)
                .expect("game builds");
        // Boot reads `game_rules.dat` the way the lifecycle does.
        let root = dir.path().join("world");
        game.load_game_rules(&root);
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            dir,
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

    fn command(&mut self, id: ConnectionId, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let mut report = mc_server::game::TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
        let mut lines = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT
                && let Ok(chat) = mc_protocol::packets::play::SystemChat::decode(&raw.payload)
            {
                lines.push(chat.content.as_plain().to_owned());
            }
        }
        lines
    }

    fn run(&mut self, ticks: usize) -> mc_server::game::TickReport {
        let mut report = mc_server::game::TickReport::default();
        for _ in 0..ticks {
            report = self.game.tick().expect("tick");
        }
        report
    }

    fn stand(&mut self, id: ConnectionId, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    fn block_name(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("registered")
            .to_owned()
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

/// Drain every `game_event` currently queued for the session.
fn game_events(out: &mut InboundReceiver) -> Vec<(u8, f32)> {
    let mut events = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::GAME_EVENT
            && let Ok(event) = GameEvent::decode(&raw.payload)
        {
            events.push((event.event, event.value));
        }
    }
    events
}

/// `/gamerule` lists, queries, sets and refuses through the dispatcher.
#[test]
fn gamerule_lists_queries_sets_and_refuses() {
    let mut harness = Harness::new("gamerule-command", ops_for("Chief", 4));
    let (op, mut op_out) = harness.join("Chief");
    let (pleb, mut pleb_out) = harness.join("Pleb");

    // Bare form lists all twelve rules with their jar defaults. The list
    // rides one chat message (lines joined with `\n`), beside the join
    // welcome already sitting in the channel.
    let lines = harness.command(op, &mut op_out, "gamerule");
    let list = lines
        .iter()
        .find(|line| line.starts_with("Game rules:"))
        .expect("the list arrives as one message");
    assert_eq!(
        list.matches('=').count(),
        12,
        "twelve rules listed, saw {lines:?}"
    );
    for expected in [
        "random_tick_speed = 3",
        "keep_inventory = false",
        "advance_weather = true",
        "players_sleeping_percentage = 100",
    ] {
        assert!(
            list.contains(expected),
            "{expected:?} is listed, saw {list:?}"
        );
    }
    // Query, set, query again.
    let lines = harness.command(op, &mut op_out, "gamerule keep_inventory");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("currently set to: false")),
        "query reports the default, saw {lines:?}"
    );
    let lines = harness.command(op, &mut op_out, "gamerule keep_inventory true");
    assert!(
        lines.iter().any(|line| line.contains("now set to: true")),
        "set confirms, saw {lines:?}"
    );
    let lines = harness.command(op, &mut op_out, "gamerule keep_inventory");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("currently set to: true")),
        "the set survives the query, saw {lines:?}"
    );
    // The `minecraft:` prefix is accepted like vanilla's argument type.
    let lines = harness.command(op, &mut op_out, "gamerule minecraft:pvp false");
    assert!(
        lines.iter().any(|line| line.contains("now set to: false")),
        "prefixed set works, saw {lines:?}"
    );
    // Refusals: unknown names (wiki camelCase included), mistyped values,
    // negative integers.
    for (command, hint) in [
        ("gamerule keepInventory true", "unknown game rule"),
        ("gamerule doDaylightCycle false", "unknown game rule"),
        ("gamerule not_a_rule", "unknown game rule"),
        ("gamerule pvp yes", "not true or false"),
        ("gamerule random_tick_speed fast", "non-negative integer"),
        ("gamerule random_tick_speed -1", "non-negative integer"),
    ] {
        let lines = harness.command(op, &mut op_out, command);
        assert!(
            lines.iter().any(|line| line.contains(hint)),
            "{command:?} is refused with {hint:?}, saw {lines:?}"
        );
    }
    // The stored-but-inert rule says so on the face of the reply.
    let lines = harness.command(
        op,
        &mut op_out,
        "gamerule fire_spread_radius_around_player 64",
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("no fire model reads it")),
        "the inert rule is labelled, saw {lines:?}"
    );
    // Level 0 may not touch the rules.
    let lines = harness.command(pleb, &mut pleb_out, "gamerule keep_inventory false");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "a level-0 set is refused, saw {lines:?}"
    );
}

/// `random_tick_speed 0` stops the draws; restoring it restarts them.
#[test]
fn random_tick_speed_zero_stops_the_draws() {
    let mut harness = Harness::new("gamerule-speed", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.stand(op, 8, 121, 8);
    let report = harness.run(3);
    assert!(
        report.random_tick_samples > 0,
        "the default speed draws samples"
    );
    let lines = harness.command(op, &mut out, "gamerule random_tick_speed 0");
    assert!(
        lines.iter().any(|line| line.contains("now set to: 0")),
        "speed 0 sets, saw {lines:?}"
    );
    let report = harness.run(3);
    assert_eq!(
        report.random_tick_samples, 0,
        "speed 0 draws nothing (neutralise the rules read and this fails)"
    );
    harness.command(op, &mut out, "gamerule random_tick_speed 3");
    let report = harness.run(3);
    assert!(
        report.random_tick_samples > 0,
        "restoring the speed restarts the draws"
    );
}

/// `advance_weather false` freezes the cycle; `true` resumes it.
#[test]
fn advance_weather_false_freezes_the_cycle() {
    let mut harness = Harness::new("gamerule-weather", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    // Three ticks of rain left: with the cycle running, the flip lands a
    // STOP_RAINING event on the client within a few ticks.
    harness.game.set_weather(0, 3, 0, true, false);
    harness.command(op, &mut out, "gamerule advance_weather false");
    harness.run(10);
    assert!(
        !game_events(&mut out).iter().any(|(event, _)| *event == 2),
        "no STOP_RAINING while the rule reads false"
    );
    let lines = harness.command(op, &mut out, "gamerule advance_weather true");
    assert!(
        lines.iter().any(|line| line.contains("now set to: true")),
        "resume sets, saw {lines:?}"
    );
    harness.run(5);
    assert!(
        game_events(&mut out).iter().any(|(event, _)| *event == 2),
        "resuming the cycle lets the pending flip land (neutralise the gate and the first half fails)"
    );
}

/// `advance_time false` freezes the clock; `true` resumes it.
#[test]
fn advance_time_false_freezes_the_clock() {
    let mut harness = Harness::new("gamerule-time", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.command(op, &mut out, "gamerule advance_time false");
    harness.run(5);
    // Read twice around a forty-tick gap: the freeze captures on the first
    // tick after the set, so the pre-freeze reading is one tick behind by
    // construction — only the two post-freeze readings must agree.
    let frozen = harness.command(op, &mut out, "time query");
    harness.run(40);
    let still = harness.command(op, &mut out, "time query");
    assert_eq!(
        frozen, still,
        "forty ticks pass with no clock movement (neutralise the freeze and this fails)"
    );
    harness.command(op, &mut out, "gamerule advance_time true");
    harness.run(40);
    let after = harness.command(op, &mut out, "time query");
    assert_ne!(frozen, after, "resuming the clock moves it again");
}

/// `keep_inventory true` keeps the inventory, the bar and the ground clear.
#[test]
fn keep_inventory_keeps_both() {
    let mut harness = Harness::new("gamerule-keep", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.command(op, &mut out, "give Chief minecraft:diamond 3");
    harness.command(op, &mut out, "xp set 7 levels");
    let level_before = harness.game.player(op).expect("player").level;
    assert_eq!(level_before, 7, "the setup grants 7 levels");
    harness.command(op, &mut out, "gamerule keep_inventory true");
    harness.command(op, &mut out, "kill Chief");
    harness.run(2);
    let player = harness.game.player(op).expect("player");
    assert_eq!(player.level, 7, "the bar survives the death");
    let diamond = harness
        .game
        .registries()
        .items
        .id("minecraft:diamond")
        .expect("diamond");
    let diamonds: i32 = (0..36)
        .map(|slot| {
            let stack = player.inventory.slot(slot);
            if stack.item_id() == Some(diamond) {
                stack.count()
            } else {
                0
            }
        })
        .sum();
    assert_eq!(diamonds, 3, "the diamonds stay in the inventory");
    let drops = harness
        .game
        .entity_store()
        .iter()
        .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Item(_)))
        .count();
    assert_eq!(drops, 0, "nothing scatters onto the ground");
}

/// `pvp false` refuses player-on-player swings; mobs are unaffected.
#[test]
fn pvp_false_blocks_player_hits() {
    let mut harness = Harness::new("gamerule-pvp", ops_for("Brawler", 4));
    let (attacker, mut attacker_out) = harness.join("Brawler");
    let (victim, _) = harness.join("Target");
    // Two blocks apart: inside the interaction range by a wide margin.
    let at = harness.game.player(attacker).expect("attacker").position;
    harness.game.player_mut(victim).expect("victim").position =
        mc_world::Vec3::new(at.x + 2.0, at.y, at.z);
    harness.run(1);
    let victim_entity = harness
        .game
        .entity_store()
        .iter()
        .find(|entity| {
            harness.game.player(victim).map(|player| player.entity_id) == Some(entity.id.get())
        })
        .map(|entity| entity.id)
        .expect("the victim's projection is in the store");
    // Default true: the swing lands.
    harness
        .events
        .try_send(ClientEvent {
            id: attacker,
            kind: ClientEventKind::Intent(PlayIntent::Interact {
                entity: victim_entity.get(),
                kind: 1,
            }),
        })
        .expect("swing queued");
    harness.run(1);
    assert_eq!(
        harness.game.player(victim).expect("victim").health,
        19.0,
        "a bare fist takes one half-heart off the default-rule victim"
    );
    // Rule off: the same swing whiffs entirely. The hurt window from the
    // first swing (10 ticks) must decay first, or the second swing lands
    // nothing for that reason and the pin is vacuous — this run-out is what
    // makes the neutralised gate go red.
    let lines = harness.command(attacker, &mut attacker_out, "gamerule pvp false");
    assert!(
        lines.iter().any(|line| line.contains("now set to: false")),
        "pvp off sets, saw {lines:?}"
    );
    harness.run(12);
    harness
        .events
        .try_send(ClientEvent {
            id: attacker,
            kind: ClientEventKind::Intent(PlayIntent::Interact {
                entity: victim_entity.get(),
                kind: 1,
            }),
        })
        .expect("swing queued");
    harness.run(1);
    assert_eq!(
        harness.game.player(victim).expect("victim").health,
        19.0,
        "pvp=false refuses the hit (neutralise the gate and this fails)"
    );
}

/// `players_sleeping_percentage 50` lets one of two sleepers skip the night.
#[test]
fn sleeping_percentage_custom_moves_the_quorum() {
    for (tag, percentage, skips) in [
        ("gamerule-sleep-half", Some(50), true),
        ("gamerule-sleep-full", None, false),
    ] {
        let mut harness = Harness::new(tag, ops_for("Chief", 4));
        let (op, mut out) = harness.join("Chief");
        let (friend, _) = harness.join("Friend");
        // No monsters near the beds: natural monster packs would refuse
        // every attempt, which is a different test's subject.
        harness.command(op, &mut out, "gamerule spawn_monsters false");
        harness.command(op, &mut out, "gamerule spawn_mobs false");
        if let Some(pct) = percentage {
            harness.command(
                op,
                &mut out,
                &format!("gamerule players_sleeping_percentage {pct}"),
            );
        }
        assert!(
            harness.game.load_chunk(ChunkPos::new(0, 0)),
            "bed chunk loads"
        );
        lay_bed(&mut harness.game, 8, 120, 8);
        harness.command(op, &mut out, "time set 18000");
        harness.stand(op, 8, 121, 7);
        // One sleeper: right-click the foot on its top face.
        harness
            .events
            .try_send(ClientEvent {
                id: op,
                kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
                    hand: 0,
                    position: block_position(8, 120, 8),
                    face: 1,
                    cursor_x: 0.5,
                    cursor_y: 0.5,
                    cursor_z: 0.5,
                    inside_block: false,
                    world_border_hit: false,
                    sequence: 0,
                }),
            })
            .expect("sleep intent queued");
        harness.run(130);
        let _ = friend;
        // The skip lands the clock at 0 and it advances from there, so the
        // assertion reads "morning" (under 2000), not exactly 0: ~30 ticks
        // pass between the skip and this query.
        let lines = harness.command(op, &mut out, "time query");
        let morning = lines.iter().any(|line| {
            line.split_whitespace()
                .nth(3)
                .and_then(|word| word.parse::<i64>().ok())
                .is_some_and(|time| time < 2000)
        });
        assert_eq!(
            morning, skips,
            "percentage {percentage:?} skips={skips}, saw {lines:?}"
        );
    }
}

/// Closed spawn gates keep the night empty; opening them fills it.
#[test]
fn spawn_gates_stop_natural_spawns() {
    let mut harness = Harness::new("gamerule-spawn", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.stand(op, 8, 121, 8);
    // Night, so the monster gate has something to refuse: daylight would
    // leave hostiles at zero with or without the rule (measured — a day
    // variant of this pin stayed green with both gates neutralised).
    harness.command(op, &mut out, "time set 18000");
    harness.command(op, &mut out, "gamerule spawn_mobs false");
    harness.command(op, &mut out, "gamerule spawn_monsters false");
    harness.run(300);
    let mobs = harness
        .game
        .entity_store()
        .iter()
        .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Mob(_)))
        .count();
    assert_eq!(
        mobs, 0,
        "closed gates spawn nothing in 300 night ticks (neutralise a gate and this fails)"
    );
    harness.command(op, &mut out, "gamerule spawn_mobs true");
    harness.command(op, &mut out, "gamerule spawn_monsters true");
    harness.run(300);
    let mobs = harness
        .game
        .entity_store()
        .iter()
        .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Mob(_)))
        .count();
    assert!(
        mobs > 0,
        "open gates spawn something in 300 night ticks (the positive control)"
    );
}

/// `mob_griefing false` spares mob tramples but never the player's own.
#[test]
fn mob_griefing_false_spares_mob_tramples() {
    let mut harness = Harness::new("gamerule-grief", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.command(op, &mut out, "gamerule mob_griefing false");
    put_moist_farmland(&mut harness.game, 8, 120, 8);
    // A stone shaft around the fall column: mob steering works mid-air, so
    // an open drop wanders off (till.rs measured this).
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for y in 121..=150 {
        for (x, z) in [
            (7, 7),
            (7, 8),
            (7, 9),
            (8, 7),
            (8, 9),
            (9, 7),
            (9, 8),
            (9, 9),
        ] {
            harness
                .game
                .world_mut()
                .set_block(x, y, z, stone)
                .expect("shaft writes");
        }
    }
    for k in 0..8 {
        harness
            .game
            .spawn_mob(
                MobKind::Zombie,
                mc_world::Vec3::new(8.5, 150.0 + f64::from(k) * 2.0, 8.5),
            )
            .expect("zombie spawns");
    }
    harness.run(150);
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:farmland",
        "eight mob landings trample nothing while the rule reads false"
    );
    // The player's own landing still tramples: the jar checks the rule for
    // non-players only. Hover first: teleporting via `stand` leaves a stale
    // `on_ground`, and a landing needs the airborne edge (till.rs).
    harness.stand(op, 8, 125, 8);
    harness
        .events
        .try_send(ClientEvent {
            id: op,
            kind: ClientEventKind::Intent(PlayIntent::MovePlayerPos {
                x: 8.5,
                y: 125.0,
                z: 8.5,
                on_ground: false,
            }),
        })
        .expect("hover intent queued");
    harness.game.tick().expect("tick");
    harness
        .events
        .try_send(ClientEvent {
            id: op,
            kind: ClientEventKind::Intent(PlayIntent::MovePlayerPos {
                x: 8.5,
                y: 120.0,
                z: 8.5,
                on_ground: false,
            }),
        })
        .expect("fall intent queued");
    harness.game.tick().expect("tick");
    assert_eq!(
        harness.block_name(8, 120, 8),
        "minecraft:dirt",
        "a 5-block player landing tramples regardless of the rule"
    );
}

/// `water_source_conversion` feeds the engine: two neighbouring sources
/// make a third iff the rule reads true.
#[test]
fn water_source_conversion_feeds_the_engine() {
    for (tag, rule, source) in [
        ("gamerule-convert-on", None, true),
        ("gamerule-convert-off", Some(false), false),
    ] {
        let mut harness = Harness::new(tag, ops_for("Chief", 4));
        let (op, mut out) = harness.join("Chief");
        if let Some(flag) = rule {
            harness.command(
                op,
                &mut out,
                &format!("gamerule water_source_conversion {flag}"),
            );
        }
        assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "pool loads");
        stone_floor(&mut harness.game);
        // Two sources with a level-1 cell between them: the engine's own
        // `getNewLiquid` shape (`sourceNeighborCount >= 2`), driven through
        // the phase rather than the unit API.
        place_water(&mut harness.game, -1, 64, 0, 0);
        place_water(&mut harness.game, 1, 64, 0, 0);
        place_water(&mut harness.game, 0, 64, 0, 1);
        harness.run(15);
        let level = water_level(&harness.game, 0, 64, 0);
        assert_eq!(
            level == Some(0),
            source,
            "conversion forms a source iff the rule reads true, saw level {level:?}"
        );
    }
}
/// `keep_inventory true` also re-syncs the menu on respawn: the death
/// screen clears the client's inventory view without telling the server,
/// so a delta sync would send nothing for items the server never lost.
/// The respawn handler sends the full window unconditionally.
#[test]
fn keep_inventory_respawn_resyncs_the_menu() {
    let mut harness = Harness::new("gamerule-resync", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.command(op, &mut out, "give Chief minecraft:diamond 3");
    harness.command(op, &mut out, "gamerule keep_inventory true");
    // Drain the give-time menu sync: without this the assertion below would
    // pass on the stale `give` packet even with the respawn re-sync removed.
    while out.try_recv().is_some() {}
    harness.command(op, &mut out, "kill Chief");
    harness.run(2);
    // Death itself may push packets (vitals, death screen); only the
    // respawn answer counts.
    while out.try_recv().is_some() {}
    // The respawn button: ClientCommand action 0.
    harness
        .events
        .try_send(ClientEvent {
            id: op,
            kind: ClientEventKind::Intent(PlayIntent::ClientCommand { action: 0 }),
        })
        .expect("respawn queued");
    harness.run(2);
    let diamond = harness
        .game
        .registries()
        .items
        .id("minecraft:diamond")
        .expect("diamond");
    let mut saw_diamonds = false;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::CONTAINER_SET_CONTENT
            && let Ok(contents) = ContainerSetContent::decode(&raw.payload)
        {
            saw_diamonds |= contents
                .slots
                .iter()
                .any(|stack| stack.item_id == diamond && stack.count == 3);
        }
    }
    assert!(
        saw_diamonds,
        "respawn re-sends the full menu with the kept diamonds (neutralise the resync and the client keeps showing an empty inventory)"
    );
}

/// Rules ride the save and come back on the next boot.
#[test]
fn rules_survive_restart() {
    let mut harness = Harness::new("gamerule-restart", ops_for("Chief", 4));
    let (op, mut out) = harness.join("Chief");
    harness.command(op, &mut out, "gamerule keep_inventory true");
    harness.command(op, &mut out, "gamerule random_tick_speed 7");
    harness.game.save_all_owned().expect("saves");
    harness.game.close_storage().expect("storage closes");
    let root = harness.dir.path().join("world");
    let Harness {
        game,
        dir: _keep_dir,
        ..
    } = harness;
    drop(game);

    // The document on disk carries the set values under `minecraft:` keys.
    let document = mc_persistence::level::read_game_rules(&root)
        .expect("rules read")
        .expect("rules were saved");
    let data = document.get_compound("data").expect("data compound");
    assert_eq!(
        data.get("minecraft:keep_inventory"),
        Some(&mc_nbt::NbtTag::Byte(1)),
        "keep_inventory persists"
    );
    assert_eq!(
        data.get("minecraft:random_tick_speed"),
        Some(&mc_nbt::NbtTag::Int(7)),
        "random_tick_speed persists"
    );

    // And a fresh boot reads them back instead of starting from defaults.
    let config = mc_server::config::StorageConfig {
        world_dir: root.clone(),
        autosave_ticks: 0,
        seed: None,
    };
    let service = WorldService::open(&config).expect("world reopens");
    let (tx, rx) = game_channel(256);
    // Rebuilt the way the lifecycle boots, grants included: the query below
    // runs at level 4, and the value must already be true before any new set.
    let mut second = Game::build_with_operators(
        None,
        Some(service),
        2,
        rx,
        DEFAULT_RANDOM_SEED,
        ops_for("Chief", 4),
    )
    .expect("game rebuilds");
    assert!(second.load_game_rules(&root), "the saved document loads");
    let id = mc_network::bridge::ConnectionId(1);
    let (outbound, mut out) = OutboundSender::pair(id, 8192);
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Chief"),
            outbound,
        },
    })
    .expect("join event queued");
    second.tick().expect("tick");
    let mut report = mc_server::game::TickReport::default();
    second
        .dispatch_command(id, "gamerule keep_inventory", &mut report)
        .expect("a command is answered");
    let mut lines = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::DISGUISED_CHAT
            && let Ok(chat) = mc_protocol::packets::play::SystemChat::decode(&raw.payload)
        {
            lines.push(chat.content.as_plain().to_owned());
        }
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("currently set to: true")),
        "the rebooted world kept the rule without any new set, saw {lines:?}"
    );
}

/// A bed with explicit facing (foot at `(x, y, z)`, head forward south).
fn lay_bed(game: &mut Game, x: i32, y: i32, z: i32) {
    let blocks = game.registries().blocks.clone();
    let foot = blocks
        .state_id(
            "minecraft:white_bed",
            &[
                ("facing".to_owned(), "south".to_owned()),
                ("occupied".to_owned(), "false".to_owned()),
                ("part".to_owned(), "foot".to_owned()),
            ],
        )
        .expect("foot state");
    let head = blocks
        .state_id(
            "minecraft:white_bed",
            &[
                ("facing".to_owned(), "south".to_owned()),
                ("occupied".to_owned(), "false".to_owned()),
                ("part".to_owned(), "head".to_owned()),
            ],
        )
        .expect("head state");
    game.world_mut()
        .set_block(x, y, z, foot)
        .expect("foot placed");
    game.world_mut()
        .set_block(x, y, z + 1, head)
        .expect("head placed");
}

fn put_moist_farmland(game: &mut Game, x: i32, y: i32, z: i32) {
    let soil = game
        .registries()
        .blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "7".to_owned())],
        )
        .expect("moist farmland");
    game.world_mut()
        .set_block(x, y, z, soil)
        .expect("soil placed");
}

/// A stone floor at `y = 63` under the pool, so water has something to sit
/// on (`fluid_core`'s shape, not its subject).
fn stone_floor(game: &mut Game) {
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone is registered");
    for x in -2..=2 {
        for z in -2..=2 {
            game.world_mut()
                .set_block(x, 63, z, stone)
                .expect("floor writes");
        }
    }
}

/// Write water at `level` and feed the change the way a placement does.
fn place_water(game: &mut Game, x: i32, y: i32, z: i32, level: u8) {
    let water = game
        .registries()
        .blocks
        .state_id(
            "minecraft:water",
            &[("level".to_owned(), level.to_string())],
        )
        .expect("water resolves");
    game.world_mut()
        .set_block(x, y, z, water)
        .expect("water writes");
    game.feed_block_change(x, y, z);
}

/// The `level` property of the water at `(x, y, z)`, if it has one.
fn water_level(game: &Game, x: i32, y: i32, z: i32) -> Option<u8> {
    let id = game.world().get_block_loaded(x, y, z)?;
    game.registries()
        .blocks
        .properties_of(id)
        .ok()?
        .iter()
        .find(|(key, _)| key == "level")
        .and_then(|(_, value)| value.parse::<u8>().ok())
}
