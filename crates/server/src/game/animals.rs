//! Farm animals: breeding, babies, tempt, shearing, milking, eggs (P20-06).
//!
//! ## Jar and reference truth
//!
//! - Ages and love timers are Vanilla's `AgeableMob` shape, quoted from the
//!   local reference (Pumpkin-master, GPL-3.0 — numbers only, no code):
//!   bred babies start at `BABY_START_AGE` (-24000, `ageable.rs`), one
//!   feeding grants 600 love ticks (`animal.rs` `set_love_ticks(600)`), and
//!   parents cool down afterwards. A single `age` carries both signs:
//!   negative grows up, positive cools down, zero breeds.
//! - Breeding foods are the 26.x item tags, quoted from Pumpkin's generated
//!   tag table (`pumpkin-data/src/generated/tag.rs`): `cow_food`/`sheep_food`
//!   are `[wheat]`, `pig_food` is `[carrot, potato, beetroot]`, `chicken_food`
//!   is the six seed kinds. The pack's own `tags/item/*_food.json` wins when
//!   present; the table below is the fallback, so breeding survives a world
//!   with no vanilla data (an empty set would switch the whole feature off,
//!   which is a worse degrade than a documented fallback).
//! - Lightning damage (5.0, no knockback, armour applies) is quoted in
//!   `bolt.rs`; the 8-second fire there is a named gap (no fire model).
//! - Sheep shear 1–3 wool, chickens lay every `6000 + nextInt(6000)` ticks,
//!   breeding scatters 1–7 XP: vanilla's documented numbers, labelled as
//!   such (not re-read from the jar in this slice).
//!
//! ## What is and is not here
//!
//! Done: feeding (babies grow 10% closer, adults enter love mode), pair
//! breeding with cooldowns and XP, baby scale (0.5 hitbox) and grow-up,
//! tempt-following, shearing with graze regrow, milking plus milk drinking
//! (clears effects), egg laying, `Age`/`Sheared` persistence. Named gaps:
//! babies render adult-sized (no scale metadata channel, like the sleep
//! pose); creeper charging, villager/witch and pig/piglin conversion on
//! strike; thrown eggs never hatch; love hearts and eat particles (no
//! particle channel); off-hand interaction uses the main hand (the decoded
//! intent carries no hand).

use super::Game;
use mc_entity::mob::MobKind;
use std::collections::{BTreeMap, BTreeSet};

/// How close two adults must stand to breed (a chosen bound: near enough
/// that both farmers can see the pair, far enough that a fence line works).
pub(crate) const BREED_RADIUS: f64 = 8.0;

/// How far a tempting player pulls a passive mob (Vanilla `TemptGoal` range).
pub(crate) const TEMPT_RANGE: f64 = 10.0;

/// Breeding food per kind (jar-mirrored fallback; see module docs).
#[derive(Debug, Clone)]
pub(crate) struct AnimalFoods {
    cow: BTreeSet<String>,
    sheep: BTreeSet<String>,
    pig: BTreeSet<String>,
    chicken: BTreeSet<String>,
}

impl AnimalFoods {
    /// The jar-mirrored fallback, used until a pack installs its tags (and
    /// whenever a pack omits one).
    pub(crate) fn with_fallbacks() -> Self {
        let set = |items: &[&str]| -> BTreeSet<String> {
            items.iter().map(|item| (*item).to_owned()).collect()
        };
        Self {
            cow: set(&["minecraft:wheat"]),
            sheep: set(&["minecraft:wheat"]),
            pig: set(&["minecraft:carrot", "minecraft:potato", "minecraft:beetroot"]),
            chicken: set(&[
                "minecraft:wheat_seeds",
                "minecraft:melon_seeds",
                "minecraft:pumpkin_seeds",
                "minecraft:beetroot_seeds",
                "minecraft:torchflower_seeds",
                "minecraft:pitcher_pod",
            ]),
        }
    }

    /// The pack's `tags/item/*_food.json` sets, falling back per kind.
    ///
    /// A present-but-empty tag is honoured as empty (the pack overrode the
    /// food away); only a *missing* tag falls back. That keeps an explicit
    /// pack choice authoritative while a dataless world still breeds.
    pub(crate) fn install_from_pack(&mut self, tags: &BTreeMap<String, BTreeSet<String>>) {
        let take = |tags: &BTreeMap<String, BTreeSet<String>>, key: &str| tags.get(key).cloned();
        if let Some(set) = take(tags, "minecraft:cow_food") {
            self.cow = set;
        }
        if let Some(set) = take(tags, "minecraft:sheep_food") {
            self.sheep = set;
        }
        if let Some(set) = take(tags, "minecraft:pig_food") {
            self.pig = set;
        }
        if let Some(set) = take(tags, "minecraft:chicken_food") {
            self.chicken = set;
        }
    }

    /// The food set for `kind`, or `None` for unmodelled kinds.
    fn for_kind(&self, kind: MobKind) -> Option<&BTreeSet<String>> {
        match kind {
            MobKind::Cow => Some(&self.cow),
            MobKind::Sheep => Some(&self.sheep),
            MobKind::Pig => Some(&self.pig),
            MobKind::Chicken => Some(&self.chicken),
            _ => None,
        }
    }
}

impl Game {
    /// Install the pack's breeding-food tags (P20-06).
    ///
    /// Called once from the pack loader beside the growth tags; a game that
    /// never loads packs keeps the jar-mirrored fallbacks.
    pub fn set_animal_foods(&mut self, tags: &BTreeMap<String, BTreeSet<String>>) {
        self.animal_foods.install_from_pack(tags);
    }

    /// Whether `item` (a `minecraft:` name) feeds `kind`.
    #[must_use]
    pub(crate) fn is_animal_food(&self, kind: MobKind, item: &str) -> bool {
        self.animal_foods
            .for_kind(kind)
            .is_some_and(|set| set.contains(item))
    }

    /// Feed the mob behind `target` with `item` (P20-06).
    ///
    /// Babies grow 10% closer to adulthood; ready adults enter love mode;
    /// cooling adults and non-foods are refused. Returns whether the item
    /// was accepted — the caller consumes exactly then.
    pub(crate) fn feed_animal(&mut self, target: mc_entity::EntityId, item: &str) -> bool {
        // Read the kind first: the mutation below borrows the store mutably,
        // which does not compose with the food-table read on `self`.
        let Some(kind) = self
            .entities
            .get(target)
            .and_then(|entity| match &entity.body {
                mc_entity::EntityBody::Mob(mob) => Some(mob.kind),
                _ => None,
            })
        else {
            return false;
        };
        if !self.is_animal_food(kind, item) {
            return false;
        }
        let Some(entity) = self.entities.get_mut(target) else {
            return false;
        };
        let mc_entity::EntityBody::Mob(mob) = &mut entity.body else {
            return false;
        };
        if mob.age < 0 {
            // A tenth off the remaining childhood per feed (Vanilla's growth
            // acceleration, minimum one tick so tiny ages still move).
            let step = (-mob.age / 10).max(1);
            mob.age = (mob.age + step).min(0);
            return true;
        }
        if mob.age != 0 {
            return false;
        }
        mob.in_love = mc_entity::mob::LOVE_TICKS;
        true
    }

    /// Shear the sheep behind `target` (P20-06).
    ///
    /// Adults with wool return their 1–3 yield and go bare; everything else
    /// (babies, the already shorn, non-sheep) returns `None` and the shears
    /// stay sharp. The caller drops the wool and wears the shears.
    pub(crate) fn shear_sheep(&mut self, target: mc_entity::EntityId) -> Option<i32> {
        // Eligibility first on a shared borrow: the wool roll below draws
        // from the game's RNG, which does not compose with a live store
        // borrow — and a refusal must not consume randomness either.
        let eligible = self.entities.get(target).is_some_and(|entity| {
            matches!(&entity.body, mc_entity::EntityBody::Mob(mob)
                if mob.kind == MobKind::Sheep && mob.age >= 0 && !mob.sheared)
        });
        if !eligible {
            return None;
        }
        let wool = 1 + self.random.next_i32_bounded(3);
        let entity = self.entities.get_mut(target)?;
        let mc_entity::EntityBody::Mob(mob) = &mut entity.body else {
            return None;
        };
        mob.sheared = true;
        mob.forage_ticks = 0;
        Some(wool)
    }

    /// Milk the cow behind `target` (P20-06).
    ///
    /// Adults give milk; calves do not. The caller exchanges the bucket.
    /// Drinking (which clears effects) is [`Game::drink_milk`].
    pub(crate) fn milk_cow(&mut self, target: mc_entity::EntityId) -> bool {
        let Some(entity) = self.entities.get_mut(target) else {
            return false;
        };
        let mc_entity::EntityBody::Mob(mob) = &mut entity.body else {
            return false;
        };
        mob.kind == MobKind::Cow && mob.age >= 0
    }

    /// Drink the held milk: clear every active effect, hold a bucket after.
    ///
    /// Milk is not food (no `food`/`consumable` components), so the eat path
    /// never sees it — this is its own arm on `UseItem`, before `start_eat`.
    /// Drinking always converts the milk to a bucket (Vanilla's drink, which
    /// never leaves an empty hand behind in either mode).
    pub(crate) fn drink_milk(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        hand: mc_entity::Hand,
        report: &mut super::TickReport,
    ) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.player.effects.clear();
        }
        // The drink converts the milk to a bucket (Vanilla's drink never
        // leaves an empty hand behind, in either mode — like the bucket
        // exchange, this is unconditional).
        self.exchange_held(id, hand, "minecraft:bucket", report);
    }

    /// Players whose hands may tempt a mob this tick: position plus held
    /// item name, resolved once per tick (P20-06).
    ///
    /// Hoisted out of the per-mob tempt check: resolving the registry name
    /// per mob per session would multiply one table probe per session into
    /// one per mob per session. Sessions holding nothing (or an unresolvable
    /// item) never enter the list.
    pub(crate) fn ready_food_holders(&self) -> Vec<(mc_world::Vec3, String)> {
        self.sessions
            .values()
            .filter(|session| session.ready)
            .filter_map(|session| {
                let item_id = session.player.inventory.selected_item().item_id()?;
                let name = self.registries.items.name(item_id).ok()?.to_owned();
                Some((session.player.position, name))
            })
            .collect()
    }

    /// Tempt override for one mob's goal (P20-06).
    ///
    /// A passive mob whose nearest ready player stands within
    /// [`TEMPT_RANGE`] holding its food walks to the player's cell instead
    /// of whatever `decide` chose. The AI's own walk memory is pointed at
    /// the player too, so the follow persists across ticks while the
    /// temptation holds and degrades to an ordinary walk when it ends.
    /// Babies follow like adults (Vanilla tempts both).
    pub(crate) fn tempt_override(
        &mut self,
        id: mc_entity::EntityId,
        kind: MobKind,
        position: mc_world::Vec3,
        goal: mc_entity::mob::MobGoal,
        holders: &[(mc_world::Vec3, String)],
    ) -> mc_entity::mob::MobGoal {
        if !matches!(
            kind,
            MobKind::Cow | MobKind::Pig | MobKind::Sheep | MobKind::Chicken
        ) {
            return goal;
        }
        // The tempting holder: nearest, close, holding food. Holder names
        // arrive resolved (see `ready_food_holders`); this loop only
        // measures, so the sweep costs sessions — not sessions × mobs.
        let mut tempting: Option<(mc_world::Vec3, f64)> = None;
        for (held_pos, held_name) in holders {
            if !self.is_animal_food(kind, held_name) {
                continue;
            }
            let dx = held_pos.x - position.x;
            let dy = held_pos.y - position.y;
            let dz = held_pos.z - position.z;
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            if distance <= TEMPT_RANGE && tempting.is_none_or(|(_, best)| distance < best) {
                tempting = Some((*held_pos, distance));
            }
        }
        let Some((player_pos, _)) = tempting else {
            return goal;
        };
        let cell = (
            player_pos.x.floor() as i32,
            player_pos.y.floor() as i32,
            player_pos.z.floor() as i32,
        );
        if let Some(entity) = self.entities.get_mut(id)
            && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
        {
            mob.ai.wander_target = Some(cell);
            mob.ai.cooldown = mob.ai.cooldown.max(20);
        }
        mc_entity::mob::MobGoal::Wander { target: cell }
    }

    /// One tick of animal life for the whole store (P20-06).
    ///
    /// Runs after the per-entity loop in the Entities phase, on settled
    /// positions: age/in-love timers, pair breeding with XP, chicken eggs,
    /// sheep graze regrow. Pairing is first-found per mob per tick over
    /// ascending ids, so a crowd cannot chain-breed through one tick.
    pub(crate) fn tick_animals(&mut self) {
        // Timers first: growing up, cooling down, falling out of love. Ids
        // collected up front — the store has no mutable iterator, and the
        // loop below only ever holds one entity at a time.
        let ids: Vec<mc_entity::EntityId> = self.entities.ids().collect();
        for id in ids {
            let Some(entity) = self.entities.get_mut(id) else {
                continue;
            };
            let mc_entity::EntityBody::Mob(mob) = &mut entity.body else {
                continue;
            };
            if mob.age < 0 {
                mob.age += 1;
            } else if mob.age > 0 {
                mob.age -= 1;
            }
            if mob.in_love > 0 {
                mob.in_love -= 1;
            }
        }
        // Breeding pairs.
        let mut bred = std::collections::BTreeSet::new();
        let adults: Vec<(mc_entity::EntityId, MobKind, mc_world::Vec3)> = self
            .entities
            .iter()
            .filter_map(|entity| match &entity.body {
                mc_entity::EntityBody::Mob(mob) if mob.is_breedable() => {
                    Some((entity.id, mob.kind, entity.position))
                }
                _ => None,
            })
            .collect();
        for (index, &(a, kind_a, pos_a)) in adults.iter().enumerate() {
            if bred.contains(&a) {
                continue;
            }
            for &(b, kind_b, pos_b) in adults.iter().skip(index + 1) {
                if kind_a != kind_b || bred.contains(&b) {
                    continue;
                }
                let dx = pos_a.x - pos_b.x;
                let dy = pos_a.y - pos_b.y;
                let dz = pos_a.z - pos_b.z;
                if dx * dx + dy * dy + dz * dz > BREED_RADIUS * BREED_RADIUS {
                    continue;
                }
                bred.insert(a);
                bred.insert(b);
                self.breed_pair(a, b, kind_a);
                break;
            }
        }
        // Eggs and grazing ride the same pass (both need the world or the
        // spawner, so neither fits the pure `decide` shape).
        let layers: Vec<mc_entity::EntityId> = self
            .entities
            .iter()
            .filter_map(|entity| match &entity.body {
                mc_entity::EntityBody::Mob(mob) if mob.kind == MobKind::Chicken && mob.age >= 0 => {
                    Some(entity.id)
                }
                _ => None,
            })
            .collect();
        for id in layers {
            self.tick_chicken_egg(id);
        }
        let grazers: Vec<mc_entity::EntityId> = self
            .entities
            .iter()
            .filter_map(|entity| match &entity.body {
                mc_entity::EntityBody::Mob(mob) if mob.kind == MobKind::Sheep && mob.sheared => {
                    Some(entity.id)
                }
                _ => None,
            })
            .collect();
        for id in grazers {
            self.tick_sheep_graze(id);
        }
    }

    /// Breed two adults of one kind (P20-06).
    ///
    /// Both enter cooldown with their love spent; the calf starts at
    /// [`mc_entity::mob::BABY_START_AGE`] between them; 1–7 XP scatters.
    /// Positions are already settled (this runs after the per-entity loop).
    fn breed_pair(&mut self, a: mc_entity::EntityId, b: mc_entity::EntityId, kind: MobKind) {
        let (pos_a, pos_b) = match (self.entities.get(a), self.entities.get(b)) {
            (Some(x), Some(y)) => (x.position, y.position),
            _ => return,
        };
        for id in [a, b] {
            if let Some(entity) = self.entities.get_mut(id)
                && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
            {
                mob.age = mc_entity::mob::BREED_COOLDOWN_TICKS;
                mob.in_love = 0;
            }
        }
        let at = mc_world::Vec3::new(
            f64::midpoint(pos_a.x, pos_b.x),
            f64::midpoint(pos_a.y, pos_b.y),
            f64::midpoint(pos_a.z, pos_b.z),
        );
        let Ok(baby) = self.entities.spawn(
            mc_entity::EntityBody::Mob(mc_entity::mob::Mob::new(kind)),
            at,
        ) else {
            return;
        };
        if let Some(entity) = self.entities.get_mut(baby)
            && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
        {
            mob.age = mc_entity::mob::BABY_START_AGE;
        }
        self.pending_entity_spawns.push(baby);
        let xp = 1 + self.random.next_i32_bounded(7);
        self.scatter_experience(at, xp.max(0) as u32);
    }

    /// One chicken's egg timer (P20-06).
    ///
    /// Adults count down; at zero an egg drops and the timer redraws
    /// `6000 + nextInt(6000)`. New spawns start at the minimum (the game
    /// randomises them at spawn; direct constructions keep the sane floor).
    fn tick_chicken_egg(&mut self, id: mc_entity::EntityId) {
        let at = match self.entities.get_mut(id) {
            Some(entity) => match &mut entity.body {
                mc_entity::EntityBody::Mob(mob) => {
                    if mob.egg_ticks > 0 {
                        mob.egg_ticks -= 1;
                        return;
                    }
                    entity.position
                }
                _ => return,
            },
            None => return,
        };
        let Ok(egg) = self.registries.items.id("minecraft:egg") else {
            return;
        };
        let Ok(stack) = mc_entity::stack::ItemStack::new(egg, 1) else {
            return;
        };
        if self.spawn_item(stack, at).is_err() {
            return;
        }
        if let Some(entity) = self.entities.get_mut(id)
            && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
        {
            mob.egg_ticks = mc_entity::mob::EGG_DELAY_MIN_TICKS
                + self
                    .random
                    .next_i32_bounded(mc_entity::mob::EGG_DELAY_SPREAD_TICKS as i32)
                    as u32;
        }
    }

    /// One shorn sheep's graze timer (P20-06).
    ///
    /// Standing on grass grows the counter; stepping off resets it; at
    /// [`mc_entity::mob::FORAGE_REGROW_TICKS`] the wool is back. A fixed
    /// count standing in for vanilla's graze goal.
    fn tick_sheep_graze(&mut self, id: mc_entity::EntityId) {
        let below = match self.entities.get(id) {
            Some(entity) => {
                let x = entity.position.x.floor() as i32;
                let y = entity.position.y.floor() as i32 - 1;
                let z = entity.position.z.floor() as i32;
                self.world
                    .get_block_loaded(x, y, z)
                    .and_then(|cell| self.registries.blocks.block_name(cell).ok())
                    .is_some_and(|name| name == "minecraft:grass_block")
            }
            None => return,
        };
        if !below {
            if let Some(entity) = self.entities.get_mut(id)
                && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
            {
                mob.forage_ticks = 0;
            }
            return;
        }
        let regrown = match self.entities.get_mut(id) {
            Some(entity) => match &mut entity.body {
                mc_entity::EntityBody::Mob(mob) => {
                    mob.forage_ticks += 1;
                    mob.forage_ticks >= mc_entity::mob::FORAGE_REGROW_TICKS
                }
                _ => false,
            },
            None => false,
        };
        if regrown
            && let Some(entity) = self.entities.get_mut(id)
            && let mc_entity::EntityBody::Mob(mob) = &mut entity.body
        {
            mob.sheared = false;
            mob.forage_ticks = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Mechanism pins for P20-06, called directly (TEST-TIME-PLAN §2).
    //!
    //! In-crate, so mobs are manipulated the way saves do (fields, not
    //! packets): no players, no streaming, no ticks except the explicit
    //! `tick_animals` calls. The click paths that reach these mechanisms
    //! live in `tests/animals.rs`.

    use super::Game;
    use crate::storage::WorldService;
    use mc_entity::mob::MobKind;
    use mc_world::ChunkPos;

    /// A game with owned storage on a scratch dir (no players, one chunk).
    fn bare_game(tag: &str) -> (Game, mc_test_support::fixtures::TempDir) {
        let dir = mc_test_support::fixtures::TempDir::new(tag);
        let config = crate::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (_tx, rx) = mc_network::bridge::game_channel(64);
        let game = Game::with_seed_and_storage(storage, 2, rx, crate::game::DEFAULT_RANDOM_SEED)
            .expect("game builds");
        (game, dir)
    }

    fn spawn_cow(game: &mut Game, x: f64, y: f64, z: f64) -> mc_entity::EntityId {
        game.spawn_mob(MobKind::Cow, mc_world::Vec3::new(x, y, z))
            .expect("cow spawns")
    }

    fn mob_age(game: &Game, id: mc_entity::EntityId) -> i32 {
        match &game.entities.get(id).expect("mob").body {
            mc_entity::EntityBody::Mob(mob) => mob.age,
            _ => panic!("not a mob"),
        }
    }

    fn set_age(game: &mut Game, id: mc_entity::EntityId, age: i32) {
        match &mut game.entities.get_mut(id).expect("mob").body {
            mc_entity::EntityBody::Mob(mob) => mob.age = age,
            _ => panic!("not a mob"),
        }
    }

    #[test]
    fn food_table_falls_back_and_pack_overrides() {
        let (mut game, _dir) = bare_game("food-unit");
        assert!(
            game.is_animal_food(MobKind::Cow, "minecraft:wheat"),
            "cows eat wheat by default"
        );
        assert!(
            !game.is_animal_food(MobKind::Cow, "minecraft:stone"),
            "cows refuse stone"
        );
        assert!(
            game.is_animal_food(MobKind::Pig, "minecraft:carrot"),
            "pigs eat carrots by default"
        );
        assert!(
            game.is_animal_food(MobKind::Chicken, "minecraft:wheat_seeds"),
            "chickens eat seeds by default"
        );
        assert!(
            !game.is_animal_food(MobKind::Zombie, "minecraft:wheat"),
            "unmodelled kinds have no foods"
        );
        // A pack tag overrides; a missing tag keeps its fallback.
        let mut tags = std::collections::BTreeMap::new();
        tags.insert(
            "minecraft:cow_food".to_owned(),
            ["minecraft:diamond".to_owned()].into_iter().collect(),
        );
        game.set_animal_foods(&tags);
        assert!(
            !game.is_animal_food(MobKind::Cow, "minecraft:wheat"),
            "a present tag replaces the fallback"
        );
        assert!(
            game.is_animal_food(MobKind::Cow, "minecraft:diamond"),
            "with the pack's set"
        );
        assert!(
            game.is_animal_food(MobKind::Sheep, "minecraft:wheat"),
            "an absent tag keeps its fallback"
        );
    }

    #[test]
    fn feeding_sets_love_refuses_cooldown_and_grows_babies() {
        let (mut game, _dir) = bare_game("feed-unit");
        let cow = spawn_cow(&mut game, 8.5, 121.0, 8.5);
        assert!(
            game.feed_animal(cow, "minecraft:wheat"),
            "wheat feeds a ready cow"
        );
        assert_eq!(
            match &game.entities.get(cow).expect("cow").body {
                mc_entity::EntityBody::Mob(mob) => mob.in_love,
                _ => 0,
            },
            mc_entity::mob::LOVE_TICKS,
            "love lasts the full window"
        );
        assert!(
            !game.feed_animal(cow, "minecraft:stone"),
            "stone is refused"
        );
        set_age(&mut game, cow, -100);
        assert!(
            game.feed_animal(cow, "minecraft:wheat"),
            "babies take food too"
        );
        assert_eq!(mob_age(&game, cow), -90, "a tenth off the childhood");
        set_age(&mut game, cow, 5);
        assert!(
            !game.feed_animal(cow, "minecraft:wheat"),
            "cooling adults are refused (neutralise the age gate and food lands mid-cooldown)"
        );
    }

    #[test]
    fn breeding_makes_a_calf_and_cools_the_parents() {
        let (mut game, _dir) = bare_game("breed-unit");
        let a = spawn_cow(&mut game, 8.5, 121.0, 8.5);
        let b = spawn_cow(&mut game, 10.5, 121.0, 8.5);
        assert!(game.feed_animal(a, "minecraft:wheat"), "first fed");
        assert!(game.feed_animal(b, "minecraft:wheat"), "second fed");
        game.tick_animals();
        // The calf: baby-aged, between the parents.
        let calves: Vec<_> = game
            .entities
            .iter()
            .filter(|entity| {
                matches!(&entity.body, mc_entity::EntityBody::Mob(mob)
                    if mob.kind == MobKind::Cow && mob.age < 0)
            })
            .collect();
        assert_eq!(calves.len(), 1, "one calf is born");
        assert_eq!(
            mob_age(&game, calves[0].id),
            mc_entity::mob::BABY_START_AGE,
            "at the vanilla starting age"
        );
        // The parents: cooling, love spent, exactly once (no chain: a
        // second calf would need a third adult).
        for parent in [a, b] {
            assert_eq!(
                mob_age(&game, parent),
                mc_entity::mob::BREED_COOLDOWN_TICKS,
                "parents cool down"
            );
        }
        let orbs = game
            .entities
            .iter()
            .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Orb(_)))
            .count();
        assert!(
            orbs > 0,
            "breeding scatters XP (neutralise the breed arm and no calf, no orbs)"
        );
    }

    #[test]
    fn distant_adults_do_not_breed() {
        let (mut game, _dir) = bare_game("breed-far-unit");
        let a = spawn_cow(&mut game, 8.5, 121.0, 8.5);
        let b = spawn_cow(&mut game, 60.5, 121.0, 8.5);
        assert!(game.feed_animal(a, "minecraft:wheat"), "first fed");
        assert!(game.feed_animal(b, "minecraft:wheat"), "second fed");
        for _ in 0..5 {
            game.tick_animals();
        }
        assert_eq!(
            game.entities
                .iter()
                .filter(|entity| matches!(&entity.body, mc_entity::EntityBody::Mob(_)))
                .count(),
            2,
            "twenty blocks apart is no meeting"
        );
    }

    #[test]
    fn timers_age_cool_and_fade() {
        let (mut game, _dir) = bare_game("timers-unit");
        let cow = spawn_cow(&mut game, 8.5, 121.0, 8.5);
        set_age(&mut game, cow, -10);
        for _ in 0..10 {
            game.tick_animals();
        }
        assert_eq!(mob_age(&game, cow), 0, "ten ticks grow ten ages");
        set_age(&mut game, cow, 5);
        for _ in 0..5 {
            game.tick_animals();
        }
        assert_eq!(mob_age(&game, cow), 0, "cooldowns count down too");
        assert!(game.feed_animal(cow, "minecraft:wheat"), "fed");
        for _ in 0..mc_entity::mob::LOVE_TICKS {
            game.tick_animals();
        }
        assert!(
            !game
                .entities
                .get(cow)
                .is_some_and(|entity| matches!(&entity.body,
                mc_entity::EntityBody::Mob(mob) if mob.in_love > 0)),
            "love fades after its window"
        );
    }

    #[test]
    fn eggs_drop_on_schedule_and_reset() {
        let (mut game, _dir) = bare_game("egg-unit");
        let hen = game
            .spawn_mob(MobKind::Chicken, mc_world::Vec3::new(8.5, 121.0, 8.5))
            .expect("hen spawns");
        match &mut game.entities.get_mut(hen).expect("hen").body {
            mc_entity::EntityBody::Mob(mob) => mob.egg_ticks = 2,
            _ => panic!("not a mob"),
        }
        for _ in 0..3 {
            game.tick_animals();
        }
        let eggs = game
            .entities
            .iter()
            .filter(|entity| matches!(&entity.body, mc_entity::EntityBody::Item(_)))
            .count();
        assert_eq!(eggs, 1, "one timer expiry lays one egg");
        let timer = match &game.entities.get(hen).expect("hen").body {
            mc_entity::EntityBody::Mob(mob) => mob.egg_ticks,
            _ => 0,
        };
        assert!(
            timer >= mc_entity::mob::EGG_DELAY_MIN_TICKS,
            "the timer redraws long (neutralise the lay arm and nothing drops)"
        );
    }

    #[test]
    fn shearing_yields_wool_and_grazing_regrows_it() {
        let (mut game, _dir) = bare_game("shear-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let grass = game
            .registries()
            .blocks
            .default_state("minecraft:grass_block")
            .expect("grass");
        game.world_mut()
            .set_block(8, 120, 8, grass)
            .expect("pasture placed");
        let sheep = game
            .spawn_mob(MobKind::Sheep, mc_world::Vec3::new(8.5, 121.0, 8.5))
            .expect("sheep spawns");
        let wool = game.shear_sheep(sheep).expect("a fleeced sheep gives wool");
        assert!((1..=3).contains(&wool), "1–3 wool per shearing, got {wool}");
        assert!(
            game.shear_sheep(sheep).is_none(),
            "the shorn sheep is bare (neutralise the flag and it shears forever)"
        );
        for _ in 0..mc_entity::mob::FORAGE_REGROW_TICKS {
            game.tick_animals();
        }
        assert!(
            game.shear_sheep(sheep).is_some(),
            "a hundred grass ticks regrow the wool"
        );
    }

    #[test]
    fn shearing_refuses_babies_bare_and_beasts() {
        let (mut game, _dir) = bare_game("shear-no-unit");
        let lamb = game
            .spawn_mob(MobKind::Sheep, mc_world::Vec3::new(8.5, 121.0, 8.5))
            .expect("lamb spawns");
        set_age(&mut game, lamb, -100);
        assert!(game.shear_sheep(lamb).is_none(), "lambs keep their wool");
        let zombie = game
            .spawn_mob(MobKind::Zombie, mc_world::Vec3::new(10.5, 121.0, 8.5))
            .expect("zombie spawns");
        assert!(
            game.shear_sheep(zombie).is_none(),
            "the undead are not sheep"
        );
    }

    #[test]
    fn milking_takes_adults_only() {
        let (mut game, _dir) = bare_game("milk-unit");
        let cow = spawn_cow(&mut game, 8.5, 121.0, 8.5);
        assert!(game.milk_cow(cow), "an adult cow gives milk");
        set_age(&mut game, cow, -100);
        assert!(!game.milk_cow(cow), "calves give none");
        let zombie = game
            .spawn_mob(MobKind::Zombie, mc_world::Vec3::new(10.5, 121.0, 8.5))
            .expect("zombie spawns");
        assert!(
            !game.milk_cow(zombie),
            "and neither do zombies (neutralise the kind gate and everything milks)"
        );
    }
}
