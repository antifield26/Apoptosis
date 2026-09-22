//! Differential: build the crafting table from the **real** 26.1.2 recipes
//! (P12-07, P17-03).
//!
//! The unit tests use a hand-built book, which proves the conversion but not
//! that it handles what the jar ships. This one loads the actual `recipe/`
//! directory, expands item tags, converts shaped/shapeless/simple transmute
//! into a [`RecipeRegistry`], and asserts the census figures. Complex
//! transmutes (map cloning) stay counted rather than flattened.
//!
//! ```text
//! set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-container --test vanilla_crafting -- --ignored --nocapture
//! ```

use mc_container::crafting::RecipeRegistry;
use mc_core::ids::ResourceId;
use mc_data::recipe::{RecipeLoadReport, load_directory};
use mc_data::tag::{RegistryContents, TagKey, load_directory as load_tags};
use mc_data::{Limits, RecipeBook, TagLoadReport, TagSet};
use mc_registry::ItemRegistry;
use std::collections::BTreeSet;
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

    // Resolve item tags the same way the server does (P17-03).
    let item_ids: BTreeSet<ResourceId> = registry
        .names()
        .filter_map(|name| ResourceId::parse(name).ok())
        .collect();
    let mut contents = RegistryContents::new();
    contents.insert("item", item_ids);
    let mut tag_report = TagLoadReport::default();
    let files = load_tags(&root, "minecraft", Limits::DEFAULT, &mut tag_report);
    let tags = TagSet::from_map(mc_data::resolve_all(&files, &contents, &mut tag_report));
    let resolve_tag = &|tag: &ResourceId| {
        tags.get(&TagKey {
            registry: "item".to_owned(),
            name: tag.clone(),
        })
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default()
    };

    let (table, conversion) =
        RecipeRegistry::from_book(&book, &registry, Some(resolve_tag)).expect("converts");
    // With tags expanded, hundreds more convert than the item-only subset.
    // Bounds, not exact census: the pack moves. The report carries the numbers.
    assert!(
        conversion.converted > 400,
        "tag expansion must convert hundreds more than item-only, saw {conversion:?}"
    );
    assert!(
        conversion.transmute_seen >= 33,
        "the pack ships 33 transmute recipes, saw {conversion:?}"
    );
    assert!(
        conversion.complex_transmute >= 1,
        "map_cloning must stay counted as complex, saw {conversion:?}"
    );
    assert!(!table.is_empty(), "the converted table must not be empty");
    // Sticks are tag-shaped in the pack (`#minecraft:planks`) and must survive
    // the expansion under some name.
    let names = table.names().join(",");
    assert!(
        names.contains("stick"),
        "a stick recipe must convert, saw {names:?}"
    );
    // A dye transmute converts: black_bundle is in the table's names.
    assert!(
        names.contains("black_bundle"),
        "a simple dye transmute must convert, saw {names:?}"
    );
}
