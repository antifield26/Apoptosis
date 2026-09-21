//! Dig-rate evaluation: tool rules × hardness → per-tick progress (P16-05).
//!
//! The formula mirrors pumpkin's `calc_block_breaking` over
//! `Player::get_mining_speed`, which itself mirrors vanilla
//! (`ServerPlayerGameMode`): progress per tick is
//! `speed / hardness / divisor`, with divisor 30 when the block is
//! harvestable and 100 when it is not. Speed is the held item's first
//! speed-setting rule whose tag contains the block (else its default, 1.0
//! for every vanilla tool — asserted by `target/extract_mining.py`), divided
//! by 5 mid-air. Harvestable is `!requires_tool || correct`, where correct
//! is the first correct-judging rule whose tag contains the block.
//!
//! What is deliberately absent, and why:
//!
//! - **Efficiency / Haste / Mining Fatigue.** Enchantments are P18, and this
//!   build's effect set has no Haste or Mining Fatigue to read — inventing
//!   multipliers from memory would be exactly the guessing AGENTS.md §3.3
//!   forbids. The shape (additive efficiency when speed > 1.0, multiplicative
//!   haste/fatigue) is where the hooks go.
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

/// Per-tick dig rate for a survival dig.
///
/// `held_item` is the full item name or `None` for an empty hand;
/// `on_ground` carries vanilla's mid-air `/5`. A missing hardness entry
/// (fixture predates the block) digs at the flat 1.0 fallback the loader
/// documents rather than refusing the dig.
#[must_use]
pub fn dig_rate(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    held_item: Option<&str>,
    block: &str,
    on_ground: bool,
) -> DigRate {
    let hardness = blocks.hardness(block).unwrap_or(1.0);
    if hardness < 0.0 {
        return DigRate::Unbreakable;
    }
    if hardness == 0.0 {
        return DigRate::Instant;
    }
    let (mut speed, _) = tool_match(blocks, items, held_item, block);
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
        );
        assert_eq!(rate.ticks_to_break(), Some(6), "8/1.5/30 = 0.178/tick");
        // Same stone by hand: speed 1.0, divisor 100 → 150 ticks.
        let rate = dig_rate(&blocks, &items, None, "minecraft:stone", true);
        assert_eq!(rate.ticks_to_break(), Some(150));
        // Dirt (0.5, no requirement) by hand: 1/0.5/30 → 15 ticks.
        let rate = dig_rate(&blocks, &items, None, "minecraft:dirt", true);
        assert_eq!(rate.ticks_to_break(), Some(15));
        // Mid-air costs a factor of 5: the diamond pick needs 29 ticks.
        let rate = dig_rate(
            &blocks,
            &items,
            Some("minecraft:diamond_pickaxe"),
            "minecraft:stone",
            false,
        );
        assert_eq!(rate.ticks_to_break(), Some(29));
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
                true
            ),
            DigRate::Unbreakable
        );
        assert_eq!(
            dig_rate(&blocks, &items, None, "minecraft:torch", true),
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
