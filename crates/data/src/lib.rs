//! Data pack loading: tags, recipes and the pack/namespace model (P07-03, P07-09).
//!
//! ## Why this is a crate and not part of `mc-registry`
//!
//! `AGENTS.md` §7 groups "Minecraft registry/data loading and lookup" under
//! `registry/`. `mc-registry` is the **id tables** — block states and items,
//! generated from the jar into a compact fixture it parses itself, with no I/O and no
//! JSON dependency. A data pack is a different concern: a filesystem tree of JSON with
//! cross-file semantics (tags referencing tags, packs overriding packs, namespaces).
//! Folding it in would have made `mc-registry` depend on `serde_json` and the
//! filesystem to serve a caller that only wants `state_id("minecraft:stone")`.
//!
//! The split is recorded in
//! [ADR-0004](../../docs/adr/ADR-0004-data-loading-and-registry-split.md) as a
//! refinement of §7's grouping rather than a departure from it: both crates are
//! "data", they are just two different kinds.
//!
//! ## What is verified against real data, and what is not
//!
//! The formats here were established by **surveying the actual 26.1.2 jar**, not from
//! memory. `target/vanilla-26.1.2/survey_tags.py` and the fixture manifest record the
//! numbers this module is built against:
//!
//! | Fact | Value | Consequence |
//! |---|---|---|
//! | tag files in `data/minecraft/tags/` | 758 | nesting is the common case, not an edge case |
//! | nested (`#other`) references | 384 | resolution must be transitive |
//! | deepest observed nesting | 4 | a cycle is reachable from a hostile pack, so it must be detected |
//! | cycles in vanilla | none | the cycle detector is *only* exercised by our tests |
//! | tags using `replace` | 0 | supported because the format defines it; **not** verified against vanilla |
//! | dict-form `{"id":…,"required":false}` entries | 0 | same: supported, not vanilla-exercised |
//! | tag registries | 17 | the loader is registry-agnostic; it does not know what an "item" is |
//!
//! The last two rows matter for honesty: this loader implements the *documented*
//! format, and the parts vanilla does not use are labelled as unverified rather than
//! presented as tested behaviour.
//!
//! ## Hostile input (AGENTS.md section 10)
//!
//! A data pack is operator-supplied, but "trusted" is not a security model: a pack
//! can come from a download. So every read is bounded ([`Limits`]), a tag cycle is
//! reported rather than recursed into, a missing reference is a reported problem, and
//! no input can panic. Deep-nesting JSON is additionally guarded by `serde_json`'s
//! own recursion limit.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

pub mod advancement;
pub mod enabled;
pub mod function;
pub mod json;
pub mod loot;
pub mod pack;
pub mod recipe;
pub mod tag;
pub mod tag_resolve;

pub use advancement::{
    Advancement, AdvancementDisplay, AdvancementIcon, AdvancementLoadReport, AdvancementProblem,
    AdvancementRegistry, AdvancementRewards, Chain, Criterion, Frame, TextComponent,
    read_advancement_file,
};
pub use function::{
    FunctionError, FunctionFile, FunctionLimits, FunctionLoadReport, FunctionRegistry,
    SUGGESTED_MAX_RECURSION_DEPTH, parse_function, read_function_file,
};
pub use json::{JsonError, Limits};
pub use loot::{
    ChanceProvider, CountProvider, EnchantmentRequirement, ForwardReference, ItemStackLike,
    LootCondition, LootContext, LootEntry, LootFunction, LootLoadReport, LootPool, LootTable,
    LootTables, Refusal, Rng, RollError, load_directory as load_loot_tables,
    roll as roll_loot_table,
};
pub use pack::{DataPack, DataPackSet, PackError, PackMetadata, PackSource};
pub use recipe::{
    CookingRecipe, Ingredient, Recipe, RecipeBook, RecipeKind, RecipeLoadReport, ShapedRecipe,
    ShapelessRecipe, SmeltingKind, StonecuttingRecipe,
};
pub use tag::{
    RegistryContents, TagEntry, TagFile, TagKey, TagLoadReport, TagProblem, TagSet, TagValue,
};
pub use tag_resolve::{MAX_TAG_DEPTH, resolve_all};

/// Count one occurrence of a type string in a census map.
///
/// The loaders all report "how many of each type did I see", and doing it in one place keeps
/// the three reports (`loot`, `function`, `advancement`) shaped the same way rather than three
/// near-copies of `entry(kind).or_insert(0) += 1`. Ordered map, so iterating a report is
/// deterministic (AGENTS.md §3.6).
pub(crate) fn bump(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_owned()).or_insert(0) += 1;
}

/// Namespace of the built-in data, as it appears in `data/minecraft/`.
pub const VANILLA_NAMESPACE: &str = "minecraft";

/// Directories a data pack may contribute, in the order they are loaded.
///
/// The order matters because a later pack overrides an earlier one *per file*, and
/// within a path the registry directory comes first. Kept as a list rather than
/// hard-coded call sites so "what does the loader know about" is one readable table.
pub const DATA_DIRECTORIES: &[&str] = &[
    "tags",
    "recipe",
    "loot_table",
    "advancement",
    "function",
    "predicate",
    "item_modifier",
    "worldgen",
    "damage_type",
    "enchantment",
    "dialog",
];
