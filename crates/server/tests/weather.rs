//! Weather, slice 1: cycle, packets, `/weather`, persistence (P20-03).
//!
//! All assertions are client-visible or on-disk: `game_event` packets on a
//! joined player's channel (ids 1/2/7/8, jar-verified), `/weather` answers,
//! and `weather.dat` after a save. Nothing here reads `Game`'s private
//! weather state — the packets *are* the state as far as any client is
//! concerned.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::GameEvent;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
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
        let service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        // View distance 2 (TEST-TIME-PLAN §3): the cycle, its packets and the
        // storm's wetting are all read in the ring's inner chunks, and the
        // per-cell random-tick rate is invariant to the loaded-chunk count.
        let game =
            Game::build_with_operators(None, Some(service), 2, rx, DEFAULT_RANDOM_SEED, operators)
                .expect("game builds");
        // Boot reads `weather.dat` the way the lifecycle does (P20-03 wiring
        // lives in `lifecycle.rs`; the test calls the same loader).
        let mut harness = Self {
            game,
            events: tx,
            id: ConnectionId(1),
            dir,
        };
        let root = harness.dir.path().join("world");
        harness.game.load_weather(&root);
        harness
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

    /// Stand the player in the block at `(x, y, z)` (the sweep centres on
    /// players, so farm tests stand over their fields).
    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    fn command(&mut self, id: ConnectionId, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let (lines, _) = self.command_raw(id, out, text);
        lines
    }

    /// [`Harness::command`], keeping every packet the command and its tick
    /// produced — the chat-only drain above silently eats `game_event`s, which
    /// is exactly what a weather test asserts on.
    fn command_raw(
        &mut self,
        id: ConnectionId,
        out: &mut InboundReceiver,
        text: &str,
    ) -> (Vec<String>, Vec<mc_protocol::RawPacket>) {
        let mut report = mc_server::game::TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
        let mut lines = Vec::new();
        let mut raws = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT {
                match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                    Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                    Err(error) => panic!("a disguised_chat must decode: {error}"),
                }
            } else {
                raws.push(raw);
            }
        }
        (lines, raws)
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

/// Drain every `game_event` currently queued for the session.
fn game_events(out: &mut InboundReceiver) -> Vec<(u8, f32)> {
    let mut raws = Vec::new();
    while let Some(raw) = out.try_recv() {
        raws.push(raw);
    }
    decode_game_events(&raws)
}

/// Pull the `game_event`s out of collected raw packets.
fn decode_game_events(raws: &[mc_protocol::RawPacket]) -> Vec<(u8, f32)> {
    let mut events = Vec::new();
    for raw in raws {
        if raw.id == clientbound::play::GAME_EVENT {
            let event = GameEvent::decode(&raw.payload).expect("game_event decodes");
            events.push((event.event, event.value));
        }
    }
    events
}

/// Rain starts on schedule and the client hears about it (ids 1 and 7).
#[test]
fn rain_starts_and_packets_reach_the_client() {
    let mut harness = Harness::new("weather-start", ops_for("Chief", 4));
    let mut out = harness.join("Watcher");
    // Two ticks of dry weather left, then the flag flips.
    harness.game.set_weather(0, 2, 0, false, false);
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let events = game_events(&mut out);
    assert!(
        events.iter().any(|&(id, value)| id == 1 && value == 0.0),
        "START_RAINING must reach the client, saw {events:?}"
    );
    assert!(
        events.iter().any(|&(id, value)| id == 7 && value > 0.0),
        "RAIN_LEVEL_CHANGE must ease above zero, saw {events:?}"
    );
}

/// Thunder follows rain with its own level packet (id 8, no start event —
/// the jar sends none for thunder).
#[test]
fn thunder_follows_rain_with_its_level_packet() {
    let mut harness = Harness::new("weather-thunder", ops_for("Chief", 4));
    let mut out = harness.join("Watcher");
    harness.game.set_weather(0, 2, 2, false, false);
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let events = game_events(&mut out);
    assert!(
        events.iter().any(|&(id, _)| id == 1),
        "rain starts too, saw {events:?}"
    );
    assert!(
        events.iter().any(|&(id, value)| id == 8 && value > 0.0),
        "THUNDER_LEVEL_CHANGE must ease above zero, saw {events:?}"
    );
}

/// `/weather` sets, stops, refuses and needs operator level.
#[test]
fn weather_command_sets_stops_and_refuses() {
    let mut harness = Harness::new("weather-cmd", ops_for("Chief", 4));
    let (chief, mut chief_out) = (harness.id, harness.join("Chief"));
    // A second, unprivileged connection (id 2 shares nothing with id 1).
    let pleb = ConnectionId(2);
    let (pleb_outbound, mut pleb_out) = OutboundSender::pair(pleb, 8192);
    harness
        .events
        .try_send(ClientEvent {
            id: pleb,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Pleb"),
                outbound: pleb_outbound,
            },
        })
        .expect("join event queued");
    harness.game.tick().expect("tick");

    let lines = harness.command(pleb, &mut pleb_out, "weather rain");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "level 0 must be refused, saw {lines:?}"
    );

    let lines = harness.command(chief, &mut chief_out, "weather fog");
    assert!(
        lines.iter().any(|line| line.contains("Usage")),
        "an unknown kind is usage, saw {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_out, "weather rain 0");
    assert!(
        lines.iter().any(|line| line.contains("at least 1")),
        "a zero duration is refused, saw {lines:?}"
    );
    // The command's own packets are kept (the chat-only drain would eat
    // them), so the collection below sees what the rain command produced.

    let (lines, mut raws) = harness.command_raw(chief, &mut chief_out, "weather rain 50");
    assert!(
        lines.iter().any(|line| line.contains("rain")),
        "success names the weather, saw {lines:?}"
    );
    harness.game.tick().expect("tick");
    while let Some(raw) = chief_out.try_recv() {
        raws.push(raw);
    }
    let events = decode_game_events(&raws);
    assert!(
        events.iter().any(|&(id, _)| id == 1),
        "the command rains on clients the same tick, saw {events:?}"
    );

    let (lines, mut raws) = harness.command_raw(chief, &mut chief_out, "weather clear 5");
    assert!(
        lines.iter().any(|line| line.contains("clear")),
        "success names the weather, saw {lines:?}"
    );
    harness.game.tick().expect("tick");
    while let Some(raw) = chief_out.try_recv() {
        raws.push(raw);
    }
    let events = decode_game_events(&raws);
    assert!(
        events.iter().any(|&(id, _)| id == 2),
        "STOP_RAINING must reach the client, saw {events:?}"
    );
}

/// Rain wets farmland with no water nearby (the P20-02 rain arm, live).
#[test]
fn rain_wets_farmland_with_no_water_nearby() {
    let mut harness = Harness::new("weather-wets", ops_for("Chief", 4));
    harness.join("Watcher");
    for cz in -1..=1 {
        for cx in -1..=1 {
            assert!(
                harness.game.load_chunk(ChunkPos::new(cx, cz)),
                "farm chunk loads"
            );
        }
    }
    let blocks = harness.game.registries().blocks.clone();
    let dry = blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "0".to_owned())],
        )
        .expect("dry farmland");
    // 400 dry soils on an open platform, no water anywhere near.
    for i in 0..400 {
        let x = i % 20 - 2;
        let z = i / 20 - 2;
        harness
            .game
            .world_mut()
            .set_block(x, 120, z, dry)
            .expect("soil placed");
    }
    // Storm for the whole run (timers far beyond it).
    harness.game.set_weather(0, 100_000, 100_000, true, false);
    harness.stand(8, 121, 8);
    // 150 ticks, not 300: the per-cell rate is invariant, so the expectation
    // halves with the ticks (≈44 hits, floor 15) and the pin still separates a
    // storm from dry weather by a wide margin. The 300-tick draft paid 60 s for
    // margin the 20-hit floor never used.
    for _ in 0..150 {
        harness.game.tick().expect("tick");
    }
    let mut wet = 0_usize;
    for i in 0..400 {
        let x = i % 20 - 2;
        let z = i / 20 - 2;
        let Some(id) = harness.game.world().get_block_loaded(x, 120, z) else {
            continue;
        };
        let props = blocks.properties_of(id).unwrap_or_default();
        if props.iter().any(|(k, v)| k == "moisture" && v == "7") {
            wet += 1;
        }
    }
    // ≈44 random-tick hits over the run, each wetting straight to 7.
    assert!(
        wet >= 15,
        "a storm must wet the field (a dry one stays dry), wet {wet}/400"
    );
}

/// Weather survives a restart through `weather.dat`.
#[test]
fn weather_survives_restart() {
    let mut harness = Harness::new("weather-restart", ops_for("Chief", 4));
    harness.join("Watcher");
    harness.game.set_weather(0, 5_000, 3_000, true, true);
    harness.game.save_all_owned().expect("saves");
    harness.game.close_storage().expect("storage closes");
    // The dir must outlive the game: `TempDir` removes it on drop, and the
    // reopen below needs the saved document still there.
    let root = harness.dir.path().join("world");
    let Harness {
        game,
        dir: _keep_dir,
        ..
    } = harness;
    drop(game);

    // The document on disk carries the storm.
    let document = mc_persistence::level::read_weather(&root)
        .expect("weather reads")
        .expect("weather was saved");
    let data = document.get_compound("data").expect("data compound");
    assert_eq!(
        data.get("raining"),
        Some(&mc_nbt::NbtTag::Byte(1)),
        "raining persists"
    );
    assert_eq!(
        data.get("thundering"),
        Some(&mc_nbt::NbtTag::Byte(1)),
        "thundering persists"
    );

    // And a fresh boot reads it back instead of starting clear: dry soil
    // under open sky wets, which only happens while it rains.
    let config = mc_server::config::StorageConfig {
        world_dir: root.clone(),
        autosave_ticks: 0,
        seed: None,
    };
    let service = WorldService::open(&config).expect("world reopens");
    let (tx, rx) = game_channel(256);
    let mut second =
        Game::with_seed_and_storage(service, 2, rx, DEFAULT_RANDOM_SEED).expect("game rebuilds");
    second.load_weather(&root);
    for cz in -1..=1 {
        for cx in -1..=1 {
            assert!(second.load_chunk(ChunkPos::new(cx, cz)), "farm chunk loads");
        }
    }
    // A player centres the sweep (no player, no random ticks).
    let id = mc_network::bridge::ConnectionId(1);
    let (outbound, _out) = mc_network::bridge::OutboundSender::pair(id, 8192);
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Watcher"),
            outbound,
        },
    })
    .expect("join event queued");
    second.tick().expect("tick");
    second.player_mut(id).expect("player").position = mc_world::Vec3::new(8.5, 121.0, 8.5);
    let blocks = second.registries().blocks.clone();
    let dry = blocks
        .state_id(
            "minecraft:farmland",
            &[("moisture".to_owned(), "0".to_owned())],
        )
        .expect("dry farmland");
    for i in 0..200 {
        let x = i % 20 - 2;
        let z = i / 20 - 2;
        second
            .world_mut()
            .set_block(x, 120, z, dry)
            .expect("soil placed");
    }
    for _ in 0..300 {
        second.tick().expect("tick");
    }
    let mut wet = 0_usize;
    for i in 0..200 {
        let x = i % 20 - 2;
        let z = i / 20 - 2;
        let Some(id) = second.world().get_block_loaded(x, 120, z) else {
            continue;
        };
        let props = blocks.properties_of(id).unwrap_or_default();
        if props.iter().any(|(k, v)| k == "moisture" && v == "7") {
            wet += 1;
        }
    }
    assert!(
        wet > 0,
        "rebooted rain must wet dry soil (a clear boot stays dry)"
    );
}
