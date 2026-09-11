//! Building furnace tables from **real** data-pack recipes (P07-09).
//!
//! ## What this replaces
//!
//! `furnace.rs` ships a hand-written `SmeltingRegistry::baseline` — six recipes from
//! community knowledge, which `PHASE-06-REPORT.md` §5.9 had to record as unverified. The
//! 26.1.2 jar ships **73 `smelting` recipes**, 25 `blasting`, 9 `smoking` and 9
//! `campfire_cooking`, and `mc-data` loads them.
//!
//! This module is the join: [`SmeltingRegistry::from_recipes`] turns a loaded
//! [`mc_data::RecipeBook`] into the table `Furnace::tick` consumes, so the furnace works
//! from data the jar defines rather than from a list someone typed. The two values the
//! baseline guessed are confirmed by the jar (`iron_ingot_from_smelting_raw_iron` is
//! `cookingtime: 200, experience: 0.7`), which is exactly the kind of thing the join
//! makes checkable.
//!
//! ## What it does not do, and why that is stated here
//!
//! - **Tags are not resolved.** A vanilla smelting recipe's `ingredient` may be a tag
//!   (`#minecraft:logs_that_burn`), and a tag is a *set* of items. One
//!   [`SmeltingRecipe`] holds one input id, so a tag ingredient would have to expand into
//!   one recipe per member. That expansion needs `mc_data::TagSet`, which the caller has,
//!   so [`SmeltingRegistry::from_recipes`] takes an optional resolver and reports how
//!   many tag ingredients it could not expand rather than dropping them.
//! - **Only one kind at a time.** `blasting`, `smoking` and `campfire_cooking` have the
//!   same shape but different blocks and times; the caller picks which
//!   [`mc_data::SmeltingKind`] to build.
//! - **Fuel values are still a table.** Vanilla's fuel burn times are item data
//!   components, not recipes. `FUEL_BURST_TICKS` remains a labelled table (P07-03's
//!   remaining half), and this module does not pretend otherwise.
//! - **`SmeltingRegistry::new` refuses two recipes with the same input**, and vanilla
//!   genuinely has those: 73 smelting recipes across 12 wood types plus ore variants can
//!   collide on a shared input via tags. So this conversion **groups by input** and keeps
//!   the first, counting the duplicates instead of failing — a real data set must not be
//!   rejected for a shape the model cannot represent.

use mc_core::error::{ServerError, ServerResult};
use mc_data::{Ingredient, Recipe, RecipeBook, SmeltingKind};

use crate::furnace::{SmeltingRecipe, SmeltingRegistry};

/// The caller's tag-membership lookup: given a tag id, the item ids it contains.
///
/// Supplied by the caller rather than reached for, because resolving a tag needs the
/// loaded [`mc_data::TagSet`] and the registry a tag belongs to — knowledge the *loader*
/// has and a furnace table does not. `None` means tags are not resolved, in which case
/// every tag ingredient is counted as an unresolved recipe rather than dropped.
pub type TagResolver<'a> = &'a dyn Fn(&mc_core::ids::ResourceId) -> Vec<mc_core::ids::ResourceId>;

/// A resource id's full `namespace:path` name.
///
/// `ResourceId::value()` is the **path** alone (`iron_ingot`), while the registries key on
/// the full name (`minecraft:iron_ingot`). Spelling that out once is what keeps the four
/// lookups below from disagreeing.
fn full_name(id: &mc_core::ids::ResourceId) -> String {
    format!("{}:{}", id.namespace(), id.value())
}

/// What a conversion did, including what it could not represent.
///
/// Counted rather than logged: a caller that loads the real pack needs to know that 30
/// recipes were skipped and why, or a furnace that does not smelt an ore looks like a bug
/// rather than a modelled limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SmeltingConversion {
    /// Recipes of the requested kind that were read.
    pub recipes_seen: usize,
    /// Recipes that produced at least one table row.
    ///
    /// **Not the table's size**, and the difference matters: one recipe can yield several
    /// rows, because an ingredient is a list of alternatives and a tag expands to its
    /// members. The first version of this report counted *rows* here, which meant a
    /// dropped recipe would have been invisible — 156 rows from 116 recipes read as
    /// success. Found by the differential test against the real pack.
    pub recipes_converted: usize,
    /// Table rows produced.
    pub rows: usize,
    /// Recipes that produced no row because every candidate input was already taken.
    ///
    /// Vanilla genuinely has these: an ingredient may be a tag, and two recipes can reach
    /// the same item through different tags. This model keys a row by item id, so the
    /// later recipe cannot be represented and is counted.
    pub duplicate_inputs: usize,
    /// Recipes with no usable input because a tag ingredient had no resolver.
    pub unresolved_tag_recipes: usize,
    /// Recipes with no usable input because every tag ingredient resolved to nothing.
    pub empty_tag_recipes: usize,
    /// Recipes whose result name is not in the item registry.
    ///
    /// Counted separately from the three above because it is a *data* problem rather than
    /// a modelling limit, and a caller may want to fail on it.
    pub unknown_results: Vec<String>,
    /// Tag ids the resolver expanded into nothing. A per-alternative diagnostic, not a
    /// recipe tally, so it is deliberately **not** part of [`Self::seen`].
    pub empty_tags: Vec<String>,
}

impl SmeltingConversion {
    /// Recipes that were **accounted for**: converted, no usable input, or already taken.
    ///
    /// Every recipe of the requested kind falls into exactly one of these, so this is the
    /// denominator that makes "nothing was dropped" checkable — and the first version of
    /// this type got it wrong by mixing per-recipe and per-alternative tallies, which the
    /// `debug_assert_eq!` in `from_recipes` now catches on every run.
    #[must_use]
    pub const fn seen(&self) -> usize {
        self.recipes_converted
            + self.duplicate_inputs
            + self.unresolved_tag_recipes
            + self.empty_tag_recipes
    }

    /// Whether every recipe of the requested kind became at least one table row.
    #[must_use]
    pub const fn is_total(&self) -> bool {
        self.duplicate_inputs == 0
            && self.unresolved_tag_recipes == 0
            && self.empty_tag_recipes == 0
            && self.unknown_results.is_empty()
    }
}

impl SmeltingRegistry {
    /// Build a furnace table from a loaded recipe book.
    ///
    /// `resolve_tag` maps a tag id to the item ids it contains; `None` means tags are not
    /// resolved, and each tag ingredient is counted as skipped. `items` resolves result
    /// and ingredient names to ids.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the resulting table is not constructible, which is
    /// a programming error rather than a data one: duplicate inputs are handled by
    /// grouping, so this can only fire on an internal inconsistency.
    pub fn from_recipes(
        book: &RecipeBook,
        kind: SmeltingKind,
        items: &mc_registry::ItemRegistry,
        resolve_tag: Option<TagResolver<'_>>,
    ) -> ServerResult<(Self, SmeltingConversion)> {
        let mut report = SmeltingConversion::default();
        // A row count and a recipe count are different numbers, and conflating them hides
        // a dropped recipe. See `SmeltingConversion::recipes_converted`.
        let mut recipes: Vec<SmeltingRecipe> = Vec::new();
        let mut taken: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();

        // Ascending by name, so the table is the same on every run (AGENTS.md §3.6) and
        // "which recipe wins a duplicate" is a documented rule rather than load order.
        let mut candidates: Vec<&Recipe> = book
            .recipes()
            .iter()
            .filter(|recipe| matches!(recipe, Recipe::Cooking(cooking) if cooking.kind == kind))
            .collect();
        candidates.sort_by(|a, b| a.name().cmp(b.name()));

        for recipe in candidates {
            let Recipe::Cooking(cooking) = recipe else {
                continue;
            };
            report.recipes_seen += 1;
            let Some(output) = items.id(&full_name(&cooking.result)).ok() else {
                report.unknown_results.push(cooking.result.to_string());
                continue;
            };
            // Whether this recipe produced anything at all; set once, below.
            let mut produced = false;
            // Why it produced nothing, so the recipe lands in exactly one bucket.
            let mut blocked_by_tag = false;
            let mut blocked_by_empty_tag = false;

            // An ingredient is a list of alternatives. Every *item* alternative becomes a
            // row; a *tag* alternative needs the resolver.
            let mut inputs: Vec<i32> = Vec::new();
            for ingredient in &cooking.ingredient {
                match ingredient {
                    Ingredient::Item(id) => {
                        if let Ok(item_id) = items.id(&full_name(id)) {
                            inputs.push(item_id);
                        }
                    }
                    Ingredient::Tag(tag) => {
                        let Some(resolve) = resolve_tag else {
                            blocked_by_tag = true;
                            continue;
                        };
                        let members = resolve(tag);
                        if members.is_empty() {
                            report.empty_tags.push(tag.to_string());
                            blocked_by_empty_tag = true;
                            continue;
                        }
                        for member in members {
                            if let Ok(item_id) = items.id(&full_name(&member)) {
                                inputs.push(item_id);
                            }
                        }
                    }
                }
            }

            for input in inputs {
                if !taken.insert(input) {
                    continue;
                }
                produced = true;
                // The format stores ticks and experience per recipe; a negative or
                // absurd value is a malformed pack rather than something to clamp
                // silently, so it is refused by name.
                let cook_ticks = u32::try_from(cooking.cooking_time).map_err(|_| {
                    ServerError::CorruptData(format!(
                        "{}: cookingtime {} is not a tick count",
                        cooking.name, cooking.cooking_time
                    ))
                })?;
                let experience = cooking.experience;
                if !experience.is_finite() || experience < 0.0 {
                    return Err(ServerError::CorruptData(format!(
                        "{}: experience {experience} is not a grant",
                        cooking.name
                    )));
                }
                recipes.push(SmeltingRecipe {
                    input,
                    output,
                    output_count: cooking.result_count,
                    cook_ticks,
                    experience: experience as f32,
                });
                report.rows += 1;
            }

            if produced {
                report.recipes_converted += 1;
            } else if blocked_by_tag {
                report.unresolved_tag_recipes += 1;
            } else if blocked_by_empty_tag {
                report.empty_tag_recipes += 1;
            } else {
                // Every candidate input was already taken, so nothing could be added.
                report.duplicate_inputs += 1;
            }
        }

        debug_assert_eq!(
            report.seen() + report.unknown_results.len(),
            report.recipes_seen,
            "every recipe read must land in exactly one bucket"
        );

        Ok((Self::new(recipes)?, report))
    }
}

#[cfg(test)]
mod tests;
