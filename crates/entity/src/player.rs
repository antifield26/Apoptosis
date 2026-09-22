//! Player state: identity, position, game mode, health, food, experience and
//! inventory, plus the `playerdata` persistence boundary.
//!
//! ## Vanilla rules implemented here, and where each number comes from
//!
//! Every rule below is stated with a citation-by-description of the vanilla
//! mechanism it encodes. Those descriptions are *behavioural* facts read off the
//! public rules of the game; the implementation is this crate's own (AGENTS.md
//! section 6: no GPL source is copied, and Paper/Valiant/Pumpkin are reference
//! implementations, not specifications).
//!
//! | Rule | Value | Vanilla mechanism |
//! |---|---|---|
//! | max health | 20.0 (10 hearts) | `Player.MAX_HEALTH` / `Attributes.MAX_HEALTH` base value |
//! | max food | 20 | `FoodData.MAX_FOOD_LEVEL`, `foodLevel` field |
//! | max saturation | 5.0 | vanilla caps `foodSaturationLevel` at the food level, and 20 food cannot hold more than 5 saturation in practice |
//! | exhaustion | 4.0 per point | `FoodData.addExhaustion` spends one saturation/food point per 4.0 exhaustion (`FoodConstants.EXHAUSTION_DROP`) |
//! | natural regen | food ≥ 18 and health < max | `FoodData.tick`: "the player regenerates 1 health per 4 seconds while food ≥ 18" |
//! | saturation regen | food ≥ 20 and saturation > 0 | `FoodData.tick`: the faster regeneration tier, which spends saturation first |
//! | starvation | food == 0 | `FoodData.tick`: once food is exhausted the player takes 1 damage per 4 seconds (`STARVE`) |
//! | levels | `2L+7` / `5L−38` / `9L−158` | `Player.getXpNeededForNextLevel`, the three-branch formula |
//! | death | health == 0 | `Player.hurt` returns dead at 0; health is never negative |
//!
//! ## Known gaps (AGENTS.md section 3.3 — exhaustive, not implied away)
//!
//! - **No difficulty scaling.** Starvation damage is 1.0, the normal/hard value;
//!   peaceful, easy scaling and mob damage scaling are not modelled.
//! - **No enchantments, absorption or fire.** [`Player::apply_damage`] takes a
//!   [`DamageSource`](crate::combat::DamageSource) plus armour stats: melee,
//!   fall and starvation typing and armour/toughness absorb are modelled;
//!   knockback lands on entity-store victims (mob bodies), not on player
//!   projections, which carry no velocity under client-driven motion;
//!   enchantment math, absorption hearts and fire/drowning/void sources are
//!   not.
//! - **No natural health regeneration.** [`Player::tick_food`] models the food
//!   *budget* (exhaustion/saturation/regen/starve) but there is no per-tick
//!   simulation driver, no `foodTickTimer` phase, and no peaceful-mode instant
//!   heal.
//! - **No movement, physics, collision, fall damage or respawn anchoring.**
//!   Position is stored, never simulated; `respawn` does not search for a safe
//!   spawn or move the player.
//! - **No `keep_inventory` game rule.** Death drops are drained by
//!   `Game::after_damage` (P11-07); [`Player::respawn`] does not drop again.
//!   restores health/food only; the caller decides what happens to the inventory
//!   and to experience (see [`Player::respawn`]).
//! - **No effects, attributes, statistics, advancements, ender chest,
//!   hunger-based difficulty lock or hardcore flag.**
//! - `air`/`Fire`/`XpSeed`/`abilities` from a real `playerdata` file are
//!   preserved verbatim in [`Player::extra`] and not interpreted.
//! - The 41..=45 crafting slots and all container interaction are out of scope
//!   (see [`crate::inventory`]).

use crate::combat::{CombatStats, DamageSource};
use crate::effect::ActiveEffect;
use crate::inventory::PlayerInventory;
use crate::profile::GameProfile;
use crate::stack::ItemStack;
use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use mc_registry::ItemRegistry;
use mc_world::Vec3;
use std::collections::BTreeMap;

/// Maximum player health (10 hearts).
pub const MAX_HEALTH: f32 = 20.0;

/// Maximum food level (10 haunches).
pub const MAX_FOOD: i32 = 20;

/// Maximum saturation.
/// Highest saturation the model tracks.
///
/// **This is not Vanilla's rule.** Vanilla caps saturation at the food level (so up
/// to 20) and each food item has its own saturation modifier; 5.0 is this project's
/// own simplification and is presented as such rather than as a game fact (Audit 03
/// found this constant was the one value in the crate carrying no label). The real
/// cap and the per-food restoration table are P07-03 data.
pub const MAX_SATURATION: f32 = 5.0;

/// Exhaustion that must accrue before one saturation/food point is spent.
pub const EXHAUSTION_PER_POINT: f32 = 4.0;

/// Damage dealt per starvation tick at food level 0 (normal/hard difficulty).
pub const STARVATION_DAMAGE: f32 = 1.0;

/// Health restored per saturation-regeneration tick.
pub const REGEN_HEALTH_PER_TICK: f32 = 1.0;

/// Food level at or above which natural regeneration runs.
pub const REGEN_FOOD_THRESHOLD: i32 = 18;

/// `DataVersion` written by 26.1.2 (measured on a real world; see
/// `mc_persistence::level::DATA_VERSION_26_1_2`). Repeated here as a literal
/// because `mc-entity` must not depend on `mc-persistence`.
pub const DATA_VERSION_26_1_2: i32 = 4790;

/// Vanilla game mode, as stored in `playerGameType` (0..=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GameMode {
    /// Vanilla Survival: health, hunger, damage, drops.
    Survival,
    /// Creative: flight, instant break, invulnerable, unlimited blocks.
    Creative,
    /// Adventure: survival rules, but blocks cannot be broken or placed.
    Adventure,
    /// Spectator: no interaction, no collision, invisible to survival players.
    Spectator,
}

impl GameMode {
    /// All game modes in vanilla id order, for deterministic iteration.
    pub const ALL: [Self; 4] = [
        Self::Survival,
        Self::Creative,
        Self::Adventure,
        Self::Spectator,
    ];

    /// Vanilla `GameType.getId()` (0 survival, 1 creative, 2 adventure,
    /// 3 spectator).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }

    /// Parse a vanilla `GameType` id. `None` for anything outside 0..=3, so a
    /// hostile `playerGameType` cannot become a game mode.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Survival),
            1 => Some(Self::Creative),
            2 => Some(Self::Adventure),
            3 => Some(Self::Spectator),
            _ => None,
        }
    }

    /// Whether the mode ignores damage, hunger and block breaking rules
    /// (creative and spectator).
    #[must_use]
    pub const fn is_creative(self) -> bool {
        matches!(self, Self::Creative | Self::Spectator)
    }

    /// Whether the mode is invulnerable to ordinary damage. Vanilla:
    /// `Player.hurt` returns false for a spectator, and
    /// `Player.isInvulnerableTo` returns true for a creative player against
    /// non-`outOfWorld`/`genericKill` sources.
    #[must_use]
    pub const fn is_invulnerable(self) -> bool {
        matches!(self, Self::Creative | Self::Spectator)
    }

    /// Whether the mode can interact with blocks and entities (not spectator).
    #[must_use]
    pub const fn can_interact(self) -> bool {
        !matches!(self, Self::Spectator)
    }

    /// Whether the mode can break and place blocks (not adventure or spectator).
    #[must_use]
    pub const fn can_build(self) -> bool {
        matches!(self, Self::Survival | Self::Creative)
    }

    /// The NBT `playerGameType` value: `GameType.getId()`.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self.id() as i32
    }
}

impl Default for GameMode {
    /// Survival is the vanilla default for a new player.
    fn default() -> Self {
        Self::Survival
    }
}

/// Outcome of one [`Player::apply_damage`] call.
///
/// `died` is true on exactly the call that takes health to 0, so "death happens
/// once" is observable rather than inferred.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageOutcome {
    /// Whether health actually changed.
    pub applied: bool,
    /// Whether this call is the one that killed the player.
    pub died: bool,
    /// Health removed (`0.0` when nothing applied).
    pub dealt: f32,
    /// Health after the call.
    pub health: f32,
}

/// A player's persistent state.
///
/// ## Ownership and threading (AGENTS.md section 8)
///
/// A `Player` is a plain value with no interior mutability and no handles, so
/// the tick thread can own it exclusively; the cross-thread boundary is the
/// *value*, not this type. Today nothing in the crate is `Sync`-shared, no
/// synchronisation primitive is used, and there is no queue — a future player
/// map owns one `Player` per connection and must document its own ordering and
/// shutdown rules.
///
/// ## Field set
///
/// Mirrors the parts of vanilla's `ServerPlayer`/`Player` that this phase owns,
/// named after the `playerdata` keys they map to. The boolean field is kept as a
/// bool because it mirrors the file's own `OnGround` flag; grouping it into a
/// bitfield would obscure that mapping (same precedent as `LevelDat`).
///
/// ## `extra`
///
/// Any `playerdata` entry this model does not interpret is preserved verbatim,
/// so a load/save cycle never drops operator, mod or future-vanilla data. Keys
/// this type *does* write (`Pos`, `Health`, …) win over a stale `extra` entry,
/// so a re-save cannot produce a document with two conflicting values. `id`
/// (the profile UUID) and `entity_id` deliberately live in `extra`/the entity
/// system: neither is part of the disk schema, and the authenticated profile is
/// the authority for the UUID.
///
/// ## Equality
///
/// [`PartialEq`] is implemented by hand and compares every persisted field
/// including [`Player::extra`]. `entity_id` deliberately does **not**
/// participate: it is assigned by the entity system, is never persisted, and is
/// not part of what a `playerdata` load reconstructs, so comparing two players
/// across a load/save cycle must not fail just because the runtime id was
/// reallocated.
#[derive(Debug, Clone)]
pub struct Player {
    /// Authenticated identity; the key `playerdata/<uuid>.dat` is named after.
    pub profile: GameProfile,
    /// Runtime entity id, **not** persisted: the entity system assigns it.
    pub entity_id: i32,
    /// Dimension key, e.g. `minecraft:overworld`.
    pub dimension: String,
    /// Feet position.
    pub position: Vec3,
    /// Horizontal rotation in degrees (`Rotation[0]`).
    pub yaw: f32,
    /// Vertical rotation in degrees (`Rotation[1]`).
    pub pitch: f32,
    /// Whether the player is standing on a block (`OnGround`).
    pub on_ground: bool,
    /// Current game mode (`playerGameType`).
    pub game_mode: GameMode,
    /// Health in `0.0..=20.0`.
    pub health: f32,
    /// Food level in `0..=20` (`foodLevel`).
    pub food: i32,
    /// Saturation in `0.0..=food as f32` (`foodSaturationLevel`).
    pub saturation: f32,
    /// Exhaustion in `0.0..4.0` (`foodExhaustionLevel`).
    pub exhaustion: f32,
    /// Progress towards the next level in `0.0..1.0` (`XpP`).
    pub experience: f32,
    /// Experience level (`XpLevel`).
    pub level: i32,
    /// Lifetime experience points (`XpTotal`).
    pub total_experience: i32,
    /// Inventory (slots 0..=40).
    pub inventory: PlayerInventory,
    /// Active status effects, keyed by effect id (ascending, deterministic),
    /// mirroring [`crate::entity::Entity::effects`] for mobs.
    pub effects: BTreeMap<i32, ActiveEffect>,
    /// Uninterpreted `playerdata` entries, preserved verbatim.
    pub extra: Vec<(String, NbtTag)>,
}

/// Compares every persisted field, including [`Player::extra`]; see the note on
/// [`Player`] for why `entity_id` is excluded.
impl PartialEq for Player {
    fn eq(&self, other: &Self) -> bool {
        self.profile == other.profile
            && self.dimension == other.dimension
            && self.position == other.position
            && self.yaw == other.yaw
            && self.pitch == other.pitch
            && self.on_ground == other.on_ground
            && self.game_mode == other.game_mode
            && self.health == other.health
            && self.food == other.food
            && self.saturation == other.saturation
            && self.exhaustion == other.exhaustion
            && self.experience == other.experience
            && self.level == other.level
            && self.total_experience == other.total_experience
            && self.effects == other.effects
            && self.inventory == other.inventory
            && self.extra == other.extra
    }
}

impl Player {
    /// A fresh player at the origin of `dimension` with survival defaults.
    ///
    /// Defaults, stated rather than implied: full health (20.0), full food (20),
    /// saturation 5.0, no exhaustion, level 0, position `(0.0, 0.0, 0.0)`,
    /// rotation `(0, 0)`, `on_ground == false`, an empty inventory with hotbar
    /// slot 0 selected. Vanilla gives a new player the world spawn point
    /// instead; the caller sets that, because spawn selection is the world's
    /// business, not this type's.
    #[must_use]
    pub fn new(
        profile: GameProfile,
        entity_id: i32,
        dimension: &str,
        inventory: PlayerInventory,
    ) -> Self {
        Self {
            profile,
            entity_id,
            dimension: dimension.to_owned(),
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            game_mode: GameMode::Survival,
            health: MAX_HEALTH,
            food: MAX_FOOD,
            saturation: MAX_SATURATION,
            exhaustion: 0.0,
            experience: 0.0,
            level: 0,
            total_experience: 0,
            inventory,
            effects: BTreeMap::new(),
            extra: Vec::new(),
        }
    }

    /// Set the profile (used after a `playerdata` load, where the
    /// authenticated profile is the authority for the UUID).
    pub fn set_profile(&mut self, profile: GameProfile) {
        self.profile = profile;
    }

    /// Whether the player is alive (health above 0).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }

    /// Whether the game mode makes the player invulnerable to ordinary damage.
    #[must_use]
    pub const fn is_invulnerable(&self) -> bool {
        self.game_mode.is_invulnerable()
    }

    /// Kill outright, bypassing invulnerability, for `/kill` (P14-01).
    ///
    /// Vanilla's kill command works in creative mode: it is not ordinary
    /// damage, so [`GameMode::is_invulnerable`] does not stop it. A dead
    /// player is unaffected (death happens once — see [`Player::apply_damage`]
    /// rule 3), and the returned outcome feeds the same death path
    /// (`drain_all` drops, death message) as any other lethal hit.
    pub fn kill(&mut self) -> DamageOutcome {
        if !self.is_alive() {
            return DamageOutcome {
                applied: false,
                died: false,
                dealt: 0.0,
                health: self.health,
            };
        }
        let dealt = self.health;
        self.health = 0.0;
        DamageOutcome {
            applied: true,
            died: true,
            dealt,
            health: 0.0,
        }
    }

    /// Set health, clamped into `0.0..=20.0` (non-finite input becomes 0.0).
    pub fn set_health(&mut self, health: f32) {
        self.health = if health.is_finite() {
            health.clamp(0.0, MAX_HEALTH)
        } else {
            0.0
        };
    }

    /// Heal `amount`, clamped at [`MAX_HEALTH`].
    ///
    /// An invulnerable (creative/spectator) player is already at full health
    /// only if nothing else moved it, so healing is *not* short-circuited by the
    /// game mode. A non-positive or non-finite `amount` is a no-op; healing never
    /// resurrects a dead player (vanilla's `heal` requires `isAlive`).
    pub fn heal(&mut self, amount: f32) {
        if amount <= 0.0 || !amount.is_finite() || !self.is_alive() {
            return;
        }
        self.health = (self.health + amount).min(MAX_HEALTH);
    }

    /// Apply raw damage, returning what actually happened.
    ///
    /// Rules, in order:
    ///
    /// 1. A non-finite or non-positive `amount` does nothing (hostile input:
    ///    NaN would otherwise poison the health field forever).
    /// 2. A creative or spectator player is invulnerable
    ///    ([`GameMode::is_invulnerable`]).
    /// 3. A dead player takes no further damage, so a second lethal hit reports
    ///    `died == false` — exactly once per life.
    /// 4. Health floors at 0.0 and is *reported* dead on the call that reaches
    ///    it; it never goes negative.
    ///
    /// `amount` is damage after armour and effects, which this crate does not
    /// model (see the module gap list).
    pub fn apply_damage(
        &mut self,
        amount: f32,
        source: DamageSource,
        armor: &CombatStats,
    ) -> DamageOutcome {
        let unchanged = DamageOutcome {
            applied: false,
            died: false,
            dealt: 0.0,
            health: self.health,
        };
        if !amount.is_finite() || amount <= 0.0 || self.is_invulnerable() || !self.is_alive() {
            return unchanged;
        }
        // Armour first (vanilla order: absorb before resistance effects), unless
        // the source bypasses it. I-frame windows are owned by the callers, as
        // before: this function applies an amount, it does not gate one.
        let amount = if source.bypasses_armor() {
            amount
        } else {
            crate::combat::armor_absorb(amount, armor.armor, armor.toughness)
        };
        let dealt = amount.min(self.health);
        self.health -= dealt;
        if self.health <= 0.0 {
            self.health = 0.0;
        }
        DamageOutcome {
            applied: true,
            died: self.health == 0.0,
            dealt,
            health: self.health,
        }
    }

    /// Give (or refresh) a status effect: a higher amplifier wins, and at
    /// equal amplifier the longer duration wins (vanilla's combine rule).
    /// A non-positive duration is a refusal, not a zero-tick effect.
    pub fn give_effect(&mut self, id: i32, amplifier: i32, duration: i32) {
        if duration <= 0 {
            return;
        }
        let incoming = ActiveEffect {
            id,
            amplifier: amplifier.max(0),
            duration,
            ambient: false,
        };
        match self.effects.get(&id) {
            Some(current)
                if current.amplifier > incoming.amplifier
                    || (current.amplifier == incoming.amplifier
                        && current.duration >= incoming.duration) =>
            {
                // The incumbent is strictly better; the refresh fizzles.
            }
            _ => {
                self.effects.insert(id, incoming);
            }
        }
    }

    /// Clear one effect by id, reporting whether one was present.
    pub fn clear_effect(&mut self, id: i32) -> bool {
        self.effects.remove(&id).is_some()
    }

    /// Tick every active effect down by one and apply the over-time ones.
    ///
    /// Poison and wither deal [`damage_over_time`](crate::effect::damage_over_time)
    /// through [`Self::apply_damage`] with their mapped sources (poison floors
    /// at 1.0 health via [`can_kill`](crate::effect::can_kill)); regeneration
    /// heals 1.0 every `max(50 >> amplifier, 1)` ticks, the documented vanilla
    /// cadence. Returns the ids that expired this tick, so the caller can
    /// announce their removal.
    pub fn tick_effects(&mut self, tick: u64, armor: &CombatStats) -> Vec<i32> {
        use crate::effect::{can_kill, damage_over_time};
        let mut expired = Vec::new();
        let mut done: Vec<(i32, ActiveEffect)> = Vec::new();
        for (id, effect) in &mut self.effects {
            if effect.duration <= 1 {
                expired.push(*id);
            } else {
                effect.duration -= 1;
            }
            done.push((*id, *effect));
        }
        for id in &expired {
            self.effects.remove(id);
        }
        for (_, effect) in done {
            if !self.is_alive() {
                break;
            }
            let hit = damage_over_time(&effect, tick);
            if hit > 0.0 {
                let source = match effect.kind() {
                    Some(crate::effect::EffectKind::Wither) => DamageSource::Wither,
                    _ => DamageSource::Poison,
                };
                let mut hit = hit;
                if !can_kill(&effect) {
                    hit = hit.min((self.health - 1.0).max(0.0));
                }
                if hit > 0.0 {
                    self.apply_damage(hit, source, armor);
                }
            }
            if effect.kind() == Some(crate::effect::EffectKind::Regeneration) {
                let shift = u32::try_from(effect.amplifier.max(0)).unwrap_or(0);
                let interval = (50u64 >> shift.min(6)).max(1);
                if tick.is_multiple_of(interval) {
                    self.heal(1.0);
                }
            }
        }
        expired
    }

    /// Set the food level, clamped into `0..=20`, and re-clamp saturation to it.
    pub fn set_food(&mut self, food: i32) {
        self.food = food.clamp(0, MAX_FOOD);
        self.saturation = self.saturation.clamp(0.0, Self::max_saturation(self.food));
    }

    /// Set saturation, clamped into `0.0..=food as f32`, capped at
    /// [`MAX_SATURATION`].
    pub fn set_saturation(&mut self, saturation: f32) {
        self.saturation = if saturation.is_finite() {
            saturation.clamp(0.0, Self::max_saturation(self.food))
        } else {
            0.0
        };
    }

    /// The highest saturation a given food level allows
    /// (`min(5.0, food as f32)`, never negative).
    #[must_use]
    pub fn max_saturation(food: i32) -> f32 {
        let food = food.clamp(0, MAX_FOOD);
        #[allow(
            clippy::cast_precision_loss,
            reason = "food is clamped to 0..=20, exactly representable in f32"
        )]
        let as_float = food as f32;
        as_float.min(MAX_SATURATION)
    }

    /// Whether natural regeneration is running (`food >= 18`).
    #[must_use]
    pub const fn is_regenerating(&self) -> bool {
        self.food >= REGEN_FOOD_THRESHOLD && self.health < MAX_HEALTH
    }

    /// Advance the food/health budget by one exhaustion step.
    ///
    /// `exhaustion_delta` is the exhaustion this tick's actions accrued (sprint,
    /// jump, block break, attack, …). Those individual costs are **not** modelled
    /// here: the caller adds them up, because most of them depend on movement and
    /// block breaking, which this crate does not own. A non-finite or negative
    /// value is ignored.
    ///
    /// Order of operations (vanilla `FoodData.tick` + `addExhaustion`):
    ///
    /// 1. Accrue `exhaustion_delta`; every full [`EXHAUSTION_PER_POINT`] spends
    ///    one point, saturation first and food after it.
    /// 2. Clamp saturation to the food level — vanilla does this because
    ///    saturation may never exceed the food that supports it.
    /// 3. Regenerate when food `>= 18`: at food `>= 20` with saturation left,
    ///    heal one point and spend saturation; otherwise (or when saturation is
    ///    gone) heal one point and spend food. This is the tiered
    ///    saturation/food regeneration.
    /// 4. Starve at food `0`: [`STARVATION_DAMAGE`] health, which *can* kill.
    ///
    /// Returns the [`DamageOutcome`] of the starvation step, so a caller can see
    /// a death caused by hunger. There is no simulation driver calling this yet;
    /// the tick loop owns that.
    pub fn tick_food(&mut self, exhaustion_delta: f32) -> DamageOutcome {
        if exhaustion_delta.is_finite() && exhaustion_delta > 0.0 {
            self.exhaustion += exhaustion_delta;
        }
        // Spend one saturation/food point per full 4.0 exhaustion.
        while self.exhaustion >= EXHAUSTION_PER_POINT {
            self.exhaustion -= EXHAUSTION_PER_POINT;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else if self.food > 0 {
                self.food -= 1;
            } else {
                // Nothing left to spend; drop the exhaustion instead of
                // letting it grow without bound.
                self.exhaustion = 0.0;
                break;
            }
        }
        self.saturation = self.saturation.clamp(0.0, Self::max_saturation(self.food));

        if self.food >= MAX_FOOD && self.saturation > 0.0 && self.health < MAX_HEALTH {
            // Floor at zero. The clamp above is a *ceiling*, so a plain `-= 1.0`
            // here left a fractional saturation negative (Audit 02), which then
            // suppressed further regeneration and misreported `is_regenerating`.
            self.saturation = (self.saturation - 1.0).max(0.0);
            self.heal(REGEN_HEALTH_PER_TICK);
        } else if self.is_regenerating() {
            self.set_food(self.food - 1);
            self.heal(REGEN_HEALTH_PER_TICK);
        }

        if self.food == 0 {
            return self.apply_damage(
                STARVATION_DAMAGE,
                DamageSource::Starvation,
                &CombatStats::ZERO,
            );
        }
        DamageOutcome {
            applied: false,
            died: false,
            dealt: 0.0,
            health: self.health,
        }
    }

    /// Experience points needed to advance from `level` to `level + 1`.
    ///
    /// Vanilla `Player.getXpNeededForNextLevel`, the three-branch formula:
    /// `2L + 7` up to level 15, `5L − 38` for 16..=30, `9L − 158` above 30.
    /// A negative level is treated as 0 (vanilla never produces one).
    #[must_use]
    pub const fn experience_needed_for_level(level: i32) -> i32 {
        let level = if level < 0 { 0 } else { level };
        // Saturating arithmetic: `9 * level` overflows `i32` from about level
        // 2.4e8, and `XpLevel` arrives from persisted NBT, which a hand-edited or
        // corrupt file controls. Overflow would panic in a debug build — i.e. a
        // hostile save file could take the server down (Audit 02). Vanilla caps
        // the level far below this; the exact ceiling is **not** enforced here
        // because its basis could not be verified, so this only removes the
        // panic and documents the gap.
        // Written with explicit comparisons rather than `.max(0)` because
        // `Ord::max` is not callable from a `const fn` on this toolchain. The
        // `level < 0` fold above already makes every branch non-negative, so the
        // saturating subtract is belt-and-braces against a future edit.
        if level <= 15 {
            2 * level + 7
        } else if level <= 30 {
            5 * level - 38
        } else {
            let needed = 9i32.saturating_mul(level).saturating_sub(158);
            if needed < 0 { 0 } else { needed }
        }
    }

    /// Progress bar fraction (`XpP`), i.e. [`Player::experience`] itself.
    ///
    /// Provided so protocol code has a name for the value it sends and cannot
    /// confuse it with the point count stored in `XpTotal`.
    #[must_use]
    pub fn experience_progress(&self) -> f32 {
        self.experience.clamp(0.0, 1.0)
    }

    /// Add experience points, carrying the remainder into the next level.
    ///
    /// Vanilla `Player.giveExperiencePoints`: `XpTotal` accumulates by the raw
    /// amount, the current bar progress is converted **from** the `XpP` fraction
    /// (`ceil(XpP * needed)`, which is how vanilla reconstructs the banked points
    /// from the persisted fraction), the sum is spent against the same
    /// three-branch formula in reverse (`while points >= needed { points -=
    /// needed; level += 1 }`), and whatever is left is stored back as a fraction
    /// of the new level's requirement.
    ///
    /// So [`Player::experience`] stays in `0.0..1.0` — it is `XpP`, a fraction,
    /// not a point count. A non-positive `points` is a no-op; a non-finite or
    /// out-of-range `XpP` (hostile NBT) is treated as 0.
    ///
    /// Returns the number of levels gained.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`needed` is at most 9*level-158 and exact in f32 for every reachable level; the derived point count is a small non-negative integer"
    )]
    pub fn add_experience(&mut self, points: i32) -> i32 {
        if points <= 0 {
            return 0;
        }
        self.total_experience = self.total_experience.saturating_add(points);
        if self.level < 0 {
            self.level = 0;
        }
        if !self.experience.is_finite() {
            self.experience = 0.0;
        }
        // Points banked on the current bar, recovered from the fraction. The
        // first branch's requirement is at least 7, so there is always a
        // positive divisor and `XpP = 0` yields exactly 0 banked points.
        let needed = Self::experience_needed_for_level(self.level).max(1);
        let banked = (self.experience.clamp(0.0, 1.0) * needed as f32).ceil() as i32;
        let mut remaining = points.saturating_add(banked);
        let mut gained = 0;
        loop {
            let needed = Self::experience_needed_for_level(self.level);
            if needed <= 0 || remaining < needed {
                break;
            }
            remaining -= needed;
            self.level += 1;
            gained += 1;
        }
        let needed = Self::experience_needed_for_level(self.level);
        self.experience = if needed > 0 {
            (remaining as f32 / needed as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        gained
    }

    /// Reset the experience bar, level and total to zero.
    ///
    /// Vanilla calls the equivalent (`Player.resetExperience` behaviour) when a
    /// player dies **without** the `keepInventory` game rule.
    pub fn reset_experience(&mut self) {
        self.level = 0;
        self.experience = 0.0;
        self.total_experience = 0;
    }

    /// Restore the player to a living, fed state after death.
    ///
    /// What this **does**: health to [`MAX_HEALTH`], food to [`MAX_FOOD`],
    /// saturation to [`MAX_SATURATION`], exhaustion to 0.
    ///
    /// What this deliberately **does not** do, and why:
    ///
    /// - **Inventory.** Vanilla drops the inventory on death unless the
    ///   `keepInventory` game rule is on. That rule is stored in
    ///   `data/minecraft/game_rules.dat` (26.1 moved game rules out of
    ///   `level.dat`), which this crate does not read, and dropping items needs
    ///   a world to drop them into. So the caller decides: [`Player::respawn`]
    ///   returns the inventory that *would* drop, as a list of stacks, and
    ///   leaves `self.inventory` untouched. Passing `keep_inventory == true`
    ///   returns an empty list.
    /// - **Experience.** Same gate: `keepInventory` also keeps XP in vanilla.
    ///   With `keep_inventory == false` this resets level/bar/total (vanilla
    ///   drops the orbs, which requires a world); with `true` it keeps them.
    /// - **Position, dimension, rotation.** Spawn selection and dimension
    ///   transfer belong to the world/teleport path; the caller sets
    ///   [`Player::position`] and [`Player::dimension`].
    /// - **Game mode.** Untouched: respawning does not change it.
    ///
    /// Returns the stacks that would have been dropped, in slot order
    /// (hotbar, main, armour, offhand) for a deterministic drop sequence
    /// (AGENTS.md section 3.6).
    pub fn respawn(&mut self, keep_inventory: bool) -> Vec<ItemStack> {
        self.health = MAX_HEALTH;
        self.food = MAX_FOOD;
        self.saturation = MAX_SATURATION;
        self.exhaustion = 0.0;
        self.on_ground = false;
        if !keep_inventory {
            self.reset_experience();
            let mut dropped = Vec::new();
            for index in 0..self.inventory.stored_slots() {
                let stack = self.inventory.slot(index);
                if !stack.is_empty() {
                    dropped.push(stack);
                }
            }
            return dropped;
        }
        Vec::new()
    }
}

/// `playerdata` keys this model interprets. Everything else goes to
/// [`Player::extra`].
const KNOWN_PLAYER_KEYS: &[&str] = &[
    "DataVersion",
    "Dimension",
    "Health",
    "Inventory",
    "OnGround",
    "Pos",
    "Rotation",
    "SelectedItemSlot",
    "XpLevel",
    "XpP",
    "XpTotal",
    "active_effects",
    "foodExhaustionLevel",
    "foodLevel",
    "foodSaturationLevel",
    "playerGameType",
];

impl Player {
    /// Decode a `playerdata` compound.
    ///
    /// Tolerant reads, matching `LevelDat`'s policy:
    ///
    /// - a missing/wrong-typed entry takes the documented default rather than
    ///   failing the load (`Pos` → origin, `Health` → 20.0, …);
    /// - `Health` is clamped into `0.0..=20.0`, `XpP` into `0.0..=1.0`,
    ///   `SelectedItemSlot` into `0..=8`, because an out-of-range value there is
    ///   cosmetic corruption, not a reason to lose a player's inventory;
    /// - `playerGameType` **must** be a legal id and an inventory item **must**
    ///   resolve: those change behaviour or identity, so guessing is worse than
    ///   refusing.
    ///
    /// `profile` is the authenticated identity and is authoritative over any
    /// `id`/`UUID` entry in the document; an `id` entry therefore survives in
    /// [`Player::extra`] rather than being interpreted. `entity_id` is assigned
    /// by the entity system and is never persisted.
    ///
    /// The root is the player compound itself: vanilla writes it unnamed at the
    /// file's top level, unlike `level.dat`'s `{Data: {...}}` wrapper. A root
    /// carrying a `Data` compound is therefore *not* this format and fails.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the root is not a compound, when
    /// `playerGameType` is not `0..=3`, or when an `Inventory` entry names an
    /// item the registry does not contain.
    pub fn from_nbt(
        root: &NbtTag,
        profile: GameProfile,
        entity_id: i32,
        items: &ItemRegistry,
    ) -> ServerResult<Self> {
        if root.entries().is_none() {
            return Err(ServerError::CorruptData(format!(
                "player data root is {}, expected TAG_Compound",
                root.type_name()
            )));
        }

        let dimension = root
            .get_str("Dimension")
            .unwrap_or("minecraft:overworld")
            .to_owned();
        let position = match root.get_list("Pos") {
            Some(list) if list.len() == 3 => {
                let mut values = [0.0_f64; 3];
                for (index, tag) in list.iter().enumerate() {
                    values[index] = tag.as_f64().unwrap_or(0.0);
                }
                Vec3::new(values[0], values[1], values[2])
            }
            _ => Vec3::ZERO,
        };
        let (yaw, pitch) = match root.get_list("Rotation") {
            Some(list) if list.len() == 2 => (
                finite_f32(list[0].as_f64(), 0.0),
                finite_f32(list[1].as_f64(), 0.0),
            ),
            _ => (0.0, 0.0),
        };
        let game_mode = root
            .get_i32("playerGameType")
            .and_then(|id| u8::try_from(id).ok())
            .and_then(GameMode::from_id)
            .ok_or_else(|| {
                ServerError::CorruptData(format!(
                    "player data has no legal playerGameType (got {:?})",
                    root.get("playerGameType")
                ))
            })?;

        let inventory = crate::inventory::inventory_for_registry(items)?;
        let mut player = Self {
            profile,
            entity_id,
            dimension,
            position,
            yaw,
            pitch,
            on_ground: root.get_bool("OnGround").unwrap_or(false),
            game_mode,
            health: MAX_HEALTH,
            food: MAX_FOOD,
            saturation: MAX_SATURATION,
            exhaustion: 0.0,
            experience: 0.0,
            level: 0,
            total_experience: 0,
            inventory,
            effects: BTreeMap::new(),
            extra: Vec::new(),
        };

        player.set_health(finite_f32(root.get_f64("Health"), MAX_HEALTH));
        player.set_food(root.get_i32("foodLevel").unwrap_or(MAX_FOOD));
        player.set_saturation(in_range_f32(
            root.get_f64("foodSaturationLevel"),
            0.0,
            Player::max_saturation(player.food),
            // Absent means "a fresh player", whose saturation is full.
            Player::max_saturation(player.food),
            // Present but out of range means corruption: the safe reading is 0.
            0.0,
        ));
        player.exhaustion = in_range_f32(
            root.get_f64("foodExhaustionLevel"),
            0.0,
            EXHAUSTION_PER_POINT,
            0.0,
            0.0,
        );
        player.level = root.get_i32("XpLevel").unwrap_or(0).max(0);
        player.experience = in_range_f32(root.get_f64("XpP"), 0.0, 1.0, 0.0, 0.0);
        player.total_experience = root.get_i32("XpTotal").unwrap_or(0).max(0);
        player.effects = read_effects(root);
        player.inventory = read_inventory(root, items)?;
        if let Some(slot) = root.get_i32("SelectedItemSlot") {
            player.inventory.set_selected_hotbar_tolerant(slot);
        }

        player.extra = root
            .entries()
            .unwrap_or(&[])
            .iter()
            .filter(|(key, _)| !KNOWN_PLAYER_KEYS.contains(&key.as_str()))
            .cloned()
            .collect();
        Ok(player)
    }

    /// Encode as the `playerdata` compound (uncompressed NBT value).
    ///
    /// Field order follows the measured vanilla layout so a round trip is
    /// diff-friendly. `DataVersion` is written as the 26.1.2 value; the caller
    /// (persistence) owns compression, atomic rename and *when* to write.
    ///
    /// An empty stack is never written into `Inventory`, matching vanilla, which
    /// stores only occupied slots — so a load/save cycle of a partially used
    /// inventory is byte-stable.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when a stored stack holds an item id `items`
    /// does not know. That cannot happen for state built through this crate (the
    /// inventory validates every write against the registry), so reaching it
    /// means memory was corrupted or two registries were mixed — a programmer
    /// invariant, not a data problem. It is an error rather than a placeholder
    /// name so a corrupted item can never be written to disk under a name that
    /// would later load as a *different* item (AGENTS.md sections 3.3, 9).
    pub fn to_nbt(&self, items: &ItemRegistry) -> ServerResult<NbtTag> {
        let mut entries: Vec<(String, NbtTag)> = vec![
            ("DataVersion".to_owned(), NbtTag::Int(DATA_VERSION_26_1_2)),
            (
                "Dimension".to_owned(),
                NbtTag::String(self.dimension.clone()),
            ),
            ("Health".to_owned(), NbtTag::Float(self.health)),
            (
                "Pos".to_owned(),
                NbtTag::List(vec![
                    NbtTag::Double(self.position.x),
                    NbtTag::Double(self.position.y),
                    NbtTag::Double(self.position.z),
                ]),
            ),
            (
                "Rotation".to_owned(),
                NbtTag::List(vec![NbtTag::Float(self.yaw), NbtTag::Float(self.pitch)]),
            ),
            (
                "OnGround".to_owned(),
                NbtTag::Byte(i8::from(self.on_ground)),
            ),
            (
                "playerGameType".to_owned(),
                NbtTag::Int(self.game_mode.as_i32()),
            ),
            (
                "SelectedItemSlot".to_owned(),
                NbtTag::Int(i32::from(self.inventory.selected_hotbar())),
            ),
            ("foodLevel".to_owned(), NbtTag::Int(self.food)),
            (
                "foodSaturationLevel".to_owned(),
                NbtTag::Float(self.saturation),
            ),
            (
                "foodExhaustionLevel".to_owned(),
                NbtTag::Float(self.exhaustion),
            ),
            ("XpLevel".to_owned(), NbtTag::Int(self.level)),
            ("XpP".to_owned(), NbtTag::Float(self.experience)),
            ("XpTotal".to_owned(), NbtTag::Int(self.total_experience)),
            (
                "active_effects".to_owned(),
                NbtTag::List(
                    self.effects
                        .values()
                        .map(|effect| {
                            NbtTag::compound([
                                ("id".to_owned(), NbtTag::Int(effect.id)),
                                ("amplifier".to_owned(), NbtTag::Int(effect.amplifier)),
                                ("duration".to_owned(), NbtTag::Int(effect.duration)),
                                ("ambient".to_owned(), NbtTag::Byte(i8::from(effect.ambient))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ];
        let mut inventory: Vec<NbtTag> = Vec::new();
        for index in 0..self.inventory.stored_slots() {
            let stack = self.inventory.slot(index);
            let Some(item_id) = stack.item_id() else {
                continue;
            };
            let name = items.name(item_id).map_err(|_| {
                ServerError::Invariant(format!(
                    "inventory slot {index} holds item id {item_id}, which the item registry does not contain"
                ))
            })?;
            inventory.push(NbtTag::compound([
                (
                    "Slot".to_owned(),
                    NbtTag::Byte(byte_from_i32(
                        i32::try_from(index).unwrap_or(i32::MAX),
                        "inventory slot",
                    )),
                ),
                ("id".to_owned(), NbtTag::String(name.to_owned())),
                (
                    "Count".to_owned(),
                    NbtTag::Byte(byte_from_i32(stack.count(), "item count")),
                ),
            ]));
        }
        if !inventory.is_empty() {
            entries.push(("Inventory".to_owned(), NbtTag::List(inventory)));
        }
        // Preserved entries are appended after the interpreted ones, and an
        // `extra` key that collides with an interpreted key is dropped:
        // `NbtTag::get` is last-wins, so writing both would let stale data
        // shadow the live value on the next load.
        let preserved: Vec<(String, NbtTag)> = self
            .extra
            .iter()
            .filter(|(key, _)| {
                !KNOWN_PLAYER_KEYS.contains(&key.as_str())
                    && !entries.iter().any(|(written, _)| written == key)
            })
            .cloned()
            .collect();
        entries.extend(preserved);
        Ok(NbtTag::Compound(entries))
    }
}

/// Read the `Inventory` list into a resolved inventory.
///
/// Slots outside `0..=40`, entries with a non-integer `Slot`/`Count`, and a
/// `Count` that exceeds either the absolute limit or the item's own stack size
/// are **skipped**, not fatal: vanilla ignores what it cannot place, and one bad
/// row in a long inventory must not cost the player the other 40. An unknown
/// item **name** is fatal — that is a registry/client mismatch, and silently
/// dropping the item would destroy player property (AGENTS.md section 3.3).
fn read_effects(root: &NbtTag) -> BTreeMap<i32, ActiveEffect> {
    let mut effects = BTreeMap::new();
    // Tolerantly, like rows above: one malformed entry is skipped rather than
    // failing the whole player load (a buff is transient; the player is not).
    // Durations and amplifiers are clamped non-negative, matching
    // `ActiveEffect::new`.
    if let Some(entries) = root.get_list("active_effects") {
        for entry in entries {
            let NbtTag::Compound(fields) = entry else {
                continue;
            };
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
            };
            let (Some(id), Some(amplifier), Some(duration)) = (
                get("id")
                    .and_then(NbtTag::as_i64)
                    .and_then(|v| i32::try_from(v).ok()),
                get("amplifier")
                    .and_then(NbtTag::as_i64)
                    .and_then(|v| i32::try_from(v).ok()),
                get("duration")
                    .and_then(NbtTag::as_i64)
                    .and_then(|v| i32::try_from(v).ok()),
            ) else {
                continue;
            };
            if duration <= 0 {
                continue;
            }
            let ambient = get("ambient")
                .and_then(NbtTag::as_i64)
                .is_some_and(|v| v != 0);
            effects.insert(
                id,
                ActiveEffect {
                    id,
                    amplifier: amplifier.max(0),
                    duration,
                    ambient,
                },
            );
        }
    }
    effects
}
fn read_inventory(root: &NbtTag, items: &ItemRegistry) -> ServerResult<PlayerInventory> {
    let mut inventory = crate::inventory::inventory_for_registry(items)?;
    let Some(list) = root.get_list("Inventory") else {
        return Ok(inventory);
    };
    for entry in list {
        if entry.entries().is_none() {
            continue;
        }
        let Some(slot) = entry
            .get_i32("Slot")
            .and_then(|slot| usize::try_from(slot).ok())
        else {
            continue;
        };
        let name = entry
            .get_str("id")
            .ok_or_else(|| ServerError::CorruptData("inventory entry has no item id".to_owned()))?;
        let item_id = items.id(name).map_err(|_| {
            ServerError::CorruptData(format!("player inventory names unknown item {name:?}"))
        })?;
        let Some(count) = entry.get_i32("Count") else {
            continue;
        };
        let Ok(stack) = ItemStack::new(item_id, count) else {
            continue;
        };
        if stack.is_empty() || slot > crate::inventory::LAST_STORED_SLOT {
            continue;
        }
        // Per-item limit: a bucket row claiming 64 is corruption, so the stack
        // is skipped rather than stored oversized.
        if stack.count() > inventory.stack_sizes().max_stack_size(item_id) {
            tracing::warn!(
                slot,
                item = name,
                count = stack.count(),
                "skipping oversized inventory stack in player data"
            );
            continue;
        }
        // Duplicate slots: last wins, matching `NbtTag::get`'s last-wins lookup,
        // so a hand-edited file cannot make the load fail.
        if let Err(error) = inventory.set_slot(slot, stack) {
            tracing::warn!(slot, %error, "skipping unplaceable inventory stack in player data");
        }
    }
    Ok(inventory)
}

/// An `f64` from NBT as `f32`, with a default for a missing or non-finite value.
///
/// Non-finite floats are rejected rather than stored: a NaN position or health
/// would propagate into every later comparison, and JSON/NBT have no NaN literal
/// to round-trip.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is checked for finiteness first, and rotation/health are f32 on the wire"
)]
fn finite_f32(value: Option<f64>, default: f32) -> f32 {
    match value {
        Some(value) if value.is_finite() => value as f32,
        _ => default,
    }
}

/// An optional `f64` from NBT as `f32`, but only when it is inside
/// `min..=max`.
///
/// - **absent or non-finite** → `absent` (the documented default for a missing
///   field, which is not always "zero": a missing `foodSaturationLevel` means a
///   fresh player, not a starving one);
/// - **present but outside the range** → `out_of_range`. Callers pass `0.0`,
///   because a value the schema forbids is corruption, and *rejecting* it beats
///   clamping: clamping `XpP = 42.0` to `1.0` would hand the player a full
///   experience bar, a small but free gain. [`Player::health`] is the deliberate
///   exception elsewhere in this file: it is a real quantity, so the nearest
///   legal value is the honest reading, and death is the safe direction.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is checked for finiteness and range first"
)]
fn in_range_f32(value: Option<f64>, min: f32, max: f32, absent: f32, out_of_range: f32) -> f32 {
    match value {
        None => absent,
        Some(value) if !value.is_finite() => out_of_range,
        Some(value) if value >= f64::from(min) && value <= f64::from(max) => value as f32,
        Some(_) => out_of_range,
    }
}

/// Narrow a known-small count to the `TAG_Byte` the file format uses.
///
/// Values outside `i8` are clamped instead of panicking; the only way to reach
/// the clamp is an item count above 127, which [`ItemStack`] cannot produce.
fn byte_from_i32(value: i32, what: &str) -> i8 {
    if let Ok(value) = i8::try_from(value) {
        value
    } else {
        tracing::warn!(value, what, "clamping a value that does not fit TAG_Byte");
        if value < 0 { i8::MIN } else { i8::MAX }
    }
}

#[cfg(test)]
mod tests {
    // Health, hunger and experience are exact values in this crate's ranges
    // (0.0..=20.0, 0.0..=5.0, the level costs and their exact fractions), and the
    // tests assert them exactly on purpose: an epsilon would hide the rounding
    // bug the assertions exist to catch, because these are equality checks on
    // values that must be bit-identical.
    #![allow(clippy::float_cmp)]
    use super::{
        DamageOutcome, EXHAUSTION_PER_POINT, GameMode, MAX_FOOD, MAX_HEALTH, MAX_SATURATION,
        Player, REGEN_FOOD_THRESHOLD, STARVATION_DAMAGE, Vec3,
    };
    use crate::combat::{CombatStats, DamageSource};
    use crate::inventory::{LAST_STORED_SLOT, PlayerInventory, inventory_for_registry};
    use crate::profile::GameProfile;
    use crate::stack::ItemStack;
    use mc_core::error::ServerError;
    use mc_nbt::NbtTag;
    use mc_registry::ItemRegistry;
    use std::path::Path;

    const STONE: i32 = 1;
    const BUCKET: i32 = 1013;
    const DIAMOND_PICKAXE: i32 = 942;

    fn registry() -> ItemRegistry {
        ItemRegistry::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/test-support/fixtures/registry/items.tsv"),
        )
        .expect("items fixture loads")
    }

    fn empty_inventory() -> PlayerInventory {
        inventory_for_registry(&registry()).expect("table resolves")
    }

    fn profile() -> GameProfile {
        GameProfile::unvalidated("b50ad385-829d-3141-a216-7e7d7539ba7f", "Notch")
    }

    /// A default survival player: full health, full food, empty inventory.
    fn survivor() -> Player {
        Player::new(profile(), 7, "minecraft:overworld", empty_inventory())
    }

    fn stack(item_id: i32, count: i32) -> ItemStack {
        ItemStack::new(item_id, count).expect("legal stack")
    }

    #[test]
    fn game_mode_ids_match_vanilla() {
        for (mode, id) in [
            (GameMode::Survival, 0u8),
            (GameMode::Creative, 1),
            (GameMode::Adventure, 2),
            (GameMode::Spectator, 3),
        ] {
            assert_eq!(mode.id(), id);
            assert_eq!(GameMode::from_id(id), Some(mode));
            assert_eq!(mode.as_i32(), i32::from(id));
        }
        for bad in 4..=255u8 {
            assert_eq!(GameMode::from_id(bad), None, "id {bad} is not a game mode");
        }
        assert!(GameMode::Creative.is_creative());
        assert!(
            GameMode::Spectator.is_creative(),
            "spectator is creative-like"
        );
        assert!(!GameMode::Survival.is_creative());
        assert!(!GameMode::Adventure.is_creative());
        assert_eq!(GameMode::default(), GameMode::Survival);
        assert_eq!(GameMode::ALL.len(), 4);
        assert!(GameMode::default().can_build());
        assert!(!GameMode::Adventure.can_build());
        assert!(!GameMode::Spectator.can_interact());
    }

    #[test]
    fn creative_and_spectator_are_invulnerable() {
        for mode in [GameMode::Creative, GameMode::Spectator] {
            let mut player = survivor();
            player.game_mode = mode;
            let outcome = player.apply_damage(1000.0, DamageSource::MobAttack, &CombatStats::ZERO);
            assert_eq!(
                outcome,
                DamageOutcome {
                    applied: false,
                    died: false,
                    dealt: 0.0,
                    health: MAX_HEALTH,
                }
            );
            assert_eq!(player.health, MAX_HEALTH);
            assert!(player.is_alive());
            assert!(player.is_invulnerable());
        }
        let mut survival = survivor();
        assert!(
            survival
                .apply_damage(1.0, DamageSource::MobAttack, &CombatStats::ZERO)
                .applied
        );
        assert!(!survival.is_invulnerable());
    }

    #[test]
    fn lethal_damage_reports_death_exactly_once() {
        let mut player = survivor();
        let outcome = player.apply_damage(MAX_HEALTH, DamageSource::MobAttack, &CombatStats::ZERO);
        assert_eq!(outcome.dealt, MAX_HEALTH);
        assert_eq!(outcome.health, 0.0);
        assert!(outcome.applied);
        assert!(outcome.died, "the call that reaches 0 reports the death");
        assert!(!player.is_alive());

        let again = player.apply_damage(5.0, DamageSource::MobAttack, &CombatStats::ZERO);
        assert!(!again.applied);
        assert!(!again.died, "death is reported exactly once");
        assert_eq!(again.health, 0.0);

        // Overshooting damage is capped at the health actually present.
        let mut player = survivor();
        let outcome = player.apply_damage(1000.0, DamageSource::MobAttack, &CombatStats::ZERO);
        assert_eq!(outcome.dealt, MAX_HEALTH);
        assert_eq!(player.health, 0.0);
        assert!(outcome.died);
    }

    #[test]
    fn kill_bypasses_invulnerability_but_not_death() {
        // `/kill` works in creative mode (P14-01): it is not ordinary damage.
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Spectator,
            GameMode::Adventure,
        ] {
            let mut player = survivor();
            player.game_mode = mode;
            player.apply_damage(7.0, DamageSource::MobAttack, &CombatStats::ZERO);
            let before = player.health;
            let outcome = player.kill();
            assert_eq!(outcome.dealt, before, "{mode:?}");
            assert_eq!(outcome.health, 0.0);
            assert!(outcome.applied && outcome.died);
            assert!(!player.is_alive());
            // Death is still reported exactly once.
            let again = player.kill();
            assert!(!again.applied && !again.died);
        }
    }

    #[test]
    fn health_never_leaves_its_range() {
        let mut player = survivor();
        player.heal(1000.0);
        assert_eq!(player.health, MAX_HEALTH);
        for _ in 0..50 {
            player.apply_damage(0.25, DamageSource::MobAttack, &CombatStats::ZERO);
        }
        assert_eq!(player.health, 7.5);
        for _ in 0..40 {
            player.apply_damage(1.0, DamageSource::MobAttack, &CombatStats::ZERO);
        }
        assert_eq!(player.health, 0.0);
        assert!(player.health >= 0.0);
        assert!(player.health <= MAX_HEALTH);
        // Hostile damage amounts are ignored, not propagated.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            let mut victim = survivor();
            let outcome = victim.apply_damage(bad, DamageSource::MobAttack, &CombatStats::ZERO);
            assert!(!outcome.applied, "{bad} must not apply");
            assert_eq!(victim.health, MAX_HEALTH);
        }
        // Heal cannot resurrect, and non-positive healing does nothing.
        let mut dead = survivor();
        dead.apply_damage(1000.0, DamageSource::MobAttack, &CombatStats::ZERO);
        dead.heal(5.0);
        assert_eq!(dead.health, 0.0);
        let mut alive = survivor();
        alive.set_health(10.0);
        alive.heal(f32::NAN);
        alive.heal(0.0);
        alive.heal(-3.0);
        assert_eq!(alive.health, 10.0);
        alive.heal(3.0);
        assert_eq!(alive.health, 13.0);
    }

    #[test]
    fn armour_blunts_melee_but_not_falls_or_starvation() {
        // P16-01: full iron (15 points, no toughness) turns a 7.0 melee hit
        // into 3.78; falls and starvation bypass armour entirely (vanilla
        // `bypasses_armor` tag), so the same kit changes nothing for them.
        let iron = CombatStats {
            armor: 15.0,
            toughness: 0.0,
            knockback_resistance: 0.0,
        };
        let mut player = survivor();
        let outcome = player.apply_damage(7.0, DamageSource::MobAttack, &iron);
        assert!((outcome.dealt - 3.78).abs() < 1e-3, "got {}", outcome.dealt);
        assert!((player.health - (MAX_HEALTH - 3.78)).abs() < 1e-3);

        let mut player = survivor();
        let outcome = player.apply_damage(10.0, DamageSource::Fall, &iron);
        assert_eq!(outcome.dealt, 10.0, "falls ignore armour");
        assert_eq!(player.health, MAX_HEALTH - 10.0);

        let mut player = survivor();
        let outcome = player.apply_damage(1.0, DamageSource::Starvation, &iron);
        assert_eq!(outcome.dealt, 1.0, "starvation ignores armour");
    }

    #[test]
    fn health_setters_clamp_and_reject_non_finite_values() {
        let mut player = survivor();
        player.set_health(1000.0);
        assert_eq!(player.health, MAX_HEALTH);
        player.set_health(-1000.0);
        assert_eq!(player.health, 0.0);
        player.set_health(f32::NAN);
        assert_eq!(player.health, 0.0, "NaN must not poison health");
        player.set_health(13.5);
        assert_eq!(player.health, 13.5);
    }

    #[test]
    fn exhaustion_spends_saturation_before_food() {
        let mut player = survivor();
        assert_eq!(player.food, MAX_FOOD);
        assert_eq!(player.saturation, MAX_SATURATION);
        // Not enough exhaustion to spend anything yet.
        player.tick_food(3.0);
        assert_eq!(player.exhaustion, 3.0);
        assert_eq!(player.saturation, MAX_SATURATION);
        assert_eq!(player.food, MAX_FOOD);
        // One more point spends one saturation, leaving food alone.
        player.tick_food(1.0);
        assert_eq!(player.exhaustion, 0.0);
        assert_eq!(player.saturation, MAX_SATURATION - 1.0);
        assert_eq!(
            player.food, MAX_FOOD,
            "food is untouched while saturation lasts"
        );
        // Saturation is clamped to the food level, so it keeps being usable.
        assert!(player.saturation <= Player::max_saturation(player.food));
        assert_eq!(
            player.saturation,
            MAX_SATURATION - 1.0,
            "one exhaustion point spent one saturation"
        );
        // Drain the rest of the saturation (4 points), which leaves food alone.
        player.tick_food(EXHAUSTION_PER_POINT * 4.0);
        assert_eq!(player.saturation, 0.0);
        assert_eq!(player.food, MAX_FOOD, "still no food spent");
        // Now food pays.
        player.tick_food(EXHAUSTION_PER_POINT);
        assert_eq!(player.food, MAX_FOOD - 1);
    }

    #[test]
    fn saturation_is_clamped_to_the_food_level() {
        let mut player = survivor();
        player.set_food(3);
        assert_eq!(player.saturation, 3.0, "saturation cannot exceed food");
        player.set_saturation(100.0);
        assert_eq!(player.saturation, 3.0);
        player.set_saturation(-1.0);
        assert_eq!(player.saturation, 0.0);
        player.set_saturation(f32::NAN);
        assert_eq!(player.saturation, 0.0);
        player.set_food(0);
        assert_eq!(player.saturation, 0.0);
        assert_eq!(Player::max_saturation(0), 0.0);
        assert_eq!(Player::max_saturation(3), 3.0);
        assert_eq!(Player::max_saturation(20), MAX_SATURATION);
        assert_eq!(Player::max_saturation(999), MAX_SATURATION);
    }

    #[test]
    fn regeneration_runs_at_or_above_the_food_threshold() {
        let mut player = survivor();
        player.set_health(10.0);
        player.set_food(REGEN_FOOD_THRESHOLD - 1);
        player.set_saturation(0.0);
        player.tick_food(0.0);
        assert_eq!(player.health, 10.0, "food below 18 does not regenerate");
        assert_eq!(player.food, REGEN_FOOD_THRESHOLD - 1);

        player.set_food(REGEN_FOOD_THRESHOLD);
        player.tick_food(0.0);
        assert_eq!(player.health, 11.0, "food 18 regenerates");
        assert_eq!(
            player.food,
            REGEN_FOOD_THRESHOLD - 1,
            "food without saturation pays for the heal"
        );

        // With saturation available, saturation pays instead of food.
        player.set_food(MAX_FOOD);
        player.set_saturation(MAX_SATURATION);
        player.set_health(10.0);
        player.tick_food(0.0);
        assert_eq!(player.health, 11.0);
        assert_eq!(player.saturation, MAX_SATURATION - 1.0);
        assert_eq!(player.food, MAX_FOOD, "saturated players do not lose food");

        // A healthy player at full health does not spend anything.
        player.set_health(MAX_HEALTH);
        let before = (player.food, player.saturation);
        player.tick_food(0.0);
        assert_eq!((player.food, player.saturation), before);
    }

    #[test]
    fn starvation_only_at_food_zero_and_can_kill() {
        let mut player = survivor();
        player.set_food(0);
        player.set_saturation(0.0);
        player.set_health(MAX_HEALTH);
        let outcome = player.tick_food(0.0);
        assert!(outcome.applied, "food 0 starves");
        assert_eq!(outcome.dealt, STARVATION_DAMAGE);
        assert_eq!(player.health, MAX_HEALTH - STARVATION_DAMAGE);
        assert!(!outcome.died);

        // 20 starvation ticks kill a full-health player, and exactly one of them
        // reports the death.
        let mut deaths = 0;
        for _ in 0..(MAX_HEALTH as i32) {
            if player.tick_food(0.0).died {
                deaths += 1;
            }
            if !player.is_alive() {
                break;
            }
        }
        assert!(!player.is_alive(), "starvation is eventually lethal");
        assert_eq!(deaths, 1, "death reported exactly once");

        // Food 1 does not starve.
        let mut fed = survivor();
        fed.set_food(1);
        fed.set_saturation(0.0);
        let outcome = fed.tick_food(0.0);
        assert!(!outcome.applied);
        assert_eq!(fed.health, MAX_HEALTH);

        // Creative players do not starve.
        let mut creative = survivor();
        creative.game_mode = GameMode::Creative;
        creative.set_food(0);
        assert!(!creative.tick_food(0.0).applied);
        assert_eq!(creative.health, MAX_HEALTH);
    }
    #[test]
    fn exhaustion_at_zero_food_is_discarded_not_accumulated() {
        let mut player = survivor();
        player.set_food(0);
        player.set_saturation(0.0);
        player.set_health(MAX_HEALTH);
        player.tick_food(EXHAUSTION_PER_POINT * 3.0);
        assert_eq!(player.exhaustion, 0.0, "nothing left to spend it on");
        assert_eq!(player.food, 0);
        // Hostile exhaustion values are ignored.
        player.tick_food(f32::NAN);
        player.tick_food(-100.0);
        assert_eq!(player.exhaustion, 0.0);
    }

    #[test]
    fn level_formula_matches_all_three_branches() {
        for (level, needed) in [
            (0, 7),
            (1, 9),
            (10, 27),
            (15, 37),
            (16, 42),
            (20, 62),
            (30, 112),
            (31, 121),
            (35, 157),
        ] {
            assert_eq!(
                Player::experience_needed_for_level(level),
                needed,
                "level {level}"
            );
        }
        assert_eq!(
            Player::experience_needed_for_level(-5),
            7,
            "a negative level is treated as 0"
        );
        // Branch boundaries: 15 -> 16 jumps from 2L+7 to 5L-38, 30 -> 31 from
        // 5L-38 to 9L-158.
        assert_eq!(Player::experience_needed_for_level(16), 5 * 16 - 38);
        assert_eq!(Player::experience_needed_for_level(31), 9 * 31 - 158);
    }

    #[test]
    fn add_experience_carries_the_remainder_into_the_next_level() {
        let mut player = survivor();
        assert_eq!(player.add_experience(0), 0, "zero points gain nothing");
        assert_eq!(player.add_experience(-50), 0);
        // Level 0 costs 7 points, level 1 costs 9, so 10 points is exactly one
        // level with 3 of the 9 needed for level 2 on the bar.
        let gained = player.add_experience(10);
        assert_eq!(gained, 1, "7 points for level 0, 3 carried");
        assert_eq!(player.level, 1);
        assert_eq!(player.experience, 3.0 / 9.0);
        assert_eq!(player.total_experience, 10);
        assert!((player.experience_progress() - 3.0 / 9.0).abs() < 1.0e-5);

        // A fresh player: level 0 needs 7, level 1 needs 9, level 2 needs 11 --
        // 27 points is exactly three levels with an empty bar.
        let mut player = survivor();
        assert_eq!(player.add_experience(27), 3, "0->1->2->3 costs 27");
        assert_eq!(player.level, 3);
        assert_eq!(player.experience, 0.0, "an exact spend leaves an empty bar");

        // Enough for many levels at once: 700 points reach level 22 with 21 of
        // the 72 needed for level 23 (levels 0..=21 cost 679 points).
        let mut player = survivor();
        let gained = player.add_experience(700);
        assert_eq!(gained, 22, "700 points reach level 22");
        assert_eq!(player.level, 22);
        assert_eq!(player.total_experience, 700);
        assert!((player.experience - 21.0 / 72.0).abs() < 1.0e-5);

        // Far enough to cross into the third formula branch (level > 30, where
        // the cost is 9L-158): 32 625 points reach level 102 with 162 of the 760
        // needed for level 103 (levels 0..=101 cost 32 463 points).
        let mut player = survivor();
        let gained = player.add_experience(32_625);
        assert_eq!(player.level, 102, "the third branch is reached");
        assert_eq!(gained, 102);
        assert!((player.experience - 162.0 / 760.0).abs() < 1.0e-5);

        // The bar stays in 0.0..1.0 for every reachable grant size.
        let mut player = survivor();
        for _ in 0..200 {
            player.add_experience(37);
            assert!(
                (0.0..=1.0).contains(&player.experience),
                "XpP is a fraction, not points: {}",
                player.experience
            );
        }
        assert!(player.level > 0);
    }

    #[test]
    fn a_non_finite_experience_bar_is_repaired() {
        let mut player = survivor();
        player.experience = f32::NAN;
        player.level = -4;
        player.add_experience(7);
        assert_eq!(player.level, 1, "a negative level is normalised first");
        assert_eq!(player.experience, 0.0);
        player.experience = 0.5;
        assert!((player.experience_progress() - 0.5).abs() < 1.0e-5);
    }

    #[test]
    fn respawn_restores_vitals_and_reports_the_dropped_inventory() {
        let mut player = survivor();
        player.set_health(0.0);
        player.set_food(3);
        player.set_saturation(1.0);
        player.exhaustion = 2.0;
        player.on_ground = true;
        player.game_mode = GameMode::Adventure;
        player.add_experience(100);
        player
            .inventory
            .set_slot(0, stack(STONE, 10))
            .expect("slot 0");
        player
            .inventory
            .set_slot(LAST_STORED_SLOT, stack(BUCKET, 1))
            .expect("offhand");

        let dropped = player.respawn(false);
        assert_eq!(player.health, MAX_HEALTH);
        assert_eq!(player.food, MAX_FOOD);
        assert_eq!(player.saturation, MAX_SATURATION);
        assert_eq!(player.exhaustion, 0.0);
        assert!(!player.on_ground);
        assert_eq!(
            player.game_mode,
            GameMode::Adventure,
            "respawn keeps the mode"
        );
        assert_eq!(player.level, 0, "XP is lost without keepInventory");
        assert_eq!(player.total_experience, 0);
        assert_eq!(dropped.len(), 2);
        assert_eq!(dropped[0], stack(STONE, 10));
        assert_eq!(dropped[1], stack(BUCKET, 1));
        assert_eq!(
            player.inventory.slot(0),
            stack(STONE, 10),
            "respawn does not clear the inventory; the caller drops `dropped`"
        );
    }

    #[test]
    fn respawn_with_keep_inventory_keeps_experience_and_drops_nothing() {
        let mut player = survivor();
        player.set_health(0.0);
        player.add_experience(100);
        player
            .inventory
            .set_slot(0, stack(STONE, 10))
            .expect("slot 0");
        let level = player.level;
        let dropped = player.respawn(true);
        assert!(dropped.is_empty(), "keepInventory drops nothing");
        assert_eq!(player.level, level);
        assert_eq!(player.total_experience, 100);
        assert_eq!(player.inventory.slot(0), stack(STONE, 10));
    }

    #[test]
    fn vec3_geometry_is_finite_safe() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(1.0, 2.0, 6.0);
        assert_eq!(a.plus(b), Vec3::new(2.0, 4.0, 9.0));
        assert_eq!(b.minus(a), Vec3::new(0.0, 0.0, 3.0));
        assert_eq!(a.distance(b), 3.0);
        assert_eq!(Vec3::ZERO.length(), 0.0);
        assert!(a.is_finite());
        assert!(!Vec3::new(f64::NAN, 0.0, 0.0).is_finite());
        assert_eq!(Vec3::default(), Vec3::ZERO);
    }

    #[test]
    fn new_player_defaults_are_full_vitals_at_the_origin() {
        let player = survivor();
        assert_eq!(player.profile, profile());
        assert_eq!(player.entity_id, 7);
        assert_eq!(player.dimension, "minecraft:overworld");
        assert_eq!(player.position, Vec3::ZERO);
        assert_eq!((player.yaw, player.pitch), (0.0, 0.0));
        assert!(!player.on_ground);
        assert_eq!(player.game_mode, GameMode::Survival);
        assert_eq!(player.health, MAX_HEALTH);
        assert_eq!(player.food, MAX_FOOD);
        assert_eq!(player.saturation, MAX_SATURATION);
        assert_eq!(player.exhaustion, 0.0);
        assert_eq!(player.level, 0);
        assert_eq!(player.total_experience, 0);
        assert!(player.inventory.slot(0).is_empty());
        assert!(player.extra.is_empty());
        assert!(player.is_alive());
    }

    #[test]
    fn a_fully_populated_player_round_trips_through_nbt() {
        let items = registry();
        let mut player = survivor();
        player.game_mode = GameMode::Creative;
        player.position = Vec3::new(-12.5, 64.0625, 101.5);
        player.yaw = 90.5;
        player.pitch = -45.25;
        player.on_ground = true;
        player.set_health(13.5);
        player.set_food(17);
        player.set_saturation(2.5);
        player.exhaustion = 1.25;
        player.level = 35;
        player.experience = 0.75;
        player.total_experience = 1234;
        player.inventory.select(4).expect("select");
        player
            .inventory
            .set_slot(4, stack(STONE, 64))
            .expect("hotbar");
        player
            .inventory
            .set_slot(20, stack(DIAMOND_PICKAXE, 1))
            .expect("main");
        player
            .inventory
            .set_slot(39, stack(BUCKET, 16))
            .expect("helmet slot");
        player
            .inventory
            .set_slot(LAST_STORED_SLOT, stack(STONE, 3))
            .expect("offhand");
        player.extra.push((
            "SomeModData".to_owned(),
            NbtTag::String("keep me".to_owned()),
        ));

        let root = player.to_nbt(&items).expect("encodes");
        let decoded =
            Player::from_nbt(&root, profile(), 7, &items).expect("a self-written document loads");
        assert_eq!(decoded, player);
        assert_eq!(decoded.game_mode, GameMode::Creative);
        assert_eq!(decoded.position, Vec3::new(-12.5, 64.0625, 101.5));
        assert_eq!(decoded.inventory.slot(20), stack(DIAMOND_PICKAXE, 1));
        assert_eq!(decoded.inventory.selected_hotbar(), 4);
        // And it stays equal through a second cycle.
        assert_eq!(decoded.to_nbt(&items).expect("re-encodes"), root);
    }

    #[test]
    fn effects_round_trip_through_playerdata() {
        let items = registry();
        let mut player = survivor();
        player.give_effect(crate::effect::effect_id::POISON, 1, 200);
        player.give_effect(crate::effect::effect_id::SPEED, 0, 600);
        let root = player.to_nbt(&items).expect("encodes");
        let decoded = Player::from_nbt(&root, profile(), 7, &items).expect("loads");
        assert_eq!(decoded.effects, player.effects);
        assert_eq!(decoded, player);
        assert_eq!(decoded.to_nbt(&items).expect("re-encodes"), root);
    }

    #[test]
    fn give_effect_refresh_follows_better_wins() {
        use crate::effect::effect_id::POISON;
        let mut player = survivor();
        player.give_effect(POISON, 0, 100);
        // Weaker amplifier never displaces.
        player.give_effect(POISON, 0, 50);
        assert_eq!(player.effects[&POISON].duration, 100);
        // Longer duration at equal amplifier refreshes.
        player.give_effect(POISON, 0, 200);
        assert_eq!(player.effects[&POISON].duration, 200);
        // Higher amplifier wins even when shorter.
        player.give_effect(POISON, 1, 10);
        assert_eq!(player.effects[&POISON].amplifier, 1);
        // Non-positive durations are refused, not stored.
        player.give_effect(POISON, 5, 0);
        assert_eq!(player.effects[&POISON].amplifier, 1);
        assert!(player.clear_effect(POISON));
        assert!(!player.clear_effect(POISON));
        assert!(player.effects.is_empty());
    }

    #[test]
    fn poison_ticks_damage_and_floors_at_half_a_heart() {
        use crate::combat::CombatStats;
        use crate::effect::effect_id::POISON;
        let mut player = survivor();
        player.give_effect(POISON, 0, 1000);
        // Amplifier 0 hits every 25th tick; run a window with exactly four.
        player.tick_effects(24, &CombatStats::ZERO);
        assert_eq!(player.health, MAX_HEALTH, "tick 24 is quiet");
        player.tick_effects(25, &CombatStats::ZERO);
        assert_eq!(player.health, MAX_HEALTH - 1.0);
        // Near death the floor holds: poison never kills.
        player.set_health(1.0);
        for tick in 26..60 {
            player.tick_effects(tick, &CombatStats::ZERO);
        }
        assert_eq!(player.health, 1.0, "poison stops at half a heart");
        assert!(player.is_alive());
    }

    #[test]
    fn wither_ticks_can_kill_and_regeneration_heals() {
        use crate::combat::CombatStats;
        use crate::effect::{effect_id::REGENERATION, effect_id::WITHER};
        let mut player = survivor();
        player.set_health(3.0);
        player.give_effect(WITHER, 4, 100);
        // Amplifier 4 hits every tick (25 >> 4 = 1).
        player.tick_effects(7, &CombatStats::ZERO);
        assert_eq!(player.health, 2.0);
        player.tick_effects(8, &CombatStats::ZERO);
        player.tick_effects(9, &CombatStats::ZERO);
        assert_eq!(player.health, 0.0);
        assert!(!player.is_alive(), "wither finishes the job");

        let mut player = survivor();
        player.set_health(10.0);
        player.give_effect(REGENERATION, 0, 200);
        // Amplifier 0 heals every 50th tick.
        player.tick_effects(49, &CombatStats::ZERO);
        assert_eq!(player.health, 10.0, "tick 49 is quiet");
        let expired = player.tick_effects(50, &CombatStats::ZERO);
        assert_eq!(player.health, 11.0);
        assert!(expired.is_empty(), "nothing expired yet");
    }

    #[test]
    fn expired_effects_come_back_for_removal() {
        use crate::combat::CombatStats;
        use crate::effect::effect_id::SPEED;
        let mut player = survivor();
        player.give_effect(SPEED, 0, 2);
        assert!(player.tick_effects(1, &CombatStats::ZERO).is_empty());
        assert_eq!(player.tick_effects(2, &CombatStats::ZERO), vec![SPEED]);
        assert!(player.effects.is_empty());
    }

    #[test]
    fn a_minimal_player_gets_documented_defaults() {
        let items = registry();
        // The smallest document vanilla could write for a live player.
        let root = NbtTag::Compound(vec![("playerGameType".to_owned(), NbtTag::Int(0))]);
        let player = Player::from_nbt(&root, profile(), 3, &items).expect("loads");
        assert_eq!(player.dimension, "minecraft:overworld");
        assert_eq!(player.position, Vec3::ZERO);
        assert_eq!((player.yaw, player.pitch), (0.0, 0.0));
        assert!(!player.on_ground);
        assert_eq!(player.health, MAX_HEALTH);
        assert_eq!(player.food, MAX_FOOD);
        assert_eq!(player.saturation, MAX_SATURATION);
        assert_eq!(player.exhaustion, 0.0);
        assert_eq!(player.level, 0);
        assert_eq!(player.experience, 0.0);
        assert_eq!(player.total_experience, 0);
        assert_eq!(player.inventory.selected_hotbar(), 0);
        assert!(player.extra.is_empty());
        assert_eq!(player.entity_id, 3, "the entity system assigns the id");
    }

    #[test]
    fn out_of_range_scalar_fields_are_normalised_not_fatal() {
        let items = registry();
        let root = NbtTag::Compound(vec![
            ("DataVersion".to_owned(), NbtTag::Int(4790)),
            ("playerGameType".to_owned(), NbtTag::Int(2)),
            ("Health".to_owned(), NbtTag::Float(9999.0)),
            ("foodLevel".to_owned(), NbtTag::Int(500)),
            ("foodSaturationLevel".to_owned(), NbtTag::Float(9999.0)),
            ("foodExhaustionLevel".to_owned(), NbtTag::Float(-5.0)),
            ("XpLevel".to_owned(), NbtTag::Int(-7)),
            ("XpP".to_owned(), NbtTag::Float(42.0)),
            ("XpTotal".to_owned(), NbtTag::Int(-1)),
            ("SelectedItemSlot".to_owned(), NbtTag::Int(200)),
            ("Pos".to_owned(), NbtTag::List(vec![NbtTag::Double(1.0)])),
            (
                "Rotation".to_owned(),
                NbtTag::List(vec![NbtTag::String("x".to_owned())]),
            ),
        ]);
        let player = Player::from_nbt(&root, profile(), 1, &items).expect("loads");
        assert_eq!(player.game_mode, GameMode::Adventure);
        assert_eq!(player.health, MAX_HEALTH, "clamped into range");
        assert_eq!(player.food, MAX_FOOD);
        assert_eq!(player.saturation, 0.0, "an out-of-range ratio is rejected");
        assert_eq!(player.exhaustion, 0.0, "negative exhaustion clamped");
        assert_eq!(player.level, 0);
        assert_eq!(player.experience, 0.0);
        assert_eq!(player.total_experience, 0);
        assert_eq!(player.inventory.selected_hotbar(), 0, "hotbar clamped to 0");
        assert_eq!(player.position, Vec3::ZERO, "a short Pos is not trusted");
        assert_eq!(
            (player.yaw, player.pitch),
            (0.0, 0.0),
            "a bad Rotation is not trusted"
        );
    }

    #[test]
    fn unknown_top_level_entries_survive_a_round_trip() {
        let items = registry();
        let player = survivor();
        let mut expected = player.clone();
        expected.extra.push(("Air".to_owned(), NbtTag::Short(300)));
        expected.extra.push((
            "abilities".to_owned(),
            NbtTag::compound([("flying".to_owned(), NbtTag::Byte(1))]),
        ));
        expected
            .extra
            .push(("UUID".to_owned(), NbtTag::IntArray(vec![1, 2, 3, 4])));

        let loaded = Player::from_nbt(
            &expected.to_nbt(&items).expect("encodes"),
            profile(),
            1,
            &items,
        )
        .expect("loads");
        assert_eq!(loaded.extra.len(), 3);
        assert_eq!(
            loaded
                .extra
                .iter()
                .find(|(key, _)| key == "Air")
                .map(|(_, value)| value.clone()),
            Some(NbtTag::Short(300))
        );
        assert_eq!(loaded, expected, "preserved entries are part of equality");

        let reencoded = loaded.to_nbt(&items).expect("re-encodes");
        assert_eq!(
            reencoded.get_i16("Air"),
            Some(300),
            "an uninterpreted entry survives"
        );
        assert!(reencoded.get_compound("abilities").is_some());
        assert_eq!(reencoded.get_int_array("UUID"), Some(&[1, 2, 3, 4][..]));
    }

    #[test]
    fn an_extra_entry_cannot_shadow_an_interpreted_one() {
        let items = registry();
        let mut player = survivor();
        player.set_health(5.0);
        player
            .extra
            .push(("Health".to_owned(), NbtTag::Float(20.0)));
        let root = player.to_nbt(&items).expect("encodes");
        assert_eq!(root.get_f64("Health"), Some(5.0), "the live value wins");
        let reloaded = Player::from_nbt(&root, profile(), 1, &items).expect("loads");
        assert_eq!(reloaded.health, 5.0);
        assert!(reloaded.extra.is_empty(), "the interpreted key is consumed");
    }

    #[test]
    fn an_unknown_item_name_is_corrupt_data() {
        let items = registry();
        let root = NbtTag::Compound(vec![
            ("playerGameType".to_owned(), NbtTag::Int(0)),
            (
                "Inventory".to_owned(),
                NbtTag::List(vec![NbtTag::compound([
                    ("Slot".to_owned(), NbtTag::Byte(0)),
                    (
                        "id".to_owned(),
                        NbtTag::String("minecraft:not_an_item".to_owned()),
                    ),
                    ("Count".to_owned(), NbtTag::Byte(1)),
                ])]),
            ),
        ]);
        let err = Player::from_nbt(&root, profile(), 1, &items).expect_err("unknown item");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    }

    #[test]
    fn item_entries_are_written_in_the_vanilla_shape() {
        let items = registry();
        let mut player = survivor();
        player
            .inventory
            .set_slot(5, stack(STONE, 12))
            .expect("slot");
        let root = player.to_nbt(&items).expect("encodes");
        let list = root.get_list("Inventory").expect("Inventory list");
        assert_eq!(list.len(), 1);
        let entry = &list[0];
        assert_eq!(entry.get_i32("Slot"), Some(5), "Slot is a byte");
        assert_eq!(entry.get_str("id"), Some("minecraft:stone"), "id is a name");
        assert_eq!(entry.get_i32("Count"), Some(12), "Count is a byte");
        assert!(matches!(entry.get("Slot"), Some(NbtTag::Byte(_))));
        assert!(matches!(entry.get("Count"), Some(NbtTag::Byte(_))));
        // An empty inventory omits the list entirely, like vanilla.
        let empty = player_only().to_nbt(&items).expect("encodes");
        assert!(empty.get("Inventory").is_none());

        // A bucket count above the per-item limit is skipped, not stored.
        let oversized = NbtTag::Compound(vec![
            ("playerGameType".to_owned(), NbtTag::Int(0)),
            (
                "Inventory".to_owned(),
                NbtTag::List(vec![NbtTag::compound([
                    ("Slot".to_owned(), NbtTag::Byte(0)),
                    (
                        "id".to_owned(),
                        NbtTag::String("minecraft:bucket".to_owned()),
                    ),
                    ("Count".to_owned(), NbtTag::Byte(64)),
                ])]),
            ),
        ]);
        let loaded = Player::from_nbt(&oversized, profile(), 1, &items).expect("loads");
        assert!(
            loaded.inventory.slot(0).is_empty(),
            "a 64-bucket row is corruption, not a stack"
        );
    }

    /// A player with a guaranteed-empty inventory, for the "omitted list" case.
    fn player_only() -> Player {
        let mut player = survivor();
        for index in 0..player.inventory.stored_slots() {
            player
                .inventory
                .set_slot(index, ItemStack::EMPTY)
                .expect("clear");
        }
        player
    }

    #[test]
    fn slots_outside_the_inventory_are_ignored() {
        let items = registry();
        let root = NbtTag::Compound(vec![
            ("playerGameType".to_owned(), NbtTag::Int(0)),
            (
                "Inventory".to_owned(),
                NbtTag::List(vec![
                    NbtTag::compound([
                        ("Slot".to_owned(), NbtTag::Byte(45)),
                        (
                            "id".to_owned(),
                            NbtTag::String("minecraft:stone".to_owned()),
                        ),
                        ("Count".to_owned(), NbtTag::Byte(1)),
                    ]),
                    NbtTag::compound([
                        ("Slot".to_owned(), NbtTag::Byte(-1)),
                        (
                            "id".to_owned(),
                            NbtTag::String("minecraft:stone".to_owned()),
                        ),
                        ("Count".to_owned(), NbtTag::Byte(1)),
                    ]),
                    NbtTag::Int(7),
                ]),
            ),
        ]);
        let player = Player::from_nbt(&root, profile(), 1, &items).expect("loads");
        assert!(player.inventory.slot(0).is_empty());
        assert!(player.inventory.slot(40).is_empty());
    }

    #[test]
    fn truncated_or_garbage_player_nbt_errors_without_panicking() {
        let items = registry();
        let garbage: Vec<NbtTag> = vec![
            NbtTag::Int(5),
            NbtTag::List(vec![]),
            NbtTag::ByteArray(vec![0xFF; 64]),
            NbtTag::String("not a player".to_owned()),
            NbtTag::Compound(vec![(
                "playerGameType".to_owned(),
                NbtTag::String("x".to_owned()),
            )]),
            NbtTag::Compound(vec![("playerGameType".to_owned(), NbtTag::Int(9))]),
            NbtTag::Compound(vec![("playerGameType".to_owned(), NbtTag::Int(-1))]),
            NbtTag::Compound(vec![(
                "playerGameType".to_owned(),
                NbtTag::Long(i64::from(i32::MAX) + 1),
            )]),
            NbtTag::Compound(vec![
                ("playerGameType".to_owned(), NbtTag::Int(0)),
                ("Inventory".to_owned(), NbtTag::Int(3)),
            ]),
            NbtTag::Compound(vec![
                ("playerGameType".to_owned(), NbtTag::Int(0)),
                (
                    "Inventory".to_owned(),
                    NbtTag::List(vec![NbtTag::compound([])]),
                ),
            ]),
        ];
        for tag in garbage {
            let result = Player::from_nbt(&tag, profile(), 1, &items);
            match result {
                Ok(player) => {
                    // Only the "no playerGameType" compounds are loadable, and
                    // none of them are in this list except the empty-inventory
                    // garbage cases, which must still be in range.
                    assert!(player.health >= 0.0 && player.health <= MAX_HEALTH);
                }
                Err(err) => assert!(
                    matches!(err, ServerError::CorruptData(_)),
                    "unexpected error class: {err:?}"
                ),
            }
        }
    }

    #[test]
    fn truncated_nbt_bytes_do_not_produce_a_player() {
        use mc_nbt::{Limits, read_named};
        let items = registry();
        let mut bytes = Vec::new();
        mc_nbt::write_named("", &survivor().to_nbt(&items).expect("encodes"), &mut bytes)
            .expect("writes");
        // Every truncation must fail to parse; none may panic.
        for cut in 0..bytes.len() {
            assert!(
                read_named(&bytes[..cut], Limits::DISK).is_err(),
                "a {cut}-byte prefix is not a complete document"
            );
        }
        // Bit flips in the header region must not panic either.
        for index in 0..bytes.len().min(24) {
            let mut corrupted = bytes.clone();
            corrupted[index] ^= 0xFF;
            let _ = read_named(&corrupted, Limits::DISK)
                .and_then(|(_, tag)| Player::from_nbt(&tag, profile(), 1, &items).map(|_| ()));
        }
    }

    #[test]
    fn a_player_data_version_is_written() {
        let items = registry();
        let root = survivor().to_nbt(&items).expect("encodes");
        assert_eq!(
            root.get_i32("DataVersion"),
            Some(super::DATA_VERSION_26_1_2)
        );
        assert_eq!(super::DATA_VERSION_26_1_2, 4790);
        // The version is interpreted, so an `extra` copy is dropped rather than
        // duplicated into the document.
        let mut with_extra_version = survivor();
        with_extra_version.extra.push((
            "DataVersion".to_owned(),
            NbtTag::Int(super::DATA_VERSION_26_1_2),
        ));
        let reencoded = with_extra_version.to_nbt(&items).expect("encodes");
        let versions = reencoded
            .entries()
            .expect("compound")
            .iter()
            .filter(|(key, _)| key == "DataVersion")
            .count();
        assert_eq!(versions, 1, "DataVersion appears exactly once");
        let loaded = Player::from_nbt(&reencoded, profile(), 1, &items).expect("loads");
        assert!(loaded.extra.is_empty());
    }
}
