//! Farm animals through player-visible paths: feeding, milking, shearing,
//! tempting, birth, and persistence (P20-06).
//!
//! The mechanisms are pinned by direct calls in `game::animals::tests`; this
//! file drives the same entries the way a client does — right-clicks,
//! uses, ticks, saves — plus the two properties only a client (or a
//! reloaded chunk) can see: hitbox scale and restart survival. Fast
//! harnesses (view distance 2, TEST-TIME-PLAN §3) throughout.

use mc_entity::mob::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::PlayIntent;
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
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::build_with_operators(
            None,
            Some(storage),
            2,
            rx,
            DEFAULT_RANDOM_SEED,
            ops_for("Chief", 4),
        )
        .expect("game builds");
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

    fn command(&mut self, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let mut report = mc_server::game::TickReport::default();
        self.game
            .dispatch_command(self.id, text, &mut report)
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

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    /// Put one `item` in the held slot (hotbar slot 0).
    fn give(&mut self, item: &str) {
        let item_id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(item_id, 1).expect("stack"),
            )
            .expect("slot 0 takes it");
    }

    fn held_name(&self) -> Option<String> {
        let held = self
            .game
            .player(self.id)
            .expect("player")
            .inventory
            .selected_item()
            .item_id()?;
        self.game
            .registries()
            .items
            .name(held)
            .ok()
            .map(str::to_owned)
    }

    /// Right-click the entity `target` (use action).
    fn interact(&mut self, target: mc_entity::EntityId) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::Interact {
                    entity: target.get(),
                    kind: 0,
                }),
            })
            .expect("interact queued");
        self.game.tick().expect("tick");
    }

    /// Use the held item (drink, eat).
    fn use_item(&mut self) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(PlayIntent::UseItem {
                    sequence: 0,
                    hand: 0,
                    yaw: 0.0,
                    pitch: 0.0,
                }),
            })
            .expect("use queued");
        self.game.tick().expect("tick");
    }

    /// The live mob of `kind`, if exactly one is around.
    fn mob_of_kind(&self, kind: MobKind) -> Vec<mc_entity::EntityId> {
        self.game
            .entity_store()
            .iter()
            .filter(|entity| {
                matches!(&entity.body, mc_entity::EntityBody::Mob(mob) if mob.kind == kind)
            })
            .map(|entity| entity.id)
            .collect()
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

/// Feeding two cows breeds a calf that stands half-scale.
#[test]
fn feeding_two_cows_breeds_a_small_calf() {
    let mut harness = Harness::new("animal-birth");
    let mut out = harness.join("Chief");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    // No wild courtships: natural spawns would add uninvited parents.
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(8.5, 121.0, 8.5))
        .expect("first cow spawns");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(10.5, 121.0, 8.5))
        .expect("second cow spawns");
    harness.stand(9, 121, 8);
    harness.run(1);
    let parents = harness.mob_of_kind(MobKind::Cow);
    assert_eq!(parents.len(), 2, "two cows to begin with");
    harness.give("minecraft:wheat");
    harness.interact(parents[0]);
    harness.give("minecraft:wheat");
    harness.interact(parents[1]);
    harness.run(5);
    let herd = harness.mob_of_kind(MobKind::Cow);
    assert_eq!(
        herd.len(),
        3,
        "a calf is born (neutralise the breed arm and the herd stays two)"
    );
    // The calf reads smaller than either parent: babies ride at half scale.
    let mut sizes: Vec<f64> = herd
        .iter()
        .map(|id| {
            let entity = harness.game.entity_store().get(*id).expect("cow");
            let hitbox = entity.hitbox();
            hitbox.max_y - hitbox.min_y
        })
        .collect();
    sizes.sort_by(f64::total_cmp);
    assert!(
        sizes[0] * 2.0 <= sizes[2] + 0.01,
        "the calf stands half-scale, saw {sizes:?}"
    );
}

/// Milking a cow, then drinking, clears every effect.
#[test]
fn milk_cures_what_ails() {
    let mut harness = Harness::new("animal-milk");
    let mut out = harness.join("Chief");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(8.5, 121.0, 8.5))
        .expect("cow spawns");
    harness.stand(9, 121, 8);
    harness.run(1);
    let lines = harness.command(&mut out, "effect give Chief poison 30 1");
    assert!(
        lines.iter().any(|line| line.contains("poison")),
        "poison lands, saw {lines:?}"
    );
    assert!(
        !harness
            .game
            .player(harness.id)
            .expect("player")
            .effects
            .is_empty(),
        "the effect is live"
    );
    let cows = harness.mob_of_kind(MobKind::Cow);
    harness.give("minecraft:bucket");
    harness.interact(cows[0]);
    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:milk_bucket"),
        "the bucket comes back full (neutralise the milk arm and it stays empty)"
    );
    harness.use_item();
    assert!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .effects
            .is_empty(),
        "drinking clears every effect"
    );
    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:bucket"),
        "and leaves the bucket behind"
    );
}

/// Shearing drops wool; grazing brings it back.
#[test]
fn shearing_drops_wool_and_grazing_regrows_it() {
    let mut harness = Harness::new("animal-shear");
    let mut out = harness.join("Chief");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    // A grass pasture under the sheep: regrow needs grass below.
    let blocks = harness.game.registries().blocks.clone();
    let grass = blocks
        .default_state("minecraft:grass_block")
        .expect("grass");
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    for x in 5..=12 {
        for z in 5..=12 {
            harness
                .game
                .world_mut()
                .set_block(x, 120, z, grass)
                .expect("pasture placed");
        }
    }
    // A stone pen around (8, 8): wandering would walk the sheep off its
    // grass and reset the graze counter, which is a different test's
    // subject (unit-pinned: stepping off resets).
    for x in 6..=10 {
        for z in [6, 10] {
            harness
                .game
                .world_mut()
                .set_block(x, 121, z, stone)
                .expect("pen placed");
        }
        for z in 7..=9 {
            harness
                .game
                .world_mut()
                .set_block(6, 121, z, stone)
                .expect("pen placed");
            harness
                .game
                .world_mut()
                .set_block(10, 121, z, stone)
                .expect("pen placed");
        }
    }
    harness
        .game
        .spawn_mob(MobKind::Sheep, mc_world::Vec3::new(8.5, 121.0, 8.5))
        .expect("sheep spawns");
    harness.stand(9, 121, 8);
    harness.run(1);
    let sheep = harness.mob_of_kind(MobKind::Sheep);
    harness.give("minecraft:shears");
    harness.interact(sheep[0]);
    harness.run(2);
    let wool: i32 = harness
        .game
        .entity_store()
        .iter()
        .filter_map(|entity| match &entity.body {
            mc_entity::EntityBody::Item(item) => {
                let name = harness
                    .game
                    .registries()
                    .items
                    .name(item.stack.item_id()?)
                    .ok()?;
                (name == "minecraft:white_wool").then(|| item.stack.count())
            }
            _ => None,
        })
        .sum();
    assert!(
        (1..=3).contains(&wool),
        "one shearing drops 1–3 wool, saw {wool}"
    );
    // A hundred grass ticks regrow it: shearing again yields wool.
    harness.run(120);
    let sheep = harness.mob_of_kind(MobKind::Sheep);
    assert_eq!(sheep.len(), 1, "the sheep is still around");
    harness.give("minecraft:shears");
    harness.interact(sheep[0]);
    harness.run(2);
    let wool: i32 = harness
        .game
        .entity_store()
        .iter()
        .filter_map(|entity| match &entity.body {
            mc_entity::EntityBody::Item(item) => {
                let name = harness
                    .game
                    .registries()
                    .items
                    .name(item.stack.item_id()?)
                    .ok()?;
                (name == "minecraft:white_wool").then(|| item.stack.count())
            }
            _ => None,
        })
        .sum();
    assert!(
        wool >= 1,
        "grazing regrew the wool (neutralise the graze arm and the second shearing is bare)"
    );
}

/// A wheat-holder pulls a cow across the pen — and back.
#[allow(
    clippy::cast_possible_truncation,
    reason = "arena coordinates are single-digit; the floor keeps them exact"
)]
#[test]
fn tempted_cow_follows_wheat() {
    let mut harness = Harness::new("animal-tempt");
    let mut out = harness.join("Chief");
    for chunk in [
        ChunkPos::new(-1, 0),
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(2, 0),
        ChunkPos::new(-1, 1),
        ChunkPos::new(0, 1),
        ChunkPos::new(1, 1),
        ChunkPos::new(2, 1),
    ] {
        assert!(harness.game.load_chunk(chunk), "arena chunk loads");
    }
    // A walled dirt arena at y140 (above all terrain): nothing wanders off,
    // nothing falls, and the two legs stay on one level. The wander that
    // ruined the open-road version — a sidestep off a one-wide road and a
    // 56-block fall out of tempt range — has nowhere to happen here.
    let blocks = harness.game.registries().blocks.clone();
    let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    for x in -12..=32 {
        for z in 2..=14 {
            harness
                .game
                .world_mut()
                .set_block(x, 140, z, dirt)
                .expect("floor placed");
        }
    }
    for y in 141..=142 {
        for x in -12..=32 {
            for z in [2, 14] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, stone)
                    .expect("wall placed");
            }
        }
        for z in 3..=13 {
            for x in [-12, 32] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, stone)
                    .expect("wall placed");
            }
        }
    }
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(8.5, 141.0, 8.5))
        .expect("cow spawns");
    harness.stand(20, 141, 8);
    harness.give("minecraft:wheat");
    harness.run(1);
    let cow = harness.mob_of_kind(MobKind::Cow)[0];
    let position = |harness: &Harness| {
        harness
            .game
            .entity_store()
            .get(cow)
            .expect("cow")
            .position
            .x
    };
    let start = position(&harness);
    // Leg one: the holder stands east, the cow must come east.
    harness.run(150);
    let east = position(&harness);
    assert!(
        east - start >= 2.0,
        "the cow walks toward the wheat ({start} → {east}; neutralise the tempt arm and it wanders)"
    );
    // Leg two: the holder steps just west of the cow, and the cow turns
    // around. A wanderer does not reverse with the player — only a follower
    // does. (Within tempt range: a teleport across the map would read as
    // the food vanishing, and wandering is the honest answer to that.)
    harness.stand(east.floor() as i32 - 8, 141, 8);
    harness.run(150);
    let west = position(&harness);
    assert!(
        east - west >= 2.0,
        "the cow follows the holder back west ({east} → {west})"
    );
}

/// Calves stay small and shorn sheep stay bare across a restart.
#[test]
fn breeding_survives_restart() {
    let mut harness = Harness::new("animal-restart");
    let mut out = harness.join("Chief");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    harness.command(&mut out, "gamerule spawn_mobs false");
    harness.command(&mut out, "gamerule spawn_monsters false");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(8.5, 121.0, 8.5))
        .expect("first cow spawns");
    harness
        .game
        .spawn_mob(MobKind::Cow, mc_world::Vec3::new(10.5, 121.0, 8.5))
        .expect("second cow spawns");
    harness
        .game
        .spawn_mob(MobKind::Sheep, mc_world::Vec3::new(14.5, 121.0, 8.5))
        .expect("sheep spawns");
    harness.stand(9, 121, 8);
    harness.run(1);
    let cows = harness.mob_of_kind(MobKind::Cow);
    harness.give("minecraft:wheat");
    harness.interact(cows[0]);
    harness.give("minecraft:wheat");
    harness.interact(cows[1]);
    let sheep = harness.mob_of_kind(MobKind::Sheep);
    harness.give("minecraft:shears");
    harness.interact(sheep[0]);
    harness.run(5);
    assert_eq!(
        harness.mob_of_kind(MobKind::Cow).len(),
        3,
        "the calf is born before the save"
    );
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
    // Three cows, one of them still small: `Age` round-tripped.
    let cows: Vec<_> = second
        .entity_store()
        .iter()
        .filter(|entity| {
            matches!(&entity.body, mc_entity::EntityBody::Mob(mob) if mob.kind == MobKind::Cow)
        })
        .collect();
    assert_eq!(cows.len(), 3, "the whole herd comes back");
    let mut heights: Vec<f64> = cows
        .iter()
        .map(|entity| {
            let hitbox = entity.hitbox();
            hitbox.max_y - hitbox.min_y
        })
        .collect();
    heights.sort_by(f64::total_cmp);
    assert!(
        heights[0] * 2.0 <= heights[2] + 0.01,
        "the calf is still a calf after the restart, saw {heights:?}"
    );
    // And the sheep is still bare: `Sheared` round-tripped.
    let sheep: Vec<_> = second
        .entity_store()
        .iter()
        .filter(|entity| {
            matches!(&entity.body, mc_entity::EntityBody::Mob(mob) if mob.kind == MobKind::Sheep)
        })
        .map(|entity| entity.id)
        .collect();
    assert_eq!(sheep.len(), 1, "the sheep comes back");
    let bare = match &second.entity_store().get(sheep[0]).expect("sheep").body {
        mc_entity::EntityBody::Mob(mob) => mob.sheared,
        _ => false,
    };
    assert!(bare, "still shorn after the restart");
}
