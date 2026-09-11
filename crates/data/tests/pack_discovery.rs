//! World data-pack discovery end to end (P07-12).
//!
//! `enabled.rs`'s unit tests prove the naming and matching rules. This proves the **effect** on a
//! real filesystem: a pack the world enables loads, a pack it disables does **not**, and — the
//! case that matters most — a world whose `level.dat` has no `DataPacks` section at all keeps
//! loading everything.
//!
//! That last one is why this file exists. Treating an absent list as "nothing is enabled" would
//! silently disable every pack in a world that has been running for months, and the symptom would
//! be "my datapack stopped working" with no cause anywhere.

use mc_data::enabled::EnabledPacks;
use mc_data::pack::{DataPackSet, PackSource};
use mc_data::{Limits, VANILLA_NAMESPACE};
use mc_test_support::fixtures::TempDir;

/// Build a world directory with packs, each holding one tag file so it has real content.
fn world_with_packs(dir: &TempDir, packs: &[&str]) -> std::path::PathBuf {
    let world = dir.path().join("world");
    let datapacks = world.join("datapacks");
    std::fs::create_dir_all(&datapacks).expect("datapacks dir");
    for name in packs {
        let data = datapacks
            .join(name)
            .join("data")
            .join("testns")
            .join("tags")
            .join("item");
        std::fs::create_dir_all(&data).expect("pack data dir");
        std::fs::write(
            data.join("marker.json"),
            r#"{"values": ["minecraft:stone"]}"#,
        )
        .expect("write a tag");
    }
    world
}

/// The names of the packs that loaded.
fn loaded_names(set: &DataPackSet) -> Vec<String> {
    let mut names: Vec<String> = set
        .load_order()
        .iter()
        .filter_map(|pack| {
            pack.root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_world_with_no_pack_list_loads_every_pack() {
    // **The regression this module exists for.** A world whose `level.dat` has no `DataPacks`
    // section — because it predates the field, or another tool wrote it — must keep loading all of
    // its packs. Reading an absent list as "nothing enabled" would silently disable every pack.
    let dir = TempDir::new("pack-nolist");
    let world = world_with_packs(&dir, &["alpha", "beta"]);

    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);

    assert_eq!(
        loaded_names(&set),
        vec!["alpha", "beta"],
        "an unspecified list enables everything"
    );
    assert!(
        set.skipped().is_empty(),
        "nothing was skipped: {:?}",
        set.skipped()
    );
    assert!(
        set.rejected().is_empty(),
        "nothing was rejected: {:?}",
        set.rejected()
    );
}

#[test]
fn a_disabled_pack_does_not_load_and_the_reason_is_recorded() {
    let dir = TempDir::new("pack-disabled");
    let world = world_with_packs(&dir, &["alpha", "beta"]);

    let enabled = EnabledPacks::from_level_dat(&["vanilla".to_owned(), "alpha".to_owned()], &[]);
    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &enabled, Limits::DEFAULT);

    assert_eq!(
        loaded_names(&set),
        vec!["alpha"],
        "only the named pack loads; beta is not in the list"
    );
    assert_eq!(
        set.skipped().len(),
        1,
        "and the skip is recorded: {:?}",
        set.skipped()
    );
    assert!(
        set.skipped()[0].contains("beta"),
        "naming the pack: {:?}",
        set.skipped()
    );
    assert!(
        set.skipped()[0].contains("DataPacks"),
        "and the reason: {:?}",
        set.skipped()
    );
    assert!(
        set.rejected().is_empty(),
        "a disabled pack is not a broken one: {:?}",
        set.rejected()
    );
}

#[test]
fn an_explicitly_disabled_pack_does_not_load_even_when_also_enabled() {
    let dir = TempDir::new("pack-both");
    let world = world_with_packs(&dir, &["alpha"]);

    let enabled = EnabledPacks::from_level_dat(&["alpha".to_owned()], &["alpha".to_owned()]);
    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &enabled, Limits::DEFAULT);

    assert!(
        loaded_names(&set).is_empty(),
        "disabling wins over enabling: {:?}",
        loaded_names(&set)
    );
    assert_eq!(set.skipped().len(), 1);
}

#[test]
fn the_three_spellings_of_a_pack_name_all_enable_it() {
    // Vanilla writes a world-folder pack as `file/<name>` and an archive as `<name>.zip`; a naive
    // comparison would make a legitimately enabled pack look absent.
    for spelling in ["my_pack", "file/my_pack", "my_pack.zip", "file/My_Pack.zip"] {
        let dir = TempDir::new(&format!("pack-name-{}", spelling.replace(['/', '.'], "-")));
        let world = world_with_packs(&dir, &["my_pack"]);
        let enabled = EnabledPacks::from_level_dat(&[spelling.to_owned()], &[]);
        let mut set = DataPackSet::new();
        set.discover_world_packs_enabled(&world, &enabled, Limits::DEFAULT);
        assert_eq!(
            loaded_names(&set),
            vec!["my_pack"],
            "{spelling:?} must enable the pack"
        );
    }
}

#[test]
fn a_directory_that_is_not_a_pack_is_rejected_not_skipped() {
    // The distinction matters: a broken pack needs looking at, a disabled one does not. Conflating
    // them sends an operator hunting for a corruption that is not there.
    //
    // **Two cases, and only the second tests the check ordering.** With `EnabledPacks::default()`
    // — everything enabled — `allows("not_a_pack")` is `true`, so the two orderings agree and an
    // earlier version of this test passed with them swapped. The ordering is observable only when
    // the world gives an explicit list that does **not** name the directory: then a swapped order
    // skips it as "disabled", which is a wrong diagnosis for something that was never a pack.
    let dir = TempDir::new("pack-notapack");
    let world = world_with_packs(&dir, &["good"]);
    std::fs::create_dir_all(world.join("datapacks").join("not_a_pack")).expect("dir");

    // Case 1: everything enabled. The directory is rejected either way — a weak check, kept
    // because it pins the reason text.
    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);
    assert_eq!(loaded_names(&set), vec!["good"]);
    assert_eq!(set.rejected().len(), 1, "case 1: {:?}", set.rejected());
    assert!(
        set.rejected()[0].contains("not_a_pack"),
        "case 1: naming the directory: {:?}",
        set.rejected()
    );
    assert!(
        set.skipped().is_empty(),
        "case 1: and it is not a skip: {:?}",
        set.skipped()
    );

    // Case 2: an explicit list that does **not** name the directory — the observable case.
    let enabled = EnabledPacks::from_level_dat(&["good".to_owned()], &[]);
    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &enabled, Limits::DEFAULT);
    assert_eq!(loaded_names(&set), vec!["good"]);
    assert_eq!(
        set.rejected().len(),
        1,
        "case 2: an unnamed non-pack must still be rejected: {:?}",
        set.rejected()
    );
    assert!(
        set.rejected()[0].contains("no data/ directory"),
        "case 2: and the reason names the real problem: {:?}",
        set.rejected()
    );
    assert!(
        set.skipped().is_empty(),
        "case 2: nothing was disabled, so nothing may be reported as disabled: {:?}",
        set.skipped()
    );
}

#[test]
fn a_world_with_no_datapacks_directory_discovers_nothing() {
    let dir = TempDir::new("pack-nodir");
    let world = dir.path().join("world");
    std::fs::create_dir_all(&world).expect("world dir");

    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);
    assert!(set.is_empty());
    assert!(set.skipped().is_empty() && set.rejected().is_empty());
}

#[test]
fn discovery_order_is_reproducible() {
    // Two runs over the same directory must load the same packs in the same order, or a pack's
    // override would apply differently between restarts (AGENTS.md §3.6).
    let dir = TempDir::new("pack-order");
    let world = world_with_packs(&dir, &["zeta", "alpha", "mu"]);

    let mut first = DataPackSet::new();
    first.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);
    let mut second = DataPackSet::new();
    second.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);

    assert_eq!(loaded_names(&first), loaded_names(&second));
    assert_eq!(
        first.load_plan(),
        second.load_plan(),
        "the whole plan, including paths, must match"
    );
    assert_eq!(
        loaded_names(&first),
        vec!["alpha", "mu", "zeta"],
        "sorted by directory name"
    );
}

#[test]
fn the_unfiltered_discovery_still_loads_everything() {
    // The original entry point is kept and must keep its behaviour: a caller with no world to
    // consult — a test, or a tool validating a pack directory — wants all of them.
    let dir = TempDir::new("pack-unfiltered");
    let world = world_with_packs(&dir, &["alpha", "beta"]);

    let mut set = DataPackSet::new();
    set.discover_world_packs(&world, Limits::DEFAULT);
    assert_eq!(loaded_names(&set), vec!["alpha", "beta"]);
}

#[test]
fn a_pack_directory_with_no_data_directory_is_reported_at_the_right_level() {
    // `discover_world_packs_enabled` checks `data/` *before* the enabled test, so a directory that
    // is not a pack is reported as rejected whether or not the world names it. Reporting it as
    // "disabled" would be wrong: it was never a pack.
    let dir = TempDir::new("pack-order-of-checks");
    let world = world_with_packs(&dir, &["good"]);
    std::fs::create_dir_all(world.join("datapacks").join("junk")).expect("dir");

    // A world that explicitly enables `junk` — it still is not a pack.
    let enabled = EnabledPacks::from_level_dat(&["junk".to_owned(), "good".to_owned()], &[]);
    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &enabled, Limits::DEFAULT);

    assert_eq!(loaded_names(&set), vec!["good"]);
    assert_eq!(set.rejected().len(), 1, "{:?}", set.rejected());
    assert!(set.skipped().is_empty(), "{:?}", set.skipped());
}

#[test]
fn the_namespaces_of_loaded_packs_are_visible() {
    // The reason to load a pack at all: its namespaces contribute data. Asserted so a discovery
    // that loaded packs but lost their contents would be caught.
    let dir = TempDir::new("pack-namespaces");
    let world = world_with_packs(&dir, &["alpha"]);

    let mut set = DataPackSet::new();
    set.discover_world_packs_enabled(&world, &EnabledPacks::default(), Limits::DEFAULT);

    assert_eq!(loaded_names(&set), vec!["alpha"]);
    let pack = &set.load_order()[0];
    assert_eq!(pack.source, PackSource::World);
    assert!(
        pack.namespaces().contains(&"testns".to_owned()),
        "the pack's namespace is discoverable: {:?}",
        pack.namespaces()
    );
    assert_eq!(VANILLA_NAMESPACE, "minecraft");
    assert!(
        pack.namespace_dir("testns").is_dir(),
        "and its directory resolves"
    );
}
