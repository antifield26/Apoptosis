//! Unit tests for [`super`] (P07-08).
//!
//! There is no vanilla data behind this module — the 26.1.2 jar ships **zero**
//! `.mcfunction` files — so these tests *are* the evidence for the parser. The formats they
//! encode are the documented ones, and the module documentation says so rather than implying
//! they were checked against a pack.

use super::{
    FunctionError, FunctionFile, FunctionLimits, FunctionLoadReport, FunctionRegistry,
    SUGGESTED_MAX_RECURSION_DEPTH, is_function_call, parse_function, read_function_file,
};
use mc_core::ids::ResourceId;
use std::path::Path;

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid id")
}

fn parse(text: &str) -> FunctionFile {
    parse_function(
        text,
        Path::new("test.mcfunction"),
        id("minecraft:test"),
        FunctionLimits::DEFAULT,
    )
    .expect("parses")
}

#[test]
fn commands_are_kept_in_file_order() {
    let file = parse("say one\nsay two\nsay three\n");
    assert_eq!(file.commands, vec!["say one", "say two", "say three"]);
    assert_eq!(file.len(), 3);
    assert_eq!(file.command_list().len(), 3);
    assert!(!file.is_empty());
    assert_eq!(file.total_chars(), 7 + 7 + 9);
}

#[test]
fn comments_and_blank_lines_are_skipped() {
    let file = parse(
        "# a leading comment\n\
         \n\
         say hello\n\
         \n\
         # another\n\
         say goodbye\n",
    );
    assert_eq!(file.commands, vec!["say hello", "say goodbye"]);
}

#[test]
fn an_indented_comment_is_still_a_comment() {
    // The `#` test is on the *trimmed* line: the dispatcher strips leading whitespace before
    // deciding, so `"   # x"` is a comment and not a command called `#`.
    let file = parse("   # indented\n\t# tabbed\nsay ok\n");
    assert_eq!(file.commands, vec!["say ok"]);
}

#[test]
fn a_trailing_hash_is_not_a_comment() {
    // Only a *leading* `#` comments a line; a `#` inside a command is part of it, which is how
    // `execute if block ... #minecraft:x` style selectors survive.
    let file = parse("say a#b\n");
    assert_eq!(file.commands, vec!["say a#b"]);
}

#[test]
fn surrounding_whitespace_is_stripped_and_carriage_returns_are_dropped() {
    let file = parse("say a\r\n\t say b \t\r\n");
    assert_eq!(file.commands, vec!["say a", "say b"]);
}

#[test]
fn a_comment_only_file_is_empty_and_that_is_not_an_error() {
    let file = parse("# nothing here\n\n# still nothing\n");
    assert!(file.is_empty());
    assert_eq!(file.len(), 0);
}

#[test]
fn an_empty_file_parses() {
    let file = parse("");
    assert!(file.is_empty());
}

#[test]
fn macros_are_flagged_but_not_expanded() {
    // The arguments do not exist at load time, so the line is preserved verbatim and flagged:
    // a dispatcher that cannot expand macros can refuse *before* running half a function.
    let file = parse("$say hello $(name)\nsay plain\n");
    assert!(file.has_macros);
    assert_eq!(
        file.commands,
        vec!["$say hello $(name)", "say plain"],
        "the macro line is preserved, not rewritten"
    );

    let plain = parse("say plain\n");
    assert!(!plain.has_macros);
}

#[test]
fn the_line_limit_refuses_a_function_that_is_too_long() {
    // "A function with a million lines must be refused": the cheapest way to say that is a
    // small limit and a file that trips it.
    let limits = FunctionLimits::with_max_lines(4);
    let text = "say 1\nsay 2\nsay 3\nsay 4\nsay 5\n";
    let error = parse_function(
        text,
        Path::new("big.mcfunction"),
        id("minecraft:big"),
        limits,
    )
    .expect_err("must refuse");
    assert!(
        matches!(error, FunctionError::TooManyLines { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("big.mcfunction"), "{error}");
    assert_eq!(error.path(), Path::new("big.mcfunction"));
    // Exactly at the limit is fine.
    let text = "say 1\nsay 2\nsay 3\nsay 4\n";
    assert!(parse_function(text, Path::new("ok.mcfunction"), id("minecraft:ok"), limits).is_ok());
}

#[test]
fn the_line_limit_counts_comments_and_blanks() {
    // Otherwise a file of a million comment lines would slip past a "commands" bound and
    // still be a million lines to read.
    let limits = FunctionLimits::with_max_lines(3);
    let text = "# one\n# two\n# three\n# four\n";
    let error = parse_function(text, Path::new("c.mcfunction"), id("minecraft:c"), limits)
        .expect_err("must refuse");
    assert!(
        matches!(error, FunctionError::TooManyLines { .. }),
        "{error}"
    );
}

#[test]
fn a_single_oversized_line_is_refused() {
    let limits = FunctionLimits {
        max_line_bytes: 8,
        ..FunctionLimits::DEFAULT
    };
    let error = parse_function(
        "say averylongcommand\n",
        Path::new("long.mcfunction"),
        id("minecraft:long"),
        limits,
    )
    .expect_err("must refuse");
    match error {
        FunctionError::LineTooLong { line, bytes, .. } => {
            assert_eq!(line, 1);
            assert_eq!(bytes, "say averylongcommand".len());
            assert_eq!(bytes, 20);
        }
        other => panic!("expected LineTooLong, got {other}"),
    }
}

#[test]
fn a_file_over_the_byte_limit_is_refused_before_being_read() {
    // A real file, so the metadata path is exercised rather than mocked.
    let dir = mc_test_support::fixtures::TempDir::new("function-limits");
    let file = dir.path().join("big.mcfunction");
    std::fs::write(&file, "say hello\n").expect("write");
    assert!(read_function_file(&file, id("minecraft:big"), FunctionLimits::DEFAULT).is_ok());
    let error = read_function_file(
        &file,
        id("minecraft:big"),
        FunctionLimits::with_max_bytes(1),
    )
    .expect_err("must refuse");
    assert!(matches!(error, FunctionError::TooLarge { .. }), "{error}");
    assert!(error.to_string().contains("exceeds"), "{error}");
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn a_missing_file_is_an_io_error_naming_it() {
    let error = read_function_file(
        Path::new("does-not-exist.mcfunction"),
        id("minecraft:x"),
        FunctionLimits::DEFAULT,
    )
    .expect_err("must fail");
    assert!(matches!(error, FunctionError::Io { .. }), "{error}");
    assert!(error.to_string().contains("does-not-exist"), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn invalid_utf8_is_refused_rather_than_lossily_decoded() {
    let dir = mc_test_support::fixtures::TempDir::new("function-utf8");
    let file = dir.path().join("bad.mcfunction");
    std::fs::write(&file, [0x73, 0x61, 0x79, 0x20, 0xff, 0xfe]).expect("write");
    let error = read_function_file(&file, id("minecraft:bad"), FunctionLimits::DEFAULT)
        .expect_err("must refuse");
    assert!(matches!(error, FunctionError::NotUtf8 { .. }), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn a_real_directory_loads_and_accounts_for_every_file() {
    let dir = mc_test_support::fixtures::TempDir::new("function-load");
    let base = dir.path().join("function");
    std::fs::create_dir_all(base.join("sub")).expect("mkdir");
    std::fs::write(base.join("a.mcfunction"), "say a\n").expect("write");
    std::fs::write(base.join("sub/b.mcfunction"), "say b\n").expect("write");
    // A file with the wrong extension is not part of the directory's contents.
    std::fs::write(base.join("c.txt"), "not a function\n").expect("write");

    let mut report = FunctionLoadReport::default();
    let registry = FunctionRegistry::load_directory(
        dir.path(),
        "minecraft",
        FunctionLimits::DEFAULT,
        &mut report,
    );
    assert_eq!(report.files, 2, "only .mcfunction files count");
    assert_eq!(report.loaded, 2);
    assert!(report.is_fully_accounted());
    assert!(report.is_clean(), "{:?}", report.skipped);
    assert_eq!(report.commands, 2);
    assert_eq!(registry.len(), 2);
    let names: Vec<String> = registry.names().map(ToString::to_string).collect();
    assert_eq!(
        names,
        vec!["minecraft:a".to_owned(), "minecraft:sub/b".to_owned()],
        "names are ascending and subdirectories become path segments"
    );
    assert!(registry.by_name(&id("minecraft:sub/b")).is_some());
}

#[test]
fn a_missing_directory_is_not_an_error() {
    // Most packs do not have every directory, so "no function/" must load as nothing.
    let dir = mc_test_support::fixtures::TempDir::new("function-missing");
    let mut report = FunctionLoadReport::default();
    let registry = FunctionRegistry::load_directory(
        dir.path(),
        "minecraft",
        FunctionLimits::DEFAULT,
        &mut report,
    );
    assert!(registry.is_empty());
    assert_eq!(report.files, 0);
    assert!(report.is_clean());
    assert!(report.is_fully_accounted());
}

#[test]
fn an_oversized_file_is_skipped_and_counted_not_ignored() {
    let dir = mc_test_support::fixtures::TempDir::new("function-skip");
    let base = dir.path().join("function");
    std::fs::create_dir_all(&base).expect("mkdir");
    std::fs::write(base.join("big.mcfunction"), "say a\nsay b\nsay c\n").expect("write");
    let limits = FunctionLimits::with_max_lines(1);
    let mut report = FunctionLoadReport::default();
    let registry = FunctionRegistry::load_directory(dir.path(), "minecraft", limits, &mut report);
    assert!(registry.is_empty());
    assert_eq!(report.files, 1);
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped.len(), 1);
    assert!(
        report.is_fully_accounted(),
        "one file, one skip: {report:?}"
    );
    assert!(!report.is_clean());
}

#[test]
fn a_later_function_with_the_same_name_overrides() {
    let dir = mc_test_support::fixtures::TempDir::new("function-override");
    let base = dir.path().join("function");
    std::fs::create_dir_all(&base).expect("mkdir");
    std::fs::write(base.join("a.mcfunction"), "say first\n").expect("write");
    let mut report = FunctionLoadReport::default();
    let mut registry = FunctionRegistry::load_directory(
        dir.path(),
        "minecraft",
        FunctionLimits::DEFAULT,
        &mut report,
    );
    registry.insert(parse("say second\n"));
    assert_eq!(registry.len(), 2, "`minecraft:test` is a different name");
    // The same name, from a later pack: it replaces rather than appends.
    let mut replacement = parse("say second\n");
    replacement.name = id("minecraft:a");
    registry.insert(replacement);
    assert_eq!(
        registry.len(),
        2,
        "the later file overrides the earlier one"
    );
    let function = registry.by_name(&id("minecraft:a")).expect("present");
    assert_eq!(function.commands, vec!["say second"]);
}

#[test]
fn function_call_detection_handles_the_documented_forms() {
    for call in [
        "function minecraft:foo",
        "/function minecraft:foo",
        "FUNCTION minecraft:foo",
        "  /function  minecraft:foo  ",
        "function minecraft:foo with entity @s",
        "function",
    ] {
        assert!(is_function_call(call), "{call:?} should be a call");
    }
    for not_call in [
        "say function",
        "functionx minecraft:foo",
        "# function x",
        "execute run functionx",
    ] {
        assert!(!is_function_call(not_call), "{not_call:?} is not a call");
    }
}

#[test]
fn the_registry_finds_callers_and_cycles() {
    let mut registry = FunctionRegistry::new();
    let mut a = parse("function minecraft:b\nsay a\n");
    a.name = id("minecraft:a");
    let mut b = parse("function minecraft:c\n");
    b.name = id("minecraft:b");
    let mut c = parse("say c\n");
    c.name = id("minecraft:c");
    registry.insert(a);
    registry.insert(b);
    registry.insert(c);

    let callers = registry.calling(&id("minecraft:b"));
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, id("minecraft:a"));
    assert!(registry.calling(&id("minecraft:a")).is_empty());
    assert!(!registry.has_recursion(&id("minecraft:a")));
    assert_eq!(registry.functions_calling_functions().len(), 2);

    // Make it recursive: c calls a again.
    let mut c = parse("function minecraft:a\n");
    c.name = id("minecraft:c");
    registry.insert(c);
    assert!(
        registry.has_recursion(&id("minecraft:a")),
        "a -> b -> c -> a is a cycle"
    );
    // The other direction is not a cycle from c's point of view, because the walk starts
    // there — but it is still reachable, so it is reported.
    assert!(registry.has_recursion(&id("minecraft:c")));
}

#[test]
fn a_namespaceless_call_target_resolves_within_the_calling_namespace() {
    // The command parser defaults a bare name to the caller's namespace, so the scan must too.
    let mut registry = FunctionRegistry::new();
    let mut a = parse("function plain\n");
    a.name = id("minecraft:a");
    let mut plain = parse("say plain\n");
    plain.name = id("minecraft:plain");
    registry.insert(a);
    registry.insert(plain);
    assert_eq!(registry.calling(&id("minecraft:plain")).len(), 1);
}

#[test]
fn the_command_budget_sums_every_command() {
    let mut registry = FunctionRegistry::new();
    let mut a = parse("say a\nsay b\n");
    a.name = id("minecraft:a");
    let mut b = parse("say c\n");
    b.name = id("minecraft:b");
    registry.insert(a);
    registry.insert(b);
    assert_eq!(registry.command_budget(), 3);
    assert_eq!(registry.functions().len(), 2);
    assert!(!registry.is_empty());
}

/// A bound of 64 must be reachable *and* cheap: a caller enforcing it does at most 64 nested
/// dispatches before refusing, which is bounded work rather than a hang.
const _: () = assert!(SUGGESTED_MAX_RECURSION_DEPTH > 1);

#[test]
fn the_suggested_recursion_bound_is_recorded_and_sane() {
    assert_eq!(SUGGESTED_MAX_RECURSION_DEPTH, 64);
}

#[test]
fn the_default_limits_are_bounded() {
    let limits = FunctionLimits::DEFAULT;
    assert_eq!(limits.max_bytes, 256 * 1024);
    assert_eq!(limits.max_lines, 65_536);
    assert_eq!(limits.max_line_bytes, 32_767);
    assert_eq!(FunctionLimits::default(), limits);
    assert_eq!(
        FunctionLimits::with_max_lines(3).max_bytes,
        limits.max_bytes
    );
    assert_eq!(
        FunctionLimits::with_max_bytes(3).max_lines,
        limits.max_lines
    );
}

#[test]
fn a_file_with_a_million_lines_is_refused_cheaply() {
    // The concrete threat: a function whose *line count* is the attack. The byte limit is
    // lifted here so this exercises the line bound specifically, and must trip at the point
    // the limit is crossed rather than after the whole thing is parsed.
    let mut text = String::with_capacity(2 * 1024 * 1024);
    for index in 0..200_000 {
        text.push_str("say ");
        text.push_str(&index.to_string());
        text.push('\n');
    }
    let limits = FunctionLimits {
        max_bytes: u64::MAX,
        ..FunctionLimits::DEFAULT
    };
    let error = parse_function(
        &text,
        Path::new("huge.mcfunction"),
        id("minecraft:huge"),
        limits,
    )
    .expect_err("must refuse");
    assert!(
        matches!(error, FunctionError::TooManyLines { .. }),
        "{error}"
    );

    // And with the byte limit in force it is refused before that, still naming the file.
    let error = parse_function(
        &text,
        Path::new("huge.mcfunction"),
        id("minecraft:huge"),
        FunctionLimits::DEFAULT,
    )
    .expect_err("must refuse");
    assert!(error.to_string().contains("huge.mcfunction"), "{error}");
}

#[test]
fn function_error_messages_are_stable() {
    // P15-06: operator-visible strings; the thiserror migration must not reword them.
    use std::path::PathBuf;
    let path = PathBuf::from("cmds/a.mcfunction");
    let io = |msg: &str| FunctionError::Io {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, msg.to_owned()),
    };
    let cases = [
        (io("nope"), "cmds/a.mcfunction: nope"),
        (
            FunctionError::TooLarge {
                path: path.clone(),
                bytes: 1500,
                limit: 1000,
            },
            "cmds/a.mcfunction: 1500 bytes exceeds the 1000-byte function limit",
        ),
        (
            FunctionError::TooManyLines {
                path: path.clone(),
                lines: 300,
                limit: 200,
            },
            "cmds/a.mcfunction: 300 lines exceeds the 200-line function limit",
        ),
        (
            FunctionError::LineTooLong {
                path: path.clone(),
                line: 7,
                bytes: 500,
                limit: 100,
            },
            "cmds/a.mcfunction:7: 500 bytes exceeds the 100-byte line limit",
        ),
        (
            FunctionError::NotUtf8 {
                path: path.clone(),
                source: String::from_utf8(vec![0xff]).expect_err("not UTF-8"),
            },
            "cmds/a.mcfunction: not UTF-8: invalid utf-8 sequence of 1 bytes from index 0",
        ),
        (
            FunctionError::BadName {
                path,
                reason: "bad".to_owned(),
            },
            "cmds/a.mcfunction: bad",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn function_io_errors_keep_their_source() {
    // P15-06: thiserror auto-sources the `source` field; pin the chain.
    use std::error::Error;
    use std::path::PathBuf;
    let error = FunctionError::Io {
        path: PathBuf::from("cmds/a.mcfunction"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
    };
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("nope")
    );
    assert!(
        FunctionError::BadName {
            path: PathBuf::from("x"),
            reason: String::new(),
        }
        .source()
        .is_none()
    );
}
