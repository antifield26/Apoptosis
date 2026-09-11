//! Differential test: load the **real** 26.1.2 data pack (P07-03, P07-18).
//!
//! `mc-data`'s unit tests use hand-written fixtures, which proves the parser but not
//! that it handles the data a real jar ships. This test loads the actual
//! `data/minecraft/` tree and asserts the figures `docs/research/data-pack-baseline.md`
//! measured from the jar.
//!
//! It is `#[ignore]`d because it needs `MC_VANILLA_DATA` -- a directory holding the
//! extracted `data/minecraft/` -- which is not committed (8.5 MiB of Mojang data).
//! `docs/research/data-pack-baseline.md` section 0 gives the extraction command; it is
//! one `python -c` line using only the standard library, so it is reproducible *from
//! the documentation* rather than from an uncommitted script -- which Audit 05 flagged
//! as a circularity in the project's other regeneration claims.
//!
//! ```text
//! set MC_VANILLA_DATA=target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-data --test vanilla_pack -- --ignored --nocapture
//! ```
//!
//! Why this is worth having: a hand-written fixture cannot catch a format detail that
//! only appears in real data -- 384 nested tag references, the dict-form entries, the
//! 21 recipe types -- and those are exactly where a loader written from a description
//! goes wrong.

use mc_core::ids::ResourceId;
use mc_data::recipe::{RecipeLoadReport, load_directory as load_recipes};
use mc_data::resolve_all;
use mc_data::tag::{RegistryContents, load_directory as load_tags};
use mc_data::{Ingredient, Limits, Recipe, RecipeBook, RecipeKind, TagKey, TagLoadReport, TagSet};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the extracted vanilla pack lives, or `None` when the variable is unset.
fn pack_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(
        root.is_dir(),
        "MC_VANILLA_DATA points at {}, which is not a directory. See \
         docs/research/data-pack-baseline.md section 0 for the extraction command.",
        root.display()
    );
    assert!(
        root.join("tags").is_dir() && root.join("recipe").is_dir(),
        "{} does not look like an extracted data/minecraft (no tags/ or recipe/)",
        root.display()
    );
    Some(root)
}

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid resource id")
}

/// Declare the registries the vanilla pack references, from our own id tables.
///
/// Returns the counts too, so the output says what was actually *known* rather than
/// leaving "why did no id come back missing" as an open question. Both item and block
/// are declared because leaving a registry undeclared makes the loader silently accept
/// every reference into it, which would make this test weaker than it looks.
fn registry_contents() -> (RegistryContents, usize, usize) {
    let items = mc_registry::ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the item fixture loads");
    let blocks = mc_registry::BlockRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/blocks.tsv"),
    )
    .expect("the block fixture loads");

    let item_ids: BTreeSet<ResourceId> = items.names().filter_map(|n| id(n).into()).collect();
    let block_ids: BTreeSet<ResourceId> =
        blocks.block_names().filter_map(|n| id(n).into()).collect();

    let mut contents = RegistryContents::new();
    contents.insert("item", item_ids.clone());
    contents.insert("block", block_ids.clone());
    (contents, item_ids.len(), block_ids.len())
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_real_vanilla_pack_loads_without_a_single_problem() {
    let Some(pack_root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    check_tags(&pack_root);
    check_recipes(&pack_root);
}

/// Tags: resolve the real pack against our own registries and assert it is clean.
fn check_tags(pack_root: &Path) {
    let (contents, item_count, block_count) = registry_contents();
    let mut report = TagLoadReport::default();
    let files = load_tags(pack_root, "minecraft", Limits::DEFAULT, &mut report);
    let resolved = TagSet::from_map(resolve_all(&files, &contents, &mut report));

    println!("--- vanilla tags ---");
    println!("files merged      : {}", report.files);
    println!("tags resolved     : {}", resolved.len());
    println!("ids total         : {}", resolved.total_entries());
    println!("registries        : {}", resolved.registries().len());
    println!("skipped files     : {}", report.skipped.len());
    println!("problems          : {}", report.problems.len());
    println!("known ids         : {item_count} items, {block_count} blocks");
    for problem in report.problems.iter().take(15) {
        println!("  problem: {problem}");
    }
    for skipped in report.skipped.iter().take(15) {
        println!("  skipped: {skipped}");
    }

    assert_eq!(
        report.files, 758,
        "the baseline measured 758 tag files in the 26.1.2 jar"
    );
    assert!(
        report.skipped.is_empty(),
        "no vanilla tag file should be unreadable: {:?}",
        report.skipped
    );
    assert!(
        report.cycles().is_empty(),
        "vanilla has no tag cycles; a cycle means the resolver is wrong"
    );
    // The strong claim: with the item and block registries declared known, every
    // reference in vanilla's own pack resolves. One problem means the loader disagrees
    // with the data it was built from.
    assert!(
        report.problems.is_empty(),
        "vanilla's pack must resolve cleanly against its own registries: {:?}",
        report.problems
    );
    assert!(
        resolved.len() >= 750,
        "expected most of the 758 tags, got {}",
        resolved.len()
    );

    // Spot-check a flat tag.
    let planks = TagKey::parse("item/minecraft:planks").expect("a valid key");
    let planks_ids = resolved.get(&planks).expect("minecraft:planks exists");
    println!("minecraft:planks  : {} species", planks_ids.len());
    assert!(
        planks_ids.len() >= 11,
        "the planks tag should hold at least 11 species, got {}",
        planks_ids.len()
    );
    assert!(planks_ids.contains(&id("minecraft:oak_planks")));

    // Spot-check transitivity with a tag written as *only* two references.
    let hanging = TagKey::parse("block/minecraft:all_hanging_signs").expect("a valid key");
    let hanging_ids = resolved.get(&hanging).expect("all_hanging_signs exists");
    println!(
        "all_hanging_signs : {} ids through 2 nested references",
        hanging_ids.len()
    );
    assert!(
        !hanging_ids.is_empty(),
        "all_hanging_signs is two nested references and no concrete ids, so an empty \
         result means transitivity never ran"
    );
}

/// Recipes: load the real pack and assert the per-type census.
fn check_recipes(pack_root: &Path) {
    let mut recipes = RecipeLoadReport::default();
    let loaded = load_recipes(pack_root, "minecraft", Limits::DEFAULT, &mut recipes);
    let mut book = RecipeBook::new();
    for recipe in loaded {
        book.insert(recipe);
    }

    println!("--- vanilla recipes ---");
    println!("files             : {}", recipes.files);
    println!("loaded            : {}", recipes.total_loaded());
    println!("unmodelled        : {}", recipes.total_unmodelled());
    for kind in [
        RecipeKind::Shaped,
        RecipeKind::Shapeless,
        RecipeKind::Stonecutting,
        RecipeKind::Cooking,
    ] {
        println!(
            "  {:12} {}",
            kind.name(),
            recipes.loaded.get(&kind).copied().unwrap_or(0)
        );
    }
    for (kind, count) in &recipes.unmodelled {
        println!("  ignored {kind}: {count}");
    }

    assert!(
        recipes.skipped.is_empty(),
        "no vanilla recipe should fail to parse: {:?}",
        recipes.skipped
    );
    assert_eq!(
        recipes.total_loaded(),
        708 + 322 + 275 + 73 + 25 + 9 + 9,
        "the baseline's modelled-type counts"
    );
    // 1 515, not 1 516: the baseline's original figure counted the `recipe/`
    // **directory entry**, which `zipfile` reports as its own name. The loader was
    // right and the measurement was wrong (see the baseline's section 0a).
    assert_eq!(
        recipes.total_loaded() + recipes.total_unmodelled(),
        1515,
        "every recipe file must be either loaded or explicitly counted as unmodelled"
    );
    assert_eq!(recipes.loaded.get(&RecipeKind::Shaped).copied(), Some(708));
    assert_eq!(
        recipes.loaded.get(&RecipeKind::Shapeless).copied(),
        Some(322)
    );
    assert_eq!(
        recipes.loaded.get(&RecipeKind::Stonecutting).copied(),
        Some(275)
    );
    assert_eq!(
        recipes.loaded.get(&RecipeKind::Cooking).copied(),
        Some(73 + 25 + 9 + 9)
    );

    // The two values that replace Phase 06's guessed furnace table.
    let iron = id("minecraft:iron_ingot_from_smelting_raw_iron");
    match book.by_name(&iron).expect("the iron smelting recipe") {
        Recipe::Cooking(cooking) => {
            assert_eq!(cooking.cooking_time, 200, "measured from the jar");
            assert!(
                (cooking.experience - 0.7).abs() < 1e-9,
                "measured from the jar"
            );
            assert_eq!(cooking.result.value(), "iron_ingot");
            assert!(matches!(cooking.ingredient[0], Ingredient::Item(_)));
        }
        other => panic!("expected a cooking recipe, got {}", other.kind().name()),
    }

    // A shaped recipe whose key is a **tag** -- the common vanilla shape, and the one a
    // fixture written from a description is most likely to get wrong.
    match book
        .by_name(&id("minecraft:stick"))
        .expect("the stick recipe")
    {
        Recipe::Shaped(shaped) => {
            assert_eq!(shaped.pattern, vec!["#".to_owned(), "#".to_owned()]);
            assert_eq!(shaped.result_count, 4);
            let key = shaped.key.get(&'#').expect("the pattern symbol resolves");
            assert!(
                matches!(key[0], Ingredient::Tag(_)),
                "the stick recipe's key is a tag reference, got {key:?}"
            );
        }
        other => panic!("expected a shaped recipe, got {}", other.kind().name()),
    }

    // Every shapeless recipe parsed with at least one ingredient alternative.
    let mut shapeless_seen = 0;
    for recipe in book.of_kind(RecipeKind::Shapeless) {
        if let Recipe::Shapeless(shapeless) = recipe {
            assert!(!shapeless.ingredients.is_empty());
            shapeless_seen += 1;
        }
    }
    assert_eq!(shapeless_seen, 322);
}
