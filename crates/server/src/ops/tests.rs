//! `ops.json` tests (P07-04).

use super::{LIMITS, OperatorList, ops_directory};
use mc_command::PermissionLevel;
use std::path::Path;

fn parse(text: &str) -> Result<OperatorList, String> {
    OperatorList::parse(text, Path::new("ops.json")).map_err(|error| error.to_string())
}

/// Vanilla's own file shape, with two entries.
const VANILLA_SHAPE: &str = r#"[
  {
    "uuid": "069a79f4-44e9-4726-a5be-fca90e38aaf5",
    "name": "Notch",
    "level": 4,
    "bypassesPlayerLimit": true
  },
  {
    "uuid": "853c80ef-3c37-49fd-aa49-938b674adae6",
    "name": "jeb_",
    "level": 2,
    "bypassesPlayerLimit": false
  }
]"#;

#[test]
fn a_vanilla_shaped_file_loads_and_grants_the_stated_levels() {
    let list = parse(VANILLA_SHAPE).expect("parses");
    assert_eq!(list.len(), 2);
    assert!(list.is_operator("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
    assert_eq!(
        list.level_for("069a79f4-44e9-4726-a5be-fca90e38aaf5"),
        PermissionLevel::Console,
        "level 4 is the console tier, which is what Vanilla's ops.json means by it"
    );
    assert_eq!(
        list.level_for("853c80ef-3c37-49fd-aa49-938b674adae6"),
        PermissionLevel::Operator
    );
    assert_eq!(list.bypass_count(), 1);

    let entry = list
        .get("069a79f4-44e9-4726-a5be-fca90e38aaf5")
        .expect("entry");
    assert_eq!(entry.name, "Notch", "the name is carried for messages");
    assert!(entry.bypasses_player_limit);
}

#[test]
fn a_player_who_is_not_listed_holds_no_authority() {
    let list = parse(VANILLA_SHAPE).expect("parses");
    assert!(!list.is_operator("00000000-0000-0000-0000-000000000000"));
    assert_eq!(
        list.level_for("00000000-0000-0000-0000-000000000000"),
        PermissionLevel::All,
        "an unlisted player must be level 0, not an error and not an operator"
    );
    assert!(list.get("nobody").is_none());
}

#[test]
fn a_missing_file_is_an_empty_list_rather_than_an_error() {
    // A server with no operators is ordinary; Vanilla writes the file on first run, and
    // requiring it would mean the server cannot start until an operator exists.
    let dir = mc_test_support::fixtures::TempDir::new("ops-missing");
    let list = OperatorList::load(dir.path()).expect("a missing file is not an error");
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert!(!list.is_operator("anyone"));
    assert_eq!(list.level_for("anyone"), PermissionLevel::All);
}

#[test]
fn an_empty_array_is_an_empty_list() {
    let list = parse("[]").expect("parses");
    assert!(list.is_empty());
    assert_eq!(list.bypass_count(), 0);
    assert_eq!(list.operators().count(), 0);
}

#[test]
fn a_file_that_exists_and_is_malformed_is_refused_by_name() {
    // The property this module exists for: silently running with **no** operators because a
    // JSON comma was wrong would look like "permissions stopped working" with no cause
    // anywhere.
    for (text, expect_in_message) in [
        // The JSON boundary produces its own wording, which is better than mine: it names
        // the file *and* the position. What the test pins is that the message identifies the
        // file and says what was wrong, not a particular phrasing.
        ("{", "EOF while parsing"),
        ("", "EOF while parsing"),
        ("{}", "must be a JSON array"),
        ("null", "must be a JSON array"),
        ("42", "must be a JSON array"),
        ("[1, 2]", "not an object"),
        (r#"[{"name": "Nobody"}]"#, "no string `uuid`"),
        (r#"[{"uuid": ""}]"#, "empty `uuid`"),
        (r#"[{"uuid": "a"}]"#, "no integer `level`"),
        (r#"[{"uuid": "a", "level": "high"}]"#, "no integer `level`"),
        (r#"[{"uuid": "a", "level": 5}]"#, "unknown one is refused"),
        (r#"[{"uuid": "a", "level": -1}]"#, "not 0-4"),
        (r#"[{"uuid": "a", "level": 999}]"#, "not 0-4"),
    ] {
        match parse(text) {
            Ok(list) => panic!("{text:?} must be refused, got {} entries", list.len()),
            Err(message) => {
                assert!(
                    message.contains(expect_in_message),
                    "{text:?}: message {message:?} should mention {expect_in_message:?}"
                );
                assert!(message.contains("ops.json"), "{text:?}: {message}");
            }
        }
    }
}

#[test]
fn an_unknown_level_is_refused_rather_than_clamped() {
    // Clamping 5 down to 4 would grant *more* authority than the file asks for, and clamping
    // it to 0 would make a listed operator a silent no-op. Refusing is the only option that
    // cannot be wrong in a way nobody notices.
    for level in [5, 6, 10, 100, 255] {
        let text = format!(r#"[{{"uuid": "a", "level": {level}}}]"#);
        let error = parse(&text).expect_err(&format!("level {level} must be refused"));
        assert!(error.contains("refused rather than clamped"), "{error}");
    }
    // And every valid level is accepted.
    for level in 0..=4u8 {
        let text = format!(r#"[{{"uuid": "a", "level": {level}}}]"#);
        let list = parse(&text).unwrap_or_else(|e| panic!("level {level} must parse: {e}"));
        assert_eq!(
            list.level_for("a"),
            PermissionLevel::from_level(level).expect("a known level")
        );
    }
}

#[test]
fn a_repeated_uuid_is_refused_rather_than_resolved_by_order() {
    // The two entries could name different levels, in which case the authority granted would
    // depend on file order — which is not a property a permissions file should have.
    let text = r#"[
      {"uuid": "abc", "name": "One", "level": 4},
      {"uuid": "abc", "name": "Two", "level": 1}
    ]"#;
    let error = parse(text).expect_err("a repeated uuid must be refused");
    assert!(error.contains("repeats uuid"), "{error}");
    assert!(
        error.contains('4') && error.contains('1'),
        "both levels: {error}"
    );
}

#[test]
fn uuids_are_case_insensitive_and_trimmed() {
    // A uuid is conventionally lower-case but files in the wild are not consistent, and a
    // silent mismatch here is exactly the "my op stopped working" failure.
    let list = parse(r#"[{"uuid": "  069A79F4-44E9-4726-A5BE-FCA90E38AAF5  ", "level": 4}]"#)
        .expect("parses");
    assert!(list.is_operator("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
    assert!(list.is_operator("069A79F4-44E9-4726-A5BE-FCA90E38AAF5"));
    assert!(list.is_operator("  069a79f4-44e9-4726-a5be-fca90e38aaf5  "));
}

#[test]
fn a_stale_name_still_grants_because_the_uuid_is_the_identity() {
    // Names change; a uuid does not. Matching by name would let anyone take an operator's
    // identity by taking their name.
    let list = parse(r#"[{"uuid": "abc", "name": "OldName", "level": 3}]"#).expect("parses");
    assert!(list.is_operator("abc"));
    assert_eq!(list.level_for("abc"), PermissionLevel::Administrator);
    assert_eq!(list.get("abc").expect("entry").name, "OldName");
    // And a different uuid with the same name gets nothing.
    assert!(!list.is_operator("def"));
}

#[test]
fn a_name_is_optional_because_matching_does_not_use_it() {
    let list = parse(r#"[{"uuid": "abc", "level": 2}]"#).expect("parses");
    assert_eq!(list.get("abc").expect("entry").name, "");
    assert_eq!(list.level_for("abc"), PermissionLevel::Operator);
}

#[test]
fn a_missing_bypass_field_defaults_to_false() {
    let list = parse(r#"[{"uuid": "abc", "level": 4}]"#).expect("parses");
    assert!(!list.get("abc").expect("entry").bypasses_player_limit);
    assert_eq!(list.bypass_count(), 0);

    // And a non-boolean one is treated as absent rather than refused: it is an unenforced
    // field, so refusing the file over it would block a working operator list.
    let list =
        parse(r#"[{"uuid": "abc", "level": 4, "bypassesPlayerLimit": "yes"}]"#).expect("parses");
    assert!(!list.get("abc").expect("entry").bypasses_player_limit);
}

#[test]
fn operators_are_iterated_in_a_reproducible_order() {
    let list = parse(
        r#"[
          {"uuid": "zzz", "level": 1},
          {"uuid": "aaa", "level": 2},
          {"uuid": "mmm", "level": 3}
        ]"#,
    )
    .expect("parses");
    let order: Vec<&str> = list.operators().map(|o| o.uuid.as_str()).collect();
    assert_eq!(order, vec!["aaa", "mmm", "zzz"], "ascending by uuid");
}

#[test]
fn an_over_large_file_is_refused_before_it_is_read() {
    let dir = mc_test_support::fixtures::TempDir::new("ops-large");
    let path = dir.path().join(super::OPS_FILE_NAME);
    // One byte past the limit, so the boundary is where it claims to be.
    let blob = vec![b' '; usize::try_from(LIMITS.max_bytes).expect("fits") + 1];
    std::fs::write(&path, &blob).expect("write");
    let error = OperatorList::load(dir.path()).expect_err("must refuse");
    let message = error.to_string();
    assert!(message.contains("exceeds"), "{message}");
    assert!(
        message.contains(&LIMITS.max_bytes.to_string()),
        "the limit: {message}"
    );
}

#[test]
fn ops_lives_beside_the_world_not_inside_it() {
    // Vanilla puts `ops.json` next to `server.properties`, which is the directory
    // *containing* the world. Getting this wrong finds no operators and looks like
    // "permissions stopped working".
    assert_eq!(
        ops_directory(Path::new("/srv/minecraft/world")),
        Path::new("/srv/minecraft")
    );
    assert_eq!(
        ops_directory(Path::new("/srv/minecraft/world/save")),
        Path::new("/srv/minecraft/world")
    );
    // A world path with no parent falls back to the current directory rather than panicking.
    assert_eq!(ops_directory(Path::new("world")), Path::new("."));
}

#[test]
fn hostile_files_do_not_panic() {
    let cases = [
        "",
        " ",
        "[",
        "]",
        "[[]]",
        "[{}]",
        "[null]",
        r#"[{"uuid": null}]"#,
        r#"[{"uuid": 1}]"#,
        r#"[{"uuid": "a", "level": null}]"#,
        r#"[{"uuid": "a", "level": 1e400}]"#,
        r#"[{"uuid": "a", "level": 3.5}]"#,
        r#"[{"uuid": "a", "name": null, "level": 4}]"#,
        r#"[{"uuid": "\u0000", "level": 4}]"#,
        r#"[{"uuid": "a", "level": 4}, "trailing"]"#,
        "[[[[[[[[[[",
        &format!(r#"[{{"uuid": "{}", "level": 4}}]"#, "a".repeat(100_000)),
    ];
    for case in cases {
        // The only requirement is an outcome rather than an unwind; a refusal is fine.
        let _ = parse(case);
    }
}

#[test]
fn a_file_that_is_a_directory_is_an_error_not_a_missing_file() {
    let dir = mc_test_support::fixtures::TempDir::new("ops-dir");
    std::fs::create_dir_all(dir.path().join(super::OPS_FILE_NAME)).expect("mkdir");
    // Reading a directory as a string fails; the point is that it is reported rather than
    // treated as "no operators".
    assert!(OperatorList::load(dir.path()).is_err());
}

#[test]
fn a_realistic_file_round_trips_through_load() {
    let dir = mc_test_support::fixtures::TempDir::new("ops-load");
    std::fs::write(dir.path().join(super::OPS_FILE_NAME), VANILLA_SHAPE).expect("write");
    let list = OperatorList::load(dir.path()).expect("loads");
    assert_eq!(list.len(), 2);
    assert_eq!(
        list.level_for("069a79f4-44e9-4726-a5be-fca90e38aaf5"),
        PermissionLevel::Console
    );
    assert_eq!(list.bypass_count(), 1);
}
