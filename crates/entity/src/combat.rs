//! Melee combat numbers: damage sources, armour, knockback, reach (P16-01).
//!
//! The values below are vanilla data, not tuning knobs. Weapon bonuses,
//! armour points/toughness and the knockback base come from the 26.x item and
//! combat model as mirrored by pumpkin's generated data
//! (`pumpkin-data/src/generated/item.rs` attribute modifiers,
//! `pumpkin/src/entity/combat.rs` mirroring `CombatRules`, and
//! `living.rs:2961` for the hurt-path knockback); the damage-type armour
//! rules come from the vanilla damage-type tags (`bypasses_armor` membership
//! as listed in pumpkin-data's generated damage-type tag table). Where this
//! build knowingly stops — enchantments, absorption, per-item attack reach,
//! projectiles, fire/void sources — the item says so instead of guessing
//! (AGENTS.md section 3.3).

/// What dealt the damage. Only sources with a production caller exist here:
/// an arrow or explosion variant arrives with P16-04's bow/fuse wiring, and
/// fire/void/magic have no sources in the tree at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageSource {
    /// A player's melee swing.
    PlayerAttack,
    /// A mob's melee hit.
    MobAttack,
    /// A fall (`is_fall` tag family).
    Fall,
    /// Hunger (`starve` tag).
    Starvation,
}

impl DamageSource {
    /// Whether armour (and toughness) reduce this damage. From the vanilla
    /// `bypasses_armor` tag: fall, starvation and their kin bypass; melee and
    /// projectiles do not.
    #[must_use]
    pub const fn bypasses_armor(self) -> bool {
        match self {
            Self::PlayerAttack | Self::MobAttack => false,
            Self::Fall | Self::Starvation => true,
        }
    }

    /// Whether the hit shoves the victim. Melee carries vanilla's base
    /// knockback; falls and starvation have no source direction to shove from.
    #[must_use]
    pub const fn applies_knockback(self) -> bool {
        match self {
            Self::PlayerAttack | Self::MobAttack => true,
            Self::Fall | Self::Starvation => false,
        }
    }
}

/// Base melee knockback strength, from vanilla `LivingEntity.hurt` (pumpkin
/// `living.rs:2961` applies `knockback_after_resistance(0.4, resistance)`).
/// Enchantment and sprint bonuses do not exist in this build yet; a plain hit
/// adds nothing on top of the base (pumpkin `player.rs:1339-1343`).
pub const BASE_MELEE_KNOCKBACK: f64 = 0.4;

/// Vanilla `CombatRules.getDamageAfterAbsorb` with breach level 0 (breach is
/// an enchantment effect, deferred with the rest of enchantment combat math).
///
/// `toughness` here is the *summed* armour-toughness attribute; the base 2.0
/// is vanilla's `BASE_ARMOR_TOUGHNESS`, the 0.2 floor is `MIN_ARMOR_RATIO`,
/// the 20 cap is `MAX_ARMOR`, and damage is split 25 ways
/// (`ARMOR_PROTECTION_DIVIDER`).
#[must_use]
pub fn armor_absorb(damage: f32, armor: f32, toughness: f32) -> f32 {
    let rated_toughness = 2.0 + toughness / 4.0;
    let real_armor = (armor - damage / rated_toughness).clamp(armor * 0.2, 20.0);
    damage * (1.0 - real_armor / 25.0)
}

/// The player's base attack damage with an empty hand (vanilla 1.0; the same
/// figure `FIST_ATTACK_DAMAGE` carries, now as the floor under the bonus).
pub const FIST_DAMAGE: f32 = 1.0;

/// Melee bonus of a held item, added to [`FIST_DAMAGE`].
///
/// The `base_attack_damage` attribute modifier each weapon carries for the
/// main hand (pumpkin-data generated `item.rs`, vanilla-stable across a
/// decade: wooden sword 3, diamond sword 6, netherite axe 9, trident 8, mace
/// 5, hoes 0). Anything without a modifier — sticks, blocks, air — adds
/// nothing, which is the fist. Unknown names also add nothing: an inventory
/// holding an unregistered id is hostile or corrupt input, and the safe
/// fallback is the fist, not a refusal mid-swing.
#[must_use]
pub fn melee_damage_bonus(item_name: &str) -> f32 {
    match item_name {
        "minecraft:wooden_sword" => 3.0,
        "minecraft:stone_sword" | "minecraft:copper_sword" => 4.0,
        "minecraft:iron_sword" => 5.0,
        "minecraft:diamond_sword" => 6.0,
        "minecraft:golden_sword" => 3.0,
        "minecraft:netherite_sword" => 7.0,
        "minecraft:wooden_axe" | "minecraft:golden_axe" => 6.0,
        "minecraft:stone_axe"
        | "minecraft:copper_axe"
        | "minecraft:iron_axe"
        | "minecraft:diamond_axe" => 8.0,
        "minecraft:netherite_axe" => 9.0,
        "minecraft:wooden_pickaxe" | "minecraft:golden_pickaxe" => 1.0,
        "minecraft:stone_pickaxe" | "minecraft:copper_pickaxe" => 2.0,
        "minecraft:iron_pickaxe" => 3.0,
        "minecraft:diamond_pickaxe" => 4.0,
        "minecraft:netherite_pickaxe" => 5.0,
        "minecraft:wooden_shovel" | "minecraft:golden_shovel" => 1.5,
        "minecraft:stone_shovel" | "minecraft:copper_shovel" => 2.5,
        "minecraft:iron_shovel" => 3.5,
        "minecraft:diamond_shovel" => 4.5,
        "minecraft:netherite_shovel" => 5.5,
        "minecraft:wooden_hoe"
        | "minecraft:stone_hoe"
        | "minecraft:copper_hoe"
        | "minecraft:iron_hoe"
        | "minecraft:golden_hoe"
        | "minecraft:diamond_hoe"
        | "minecraft:netherite_hoe" => 0.0,
        "minecraft:wooden_spear" | "minecraft:golden_spear" => 0.0,
        "minecraft:stone_spear" | "minecraft:copper_spear" => 1.0,
        "minecraft:iron_spear" => 2.0,
        "minecraft:diamond_spear" => 3.0,
        "minecraft:netherite_spear" => 4.0,
        "minecraft:trident" => 8.0,
        "minecraft:mace" => 5.0,
        _ => 0.0,
    }
}

/// One worn piece's combat contribution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmorPiece {
    /// Armour points.
    pub points: f32,
    /// Armour toughness.
    pub toughness: f32,
    /// Knockback-resistance attribute (netherite 0.1 per piece).
    pub knockback_resistance: f32,
}

/// Armour values of a wearable item, by registry name.
///
/// Per-piece `armor.*` / toughness / knockback-resistance attribute modifiers
/// (pumpkin-data generated `item.rs`): leather 1/3/2/1, gold 2/5/3/1, chain
/// 2/5/4/1, copper 2/4/3/1, iron 2/6/5/2, turtle helmet 2, diamond 3/8/6/3
/// with toughness 2, netherite 3/8/6/3 with toughness 3 and 0.1 resistance
/// per piece. Horse/wolf/nautilus body armour is excluded: players cannot
/// wear it, and no mob equipment system exists to read it. Unknown names
/// yield `None` (no phantom armour for hostile input).
#[must_use]
pub fn armor_of(item_name: &str) -> Option<ArmorPiece> {
    let (points, toughness, knockback_resistance) = match item_name {
        "minecraft:leather_helmet" => (1.0, 0.0, 0.0),
        "minecraft:leather_chestplate" => (3.0, 0.0, 0.0),
        "minecraft:leather_leggings" => (2.0, 0.0, 0.0),
        "minecraft:leather_boots" => (1.0, 0.0, 0.0),
        "minecraft:golden_helmet" | "minecraft:chainmail_helmet" | "minecraft:copper_helmet"
        | "minecraft:iron_helmet" | "minecraft:turtle_helmet" => (2.0, 0.0, 0.0),
        "minecraft:golden_chestplate" | "minecraft:chainmail_chestplate" => (5.0, 0.0, 0.0),
        "minecraft:copper_chestplate" => (4.0, 0.0, 0.0),
        "minecraft:iron_chestplate" => (6.0, 0.0, 0.0),
        "minecraft:golden_leggings" => (3.0, 0.0, 0.0),
        "minecraft:chainmail_leggings" => (4.0, 0.0, 0.0),
        "minecraft:copper_leggings" => (3.0, 0.0, 0.0),
        "minecraft:iron_leggings" => (5.0, 0.0, 0.0),
        "minecraft:golden_boots"
        | "minecraft:chainmail_boots"
        | "minecraft:copper_boots"
        | "minecraft:leather_boots" => (1.0, 0.0, 0.0),
        "minecraft:iron_boots" => (2.0, 0.0, 0.0),
        "minecraft:diamond_helmet" | "minecraft:diamond_boots" => (3.0, 2.0, 0.0),
        "minecraft:diamond_chestplate" => (8.0, 2.0, 0.0),
        "minecraft:diamond_leggings" => (6.0, 2.0, 0.0),
        "minecraft:netherite_helmet" | "minecraft:netherite_boots" => (3.0, 3.0, 0.1),
        "minecraft:netherite_chestplate" => (8.0, 3.0, 0.1),
        "minecraft:netherite_leggings" => (6.0, 3.0, 0.1),
        _ => return None,
    };
    Some(ArmorPiece {
        points,
        toughness,
        knockback_resistance,
    })
}

/// A defender's summed combat stats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatStats {
    /// Summed armour points.
    pub armor: f32,
    /// Summed toughness.
    pub toughness: f32,
    /// Summed knockback resistance.
    pub knockback_resistance: f32,
}

impl CombatStats {
    /// No armour, the mob (and bare-player) case.
    pub const ZERO: Self = Self {
        armor: 0.0,
        toughness: 0.0,
        knockback_resistance: 0.0,
    };
}

/// Extra entity reach granted by the held item's `attack_range` component.
///
/// The component schema is real 26.x data (pumpkin `AttackRangeImpl`:
/// `min_reach` 0.0, `max_reach` 3.0, `min_creative_reach` 0.0,
/// `max_creative_reach` 5.0, `hitbox_margin` 0.3, `mob_factor` 1.0) — but no
/// stack in this build carries components, and no reachable reference states
/// how vanilla combines the six fields with the eye-to-box distance, so every
/// item reports the default zero and the gate below stays exactly today's.
/// Per-item values and the combination formula land with components (P18);
/// creative-mode attack reach is unmodelled for the same reason.
#[must_use]
pub fn attack_range_bonus(_item_name: &str) -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_MELEE_KNOCKBACK, CombatStats, DamageSource, armor_absorb, armor_of, attack_range_bonus,
        melee_damage_bonus,
    };

    #[test]
    fn damage_sources_classify_armor_and_knockback() {
        assert!(!DamageSource::PlayerAttack.bypasses_armor());
        assert!(!DamageSource::MobAttack.bypasses_armor());
        assert!(DamageSource::Fall.bypasses_armor());
        assert!(DamageSource::Starvation.bypasses_armor());
        assert!(DamageSource::PlayerAttack.applies_knockback());
        assert!(DamageSource::MobAttack.applies_knockback());
        assert!(!DamageSource::Fall.applies_knockback());
        assert!(!DamageSource::Starvation.applies_knockback());
        assert_eq!(BASE_MELEE_KNOCKBACK, 0.4);
    }

    #[test]
    fn armor_absorb_matches_the_combat_rules_shape() {
        // No armour: untouched, exactly.
        assert_eq!(armor_absorb(7.0, 0.0, 0.0), 7.0);
        // Full diamond (20 pts, toughness 8) against 7: ~1.89.
        let diamond = armor_absorb(7.0, 20.0, 8.0);
        assert!(
            (diamond - 1.89).abs() < 1e-3,
            "full diamond vs 7 should leave ~1.89, got {diamond}"
        );
        // Leather set (7 pts, no toughness) against 5: 4.1.
        let leather = armor_absorb(5.0, 7.0, 0.0);
        assert!(
            (leather - 4.1).abs() < 1e-3,
            "leather vs 5 should leave ~4.1, got {leather}"
        );
        // Zero incoming stays zero; armour never amplifies.
        assert_eq!(armor_absorb(0.0, 20.0, 8.0), 0.0);
        let _ = CombatStats::ZERO;
    }

    #[test]
    fn weapon_bonuses_match_the_attribute_modifiers() {
        assert_eq!(melee_damage_bonus("minecraft:diamond_sword"), 6.0);
        assert_eq!(melee_damage_bonus("minecraft:netherite_axe"), 9.0);
        assert_eq!(melee_damage_bonus("minecraft:iron_shovel"), 3.5);
        assert_eq!(melee_damage_bonus("minecraft:trident"), 8.0);
        assert_eq!(melee_damage_bonus("minecraft:mace"), 5.0);
        assert_eq!(melee_damage_bonus("minecraft:wooden_hoe"), 0.0);
        assert_eq!(melee_damage_bonus("minecraft:air"), 0.0);
        assert_eq!(melee_damage_bonus("minecraft:stone"), 0.0);
        assert_eq!(melee_damage_bonus("nope:not_an_item"), 0.0);
    }

    #[test]
    fn armor_values_match_the_attribute_modifiers() {
        let diamond = armor_of("minecraft:diamond_chestplate").expect("diamond plate");
        assert_eq!((diamond.points, diamond.toughness, diamond.knockback_resistance), (8.0, 2.0, 0.0));
        let netherite = armor_of("minecraft:netherite_boots").expect("netherite boots");
        assert_eq!(
            (
                netherite.points,
                netherite.toughness,
                netherite.knockback_resistance
            ),
            (3.0, 3.0, 0.1)
        );
        assert_eq!(
            armor_of("minecraft:iron_helmet").expect("iron helm").points,
            2.0
        );
        assert!(armor_of("minecraft:diamond_sword").is_none());
        assert!(armor_of("nope:not_an_item").is_none());
    }

    #[test]
    fn attack_range_hook_preserves_todays_gate() {
        // Every item reports the default zero until components land: the
        // reach gate below is byte-for-byte today's behavior.
        for name in [
            "minecraft:air",
            "minecraft:diamond_sword",
            "minecraft:stick",
            "minecraft:netherite_spear",
        ] {
            assert_eq!(attack_range_bonus(name), 0.0);
        }
    }
}
