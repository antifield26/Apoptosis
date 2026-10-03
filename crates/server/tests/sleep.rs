//! Beds and sleep, slice 1: placement, attempts, skip-night (P20-04).
//!
//! Sleep state itself (`session.sleeping`) is crate-private, so every pin
//! below reads what a client or the world can see: the bed's `occupied`
//! flags, the clock after a skip, chat refusals, and block pairs. A sleeper
//! is "awake" exactly when their bed reads unoccupied on both halves.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{PlayIntent, block_position};
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
        let service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        // View distance 2 (TEST-TIME-PLAN §3): every pin here reads a bed or a
        // clock in chunk (0, 0), so the 81-chunk ring bought cost, not coverage.
        let game =
            Game::build_with_operators(None, Some(service), 2, rx, DEFAULT_RANDOM_SEED, operators)
                .expect("game builds");
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
            if raw.id == clientbound::play::DISGUISED_CHAT {
                match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                    Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                    Err(error) => panic!("a disguised_chat must decode: {error}"),
                }
            }
        }
        lines
    }

    /// Stand the player in the block at `(x, y, z)`.
    fn stand(&mut self, id: ConnectionId, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    /// Right-click the block at `(x, y, z)` on its top face with an empty hand.
    fn click(&mut self, id: ConnectionId, x: i32, y: i32, z: i32) {
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
                    hand: 0,
                    position: block_position(x, y, z),
                    face: 1,
                    cursor_x: 0.5,
                    cursor_y: 0.5,
                    cursor_z: 0.5,
                    inside_block: false,
                    world_border_hit: false,
                    sequence: 0,
                }),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    /// Put one `item` in the held slot (hotbar slot 0).
    fn give(&mut self, id: ConnectionId, item: &str) {
        let item_id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(id)
            .expect("player")
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(item_id, 1).expect("stack"),
            )
            .expect("slot 0 takes it");
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

fn ops_empty() -> OperatorList {
    OperatorList::parse("[]", Path::new("ops.json")).expect("empty parses")
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

/// `occupied` of the foot and head starting at `(x, y, z)` (head south).
fn occupied_pair(game: &Game, x: i32, y: i32, z: i32) -> (bool, bool) {
    let blocks = &game.registries().blocks;
    let flag = |x: i32, y: i32, z: i32| {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        assert_eq!(
            blocks.block_name(id).expect("registered"),
            "minecraft:white_bed",
            "the bed is still a bed at ({x}, {y}, {z})"
        );
        blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "occupied")
            .map(|(_, v)| v == "true")
            .expect("occupied reads")
    };
    (flag(x, y, z), flag(x, y, z + 1))
}

/// Night falls on demand: `/time set` moves the clock, and the phase scope
/// sleeps at night or in thunder.
fn make_night(harness: &mut Harness, id: ConnectionId, out: &mut InboundReceiver) {
    let lines = harness.command(id, out, "time set 18000");
    assert!(
        lines.iter().any(|line| line.contains("18000")),
        "the clock moves, saw {lines:?}"
    );
}

/// A bed item places foot and head with the clicker's facing.
#[test]
fn bed_item_places_foot_and_head() {
    let mut harness = Harness::new("sleep-place", ops_for("Chief", 4));
    let (id, _out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "pad chunk loads"
    );
    // A dirt pad at y=120; the foot lands on its top face.
    let dirt = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt");
    harness
        .game
        .world_mut()
        .set_block(8, 120, 8, dirt)
        .expect("pad placed");
    harness.give(id, "minecraft:white_bed");
    harness.stand(id, 8, 121, 6);
    harness.click(id, 8, 120, 8);
    let blocks = harness.game.registries().blocks.clone();
    let foot = harness
        .game
        .world()
        .get_block_loaded(8, 121, 8)
        .expect("loaded");
    assert_eq!(
        blocks.block_name(foot).expect("registered"),
        "minecraft:white_bed",
        "the foot lands on the clicked top face"
    );
    // The head is one step along the facing, shares it, and both halves
    // start unoccupied.
    let facing = blocks
        .properties_of(foot)
        .expect("properties")
        .iter()
        .find(|(k, _)| k == "facing")
        .map(|(_, v)| v.clone())
        .expect("facing reads");
    let (hx, hz) = match facing.as_str() {
        "north" => (8, 7),
        "south" => (8, 9),
        "west" => (7, 8),
        "east" => (9, 8),
        other => panic!("cardinal facing, got {other}"),
    };
    let head = harness
        .game
        .world()
        .get_block_loaded(hx, 121, hz)
        .expect("loaded");
    assert_eq!(
        blocks.block_name(head).expect("registered"),
        "minecraft:white_bed",
        "the head lands forward of the foot"
    );
    for (cell, part) in [(foot, "foot"), (head, "head")] {
        let props = blocks.properties_of(cell).expect("properties");
        assert!(
            props.iter().any(|(k, v)| k == "part" && v == part),
            "halves read {part}: {props:?}"
        );
        assert!(
            props.iter().any(|(k, v)| k == "occupied" && v == "false"),
            "a fresh bed is unoccupied: {props:?}"
        );
        assert!(
            props.iter().any(|(k, v)| k == "facing" && v == &facing),
            "halves share the facing: {props:?}"
        );
    }
    // Survival consumes the item.
    let stack = harness
        .game
        .player_mut(id)
        .expect("player")
        .inventory
        .selected_item();
    assert!(stack.is_empty(), "placing consumes one bed in survival");
}

/// Every refusal reason, each with its message.
#[test]
fn sleep_refusals_each_tested() {
    let mut harness = Harness::new("sleep-refuse", ops_empty());
    let (id, mut out) = harness.join("Watcher");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    harness.stand(id, 8, 121, 6);
    // Day: the click still records home (jar order), but no sleep.
    harness.click(id, 8, 121, 8);
    let mut lines = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("night or during thunderstorms")),
        "daylight refuses with the night rule, saw {lines:?}"
    );
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (false, false),
        "a refused click never occupies"
    );
}

/// Ten sleepers skip the night; nine do not (percentage 100 default).
#[test]
fn ten_players_skip_the_night() {
    // Every sleeper operates: one of them moves the clock and the difficulty.
    // UUIDs come from the same offline derivation the join path reads, or
    // the grants below would miss their players.
    let mut entries = Vec::new();
    for n in 0..10 {
        let uuid = mc_network::auth::offline_profile(&format!("Sleeper{n}")).id;
        entries.push(format!(
            "{{\"uuid\": \"{uuid}\", \"name\": \"Sleeper{n}\", \"level\": 4}}"
        ));
    }
    let operators = OperatorList::parse(&format!("[{}]", entries.join(",")), Path::new("ops.json"))
        .expect("ten ops parse");
    let mut harness = Harness::new("sleep-skip", operators);
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    assert!(
        harness.game.load_chunk(ChunkPos::new(1, 0)),
        "bed chunk loads"
    );
    // Ten beds in a row, one per sleeper (a bed holds one — the second
    // clicker hears "occupied", which the next test pins).
    for bed in 0..10 {
        lay_bed(&mut harness.game, bed * 3, 121, 8);
    }
    let mut ids = Vec::new();
    let mut outs = Vec::new();
    for n in 0..10 {
        let (id, out) = harness.join(&format!("Sleeper{n}"));
        ids.push(id);
        outs.push(out);
    }
    // Night for the attempt, peaceful for the run: no natural monster may
    // wander into a bed box mid-test and refuse a sleeper the suite did not
    // invite (the peaceful gate is the product's own).
    let lines = harness.command(ids[0], &mut outs[0], "difficulty peaceful");
    assert!(
        lines.iter().any(|line| line.contains("Peaceful")),
        "difficulty sets, saw {lines:?}"
    );
    make_night(&mut harness, ids[0], &mut outs[0]);
    for (n, id) in ids.iter().enumerate().take(9) {
        let x = i32::try_from(n).expect("row fits") * 3;
        harness.stand(*id, x, 121, 6);
        harness.click(*id, x, 121, 8);
    }
    // Nine asleep, one awake: the night holds for a long run.
    for _ in 0..150 {
        harness.game.tick().expect("tick");
    }
    for bed in 0..9 {
        let x = bed * 3;
        assert_eq!(
            occupied_pair(&harness.game, x, 121, 8),
            (true, true),
            "nine sleepers stay asleep without the tenth"
        );
    }
    // The tenth tips the percentage: morning comes and every bed clears.
    harness.stand(ids[9], 27, 121, 6);
    harness.click(ids[9], 27, 121, 8);
    for _ in 0..150 {
        harness.game.tick().expect("tick");
        if (0..10).all(|bed| {
            let x = bed * 3;
            occupied_pair(&harness.game, x, 121, 8) == (false, false)
        }) {
            break;
        }
    }
    for bed in 0..10 {
        let x = bed * 3;
        assert_eq!(
            occupied_pair(&harness.game, x, 121, 8),
            (false, false),
            "the skip wakes every bed"
        );
    }
    let lines = harness.command(ids[0], &mut outs[0], "time");
    assert!(
        lines.iter().any(|line| line.contains("The time is 0 ")),
        "the skip jumps to morning, saw {lines:?}"
    );
}

/// A second sleeper at the same bed hears "occupied" (one bed, one player).
#[test]
fn second_sleeper_hears_occupied() {
    let mut harness = Harness::new("sleep-occupied", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    let (pleb, mut pleb_out) = harness.join("Pleb");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the first sleeper occupies both halves"
    );
    // The second sleeper stands close with night overhead and no monsters:
    // the only gate left is the occupant.
    harness.stand(pleb, 10, 121, 6);
    harness.click(pleb, 8, 121, 8);
    let mut lines = Vec::new();
    while let Some(raw) = pleb_out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines.iter().any(|line| line.contains("occupied")),
        "the second sleeper is refused, saw {lines:?}"
    );
}

/// A hostile mob in the bed box refuses the attempt.
#[test]
fn monsters_nearby_refuse_sleep() {
    let mut harness = Harness::new("sleep-monsters", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, id, &mut out);
    harness
        .game
        .spawn_mob(
            mc_entity::mob::MobKind::Zombie,
            mc_world::Vec3::new(8.5, 121.0, 10.5),
        )
        .expect("zombie spawns");
    harness.stand(id, 8, 121, 6);
    harness.click(id, 8, 121, 8);
    let mut lines = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines.iter().any(|line| line.contains("monsters nearby")),
        "a zombie in the box refuses, saw {lines:?}"
    );
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (false, false),
        "a refused click never occupies"
    );
}

/// Taking damage wakes the sleeper and clears the bed.
#[test]
fn damage_wakes_the_sleeper() {
    let mut harness = Harness::new("sleep-damage", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    harness.join("Watcher");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    // A floor under the fight: the bed floats (no support checks yet), but
    // the zombie obeys gravity — without stone under it, it falls 55 blocks
    // to the terrain and never reaches the sleeper.
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for x in 5..=12 {
        for z in 5..=11 {
            harness
                .game
                .world_mut()
                .set_block(x, 119, z, stone)
                .expect("floor writes");
        }
    }
    make_night(&mut harness, chief, &mut chief_out);
    // Two sessions: the skip needs both (percentage 100), so it can never
    // fire mid-test and mask the damage wake.
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the sleeper is down"
    );
    let before = harness.game.player(chief).expect("player").health;
    // Adjacent to the sleeper, on the floor: the 300-tick draft spawned it two
    // blocks away and paid ~88 s of pathing before the first hit. The pin is
    // "a hit wakes the sleeper", not "a zombie can walk" (`mob_pathing` owns
    // that), so the wait is the attack cooldown with a margin.
    harness
        .game
        .spawn_mob(
            mc_entity::mob::MobKind::Zombie,
            mc_world::Vec3::new(9.5, 121.0, 6.5),
        )
        .expect("zombie spawns");
    for _ in 0..80 {
        harness.game.tick().expect("tick");
    }
    let after = harness.game.player(chief).expect("player").health;
    assert!(
        after < before,
        "the zombie must land a hit (health {before} -> {after})"
    );
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (false, false),
        "the hit wakes the sleeper"
    );
}

/// Disconnecting wakes the sleeper and clears the bed.
#[test]
fn disconnect_wakes_the_sleeper() {
    let mut harness = Harness::new("sleep-leave", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    harness.join("Watcher");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the sleeper is down"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Left,
        })
        .expect("leave queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (false, false),
        "leaving clears the bed"
    );
}

/// Breaking either half wakes the sleeper and removes the partner.
#[test]
fn bed_break_wakes_and_removes_the_partner() {
    let mut harness = Harness::new("sleep-break", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the sleeper is down"
    );
    // Creative breaks instantly through the shared path; survival would dig
    // through the same `break_block_now` a few ticks later.
    let lines = harness.command(chief, &mut chief_out, "gamemode creative");
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "the op goes creative, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::PlayerAction {
                    status: 0,
                    position: mc_protocol::packets::play::block_position(8, 121, 8),
                    facing: 1,
                    sequence: 1,
                },
            ),
        })
        .expect("dig queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let blocks = harness.game.registries().blocks.clone();
    let air = blocks.air_id();
    assert_eq!(
        harness.game.world().get_block_loaded(8, 121, 8),
        Some(air),
        "the dug half is gone"
    );
    assert_eq!(
        harness.game.world().get_block_loaded(8, 121, 9),
        Some(air),
        "the partner goes with it (one drop, not two)"
    );
    // The sleeper is awake again: a fresh bed one step over accepts them
    // (a still-sleeping click is a silent no-op and leaves it empty).
    lay_bed(&mut harness.game, 14, 121, 8);
    harness.stand(chief, 14, 121, 6);
    harness.click(chief, 14, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 14, 121, 8),
        (true, true),
        "the woken player can sleep again"
    );
}

/// Dying with a recorded bed respawns on it (slice 2).
#[test]
fn bed_death_respawns_on_the_bed() {
    let mut harness = Harness::new("sleep-respawn", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the sleeper is down"
    );
    let lines = harness.command(chief, &mut chief_out, "kill");
    assert!(
        lines.iter().any(|line| line.contains("Killed")),
        "the op dies, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::ClientCommand { action: 0 },
            ),
        })
        .expect("respawn queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let player = harness.game.player(chief).expect("player");
    assert!(
        (player.position.x - 8.5).abs() < 0.01 && (player.position.z - 9.5).abs() < 0.01,
        "respawn stands at the bed x/z, at {:?}",
        player.position
    );
    assert!(
        player.position.y <= 122.01 && player.position.y > 121.0,
        "respawn starts above the head and settles onto the bed, at {:?}",
        player.position
    );
}

/// A broken bed spends the point: world spawn plus one message.
#[test]
fn broken_bed_respawns_at_world_spawn_with_message() {
    let mut harness = Harness::new("sleep-spent", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    // Creative dig removes the foot; the hook takes the head with it.
    let lines = harness.command(chief, &mut chief_out, "gamemode creative");
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "the op goes creative, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::PlayerAction {
                    status: 0,
                    position: mc_protocol::packets::play::block_position(8, 121, 8),
                    facing: 1,
                    sequence: 1,
                },
            ),
        })
        .expect("dig queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let lines = harness.command(chief, &mut chief_out, "gamemode survival");
    let _ = lines;
    let lines = harness.command(chief, &mut chief_out, "kill");
    assert!(
        lines.iter().any(|line| line.contains("Killed")),
        "the op dies, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::ClientCommand { action: 0 },
            ),
        })
        .expect("respawn queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let mut lines = Vec::new();
    while let Some(raw) = chief_out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("missing or obstructed")),
        "the spent point messages once, saw {lines:?}"
    );
    let (sx, _, _) = harness.game.world().spawn();
    let player = harness.game.player(chief).expect("player");
    assert!(
        (player.position.x - (f64::from(sx) + 0.5)).abs() < 1.0,
        "respawn falls back to world spawn, at {:?}",
        player.position
    );
}

/// `/spawnpoint` in all its forms, plus refusals.
#[test]
fn spawnpoint_sets_self_others_and_refuses() {
    let mut harness = Harness::new("sleep-spawnpoint", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    let (pleb, mut pleb_out) = harness.join("Pleb");
    // Level 0 cannot touch it.
    let lines = harness.command(pleb, &mut pleb_out, "spawnpoint");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "level 0 is refused, saw {lines:?}"
    );
    // Bare form: own feet.
    let lines = harness.command(chief, &mut chief_out, "spawnpoint");
    assert!(
        lines.iter().any(|line| line.contains("spawn point")),
        "bare form answers, saw {lines:?}"
    );
    // Explicit pos + yaw on self.
    let lines = harness.command(chief, &mut chief_out, "spawnpoint 100 64 -30 90");
    assert!(
        lines.iter().any(|line| line.contains("(100, 64, -30)")),
        "pos form answers, saw {lines:?}"
    );
    // Another player, by name, at their feet.
    let lines = harness.command(chief, &mut chief_out, "spawnpoint Pleb");
    assert!(
        lines.iter().any(|line| line.contains("Pleb")),
        "named form answers, saw {lines:?}"
    );
    // Unknown player, bad numbers, bad yaw: usage, not a side effect.
    for text in [
        "spawnpoint Nobody",
        "spawnpoint 1 2",
        "spawnpoint 1 2 three",
        "spawnpoint 1 2 3 north",
    ] {
        let lines = harness.command(chief, &mut chief_out, text);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Usage") || line.contains("not online")),
            "{text:?} is refused, saw {lines:?}"
        );
    }
    // The set point redirects death: Chief points at open sky (set
    // blindly, like the jar) and wakes up falling through it.
    let lines = harness.command(chief, &mut chief_out, "spawnpoint 200 120 40");
    assert!(
        lines.iter().any(|line| line.contains("(200, 120, 40)")),
        "pos form answers, saw {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_out, "kill");
    assert!(
        lines.iter().any(|line| line.contains("Killed")),
        "the op dies, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::ClientCommand { action: 0 },
            ),
        })
        .expect("respawn queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let player = harness.game.player(chief).expect("player");
    assert!(
        (player.position.x - 200.5).abs() < 0.01 && (player.position.z - 40.5).abs() < 0.01,
        "respawn stands at the set x/z, at {:?}",
        player.position
    );
    assert!(
        player.position.y <= 120.2 && player.position.y > 115.0,
        "respawn starts at the set height and falls, at {:?}",
        player.position
    );
}

/// A forced point inside solid rock still falls back (both cells must pass).
#[test]
fn forced_point_inside_rock_falls_back() {
    let mut harness = Harness::new("sleep-forced-rock", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    // Bury the point: solid stone at feet and head height.
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for y in [120, 121] {
        harness
            .game
            .world_mut()
            .set_block(200, y, 40, stone)
            .expect("rock placed");
    }
    let lines = harness.command(chief, &mut chief_out, "spawnpoint 200 120 40");
    assert!(
        lines.iter().any(|line| line.contains("(200, 120, 40)")),
        "set blindly, saw {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_out, "kill");
    assert!(
        lines.iter().any(|line| line.contains("Killed")),
        "the op dies, saw {lines:?}"
    );
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Intent(
                mc_protocol::packets::play::PlayIntent::ClientCommand { action: 0 },
            ),
        })
        .expect("respawn queued");
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let mut lines = Vec::new();
    while let Some(raw) = chief_out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            match mc_protocol::packets::play::SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("missing or obstructed")),
        "the failed point messages, saw {lines:?}"
    );
    let (sx, _, _) = harness.game.world().spawn();
    let player = harness.game.player(chief).expect("player");
    assert!(
        (player.position.x - (f64::from(sx) + 0.5)).abs() < 1.0,
        "respawn falls back to world spawn, at {:?}",
        player.position
    );
}

/// Respawn points survive a restart through playerdata.
#[test]
fn respawn_survives_restart() {
    let mut harness = Harness::new("sleep-persist", ops_for("Chief", 4));
    let (chief, mut chief_out) = harness.join("Chief");
    assert!(
        harness.game.load_chunk(ChunkPos::new(0, 0)),
        "bed chunk loads"
    );
    lay_bed(&mut harness.game, 8, 121, 8);
    make_night(&mut harness, chief, &mut chief_out);
    harness.stand(chief, 8, 121, 6);
    harness.click(chief, 8, 121, 8);
    assert_eq!(
        occupied_pair(&harness.game, 8, 121, 8),
        (true, true),
        "the sleeper is down"
    );
    // Leave writes playerdata (with the Spawn keys); the dir outlives the game.
    harness
        .events
        .try_send(mc_network::bridge::ClientEvent {
            id: chief,
            kind: mc_network::bridge::ClientEventKind::Left,
        })
        .expect("leave queued");
    for _ in 0..3 {
        harness.game.tick().expect("tick");
    }
    harness.game.save_all_owned().expect("saves");
    harness.game.close_storage().expect("storage closes");
    let root = harness.dir.path().join("world");
    let Harness {
        game,
        dir: _keep_dir,
        ..
    } = harness;
    drop(game);
    // The document on disk carries the bed: Spawn keys beside the Pos.
    let uuid = mc_network::auth::offline_profile("Chief").id.to_string();
    let tag = mc_server::playerdata::load(&root, &uuid)
        .expect("playerdata reads")
        .expect("the leave wrote it");
    assert_eq!(tag.get_i32("SpawnX"), Some(8), "SpawnX persists");
    assert_eq!(tag.get_i32("SpawnZ"), Some(9), "SpawnZ persists");
    let config = mc_server::config::StorageConfig {
        world_dir: root.clone(),
        autosave_ticks: 0,
        seed: None,
    };
    let service = WorldService::open(&config).expect("world reopens");
    let (tx, rx) = game_channel(256);
    let mut second = Game::build_with_operators(
        None,
        Some(service),
        4,
        rx,
        DEFAULT_RANDOM_SEED,
        ops_for("Chief", 4),
    )
    .expect("game rebuilds");
    second.load_weather(&root);
    assert!(second.load_chunk(ChunkPos::new(0, 0)), "bed chunk loads");
    let id = mc_network::bridge::ConnectionId(1);
    let (outbound, _out) = OutboundSender::pair(id, 8192);
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Chief"),
            outbound,
        },
    })
    .expect("join queued");
    second.tick().expect("tick");
    // Die and ask to respawn: the rebooted point aims at the same bed.
    {
        let mut report = mc_server::game::TickReport::default();
        second
            .dispatch_command(id, "kill", &mut report)
            .expect("answered");
    }
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Intent(mc_protocol::packets::play::PlayIntent::ClientCommand {
            action: 0,
        }),
    })
    .expect("respawn queued");
    for _ in 0..5 {
        second.tick().expect("tick");
    }
    let player = second.player(id).expect("player");
    assert!(
        (player.position.x - 8.5).abs() < 0.01 && (player.position.z - 9.5).abs() < 0.01,
        "rebooted respawn stands at the bed x/z, at {:?}",
        player.position
    );
}
