//! Differential: build the crafting table from the **real** 26.1.2 recipes
//! (P12-07).
//!
//! The unit tests use a hand-built book, which proves the conversion but not
//! that it handles what the jar ships. This one loads the actual `recipe/`
//! directory, converts item-only `crafting_shaped`/`crafting_shapeless` into a
//! [`RecipeRegistry`], and asserts the census figures. Tag recipes are counted
//! and skipped (no resolver here) — the report is the evidence, not a silent
//! drop.
//!
//! ```text
//! set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-container --test vanilla_crafting -- --ignored --nocapture
//! ```

use mc_container::crafting::RecipeRegistry;
use mc_data::recipe::{RecipeLoadReport, load_directory};
use mc_data::{Limits, RecipeBook};
use mc_registry::ItemRegistry;
use std::path::{Path, PathBuf};

fn pack_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(root.is_dir(), "{} is not a directory", root.display());
    Some(root)
}

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the item fixture loads")
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_crafting_table_is_built_from_the_real_recipes() {
    let Some(root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let registry = items();

    let mut report = RecipeLoadReport::default();
    let loaded = load_directory(&root, "minecraft", Limits::DEFAULT, &mut report);
    let mut book = RecipeBook::new();
    for recipe in loaded {
        book.insert(recipe);
    }
    assert!(
        book.len() > 1000,
        "the real pack holds 1 500+ recipes, saw {}",
        book.len()
    );

    let (table, conversion) = RecipeRegistry::from_book(&book, &registry).expect("converts");
    // Item-only subset converts; tag recipes are the counted remainder. The
    // exact split moves with the pack, so the assertions are bounds, not exact
    // census figures — the report carries the exact numbers.
    assert!(
        conversion.converted > 200,
        "hundreds of item-only recipes must convert, saw {conversion:?}"
    );
    assert!(!table.is_empty(), "the converted table must not be empty");
    // Sticks are item-only in the pack (planks + planks) and must survive the
    // conversion under some name.
    let names = table.names().join(",");
    assert!(
        names.contains("stick"),
        "a stick recipe must convert, saw {names:?}"
    );
}
