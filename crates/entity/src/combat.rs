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
//! fire/drowning/void/magic sources — the item says so instead of guessing
//! (AGENTS.md section 3.3). P16-04 added the Arrow/Explosion sources with
//! the bow and the creeper fuse as production callers.

/// What dealt the damage. Only sources with a production caller exist here:
/// fire/drowning/void/magic sources have no callers in the tree at all.
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
    /// Poison effect ticks (`poison` tag: armour applies, no knockback).
    Poison,
    /// Wither effect ticks (`wither` tag: bypasses armour, no knockback).
    Wither,
    /// A skeleton's arrow (`arrow` tag: armour applies, knockback applies).
    Arrow,
    /// A creeper explosion (`explosion` tag: armour applies, but the hurt
    /// path carries no knockback — vanilla flings separately).
    Explosion,
}

impl DamageSource {
    /// Whether armour (and toughness) reduce this damage. From the vanilla
    /// `bypasses_armor` tag: fall, starvation and their kin bypass; melee and
    /// projectiles do not.
    #[must_use]
    pub const fn bypasses_armor(self) -> bool {
        match self {
            Self::PlayerAttack | Self::MobAttack | Self::Poison | Self::Arrow | Self::Explosion => {
                false
            }
            Self::Fall | Self::Starvation | Self::Wither => true,
        }
    }

    /// Whether the hit shoves the victim. Melee and arrows carry vanilla's
    /// base knockback; falls, starvation, effect ticks and explosions have no
    /// source direction (explosions fling separately) to shove from.
    #[must_use]
    pub const fn applies_knockback(self) -> bool {
        match self {
            Self::PlayerAttack | Self::MobAttack | Self::Arrow => true,
            Self::Fall | Self::Starvation | Self::Poison | Self::Wither | Self::Explosion => false,
        }
    }
}

/// Base melee knockback strength, from vanilla `LivingEntity.hurt` (pumpkin
/// `living.rs:2961` applies `knockback_after_resistance(0.4, resistance)`).
/// Enchantment and sprint bonuses do not exist in this build yet; a plain hit
/// adds nothing on top of the base (pumpkin `player.rs:1339-1343`).
pub const BASE_MELEE_KNOCKBACK: f64 = 0.4;

/// Per-tick decay of the unresolved knockback impulse.
///
/// Approximation with a visible target: 0.4 strength travels about a block
/// total (0.4 + 0.24 + …), a shove rather than a launch. Vanilla decays
/// through block friction (~0.91), which would carry over four blocks;
/// the runbook's "~0.4 recoil" reads closer to one, so this stays until a
/// capture measures a real shove.
/// Per-tick decay of the unresolved knockback impulse.
///
/// Approximation with a visible target: 0.4 strength travels about a block
/// total (0.4 + 0.24 + …), a shove rather than a launch. Vanilla decays
/// through block friction (~0.91), which would carry over four blocks;
/// the runbook's "~0.4 recoil" reads closer to one, so this stays until a
/// capture measures a real shove.
pub const KNOCKBACK_DECAY: f64 = 0.6;

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
/// figure [`FIST_DAMAGE`] carries, now as the floor under the bonus).
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
#[allow(clippy::match_same_arms)] // A data table, not logic: distinct items
// share values (wooden and golden swords are both 3.0), and grouping arms by
// value would hide which vanilla fact each row is.
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
#[allow(clippy::match_same_arms)] // Same rationale as above: per-piece facts.
pub fn armor_of(item_name: &str) -> Option<ArmorPiece> {
    let (points, toughness, knockback_resistance) = match item_name {
        "minecraft:leather_helmet" => (1.0, 0.0, 0.0),
        "minecraft:leather_chestplate" => (3.0, 0.0, 0.0),
        "minecraft:leather_leggings" => (2.0, 0.0, 0.0),
        "minecraft:leather_boots" => (1.0, 0.0, 0.0),
        "minecraft:golden_helmet"
        | "minecraft:chainmail_helmet"
        | "minecraft:copper_helmet"
        | "minecraft:iron_helmet"
        | "minecraft:turtle_helmet" => (2.0, 0.0, 0.0),
        "minecraft:golden_chestplate" | "minecraft:chainmail_chestplate" => (5.0, 0.0, 0.0),
        "minecraft:copper_chestplate" => (4.0, 0.0, 0.0),
        "minecraft:iron_chestplate" => (6.0, 0.0, 0.0),
        "minecraft:golden_leggings" => (3.0, 0.0, 0.0),
        "minecraft:chainmail_leggings" => (4.0, 0.0, 0.0),
        "minecraft:copper_leggings" => (3.0, 0.0, 0.0),
        "minecraft:iron_leggings" => (5.0, 0.0, 0.0),
        "minecraft:golden_boots" | "minecraft:chainmail_boots" | "minecraft:copper_boots" => {
            (1.0, 0.0, 0.0)
        }
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

use crate::inventory::{ARMOR_SLOTS, ARMOR_START, PlayerInventory};
use mc_registry::ItemRegistry;
use mc_world::Vec3;

/// Who dealt a melee hit: knockback pushes away from `pos`, falling back to
/// the `yaw` facing when attacker and victim share x/z (deterministic where
/// vanilla nudges randomly — see `Entity::apply_knockback`).
#[derive(Debug, Clone, Copy)]
pub struct Attacker {
    /// Attacker eye/feet-agnostic position (only x/z steer the shove).
    pub pos: Vec3,
    /// Attacker yaw in degrees, vanilla facing convention.
    pub yaw: f32,
}

/// Total melee damage for a swing with what `inv` holds: [`FIST_DAMAGE`] plus
/// the held weapon's bonus. An empty hand, a non-weapon, or an id the
/// registry does not know all fall back to the fist — the safe default for
/// hostile or corrupt inventory states, never a refusal mid-swing.
#[must_use]
pub fn held_damage(inv: &PlayerInventory, items: &ItemRegistry) -> f32 {
    let Some(id) = inv.selected_item().item_id() else {
        return FIST_DAMAGE;
    };
    let Ok(name) = items.name(id) else {
        return FIST_DAMAGE;
    };
    FIST_DAMAGE + melee_damage_bonus(name)
}

/// [`held_damage`] plus the Strength/Weakness flat bonus from `effects`,
/// floored at zero so a Weakness stack cannot deal negative damage.
#[must_use]
pub fn held_damage_with_effects(
    inv: &PlayerInventory,
    items: &ItemRegistry,
    effects: &[crate::effect::ActiveEffect],
) -> f32 {
    (held_damage(inv, items) + crate::effect::attack_damage_bonus(effects)).max(0.0)
}

/// Summed combat stats of what `inv` wears: the four armour slots starting
/// at [`ARMOR_START`]. Unknown ids contribute nothing (same fallback rule as
/// [`held_damage`]).
#[must_use]
pub fn worn_stats(inv: &PlayerInventory, items: &ItemRegistry) -> CombatStats {
    let mut stats = CombatStats::ZERO;
    for slot in ARMOR_START..ARMOR_START + ARMOR_SLOTS {
        let Some(id) = inv.slot(slot).item_id() else {
            continue;
        };
        let Ok(name) = items.name(id) else {
            continue;
        };
        if let Some(piece) = armor_of(name) {
            stats.armor += piece.points;
            stats.toughness += piece.toughness;
            stats.knockback_resistance += piece.knockback_resistance;
        }
    }
    stats
}

#[cfg(test)]
// Table values must be bit-identical to the modifiers they mirror, so exact
// float equality here is the assertion doing its job, not sloppiness.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        BASE_MELEE_KNOCKBACK, CombatStats, DamageSource, armor_absorb, armor_of,
        attack_range_bonus, held_damage, held_damage_with_effects, melee_damage_bonus, worn_stats,
    };

    #[test]
    fn damage_sources_classify_armor_and_knockback() {
        assert!(!DamageSource::PlayerAttack.bypasses_armor());
        assert!(!DamageSource::MobAttack.bypasses_armor());
        assert!(!DamageSource::Poison.bypasses_armor());
        assert!(!DamageSource::Arrow.bypasses_armor());
        assert!(!DamageSource::Explosion.bypasses_armor());
        assert!(DamageSource::Fall.bypasses_armor());
        assert!(DamageSource::Starvation.bypasses_armor());
        assert!(DamageSource::Wither.bypasses_armor());
        assert!(DamageSource::PlayerAttack.applies_knockback());
        assert!(DamageSource::MobAttack.applies_knockback());
        assert!(DamageSource::Arrow.applies_knockback());
        assert!(!DamageSource::Fall.applies_knockback());
        assert!(!DamageSource::Starvation.applies_knockback());
        assert!(!DamageSource::Poison.applies_knockback());
        assert!(!DamageSource::Wither.applies_knockback());
        assert!(!DamageSource::Explosion.applies_knockback());
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
        assert_eq!(
            (
                diamond.points,
                diamond.toughness,
                diamond.knockback_resistance
            ),
            (8.0, 2.0, 0.0)
        );
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

    /// The real item table: combat numbers must resolve against the names the
    /// game actually holds (same pattern as the inventory tests).
    fn registry() -> mc_registry::ItemRegistry {
        mc_registry::ItemRegistry::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/test-support/fixtures/registry/items.tsv"),
        )
        .expect("items fixture loads")
    }

    fn stocked_inventory() -> crate::inventory::PlayerInventory {
        crate::inventory::inventory_for_registry(&registry()).expect("table resolves")
    }

    #[test]
    fn held_damage_adds_the_weapon_bonus_over_the_fist() {
        use crate::stack::ItemStack;
        let items = registry();
        let mut inv = stocked_inventory();
        assert_eq!(held_damage(&inv, &items), 1.0);
        let sword = items.id("minecraft:diamond_sword").expect("sword");
        inv.set_slot(0, ItemStack::new(sword, 1).expect("sword"))
            .expect("set");
        assert_eq!(held_damage(&inv, &items), 7.0);
        let stick = items.id("minecraft:stick").expect("stick");
        inv.set_slot(0, ItemStack::new(stick, 1).expect("stick"))
            .expect("set");
        assert_eq!(held_damage(&inv, &items), 1.0);
    }

    #[test]
    fn strength_and_weakness_modifiers_fight_the_fist_and_the_sword() {
        use crate::stack::ItemStack;
        let items = registry();
        let inv = stocked_inventory();
        let strength = [crate::effect::ActiveEffect::new(
            crate::effect::effect_id::STRENGTH,
            0,
            100,
        )];
        let weakness = [crate::effect::ActiveEffect::new(
            crate::effect::effect_id::WEAKNESS,
            0,
            100,
        )];
        assert_eq!(held_damage_with_effects(&inv, &items, &strength), 4.0);
        assert_eq!(held_damage_with_effects(&inv, &items, &weakness), 0.0);
        let mut armed = stocked_inventory();
        let sword = items.id("minecraft:diamond_sword").expect("sword");
        armed
            .set_slot(0, ItemStack::new(sword, 1).expect("sword"))
            .expect("set");
        assert_eq!(held_damage_with_effects(&armed, &items, &strength), 10.0);
        assert_eq!(held_damage_with_effects(&armed, &items, &weakness), 3.0);
    }

    #[test]
    fn worn_stats_sum_only_what_the_armour_slots_hold() {
        use crate::inventory::{ARMOR_START, PlayerInventory};
        use crate::stack::ItemStack;
        let items = registry();
        let mut inv: PlayerInventory = stocked_inventory();
        assert_eq!(worn_stats(&inv, &items), CombatStats::ZERO);
        let plate = items.id("minecraft:iron_chestplate").expect("plate");
        inv.set_slot(ARMOR_START + 1, ItemStack::new(plate, 1).expect("plate"))
            .expect("set");
        let stats = worn_stats(&inv, &items);
        assert_eq!(
            (stats.armor, stats.toughness, stats.knockback_resistance),
            (6.0, 0.0, 0.0)
        );
        // A sword in an armour slot is not armour.
        let sword = items.id("minecraft:diamond_sword").expect("sword");
        inv.set_slot(ARMOR_START, ItemStack::new(sword, 1).expect("sword"))
            .expect("set");
        assert_eq!(worn_stats(&inv, &items).armor, 6.0);
    }
}
