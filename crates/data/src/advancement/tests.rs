//! Unit tests for [`super`] (P07-11).
//!
//! The three hazards the loader exists to catch — a missing parent, a cycle, a duplicate id —
//! are **absent** from vanilla 26.1.2 (measured: 0 of each), so these tests are the only place
//! they are exercised. That is the reason the detectors are here rather than assumed away.

use super::{
    Advancement, AdvancementLoadReport, AdvancementProblem, AdvancementRegistry,
    AdvancementRewards, Chain, Criterion, Frame, MAX_PARENT_DEPTH, TextComponent,
    read_advancement_file,
};
use crate::json::Limits;
use mc_core::ids::ResourceId;
use serde_json::json;

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid id")
}

/// An advancement with one criterion and no parent.
fn simple(name: &str) -> Advancement {
    Advancement {
        name: id(name),
        parent: None,
        display: None,
        criteria: vec![Criterion {
            name: "minecraft:crit".to_owned(),
            trigger: "minecraft:inventory_changed".to_owned(),
            conditions: json!({"items": [{"items": "minecraft:crafting_table"}]}),
        }],
        requirements: vec![vec!["minecraft:crit".to_owned()]],
        rewards: AdvancementRewards::default(),
        sends_telemetry_event: false,
    }
}

fn child(name: &str, parent: &str) -> Advancement {
    Advancement {
        parent: Some(id(parent)),
        ..simple(name)
    }
}

#[test]
fn a_root_has_no_parent_and_depth_one() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([simple("minecraft:story/root")]);
    let roots = registry.roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].name, id("minecraft:story/root"));
    assert_eq!(registry.depth_of(&id("minecraft:story/root")), Some(1));
    assert_eq!(
        registry.chain_of(&id("minecraft:story/root")),
        Chain::Rooted { depth: 1 }
    );
    assert!(registry.problems().is_empty(), "{:?}", registry.problems());
}

#[test]
fn parent_links_build_a_tree() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        simple("minecraft:story/root"),
        child("minecraft:story/a", "minecraft:story/root"),
        child("minecraft:story/b", "minecraft:story/a"),
    ]);
    assert_eq!(registry.len(), 3);
    assert_eq!(registry.children_of(&id("minecraft:story/root")).len(), 1);
    assert_eq!(
        registry.children_of(&id("minecraft:story/root"))[0].name,
        id("minecraft:story/a")
    );
    assert_eq!(
        registry
            .parent_of(&id("minecraft:story/b"))
            .map(|a| a.name.clone()),
        Some(id("minecraft:story/a"))
    );
    assert_eq!(registry.depth_of(&id("minecraft:story/b")), Some(3));
    assert_eq!(registry.max_depth(), Some(3));
    assert_eq!(registry.ancestors_of(&id("minecraft:story/b")).len(), 3);
    assert_eq!(registry.subtree_of(&id("minecraft:story/root")).len(), 3);

    let histogram = registry.depth_histogram();
    assert_eq!(histogram.get(&1).copied(), Some(1));
    assert_eq!(histogram.get(&2).copied(), Some(1));
    assert_eq!(histogram.get(&3).copied(), Some(1));
}

#[test]
fn a_missing_parent_is_reported_and_does_not_make_a_root() {
    // Vanilla has 0 of these. Treating one as a root would silently re-parent a subtree.
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        simple("minecraft:story/root"),
        child("minecraft:story/a", "minecraft:story/ghost"),
    ]);
    let problems = registry.problems();
    assert!(
        problems.iter().any(|problem| matches!(
            problem,
            AdvancementProblem::MissingParent { missing, .. } if missing == &id("minecraft:story/ghost")
        )),
        "{problems:?}"
    );
    assert_eq!(
        registry.roots().len(),
        1,
        "the child must not become a second root"
    );
    assert_eq!(registry.depth_of(&id("minecraft:story/a")), None);
    assert_eq!(
        registry.chain_of(&id("minecraft:story/a")),
        Chain::MissingParent {
            at: id("minecraft:story/ghost")
        }
    );
    // The ancestors walk terminates at the break rather than looping or panicking.
    assert_eq!(registry.ancestors_of(&id("minecraft:story/a")).len(), 1);
    assert_eq!(registry.max_depth(), None, "a broken chain has no answer");
}

#[test]
fn a_two_node_cycle_is_reported_and_terminates() {
    // Vanilla has 0. A walker without a visited set hangs here, so the detector is what makes
    // the tree walkable at all.
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        child("minecraft:a", "minecraft:b"),
        child("minecraft:b", "minecraft:a"),
    ]);
    let cycles = registry.cycles();
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    match &cycles[0] {
        AdvancementProblem::Cycle { path } => {
            assert_eq!(path.first(), path.last(), "the path closes on itself");
            assert!(path.len() >= 2);
        }
        other => panic!("expected a cycle, got {other}"),
    }
    assert!(registry.depth_of(&id("minecraft:a")).is_none());
    assert!(registry.depth_of(&id("minecraft:b")).is_none());
    assert_eq!(registry.max_depth(), None);
    // The walks that could hang must not.
    assert!(!registry.ancestors_of(&id("minecraft:a")).is_empty());
    assert!(!registry.subtree_of(&id("minecraft:a")).is_empty());
    let histogram = registry.depth_histogram();
    assert_eq!(
        histogram.get(&0).copied(),
        Some(2),
        "cyclic counts as depth 0"
    );
}

#[test]
fn a_self_parent_is_a_cycle() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([child("minecraft:a", "minecraft:a")]);
    assert_eq!(registry.cycles().len(), 1);
    assert_eq!(registry.chain_of(&id("minecraft:a")), Chain::Cyclic);
}

#[test]
fn a_three_node_cycle_is_reported_once() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        child("minecraft:a", "minecraft:b"),
        child("minecraft:b", "minecraft:c"),
        child("minecraft:c", "minecraft:a"),
    ]);
    assert_eq!(registry.cycles().len(), 1, "the cycle is reported once");
    match &registry.cycles()[0] {
        AdvancementProblem::Cycle { path } => assert_eq!(path.len(), 4, "{path:?}"),
        other => panic!("expected a cycle, got {other}"),
    }
}

#[test]
fn a_duplicate_id_is_recorded_and_the_later_one_wins() {
    // Impossible within one namespace and directory — a duplicate path is one file — so this
    // can only come from merging several namespaces or packs.
    let mut registry = AdvancementRegistry::new();
    let mut first = simple("minecraft:a");
    first.criteria[0].trigger = "minecraft:first".to_owned();
    let mut second = simple("minecraft:a");
    second.criteria[0].trigger = "minecraft:second".to_owned();
    registry.insert_all([first, second]);
    assert_eq!(registry.len(), 1, "a duplicate is one advancement, not two");
    assert_eq!(registry.duplicates(), [id("minecraft:a")]);
    assert_eq!(
        registry
            .by_name(&id("minecraft:a"))
            .expect("present")
            .criteria[0]
            .trigger,
        "minecraft:second",
        "the pack-override rule: the later definition wins"
    );
    assert!(
        registry
            .problems()
            .iter()
            .any(|problem| matches!(problem, AdvancementProblem::Duplicate { .. }))
    );
}

#[test]
fn merging_two_registries_reports_every_duplicate_once() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        simple("minecraft:a"),
        simple("minecraft:a"),
        simple("minecraft:a"),
    ]);
    assert_eq!(registry.duplicates().len(), 1);
    let duplicates = registry
        .problems()
        .into_iter()
        .filter(|problem| matches!(problem, AdvancementProblem::Duplicate { .. }))
        .count();
    assert_eq!(duplicates, 1);
}

#[test]
fn a_requirement_naming_no_criterion_is_reported() {
    let mut advancement = simple("minecraft:a");
    advancement.requirements = vec![vec![
        "minecraft:crit".to_owned(),
        "minecraft:ghost".to_owned(),
    ]];
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([advancement]);
    assert!(registry.problems().iter().any(|problem| matches!(
        problem,
        AdvancementProblem::UndefinedRequirement { criterion, .. } if criterion == "minecraft:ghost"
    )));
}

#[test]
fn a_criterion_no_group_mentions_is_reported_but_is_not_an_error() {
    let mut advancement = simple("minecraft:a");
    advancement.criteria.push(Criterion {
        name: "minecraft:extra".to_owned(),
        trigger: "minecraft:tick".to_owned(),
        conditions: serde_json::Value::Null,
    });
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([advancement]);
    let report = {
        let mut report = AdvancementLoadReport::default();
        report.check(&registry);
        report
    };
    assert!(report.problems().iter().any(|problem| matches!(
        problem,
        AdvancementProblem::UnreferencedCriterion { criterion, .. } if criterion == "minecraft:extra"
    )));
    assert!(
        report.errors().is_empty(),
        "an unused criterion is a smell, not a broken tree: {:?}",
        report.errors()
    );
}

#[test]
fn absent_requirements_mean_every_criterion_is_required() {
    // The format's default, materialised so a caller never special-cases it.
    let mut advancement = simple("minecraft:a");
    advancement.requirements = Vec::new();
    assert_eq!(
        advancement.effective_requirements(),
        vec![vec!["minecraft:crit"]]
    );
    assert!(advancement.undefined_requirements().is_empty());
    // And with an explicit group, that group is used verbatim.
    advancement.requirements = vec![vec!["minecraft:crit".to_owned()]];
    assert_eq!(
        advancement.effective_requirements(),
        vec![vec!["minecraft:crit"]]
    );
}

#[test]
fn a_chain_deeper_than_the_limit_is_reported_not_walked_forever() {
    let mut registry = AdvancementRegistry::new();
    let depth = MAX_PARENT_DEPTH + 8;
    registry.insert_all([simple("minecraft:n0")]);
    for level in 0..depth {
        registry.insert_all([child(
            &format!("minecraft:n{}", level + 1),
            &format!("minecraft:n{level}"),
        )]);
    }
    let deepest = format!("minecraft:n{depth}");
    assert!(matches!(
        registry.chain_of(&id(&deepest)),
        Chain::TooDeep { .. }
    ));
    assert!(
        registry
            .problems()
            .iter()
            .any(|problem| matches!(problem, AdvancementProblem::TooDeep { .. }))
    );
    // The ancestor walk is bounded rather than unbounded.
    assert!(registry.ancestors_of(&id(&deepest)).len() <= MAX_PARENT_DEPTH);
}

#[test]
fn problems_are_sorted_and_deduplicated() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        child("minecraft:a", "minecraft:ghost"),
        child("minecraft:b", "minecraft:ghost"),
        child("minecraft:c", "minecraft:a"),
    ]);
    let problems = registry.problems();
    let mut sorted = problems.clone();
    sorted.sort();
    assert_eq!(problems, sorted, "the order is reproducible");
    let mut deduped = problems.clone();
    deduped.dedup();
    assert_eq!(problems.len(), deduped.len());
}

#[test]
fn triggers_are_collected_across_the_registry() {
    let mut registry = AdvancementRegistry::new();
    let mut a = simple("minecraft:a");
    a.criteria = vec![
        Criterion {
            name: "one".to_owned(),
            trigger: "minecraft:tick".to_owned(),
            conditions: serde_json::Value::Null,
        },
        Criterion {
            name: "two".to_owned(),
            trigger: "minecraft:location".to_owned(),
            conditions: json!({}),
        },
    ];
    a.requirements = vec![vec!["one".to_owned()], vec!["two".to_owned()]];
    registry.insert_all([a]);
    let triggers = registry.triggers();
    assert_eq!(triggers.len(), 2);
    assert!(triggers.contains("minecraft:tick"));
    assert_eq!(registry.with_trigger("minecraft:tick").len(), 1);
    assert!(registry.with_trigger("minecraft:nope").is_empty());
    let advancement = registry.by_name(&id("minecraft:a")).expect("present");
    assert_eq!(advancement.criterion_names(), vec!["one", "two"]);
    assert_eq!(
        advancement.criterion("one").map(Criterion::has_conditions),
        Some(false)
    );
    assert_eq!(
        advancement.criterion("two").map(Criterion::has_conditions),
        Some(true)
    );
}

#[test]
fn an_advancement_with_no_display_is_modelled_and_has_no_icon() {
    // 1 492 of vanilla's 1 617 have no display; that is normal, not a load failure.
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([simple("minecraft:a")]);
    let advancement = registry.by_name(&id("minecraft:a")).expect("present");
    assert!(advancement.display.is_none());
    assert!(advancement.icon_item().is_none());
}

#[test]
fn a_display_with_a_translation_is_kept_as_a_key() {
    // All 125 vanilla displays use `{"translate": …}`; flattening it would put the key on
    // screen.
    let dir = mc_test_support::fixtures::TempDir::new("adv-display");
    let file = dir.path().join("root.json");
    std::fs::write(
        &file,
        r#"{
          "display": {
            "title": {"translate": "advancements.story.root.title"},
            "description": {"translate": "advancements.story.root.description"},
            "icon": {"id": "minecraft:grass_block"},
            "background": "minecraft:gui/advancements/backgrounds/stone",
            "show_toast": false,
            "announce_to_chat": false
          },
          "criteria": {"crafting_table": {"trigger": "minecraft:inventory_changed"}},
          "requirements": [["crafting_table"]],
          "sends_telemetry_event": true
        }"#,
    )
    .expect("write");
    let advancement =
        read_advancement_file(&file, id("minecraft:story/root"), mc_data_limits()).expect("parses");
    let display = advancement.display.as_ref().expect("a display");
    assert_eq!(
        display.title.translation_key(),
        Some("advancements.story.root.title")
    );
    assert!(
        !display.title.is_resolved(),
        "a key is not displayable text"
    );
    assert_eq!(
        display.description.translation_key(),
        Some("advancements.story.root.description")
    );
    assert_eq!(display.icon.item, id("minecraft:grass_block"));
    assert!(display.icon.components.is_none());
    assert_eq!(
        display.frame,
        Frame::Task,
        "an absent frame defaults to task"
    );
    assert_eq!(
        display.background,
        Some(id("minecraft:gui/advancements/backgrounds/stone"))
    );
    assert!(!display.show_toast);
    assert!(!display.announce_to_chat);
    assert!(!display.hidden, "an absent hidden defaults to false");
    assert!(advancement.sends_telemetry_event);
    // The criterion's absent `conditions` is null, not a fabricated object.
    assert!(!advancement.criteria[0].has_conditions());
}

#[test]
fn a_literal_title_and_an_explicit_frame_parse() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-literal");
    let file = dir.path().join("challenge.json");
    std::fs::write(
        &file,
        r#"{
          "display": {
            "title": "Plain title",
            "description": "Plain description",
            "icon": {"id": "minecraft:dragon_head", "components": {"minecraft:damage": 3}},
            "frame": "challenge",
            "hidden": true
          },
          "criteria": {"c": {"trigger": "minecraft:impossible", "conditions": {}}},
          "requirements": [["c"]]
        }"#,
    )
    .expect("write");
    let advancement = read_advancement_file(&file, id("minecraft:end/challenge"), mc_data_limits())
        .expect("parses");
    let display = advancement.display.as_ref().expect("a display");
    assert_eq!(
        display.title,
        TextComponent::Literal("Plain title".to_owned())
    );
    assert!(display.title.is_resolved());
    assert_eq!(display.title.literal(), Some("Plain title"));
    assert_eq!(display.frame, Frame::Challenge);
    assert!(display.hidden);
    assert!(display.show_toast, "absent show_toast defaults to true");
    assert!(display.announce_to_chat);
    assert!(
        display.icon.components.is_some(),
        "components are kept, not dropped"
    );
    assert!(advancement.criteria[0].has_conditions());
}

#[test]
fn a_rewards_block_is_modelled_but_grants_nothing() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-rewards");
    let file = dir.path().join("r.json");
    std::fs::write(
        &file,
        r#"{
          "parent": "minecraft:story/root",
          "criteria": {"c": {"trigger": "minecraft:tick"}},
          "requirements": [["c"]],
          "rewards": {
            "recipes": ["minecraft:stick", "minecraft:torch"],
            "experience": 100
          }
        }"#,
    )
    .expect("write");
    let advancement =
        read_advancement_file(&file, id("minecraft:story/r"), mc_data_limits()).expect("parses");
    assert_eq!(advancement.rewards.experience, 100);
    assert_eq!(advancement.rewards.recipes.len(), 2);
    assert!(advancement.rewards.loot.is_empty());
    assert!(advancement.rewards.function.is_none());
    assert!(!advancement.rewards.is_empty());
    assert_eq!(advancement.parent, Some(id("minecraft:story/root")));
}

#[test]
fn a_malformed_file_is_an_error_naming_the_path() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-bad");
    let cases: &[(&str, &str)] = &[
        ("c1.json", "{ not json"),
        ("c2.json", r#"{"criteria": "not an object"}"#),
        ("c3.json", r#"{"criteria": {"c": {"no_trigger": true}}}"#),
        ("c4.json", r#"{"criteria": {"c": {"trigger": 1}}}"#),
        (
            "c5.json",
            r#"{"parent": "Not A Name", "criteria": {"c": {"trigger": "t"}}}"#,
        ),
        (
            "c6.json",
            r#"{"criteria": {"c": {"trigger": "t"}}, "requirements": ["c"]}"#,
        ),
        (
            "c7.json",
            r#"{"criteria": {"c": {"trigger": "t"}}, "requirements": [[1]]}"#,
        ),
        (
            "c8.json",
            r#"{"criteria": {"c": {"trigger": "t"}}, "rewards": {"experience": "lots"}}"#,
        ),
        (
            "c9.json",
            r#"{"criteria": {"c": {"trigger": "t"}}, "rewards": 5}"#,
        ),
        (
            "c10.json",
            r#"{"criteria": {"c": {"trigger": "t"}}, "sends_telemetry_event": "yes"}"#,
        ),
    ];
    for (name, text) in cases {
        let file = dir.path().join(name);
        std::fs::write(&file, text).expect("write");
        let error =
            read_advancement_file(&file, id("minecraft:bad"), mc_data_limits()).expect_err(name);
        assert!(
            error.to_string().contains(name),
            "{name}: the message must name the file: {error}"
        );
    }
}

#[test]
fn an_absent_criteria_block_is_refused() {
    // Every one of vanilla's 1 617 states `criteria`, and an advancement with none could never
    // be completed, so its absence is refused rather than defaulted to empty.
    let dir = mc_test_support::fixtures::TempDir::new("adv-nocrit");
    let file = dir.path().join("n.json");
    std::fs::write(&file, r#"{"parent": "minecraft:story/root"}"#).expect("write");
    let error = read_advancement_file(&file, id("minecraft:bad"), mc_data_limits())
        .expect_err("must refuse");
    assert!(error.to_string().contains("criteria"), "{error}");
}

#[test]
fn an_unknown_frame_is_refused() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-frame");
    let file = dir.path().join("f.json");
    std::fs::write(
        &file,
        r#"{"display": {"title": "t", "description": "d", "icon": {"id": "minecraft:stone"}, "frame": "epic"},
            "criteria": {"c": {"trigger": "t"}}}"#,
    )
    .expect("write");
    let error = read_advancement_file(&file, id("minecraft:bad"), mc_data_limits())
        .expect_err("must refuse");
    assert!(error.to_string().contains("epic"), "{error}");
    assert_eq!(Frame::from_name("goal"), Some(Frame::Goal));
    assert_eq!(Frame::from_name("epic"), None);
    assert_eq!(Frame::Goal.name(), "goal");
}

#[test]
fn a_real_directory_loads_and_accounts_for_every_file() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-load");
    let base = dir.path().join("advancement");
    std::fs::create_dir_all(base.join("story")).expect("mkdir");
    std::fs::write(
        base.join("story/root.json"),
        r#"{"criteria": {"c": {"trigger": "minecraft:tick"}}}"#,
    )
    .expect("write");
    std::fs::write(
        base.join("story/child.json"),
        r#"{"parent": "minecraft:story/root", "criteria": {"c": {"trigger": "minecraft:tick"}}}"#,
    )
    .expect("write");
    std::fs::write(base.join("broken.json"), "{ nope").expect("write");

    let mut report = AdvancementLoadReport::default();
    let registry =
        AdvancementRegistry::load_directory(dir.path(), "minecraft", mc_data_limits(), &mut report);
    assert_eq!(report.files, 3);
    assert_eq!(report.loaded, 2);
    assert_eq!(report.skipped.len(), 1);
    assert!(
        report.is_fully_accounted(),
        "every file is loaded, unmodelled or skipped: {report:?}"
    );
    assert_eq!(report.advancements, 2);
    assert_eq!(registry.len(), 2);
    assert_eq!(registry.roots().len(), 1);

    report.check(&registry);
    assert!(report.problems().is_empty(), "{:?}", report.problems());
    assert!(report.errors().is_empty());
    assert!(report.cycles().is_empty());
}

#[test]
fn a_missing_directory_is_not_an_error() {
    let dir = mc_test_support::fixtures::TempDir::new("adv-missing");
    let mut report = AdvancementLoadReport::default();
    let registry =
        AdvancementRegistry::load_directory(dir.path(), "minecraft", mc_data_limits(), &mut report);
    assert!(registry.is_empty());
    assert_eq!(report.files, 0);
    assert!(report.is_clean());
    assert!(report.is_fully_accounted());
    assert_eq!(report.problems().len(), 0);
}

#[test]
fn an_unmodellable_file_name_is_counted_rather_than_dropped() {
    // A file whose stem is not a valid resource id cannot be named, so it cannot be loaded;
    // counting it in `unmodelled` is what keeps the file census exact.
    let dir = mc_test_support::fixtures::TempDir::new("adv-name");
    let base = dir.path().join("advancement");
    std::fs::create_dir_all(&base).expect("mkdir");
    std::fs::write(base.join("Bad Name.json"), "{}").expect("write");
    let mut report = AdvancementLoadReport::default();
    let registry =
        AdvancementRegistry::load_directory(dir.path(), "minecraft", mc_data_limits(), &mut report);
    assert!(registry.is_empty());
    assert_eq!(report.files, 1);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.total_unmodelled_files(), 1);
    assert!(report.is_fully_accounted());
    assert!(!report.is_clean());
}

#[test]
fn sorted_is_ascending_and_matches_len() {
    let mut registry = AdvancementRegistry::new();
    registry.insert_all([
        simple("minecraft:z"),
        simple("minecraft:a"),
        simple("minecraft:m"),
    ]);
    let names: Vec<String> = registry
        .sorted()
        .iter()
        .map(|advancement| advancement.name.to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "minecraft:a".to_owned(),
            "minecraft:m".to_owned(),
            "minecraft:z".to_owned()
        ]
    );
    assert_eq!(registry.sorted().len(), registry.len());
    assert_eq!(registry.names().count(), registry.len());
}

fn mc_data_limits() -> Limits {
    Limits::DEFAULT
}
