//! Argument tests, weighted towards hostile input (P07-02, P07-15).

use super::{
    Argument, ArgumentKind, ArgumentValue, Coordinate, ParseError, ValueRange, parse_value,
    split_root, tokenize,
};
use mc_core::ids::ResourceId;

fn word(name: &'static str) -> Argument {
    Argument::word(name)
}

#[test]
fn tokenizing_splits_on_whitespace() {
    assert_eq!(tokenize("say hello").expect("tokens"), vec!["say", "hello"]);
    assert_eq!(tokenize("  a   b  ").expect("tokens"), vec!["a", "b"]);
    assert_eq!(tokenize("single").expect("tokens"), vec!["single"]);
    // Tabs and newlines are whitespace too; a client can send either.
    assert_eq!(tokenize("a\tb\nc").expect("tokens"), vec!["a", "b", "c"]);
}

#[test]
fn quotes_group_and_are_removed() {
    let tokens = tokenize(r#"say "hello world""#).expect("tokens");
    assert_eq!(tokens, vec!["say", "hello world"]);
    // A quote in the middle of a word joins the halves.
    assert_eq!(tokenize(r#"a"b c"d"#).expect("tokens"), vec!["ab cd"]);
    // An empty quoted string is a real, empty token — different from no token.
    assert_eq!(tokenize(r#"say """#).expect("tokens"), vec!["say", ""]);
}

#[test]
fn backslash_escapes_the_next_character() {
    // `r#"…"#` because a raw string cannot contain a quote: the input is the nine
    // characters `say "quoted"` after the escapes are processed.
    assert_eq!(
        tokenize(r#"say \"quoted\""#).expect("tokens"),
        vec!["say", "\"quoted\""]
    );
    assert_eq!(tokenize(r"a\ b").expect("tokens"), vec!["a b"]);
    // A trailing backslash is kept as itself rather than dropping the character.
    assert_eq!(tokenize(r"a\").expect("tokens"), vec!["a\\"]);
}

#[test]
fn unterminated_quotes_and_empty_input_are_refused() {
    assert_eq!(
        tokenize(r#"say "unclosed"#),
        Err(ParseError::UnterminatedQuote)
    );
    assert_eq!(tokenize(""), Err(ParseError::Empty));
    assert_eq!(tokenize("   "), Err(ParseError::Empty));
    // A lone quote is unterminated, not an empty token.
    assert_eq!(tokenize("\""), Err(ParseError::UnterminatedQuote));
}

#[test]
fn an_over_long_command_is_refused_before_tokenizing() {
    let long = "a".repeat(crate::MAX_COMMAND_CHARS + 1);
    match tokenize(&long) {
        Err(ParseError::TooLong { length, limit }) => {
            assert_eq!(length, crate::MAX_COMMAND_CHARS + 1);
            assert_eq!(limit, crate::MAX_COMMAND_CHARS);
        }
        other => panic!("expected TooLong, got {other:?}"),
    }
    // Exactly at the limit is accepted.
    let at_limit = "a".repeat(crate::MAX_COMMAND_CHARS);
    assert!(tokenize(&at_limit).is_ok());
}

#[test]
fn multi_byte_characters_are_counted_as_characters_not_bytes() {
    // A 20 000-character string of 3-byte characters is 60 000 bytes; counting bytes
    // would refuse it for the wrong reason and, worse, a byte-indexed truncation would
    // panic. Both are avoided by working in `Vec<char>`.
    let text = "あ".repeat(3_000);
    assert_eq!(text.len(), 9_000, "bytes");
    assert_eq!(text.chars().count(), 3_000);
    assert!(
        tokenize(&text).is_ok(),
        "3 000 characters is under the limit"
    );
}

#[test]
fn integers_are_bounded_and_a_fraction_is_refused() {
    let argument = Argument::ranged("count", 1, 10);
    assert_eq!(
        parse_value(&argument, "5").expect("valid"),
        ArgumentValue::Integer(5)
    );
    assert_eq!(
        parse_value(&argument, "1").expect("valid"),
        ArgumentValue::Integer(1)
    );
    assert_eq!(
        parse_value(&argument, "10").expect("valid"),
        ArgumentValue::Integer(10)
    );

    for bad in ["0", "11", "-1", "999"] {
        match parse_value(&argument, bad) {
            Err(ParseError::OutOfRange { min, max, .. }) => {
                assert_eq!((min, max), (1, 10), "{bad}");
            }
            other => panic!("expected OutOfRange for {bad}, got {other:?}"),
        }
    }
    // A fraction is not an integer, and must not be truncated.
    assert!(matches!(
        parse_value(&argument, "5.5"),
        Err(ParseError::Invalid { .. })
    ));
    // Nor is a non-number.
    assert!(parse_value(&argument, "abc").is_err());
}

#[test]
fn the_integer_range_is_checked_before_narrowing() {
    // A value inside `i64` but outside `i32` must be refused, because an `Integer`
    // argument is an `i32` on the wire.
    let argument = Argument::required("n", ArgumentKind::Long(ValueRange::new(i64::MIN, i64::MAX)));
    let value = i64::from(i32::MAX) + 1;
    assert_eq!(
        parse_value(&argument, &value.to_string()).expect("a long accepts it"),
        ArgumentValue::Integer(value)
    );

    let narrow = Argument::integer("n");
    match parse_value(&narrow, &value.to_string()) {
        Err(ParseError::OutOfRange { min, max, .. }) => {
            assert_eq!(min, i64::from(i32::MIN));
            assert_eq!(max, i64::from(i32::MAX));
        }
        other => panic!("expected OutOfRange, got {other:?}"),
    }
    // The extremes are accepted.
    assert!(parse_value(&narrow, &i32::MIN.to_string()).is_ok());
    assert!(parse_value(&narrow, &i32::MAX.to_string()).is_ok());
    // And beyond them refused rather than wrapped.
    assert!(parse_value(&narrow, &(i64::from(i32::MIN) - 1).to_string()).is_err());
}

#[test]
fn doubles_refuse_non_finite_values() {
    let argument = Argument::required(
        "x",
        ArgumentKind::Double {
            min: -100.0,
            max: 100.0,
        },
    );
    assert_eq!(
        parse_value(&argument, "1.5").expect("valid"),
        ArgumentValue::Double(1.5)
    );
    // `f64::parse` accepts these spellings, so the finiteness check is what stops them.
    for bad in ["NaN", "nan", "inf", "-inf", "infinity"] {
        assert!(
            parse_value(&argument, bad).is_err(),
            "{bad} must not be a legal coordinate"
        );
    }
    // Out of range.
    assert!(parse_value(&argument, "101").is_err());
    assert!(parse_value(&argument, "-101").is_err());
    // A huge but finite exponent is finite and merely out of range.
    assert!(parse_value(&argument, "1e308").is_err());
}

#[test]
fn booleans_accept_only_the_two_literals() {
    let argument = Argument::required("flag", ArgumentKind::Bool);
    assert_eq!(
        parse_value(&argument, "true").expect("valid"),
        ArgumentValue::Bool(true)
    );
    assert_eq!(
        parse_value(&argument, "false").expect("valid"),
        ArgumentValue::Bool(false)
    );
    for bad in ["True", "TRUE", "1", "0", "yes"] {
        assert!(
            parse_value(&argument, bad).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn resource_ids_accept_a_bare_name_and_a_namespaced_one() {
    let argument = Argument::required("block", ArgumentKind::Resource);
    let bare = parse_value(&argument, "stone").expect("valid");
    let namespaced = parse_value(&argument, "minecraft:stone").expect("valid");
    // Both forms are legal, and both must produce a usable id.
    assert!(bare.as_resource().is_some());
    assert!(namespaced.as_resource().is_some());
    assert_eq!(
        namespaced.as_resource().map(ResourceId::namespace),
        Some("minecraft")
    );
    for bad in ["", ":", "minecraft:", ":stone"] {
        assert!(
            parse_value(&argument, bad).is_err(),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn block_positions_accept_absolute_and_relative_forms() {
    let argument = Argument::required("pos", ArgumentKind::BlockPos);
    let absolute = parse_value(&argument, "1 2 3").expect("valid");
    assert_eq!(
        absolute,
        ArgumentValue::BlockPos {
            x: Coordinate::absolute(1),
            y: Coordinate::absolute(2),
            z: Coordinate::absolute(3)
        }
    );
    let relative = parse_value(&argument, "~ ~ ~").expect("valid");
    assert_eq!(
        relative,
        ArgumentValue::BlockPos {
            x: Coordinate::relative(0),
            y: Coordinate::relative(0),
            z: Coordinate::relative(0)
        }
    );
    let offsets = parse_value(&argument, "~5 ~-3 ~").expect("valid");
    assert_eq!(
        offsets,
        ArgumentValue::BlockPos {
            x: Coordinate::relative(5),
            y: Coordinate::relative(-3),
            z: Coordinate::relative(0)
        }
    );

    // **The assertion this type exists for.** `12` and `~12` used to parse to the same value, so a consumer
    // that read one as absolute — which `/tp` did — threw the source's position away for every offset form.
    // They must differ as values, and they must resolve to different places from a source away from the origin.
    let absolute_twelve = parse_value(&argument, "12 0 0").expect("valid");
    let relative_twelve = parse_value(&argument, "~12 0 0").expect("valid");
    assert_ne!(
        absolute_twelve, relative_twelve,
        "an absolute coordinate and a relative one must not be the same value"
    );
    let (ArgumentValue::BlockPos { x: abs, .. }, ArgumentValue::BlockPos { x: rel, .. }) =
        (&absolute_twelve, &relative_twelve)
    else {
        panic!("both are block positions");
    };
    assert_eq!(
        abs.resolve(100),
        12,
        "an absolute coordinate ignores the source"
    );
    assert_eq!(rel.resolve(100), 112, "a relative one is measured from it");
    // Wrong number of axes.
    for bad in ["1 2", "1 2 3 4", "", "1 2 x", "~x 0 0"] {
        assert!(
            parse_value(&argument, bad).is_err(),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn empty_words_are_refused() {
    for kind in [
        ArgumentKind::Word,
        ArgumentKind::PlayerName,
        ArgumentKind::GreedyString,
    ] {
        let argument = Argument::required("text", kind.clone());
        assert!(
            parse_value(&argument, "").is_err(),
            "{kind:?} must refuse an empty token"
        );
        assert!(parse_value(&argument, "x").is_ok());
    }
}

#[test]
fn error_messages_name_the_argument_and_the_problem() {
    let argument = Argument::ranged("count", 1, 10);
    let error = parse_value(&argument, "50").expect_err("out of range");
    let text = error.to_string();
    assert!(text.contains("count"), "{text}");
    assert!(text.contains("50"), "{text}");
    assert!(text.contains('1') && text.contains("10"), "{text}");

    let error = parse_value(&word("name"), "").expect_err("empty");
    // Every error implements Display and Error, so a caller can log or chain it.
    let _: &dyn std::error::Error = &error;
    assert!(!error.to_string().is_empty());
}

#[test]
fn a_long_found_value_is_truncated_on_a_character_boundary() {
    // A 10 000-character multi-byte token would panic a byte-indexed truncation.
    let argument = Argument::ranged("count", 1, 10);
    let long = "あ".repeat(1_000);
    let error = parse_value(&argument, &long).expect_err("not an integer");
    if let ParseError::Invalid { found, .. } = error {
        assert!(
            found.chars().count() <= 33,
            "{} chars",
            found.chars().count()
        );
        assert!(found.ends_with('…'));
    } else {
        panic!("expected Invalid, got {error:?}");
    }
}

#[test]
fn split_root_handles_the_optional_slash_and_absent_arguments() {
    assert_eq!(split_root("/say hi"), ("say", "hi"));
    assert_eq!(split_root("say hi"), ("say", "hi"));
    assert_eq!(split_root("say"), ("say", ""));
    assert_eq!(split_root("/say"), ("say", ""));
    assert_eq!(split_root("  /say   hi  "), ("say", "hi  "));
    assert_eq!(split_root(""), ("", ""));
}

#[test]
fn argument_constructors_set_the_expected_shape() {
    let required = Argument::integer("n");
    assert!(!required.optional);
    assert_eq!(required.kind.expected(), "an integer");

    let optional = Argument::optional("n", ArgumentKind::Word);
    assert!(optional.optional);

    let greedy = Argument::greedy("message");
    assert!(greedy.kind.is_greedy());
    assert_eq!(greedy.kind.expected(), "text");

    // Every kind describes itself, so no error message can be empty.
    let kinds = [
        ArgumentKind::Word,
        ArgumentKind::GreedyString,
        ArgumentKind::Integer(ValueRange::i32_range()),
        ArgumentKind::Long(ValueRange::new(0, 1)),
        ArgumentKind::Double { min: 0.0, max: 1.0 },
        ArgumentKind::Bool,
        ArgumentKind::Resource,
        ArgumentKind::BlockPos,
        ArgumentKind::PlayerName,
    ];
    for kind in &kinds {
        assert!(!kind.expected().is_empty(), "{kind:?}");
        assert!(!kind.is_greedy() || *kind == ArgumentKind::GreedyString);
    }
}

#[test]
fn value_ranges_validate_and_test_membership() {
    let range = ValueRange::new(1, 10);
    assert!(range.is_valid());
    assert!(range.contains(1) && range.contains(10));
    assert!(!range.contains(0) && !range.contains(11));

    assert!(!ValueRange::new(10, 1).is_valid());
    assert!(
        ValueRange::new(5, 5).is_valid(),
        "a single value is a range"
    );

    let full = ValueRange::i32_range();
    assert!(full.contains(i64::from(i32::MIN)));
    assert!(full.contains(i64::from(i32::MAX)));
    assert!(!full.contains(i64::from(i32::MAX) + 1));
}

#[test]
fn parse_error_messages_are_stable() {
    // P15-06: player-visible strings; the thiserror migration must not reword them.
    let cases = [
        (
            ParseError::MissingArgument { name: "count" },
            "expected a value for <count>",
        ),
        (
            ParseError::Invalid {
                name: "n",
                expected: "an integer",
                found: "abc".to_owned(),
            },
            "<n> expects an integer, got \"abc\"",
        ),
        (
            ParseError::OutOfRange {
                name: "n",
                value: 99,
                min: 0,
                max: 10,
            },
            "<n> must be between 0 and 10, got 99",
        ),
        (ParseError::UnterminatedQuote, "unterminated quoted string"),
        (
            ParseError::TooLong {
                length: 5000,
                limit: 256,
            },
            "command is 5000 characters, limit is 256",
        ),
        (ParseError::Empty, "empty command"),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
