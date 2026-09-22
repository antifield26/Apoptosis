//! Status effects (P05-10).
//!
//! Vanilla applies effects as a per-tick modifier on attributes and behaviour.
//! Phase 05 needs the *container* and the tick semantics; it does not need the
//! full 40-effect table, because only a handful are reachable before mobs and
//! potions exist.
//!
//! ## What is modelled, and what is not
//!
//! Modelled: an effect's identity, amplifier, remaining duration, the ambient flag,
//! expiry, and the numeric modifiers a caller can ask for — movement speed,
//! damage taken, and melee attack damage (Strength/Weakness). [`EffectKind`]
//! lists the effects whose *numeric* effect we apply.
//!
//! **Not** modelled, and therefore not claimed: particles, effect colour,
//! per-effect behaviour beyond those modifiers (poison, wither, levitation,
//! blindness…), instant effects (they apply once rather than over time), effect
//! removal by milk, beacons or conduits. Icons reach the client via
//! `update_mob_effect` (P16-03).

/// Effect ids, stable identity for storage and tests (P05-10).
///
/// The legacy 1-based numbering is storage (and log) identity only — it is
/// NOT the wire numbering. The update and remove packets carry the effect
/// as a `Holder<MobEffect>` through `MobEffect.STREAM_CODEC`, which is
/// `ByteBufCodecs.holderRegistry` (anonymous class `$29`, verified by
/// `javap -c` on the 26.1.2 jar): encode writes the raw registry id with
/// `VarInt`, no offset (`IdMap.getIdOrThrow` straight into the buffer;
/// decode is `byIdOrThrow` of the same number). Round 1 examined the wrong
/// codec (`holder` / `$30`, the inline-capable variant that writes
/// `raw_id + 1` with 0 as the inline marker) and concluded the legacy
/// table already satisfied the wire — a live client then showed
/// `/effect give speed` as Slowness, exactly the +1 shift. [`EffectKind::wire_id`]
/// subtracts the one back to the raw id; playerdata persists the legacy
/// table unchanged.
///
/// Only the ids whose numeric effect this crate applies are named; a numeric id
/// that is not recognised is still stored (so a future effect can round-trip) but
/// contributes no modifier.
pub mod effect_id {
    /// `minecraft:speed` — movement speed up.
    pub const SPEED: i32 = 1;
    /// `minecraft:slowness` — movement speed down.
    pub const SLOWNESS: i32 = 2;
    /// `minecraft:haste` — mining speed up.
    pub const HASTE: i32 = 3;
    /// `minecraft:mining_fatigue` — mining speed down.
    pub const MINING_FATIGUE: i32 = 4;
    /// `minecraft:strength` — attack damage up.
    pub const STRENGTH: i32 = 5;
    /// `minecraft:instant_health`.
    pub const INSTANT_HEALTH: i32 = 6;
    /// `minecraft:instant_damage`.
    pub const INSTANT_DAMAGE: i32 = 7;
    /// `minecraft:jump_boost`.
    pub const JUMP_BOOST: i32 = 8;
    /// `minecraft:regeneration` — health over time.
    pub const REGENERATION: i32 = 10;
    /// `minecraft:resistance` — damage taken down.
    pub const RESISTANCE: i32 = 11;
    /// `minecraft:fire_resistance`.
    pub const FIRE_RESISTANCE: i32 = 12;
    /// `minecraft:water_breathing`.
    pub const WATER_BREATHING: i32 = 13;
    /// `minecraft:weakness` — attack damage down.
    pub const WEAKNESS: i32 = 18;
    /// `minecraft:poison` — damage over time.
    pub const POISON: i32 = 19;
    /// `minecraft:wither` — damage over time.
    pub const WITHER: i32 = 20;
    /// `minecraft:slow_falling`.
    pub const SLOW_FALLING: i32 = 28;
}

/// Effects whose numeric contribution this crate applies.
///
/// Anything else is stored and ticked but changes no modifier, which is honest:
/// the alternative is a table of numbers we have not verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectKind {
    /// Movement speed up (id 1).
    Speed,
    /// Movement speed down (id 2).
    Slowness,
    /// Attack damage up (id 5).
    Strength,
    /// Attack damage down (id 18).
    Weakness,
    /// Damage taken down (id 11).
    Resistance,
    /// Damage over time (id 19).
    Poison,
    /// Damage over time (id 20).
    Wither,
    /// Health over time (id 10).
    Regeneration,
}

impl EffectKind {
    /// Recognise a wire id.
    #[must_use]
    pub const fn from_id(id: i32) -> Option<Self> {
        match id {
            effect_id::SPEED => Some(Self::Speed),
            effect_id::SLOWNESS => Some(Self::Slowness),
            effect_id::STRENGTH => Some(Self::Strength),
            effect_id::WEAKNESS => Some(Self::Weakness),
            effect_id::RESISTANCE => Some(Self::Resistance),
            effect_id::POISON => Some(Self::Poison),
            effect_id::WITHER => Some(Self::Wither),
            effect_id::REGENERATION => Some(Self::Regeneration),
            _ => None,
        }
    }

    /// The wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Speed => effect_id::SPEED,
            Self::Slowness => effect_id::SLOWNESS,
            Self::Strength => effect_id::STRENGTH,
            Self::Weakness => effect_id::WEAKNESS,
            Self::Resistance => effect_id::RESISTANCE,
            Self::Poison => effect_id::POISON,
            Self::Wither => effect_id::WITHER,
            Self::Regeneration => effect_id::REGENERATION,
        }
    }

    /// The id the 26.1 client reads on the wire: the raw registry id.
    ///
    /// `update_mob_effect` / `remove_mob_effect` carry the effect through
    /// `MobEffect.STREAM_CODEC` = `ByteBufCodecs.holderRegistry` (`$29`,
    /// jar-verified: `IdMap.getIdOrThrow` written as a bare `VarInt`, no
    /// offset), so the legacy stored id minus one is the wire value
    /// (speed stored 1 rides 0). Round 1 read the wrong codec (`holder` /
    /// `$30`, `raw_id + 1`) and sent the stored id unchanged; a live client
    /// showed speed as Slowness, the exact +1 shift, and this subtraction
    /// is the correction. Do not "simplify" it away without jar evidence.
    #[must_use]
    pub const fn wire_id(self) -> i32 {
        self.id() - 1
    }

    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Slowness => "slowness",
            Self::Strength => "strength",
            Self::Weakness => "weakness",
            Self::Resistance => "resistance",
            Self::Poison => "poison",
            Self::Wither => "wither",
            Self::Regeneration => "regeneration",
        }
    }

    /// Recognise a `minecraft:<name>` effect id for the eight modelled kinds.
    /// Anything else (jump boost, fire resistance, ...) is storable but has
    /// no numeric behaviour here, so the command layer refuses it rather than
    /// granting a dead icon.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        Some(match bare {
            "speed" => Self::Speed,
            "slowness" => Self::Slowness,
            "strength" => Self::Strength,
            "weakness" => Self::Weakness,
            "resistance" => Self::Resistance,
            "poison" => Self::Poison,
            "wither" => Self::Wither,
            "regeneration" => Self::Regeneration,
            _ => return None,
        })
    }

    /// Whether this effect damages over time.
    #[must_use]
    pub const fn is_harmful_over_time(self) -> bool {
        matches!(self, Self::Poison | Self::Wither)
    }
}

/// The wire id of a stored effect id, or `None` when the stored id names
/// no modelled kind.
///
/// The update/remove packets must carry registry raw ids; a stored id from
/// outside the modelled set (a future effect round-tripping through
/// playerdata) has no known raw id, so callers skip its packet rather than
/// send a neighbouring effect's icon.
#[must_use]
pub fn wire_id_of(stored: i32) -> Option<i32> {
    EffectKind::from_id(stored).map(EffectKind::wire_id)
}

/// One active effect on an entity.
///
/// Field names match what the tick loop and the `update_mob_effect`
/// encoder need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveEffect {
    /// Stored (legacy 1-based) id of the effect; see [`EffectKind::wire_id`]
    /// for what goes on the wire.
    pub id: i32,
    /// Amplifier, 0-based (amplifier 0 is "level I").
    pub amplifier: i32,
    /// Remaining duration in ticks.
    pub duration: i32,
    /// Whether the effect came from a beacon/conduit rather than a potion.
    pub ambient: bool,
}

impl ActiveEffect {
    /// A new effect; duration is clamped to a non-negative value.
    #[must_use]
    pub fn new(id: i32, amplifier: i32, duration: i32) -> Self {
        Self {
            id,
            amplifier: amplifier.max(0),
            duration: duration.max(0),
            ambient: false,
        }
    }

    /// The effect kind, when this crate models its numeric behaviour.
    #[must_use]
    pub const fn kind(&self) -> Option<EffectKind> {
        EffectKind::from_id(self.id)
    }

    /// Amplifier as a "level" for display and for the vanilla formulas, which are
    /// written in terms of `amplifier + 1`.
    #[must_use]
    pub const fn level(&self) -> i32 {
        self.amplifier.saturating_add(1)
    }

    /// Whether the effect has run out.
    #[must_use]
    pub const fn is_expired(&self) -> bool {
        self.duration <= 0
    }
}

/// Multiplier applied to movement speed given a set of active effects.
///
/// Vanilla adds `0.2 × level` for Speed and subtracts `0.15 × level` for Slowness.
/// Both are documented community values rather than constants read out of the jar;
/// they are **not** presented as verified parity, and the result is clamped to a
/// positive range so a hostile stack of effects cannot produce a negative or absurd
/// speed (AGENTS.md §10).
#[must_use]
pub fn movement_speed_multiplier(effects: &[ActiveEffect]) -> f64 {
    let mut multiplier = 1.0f64;
    for effect in effects {
        match effect.kind() {
            Some(EffectKind::Speed) => multiplier += 0.2 * f64::from(effect.level()),
            Some(EffectKind::Slowness) => multiplier -= 0.15 * f64::from(effect.level()),
            _ => {}
        }
    }
    multiplier.clamp(0.0, 4.0)
}

/// Multiplier applied to incoming damage given a set of active effects.
///
/// Vanilla's Resistance reduces damage by `20%` per level, capped at 100% (level
/// IV+ is full immunity). Clamped so the result is never negative.
#[must_use]
pub fn damage_taken_multiplier(effects: &[ActiveEffect]) -> f64 {
    let mut reduction = 0.0f64;
    for effect in effects {
        if effect.kind() == Some(EffectKind::Resistance) {
            reduction += 0.2 * f64::from(effect.level());
        }
    }
    (1.0 - reduction).clamp(0.0, 1.0)
}

/// Flat melee damage bonus from Strength / Weakness.
///
/// Vanilla adds `3 × level` for Strength and subtracts `4 × level` for
/// Weakness (community-documented attribute modifiers, same honesty tier as
/// [`movement_speed_multiplier`]). The sum is **not** clamped here; callers
/// fold it into a total that floors at zero so a Weakness stack cannot deal
/// negative damage.
#[must_use]
pub fn attack_damage_bonus(effects: &[ActiveEffect]) -> f32 {
    let mut bonus = 0.0f32;
    for effect in effects {
        match effect.kind() {
            Some(EffectKind::Strength) => {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "amplifier is a small game integer; level is already i32"
                )]
                {
                    bonus += 3.0 * effect.level() as f32;
                }
            }
            Some(EffectKind::Weakness) => {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "amplifier is a small game integer; level is already i32"
                )]
                {
                    bonus -= 4.0 * effect.level() as f32;
                }
            }
            _ => {}
        }
    }
    bonus
}

/// Damage an effect deals this tick, if any.
///
/// Vanilla's poison and wither deal one point on a cadence that depends on the
/// amplifier; the *cadence table* is not verified here, so this returns damage
/// every `interval` ticks where the interval is the documented vanilla pattern
/// (25 ticks at amplifier 0, shortening as the amplifier rises, floor 1). Poison
/// cannot kill: the caller must clamp the result so health stops at 1.
#[must_use]
pub fn damage_over_time(effect: &ActiveEffect, tick: u64) -> f32 {
    let Some(kind) = effect.kind() else {
        return 0.0;
    };
    if !kind.is_harmful_over_time() {
        return 0.0;
    }
    // Vanilla: interval = 25 >> amplifier, minimum 1 tick.
    let shift = u32::try_from(effect.amplifier).unwrap_or(0);
    let interval = (25u64 >> shift.min(4)).max(1);
    if tick.is_multiple_of(interval) {
        1.0
    } else {
        0.0
    }
}

/// Whether poison from this effect is allowed to reduce health below 1.
///
/// Poison stops at half a heart; wither does not.
#[must_use]
pub fn can_kill(effect: &ActiveEffect) -> bool {
    effect.kind() == Some(EffectKind::Wither)
}

#[cfg(test)]
// `damage_over_time` returns a literal `0.0` or `1.0` and the multiplier helpers
// return exact literals at their clamp boundaries, so these comparisons are exact
// by construction rather than approximate.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        ActiveEffect, EffectKind, attack_damage_bonus, can_kill, damage_over_time,
        damage_taken_multiplier, effect_id, movement_speed_multiplier, wire_id_of,
    };

    #[test]
    fn wire_ids_match_the_vanilla_registry() {
        // The 26.1 client reads effect ids through `MobEffect.STREAM_CODEC`
        // = `ByteBufCodecs.holderRegistry` (`$29`, jar-verified: raw
        // registry id as a bare `VarInt`, no offset — pumpkin's generated
        // raw ids: speed raw 0 … wither raw 19). The stored table is legacy
        // 1-based, so the wire value is one less. Round 1 read the wrong
        // codec (`holder` / `$30`, `raw_id + 1`) and sent the stored value;
        // a live client showed speed as Slowness, the exact +1 shift this
        // pin guards against.
        let expected = [
            (EffectKind::Speed, 0),
            (EffectKind::Slowness, 1),
            (EffectKind::Strength, 4),
            (EffectKind::Weakness, 17),
            (EffectKind::Resistance, 10),
            (EffectKind::Poison, 18),
            (EffectKind::Wither, 19),
            (EffectKind::Regeneration, 9),
        ];
        for (kind, wire) in expected {
            assert_eq!(kind.wire_id(), wire, "{kind:?}");
            assert_eq!(wire_id_of(kind.id()), Some(wire));
        }
        assert_eq!(wire_id_of(effect_id::JUMP_BOOST), None);
        assert_eq!(wire_id_of(-1), None);
    }

    #[test]
    fn ids_round_trip_for_modelled_effects() {
        for kind in [
            EffectKind::Speed,
            EffectKind::Slowness,
            EffectKind::Strength,
            EffectKind::Weakness,
            EffectKind::Resistance,
            EffectKind::Poison,
            EffectKind::Wither,
            EffectKind::Regeneration,
        ] {
            assert_eq!(EffectKind::from_id(kind.id()), Some(kind));
            assert!(!kind.name().is_empty());
        }
        // An unmodelled id is stored but contributes nothing.
        assert_eq!(EffectKind::from_id(effect_id::JUMP_BOOST), None);
        assert_eq!(EffectKind::from_id(-1), None);
        assert_eq!(EffectKind::from_id(9999), None);
    }

    #[test]
    fn amplifiers_and_durations_are_clamped_on_construction() {
        let effect = ActiveEffect::new(effect_id::SPEED, -5, -10);
        assert_eq!(effect.amplifier, 0);
        assert_eq!(effect.duration, 0);
        assert!(effect.is_expired());
        assert_eq!(effect.level(), 1);
        assert_eq!(
            ActiveEffect::new(effect_id::SPEED, i32::MAX, 0).level(),
            i32::MAX,
            "level saturates rather than wrapping"
        );
    }

    #[test]
    fn speed_multiplier_follows_the_vanilla_shape() {
        let plain = movement_speed_multiplier(&[]);
        assert!((plain - 1.0).abs() < 1e-9);

        let speed_two = movement_speed_multiplier(&[ActiveEffect::new(effect_id::SPEED, 1, 100)]);
        assert!((speed_two - 1.4).abs() < 1e-9, "{speed_two}");

        let slow_two = movement_speed_multiplier(&[ActiveEffect::new(effect_id::SLOWNESS, 1, 100)]);
        assert!((slow_two - 0.7).abs() < 1e-9, "{slow_two}");

        // Absurd amplifiers cannot produce a negative or unbounded speed.
        let absurd =
            movement_speed_multiplier(&[ActiveEffect::new(effect_id::SLOWNESS, 1000, 100)]);
        assert!((0.0..=4.0).contains(&absurd), "{absurd}");
        let absurd_fast =
            movement_speed_multiplier(&[ActiveEffect::new(effect_id::SPEED, 1000, 100)]);
        assert!(absurd_fast <= 4.0, "{absurd_fast}");
    }

    #[test]
    fn damage_multiplier_caps_at_full_resistance() {
        assert!((damage_taken_multiplier(&[]) - 1.0).abs() < 1e-9);
        let resist_one =
            damage_taken_multiplier(&[ActiveEffect::new(effect_id::RESISTANCE, 0, 100)]);
        assert!((resist_one - 0.8).abs() < 1e-9, "{resist_one}");
        let resist_four =
            damage_taken_multiplier(&[ActiveEffect::new(effect_id::RESISTANCE, 3, 100)]);
        assert!((resist_four - 0.2).abs() < 1e-9, "{resist_four}");
        let resist_ten =
            damage_taken_multiplier(&[ActiveEffect::new(effect_id::RESISTANCE, 9, 100)]);
        assert!(
            resist_ten.abs() < 1e-9,
            "capped at full immunity: {resist_ten}"
        );
    }

    #[test]
    fn strength_and_weakness_shift_melee_damage() {
        assert_eq!(attack_damage_bonus(&[]), 0.0);
        let strength_one = attack_damage_bonus(&[ActiveEffect::new(effect_id::STRENGTH, 0, 100)]);
        assert_eq!(strength_one, 3.0, "Strength I is +3");
        let strength_two = attack_damage_bonus(&[ActiveEffect::new(effect_id::STRENGTH, 1, 100)]);
        assert_eq!(strength_two, 6.0, "Strength II is +6");
        let weakness_one = attack_damage_bonus(&[ActiveEffect::new(effect_id::WEAKNESS, 0, 100)]);
        assert_eq!(weakness_one, -4.0, "Weakness I is −4");
        let both = attack_damage_bonus(&[
            ActiveEffect::new(effect_id::STRENGTH, 0, 100),
            ActiveEffect::new(effect_id::WEAKNESS, 0, 100),
        ]);
        assert_eq!(both, -1.0, "stacks add: +3 − 4");
    }

    #[test]
    fn damage_over_time_fires_on_its_cadence() {
        let poison = ActiveEffect::new(effect_id::POISON, 0, 100);
        assert_eq!(damage_over_time(&poison, 0), 1.0);
        assert_eq!(damage_over_time(&poison, 1), 0.0);
        assert_eq!(damage_over_time(&poison, 25), 1.0);
        // A higher amplifier shortens the interval.
        let strong = ActiveEffect::new(effect_id::POISON, 2, 100);
        assert_eq!(damage_over_time(&strong, 6), 1.0, "25 >> 2 = 6");
        // Non-damaging and unmodelled effects never deal damage.
        assert_eq!(
            damage_over_time(&ActiveEffect::new(effect_id::SPEED, 0, 100), 0),
            0.0
        );
        assert_eq!(
            damage_over_time(&ActiveEffect::new(effect_id::JUMP_BOOST, 0, 100), 0),
            0.0
        );
        assert_eq!(damage_over_time(&ActiveEffect::new(9999, 0, 100), 0), 0.0);
    }

    #[test]
    fn only_wither_may_kill() {
        assert!(can_kill(&ActiveEffect::new(effect_id::WITHER, 0, 10)));
        assert!(!can_kill(&ActiveEffect::new(effect_id::POISON, 0, 10)));
        assert!(!can_kill(&ActiveEffect::new(effect_id::SPEED, 0, 10)));
        assert!(!can_kill(&ActiveEffect::new(9999, 0, 10)));
    }
}
