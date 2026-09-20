//! Entity model: the player entity (state, inventory, health/hunger/experience,
//! game modes, respawn) and its `playerdata` persistence boundary.
//!
//! Phase 04 owns entities, so the entity model lives here: the player, the
//! dropped item, the projectile baseline, the entity id allocator and the entity
//! map, plus the item-stack and inventory primitives they need.
//!
//! | Module | Owns |
//! |---|---|
//! | [`stack`] | [`ItemStack`] and the resolved vanilla stack-size table |
//! | [`inventory`] | [`PlayerInventory`]: 41 stored slots, hands, container payload |
//! | [`profile`] | [`GameProfile`]: the identity player state is keyed by |
//! | [`player`] | [`Player`], [`GameMode`], damage/food/experience, `playerdata` |
//! | [`item_entity`] | [`ItemEntity`]: a dropped [`ItemStack`], its timers and its pure physics |
//! | [`projectile`] | [`Projectile`], [`ProjectileKind`]: the arrow/snowball trajectory baseline |
//! | [`entity`] | [`EntityId`], [`EntityKind`], [`EntityBody`], [`Entity`], [`EntityStore`] |
//! | [`effect`] | [`ActiveEffect`]: one status effect on an entity |
//! | [`mob`] | [`MobKind`], [`Mob`], [`MobAi`]: the mob table and its AI goals |
//! | [`pathfind`] | [`BlockView`], [`SearchLimits`], [`find_path`]: bounded A* over caller-supplied blocks |
//!
//! ```
//! use mc_entity::{GameMode, Player, PlayerInventory, StackSizeTable};
//! use mc_registry::ItemRegistry;
//! use std::path::Path;
//!
//! // Startup: resolve the stack-size table once, against the real registry.
//! # let items = ItemRegistry::load(
//! #     &Path::new(env!("CARGO_MANIFEST_DIR"))
//! #         .join("../../crates/test-support/fixtures/registry/items.tsv")).expect("fixture");
//! let sizes = StackSizeTable::resolve(&items).expect("table resolves");
//! let inventory = PlayerInventory::new(sizes);
//! let profile = mc_entity::GameProfile::new("b50ad385-829d-3141-a216-7e7d7539ba7f", "Notch")
//!     .expect("valid name");
//! let mut player = Player::new(profile, 1, "minecraft:overworld", inventory);
//!
//! // A hostile packet claims hotbar slot 200: rejected, never wrapped.
//! assert!(player.inventory.select(200).is_err());
//! // 20 points of melee damage on a survival player at full health.
//! let outcome = player.apply_damage(
//!     20.0,
//!     mc_entity::combat::DamageSource::MobAttack,
//!     &mc_entity::combat::CombatStats::ZERO,
//! );
//! assert!(outcome.died && !player.is_alive());
//! player.respawn(false);
//! assert_eq!(player.health, 20.0);
//! assert_eq!(player.game_mode, GameMode::Survival);
//! ```
//!
//! ## Boundaries
//!
//! This crate depends on `mc-core`, `mc-world`, `mc-nbt` and `mc-registry`. It
//! deliberately does **not** depend on `mc-network` (which also defines a
//! `GameProfile`), `mc-persistence` or `mc-protocol`: the simulation layer must
//! not pull in Tokio, sockets or the disk format. Position vectors come from
//! [`mc_world::Vec3`]; the player-specific primitives are documented in [`profile`].
//!
//! The `mc-world` dependency is one-directional (world never depends on entity)
//! and exists so the project has exactly **one** `Aabb` and one collision solver:
//! an entity's hitbox is expressed with the same type the world resolves movement
//! against. A second box type plus a conversion at every call site was rejected as
//! the more error-prone option. [`pathfind`] does **not** use it: that module sees
//! the world only through its own [`BlockView`] trait, so the search stays
//! testable against a flat grid with no world, no registry and no chunk store.
//!
//! ## Error policy (AGENTS.md section 9)
//!
//! - Hostile client input — hotbar index, hand id, slot index, item count,
//!   damage amount, item id — is [`ServerError::InvalidAction`].
//! - A hostile or truncated `playerdata` document is
//!   [`ServerError::CorruptData`], and an unknown item *name* is corrupt data
//!   rather than a silent drop.
//! - Nothing outside `#[cfg(test)]` panics: every fallible entry point returns
//!   [`ServerResult`], and every "total" accessor returns a documented default
//!   instead of indexing out of bounds.
//!
//! ## Unsupported behaviour, explicitly (AGENTS.md section 3.3)
//!
//! Summarised here; each module carries its own precise list:
//!
//! - no collision, movement resolution, fall damage or reach checks: an entity's
//!   velocity is integrated by the caller and only the item and projectile
//!   modules provide the *pure* half of that arithmetic;
//! - no mob movement, path **following**, head rotation, spawning rules, loot,
//!   daylight burning or creeper fuse: [`mob`] decides goals and [`pathfind`]
//!   returns block steps, and the tick loop is what walks them;
//! - no mob attack resolution: [`MobAttackStyle::Ranged`] and
//!   [`MobAttackStyle::Explosive`] are recorded so a caller can refuse them, and
//!   [`MobKind::attack_damage`] for those kinds is documented Vanilla damage, not
//!   a melee value;
//! - no mob width in pathfinding (clearance is height only), no water/lava/
//!   ladder/door handling and no Vanilla `PathNavigation` parity: see the
//!   [`pathfind`] gap list, which also records that the movement-speed conversion
//!   in [`mob`] is derived rather than verified;
//! - no item drop, item pickup, item merge search or projectile/entity hit test:
//!   [`ItemEntity`] and [`Projectile`] provide the decisions, the caller owns the
//!   world queries;
//! - no vanilla arrow behaviour beyond a trajectory baseline (no critical hits,
//!   enchantments, pickup state, tipped/spectral arrows or impact response — see
//!   [`projectile`]);
//! - no items falling slower in water ([`item_entity`]);
//! - no container transactions (clicks, drags, shift-click, cursor stack);
//! - no crafting: payload slots 0 and 1..=4 are always empty;
//! - no armour/effect/absorption damage reduction and no `DamageSource` typing;
//! - no `keepInventory` game rule lookup (the caller passes the flag) and no
//!   item dropping (the caller receives the stacks);
//! - no difficulty: starvation damage is the normal/hard value;
//! - no per-stack `components`/durability/enchantments, so a datapack's
//!   `max_stack_size` override is not honoured.

#![forbid(unsafe_code)]
// Entity code crosses numeric domains constantly: an item count is an `i32` on
// the wire and a `TAG_Byte` on disk, health is `f32` in the game and `f64` in
// NBT, and a slot index is a `usize`, a `u8` and a signed byte in three
// encodings. Every conversion site either bounds-checks first, uses `try_from`,
// or is lossless by construction (documented at each site), so the pedantic cast
// lints would only add noise — the same exemption `mc-persistence` and `mc-nbt`
// already document.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod combat;
pub mod effect;
pub mod entity;
pub mod identity;
pub mod inventory;
pub mod item_entity;
pub mod mob;
pub mod orb;
pub mod pathfind;
pub mod player;
pub mod profile;
pub mod projectile;
pub mod stack;

pub use effect::ActiveEffect;
pub use entity::{AIR_TICKS, Entity, EntityBody, EntityId, EntityKind, EntityStore, MAX_ENTITIES};
pub use inventory::{
    CONTAINER_SLOT_COUNT, CRAFTING_RESULT_SLOT, CRAFTING_START, Hand, OFFHAND_SLOT,
    PLAYER_WINDOW_ID, PlayerInventory, inventory_for_registry,
};
pub use item_entity::{DESPAWN_AGE_TICKS, ItemEntity, PICKUP_DELAY_TICKS};
pub use mob::{
    AGGRO_RADIUS, ATTACK_COOLDOWN_TICKS, ATTACK_RANGE, DECISION_INTERVAL_TICKS, Mob, MobAi,
    MobAttackStyle, MobBehaviour, MobGoal, MobGoalKind, MobKind, MobObservation, MobSighting,
    TARGET_LOSE_RADIUS, WALK_DURATION_TICKS, WANDER_RADIUS,
};
pub use pathfind::{BlockView, SearchLimits, SearchStats, find_path, find_path_with_stats};
pub use player::{
    DamageOutcome, EXHAUSTION_PER_POINT, GameMode, MAX_FOOD, MAX_HEALTH, MAX_SATURATION, Player,
    REGEN_FOOD_THRESHOLD, STARVATION_DAMAGE,
};
pub use profile::{GameProfile, validate_username};
pub use projectile::{MAX_LIFETIME_ARROW, MAX_LIFETIME_SNOWBALL, Projectile, ProjectileKind};
pub use stack::{
    AIR_ITEM_ID, DEFAULT_MAX_STACK_SIZE, HARD_MAX_STACK_SIZE, ItemStack, MAX_STACK_SIZE_1,
    MAX_STACK_SIZE_16, StackSizeTable, max_stack_size_for_name,
};

/// Re-exported so callers can match on failure classes without another import.
pub use mc_core::error::{ServerError, ServerResult};
