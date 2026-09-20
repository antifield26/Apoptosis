//! Projectile **trajectory baseline**: arrows and snowballs only.
//!
//! Read this first: **this is not vanilla arrow behaviour.** What is implemented
//! is the closed-form per-tick integration of a thrown projectile - gravity, air
//! drag, a lifetime counter and the per-kind constants - with the caller owning
//! everything that needs the world. Vanilla's `AbstractArrow` is a large state
//! machine (per-tick speed accumulation, water and lava handling, block and
//! entity hit tests, critical-hit rules, enchantment effects, piercing, pickup
//! state, despawn rules that depend on the shooter's game mode). Modelling it
//! needs `mc-world`, and even with it the work is a whole subsystem, so the
//! honest description of this module is *the part of a projectile that can be
//! computed without a world*.
//!
//! ## What this module does not model (AGENTS.md section 3.3)
//!
//! Explicit and exhaustive, so nothing below is implied to exist:
//!
//! - **Critical arrows.** [`ProjectileKind::is_critical_capable`] only reports
//!   whether the kind admits the vanilla rule; the rule itself (a fully drawn bow,
//!   a non-riding shooter, a non-liquid, non-ladder origin) is not evaluated and
//!   no damage multiplier is applied. [`ProjectileKind::base_damage`] is the
//!   *minimum* non-critical figure, never the critical one.
//! - **Punch / knockback enchantment** and every other enchantment effect.
//! - **Arrow pickup.** No `pickup` state (`ALLOWED` / `CREATIVE_ONLY` /
//!   `DISALLOWED`), no "an arrow fired by an infinity bow is never recoverable",
//!   no player-creative special case.
//! - **Tipped and spectral arrows**, and the lingering-potion cloud a tipped
//!   arrow leaves. There is no effect-on-hit path.
//! - **Tridents**, thrown potions, ender pearls, eyes of ender, experience
//!   bottles, fireballs (large and small), wither skulls, shulker bullets, llama
//!   spit, fishing bobbers, wind charges and every other projectile type. Only
//!   [`ProjectileKind::Arrow`] and [`ProjectileKind::Snowball`] exist.
//! - **Eggs.** Vanilla gives an egg snowball physics and zero entity damage, and
//!   the difference (spawning a chick on a block hit) needs the world, so an
//!   `Egg` variant would be a physics entry this module cannot complete. Omitted
//!   deliberately rather than shipped as an unexercised variant.
//! - **Any per-entity-type collision *response*.** No hit detection, no
//!   penetration, no `onHitEntity`/`onHitBlock`, no bounce (a snowball bounces, an
//!   arrow sticks), no removal on impact, no damage application, no knockback
//!   impulse, no owner immunity. [`Projectile::step`] returns a position and the
//!   caller must test that swept segment against blocks and entities.
//! - **Fluids.** An arrow in water or lava does not slow down here, and it does
//!   not lose its critical state.
//! - **Item-based damage scaling.** Vanilla derives arrow damage from the fired
//!   stack's `weapon_damage` component and the bow's charge, so an arrow fired
//!   from a low-power bow is weaker than [`ARROW_BASE_DAMAGE`]; that value is a
//!   floor, not a computed result.
//!
//! ## Confidence in the constants
//!
//! Every constant states whether it is a vanilla value or an approximation. The
//! three a reader must treat with care are [`PROJECTILE_DRAG`]
//! (community-consistent, not read from a 26.1.2 source tree),
//! [`ARROW_BASE_DAMAGE`] (the pre-1.9 minimum, which is a floor for a *charged*
//! bow) and [`MAX_LIFETIME_SNOWBALL`] (an explicit placeholder, not a verified
//! vanilla figure).

use crate::entity::EntityId;
use mc_world::Vec3;
use std::fmt;

/// Downward velocity added to a projectile every tick, in blocks per tick squared.
///
/// **Approximation**, not a verified vanilla figure: vanilla adds a base gravity
/// of `0.03` and multiplies it by a per-entity gravity attribute, and an arrow's
/// effective value also passes through the projectile's own speed handling. This
/// module uses the base value; a projectile that should fall faster or slower
/// needs a per-kind override, not a different constant here.
pub const PROJECTILE_GRAVITY: f64 = 0.03;

/// Velocity retained per tick by a projectile in air (all three components).
///
/// **Confidence: medium.** The `0.99` air-drag factor is the widely documented
/// value for arrow-like projectiles (`getDeltaMovement().scale(0.99)` before the
/// move), and the `0.99`-vs-`0.98` distinction between a projectile and an item is
/// consistent with observed flight times, but it was **not** read from a 26.1.2
/// source tree during this task. Snowballs and eggs are documented as sharing the
/// arrow's drag; this module assumes that rather than proving it.
pub const PROJECTILE_DRAG: f64 = 0.99;

/// Lifetime of an arrow, in ticks.
///
/// Vanilla's `AbstractArrow` base tick count is `200` (10 seconds), after which
/// the arrow despawns. A stuck arrow in the ground uses the same counter.
pub const MAX_LIFETIME_ARROW: u32 = 200;

/// Lifetime of a snowball, in ticks.
///
/// **Confidence: low - a documented placeholder.** Vanilla removes a snowball
/// either on impact or after its own lifetime counter expires, and this module
/// could not verify that counter against 26.1.2. `100` (5 seconds) is chosen as a
/// conservative bound, not as a parity claim: it is short enough that an un-swept
/// snowball cannot accumulate, and a differential test that disagrees must change
/// this constant rather than the test.
pub const MAX_LIFETIME_SNOWBALL: u32 = 100;

/// Minimum non-critical damage of a charged arrow.
///
/// **Confidence: medium-low - a floor, not the vanilla formula.** Vanilla (since
/// 1.9) computes arrow damage from the bow's charge and the arrow's
/// `weapon_damage` component rather than from a fixed constant; a fully charged
/// bow is commonly documented as dealing `6` (a `2.0` base scaled by charge and
/// velocity, with the critical path multiplying further). `2.0` is the classic
/// base damage and is used here as the guaranteed **minimum** that a charge-scaled
/// calculation should never fall below. It must not be presented as the damage a
/// particular shot deals.
pub const ARROW_BASE_DAMAGE: f64 = 2.0;

/// What kind of projectile this is.
///
/// Closed by design: a variant is added when its trajectory *and* its collision
/// response exist, because a physics-only entry with no response is the fake
/// completeness AGENTS.md section 3.3 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProjectileKind {
    /// A bow- or dispenser-fired arrow (normal, not tipped or spectral).
    Arrow,
    /// A thrown snowball: harmless, but it still flies and it still expires.
    Snowball,
}

impl ProjectileKind {
    /// Every kind this module implements, in declaration order.
    pub const ALL: [Self; 2] = [Self::Arrow, Self::Snowball];

    /// Stable name for logs, metrics and tests.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Arrow => "arrow",
            Self::Snowball => "snowball",
        }
    }

    /// Downward acceleration this kind experiences, in blocks per tick squared.
    ///
    /// Both kinds currently share [`PROJECTILE_GRAVITY`]. It is a method rather
    /// than a bare constant so a kind that needs its own value - a fireball, say -
    /// splits one match arm instead of changing every call site.
    #[must_use]
    pub const fn gravity(self) -> f64 {
        match self {
            Self::Arrow | Self::Snowball => PROJECTILE_GRAVITY,
        }
    }

    /// Velocity retained per tick by this kind, in air.
    ///
    /// Both kinds currently share [`PROJECTILE_DRAG`], for the same reason as
    /// [`ProjectileKind::gravity`].
    #[must_use]
    pub const fn drag(self) -> f64 {
        match self {
            Self::Arrow | Self::Snowball => PROJECTILE_DRAG,
        }
    }

    /// Damage this kind deals to an entity it hits, before any caller-side
    /// scaling. See [`ARROW_BASE_DAMAGE`] for why the arrow value is a floor.
    ///
    /// A snowball deals **no** entity damage: vanilla's snowball only applies a
    /// knockback impulse (and damages a blaze). Returning `0.0` is the honest
    /// figure; the knockback is not modelled at all, so a snowball here is
    /// entirely harmless until the caller implements its hit response.
    #[must_use]
    pub const fn base_damage(self) -> f64 {
        match self {
            Self::Arrow => ARROW_BASE_DAMAGE,
            Self::Snowball => 0.0,
        }
    }

    /// Whether vanilla's critical-hit rule can apply to this kind.
    ///
    /// Reported only; nothing in this module evaluates the rule. `false` for a
    /// snowball, for which vanilla has no critical path at all.
    #[must_use]
    pub const fn is_critical_capable(self) -> bool {
        match self {
            Self::Arrow => true,
            Self::Snowball => false,
        }
    }

    /// Ticks this kind may exist before it is removed if nothing stops it.
    #[must_use]
    pub const fn max_lifetime(self) -> u32 {
        match self {
            Self::Arrow => MAX_LIFETIME_ARROW,
            Self::Snowball => MAX_LIFETIME_SNOWBALL,
        }
    }
}

impl fmt::Display for ProjectileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A live projectile's identity, owner and remaining lifetime.
///
/// Deliberately **not** a position holder: the position and velocity live in
/// [`crate::entity::Entity`], and [`Projectile::step`] takes them by value so the
/// same arithmetic can be replayed in a test without an entity at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Projectile {
    /// What is flying.
    pub kind: ProjectileKind,
    /// Who fired it, for damage attribution and for the owner immunity the caller
    /// must apply (a projectile must not hit the entity that fired it).
    pub owner: Option<EntityId>,
    /// Ticks of life remaining, counted down by [`Projectile::tick`].
    pub life: u32,
}

impl Projectile {
    /// A projectile that has not yet spent any of its lifetime.
    #[must_use]
    pub const fn new(kind: ProjectileKind, owner: Option<EntityId>) -> Self {
        Self {
            kind,
            owner,
            life: kind.max_lifetime(),
        }
    }

    /// This kind's gravity, in blocks per tick squared (see
    /// [`ProjectileKind::gravity`]).
    #[must_use]
    pub const fn gravity(&self) -> f64 {
        self.kind.gravity()
    }

    /// This kind's per-tick drag (see [`ProjectileKind::drag`]).
    #[must_use]
    pub const fn drag(&self) -> f64 {
        self.kind.drag()
    }

    /// This kind's pre-scaling damage (see [`ProjectileKind::base_damage`]).
    #[must_use]
    pub const fn base_damage(&self) -> f64 {
        self.kind.base_damage()
    }

    /// Whether this kind admits vanilla's critical-hit rule.
    #[must_use]
    pub const fn is_critical_capable(&self) -> bool {
        self.kind.is_critical_capable()
    }

    /// Whether the projectile's lifetime has run out.
    #[must_use]
    pub const fn is_expired(&self) -> bool {
        self.life == 0
    }

    /// Spend one tick of lifetime.
    ///
    /// This is the **lifetime counter only**. The trajectory is
    /// [`Projectile::step`], and a caller runs both: `tick()` decides when the
    /// projectile is abandoned, `step()` decides where it is. Saturates at zero,
    /// so an expired projectile stays expired and the counter cannot wrap.
    pub const fn tick(&mut self) {
        self.life = self.life.saturating_sub(1);
    }

    /// Integrate one tick of gravity and drag, returning `(position, velocity)`.
    ///
    /// `position` is the projectile's current position in blocks and `velocity`
    /// its current velocity in blocks per tick. The returned tuple is the position
    /// after this tick and the velocity to carry into the next; the caller stores
    /// both. The order matches vanilla's per-tick order for a projectile:
    ///
    /// 1. gravity is added to the vertical component ([`ProjectileKind::gravity`]),
    /// 2. all three components are scaled by the kind's drag
    ///    ([`ProjectileKind::drag`]),
    /// 3. the position advances by the velocity that resulted.
    ///
    /// So a horizontal shot loses height at an increasing rate and horizontal
    /// speed geometrically - the two behaviours this module's tests pin.
    ///
    /// ## The caller owns collision
    ///
    /// This function performs **no** hit test. The world query it would need
    /// (block lookups along the segment, entity boxes within the swept volume)
    /// lives in `mc-world` and is a separate subsystem, not an arithmetic step. A
    /// caller stepping a projectile must therefore, every tick:
    ///
    /// 1. keep the previous position,
    /// 2. call `step`,
    /// 3. test `previous ..= new` against blocks **and** against the entities it
    ///    may hit, skipping [`Projectile::owner`],
    /// 4. resolve the first hit itself: apply [`Projectile::base_damage`], any
    ///    knockback, the impact removal or the arrow-sticks-in-ground state.
    ///
    /// A caller that skips step 3 has an infinite-range hitscan, which is the
    /// single most damaging way to misuse this function.
    ///
    /// ## Hostile input
    ///
    /// A non-finite component of `position` or `velocity` is replaced with `0.0`
    /// before any arithmetic, and every output component is clamped to
    /// [`PROJECTILE_MAX_COMPONENT`]. Vanilla's arrows are already the fastest
    /// common entity (a fully charged bow is about 3 blocks per tick), so the
    /// clamp is orders of magnitude above legitimate flight while refusing a value
    /// that would cross a view distance in one tick. Dropping a bad tick to rest is
    /// chosen over propagating a `NaN`, which would otherwise poison the position
    /// for the entity's whole life and then persist into the chunk (AGENTS.md
    /// section 10).
    #[must_use]
    pub fn step(&self, position: Vec3, velocity: Vec3) -> (Vec3, Vec3) {
        let (px, py, pz) = sanitize(position);
        let (mut vx, mut vy, mut vz) = sanitize(velocity);
        vy -= self.kind.gravity();
        let drag = self.kind.drag();
        vx *= drag;
        vy *= drag;
        vz *= drag;
        let velocity = Vec3::new(
            vx.clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
            vy.clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
            vz.clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
        );
        let position = Vec3::new(
            (px + velocity.x).clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
            (py + velocity.y).clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
            (pz + velocity.z).clamp(-PROJECTILE_MAX_COMPONENT, PROJECTILE_MAX_COMPONENT),
        );
        (position, velocity)
    }
}

/// Largest magnitude any position or velocity component may reach, in blocks or
/// blocks per tick.
///
/// A Minecraft world spans roughly 30 million blocks in each direction; this
/// bound is tighter on purpose, because a projectile anywhere near it has already
/// escaped every legitimate launch and must not be allowed to keep integrating.
pub const PROJECTILE_MAX_COMPONENT: f64 = 10.0;

/// Replace non-finite components with `0.0`, leaving finite ones untouched.
///
/// See [`Projectile::step`] for why dropping is chosen over propagating.
fn sanitize(vector: Vec3) -> (f64, f64, f64) {
    let clean = |component: f64| {
        if component.is_finite() {
            component
        } else {
            0.0
        }
    };
    (clean(vector.x), clean(vector.y), clean(vector.z))
}

#[cfg(test)]
mod tests {
    // The constants and the lifespan boundaries are asserted exactly on purpose:
    // an epsilon there would hide exactly the drift those assertions exist to
    // catch. Simulated trajectories use an explicit epsilon instead.
    #![allow(clippy::float_cmp)]
    use super::{
        ARROW_BASE_DAMAGE, MAX_LIFETIME_ARROW, MAX_LIFETIME_SNOWBALL, PROJECTILE_DRAG,
        PROJECTILE_MAX_COMPONENT, Projectile, ProjectileKind,
    };
    use crate::entity::EntityId;
    use mc_world::Vec3;

    /// Trajectory tolerance, in blocks: `1e-9` is far below the smallest
    /// displacement that can matter at 20 TPS.
    const EPSILON: f64 = 1e-9;

    fn id(raw: i32) -> EntityId {
        EntityId::new(raw).expect("test ids are positive")
    }

    fn arrow(shooter: Option<EntityId>) -> Projectile {
        Projectile::new(ProjectileKind::Arrow, shooter)
    }

    #[test]
    fn kinds_expose_their_documented_constants() {
        assert_eq!(
            ProjectileKind::ALL,
            [ProjectileKind::Arrow, ProjectileKind::Snowball]
        );
        assert_eq!(ProjectileKind::Arrow.name(), "arrow");
        assert_eq!(ProjectileKind::Snowball.to_string(), "snowball");
        assert!(ProjectileKind::Arrow.gravity() > 0.0);
        assert!(ProjectileKind::Arrow.drag() < 1.0 && ProjectileKind::Arrow.drag() > 0.0);
        assert_eq!(ProjectileKind::Arrow.drag(), PROJECTILE_DRAG);
        assert_eq!(ProjectileKind::Arrow.base_damage(), ARROW_BASE_DAMAGE);
        assert_eq!(
            ProjectileKind::Snowball.base_damage(),
            0.0,
            "a snowball deals no entity damage in vanilla"
        );
        assert!(ProjectileKind::Arrow.is_critical_capable());
        assert!(!ProjectileKind::Snowball.is_critical_capable());
        assert_eq!(ProjectileKind::Arrow.max_lifetime(), MAX_LIFETIME_ARROW);
        assert_eq!(
            ProjectileKind::Snowball.max_lifetime(),
            MAX_LIFETIME_SNOWBALL
        );
    }

    #[test]
    fn a_fresh_projectile_is_not_expired() {
        let fresh = arrow(Some(id(1)));
        assert!(!fresh.is_expired());
        assert_eq!(fresh.life, MAX_LIFETIME_ARROW);
        assert_eq!(fresh.life, 200, "the documented arrow lifetime");
        assert_eq!(fresh.owner, Some(id(1)));
        assert_eq!(fresh.kind, ProjectileKind::Arrow);
        assert_eq!(fresh.gravity(), ProjectileKind::Arrow.gravity());
        assert_eq!(fresh.drag(), ProjectileKind::Arrow.drag());
        assert_eq!(fresh.base_damage(), ARROW_BASE_DAMAGE);
        assert!(fresh.is_critical_capable());

        let snowball = Projectile::new(ProjectileKind::Snowball, None);
        assert!(!snowball.is_expired());
        assert_eq!(snowball.life, MAX_LIFETIME_SNOWBALL);
        assert_eq!(snowball.owner, None);
        assert!(!snowball.is_critical_capable());
        assert_eq!(snowball.base_damage(), 0.0);
    }

    #[test]
    fn life_expires_at_the_documented_tick_and_never_wraps() {
        let mut flight = arrow(None);
        for tick in 1..MAX_LIFETIME_ARROW {
            flight.tick();
            assert_eq!(flight.life, MAX_LIFETIME_ARROW - tick);
            assert!(!flight.is_expired(), "still flying after {tick} ticks");
        }
        flight.tick();
        assert_eq!(flight.life, 0);
        assert!(
            flight.is_expired(),
            "an arrow expires on tick {MAX_LIFETIME_ARROW}"
        );
        // Further ticks keep it expired instead of wrapping the counter.
        for _ in 0..MAX_LIFETIME_ARROW * 2 {
            flight.tick();
        }
        assert!(flight.is_expired());
        assert_eq!(flight.life, 0);

        let mut snowball = Projectile::new(ProjectileKind::Snowball, None);
        for _ in 1..MAX_LIFETIME_SNOWBALL {
            snowball.tick();
        }
        assert!(!snowball.is_expired());
        snowball.tick();
        assert!(snowball.is_expired());
    }

    #[test]
    fn a_horizontal_arrow_loses_height_over_time() {
        let flight = arrow(Some(id(2)));
        let mut position = Vec3::new(0.0, 64.0, 0.0);
        // Roughly a fully charged bow: about 3 blocks per tick.
        let mut velocity = Vec3::new(3.0, 0.0, 0.0);
        let start_y = position.y;
        let mut previous_y = position.y;
        for tick in 1..=20 {
            let (next_position, next_velocity) = flight.step(position, velocity);
            position = next_position;
            velocity = next_velocity;
            assert!(
                position.y < previous_y,
                "gravity must win every tick: tick {tick} kept height {}",
                position.y
            );
            assert!(previous_y - position.y > 0.0);
            previous_y = position.y;
        }
        let drop = start_y - position.y;
        assert!(
            drop > 0.5,
            "a 20-tick horizontal shot must visibly fall, dropped only {drop} blocks"
        );
        assert!(
            velocity.y < 0.0,
            "vertical velocity stays negative once falling"
        );
    }

    #[test]
    fn drag_reduces_projectile_speed() {
        let flight = arrow(None);
        let mut position = Vec3::new(0.0, 64.0, 0.0);
        let mut velocity = Vec3::new(1.0, 0.0, 0.0);
        let mut previous_horizontal = velocity.length();
        for tick in 1..=20 {
            let (next_position, next_velocity) = flight.step(position, velocity);
            position = next_position;
            velocity = next_velocity;
            // Horizontal drag, not total speed: gravity accelerates the vertical
            // component, so the magnitude of the velocity can grow while the
            // projectile is still being slowed down horizontally.
            let horizontal = velocity.x.hypot(velocity.z);
            assert!(
                horizontal < previous_horizontal,
                "drag must reduce horizontal speed: tick {tick} went from \
                 {previous_horizontal} to {horizontal}"
            );
            assert!(
                horizontal < 1.0,
                "horizontal speed never exceeds the launch speed: tick {tick} is {horizontal}"
            );
            previous_horizontal = horizontal;
        }
        assert!(
            (velocity.x - PROJECTILE_DRAG.powi(20)).abs() < EPSILON,
            "the horizontal component is exactly drag^n"
        );
        assert!(
            velocity.y < 0.0,
            "gravity still accumulates on the vertical component"
        );
        assert!(
            velocity.z.abs() < EPSILON,
            "an untouched axis stays at zero"
        );
    }

    #[test]
    fn gravity_and_drag_are_applied_in_vanillas_order() {
        let flight = arrow(None);
        let (_, velocity) = flight.step(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0));
        let gravity = ProjectileKind::Arrow.gravity();
        // Gravity first, then drag: (1 - g) * drag.
        assert!((velocity.y - (1.0 - gravity) * PROJECTILE_DRAG).abs() < EPSILON);
        assert!(
            (velocity.y - (PROJECTILE_DRAG - gravity)).abs() > 1e-5,
            "the order is observable, so it must be the vanilla one"
        );
    }

    #[test]
    fn a_snowball_follows_the_same_trajectory_shape() {
        let ball = Projectile::new(ProjectileKind::Snowball, None);
        let (position, velocity) = ball.step(Vec3::new(0.0, 70.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        assert!(position.y < 70.0, "a snowball falls too");
        assert!(position.x > 0.0, "and travels forward");
        assert!(velocity.x < 1.0, "and is slowed by drag");
        assert_eq!(
            velocity.y,
            -ProjectileKind::Snowball.gravity() * PROJECTILE_DRAG
        );
    }

    #[test]
    fn non_finite_input_cannot_poison_the_trajectory() {
        let flight = arrow(None);
        for hostile in [
            Vec3::new(f64::NAN, f64::NAN, f64::NAN),
            Vec3::new(f64::INFINITY, f64::NEG_INFINITY, f64::NAN),
            Vec3::new(1.0, f64::NAN, f64::INFINITY),
        ] {
            let (position, velocity) = flight.step(hostile, hostile);
            assert!(
                position.is_finite() && velocity.is_finite(),
                "hostile input {hostile:?} produced {position:?} / {velocity:?}"
            );
            assert!(position.x.abs() <= PROJECTILE_MAX_COMPONENT);
            assert!(velocity.y.abs() <= PROJECTILE_MAX_COMPONENT);
        }
        // A NaN velocity with a finite position integrates to a finite step.
        let (position, velocity) =
            flight.step(Vec3::new(1.0, 2.0, 3.0), Vec3::new(f64::NAN, 0.0, 0.0));
        assert_eq!(position.x, 1.0, "a dropped NaN contributes no motion");
        assert!(position.y < 2.0, "gravity still applies");
        assert_eq!(velocity.x, 0.0);
    }

    #[test]
    fn absurd_but_finite_input_is_clamped() {
        let flight = arrow(None);
        let (position, velocity) = flight.step(
            Vec3::new(f64::MAX, f64::MIN, f64::MAX),
            Vec3::new(f64::MAX, f64::MIN, f64::MAX),
        );
        assert!(position.is_finite() && velocity.is_finite());
        assert!(position.x.abs() <= PROJECTILE_MAX_COMPONENT);
        assert!(velocity.y >= -PROJECTILE_MAX_COMPONENT);
    }

    #[test]
    fn projectiles_round_trip_clone_and_eq() {
        let original = arrow(Some(id(5)));
        let copy = original.clone();
        assert_eq!(copy, original);

        let mut moved = copy.clone();
        moved.tick();
        assert_ne!(moved, original, "life participates in equality");

        let mut reowned = copy.clone();
        reowned.owner = Some(id(6));
        assert_ne!(reowned, original, "the owner participates in equality");

        let mut rekinned = copy.clone();
        rekinned.kind = ProjectileKind::Snowball;
        assert_ne!(rekinned, original, "the kind participates in equality");
    }
}
