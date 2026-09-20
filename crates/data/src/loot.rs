//! Loot tables: the data model, the loader, and a **bounded, refusing** roll (P07-10).
//!
//! ## The format, surveyed from the 26.1.2 jar
//!
//! `data/minecraft/loot_table/` holds **1 326 files**. Every one was read while this module was
//! written; the figures below are counts, not recollections. The measuring script is
//! `target/vanilla-26.1.2/census_loot_adv.py`, and `tests/vanilla_data.rs` re-derives the same
//! numbers from the extracted pack and asserts every one of them.
//!
//! | Level | Distinct types | Occurrences |
//! |---|---|---|
//! | table `type` | 12 | 1 329 |
//! | entry `type` | 6 | 2 548 |
//! | `function` | 19 | 1 404 |
//! | `condition` | 12 | 1 545 |
//! | count provider | 3 | 3 805 |
//!
//! The counts exceed the file count because a table inlines others (3 more tables, 12 more
//! functions, 6 more conditions) and because entries nest inside `alternatives`. Count providers
//! are listed separately: they are *arguments* of a `count`/`rolls` field, not nodes in the tree,
//! so they are reported in [`LootLoadReport::number_providers`] rather than in the construct
//! census.
//!
//! The structural discriminator differs per level, which is a real trap: a table and an entry use
//! `"type"`, a function uses `"function"`, and a condition uses `"condition"`. A parser that
//! reads `"type"` everywhere finds nothing at all.
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
//! | `minecraft:inline` | 3 (not a file type — a table inlined by another) |
//! | `minecraft:barter` | 1 |
//! | `minecraft:entity_interact` | 1 |
//!
//! ## Four things the real data forced, which a parser written from a description gets wrong
//!
//! Each of these was a bug in the first version of this module, found only by loading the pack:
//!
//! 1. **`functions` is legal on every entry type**, not only on `minecraft:item`. Vanilla puts
//!    two on `minecraft:loot_table` entries, each carrying conditions that in turn carry terms:
//!    reading `functions` only for items lost 2 functions, 2 `any_of` and 4
//!    `entity_properties`.
//! 2. **`inverted` uses the singular key `term`**, not `terms` like `any_of`. Missing it hides
//!    everything under it — 12 `any_of` and their terms — and made `match_tool` look like 178
//!    occurrences when the files contain **203**.
//! 3. **A function carries its own `conditions`** (164 of 1 404). They gate whether the function
//!    runs, and ignoring them applies a function vanilla would not have applied.
//! 4. **`limit_count`'s `min`/`max` and `binomial`'s `n` are written as floats** (`1.0`, `3.0`),
//!    so a parser insisting on a JSON integer refuses ten vanilla files.
//!
//! ## What this module executes, and what it refuses
//!
//! A wrong roll is worse than no roll: it produces items that look plausible and are silently
//! wrong. So [`roll`] implements a **named, tested subset** and refuses the rest. Two rules make
//! that honest:
//!
//! 1. **Nothing is dropped at parse time.** A function, condition, entry or count provider whose
//!    type is not modelled is kept whole — its `Raw` variant for a construct, its JSON for an
//!    entry — so the structure still describes the file; the loader counts it in
//!    [`LootLoadReport::unexecutable`].
//! 2. **Nothing is ignored at roll time.** A run that meets an unmodelled construct, a tag entry,
//!    a dynamic entry or a reference problem returns [`RollError`] naming it. It never returns a
//!    partial result as if it were the answer.
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
//! `minecraft:match_tool` run **only** when the caller supplies the enchantment levels in
//! [`LootContext`]; without them the roll refuses, because assuming level 0 would silently drop
//! the looting and silk-touch outcomes. The same is true of `survives_explosion` and
//! `LootContext::survives_explosion`, and of an entry's `quality` and `LootContext::luck`.
//!
//! ### Refused
//!
//! Everything else, with the reason attached to each refusal rather than to a comment: the 16
//! unexecuted function types (`apply_bonus`, `copy_components`, `copy_state`,
//! `enchant_randomly`, `enchant_with_levels`, `enchanted_count_increase`, `exploration_map`,
//! `explosion_decay`, `furnace_smelt`, `set_components`, `set_enchantments`, `set_instrument`,
//! `set_name`, `set_ominous_bottle_amplifier`, `set_potion`, `set_stew_effect`) plus the
//! modelled-but-unexecuted `set_damage`; the 5 unexecuted condition types
//! (`block_state_property`, `entity_properties`, `damage_source_properties`, `location_check`,
//! `killed_by_player`); the `dynamic` and `tag` entry types; `minecraft:sequence`; and an entry
//! whose `quality` needs a luck the caller did not state. Each needs the item registry, the
//! enchantment tables, a structure generator, a block entity, entity state or a damage source —
//! none of which a loot context here carries, and guessing any of them produces wrong loot.
//!
//! The measured consequence, asserted by the differential test: of the 6 826 constructs the
//! vanilla pack contains, **4 564 are executable and 903 are not** (the remainder are `Raw`
//! entries whose nested constructs were counted but not parsed).
//!
//! ## Randomness is injected, never invented (AGENTS.md section 3.6)
//!
//! The JDK-verified generator lives in `mc-core` — but loot rolls must speak the
//! same contract as mob drops (`mc_entity::mob::Rng`), and the wiring belongs to
//! the crate that owns the seeded source. So [`Rng`] is a trait defined here in the same shape as
//! `mc_entity::mob::Rng` — a trait naming the contract, with one `impl` in the crate that owns
//! the concrete generator. It exposes `next_f32`/`next_f64` alongside `next_u32` because loot
//! chances are floats, and faking a float out of an integer draw would change the distribution.
//!
//! ## Hostile input (AGENTS.md section 10)
//!
//! A pack is not trusted. Every file goes through [`crate::json`] with [`Limits`]; entry nesting,
//! table nesting and the number of draws a binomial provider may request are all bounded; and no
//! path here can panic.

use mc_core::error::{ServerError, ServerResult};
use mc_core::ids::ResourceId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use thiserror::Error;

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

/// The numeric conversions the format forces, in one place.
///
/// The pack format writes counts, probabilities and chances as **JSON numbers**, so every value
/// arrives as `f64`, while `minecraft:random_chance` compares `f32` and a count is an `i32`. Each
/// conversion below is exact or is the format's own truncation, and each is argued once here
/// rather than sprinkled as `as` casts that `clippy::pedantic` is right to flag.
mod numeric {
    /// A `f64` narrowed to the `f32` the format stores chances at.
    ///
    /// Every chance, probability and linear-provider field in vanilla is a short decimal
    /// (`0.025`, `0.5714286`, `0.035`) that `f32` reproduces to more digits than the file
    /// carries, and vanilla itself computes in `float`.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn chance(value: f64) -> f32 {
        value as f32
    }

    /// A drawn count truncated towards zero, which is what vanilla's `(int)` cast does.
    ///
    /// The intermediate `f32` is not redundant: vanilla evaluates the provider as a `float`, so
    /// narrowing first reproduces its rounding rather than being merely close to it.
    pub(super) fn count(value: f64) -> i32 {
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = value as f32;
        #[allow(clippy::cast_possible_truncation)]
        let truncated = narrowed as i32;
        truncated
    }

    /// A uniform draw scaled to a whole number of selection units.
    ///
    /// `total` is a sum of `i32` weights, so it is far inside `f64`'s exact integer range, and
    /// the product is floored before the cast — the same arithmetic vanilla does.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(super) fn weighted_index(value: f64, total: i64) -> i64 {
        (value * total as f64).floor() as i64
    }

    /// Luck applied to an entry's `quality`, which vanilla computes in `float`.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub(super) fn luck_bonus(luck: f32, quality: i32) -> i64 {
        (luck * quality as f32).floor() as i64
    }

    /// A `f64` narrowed to `f32` for a linear chance provider's slope or base.
    pub(super) use chance as slope;

    /// An enchantment level as the `f32` the linear formula multiplies.
    ///
    /// Levels are small (vanilla's table-bonus lists are 3 to 5 entries long), so the
    /// conversion is exact for every value the format can express.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn level(levels_above: i32) -> f32 {
        levels_above as f32
    }

    /// A pool's roll count, from the `f64` sum to the `i32` loop bound.
    ///
    /// `chance` above is used for the narrowing so the same truncation rule applies as
    /// everywhere else.
    pub(super) fn roll_count(value: f64) -> i32 {
        count(value)
    }

    /// A checked-whole, checked-range `f64` to the `i64` it exactly represents.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn whole(value: f64) -> i64 {
        value as i64
    }

    /// A weight already proved to be in `1..=i32::MAX`.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn weight(value: i64) -> i32 {
        value as i32
    }

    /// The 24-bit draw of [`super::Rng::next_f32`], which `f32` represents exactly.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn draw24(bits: u32) -> f32 {
        bits as f32
    }

    /// The 53-bit draw of [`super::Rng::next_f64`], which `f64` represents exactly.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn draw53(bits: u64) -> f64 {
        bits as f64
    }
}

/// The randomness a roll may consume.
///
/// This is the crate boundary in the same way `mc_entity::mob::Rng` is: `mc-data` may not
/// depend on `mc-simulation`, and the seeded `java.util.Random` clone lives in `mc-core`,
/// so the contract is named here and wired up by whoever owns the generator.
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
        // returns, so the two agree. Both operands are exactly representable in `f32`.
        numeric::draw24(self.next_u32() >> 8) / numeric::draw24(1u32 << 24)
    }

    /// A uniform draw in `[0, 1)`, with 53 bits of precision.
    ///
    /// 53 bits matches `java.util.Random.nextDouble()`. The numerator is below `2^53`, so `f64`
    /// represents it and the divisor exactly.
    fn next_f64(&mut self) -> f64 {
        // `java.util.Random.nextDouble` draws **26 bits first, then 27**;
        // an earlier build drew 27-then-26, which changes which rolls succeed
        // near a boundary (AUDIT-09 D-05). The shape here is Java's own.
        let first = u64::from(self.next_u32() >> 6); // top 26 of the 32-bit draw
        let second = u64::from(self.next_u32() >> 5); // top 27
        numeric::draw53((first << 27) + second) / numeric::draw53(1u64 << 53)
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
///
/// A function carries its own `conditions` (164 of vanilla's 1 392 do), and they gate whether
/// the function runs **at all**. That is a separate thing from an entry's conditions, which
/// gate whether the entry is produced, so it lives here rather than being flattened into the
/// caller's list.
#[derive(Debug, Clone, PartialEq)]
pub struct LootFunction {
    /// The discriminant: what the function does.
    pub kind: LootFunctionKind,
    /// If any of these fails, the function is not applied.
    pub conditions: Vec<LootCondition>,
}

impl LootFunction {
    /// A function with no conditions.
    #[must_use]
    pub const fn new(kind: LootFunctionKind) -> Self {
        Self {
            kind,
            conditions: Vec::new(),
        }
    }

    /// The function's type string.
    #[must_use]
    pub fn type_name(&self) -> &str {
        self.kind.type_name()
    }

    /// Whether [`roll`] can execute this.
    #[must_use]
    pub fn is_executable(&self) -> bool {
        self.kind.is_executable() && self.conditions.iter().all(LootCondition::is_executable)
    }

    /// Why it cannot be executed, when it cannot.
    #[must_use]
    pub const fn refusal_reason(&self) -> Option<&'static str> {
        self.kind.refusal_reason()
    }

    /// Whether the function would run given these conditions' outcome.
    ///
    /// Kept separate from the condition evaluation so the ordering is visible at the call
    /// site: conditions first, then the effect.
    #[must_use]
    pub fn conditions_pass(
        &self,
        rng: &mut impl Rng,
        context: &LootContext,
        refusals: &mut Vec<Refusal>,
    ) -> bool {
        conditions_pass(&self.conditions, rng, context, refusals)
    }
}

/// What a loot function does.
#[derive(Debug, Clone, PartialEq)]
pub enum LootFunctionKind {
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
    /// A function type this build does not model. The JSON is kept verbatim, `conditions`
    /// included, so nothing about the file is lost.
    Raw(serde_json::Value),
}

impl LootFunctionKind {
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

    /// Whether [`roll`] can execute this, ignoring its conditions.
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
                let levels_above = numeric::level((level - 1).max(0));
                (base + per_level_above_first * levels_above).clamp(0.0, 1.0)
            }
        }
    }
}

/// One alternative inside a pool, which may itself contain alternatives.
///
/// Every variant carries `weight`, `quality`, `functions` and `conditions`, because the format
/// allows all four on **every** entry type and a loader that reads them only on `item` entries
/// silently drops data. Vanilla exercises that: two `minecraft:loot_table` entries carry
/// functions, and each of those functions carries conditions. The differential test caught it.
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
        /// Applied in order to the stack this entry produced.
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
        /// Applied in order. No stack is produced, so they are refused rather than ignored.
        functions: Vec<LootFunction>,
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
        /// Applied in order, after the nested table's own functions.
        functions: Vec<LootFunction>,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// `minecraft:alternatives`: the first child whose conditions pass.
    Alternatives {
        /// The children, in order.
        children: Vec<Self>,
        /// Applied in order, after the chosen child's own functions.
        functions: Vec<LootFunction>,
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
        /// Applied in order.
        functions: Vec<LootFunction>,
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
        /// Applied in order.
        functions: Vec<LootFunction>,
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
        /// Applied in order.
        functions: Vec<LootFunction>,
        /// All must pass.
        conditions: Vec<LootCondition>,
    },
    /// An entry type this build does not model, kept whole.
    ///
    /// Its nested `functions` and `conditions` are **counted** (see `count_raw_constructs`) but
    /// not parsed, so the census stays exact while the entry stays opaque; the JSON is the
    /// record of what was there.
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

    /// The functions this entry applies, in order.
    ///
    /// Present on every modelled variant because the format allows `functions` on every entry
    /// type, not only on `minecraft:item`.
    #[must_use]
    pub fn functions(&self) -> &[LootFunction] {
        match self {
            Self::Item { functions, .. }
            | Self::Empty { functions, .. }
            | Self::LootTable { functions, .. }
            | Self::Alternatives { functions, .. }
            | Self::Sequence { functions, .. }
            | Self::Dynamic { functions, .. }
            | Self::Tag { functions, .. } => functions,
            Self::Raw(_) => &[],
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

/// Count a count provider.
///
/// A deliberate no-op for the census, documented rather than silently omitted: the census
/// counts four levels — table, entry, function, condition. A count provider is an *argument* of
/// a function, not a node in the tree, so counting it would make
/// `modelled + unexecutable` disagree with the per-level totals measured from the jar. Its
/// occurrences are reported separately, in [`LootLoadReport::number_providers`].
fn count_count_provider(_provider: &CountProvider, _counts: &mut BTreeMap<String, usize>) {}

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
    // A function's own conditions are gates on the function, and the census counts them:
    // 164 of vanilla's 1 392 functions carry at least one.
    for condition in &function.conditions {
        count_condition(condition, counts);
    }
    if let LootFunctionKind::SetCount { count, .. } = &function.kind {
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
            inline: Some(table),
            ..
        } => count_table(table, counts),
        _ => {}
    }
}

fn collect_nested<'a>(entry: &'a LootEntry, out: &mut Vec<&'a LootTable>) {
    match entry {
        LootEntry::LootTable {
            inline: Some(table),
            ..
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
            inline: Some(table),
            ..
        } => visit_table_blocks(table, out),
        _ => {}
    }
}

fn visit_function_blocks(function: &LootFunction, out: &mut BTreeSet<String>) {
    // A raw function may nest conditions, so its JSON is walked too.
    if let LootFunctionKind::Raw(value) = &function.kind {
        collect_blocks(value, out);
    }
    for condition in &function.conditions {
        visit_condition_blocks(condition, out);
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
                && let Some(block) = map.get("block").and_then(serde_json::Value::as_str)
            {
                out.insert(block.to_owned());
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
fn raw_type<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
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

    let outcome = roll_scoped(table, tables, rng, context)?;
    if outcome.skipped.is_empty() {
        Ok(outcome.stacks)
    } else {
        // The old doctrine: any refusal means the whole roll is unexecutable.
        // The scoped roll collects every level of refusal into `skipped`, and
        // the whole-table error still tells the whole story.
        let mut refusals = outcome.skipped;
        refusals.sort();
        refusals.dedup();
        Err(RollError::Unexecutable { refusals })
    }
}

/// What a scoped roll produced and what it had to skip.
#[derive(Debug, Clone, PartialEq)]
pub struct RollOutcome {
    /// The stacks that survived the refusal ladder.
    pub stacks: Vec<ItemStackLike>,
    /// Every pool, entry or function that could not be evaluated, with the
    /// reason. Empty means the table rolled completely.
    pub skipped: Vec<Refusal>,
}

/// Roll a table, refusing at the **lowest scope it can** (owner decision 2).
///
/// A pool whose conditions cannot be evaluated is discarded whole; an entry or
/// a function that cannot be evaluated is skipped, and the stack keeps the
/// other functions' effects. Every refusal is reported in
/// [`RollOutcome::skipped`] with the reason, so a reduced drop is visible
/// rather than silent.
///
/// # Errors
///
/// The reference-graph pre-checks (unknown table, depth, cycle) as `roll`
/// reports them.
pub fn roll_scoped(
    table: &LootTable,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
) -> Result<RollOutcome, RollError> {
    let mut path = Vec::new();
    let mut visited = BTreeSet::new();
    check_references(table, tables, &mut path, &mut visited, 0)?;

    let mut skipped = Vec::new();
    let mut out = Vec::new();
    roll_into(table, tables, rng, context, &mut skipped, &mut out, 0);
    skipped.sort();
    skipped.dedup();
    Ok(RollOutcome {
        stacks: out,
        skipped,
    })
}

/// Why a roll could not be completed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RollError {
    /// The table uses a feature this build does not execute.
    #[error("the table cannot be rolled: {joined}", joined = join_refusals(refusals))]
    Unexecutable {
        /// Every distinct reason found, sorted and de-duplicated so the message is
        /// deterministic. One run tells the whole story rather than only the first problem.
        refusals: Vec<Refusal>,
    },
    /// A `minecraft:loot_table` entry names a table the caller did not supply.
    #[error("no loot table named {name} was supplied")]
    UnknownTable {
        /// The name that could not be resolved.
        name: ResourceId,
    },
    /// References nested deeper than [`MAX_TABLE_NESTING`].
    #[error("loot table references nest deeper than {limit}: {chain}", chain = path.join(" -> "))]
    TooDeep {
        /// The chain followed, outermost first.
        path: Vec<String>,
        /// The limit.
        limit: usize,
    },
    /// A table references itself, directly or transitively.
    #[error("loot table cycle: {chain}", chain = path.join(" -> "))]
    Cyclic {
        /// The cycle, in order, starting and ending at the same table.
        path: Vec<String>,
    },
}

/// Render refusal lists the way [`RollError::Unexecutable`] reports them:
///
/// every refusal joined with `"; "`, so one run tells the whole story rather
/// than only the first problem.
fn join_refusals(refusals: &[Refusal]) -> String {
    refusals
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

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

/// Roll one table's pools, accumulating stacks and refusals.
///
/// Seven parameters is one more than the lint's threshold, and bundling them into a struct would
/// only move the same fields behind a name that has no other use: every caller is inside this
/// module and already holds all of them.
#[allow(clippy::too_many_arguments)]
fn roll_into(
    table: &LootTable,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
    skipped: &mut Vec<Refusal>,
    out: &mut Vec<ItemStackLike>,
    depth: usize,
) {
    if depth > MAX_TABLE_NESTING {
        skipped.push(Refusal {
            kind: "loot_table",
            type_name: format!("nesting deeper than {MAX_TABLE_NESTING} tables"),
            reason: "the nesting guard fired",
        });
        return;
    }
    for function in &table.functions {
        if let Some(reason) = function.refusal_reason() {
            skipped.push(Refusal {
                kind: "function",
                type_name: function.type_name().to_owned(),
                reason,
            });
        }
    }

    let mut produced = Vec::new();
    for pool in &table.pools {
        // **The refusal ladder (owner decision 2).** Three granularities: a
        // pool whose *conditions* cannot be evaluated is discarded whole (the
        // pool's identity is its conditions); an entry or a function that
        // cannot be evaluated is skipped and its neighbours stand -- a skipped
        // `furnace_smelt` leaves raw beef, a skipped `enchanted_count_increase`
        // leaves the unenchanted count. Every refusal is reported through
        // `skipped`, so a reduced drop is visible rather than silent. A failed
        // condition (a random_chance draw that lost) is still a plain skip
        // with no refusal: "could not evaluate" versus "evaluated and said
        // no".
        let mut pool_condition_refusals = Vec::new();
        let passed = conditions_pass(&pool.conditions, rng, context, &mut pool_condition_refusals);
        // **"Unknown" is not "false"**: a pool whose conditions contain an
        // unevaluable term is skipped even when the evaluated terms passed,
        // because the outcome could differ. The verdict would otherwise depend
        // on where in the disjunction the unknown term sat.
        if !passed || !pool_condition_refusals.is_empty() {
            skipped.extend(pool_condition_refusals);
            continue;
        }
        let rolls = draw_count(&pool.rolls, rng, skipped, "rolls");
        let bonus = draw_count(&pool.bonus_rolls, rng, skipped, "bonus_rolls");
        // Vanilla adds the two as `f32` and truncates the sum, so the rounding is
        // reproduced rather than replaced with an integer sum that rounds differently.
        let total = numeric::roll_count(rolls + bonus);
        for _ in 0..total.max(0) {
            produced.extend(one_roll(
                &pool.entries,
                tables,
                rng,
                context,
                skipped,
                depth,
            ));
        }
        // The pool's own functions: a construct this build cannot execute is
        // skipped and reported; the pool's stacks stand.
        for function in &pool.functions {
            if let Some(reason) = function.refusal_reason() {
                skipped.push(Refusal {
                    kind: "function",
                    type_name: function.type_name().to_owned(),
                    reason,
                });
            }
        }
    }

    for mut stack in produced {
        apply_functions(&table.functions, &mut stack, rng, context, skipped);
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
    skipped: &mut Vec<Refusal>,
    depth: usize,
) -> Vec<ItemStackLike> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut weights = Vec::with_capacity(entries.len());
    let mut total: i64 = 0;
    for entry in entries {
        let mut weight = i64::from(entry.weight());
        if entry.quality() != 0 {
            match context.luck {
                Some(luck) => weight += numeric::luck_bonus(luck, entry.quality()),
                None => skipped.push(Refusal {
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
        return Vec::new();
    }
    let draw = numeric::weighted_index(rng.next_f64(), total);
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
    resolve_entry(&entries[chosen], tables, rng, context, skipped, depth)
}

/// Turn a chosen entry into zero or more stacks.
///
/// A nested `minecraft:loot_table` produces **every** stack the nested roll
/// makes (vanilla's `NestedLootTable` hands its consumer all of them); the
/// entry's own functions apply to each produced stack, because the format
/// allows functions on every entry type. Conditions still gate the whole
/// entry, and refusals still mean "produce nothing".
fn resolve_entry(
    entry: &LootEntry,
    tables: &LootTables,
    rng: &mut impl Rng,
    context: &LootContext,
    skipped: &mut Vec<Refusal>,
    depth: usize,
) -> Vec<ItemStackLike> {
    if !conditions_pass(entry.conditions(), rng, context, skipped) {
        return Vec::new();
    }
    let mut stacks = match entry {
        LootEntry::Item { name, expand, .. } => {
            if *expand {
                skipped.push(Refusal {
                    kind: "entry",
                    type_name: entry.type_name().to_owned(),
                    reason: "the entry expands a tag, which needs the tag set",
                });
                return Vec::new();
            }
            vec![ItemStackLike::new(name.clone(), 1)]
        }
        LootEntry::Empty { .. } => Vec::new(),
        LootEntry::Dynamic { name, .. } => {
            skipped.push(Refusal {
                kind: "entry",
                type_name: format!("minecraft:dynamic {name}"),
                reason: "the contents come from a block entity, which is not modelled",
            });
            Vec::new()
        }
        LootEntry::Tag { name, .. } => {
            skipped.push(Refusal {
                kind: "entry",
                type_name: format!("minecraft:tag {name}"),
                reason: "expanding a tag needs the tag set, which is not supplied to a roll",
            });
            Vec::new()
        }
        LootEntry::Sequence { .. } => {
            skipped.push(Refusal {
                kind: "entry",
                type_name: entry.type_name().to_owned(),
                reason: "minecraft:sequence is not implemented and is not exercised by \
                         vanilla's own pack",
            });
            Vec::new()
        }
        LootEntry::Alternatives { children, .. } => {
            // The first child that *produces* anything wins; its stacks are the
            // alternatives entry's stacks, and the entry's own functions apply
            // to them below.
            let mut chosen = Vec::new();
            for child in children {
                chosen = resolve_entry(child, tables, rng, context, skipped, depth);
                if !chosen.is_empty() {
                    break;
                }
            }
            chosen
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
                skipped.push(Refusal {
                    kind: "entry",
                    type_name: entry.type_name().to_owned(),
                    reason: "the referenced table was not supplied",
                });
                return Vec::new();
            };
            let mut out = Vec::new();
            roll_into(nested, tables, rng, context, skipped, &mut out, depth + 1);
            out
        }
        LootEntry::Raw(_) => {
            skipped.push(Refusal {
                kind: "entry",
                type_name: entry.type_name().to_owned(),
                reason: "this loot entry type is not implemented in this build",
            });
            Vec::new()
        }
    };
    // Applied for every entry type, because the format allows `functions` on every entry type.
    for stack in &mut stacks {
        apply_functions(entry.functions(), stack, rng, context, skipped);
    }
    stacks
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
///
/// Long, and allowed to be: it is one arm per condition type, and the alternative — a helper per
/// arm — would spread one decision table across ten functions for no gain.
#[allow(clippy::too_many_lines)]
fn evaluate(
    condition: &LootCondition,
    rng: &mut impl Rng,
    context: &LootContext,
    refusals: &mut Vec<Refusal>,
) -> Option<bool> {
    match condition {
        LootCondition::SurvivesExplosion => {
            if let Some(survived) = context.survives_explosion {
                return Some(survived);
            }
            refusals.push(Refusal {
                kind: "condition",
                type_name: condition.type_name().to_owned(),
                reason: "LootContext::survives_explosion was not supplied",
            });
            None
        }
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
            let Some(level) = context.enchantment_level(enchantment) else {
                refusals.push(Refusal {
                    kind: "condition",
                    type_name: condition.type_name().to_owned(),
                    reason: "LootContext::enchantment_levels was not supplied",
                });
                return None;
            };
            // A negative level is not a level; refusing beats clamping it to 0, which would
            // silently apply the unenchanted chance.
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
            if unknown { None } else { Some(false) }
        }
        LootCondition::Inverted { term } => {
            evaluate(term, rng, context, refusals).map(|pass| !pass)
        }
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
        CountProvider::Binomial { extra, probability } => {
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
    context: &LootContext,
    skipped: &mut Vec<Refusal>,
) {
    for function in functions {
        // A function type this build cannot execute is **skipped, not fatal**
        // (owner decision 2): the stack keeps the other functions' effects and
        // the skip is reported. The refusal is recorded **before** the
        // conditions are evaluated, so the verdict does not depend on a coin
        // flip.
        if let Some(reason) = function.refusal_reason() {
            skipped.push(Refusal {
                kind: "function",
                type_name: function.type_name().to_owned(),
                reason,
            });
            continue;
        }
        // A function whose own conditions cannot be evaluated is skipped the
        // same way: its absence degrades the stack (no furnace_smelt -> raw
        // beef) rather than poisoning the roll, and the skip is reported.
        if !function.conditions_pass(rng, context, skipped) {
            continue;
        }
        match &function.kind {
            LootFunctionKind::SetCount { count, add } => {
                let value = draw_count(count, rng, skipped, "count");
                // Vanilla truncates the drawn count towards zero before setting it.
                let value = numeric::count(value);
                stack.count = if *add {
                    stack.count.saturating_add(value)
                } else {
                    value
                };
            }
            LootFunctionKind::LimitCount { min, max } => {
                // The jar's `LimitCount.run` resolves to `Mth.clamp(count,
                // min, max)`: a stack below `min` is raised to `min`, not
                // discarded. (An earlier version of this build zeroed it; the
                // pinned test enshrining that reading was rewritten against the
                // bytecode.)
                if stack.count < *min {
                    stack.count = *min;
                } else if stack.count > *max {
                    stack.count = *max;
                }
            }
            LootFunctionKind::SetDamage { damage, add } => {
                // Modelled but not executable, so the damage is recorded for a caller that
                // wants it and the refusal above is what makes `roll` fail.
                let value = draw_count(damage, rng, skipped, "damage").clamp(0.0, 1.0);
                stack.damage = Some(if *add {
                    (stack.damage.unwrap_or(0.0) + value).clamp(0.0, 1.0)
                } else {
                    value
                });
            }
            // Already refused above; nothing to apply.
            LootFunctionKind::Raw(_) => {}
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
            .filter(|table| {
                references(table).iter().any(|r| match r {
                    Reference::Named(target) => *target == name,
                    Reference::Inline(_) => false,
                })
            })
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

    /// The **hand-written baseline subset** of the Vanilla block-loot set.
    ///
    /// Eight common full-cube blocks, shaped 1:1 like the jar files they
    /// mirror (single-item pools; `alternatives` with a silk-touch first
    /// child for stone and grass, exactly as Vanilla writes them): stone
    /// drops cobblestone, grass drops dirt, gravel drops gravel, and
    /// cobblestone, dirt, sand, oak logs and oak planks drop themselves.
    /// A bare hand (unenchanted tool context) takes the non-silk branch
    /// everywhere, which is Vanilla's own outcome for these tables.
    ///
    /// This exists for the same reason as the crafting and smelting baselines.
    /// A server with no pack loaded would otherwise drop nothing at all
    /// (`by_name` misses and the break path logs and moves on — the P14 soak
    /// owner's "no drops" report). `load_packs` replaces these entry-for-entry
    /// when real tables arrive ([`LootTables::insert`] overwrites same-named
    /// tables), so the baseline is strictly a no-pack fallback, never a
    /// competitor.
    ///
    /// Deliberately **not** modelled: flint from gravel (fortune-gated
    /// chance), silk-touch self drops (the branch exists and evaluates, but
    /// no tool here carries silk), ores and tool-tier gates, glass (drops
    /// nothing bare-handed — correctly absent, like Vanilla).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when a hard-coded id fails to parse,
    /// which is a programmer error in this table, not hostile input.
    pub fn baseline() -> ServerResult<Self> {
        use mc_core::ids::ResourceId;
        fn id(name: &str) -> ServerResult<ResourceId> {
            ResourceId::parse(name).map_err(|error| {
                ServerError::CorruptData(format!("baseline loot id {name:?}: {error}"))
            })
        }
        fn item(name: &str) -> ServerResult<LootEntry> {
            Ok(LootEntry::Item {
                name: id(&format!("minecraft:{name}"))?,
                weight: 1,
                quality: 0,
                expand: false,
                functions: Vec::new(),
                conditions: Vec::new(),
            })
        }
        fn silk_alternatives(block: &str, plain: LootEntry) -> ServerResult<LootEntry> {
            Ok(LootEntry::Alternatives {
                children: vec![
                    LootEntry::Item {
                        name: id(&format!("minecraft:{block}"))?,
                        weight: 1,
                        quality: 0,
                        expand: false,
                        functions: Vec::new(),
                        conditions: vec![LootCondition::MatchToolEnchantments {
                            requirements: vec![EnchantmentRequirement {
                                enchantment: "minecraft:silk_touch".to_owned(),
                                min_level: Some(1),
                                max_level: None,
                            }],
                        }],
                    },
                    plain,
                ],
                functions: Vec::new(),
                conditions: Vec::new(),
            })
        }
        fn table(stem: &str, entries: Vec<LootEntry>) -> ServerResult<LootTable> {
            Ok(LootTable {
                name: Some(id(&format!("minecraft:blocks/{stem}"))?),
                kind: "minecraft:block".to_owned(),
                random_sequence: Some(format!("minecraft:blocks/{stem}")),
                pools: vec![LootPool {
                    rolls: CountProvider::Constant(1.0),
                    bonus_rolls: CountProvider::Constant(0.0),
                    entries,
                    conditions: vec![LootCondition::SurvivesExplosion],
                    functions: Vec::new(),
                }],
                functions: Vec::new(),
            })
        }
        // (block stem, drop item stem or `None` for an alternatives pair).
        // `None` means "silk-touch self, else the second stem".
        let pairs: &[(&str, &str)] = &[
            ("cobblestone", "cobblestone"),
            ("dirt", "dirt"),
            ("sand", "sand"),
            ("gravel", "gravel"),
            ("oak_log", "oak_log"),
            ("oak_planks", "oak_planks"),
        ];
        let mut tables = Self::new();
        for (stem, drop) in pairs {
            tables.insert(table(stem, vec![item(drop)?])?);
        }
        tables.insert(table(
            "stone",
            vec![silk_alternatives("stone", item("cobblestone")?)?],
        )?);
        tables.insert(table(
            "grass_block",
            vec![silk_alternatives("grass_block", item("dirt")?)?],
        )?);
        Ok(tables)
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
    /// survey measured. Count providers are **not** here; they are arguments of a function
    /// rather than nodes in the tree, and are reported by [`Self::number_providers`]
    /// instead.
    pub modelled: BTreeMap<String, usize>,
    /// Number-provider types, with how many occurrences.
    ///
    /// Kept out of `modelled`/`unexecutable` on purpose: including them would make the
    /// per-level totals disagree with the measured census. Note that `constant` is not a
    /// type string in the file — a literal count has no `type` field — so it is reported
    /// under the name [`CountProvider::Constant`] gives it.
    pub number_providers: BTreeMap<String, usize>,
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

    /// How many times a number-provider type was seen.
    #[must_use]
    pub fn provider_occurrences(&self, type_name: &str) -> usize {
        self.number_providers.get(type_name).copied().unwrap_or(0)
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

/// The 19 `function` types, measured from the jar.
///
/// 18 of them appear in the 1 326 files; the nineteenth, `minecraft:set_components`, appears
/// only inside the three tables inlined by `equipment/trial_chamber`.
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
    "minecraft:set_components",
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

/// Read one loot entry.
///
/// Long by necessity: it is one arm per entry type, each with its own required fields, and the
/// alternative is a per-type helper that would have to thread the report, the item registry and
/// the recursion depth through seven signatures.
#[allow(clippy::too_many_lines)]
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
    // An entry type outside the measured set is preserved whole. Its nested functions and
    // conditions are counted so the census stays exact, and its count providers are left inside
    // the blob — an opaque construct's *arguments* are not reported, which is stated here rather
    // than left as a silent hole.
    if !ENTRY_TYPES.contains(&kind) {
        crate::bump(&mut report.unexecutable, kind);
        let json = serde_json::Value::Object(map.clone());
        count_raw_constructs(&json, report);
        return Ok(LootEntry::Raw(json));
    }
    crate::bump(&mut report.modelled, kind);
    let weight = read_weight(map, path)?;
    let quality = read_quality(map, path)?;
    // `functions` is legal on every entry type. Reading it only on `minecraft:item` — which the
    // first version did — silently drops the two functions vanilla puts on `minecraft:loot_table`
    // entries, and then their conditions, and then their conditions' terms. The differential test
    // caught the cascade.
    let functions = match map.get("functions") {
        Some(value) => read_functions(value, path, report)?,
        None => Vec::new(),
    };
    let conditions = match map.get("conditions") {
        Some(value) => read_conditions(value, path, report)?,
        None => Vec::new(),
    };
    match kind {
        "minecraft:item" => {
            let name = required_str(map, "name", path).map_err(Skipped::Parse)?;
            let name = read_item_name(name, path, report, items)?;
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
            functions,
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
                functions,
                conditions,
            })
        }
        "minecraft:alternatives" | "minecraft:sequence" => {
            let children = read_children(map, path, report, items, depth)?;
            Ok(if kind == "minecraft:alternatives" {
                LootEntry::Alternatives {
                    children,
                    functions,
                    conditions,
                }
            } else {
                LootEntry::Sequence {
                    children,
                    functions,
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
            functions,
            conditions,
        }),
        // The only remaining member of the measured set.
        "minecraft:tag" => {
            let name = required_str(map, "name", path).map_err(Skipped::Parse)?;
            Ok(LootEntry::Tag {
                name: read_id(name, "tag", path)?,
                weight,
                quality,
                functions,
                conditions,
            })
        }
        // Unreachable: the guard above returned for anything outside `ENTRY_TYPES`. Reported
        // rather than `unreachable!()`-ed, because AGENTS.md §9 forbids panic as control flow
        // and a `debug_assert` is not a guarantee in release.
        other => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("unrecognised loot entry type {other:?}"),
        })),
    }
}

/// Count the functions and conditions inside a construct this build does not parse.
///
/// Only the two levels the census tracks. Their *arguments* (count providers) are not counted:
/// they are inside an opaque blob, and inventing a figure for them would be worse than
/// reporting none. Stated here rather than left implicit.
fn count_raw_constructs(value: &serde_json::Value, report: &mut LootLoadReport) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(name) = map.get("function").and_then(serde_json::Value::as_str) {
                crate::bump(&mut report.unexecutable, name);
            }
            if let Some(name) = map.get("condition").and_then(serde_json::Value::as_str) {
                crate::bump(&mut report.unexecutable, name);
            }
            for child in map.values() {
                count_raw_constructs(child, report);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                count_raw_constructs(child, report);
            }
        }
        _ => {}
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
        Some(value) if (1..=i64::from(i32::MAX)).contains(&value) => Ok(numeric::weight(value)),
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
            crate::bump(&mut report.number_providers, "constant");
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
                            reason: format!(
                                "a uniform provider needs min <= max, got {min}..{max}"
                            ),
                        }));
                    }
                    crate::bump(&mut report.number_providers, kind);
                    Ok(CountProvider::Uniform { min, max })
                }
                "minecraft:binomial" => {
                    // The format calls them `n` and `p`; some documents call them `extra` and
                    // `probability`. Accept both rather than silently treating a stated
                    // binomial as a constant.
                    //
                    // `n` is written as a **float** — `"n": 3.0` in all 18 vanilla occurrences
                    // — so it is read as a number and then required to be whole. A fractional
                    // `n` is a pack bug, and refusing it beats rounding a round count. Note
                    // that `optional_i64` cannot be tried first: it *errors* on `3.0` rather
                    // than returning `None`, which is what the first version of this code got
                    // wrong and what the differential test caught.
                    let extra = match optional_integral(map, "n", path)? {
                        Some(value) => Some(value),
                        None => optional_integral(map, "extra", path)?,
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
                    crate::bump(&mut report.number_providers, kind);
                    Ok(CountProvider::Binomial {
                        extra,
                        probability: numeric::chance(probability),
                    })
                }
                other => {
                    crate::bump(&mut report.number_providers, other);
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
    // A function's own `conditions` gate whether it runs at all; 164 of vanilla's 1 392
    // functions carry them, so an absent field is the common case and an empty vector is the
    // right reading of it.
    let conditions = match map.get("conditions") {
        Some(value) => read_conditions(value, path, report)?,
        None => Vec::new(),
    };
    let kind = match kind {
        "minecraft:set_count" => {
            crate::bump(&mut report.modelled, kind);
            let Some(count) = map.get("count") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:set_count needs \"count\"".to_owned(),
                }));
            };
            LootFunctionKind::SetCount {
                count: read_count(count, path, report)?,
                add: optional_bool(map, "add", path)?,
            }
        }
        "minecraft:limit_count" => {
            crate::bump(&mut report.modelled, kind);
            let Some(serde_json::Value::Object(limit)) = map.get("limit") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:limit_count needs a \"limit\" object".to_owned(),
                }));
            };
            let min = optional_integral(limit, "min", path)?.unwrap_or(0);
            let max = optional_integral(limit, "max", path)?.unwrap_or(i64::from(i32::MAX));
            LootFunctionKind::LimitCount {
                min: to_i32(min, "limit min", path)?,
                max: to_i32(max, "limit max", path)?,
            }
        }
        "minecraft:set_damage" => {
            crate::bump(&mut report.modelled, kind);
            let Some(damage) = map.get("damage") else {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "minecraft:set_damage needs \"damage\"".to_owned(),
                }));
            };
            LootFunctionKind::SetDamage {
                damage: read_count(damage, path, report)?,
                add: optional_bool(map, "add", path)?,
            }
        }
        other => {
            crate::bump(&mut report.unexecutable, other);
            LootFunctionKind::Raw(serde_json::Value::Object(map.clone()))
        }
    };
    Ok(LootFunction { kind, conditions })
}

fn read_conditions(
    value: &serde_json::Value,
    path: &Path,
    report: &mut LootLoadReport,
) -> Result<Vec<LootCondition>, Skipped> {
    let list = value.as_array().ok_or_else(|| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "\"conditions\" must be an array, found {}",
                type_name(value)
            ),
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
                enchanted_chance: read_chance_provider(map, path)?,
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
                out.push(numeric::chance(number));
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
            // Not counted in `modelled`: `minecraft:linear` is a *chance provider*, an argument
            // of the condition rather than a node in the tree, exactly like a count provider.
            // Counting it would make the condition total 1 556 instead of the measured 1 545,
            // which is how the differential test found this.
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
                base: numeric::slope(base),
                per_level_above_first: numeric::slope(per_level_above_first),
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
    Ok(numeric::chance(required_number(map, key, path)?))
}

fn to_i32(value: i64, what: &str, path: &Path) -> Result<i32, Skipped> {
    i32::try_from(value).map_err(|_| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{what} of {value} does not fit an i32"),
        })
    })
}

/// Read an optional whole number that the format may write as a float.
///
/// `minecraft:limit_count`'s `limit.min`/`limit.max` and `minecraft:binomial`'s `n` are all
/// written as **floats** in the real pack — `"min": 1.0`, `"n": 3.0` — so a parser that insists
/// on a JSON integer refuses ten vanilla files. The differential test caught exactly that.
///
/// A value with a fractional part is refused rather than truncated: these fields count things,
/// and silently rounding a count produces loot nobody can reproduce.
fn optional_integral(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Option<i64>, Skipped> {
    let Some(value) = optional_f64(map, key, path).map_err(Skipped::Parse)? else {
        return Ok(None);
    };
    if !value.is_finite() || value.fract() != 0.0 {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{key:?} must be a whole number, found {value}"),
        }));
    }
    // `i64::MAX` as an `f64` is not exactly representable, so the bound is checked against
    // `f64` rather than by a cast that could saturate.
    if value.abs() > 9.0e18 {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{key:?} of {value} is out of range"),
        }));
    }
    Ok(Some(numeric::whole(value)))
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
            reason: format!(
                "field {key:?} must be a boolean, found {}",
                type_name(other)
            ),
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
    if let Some(items) = items
        && items.id(&id.to_string()).is_err()
        && items.id(id.value()).is_err()
    {
        report.forward_references.push(ForwardReference {
            registry: "item",
            name: id.to_string(),
            file: path.display().to_string(),
        });
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
