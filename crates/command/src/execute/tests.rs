//! `/execute` tests, weighted towards the constructs that must be *refused* rather than
//! ignored (P07-07, P07-15).

use super::{
    Axes, Condition, ExecuteError, MAX_MODIFIERS, Modifier, UNSUPPORTED_CONDITIONS,
    UNSUPPORTED_MODIFIERS, align, parse, resolve_coordinates,
};
use crate::selector::SelectorKind;

fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

fn parse_str(text: &str) -> Result<super::ExecuteChain, ExecuteError> {
    parse(&tokens(text))
}

#[test]
fn a_bare_run_carries_no_modifiers() {
    let chain = parse_str("run say hi").expect("parses");
    assert!(chain.modifiers.is_empty());
    assert_eq!(chain.run, "say hi");
    assert!(!chain.has_as());
    assert!(!chain.has_position_modifier());
    assert_eq!(chain.condition_count(), 0);
}

#[test]
fn modifiers_parse_in_every_supported_form() {
    let chain = parse_str("as @a at @s positioned 1 2 3 align xyz run say hi").expect("parses");
    assert_eq!(chain.modifiers.len(), 4);
    assert!(matches!(&chain.modifiers[0], Modifier::As(s) if s.kind == SelectorKind::AllPlayers));
    assert!(matches!(&chain.modifiers[1], Modifier::At(s) if s.kind == SelectorKind::SelfEntity));
    assert!(matches!(
        &chain.modifiers[2],
        Modifier::Positioned {
            x: Some(1),
            y: Some(2),
            z: Some(3)
        }
    ));
    assert!(matches!(
        &chain.modifiers[3],
        Modifier::Align(a) if a.x && a.y && a.z
    ));
    assert_eq!(chain.run, "say hi");
    assert!(chain.has_as());
    assert!(chain.has_position_modifier());
}

#[test]
fn modifier_order_is_preserved_because_it_changes_the_meaning() {
    // `as @a at @s` and `at @s as @a` are different commands in Vanilla: the first takes each
    // player's position, the second takes the source's. Normalising the order would silently
    // change one of them.
    let first = parse_str("as @a at @s run say hi").expect("parses");
    let second = parse_str("at @s as @a run say hi").expect("parses");
    assert_ne!(first.modifiers, second.modifiers);
    assert!(matches!(first.modifiers[0], Modifier::As(_)));
    assert!(matches!(second.modifiers[0], Modifier::At(_)));
}

#[test]
fn repeated_modifiers_are_kept_because_the_last_one_wins_at_resolution() {
    // `as @a as @p` is legal and means `@p`. Keeping both lets the caller apply them in order
    // rather than this layer having to decide which is "the" one.
    let chain = parse_str("as @a as @p run say hi").expect("parses");
    assert_eq!(chain.modifiers.len(), 2);
    assert!(
        matches!(&chain.modifiers[1], Modifier::As(s) if s.kind == SelectorKind::NearestPlayer)
    );
}

#[test]
fn a_condition_is_parsed_for_entity_and_block() {
    let chain = parse_str("if entity @a run say hi").expect("parses");
    assert!(matches!(
        &chain.modifiers[0],
        Modifier::If(Condition::Entity(_))
    ));
    assert_eq!(chain.condition_count(), 1);

    let chain = parse_str("unless block 1 2 3 minecraft:stone run say hi").expect("parses");
    match &chain.modifiers[0] {
        Modifier::Unless(Condition::Block { x, y, z, block }) => {
            assert_eq!((*x, *y, *z), (Some(1), Some(2), Some(3)));
            assert_eq!(block.namespace(), "minecraft");
            assert_eq!(block.value(), "stone");
        }
        other => panic!("expected an unless-block, got {other:?}"),
    }
}

#[test]
fn a_block_condition_accepts_relative_coordinates() {
    let chain = parse_str("if block ~ ~1 ~ minecraft:air run say hi").expect("parses");
    match &chain.modifiers[0] {
        Modifier::If(Condition::Block { x, y, z, .. }) => {
            assert_eq!((*x, *y, *z), (None, Some(1), None));
        }
        other => panic!("expected an if-block, got {other:?}"),
    }
}

#[test]
fn missing_run_is_refused() {
    for text in ["", "as @a", "as @a at @s", "if entity @a"] {
        match parse_str(text) {
            Err(ExecuteError::MissingRun) => {}
            other => panic!("{text:?} must be refused as missing run, got {other:?}"),
        }
    }
}

#[test]
fn an_empty_run_is_refused() {
    match parse_str("as @a run") {
        Err(ExecuteError::EmptyRun) => {}
        other => panic!("expected EmptyRun, got {other:?}"),
    }
}

#[test]
fn unsupported_modifiers_are_named_rather_than_ignored() {
    // The property that matters most here: a dropped `store` sends output to chat instead of a
    // block, and a dropped `rotated` aims a command the wrong way. Refusing names the problem.
    for modifier in UNSUPPORTED_MODIFIERS {
        let text = format!("{modifier} something run say hi");
        match parse_str(&text) {
            Err(ExecuteError::UnsupportedModifier(name)) => assert_eq!(&name, modifier),
            other => panic!("{text:?} must name {modifier}, got {other:?}"),
        }
    }
    // And a keyword that exists in no version is refused by name too.
    match parse_str("bogus 1 run say hi") {
        Err(ExecuteError::UnsupportedModifier(name)) => assert_eq!(name, "bogus"),
        other => panic!("expected UnsupportedModifier, got {other:?}"),
    }
}

#[test]
fn unsupported_conditions_are_named_rather_than_ignored() {
    for condition in UNSUPPORTED_CONDITIONS {
        for keyword in ["if", "unless"] {
            let text = format!("{keyword} {condition} whatever run say hi");
            match parse_str(&text) {
                Err(ExecuteError::UnsupportedCondition(name)) => assert_eq!(&name, condition),
                other => panic!("{text:?} must name {condition}, got {other:?}"),
            }
        }
    }
}

#[test]
fn a_modifier_missing_its_argument_is_refused() {
    for text in [
        "as",
        "at",
        "positioned",
        "positioned 1",
        "positioned 1 2",
        "align",
        "if",
        "if entity",
        "if block",
        "if block 1",
        "if block 1 2",
        "if block 1 2 3",
    ] {
        // Each is missing something; with `run` appended it must still be refused, and without
        // it the refusal is `MissingRun`. Either way it must not parse.
        let with_run = format!("{text} run say hi");
        let result = parse_str(&with_run);
        assert!(
            result.is_err(),
            "{with_run:?} must be refused, got {result:?}"
        );
    }
}

#[test]
fn a_malformed_argument_is_refused_with_the_modifier_named() {
    let cases: &[(&str, &str)] = &[
        ("positioned x 2 3 run say hi", "positioned"),
        ("positioned 1 y 3 run say hi", "positioned"),
        ("positioned 1 2 z run say hi", "positioned"),
        ("positioned ~x 2 3 run say hi", "positioned"),
        ("align q run say hi", "align"),
        ("as @z run say hi", "as"),
        ("if entity @z run say hi", "if"),
        // A genuinely invalid id. `"not"` would *not* do: a bare path with no namespace is
        // a legal resource id, so that fixture parsed successfully and the failure it produced
        // came from the following token — the test was wrong, not the parser.
        ("if block 1 2 3 : run say hi", "if"),
    ];
    for (text, expected) in cases {
        match parse_str(text) {
            Err(ExecuteError::BadValue { modifier, reason }) => {
                assert_eq!(&modifier, expected, "{text}");
                assert!(!reason.is_empty(), "{text} needs a reason");
            }
            other => panic!("{text:?} must be a BadValue naming {expected}, got {other:?}"),
        }
    }
}

#[test]
fn the_chain_length_is_bounded() {
    let long = format!("{}run say hi", "as @s ".repeat(MAX_MODIFIERS + 1));
    match parse_str(&long) {
        Err(ExecuteError::TooLong { count, limit }) => {
            assert_eq!(limit, MAX_MODIFIERS);
            assert!(count > limit, "{count}");
        }
        other => panic!("expected TooLong, got {other:?}"),
    }
    // Exactly at the limit is accepted, so the boundary is where it claims to be.
    let at_limit = format!("{}run say hi", "as @s ".repeat(MAX_MODIFIERS));
    let chain = parse_str(&at_limit).expect("MAX_MODIFIERS is legal");
    assert_eq!(chain.modifiers.len(), MAX_MODIFIERS);
}

#[test]
fn the_run_text_is_carried_whole() {
    // Everything after `run` belongs to the inner command, including words that look like
    // `execute` modifiers — otherwise `/execute run say as @a` would lose its message.
    let chain = parse_str("as @a run say as @a run").expect("parses");
    assert_eq!(chain.run, "say as @a run");
    assert_eq!(chain.modifiers.len(), 1);
}

#[test]
fn nested_execute_is_representable_because_the_inner_text_is_opaque() {
    // The recursion is the caller's, with its own depth bound; this layer must not try to
    // flatten it, or `/execute run execute run …` would be parsed twice with different results.
    let chain = parse_str("as @a run execute at @s run say hi").expect("parses");
    assert_eq!(chain.run, "execute at @s run say hi");
    assert_eq!(chain.modifiers.len(), 1);
}

#[test]
fn axes_parse_and_refuse_repeats_and_unknowns() {
    assert_eq!(
        Axes::parse("xyz").expect("parses"),
        Axes {
            x: true,
            y: true,
            z: true
        }
    );
    assert_eq!(
        Axes::parse("xz").expect("parses"),
        Axes {
            x: true,
            y: false,
            z: true
        }
    );
    assert_eq!(
        Axes::parse("y").expect("parses"),
        Axes {
            x: false,
            y: true,
            z: false
        }
    );
    assert_eq!(Axes::parse("xyz").expect("parses").count(), 3);
    assert_eq!(Axes::parse("y").expect("parses").count(), 1);
    assert!(!Axes::parse("y").expect("parses").is_empty());

    for bad in ["", "q", "xyq", "xx", "xyx", "zz", "X"] {
        let result = Axes::parse(bad);
        assert!(result.is_err(), "{bad:?} must be refused, got {result:?}");
        assert!(!result.unwrap_err().is_empty(), "{bad:?} needs a message");
    }
    assert_eq!(
        Axes::default(),
        Axes {
            x: false,
            y: false,
            z: false
        }
    );
    assert!(Axes::default().is_empty());
}

#[test]
fn aligning_floors_rather_than_truncating() {
    // Vanilla's `align` floors. Truncating toward zero would give a different answer for
    // negative coordinates, which is exactly where a silent mistake would live.
    let all = Axes {
        x: true,
        y: true,
        z: true,
    };
    assert_eq!(align((1.9, 2.1, 3.5), all), (1.0, 2.0, 3.0));
    assert_eq!(align((-0.5, -1.5, -2.5), all), (-1.0, -2.0, -3.0));
    // Positive coordinates cannot tell the two rules apart; the negative case above is the
    // one that matters.

    let x_only = Axes {
        x: true,
        y: false,
        z: false,
    };
    assert_eq!(align((1.9, 2.9, 3.9), x_only), (1.0, 2.9, 3.9));

    // An unselected axis is untouched, including its sign and fraction.
    assert_eq!(align((-0.5, 0.0, 0.0), Axes::default()), (-0.5, 0.0, 0.0));
}

#[test]
fn bare_tilde_resolves_to_the_source_and_not_to_zero() {
    // `~` means "wherever the source is"; `~0` is an offset of zero. They are the same number
    // only when the source is at the origin, so the distinction has to survive to here.
    let source = (10, 64, -20);
    assert_eq!(resolve_coordinates(None, None, None, source), source);
    assert_eq!(
        resolve_coordinates(Some(0), Some(0), Some(0), source),
        (0, 0, 0)
    );
    assert_eq!(
        resolve_coordinates(Some(5), Some(-3), None, source),
        (5, -3, -20)
    );
}

#[test]
fn a_bare_unqualified_name_is_a_legal_block_id() {
    // This is what made a fixture of mine wrong: `minecraft:` is optional in the resource
    // format, so `if block 1 2 3 stone run …` is a valid chain and the parser must accept it.
    let chain = parse_str("if block 1 2 3 stone run say hi").expect("a bare path is legal");
    match &chain.modifiers[0] {
        Modifier::If(Condition::Block { block, .. }) => {
            assert_eq!(block.value(), "stone");
            assert_eq!(
                block.namespace(),
                "minecraft",
                "the default namespace is filled in"
            );
        }
        other => panic!("expected an if-block, got {other:?}"),
    }
}

#[test]
fn hostile_chains_do_not_panic() {
    let cases = [
        "",
        " ",
        "run",
        "run ",
        "as",
        "as ",
        "as @e[",
        "at @e[type=]",
        "positioned 99999999999999999999 0 0 run x",
        "positioned ~99999999999999999999 0 0 run x",
        "align xyzxyz",
        "if",
        "if entity",
        "if block ~ ~ ~",
        "if block ~ ~ ~ #minecraft:logs run x",
        "unless unless run x",
        "run run run",
        &"as @s ".repeat(500),
        &format!("run {}", "x".repeat(10_000)),
        "\u{0} run x",
        "as @e[type=minecraft:zombie,distance=..10] run say hi",
    ];
    for case in cases {
        // The only requirement is an outcome rather than an unwind.
        let _ = parse_str(case);
    }
}

#[test]
fn a_realistic_chain_parses_completely() {
    let chain = parse_str(
        "as @a[gamemode=!spectator] at @s positioned ~ ~1 ~ align xz \
         if entity @e[type=minecraft:zombie,distance=..5] run say hello there",
    )
    .expect("parses");
    // `as`, `at`, `positioned`, `align`, `if` — five.
    assert_eq!(chain.modifiers.len(), 5);
    assert_eq!(chain.condition_count(), 1);
    assert!(chain.has_as() && chain.has_position_modifier());
    assert_eq!(chain.run, "say hello there");
}

#[test]
fn every_error_has_a_message_so_none_is_a_silent_failure() {
    let errors = vec![
        parse_str("as @a").expect_err("missing run"),
        parse_str("run").expect_err("empty run"),
        parse_str("store x run y").expect_err("unsupported"),
        parse_str("if score x run y").expect_err("unsupported condition"),
        parse_str("as").expect_err("missing argument"),
        parse_str("align q run y").expect_err("bad value"),
        parse_str(&format!("{}run y", "as @s ".repeat(MAX_MODIFIERS + 1))).expect_err("too long"),
    ];
    for error in errors {
        let text = error.to_string();
        assert!(!text.is_empty(), "{error:?}");
        // And each is usable as an error.
        let _: &dyn std::error::Error = &error;
    }
}

#[test]
fn execute_error_messages_are_stable() {
    // P15-06: player-visible strings; the thiserror migration must not reword them.
    let cases = [
        (
            ExecuteError::MissingRun,
            "an execute chain must end with `run <command>`",
        ),
        (ExecuteError::EmptyRun, "`run` needs a command after it"),
        (
            ExecuteError::UnsupportedModifier("as".to_owned()),
            "the `as` modifier is not supported by this build",
        ),
        (
            ExecuteError::UnsupportedCondition("x".to_owned()),
            "the `x` condition is not supported by this build",
        ),
        (
            ExecuteError::MissingArgument {
                modifier: "at".to_owned(),
            },
            "`at` is missing an argument",
        ),
        (
            ExecuteError::BadValue {
                modifier: "at".to_owned(),
                reason: "bad".to_owned(),
            },
            "`at`: bad",
        ),
        (
            ExecuteError::TooLong { count: 9, limit: 8 },
            "an execute chain has at most 8 modifiers, found 9",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
