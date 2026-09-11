//! Block names, and the name-to-[`PowerSource`] table the propagation rule uses.
//!
//! This module is the **only** place in the crate that matches a block *name*. Keeping
//! it in one file means the list of blocks this model treats as redstone components is
//! reviewable in one screen, and a test asserts that every name here exists in the
//! loaded registry — so a renamed or removed block cannot rot into a silently dead
//! entry.
//!
//! ## Evidence
//!
//! The names themselves are **derived from the project's own registry fixture**
//! (`crates/test-support/fixtures/registry/blocks.tsv`), which is dumped from the
//! official 26.1.2 server jar's registry by the `mc-registry` pipeline — so "this
//! block exists in 26.1.2 and spells its `powered` property this way" is a fact taken
//! from Mojang material, not from a wiki or a reference implementation.
//!
//! Which names *behave* as power sources is a model decision; see
//! [`crate::power::PowerSource`] for its label.

use crate::power::PowerSource;

/// The block name of redstone dust.
///
/// **derived from the registry fixture** — the block's registered name is
/// `minecraft:redstone_wire`, not `minecraft:redstone_dust`, and its state carries
/// `power=0|1|...|15` plus the four side-connection properties.
pub const REDSTONE_WIRE: &str = "minecraft:redstone_wire";

/// The block name of a lever.
pub const LEVER: &str = "minecraft:lever";

/// The block name of a standing redstone torch.
///
/// The wall variant (`minecraft:redstone_wall_torch`) carries the same `lit` property
/// and is listed separately in [`SOURCE_BLOCKS`].
pub const REDSTONE_TORCH: &str = "minecraft:redstone_torch";

/// The block name of a redstone torch attached to a wall.
pub const REDSTONE_WALL_TORCH: &str = "minecraft:redstone_wall_torch";

/// The block name of a block of redstone.
pub const REDSTONE_BLOCK: &str = "minecraft:redstone_block";

/// The block name of a repeater.
pub const REPEATER: &str = "minecraft:repeater";

/// The block name of a comparator.
pub const COMPARATOR: &str = "minecraft:comparator";

/// Every block name this crate treats as a power source, mapped to the source it is.
///
/// **approximation** — completeness is explicitly *not* claimed. Vanilla has many more
/// signal sources (observers, daylight detectors, detector rails, target blocks,
/// trapped chests, sculk sensors, jukeboxes, lecterns, `minecraft:light_weighted_…`
/// and the weathered/waxed lightning-rod variants, ...). A block that is missing from
/// this list is classified [`crate::propagation::BlockRole::Passive`] and therefore
/// emits nothing: it is *absent*, not silently something else.
///
/// The variants of one component are grouped because they behave identically for the
/// properties this model reads:
///
/// - buttons: every wood type plus stone and polished blackstone;
/// - pressure plates: every plain plate; the weighted plates are **deliberately
///   omitted** because their `power` property is a 0–15 level this model's
///   [`PowerSource::PressurePlate`] (a `powered=true|false` block) cannot represent —
///   listing them would report 15 for a plate that may be at 3;
/// - lightning rods: all eight oxidation/waxed variants, since the model reads only
///   `powered`.
pub const SOURCE_BLOCKS: &[(&str, PowerSource)] = &[
    (REDSTONE_BLOCK, PowerSource::RedstoneBlock),
    (REDSTONE_TORCH, PowerSource::Torch),
    (REDSTONE_WALL_TORCH, PowerSource::Torch),
    (LEVER, PowerSource::Lever),
    ("minecraft:stone_button", PowerSource::Button),
    ("minecraft:oak_button", PowerSource::Button),
    ("minecraft:spruce_button", PowerSource::Button),
    ("minecraft:birch_button", PowerSource::Button),
    ("minecraft:jungle_button", PowerSource::Button),
    ("minecraft:acacia_button", PowerSource::Button),
    ("minecraft:cherry_button", PowerSource::Button),
    ("minecraft:dark_oak_button", PowerSource::Button),
    ("minecraft:pale_oak_button", PowerSource::Button),
    ("minecraft:mangrove_button", PowerSource::Button),
    ("minecraft:bamboo_button", PowerSource::Button),
    ("minecraft:crimson_button", PowerSource::Button),
    ("minecraft:warped_button", PowerSource::Button),
    ("minecraft:polished_blackstone_button", PowerSource::Button),
    ("minecraft:stone_pressure_plate", PowerSource::PressurePlate),
    ("minecraft:oak_pressure_plate", PowerSource::PressurePlate),
    (
        "minecraft:spruce_pressure_plate",
        PowerSource::PressurePlate,
    ),
    ("minecraft:birch_pressure_plate", PowerSource::PressurePlate),
    (
        "minecraft:jungle_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:acacia_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:cherry_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:dark_oak_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:pale_oak_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:mangrove_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:bamboo_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:crimson_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:warped_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (
        "minecraft:polished_blackstone_pressure_plate",
        PowerSource::PressurePlate,
    ),
    (REPEATER, PowerSource::Repeater),
    (COMPARATOR, PowerSource::Comparator),
    ("minecraft:lightning_rod", PowerSource::LightningRod),
    ("minecraft:exposed_lightning_rod", PowerSource::LightningRod),
    (
        "minecraft:weathered_lightning_rod",
        PowerSource::LightningRod,
    ),
    (
        "minecraft:oxidized_lightning_rod",
        PowerSource::LightningRod,
    ),
    ("minecraft:waxed_lightning_rod", PowerSource::LightningRod),
    (
        "minecraft:waxed_exposed_lightning_rod",
        PowerSource::LightningRod,
    ),
    (
        "minecraft:waxed_weathered_lightning_rod",
        PowerSource::LightningRod,
    ),
    (
        "minecraft:waxed_oxidized_lightning_rod",
        PowerSource::LightningRod,
    ),
];

/// The source a block name is, if it is one.
#[must_use]
pub fn source_for_name(name: &str) -> Option<PowerSource> {
    SOURCE_BLOCKS
        .iter()
        .find(|(block, _)| *block == name)
        .map(|(_, source)| *source)
}

/// The canonical block name of a source, for resolving a state id in tests and tools.
///
/// A source with several blocks (buttons, pressure plates, lightning rods) reports its
/// first entry in [`SOURCE_BLOCKS`]; use that entry when one representative block is
/// enough. The value is the same one as [`PowerSource::block_name`], and a test asserts
/// the two agree so the two lists cannot drift.
#[must_use]
pub fn primary_block_name(source: PowerSource) -> &'static str {
    let from_table = SOURCE_BLOCKS
        .iter()
        .find(|(_, candidate)| *candidate == source)
        .map_or("minecraft:air", |(block, _)| *block);
    debug_assert_eq!(from_table, source.block_name());
    from_table
}

#[cfg(test)]
mod tests {
    use super::{
        COMPARATOR, LEVER, REDSTONE_BLOCK, REDSTONE_TORCH, REDSTONE_WALL_TORCH, REDSTONE_WIRE,
        REPEATER, SOURCE_BLOCKS, primary_block_name, source_for_name,
    };
    use crate::power::PowerSource;
    use mc_registry::Registries;

    fn registry() -> mc_registry::BlockRegistry {
        Registries::vanilla().expect("registry").blocks
    }

    #[test]
    fn every_listed_block_exists_in_the_registry() {
        // The anti-rot test: a regenerated fixture that renames or drops a block fails
        // here instead of leaving a dead entry that silently stops emitting.
        let blocks = registry();
        assert!(blocks.contains(REDSTONE_WIRE), "{REDSTONE_WIRE}");
        for (name, source) in SOURCE_BLOCKS {
            assert!(
                blocks.contains(name),
                "{name} ({source}) is not in the registry"
            );
        }
        // The five named constants are the ones the propagation rule depends on.
        for name in [
            REDSTONE_WIRE,
            LEVER,
            REDSTONE_TORCH,
            REDSTONE_WALL_TORCH,
            REDSTONE_BLOCK,
            REPEATER,
            COMPARATOR,
        ] {
            assert!(blocks.contains(name), "{name} missing");
        }
    }

    #[test]
    fn the_named_constants_spell_the_registry_names_exactly() {
        let blocks = registry();
        assert_eq!(
            blocks
                .block_name(blocks.default_state(REDSTONE_BLOCK).expect("block"))
                .expect("name"),
            "minecraft:redstone_block"
        );
        // A wall torch really is a separate block from a standing torch, which is why
        // both are listed.
        let standing = blocks.default_state(REDSTONE_TORCH).expect("torch");
        let wall = blocks
            .default_state(REDSTONE_WALL_TORCH)
            .expect("wall torch");
        assert_ne!(standing, wall);
        assert_ne!(blocks.default_state(REPEATER).expect("repeater"), 0);
        assert_ne!(blocks.default_state(COMPARATOR).expect("comparator"), 0);
        assert_ne!(blocks.default_state(LEVER).expect("lever"), 0);
    }

    #[test]
    fn every_source_block_name_resolves_back_to_its_source() {
        for (name, source) in SOURCE_BLOCKS {
            assert_eq!(source_for_name(name), Some(*source), "{name}");
            assert!(!primary_block_name(*source).is_empty());
        }
        assert_eq!(source_for_name("minecraft:stone"), None);
        assert_eq!(source_for_name("minecraft:redstone_dust"), None);
        assert_eq!(source_for_name(""), None);
    }

    #[test]
    fn the_representative_name_agrees_with_the_power_source_table() {
        // `PowerSource::block_name` and `SOURCE_BLOCKS` describe the same fact in two
        // places; this is the check that they cannot drift apart silently.
        for source in [
            PowerSource::RedstoneBlock,
            PowerSource::Torch,
            PowerSource::Lever,
            PowerSource::Button,
            PowerSource::PressurePlate,
            PowerSource::Repeater,
            PowerSource::Comparator,
            PowerSource::LightningRod,
        ] {
            assert_eq!(primary_block_name(source), source.block_name(), "{source}");
        }
    }

    #[test]
    fn the_source_table_has_no_duplicate_names() {
        let mut names: Vec<&str> = SOURCE_BLOCKS.iter().map(|(name, _)| *name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate name in SOURCE_BLOCKS");
    }

    #[test]
    fn weighted_pressure_plates_are_deliberately_absent() {
        // Their `power` property is a 0..=15 level, which `PowerSource::PressurePlate`
        // cannot represent; listing them would emit 15 for a plate that may be at 3.
        for name in [
            "minecraft:light_weighted_pressure_plate",
            "minecraft:heavy_weighted_pressure_plate",
        ] {
            assert_eq!(
                source_for_name(name),
                None,
                "{name} must not be a source yet"
            );
            assert!(
                registry().contains(name),
                "{name} exists in 26.1.2, so its absence is a model gap, not a typo"
            );
        }
    }

    #[test]
    fn every_power_source_variant_has_at_least_one_block() {
        for source in [
            PowerSource::RedstoneBlock,
            PowerSource::Torch,
            PowerSource::Lever,
            PowerSource::Button,
            PowerSource::PressurePlate,
            PowerSource::Repeater,
            PowerSource::Comparator,
            PowerSource::LightningRod,
        ] {
            assert!(
                SOURCE_BLOCKS
                    .iter()
                    .any(|(_, candidate)| *candidate == source),
                "{source} has no block name"
            );
            assert_ne!(primary_block_name(source), "minecraft:air", "{source}");
        }
    }
}
