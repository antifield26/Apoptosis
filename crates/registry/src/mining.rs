//! Dig-rate evaluation: tool rules × hardness × Efficiency → per-tick progress
//! (P16-05, P18-01b).
//!
//! The formula mirrors pumpkin's `calc_block_breaking` over
//! `Player::get_mining_speed`, which itself mirrors vanilla
//! (`ServerPlayerGameMode`): progress per tick is
//! `speed / hardness / divisor`, with divisor 30 when the block is
//! harvestable and 100 when it is not. Speed is the held item's first
//! speed-setting rule whose tag contains the block (else its default, 1.0
//! for every vanilla tool — asserted by `target/extract_mining.py`); **when
//! that base speed is already `> 1.0`**, Efficiency adds `level² + 1`
//! (jar `efficiency.json` `levels_squared` + `added: 1.0`; pumpkin
//! `get_mining_speed` gates the same way). Mid-air divides by 5. Harvestable
//! is `!requires_tool || correct`, where correct is the first correct-judging
//! rule whose tag contains the block.
//!
//! What is deliberately absent, and why:
//!
//! - **Haste / Mining Fatigue.** This build's effect set has no numeric
//!   Haste or Fatigue to read — inventing multipliers from memory would be
//!   exactly the guessing AGENTS.md §3.3 forbids. The shape (multiplicative
//!   haste/fatigue after Efficiency) is where the hooks go.
//! - **Water penalty.** Vanilla divides by 5 without Aqua Affinity; water
//!   checks are a fluid-system question for another phase.
//! - **Sweeping/creative nuances.** Creative never reaches this module.

use crate::{BlockRegistry, ItemRegistry};

/// What one survival tick of digging does to a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DigRate {
    /// Hardness below 0 (bedrock and friends): refused, never accumulates.
    Unbreakable,
    /// Hardness exactly 0 (torches, grass tufts): breaks the tick digging starts.
    Instant,
    /// Progress added per tick; the block breaks at 1.0.
    PerTick(f32),
}

impl DigRate {
    /// Ticks of uninterrupted digging to break, rounded up. `None` for
    /// [`DigRate::Unbreakable`]; `Some(1)` for [`DigRate::Instant`].
    #[must_use]
    pub fn ticks_to_break(self) -> Option<u32> {
        match self {
            Self::Unbreakable => None,
            Self::Instant => Some(1),
            Self::PerTick(rate) => {
                if rate.is_finite() && rate > 0.0 {
                    Some((1.0 / rate).ceil() as u32)
                } else {
                    None
                }
            }
        }
    }
}

/// Mining speed and harvest judgment for a held item against a block.
///
/// Returns `(speed, correct)`: the first speed-setting rule whose tag holds
/// the block wins the speed (else the tool default, 1.0 for a bare hand or
/// an unknown item), and the first correct-judging rule whose tag holds the
/// block wins the harvest judgment (else `false`).
#[must_use]
pub fn tool_match(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    held_item: Option<&str>,
    block: &str,
) -> (f32, bool) {
    let Some(entry) = held_item.and_then(|name| items.tool(name)) else {
        return (1.0, false);
    };
    // A rule matches when its tag sits in either membership list: the
    // mineable/efficiency tags or the incorrect-tier tags. Both lists hold
    // full tag names, so one comparison covers both.
    let matches = |tag: &str| {
        blocks.mineable_tags(block).iter().any(|t| t == tag)
            || blocks.refused_tiers(block).iter().any(|t| t == tag)
    };
    let mut speed = entry.default_speed;
    for rule in &entry.rules {
        if rule.speed.is_some() && matches(&rule.tag) {
            speed = rule.speed.unwrap_or(speed);
            break;
        }
    }
    let mut correct = false;
    for rule in &entry.rules {
        if rule.correct.is_some() && matches(&rule.tag) {
            correct = rule.correct.unwrap_or(false);
            break;
        }
    }
    (speed, correct)
}

/// Whether digging drops anything: every block without a tool requirement
/// harvests, the rest need a correct tool (vanilla `can_harvest`).
#[must_use]
pub fn can_harvest(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    held_item: Option<&str>,
    block: &str,
) -> bool {
    if !blocks.requires_tool(block) {
        return true;
    }
    tool_match(blocks, items, held_item, block).1
}

/// Efficiency's additive `mining_efficiency` contribution: `level² + 1`.
///
/// Mirrors the enchantment JSON (`levels_squared`, `added: 1.0`) and
/// pumpkin's `EFFICIENCY_ATTRIBUTE_MODIFIER_ID` amount. Only added when the
/// tool's base speed is already `> 1.0`.
#[must_use]
pub fn efficiency_bonus(level: i32) -> f32 {
    if level <= 0 {
        0.0
    } else {
        #[allow(
            clippy::cast_precision_loss,
            reason = "enchantment level is a small integer (1..=5); f32 holds it exactly"
        )]
        let level = level as f32;
        level * level + 1.0
    }
}

/// Per-tick dig rate for a survival dig.
///
/// `held_item` is the full item name or `None` for an empty hand;
/// `on_ground` carries vanilla's mid-air `/5`; `efficiency_level` is the
/// held tool's Efficiency (0 when absent or unenchanted). A missing hardness
/// entry (fixture predates the block) digs at the flat 1.0 fallback the
/// loader documents rather than refusing the dig.
#[must_use]
pub fn dig_rate(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    held_item: Option<&str>,
    block: &str,
    on_ground: bool,
    efficiency_level: i32,
) -> DigRate {
    let hardness = blocks.hardness(block).unwrap_or(1.0);
    if hardness < 0.0 {
        return DigRate::Unbreakable;
    }
    if hardness == 0.0 {
        return DigRate::Instant;
    }
    let (mut speed, _) = tool_match(blocks, items, held_item, block);
    // Efficiency only when the tool already breaks this block faster than a
    // fist (pumpkin `Player::get_mining_speed`).
    if speed > 1.0 {
        speed += efficiency_bonus(efficiency_level);
    }
    if !on_ground {
        speed /= 5.0;
    }
    let divisor = if can_harvest(blocks, items, held_item, block) {
        30.0
    } else {
        100.0
    };
    DigRate::PerTick(speed / hardness / divisor)
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "rates asserted are exact small-decimal quotients"
)]
mod tests {
    use super::{DigRate, can_harvest, dig_rate, tool_match};
    use std::path::Path;

    fn registries() -> (crate::BlockRegistry, crate::ItemRegistry) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/test-support/fixtures/registry");
        let blocks = crate::BlockRegistry::load(&dir.join("blocks.tsv")).expect("blocks load");
        let items = crate::ItemRegistry::load(&dir.join("items.tsv")).expect("items load");
        (blocks, items)
    }

    #[test]
    fn labelled_rates_match_the_formula() {
        let (blocks, items) = registries();
        // Stone (1.5, requires tool) with a diamond pick (8.0): 8/1.5/30.
        let rate = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_pickaxe"),
            "minecraft:stone",
            true,
            0,
        );
        assert_eq!(rate.ticks_to_break(), Some(6), "8/1.5/30 = 0.178/tick");
        // Same stone by hand: speed 1.0, divisor 100 → 150 ticks.
        let rate = dig_rate(&blocks, &items, None, "minecraft:stone", true, 0);
        assert_eq!(rate.ticks_to_break(), Some(150));
        // Dirt (0.5, no requirement) by hand: 1/0.5/30 → 15 ticks.
        let rate = dig_rate(&blocks, &items, None, "minecraft:dirt", true, 0);
        assert_eq!(rate.ticks_to_break(), Some(15));
        // Mid-air costs a factor of 5: the diamond pick needs 29 ticks.
        let rate = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_pickaxe"),
            "minecraft:stone",
            false,
            0,
        );
        assert_eq!(rate.ticks_to_break(), Some(29));
    }

    /// Named behaviour pin (P18-01b): Efficiency speeds a correct-tool dig.
    /// Neutralising `efficiency_bonus` (return 0.0) turns this red.
    #[test]
    fn efficiency_speeds_up_a_correct_tool_dig() {
        let (blocks, items) = registries();
        // Baseline stone + diamond pick: 8/1.5/30 → 6 ticks.
        let plain = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_pickaxe"),
            "minecraft:stone",
            true,
            0,
        );
        assert_eq!(plain.ticks_to_break(), Some(6));
        // Efficiency III: speed 8 + 10 = 18 → 18/1.5/30 = 0.4/tick → 3 ticks.
        let fast = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_pickaxe"),
            "minecraft:stone",
            true,
            3,
        );
        assert_eq!(fast.ticks_to_break(), Some(3));
        // A bare hand (speed 1.0) gains nothing: the gate is speed > 1.0.
        let hand = dig_rate(&blocks, &items, None, "minecraft:dirt", true, 5);
        let plain_hand = dig_rate(&blocks, &items, None, "minecraft:dirt", true, 0);
        assert_eq!(hand.ticks_to_break(), plain_hand.ticks_to_break());
    }

    #[test]
    fn unbreakable_instant_and_harvest_judgments() {
        let (blocks, items) = registries();
        assert_eq!(
            dig_rate(
                &blocks,
                &items,
                Some("minecraft:diamond_pickaxe"),
                "minecraft:bedrock",
                true,
                0
            ),
            DigRate::Unbreakable
        );
        assert_eq!(
            dig_rate(&blocks, &items, None, "minecraft:torch", true, 0),
            DigRate::Instant
        );
        // Diamond ore (3.0, needs iron): a stone pick is too weak (divisor
        // 100) while iron harvests (divisor 30).
        assert!(!can_harvest(
            &blocks,
            &items,
            Some("minecraft:stone_pickaxe"),
            "minecraft:diamond_ore"
        ));
        assert!(can_harvest(
            &blocks,
            &items,
            Some("minecraft:iron_pickaxe"),
            "minecraft:diamond_ore"
        ));
        // A sword's instant rule fires on bamboo: the rate already exceeds 1.
        let DigRate::PerTick(rate) = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_sword"),
            "minecraft:bamboo",
            true,
            0,
        ) else {
            panic!("bamboo with a sword must be a per-tick rate");
        };
        assert!(rate >= 1.0, "sword_instantly_mines breaks in one tick");
        // Shears on oak leaves ride the extreme rule at 15.0.
        let (speed, _) = tool_match(
            &blocks,
            &items,
            Some("minecraft:shears"),
            "minecraft:oak_leaves",
        );
        assert_eq!(speed, 15.0);
    }
}
