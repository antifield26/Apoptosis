//! Dropped item entities: the timers and the pure part of their physics.
//!
//! A dropped item is the smallest interesting entity in the game: it is a
//! [`ItemStack`] on the ground with two timers attached. This module owns that
//! state, the merge rule that turns two of them back into one, and the arithmetic
//! half of their movement.
//!
//! It deliberately owns **nothing that needs a world query**. This crate does
//! depend on `mc-world`, but only so that an entity's hitbox is expressed with the
//! one `Aabb` type the collision solver uses ([`crate::entity::Entity::hitbox`]);
//! block and entity lookups stay with the caller, which is why every function here
//! is a pure function of the values passed in.
//!
//! ## Physics constants, with their basis (AGENTS.md section 3.1)
//!
//! Vanilla's `Entity` movement is `moveRelative` -> `move` -> `MoverType`, and an
//! item reuses that path with three overridden constants. The confidence column
//! is the point of the table: a constant this module cannot cite is marked as an
//! approximation rather than dressed up as a vanilla fact.
//!
//! | Constant | Value | Basis | Confidence |
//! |---|---|---|---|
//! | [`ITEM_GRAVITY`] | `0.04` blocks/tick^2 | appended to `deltaMovement.y` once per tick before the move (`Entity.getGravity`, `0.04` for a non-living entity that is not a boat/minecart) | vanilla value |
//! | [`ITEM_DRAG`] | `0.98`, horizontal **and** vertical | `ItemEntity.tick`: `this.setDeltaMovement(this.getDeltaMovement().multiply(0.98, 0.98, 0.98))` in air, `0.99` when `isInWater()` | vanilla value |
//! | [`ITEM_GROUND_FRICTION`] | `0.6`, horizontal only | the block-friction factor `getFrictionInfluencedSpeed`: `0.6` on ground, `0.91` in air, `0.98` when flying | vanilla value |
//! | [`PICKUP_DELAY_TICKS`] | `10` | `ItemEntity.setDefaultPickUpDelay()` sets `pickupDelay = 10`; it is the **default**, and some drop paths deliberately exceed it | default verified, per-path use not verified |
//! | [`DESPAWN_AGE_TICKS`] | `6000` (5 min at 20 TPS) | `ItemEntity.tick`: `if (this.age >= 6000) this.discard()`, i.e. the item is removed on the tick its age reaches 6000 | vanilla value |
//!
//! Vanilla's per-tick order is gravity -> drag -> move, and the drag is applied
//! **before** the position update, so a tick's displacement already includes that
//! tick's gravity and drag. [`ItemEntity::tick_physics`] follows that order; it is
//! the whole reason the two steps are separate from the position update the caller
//! performs (see below).
//!
//! ## What this module does not do
//!
//! - **Items do not fall slower in water.** Vanilla raises the drag to `0.99`
//!   while `isInWater()`, which is what makes a dropped item sink gently rather
//!   than drop like a stone. That needs a fluid query, so it is **not
//!   implemented** here; a caller that has already established the entity is in
//!   water must not expect this module to compensate. Adding it later means
//!   passing a fluid flag into [`ItemEntity::tick_physics`], exactly as
//!   `on_ground` is passed today.
//! - **No collision.** The returned velocity is integrated by the caller against
//!   the world; this module never moves a position and never decides that an item
//!   has landed.
//! - **No per-item gravity exceptions.** Vanilla multiplies an item's gravity by
//!   `1 - buoyancy` for a few items; that is a data-driven per-item property and
//!   is not modelled.
//! - **No pickup search, no merge search, no inventory insert.** Those need the
//!   world and the player list. This module provides only the decisions
//!   ([`ItemEntity::can_be_picked_up`]) and the primitive
//!   ([`ItemEntity::merge_with`]).
//! - **No `Thrower`/`Owner` UUID.** Vanilla stores the owning player's UUID; the
//!   engine keys entities by [`EntityId`], so the ids are what is stored.

use crate::entity::EntityId;
use crate::player::Vec3;
use crate::stack::ItemStack;

/// Downward velocity added to an item every tick, in blocks per tick squared.
///
/// The vanilla value: `Entity.getGravity` returns `0.04` for a non-living entity
/// that is not a boat or a minecart, and it is appended to `deltaMovement.y`
/// before the move each tick.
pub const ITEM_GRAVITY: f64 = 0.04;

/// Air drag applied to all three velocity components of an item, per tick.
///
/// The vanilla value from `ItemEntity.tick` (`multiply(0.98, 0.98, 0.98)`).
pub const ITEM_DRAG: f64 = 0.98;

/// Horizontal velocity retained per tick by an item resting on the ground.
///
/// The vanilla block-friction factor (`0.6` on ground). It is applied **in
/// addition** to [`ITEM_DRAG`], as vanilla's two-stage `moveRelative`/`move` does,
/// so a grounded item's horizontal speed decays by `0.6 x 0.98 = 0.588` per tick.
pub const ITEM_GROUND_FRICTION: f64 = 0.6;

/// Ticks during which a freshly dropped item cannot be picked up.
///
/// The vanilla **default** pickup delay: `ItemEntity.setDefaultPickUpDelay()` sets
/// `pickupDelay = 10`, and `10` is the value this module is specified to use for a
/// player drop.
///
/// **Confidence: medium.** The default is well established, but not every vanilla
/// drop path uses it - a "drop around" drop (death drops, `/give`-style spread) is
/// documented as raising the delay above the default. A caller that needs exact
/// per-path parity must set [`ItemEntity::pickup_delay`] itself; that field is
/// public for exactly this reason.
pub const PICKUP_DELAY_TICKS: u32 = 10;

/// Ticks an item survives before it is discarded, at 20 TPS.
///
/// The vanilla value: `ItemEntity.tick` discards at `age >= 6000`, i.e. 5 minutes.
pub const DESPAWN_AGE_TICKS: u32 = 6000;

/// A dropped [`ItemStack`] with its pickup and despawn timers.
///
/// The struct is data plus pure functions; the caller owns its position and
/// velocity (in [`crate::entity::Entity`]) and runs collision against the world.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemEntity {
    /// The stack on the ground.
    ///
    /// Expected to be non-empty for a live entity: a zero-item drop is invisible
    /// and unkillable, so a caller that spawns one has created a leak. That is a
    /// **caller contract, not an enforced invariant** - nothing in this module
    /// rejects an empty stack, and [`ItemEntity::merge_with`] refuses to merge into
    /// one rather than pretending the situation is fine.
    pub stack: ItemStack,
    /// Ticks remaining before the stack may be picked up. Counts down, never up.
    pub pickup_delay: u32,
    /// Ticks this item has existed, saturating at [`DESPAWN_AGE_TICKS`].
    pub despawn_age: u32,
    /// Player whose inventory the stack left, if it was not a world drop.
    pub owner: Option<EntityId>,
    /// The player the drop is credited to, if any (vanilla's `Thrower`).
    ///
    /// Set only by [`ItemEntity::with_thrower`], so the common path stays a single
    /// call. It exists so drop attribution and statistics have a source; **no rule
    /// is evaluated from it here** (vanilla's owner/thrower pickup and targeting
    /// rules are caller-side and are not modelled).
    pub thrower: Option<EntityId>,
    /// Whether the world reported the item as resting on solid ground.
    ///
    /// Not part of the vanilla item struct - vanilla re-derives it every tick
    /// during the move - but the collision pass here lives in the caller, so the
    /// flag has to be carried between the caller's move and
    /// [`ItemEntity::tick_physics`]. The caller sets it after collision each tick.
    pub on_ground: bool,
}

impl ItemEntity {
    /// A freshly dropped stack: default pickup delay, zero age, no thrower.
    ///
    /// `owner` is the player whose inventory the stack came out of; pass `None`
    /// for a world drop (block break, mob drop, `/summon`). The despawn timer
    /// starts at zero and the pickup delay at [`PICKUP_DELAY_TICKS`], the vanilla
    /// default - see that constant for the paths that deliberately differ.
    #[must_use]
    pub const fn new(stack: ItemStack, owner: Option<EntityId>) -> Self {
        Self {
            stack,
            pickup_delay: PICKUP_DELAY_TICKS,
            despawn_age: 0,
            owner,
            thrower: None,
            on_ground: false,
        }
    }

    /// A drop that was killed by `thrower` (the credit for statistics).
    ///
    /// A builder rather than a second constructor so the common path stays a
    /// single call.
    #[must_use]
    pub const fn with_thrower(mut self, thrower: Option<EntityId>) -> Self {
        self.thrower = thrower;
        self
    }

    /// Whether the pickup delay has elapsed and the item may be taken.
    ///
    /// Note that this answers only the *delay* half of vanilla's rule: whether a
    /// player may take the item also depends on distance, the inventory having
    /// room and the pickup cooldown, all of which need the world.
    #[must_use]
    pub const fn can_be_picked_up(&self) -> bool {
        self.pickup_delay == 0
    }

    /// Whether the item has outlived [`DESPAWN_AGE_TICKS`] and must be discarded.
    ///
    /// Vanilla discards at `age >= 6000`, and [`ItemEntity::despawn_age`]
    /// saturates at exactly that value, so the comparison is `>=`.
    #[must_use]
    pub const fn should_despawn(&self) -> bool {
        self.despawn_age >= DESPAWN_AGE_TICKS
    }

    /// Advance both timers by one tick, saturating instead of wrapping.
    ///
    /// - `pickup_delay` counts down to zero and stays there (`saturating_sub`), so
    ///   an item dropped long ago never acquires a delay again.
    /// - `despawn_age` counts up and stops at [`DESPAWN_AGE_TICKS`]. That bound is
    ///   what makes the counter unable to wrap even if a caller runs it for
    ///   billions of ticks without sweeping the entity, and it keeps
    ///   [`ItemEntity::should_despawn`] a stable `true` once it fires.
    pub const fn tick_timers(&mut self) {
        self.pickup_delay = self.pickup_delay.saturating_sub(1);
        // Branch rather than `min`: `Ord::min` is not const-callable on this
        // toolchain. Saturating add first, then the documented bound, so a counter
        // that somehow arrived above the bound (corrupted save, older build) is
        // pulled back to it and the item is swept on the next check.
        self.despawn_age = if self.despawn_age >= DESPAWN_AGE_TICKS {
            DESPAWN_AGE_TICKS
        } else {
            self.despawn_age.saturating_add(1)
        };
    }

    /// Integrate one tick of gravity and drag, returning the new velocity.
    ///
    /// The caller adds the returned velocity to the item's position and then runs
    /// collision; it is a pure function of `(velocity, on_ground)`, so it can be
    /// replayed in a test without a world. The order matches vanilla's tick:
    ///
    /// 1. gravity is added to the vertical component ([`ITEM_GRAVITY`]),
    /// 2. all three components are scaled by [`ITEM_DRAG`],
    /// 3. if [`ItemEntity::on_ground`] is set, the vertical component is zeroed
    ///    and the horizontal ones are additionally scaled by
    ///    [`ITEM_GROUND_FRICTION`].
    ///
    /// Step 3 is what stops the settle from oscillating: a grounded item does not
    /// bounce, it just loses its vertical velocity and slides to a halt.
    ///
    /// ## Hostile input
    ///
    /// Every component of `velocity` is sanitised to `0.0` when it is not finite,
    /// and the result is clamped to [`MAX_COMPONENT`]. A non-finite component can
    /// only reach here from a corrupted save or from arithmetic on client-influenced
    /// knockback, and once it is in the position it is unrecoverable: `NaN`
    /// propagates through every later tick and through the chunk's entity section
    /// on save. Dropping a bad tick to zero is therefore chosen over propagating it
    /// (AGENTS.md section 10). The magnitude clamp bounds the displacement a single
    /// tick can produce, which is the same defence [`crate::entity::Entity`]
    /// applies to impulses.
    #[must_use]
    pub fn tick_physics(&mut self, velocity: Vec3) -> Vec3 {
        let (mut vx, mut vy, mut vz) = sanitize(velocity);
        vy -= ITEM_GRAVITY;
        vx *= ITEM_DRAG;
        vy *= ITEM_DRAG;
        vz *= ITEM_DRAG;
        if self.on_ground {
            vx *= ITEM_GROUND_FRICTION;
            vy = 0.0;
            vz *= ITEM_GROUND_FRICTION;
        }
        Vec3::new(
            vx.clamp(-MAX_COMPONENT, MAX_COMPONENT),
            vy.clamp(-MAX_COMPONENT, MAX_COMPONENT),
            vz.clamp(-MAX_COMPONENT, MAX_COMPONENT),
        )
    }

    /// Merge `other` into this item, up to `max_stack` of this item, returning
    /// whether **any** item moved.
    ///
    /// Vanilla joins two nearby stacks of the same item into one entity; this is
    /// the primitive half of that (the radius search is the caller's). `other` is
    /// left holding whatever did not fit, exactly like [`ItemStack::merge_capped`],
    /// so an item can never be destroyed by a merge - only moved or left behind.
    ///
    /// Returns `false` when nothing merged: different items, an empty `other`, an
    /// empty `self`, a full `self`, or a non-positive `max_stack`.
    ///
    /// An **empty `self` refuses the merge** even though [`ItemStack::merge_capped`]
    /// would happily fill it. A live item entity always carries a stack (the engine
    /// never spawns one for [`ItemStack::EMPTY`], because an invisible zero-item
    /// entity can neither be picked up nor seen), so an empty stack here means the
    /// entity is already doomed to be swept this tick; letting it absorb a
    /// neighbour would revive a corpse and lose the neighbour's items with it.
    /// Refusing is the failure mode that cannot destroy items.
    ///
    /// ## `max_stack` is attacker-adjacent input
    ///
    /// `max_stack` normally comes from [`crate::StackSizeTable`], but a damaged
    /// data pack or a bogus per-stack `max_stack_size` component can supply `0` or
    /// a negative number. Such a limit means "nothing fits", and it is treated as
    /// exactly that: the function returns `false` without looping and without
    /// touching either stack. It is **not** silently promoted to `1`, and it is
    /// **not** allowed to overflow the `limit - count` arithmetic that
    /// [`ItemStack::merge_capped`] performs.
    ///
    /// The same clamp protects the other end: a `max_stack` above vanilla's hard
    /// ceiling of 64 cannot create a stack larger than 64, because
    /// [`ItemStack::merge_capped`] clamps its own limit as well.
    pub fn merge_with(&mut self, other: &mut Self, max_stack: i32) -> bool {
        if max_stack <= 0 || self.stack.is_empty() || other.stack.is_empty() {
            return false;
        }
        let before = self.stack.count();
        // `merge_capped` returns what did not fit; the moved amount is the
        // difference, which is cheaper to compare than to re-derive from ids.
        let _leftover = self.stack.merge_capped(&mut other.stack, max_stack);
        self.stack.count() > before
    }

    /// The registry id of the stack this item carries, or `None` when empty.
    #[must_use]
    pub const fn item_id(&self) -> Option<i32> {
        self.stack.item_id()
    }

    /// Number of items left in the stack.
    #[must_use]
    pub const fn count(&self) -> i32 {
        self.stack.count()
    }
}

/// Largest magnitude any velocity component may leave [`ItemEntity::tick_physics`]
/// with, in blocks per tick.
///
/// Vanilla's terminal velocity is about `3.9` blocks/tick and an item is slower
/// still, so `10` accepts every legitimate velocity while refusing a value that
/// would cross a view distance in one tick.
pub const MAX_COMPONENT: f64 = 10.0;

/// Replace non-finite components with `0.0`, leaving finite ones untouched.
///
/// See [`ItemEntity::tick_physics`] for why dropping is chosen over propagating.
fn sanitize(velocity: Vec3) -> (f64, f64, f64) {
    let clean = |component: f64| {
        if component.is_finite() {
            component
        } else {
            0.0
        }
    };
    (clean(velocity.x), clean(velocity.y), clean(velocity.z))
}

#[cfg(test)]
mod tests {
    // Vanilla constants are asserted exactly on purpose: an epsilon would hide
    // the drift this file's constant table exists to prevent. Behavioural
    // assertions on simulated physics use an explicit epsilon instead.
    #![allow(clippy::float_cmp)]
    use super::{
        DESPAWN_AGE_TICKS, ITEM_DRAG, ITEM_GRAVITY, ITEM_GROUND_FRICTION, ItemEntity,
        MAX_COMPONENT, PICKUP_DELAY_TICKS,
    };
    use crate::entity::EntityId;
    use crate::player::Vec3;
    use crate::stack::ItemStack;

    /// Physics tolerance: velocities are in blocks/tick and a tick is 50 ms, so
    /// `1e-9` is far below the smallest displacement that can matter.
    const EPSILON: f64 = 1e-9;

    fn id(raw: i32) -> EntityId {
        EntityId::new(raw).expect("test ids are positive")
    }

    /// Item id 1 is `minecraft:stone` in the registry fixture; the physics here
    /// never consults the registry, so any non-zero id does.
    fn stack(count: i32) -> ItemStack {
        ItemStack::new(1, count).expect("1..=64 is a legal stack")
    }

    fn item(count: i32, owner: Option<EntityId>) -> ItemEntity {
        ItemEntity::new(stack(count), owner)
    }

    #[test]
    fn fresh_item_has_vanilla_pickup_delay_and_no_age() {
        let dropped = item(1, Some(id(7)));
        assert_eq!(dropped.pickup_delay, PICKUP_DELAY_TICKS);
        assert_eq!(dropped.pickup_delay, 10, "vanilla setDefaultPickUpDelay");
        assert_eq!(dropped.despawn_age, 0);
        assert_eq!(dropped.owner, Some(id(7)));
        assert_eq!(dropped.thrower, None);
        assert!(!dropped.on_ground);
        assert_eq!(dropped.item_id(), Some(1));
        assert_eq!(dropped.count(), 1);
        // A world drop gets the same delay as a player drop.
        assert_eq!(item(1, None).pickup_delay, PICKUP_DELAY_TICKS);
    }

    #[test]
    fn gravity_reduces_upward_velocity() {
        let mut dropped = item(1, None);
        let mut velocity = Vec3::new(0.0, 1.0, 0.0);
        let mut previous = velocity.y;
        for _ in 0..5 {
            velocity = dropped.tick_physics(velocity);
            assert!(
                velocity.y < previous,
                "gravity must keep lowering the vertical component: {previous} then {}",
                velocity.y
            );
            previous = velocity.y;
        }
        // Vanilla order: gravity then drag, so the first tick is
        // (1.0 - 0.04) * 0.98 = 0.9408, not 1.0 * 0.98 - 0.04.
        assert!((0.9408_f64 - 0.98 * (1.0 - ITEM_GRAVITY)).abs() < EPSILON);
        assert!(
            (0.9408_f64 - (1.0 * ITEM_DRAG - ITEM_GRAVITY)).abs() > 1e-4,
            "the order of gravity and drag is observable, so it must be vanilla's"
        );
    }

    #[test]
    fn drag_reduces_horizontal_speed_in_air() {
        let mut dropped = item(1, None);
        assert!(!dropped.on_ground);
        let mut velocity = Vec3::new(1.0, 0.0, 0.0);
        let mut previous = velocity.x;
        for _ in 0..10 {
            velocity = dropped.tick_physics(velocity);
            assert!(
                velocity.x < previous,
                "horizontal speed must decay in air: {previous} then {}",
                velocity.x
            );
            previous = velocity.x;
        }
        assert!(
            (velocity.x - ITEM_DRAG.powi(10)).abs() < EPSILON,
            "in air the horizontal component is exactly drag^n"
        );
        assert!((velocity.z).abs() < EPSILON, "zero stays zero");
    }

    #[test]
    fn a_grounded_item_settles_without_oscillating() {
        let mut dropped = item(1, None);
        dropped.on_ground = true;
        let mut velocity = Vec3::new(1.0, 1.0, -0.5);
        for tick in 0..120 {
            velocity = dropped.tick_physics(velocity);
            assert!(
                velocity.y == 0.0,
                "a grounded item never bounces: tick {tick} gave vy {}",
                velocity.y
            );
            assert!(
                velocity.x >= 0.0 && velocity.z <= 0.0,
                "friction must not reverse the direction of travel at tick {tick}"
            );
        }
        let speed = velocity.x.hypot(velocity.z);
        assert!(
            speed < 1e-6,
            "the item must converge to rest, got {speed} after 120 ticks"
        );
        // Convergence is geometric with the vanilla factor 0.6 * 0.98 = 0.588.
        assert!((ITEM_GROUND_FRICTION * ITEM_DRAG - 0.588).abs() < EPSILON);
    }

    #[test]
    fn non_finite_velocity_cannot_poison_the_next_velocity() {
        let mut dropped = item(1, None);
        for hostile in [
            Vec3::new(f64::NAN, f64::NAN, f64::NAN),
            Vec3::new(f64::INFINITY, f64::NEG_INFINITY, f64::NAN),
            Vec3::new(1.0, f64::NAN, f64::INFINITY),
        ] {
            let stepped = dropped.tick_physics(hostile);
            assert!(
                stepped.is_finite(),
                "hostile velocity {hostile:?} produced a non-finite result {stepped:?}"
            );
            assert!(stepped.x.is_finite() && stepped.y.is_finite() && stepped.z.is_finite());
            assert!(stepped.x.abs() <= MAX_COMPONENT);
        }
        // Sanitising a NaN x leaves the finite components to behave normally.
        let mixed = dropped.tick_physics(Vec3::new(f64::NAN, 0.0, 0.0));
        assert_eq!(mixed.x, 0.0, "NaN is dropped, not propagated");
        assert!((mixed.y + ITEM_GRAVITY * ITEM_DRAG).abs() < EPSILON);
    }

    #[test]
    fn absurd_but_finite_velocity_is_clamped() {
        let mut dropped = item(1, None);
        let stepped = dropped.tick_physics(Vec3::new(f64::MAX, f64::MIN, f64::MAX));
        assert!(stepped.x <= MAX_COMPONENT && stepped.y >= -MAX_COMPONENT);
        assert!(stepped.is_finite());
    }

    #[test]
    fn pickup_delay_elapses_after_exactly_ten_ticks() {
        let mut dropped = item(1, Some(id(3)));
        assert!(
            !dropped.can_be_picked_up(),
            "a fresh player drop is intangible, or a player picks their own drop back up mid-swing"
        );
        for tick in 0..PICKUP_DELAY_TICKS {
            dropped.tick_timers();
            let remaining = PICKUP_DELAY_TICKS - tick - 1;
            assert_eq!(dropped.pickup_delay, remaining, "after {} ticks", tick + 1);
            assert_eq!(
                dropped.can_be_picked_up(),
                remaining == 0,
                "pickup must become possible on the tick the delay reaches 0"
            );
        }
        assert!(dropped.can_be_picked_up());
        dropped.tick_timers();
        assert_eq!(dropped.pickup_delay, 0, "the delay saturates at zero");
        assert!(dropped.can_be_picked_up());
    }

    #[test]
    fn despawn_fires_at_the_limit_and_never_wraps() {
        let mut dropped = item(1, None);
        assert!(!dropped.should_despawn(), "age 0 must not despawn");
        dropped.despawn_age = DESPAWN_AGE_TICKS - 1;
        assert!(!dropped.should_despawn());
        dropped.tick_timers();
        assert_eq!(dropped.despawn_age, DESPAWN_AGE_TICKS);
        assert!(dropped.should_despawn(), "vanilla discards at age >= 6000");

        // Ten times the limit: the counter saturates rather than wrapping to a
        // value that would make the item immortal.
        for _ in 0..DESPAWN_AGE_TICKS * 10 {
            dropped.tick_timers();
        }
        assert_eq!(dropped.despawn_age, DESPAWN_AGE_TICKS);
        assert!(dropped.should_despawn());
        assert!(
            dropped.can_be_picked_up(),
            "a long-lived item has long since elapsed its pickup delay"
        );

        // A counter already at u32::MAX (a corrupted save) is pulled back to the
        // documented bound, not wrapped past it.
        let mut corrupt = item(1, None);
        corrupt.despawn_age = u32::MAX;
        corrupt.tick_timers();
        assert_eq!(corrupt.despawn_age, DESPAWN_AGE_TICKS);
        let mut corrupt_delay = item(1, None);
        corrupt_delay.pickup_delay = u32::MAX;
        corrupt_delay.tick_timers();
        assert_eq!(corrupt_delay.pickup_delay, u32::MAX - 1);
    }

    #[test]
    fn merging_the_same_item_reports_the_remainder() {
        let mut target = item(50, Some(id(1)));
        let mut source = item(20, Some(id(2)));
        assert!(target.merge_with(&mut source, 64));
        assert_eq!(target.count(), 64, "capped at the limit");
        assert_eq!(source.count(), 6, "the remainder stays on the ground");
        assert_eq!(source.item_id(), Some(1), "the remainder keeps its item");

        // A merge with room to spare empties the other entity.
        let mut target = item(10, None);
        let mut source = item(5, None);
        assert!(target.merge_with(&mut source, 64));
        assert_eq!(target.count(), 15);
        assert!(source.stack.is_empty());

        // A per-item limit of 16 is honoured (buckets, snowballs, eggs).
        let mut target = item(10, None);
        let mut source = item(10, None);
        assert!(target.merge_with(&mut source, 16));
        assert_eq!(target.count(), 16);
        assert_eq!(source.count(), 4);
    }

    #[test]
    fn different_items_never_merge() {
        let mut target = ItemEntity::new(stack(10), None);
        let mut source = ItemEntity::new(ItemStack::new(2, 10).expect("dirt stacks to 64"), None);
        assert!(!target.merge_with(&mut source, 64));
        assert_eq!(target.count(), 10);
        assert_eq!(source.count(), 10, "nothing moved, nothing lost");
        assert_eq!(source.item_id(), Some(2));

        // An empty source merges nothing and is not treated as a match.
        let mut target = item(10, None);
        let mut empty = ItemEntity::new(ItemStack::EMPTY, None);
        assert!(!target.merge_with(&mut empty, 64));
        assert_eq!(target.count(), 10);

        // An empty *target* also refuses, so a doomed entity cannot absorb a
        // neighbour's items and take them out of the world with it.
        let mut doomed = ItemEntity::new(ItemStack::EMPTY, None);
        let mut source = item(10, None);
        assert!(!doomed.merge_with(&mut source, 64));
        assert!(doomed.stack.is_empty());
        assert_eq!(source.count(), 10, "the neighbour keeps every item");
    }

    #[test]
    fn merging_into_a_full_stack_is_a_no_op() {
        let mut target = item(64, None);
        let mut source = item(64, None);
        assert!(!target.merge_with(&mut source, 64));
        assert_eq!(target.count(), 64);
        assert_eq!(source.count(), 64, "a full target absorbs nothing");
    }

    #[test]
    fn a_non_positive_max_stack_is_handled_without_panicking_or_looping() {
        for limit in [0, -1, i32::MIN] {
            let mut target = item(1, None);
            let mut source = item(64, None);
            assert!(
                !target.merge_with(&mut source, limit),
                "limit {limit} means nothing fits"
            );
            assert_eq!(target.count(), 1, "limit {limit} must not grow the target");
            assert_eq!(source.count(), 64, "limit {limit} must not lose items");
        }
        // An absurd positive limit cannot inflate a stack past vanilla's ceiling
        // either, because `ItemStack` clamps its own limit.
        let mut target = item(1, None);
        let mut source = item(64, None);
        assert!(target.merge_with(&mut source, i32::MAX));
        assert_eq!(target.count(), 64);
        assert_eq!(source.count(), 1);
    }

    #[test]
    fn item_entities_round_trip_clone_and_eq() {
        let original = ItemEntity::new(stack(32), Some(id(4))).with_thrower(Some(id(9)));
        let copy = original.clone();
        assert_eq!(copy, original);
        assert_eq!(copy.thrower, Some(id(9)));
        assert_eq!(copy.owner, Some(id(4)));

        let mut moved = copy.clone();
        moved.on_ground = true;
        assert_ne!(moved, original, "on_ground participates in equality");
        moved.on_ground = false;
        moved.pickup_delay = 3;
        assert_ne!(moved, original, "the pickup delay participates in equality");
        moved.tick_timers();
        assert_ne!(moved, original, "timers differ after one tick");
    }
}
