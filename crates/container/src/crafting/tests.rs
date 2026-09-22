//! Crafting tests (P06-04), weighted towards the matching rules and the hostile
//! grids.
//!
//! The properties under test are the ones a wrong implementation gets wrong:
//!
//! * **offset** — a 2x2 recipe matches in every corner of a 3x3 grid;
//! * **shape** — the right ingredients in the wrong arrangement do not match, and
//!   an extra item never matches;
//! * **consumption** — one craft removes exactly one of each ingredient and leaves
//!   the rest of the grid alone;
//! * **results** — every baseline recipe resolves by name and is reachable from the
//!   ingredients it names;
//! * **hostile grids** — empty, too small, ragged, zero-width and air-filled grids
//!   produce no result and no panic.

use mc_entity::stack::{ItemStack, StackSizeTable};
use mc_registry::ItemRegistry;
use std::path::Path;

use super::{
    Ingredient, MAX_ALTERNATIVES_PER_KEY, MAX_GRID_SLOTS, Recipe, RecipeKind, RecipeRegistry,
    Shape, ShapedPattern, ShapedRecipe, ShapelessRecipe,
};
use crate::container::Container;

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the registry fixture loads")
}

fn sizes() -> StackSizeTable {
    StackSizeTable::resolve(&items()).expect("the stack-size table resolves")
}

/// An item id resolved by name.
///
/// Numeric ids are **not** hard-coded here: guessing them produced two wrong
/// constants in this workspace already (id 3 is polished granite, not a bucket).
/// A name lookup cannot drift from the registry.
fn item(name: &str) -> i32 {
    items()
        .id(name)
        .unwrap_or_else(|_| panic!("{name} must exist in the item registry"))
}

fn stack(item_id: i32, count: i32) -> ItemStack {
    ItemStack::new(item_id, count).expect("a valid stack")
}

fn baseline() -> RecipeRegistry {
    RecipeRegistry::baseline(&items())
        .expect("the baseline recipe set resolves against the registry")
}

/// A 9-slot 3x3 grid, empty.
fn grid3() -> Vec<ItemStack> {
    vec![ItemStack::EMPTY; MAX_GRID_SLOTS]
}

/// A 4-slot 2x2 grid, empty.
fn grid2() -> Vec<ItemStack> {
    vec![ItemStack::EMPTY; 4]
}

fn oak_planks() -> i32 {
    item("minecraft:oak_planks")
}

fn cobblestone() -> i32 {
    item("minecraft:cobblestone")
}

fn oak_log() -> i32 {
    item("minecraft:oak_log")
}

fn stick() -> i32 {
    item("minecraft:stick")
}

fn stone() -> i32 {
    item("minecraft:stone")
}

// ------------------------------------------------------------------ shapes

#[test]
fn a_two_by_two_recipe_matches_in_all_four_corners_of_a_three_by_three_grid() {
    let recipes = baseline();
    // The crafting table is 2x2 planks: the archetypal offset case.
    let corners: [(&str, [usize; 4]); 4] = [
        ("top-left", [0, 1, 3, 4]),
        ("top-right", [1, 2, 4, 5]),
        ("bottom-left", [3, 4, 6, 7]),
        ("bottom-right", [4, 5, 7, 8]),
    ];
    for (corner, cells) in corners {
        let mut grid = grid3();
        for cell in cells {
            grid[cell] = stack(oak_planks(), 1);
        }
        let found = recipes
            .find_match(&grid, 3)
            .unwrap_or_else(|| panic!("the 2x2 recipe must match in the {corner} corner"));
        assert_eq!(
            found.recipe().name(),
            "baseline_crafting_table",
            "{corner}: the wrong recipe matched"
        );
        assert_eq!(
            found.consumed_slots(),
            cells.to_vec(),
            "{corner}: the recipe must consume exactly the four cells it matched"
        );
        // The centre of the 3x3 is not part of a corner placement: a "top-left"
        // match must not have wandered into the middle row.
        for (index, cell) in grid.iter().enumerate() {
            assert_eq!(
                !cell.is_empty(),
                cells.contains(&index),
                "{corner}: the premise of the test is that only its four cells are filled"
            );
        }
    }
}

#[test]
fn a_shaped_recipe_also_matches_its_horizontal_mirror() {
    // Vanilla tries the mirrored pattern when the direct one fails, so a recipe
    // drawn right-to-left still crafts. `baseline_torch` is coal over a stick: its
    // mirror is the same shape, so use the asymmetric 3-wide slab to prove the
    // mirror path, then a two-row asymmetric pattern for the row case.
    let recipes = baseline();
    let stick_over_coal = {
        let mut grid = grid3();
        grid[0] = stack(stick(), 1);
        grid[3] = stack(item("minecraft:coal"), 1);
        grid
    };
    // Stick above coal is not a torch, but it *is* the mirror-image arrangement of
    // the two-cell pattern in one sense and not in the other; the honest assertion
    // is that the recipe only matches in the documented orientation.
    assert!(
        recipes.find_match(&stick_over_coal, 3).is_none(),
        "a torch needs the coal above the stick"
    );

    // The mirrored form is reachable directly: build a pattern whose mirror is
    // distinct and check both orientations match.
    let planks = Ingredient::any_of(&[oak_planks()]).expect("one alternative");
    let cobble = Ingredient::item(cobblestone());
    let pattern = ShapedPattern::from_rows(&[
        &[Some(planks.clone()), Some(cobble.clone())],
        &[Some(cobble.clone()), None],
    ])
    .expect("a 2x2 pattern");
    let recipe = Recipe::Shaped(Box::new(
        ShapedRecipe::new("test_mirror", pattern, stone(), 1).expect("a valid shaped recipe"),
    ));
    let single = RecipeRegistry::new(vec![recipe]).expect("one recipe");

    // Direct form: planks top-left, cobblestone top-right and bottom-left.
    let mut direct = grid2();
    direct[0] = stack(oak_planks(), 1);
    direct[1] = stack(cobblestone(), 1);
    direct[2] = stack(cobblestone(), 1);
    let found = single
        .find_match(&direct, 2)
        .expect("the direct form matches");
    assert!(!found.mirrored(), "the direct form must be found first");

    // Mirrored form: planks top-right, cobblestone top-left and bottom-right.
    let mut mirrored = grid2();
    mirrored[1] = stack(oak_planks(), 1);
    mirrored[0] = stack(cobblestone(), 1);
    mirrored[3] = stack(cobblestone(), 1);
    let found = single
        .find_match(&mirrored, 2)
        .expect("the mirrored form must match");
    assert!(found.mirrored(), "only the mirror can match this way round");
    assert_eq!(found.consumed_slots(), vec![0, 1, 3]);
}

#[test]
fn a_shaped_recipe_does_not_match_a_different_arrangement() {
    let recipes = baseline();
    // Sticks are two planks stacked. Side by side is not the recipe.
    let mut side_by_side = grid2();
    side_by_side[0] = stack(oak_planks(), 1);
    side_by_side[1] = stack(oak_planks(), 1);
    assert!(
        recipes.find_match(&side_by_side, 2).is_none(),
        "two planks side by side is not the sticks recipe"
    );
    // And the same two planks diagonally is not either.
    let mut diagonal = grid2();
    diagonal[0] = stack(oak_planks(), 1);
    diagonal[3] = stack(oak_planks(), 1);
    assert!(
        recipes.find_match(&diagonal, 2).is_none(),
        "two planks diagonally is not the sticks recipe"
    );
    // The vertical arrangement is.
    let mut vertical = grid2();
    vertical[0] = stack(oak_planks(), 1);
    vertical[2] = stack(oak_planks(), 1);
    let found = recipes
        .find_match(&vertical, 2)
        .expect("two planks stacked must match");
    assert_eq!(found.recipe().name(), "baseline_sticks_from_planks");
}

#[test]
fn a_shaped_recipe_does_not_match_with_an_extra_item_inside_the_matched_area() {
    let recipes = baseline();
    // The 3x3 furnace ring with an extra cobblestone in the middle: the ring shape
    // says that cell is empty, so this must not match anything.
    let ring = [0, 1, 2, 3, 5, 6, 7, 8];
    let mut filled = grid3();
    for cell in ring {
        filled[cell] = stack(cobblestone(), 1);
    }
    assert!(
        recipes.find_match(&filled, 3).is_some(),
        "the premise: the empty-centre ring itself matches"
    );
    let mut extra = filled.clone();
    extra[4] = stack(cobblestone(), 1);
    assert!(
        recipes.find_match(&extra, 3).is_none(),
        "a ring with a filled centre is neither the furnace nor the chest"
    );
}

#[test]
fn the_chest_ring_distinguishes_planks_from_cobblestone() {
    let recipes = baseline();
    // The furnace ring of cobblestone must not be read as the chest ring.
    let mut cobble_ring = grid3();
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        cobble_ring[cell] = stack(cobblestone(), 1);
    }
    let found = recipes.find_match(&cobble_ring, 3).expect("furnace");
    assert_eq!(found.recipe().name(), "baseline_furnace");
    assert_eq!(found.recipe().result(), item("minecraft:furnace"));

    // The same ring of planks is the chest.
    let mut planks_ring = grid3();
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        planks_ring[cell] = stack(oak_planks(), 1);
    }
    let found = recipes.find_match(&planks_ring, 3).expect("the chest ring");
    assert_eq!(found.recipe().name(), "baseline_chest");
    assert_eq!(found.recipe().result(), item("minecraft:chest"));

    // A ring of the *wrong* material matches neither recipe, and neither does a ring
    // with one cell short: the shape is the whole ring or nothing.
    let mut stone_ring = grid3();
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        stone_ring[cell] = stack(stone(), 1);
    }
    assert!(
        recipes.find_match(&stone_ring, 3).is_none(),
        "a stone ring is neither the furnace nor the chest"
    );
    let mut broken_ring = cobble_ring.clone();
    broken_ring[8] = ItemStack::EMPTY;
    assert!(
        recipes.find_match(&broken_ring, 3).is_none(),
        "a ring missing a corner is not the furnace"
    );
}

// -------------------------------------------------------------- shapeless

#[test]
fn shapeless_matching_is_order_independent_and_rejects_extras() {
    let recipes = baseline();
    // A log in a 2x2 grid: any of the four positions is the same recipe.
    for position in 0..4 {
        let mut grid = grid2();
        grid[position] = stack(oak_log(), 1);
        let found = recipes
            .find_match(&grid, 2)
            .unwrap_or_else(|| panic!("a log in position {position} must match"));
        assert_eq!(
            found.recipe().name(),
            "baseline_planks_from_minecraft:oak_log"
        );
        assert_eq!(found.consumed_slots(), vec![position]);
    }

    // In a 3x3 grid, any of the nine positions likewise.
    for position in 0..MAX_GRID_SLOTS {
        let mut grid = grid3();
        grid[position] = stack(oak_log(), 1);
        assert!(
            recipes.find_match(&grid, 3).is_some(),
            "a log in position {position} of a 3x3 grid must match"
        );
    }

    // An extra ingredient is refused.
    let mut with_extra = grid2();
    with_extra[0] = stack(oak_log(), 1);
    with_extra[1] = stack(oak_planks(), 1);
    assert!(
        recipes.find_match(&with_extra, 2).is_none(),
        "a log plus a plank is not a shapeless planks recipe"
    );
    // Two logs are not one log either: the multiset must match exactly.
    let mut two_logs = grid2();
    two_logs[0] = stack(oak_log(), 1);
    two_logs[1] = stack(oak_log(), 1);
    assert!(
        recipes.find_match(&two_logs, 2).is_none(),
        "two logs are not a one-log recipe"
    );
}

#[test]
fn a_shapeless_recipe_with_alternatives_matches_any_alternative() {
    // Eleven "any planks" alternatives on one ingredient: every wood must satisfy
    // it, and a non-wood must not.
    let alternatives: Vec<i32> = [
        "minecraft:oak_planks",
        "minecraft:birch_planks",
        "minecraft:spruce_planks",
        "minecraft:jungle_planks",
    ]
    .iter()
    .map(|name| item(name))
    .collect();
    let ingredient = Ingredient::any_of(&alternatives).expect("four alternatives");
    assert_eq!(ingredient.item_ids(), {
        let mut sorted = alternatives.clone();
        sorted.sort_unstable();
        sorted
    });
    for alternative in &alternatives {
        assert!(
            ingredient.accepts(*alternative),
            "{alternative} must be accepted"
        );
    }
    assert!(!ingredient.accepts(cobblestone()));
    assert!(!ingredient.accepts(0), "air is never an ingredient");

    // A duplicate alternative is deduplicated rather than counted twice.
    let duplicated = Ingredient::any_of(&[oak_planks(), oak_planks()]).expect("duplicates");
    assert_eq!(duplicated.item_ids().len(), 1);

    // The cap is enforced.
    let too_many: Vec<i32> =
        (1..=i32::try_from(MAX_ALTERNATIVES_PER_KEY).expect("fits") + 1).collect();
    assert!(
        Ingredient::any_of(&too_many).is_err(),
        "more alternatives than the cap must be refused"
    );
    assert!(
        Ingredient::any_of(&[]).is_err(),
        "an ingredient nothing satisfies is a table bug"
    );

    // And a shapeless recipe over an alternative ingredient matches either wood.
    let recipe = Recipe::Shapeless(Box::new(
        ShapelessRecipe::new(
            "test_planks",
            vec![Ingredient::any_of(&alternatives).expect("alternatives")],
            item("minecraft:oak_slab"),
            6,
        )
        .expect("a valid shapeless recipe"),
    ));
    let registry = RecipeRegistry::new(vec![recipe]).expect("one recipe");
    for alternative in &alternatives {
        let mut grid = grid2();
        grid[3] = stack(*alternative, 1);
        assert!(
            registry.matches(&grid, 2),
            "item {alternative} must satisfy the ingredient"
        );
    }
    let mut wrong = grid2();
    wrong[0] = stack(cobblestone(), 1);
    assert!(!registry.matches(&wrong, 2));
}

// ------------------------------------------------------------ consumption

#[test]
fn crafting_consumes_exactly_one_of_each_ingredient() {
    let recipes = baseline();
    let mut grid = grid3();
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        grid[cell] = stack(cobblestone(), 10);
    }
    let before: i64 = grid.iter().map(|s| i64::from(s.count())).sum();
    let result = recipes
        .craft(&mut grid, 3, &items())
        .expect("crafting applies")
        .expect("the furnace ring matches");
    assert_eq!(result, stack(item("minecraft:furnace"), 1));
    let after: i64 = grid.iter().map(|s| i64::from(s.count())).sum();
    assert_eq!(after, before - 8, "exactly one cobblestone per ring cell");
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        assert_eq!(grid[cell], stack(cobblestone(), 9), "cell {cell}");
    }
    assert!(grid[4].is_empty(), "the centre stays empty");
}

#[test]
fn a_full_stack_grid_crafts_once_and_leaves_sixty_three() {
    let recipes = baseline();
    // Every ring cell holds 64 cobblestone.
    let mut grid = grid3();
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        grid[cell] = stack(cobblestone(), 64);
    }
    let before: i64 = grid.iter().map(|s| i64::from(s.count())).sum();
    assert_eq!(before, 8 * 64);
    let result = recipes
        .craft(&mut grid, 3, &items())
        .expect("crafting applies")
        .expect("the ring matches");
    assert_eq!(result.count(), 1, "one craft produces one furnace");
    for cell in [0, 1, 2, 3, 5, 6, 7, 8] {
        assert_eq!(
            grid[cell],
            stack(cobblestone(), 63),
            "cell {cell} must keep 63 items"
        );
    }
    let after: i64 = grid.iter().map(|s| i64::from(s.count())).sum();
    assert_eq!(
        after,
        before - 8,
        "one craft consumed exactly one item from each of the eight cells"
    );

    // A second craft empties nothing and still leaves 62 each.
    let second = recipes
        .craft(&mut grid, 3, &items())
        .expect("crafting applies")
        .expect("the ring still matches");
    assert_eq!(second.count(), 1);
    assert_eq!(grid[0], stack(cobblestone(), 62));
}

#[test]
fn a_single_item_cell_is_emptied_by_the_craft_that_uses_it() {
    let recipes = baseline();
    let mut grid = grid2();
    grid[0] = stack(oak_log(), 1);
    let result = recipes
        .craft(&mut grid, 2, &items())
        .expect("crafting applies")
        .expect("the log matches");
    assert_eq!(result, stack(oak_planks(), 4));
    assert!(
        grid[0].is_empty(),
        "a one-item cell must be left empty, not a zero-count stack"
    );
}
#[test]
fn crafting_leaves_the_rest_of_the_grid_intact() {
    let recipes = baseline();
    // A 2x2 grid holding exactly the 1x2 sticks pattern: the pattern is the whole
    // grid, so this is the shape Vanilla crafts from.
    let mut grid = grid2();
    grid[0] = stack(oak_planks(), 5);
    grid[2] = stack(oak_planks(), 5);
    let result = recipes
        .craft(&mut grid, 2, &items())
        .expect("crafting applies")
        .expect("the sticks column matches");
    assert_eq!(result, stack(stick(), 4));
    assert_eq!(grid[0], stack(oak_planks(), 4), "one plank consumed");
    assert_eq!(grid[2], stack(oak_planks(), 4), "one plank consumed");
    assert!(
        grid[1].is_empty() && grid[3].is_empty(),
        "the unused cells stay empty"
    );
}

/// A shaped recipe must fill the grid **exactly**, which is what stops a pattern
/// from matching a grid that merely contains it.
///
/// This is the rule that an earlier version of the matcher got wrong (it matched a
/// 1x2 pattern inside a 2x2 grid of planks, so four planks were read as two
/// disjoint sticks recipes instead of a crafting table). The corrected behaviour is
/// asserted here directly, and the earlier `a_two_by_two_recipe_matches_in_all_four_corners`
/// test checks the other half of the rule from the offset side.
#[test]
fn a_shaped_recipe_requires_the_grid_to_hold_nothing_else() {
    let recipes = baseline();
    // Two planks stacked in the left column of a 3x3 grid, with unrelated items in
    // the other columns: the leftover cells mean this is not the sticks recipe.
    let mut with_extras = grid3();
    with_extras[0] = stack(oak_planks(), 5);
    with_extras[3] = stack(oak_planks(), 5);
    with_extras[1] = stack(cobblestone(), 3);
    with_extras[8] = stack(stone(), 1);
    assert!(
        recipes.find_match(&with_extras, 3).is_none(),
        "a grid with items outside the pattern is not the pattern"
    );
    // Nothing was consumed by the refused match.
    let before = with_extras.clone();
    assert!(
        recipes
            .craft(&mut with_extras, 3, &items())
            .expect("no error")
            .is_none()
    );
    assert_eq!(
        with_extras, before,
        "a refused craft must not consume a grid"
    );

    // Remove the extras and the same two planks *do* craft, which is the premise
    // that makes the assertion above meaningful rather than vacuous.
    let mut clean = grid3();
    clean[0] = stack(oak_planks(), 5);
    clean[3] = stack(oak_planks(), 5);
    let result = recipes
        .craft(&mut clean, 3, &items())
        .expect("crafting applies")
        .expect("the sticks column matches once the extras are gone");
    assert_eq!(result, stack(stick(), 4));
    assert_eq!(clean[0], stack(oak_planks(), 4));
    assert_eq!(clean[3], stack(oak_planks(), 4));
    let untouched: Vec<usize> = (0..MAX_GRID_SLOTS).filter(|i| *i != 0 && *i != 3).collect();
    for index in untouched {
        assert!(
            clean[index].is_empty(),
            "cell {index} must be untouched by the craft"
        );
    }
}

#[test]
fn recompute_result_does_not_consume_and_reports_the_full_count() {
    let recipes = baseline();
    let mut grid = grid3();
    grid[0] = stack(oak_log(), 1);
    let unchanged = grid.clone();
    let result = recipes
        .recompute_result(&grid, 3, &items(), &sizes())
        .expect("recompute applies");
    assert_eq!(
        result,
        stack(oak_planks(), 4),
        "Vanilla's result slot shows the recipe's full output count"
    );
    assert_eq!(grid, unchanged, "recompute must not consume anything");

    // Calling it repeatedly is idempotent.
    let again = recipes
        .recompute_result(&grid, 3, &items(), &sizes())
        .expect("recompute applies");
    assert_eq!(again, result);
}

#[test]
fn the_result_is_never_larger_than_the_stack_that_holds_it() {
    let recipes = baseline();
    let sizes = sizes();
    let mut grid = grid3();
    grid[0] = stack(oak_log(), 1);
    let result = recipes
        .recompute_result(&grid, 3, &items(), &sizes)
        .expect("recompute applies");
    // The premise: this recipe's count is within the item's limit, so the clamp is
    // not silently truncating the baseline data.
    assert!(
        result.count() <= sizes.max_stack_size(result.item_id().expect("non-empty")),
        "the baseline result must fit its item's limit"
    );
    // And the clamp does its job when a recipe count exceeds the limit. A bucket
    // stacks to 16, so a recipe claiming 64 must come back as 16.
    assert_eq!(
        sizes.max_stack_size(item("minecraft:bucket")),
        16,
        "the premise: a bucket stacks to 16, so the clamp must bite"
    );
    let cobble = Ingredient::item(cobblestone());
    let silly = RecipeRegistry::new(vec![Recipe::Shaped(Box::new(
        ShapedRecipe::new(
            "test_oversized",
            ShapedPattern::from_rows(&[&[Some(cobble)]]).expect("1x1"),
            item("minecraft:bucket"),
            64,
        )
        .expect("64 is within the hard ceiling"),
    ))])
    .expect("one recipe");
    let mut grid = grid2();
    grid[0] = stack(cobblestone(), 1);
    let clamped = silly
        .recompute_result(&grid, 2, &items(), &sizes)
        .expect("recompute applies");
    assert_eq!(clamped.count(), 16, "clamped to the bucket's own limit");
    // `craft` is not clamped: it returns the recipe's own count, which is why the
    // result-slot path goes through `recompute_result`.
    let crafted = silly
        .craft(&mut grid, 2, &items())
        .expect("crafting applies")
        .expect("the 1x1 pattern matches");
    assert_eq!(crafted.count(), 64);
}

// ------------------------------------------------------------- the baseline

// One long test because it is a single property over the whole table (every recipe
// resolves *and* every recipe is reachable from its own ingredients); splitting it
// would duplicate the setup that makes the property meaningful.
#[allow(clippy::too_many_lines)]
#[test]
fn every_baseline_recipe_resolves_and_is_reachable_from_its_ingredients() {
    let items = items();
    let recipes = baseline();
    assert!(
        recipes.len() >= 10,
        "the baseline must cover the recipes the task names, got {}",
        recipes.len()
    );

    // Names are unique (also enforced by `new`) and non-empty.
    let names = recipes.names();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "recipe names must be unique");

    for recipe in recipes.recipes() {
        assert!(!recipe.name().is_empty());
        assert!(
            recipe.ingredient_count() >= 1,
            "{} has no ingredients",
            recipe.name()
        );
        // The result resolves by name and is a real item.
        let name = items
            .name(recipe.result())
            .expect("every baseline result must be a registered item");
        assert!(name.starts_with("minecraft:"), "{name} is not namespaced");
        assert!(recipe.result_count() >= 1);
        assert!(
            recipe.result_count() <= sizes().max_stack_size(recipe.result()),
            "{} produces {} items, more than its item's stack limit",
            recipe.name(),
            recipe.result_count()
        );
        // Every ingredient id resolves, and air is never an ingredient.
        match recipe {
            Recipe::Shaped(shaped) => {
                let shape = shaped.pattern().shape();
                assert!(!shape.is_empty());
                assert!(
                    shape.width() >= 1 && shape.width() <= 3,
                    "{}",
                    recipe.name()
                );
                assert!(
                    shape.height() >= 1 && shape.height() <= 3,
                    "{}",
                    recipe.name()
                );
                for cell in shaped.pattern().cells().iter().flatten() {
                    assert!(!cell.item_ids().is_empty());
                    for item_id in cell.item_ids() {
                        items
                            .entry(*item_id)
                            .expect("an ingredient must be registered");
                        assert_ne!(*item_id, 0, "air is not an ingredient");
                    }
                }
            }
            Recipe::Shapeless(shapeless) => {
                assert!(!shapeless.ingredients().is_empty());
                for ingredient in shapeless.ingredients() {
                    assert!(!ingredient.item_ids().is_empty());
                    for item_id in ingredient.item_ids() {
                        items
                            .entry(*item_id)
                            .expect("an ingredient must be registered");
                        assert_ne!(*item_id, 0, "air is not an ingredient");
                    }
                }
            }
        }
    }

    // Reachability: build a grid out of the *first* alternative of every
    // ingredient and check that this exact recipe is what matches it. For the
    // shaped recipes the first alternative of `any planks` (oak) is used, which is
    // the wood the baseline's own results assume.
    let mut checked = 0usize;
    for recipe in recipes.recipes() {
        let expected = recipe.name().to_owned();
        match recipe {
            Recipe::Shaped(shaped) => {
                let shape = shaped.pattern().shape();
                assert!(
                    shape.width() <= 3 && shape.height() <= 3,
                    "{expected} does not fit a 3x3 grid"
                );
                let mut grid = vec![ItemStack::EMPTY; MAX_GRID_SLOTS];
                for y in 0..shape.height() {
                    for x in 0..shape.width() {
                        if let Some(ingredient) = shaped.pattern().at(x, y) {
                            let item_id = ingredient.item_ids()[0];
                            grid[y * 3 + x] = stack(item_id, 1);
                        }
                    }
                }
                assert!(
                    recipes.matches_with(&grid, 3, &items).expect("ids resolve"),
                    "{expected} must be reachable from its own pattern"
                );
                checked += 1;
            }
            Recipe::Shapeless(shapeless) => {
                let mut grid = vec![ItemStack::EMPTY; MAX_GRID_SLOTS];
                for (index, ingredient) in shapeless.ingredients().iter().enumerate() {
                    grid[index] = stack(ingredient.item_ids()[0], 1);
                }
                assert!(
                    recipes.matches_with(&grid, 3, &items).expect("ids resolve"),
                    "{expected} must be reachable from its own ingredients"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, recipes.len(), "every recipe was checked");
}

#[test]
fn the_named_baseline_recipes_are_present_with_the_documented_results() {
    let recipes = baseline();
    let expectations: [(&str, &str, i32); 10] = [
        (
            "baseline_planks_from_minecraft:oak_log",
            "minecraft:oak_planks",
            4,
        ),
        (
            "baseline_planks_from_minecraft:birch_log",
            "minecraft:birch_planks",
            4,
        ),
        (
            "baseline_planks_from_minecraft:crimson_stem",
            "minecraft:crimson_planks",
            4,
        ),
        ("baseline_sticks_from_planks", "minecraft:stick", 4),
        ("baseline_crafting_table", "minecraft:crafting_table", 1),
        ("baseline_furnace", "minecraft:furnace", 1),
        ("baseline_chest", "minecraft:chest", 1),
        ("baseline_torch", "minecraft:torch", 4),
        ("baseline_torch_from_charcoal", "minecraft:torch", 4),
        ("baseline_stone", "minecraft:stone", 1),
    ];
    for (name, result_name, count) in expectations {
        let recipe = recipes
            .by_name(name)
            .unwrap_or_else(|| panic!("{name} must be in the baseline"));
        assert_eq!(
            recipe.result(),
            item(result_name),
            "{name} must produce {result_name}"
        );
        assert_eq!(recipe.result_count(), count, "{name} count");
    }
    // The torch pair share a result but differ in ingredient: charcoal must not
    // satisfy the coal recipe, and vice versa.
    let mut coal_torch = grid3();
    coal_torch[0] = stack(item("minecraft:coal"), 1);
    coal_torch[3] = stack(stick(), 1);
    assert_eq!(
        recipes
            .find_match(&coal_torch, 3)
            .expect("coal torch")
            .recipe()
            .name(),
        "baseline_torch"
    );
    let mut charcoal_torch = grid3();
    charcoal_torch[0] = stack(item("minecraft:charcoal"), 1);
    charcoal_torch[3] = stack(stick(), 1);
    assert_eq!(
        recipes
            .find_match(&charcoal_torch, 3)
            .expect("charcoal torch")
            .recipe()
            .name(),
        "baseline_torch_from_charcoal"
    );
}

#[test]
fn each_wood_log_produces_its_own_planks() {
    let recipes = baseline();
    for (log, planks) in [
        ("minecraft:oak_log", "minecraft:oak_planks"),
        ("minecraft:birch_log", "minecraft:birch_planks"),
        ("minecraft:spruce_log", "minecraft:spruce_planks"),
        ("minecraft:jungle_log", "minecraft:jungle_planks"),
        ("minecraft:acacia_log", "minecraft:acacia_planks"),
        ("minecraft:dark_oak_log", "minecraft:dark_oak_planks"),
        ("minecraft:mangrove_log", "minecraft:mangrove_planks"),
        ("minecraft:cherry_log", "minecraft:cherry_planks"),
        ("minecraft:pale_oak_log", "minecraft:pale_oak_planks"),
        ("minecraft:crimson_stem", "minecraft:crimson_planks"),
        ("minecraft:warped_stem", "minecraft:warped_planks"),
    ] {
        let mut grid = grid2();
        grid[0] = stack(item(log), 1);
        let found = recipes
            .find_match(&grid, 2)
            .unwrap_or_else(|| panic!("{log} must match a planks recipe"));
        assert_eq!(found.recipe().result(), item(planks), "{log} -> {planks}");
        assert_eq!(found.recipe().result_count(), 4);
    }
}

// --------------------------------------------------------------- hostile

#[test]
fn hostile_grids_produce_no_result_and_never_panic() {
    let recipes = baseline();
    let items = items();
    let sizes = sizes();

    // Empty grid.
    let empty = grid3();
    assert!(!recipes.matches(&empty, 3));
    assert!(!recipes.matches_with(&empty, 3, &items).expect("resolves"));
    let mut empty_mut = grid3();
    assert!(
        recipes
            .craft(&mut empty_mut, 3, &items)
            .expect("no error")
            .is_none()
    );
    assert_eq!(
        recipes
            .recompute_result(&empty, 3, &items, &sizes)
            .expect("no error"),
        ItemStack::EMPTY
    );

    // Zero width, and a width above the limit: not a grid at all.
    for width in [0usize, 4, 5, 64, usize::MAX] {
        assert!(!recipes.matches(&empty, width), "width {width}");
        assert!(
            !recipes
                .matches_with(&empty, width, &items)
                .expect("resolves"),
            "width {width}"
        );
        let mut grid = empty.clone();
        assert!(
            recipes
                .craft(&mut grid, width, &items)
                .expect("no error")
                .is_none(),
            "width {width}"
        );
    }

    // A grid that is too small for the recipe: one cell cannot hold the 2x2
    // crafting table, whatever its width.
    let mut tiny = vec![ItemStack::EMPTY; 1];
    tiny[0] = stack(oak_planks(), 1);
    assert!(!recipes.matches(&tiny, 1));
    let mut tiny = vec![ItemStack::EMPTY; 2];
    tiny[0] = stack(oak_planks(), 1);
    tiny[1] = stack(oak_planks(), 1);
    assert!(!recipes.matches(&tiny, 2), "a 1x2 grid cannot hold 2x2");

    // A ragged length (not a multiple of the width) is refused.
    let mut ragged = vec![ItemStack::EMPTY; 5];
    ragged[0] = stack(oak_planks(), 1);
    assert!(!recipes.matches(&ragged, 2));

    // More slots than the largest grid.
    let mut oversized = vec![ItemStack::EMPTY; MAX_GRID_SLOTS + 1];
    oversized[0] = stack(oak_planks(), 1);
    assert!(!recipes.matches(&oversized, 3));

    // A grid of air stacks (the "contains empty stacks" case) matches nothing.
    let mut air = grid3();
    for slot in &mut air {
        *slot = ItemStack::new(0, 10).expect("air normalises to empty");
    }
    assert!(air.iter().all(ItemStack::is_empty));
    assert!(!recipes.matches(&air, 3));

    // Partially air-filled: the sticks recipe needs both cells.
    let mut partial = grid3();
    partial[0] = stack(oak_planks(), 1);
    assert!(
        recipes.find_match(&partial, 3).is_none(),
        "one plank is not two planks"
    );

    // A malformed grid leaves the caller's array exactly as it was.
    let before = partial.clone();
    assert!(
        recipes
            .craft(&mut partial, 0, &items)
            .expect("no error")
            .is_none()
    );
    assert_eq!(partial, before, "a refused craft must not mutate the grid");
}

#[test]
fn an_empty_registry_matches_nothing() {
    let recipes = RecipeRegistry::empty();
    let mut grid = grid2();
    grid[0] = stack(oak_log(), 1);
    assert!(recipes.is_empty());
    assert!(!recipes.matches(&grid, 2));
    assert!(
        recipes
            .craft(&mut grid, 2, &items())
            .expect("no error")
            .is_none()
    );
    assert_eq!(grid[0], stack(oak_log(), 1), "nothing was consumed");
}

// ----------------------------------------------------------- construction

#[test]
fn malformed_recipes_are_refused_at_construction() {
    // An empty ingredient list.
    assert!(Ingredient::any_of(&[]).is_err());
    // A ragged pattern.
    let a = Ingredient::item(1);
    assert!(
        ShapedPattern::from_rows(&[&[Some(a.clone())], &[Some(a.clone()), Some(a.clone())]])
            .is_err()
    );
    // An empty pattern.
    assert!(ShapedPattern::from_rows(&[]).is_err());
    assert!(ShapedPattern::from_rows(&[&[]]).is_err());
    assert!(ShapedPattern::from_rows(&[&[None, None]]).is_err());
    // A pattern that does not fit a 3x3 grid.
    let row = [
        Some(a.clone()),
        Some(a.clone()),
        Some(a.clone()),
        Some(a.clone()),
    ];
    assert!(ShapedPattern::from_rows(&[&row]).is_err());
    let single_row = [Some(a.clone())];
    let four_rows: Vec<&[Option<Ingredient>]> = vec![&single_row; 4];
    assert!(ShapedPattern::from_rows(&four_rows).is_err());

    // Trailing empty rows and columns are trimmed, so a 3x3 drawing of a 1x2
    // recipe is a 1x2 pattern.
    let trimmed = ShapedPattern::from_rows(&[
        &[Some(a.clone()), None, None],
        &[Some(a.clone()), None, None],
        &[None, None, None],
    ])
    .expect("trailing emptiness is trimmed");
    assert_eq!(trimmed.shape(), Shape::new(1, 2));
    assert_eq!(trimmed.at(0, 0), Some(&a));
    assert_eq!(trimmed.at(0, 1), Some(&a));
    assert_eq!(trimmed.at(1, 0), None, "outside the pattern");
    assert_eq!(trimmed.at(0, 9), None, "outside the pattern");

    // A result count outside 1..=64.
    let pattern = ShapedPattern::from_rows(&[&[Some(a.clone())]]).expect("1x1");
    assert!(ShapedRecipe::new("zero", pattern.clone(), 1, 0).is_err());
    assert!(ShapedRecipe::new("too_many", pattern.clone(), 1, 65).is_err());
    assert!(ShapedRecipe::new("", pattern, 1, 1).is_err());

    // A shapeless recipe with no ingredients, or more than a grid can hold.
    assert!(ShapelessRecipe::new("empty", Vec::new(), 1, 1).is_err());
    assert!(
        ShapelessRecipe::new("huge", vec![Ingredient::item(1); MAX_GRID_SLOTS + 1], 1, 1).is_err()
    );
    assert!(ShapelessRecipe::new("zero", vec![Ingredient::item(1)], 1, 0).is_err());

    // Duplicate names are refused: "which recipe matched" must be answerable.
    let one = Recipe::Shapeless(Box::new(
        ShapelessRecipe::new("same", vec![Ingredient::item(1)], 2, 1).expect("valid"),
    ));
    let two = Recipe::Shapeless(Box::new(
        ShapelessRecipe::new("same", vec![Ingredient::item(2)], 3, 1).expect("valid"),
    ));
    assert!(RecipeRegistry::new(vec![one.clone()]).is_ok());
    assert!(RecipeRegistry::new(vec![one, two]).is_err());
}

#[test]
fn a_missing_item_name_is_corrupt_data_not_a_silent_skip() {
    // A registry holding only air: the baseline cannot resolve, and says which
    // item is missing rather than quietly dropping recipes.
    let tiny = ItemRegistry::parse("0\tminecraft:air\t-\n").expect("parses");
    let error = RecipeRegistry::baseline(&tiny).expect_err("the baseline cannot resolve");
    assert!(
        matches!(error, mc_core::error::ServerError::CorruptData(_)),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(
        text.contains("minecraft:oak_log"),
        "the error must name the missing item, got {text:?}"
    );
}

#[test]
fn a_recipe_whose_result_is_unregistered_is_corrupt_data() {
    // Build a registry over an id the item registry does not contain: matching
    // succeeds structurally, and asking the registry-aware form reports it.
    let a = Ingredient::item(1);
    let recipe = Recipe::Shaped(Box::new(
        ShapedRecipe::new(
            "test_bogus_result",
            ShapedPattern::from_rows(&[&[Some(a)]]).expect("1x1"),
            900_000,
            1,
        )
        .expect("valid"),
    ));
    let recipes = RecipeRegistry::new(vec![recipe]).expect("one recipe");
    let mut grid = grid2();
    grid[0] = stack(stone(), 1);
    assert!(
        recipes.matches(&grid, 2),
        "the pure form matches structurally"
    );
    let error = recipes
        .matches_with(&grid, 2, &items())
        .expect_err("an unregistered result is corrupt data");
    assert!(
        matches!(error, mc_core::error::ServerError::CorruptData(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("900000"), "{error:?}");
    assert!(
        recipes.craft(&mut grid, 2, &items()).is_err(),
        "craft must refuse rather than build an unnameable stack"
    );
    assert_eq!(
        grid[0],
        stack(stone(), 1),
        "a refused craft must not consume the grid"
    );
}

#[test]
fn recipe_kinds_and_names_are_reported_faithfully() {
    let recipes = baseline();
    let shaped = recipes
        .by_name("baseline_crafting_table")
        .expect("the crafting table");
    assert_eq!(shaped.kind(), RecipeKind::Shaped);
    assert_eq!(shaped.kind().name(), "shaped");
    let shapeless = recipes
        .by_name("baseline_planks_from_minecraft:oak_log")
        .expect("the log");
    assert_eq!(shapeless.kind(), RecipeKind::Shapeless);
    assert_eq!(shapeless.kind().name(), "shapeless");
    assert!(
        recipes.by_name("no_such_recipe").is_none(),
        "an unknown name is not a recipe"
    );
}

#[test]
fn crafted_items_are_consumed_from_a_container_backed_grid() {
    // The integration shape the server uses: the grid lives in a `Container` (the
    // 4-slot grid container of `Menu::player`, which the menu maps to slots 1..=4),
    // and a craft is one write per changed slot, so `changed()` names exactly the
    // slots the client must be told about.
    let recipes = baseline();
    let mut grid_container = Container::new(crate::container::ContainerKind::Crafting, 4)
        .expect("a 4-slot crafting grid, matching Menu::player's grid container");
    grid_container
        .set(0, stack(oak_planks(), 3))
        .expect("cell 0");
    grid_container
        .set(2, stack(oak_planks(), 3))
        .expect("cell 2");
    grid_container.clear_changed();

    let mut grid = grid_container.slots().to_vec();
    assert_eq!(grid.len(), 4, "a width-2 grid is four slots");
    let result = recipes
        .craft(&mut grid, 2, &items())
        .expect("crafting applies")
        .expect("the stick column matches");
    assert_eq!(result, stack(stick(), 4));
    for (index, value) in grid.iter().enumerate() {
        grid_container.set(index, *value).expect("write back");
    }
    assert_eq!(grid_container.get(0), stack(oak_planks(), 2));
    assert_eq!(grid_container.get(2), stack(oak_planks(), 2));
    assert_eq!(
        grid_container.changed().iter().copied().collect::<Vec<_>>(),
        vec![0, 2],
        "only the two consumed slots changed"
    );
    assert_eq!(grid_container.total_items(), 4);

    // The other integration shape, and the trap an earlier version of this test fell
    // into: `ContainerKind::Crafting`'s *default* size is 10 slots (Vanilla's
    // result-plus-grid block entity layout), which is **not** a grid. Passing all ten
    // as one is refused rather than misread as a 2x5 grid, and refused rather than
    // panicking on the height.
    let whole = Container::with_default_size(crate::container::ContainerKind::Crafting)
        .expect("a 10-slot crafting container");
    assert_eq!(whole.len(), 10);
    let mut not_a_grid = whole.slots().to_vec();
    not_a_grid[0] = stack(oak_planks(), 1);
    not_a_grid[2] = stack(oak_planks(), 1);
    assert!(
        !recipes.matches(&not_a_grid, 2),
        "a 10-slot container is not a 2x5 grid; the height limit must refuse it"
    );
    assert!(
        recipes
            .craft(&mut not_a_grid, 2, &items())
            .expect("no error")
            .is_none(),
        "and it must refuse to craft from it"
    );
    assert_eq!(
        not_a_grid[0],
        stack(oak_planks(), 1),
        "nothing was consumed by the refusal"
    );
}

/// P17-03: a pack book expands tag ingredients when a resolver is supplied,
/// and a simple transmute becomes a two-ingredient shapeless recipe while a
/// multi-material one is counted as complex rather than flattened.
#[test]
fn a_pack_book_expands_tags_and_converts_simple_transmute() {
    use mc_core::ids::ResourceId;
    use mc_data::{
        Ingredient as DataIngredient, Recipe as DataRecipe, RecipeBook, TransmuteRecipe,
    };
    use std::collections::BTreeMap;

    fn id(text: &str) -> ResourceId {
        ResourceId::parse(text).expect("a valid id")
    }
    let mut book = RecipeBook::new();
    // Sticks from a planks tag — the real pack's shape.
    book.insert(DataRecipe::Shaped(mc_data::ShapedRecipe {
        name: id("minecraft:test_sticks"),
        pattern: vec!["#".to_owned(), "#".to_owned()],
        key: BTreeMap::from([('#', vec![DataIngredient::Tag(id("minecraft:planks"))])]),
        result: id("minecraft:stick"),
        result_count: 4,
        group: None,
    }));
    // Simple dye transmute: bundle + black dye → black bundle.
    book.insert(DataRecipe::Transmute(TransmuteRecipe {
        name: id("minecraft:black_bundle"),
        input: vec![DataIngredient::Item(id("minecraft:bundle"))],
        material: vec![DataIngredient::Item(id("minecraft:black_dye"))],
        result: id("minecraft:black_bundle"),
        result_count: 1,
        material_count_min: 1,
        material_count_max: 1,
        add_material_count_to_result: false,
        group: None,
    }));
    // Map cloning is the multi-material shape and stays counted, not guessed.
    book.insert(DataRecipe::Transmute(TransmuteRecipe {
        name: id("minecraft:map_cloning"),
        input: vec![DataIngredient::Item(id("minecraft:filled_map"))],
        material: vec![DataIngredient::Item(id("minecraft:map"))],
        result: id("minecraft:filled_map"),
        result_count: 1,
        material_count_min: 1,
        material_count_max: 8,
        add_material_count_to_result: true,
        group: None,
    }));

    let oak_planks = id("minecraft:oak_planks");
    let birch_planks = id("minecraft:birch_planks");
    let resolve_tag = &|tag: &ResourceId| -> Vec<ResourceId> {
        if tag.value() == "planks" {
            vec![oak_planks.clone(), birch_planks.clone()]
        } else {
            Vec::new()
        }
    };

    let (table, report) =
        RecipeRegistry::from_book(&book, &items(), Some(resolve_tag)).expect("converts");
    assert_eq!(report.shaped_seen, 1, "{report:?}");
    assert_eq!(report.transmute_seen, 2, "{report:?}");
    assert_eq!(report.complex_transmute, 1, "{report:?}");
    assert_eq!(report.converted, 2, "{report:?}");
    assert_eq!(report.tag_or_unknown, 0, "{report:?}");
    assert_eq!(table.len(), 2);

    // Tag-expanded sticks match from either plank alternative.
    for plank in [item("minecraft:oak_planks"), item("minecraft:birch_planks")] {
        let mut grid = vec![ItemStack::EMPTY; 4];
        grid[0] = stack(plank, 1);
        grid[2] = stack(plank, 1);
        assert!(table.matches(&grid, 2), "plank {plank} must craft sticks");
        let mut crafting = grid.clone();
        let made = table
            .craft(&mut crafting, 2, &items())
            .expect("crafts")
            .expect("a result");
        assert_eq!(made.item_id(), Some(item("minecraft:stick")));
        assert_eq!(made.count(), 4);
    }

    // Simple transmute: one bundle + one black dye → black bundle.
    let mut grid = vec![ItemStack::EMPTY; 4];
    grid[0] = stack(item("minecraft:bundle"), 1);
    grid[1] = stack(item("minecraft:black_dye"), 1);
    assert!(table.matches(&grid, 2), "the dye transmute must match");
    let mut crafting = grid.clone();
    let made = table
        .craft(&mut crafting, 2, &items())
        .expect("crafts")
        .expect("a result");
    assert_eq!(made.item_id(), Some(item("minecraft:black_bundle")));
    assert_eq!(made.count(), 1);

    // Without a resolver the tag sticks refuse (counted, not guessed).
    let (narrow, report) = RecipeRegistry::from_book(&book, &items(), None).expect("converts");
    assert_eq!(report.converted, 1, "{report:?}");
    assert_eq!(report.tag_or_unknown, 1, "{report:?}");
    assert_eq!(narrow.len(), 1);
}

/// P12-07: a pack book converts to a matching table (item-only).
///
/// A hand-built book with one shaped (sticks) and one shapeless (planks)
/// recipe converts to two table entries that match and craft; a tag-ingredient
/// recipe and a cooking recipe are counted and skipped without a resolver,
/// not guessed.
#[test]
fn a_pack_book_converts_item_recipes_and_counts_the_rest() {
    use mc_core::ids::ResourceId;
    use mc_data::{Ingredient as DataIngredient, Recipe as DataRecipe, RecipeBook};
    use std::collections::BTreeMap;

    fn id(text: &str) -> ResourceId {
        ResourceId::parse(text).expect("a valid id")
    }
    let mut book = RecipeBook::new();
    // Shaped sticks: two oak planks stacked (pattern 1x2).
    book.insert(DataRecipe::Shaped(mc_data::ShapedRecipe {
        name: id("minecraft:test_sticks"),
        pattern: vec!["#".to_owned(), "#".to_owned()],
        key: BTreeMap::from([('#', vec![DataIngredient::Item(id("minecraft:oak_planks"))])]),
        result: id("minecraft:stick"),
        result_count: 4,
        group: None,
    }));
    // Shapeless planks: one oak log.
    book.insert(DataRecipe::Shapeless(mc_data::ShapelessRecipe {
        name: id("minecraft:test_planks"),
        ingredients: vec![vec![DataIngredient::Item(id("minecraft:oak_log"))]],
        result: id("minecraft:oak_planks"),
        result_count: 4,
        group: None,
    }));
    // Tag recipe: skipped, counted.
    book.insert(DataRecipe::Shapeless(mc_data::ShapelessRecipe {
        name: id("minecraft:test_tag"),
        ingredients: vec![vec![DataIngredient::Tag(id("minecraft:planks"))]],
        result: id("minecraft:stick"),
        result_count: 1,
        group: None,
    }));
    // Cooking recipe: other kind, counted.
    book.insert(DataRecipe::Cooking(mc_data::CookingRecipe {
        name: id("minecraft:test_cooking"),
        kind: mc_data::SmeltingKind::Smelting,
        ingredient: vec![DataIngredient::Item(id("minecraft:raw_iron"))],
        result: id("minecraft:iron_ingot"),
        result_count: 1,
        cooking_time: 200,
        experience: 0.7,
    }));

    let (table, report) = RecipeRegistry::from_book(&book, &items(), None).expect("converts");
    assert_eq!(report.converted, 2, "{report:?}");
    assert_eq!(report.tag_or_unknown, 1, "{report:?}");
    assert_eq!(report.other_kinds, 1, "{report:?}");
    assert_eq!(table.len(), 2);

    // The converted sticks match in a 2x2 grid and craft.
    let planks = item("minecraft:oak_planks");
    let mut grid = vec![ItemStack::EMPTY; 4];
    grid[0] = stack(planks, 1);
    grid[2] = stack(planks, 1);
    assert!(table.matches(&grid, 2), "pack sticks must match");
    let mut crafting = grid.clone();
    let result = table
        .craft(&mut crafting, 2, &items())
        .expect("crafts")
        .expect("a result");
    assert_eq!(result.item_id(), Some(item("minecraft:stick")));
    assert_eq!(result.count(), 4);
}
