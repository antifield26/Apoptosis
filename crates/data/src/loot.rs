//! Loot tables: the data model, the loader, and a **bounded, refusing** roll (P07-10).
//!
//! ## The format, surveyed from the 26.1.2 jar
//!
//! `data/minecraft/loot_table/` holds **1 326 files**. Every one was read while this
//! module was written; the figures below are counts, not recollections. The survey script
//! is `target/vanilla-26.1.2/survey_loot_adv.py`, and `tests/vanilla_data.rs` re-derives
//! the same numbers from the extracted pack.
//!
//! | Level | Distinct types | Occurrences |
//! |---|---|---|
//! | table `type` | 11 | 1 326 |
//! | entry `type` | 6 | 2 542 |
//! | `function` | 18 | 1 392 |
//! | `condition` | 12 | 1 500 |
//!
//! The structural discriminator differs per level, which is a real trap: a table and an
//! entry use `"type"`, a function uses `"function"`, and a condition uses `"condition"`.
//! A parser that reads `"type"` everywhere finds nothing at all.
//!
//! | Table `type` | Files |
//! |---|---|
//! | `minecraft:block` | 1 085 |
//! | `minecraft:entity` | 114 |
//! | `minecraft:chest` | 65 |
//! | `minecraft:shearing` | 22 |
//! | `minecraft:gift` | 21 |
//! | `minecraft:archaeology` | 6 |
//! | `minecraft:block_interact` | 4 |
//! | `minecraft:fishing` | 4 |
//! | `minecraft:equipment` | 3 |
//! | `minecraft:entity_interact` | 1 |
//! | `minecraft:barter` | 1 |
//!
//! ## What this module executes, and what it refuses
//!
//! A wrong roll is worse than no roll: it produces items that look plausible and are
//! silently wrong. So [`roll`] implements a **named, tested subset** and refuses the
//! rest. Two rules make that honest:
//!
//! 1. **Nothing is dropped at parse time.** A function, condition, entry or count
//!    provider whose type is not modelled is kept as its `Raw` variant holding the
//!    original JSON, so the structure still describes the file; the loader counts it in
//!    [`LootLoadReport::unexecutable`].
//! 2. **Nothing is ignored at roll time.** A run that meets a `Raw` construct, a tag
//!    entry, a dynamic entry or a nesting problem returns [`RollError`] naming it. It
//!    never returns a partial result as if it were the answer.
//!
//! ### Executable
//!
//! | Level | Types |
//! |---|---|
//! | entries | `item`, `empty`, `alternatives`, `loot_table` (named or inline) |
//! | functions | `set_count`, `limit_count` |
//! | conditions | `survives_explosion`, `random_chance`, `random_chance_with_enchanted_bonus`, `match_tool` (enchantment tests only), `table_bonus`, `any_of`, `inverted` |
//! | count providers | number literal, `minecraft:uniform`, `minecraft:binomial` |
//!
//! `minecraft:table_bonus`, `minecraft:random_chance_with_enchanted_bonus` and
//! `minecraft:match_tool` are executed **only** when the caller supplies the enchantment
//! levels in [`LootContext`]; without them the roll refuses, because assuming level 0
//! would silently drop the looting and silk-touch outcomes.
//!
//! ### Refused
//!
//! Everything else, with the reason attached to each refusal rather than to a comment:
//! the 16 unexecuted function types (`enchant_randomly`, `enchant_with_levels`,
//! `apply_bonus`, `explosion_decay`, `enchanted_count_increase`, `copy_components`,
//! `copy_state`, `exploration_map`, `furnace_smelt`, `set_potion`, `set_enchantments`,
//! `set_name`, `set_instrument`, `set_stew_effect`, `set_ominous_bottle_amplifier`,
//! `set_components`), the 5 unexecuted condition types (`block_state_property`,
//! `entity_properties`, `damage_source_properties`, `location_check`,
//! `killed_by_player`), the `dynamic` and `tag` entry types, and `minecraft:sequence`.
//! Each needs the item registry, the enchantment tables, a structure generator, a block
//! entity, entity state or a damage source — none of which a loot context here carries,
//! and guessing any of them produces wrong loot.
//!
//! ## Randomness is injected, never invented (AGENTS.md section 3.6)
//!
//! `mc-data` sits below `mc-simulation`, which owns the JDK-verified `RandomSource`;
//! depending on it would invert the layering. So [`Rng`] is a trait defined here in the
//! same shape as `mc_entity::mob::Rng` — a trait naming the contract, with one `impl` in
//! the crate that owns the concrete generator. It exposes `next_f32`/`next_f64` alongside
//! `next_u32` because loot chances are floats, and faking a float out of an integer draw
//! would change the distribution.
//!
//! ## Hostile input (AGENTS.md section 10)
//!
//! A pack is not trusted. Every file goes through [`crate::json`] with [`Limits`]; entry
//! nesting, table nesting and the number of draws a binomial provider may request are all
//! bounded; and no path here can panic.

use mc_core::ids::ResourceId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use crate::json::{
    JsonError, Limits, optional_f64, optional_i64, optional_str, read_json_object, required_array,
    required_str, type_name,
};

/// Deepest nesting of entry `children` this loader will follow.
///
/// Vanilla's deepest is **2** (`blocks/decorated_pot`: `alternatives` -> child -> child),
/// so 16 is 8x headroom. A deeper tree is generated or hostile, and is reported instead of
/// recursed into.
pub const MAX_ENTRY_DEPTH: usize = 16;

/// Deepest nesting of `minecraft:loot_table` references a roll will follow.
///
/// Vanilla's deepest chain is **2** (`entities/zombie` -> `charged_creeper/zombie` -> a
/// table with no further reference). 8 is 4x headroom, and the guard is what makes a
/// cyclic pair of tables terminate instead of recursing forever.
pub const MAX_TABLE_NESTING: usize = 8;

/// Largest `n` a `minecraft:binomial` provider may request before the roll refuses it.
///
/// Vanilla's own `n` is 3. A pack asking for `n = 10^9` would otherwise burn a billion
/// draws, which is exactly the CPU amplification AGENTS.md section 10 warns about; past
/// this ceiling the provider is refused.
pub const MAX_BINOMIAL_ROUNDS: u32 = 4096;

/// The randomness a roll may consume.
///
/// This is the crate boundary in the same way `mc_entity::mob::Rng` is: `mc-data` may not
/// depend on `mc-simulation`, which owns the seeded `java.util.Random` clone, so the
/// contract is named here and wired up by whoever owns the generator.
///
/// Implementations **must** be a pure function of their own state: the same seed has to
/// produce the same sequence, or a loot roll cannot be reproduced (AGENTS.md §3.6).
pub trait Rng {
    /// A uniform 32-bit draw.
    fn next_u32(&mut self) -> u32;

    /// A uniform draw in `[0, 1)`, with 24 bits of precision.
    ///
    /// 24, not 32: `java.util.Random.nextFloat()` is `next(24) / 2^24`, and
    /// `minecraft:random_chance` compares against exactly that. A wider draw would change
    /// which rolls succeed near the boundary.
    fn next_f32(&mut self) -> f32 {
        // `>> 8` keeps the high 24 bits, which is what `java.util.Random.next(24)`
        // returns, so the two agree.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A uniform draw in `[0, 1)`, with 53 bits of precision.
    ///
    /// 53 bits matches `java.util.Random.nextDouble()`.
    fn next_f64(&mut self) -> f64 {
        let high = u64::from(self.next_u32() >> 5);
        let low = u64::from(self.next_u32() >> 6);
        ((high << 26) + low) as f64 / (1u64 << 53) as f64
    }

    /// Whether a draw of probability `chance` succeeds.
    ///
    /// The comparison is strict (`<`), which is what vanilla does: a chance of `0.0` never
    /// succeeds and `1.0` always does.
    fn chance(&mut self, chance: f32) -> bool {
        self.next_f32() < chance
    }
}

/// One item produced by a roll, with the fields a loot table can actually decide.
///
/// Deliberately *not* `mc_entity`'s `ItemStack`: this crate sits below `mc-entity`, and
/// depending on it would invert the layering. The caller converts. `damage` is a
/// fractional wear in `[0, 1]` because that is what the format specifies; turning it into
/// durability points needs the item registry, which is why `minecraft:set_damage` is
/// modelled but not executable.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStackLike {
    /// The item.
    pub item: ResourceId,
    /// How many.
    pub count: i32,
    /// Fractional damage in `[0, 1]`, when a function set it.
    pub damage: Option<f64>,
}

impl ItemStackLike {
    /// A stack of `count` of one item.
    #[must_use]
    pub const fn new(item: ResourceId, count: i32) -> Self {
        Self {
            item,
            count,
            damage: None,
        }
    }
}

/// What the caller knows about the situation the roll happens in.
///
/// The point of this type is that *absence is not neutral*. A field left `None` means "the
/// caller did not say", and a condition that depends on it is **refused** rather than
/// assumed false. Assuming false is how a roll silently loses the silk-touch drop or the
/// looting bonus.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootContext {
    /// Enchantment levels on the tool that produced the drop.
    ///
    /// `None` (the default) means the tool is unknown, and every enchantment-dependent
    /// condition refuses. `Some(map)` with an absent key means level 0: a tool that is
    /// known and simply does not carry that enchantment.
    pub enchantment_levels: Option<BTreeMap<ResourceId, i32>>,
    /// Whether the block survived the explosion that broke it.
    pub survives_explosion: Option<bool>,
    /// Block state properties as strings, exactly as the file writes them (`{"age": "7"}`,
    /// `{"cracked": "true"}`).
    pub block_properties: Option<BTreeMap<String, String>>,
    /// The killer's luck attribute, which scales entry `quality`.
    ///
    /// Required only by a table whose entries carry `quality`; those are refused without
    /// it.
    pub luck: Option<f32>,
}

impl LootContext {
    /// A context that knows the tool, which is the case for every block break.
    #[must_use]
    pub const fn with_tool(enchantment_levels: BTreeMap<ResourceId, i32>) -> Self {
        Self {
            enchantment_levels: Some(enchantment_levels),
            survives_explosion: None,
            block_properties: None,
            luck: None,
        }
    }

    /// The level of an enchantment, or `None` when the tool is unknown.
    #[must_use]
    pub fn enchantment_level(&self, enchantment: &ResourceId) -> Option<i32> {
        self.enchantment_levels
            .as_ref()
            .map(|levels| levels.get(enchantment).copied().unwrap_or(0))
    }
}

/// A number the format lets a pack express four ways.
#[derive(Debug, Clone, PartialEq)]
pub enum CountProvider {
    /// A literal. Vanilla writes `"rolls": 1.0` and `"count": 3`; both are literals.
    Constant(f64),
    /// A uniform draw in `[min, max)`.
    Uniform {
        /// Lower bound.
        min: f64,
        /// Upper bound.
        max: f64,
    },
    /// A binomial draw: one plus the successes of `n` rounds at `p`.
    Binomial {
        /// Rounds.
        extra: i32,
        /// Per-round probability.
        probability: f32,
    },
    /// A provider type outside the executable subset, kept whole.
    ///
    /// `minecraft:score`, `minecraft:storage` and anything else land here: the table still
    /// describes itself, and a roll refuses them.
    Raw(serde_json::Value),
}

impl CountProvider {
    /// The provider's type string, as written in the file.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::Constant(_) => "constant",
            Self::Uniform { .. } => "minecraft:uniform",
            Self::Binomial { .. } => "minecraft:binomial",
            Self::Raw(value) => value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<provider without a type>"),
        }
    }

    /// Whether [`roll`] can draw from this.
    #[must_use]
    pub const fn is_executable(&self) -> bool {
        !matches!(self, Self::Raw(_))
    }
}

/// A loot function: something that modifies the stack an entry produced.
#[derive(Debug, Clone, PartialEq)]
pub enum LootFunction {
    /// `minecraft:set_count`. `add` means "add to the existing count" rather than "replace
    /// it".
    SetCount {
        /// The count to set, or to add.
        count: CountProvider,
        /// Whether to add rather than replace.
        add: bool,
    },
    /// `minecraft:limit_count`. Discards a stack below `min` and clamps one above `max`.
    LimitCount {
        /// Lower bound.
        min: i32,
        /// Upper bound.
        max: i32,
    },
    /// `minecraft:set_damage`. **Modelled, not executed**: turning a fraction into
    /// durability points needs the item registry's max-damage table.
    SetDamage {
        /// Fractional damage in `[0, 1]`.
        damage: CountProvider,
        /// Whether to add rather than replace.
        add: bool,
    },
    /// A function type this build does not model. The JSON is kept verbatim.
    Raw(serde_json::Value),
}

impl LootFunction {
    /// The function's type string.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::SetCount { .. } => "minecraft:set_count",
            Self::LimitCount { .. } => "minecraft:limit_count",
            Self::SetDamage { .. } => "minecraft:set_damage",
            Self::Raw(value) => raw_type(value, "function"),
        }
    }

    /// Whether [`roll`] can execute this.
    #[must_use]
    pub fn is_executable(&self) -> bool {
        match self {
            Self::SetCount { count, .. } => count.is_executable(),
            Self::LimitCount { .. } => true,
            Self::SetDamage { .. } | Self::Raw(_) => false,
        }
    }

    /// Why it cannot be executed, when it cannot.
    #[must_use]
    pub const fn refusal_reason(&self) -> Option<&'static str> {
        match self {
            Self::SetCount { .. } | Self::LimitCount { .. } => None,
            Self::SetDamage { .. } => Some(
                "minecraft:set_damage converts a fraction into durability points, which needs \
                 the item registry's max-damage table",
            ),
            Self::Raw(_) => Some("this loot function type is not implemented in this build"),
        }
    }
}

/// A loot condition.
#[derive(Debug, Clone, PartialEq)]
pub enum LootCondition {
    /// `minecraft:survives_explosion`. Needs [`LootContext::survives_explosion`].
    SurvivesExplosion,
    /// `minecraft:random_chance`.
    RandomChance {
        /// Probability in `[0, 1]`.
        chance: f32,
    },
    /// `minecraft:random_chance_with_enchanted_bonus`. Needs the enchantment level.
    RandomChanceWithEnchantedBonus {
        /// The enchantment whose level picks the chance.
        enchantment: ResourceId,
        /// Chance at level 0.
        unenchanted_chance: f32,
        /// Chance at level 1 and above.
        enchanted_chance: ChanceProvider,
    },
    /// `minecraft:table_bonus`. Needs the enchantment level.
    TableBonus {
        /// The enchantment whose level indexes `chances`.
        enchantment: ResourceId,
        /// Chance per level, saturating at the last entry.
        chances: Vec<f32>,
    },
    /// `minecraft:match_tool`, restricted to the part that is checkable here: an
    /// enchantment-level test.
    ///
    /// The full predicate also covers item type, count, durability and item components,
    /// none of which a [`LootContext`] carries. A `match_tool` whose predicate contains
    /// anything else is **not** represented by this variant: the loader puts it in
    /// [`LootCondition::Raw`], so a roll cannot mistake "I checked the enchantments" for "I
    /// checked the whole predicate".
    MatchToolEnchantments {
        /// Each requirement: an enchantment (or a tag, which cannot be resolved here) and
        /// the level range it must fall in.
        requirements: Vec<EnchantmentRequirement>,
    },
    /// `minecraft:any_of`. Executable when every term is.
    AnyOf {
        /// The terms.
        terms: Vec<Self>,
    },
    /// `minecraft:inverted`. Executable when its term is.
    Inverted {
        /// The term.
        term: Box<Self>,
    },
    /// A condition type this build does not model. The JSON is kept verbatim.
    Raw(serde_json::Value),
}

impl LootCondition {
    /// The condition's type string.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::SurvivesExplosion => "minecraft:survives_explosion",
            Self::RandomChance { .. } => "minecraft:random_chance",
            Self::RandomChanceWithEnchantedBonus { .. } => {
                "minecraft:random_chance_with_enchanted_bonus"
            }
            Self::TableBonus { .. } => "minecraft:table_bonus",
            Self::MatchToolEnchantments { .. } => "minecraft:match_tool",
            Self::AnyOf { .. } => "minecraft:any_of",
            Self::Inverted { .. } => "minecraft:inverted",
            Self::Raw(value) => raw_type(value, "condition"),
        }
    }

    /// Whether [`roll`] can evaluate this.
    #[must_use]
    pub fn is_executable(&self) -> bool {
        match self {
            Self::SurvivesExplosion
            | Self::RandomChance { .. }
            | Self::RandomChanceWithEnchantedBonus { .. }
            | Self::MatchToolEnchantments { .. } => true,
            // An empty `chances` list has no level to index, so it cannot be evaluated; the
            // loader refuses to build one, and this is the belt-and-braces check for a
            // hand-built table.
            Self::TableBonus { chances, .. } => !chances.is_empty(),
            Self::AnyOf { terms } => terms.iter().all(Self::is_executable),
            Self::Inverted { term } => term.is_executable(),
            Self::Raw(_) => false,
        }
    }
}

/// One enchantment requirement inside a `minecraft:match_tool` predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentRequirement {
    /// The enchantment name, or a tag when the file writes `#minecraft:smelts_loot`. A tag
    /// cannot be resolved here, so a roll refuses it.
    pub enchantment: String,
    /// The lowest level that satisfies it, when the file states one.
    pub min_level: Option<i32>,
    /// The highest level that satisfies it, when the file states one.
    pub max_level: Option<i32>,
}

/// A chance that scales with an enchantment level.
///
/// Vanilla writes `minecraft:linear` here (`{"base": 0.035,
/// "per_level_above_first": 0.01}`); a bare number is also accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum ChanceProvider {
    /// A fixed chance.
    Constant(f32),
    /// `base + per_level_above_first * (level - 1)`, clamped to `[0, 1]`.
    Linear {
        /// The chance at level 1.
        base: f32,
        /// Added per level above the first.
        per_level_above_first: f32,
    },
}

impl ChanceProvider {
    /// The chance at a given enchantment level.
    ///
    /// A level below 1 is treated as level 1, which is where the linear formula starts.
    #[must_use]
    pub fn at_level(&self, level: i32) -> f32 {
        match self {
            Self::Constant(chance) => *chance,
            Self::Linear {
                base,
                per_level_above_first,
            } => {
                let levels_above = (level - 1).max(0) as f32;
                (base + per_level_above_first * levels_above).clamp(0.0, 1.0)
            }
        }
    }
}

/// One alternative inside a pool, which may itself contain alternatives.
#[derive(Debug, Clone, PartialEq)]
pub enum LootEntry {
    /// `minecraft:item`: a concrete stack.
    Item {
        /// The item.
        name: ResourceId,
        /// Draw weight; 1 when the file omits it.
        weight: i32,
        /// Luck scaling; 0 when the file omits it.
        quality: i32,
        /// Whether to expand a tag name into one entry per member. `expand` is **never**
        /// present in vanilla, and expanding needs the tag set, so a roll refuses it.
        expand: bool,
        /// Applied in order.
        functions: Vec<LootFunction>,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:empty`: a weighted nothing.
    Empty {
        /// Draw weight.
        weight: i32,
        /// Luck scaling.
        quality: i32,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:loot_table`: another table, either named or inlined.
    LootTable {
        /// The named table, when the file gives a string.
        name: Option<ResourceId>,
        /// The inlined table, when the file gives an object. Vanilla does this three times,
        /// all in `equipment/trial_chamber`.
        inline: Option<Box<LootTable>>,
        /// Draw weight.
        weight: i32,
        /// Luck scaling.
        quality: i32,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:alternatives`: the first child whose conditions pass.
    Alternatives {
        /// The children, in order.
        children: Vec<Self>,
        /// All must pass before any child is considered.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:sequence`: children in order, each rolled until one fails.
    ///
    /// In the enum because the format defines it; **not** exercised by vanilla's own pack
    /// (zero occurrences in 1 326 files), so it is labelled unverified rather than
    /// presented as tested. A roll refuses it.
    Sequence {
        /// The children, in order.
        children: Vec<Self>,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:dynamic`: a drop whose contents come from a block entity.
    Dynamic {
        /// The dynamic source string, e.g. `minecraft:sherds`.
        name: String,
        /// Draw weight.
        weight: i32,
        /// Luck scaling.
        quality: i32,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:tag`: every item in a tag, expanded at roll time.
    Tag {
        /// The tag name.
        name: ResourceId,
        /// Draw weight.
        weight: i32,
        /// Luck scaling.
        quality: i32,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// An entry type this build does not model. The JSON is kept verbatim.
    Raw(serde_json::Value),
}

impl LootEntry {
    /// The entry's type string.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::Item { .. } => "minecraft:item",
            Self::Empty { .. } => "minecraft:empty",
            Self::LootTable { .. } => "minecraft:loot_table",
            Self::Alternatives { .. } => "minecraft:alternatives",
            Self::Sequence { .. } => "minecraft:sequence",
            Self::Dynamic { .. } => "minecraft:dynamic",
            Self::Tag { .. } => "minecraft:tag",
            Self::Raw(value) => raw_type(value, "type"),
        }
    }

    /// The draw weight.
    #[must_use]
    pub const fn weight(&self) -> i32 {
        match self {
            Self::Item { weight, .. }
            | Self::Empty { weight, .. }
            | Self::LootTable { weight, .. }
            | Self::Dynamic { weight, .. }
            | Self::Tag { weight, .. } => *weight,
            // `alternatives` and `sequence` pick the first passing child rather than
            // drawing, so they are never in a weighted pool; 1 keeps the sum well-defined
            // if a pack puts one there anyway.
            Self::Alternatives { .. } | Self::Sequence { .. } | Self::Raw(_) => 1,
        }
    }

    /// The luck scaling, which only the leaf variants carry.
    #[must_use]
    pub const fn quality(&self) -> i32 {
        match self {
            Self::Item { quality, .. }
            | Self::Empty { quality, .. }
            | Self::LootTable { quality, .. }
            | Self::Dynamic { quality, .. }
            | Self::Tag { quality, .. } => *quality,
            Self::Alternatives { .. } | Self::Sequence { .. } | Self::Raw(_) => 0,
        }
    }

    /// The conditions that gate this entry.
    #[must_use]
    pub fn conditions(&self) -> &[LootCondition] {
        match self {
            Self::Item { conditions, .. }
            | Self::Empty { conditions, .. }
            | Self::LootTable { conditions, .. }
            | Self::Alternatives { conditions, .. }
            | Self::Sequence { conditions, .. }
            | Self::Dynamic { conditions, .. }
            | Self::Tag { conditions, .. } => conditions,
            Self::Raw(_) => &[],
        }
    }

    /// The functions this entry applies, which only an `item` entry carries.
    #[must_use]
    pub fn functions(&self) -> &[LootFunction] {
        match self {
            Self::Item { functions, .. } => functions,
            _ => &[],
        }
    }

    /// The children of a branching entry.
    #[must_use]
    pub fn children(&self) -> &[Self] {
        match self {
            Self::Alternatives { children, .. } | Self::Sequence { children, .. } => children,
            _ => &[],
        }
    }
}

/// One pool: `rolls` draws from `entries`, all gated by `conditions`.
#[derive(Debug, Clone, PartialEq)]
pub struct LootPool {
    /// How many draws. Vanilla always states it: 1 383 literals and 76 `uniform` providers.
    pub rolls: CountProvider,
    /// Extra draws, which vanilla always writes as the literal `0.0`.
    pub bonus_rolls: CountProvider,
    /// The alternatives to draw from.
    pub entries: Vec<LootEntry>,
    /// All must pass before the pool rolls at all.
    pub conditions: Vec<LootCondition>,
    /// Applied to everything the pool produced, in order.
    pub functions: Vec<LootFunction>,
}

/// A loot table.
#[derive(Debug, Clone, PartialEq)]
pub struct LootTable {
    /// The table's name, or `None` for a table inlined inside another table.
    pub name: Option<ResourceId>,
    /// The table's `type`.
    pub kind: String,
    /// `random_sequence`, which vanilla states on all 1 326 tables.
    ///
    /// Carried as a string and **not used for anything**. It exists so a caller holding the
    /// world seed can implement positional randomness; this build does not, and does not
    /// pretend to.
    pub random_sequence: Option<String>,
    /// The pools, in file order. Vanilla has 0 to 7.
    pub pools: Vec<LootPool>,
    /// Functions applied to the concatenation of every pool's output, in order.
    ///
    /// Nine vanilla tables use this: `blocks/wheat` and its eight crop siblings.
    pub functions: Vec<LootFunction>,
}

/// Which level of the format a construct sits at.
///
/// The census counts four levels — table, entry, function, condition — and not the count
/// providers nested inside a function argument. Tracking the level as the tree is walked
/// is what keeps [`LootTable::mentioned_types`] the same shape as
/// [`LootLoadReport::modelled`], so the two can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// The table's own `type`.
    Table,
    /// An entry `type`.
    Entry,
    /// A `function`.
    Function,
    /// A `condition`.
    Condition,
    /// A count provider, which the census does not count as a node.
    Provider,
}

impl Level {
    /// Whether the census counts this level.
    const fn counted(self) -> bool {
        !matches!(self, Self::Provider)
    }
}

impl LootTable {
    /// Every table nested inside this one, to any depth.
    #[must_use]
    pub fn nested_tables(&self) -> Vec<&Self> {
        let mut out = Vec::new();
        for pool in &self.pools {
            for entry in &pool.entries {
                collect_nested(entry, &mut out);
            }
        }
        out
    }

    /// Every construct type this table mentions, with how many times.
    ///
    /// The table's own `type`, then every entry, function and condition inside it, nested
    /// tables included. Count providers are deliberately **not** counted here, so summing
    /// this over every table gives exactly [`LootLoadReport::modelled`] plus
    /// [`LootLoadReport::unexecutable`].
    #[must_use]
    pub fn mentioned_types(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        count_table(self, &mut counts);
        counts
    }

    /// Every type this table mentions that [`roll`] cannot execute.
    ///
    /// Equals [`Self::mentioned_types`] minus the modelled types.
    #[must_use]
    pub fn unmodelled_types(&self) -> BTreeMap<String, usize> {
        let mut counts = self.mentioned_types();
        for modelled in MODELLED_TYPES {
            counts.remove(*modelled);
        }
        counts
    }

    /// Every block name this table mentions in a `block_state_property` condition.
    ///
    /// The loader keeps those conditions raw, so it walks the raw JSON to find them;
    /// without that a table could name a block that does not exist and nothing would say
    /// so.
    #[must_use]
    pub fn mentioned_block_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        visit_table_blocks(self, &mut out);
        out
    }
}

/// The type strings [`roll`] executes, so [`LootTable::unmodelled_types`] can subtract
/// them.
const MODELLED_TYPES: &[&str] = &[
    "minecraft:item",
    "minecraft:empty",
    "minecraft:loot_table",
    "minecraft:alternatives",
    "minecraft:set_count",
    "minecraft:limit_count",
    "minecraft:survives_explosion",
    "minecraft:random_chance",
    "minecraft:random_chance_with_enchanted_bonus",
    "minecraft:table_bonus",
    "minecraft:match_tool",
    "minecraft:any_of",
    "minecraft:inverted",
];

fn count_count_provider(provider: &CountProvider, counts: &mut BTreeMap<String, usize>) {
    if Level::Provider.counted() {
        crate::bump(counts, provider.type_name());
    }
}

fn count_table(table: &LootTable, counts: &mut BTreeMap<String, usize>) {
    crate::bump(counts, &table.kind);
    for function in &table.functions {
        count_function(function, counts);
    }
    for pool in &table.pools {
        count_count_provider(&pool.rolls, counts);
        count_count_provider(&pool.bonus_rolls, counts);
        for function in &pool.functions {
            count_function(function, counts);
        }
        for condition in &pool.conditions {
            count_condition(condition, counts);
        }
        for entry in &pool.entries {
            count_entry(entry, counts);
        }
    }
}

fn count_function(function: &LootFunction, counts: &mut BTreeMap<String, usize>) {
    crate::bump(counts, function.type_name());
    if let LootFunction::SetCount { count, .. } = function {
        count_count_provider(count, counts);
    }
}

fn count_condition(condition: &LootCondition, counts: &mut BTreeMap<String, usize>) {
    crate::bump(counts, condition.type_name());
    match condition {
        LootCondition::AnyOf { terms } => {
            for term in terms {
                count_condition(term, counts);
            }
        }
        LootCondition::Inverted { term } => count_condition(term, counts),
        _ => {}
    }
}

fn count_entry(entry: &LootEntry, counts: &mut BTreeMap<String, usize>) {
    crate::bump(counts, entry.type_name());
    for function in entry.functions() {
        count_function(function, counts);
    }
    for condition in entry.conditions() {
        count_condition(condition, counts);
    }
    match entry {
        LootEntry::Alternatives { children, .. } | LootEntry::Sequence { children, .. } => {
            for child in children {
                count_entry(child, counts);
            }
        }
        LootEntry::LootTable {
            inline: Some(table), ..
        } => count_table(table, counts),
        _ => {}
    }
}

fn collect_nested<'a>(entry: &'a LootEntry, out: &mut Vec<&'a LootTable>) {
    match entry {
        LootEntry::LootTable {
            inline: Some(table), ..
        } => {
            out.push(table);
            for pool in &table.pools {
                for child in &pool.entries {
                    collect_nested(child, out);
                }
            }
        }
        LootEntry::Alternatives { children, .. } | LootEntry::Sequence { children, .. } => {
            for child in children {
                collect_nested(child, out);
            }
        }
        _ => {}
    }
}

fn visit_table_blocks(table: &LootTable, out: &mut BTreeSet<String>) {
    for function in &table.functions {
        visit_function_blocks(function, out);
    }
    for pool in &table.pools {
        for condition in &pool.conditions {
            visit_condition_blocks(condition, out);
        }
        for function in &pool.functions {
            visit_function_blocks(function, out);
        }
        for entry in &pool.entries {
            visit_entry_blocks(entry, out);
        }
    }
}

fn visit_entry_blocks(entry: &LootEntry, out: &mut BTreeSet<String>) {
    for condition in entry.conditions() {
        visit_condition_blocks(condition, out);
    }
    for function in entry.functions() {
        visit_function_blocks(function, out);
    }
    match entry {
        LootEntry::Alternatives { children, .. } | LootEntry::Sequence { children, .. } => {
            for child in children {
                visit_entry_blocks(child, out);
            }
        }
        LootEntry::LootTable {
            inline: Some(table), ..
        } => visit_table_blocks(table, out),
        _ => {}
    }
}

fn visit_function_blocks(function: &LootFunction, out: &mut BTreeSet<String>) {
    // A raw function may nest conditions, so its JSON is walked too.
    if let LootFunction::Raw(value) = function {
        collect_blocks(value, out);
    }
}

fn visit_condition_blocks(condition: &LootCondition, out: &mut BTreeSet<String>) {
    match condition {
        LootCondition::Raw(value) => collect_blocks(value, out),
        LootCondition::AnyOf { terms } => {
            for term in terms {
                visit_condition_blocks(term, out);
            }
        }
        LootCondition::Inverted { term } => visit_condition_blocks(term, out),
        _ => {}
    }
}

fn collect_blocks(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("condition").and_then(serde_json::Value::as_str)
                == Some("minecraft:block_state_property")
            {
                if let Some(block) = map.get("block").and_then(serde_json::Value::as_str) {
                    out.insert(block.to_owned());
                }
            }
            for child in map.values() {
                collect_blocks(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_blocks(child, out);
            }
        }
        _ => {}
    }
}

/// The `type`/`function`/`condition` discriminator of a raw construct.
fn raw_type(value: &serde_json::Value, key: &str) -> &str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no discriminator>")
}

/// Roll a table, or say why it cannot be rolled.
///
/// `tables` resolves `minecraft:loot_table` references; pass [`LootTables::new()`] when the
/// table has none.
///
/// # Errors
///
/// [`RollError::Unexecutable`] when the table uses a feature outside the executable subset,
/// [`RollError::UnknownTable`] when a reference cannot be resolved, and
/// [`RollError::TooDeep`] or [`RollError::Cyclic`] for the reference graph. **A refusal is
/// never a partial result**: the caller either gets the complete output or an error.
pub fn roll(
    table: &LootTable,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
) -> Result<Vec<ItemStackLike>, RollError> {
    // Resolve and validate the whole reference graph *before* drawing anything: a missing
    // or cyclic reference must not leave the RNG half-consumed, and proving the graph sound
    // up front is what lets the nested roll below be infallible.
    let mut path = Vec::new();
    let mut visited = BTreeSet::new();
    check_references(table, tables, &mut path, &mut visited, 0)?;

    let mut refusals = Vec::new();
    let mut out = Vec::new();
    roll_into(table, tables, rng, context, &mut refusals, &mut out, 0);
    if refusals.is_empty() {
        Ok(out)
    } else {
        refusals.sort();
        refusals.dedup();
        Err(RollError::Unexecutable { refusals })
    }
}

/// Why a roll could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollError {
    /// The table uses a feature this build does not execute.
    Unexecutable {
        /// Every distinct reason found, sorted and de-duplicated so the message is
        /// deterministic. One run tells the whole story rather than only the first problem.
        refusals: Vec<Refusal>,
    },
    /// A `minecraft:loot_table` entry names a table the caller did not supply.
    UnknownTable {
        /// The name that could not be resolved.
        name: ResourceId,
    },
    /// References nested deeper than [`MAX_TABLE_NESTING`].
    TooDeep {
        /// The chain followed, outermost first.
        path: Vec<String>,
        /// The limit.
        limit: usize,
    },
    /// A table references itself, directly or transitively.
    Cyclic {
        /// The cycle, in order, starting and ending at the same table.
        path: Vec<String>,
    },
}

impl fmt::Display for RollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unexecutable { refusals } => {
                write!(f, "the table cannot be rolled: ")?;
                for (index, refusal) in refusals.iter().enumerate() {
                    if index > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{refusal}")?;
                }
                Ok(())
            }
            Self::UnknownTable { name } => write!(f, "no loot table named {name} was supplied"),
            Self::TooDeep { path, limit } => write!(
                f,
                "loot table references nest deeper than {limit}: {}",
                path.join(" -> ")
            ),
            Self::Cyclic { path } => {
                write!(f, "loot table cycle: {}", path.join(" -> "))
            }
        }
    }
}

impl std::error::Error for RollError {}

/// One reason a roll was refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Refusal {
    /// The kind of construct: `"function"`, `"condition"`, `"entry"`, `"provider"` or
    /// `"loot_table"`.
    pub kind: &'static str,
    /// Its type string.
    pub type_name: String,
    /// Why it is refused.
    pub reason: &'static str,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} ({})", self.kind, self.type_name, self.reason)
    }
}

/// The label a table is known by, for a diagnostic path.
fn table_label(table: &LootTable) -> String {
    table
        .name
        .as_ref()
        .map_or_else(|| "<inline>".to_owned(), ToString::to_string)
}

/// Walk the reference graph once, refusing a missing target, a cycle or too much nesting.
///
/// All three checks happen here rather than during the roll, so the error is the *first*
/// structural problem rather than whichever branch the RNG happened to take.
fn check_references(
    table: &LootTable,
    tables: &LootTables,
    path: &mut Vec<String>,
    visited: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), RollError> {
    if depth > MAX_TABLE_NESTING {
        return Err(RollError::TooDeep {
            path: path.clone(),
            limit: MAX_TABLE_NESTING,
        });
    }
    let label = table_label(table);
    path.push(label.clone());
    for reference in references(table) {
        let nested = match reference {
            Reference::Inline(nested) => nested,
            Reference::Named(name) => {
                let Some(nested) = tables.by_name(name) else {
                    return Err(RollError::UnknownTable { name: name.clone() });
                };
                nested
            }
        };
        let nested_label = table_label(nested);
        if path.contains(&nested_label) {
            let mut cycle = path.clone();
            cycle.push(nested_label);
            return Err(RollError::Cyclic { path: cycle });
        }
        // A table already checked at this or a shallower depth cannot introduce a new cycle
        // on this path, so it is not re-walked; that keeps a diamond reference (which
        // vanilla has, via the trial-chamber reward tables) from being exponential.
        if visited.insert(nested_label) {
            check_references(nested, tables, path, visited, depth + 1)?;
        }
    }
    path.pop();
    Ok(())
}

/// A `minecraft:loot_table` reference, resolved as far as it can be.
enum Reference<'a> {
    /// An inlined table.
    Inline(&'a LootTable),
    /// A named table, which the caller's [`LootTables`] may or may not hold.
    Named(&'a ResourceId),
}

/// Every table reference in one table, in file order.
fn references(table: &LootTable) -> Vec<Reference<'_>> {
    let mut out = Vec::new();
    for pool in &table.pools {
        for entry in &pool.entries {
            collect_references(entry, &mut out);
        }
    }
    out
}

fn collect_references<'a>(entry: &'a LootEntry, out: &mut Vec<Reference<'a>>) {
    match entry {
        LootEntry::LootTable { name, inline, .. } => {
            if let Some(table) = inline {
                out.push(Reference::Inline(table));
            } else if let Some(name) = name {
                out.push(Reference::Named(name));
            }
        }
        LootEntry::Alternatives { children, .. } | LootEntry::Sequence { children, .. } => {
            for child in children {
                collect_references(child, out);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn roll_into(
    table: &LootTable,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
    out: &mut Vec<ItemStackLike>,
    depth: usize,
) {
    if depth > MAX_TABLE_NESTING {
        refusals.push(Refusal {
            kind: "loot_table",
            type_name: format!("nesting deeper than {MAX_TABLE_NESTING} tables"),
            reason: "the nesting guard fired",
        });
        return;
    }
    for function in &table.functions {
        if let Some(reason) = function.refusal_reason() {
            refusals.push(Refusal {
                kind: "function",
                type_name: function.type_name().to_owned(),
                reason,
            });
        }
    }

    let mut produced = Vec::new();
    for pool in &table.pools {
        if !conditions_pass(&pool.conditions, rng, context, refusals) {
            continue;
        }
        for function in &pool.functions {
            if let Some(reason) = function.refusal_reason() {
                refusals.push(Refusal {
                    kind: "function",
                    type_name: function.type_name().to_owned(),
                    reason,
                });
            }
        }
        let rolls = draw_count(&pool.rolls, rng, refusals, "rolls");
        let bonus = draw_count(&pool.bonus_rolls, rng, refusals, "bonus_rolls");
        // Vanilla adds the two as `f32` and truncates the sum, so the rounding is
        // reproduced rather than replaced with an integer sum that rounds differently.
        let total = ((rolls + bonus) as f32) as i32;
        for _ in 0..total.max(0) {
            produced.extend(one_roll(&pool.entries, tables, rng, context, refusals, depth));
        }
    }

    for mut stack in produced {
        apply_functions(&table.functions, &mut stack, rng, refusals);
        out.push(stack);
    }
    // A table function that could not be executed has already been refused, so the output
    // above is discarded by `roll`. Nothing is silently half-applied.
}

/// One draw from a pool: pick a weighted entry, then resolve it.
fn one_roll(
    entries: &[LootEntry],
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
    depth: usize,
) -> Option<ItemStackLike> {
    if entries.is_empty() {
        return None;
    }
    let mut weights = Vec::with_capacity(entries.len());
    let mut total: i64 = 0;
    for entry in entries {
        let mut weight = i64::from(entry.weight());
        if entry.quality() != 0 {
            match context.luck {
                Some(luck) => weight += (luck * entry.quality() as f32).floor() as i64,
                None => refusals.push(Refusal {
                    kind: "entry",
                    type_name: format!("{} with quality", entry.type_name()),
                    reason: "the entry scales with luck and LootContext::luck was not supplied",
                }),
            }
        }
        let weight = weight.max(0);
        total += weight;
        weights.push(weight);
    }
    if total <= 0 {
        return None;
    }
    let draw = (rng.next_f64() * total as f64).floor() as i64;
    // A draw of exactly `total` is impossible for a generator returning `[0, 1)`, but a
    // hostile `Rng` impl could return 1.0; clamping keeps the index in range instead of
    // reading past the end.
    let mut remaining = draw.clamp(0, total - 1);
    let mut chosen = entries.len() - 1;
    for (index, weight) in weights.iter().enumerate() {
        remaining -= weight;
        if remaining < 0 {
            chosen = index;
            break;
        }
    }
    resolve_entry(&entries[chosen], tables, rng, context, refusals, depth)
}

/// Turn a chosen entry into a stack.
fn resolve_entry(
    entry: &LootEntry,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
    depth: usize,
) -> Option<ItemStackLike> {
    if !conditions_pass(entry.conditions(), rng, context, refusals) {
        return None;
    }
    match entry {
        LootEntry::Item {
            name,
            expand,
            functions,
            ..
        } => {
            if *expand {
                refusals.push(Refusal {
                    kind: "entry",
                    type_name: entry.type_name().to_owned(),
                    reason: "the entry expands a tag, which needs the tag set",
                });
                return None;
            }
            let mut stack = ItemStackLike::new(name.clone(), 1);
            apply_functions(functions, &mut stack, rng, refusals);
            Some(stack)
        }
        LootEntry::Empty { .. } => None,
        LootEntry::Dynamic { name, .. } => {
            refusals.push(Refusal {
                kind: "entry",
                type_name: format!("minecraft:dynamic {name}"),
                reason: "the contents come from a block entity, which is not modelled",
            });
            None
        }
        LootEntry::Tag { name, .. } => {
            refusals.push(Refusal {
                kind: "entry",
                type_name: format!("minecraft:tag {name}"),
                reason: "expanding a tag needs the tag set, which is not supplied to a roll",
            });
            None
        }
        LootEntry::Sequence { .. } => {
            refusals.push(Refusal {
                kind: "entry",
                type_name: entry.type_name().to_owned(),
                reason: "minecraft:sequence is not implemented and is not exercised by \
                         vanilla's own pack",
            });
            None
        }
        LootEntry::Alternatives { children, .. } => {
            for child in children {
                if let Some(stack) = resolve_entry(child, tables, rng, context, refusals, depth) {
                    return Some(stack);
                }
            }
            None
        }
        LootEntry::LootTable { name, inline, .. } => {
            // `check_references` has already proved that `inline` is set or `name` resolves.
            let nested = if let Some(table) = inline {
                Some(&**table)
            } else {
                name.as_ref().and_then(|name| tables.by_name(name))
            };
            let Some(nested) = nested else {
                // Unreachable after `check_references`; refused rather than silently
                // producing nothing, because "unreachable" is not a guarantee.
                refusals.push(Refusal {
                    kind: "entry",
                    type_name: entry.type_name().to_owned(),
                    reason: "the referenced table was not supplied",
                });
                return None;
            };
            let mut out = Vec::new();
            roll_into(nested, tables, rng, context, refusals, &mut out, depth + 1);
            out.into_iter().next()
        }
        LootEntry::Raw(_) => {
            refusals.push(Refusal {
                kind: "entry",
                type_name: entry.type_name().to_owned(),
                reason: "this loot entry type is not implemented in this build",
            });
            None
        }
    }
}

/// Whether every condition passes. `false` means "do not roll"; a refusal means "I do not
/// know", which is recorded and makes the whole roll fail.
fn conditions_pass(
    conditions: &[LootCondition],
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
) -> bool {
    let mut pass = true;
    for condition in conditions {
        if evaluate(condition, rng, context, refusals) != Some(true) {
            pass = false;
        }
    }
    pass
}

/// Evaluate one condition. `None` means "this build cannot decide", and a refusal has been
/// recorded.
fn evaluate(
    condition: &LootCondition,
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
) -> Option<bool> {
    match condition {
        LootCondition::SurvivesExplosion => match context.survives_explosion {
            Some(survived) => Some(survived),
            None => {
                refusals.push(Refusal {
                    kind: "condition",
                    type_name: condition.type_name().to_owned(),
                    reason: "LootContext::survives_explosion was not supplied",
                });
                None
            }
        },
        LootCondition::RandomChance { chance } => Some(rng.chance(*chance)),
        LootCondition::RandomChanceWithEnchantedBonus {
            enchantment,
            unenchanted_chance,
            enchanted_chance,
        } => match context.enchantment_level(enchantment) {
            Some(0) => Some(rng.chance(*unenchanted_chance)),
            Some(level) => Some(rng.chance(enchanted_chance.at_level(level))),
            None => {
                refusals.push(Refusal {
                    kind: "condition",
                    type_name: condition.type_name().to_owned(),
                    reason: "LootContext::enchantment_levels was not supplied",
                });
                None
            }
        },
        LootCondition::TableBonus {
            enchantment,
            chances,
        } => {
            if chances.is_empty() {
                refusals.push(Refusal {
                    kind: "condition",
                    type_name: condition.type_name().to_owned(),
                    reason: "the condition lists no chances to index",
                });
                return None;
            }
            match context.enchantment_level(enchantment) {
                Some(level) => {
                    // A negative level is not a level; refusing beats clamping it to 0,
                    // which would silently apply the unenchanted chance.
                    if level < 0 {
                        refusals.push(Refusal {
                            kind: "condition",
                            type_name: condition.type_name().to_owned(),
                            reason: "the enchantment level is negative",
                        });
                        return None;
                    }
                    let index = usize::try_from(level).unwrap_or(0).min(chances.len() - 1);
                    Some(rng.chance(chances[index]))
                }
                None => {
                    refusals.push(Refusal {
                        kind: "condition",
                        type_name: condition.type_name().to_owned(),
                        reason: "LootContext::enchantment_levels was not supplied",
                    });
                    None
                }
            }
        }
        LootCondition::MatchToolEnchantments { requirements } => {
            for requirement in requirements {
                if requirement.enchantment.starts_with('#') {
                    refusals.push(Refusal {
                        kind: "condition",
                        type_name: condition.type_name().to_owned(),
                        reason: "the predicate tests an enchantment tag, which needs the tag set",
                    });
                    return None;
                }
                let Ok(name) = ResourceId::parse(&requirement.enchantment) else {
                    refusals.push(Refusal {
                        kind: "condition",
                        type_name: condition.type_name().to_owned(),
                        reason: "the predicate names an unparseable enchantment",
                    });
                    return None;
                };
                let Some(level) = context.enchantment_level(&name) else {
                    refusals.push(Refusal {
                        kind: "condition",
                        type_name: condition.type_name().to_owned(),
                        reason: "LootContext::enchantment_levels was not supplied",
                    });
                    return None;
                };
                if requirement.min_level.is_some_and(|min| level < min)
                    || requirement.max_level.is_some_and(|max| level > max)
                {
                    return Some(false);
                }
            }
            Some(true)
        }
        LootCondition::AnyOf { terms } => {
            let mut unknown = false;
            for term in terms {
                match evaluate(term, rng, context, refusals) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            if unknown {
                None
            } else {
                Some(false)
            }
        }
        LootCondition::Inverted { term } => evaluate(term, rng, context, refusals).map(|pass| !pass),
        LootCondition::Raw(_) => {
            refusals.push(Refusal {
                kind: "condition",
                type_name: condition.type_name().to_owned(),
                reason: "this loot condition type is not implemented in this build",
            });
            None
        }
    }
}

/// Draw a count from a provider, recording a refusal when it is outside the subset.
fn draw_count(
    provider: &CountProvider,
    rng: &mut impl Rng,
    refusals: &mut Vec<Refusal>,
    slot: &'static str,
) -> f64 {
    match provider {
        CountProvider::Constant(value) => *value,
        CountProvider::Uniform { min, max } => min + rng.next_f64() * (max - min),
        CountProvider::Binomial {
            extra,
            probability,
        } => {
            // The format's own definition, reproduced: one plus the number of successes in
            // `n` independent draws at `p`. Doing the arithmetic beats sampling some other
            // distribution and hoping it matches.
            let rounds = u32::try_from(*extra).unwrap_or(0);
            if rounds > MAX_BINOMIAL_ROUNDS {
                refusals.push(Refusal {
                    kind: "provider",
                    type_name: format!("{slot}: minecraft:binomial n={rounds}"),
                    reason: "more binomial rounds than the draw budget allows",
                });
                return 0.0;
            }
            let mut successes = 0.0;
            for _ in 0..rounds {
                if rng.chance(*probability) {
                    successes += 1.0;
                }
            }
            1.0 + successes
        }
        CountProvider::Raw(_) => {
            refusals.push(Refusal {
                kind: "provider",
                type_name: format!("{slot}: {}", provider.type_name()),
                reason: "this number provider type is not implemented in this build",
            });
            0.0
        }
    }
}

/// Apply functions in order. A function that cannot run is refused and left out; the
/// refusal makes `roll` fail, so the omission is never observable as a result.
fn apply_functions(
    functions: &[LootFunction],
    stack: &mut ItemStackLike,
    rng: &mut impl Rng,
    refusals: &mut Vec<Refusal>,
) {
    for function in functions {
        match function {
            LootFunction::SetCount { count, add } => {
                let value = draw_count(count, rng, refusals, "count");
                // Vanilla truncates the drawn count towards zero before setting it.
                let value = (value as f32) as i32;
                stack.count = if *add {
                    stack.count.saturating_add(value)
                } else {
                    value
                };
            }
            LootFunction::LimitCount { min, max } => {
                if stack.count < *min {
                    stack.count = 0;
                } else if stack.count > *max {
                    stack.count = *max;
                }
            }
            LootFunction::SetDamage { damage, add } => {
                // The draw is still taken, so the refusal is recorded even when the
                // function would have been a no-op.
                let value = draw_count(damage, rng, refusals, "damage").clamp(0.0, 1.0);
                stack.damage = Some(if *add {
                    (stack.damage.unwrap_or(0.0) + value).clamp(0.0, 1.0)
                } else {
                    value
                });
                refusals.push(Refusal {
                    kind: "function",
                    type_name: function.type_name().to_owned(),
                    reason: "minecraft:set_damage converts a fraction into durability points, \
                             which needs the item registry's max-damage table",
                });
            }
            LootFunction::Raw(_) => refusals.push(Refusal {
                kind: "function",
                type_name: function.type_name().to_owned(),
                reason: "this loot function type is not implemented in this build",
            }),
        }
    }
}

/// Every loaded loot table, indexed by name.
#[derive(Debug, Clone, Default)]
pub struct LootTables {
    tables: Vec<LootTable>,
    by_name: BTreeMap<ResourceId, usize>,
}

impl LootTables {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a table. A later table with the same name **replaces** an earlier one, which is
    /// the pack-override rule.
    ///
    /// An unnamed table — an inlined one — is ignored here rather than stored: it has no
    /// registry key, so storing it would make it unreachable *and* make [`Self::len`]
    /// disagree with [`Self::names`].
    pub fn insert(&mut self, table: LootTable) {
        let Some(name) = table.name.clone() else {
            return;
        };
        if let Some(index) = self.by_name.get(&name).copied() {
            self.tables[index] = table;
            return;
        }
        self.by_name.insert(name, self.tables.len());
        self.tables.push(table);
    }

    /// Every table, in load order.
    #[must_use]
    pub fn tables(&self) -> &[LootTable] {
        &self.tables
    }

    /// How many named tables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// A table by name.
    #[must_use]
    pub fn by_name(&self, name: &ResourceId) -> Option<&LootTable> {
        self.by_name.get(name).map(|index| &self.tables[*index])
    }

    /// Every table name, ascending.
    pub fn names(&self) -> impl Iterator<Item = &ResourceId> {
        self.by_name.keys()
    }

    /// Tables whose `type` is `kind`.
    #[must_use]
    pub fn of_kind(&self, kind: &str) -> Vec<&LootTable> {
        self.tables
            .iter()
            .filter(|table| table.kind == kind)
            .collect()
    }

    /// Every table that references `name` through a `minecraft:loot_table` entry.
    #[must_use]
    pub fn referencing(&self, name: &ResourceId) -> Vec<&LootTable> {
        self.tables
            .iter()
            .filter(|table| references(table).iter().any(|r| match r {
                Reference::Named(target) => *target == name,
                Reference::Inline(_) => false,
            }))
            .collect()
    }

    /// Roll a table by name.
    ///
    /// # Errors
    ///
    /// As [`roll`], plus [`RollError::UnknownTable`] when `name` is not loaded.
    pub fn roll_named(
        &self,
        name: &ResourceId,
        rng: &mut impl Rng,
        context: &LootContext,
    ) -> Result<Vec<ItemStackLike>, RollError> {
        let table = self
            .by_name(name)
            .ok_or_else(|| RollError::UnknownTable { name: name.clone() })?;
        roll(table, self, rng, context)
    }
}

/// A name that did not resolve against a registry the caller declared known.
///
/// This is the "forward reference to another mod's content" case, made visible instead of
/// silently accepted or silently rejected.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForwardReference {
    /// Which namespace of content: `"item"` or `"block"`.
    pub registry: &'static str,
    /// The name that was not found.
    pub name: String,
    /// The file that mentioned it.
    pub file: String,
}

/// What a loot-table load did.
///
/// The accounting is the point. For the files in the directory:
///
/// ```text
/// files == loaded + unmodelled.values().sum() + skipped.len()
/// ```
///
/// and, for the constructs *inside* the loaded files, every table `type`, entry `type`,
/// function and condition is counted in exactly one of [`Self::modelled`] or
/// [`Self::unexecutable`]. A dropped file — or a dropped function — makes one of those two
/// sums wrong, which is what [`Self::is_fully_accounted`] and `tests/vanilla_data.rs`
/// check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootLoadReport {
    /// JSON files found in the directory.
    pub files: usize,
    /// Files whose table parsed. A file counts once, however many tables it yields.
    pub loaded: usize,
    /// Table `type`s this build does not model, with how many files used them.
    ///
    /// Vanilla's own pack contributes **zero**: all 11 of its table types are modelled. The
    /// map exists because a pack may write a type this build has never heard of, and
    /// silently ignoring that file is exactly what must not happen.
    pub unmodelled: BTreeMap<String, usize>,
    /// Files that could not be read or parsed, with the reason.
    pub skipped: Vec<String>,
    /// Construct types this build does not execute, with how many occurrences.
    ///
    /// Every one is preserved in the loaded structure as raw JSON; this only counts them,
    /// so "the table loaded, but 147 of its functions cannot run" is a fact a caller can
    /// read rather than discover.
    pub unexecutable: BTreeMap<String, usize>,
    /// Every construct type the loader modelled, with how many occurrences.
    ///
    /// Together with `unexecutable` this is the full census: per level — table, entry,
    /// function, condition — the two maps' entries at that level sum to the total the
    /// survey measured.
    pub modelled: BTreeMap<String, usize>,
    /// Names that did not resolve against a declared-known registry.
    pub forward_references: Vec<ForwardReference>,
    /// Distinct table names loaded, after pack overrides.
    pub tables: usize,
}

impl LootLoadReport {
    /// Files whose table was explicitly counted as unmodelled.
    #[must_use]
    pub fn total_unmodelled_files(&self) -> usize {
        self.unmodelled.values().sum()
    }

    /// Whether every file was accounted for.
    ///
    /// The loader asserts this itself; it is public so a caller can re-check it after
    /// merging reports from several namespaces.
    #[must_use]
    pub fn is_fully_accounted(&self) -> bool {
        self.loaded + self.total_unmodelled_files() + self.skipped.len() == self.files
    }

    /// Total construct occurrences this build does not execute.
    #[must_use]
    pub fn total_unexecutable(&self) -> usize {
        self.unexecutable.values().sum()
    }

    /// How many times a construct type was seen, modelled or not.
    #[must_use]
    pub fn occurrences(&self, type_name: &str) -> usize {
        self.modelled.get(type_name).copied().unwrap_or(0)
            + self.unexecutable.get(type_name).copied().unwrap_or(0)
    }

    /// Occurrences at one level, given the closed set of types at that level.
    #[must_use]
    pub fn occurrences_of(&self, level: &[&str]) -> usize {
        level.iter().map(|kind| self.occurrences(kind)).sum()
    }

    /// Whether nothing was skipped and no table type was unmodelled.
    ///
    /// Note that this can be `true` while `unexecutable` is non-empty: a table can load
    /// completely and still not be rollable. That distinction is deliberate.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.skipped.is_empty() && self.unmodelled.is_empty()
    }
}

/// Load every loot table under `<namespace_dir>/loot_table/`.
///
/// `items` and `blocks`, when supplied, are declared-known registries: a name that does not
/// resolve is recorded in [`LootLoadReport::forward_references`] rather than silently
/// accepted. Passing `None` is legitimate for a pack that references another pack's
/// content, and is reported the same way, so either choice stays visible.
///
/// Never fails as a whole: a malformed file is recorded and skipped, so one bad file in a
/// pack does not discard the rest (AGENTS.md §9).
#[must_use]
pub fn load_directory(
    namespace_dir: &Path,
    namespace: &str,
    limits: Limits,
    items: Option<&mc_registry::ItemRegistry>,
    blocks: Option<&mc_registry::BlockRegistry>,
    report: &mut LootLoadReport,
) -> Vec<LootTable> {
    let base = namespace_dir.join("loot_table");
    let mut out = Vec::new();
    for path in crate::tag::json_files(&base) {
        report.files += 1;
        let Ok(relative) = path.strip_prefix(&base) else {
            report.skipped.push(format!(
                "{}: not under the loot_table directory",
                path.display()
            ));
            continue;
        };
        // `with_extension` returns an owned PathBuf, so bind it before borrowing.
        let stem_path = relative.with_extension("");
        let Some(stem) = stem_path.to_str() else {
            report
                .skipped
                .push(format!("{}: unreadable file name", path.display()));
            continue;
        };
        let stem = stem.replace('\\', "/");
        let Ok(name) = ResourceId::parse(&format!("{namespace}:{stem}")) else {
            report
                .skipped
                .push(format!("{}: unusable loot table name", path.display()));
            continue;
        };
        match read_table_file(&path, Some(name), limits, report, items, 0) {
            Ok(table) => {
                report.loaded += 1;
                if let Some(blocks) = blocks {
                    check_blocks(&table, blocks, &path, report);
                }
                out.push(table);
            }
            Err(Skipped::Unmodelled(kind)) => {
                *report.unmodelled.entry(kind).or_insert(0) += 1;
            }
            Err(Skipped::Parse(error)) => report.skipped.push(error.to_string()),
        }
    }
    report.tables = out.len();
    out
}

/// Record every block name in `table` that a declared-known block registry does not hold.
fn check_blocks(
    table: &LootTable,
    blocks: &mc_registry::BlockRegistry,
    path: &Path,
    report: &mut LootLoadReport,
) {
    for name in table.mentioned_block_names() {
        let known = blocks.contains(&name)
            || name
                .strip_prefix("minecraft:")
                .is_some_and(|bare| blocks.contains(bare));
        if !known {
            report.forward_references.push(ForwardReference {
                registry: "block",
                name,
                file: path.display().to_string(),
            });
        }
    }
    for nested in table.nested_tables() {
        check_blocks(nested, blocks, path, report);
    }
}

/// Why a table file produced no table.
enum Skipped {
    /// A table `type` this build does not model.
    Unmodelled(String),
    /// The file is malformed.
    Parse(JsonError),
}

/// The 11 table `type`s, measured from the jar (see the module table).
pub const TABLE_TYPES: &[&str] = &[
    "minecraft:block",
    "minecraft:entity",
    "minecraft:chest",
    "minecraft:shearing",
    "minecraft:gift",
    "minecraft:archaeology",
    "minecraft:block_interact",
    "minecraft:fishing",
    "minecraft:equipment",
    "minecraft:entity_interact",
    "minecraft:barter",
];

/// The 6 entry `type`s, measured from the jar.
pub const ENTRY_TYPES: &[&str] = &[
    "minecraft:item",
    "minecraft:empty",
    "minecraft:loot_table",
    "minecraft:alternatives",
    "minecraft:dynamic",
    "minecraft:tag",
];

/// The 18 `function` types, measured from the jar.
pub const FUNCTION_TYPES: &[&str] = &[
    "minecraft:set_count",
    "minecraft:explosion_decay",
    "minecraft:enchanted_count_increase",
    "minecraft:copy_components",
    "minecraft:enchant_randomly",
    "minecraft:set_potion",
    "minecraft:enchant_with_levels",
    "minecraft:apply_bonus",
    "minecraft:set_damage",
    "minecraft:furnace_smelt",
    "minecraft:copy_state",
    "minecraft:limit_count",
    "minecraft:set_enchantments",
    "minecraft:set_stew_effect",
    "minecraft:exploration_map",
    "minecraft:set_name",
    "minecraft:set_ominous_bottle_amplifier",
    "minecraft:set_instrument",
];

/// The 12 `condition` types, measured from the jar.
pub const CONDITION_TYPES: &[&str] = &[
    "minecraft:survives_explosion",
    "minecraft:block_state_property",
    "minecraft:match_tool",
    "minecraft:entity_properties",
    "minecraft:any_of",
    "minecraft:table_bonus",
    "minecraft:killed_by_player",
    "minecraft:inverted",
    "minecraft:random_chance",
    "minecraft:random_chance_with_enchanted_bonus",
    "minecraft:damage_source_properties",
    "minecraft:location_check",
];

/// The count-provider `type`s, measured from the jar.
pub const COUNT_PROVIDER_TYPES: &[&str] = &["minecraft:uniform", "minecraft:binomial"];

/// Read one loot-table file.
fn read_table_file(
    path: &Path,
    name: Option<ResourceId>,
    limits: Limits,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
    depth: usize,
) -> Result<LootTable, Skipped> {
    if depth > MAX_ENTRY_DEPTH {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("loot tables nested deeper than {MAX_ENTRY_DEPTH}"),
        }));
    }
    let map = read_json_object(path, limits).map_err(Skipped::Parse)?;
    let kind = required_str(&map, "type", path).map_err(Skipped::Parse)?;
    if !TABLE_TYPES.contains(&kind) {
        return Err(Skipped::Unmodelled(kind.to_owned()));
    }
    crate::bump(&mut report.modelled, kind);
    let random_sequence = optional_str(&map, "random_sequence", path)
        .map_err(Skipped::Parse)?
        .map(str::to_owned);

    let mut functions = Vec::new();
    if let Some(value) = map.get("functions") {
        functions = read_functions(value, path, report)?;
    }

    let mut pools = Vec::new();
    if let Some(value) = map.get("pools") {
        let entries = value.as_array().ok_or_else(|| {
            Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("\"pools\" must be an array, found {}", type_name(value)),
            })
        })?;
        for pool in entries {
            pools.push(read_pool(pool, path, report, items)?);
        }
    }

    Ok(LootTable {
        name,
        kind: kind.to_owned(),
        random_sequence,
        pools,
        functions,
    })
}

fn read_pool(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
) -> Result<LootPool, Skipped> {
    let serde_json::Value::Object(map) = value else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("a pool must be an object, found {}", type_name(value)),
        }));
    };
    // Vanilla always writes `rolls`, and it is the field that decides how much loot
    // appears, so a missing one is refused rather than defaulted.
    let Some(rolls) = map.get("rolls") else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: "a loot pool needs \"rolls\"".to_owned(),
        }));
    };
    let rolls = read_count(rolls, path, report)?;
    let bonus_rolls = match map.get("bonus_rolls") {
        Some(value) => read_count(value, path, report)?,
        None => CountProvider::Constant(0.0),
    };
    let conditions = match map.get("conditions") {
        Some(value) => read_conditions(value, path, report)?,
        None => Vec::new(),
    };
    let functions = match map.get("functions") {
        Some(value) => read_functions(value, path, report)?,
        None => Vec::new(),
    };
    let mut entries = Vec::new();
    if let Some(value) = map.get("entries") {
        let list = value.as_array().ok_or_else(|| {
            Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "pool \"entries\" must be an array, found {}",
                    type_name(value)
                ),
            })
        })?;
        for entry in list {
            entries.push(read_entry(entry, path, report, items, 0)?);
        }
    }
    Ok(LootPool {
        rolls,
        bonus_rolls,
        entries,
        conditions,
        functions,
    })
}

fn read_entry(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
    depth: usize,
) -> Result<LootEntry, Skipped> {
    if depth > MAX_ENTRY_DEPTH {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("loot entries nested deeper than {MAX_ENTRY_DEPTH}"),
        }));
    }
    let serde_json::Value::Object(map) = value else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("a loot entry must be an object, found {}", type_name(value)),
        }));
    };
    let kind = required_str(map, "type", path).map_err(Skipped::Parse)?;
    crate::bump(&mut report.modelled, kind);
    let weight = read_weight(map, path)?;
    let quality = read_quality(map, path)?;
    let conditions = match map.get("conditions") {
        Some(value) => read_conditions(value, path, report)?,
        None => Vec::new(),
    };
    match kind {
        "minecraft:item" => {
            let name = required_str(map, "name", path).map_err(Skipped::Parse)?;
            let name = read_item_name(name, path, report, items)?;
            let functions = match map.get("functions") {
                Some(value) => read_functions(value, path, report)?,
                None => Vec::new(),
            };
            // `expand` is a tag-expansion flag. Vanilla never sets it on an item entry; a
            // pack that does is preserved, and the roll refuses it.
            let expand = match map.get("expand") {
                None => false,
                Some(serde_json::Value::Bool(flag)) => *flag,
                Some(other) => {
                    return Err(Skipped::Parse(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: format!("\"expand\" must be a boolean, found {}", type_name(other)),
                    }));
                }
            };
            Ok(LootEntry::Item {
                name,
                weight,
                quality,
                expand,
                functions,
                conditions,
            })
        }
        "minecraft:empty" => Ok(LootEntry::Empty {
            weight,
            quality,
            conditions,
        }),
        "minecraft:loot_table" => {
            let Some(target) = map.get("value") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "a minecraft:loot_table entry needs \"value\"".to_owned(),
                }));
            };
            let (name, inline) = match target {
                serde_json::Value::String(text) => (Some(read_id(text, "loot_table", path)?), None),
                serde_json::Value::Object(_) => {
                    // An inlined table: vanilla does this three times, all in
                    // `equipment/trial_chamber`. It carries no name, so it cannot be looked
                    // up and is reachable only through its parent.
                    let nested = read_table_map(target, path, report, items, depth + 1)?;
                    (None, Some(Box::new(nested)))
                }
                other => {
                    return Err(Skipped::Parse(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: format!(
                            "a minecraft:loot_table \"value\" must be a name or an object, \
                             found {}",
                            type_name(other)
                        ),
                    }));
                }
            };
            Ok(LootEntry::LootTable {
                name,
                inline,
                weight,
                quality,
                conditions,
            })
        }
        "minecraft:alternatives" | "minecraft:sequence" => {
            let children = read_children(map, path, report, items, depth)?;
            Ok(if kind == "minecraft:alternatives" {
                LootEntry::Alternatives {
                    children,
                    conditions,
                }
            } else {
                LootEntry::Sequence {
                    children,
                    conditions,
                }
            })
        }
        "minecraft:dynamic" => Ok(LootEntry::Dynamic {
            name: required_str(map, "name", path)
                .map_err(Skipped::Parse)?
                .to_owned(),
            weight,
            quality,
            conditions,
        }),
        "minecraft:tag" => {
            let name = required_str(map, "name", path).map_err(Skipped::Parse)?;
            Ok(LootEntry::Tag {
                name: read_id(name, "tag", path)?,
                weight,
                quality,
                conditions,
            })
        }
        other => {
            crate::bump(&mut report.unexecutable, other);
            Ok(LootEntry::Raw(serde_json::Value::Object(map.clone())))
        }
    }
}

fn read_children(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
    depth: usize,
) -> Result<Vec<LootEntry>, Skipped> {
    let children = required_array(map, "children", path).map_err(Skipped::Parse)?;
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        out.push(read_entry(child, path, report, items, depth + 1)?);
    }
    Ok(out)
}

fn read_weight(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<i32, Skipped> {
    match optional_i64(map, "weight", path).map_err(Skipped::Parse)? {
        None => Ok(1),
        // The format's range is 1..=2^31-1. A weight outside it is a pack bug, and refusing
        // beats producing a selection probability nobody can reason about.
        Some(value) if (1..=i64::from(i32::MAX)).contains(&value) => Ok(value as i32),
        Some(value) => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("weight {value} is outside 1..=2147483647"),
        })),
    }
}

fn read_quality(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<i32, Skipped> {
    match optional_i64(map, "quality", path).map_err(Skipped::Parse)? {
        None => Ok(0),
        Some(value) => i32::try_from(value).map_err(|_| {
            Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("quality {value} does not fit an i32"),
            })
        }),
    }
}

fn read_count(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<CountProvider, Skipped> {
    match value {
        serde_json::Value::Number(number) => {
            let Some(as_f64) = number.as_f64() else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: format!("count {number} is not representable"),
                }));
            };
            Ok(CountProvider::Constant(as_f64))
        }
        serde_json::Value::Object(map) => {
            let kind = required_str(map, "type", path).map_err(Skipped::Parse)?;
            match kind {
                "minecraft:uniform" => {
                    let min = required_number(map, "min", path)?;
                    let max = required_number(map, "max", path)?;
                    if min > max {
                        return Err(Skipped::Parse(JsonError::Invalid {
                            path: path.to_path_buf(),
                            reason: format!("a uniform provider needs min <= max, got {min}..{max}"),
                        }));
                    }
                    crate::bump(&mut report.modelled, kind);
                    Ok(CountProvider::Uniform { min, max })
                }
                "minecraft:binomial" => {
                    // The format calls them `n` and `p`; some documents call them `extra` and
                    // `probability`. Accept both rather than silently treating a stated
                    // binomial as a constant.
                    let extra = match optional_i64(map, "n", path).map_err(Skipped::Parse)? {
                        Some(value) => Some(value),
                        None => optional_i64(map, "extra", path).map_err(Skipped::Parse)?,
                    };
                    let probability = match optional_f64(map, "p", path).map_err(Skipped::Parse)? {
                        Some(value) => Some(value),
                        None => optional_f64(map, "probability", path).map_err(Skipped::Parse)?,
                    };
                    let (Some(extra), Some(probability)) = (extra, probability) else {
                        return Err(Skipped::Parse(JsonError::Invalid {
                            path: path.to_path_buf(),
                            reason: "a minecraft:binomial provider needs \"n\" and \"p\""
                                .to_owned(),
                        }));
                    };
                    let extra = i32::try_from(extra).map_err(|_| {
                        Skipped::Parse(JsonError::Invalid {
                            path: path.to_path_buf(),
                            reason: format!("binomial n {extra} does not fit an i32"),
                        })
                    })?;
                    crate::bump(&mut report.modelled, kind);
                    Ok(CountProvider::Binomial {
                        extra,
                        probability: probability as f32,
                    })
                }
                other => {
                    crate::bump(&mut report.unexecutable, other);
                    Ok(CountProvider::Raw(value.clone()))
                }
            }
        }
        other => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "a count must be a number or a provider object, found {}",
                type_name(other)
            ),
        })),
    }
}

fn read_functions(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<Vec<LootFunction>, Skipped> {
    let list = value.as_array().ok_or_else(|| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("\"functions\" must be an array, found {}", type_name(value)),
        })
    })?;
    let mut out = Vec::with_capacity(list.len());
    for function in list {
        out.push(read_function(function, path, report)?);
    }
    Ok(out)
}

fn read_function(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<LootFunction, Skipped> {
    let serde_json::Value::Object(map) = value else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "a loot function must be an object, found {}",
                type_name(value)
            ),
        }));
    };
    let kind = required_str(map, "function", path).map_err(Skipped::Parse)?;
    match kind {
        "minecraft:set_count" => {
            crate::bump(&mut report.modelled, kind);
            let Some(count) = map.get("count") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:set_count needs \"count\"".to_owned(),
                }));
            };
            Ok(LootFunction::SetCount {
                count: read_count(count, path, report)?,
                add: optional_bool(map, "add", path)?,
            })
        }
        "minecraft:limit_count" => {
            crate::bump(&mut report.modelled, kind);
            let Some(serde_json::Value::Object(limit)) = map.get("limit") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:limit_count needs a \"limit\" object".to_owned(),
                }));
            };
            let min = optional_i64(limit, "min", path)
                .map_err(Skipped::Parse)?
                .unwrap_or(0);
            let max = optional_i64(limit, "max", path)
                .map_err(Skipped::Parse)?
                .unwrap_or(i64::from(i32::MAX));
            Ok(LootFunction::LimitCount {
                min: to_i32(min, "limit min", path)?,
                max: to_i32(max, "limit max", path)?,
            })
        }
        "minecraft:set_damage" => {
            crate::bump(&mut report.modelled, kind);
            let Some(damage) = map.get("damage") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:set_damage needs \"damage\"".to_owned(),
                }));
            };
            Ok(LootFunction::SetDamage {
                damage: read_count(damage, path, report)?,
                add: optional_bool(map, "add", path)?,
            })
        }
        other => {
            crate::bump(&mut report.unexecutable, other);
            Ok(LootFunction::Raw(serde_json::Value::Object(map.clone())))
        }
    }
}

fn read_conditions(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<Vec<LootCondition>, Skipped> {
    let list = value.as_array().ok_or_else(|| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("\"conditions\" must be an array, found {}", type_name(value)),
        })
    })?;
    let mut out = Vec::with_capacity(list.len());
    for condition in list {
        out.push(read_condition(condition, path, report)?);
    }
    Ok(out)
}

fn read_condition(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<LootCondition, Skipped> {
    let serde_json::Value::Object(map) = value else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "a loot condition must be an object, found {}",
                type_name(value)
            ),
        }));
    };
    let kind = required_str(map, "condition", path).map_err(Skipped::Parse)?;
    match kind {
        "minecraft:survives_explosion" => {
            crate::bump(&mut report.modelled, kind);
            Ok(LootCondition::SurvivesExplosion)
        }
        "minecraft:random_chance" => {
            crate::bump(&mut report.modelled, kind);
            Ok(LootCondition::RandomChance {
                chance: required_chance(map, "chance", path)?,
            })
        }
        "minecraft:random_chance_with_enchanted_bonus" => {
            crate::bump(&mut report.modelled, kind);
            let enchantment = required_str(map, "enchantment", path).map_err(Skipped::Parse)?;
            Ok(LootCondition::RandomChanceWithEnchantedBonus {
                enchantment: read_id(enchantment, "enchantment", path)?,
                unenchanted_chance: required_chance(map, "unenchanted_chance", path)?,
                enchanted_chance: read_chance_provider(map, path, report)?,
            })
        }
        "minecraft:table_bonus" => {
            crate::bump(&mut report.modelled, kind);
            let enchantment = required_str(map, "enchantment", path).map_err(Skipped::Parse)?;
            let chances = required_array(map, "chances", path).map_err(Skipped::Parse)?;
            if chances.is_empty() {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:table_bonus needs at least one chance".to_owned(),
                }));
            }
            let mut out = Vec::with_capacity(chances.len());
            for chance in chances {
                let Some(number) = chance.as_f64() else {
                    return Err(Skipped::Parse(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: "every table_bonus chance must be a number".to_owned(),
                    }));
                };
                out.push(number as f32);
            }
            Ok(LootCondition::TableBonus {
                enchantment: read_id(enchantment, "enchantment", path)?,
                chances: out,
            })
        }
        "minecraft:match_tool" => {
            crate::bump(&mut report.modelled, kind);
            let Some(serde_json::Value::Object(predicate)) = map.get("predicate") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:match_tool needs a \"predicate\" object".to_owned(),
                }));
            };
            match read_match_tool(predicate, path)? {
                Some(requirements) => Ok(LootCondition::MatchToolEnchantments { requirements }),
                // The predicate tests something a `LootContext` does not carry, so it is kept
                // whole rather than half-checked. Note that the count stays in `modelled`:
                // `match_tool` *is* modelled, this occurrence is just not executable, and the
                // per-table view reports that through `LootTable::unmodelled_types`.
                None => Ok(LootCondition::Raw(serde_json::Value::Object(map.clone()))),
            }
        }
        "minecraft:any_of" => {
            crate::bump(&mut report.modelled, kind);
            let terms = required_array(map, "terms", path).map_err(Skipped::Parse)?;
            let mut out = Vec::with_capacity(terms.len());
            for term in terms {
                out.push(read_condition(term, path, report)?);
            }
            Ok(LootCondition::AnyOf { terms: out })
        }
        "minecraft:inverted" => {
            crate::bump(&mut report.modelled, kind);
            let Some(term) = map.get("term") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:inverted needs \"term\"".to_owned(),
                }));
            };
            Ok(LootCondition::Inverted {
                term: Box::new(read_condition(term, path, report)?),
            })
        }
        other => {
            crate::bump(&mut report.unexecutable, other);
            Ok(LootCondition::Raw(serde_json::Value::Object(map.clone())))
        }
    }
}

fn read_chance_provider(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<ChanceProvider, Skipped> {
    match map.get("enchanted_chance") {
        Some(serde_json::Value::Number(_)) => Ok(ChanceProvider::Constant(required_chance(
            map,
            "enchanted_chance",
            path,
        )?)),
        Some(serde_json::Value::Object(provider)) => {
            let provider_kind = required_str(provider, "type", path).map_err(Skipped::Parse)?;
            if provider_kind != "minecraft:linear" {
                return Err(Skipped::Unmodelled(provider_kind.to_owned()));
            }
            crate::bump(&mut report.modelled, provider_kind);
            let base = optional_f64(provider, "base", path)
                .map_err(Skipped::Parse)?
                .ok_or_else(|| {
                    Skipped::Parse(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: "a minecraft:linear chance needs \"base\"".to_owned(),
                    })
                })?;
            let per_level_above_first = optional_f64(provider, "per_level_above_first", path)
                .map_err(Skipped::Parse)?
                .unwrap_or(0.0);
            Ok(ChanceProvider::Linear {
                base: base as f32,
                per_level_above_first: per_level_above_first as f32,
            })
        }
        other => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "\"enchanted_chance\" must be a number or a provider, found {}",
                other.map_or("nothing", type_name)
            ),
        })),
    }
}

/// Read a `match_tool` predicate, but only when it is *entirely* an enchantment test.
///
/// `None` means the predicate also tests something outside the executable subset.
fn read_match_tool(
    predicate: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<Option<Vec<EnchantmentRequirement>>, Skipped> {
    let Some(serde_json::Value::Object(predicates)) = predicate.get("predicates") else {
        // Vanilla's 178 `match_tool` predicates all use `predicates`; a test written against
        // `items`, `count` or `durability` is not modelled.
        return Ok(None);
    };
    let mut requirements = Vec::new();
    for (key, value) in predicates {
        if key != "minecraft:enchantments" {
            return Ok(None);
        }
        let list = value.as_array().ok_or_else(|| {
            Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: "an enchantments predicate must be an array".to_owned(),
            })
        })?;
        for entry in list {
            let serde_json::Value::Object(entry) = entry else {
                return Ok(None);
            };
            let Some(serde_json::Value::String(name)) = entry.get("enchantments") else {
                return Ok(None);
            };
            let (min_level, max_level) = match entry.get("levels") {
                None => (Some(1), None),
                Some(serde_json::Value::Number(number)) => {
                    let Some(level) = number.as_i64() else {
                        return Ok(None);
                    };
                    let level = i32::try_from(level).unwrap_or(i32::MAX);
                    (Some(level), Some(level))
                }
                Some(serde_json::Value::Object(range)) => (
                    optional_i64(range, "min", path)
                        .map_err(Skipped::Parse)?
                        .map(|value| i32::try_from(value).unwrap_or(i32::MAX)),
                    optional_i64(range, "max", path)
                        .map_err(Skipped::Parse)?
                        .map(|value| i32::try_from(value).unwrap_or(i32::MAX)),
                ),
                Some(_) => return Ok(None),
            };
            requirements.push(EnchantmentRequirement {
                enchantment: name.clone(),
                min_level,
                max_level,
            });
        }
    }
    if requirements.is_empty() {
        return Ok(None);
    }
    Ok(Some(requirements))
}

fn required_number(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<f64, Skipped> {
    let value = optional_f64(map, key, path)
        .map_err(Skipped::Parse)?
        .ok_or_else(|| {
            Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("missing required field {key:?}"),
            })
        })?;
    if !value.is_finite() {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{key:?} must be a finite number"),
        }));
    }
    Ok(value)
}

fn required_chance(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<f32, Skipped> {
    Ok(required_number(map, key, path)? as f32)
}

fn to_i32(value: i64, what: &str, path: &Path) -> Result<i32, Skipped> {
    i32::try_from(value).map_err(|_| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{what} of {value} does not fit an i32"),
        })
    })
}

fn optional_bool(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<bool, Skipped> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(flag)) => Ok(*flag),
        Some(other) => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("field {key:?} must be a boolean, found {}", type_name(other)),
        })),
    }
}

/// Read an item name, recording it when a declared-known item registry lacks it.
fn read_item_name(
    text: &str,
    path: &Path,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
) -> Result<ResourceId, Skipped> {
    let id = read_id(text, "item", path)?;
    if let Some(items) = items {
        if items.id(&id.to_string()).is_err() && items.id(id.value()).is_err() {
            report.forward_references.push(ForwardReference {
                registry: "item",
                name: id.to_string(),
                file: path.display().to_string(),
            });
        }
    }
    Ok(id)
}

fn read_id(text: &str, registry: &str, path: &Path) -> Result<ResourceId, Skipped> {
    ResourceId::parse(text).map_err(|error| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{registry} name {text:?} is not a valid resource id: {error}"),
        })
    })
}

/// Read an inlined loot table, which has the same shape as a file but no name.
fn read_table_map(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
    items: Option<&mc_registry::ItemRegistry>,
    depth: usize,
) -> Result<LootTable, Skipped> {
    if depth > MAX_ENTRY_DEPTH {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("inlined loot tables nested deeper than {MAX_ENTRY_DEPTH}"),
        }));
    }
    let serde_json::Value::Object(map) = value else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "an inlined loot table must be an object, found {}",
                type_name(value)
            ),
        }));
    };
    // An inlined table needs no `type`: vanilla's `equipment/trial_chamber` inlines tables
    // that carry only `pools`.
    let kind = match map.get("type") {
        Some(serde_json::Value::String(text)) => text.clone(),
        None => "minecraft:inline".to_owned(),
        Some(other) => {
            return Err(Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "an inlined table's \"type\" must be a string, found {}",
                    type_name(other)
                ),
            }));
        }
    };
    crate::bump(&mut report.modelled, &kind);
    let mut functions = Vec::new();
    if let Some(value) = map.get("functions") {
        functions = read_functions(value, path, report)?;
    }
    let mut pools = Vec::new();
    if let Some(serde_json::Value::Array(list)) = map.get("pools") {
        for pool in list {
            pools.push(read_pool(pool, path, report, items)?);
        }
    }
    Ok(LootTable {
        name: None,
        kind,
        random_sequence: None,
        pools,
        functions,
    })
}

#[cfg(test)]
mod tests;
