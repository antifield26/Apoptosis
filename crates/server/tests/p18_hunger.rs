//! P18-06 hunger and eating: jar exhaustion table, `UseItem` consumables,
//! saturation fast regen, difficulty starvation floors.
//!
//! Through the real [`Game`] tick loop. Falsification shape:
//!
//! - neutralise the food tick's exhaustion argument (`tick_food_ex(0.0, …)`)
//!   and `sprint_distance_accrues_exhaustion_per_the_jar_table` goes red;
//! - make `releasing_early_restores_nothing`'s cancel arm apply the food and
//!   that test goes red;
//! - fold the saturation fast-regen branch into the slow one and
//!   `saturation_fast_regen_spends_saturation_not_food` goes red.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]

use mc_entity::components::{
    Consumable, ConsumeAnimation, DataComponent, Food, ItemComponents, SoundRef,
};
use mc_entity::stack::ItemStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_protocol::wire::PacketReader;
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::path::Path;

const ACTION_RELEASE_USE_ITEM: i32 = 5;
const BREAD: i32 = 954;
const CONSUME_SECONDS: f32 = 1.6;
const CONSUME_TICKS: u32 = 32;

/// Vanilla bread's `food`/`consumable` components (P18-01a schema).
fn bread() -> ItemStack {
    let mut components = ItemComponents::new();
    components.set(DataComponent::Food(Food {
        nutrition: 5,
        saturation: 6.0,
        can_always_eat: false,
    }));
    components.set(DataComponent::Consumable(Consumable {
        consume_seconds: CONSUME_SECONDS,
        animation: ConsumeAnimation::Eat,
        sound: SoundRef::Named {
            name: "minecraft:entity.generic.eat".to_owned(),
            range: None,
        },
        consume_particles: true,
        on_consume_effects: Vec::new(),
    }));
    ItemStack::with_components(BREAD, 3, components).expect("bread stack")
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
        let game = Game::new(&storage, 4, rx).expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn build_floor(&mut self) -> (i32, i32, i32) {
        let (sx, sy, sz) = self.game.spawn();
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        for x in (sx - 8)..=(sx + 32) {
            for z in (sz - 8)..=(sz + 8) {
                self.game
                    .world_mut()
                    .set_block(x, sy - 1, z, stone)
                    .expect("floor");
            }
        }
        (sx, sy, sz)
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, receiver) = OutboundSender::pair(self.id, 8192);
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
        receiver
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

    /// Give the player a bread stack on hotbar 0 and select it.
    fn give_bread(&mut self) {
        let player = self.game.player_mut(self.id).expect("player");
        player.inventory.set_slot(0, bread()).expect("hotbar 0");
        player.inventory.select(0).expect("select 0");
    }

    /// A game with operators, for the `/give` path (mirrors `p18_commands`).
    fn new_with_operators(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::build_with_operators(
            None,
            Some(storage),
            4,
            rx,
            mc_server::game::DEFAULT_RANDOM_SEED,
            operators,
        )
        .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

/// Sprinting N horizontal blocks charges `EXHAUSTION_SPRINT` per block
/// (jar `FoodConstants.EXHAUSTION_SPRINT = 0.1f`), through the real Game.
///
/// Named red for the neutralisation: passing 0.0 into `tick_food_ex` instead
/// of the session accumulator leaves exhaustion at 0 and fails this.
#[test]
fn sprint_distance_accrues_exhaustion_per_the_jar_table() {
    let mut harness = Harness::new("p18-06-sprint");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Runner");
    while out.try_recv().is_some() {}

    // Sprint on (`player_input` bit 6).
    harness.intent(PlayIntent::PlayerInput { input: 64 });
    let start_x = f64::from(sx) + 0.5;
    let z = f64::from(sz) + 0.5;
    // 20 one-block steps: 20 × 0.1 = 2.0 exhaustion (< 4.0, so nothing spends).
    for step in 1..=20 {
        harness.intent(PlayIntent::MovePlayerPos {
            x: start_x + f64::from(step),
            y: f64::from(sy),
            z,
            on_ground: true,
        });
    }
    // Drain the interval into the player on the next food tick (tick 80).
    harness.run(80);

    let player = harness.game.player(harness.id).expect("player");
    assert!(
        (player.exhaustion - 2.0).abs() < 1.0e-4,
        "20 sprint blocks must charge 20 × EXHAUSTION_SPRINT (0.1) = 2.0, got {}",
        player.exhaustion
    );
}

/// Walking the same distance charges nothing (jar `EXHAUSTION_WALK = 0.0`).
#[test]
fn walking_accrues_no_exhaustion() {
    let mut harness = Harness::new("p18-06-walk");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Walker");
    while out.try_recv().is_some() {}
    let start_x = f64::from(sx) + 0.5;
    let z = f64::from(sz) + 0.5;
    for step in 1..=20 {
        harness.intent(PlayIntent::MovePlayerPos {
            x: start_x + f64::from(step),
            y: f64::from(sy),
            z,
            on_ground: true,
        });
    }
    harness.run(80);
    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(
        player.exhaustion, 0.0,
        "walking is free (jar EXHAUSTION_WALK = 0.0)"
    );
}

/// One sprint jump charges 0.2; one standing jump charges 0.05.
#[test]
fn jump_costs_follow_the_jar_table() {
    let mut harness = Harness::new("p18-06-jump");
    harness.build_floor();
    let mut out = harness.join("Jumper");
    while out.try_recv().is_some() {}

    // Standing jump (bit 4 rising edge, no sprint).
    harness.intent(PlayIntent::PlayerInput { input: 16 });
    harness.intent(PlayIntent::PlayerInput { input: 0 });
    // Sprint jump (bit 4 + bit 6).
    harness.intent(PlayIntent::PlayerInput { input: 64 | 16 });
    harness.intent(PlayIntent::PlayerInput { input: 64 });
    harness.run(80);

    let player = harness.game.player(harness.id).expect("player");
    assert!(
        (player.exhaustion - 0.25).abs() < 1.0e-4,
        "0.05 + 0.2 = 0.25, got {}",
        player.exhaustion
    );
}

/// Eating bread for `consume_seconds` applies +5 nutrition / +6.0 saturation
/// (the `food` component fields the P18-01a schema carries for bread).
#[test]
fn eating_bread_applies_nutrition_and_saturation_after_consume_seconds() {
    let mut harness = Harness::new("p18-06-eat");
    harness.build_floor();
    let mut out = harness.join("Eater");
    while out.try_recv().is_some() {}
    harness.give_bread();

    // Make room: 10 food, 0 saturation.
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.set_food(10);
        player.set_saturation(0.0);
        player.set_health(20.0);
    }
    let before = {
        let player = harness.game.player(harness.id).expect("player");
        (player.food, player.saturation)
    };

    harness.intent(PlayIntent::UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    });
    // Not yet: one tick short of consume_seconds.
    harness.run(CONSUME_TICKS as usize - 2);
    {
        let player = harness.game.player(harness.id).expect("player");
        assert_eq!(
            (player.food, player.saturation),
            before,
            "nothing applies before consume_seconds"
        );
    }
    // Finish the remaining ticks (the UseItem tick already counted one).
    harness.run(2);

    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(
        player.food,
        before.0 + 5,
        "bread nutrition is 5 (food component)"
    );
    assert!(
        (player.saturation - (before.1 + 6.0)).abs() < 1.0e-4,
        "bread saturation restore is 6.0 (food component), got {}",
        player.saturation
    );
    let held = player.inventory.selected_item();
    assert_eq!(held.count(), 2, "exactly one bread is consumed");
}

/// `releasing_early_restores_nothing`: cancelling a use restores no food and
/// does not consume the item. Own named red — if the cancel arm applied the
/// food this fails.
#[test]
fn releasing_early_restores_nothing() {
    let mut harness = Harness::new("p18-06-release");
    harness.build_floor();
    let mut out = harness.join("Quitter");
    while out.try_recv().is_some() {}
    harness.give_bread();
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.set_food(10);
        player.set_saturation(0.0);
        player.set_health(20.0);
    }

    harness.intent(PlayIntent::UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    });
    // Hold for half the duration, then release.
    harness.run(CONSUME_TICKS as usize / 2);
    harness.intent(PlayIntent::PlayerAction {
        status: ACTION_RELEASE_USE_ITEM,
        position: block_position(0, 0, 0),
        facing: 0,
        sequence: 2,
    });
    // Wait past the original finish time: nothing may land late either.
    harness.run(CONSUME_TICKS as usize);

    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(player.food, 10, "early release restores nothing");
    assert_eq!(player.saturation, 0.0, "early release restores nothing");
    assert_eq!(
        player.inventory.selected_item().count(),
        3,
        "early release does not consume the item"
    );
}

/// `saturation_fast_regen_spends_saturation_not_food`: the full-food saturated
/// tier heals and pays in saturation, never in food. Own named red — folding
/// this into the slow (food-paying) branch fails it.
#[test]
fn saturation_fast_regen_spends_saturation_not_food() {
    let mut harness = Harness::new("p18-06-fast-regen");
    harness.build_floor();
    let mut out = harness.join("Regenerator");
    while out.try_recv().is_some() {}
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.set_food(20);
        player.set_saturation(5.0);
        player.set_health(10.0);
        player.exhaustion = 0.0;
    }
    // One food tick.
    harness.run(80);

    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(player.health, 11.0, "saturated tier heals 1.0");
    assert_eq!(player.food, 20, "the fast tier never spends food");
    assert_eq!(player.saturation, 4.0, "the fast tier pays in saturation");
}

/// Difficulty starvation floors: easy 10 / normal 1 / hard 0 HP.
#[test]
fn starvation_respects_the_difficulty_health_floor() {
    for (tag, difficulty, floor) in [
        ("easy", mc_persistence::level::Difficulty::Easy, 10.0f32),
        ("normal", mc_persistence::level::Difficulty::Normal, 1.0f32),
        ("hard", mc_persistence::level::Difficulty::Hard, 0.0f32),
    ] {
        let mut harness = Harness::new(&format!("p18-06-starve-{tag}"));
        harness.build_floor();
        let mut out = harness.join("Hungry");
        while out.try_recv().is_some() {}
        harness
            .game
            .set_difficulty(difficulty)
            .expect("difficulty sets");
        {
            let player = harness.game.player_mut(harness.id).expect("player");
            player.set_food(0);
            player.set_saturation(0.0);
            player.set_health(20.0);
        }
        // Many food ticks: starve until the floor stops it (or death on hard).
        harness.run(80 * 30);

        let player = harness.game.player(harness.id).expect("player");
        if difficulty == mc_persistence::level::Difficulty::Hard {
            assert!(
                !player.is_alive() || player.health == 0.0,
                "hard starvation can kill"
            );
        } else {
            assert!(
                player.health <= floor + 0.01 && player.health > floor - 1.0,
                "{tag} floor is {floor} HP, got {}",
                player.health
            );
            assert!(
                player.health > floor - 0.01 || player.health >= floor - 1.0,
                "{tag} must not starve below the floor"
            );
        }
    }
}

/// A full player cannot start eating bread (`can_always_eat` is false).
#[test]
fn a_full_player_cannot_eat_bread() {
    let mut harness = Harness::new("p18-06-full");
    harness.build_floor();
    let mut out = harness.join("Stuffed");
    while out.try_recv().is_some() {}
    harness.give_bread();
    harness.intent(PlayIntent::UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    });
    harness.run(CONSUME_TICKS as usize + 4);
    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(player.food, 20);
    assert_eq!(player.inventory.selected_item().count(), 3, "nothing eaten");
}

/// A hunger spend reaches the client as `SetHealth` (P18-05 walk: the bar
/// never moved on hunger alone because nothing sent vitals after the food
/// spend — the drain only surfaced with the next damage sync).
///
/// Named red: removing the vitals-dirty send leaves the server values
/// drained but no `SetHealth` in flight, and this fails on the missing packet.
#[test]
fn hunger_spend_reaches_the_client_as_set_health() {
    let mut harness = Harness::new("p18-06-vitals");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Hungerer");
    while out.try_recv().is_some() {}
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.set_food(20);
        player.set_saturation(5.0);
        player.set_health(20.0);
    }

    // 44 sprint blocks = 4.4 exhaustion: safely past one 4.0 spend point
    // (40 would sit on the float edge) and short of two.
    harness.intent(PlayIntent::PlayerInput { input: 64 });
    let start_x = f64::from(sx) + 0.5;
    let z = f64::from(sz) + 0.5;
    for step in 1..=44 {
        harness.intent(PlayIntent::MovePlayerPos {
            x: start_x + f64::from(step),
            y: f64::from(sy),
            z,
            on_ground: true,
        });
    }
    harness.run(80);

    let player = harness.game.player(harness.id).expect("player");
    assert!(
        (player.saturation - 4.0).abs() < 1e-4,
        "one spend point must come off saturation, got {}",
        player.saturation
    );
    let mut seen = None;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::SET_HEALTH {
            let mut reader = PacketReader::new(&raw.payload);
            let _health = reader.read_f32().expect("health");
            let food = reader.read_varint().expect("food");
            let saturation = reader.read_f32().expect("saturation");
            assert!(reader.is_empty(), "SetHealth has no trailing bytes");
            seen = Some((food, saturation));
        }
    }
    let (food, saturation) = seen.expect("the spend must sync vitals to the client");
    assert_eq!(food, 20);
    assert!(
        (saturation - 4.0).abs() < 1e-4,
        "the client must see the drained saturation, got {saturation}"
    );
}

/// `/give` bread carries the derived food defaults, so it is edible, and the
/// eat finish syncs vitals (P18-05 walk: given bread had no components, so
/// `UseItem` silently did nothing, and even a hand-built eat never moved the
/// bar because `finish_eat` sent no vitals).
///
/// Named reds: dropping the defaults attach leaves the held stack without
/// food (first assertion); dropping the finish sync leaves the values
/// restored with no `SetHealth` in flight (last assertion).
#[test]
fn given_bread_is_edible_and_the_finish_syncs_vitals() {
    let mut harness = Harness::new_with_operators("p18-06-give-eat", ops_for("Giver", 4));
    harness.build_floor();
    let mut out = harness.join("Giver");
    while out.try_recv().is_some() {}
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.set_food(10);
        player.set_saturation(0.0);
        player.set_health(20.0);
    }
    let mut report = TickReport::default();
    harness
        .game
        .dispatch_command(harness.id, "give Giver minecraft:bread", &mut report)
        .expect("give is answered");
    {
        let player = harness.game.player(harness.id).expect("player");
        let held = player.inventory.selected_item();
        assert_eq!(held.count(), 1);
        let food = held.food().expect("given bread carries food defaults");
        assert_eq!(food.nutrition, 5);
        assert!(
            held.consumable().is_some(),
            "given bread carries consumable defaults"
        );
    }
    harness.intent(PlayIntent::UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    });
    harness.run(CONSUME_TICKS as usize + 4);
    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(player.food, 15, "bread restores 5");
    assert!(
        (player.saturation - 6.0).abs() < 1e-4,
        "bread restores 6.0 saturation, got {}",
        player.saturation
    );
    assert_eq!(
        player.inventory.selected_item().count(),
        0,
        "one bread consumed"
    );
    let mut seen = None;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::SET_HEALTH {
            let mut reader = PacketReader::new(&raw.payload);
            let _health = reader.read_f32().expect("health");
            let food = reader.read_varint().expect("food");
            let saturation = reader.read_f32().expect("saturation");
            assert!(reader.is_empty(), "SetHealth has no trailing bytes");
            seen = Some((food, saturation));
        }
    }
    let (food, saturation) = seen.expect("the eat finish must sync vitals to the client");
    assert_eq!(food, 15);
    assert!(
        (saturation - 6.0).abs() < 1e-4,
        "the client must see the restored bar, got {saturation}"
    );
}
