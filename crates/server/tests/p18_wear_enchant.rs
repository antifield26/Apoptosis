//! P18-01b acceptance: durability wear and the four enchantment effects.
//!
//! Through the real tick loop: survival digs spend `WEAR_ON_DIG` on the held
//! tool (`damage + N` after N breaks), a dig that reaches `max_damage` empties
//! the slot, Efficiency shortens a correct-tool dig, and Sharpness raises the
//! swing. Unbreaking and Protection are pinned as named unit behaviour in
//! `mc-entity` (`wear` / `enchant` / `combat`); this file is the e2e matrix.
//!
//! Falsification: neutralise `WEAR_ON_DIG` to 0 and `wear_matrix_dig_n_times_adds_n_damage`
//! goes red; zero `efficiency_bonus` and the Efficiency arm goes red; zero
//! `sharpness_bonus` and the Sharpness arm goes red.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_entity::stack::ItemStack;
use mc_entity::wear::{self, WearOutcome};
use mc_entity::{DataComponent, ItemComponents};
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

const START: i32 = 0;

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

    fn feet(&self) -> (i32, i32, i32) {
        let p = self.game.player(self.id).expect("player").position;
        (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32)
    }

    fn place(&mut self, x: i32, y: i32, z: i32, block: &str) {
        let id = self
            .game
            .registries()
            .blocks
            .default_state(block)
            .unwrap_or_else(|_| panic!("{block} is a known block"));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("the block is set");
    }

    fn stand(&mut self) {
        self.game.player_mut(self.id).expect("player").on_ground = true;
    }

    fn give_tool(&mut self, item: &str, components: Option<ItemComponents>) {
        let id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        let name = item.to_owned();
        let mut stack = ItemStack::new(id, 1).expect("stack");
        wear::ensure_durability(&mut stack, &name);
        if let Some(components) = components {
            for component in components.into_entries() {
                stack.set_component(component);
            }
        }
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, stack)
            .expect("slot 0 takes the tool");
    }

    fn held(&self) -> ItemStack {
        self.game
            .player(self.id)
            .expect("player")
            .inventory
            .selected_item()
    }

    fn damage(&self) -> Option<i32> {
        self.held().damage()
    }

    fn is_held_empty(&self) -> bool {
        self.held().is_empty()
    }

    fn dig_one(&mut self, x: i32, y: i32, z: i32) {
        self.intent(PlayIntent::PlayerAction {
            status: START,
            position: block_position(x, y, z),
            facing: 1,
            sequence: 0,
        });
    }
}

/// Wear-matrix pin: N survival digs spend N durability.
/// Neutralising `WEAR_ON_DIG` to 0 turns this red.
#[test]
fn wear_matrix_dig_n_times_adds_n_damage() {
    let mut harness = Harness::new("p18-wear-matrix");
    let mut out = harness.join("Miner");
    let _ = out.try_recv();
    harness.give_tool("minecraft:iron_pickaxe", None);
    assert_eq!(harness.damage(), Some(0), "fresh tool starts at damage 0");
    harness.stand();
    let (fx, fy, fz) = harness.feet();
    // Targets beside the player (not under the feet): digging the support
    // block would drop the mover and push later targets out of reach.
    for i in 0..5 {
        let x = fx + 1 + i;
        harness.place(x, fy - 1, fz, "minecraft:dirt");
        harness.dig_one(x, fy - 1, fz);
        harness.run(20);
        assert_eq!(
            harness.damage(),
            Some(i + 1),
            "dig {} must add one durability (WEAR_ON_DIG)",
            i + 1
        );
    }
}

/// Break-at-max pin: the dig that reaches `max_damage` empties the slot.
#[test]
fn break_at_max_empties_the_slot() {
    let mut harness = Harness::new("p18-break-at-max");
    let mut out = harness.join("Miner");
    let _ = out.try_recv();
    // A pick with one durability point left.
    let mut components = ItemComponents::new();
    components.set(DataComponent::MaxDamage(1));
    components.set(DataComponent::Damage(0));
    harness.give_tool("minecraft:iron_pickaxe", Some(components));
    harness.stand();
    let (fx, fy, fz) = harness.feet();
    let y = fy - 1;
    harness.place(fx, y, fz, "minecraft:dirt");
    harness.dig_one(fx, y, fz);
    harness.run(20);
    assert!(
        harness.is_held_empty(),
        "reaching max_damage leaves an empty hand"
    );
}

/// Named behaviour pin: Efficiency shortens a correct-tool dig.
/// Neutralising `efficiency_bonus` to 0.0 turns this red.
#[test]
fn efficiency_speeds_up_the_dig() {
    // Unit-level through the mining table: the e2e dig harness would need a
    // block whose tick counts differ enough to assert without flaking.
    // Stone (1.5) + diamond pick (8.0): plain 6 ticks, Efficiency III 3.
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry");
    let blocks = mc_registry::BlockRegistry::load(&dir.join("blocks.tsv")).expect("blocks");
    let items = mc_registry::ItemRegistry::load(&dir.join("items.tsv")).expect("items");
    let plain = mc_registry::dig_rate(
        &blocks,
        &items,
        Some("minecraft:diamond_pickaxe"),
        "minecraft:stone",
        true,
        0,
    );
    let fast = mc_registry::dig_rate(
        &blocks,
        &items,
        Some("minecraft:diamond_pickaxe"),
        "minecraft:stone",
        true,
        3,
    );
    assert_eq!(plain.ticks_to_break(), Some(6));
    assert_eq!(fast.ticks_to_break(), Some(3));
}

/// Named behaviour pin: Sharpness raises the swing (e2e through `Game`).
/// Neutralising `sharpness_bonus` to 0.0 turns this red.
#[test]
fn sharpness_raises_the_swing() {
    use mc_entity::combat::held_damage_with_effects;
    use mc_entity::enchant::SHARPNESS;
    let mut harness = Harness::new("p18-sharpness");
    let mut out = harness.join("Fighter");
    let _ = out.try_recv();
    harness.give_tool("minecraft:diamond_sword", None);
    let items = harness.game.registries().items.clone();
    let plain = {
        let player = harness.game.player(harness.id).expect("player");
        held_damage_with_effects(&player.inventory, &items, &[])
    };
    assert_eq!(plain, 7.0, "fist 1 + diamond sword 6");
    let mut components = ItemComponents::new();
    components.set(DataComponent::Enchantments(vec![(SHARPNESS, 1)]));
    harness.give_tool("minecraft:diamond_sword", Some(components));
    let enchanted = {
        let player = harness.game.player(harness.id).expect("player");
        held_damage_with_effects(&player.inventory, &items, &[])
    };
    assert_eq!(enchanted, 8.0, "Sharpness I adds 1.0");
}

/// Named behaviour pin: Unbreaking can skip a wear (deterministic rolls).
/// Neutralising `unbreaking_applies` to always-true turns the skip arm red.
#[test]
fn unbreaking_can_skip_a_wear() {
    use mc_entity::enchant::UNBREAKING;
    let mut components = ItemComponents::new();
    components.set(DataComponent::MaxDamage(250));
    components.set(DataComponent::Damage(0));
    components.set(DataComponent::Enchantments(vec![(UNBREAKING, 1)]));
    let mut stack = ItemStack::with_components(1, 1, components).expect("stack");
    // Tool I: apply when roll % 2 == 0. Roll 1 skips.
    let skipped = wear::apply_wear(&mut stack, wear::WEAR_ON_DIG, false, 1, 0.0);
    assert_eq!(skipped, WearOutcome::Untouched);
    assert_eq!(stack.damage(), Some(0));
    let landed = wear::apply_wear(&mut stack, wear::WEAR_ON_DIG, false, 0, 0.0);
    assert_eq!(landed, WearOutcome::Survived);
    assert_eq!(stack.damage(), Some(1));
}

/// Named behaviour pin: Protection cuts post-armour damage.
/// Neutralising `damage_after_protection` to identity turns this red.
#[test]
fn protection_reduces_damage() {
    use mc_entity::combat::{CombatStats, armor_absorb};
    use mc_entity::enchant::damage_after_protection;
    use mc_entity::player::{MAX_HEALTH, Player};
    // Iron chest (6 armour, no toughness) + Protection IV (4 points) vs 10.0.
    let stats = CombatStats {
        armor: 6.0,
        toughness: 0.0,
        knockback_resistance: 0.0,
        protection: 4.0,
    };
    // armour_absorb(10, 6, 0): real_armor clamps to 6*0.2=1.2 → 10*(1-1.2/25).
    let armored = armor_absorb(10.0, stats.armor, stats.toughness);
    assert!((armored - 9.52).abs() < 1e-3, "got {armored}");
    // Then Protection IV: ×(1 - 4/25) = 9.52 * 0.84.
    let after = damage_after_protection(armored, stats.protection);
    assert!((after - 7.9968).abs() < 1e-3, "got {after}");
    let mut player = Player::new(
        mc_entity::GameProfile::new("b50ad385-829d-3141-a216-7e7d7539ba7f", "Tank").expect("uuid"),
        1,
        "minecraft:overworld",
        mc_entity::inventory_for_registry(
            &mc_registry::ItemRegistry::load(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../test-support/fixtures/registry/items.tsv"),
            )
            .expect("items"),
        )
        .expect("inventory"),
    );
    let outcome = player.apply_damage(10.0, mc_entity::combat::DamageSource::MobAttack, &stats);
    assert!(
        (outcome.dealt - 7.9968).abs() < 1e-3,
        "Protection IV must cut the post-armour hit; got {}",
        outcome.dealt
    );
    assert!((player.health - (MAX_HEALTH - 7.9968)).abs() < 1e-3);
}
