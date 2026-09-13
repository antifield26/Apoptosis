//! Dispatcher tests: resolution order, greedy arguments, permissions, hostile input
//! (P07-02, P07-04, P07-15).

use super::{CommandOutcome, Dispatcher, Suggestion};
use crate::argument::Coordinate;
use crate::argument::{Argument, ArgumentKind, ArgumentValue};
use crate::source::{CommandSource, PermissionLevel, SourcePosition};
use crate::tree::{Command, CommandTree, TreeError};

/// `MAX_ARGUMENTS + 1` distinct argument names.
///
/// A duplicate name would be refused for a different reason and mask the arity check, so
/// the names must be distinct — and `Argument::name` is `&'static str`, which is why this
/// is a fixed table rather than generated.
const NAMES: [&str; crate::MAX_ARGUMENTS + 1] = [
    "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "a10", "a11", "a12", "a13", "a14",
    "a15", "a16", "a17", "a18", "a19", "a20", "a21", "a22", "a23", "a24", "a25", "a26", "a27",
    "a28", "a29", "a30", "a31", "a32", "a33", "a34", "a35", "a36", "a37", "a38", "a39", "a40",
    "a41", "a42", "a43", "a44", "a45", "a46", "a47", "a48", "a49", "a50", "a51", "a52", "a53",
    "a54", "a55", "a56", "a57", "a58", "a59", "a60", "a61", "a62", "a63", "a64",
];

/// A tree with the shapes the dispatcher has to handle.
fn tree() -> CommandTree {
    let mut tree = CommandTree::new();
    tree.insert(Command::new("help", "List commands"))
        .expect("help");
    tree.insert(
        Command::new("say", "Broadcast a message").with_argument(Argument::greedy("message")),
    )
    .expect("say");
    tree.insert(Command::new("list", "List players"))
        .expect("list");
    tree.insert(
        Command::new("tp", "Teleport")
            .with_argument(Argument::word("target"))
            .with_argument(Argument::required("pos", ArgumentKind::BlockPos)),
    )
    .expect("tp");
    tree.insert(
        Command::new("time", "Query or set the time").with_argument(Argument::optional(
            "value",
            ArgumentKind::Integer(crate::argument::ValueRange::new(0, 24_000)),
        )),
    )
    .expect("time");
    tree.insert(Command::new("op", "Grant operator").requiring(PermissionLevel::Operator))
        .expect("op");
    tree.insert(Command::new("stop", "Stop the server").requiring(PermissionLevel::Console))
        .expect("stop");
    tree
}

fn dispatcher() -> Dispatcher {
    Dispatcher::new(tree())
}

fn player() -> CommandSource {
    CommandSource::player("Alex", SourcePosition::new(1.0, 64.0, 2.0))
}

fn op() -> CommandSource {
    player().with_permission(PermissionLevel::Operator)
}

#[test]
fn a_bare_command_parses_with_no_arguments() {
    let outcome = dispatcher().parse("help", &player());
    let parsed = outcome.parsed().expect("parsed");
    assert_eq!(parsed.name, "help");
    assert!(parsed.arguments.is_empty());
    assert_eq!(parsed.raw_arguments, "");
}

#[test]
fn a_leading_slash_is_accepted_and_stripped() {
    // A client sends `chat_command` without the slash, but a player typing `/help` in
    // chat produces one, and both must work.
    for input in ["help", "/help"] {
        let outcome = dispatcher().parse(input, &player());
        assert!(outcome.is_parsed(), "{input}: {outcome:?}");
        assert_eq!(outcome.parsed().expect("parsed").name, "help");
    }
}

#[test]
fn a_greedy_argument_takes_every_remaining_token() {
    let outcome = dispatcher().parse("say hello there world", &player());
    let parsed = outcome.parsed().expect("parsed");
    assert_eq!(parsed.name, "say");
    assert_eq!(parsed.string(0), Some("hello there world"));
    assert_eq!(parsed.raw_arguments, "hello there world");
}

#[test]
fn a_greedy_argument_keeps_quotes_grouped() {
    let outcome = dispatcher().parse(r#"say "a b" c"#, &player());
    let parsed = outcome.parsed().expect("parsed");
    // Quotes group during tokenizing, then the tokens are rejoined by single spaces —
    // which is what Brigadier does, and why the raw text is kept separately.
    assert_eq!(parsed.string(0), Some("a b c"));
    assert_eq!(parsed.raw_arguments, r#""a b" c"#);
}

#[test]
fn a_block_pos_argument_consumes_three_tokens() {
    let outcome = dispatcher().parse("tp Alex 10 64 -5", &player());
    let parsed = outcome.parsed().expect("parsed");
    assert_eq!(parsed.string(0), Some("Alex"));
    assert_eq!(
        parsed.argument(1),
        Some(&ArgumentValue::BlockPos {
            x: Coordinate::absolute(10),
            y: Coordinate::absolute(64),
            z: Coordinate::absolute(-5)
        })
    );
    // A relative position survives as relative.
    let outcome = dispatcher().parse("tp Alex ~ ~2 ~-1", &player());
    let parsed = outcome.parsed().expect("parsed");
    assert_eq!(
        parsed.argument(1),
        Some(&ArgumentValue::BlockPos {
            x: Coordinate::relative(0),
            y: Coordinate::relative(2),
            z: Coordinate::relative(-1)
        })
    );
}

#[test]
fn a_block_pos_argument_with_too_few_tokens_is_a_missing_argument() {
    let outcome = dispatcher().parse("tp Alex 1 2", &player());
    assert!(matches!(
        outcome,
        CommandOutcome::BadArguments(crate::argument::ParseError::MissingArgument { .. })
    ));
}

#[test]
fn an_optional_argument_may_be_omitted_or_supplied() {
    for (input, expected) in [("time", None), ("time 1000", Some(1000))] {
        let outcome = dispatcher().parse(input, &player());
        let parsed = outcome
            .parsed()
            .unwrap_or_else(|| panic!("{input}: {outcome:?}"));
        assert_eq!(parsed.integer(0), expected, "{input}");
    }
}

#[test]
fn an_unknown_command_reports_near_matches() {
    match dispatcher().parse("hel", &player()) {
        CommandOutcome::UnknownCommand { name, suggestions } => {
            assert_eq!(name, "hel");
            assert_eq!(suggestions, vec!["help"], "a prefix match is offered");
        }
        other => panic!("expected UnknownCommand, got {other:?}"),
    }
    // Nothing similar: no suggestions, and still a clean refusal.
    match dispatcher().parse("zzz", &player()) {
        CommandOutcome::UnknownCommand { suggestions, .. } => assert!(suggestions.is_empty()),
        other => panic!("expected UnknownCommand, got {other:?}"),
    }
}

#[test]
fn permission_is_checked_before_the_arguments_are_parsed() {
    // A player may not use `op`. The message must not reveal `op`'s grammar — if the
    // argument check ran first, a bad argument would report an argument error and leak
    // the shape of a command the source cannot use.
    let outcome = dispatcher().parse("op", &player());
    match outcome {
        CommandOutcome::PermissionDenied {
            name,
            required,
            actual,
        } => {
            assert_eq!(name, "op");
            assert_eq!(required, PermissionLevel::Operator);
            assert_eq!(actual, PermissionLevel::All);
        }
        other => panic!("expected PermissionDenied, got {other:?}"),
    }

    // With an operator it parses.
    assert!(dispatcher().parse("op", &op()).is_parsed());
    // The console may use everything, including the console-only command.
    let console = CommandSource::console();
    assert!(dispatcher().parse("stop", &console).is_parsed());
    assert!(matches!(
        dispatcher().parse("stop", &op()),
        CommandOutcome::PermissionDenied { .. }
    ));
}

#[test]
fn too_many_arguments_are_refused_rather_than_ignored() {
    let outcome = dispatcher().parse("help extra", &player());
    match outcome {
        CommandOutcome::BadArguments(crate::argument::ParseError::Invalid { found, .. }) => {
            assert_eq!(found, "extra");
        }
        other => panic!("expected BadArguments, got {other:?}"),
    }
}

#[test]
fn a_missing_required_argument_names_it() {
    let mut tree = CommandTree::new();
    tree.insert(Command::new("give", "Give an item").with_argument(Argument::word("item")))
        .expect("give");
    let outcome = Dispatcher::new(tree).parse("give", &player());
    match outcome {
        CommandOutcome::BadArguments(crate::argument::ParseError::MissingArgument { name }) => {
            assert_eq!(name, "item");
        }
        other => panic!("expected MissingArgument, got {other:?}"),
    }
}

#[test]
fn hostile_input_is_refused_without_panicking() {
    let dispatcher = dispatcher();
    let source = player();
    let cases = [
        "",
        " ",
        "/",
        "\"",
        "say \"",
        "say \\",
        "tp a 1 2",
        "tp a 1 2 3 4",
        "time 99999999999999999999",
        "time -1",
        "time 1.5",
        "time NaN",
        "\u{0}",
        "\u{feff}help",
        "help\u{202e}",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    for case in cases {
        let outcome = dispatcher.parse(case, &source);
        // The only requirement is that it returns: no panic, no unwind. An outcome of
        // any kind is acceptable.
        assert!(
            !matches!(outcome, CommandOutcome::Parsed(_)) || !case.trim().is_empty(),
            "{case:?} produced a parsed command unexpectedly"
        );
    }
}

#[test]
fn a_very_long_command_is_refused_at_the_boundary() {
    let long = format!("say {}", "x".repeat(crate::MAX_COMMAND_CHARS));
    match dispatcher().parse(&long, &player()) {
        CommandOutcome::BadArguments(crate::argument::ParseError::TooLong { .. }) => {}
        other => panic!("expected TooLong, got {other:?}"),
    }
}

#[test]
fn many_arguments_do_not_grow_the_parse_vector_past_the_tree() {
    // The parsed vector's length is a property of the tree, not of the input: extra
    // tokens are refused, so a hostile client cannot make the vector grow.
    let outcome = dispatcher().parse("say a b c d e f g h", &player());
    let parsed = outcome.parsed().expect("parsed");
    assert_eq!(parsed.arguments.len(), 1, "one greedy argument");
}

#[test]
fn suggestions_are_roots_only_and_respect_permission() {
    let dispatcher = dispatcher();
    let suggestions = dispatcher.suggest("h", &player());
    assert_eq!(
        suggestions,
        vec![Suggestion {
            text: "help".to_owned(),
            complete: true
        }],
        "a command with no arguments completes at its name"
    );

    // `say` takes an argument, so it is not "complete".
    let suggestions = dispatcher.suggest("sa", &player());
    assert_eq!(suggestions.len(), 1);
    assert!(!suggestions[0].complete);

    // An operator-only command is not offered to a plain player.
    assert!(dispatcher.suggest("o", &player()).is_empty());
    assert_eq!(dispatcher.suggest("o", &op()).len(), 1);

    // The empty prefix offers everything the source may use.
    let all = dispatcher.suggest("", &player());
    assert_eq!(all.len(), 5, "help, list, say, time, tp");

    // Past the first word there are no suggestions, because argument candidates need
    // knowledge the tree does not have.
    assert!(dispatcher.suggest("say hello", &player()).is_empty());
    // A leading slash is accepted.
    assert_eq!(dispatcher.suggest("/h", &player()).len(), 1);
}

#[test]
fn the_tree_is_ordered_and_lookup_is_exact() {
    let tree = tree();
    let names: Vec<&str> = tree.names().collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "iteration order must be reproducible");
    assert_eq!(tree.get("help").map(|c| c.name), Some("help"));
    assert!(tree.get("Help").is_none(), "names are case sensitive");
    assert!(tree.get("").is_none());
    assert_eq!(tree.len(), 7);
    assert!(!tree.is_empty());
    assert!(CommandTree::new().is_empty());
}

#[test]
fn a_command_reports_its_usage_and_required_count() {
    let tree = tree();
    let tp = tree.get("tp").expect("tp");
    assert_eq!(tp.usage(), "/tp <target> <pos>");
    assert_eq!(tp.required_count(), 2);
    assert!(tp.accepts_count(2));
    assert!(!tp.accepts_count(1));
    assert!(!tp.accepts_count(3));

    let time = tree.get("time").expect("time");
    assert_eq!(time.usage(), "/time [value]");
    assert_eq!(time.required_count(), 0);
    assert!(time.accepts_count(0) && time.accepts_count(1));
    assert!(!time.accepts_count(2));

    let say = tree.get("say").expect("say");
    assert_eq!(say.greedy_index(), Some(0));
    assert_eq!(tree.get("help").expect("help").greedy_index(), None);
}

#[test]
fn a_tree_refuses_structures_that_cannot_be_parsed() {
    // An optional argument followed by a required one is ambiguous.
    let mut tree = CommandTree::new();
    let error = tree
        .insert(
            Command::new("bad", "x")
                .with_argument(Argument::optional("a", ArgumentKind::Word))
                .with_argument(Argument::word("b")),
        )
        .expect_err("must refuse");
    assert!(
        matches!(error, TreeError::OptionalBeforeRequired { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("bad"), "{error}");

    // A greedy argument that is not last can never have its successor supplied.
    let mut tree = CommandTree::new();
    let error = tree
        .insert(
            Command::new("bad", "x")
                .with_argument(Argument::greedy("text"))
                .with_argument(Argument::word("after")),
        )
        .expect_err("must refuse");
    assert!(
        matches!(error, TreeError::GreedyNotLast { .. }),
        "{error:?}"
    );

    // Duplicate names.
    let mut tree = CommandTree::new();
    tree.insert(Command::new("dup", "x")).expect("first");
    let error = tree
        .insert(Command::new("dup", "y"))
        .expect_err("must refuse");
    assert!(matches!(error, TreeError::DuplicateName(_)));

    // An empty name.
    let mut tree = CommandTree::new();
    assert!(matches!(
        tree.insert(Command::new("", "x")).expect_err("must refuse"),
        TreeError::EmptyName
    ));

    // A duplicate argument name.
    let mut tree = CommandTree::new();
    let error = tree
        .insert(
            Command::new("bad", "x")
                .with_argument(Argument::word("a"))
                .with_argument(Argument::word("a")),
        )
        .expect_err("must refuse");
    assert!(matches!(error, TreeError::DuplicateArgument { .. }));

    // An inverted range.
    let mut tree = CommandTree::new();
    let error = tree
        .insert(Command::new("bad", "x").with_argument(Argument::required(
            "n",
            ArgumentKind::Integer(crate::argument::ValueRange::new(10, 1)),
        )))
        .expect_err("must refuse");
    assert!(matches!(error, TreeError::InvertedRange { .. }));

    // A non-finite double range.
    let mut tree = CommandTree::new();
    let error = tree
        .insert(Command::new("bad", "x").with_argument(Argument::required(
            "x",
            ArgumentKind::Double {
                min: f64::NAN,
                max: 1.0,
            },
        )))
        .expect_err("must refuse");
    assert!(matches!(error, TreeError::InvalidDoubleRange { .. }));

    // Too many arguments: `MAX_ARGUMENTS + 1` **distinct** names, because a duplicate
    // name would be refused for a different reason and mask this check.
    let mut over_long = Command::new("many", "x");
    for name in NAMES {
        over_long = over_long.with_argument(Argument::word(name));
    }
    let mut tree = CommandTree::new();
    let error = tree.insert(over_long).expect_err("must refuse");
    assert!(
        matches!(error, TreeError::TooManyArguments { .. }),
        "{error:?}"
    );

    // And exactly at the limit is accepted, so the boundary is where it claims to be.
    let mut at_limit = Command::new("many", "y");
    for name in &NAMES[..crate::MAX_ARGUMENTS] {
        at_limit = at_limit.with_argument(Argument::word(name));
    }
    CommandTree::new()
        .insert(at_limit)
        .expect("MAX_ARGUMENTS arguments is legal");
}

#[test]
fn an_empty_tree_parses_nothing_and_suggests_nothing() {
    let dispatcher = Dispatcher::new(CommandTree::new());
    assert!(matches!(
        dispatcher.parse("help", &player()),
        CommandOutcome::UnknownCommand { .. }
    ));
    assert!(dispatcher.suggest("", &player()).is_empty());
    assert!(dispatcher.tree().is_empty());
}
