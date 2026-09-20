//! Selector tests, weighted towards the filters that silently select the wrong set
//! (P07-06, P07-15).

use super::{
    Bound, EntityFacts, GameMode, SelectorError, SelectorKind, Sort, UNSUPPORTED_OPTIONS, parse,
};

fn player(name: &str, position: (f64, f64, f64)) -> EntityFacts<'_> {
    EntityFacts {
        type_id: "minecraft:player",
        name: Some(name),
        position,
        level: Some(0),
        game_mode: Some(GameMode::Survival),
        is_player: true,
    }
}

fn zombie(position: (f64, f64, f64)) -> EntityFacts<'static> {
    EntityFacts {
        type_id: "minecraft:zombie",
        name: None,
        position,
        level: None,
        game_mode: None,
        is_player: false,
    }
}

#[test]
fn every_selector_letter_parses() {
    for (text, kind) in [
        ("@a", SelectorKind::AllPlayers),
        ("@p", SelectorKind::NearestPlayer),
        ("@r", SelectorKind::RandomPlayer),
        ("@s", SelectorKind::SelfEntity),
        ("@e", SelectorKind::AllEntities),
        ("@n", SelectorKind::NearestEntity),
    ] {
        let selector = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(selector.kind, kind, "{text}");
        assert_eq!(selector.kind.name(), text);
        assert_eq!(selector.filter_count(), 0);
        assert!(selector.types.is_empty() && selector.names.is_empty());
    }
}

#[test]
fn range_syntax_covers_all_five_forms() {
    assert_eq!(Bound::parse("5").expect("exact"), Bound::exactly(5.0));
    assert_eq!(
        Bound::parse("..5").expect("at most"),
        Bound {
            min: None,
            max: Some(5.0)
        }
    );
    assert_eq!(
        Bound::parse("5..").expect("at least"),
        Bound {
            min: Some(5.0),
            max: None
        }
    );
    assert_eq!(
        Bound::parse("5..10").expect("between"),
        Bound {
            min: Some(5.0),
            max: Some(10.0)
        }
    );
    assert_eq!(
        Bound::parse("..").expect("anything"),
        Bound {
            min: None,
            max: None
        }
    );

    // Membership, including the boundaries.
    let between = Bound::parse("5..10").expect("between");
    assert!(!between.contains(4.99));
    assert!(between.contains(5.0));
    assert!(between.contains(10.0));
    assert!(!between.contains(10.01));
    assert!(Bound::parse("..").expect("any").contains(f64::MAX));
    assert!(Bound::parse("..").expect("any").contains(0.0));
}

#[test]
fn a_malformed_bound_is_refused_with_a_reason() {
    for bad in ["", "x", "5..x", "..x..", "10..5", "5..5..5"] {
        let result = Bound::parse(bad);
        assert!(result.is_err(), "{bad:?} must be refused, got {result:?}");
        assert!(!result.unwrap_err().is_empty(), "{bad:?} needs a message");
    }
    // Non-finite values are refused, though `f64::parse` accepts the spellings.
    for bad in ["NaN", "inf", "-inf", "..NaN"] {
        assert!(Bound::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_boundary_is_inclusive_on_both_ends() {
    // Vanilla's `distance=..10` includes exactly 10, and `10..` includes exactly 10. An
    // exclusive comparison here would silently drop the entity standing on the line.
    let at_most = Bound::parse("..10").expect("at most");
    assert!(at_most.contains(10.0));
    let at_least = Bound::parse("10..").expect("at least");
    assert!(at_least.contains(10.0));
}

#[test]
fn type_filters_are_negatable_and_accumulate() {
    let selector = parse("@e[type=minecraft:zombie]").expect("parses");
    assert_eq!(selector.types.len(), 1);
    assert!(!selector.types[0].negated);
    assert!(selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    assert!(!selector.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));

    let negated = parse("@e[type=!minecraft:zombie]").expect("parses");
    assert!(negated.types[0].negated);
    assert!(!negated.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    assert!(negated.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));

    // Written twice: both are kept, so `type=!a,type=b` means "b and not a".
    let both = parse("@e[type=!minecraft:zombie,type=minecraft:zombie]").expect("parses");
    assert_eq!(both.types.len(), 2);
    assert!(
        !both.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
        "the negation must win over the positive"
    );
}

#[test]
fn a_selector_with_two_positive_types_matches_either() {
    // `type=a,type=b` is a union, which is what Vanilla does — not an intersection, which
    // would match nothing.
    let selector = parse("@e[type=minecraft:zombie,type=minecraft:skeleton]").expect("parses");
    assert!(selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    let skeleton = EntityFacts {
        type_id: "minecraft:skeleton",
        ..zombie((0.0, 0.0, 0.0))
    };
    assert!(selector.matches(&skeleton, (0.0, 0.0, 0.0)));
    let creeper = EntityFacts {
        type_id: "minecraft:creeper",
        ..zombie((0.0, 0.0, 0.0))
    };
    assert!(!selector.matches(&creeper, (0.0, 0.0, 0.0)));
}

#[test]
fn distance_is_measured_from_the_given_centre() {
    let selector = parse("@e[distance=..10]").expect("parses");
    assert!(selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    // 3-4-5 triangle: exactly 5 blocks away.
    assert!(selector.matches(&zombie((3.0, 4.0, 0.0)), (0.0, 0.0, 0.0)));
    // Just past the boundary.
    assert!(!selector.matches(&zombie((10.0, 0.0, 1.0)), (0.0, 0.0, 0.0)));
    // The centre moves with the caller, which is the whole reason it is a parameter.
    assert!(selector.matches(&zombie((100.0, 0.0, 0.0)), (100.0, 0.0, 0.0)));
    assert!(!selector.matches(&zombie((100.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
}

#[test]
fn a_selector_position_overrides_nothing_until_the_caller_uses_it() {
    let selector = parse("@e[x=10,y=20,z=30]").expect("parses");
    assert_eq!(selector.position, Some((10.0, 20.0, 30.0)));
    // Partial: the axes not given default to 0, and the parse reports all three.
    let partial = parse("@e[x=10]").expect("parses");
    assert_eq!(partial.position, Some((10.0, 0.0, 0.0)));
    let yz = parse("@e[y=1,z=2]").expect("parses");
    assert_eq!(yz.position, Some((0.0, 1.0, 2.0)));
}

#[test]
fn level_filters_exclude_entities_with_no_level() {
    let selector = parse("@e[level=5..]").expect("parses");
    let levelled = EntityFacts {
        level: Some(10),
        ..player("Alex", (0.0, 0.0, 0.0))
    };
    assert!(selector.matches(&levelled, (0.0, 0.0, 0.0)));
    let low = EntityFacts {
        level: Some(1),
        ..player("Alex", (0.0, 0.0, 0.0))
    };
    assert!(!selector.matches(&low, (0.0, 0.0, 0.0)));
    // A zombie has no experience level, so a level filter cannot match it.
    assert!(!selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
}

#[test]
fn game_mode_filters_are_negatable_and_accept_short_forms() {
    for text in ["creative", "c", "1"] {
        let selector = parse(&format!("@a[gamemode={text}]")).expect("parses");
        let creative = EntityFacts {
            game_mode: Some(GameMode::Creative),
            ..player("Alex", (0.0, 0.0, 0.0))
        };
        assert!(selector.matches(&creative, (0.0, 0.0, 0.0)), "{text}");
        assert!(!selector.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    }

    let not_creative = parse("@a[gamemode=!creative]").expect("parses");
    assert!(not_creative.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    let creative = EntityFacts {
        game_mode: Some(GameMode::Creative),
        ..player("Alex", (0.0, 0.0, 0.0))
    };
    assert!(!not_creative.matches(&creative, (0.0, 0.0, 0.0)));

    // The non-player cases need `@e`, because `@a` is player-only and would exclude a
    // zombie for a *different* reason — an earlier version of this test used `@a` here and
    // so asserted nothing about the game-mode rule at all.
    assert!(
        !not_creative.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
        "`@a` must exclude a zombie regardless of gamemode="
    );
    let any_entity = parse("@e[gamemode=!creative]").expect("parses");
    assert!(
        any_entity.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
        "an all-negated gamemode filter does not exclude an entity with no game mode"
    );
    // A zombie has no game mode, so a positive filter cannot match it.
    assert!(
        !parse("@e[gamemode=survival]")
            .expect("parses")
            .matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0))
    );
}

#[test]
fn name_filters_are_negatable_and_case_sensitive() {
    let selector = parse("@a[name=Alex]").expect("parses");
    assert!(selector.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    assert!(!selector.matches(&player("alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    // A non-player has no name, so a positive name filter excludes it.
    assert!(!selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));

    let not_alex = parse("@a[name=!Alex]").expect("parses");
    assert!(!not_alex.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    assert!(not_alex.matches(&player("Bob", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
}

#[test]
fn player_only_selectors_never_match_a_non_player() {
    for text in ["@a", "@p", "@r", "@s"] {
        let selector = parse(text).expect("parses");
        assert!(
            !selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
            "{text} must not select a zombie"
        );
        assert!(selector.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)));
    }
    // `@e` and `@n` are not player-only.
    for text in ["@e", "@n"] {
        let selector = parse(text).expect("parses");
        assert!(
            selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
            "{text}"
        );
        assert!(
            selector.matches(&player("Alex", (0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
            "{text}"
        );
        assert!(
            !selector.is_player_only(),
            "{text} spans entities, so it is not player-only"
        );
    }
    // The other direction, and the one that makes `is_player_only` a real predicate: these
    // four are player-only, which is *why* `matches` refuses a zombie for them. Asserting
    // the method and the predicate together is what would have caught the predecessor's
    // one-answer bug.
    for text in ["@a", "@p", "@r", "@s"] {
        let selector = parse(text).expect("parses");
        assert!(selector.is_player_only(), "{text} is player-only");
        assert!(
            !selector.matches(&zombie((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0)),
            "{text} must not match a non-player"
        );
    }
    // And `defaults_to_entities` says something different: whether an unfiltered selector
    // spans entities or only players. `@e`/`@n` do; `@a`/`@p`/`@r`/`@s` do not.
    for text in ["@e", "@n"] {
        assert!(
            parse(text).expect("parses").kind.defaults_to_entities(),
            "{text} spans entities"
        );
    }
    for text in ["@a", "@p", "@r", "@s"] {
        assert!(
            !parse(text).expect("parses").kind.defaults_to_entities(),
            "{text} is player-only"
        );
    }
}

#[test]
fn limit_and_sort_parse_and_the_effective_limit_is_derived() {
    let selector = parse("@e[limit=3,sort=furthest]").expect("parses");
    assert_eq!(selector.limit, Some(3));
    assert_eq!(selector.sort, Sort::Furthest);
    assert_eq!(selector.effective_limit(), Some(3));

    // A single-entity selector has an implied limit of 1, which is what bounds a caller's
    // work when the command writer did not say.
    for text in ["@p", "@r", "@s", "@n"] {
        let selector = parse(text).expect("parses");
        assert_eq!(selector.effective_limit(), Some(1), "{text}");
    }
    // `@a`/`@e` are unbounded unless limited — the case a caller must handle.
    for text in ["@a", "@e"] {
        assert_eq!(
            parse(text).expect("parses").effective_limit(),
            None,
            "{text}"
        );
    }
    // `@p[limit=1]` is legal even though it is redundant.
    assert_eq!(parse("@p[limit=1]").expect("parses").limit, Some(1));
}

#[test]
fn a_limit_on_a_single_entity_selector_is_a_contradiction() {
    for text in ["@p[limit=2]", "@s[limit=5]", "@r[limit=10]", "@n[limit=2]"] {
        match parse(text) {
            Err(SelectorError::LimitOnSingleEntity(_)) => {}
            other => panic!("{text} should be refused, got {other:?}"),
        }
    }
}

#[test]
fn a_limit_of_zero_is_refused() {
    match parse("@e[limit=0]") {
        Err(SelectorError::BadValue { reason, .. }) => assert!(reason.contains('0')),
        other => panic!("expected a bad value, got {other:?}"),
    }
}

#[test]
fn every_sort_order_parses_and_names_itself() {
    for (text, sort, random) in [
        ("nearest", Sort::Nearest, false),
        ("furthest", Sort::Furthest, false),
        ("random", Sort::Random, true),
        ("arbitrary", Sort::Arbitrary, false),
    ] {
        assert_eq!(Sort::parse(text).expect("parses"), sort);
        assert_eq!(sort.name(), text);
        assert_eq!(sort.needs_random(), random, "{text}");
    }
    assert!(Sort::parse("closest").is_err());
    assert_eq!(Sort::default(), Sort::Nearest, "Vanilla's default");
}

#[test]
fn unsupported_options_are_named_rather_than_ignored() {
    // This is the property that matters most in this file: a silently-dropped filter
    // selects the wrong entities, which is worse than refusing the command.
    for option in UNSUPPORTED_OPTIONS {
        let text = format!("@e[{option}=something]");
        match parse(&text) {
            Err(SelectorError::UnsupportedOption(name)) => assert_eq!(&name, option),
            other => panic!("{text} must name {option}, got {other:?}"),
        }
    }
    // An option that exists in no version is also refused, by name.
    match parse("@e[bogus=1]") {
        Err(SelectorError::UnsupportedOption(name)) => assert_eq!(name, "bogus"),
        other => panic!("expected UnsupportedOption, got {other:?}"),
    }
}

#[test]
fn an_unsupported_option_with_a_comma_in_its_value_is_still_named() {
    // `scores={a=1,b=2}` contains a comma. A naive split would produce two options and
    // report `scores` for one and `b=2` for the other, losing the real diagnosis.
    match parse("@e[scores={a=1,b=2}]") {
        Err(SelectorError::UnsupportedOption(name)) => assert_eq!(name, "scores"),
        other => panic!("expected `scores` to be named, got {other:?}"),
    }
    match parse("@e[nbt={a:[1,2],b:3}]") {
        Err(SelectorError::UnsupportedOption(name)) => assert_eq!(name, "nbt"),
        other => panic!("expected `nbt` to be named, got {other:?}"),
    }
}

#[test]
fn malformed_selectors_are_refused_with_distinguishable_errors() {
    assert!(matches!(parse("Alex"), Err(SelectorError::NotASelector(_))));
    assert!(matches!(parse(""), Err(SelectorError::NotASelector(_))));
    assert!(matches!(parse("@"), Err(SelectorError::NotASelector(_))));
    assert!(matches!(
        parse("@z"),
        Err(SelectorError::UnknownSelector('z'))
    ));
    assert!(matches!(
        parse("@e["),
        Err(SelectorError::UnterminatedOptions)
    ));
    assert!(matches!(
        parse("@e[type=minecraft:zombie"),
        Err(SelectorError::UnterminatedOptions)
    ));
    assert!(matches!(
        parse("@e[type]"),
        Err(SelectorError::MalformedOption(_))
    ));
    assert!(matches!(parse("@e[]"), Err(SelectorError::EmptyOption)));
    assert!(matches!(parse("@e[ ]"), Err(SelectorError::EmptyOption)));
    // A trailing comma produces an empty option, which is refused rather than ignored.
    // Which error it lands on is an implementation detail; that it *is* an error is not.
    assert!(
        parse("@e[type=minecraft:zombie,]").is_err(),
        "a trailing comma must not be accepted"
    );
    // Every error has a message, so none is a silent failure.
    for text in ["Alex", "@z", "@e[", "@e[type]", "@e[]"] {
        let error = parse(text).expect_err(text);
        assert!(!error.to_string().is_empty(), "{text}");
    }
}

#[test]
fn a_duplicated_single_valued_option_is_refused() {
    // Last-wins would make `[distance=..1,distance=..100]` silently mean whichever came
    // last; refusing makes the mistake visible.
    for text in [
        "@e[distance=..1,distance=..100]",
        "@e[limit=1,limit=2]",
        "@e[sort=nearest,sort=furthest]",
        "@e[x=1,x=2]",
        "@e[level=1,level=2]",
    ] {
        match parse(text) {
            Err(SelectorError::DuplicateOption(_)) => {}
            other => panic!("{text} must be refused as a duplicate, got {other:?}"),
        }
    }
    // Accumulating options are *not* duplicates: Vanilla merges repeated `type=` and `name=`.
    assert!(parse("@e[type=a,type=b]").is_ok());
    assert!(parse("@a[name=a,name=b]").is_ok());
    assert!(parse("@a[gamemode=creative,gamemode=survival]").is_ok());
}

#[test]
fn hostile_selectors_do_not_panic() {
    let cases = [
        "@e[",
        "@e]",
        "@e[[[[[[[[[[",
        "@e]]]]]]]]]]",
        "@e[type=]",
        "@e[name=]",
        "@e[distance=..]",
        "@e[distance=....]",
        "@e[limit=-1]",
        "@e[limit=99999999999999999999]",
        "@e[limit=4294967296]",
        "@e[x=NaN]",
        "@e[x=inf]",
        "@e[x=1e400]",
        "@e[type=minecraft:]",
        "@e[type=:]",
        "@e[type=#minecraft:logs]",
        "@a[a][b]",
        "@e[=1]",
        "@e[=]",
        "@e[,]",
        "@e[,,,]",
        "@",
        "@@@@",
        "@e[sort=]",
        "@e[gamemode=]",
        "@e[type=!]",
        "@e[name=!]",
        &format!("@e[type={}]", "a".repeat(10_000)),
        &format!("@e[{}]", "x=1,".repeat(5_000)),
    ];
    for case in cases {
        // The only requirement is that it returns an outcome rather than unwinding. A
        // parse failure is a perfectly good outcome.
        let _ = parse(case);
    }
}

#[test]
fn a_very_long_option_list_is_bounded_by_the_caller_not_by_this_parser() {
    // The selector parser itself has no length limit, because `MAX_COMMAND_CHARS` in the
    // dispatcher already bounds the whole command and duplicating the bound here would
    // give two places to keep in step. What matters is that a long list parses without
    // panicking and without quadratic blowup.
    let long = format!(
        "@e[{}]",
        (0..500)
            .map(|i| format!("type=minecraft:t{i}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let selector = parse(&long).expect("500 type filters is legal");
    assert_eq!(selector.types.len(), 500);
    assert_eq!(selector.filter_count(), 500);
}

#[test]
fn a_selector_with_every_modelled_option_parses_together() {
    let selector = parse(
        "@e[type=minecraft:zombie,name=Bob,distance=1..10,level=..5,gamemode=!creative,\
         limit=3,sort=random,x=1,y=2,z=3]",
    )
    .expect("parses");
    assert_eq!(selector.kind, SelectorKind::AllEntities);
    assert_eq!(selector.types.len(), 1);
    assert_eq!(selector.names.len(), 1);
    assert_eq!(
        selector.distance,
        Some(Bound::parse("1..10").expect("bound"))
    );
    assert_eq!(selector.level, Some(Bound::parse("..5").expect("bound")));
    assert_eq!(selector.game_modes, vec![(GameMode::Creative, true)]);
    assert_eq!(selector.limit, Some(3));
    assert_eq!(selector.sort, Sort::Random);
    assert_eq!(selector.position, Some((1.0, 2.0, 3.0)));
    assert_eq!(selector.filter_count(), 5, "one per filter kind present");

    // And it matches only what all the filters agree on.
    assert!(
        !selector.matches(&zombie((5.0, 2.0, 3.0)), (1.0, 2.0, 3.0)),
        "name=Bob excludes a nameless zombie"
    );
}

#[test]
fn game_modes_and_selector_kinds_have_names() {
    for (mode, text) in [
        (GameMode::Survival, "survival"),
        (GameMode::Creative, "creative"),
        (GameMode::Adventure, "adventure"),
        (GameMode::Spectator, "spectator"),
    ] {
        assert_eq!(mode.name(), text);
        assert_eq!(GameMode::parse(text).expect("parses"), mode);
    }
    assert!(GameMode::parse("hardcore").is_err());

    // Single vs multi, which `effective_limit` and the `limit` check both rely on.
    assert!(SelectorKind::NearestPlayer.is_single());
    assert!(SelectorKind::RandomPlayer.is_single());
    assert!(SelectorKind::SelfEntity.is_single());
    assert!(SelectorKind::NearestEntity.is_single());
    assert!(!SelectorKind::AllPlayers.is_single());
    assert!(!SelectorKind::AllEntities.is_single());
    assert!(SelectorKind::AllEntities.defaults_to_entities());
    assert!(SelectorKind::NearestEntity.defaults_to_entities());
    assert!(!SelectorKind::AllPlayers.defaults_to_entities());
}

#[test]
fn selector_error_messages_are_stable() {
    // P15-06: player-visible strings; the thiserror migration must not reword them.
    let cases = [
        (
            SelectorError::NotASelector("foo".to_owned()),
            "\"foo\" is not a selector",
        ),
        (SelectorError::UnknownSelector('x'), "@x is not a selector"),
        (
            SelectorError::UnterminatedOptions,
            "a selector's [ is never closed",
        ),
        (
            SelectorError::MalformedOption("a".to_owned()),
            "option \"a\" has no value",
        ),
        (
            SelectorError::UnsupportedOption("tag".to_owned()),
            "tag= is not supported by this build",
        ),
        (
            SelectorError::BadValue {
                option: "limit".to_owned(),
                reason: "bad".to_owned(),
            },
            "limit=: bad",
        ),
        (
            SelectorError::DuplicateOption("type".to_owned()),
            "type= is given twice",
        ),
        (
            SelectorError::LimitOnSingleEntity("@p".to_owned()),
            "@p names one entity, so limit= is a contradiction",
        ),
        (SelectorError::EmptyOption, "a selector has an empty option"),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
