//! Tests for building a furnace table from real recipes (P07-09).

use mc_core::ids::ResourceId;
use mc_data::{CookingRecipe, Ingredient, Recipe, RecipeBook, ShapedRecipe, SmeltingKind};
use mc_registry::ItemRegistry;
use std::collections::BTreeMap;
use std::path::Path;

use crate::smelting_data::SmeltingConversion;

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the item fixture loads")
}

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid id")
}

fn name(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid id")
}

/// A cooking recipe with one item ingredient.
fn cooking(
    recipe_name: &str,
    kind: SmeltingKind,
    input: &str,
    output: &str,
    ticks: i32,
    experience: f64,
) -> Recipe {
    Recipe::Cooking(CookingRecipe {
        name: name(recipe_name),
        kind,
        ingredient: vec![Ingredient::Item(id(input))],
        result: id(output),
        result_count: 1,
        cooking_time: ticks,
        experience,
    })
}

/// A cooking recipe whose ingredient is a tag.
fn tag_cooking(recipe_name: &str, tag: &str, output: &str) -> Recipe {
    Recipe::Cooking(CookingRecipe {
        name: name(recipe_name),
        kind: SmeltingKind::Smelting,
        ingredient: vec![Ingredient::Tag(id(tag))],
        result: id(output),
        result_count: 1,
        cooking_time: 200,
        experience: 0.1,
    })
}

fn book(recipes: Vec<Recipe>) -> RecipeBook {
    let mut book = RecipeBook::new();
    for recipe in recipes {
        book.insert(recipe);
    }
    book
}

#[test]
fn a_real_shaped_recipe_is_named_and_valued_from_the_data() {
    // The recipe the jar confirms: raw iron smelts to iron in 200 ticks for 0.7 xp.
    let registry = items();
    let iron = registry.id("minecraft:iron_ingot").expect("iron exists");
    let raw = registry.id("minecraft:raw_iron").expect("raw iron exists");

    let built = book(vec![cooking(
        "minecraft:iron_ingot_from_smelting_raw_iron",
        SmeltingKind::Smelting,
        "minecraft:raw_iron",
        "minecraft:iron_ingot",
        200,
        0.7,
    )]);
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");

    assert_eq!(report.recipes_converted, 1);
    assert_eq!(report.rows, 1);
    assert!(report.is_total(), "{report:?}");
    let recipe = table.recipe_for(raw).expect("raw iron smelts");
    assert_eq!(recipe.output, iron);
    assert_eq!(recipe.cook_ticks, 200, "from the data, not a constant");
    assert!((recipe.experience - 0.7).abs() < 1e-6, "from the data");
}

#[test]
fn only_the_requested_kind_is_converted() {
    let registry = items();
    let built = book(vec![
        cooking(
            "minecraft:a",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            200,
            0.7,
        ),
        cooking(
            "minecraft:b",
            SmeltingKind::Blasting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            100,
            0.7,
        ),
    ]);
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Blasting,
        &registry,
        None,
    )
    .expect("converts");
    assert_eq!(report.recipes_converted, 1);
    let raw = registry.id("minecraft:raw_iron").expect("raw iron");
    assert_eq!(
        table.recipe_for(raw).expect("blasting row").cook_ticks,
        100,
        "the blasting time, not the smelting one"
    );
}

#[test]
fn a_tag_ingredient_without_a_resolver_is_counted_not_dropped() {
    let registry = items();
    let built = book(vec![tag_cooking(
        "minecraft:charcoal",
        "minecraft:logs_that_burn",
        "minecraft:charcoal",
    )]);
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");
    assert_eq!(report.recipes_converted, 0);
    assert_eq!(
        report.unresolved_tag_recipes, 1,
        "counted, not silently skipped"
    );
    assert!(!report.is_total());
    assert_eq!(report.seen(), 1, "the accounting must add up");
    assert!(table.is_empty());
}

#[test]
fn a_tag_ingredient_with_a_resolver_expands_to_one_row_per_member() {
    let registry = items();
    let log = id("minecraft:oak_log");
    let planks = id("minecraft:oak_planks");
    let built = book(vec![tag_cooking(
        "minecraft:charcoal",
        "minecraft:logs_that_burn",
        "minecraft:charcoal",
    )]);
    let members = vec![log.clone(), planks.clone()];
    let resolve = |_: &ResourceId| members.clone();
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        Some(&resolve),
    )
    .expect("converts");
    assert_eq!(report.rows, 2, "one row per tag member");
    assert_eq!(report.recipes_converted, 1, "but still one recipe");
    assert!(report.is_total(), "{report:?}");
    let charcoal = registry.id("minecraft:charcoal").expect("charcoal");
    for member in [log, planks] {
        // The **full** name: `value()` is the path alone and is not a registry key.
        let full = format!("{}:{}", member.namespace(), member.value());
        let item_id = registry.id(&full).expect("a real item");
        assert_eq!(
            table.recipe_for(item_id).expect("a row").output,
            charcoal,
            "{member}"
        );
    }
}

#[test]
fn an_empty_tag_is_reported_by_name() {
    let registry = items();
    let built = book(vec![tag_cooking(
        "minecraft:charcoal",
        "minecraft:logs_that_burn",
        "minecraft:charcoal",
    )]);
    let resolve = |_: &ResourceId| Vec::new();
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        Some(&resolve),
    )
    .expect("converts");
    assert_eq!(report.recipes_converted, 0);
    assert_eq!(
        report.empty_tag_recipes, 1,
        "the recipe is in the empty-tag bucket"
    );
    assert_eq!(report.seen(), 1, "and in exactly one bucket");
    assert_eq!(report.empty_tags.len(), 1);
    assert!(report.empty_tags[0].contains("logs_that_burn"));
    assert!(table.is_empty());
}

#[test]
fn duplicate_inputs_are_grouped_and_counted_rather_than_failing_the_load() {
    // The real pack can have several recipes for one input once overlapping tags expand;
    // `SmeltingRegistry::new` refuses a duplicate, so the conversion must group instead
    // of rejecting a whole data set.
    let registry = items();
    let built = book(vec![
        cooking(
            "minecraft:a_first",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            200,
            0.7,
        ),
        cooking(
            "minecraft:b_second",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            100,
            0.1,
        ),
    ]);
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("a duplicate must not fail the load");
    assert_eq!(report.recipes_converted, 1, "one recipe contributed a row");
    assert_eq!(report.rows, 1, "and it contributed exactly one row");
    assert_eq!(
        report.duplicate_inputs, 1,
        "the other had every input taken"
    );
    assert_eq!(report.seen(), 2, "the accounting must add up");
    // The winner is the first by **recipe name**, not by load order, so the table is
    // reproducible regardless of the order the pack was read in.
    let raw = registry.id("minecraft:raw_iron").expect("raw iron");
    assert_eq!(table.recipe_for(raw).expect("a row").cook_ticks, 200);
}

#[test]
fn the_winner_of_a_duplicate_does_not_depend_on_insertion_order() {
    let registry = items();
    let forward = book(vec![
        cooking(
            "minecraft:a_first",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            200,
            0.7,
        ),
        cooking(
            "minecraft:z_last",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            100,
            0.1,
        ),
    ]);
    let backward = book(vec![
        cooking(
            "minecraft:z_last",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            100,
            0.1,
        ),
        cooking(
            "minecraft:a_first",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            200,
            0.7,
        ),
    ]);
    let raw = registry.id("minecraft:raw_iron").expect("raw iron");
    let (a, _) = crate::furnace::SmeltingRegistry::from_recipes(
        &forward,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");
    let (b, _) = crate::furnace::SmeltingRegistry::from_recipes(
        &backward,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");
    assert_eq!(
        a.recipe_for(raw).expect("a row").cook_ticks,
        b.recipe_for(raw).expect("a row").cook_ticks,
        "the winner must be a property of the data, not of insertion order"
    );
}

#[test]
fn an_unknown_result_name_is_reported_rather_than_converted() {
    let registry = items();
    let built = book(vec![cooking(
        "minecraft:bogus",
        SmeltingKind::Smelting,
        "minecraft:raw_iron",
        "not_a_mod:not_an_item",
        200,
        0.7,
    )]);
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &built,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");
    assert_eq!(report.recipes_converted, 0);
    assert_eq!(report.unknown_results.len(), 1);
    assert_eq!(report.seen(), 0, "an unknown result is its own bucket");
    assert!(report.unknown_results[0].contains("not_an_item"));
    assert!(table.is_empty());
}

#[test]
fn a_malformed_numeric_field_is_corrupt_data_not_a_clamp() {
    let registry = items();
    for (ticks, experience, what) in [(-1, 0.7, "negative cook time"), (200, -1.0, "negative xp")] {
        let built = book(vec![cooking(
            "minecraft:bad",
            SmeltingKind::Smelting,
            "minecraft:raw_iron",
            "minecraft:iron_ingot",
            ticks,
            experience,
        )]);
        let error = crate::furnace::SmeltingRegistry::from_recipes(
            &built,
            SmeltingKind::Smelting,
            &registry,
            None,
        )
        .expect_err(&format!("{what} must be refused"));
        let text = error.to_string();
        assert!(text.contains("minecraft:bad"), "{text}");
    }
}

#[test]
fn an_empty_book_produces_an_empty_table_without_error() {
    let registry = items();
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &RecipeBook::new(),
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("an empty book is not an error");
    assert!(table.is_empty());
    assert_eq!(report, SmeltingConversion::default());
    assert!(report.is_total(), "nothing was skipped");
}

#[test]
fn the_conversion_ignores_non_cooking_recipes_entirely() {
    let registry = items();
    let mut book = RecipeBook::new();
    book.insert(Recipe::Shaped(ShapedRecipe {
        name: name("minecraft:stick"),
        pattern: vec!["#".to_owned(), "#".to_owned()],
        key: BTreeMap::new(),
        result: id("minecraft:stick"),
        result_count: 4,
        group: None,
    }));
    let (table, report) = crate::furnace::SmeltingRegistry::from_recipes(
        &book,
        SmeltingKind::Smelting,
        &registry,
        None,
    )
    .expect("converts");
    assert!(table.is_empty());
    assert_eq!(report, SmeltingConversion::default(), "not even counted");
}
