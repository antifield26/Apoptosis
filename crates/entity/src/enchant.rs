//! Enchantment *effects* (P18-01b): the four this build applies.
//!
//! Closed set matching `PHASE-18.md`: Efficiency (mining speed), Sharpness
//! (melee damage), Protection (damage taken) and Unbreaking (wear chance).
//! Every other stored enchantment is **inert** — shown and round-tripped, but
//! changing no number — and each is named in `docs/vanilla-parity/PARITY-MATRIX.md`.
//!
//! ## Sources (jar / 26.2 enchantment JSON)
//!
//! | Effect | Formula | Source |
//! |---|---|---|
//! | Efficiency | `mining_efficiency += level² + 1`, added to speed only when the tool's base speed is already `> 1.0` | `efficiency.json` (`levels_squared`, `added: 1.0`) + pumpkin `Player::get_mining_speed` |
//! | Sharpness | `damage += 1.0 + 0.5 × (level − 1)` | `sharpness.json` (`linear` base 1.0, per-level-above-first 0.5) |
//! | Protection | `protection += level`, then `damage × (1 − min(prot, 20) / 25)` | `protection.json` (`linear` base 1.0, per-level-above-first 1.0) + pumpkin `CombatRules::get_damage_after_magic_absorb` |
//! | Unbreaking | tools: apply wear when `roll % (level + 1) == 0`; armour: apply when `roll_f32 < 0.6 + 0.4 / (level + 1)` | `unbreaking.json` (`remove_binomial` fractions) + pumpkin `should_apply_durability_damage_with` |
//!
//! The Protection reduction runs **after** armour absorb, matching pumpkin's
//! `damage_with_context` order (armour first, then magic). Specialised
//! protections (Fire/Blast/Projectile/Feather Falling) and Breach stay inert.

#![allow(
    clippy::cast_precision_loss,
    clippy::manual_is_multiple_of,
    reason = "enchantment levels are small integers; the wear roll is a deliberate modulo"
)]

use crate::components::EnchantEntry;

/// Registry id of `minecraft:efficiency` (26.1 order, `MODELLED_ENCHANTMENTS`).
pub const EFFICIENCY: i32 = 8;
/// Registry id of `minecraft:protection`.
pub const PROTECTION: i32 = 28;
/// Registry id of `minecraft:sharpness`.
pub const SHARPNESS: i32 = 33;
/// Registry id of `minecraft:unbreaking`.
pub const UNBREAKING: i32 = 40;

/// Enchantments this build stores and **shows** but does **not** apply.
///
/// Named gap list for the parity matrix: every id here is inert. The four
/// applied effects above are deliberately absent.
pub const INERT_ENCHANTMENTS: &[&str] = &[
    "aqua_affinity",
    "bane_of_arthropods",
    "binding_curse",
    "blast_protection",
    "breach",
    "channeling",
    "density",
    "depth_strider",
    "feather_falling",
    "fire_aspect",
    "fire_protection",
    "flame",
    "fortune",
    "frost_walker",
    "impaling",
    "infinity",
    "knockback",
    "looting",
    "loyalty",
    "luck_of_the_sea",
    "lunge",
    "lure",
    "mending",
    "multishot",
    "piercing",
    "power",
    "projectile_protection",
    "punch",
    "quick_charge",
    "respiration",
    "riptide",
    "silk_touch",
    "smite",
    "soul_speed",
    "sweeping_edge",
    "swift_sneak",
    "thorns",
    "vanishing_curse",
    "wind_burst",
];

/// Level of `id` on a stack's applied enchantments (0 when absent).
#[must_use]
pub fn level_of(entries: Option<&[EnchantEntry]>, id: i32) -> i32 {
    entries
        .unwrap_or(&[])
        .iter()
        .find(|(e, _)| *e == id)
        .map_or(0, |(_, level)| (*level).max(0))
}

/// Efficiency's `mining_efficiency` contribution: `level² + 1`.
///
/// Only added when the tool's base speed is already `> 1.0` (the correct-tool
/// gate in pumpkin `Player::get_mining_speed`).
#[must_use]
pub fn efficiency_bonus(level: i32) -> f32 {
    if level <= 0 {
        0.0
    } else {
        let level = level as f32;
        level * level + 1.0
    }
}

/// Sharpness melee bonus: `1.0 + 0.5 × (level − 1)` (level 1 = 1.0, 5 = 3.0).
#[must_use]
pub fn sharpness_bonus(level: i32) -> f32 {
    if level <= 0 {
        0.0
    } else {
        1.0 + 0.5 * (level - 1) as f32
    }
}

/// Protection points contributed by one piece: `level`.
#[must_use]
pub fn protection_points(level: i32) -> f32 {
    if level <= 0 { 0.0 } else { level as f32 }
}

/// Vanilla `CombatRules.getDamageAfterMagicAbsorb` with the summed Protection
/// points: `damage × (1 − min(points, 20) / 25)`.
#[must_use]
pub fn damage_after_protection(damage: f32, protection: f32) -> f32 {
    let points = protection.clamp(0.0, 20.0);
    damage * (1.0 - points / 25.0)
}

/// Whether one unit of wear lands under Unbreaking.
///
/// Tools (and every non-armour durability item): apply when
/// `tool_roll % (level + 1) == 0` — Unbreaking III therefore lands 1 in 4.
/// Armour: apply when `armor_roll < 0.6 + 0.4 / (level + 1)` — Unbreaking III
/// lands about 70% of hits. `level <= 0` always applies.
///
/// Both rolls are parameters so a caller owns the RNG and a test can force
/// either arm without mocking (AGENTS.md §3.6).
#[must_use]
pub fn unbreaking_applies(level: i32, is_armor: bool, tool_roll: u32, armor_roll: f32) -> bool {
    if level <= 0 {
        return true;
    }
    if is_armor {
        let chance = 0.6 + 0.4 / (level as f32 + 1.0);
        armor_roll < chance
    } else {
        tool_roll % (level as u32 + 1) == 0
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "exact small-decimal quotients from the jar formulas"
)]
mod tests {
    use super::{
        EFFICIENCY, INERT_ENCHANTMENTS, PROTECTION, SHARPNESS, UNBREAKING, damage_after_protection,
        efficiency_bonus, level_of, protection_points, sharpness_bonus, unbreaking_applies,
    };

    /// Named behaviour pin: Efficiency speeds a correct-tool dig.
    /// Neutralising `efficiency_bonus` (return 0.0) turns this red.
    #[test]
    fn efficiency_bonus_is_levels_squared_plus_one() {
        assert_eq!(efficiency_bonus(0), 0.0);
        assert_eq!(efficiency_bonus(1), 2.0, "level 1: 1² + 1");
        assert_eq!(efficiency_bonus(2), 5.0, "level 2: 2² + 1");
        assert_eq!(efficiency_bonus(5), 26.0, "level 5: 5² + 1");
    }

    /// Named behaviour pin: Sharpness adds a flat melee bonus.
    /// Neutralising `sharpness_bonus` (return 0.0) turns this red.
    #[test]
    fn sharpness_bonus_is_linear_from_one() {
        assert_eq!(sharpness_bonus(0), 0.0);
        assert_eq!(sharpness_bonus(1), 1.0);
        assert_eq!(sharpness_bonus(2), 1.5);
        assert_eq!(sharpness_bonus(5), 3.0);
    }

    /// Named behaviour pin: Protection reduces damage after armour.
    /// Neutralising `damage_after_protection` (return damage unchanged) turns
    /// this red.
    #[test]
    fn protection_reduces_damage_by_points_over_25() {
        assert_eq!(protection_points(4), 4.0);
        // One Protection IV piece: 4/25 = 16% off a post-armour 10.0.
        let cut = damage_after_protection(10.0, 4.0);
        assert!((cut - 8.4).abs() < 1e-5, "got {cut}");
        // The 20-point cap is the vanilla `MAX_ARMOR` clamp.
        let capped = damage_after_protection(10.0, 50.0);
        assert!((capped - 2.0).abs() < 1e-5, "got {capped}");
        // No protection: untouched.
        assert_eq!(damage_after_protection(7.0, 0.0), 7.0);
    }

    /// Named behaviour pin: Unbreaking skips wear on the documented roll.
    /// Neutralising `unbreaking_applies` (always true) turns the skip arms red.
    #[test]
    fn unbreaking_skips_wear_on_the_documented_roll() {
        // No Unbreaking: every wear lands.
        assert!(unbreaking_applies(0, false, 1, 0.99));
        assert!(unbreaking_applies(0, true, 1, 0.99));
        // Tool III: apply when roll % 4 == 0.
        assert!(unbreaking_applies(3, false, 0, 0.0));
        assert!(!unbreaking_applies(3, false, 1, 0.0));
        assert!(!unbreaking_applies(3, false, 3, 0.0));
        assert!(unbreaking_applies(3, false, 4, 0.0));
        // Tool I: half the rolls land.
        assert!(unbreaking_applies(1, false, 0, 0.0));
        assert!(!unbreaking_applies(1, false, 1, 0.0));
        // Armour III: apply when roll < 0.6 + 0.4/4 ≈ 0.7 (f32 rounding means
        // the exact 0.70 literal can sit either side, so probe below/above).
        assert!(unbreaking_applies(3, true, 0, 0.50));
        assert!(!unbreaking_applies(3, true, 0, 0.90));
        // Armour I: apply when roll < 0.6 + 0.4/2 = 0.8.
        assert!(unbreaking_applies(1, true, 0, 0.50));
        assert!(!unbreaking_applies(1, true, 0, 0.90));
    }

    #[test]
    fn the_four_applied_effects_are_not_in_the_inert_list() {
        for id in [EFFICIENCY, PROTECTION, SHARPNESS, UNBREAKING] {
            // A smoke check that the ids resolve; the inert names are the
            // string list the parity matrix cites.
            assert!(id >= 0);
        }
        assert!(!INERT_ENCHANTMENTS.contains(&"efficiency"));
        assert!(!INERT_ENCHANTMENTS.contains(&"sharpness"));
        assert!(!INERT_ENCHANTMENTS.contains(&"protection"));
        assert!(!INERT_ENCHANTMENTS.contains(&"unbreaking"));
        assert!(INERT_ENCHANTMENTS.contains(&"silk_touch"));
        assert!(INERT_ENCHANTMENTS.contains(&"fortune"));
        assert!(INERT_ENCHANTMENTS.len() >= 30);
    }

    #[test]
    fn level_of_reads_the_stored_entry() {
        let entries = [(EFFICIENCY, 3), (SHARPNESS, 1)];
        assert_eq!(level_of(Some(&entries), EFFICIENCY), 3);
        assert_eq!(level_of(Some(&entries), SHARPNESS), 1);
        assert_eq!(level_of(Some(&entries), UNBREAKING), 0);
        assert_eq!(level_of(None, EFFICIENCY), 0);
    }
}
