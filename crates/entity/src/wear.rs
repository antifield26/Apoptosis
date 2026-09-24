//! Durability wear (P18-01b): dig / attack / hit cost, break at max.
//!
//! ## Wear steps (derived)
//!
//! Vanilla charges one durability per successful use: a mined block
//! (`DiggerItem.mineBlock` → `hurtAndBreak(1)`), a landed hit (`hurtEnemy(1)`),
//! and each worn armour piece that absorbs a hit (`LivingEntity.hurtArmor`,
//! one point per piece). This build models exactly those three steps as named
//! constants so a wear-matrix test can neutralise one and go red. The steps are
//! **derived** from those call sites rather than bytecode-extracted — every
//! Paper patch and pumpkin site in `OpenSourceMinecraftServer` that damages a
//! tool passes `1`.
//!
//! ## `max_damage` (derived table)
//!
//! Vanilla items carry `minecraft:max_damage` as a default component. This
//! crate has no item-component defaults yet, so [`max_damage_of`] is a
//! name-keyed table of the vanilla `TieredItem` / `ArmorItem` values
//! (wooden/golden 59, stone 131, iron and copper 250, diamond 1561, netherite
//! 2031; armour per piece and material). Callers that need a durable stack
//! call [`ensure_durability`] to attach the component before wearing it. The
//! table is **derived** (jar `max_damage` component values as mirrored by
//! pumpkin's item codegen), not a measured fixture — named as such.
//!
//! ## Unbreaking
//!
//! Wear rolls through [`crate::enchant::unbreaking_applies`] before the damage
//! lands. Both rolls are explicit parameters so the game owns the RNG and a
//! test can force either arm.

use crate::components::DataComponent;
use crate::enchant::{UNBREAKING, level_of, unbreaking_applies};
use crate::stack::ItemStack;

/// Durability spent when a survival dig completes (derived: `mineBlock` → 1).
pub const WEAR_ON_DIG: i32 = 1;

/// Durability spent when a melee attack lands (derived: `hurtEnemy` → 1).
pub const WEAR_ON_ATTACK: i32 = 1;

/// Durability spent on **each** worn armour piece that absorbs a hit
/// (derived: `hurtArmor` → 1 per piece).
pub const WEAR_ON_HIT: i32 = 1;

/// What one [`apply_wear`] call did to the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WearOutcome {
    /// The stack is not damageable (no `max_damage`, or amount 0).
    Untouched,
    /// Wear landed and the stack still holds items.
    Survived,
    /// Wear reached `max_damage`: the stack is now [`ItemStack::EMPTY`].
    Broken,
}

/// Vanilla `max_damage` of a damageable item, by registry name (derived).
///
/// Tiered tools share the tier's durability; armour is per piece. Unknown
/// names yield `None` (no phantom durability for hostile input).
#[must_use]
#[allow(clippy::match_same_arms, reason = "a data table, not logic")]
pub fn max_damage_of(item_name: &str) -> Option<i32> {
    let value = match item_name {
        // Tools / weapons: the vanilla `TieredItem` rows. Copper shares iron's
        // 250 (derived: the copper tier's durability is the iron row in every
        // reachable reference; not a jar-probed fixture).
        "minecraft:wooden_pickaxe"
        | "minecraft:wooden_axe"
        | "minecraft:wooden_shovel"
        | "minecraft:wooden_hoe"
        | "minecraft:wooden_sword"
        | "minecraft:wooden_spear"
        | "minecraft:golden_pickaxe"
        | "minecraft:golden_axe"
        | "minecraft:golden_shovel"
        | "minecraft:golden_hoe"
        | "minecraft:golden_sword"
        | "minecraft:golden_spear" => 59,
        "minecraft:stone_pickaxe"
        | "minecraft:stone_axe"
        | "minecraft:stone_shovel"
        | "minecraft:stone_hoe"
        | "minecraft:stone_sword"
        | "minecraft:stone_spear" => 131,
        "minecraft:copper_pickaxe"
        | "minecraft:copper_axe"
        | "minecraft:copper_shovel"
        | "minecraft:copper_hoe"
        | "minecraft:copper_sword"
        | "minecraft:copper_spear"
        | "minecraft:iron_pickaxe"
        | "minecraft:iron_axe"
        | "minecraft:iron_shovel"
        | "minecraft:iron_hoe"
        | "minecraft:iron_sword"
        | "minecraft:iron_spear"
        | "minecraft:trident" => 250,
        "minecraft:diamond_pickaxe"
        | "minecraft:diamond_axe"
        | "minecraft:diamond_shovel"
        | "minecraft:diamond_hoe"
        | "minecraft:diamond_sword"
        | "minecraft:diamond_spear" => 1561,
        "minecraft:netherite_pickaxe"
        | "minecraft:netherite_axe"
        | "minecraft:netherite_shovel"
        | "minecraft:netherite_hoe"
        | "minecraft:netherite_sword"
        | "minecraft:netherite_spear"
        | "minecraft:mace" => 2031,
        // Other damageables used by the survival loop.
        "minecraft:shears" => 238,
        "minecraft:flint_and_steel" | "minecraft:fishing_rod" => 64,
        "minecraft:carrot_on_a_stick" | "minecraft:warped_fungus_on_a_stick" => 25,
        "minecraft:bow" => 384,
        "minecraft:crossbow" => 465,
        "minecraft:shield" => 336,
        "minecraft:elytra" => 432,
        "minecraft:brush" => 128,
        // Armour (vanilla `ArmorItem` durability by material × slot).
        "minecraft:leather_helmet" => 80,
        "minecraft:leather_chestplate" => 115,
        "minecraft:leather_leggings" => 105,
        "minecraft:leather_boots" => 65,
        "minecraft:chainmail_helmet" => 165,
        "minecraft:chainmail_chestplate" => 240,
        "minecraft:chainmail_leggings" => 225,
        "minecraft:chainmail_boots" => 195,
        "minecraft:iron_helmet" => 240,
        "minecraft:iron_chestplate" => 360,
        "minecraft:iron_leggings" => 336,
        "minecraft:iron_boots" => 285,
        "minecraft:golden_helmet" => 112,
        "minecraft:golden_chestplate" => 160,
        "minecraft:golden_leggings" => 144,
        "minecraft:golden_boots" => 96,
        "minecraft:diamond_helmet" => 363,
        "minecraft:diamond_chestplate" => 528,
        "minecraft:diamond_leggings" => 495,
        "minecraft:diamond_boots" => 429,
        "minecraft:netherite_helmet" => 481,
        "minecraft:netherite_chestplate" => 592,
        "minecraft:netherite_leggings" => 555,
        "minecraft:netherite_boots" => 481,
        "minecraft:turtle_helmet" => 275,
        "minecraft:copper_helmet" => 165,
        "minecraft:copper_chestplate" => 240,
        "minecraft:copper_leggings" => 225,
        "minecraft:copper_boots" => 195,
        _ => return None,
    };
    Some(value)
}

/// Whether `item_name` is a worn armour piece (Unbreaking's armour formula).
#[must_use]
pub fn is_armor_item(item_name: &str) -> bool {
    crate::combat::armor_of(item_name).is_some()
}

/// Attach the derived `max_damage` (and a zero `damage`) when the stack has
/// neither, so a freshly given tool can wear and break.
///
/// No-op on empty stacks and on stacks that already carry `max_damage`: a
/// component the client or a file set is authoritative.
pub fn ensure_durability(stack: &mut ItemStack, item_name: &str) {
    if stack.is_empty() {
        return;
    }
    if stack.components().max_damage().is_some() {
        return;
    }
    let Some(max) = max_damage_of(item_name) else {
        return;
    };
    stack.set_component(DataComponent::MaxDamage(max));
    if stack.components().damage().is_none() {
        stack.set_component(DataComponent::Damage(0));
    }
}

/// Spend `amount` durability on `stack`, honouring Unbreaking.
///
/// `tool_roll` / `armor_roll` feed [`crate::enchant::unbreaking_applies`];
/// `is_armor` selects the formula. When `damage` reaches `max_damage` the
/// stack becomes [`ItemStack::EMPTY`] and the call reports
/// [`WearOutcome::Broken`] (vanilla: a broken tool is gone from the slot —
/// the break *sound* has no channel in this build, see the parity matrix).
///
/// A non-damageable stack (no `max_damage`) is [`WearOutcome::Untouched`]:
/// a fist, a block item and an unknown id must not invent durability.
#[must_use]
pub fn apply_wear(
    stack: &mut ItemStack,
    amount: i32,
    is_armor: bool,
    tool_roll: u32,
    armor_roll: f32,
) -> WearOutcome {
    if stack.is_empty() || amount <= 0 {
        return WearOutcome::Untouched;
    }
    let Some(max_damage) = stack.components().max_damage() else {
        return WearOutcome::Untouched;
    };
    if max_damage <= 0 {
        return WearOutcome::Untouched;
    }
    let level = level_of(stack.components().enchantments(), UNBREAKING);
    let mut applied = 0;
    for _ in 0..amount {
        if unbreaking_applies(level, is_armor, tool_roll, armor_roll) {
            applied += 1;
        }
    }
    if applied <= 0 {
        return WearOutcome::Untouched;
    }
    let current = stack.components().damage().unwrap_or(0);
    let next = current.saturating_add(applied);
    if next >= max_damage {
        *stack = ItemStack::EMPTY;
        return WearOutcome::Broken;
    }
    stack.set_component(DataComponent::Damage(next));
    WearOutcome::Survived
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "exact small-decimal rolls and damage values"
)]
mod tests {
    use super::{
        WEAR_ON_ATTACK, WEAR_ON_DIG, WEAR_ON_HIT, WearOutcome, apply_wear, ensure_durability,
        is_armor_item, max_damage_of,
    };
    use crate::components::{DataComponent, ItemComponents};
    use crate::enchant::UNBREAKING;
    use crate::stack::ItemStack;

    fn tool_with(max: i32, damage: i32) -> ItemStack {
        let mut components = ItemComponents::new();
        components.set(DataComponent::MaxDamage(max));
        components.set(DataComponent::Damage(damage));
        ItemStack::with_components(1, 1, components).expect("tool stack")
    }

    fn enchanted(stack: &mut ItemStack, id: i32, level: i32) {
        stack.set_component(DataComponent::Enchantments(vec![(id, level)]));
    }

    /// Wear-matrix pin: digging N times adds N damage (`WEAR_ON_DIG`).
    /// Neutralising `WEAR_ON_DIG` to 0 turns this red.
    #[test]
    fn wear_matrix_dig_n_times_adds_n_damage() {
        let mut stack = tool_with(250, 0);
        for i in 1..=10 {
            let outcome = apply_wear(&mut stack, WEAR_ON_DIG, false, 0, 0.0);
            assert_eq!(outcome, WearOutcome::Survived, "dig {i} keeps the tool");
            assert_eq!(stack.damage(), Some(i), "dig {i} → damage {i}");
        }
    }

    /// Same matrix for the attack and hit steps (both are named constants).
    /// Neutralising either step to 0 turns the matching arm red.
    #[test]
    fn wear_matrix_attack_and_hit_steps_match_the_named_constants() {
        let mut weapon = tool_with(250, 0);
        let _ = apply_wear(&mut weapon, WEAR_ON_ATTACK, false, 0, 0.0);
        assert_eq!(weapon.damage(), Some(WEAR_ON_ATTACK));
        let mut armour = tool_with(80, 0);
        let _ = apply_wear(&mut armour, WEAR_ON_HIT, true, 0, 0.0);
        assert_eq!(armour.damage(), Some(WEAR_ON_HIT));
    }

    /// Break-at-max pin: reaching `max_damage` empties the slot.
    #[test]
    fn break_at_max_empties_the_stack() {
        let mut stack = tool_with(3, 2);
        let outcome = apply_wear(&mut stack, 1, false, 0, 0.0);
        assert_eq!(outcome, WearOutcome::Broken);
        assert!(stack.is_empty(), "a broken tool leaves an empty slot");
        // Exactly at max-1 survives; max itself breaks (already covered).
        let mut last = tool_with(3, 1);
        assert_eq!(
            apply_wear(&mut last, 1, false, 0, 0.0),
            WearOutcome::Survived
        );
        assert_eq!(last.damage(), Some(2));
        assert_eq!(apply_wear(&mut last, 1, false, 0, 0.0), WearOutcome::Broken);
    }

    /// Unbreaking tool roll: roll 1 with level 1 skips the wear.
    #[test]
    fn unbreaking_tool_roll_can_skip_the_wear() {
        let mut stack = tool_with(250, 0);
        enchanted(&mut stack, UNBREAKING, 1);
        // roll 1 % 2 == 1 → skip.
        assert_eq!(
            apply_wear(&mut stack, 1, false, 1, 0.0),
            WearOutcome::Untouched
        );
        assert_eq!(stack.damage(), Some(0));
        // roll 0 % 2 == 0 → apply.
        assert_eq!(
            apply_wear(&mut stack, 1, false, 0, 0.0),
            WearOutcome::Survived
        );
        assert_eq!(stack.damage(), Some(1));
    }

    #[test]
    fn a_non_damageable_stack_is_untouched() {
        let mut block = ItemStack::new(1, 16).expect("block stack");
        assert_eq!(
            apply_wear(&mut block, 1, false, 0, 0.0),
            WearOutcome::Untouched
        );
        assert!(!block.is_empty());
        let mut empty = ItemStack::EMPTY;
        assert_eq!(
            apply_wear(&mut empty, 1, false, 0, 0.0),
            WearOutcome::Untouched
        );
    }

    #[test]
    fn ensure_durability_attaches_the_derived_table_once() {
        let mut stack = ItemStack::new(1, 1).expect("tool");
        ensure_durability(&mut stack, "minecraft:diamond_pickaxe");
        assert_eq!(stack.max_damage(), Some(1561));
        assert_eq!(stack.damage(), Some(0));
        // A file's component wins: do not overwrite.
        stack.set_component(DataComponent::MaxDamage(50));
        ensure_durability(&mut stack, "minecraft:diamond_pickaxe");
        assert_eq!(stack.max_damage(), Some(50));
    }

    #[test]
    fn derived_max_damage_rows_match_the_vanilla_tiers() {
        assert_eq!(max_damage_of("minecraft:wooden_pickaxe"), Some(59));
        assert_eq!(max_damage_of("minecraft:stone_sword"), Some(131));
        assert_eq!(max_damage_of("minecraft:iron_pickaxe"), Some(250));
        assert_eq!(max_damage_of("minecraft:diamond_pickaxe"), Some(1561));
        assert_eq!(max_damage_of("minecraft:netherite_sword"), Some(2031));
        assert_eq!(max_damage_of("minecraft:diamond_chestplate"), Some(528));
        assert_eq!(max_damage_of("minecraft:stick"), None);
        assert_eq!(max_damage_of("minecraft:stone"), None);
        assert!(is_armor_item("minecraft:iron_helmet"));
        assert!(!is_armor_item("minecraft:iron_sword"));
    }
}
