//! Mob kinds, their per-kind numbers, and the mob AI decision model (`P05-11`..`P05-14`).
//!
//! ## Scope, stated plainly
//!
//! This module owns exactly three things:
//!
//! 1. [`MobKind`] — the closed set of mobs this crate models, with the per-kind
//!    numbers ([`MobKind::max_health`], [`MobKind::dimensions`],
//!    [`MobKind::attack_damage`], [`MobKind::movement_speed`]).
//! 2. [`Mob`] — the payload [`crate::entity::EntityBody::Mob`] carries.
//! 3. [`MobAi`] — which goal the mob is pursuing, and [`MobAi::decide`], the
//!    decision that picks the next one.
//!
//! It does **not** move anything. Physics, collision, path *following*, head
//! rotation and the packets that show them belong to the tick loop; this module
//! answers "what does this mob want to do?" and stops there (AGENTS.md §3.4).
//! [`crate::pathfind`] answers the next question — "which blocks does that
//! imply?" — and is likewise pure.
//!
//! ## Verification status of every number (AGENTS.md §3.1)
//!
//! "Looks plausible" is not evidence, so each value is labelled:
//!
//! - **verified** — read off the public Java Edition wiki page for that mob
//!   (minecraft.wiki, fetched 2026-09-11 while this work item was being written);
//! - **task-supplied** — came with the work item and was **not** independently
//!   checked against a 26.1.2 baseline;
//! - **approximation** — this crate's own choice; it is *not* a Vanilla value.
//!
//! | Kind | Health | Width × Height | Behaviour | Attack (Normal) | Speed attr. |
//! |---|---|---|---|---|---|
//! | [`MobKind::Zombie`] | 20.0 verified | 0.6 × 1.95 verified | hostile | 3.0 melee verified | 0.23 verified |
//! | [`MobKind::Skeleton`] | 20.0 task-supplied | 0.6 × 1.99 task-supplied | hostile | 2.0 melee approximation | 0.25 task-supplied |
//! | [`MobKind::Cow`] | 10.0 task-supplied | 0.9 × 1.4 task-supplied | passive | — | 0.25 task-supplied |
//! | [`MobKind::Pig`] | 10.0 task-supplied | 0.9 × 0.9 task-supplied | passive | — | 0.25 task-supplied |
//! | [`MobKind::Sheep`] | 8.0 task-supplied | 0.9 × 1.3 task-supplied | passive | — | 0.23 task-supplied |
//! | [`MobKind::Chicken`] | 4.0 task-supplied | 0.4 × 0.7 task-supplied | passive | — | 0.25 task-supplied |
//! | [`MobKind::Spider`] | 16.0 verified | 1.4 × 0.9 task-supplied | hostile | 2.0 melee verified | 0.3 task-supplied |
//! | [`MobKind::Creeper`] | 20.0 task-supplied | 0.6 × 1.7 task-supplied | hostile | 49.0 explosion, see below | 0.25 task-supplied |
//!
//! The zombie row is the one row checked line by line: the wiki infobox gives
//! health 20, hitbox 0.6 × 1.95 (Java Edition), attack strength Easy 2.5 /
//! Normal 3 / Hard 4.5, and speed 0.23. The spider's health 16 and
//! Normal-difficulty damage 2 come from community documentation that cites the
//! game's own data, not from the wiki, so they are "verified" only in the sense
//! that an external source states them — they are **not** a 26.1.2 measurement.
//!
//! ## How movement speed is derived, and why it is still unverified
//!
//! [`MobKind::movement_speed`] is expressed as blocks per **tick**, because that
//! is the unit the simulation integrates in, but the constant behind it is
//! blocks per **second** ([`SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE`]) with the
//! tick conversion applied at the point of use:
//!
//! ```text
//! blocks/tick = MovementSpeedAttribute × 43.17 ÷ 20
//! ```
//!
//! 43.17 is the one documented anchor available: the wiki records a walking
//! player at 4.317 blocks/s, and a player's `MovementSpeedAttribute` base is 0.1,
//! so 4.317 ÷ 0.1 = 43.17 blocks/s per attribute unit.
//!
//! **That extrapolation is not verified and is probably too generous.** It
//! assumes mob speed scales linearly with the attribute, and it lands a zombie
//! (attribute 0.23) at 9.9 blocks/s, far above what a zombie actually achieves in
//! play — Vanilla derives movement inside `LivingEntity.travel`, and a mob only
//! approaches its steady-state speed on a long straight run with a live
//! navigation path. The number is therefore a *conversion*, not a measurement:
//! **any movement built on it must be measured on a real 26.1.2 server first.**
//! [`MobKind::movement_speed_attribute`] exposes the raw Vanilla attribute so the
//! conversion can be replaced without touching the table.
//!
//! ## Known gaps and deliberate approximations (AGENTS.md §3.3)
//!
//! - **No attack styles are implemented.** [`MobAttackStyle`] records how a mob
//!   attacks in Vanilla, but this crate has no bow, arrow, explosion or
//!   splash-potion code: a [`MobAttackStyle::Ranged`] or
//!   [`MobAttackStyle::Explosive`] mob's [`MobGoal::Attack`] is an *intent* the
//!   caller must resolve or refuse. In particular
//!   [`MobKind::Creeper`]'s [`MobKind::attack_damage`] is the documented
//!   point-blank **explosion** damage, not a melee value.
//! - **No per-mob follow range.** [`AGGRO_RADIUS`] is Vanilla's default
//!   `FOLLOW_RANGE`; mobs that override it in Vanilla (the zombie is widely
//!   reported to see further) are not modelled.
//! - **No line of sight, light level, day/night, difficulty scaling, anger or
//!   panic sources, baby variants, jockeys or equipment.**
//! - **Passive mobs flee on lost health rather than on being damaged.** Vanilla's
//!   panic goal triggers when a mob is hurt (a `lastHurtByMob` event); the AI here
//!   sees only a health fraction, so [`FLEE_HEALTH_FRACTION`] stands in for it.
//! - **No `RandomStrollGoal` parity.** Vanilla strolls probabilistically per tick
//!   and runs until its path is exhausted; the walk here is a *time-bounded* goal
//!   with a decision boundary, because this crate has no path-following state.
//! - **The AI has no memory beyond [`MobAi`]:** no path, no target history, no
//!   revenge timer.
//!
//! ## Determinism (AGENTS.md §3.6)
//!
//! [`MobAi::decide`] reads only its arguments, [`MobAi`]'s own fields and
//! [`Rng`]. Draws happen in a fixed order and only on a decision boundary, so the
//! same mob state plus the same observation sequence plus the same seed gives the
//! same goal sequence.

use crate::entity::EntityId;

/// Simulation ticks per second (Vanilla's fixed tick rate).
pub const TICKS_PER_SECOND: f64 = 20.0;

/// Blocks per second per unit of `MovementSpeedAttribute`.
///
/// **Derived, not Vanilla-published, and unverified for mobs.** A walking player
/// moves at 4.317 blocks/s (wiki-documented) with a `MovementSpeedAttribute` base
/// of 0.1, giving `4.317 ÷ 0.1 = 43.17`. The linear extrapolation from that one
/// player data point to other mobs is this crate's assumption; see the module
/// documentation before using it to move anything.
pub const SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE: f64 = 43.17;
// Pinned by `the_speed_constant_is_the_documented_player_derivation` in the
// tests module below: changing it is a deliberate recalibration, not an
// accident (Audit 08, L1).

/// Distance in blocks at which a hostile mob acquires a player as its target.
///
/// Natural reading: Vanilla's default `FOLLOW_RANGE` attribute (16.0). Per-mob
/// overrides are not modelled (module gap list). Approximation.
pub const AGGRO_RADIUS: f64 = 16.0;

/// Distance in blocks at which a hostile mob gives up a target it already has.
///
/// **This crate's own design choice, not a Vanilla rule.** Vanilla acquires and
/// retains on the same follow range; a wider retention radius is used here so a
/// player standing exactly on [`AGGRO_RADIUS`] cannot make the mob flicker
/// between `Chase` and `Wander` every tick.
pub const TARGET_LOSE_RADIUS: f64 = 24.0;

/// Distance in blocks at which a hostile mob stops chasing and swings.
///
/// Approximation: Vanilla computes melee reach from both hitboxes
/// (`MeleeAttackGoal`), which this crate does not model.
pub const ATTACK_RANGE: f64 = 2.0;

/// Distance in blocks at which a hurt passive mob flees.
///
/// Approximation of Vanilla's panic goal, which reacts to the *attacker's*
/// position (module gap list).
pub const FLEE_RADIUS: f64 = 8.0;

/// Health fraction at or below which a passive mob flees a nearby player.
///
/// **This crate's own threshold, not a Vanilla rule**: Vanilla panics on being
/// damaged rather than below a health fraction (module gap list).
pub const FLEE_HEALTH_FRACTION: f32 = 0.5;

/// Ticks between melee swings of one mob.
///
/// Approximation of Vanilla's roughly one-second melee cadence (a mob's attack
/// cooldown and `attackInterval`), which depends on the mob and its equipment.
pub const ATTACK_COOLDOWN_TICKS: u32 = 20;

/// Ticks between AI decision boundaries.
///
/// A mob only considers *starting a walk* on these boundaries, so the draws that
/// shape a stroll happen at a fixed phase and a replay is reproducible.
/// Approximation.
pub const DECISION_INTERVAL_TICKS: u64 = 10;

/// How long one walk lasts before the mob re-decides, in ticks (6 s).
///
/// Approximation, chosen to match the interval Vanilla's stroll goal uses. See
/// the module gap list for why the walk is time-bounded here.
pub const WALK_DURATION_TICKS: u32 = 120;

/// Largest offset, in blocks, of a random walk destination from the mob.
///
/// Natural reading: Vanilla's `RandomStrollGoal` picks a destination within ±10
/// blocks on each horizontal axis. The vertical axis is deliberately not offset:
/// choosing a y that needs a jump is [`crate::pathfind`]'s job, not the AI's.
pub const WANDER_RADIUS: i32 = 10;

/// One-in-N chance, per decision boundary, that an idle mob starts a walk.
///
/// Approximation: chosen so the expected wait is
/// `WANDER_CHANCE_ONE_IN × DECISION_INTERVAL_TICKS == 120` ticks, the interval
/// Vanilla's stroll goal uses.
pub const WANDER_CHANCE_ONE_IN: i32 = 12;

/// The AI's target retention must strictly exceed its acquisition radius, or a
/// player standing on the acquisition boundary makes the mob flicker between
/// `Chase` and `Wander` every tick. Checked at compile time because both values
/// are constants: a future edit that breaks it stops the build.
const _: () = assert!(TARGET_LOSE_RADIUS > AGGRO_RADIUS);

/// The randomness the AI may consume.
///
/// This trait is the crate boundary in the same way [`crate::pathfind::BlockView`]
/// is: `mc-entity` may not depend on `mc-simulation`, which owns the seeded
/// `java.util.Random` clone the tick loop uses, so the AI names the *contract*
/// instead of the concrete type. One `impl` in the simulation crate wires the two
/// together.
///
/// Implementations **must** be a pure function of their own state — the same seed
/// must produce the same sequence — or AGENTS.md §3.6 (deterministic simulation)
/// cannot hold.
///
/// The trait deliberately declares only the draw the AI uses today; a goal that
/// needs a float or a coin flip adds its method here and to the one `impl`, rather
/// than faking it out of [`Rng::next_i32_bounded`] (AGENTS.md §3.4).
pub trait Rng {
    /// A uniform draw in `0..bound`.
    ///
    /// Returns `0` for a non-positive `bound` rather than panicking: the bound is
    /// reachable from configuration and a wrong value must not take the server
    /// down (AGENTS.md §9).
    fn next_i32_bounded(&mut self, bound: i32) -> i32;
}

/// The closed set of mobs this crate models.
///
/// Vanilla has far more; each variant here is one the server can spawn, save,
/// walk and attack. Adding one means adding a variant and its row in the module
/// table (AGENTS.md §3.3: no silent "generic mob" fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobKind {
    /// Undead melee mob, burns in daylight (daylight is not modelled).
    Zombie,
    /// Undead ranged mob (the bow is not modelled; see [`MobAttackStyle`]).
    Skeleton,
    /// Passive source of leather and beef.
    Cow,
    /// Passive source of porkchops.
    Pig,
    /// Passive source of wool and mutton.
    Sheep,
    /// Passive source of chicken and feathers.
    Chicken,
    /// Hostile mob that can climb walls in Vanilla (climbing is not modelled).
    Spider,
    /// Hostile mob that destroys blocks on detonation (the explosion is not modelled).
    Creeper,
}

impl MobKind {
    /// Every kind, in Vanilla's rough spawn-table order, for deterministic iteration.
    pub const ALL: [Self; 8] = [
        Self::Zombie,
        Self::Skeleton,
        Self::Cow,
        Self::Pig,
        Self::Sheep,
        Self::Chicken,
        Self::Spider,
        Self::Creeper,
    ];

    /// Stable lowercase name for logs, metrics and commands.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Zombie => "zombie",
            Self::Skeleton => "skeleton",
            Self::Cow => "cow",
            Self::Pig => "pig",
            Self::Sheep => "sheep",
            Self::Chicken => "chicken",
            Self::Spider => "spider",
            Self::Creeper => "creeper",
        }
    }

    /// Maximum health, in half-hearts (Vanilla's `MAX_HEALTH` attribute base).
    ///
    /// See the module table for which rows were verified against a source.
    #[must_use]
    pub const fn max_health(self) -> f32 {
        match self {
            Self::Zombie | Self::Skeleton | Self::Creeper => 20.0,
            Self::Cow | Self::Pig => 10.0,
            Self::Sheep => 8.0,
            Self::Chicken => 4.0,
            Self::Spider => 16.0,
        }
    }

    /// Hitbox size as `(width, height)` in blocks, feet-centre convention.
    ///
    /// This is the entity's **bounding box**, not its visual model: a zombie is
    /// 0.6 × 1.95 in Java Edition even though its model is thinner.
    #[must_use]
    pub const fn dimensions(self) -> (f32, f32) {
        match self {
            Self::Zombie => (0.6, 1.95),
            Self::Skeleton => (0.6, 1.99),
            Self::Cow => (0.9, 1.4),
            Self::Pig => (0.9, 0.9),
            Self::Sheep => (0.9, 1.3),
            Self::Chicken => (0.4, 0.7),
            Self::Spider => (1.4, 0.9),
            Self::Creeper => (0.6, 1.7),
        }
    }

    /// Whole blocks of vertical space the mob needs to occupy a cell, for
    /// [`crate::pathfind::SearchLimits::for_mob`].
    ///
    /// Derived from [`MobKind::dimensions`] as `ceil(height)`, floored at 1: a
    /// chicken (0.7) needs one block, a zombie (1.95) needs two. The mob's
    /// *width* is not modelled by the pathfinder (see its gap list).
    #[must_use]
    pub const fn clearance(self) -> u8 {
        let (_, height) = self.dimensions();
        let blocks = height.ceil() as u8;
        if blocks < 1 { 1 } else { blocks }
    }

    /// Whether this kind attacks players on sight.
    #[must_use]
    pub const fn is_hostile(self) -> bool {
        matches!(
            self,
            Self::Zombie | Self::Skeleton | Self::Spider | Self::Creeper
        )
    }

    /// The goal set this kind may use.
    #[must_use]
    pub const fn behaviour(self) -> MobBehaviour {
        if self.is_hostile() {
            MobBehaviour::Hostile
        } else {
            MobBehaviour::Passive
        }
    }

    /// How this kind attacks, or `None` when it never does.
    ///
    /// `Some(style)` exactly when [`MobKind::is_hostile`], which the tests assert:
    /// a hostile mob with no attack and a passive mob with one would both be
    /// table bugs.
    #[must_use]
    pub const fn attack_style(self) -> Option<MobAttackStyle> {
        match self {
            Self::Zombie | Self::Spider => Some(MobAttackStyle::Melee),
            Self::Skeleton => Some(MobAttackStyle::Ranged),
            Self::Creeper => Some(MobAttackStyle::Explosive),
            Self::Cow | Self::Pig | Self::Sheep | Self::Chicken => None,
        }
    }

    /// Damage this kind's attack deals to a player on **Normal** difficulty,
    /// with the player at point-blank range.
    ///
    /// Difficulty scaling (Easy `0.5X + 1`, Hard `1.5X`) and armour are not
    /// modelled, and neither is distance falloff — which is why the creeper's
    /// value is the point-blank figure. Read [`MobKind::attack_style`] before
    /// applying this number: only [`MobAttackStyle::Melee`] may be applied as a
    /// direct hit, and the other styles have no implementation in this crate.
    ///
    /// `0.0` exactly when [`MobKind::attack_style`] is `None`.
    #[must_use]
    #[allow(
        clippy::match_same_arms,
        reason = "the spider's 2.0 is a verified melee value and the skeleton's 2.0 is an unverified stand-in for its bow: merging the arms would hide which of the two is checked against a source"
    )]
    pub const fn attack_damage(self) -> f32 {
        match self {
            Self::Zombie => 3.0,
            Self::Spider => 2.0,
            // Vanilla skeletons fight with a bow (1..=4 at Normal difficulty);
            // this is the skeleton's melee `ATTACK_DAMAGE` attribute instead,
            // because the bow is not modelled. Approximation.
            Self::Skeleton => 2.0,
            // Vanilla's creeper has no melee attack: 49.0 is the Normal-difficulty
            // explosion damage with the player adjacent, from community
            // documentation. An explosion implementation must not treat it as a
            // melee value. Approximation.
            Self::Creeper => 49.0,
            Self::Cow | Self::Pig | Self::Sheep | Self::Chicken => 0.0,
        }
    }

    /// The raw Vanilla `MovementSpeedAttribute` base value.
    ///
    /// Exposed so that a verified attribute → blocks/tick conversion can replace
    /// [`SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE`] without re-deriving this table.
    /// See the module table for verification status per row.
    #[must_use]
    #[allow(
        clippy::match_same_arms,
        reason = "zombie 0.23 is wiki-verified and sheep 0.23 is task-supplied; separate arms keep the module table's verification column auditable"
    )]
    pub const fn movement_speed_attribute(self) -> f64 {
        match self {
            Self::Zombie => 0.23,
            Self::Sheep => 0.23,
            Self::Spider => 0.3,
            Self::Skeleton | Self::Cow | Self::Pig | Self::Chicken | Self::Creeper => 0.25,
        }
    }

    /// Horizontal walking speed in **blocks per second**.
    ///
    /// [`MobKind::movement_speed_attribute`] ×
    /// [`SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE`]. The constant is derived from the
    /// documented player figure; the extrapolation to mobs is unverified (module
    /// documentation).
    #[must_use]
    pub const fn movement_speed_blocks_per_second(self) -> f64 {
        self.movement_speed_attribute() * SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE
    }

    /// Horizontal walking speed in **blocks per tick**, the unit the simulation
    /// integrates in.
    ///
    /// [`MobKind::movement_speed_blocks_per_second`] ÷ [`TICKS_PER_SECOND`]. Both
    /// the conversion and most of the attributes are unverified — see the module
    /// documentation before using this to move anything.
    #[must_use]
    pub const fn movement_speed(self) -> f64 {
        self.movement_speed_blocks_per_second() / TICKS_PER_SECOND
    }
}

impl std::fmt::Display for MobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// How a mob attacks, so a caller can refuse to resolve styles it has no code for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobAttackStyle {
    /// A direct hit at melee range for [`MobKind::attack_damage`].
    Melee,
    /// A projectile (Vanilla: a bow). **Not implemented**: no projectile code.
    Ranged,
    /// An area explosion (Vanilla: the creeper's fuse). **Not implemented**.
    Explosive,
}

impl MobAttackStyle {
    /// Stable lowercase name for logs and metrics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Melee => "melee",
            Self::Ranged => "ranged",
            Self::Explosive => "explosive",
        }
    }

    /// Whether this crate can resolve the attack as a direct hit today.
    ///
    /// `true` for [`MobAttackStyle::Melee`] only. Callers must check this before
    /// applying [`MobKind::attack_damage`]: the ranged and explosive numbers are
    /// documented Vanilla figures, not melee damage.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::Melee)
    }
}

/// The goal set a mob kind is allowed to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobBehaviour {
    /// Attacks players: [`MobGoalKind::Idle`], [`MobGoalKind::Wander`],
    /// [`MobGoalKind::Chase`], [`MobGoalKind::Attack`].
    Hostile,
    /// Never attacks: [`MobGoalKind::Idle`], [`MobGoalKind::Wander`],
    /// [`MobGoalKind::Flee`].
    Passive,
}

/// The goals a hostile mob may use, in the order [`MobBehaviour::goals`] returns.
const HOSTILE_GOALS: &[MobGoalKind] = &[
    MobGoalKind::Idle,
    MobGoalKind::Wander,
    MobGoalKind::Chase,
    MobGoalKind::Attack,
];

/// The goals a passive mob may use.
const PASSIVE_GOALS: &[MobGoalKind] = &[MobGoalKind::Idle, MobGoalKind::Wander, MobGoalKind::Flee];

impl MobBehaviour {
    /// Whether this behaviour attacks players.
    #[must_use]
    pub const fn is_hostile(self) -> bool {
        matches!(self, Self::Hostile)
    }

    /// Every goal this behaviour may use, in ascending priority order.
    #[must_use]
    pub const fn goals(self) -> &'static [MobGoalKind] {
        match self {
            Self::Hostile => HOSTILE_GOALS,
            Self::Passive => PASSIVE_GOALS,
        }
    }

    /// Whether this behaviour may use `goal`.
    ///
    /// The AI never produces a goal outside this set; the tests assert that over
    /// long decision sequences, so "a cow cannot chase" is checked rather than
    /// assumed.
    #[must_use]
    pub fn allows(self, goal: MobGoalKind) -> bool {
        self.goals().contains(&goal)
    }
}

/// The tag of a [`MobGoal`], without its payload.
///
/// Exists so behaviour checks and tests can talk about "a chase" without
/// inventing an [`EntityId`], and so [`MobBehaviour::allows`] is a total match
/// rather than a chain of `matches!` arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobGoalKind {
    /// Standing still, waiting for the next decision boundary.
    Idle,
    /// Walking to a block position.
    Wander,
    /// Moving towards a target entity.
    Chase,
    /// In range of a target, swinging.
    Attack,
    /// Running away from an entity.
    Flee,
}

impl MobGoalKind {
    /// Stable lowercase name for logs and metrics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Wander => "wander",
            Self::Chase => "chase",
            Self::Attack => "attack",
            Self::Flee => "flee",
        }
    }
}

/// What a mob is currently trying to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobGoal {
    /// Do nothing this tick.
    Idle,
    /// Walk towards `target`, a block position (the mob's feet cell).
    Wander {
        /// Destination block position.
        target: (i32, i32, i32),
    },
    /// Move towards a target entity; the caller resolves its position.
    Chase {
        /// Entity being chased.
        target: EntityId,
    },
    /// Swing at a target entity that is in range.
    ///
    /// `cooldown` counts down to the next allowed swing; the caller resolves the
    /// hit (see [`MobAttackStyle::is_implemented`]) and calls
    /// [`MobAi::note_attack_landed`].
    Attack {
        /// Entity being attacked.
        target: EntityId,
        /// Ticks until the next swing is allowed.
        cooldown: u32,
    },
    /// Run away from an entity; the caller resolves its position.
    Flee {
        /// Entity being fled from.
        from: EntityId,
    },
}

impl MobGoal {
    /// Which goal this is, without its payload.
    #[must_use]
    pub const fn kind(self) -> MobGoalKind {
        match self {
            Self::Idle => MobGoalKind::Idle,
            Self::Wander { .. } => MobGoalKind::Wander,
            Self::Chase { .. } => MobGoalKind::Chase,
            Self::Attack { .. } => MobGoalKind::Attack,
            Self::Flee { .. } => MobGoalKind::Flee,
        }
    }

    /// The entity this goal is about, for [`Chase`](Self::Chase),
    /// [`Attack`](Self::Attack) and [`Flee`](Self::Flee).
    #[must_use]
    pub const fn target(self) -> Option<EntityId> {
        match self {
            Self::Chase { target } | Self::Attack { target, .. } => Some(target),
            Self::Flee { from } => Some(from),
            Self::Idle | Self::Wander { .. } => None,
        }
    }

    /// The block position this goal is about, for [`Wander`](Self::Wander).
    #[must_use]
    pub const fn wander_target(self) -> Option<(i32, i32, i32)> {
        match self {
            Self::Wander { target } => Some(target),
            _ => None,
        }
    }
}

/// One player a mob can currently perceive.
///
/// The AI never looks at the world or the entity store itself: the caller (the
/// tick loop, which owns both) passes this in. That is what keeps [`MobAi::decide`]
/// testable without a world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MobSighting {
    /// The player's entity id.
    pub id: EntityId,
    /// Distance in blocks from the mob to that player.
    pub distance: f64,
}

impl MobSighting {
    /// A sighting at `distance` blocks.
    #[must_use]
    pub const fn new(id: EntityId, distance: f64) -> Self {
        Self { id, distance }
    }

    /// Whether this sighting can be used at all.
    ///
    /// A non-finite or negative distance is refused rather than clamped: it means
    /// the caller's own maths went wrong (a squared distance that was never
    /// rooted, a poisoned float), and inventing "at zero blocks" from it would
    /// let a hostile mob attack from anywhere (AGENTS.md §9).
    #[must_use]
    pub fn is_usable(self) -> bool {
        self.distance.is_finite() && self.distance >= 0.0
    }
}

/// Everything [`MobAi::decide`] is allowed to look at.
///
/// Passed by value and `Copy`, so a test can drive the AI without a world, an
/// entity store or a clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MobObservation {
    /// The mob's own feet position as a block position, needed to roll a walk
    /// destination. The AI has no other way to know where it is.
    pub position: (i32, i32, i32),
    /// The nearest player the caller considers perceptible, or `None`.
    ///
    /// The caller may pass the true nearest player regardless of distance: every
    /// radius in this module is applied here, so range filtering is the AI's job.
    pub nearest_player: Option<MobSighting>,
    /// Health as a fraction of [`MobKind::max_health`], `0.0..=1.0`.
    pub health_fraction: f32,
    /// The current tick, which fixes the phase of the decision boundary.
    pub tick: u64,
}

impl MobObservation {
    /// An observation.
    #[must_use]
    pub const fn new(
        position: (i32, i32, i32),
        nearest_player: Option<MobSighting>,
        health_fraction: f32,
        tick: u64,
    ) -> Self {
        Self {
            position,
            nearest_player,
            health_fraction,
            tick,
        }
    }

    /// The nearest player, when it is usable at all.
    #[must_use]
    fn threat(self) -> Option<MobSighting> {
        // `Option::filter` hands the predicate a reference, and `is_usable` takes
        // `self` because `MobSighting` is `Copy` (clippy: trivially_copy_pass_by_ref).
        self.nearest_player.filter(|sighting| sighting.is_usable())
    }

    /// Health fraction, clamped into `0.0..=1.0`.
    ///
    /// A non-finite value reads as `1.0` ("unhurt"): the only decision it feeds is
    /// the passive flee test, and defaulting to `0.0` would make a mob whose health
    /// field was corrupted flee forever.
    #[must_use]
    fn health(self) -> f32 {
        if self.health_fraction.is_finite() {
            self.health_fraction.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// A mob's AI state.
///
/// Three pieces of memory, each with a job:
///
/// - [`MobAi::cooldown`] — ticks left in the current **walk**;
/// - [`MobAi::wander_target`] — the destination of the most recent walk, kept so
///   a walk interrupted by a chase resumes instead of re-rolling;
/// - [`MobAi::last_target`] — the most recent entity goal target, which is what
///   implements the target-retention hysteresis ([`TARGET_LOSE_RADIUS`]).
///
/// ## Calling convention
///
/// [`MobAi::decide`] must be called **once per tick per mob**, in ascending
/// entity id (the tick loop's own ordering rule). It decrements this tick's
/// timers, so calling it twice in one tick makes the mob age twice as fast —
/// a caller bug the AI cannot detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobAi {
    /// What the mob is doing now.
    pub goal: MobGoal,
    /// Destination of the most recent walk (see the type-level note).
    pub wander_target: Option<(i32, i32, i32)>,
    /// Ticks left in the current walk; `0` when the mob is not walking.
    pub cooldown: u32,
    /// Most recent entity a goal was about.
    pub last_target: Option<EntityId>,
}

impl MobAi {
    /// A fresh AI: idle, no target, no walk.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            goal: MobGoal::Idle,
            wander_target: None,
            cooldown: 0,
            last_target: None,
        }
    }

    /// Whether the mob will consider a *new* walk this tick.
    ///
    /// `true` when no walk is in flight and the tick is a decision boundary. The
    /// goal-based decisions (chase, attack, flee) are not gated by this: a mob
    /// that sees a player reacts on the tick it sees them.
    #[must_use]
    pub const fn can_start_walk(&self, tick: u64) -> bool {
        self.cooldown == 0 && tick.is_multiple_of(DECISION_INTERVAL_TICKS)
    }

    /// Choose this tick's goal from `observation`, updating the AI's state.
    ///
    /// Rules, in order (all radii are in blocks):
    ///
    /// 1. **Timers.** The walk timer and any attack cooldown lose one tick.
    /// 2. **Hostile target.** The nearest player is a target when it is inside
    ///    [`AGGRO_RADIUS`], or inside [`TARGET_LOSE_RADIUS`] *and* already
    ///    [`MobAi::last_target`]. Inside [`ATTACK_RANGE`] the goal is
    ///    [`MobGoal::Attack`], otherwise [`MobGoal::Chase`].
    /// 3. **Passive threat.** A player inside [`FLEE_RADIUS`] while health is at
    ///    or below [`FLEE_HEALTH_FRACTION`] gives [`MobGoal::Flee`].
    /// 4. **Otherwise a walk** (see [`MobAi::can_start_walk`]): a walk in flight
    ///    continues, a finished walk is replaced with a new random destination
    ///    with probability `1 / `[`WANDER_CHANCE_ONE_IN`], and otherwise the mob
    ///    is [`MobGoal::Idle`].
    ///
    /// The result is also stored in [`MobAi::goal`] and returned, so a caller can
    /// either ignore the return value or act on it without re-reading the field.
    pub fn decide(
        &mut self,
        kind: MobKind,
        observation: MobObservation,
        rng: &mut impl Rng,
    ) -> MobGoal {
        self.tick_timers();
        let behaviour = kind.behaviour();
        let threat = observation.threat().filter(|sighting| match behaviour {
            MobBehaviour::Hostile => {
                let radius = if self.last_target == Some(sighting.id) {
                    TARGET_LOSE_RADIUS
                } else {
                    AGGRO_RADIUS
                };
                sighting.distance <= radius
            }
            MobBehaviour::Passive => {
                sighting.distance <= FLEE_RADIUS && observation.health() <= FLEE_HEALTH_FRACTION
            }
        });

        let next = match (behaviour, threat) {
            (MobBehaviour::Hostile, Some(sighting)) => {
                if sighting.distance <= ATTACK_RANGE {
                    // A swing already in progress keeps its countdown; a fresh
                    // target swings as soon as it is in range.
                    let cooldown = match self.goal {
                        MobGoal::Attack { target, cooldown } if target == sighting.id => cooldown,
                        _ => 0,
                    };
                    MobGoal::Attack {
                        target: sighting.id,
                        cooldown,
                    }
                } else {
                    MobGoal::Chase {
                        target: sighting.id,
                    }
                }
            }
            (MobBehaviour::Passive, Some(sighting)) => MobGoal::Flee { from: sighting.id },
            (_, None) => self.walk(observation, rng),
        };

        if let Some(target) = next.target() {
            self.last_target = Some(target);
        }
        self.goal = next;
        next
    }

    /// Record that the caller resolved an [`MobGoal::Attack`] into a hit.
    ///
    /// Restarts the swing timer, so the next swing is one
    /// [`ATTACK_COOLDOWN_TICKS`] away. A no-op for every other goal, so a caller
    /// cannot reset a cooldown it is not in.
    pub fn note_attack_landed(&mut self) {
        if let MobGoal::Attack { cooldown, .. } = &mut self.goal {
            *cooldown = ATTACK_COOLDOWN_TICKS;
        }
    }

    /// Decrement this tick's timers.
    fn tick_timers(&mut self) {
        self.cooldown = self.cooldown.saturating_sub(1);
        if let MobGoal::Attack { cooldown, .. } = &mut self.goal {
            *cooldown = cooldown.saturating_sub(1);
        }
    }

    /// Pick the walk goal: continue, resume, start, or idle.
    fn walk(&mut self, observation: MobObservation, rng: &mut impl Rng) -> MobGoal {
        // A walk in flight — or one interrupted by a chase — keeps its
        // destination and its timer.
        if self.cooldown > 0 {
            if let Some(target) = self.wander_target {
                return MobGoal::Wander { target };
            }
            return MobGoal::Idle;
        }
        if !self.can_start_walk(observation.tick) {
            return MobGoal::Idle;
        }
        if rng.next_i32_bounded(WANDER_CHANCE_ONE_IN) != 0 {
            return MobGoal::Idle;
        }
        let target = roll_destination(observation.position, rng);
        self.wander_target = Some(target);
        self.cooldown = WALK_DURATION_TICKS;
        MobGoal::Wander { target }
    }
}

impl Default for MobAi {
    /// Equivalent to [`MobAi::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Roll a destination within [`WANDER_RADIUS`] on each horizontal axis.
///
/// Two draws, x before z, always: the order is part of the deterministic
/// contract. The y coordinate is unchanged — [`crate::pathfind`] decides whether
/// the mob has to step up or fall to get there.
fn roll_destination(position: (i32, i32, i32), rng: &mut impl Rng) -> (i32, i32, i32) {
    let dx = rng.next_i32_bounded(WANDER_RADIUS * 2 + 1) - WANDER_RADIUS;
    let dz = rng.next_i32_bounded(WANDER_RADIUS * 2 + 1) - WANDER_RADIUS;
    (
        position.0.saturating_add(dx),
        position.1,
        position.2.saturating_add(dz),
    )
}

/// A mob: what it is, and what it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mob {
    /// Which mob this is.
    pub kind: MobKind,
    /// Its AI state.
    pub ai: MobAi,
}

impl Mob {
    /// A mob of `kind`, idle. Health is not stored here: it lives on
    /// [`crate::entity::Entity`], initialised from [`MobKind::max_health`].
    #[must_use]
    pub const fn new(kind: MobKind) -> Self {
        Self {
            kind,
            ai: MobAi::new(),
        }
    }

    /// Which mob this is.
    #[must_use]
    pub const fn kind(&self) -> MobKind {
        self.kind
    }

    /// The goal set this mob may use.
    #[must_use]
    pub const fn behaviour(&self) -> MobBehaviour {
        self.kind.behaviour()
    }
}

#[cfg(test)]
mod tests {
    // Health, damage and the speed table are checked exactly on purpose: an
    // epsilon would hide the table typo the assertions exist to catch.
    #![allow(clippy::float_cmp)]
    use super::{
        AGGRO_RADIUS, ATTACK_COOLDOWN_TICKS, ATTACK_RANGE, DECISION_INTERVAL_TICKS,
        FLEE_HEALTH_FRACTION, FLEE_RADIUS, Mob, MobAi, MobAttackStyle, MobBehaviour, MobGoal,
        MobGoalKind, MobKind, MobObservation, MobSighting, Rng,
        SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE, TARGET_LOSE_RADIUS, WALK_DURATION_TICKS,
        WANDER_RADIUS,
    };

    // Audit 08 (L1): the constant is a derivation from the documented player
    // figure, unverified against vanilla (module table). Pinning it here makes
    // any change a deliberate, reviewable recalibration.
    #[test]
    fn the_speed_constant_is_the_documented_player_derivation() {
        assert_eq!(SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE, 43.17);
        // and the derived table really is attribute x constant:
        for kind in [
            MobKind::Zombie,
            MobKind::Sheep,
            MobKind::Spider,
            MobKind::Skeleton,
            MobKind::Cow,
            MobKind::Pig,
            MobKind::Chicken,
            MobKind::Creeper,
        ] {
            let expected = kind.movement_speed_attribute() * SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE;
            assert_eq!(kind.movement_speed_blocks_per_second(), expected);
        }
    }
    use crate::entity::EntityId;

    /// `java.util.Random`'s 48-bit LCG, for driving the AI from a fixed seed.
    ///
    /// The tick loop passes its own `Rng` impl (the same algorithm, verified
    /// against the JDK). This copy exists only because `mc-entity` may not depend
    /// on `mc-simulation` and the tests need a source whose sequence a seed fixes.
    #[derive(Debug)]
    struct SeededRandom {
        seed: u64,
    }

    impl SeededRandom {
        const MULTIPLIER: u64 = 0x0005_DEEC_E66D;
        const ADDEND: u64 = 0xB;
        const MASK: u64 = (1 << 48) - 1;

        fn new(seed: i64) -> Self {
            Self {
                seed: (seed as u64 ^ Self::MULTIPLIER) & Self::MASK,
            }
        }

        fn next(&mut self, bits: u32) -> u32 {
            self.seed = self
                .seed
                .wrapping_mul(Self::MULTIPLIER)
                .wrapping_add(Self::ADDEND)
                & Self::MASK;
            (self.seed >> (48 - bits)) as u32
        }
    }

    impl Rng for SeededRandom {
        fn next_i32_bounded(&mut self, bound: i32) -> i32 {
            if bound <= 0 {
                return 0;
            }
            if bound & (bound - 1) == 0 {
                return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
            }
            loop {
                let bits = self.next(31);
                let value = bits % bound as u32;
                let test = bits.wrapping_sub(value).wrapping_add((bound - 1) as u32);
                if (test as i32) >= 0 {
                    return value as i32;
                }
            }
        }
    }

    fn player() -> EntityId {
        EntityId::new(1).expect("id 1 is positive")
    }

    fn other_player() -> EntityId {
        EntityId::new(2).expect("id 2 is positive")
    }

    fn observation(distance: Option<f64>, health_fraction: f32, tick: u64) -> MobObservation {
        MobObservation::new(
            (0, 64, 0),
            distance.map(|distance| MobSighting::new(player(), distance)),
            health_fraction,
            tick,
        )
    }

    /// Every walk destination a fresh mob picks over `boundaries` decision
    /// boundaries, with no player in sight.
    fn strolls(seed: i64, boundaries: u64) -> Vec<(i32, i32, i32)> {
        let mut rng = SeededRandom::new(seed);
        let mut destinations = Vec::new();
        for boundary in 0..boundaries {
            let mut ai = MobAi::new();
            let tick = boundary * DECISION_INTERVAL_TICKS;
            if let MobGoal::Wander { target } =
                ai.decide(MobKind::Cow, observation(None, 1.0, tick), &mut rng)
            {
                destinations.push(target);
            }
        }
        destinations
    }

    /// Index of a kind within [`MobKind::ALL`].
    ///
    /// Deliberately has no wildcard arm: adding a variant to [`MobKind`] stops
    /// compiling here until it is listed, which is how "every kind is covered"
    /// (rather than "every kind we remembered") is enforced by the compiler.
    const fn index_of(kind: MobKind) -> usize {
        match kind {
            MobKind::Zombie => 0,
            MobKind::Skeleton => 1,
            MobKind::Cow => 2,
            MobKind::Pig => 3,
            MobKind::Sheep => 4,
            MobKind::Chicken => 5,
            MobKind::Spider => 6,
            MobKind::Creeper => 7,
        }
    }

    #[test]
    fn the_kind_table_is_complete_and_sane() {
        assert_eq!(MobKind::ALL.len(), 8);
        // Every variant appears in `ALL` exactly once, and no index is left over.
        let mut seen = [false; 8];
        for kind in MobKind::ALL {
            let index = index_of(kind);
            assert!(!seen[index], "{kind} is listed twice in ALL");
            seen[index] = true;
        }
        assert!(seen.iter().all(|seen| *seen), "ALL must list every kind");
        for kind in MobKind::ALL {
            let health = kind.max_health();
            assert!(health.is_finite() && health > 0.0, "{kind} health {health}");
            let (width, height) = kind.dimensions();
            assert!(width.is_finite() && width > 0.0, "{kind} width {width}");
            assert!(height.is_finite() && height > 0.0, "{kind} height {height}");
            let attribute = kind.movement_speed_attribute();
            assert!(
                attribute.is_finite() && attribute > 0.0,
                "{kind} speed attribute {attribute}"
            );
            let per_second = kind.movement_speed_blocks_per_second();
            let speed = kind.movement_speed();
            assert!(speed.is_finite() && speed > 0.0, "{kind} speed {speed}");
            assert_eq!(speed, per_second / super::TICKS_PER_SECOND);
            assert!(
                (1..=2).contains(&kind.clearance()),
                "{kind} clearance {}",
                kind.clearance()
            );
            assert!(!kind.name().is_empty());
        }
        assert_eq!(MobKind::Chicken.clearance(), 1);
        assert_eq!(MobKind::Cow.clearance(), 2);
        assert_eq!(MobKind::Zombie.clearance(), 2);
    }

    #[test]
    fn hostile_kinds_attack_and_passive_kinds_do_not() {
        for kind in MobKind::ALL {
            let style = kind.attack_style();
            let damage = kind.attack_damage();
            assert_eq!(
                style.is_some(),
                kind.is_hostile(),
                "{kind}: hostility and attack style must agree"
            );
            if kind.is_hostile() {
                assert!(damage > 0.0, "{kind} is hostile but deals {damage}");
            } else {
                assert_eq!(damage, 0.0, "{kind} is passive but deals {damage}");
            }
            assert_eq!(kind.behaviour().is_hostile(), kind.is_hostile());
        }
        assert_eq!(MobKind::Zombie.attack_style(), Some(MobAttackStyle::Melee));
        assert_eq!(MobKind::Spider.attack_style(), Some(MobAttackStyle::Melee));
        assert_eq!(
            MobKind::Skeleton.attack_style(),
            Some(MobAttackStyle::Ranged)
        );
        assert_eq!(
            MobKind::Creeper.attack_style(),
            Some(MobAttackStyle::Explosive)
        );
        assert_eq!(MobKind::Cow.attack_style(), None);
        // Only melee may be applied as a direct hit: the creeper's number is
        // explosion damage and the skeleton's is ranged.
        assert!(MobAttackStyle::Melee.is_implemented());
        assert!(!MobAttackStyle::Ranged.is_implemented());
        assert!(!MobAttackStyle::Explosive.is_implemented());
        // The verified rows, pinned exactly.
        assert_eq!(MobKind::Zombie.max_health(), 20.0);
        assert_eq!(MobKind::Zombie.dimensions(), (0.6, 1.95));
        assert_eq!(MobKind::Zombie.attack_damage(), 3.0);
        assert_eq!(MobKind::Zombie.movement_speed_attribute(), 0.23);
        assert_eq!(MobKind::Spider.max_health(), 16.0);
        assert_eq!(MobKind::Spider.attack_damage(), 2.0);
        assert_eq!(MobKind::Spider.dimensions(), (1.4, 0.9));
    }

    #[test]
    fn behaviours_expose_exactly_their_goal_sets() {
        assert_eq!(
            MobBehaviour::Hostile.goals(),
            &[
                MobGoalKind::Idle,
                MobGoalKind::Wander,
                MobGoalKind::Chase,
                MobGoalKind::Attack
            ]
        );
        assert_eq!(
            MobBehaviour::Passive.goals(),
            &[MobGoalKind::Idle, MobGoalKind::Wander, MobGoalKind::Flee]
        );
        assert!(MobBehaviour::Hostile.allows(MobGoalKind::Chase));
        assert!(!MobBehaviour::Passive.allows(MobGoalKind::Chase));
        assert!(!MobBehaviour::Passive.allows(MobGoalKind::Attack));
        assert!(MobBehaviour::Passive.allows(MobGoalKind::Flee));
        assert!(!MobBehaviour::Hostile.allows(MobGoalKind::Flee));
    }

    #[test]
    fn a_hostile_mob_chases_inside_its_aggro_radius_and_walks_outside_it() {
        let mut rng = SeededRandom::new(7);
        // Inside the radius: a chase, on the same tick the player is seen.
        let mut ai = MobAi::new();
        assert_eq!(
            ai.decide(MobKind::Zombie, observation(Some(8.0), 1.0, 1), &mut rng),
            MobGoal::Chase { target: player() }
        );
        // Exactly on the radius: still a chase (the test is inclusive).
        let mut ai = MobAi::new();
        assert_eq!(
            ai.decide(
                MobKind::Zombie,
                observation(Some(AGGRO_RADIUS), 1.0, 1),
                &mut rng
            ),
            MobGoal::Chase { target: player() }
        );
        // Beyond it, with no prior target: back to a walk (or idling on the way
        // to one). Never a chase.
        for tick in 0..DECISION_INTERVAL_TICKS * 4 {
            let mut ai = MobAi::new();
            let goal = ai.decide(
                MobKind::Zombie,
                observation(Some(AGGRO_RADIUS + 0.1), 1.0, tick),
                &mut rng,
            );
            assert!(
                matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }),
                "tick {tick}: {goal:?}"
            );
        }
        // No player at all: also a walk, never a chase.
        let mut ai = MobAi::new();
        let goal = ai.decide(MobKind::Cow, observation(None, 1.0, 0), &mut rng);
        assert!(matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }));
    }

    #[test]
    fn a_hostile_mob_attacks_what_is_in_reach() {
        let mut rng = SeededRandom::new(11);
        let mut ai = MobAi::new();
        assert_eq!(
            ai.decide(
                MobKind::Zombie,
                observation(Some(ATTACK_RANGE), 1.0, 1),
                &mut rng
            ),
            MobGoal::Attack {
                target: player(),
                cooldown: 0
            },
            "a fresh target swings immediately"
        );
        // A landed hit restarts the swing timer, which then ticks down.
        ai.note_attack_landed();
        assert_eq!(
            ai.goal,
            MobGoal::Attack {
                target: player(),
                cooldown: ATTACK_COOLDOWN_TICKS
            }
        );
        // One tick later the cooldown has dropped by exactly one, and an
        // unrelated goal cannot reset it.
        ai.decide(MobKind::Zombie, observation(Some(1.0), 1.0, 2), &mut rng);
        assert_eq!(
            ai.goal,
            MobGoal::Attack {
                target: player(),
                cooldown: ATTACK_COOLDOWN_TICKS - 1
            }
        );
        ai.note_attack_landed(); // still an attack: resets again
        assert_eq!(
            ai.goal,
            MobGoal::Attack {
                target: player(),
                cooldown: ATTACK_COOLDOWN_TICKS
            }
        );
        for _ in 0..=ATTACK_COOLDOWN_TICKS {
            ai.decide(MobKind::Zombie, observation(Some(1.0), 1.0, 2), &mut rng);
        }
        assert_eq!(
            ai.goal,
            MobGoal::Attack {
                target: player(),
                cooldown: 0
            },
            "the countdown saturates at zero"
        );
        // `note_attack_landed` is a no-op for a non-attack goal.
        let mut chasing = MobAi::new();
        chasing.decide(MobKind::Zombie, observation(Some(6.0), 1.0, 1), &mut rng);
        let before = chasing.clone();
        chasing.note_attack_landed();
        assert_eq!(chasing, before);
    }

    #[test]
    fn a_hostile_mob_keeps_a_target_inside_the_retention_radius_only() {
        let mut rng = SeededRandom::new(3);
        let mut ai = MobAi::new();
        ai.decide(MobKind::Zombie, observation(Some(10.0), 1.0, 1), &mut rng);
        assert_eq!(ai.last_target, Some(player()));
        // 20 blocks is outside the aggro radius but inside retention.
        assert_eq!(
            ai.decide(MobKind::Zombie, observation(Some(20.0), 1.0, 2), &mut rng),
            MobGoal::Chase { target: player() }
        );
        // Past retention the chase ends. (`TARGET_LOSE_RADIUS > AGGRO_RADIUS` is
        // asserted at compile time next to the two constants.)
        let goal = ai.decide(
            MobKind::Zombie,
            observation(Some(TARGET_LOSE_RADIUS + 0.1), 1.0, 3),
            &mut rng,
        );
        assert!(matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }));
        // A *different* player gets no hysteresis: the aggro radius applies.
        let mut fresh = MobAi::new();
        let sighting = MobSighting::new(other_player(), 20.0);
        let far = MobObservation::new((0, 64, 0), Some(sighting), 1.0, 1);
        let goal = fresh.decide(MobKind::Zombie, far, &mut rng);
        assert!(matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }));
    }

    #[test]
    fn a_hurt_passive_mob_flees_a_nearby_player() {
        let mut rng = SeededRandom::new(5);
        let mut ai = MobAi::new();
        assert_eq!(
            ai.decide(MobKind::Cow, observation(Some(3.0), 0.25, 1), &mut rng),
            MobGoal::Flee { from: player() }
        );
        // Healthy: no flee, even at zero distance.
        let mut healthy = MobAi::new();
        let goal = healthy.decide(MobKind::Cow, observation(Some(0.5), 1.0, 1), &mut rng);
        assert!(matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }));
        // Hurt but far away: no flee.
        let mut far = MobAi::new();
        let goal = far.decide(
            MobKind::Cow,
            observation(Some(FLEE_RADIUS + 1.0), 0.1, 1),
            &mut rng,
        );
        assert!(matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }));
        // Exactly at the threshold health and radius: flees.
        let mut edge = MobAi::new();
        assert_eq!(
            edge.decide(
                MobKind::Sheep,
                observation(Some(FLEE_RADIUS), FLEE_HEALTH_FRACTION, 1),
                &mut rng
            ),
            MobGoal::Flee { from: player() }
        );
        // A passive mob never attacks or chases, whatever it sees.
        for tick in 0..200 {
            let mut ai = MobAi::new();
            let goal = ai.decide(
                MobKind::Chicken,
                observation(Some(0.1), 0.0, tick),
                &mut rng,
            );
            assert_eq!(goal.kind(), MobGoalKind::Flee);
        }
    }

    #[test]
    fn a_walk_starts_on_a_decision_boundary_and_keeps_its_destination() {
        let mut rng = SeededRandom::new(1234);
        let mut ai = MobAi::new();
        // Find a boundary on which the roll succeeds…
        let mut started = None;
        for boundary in 0..64 {
            let tick = boundary * DECISION_INTERVAL_TICKS;
            if let MobGoal::Wander { target } =
                ai.decide(MobKind::Cow, observation(None, 1.0, tick), &mut rng)
            {
                started = Some(target);
                break;
            }
        }
        let target = started.expect("a walk starts within 64 decision boundaries");
        let (x, y, z) = target;
        assert_eq!(ai.wander_target, Some(target));
        assert_eq!(ai.cooldown, WALK_DURATION_TICKS);
        assert_eq!(y, 64, "a walk never changes the mob's own y");
        assert!(x.abs() <= WANDER_RADIUS, "x offset {x}");
        assert!(z.abs() <= WANDER_RADIUS, "z offset {z}");
        // The same destination is kept until the walk timer expires, including on
        // ticks that are not decision boundaries. The tick a walk starts on counts
        // towards `WALK_DURATION_TICKS`: the timer is decremented at the start of
        // every tick, so the walk survives while the timer is above zero and ends
        // on the tick it reaches zero.
        let mut wander_ticks = 1;
        for step in 1..WALK_DURATION_TICKS {
            let goal = ai.decide(
                MobKind::Cow,
                observation(None, 1.0, 1 + u64::from(step)),
                &mut rng,
            );
            assert_eq!(goal, MobGoal::Wander { target }, "step {step}");
            wander_ticks += 1;
        }
        assert_eq!(
            wander_ticks, WALK_DURATION_TICKS,
            "the walk runs its course"
        );
        assert_eq!(ai.cooldown, 1, "one tick of the walk is left");
        // The next tick is past the end of the walk, and its tick number is not a
        // decision boundary (121 is not a multiple of 10), so the mob idles rather
        // than rolling a new destination.
        assert_eq!(
            ai.decide(
                MobKind::Cow,
                observation(None, 1.0, WALK_DURATION_TICKS as u64 + 1),
                &mut rng
            ),
            MobGoal::Idle
        );
        assert_eq!(ai.cooldown, 0);
        assert_eq!(
            ai.wander_target,
            Some(target),
            "the destination outlives the walk"
        );
    }

    #[test]
    fn a_walk_resumes_after_a_chase() {
        let mut rng = SeededRandom::new(21);
        let mut ai = MobAi::new();
        let mut target = None;
        for boundary in 0..64 {
            let tick = boundary * DECISION_INTERVAL_TICKS;
            if let MobGoal::Wander { target: picked } =
                ai.decide(MobKind::Zombie, observation(None, 1.0, tick), &mut rng)
            {
                target = Some(picked);
                break;
            }
        }
        let target = target.expect("a walk starts within 64 decision boundaries");
        let before = ai.cooldown;
        // A player appears: the chase takes over.
        assert_eq!(
            ai.decide(MobKind::Zombie, observation(Some(6.0), 1.0, 40), &mut rng),
            MobGoal::Chase { target: player() }
        );
        // …and leaves again: the interrupted walk resumes at the same place.
        assert_eq!(
            ai.decide(MobKind::Zombie, observation(None, 1.0, 41), &mut rng),
            MobGoal::Wander { target }
        );
        assert_eq!(ai.cooldown, before - 2, "the walk timer kept ticking");
    }

    #[test]
    fn an_idle_mob_only_rolls_on_a_decision_boundary() {
        let mut rng = SeededRandom::new(99);
        // Off-boundary ticks never draw, so they never start a walk.
        for tick in 1..DECISION_INTERVAL_TICKS {
            let mut ai = MobAi::new();
            let goal = ai.decide(MobKind::Cow, observation(None, 1.0, tick), &mut rng);
            assert_eq!(goal, MobGoal::Idle, "tick {tick}");
        }
        assert!(MobAi::new().can_start_walk(0));
        assert!(MobAi::new().can_start_walk(DECISION_INTERVAL_TICKS));
        assert!(!MobAi::new().can_start_walk(1));
    }

    #[test]
    fn the_same_seed_replays_the_same_goals() {
        let script: Vec<MobObservation> = (0..600)
            .map(|tick| {
                let distance = match tick % 4 {
                    0 => None,
                    1 => Some(1.5),
                    2 => Some(12.0),
                    _ => Some(40.0),
                };
                let health = match tick % 3 {
                    0 => 1.0,
                    1 => 0.4,
                    _ => 0.2,
                };
                observation(distance, health, tick)
            })
            .collect();
        let run = |seed: i64, kind: MobKind| {
            let mut rng = SeededRandom::new(seed);
            let mut ai = MobAi::new();
            script
                .iter()
                .map(|observation| {
                    let goal = ai.decide(kind, *observation, &mut rng);
                    assert!(kind.behaviour().allows(goal.kind()), "{goal:?}");
                    goal
                })
                .collect::<Vec<_>>()
        };
        for kind in MobKind::ALL {
            assert_eq!(run(7, kind), run(7, kind), "{kind}: same seed, same goals");
        }
        // A different seed really does stroll differently, so the determinism
        // claim above is not vacuous.
        let seven = strolls(7, 200);
        assert!(!seven.is_empty(), "seed 7 must pick at least one walk");
        assert_ne!(seven, strolls(8, 200), "different seed, different stroll");
        assert_eq!(seven, strolls(7, 200), "and the same seed repeats it");
    }

    #[test]
    fn hostile_input_cannot_make_the_ai_panic_or_act_absurdly() {
        let mut rng = SeededRandom::new(42);
        // Distances that cannot describe a real sighting: refused outright, so
        // they never produce an entity goal of any kind.
        let unusable = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, -1.0e300];
        let healths = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 2.0, 0.0];
        for distance in unusable {
            for health in healths {
                for kind in MobKind::ALL {
                    let mut ai = MobAi::new();
                    let far = observation(Some(distance), health, 0);
                    let goal = ai.decide(kind, far, &mut rng);
                    assert!(
                        kind.behaviour().allows(goal.kind()),
                        "{kind} {distance} {health}: {goal:?}"
                    );
                    assert!(
                        matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }),
                        "{kind} {distance} {health}: {goal:?}"
                    );
                }
            }
        }
        // A finite but absurd distance is valid and must still not reach the mob,
        // even at zero health.
        for kind in MobKind::ALL {
            let mut ai = MobAi::new();
            let goal = ai.decide(kind, observation(Some(f64::MAX), 0.0, 0), &mut rng);
            assert!(
                matches!(goal, MobGoal::Idle | MobGoal::Wander { .. }),
                "{kind}: {goal:?}"
            );
        }
        // Health clamps into 0..=1: a corrupted fraction cannot make a passive
        // mob flee, and a huge fraction cannot stop a hurt one from fleeing.
        let mut unwounded = MobAi::new();
        let goal = unwounded.decide(MobKind::Pig, observation(Some(1.0), 5.0, 0), &mut rng);
        assert_eq!(goal.kind(), MobGoalKind::Idle, "health 5.0 clamps to 1.0");
        let mut wounded = MobAi::new();
        let goal = wounded.decide(MobKind::Pig, observation(Some(1.0), -5.0, 0), &mut rng);
        assert_eq!(goal.kind(), MobGoalKind::Flee, "health -5.0 clamps to 0.0");
    }

    #[test]
    fn mobs_round_trip_through_their_derives() {
        for kind in MobKind::ALL {
            let mob = Mob::new(kind);
            assert_eq!(mob.kind(), kind);
            assert_eq!(mob.behaviour(), kind.behaviour());
            assert_eq!(mob.ai, MobAi::new());
            assert_eq!(mob.ai.goal, MobGoal::Idle);
            assert_eq!(mob.ai.cooldown, 0);
            assert_eq!(mob.ai.wander_target, None);
            assert_eq!(mob.ai.last_target, None);
            // Clone is independent, Eq is reflexive, Debug names the kind.
            let mut clone = mob.clone();
            clone.ai.last_target = Some(player());
            assert_ne!(clone, mob);
            assert_eq!(mob, Mob::new(kind));
            let debug = format!("{mob:?}");
            // `Debug` prints the variant name (`Zombie`); `name()` is the
            // lowercase log name (`zombie`), so compare case-insensitively.
            assert!(debug.to_lowercase().contains(kind.name()), "{debug}");
        }
        assert_eq!(MobAi::default(), MobAi::new());
    }

    #[test]
    fn goal_accessors_report_their_payloads() {
        assert_eq!(MobGoal::Idle.kind(), MobGoalKind::Idle);
        assert_eq!(MobGoal::Idle.target(), None);
        assert_eq!(MobGoal::Idle.wander_target(), None);
        let wander = MobGoal::Wander { target: (1, 2, 3) };
        assert_eq!(wander.kind(), MobGoalKind::Wander);
        assert_eq!(wander.wander_target(), Some((1, 2, 3)));
        assert_eq!(wander.target(), None);
        assert_eq!(MobGoal::Chase { target: player() }.target(), Some(player()));
        assert_eq!(
            MobGoal::Attack {
                target: player(),
                cooldown: 3
            }
            .target(),
            Some(player())
        );
        assert_eq!(
            MobGoal::Flee {
                from: other_player()
            }
            .target(),
            Some(other_player())
        );
        for kind in [
            MobGoalKind::Idle,
            MobGoalKind::Wander,
            MobGoalKind::Chase,
            MobGoalKind::Attack,
            MobGoalKind::Flee,
        ] {
            assert!(!kind.name().is_empty());
        }
        assert_eq!(MobBehaviour::Hostile.goals().len(), 4);
        assert_eq!(MobBehaviour::Passive.goals().len(), 3);
        assert_eq!(MobAttackStyle::Melee.name(), "melee");
        assert_eq!(MobKind::Zombie.to_string(), "zombie");
        assert_eq!(
            SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE * 0.1,
            MobKind::Zombie.movement_speed_blocks_per_second() / 0.23 * 0.1
        );
    }
}
