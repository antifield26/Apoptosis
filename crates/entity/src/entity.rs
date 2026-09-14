//! Entity identity, lifecycle and the entity store (P05-03).
//!
//! ## Why an enum and not a trait object
//!
//! Vanilla models entities with class inheritance. A Rust port can mirror that
//! with `dyn Entity`, and many do. This project does not, for three reasons that
//! apply here specifically:
//!
//! 1. **Determinism.** Gameplay iterates every entity each tick. A
//!    `BTreeMap<EntityId, Entity>` gives ascending-id order for free; a
//!    `Vec<Box<dyn Entity>>` invites insertion-order or pointer-order iteration,
//!    which is exactly the accidental nondeterminism AGENTS.md §3.6 forbids.
//! 2. **No speculative extensibility.** The set of entity kinds is closed and
//!    known (player, dropped item, the mobs we implement, the projectiles we
//!    implement). AGENTS.md §3.4 says an abstraction must be justified by current
//!    need; there is no plugin that can add a kind before P09.
//! 3. **Borrowing.** Per-tick collision needs to read the world while mutating an
//!    entity. A concrete enum makes that a plain split borrow; `dyn` would need
//!    interior mutability or a query system.
//!
//! The trade-off is accepted and recorded in [ADR-0003](../../docs/adr/ADR-0003-simulation-layering.md): adding a mob means adding
//! a variant, not a new impl. If the plugin boundary (P09-12) ever needs
//! third-party entity kinds, that ADR is where the decision is revisited.
//!
//! ## Identity
//!
//! [`EntityId`] is the number sent on the wire (`add_entity`, `set_entity_data`,
//! `remove_entities`). Ids are allocated ascending from 1 and are **not reused**
//! while the server runs, so a client can never see a stale id refer to a new
//! entity — the classic source of visual ghosts.

use crate::player::Vec3;
use mc_core::error::{ServerError, ServerResult};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Wire identity of a live entity.
///
/// Positive by construction: Vanilla clients treat 0 and negatives specially, and
/// reusing an id would let a stale client reference resolve to a different entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(i32);

impl EntityId {
    /// Wrap a raw wire id, rejecting non-positive values.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `raw <= 0`.
    pub fn new(raw: i32) -> ServerResult<Self> {
        if raw <= 0 {
            return Err(ServerError::InvalidAction(format!(
                "entity id {raw} is not positive"
            )));
        }
        Ok(Self(raw))
    }

    /// The wire number.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "entity#{}", self.0)
    }
}

/// What an entity is, without its per-kind payload.
///
/// Used for filtering, spawn rules, packet dispatch and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityKind {
    /// A connected player.
    Player,
    /// A dropped item stack.
    Item,
    /// A mob (any of [`MobKind`]).
    Mob,
    /// A projectile (any of [`ProjectileKind`]).
    Projectile,
}

impl EntityKind {
    /// Stable name for logs and metrics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Item => "item",
            Self::Mob => "mob",
            Self::Projectile => "projectile",
        }
    }

    /// Whether this kind is a living entity (has health, takes damage).
    #[must_use]
    pub const fn is_living(self) -> bool {
        matches!(self, Self::Player | Self::Mob)
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Per-kind payload.
///
/// `Player` carries nothing: the authoritative player state lives in
/// [`crate::player::Player`], owned by the game loop and keyed by connection, so
/// duplicating it here would create two sources of truth.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityBody {
    /// A player's presence marker; state lives in `Player`.
    Player,
    /// A dropped item.
    Item(crate::item_entity::ItemEntity),
    /// A mob.
    Mob(crate::mob::Mob),
    /// A projectile.
    Projectile(crate::projectile::Projectile),
}

impl EntityBody {
    /// Which kind this payload is.
    #[must_use]
    pub const fn kind(&self) -> EntityKind {
        match self {
            Self::Player => EntityKind::Player,
            Self::Item(_) => EntityKind::Item,
            Self::Mob(_) => EntityKind::Mob,
            Self::Projectile(_) => EntityKind::Projectile,
        }
    }
}

/// One live entity.
///
/// Position is the **feet centre** for living entities and the stack's centre for
/// dropped items, matching the client's own convention for the hitbox origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    /// Wire identity.
    pub id: EntityId,
    /// Per-kind payload.
    pub body: EntityBody,
    /// Current position.
    pub position: Vec3,
    /// Current velocity (blocks per tick).
    pub velocity: Vec3,
    /// Horizontal facing in degrees.
    pub yaw: f32,
    /// Vertical facing in degrees.
    pub pitch: f32,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
    /// Ticks this entity has existed.
    pub age: u64,
    /// Current health (living entities only).
    pub health: f32,
    /// Ticks of damage immunity remaining.
    ///
    /// Vanilla grants 10 ticks (0.5 s) after a hit; without it a mob standing in
    /// fire takes a hit every tick and dies instantly.
    pub invulnerable_ticks: u32,
    /// Ticks of fire remaining (visual + damage).
    pub fire_ticks: u32,
    /// Ticks of air remaining before drowning damage starts.
    pub air_ticks: u32,
    /// Active status effects, keyed by effect id (ascending, deterministic).
    pub effects: BTreeMap<i32, crate::effect::ActiveEffect>,
    /// Set when the entity should be removed at the end of the tick.
    pub removed: bool,
}

impl Entity {
    /// A new entity at `position`.
    #[must_use]
    pub fn new(id: EntityId, body: EntityBody, position: Vec3) -> Self {
        // Non-living entities report zero health: they are never damaged, so the
        // field is unused for them rather than meaningful.
        let health = match &body {
            EntityBody::Player => crate::player::MAX_HEALTH,
            EntityBody::Mob(mob) => mob.kind.max_health(),
            EntityBody::Item(_) | EntityBody::Projectile(_) => 0.0,
        };
        Self {
            id,
            body,
            position,
            velocity: Vec3::default(),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            age: 0,
            health,
            invulnerable_ticks: 0,
            fire_ticks: 0,
            air_ticks: AIR_TICKS,
            effects: BTreeMap::new(),
            removed: false,
        }
    }

    /// What kind this entity is.
    #[must_use]
    pub const fn kind(&self) -> EntityKind {
        self.body.kind()
    }

    /// Whether this entity is alive (non-living entities are always "alive"
    /// until removed).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.kind().is_living() || self.health > 0.0
    }

    /// Bounding box for collision, sized by kind.
    #[must_use]
    pub fn hitbox(&self) -> mc_world::Aabb {
        match &self.body {
            EntityBody::Player => mc_world::Aabb::player(mc_world::Vec3::new(
                self.position.x,
                self.position.y,
                self.position.z,
            )),
            // Items and projectiles are small cubes in this baseline: Vanilla
            // uses 0.25 for both, and neither is ever a collision partner for a
            // player in Phase 05, so one arm covers them.
            EntityBody::Item(_) | EntityBody::Projectile(_) => mc_world::Aabb::sized(
                mc_world::Vec3::new(self.position.x, self.position.y, self.position.z),
                0.25,
                0.25,
            ),
            EntityBody::Mob(mob) => {
                let (width, height) = mob.kind.dimensions();
                mc_world::Aabb::sized(
                    mc_world::Vec3::new(self.position.x, self.position.y, self.position.z),
                    f64::from(width),
                    f64::from(height),
                )
            }
        }
    }

    /// Squared distance from another position, for radius queries.
    #[must_use]
    pub fn distance_squared(&self, other: Vec3) -> f64 {
        let dx = self.position.x - other.x;
        let dy = self.position.y - other.y;
        let dz = self.position.z - other.z;
        dx * dx + dy * dy + dz * dz
    }

    /// Apply a velocity impulse, clamped to a sane magnitude.
    ///
    /// The clamp is a security measure as much as a physical one: knockback derived
    /// from client-influenced state must not be able to fling an entity across the
    /// world (AGENTS.md §10).
    pub fn add_velocity(&mut self, delta: Vec3) {
        self.velocity = Vec3::new(
            (self.velocity.x + delta.x).clamp(-MAX_VELOCITY, MAX_VELOCITY),
            (self.velocity.y + delta.y).clamp(-MAX_VELOCITY, MAX_VELOCITY),
            (self.velocity.z + delta.z).clamp(-MAX_VELOCITY, MAX_VELOCITY),
        );
    }

    /// Ticks down the per-entity timers by one.
    pub fn tick_timers(&mut self) {
        self.age = self.age.saturating_add(1);
        self.invulnerable_ticks = self.invulnerable_ticks.saturating_sub(1);
        self.fire_ticks = self.fire_ticks.saturating_sub(1);
        self.air_ticks = self.air_ticks.saturating_sub(1);
        self.effects.retain(|_, effect| {
            effect.duration = effect.duration.saturating_sub(1);
            effect.duration > 0
        });
    }
}

/// Ticks of breath a fresh entity starts with (Vanilla: 300 = 15 s).
pub const AIR_TICKS: u32 = 300;

/// Largest velocity component an impulse may produce (blocks per tick).
///
/// Vanilla's terminal velocity is about 3.9 blocks/tick; 10 leaves room for
/// legitimate launches while refusing anything that would cross a view distance in
/// one tick.
pub const MAX_VELOCITY: f64 = 10.0;

/// The dimension's live entities.
///
/// `BTreeMap` so iteration is ascending-id, which makes every phase that walks the
/// entity list reproducible.
#[derive(Debug, Clone, Default)]
pub struct EntityStore {
    next_id: i32,
    entities: BTreeMap<EntityId, Entity>,
    /// The world seed entity identities are derived from. See [`EntityStore::uuid`].
    seed: i64,
}

impl EntityStore {
    /// An empty store. Ids start at 1, and identities are derived from seed `0`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            entities: BTreeMap::new(),
            seed: 0,
        }
    }

    /// An empty store whose entity identities belong to the given world seed.
    #[must_use]
    pub fn with_seed(seed: i64) -> Self {
        Self {
            next_id: 1,
            entities: BTreeMap::new(),
            seed,
        }
    }

    /// The world seed this store derives identities from.
    #[must_use]
    pub const fn seed(&self) -> i64 {
        self.seed
    }

    /// The UUID of a live entity, derived from the store's seed and the entity's id.
    ///
    /// Returns `None` for an id that is not live, so a caller cannot label a packet with the identity of an
    /// entity that no longer exists.
    ///
    /// **Derived, not stored**, and not random: the engineering contract requires that the same initial state and
    /// the same ordered inputs over the same tick count produce the same normalized state, and a random UUID per
    /// spawn would differ between two runs of one script in a field a client sees. See
    /// [`crate::identity::entity_uuid`] for why the seed is mixed in rather than a bare counter.
    #[must_use]
    pub fn uuid(&self, id: EntityId) -> Option<Uuid> {
        self.entities
            .contains_key(&id)
            .then(|| crate::identity::entity_uuid(self.seed, id.get()))
    }

    /// Number of live entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether there are no entities.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Id that the next spawn will use.
    #[must_use]
    pub const fn next_id(&self) -> EntityId {
        // `next_id` only ever advances by one per spawn from a positive start, and
        // spawns are bounded by the entity cap, so this stays positive.
        EntityId(self.next_id)
    }

    /// Spawn an entity, returning its new id.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the id space or the entity cap is exhausted.
    /// Both are unreachable in practice (2^31 ids, [`MAX_ENTITIES`] concurrent) but
    /// are refused rather than allowed to wrap.
    pub fn spawn(&mut self, body: EntityBody, position: Vec3) -> ServerResult<EntityId> {
        if self.entities.len() >= MAX_ENTITIES {
            return Err(ServerError::Invariant(format!(
                "entity cap of {MAX_ENTITIES} reached; refusing to spawn more"
            )));
        }
        if self.next_id <= 0 {
            return Err(ServerError::Invariant(
                "entity id space exhausted".to_owned(),
            ));
        }
        let id = EntityId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.entities.insert(id, Entity::new(id, body, position));
        Ok(id)
    }

    /// Borrow an entity.
    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    /// Mutably borrow an entity.
    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.get_mut(&id)
    }

    /// Whether an id is live.
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.entities.contains_key(&id)
    }

    /// Remove an entity, returning it.
    ///
    /// Ids are never recycled, so a removal cannot alias a future spawn.
    pub fn remove(&mut self, id: EntityId) -> Option<Entity> {
        self.entities.remove(&id)
    }

    /// Every entity, ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// Every entity id, ascending.
    pub fn ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entities.keys().copied()
    }

    /// Entities of one kind, ascending by id, as a freshly collected list.
    ///
    /// Collected rather than iterated lazily because callers almost always mutate
    /// while walking; the collection is bounded by [`MAX_ENTITIES`].
    #[must_use]
    pub fn of_kind(&self, kind: EntityKind) -> Vec<EntityId> {
        self.entities
            .values()
            .filter(|entity| entity.kind() == kind)
            .map(|entity| entity.id)
            .collect()
    }

    /// Entities within `radius` of `centre`, ascending by id.
    ///
    /// A linear scan: the Phase 05 entity budget is a few thousand, so a spatial
    /// index would be speculative (AGENTS.md §3.2 — profile before optimizing).
    /// P05-18 measures this; P08-14 acts on the measurement.
    #[must_use]
    pub fn within_radius(&self, centre: Vec3, radius: f64) -> Vec<EntityId> {
        if !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }
        let limit = radius * radius;
        self.entities
            .values()
            .filter(|entity| entity.distance_squared(centre) <= limit)
            .map(|entity| entity.id)
            .collect()
    }

    /// The nearest entity of `kind` within `radius`, by ascending id on a tie.
    #[must_use]
    pub fn nearest_of_kind(&self, kind: EntityKind, centre: Vec3, radius: f64) -> Option<EntityId> {
        if !radius.is_finite() || radius < 0.0 {
            return None;
        }
        let limit = radius * radius;
        self.entities
            .values()
            .filter(|entity| entity.kind() == kind && entity.distance_squared(centre) <= limit)
            .min_by(|a, b| {
                a.distance_squared(centre)
                    .partial_cmp(&b.distance_squared(centre))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.id.cmp(&b.id))
            })
            .map(|entity| entity.id)
    }

    /// Remove every entity flagged [`Entity::removed`], returning their ids.
    ///
    /// Called once per tick by the broadcast phase so clients get one
    /// `remove_entities` batch rather than a packet per death.
    pub fn sweep_removed(&mut self) -> Vec<EntityId> {
        let doomed: Vec<EntityId> = self
            .entities
            .values()
            .filter(|entity| entity.removed)
            .map(|entity| entity.id)
            .collect();
        for id in &doomed {
            self.entities.remove(id);
        }
        doomed
    }

    /// Clear every entity (world unload / tests).
    pub fn clear(&mut self) {
        self.entities.clear();
    }
}

/// Concurrent entity cap.
///
/// Sized from the Phase 05 workload: 10 players, a few hundred mobs, a few hundred
/// dropped items. Reaching it refuses new spawns rather than degrading the server
/// (AGENTS.md §9, §10).
pub const MAX_ENTITIES: usize = 8192;

#[cfg(test)]
// Velocity clamps and offsets are exact doubles, and the fixtures use small counts,
// so these comparisons and casts are exact by construction.
#[allow(clippy::float_cmp, clippy::cast_precision_loss)]
mod tests {
    use super::{EntityBody, EntityId, EntityKind, EntityStore, MAX_ENTITIES};
    use crate::mob::{Mob, MobKind};
    use crate::player::Vec3;

    fn store_with_mobs(count: usize) -> EntityStore {
        let mut store = EntityStore::new();
        for index in 0..count {
            store
                .spawn(
                    EntityBody::Mob(Mob::new(MobKind::Zombie)),
                    Vec3::new(index as f64, 64.0, 0.0),
                )
                .expect("spawn");
        }
        store
    }

    #[test]
    fn ids_are_positive_ascending_and_never_reused() {
        let mut store = EntityStore::new();
        let first = store
            .spawn(EntityBody::Player, Vec3::default())
            .expect("spawn");
        let second = store
            .spawn(EntityBody::Player, Vec3::default())
            .expect("spawn");
        assert!(first.get() > 0);
        assert!(second.get() > first.get());
        assert_eq!(store.next_id().get(), second.get() + 1);

        // Removing the first does not free its id for reuse.
        assert!(store.remove(first).is_some());
        let third = store
            .spawn(EntityBody::Player, Vec3::default())
            .expect("spawn");
        assert!(third.get() > second.get(), "{third}");
        assert!(!store.contains(first));
    }

    #[test]
    fn entity_ids_reject_non_positive_values() {
        assert!(EntityId::new(1).is_ok());
        assert!(EntityId::new(0).is_err());
        assert!(EntityId::new(-1).is_err());
        assert!(EntityId::new(i32::MIN).is_err());
    }

    #[test]
    fn iteration_is_ascending_by_id() {
        let store = store_with_mobs(8);
        let ids: Vec<i32> = store.ids().map(EntityId::get).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "iteration order must be ascending id");
    }

    #[test]
    fn a_live_entity_has_the_identity_its_store_derives() {
        // The wrapper, not the derivation: `entity_uuid` has five tests of its own, and none of them would notice a
        // store that passed the wrong seed through or answered for an entity that is not live.
        let mut store = EntityStore::with_seed(42);
        let id = store
            .spawn(
                EntityBody::Item(crate::item_entity::ItemEntity::new(
                    crate::ItemStack::new(1, 1).expect("stone"),
                    None,
                )),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .expect("spawns");

        assert_eq!(store.seed(), 42);
        assert_eq!(
            store.uuid(id),
            Some(crate::identity::entity_uuid(42, id.get())),
            "the store and the derivation disagree about the same entity"
        );

        // Two stores at one seed agree, and two at different seeds do not: the property the trace comparison rests
        // on, asserted at the call site rather than only in the derivation.
        let mut same = EntityStore::with_seed(42);
        let same_id = same
            .spawn(
                EntityBody::Item(crate::item_entity::ItemEntity::new(
                    crate::ItemStack::new(1, 1).expect("stone"),
                    None,
                )),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .expect("spawns");
        assert_eq!(store.uuid(id), same.uuid(same_id));

        let mut other = EntityStore::with_seed(43);
        let other_id = other
            .spawn(
                EntityBody::Item(crate::item_entity::ItemEntity::new(
                    crate::ItemStack::new(1, 1).expect("stone"),
                    None,
                )),
                Vec3::new(0.5, 64.0, 0.5),
            )
            .expect("spawns");
        assert_ne!(
            store.uuid(id),
            other.uuid(other_id),
            "two worlds share an entity identity"
        );
    }

    #[test]
    fn an_entity_that_is_not_live_has_no_identity() {
        // A caller labelling a packet must not be handed the identity of something that no longer exists; `None` is
        // the answer that makes the caller handle it.
        let store = EntityStore::new();
        assert_eq!(store.uuid(EntityId::new(1).expect("positive")), None);
    }

    #[test]
    fn spawning_respects_the_cap() {
        // The cap exists so a runaway spawner cannot exhaust memory; fill a small
        // store by lowering the effective count through repeated spawns is too slow,
        // so assert the guard's shape instead: a fresh store is far below the cap.
        let store = store_with_mobs(4);
        assert!(store.len() < MAX_ENTITIES);
        assert_eq!(store.len(), 4);
    }

    #[test]
    fn kind_filtering_and_lookups() {
        let mut store = EntityStore::new();
        let player = store
            .spawn(EntityBody::Player, Vec3::new(0.0, 64.0, 0.0))
            .expect("spawn");
        store
            .spawn(
                EntityBody::Mob(Mob::new(MobKind::Zombie)),
                Vec3::new(1.0, 64.0, 0.0),
            )
            .expect("spawn");
        store
            .spawn(
                EntityBody::Mob(Mob::new(MobKind::Cow)),
                Vec3::new(5.0, 64.0, 0.0),
            )
            .expect("spawn");

        assert_eq!(store.of_kind(EntityKind::Player), vec![player]);
        assert_eq!(store.of_kind(EntityKind::Mob).len(), 2);
        assert!(store.of_kind(EntityKind::Item).is_empty());

        let near = store.within_radius(Vec3::new(0.0, 64.0, 0.0), 2.0);
        assert_eq!(near.len(), 2, "the player and the near zombie");
        assert!(!near.contains(&player) || near[0] == player);

        assert_eq!(
            store.nearest_of_kind(EntityKind::Mob, Vec3::new(0.0, 64.0, 0.0), 10.0),
            Some(EntityId::new(2).expect("id 2"))
        );
        assert_eq!(
            store.nearest_of_kind(EntityKind::Mob, Vec3::new(0.0, 64.0, 0.0), 0.5),
            None,
            "no mob is inside half a block"
        );
    }

    #[test]
    fn radius_queries_reject_hostile_arguments() {
        let store = store_with_mobs(2);
        let centre = Vec3::default();
        assert!(store.within_radius(centre, -1.0).is_empty());
        assert!(store.within_radius(centre, f64::NAN).is_empty());
        assert!(store.within_radius(centre, f64::INFINITY).is_empty());
        assert!(
            store
                .nearest_of_kind(EntityKind::Mob, centre, -5.0)
                .is_none()
        );
        assert!(
            store
                .nearest_of_kind(EntityKind::Mob, centre, f64::NAN)
                .is_none()
        );
    }

    #[test]
    fn removal_sweep_is_batched_and_stable() {
        let mut store = store_with_mobs(5);
        let ids: Vec<EntityId> = store.ids().collect();
        store.get_mut(ids[1]).expect("entity").removed = true;
        store.get_mut(ids[3]).expect("entity").removed = true;
        let removed = store.sweep_removed();
        assert_eq!(removed, vec![ids[1], ids[3]], "ascending, in one batch");
        assert_eq!(store.len(), 3);
        assert!(store.sweep_removed().is_empty(), "idempotent");
    }

    #[test]
    fn timers_tick_down_and_effects_expire() {
        let mut store = store_with_mobs(1);
        let id = store.ids().next().expect("one");
        {
            let entity = store.get_mut(id).expect("entity");
            entity.invulnerable_ticks = 2;
            entity.fire_ticks = 1;
            entity.air_ticks = 1;
            entity.effects.insert(
                1,
                crate::effect::ActiveEffect {
                    id: 1,
                    amplifier: 0,
                    duration: 2,
                    ambient: false,
                },
            );
            entity.tick_timers();
        }
        let entity = store.get(id).expect("entity");
        assert_eq!(entity.age, 1);
        assert_eq!(entity.invulnerable_ticks, 1);
        assert_eq!(entity.fire_ticks, 0);
        assert_eq!(entity.air_ticks, 0);
        assert_eq!(entity.effects.len(), 1);
        store.get_mut(id).expect("entity").tick_timers();
        assert!(
            store.get(id).expect("entity").effects.is_empty(),
            "an effect expires when its duration reaches zero"
        );
    }

    #[test]
    fn velocity_impulses_are_clamped() {
        let mut store = store_with_mobs(1);
        let id = store.ids().next().expect("one");
        let entity = store.get_mut(id).expect("entity");
        entity.add_velocity(Vec3::new(1_000_000.0, -1_000_000.0, 0.0));
        assert_eq!(entity.velocity.x, super::MAX_VELOCITY);
        assert_eq!(entity.velocity.y, -super::MAX_VELOCITY);
        // Repeated impulses cannot accumulate past the clamp either.
        for _ in 0..100 {
            entity.add_velocity(Vec3::new(5.0, 0.0, 0.0));
        }
        assert_eq!(entity.velocity.x, super::MAX_VELOCITY);
    }

    #[test]
    fn health_is_initialised_per_kind() {
        let mut store = EntityStore::new();
        let player = store
            .spawn(EntityBody::Player, Vec3::default())
            .expect("spawn");
        let zombie = store
            .spawn(EntityBody::Mob(Mob::new(MobKind::Zombie)), Vec3::default())
            .expect("spawn");
        assert_eq!(
            store.get(player).expect("player").health,
            crate::player::MAX_HEALTH
        );
        assert_eq!(
            store.get(zombie).expect("zombie").health,
            MobKind::Zombie.max_health()
        );
        assert!(store.get(player).expect("player").is_alive());
    }

    #[test]
    fn hitboxes_differ_by_kind() {
        let mut store = EntityStore::new();
        let player = store
            .spawn(EntityBody::Player, Vec3::new(0.0, 64.0, 0.0))
            .expect("spawn");
        let zombie = store
            .spawn(
                EntityBody::Mob(Mob::new(MobKind::Zombie)),
                Vec3::new(0.0, 64.0, 0.0),
            )
            .expect("spawn");
        let player_box = store.get(player).expect("player").hitbox();
        let zombie_box = store.get(zombie).expect("zombie").hitbox();
        // The player's height comes from `Aabb::player` as exact f64 (1.8), but the
        // arithmetic is `position.y + height`, so compare with a tolerance that
        // accounts for one rounding at this magnitude rather than `f64::EPSILON`
        // (which is ~2.2e-16 and far below the representable difference here).
        assert!(
            (player_box.max_y - player_box.min_y - 1.8).abs() < 1e-12,
            "player height {}",
            player_box.max_y - player_box.min_y
        );
        // A mob's height is an f32 in the kind table, so widening it is not exact:
        // `f64::from(1.95_f32)` is 1.9500000476837158. Compare against the same
        // widened value rather than the decimal literal, and keep the comparison to
        // the height the table actually holds.
        let expected = f64::from(MobKind::Zombie.dimensions().1);
        assert!(
            (zombie_box.max_y - zombie_box.min_y - expected).abs() < 1e-9,
            "a zombie is {expected} tall (f32 widened), got {}",
            zombie_box.max_y - zombie_box.min_y
        );
        // …and it is still 1.95 to the precision the table carries.
        assert!((expected - 1.95).abs() < 1e-6, "table height {expected}");
        // Width too: 0.6 f32 centred on the position.
        let width = zombie_box.max_x - zombie_box.min_x;
        assert!(
            (width - f64::from(MobKind::Zombie.dimensions().0)).abs() < 1e-9,
            "zombie width {width}"
        );
    }

    #[test]
    fn clearing_empties_the_store_without_reusing_ids() {
        let mut store = store_with_mobs(3);
        let last = store.ids().last().expect("ids");
        store.clear();
        assert!(store.is_empty());
        let next = store
            .spawn(EntityBody::Player, Vec3::default())
            .expect("spawn");
        assert!(next.get() > last.get());
    }
}
