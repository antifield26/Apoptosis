//! Differential: build the furnace table from the **real** 26.1.2 smelting recipes
//! (P07-09, P07-18).
//!
//! The unit tests use hand-written recipes, which proves the conversion but not that it
//! handles what the jar ships. This one loads the actual `recipe/` directory, converts the
//! `smelting` type into a [`SmeltingRegistry`], and asserts the figures
//! `docs/research/data-pack-baseline.md` measured.
//!
//! It is the test that retires the Phase 06 §5.9 caveat (see CHANGELOG.md): the six hand-written
//! baseline recipes were community knowledge, and the jar states their real values. If the
//! conversion disagrees with the jar on iron, either the loader or the convertor is wrong.
//!
//! (Absolute: the test's working directory is the crate's, not the repository
//! root -- AUDIT-09 D-06. Above the fence, because it is prose, not part of the command.)
//!
//! ```text
//! set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-container --test vanilla_smelting -- --ignored --nocapture
//! ```

use mc_container::furnace::SmeltingRegistry;
use mc_container::smelting_data::SmeltingConversion;
use mc_data::recipe::{RecipeLoadReport, load_directory};
use mc_data::{Limits, RecipeBook, SmeltingKind, TagKey, TagSet};
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
fn the_furnace_table_is_built_from_the_real_smelting_recipes() {
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

    // The tag resolver the conversion needs: vanilla smelting recipes use tags such as
    // `#minecraft:logs_that_burn`, and a tag is a set of items. Resolving them is what
    // makes the real recipes convertible at all, and it is why the caller supplies this
    // rather than `mc-container` reaching into `mc-data`'s tag machinery.
    let mut tag_report = mc_data::TagLoadReport::default();
    let files = mc_data::tag::load_directory(&root, "minecraft", Limits::DEFAULT, &mut tag_report);
    // Deliberately *empty* registry contents: the resolver is asked for tag **members**,
    // and passing no known ids means an unresolvable reference is reported rather than
    // silently accepted. The conversion only needs the members that do resolve.
    let contents = mc_data::tag::RegistryContents::new();
    let tags = TagSet::from_map(mc_data::resolve_all(&files, &contents, &mut tag_report));

    let resolve = |tag: &mc_core::ids::ResourceId| -> Vec<mc_core::ids::ResourceId> {
        let key = TagKey {
            registry: "item".to_owned(),
            name: tag.clone(),
        };
        tags.get(&key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    };

    let (table, conversion) =
        SmeltingRegistry::from_recipes(&book, SmeltingKind::Smelting, &registry, Some(&resolve))
            .expect("the real recipes convert");

    println!("--- real smelting recipes ---");
    println!("recipe files        : {}", report.files);
    println!(
        "cooking loaded      : {:?}",
        report.loaded.get(&mc_data::RecipeKind::Cooking)
    );
    println!("recipes converted   : {}", conversion.recipes_converted);
    println!(
        "unresolved tags     : {}",
        conversion.unresolved_tag_recipes
    );
    println!("duplicate inputs    : {}", conversion.duplicate_inputs);
    println!("unknown results     : {}", conversion.unknown_results.len());
    println!("empty tags          : {}", conversion.empty_tags.len());
    println!("table size          : {}", table.len());
    for name in conversion.unknown_results.iter().take(10) {
        println!("  unknown result: {name}");
    }
    for name in conversion.empty_tags.iter().take(10) {
        println!("  empty tag: {name}");
    }

    // 73 smelting recipes exist; every one must be accounted for in exactly one bucket.
    // Note the distinction the first version of this test missed: `seen()` counts
    // *recipes*, `rows` counts table rows, and one recipe with alternative ingredients
    // produces several rows. Conflating them let a dropped recipe look like success.
    assert_eq!(
        conversion.recipes_seen, 73,
        "the baseline measured 73 smelting recipes"
    );
    assert_eq!(
        conversion.seen() + conversion.unknown_results.len(),
        conversion.recipes_seen,
        "every recipe must land in exactly one bucket: {conversion:?}"
    );
    assert!(
        conversion.recipes_converted > 0,
        "the real pack must yield a usable furnace table"
    );
    assert_eq!(table.len(), conversion.rows);
    assert!(
        conversion.rows >= conversion.recipes_converted,
        "a recipe produces at least one row, never fewer"
    );

    check_known_values(&registry, &table);
    check_table_invariants(&table, &conversion);
}

/// The values Phase 06 hand-wrote from community knowledge, which the jar states.
///
/// This is the assertion that retires the Phase 06 §5.9 caveat, so it is worth
/// keeping separate: if it ever fails, the caveat is back.
fn check_known_values(registry: &ItemRegistry, table: &SmeltingRegistry) {
    // The two values Phase 06 guessed and the jar confirms.
    let raw_iron = registry.id("minecraft:raw_iron").expect("raw iron");
    let iron = registry.id("minecraft:iron_ingot").expect("iron");
    let recipe = table
        .recipe_for(raw_iron)
        .expect("raw iron smelts to iron in the real pack");
    assert_eq!(recipe.output, iron);
    assert_eq!(recipe.cook_ticks, 200, "the jar's cookingtime");
    assert!(
        (recipe.experience - 0.7).abs() < 1e-4,
        "the jar's experience, got {}",
        recipe.experience
    );

    // A second real row, from a different part of the table: sand to glass, which is in
    // the pack and is not one of the hand-written baseline's six.
    let sand = registry.id("minecraft:sand").expect("sand");
    if let Some(glass) = table.recipe_for(sand) {
        assert_eq!(
            glass.output,
            registry.id("minecraft:glass").expect("glass"),
            "sand smelts to glass"
        );
        println!("sand -> glass      : {} ticks", glass.cook_ticks);
    }

    // A tag-driven row, which only exists because the resolver ran: an oak log smelts to
    // charcoal through `#minecraft:logs_that_burn`. Asserted here rather than merely
    // printed, because a resolver that silently returns nothing would still pass every
    // other check in this file.
    let oak_log = registry.id("minecraft:oak_log").expect("oak log");
    let charcoal = table
        .recipe_for(oak_log)
        .expect("an oak log must smelt to charcoal through the logs_that_burn tag");
    assert_eq!(
        charcoal.output,
        registry.id("minecraft:charcoal").expect("charcoal")
    );
    println!("oak_log -> charcoal: {} ticks", charcoal.cook_ticks);
}

/// Every row of the real table must be internally sane.
///
/// On the real data rather than on fixtures, which is the point: a hand-written recipe
/// cannot produce a `cook_ticks` of zero, but a misread JSON field can.
fn check_table_invariants(table: &SmeltingRegistry, conversion: &SmeltingConversion) {
    for row in table.recipes() {
        assert!(row.cook_ticks > 0 && row.cook_ticks <= 20_000, "{row:?}");
        assert!(
            row.experience >= 0.0 && row.experience.is_finite(),
            "{row:?}"
        );
        assert!(row.output_count > 0, "{row:?}");
        assert_ne!(row.input, 0, "air must not smelt");
        // A row must be findable by its own input, or the lookup is inconsistent with
        // the table it built.
        assert!(table.recipe_for(row.input).is_some(), "{row:?}");
    }

    // The conversion is **not** total on the real pack, and that is expected rather than
    // tolerated: vanilla has several smelting recipes sharing an input once a tag expands
    // (12 wood types all reach charcoal through `#minecraft:logs_that_burn`), and a table
    // keyed by item id cannot hold two rows for one input. What must hold is that every
    // recipe is *accounted for*.
    // Whether the real pack has duplicate inputs is a **measurement**, not an
    // assumption. The first version of this test asserted `duplicate_inputs > 0` from an
    // expectation about tag overlap, which is not evidence; the arithmetic above is what
    // actually needs to hold.
    println!("duplicate inputs    : {}", conversion.duplicate_inputs);
    println!(
        "--- {} rows; {} recipes accounted for ---",
        table.len(),
        conversion.seen()
    );
}
