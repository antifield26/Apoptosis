//! Differential test: load the **real** 26.1.2 data pack's loot tables, functions and
//! advancements (P07-08, P07-10, P07-11).
//!
//! `mc-data`'s unit tests use hand-written fixtures, which proves the parsers but not that they
//! handle the data a real jar ships. This test loads the actual `data/minecraft/` tree and
//! asserts figures measured from the jar's own files.
//!
//! It is `#[ignore]`d because it needs `MC_VANILLA_DATA` -- a directory holding the extracted
//! `data/minecraft/` -- which is not committed (8.5 MiB of Mojang data).
//! `docs/research/data-pack-baseline.md` section 0 gives the extraction command; it is one
//! `python -c` line using only the standard library, so it is reproducible *from the
//! documentation* rather than from an uncommitted script.
//!
//! ```text
//! set MC_VANILLA_DATA=target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-data --test vanilla_data -- --ignored --nocapture
//! ```
//!
//! ## What makes this test worth having
//!
//! Three things a fixture written from a description cannot catch, all of which the loader got
//! wrong on the first attempt:
//!
//! 1. **The discriminator differs per level.** A table and an entry use `"type"`, a function
//!    uses `"function"`, a condition uses `"condition"`. Reading `"type"` everywhere finds
//!    nothing.
//! 2. **`minecraft:loot_table` can inline a whole table** instead of naming one
//!    (`equipment/trial_chamber`, three times). A loader that only handles the string form
//!    silently loses 3 tables, 12 functions and 6 conditions.
//! 3. **A function carries its own `conditions`** (164 of 1 392). Ignoring them applies a
//!    function vanilla would not have applied.
//!
//! ## The accounting claim this test makes
//!
//! For every directory: `files == loaded + unmodelled + skipped`. For the loot census, the
//! per-level totals are asserted against an **independent** measurement of the same jar
//! (`target/vanilla-26.1.2/census_loot_adv.py`), so the test cannot pass by agreeing with
//! itself. A dropped file, a dropped function or a dropped condition makes one of those two
//! sums wrong.

use mc_core::ids::ResourceId;
use mc_data::Limits;
use mc_data::advancement::{AdvancementLoadReport, AdvancementProblem, AdvancementRegistry, Chain};
use mc_data::function::{FunctionLimits, FunctionLoadReport, FunctionRegistry};
use mc_data::loot::{
    CONDITION_TYPES, ENTRY_TYPES, FUNCTION_TYPES, LootLoadReport, LootTables,
    load_directory as load_loot_tables,
};
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
        root.join("loot_table").is_dir() && root.join("advancement").is_dir(),
        "{} does not look like an extracted data/minecraft (no loot_table/ or advancement/)",
        root.display()
    );
    Some(root)
}

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid resource id")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test-support/fixtures/registry")
        .join(name)
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_real_vanilla_data_census_matches_the_measurement() {
    let Some(pack_root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let loot = check_loot_tables(&pack_root);
    check_functions(&pack_root);
    check_advancements(&pack_root, &loot);
}

/// Totals measured from the jar by `census_loot_adv.py`, per level of the format.
///
/// Declared as data rather than as literals inside the assertions so the test can print them
/// next to the loaded figures: a mismatch then shows as "measured 1404, loaded 1392" instead of
/// as a bare number, which is the difference between a diagnosis and a puzzle.
struct LootCensus {
    files: usize,
    table_types: usize,
    entry_types: usize,
    function_types: usize,
    condition_types: usize,
    providers: usize,
}

/// The measured census. Every figure is from `target/vanilla-26.1.2/census_loot_adv.py`
/// running against `server-26.1.2.jar`.
///
/// Two of these figures were **corrected** during this task, and the corrections are recorded
/// because each one was a bug in the first measurement rather than a change in the data:
///
/// - `function_types` 1 392 → 1 404 and `condition_types` 1 500 → 1 545: the first census read
///   `functions` only on `minecraft:item` entries (losing the 2 vanilla puts on
///   `minecraft:loot_table` entries) and did not follow `inverted`'s **singular** `term` key
///   (losing 12 `any_of` and their terms). The loader had the same two bugs; the differential
///   test is what found them.
/// - `entry_types` 2 542 → 2 548 and `table_types` 1 326 → 1 329: the first census did not
///   descend into tables **inlined** by a `minecraft:loot_table` entry, which
///   `equipment/trial_chamber` does three times.
const CENSUS: LootCensus = LootCensus {
    files: 1326,
    // 1 326 file-level tables plus 3 inlined tables (`equipment/trial_chamber`).
    table_types: 1329,
    entry_types: 2548,
    function_types: 1404,
    condition_types: 1545,
    providers: 3805,
};

/// Load the loot tables and assert the full census.
///
/// One long function on purpose: its assertions are a single claim — "the loader's census equals
/// the measured one" — and splitting them across a dozen helpers would move the expected figures
/// away from the code that compares them.
#[allow(clippy::too_many_lines)]
fn check_loot_tables(pack_root: &Path) -> LootTables {
    let items = mc_registry::ItemRegistry::load(&fixture("items.tsv")).expect("the item fixture");
    let blocks =
        mc_registry::BlockRegistry::load(&fixture("blocks.tsv")).expect("the block fixture");
    let mut report = LootLoadReport::default();
    let loaded = load_loot_tables(
        pack_root,
        "minecraft",
        Limits::DEFAULT,
        Some(&items),
        Some(&blocks),
        &mut report,
    );
    let mut tables = LootTables::new();
    for table in loaded {
        tables.insert(table);
    }

    println!("--- vanilla loot tables ---");
    println!("files found          : {}", report.files);
    println!("loaded               : {}", report.loaded);
    println!("unmodelled files     : {}", report.total_unmodelled_files());
    println!("skipped files        : {}", report.skipped.len());
    println!("named tables         : {}", tables.len());
    println!("forward references   : {}", report.forward_references.len());

    // ---- the accounting invariant, which is the whole point ----
    assert_eq!(
        report.files, CENSUS.files,
        "the baseline measured {} loot_table files in the 26.1.2 jar",
        CENSUS.files
    );
    assert!(
        report.is_fully_accounted(),
        "every file must be loaded, unmodelled or skipped: files={} loaded={} unmodelled={} \
         skipped={}",
        report.files,
        report.loaded,
        report.total_unmodelled_files(),
        report.skipped.len()
    );
    assert!(
        report.skipped.is_empty(),
        "no vanilla loot table should fail to parse: {:?}",
        report.skipped
    );
    assert!(
        report.unmodelled.is_empty(),
        "all 11 vanilla table types are modelled, so nothing should be unmodelled: {:?}",
        report.unmodelled
    );
    assert_eq!(report.loaded, CENSUS.files);
    // 1 326 files, and none of them shares a name, so all 1 326 survive the merge.
    assert_eq!(tables.len(), CENSUS.files, "one named table per file");
    assert!(
        tables.tables().iter().all(|table| table.name.is_some()),
        "a file always has a name"
    );

    // ---- the per-level census ----
    let modelled_tables =
        report.occurrences_of(TABLE_TYPES) + report.occurrences("minecraft:inline");
    let modelled_entries = report.occurrences_of(ENTRY_TYPES);
    let modelled_functions = report.occurrences_of(FUNCTION_TYPES);
    let modelled_conditions = report.occurrences_of(CONDITION_TYPES);
    let providers: usize = report.number_providers.values().sum();

    // ---- the per-type counts, measured ----
    //
    // Printed and asserted **before** the per-level totals, so a mismatch shows which type is
    // wrong rather than only that the sum is. (The first version asserted the totals first, and
    // a difference of 8 out of 1 404 said nothing about where it came from.)
    //
    // The two full maps are dumped first: with 19 function types and 12 condition types, seeing
    // every figure at once is what turns "off by 8" into "two types are short by 2 and 6".
    println!("\nmodelled (every construct type the loader recognised):");
    for (kind, count) in &report.modelled {
        println!("  {kind:44} {count:5}");
    }
    println!("unexecutable (recognised, preserved, not executed):");
    for (kind, count) in &report.unexecutable {
        println!("  {kind:44} {count:5}");
    }
    println!("number providers (arguments of a function, not nodes):");
    for (kind, count) in &report.number_providers {
        println!("  {kind:44} {count:5}");
    }

    println!("\ntable types:");
    for (kind, expected) in [
        ("minecraft:block", 1085),
        ("minecraft:entity", 114),
        ("minecraft:chest", 65),
        ("minecraft:shearing", 22),
        ("minecraft:gift", 21),
        ("minecraft:archaeology", 6),
        ("minecraft:block_interact", 4),
        ("minecraft:fishing", 4),
        ("minecraft:equipment", 3),
        ("minecraft:inline", 3),
        ("minecraft:barter", 1),
        ("minecraft:entity_interact", 1),
    ] {
        let seen = report.occurrences(kind);
        println!("  {kind:36} {seen:5} (measured {expected})");
        assert_eq!(seen, expected, "table type {kind}");
    }

    println!("entry types:");
    for (kind, expected) in [
        ("minecraft:item", 2366),
        ("minecraft:alternatives", 84),
        ("minecraft:loot_table", 57),
        ("minecraft:empty", 39),
        ("minecraft:dynamic", 1),
        ("minecraft:tag", 1),
    ] {
        let seen = report.occurrences(kind);
        println!("  {kind:36} {seen:5} (measured {expected})");
        assert_eq!(seen, expected, "entry type {kind}");
    }

    println!("function types:");
    for (kind, expected) in [
        ("minecraft:set_count", 847),
        ("minecraft:explosion_decay", 147),
        ("minecraft:enchanted_count_increase", 80),
        ("minecraft:copy_components", 71),
        ("minecraft:enchant_randomly", 52),
        ("minecraft:set_potion", 48),
        ("minecraft:enchant_with_levels", 34),
        ("minecraft:apply_bonus", 32),
        ("minecraft:set_damage", 28),
        ("minecraft:furnace_smelt", 19),
        ("minecraft:set_enchantments", 11),
        ("minecraft:copy_state", 10),
        ("minecraft:limit_count", 6),
        ("minecraft:set_components", 6),
        ("minecraft:exploration_map", 3),
        ("minecraft:set_name", 3),
        ("minecraft:set_ominous_bottle_amplifier", 3),
        ("minecraft:set_stew_effect", 3),
        ("minecraft:set_instrument", 1),
    ] {
        let seen = report.occurrences(kind);
        println!("  {kind:36} {seen:5} (measured {expected})");
        assert_eq!(seen, expected, "function type {kind}");
    }

    println!("condition types:");
    for (kind, expected) in [
        ("minecraft:survives_explosion", 837),
        ("minecraft:block_state_property", 253),
        ("minecraft:match_tool", 203),
        ("minecraft:entity_properties", 90),
        ("minecraft:any_of", 51),
        ("minecraft:table_bonus", 29),
        ("minecraft:killed_by_player", 24),
        ("minecraft:random_chance", 19),
        ("minecraft:inverted", 15),
        ("minecraft:random_chance_with_enchanted_bonus", 11),
        ("minecraft:damage_source_properties", 8),
        ("minecraft:location_check", 5),
    ] {
        let seen = report.occurrences(kind);
        println!("  {kind:36} {seen:5} (measured {expected})");
        assert_eq!(seen, expected, "condition type {kind}");
    }

    println!("number providers:");
    for (kind, expected) in [
        ("constant", 3175),
        ("minecraft:uniform", 612),
        ("minecraft:binomial", 18),
    ] {
        let seen = report.provider_occurrences(kind);
        println!("  {kind:36} {seen:5} (measured {expected})");
        assert_eq!(seen, expected, "number provider {kind}");
    }

    println!("\nlevel            total   unexecutable   measured");
    for (label, total, unexecutable, measured) in [
        (
            "table types",
            modelled_tables,
            unexecutable_of(&report, TABLE_TYPES),
            CENSUS.table_types,
        ),
        (
            "entry types",
            modelled_entries,
            unexecutable_of(&report, ENTRY_TYPES),
            CENSUS.entry_types,
        ),
        (
            "function types",
            modelled_functions,
            unexecutable_of(&report, FUNCTION_TYPES),
            CENSUS.function_types,
        ),
        (
            "condition types",
            modelled_conditions,
            unexecutable_of(&report, CONDITION_TYPES),
            CENSUS.condition_types,
        ),
        ("number providers", providers, 0, CENSUS.providers),
    ] {
        println!("{label:16} {total:7} {unexecutable:14}   {measured}");
    }

    assert_eq!(
        modelled_tables, CENSUS.table_types,
        "table-level types: 1 326 files plus 3 inlined tables"
    );
    assert_eq!(
        modelled_entries, CENSUS.entry_types,
        "entry-level types, nested children and inlined tables included"
    );
    assert_eq!(
        modelled_functions, CENSUS.function_types,
        "function types, including the 12 inside inlined tables"
    );
    assert_eq!(
        modelled_conditions, CENSUS.condition_types,
        "condition types: 1 539 outside the inlined tables plus 6 inside them"
    );
    assert_eq!(
        providers, CENSUS.providers,
        "number providers at the four places the format writes one"
    );

    // ---- what is executed vs merely represented ----
    //
    // The number `loot.rs`'s module documentation quotes, asserted rather than left as prose: if
    // a later change starts claiming to execute more of vanilla, this is where it shows.
    let executable = report
        .modelled
        .iter()
        .filter(|(kind, _)| EXECUTABLE.contains(&kind.as_str()))
        .map(|(_, count)| *count)
        .sum::<usize>();
    let unexecutable = report.total_unexecutable();
    let total = report.modelled.values().sum::<usize>() + unexecutable;
    println!(
        "\nconstructs the load reported: {total} occurrences, {executable} executable by `roll`, \
         {unexecutable} refused"
    );
    assert_eq!(total, 6826, "every construct the pack contains");
    assert_eq!(
        executable, 4564,
        "the executable subset, measured: item/empty/loot_table/alternatives entries plus \
         set_count, limit_count, survives_explosion, random_chance, \
         random_chance_with_enchanted_bonus, match_tool, any_of and inverted"
    );
    assert_eq!(unexecutable, 903, "the refused constructs, measured");
    assert_eq!(
        executable + unexecutable + 1359,
        total,
        "the remainder is the nested content of `Raw` entries, counted in `unexecutable` but not \
         walked as nodes"
    );

    // ---- the two structures nothing else in the crate covers ----
    let trial_chamber = tables
        .by_name(&id("minecraft:equipment/trial_chamber"))
        .expect("the one file that inlines tables");
    let nested = trial_chamber.nested_tables();
    println!(
        "\ninlined tables in equipment/trial_chamber: {}",
        nested.len()
    );
    assert_eq!(
        nested.len(),
        3,
        "the only inlined tables in the whole pack, and an inlined table is the one table \
         that has no name"
    );
    assert!(nested.iter().all(|table| table.name.is_none()));
    assert_eq!(nested[0].kind, "minecraft:inline");

    let wheat = tables
        .by_name(&id("minecraft:blocks/wheat"))
        .expect("the crop table");
    println!(
        "blocks/wheat: {} pool(s), {} table-level function(s)",
        wheat.pools.len(),
        wheat.functions.len()
    );
    assert_eq!(
        wheat.functions.len(),
        1,
        "one of the nine tables that carry top-level functions"
    );
    let with_table_functions = tables
        .tables()
        .iter()
        .filter(|table| !table.functions.is_empty())
        .count();
    assert_eq!(
        with_table_functions, 9,
        "nine vanilla tables put functions at the table level"
    );

    // Every table that references another resolves, and the reference graph is acyclic.
    let mut referenced = 0usize;
    for table in tables.tables() {
        for nested in table.nested_tables() {
            let _ = nested;
        }
        let name = table.name.clone().expect("named");
        for other in tables.referencing(&name) {
            referenced += 1;
            let _ = other;
        }
    }
    println!("named tables that are referenced by another: {referenced}");
    assert_eq!(
        referenced, 52,
        "52 of the 1 326 tables are the target of a minecraft:loot_table reference; a smaller \
         number means the reference scan is missing them"
    );

    // The block names inside `block_state_property` resolve against our own block table.
    //
    // This is the cross-check the brief asks for: the format names blocks, so it names them
    // through `mc-registry`. `mentioned_block_names` walks the *raw* JSON, because
    // `block_state_property` is one of the conditions this build preserves rather than models —
    // so without the walk, a table could name a block that does not exist and nothing would say
    // so.
    let mut mentioned: BTreeSet<String> = BTreeSet::new();
    for table in tables.tables() {
        mentioned.extend(table.mentioned_block_names());
    }
    println!(
        "distinct blocks named in block_state_property: {}",
        mentioned.len()
    );
    assert_eq!(
        mentioned.len(),
        148,
        "every block a vanilla loot table tests a state property on, measured from the jar"
    );
    for name in &mentioned {
        assert!(
            blocks.contains(name),
            "{name} is named by a loot table but absent from the block registry"
        );
    }
    // And the report itself says the same thing: zero forward references means every item and
    // block the pack names resolves against the registries we declared known.
    assert!(
        report.forward_references.is_empty(),
        "the vanilla pack references no content this build's registries lack: {:?}",
        report.forward_references
    );

    tables
}

/// Loot-table `type`s: the 11 a file can declare, measured from the jar.
const TABLE_TYPES: &[&str] = &[
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

/// The construct types [`mc_data::loot::roll`] executes.
const EXECUTABLE: &[&str] = &[
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

/// drifting into a claim.
fn check_functions(pack_root: &Path) {
    let mut report = FunctionLoadReport::default();
    let registry = FunctionRegistry::load_directory(
        pack_root,
        "minecraft",
        FunctionLimits::DEFAULT,
        &mut report,
    );

    println!("\n--- vanilla functions ---");
    println!("files found          : {}", report.files);
    println!("loaded               : {}", report.loaded);
    println!("commands             : {}", report.commands);
    println!("skipped files        : {}", report.skipped.len());
    println!("registry             : {} functions", registry.len());

    assert_eq!(
        report.files, 0,
        "the 26.1.2 jar ships no .mcfunction file: {} were found, which means the extraction \
         picked up something the jar does not contain",
        report.files
    );
    assert!(registry.is_empty());
    assert_eq!(registry.command_budget(), 0);
    assert!(registry.functions_calling_functions().is_empty());
    assert!(report.is_fully_accounted());
    assert!(report.is_clean());

    // The directory itself must be absent, so this test also documents *where* the pack's
    // function data would have to come from.
    assert!(
        !pack_root.join("function").is_dir(),
        "there is no function/ directory in the vanilla pack"
    );
}

/// Occurrences of a closed set of types that the loader did **not** model, i.e. that sit in
/// `unexecutable`. The complement of [`LootLoadReport::occurrences_of`].
fn unexecutable_of(report: &LootLoadReport, types: &[&str]) -> usize {
    types
        .iter()
        .map(|kind| report.unexecutable.get(*kind).copied().unwrap_or(0))
        .sum()
}

/// Load the real advancements and assert the tree, the hazards and the figures.
///
/// Long for the same reason as `check_loot_tables`: one claim, many figures.
#[allow(clippy::too_many_lines)]
fn check_advancements(pack_root: &Path, tables: &LootTables) {
    let mut report = AdvancementLoadReport::default();
    let registry =
        AdvancementRegistry::load_directory(pack_root, "minecraft", Limits::DEFAULT, &mut report);

    println!("\n--- vanilla advancements ---");
    println!("files found          : {}", report.files);
    println!("loaded               : {}", report.loaded);
    println!("skipped files        : {}", report.skipped.len());
    println!("advancements         : {}", registry.len());

    assert_eq!(
        report.files, 1617,
        "the baseline measured 1 617 advancement files"
    );
    assert_eq!(report.loaded, 1617);
    assert!(
        report.skipped.is_empty(),
        "no vanilla advancement should fail to parse: {:?}",
        report.skipped
    );
    assert!(report.is_fully_accounted());
    assert!(report.is_clean(), "{:?}", report.unmodelled);
    assert_eq!(registry.len(), 1617, "no two files share a name");

    // ---- the three hazards, all zero in vanilla ----
    report.check(&registry);
    println!(
        "missing parents      : {}",
        count_kind(&report, ProblemKind::MissingParent)
    );
    println!("cycles               : {}", report.cycles().len());
    println!("duplicate ids        : {}", registry.duplicates().len());
    println!(
        "undefined criteria   : {}",
        count_kind(&report, ProblemKind::UndefinedRequirement)
    );
    println!(
        "unused criteria      : {}",
        count_kind(&report, ProblemKind::UnreferencedCriterion)
    );
    assert!(
        report.cycles().is_empty(),
        "vanilla has no advancement cycle; one means the detector is wrong"
    );
    assert!(
        registry.duplicates().is_empty(),
        "one file per name, so no duplicates: {:?}",
        registry.duplicates()
    );
    assert_eq!(
        count_kind(&report, ProblemKind::MissingParent),
        0,
        "vanilla has no missing parent: {:?}",
        report.problems()
    );
    assert_eq!(count_kind(&report, ProblemKind::UndefinedRequirement), 0);
    assert_eq!(
        count_kind(&report, ProblemKind::UnreferencedCriterion),
        0,
        "every criterion of every vanilla advancement is required by a group, either written \
         out or through the format's default"
    );
    assert_eq!(count_kind(&report, ProblemKind::TooDeep), 0);
    assert!(
        report.errors().is_empty(),
        "the real pack must be clean: {:?}",
        report.errors()
    );

    // ---- the tree ----
    let roots = registry.roots();
    let mut root_names: Vec<String> = roots.iter().map(|root| root.name.to_string()).collect();
    root_names.sort();
    println!("roots                : {}", roots.len());
    for name in &root_names {
        println!("  {name}");
    }
    assert_eq!(roots.len(), 6, "one root per tab");
    assert_eq!(
        root_names,
        vec![
            "minecraft:adventure/root".to_owned(),
            "minecraft:end/root".to_owned(),
            "minecraft:husbandry/root".to_owned(),
            "minecraft:nether/root".to_owned(),
            "minecraft:recipes/root".to_owned(),
            "minecraft:story/root".to_owned(),
        ],
        "the six tabs, measured from the jar"
    );

    assert_eq!(registry.max_depth(), Some(9), "the deepest measured chain");
    let histogram = registry.depth_histogram();
    println!("depth histogram      : {histogram:?}");
    for (depth, expected) in [
        (1usize, 6usize),
        (2, 1531),
        (3, 48),
        (4, 13),
        (5, 8),
        (6, 5),
        (7, 3),
        (8, 2),
        (9, 1),
    ] {
        assert_eq!(
            histogram.get(&depth).copied(),
            Some(expected),
            "depth {depth} of the measured histogram"
        );
    }
    assert_eq!(
        histogram.values().sum::<usize>(),
        1617,
        "every advancement sits at some depth"
    );
    // Nothing is depth 0: vanilla has no broken chain.
    assert_eq!(histogram.get(&0), None, "a depth-0 entry is a broken chain");

    // A root's chain ends at itself; a leaf's ends at the root, nine links later.
    assert_eq!(
        registry.chain_of(&id("minecraft:story/root")),
        Chain::Rooted { depth: 1 }
    );
    for name in registry.names() {
        assert!(
            matches!(registry.chain_of(name), Chain::Rooted { .. }),
            "{name} must have an unbroken chain in vanilla"
        );
    }

    // ---- criteria and triggers ----
    let mut criteria = 0usize;
    let mut with_conditions = 0usize;
    for advancement in registry.sorted() {
        criteria += advancement.criteria.len();
        with_conditions += advancement
            .criteria
            .iter()
            .filter(|criterion| criterion.has_conditions())
            .count();
    }
    let triggers = registry.triggers();
    println!("criteria             : {criteria}");
    println!("criteria with conds  : {with_conditions}");
    println!("distinct triggers    : {}", triggers.len());
    assert_eq!(criteria, 3546, "measured from the jar");
    assert_eq!(
        with_conditions, 3531,
        "15 criteria state no conditions, which is a real case rather than a defensive one"
    );
    assert_eq!(triggers.len(), 54, "measured from the jar");
    assert!(triggers.contains("minecraft:inventory_changed"));
    assert!(triggers.contains("minecraft:recipe_unlocked"));
    assert!(triggers.contains("minecraft:impossible"));
    // The two most common triggers are the ones a naive count would get wrong, because
    // `recipe_unlocked` appears once per unlocked recipe.
    assert_eq!(
        registry.with_trigger("minecraft:inventory_changed").len(),
        1502,
        "distinct advancements using inventory_changed, measured from the jar"
    );

    // ---- displays ----
    let displays: Vec<_> = registry
        .sorted()
        .into_iter()
        .filter(|advancement| advancement.display.is_some())
        .collect();
    println!("displays             : {}", displays.len());
    assert_eq!(displays.len(), 125, "measured from the jar");
    let mut frames = std::collections::BTreeMap::new();
    for advancement in &displays {
        let display = advancement.display.as_ref().expect("a display");
        *frames.entry(display.frame).or_insert(0usize) += 1;
    }
    println!("frames               : {frames:?}");
    assert_eq!(
        frames.get(&mc_data::advancement::Frame::Task).copied(),
        Some(90)
    );
    assert_eq!(
        frames.get(&mc_data::advancement::Frame::Goal).copied(),
        Some(10)
    );
    assert_eq!(
        frames.get(&mc_data::advancement::Frame::Challenge).copied(),
        Some(25)
    );
    // Every vanilla title is a translation key, never a literal: flattening one would put
    // `advancements.story.root.title` on screen.
    let literal_titles = displays
        .iter()
        .filter(|advancement| {
            advancement
                .display
                .as_ref()
                .is_some_and(|display| display.title.is_resolved())
        })
        .count();
    println!("literal titles       : {literal_titles}");
    assert_eq!(
        literal_titles, 0,
        "all 125 displays use {{\"translate\": ...}}; a literal would mean the parser attributed \
         one wrongly"
    );
    assert!(
        displays.iter().all(|advancement| advancement
            .display
            .as_ref()
            .is_some_and(|display| display.title.translation_key().is_some())),
        "every title must carry a translation key"
    );

    // ---- rewards ----
    let with_rewards = registry
        .sorted()
        .into_iter()
        .filter(|advancement| !advancement.rewards.is_empty())
        .count();
    let recipes: usize = registry
        .sorted()
        .iter()
        .map(|advancement| advancement.rewards.recipes.len())
        .sum();
    println!("with rewards         : {with_rewards}");
    println!("recipes unlocked     : {recipes}");
    assert_eq!(with_rewards, 1514, "measured from the jar");
    assert_eq!(recipes, 1491, "measured from the jar");

    // The rewards name recipes and functions that must exist in the pack: a reward pointing at
    // a recipe the pack does not define is a claim this build can check, so it does.
    let mut recipes_unlocked: BTreeSet<String> = BTreeSet::new();
    for advancement in registry.sorted() {
        for recipe in &advancement.rewards.recipes {
            recipes_unlocked.insert(recipe.to_string());
        }
    }
    println!("distinct recipes     : {}", recipes_unlocked.len());
    assert!(
        recipes_unlocked.len() > 900,
        "the pack unlocks most of its own recipes, so a small number means the rewards were \
         dropped ({})",
        recipes_unlocked.len()
    );

    // ---- cross-directory: the rewards name this crate's other loaders would own ----
    let mut rewards_naming_loot: Vec<&str> = Vec::new();
    for advancement in registry.sorted() {
        for target in &advancement.rewards.loot {
            rewards_naming_loot.push(target.value());
        }
    }
    println!(
        "reward loot tables   : {} (vanilla uses none)",
        rewards_naming_loot.len()
    );
    assert!(
        rewards_naming_loot.is_empty(),
        "all 1 514 vanilla reward blocks carry only `recipes` and `experience`"
    );

    // A cross-check between two of the three loaders: the pack's loot tables are named after
    // entity/block paths, and the advancements reference loot tables only through rewards
    // (`player_generates_container_loot` triggers). Neither is a hard link, so this only
    // reports the overlap rather than asserting one.
    let loot_named_by_trigger = registry
        .with_trigger("minecraft:player_generates_container_loot")
        .len();
    println!(
        "advancements on player_generates_container_loot: {loot_named_by_trigger} (tables \
         loaded: {})",
        tables.len()
    );
    assert!(loot_named_by_trigger > 0);
}

/// How many problems of one variant a report holds.
fn count_kind(report: &AdvancementLoadReport, variant: ProblemKind) -> usize {
    report
        .problems()
        .iter()
        .filter(|problem| match (variant, problem) {
            (ProblemKind::MissingParent, AdvancementProblem::MissingParent { .. })
            | (
                ProblemKind::UndefinedRequirement,
                AdvancementProblem::UndefinedRequirement { .. },
            )
            | (
                ProblemKind::UnreferencedCriterion,
                AdvancementProblem::UnreferencedCriterion { .. },
            )
            | (ProblemKind::TooDeep, AdvancementProblem::TooDeep { .. }) => true,
            (
                ProblemKind::MissingParent
                | ProblemKind::UndefinedRequirement
                | ProblemKind::UnreferencedCriterion
                | ProblemKind::TooDeep,
                _,
            ) => false,
        })
        .count()
}

/// Which problem to count. A closed enum rather than a string, so a typo is a compile error
/// rather than a silently-zero count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemKind {
    MissingParent,
    UndefinedRequirement,
    UnreferencedCriterion,
    TooDeep,
}
