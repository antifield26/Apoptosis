//! The four access files are replaced atomically, and one damaged row does
//! not void the table (AUDIT-19 G-02/G-03).
//!
//! The per-module unit tests prove the file *shapes*; this suite pins the two
//! properties the audit found missing, for all three writers and all four
//! files (`whitelist.json`, `ops.json`, `banned-players.json`,
//! `banned-ips.json`):
//!
//! - **A failed write leaves the previous file live.** The writers used
//!   `std::fs::write` — truncate in place, no temp file — so a crash inside
//!   the write window left a *truncated* file, and a truncated
//!   `banned-players.json` read as "banning nobody", which let a permanently
//!   banned player join (the audit reproduced it live). The failure is forced
//!   the way `mc-persistence` forces its own: a directory occupies the
//!   staging path, so the temp file cannot be created *after* the point where
//!   the old code would already have destroyed the live file.
//! - **One damaged row does not void the table.** A row that cannot be parsed
//!   is skipped and logged, its siblings still load. A file whose *every* row
//!   is damaged is still an error naming the file: "lists nobody" and "nobody
//!   could be read" must not look alike, which is the difference between a
//!   loud failure and the audit's silent fail-open.
//!
//! Falsification shape: restore `std::fs::write` and the two atomic tests go
//! red (the blocked staging path no longer fails, and the live file is
//! replaced); restore `?` in the row loop and the sibling tests go red.

use mc_command::PermissionLevel;
use mc_core::error::ServerError;
use mc_persistence::save::temp_path;
use mc_server::bans::{BANNED_IPS_FILE_NAME, BANNED_PLAYERS_FILE_NAME, BanList, PlayerBan};
use mc_server::ops::{OPS_FILE_NAME, OperatorList};
use mc_server::whitelist::{WHITELIST_FILE_NAME, Whitelist};
use mc_test_support::fixtures::TempDir;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Two uuids whose order is known, so a loaded list can be compared whole.
const LISTED_UUID: &str = "069a79f4-44e9-4726-a5be-f4a7b64ac909";
const SECOND_UUID: &str = "11111111-1111-1111-1111-111111111111";

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text)
        .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
}

/// Occupy the staging path with a directory, so the temp file cannot be
/// created and the write fails with the live file still in place.
///
/// This is `mc-persistence`'s own way of forcing the failure
/// (`failed_write_leaves_the_previous_file_intact_and_reports`), reused here
/// so the two suites cannot drift about which write protocol is expected.
fn block_staging(path: &Path) {
    let staging = temp_path(path);
    std::fs::create_dir(&staging)
        .unwrap_or_else(|error| panic!("blocking {}: {error}", staging.display()));
}

fn unblock_staging(path: &Path) {
    std::fs::remove_dir(temp_path(path)).expect("the blocking directory is removed");
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

// ---------------------------------------------------------------- atomicity

#[test]
fn a_failed_write_leaves_the_previous_whitelist_intact() {
    let dir = TempDir::new("access-atomic-whitelist");
    let path = dir.path().join(WHITELIST_FILE_NAME);
    let mut whitelist = Whitelist::new();
    whitelist.insert(LISTED_UUID, "Listed");
    whitelist.save(dir.path()).expect("the first write lands");
    let before = read(&path);

    whitelist.insert(SECOND_UUID, "Second");
    block_staging(&path);
    let error = whitelist
        .save(dir.path())
        .expect_err("a blocked staging path must fail the write");
    assert!(
        matches!(error, ServerError::Operational(_)),
        "the caller must be told, so it can roll back: {error:?}"
    );
    assert_eq!(
        read(&path),
        before,
        "a failed write must leave the live file byte-identical"
    );
    let on_disk = Whitelist::load(dir.path()).expect("the previous file still loads");
    assert_eq!(on_disk.len(), 1, "the previous list survives the failure");
    assert!(on_disk.contains(LISTED_UUID));
    assert!(
        !on_disk.contains(SECOND_UUID),
        "the entry whose write failed is not in the file"
    );

    unblock_staging(&path);
    whitelist
        .save(dir.path())
        .expect("with the path clear the write lands");
    assert_eq!(
        Whitelist::load(dir.path()).expect("reload"),
        whitelist,
        "once the write succeeds the file and the list agree"
    );
    assert!(
        !temp_path(&path).exists(),
        "the staging file is consumed by the rename, never left beside the live file"
    );
}

#[test]
fn a_failed_write_leaves_the_previous_operators_intact() {
    let dir = TempDir::new("access-atomic-ops");
    let path = dir.path().join(OPS_FILE_NAME);
    let mut operators = OperatorList::new();
    operators.insert(LISTED_UUID, "Chief", PermissionLevel::Console);
    operators.save(dir.path()).expect("the first write lands");
    let before = read(&path);

    operators.insert(SECOND_UUID, "Second", PermissionLevel::Operator);
    block_staging(&path);
    let error = operators
        .save(dir.path())
        .expect_err("a blocked staging path must fail the write");
    assert!(matches!(error, ServerError::Operational(_)), "{error:?}");
    assert_eq!(
        read(&path),
        before,
        "a failed write must leave the live file byte-identical"
    );
    let on_disk = OperatorList::load(dir.path()).expect("the previous file still loads");
    assert_eq!(on_disk.len(), 1, "the previous operator list survives");
    assert!(on_disk.is_operator(LISTED_UUID));
    assert!(!on_disk.is_operator(SECOND_UUID));

    unblock_staging(&path);
    operators
        .save(dir.path())
        .expect("with the path clear the write lands");
    assert_eq!(OperatorList::load(dir.path()).expect("reload"), operators);
}

#[test]
fn a_failed_write_leaves_the_previous_bans_intact() {
    let dir = TempDir::new("access-atomic-bans");
    let players_path = dir.path().join(BANNED_PLAYERS_FILE_NAME);
    let ips_path = dir.path().join(BANNED_IPS_FILE_NAME);
    let mut bans = BanList::new();
    bans.ban_player(PlayerBan {
        uuid: LISTED_UUID.to_owned(),
        name: "Griefer".to_owned(),
        created: now(),
        source: "Chief".to_owned(),
        expires: None,
        reason: "griefing".to_owned(),
    });
    bans.save(dir.path()).expect("the first write lands");
    let players_before = read(&players_path);
    let ips_before = read(&ips_path);

    bans.ban_player(PlayerBan {
        uuid: SECOND_UUID.to_owned(),
        name: "Second".to_owned(),
        created: now(),
        source: "Chief".to_owned(),
        expires: None,
        reason: "griefing".to_owned(),
    });
    block_staging(&players_path);
    let error = bans
        .save(dir.path())
        .expect_err("a blocked staging path must fail the write");
    assert!(matches!(error, ServerError::Operational(_)), "{error:?}");
    assert_eq!(
        read(&players_path),
        players_before,
        "a failed write must leave the ban file byte-identical: this is the \
         truncation that lifted a permanent ban"
    );
    assert_eq!(
        read(&ips_path),
        ips_before,
        "the ip file is not half-written either"
    );
    let on_disk = BanList::load(dir.path()).expect("the previous files still load");
    assert_eq!(on_disk.player_count(), 1, "the previous ban survives");
    assert!(
        on_disk.is_player_banned_at(LISTED_UUID, now() + Duration::from_secs(4_000_000_000)),
        "the ban that was already on disk is still enforced after the failed write"
    );

    unblock_staging(&players_path);
    bans.save(dir.path())
        .expect("with the path clear the write lands");
    let reloaded = BanList::load(dir.path()).expect("reload");
    assert_eq!(reloaded.player_count(), 2);
    assert!(reloaded.is_player_banned_at(SECOND_UUID, now()));
}

// ------------------------------------------------------------- damaged rows

#[test]
fn a_damaged_whitelist_row_does_not_unlist_its_siblings() {
    let dir = TempDir::new("access-rows-whitelist");
    write(
        &dir.path().join(WHITELIST_FILE_NAME),
        &format!(
            r#"[
              {{"uuid": "{LISTED_UUID}", "name": "Listed"}},
              {{"name": "NoUuid"}},
              {{"uuid": "{SECOND_UUID}", "name": "Second"}}
            ]"#
        ),
    );

    let list = Whitelist::load(dir.path()).expect("one damaged row must not void the table");
    assert_eq!(list.len(), 2, "both intact rows load");
    assert_eq!(list.names(), vec!["Listed", "Second"]);
    assert!(list.contains(LISTED_UUID) && list.contains(SECOND_UUID));
}

#[test]
fn a_damaged_operator_row_does_not_undop_its_siblings() {
    let dir = TempDir::new("access-rows-ops");
    write(
        &dir.path().join(OPS_FILE_NAME),
        &format!(
            r#"[
              {{"uuid": "{LISTED_UUID}", "name": "Chief", "level": 4}},
              {{"uuid": "{SECOND_UUID}", "name": "Bad", "level": 5}},
              {{"uuid": "{SECOND_UUID}", "name": "Second", "level": 2}}
            ]"#
        ),
    );

    let list = OperatorList::load(dir.path()).expect("one damaged row must not void the table");
    assert_eq!(list.len(), 2, "both intact rows load");
    assert_eq!(list.level_for(LISTED_UUID), PermissionLevel::Console);
    assert_eq!(list.level_for(SECOND_UUID), PermissionLevel::Operator);
}

#[test]
fn a_damaged_ban_row_does_not_void_the_table() {
    let dir = TempDir::new("access-rows-bans");
    write(
        &dir.path().join(BANNED_PLAYERS_FILE_NAME),
        &format!(
            r#"[
              {{"uuid": "{LISTED_UUID}", "name": "Griefer", "created": "2023-11-14 22:13:20 +0000",
                "source": "Chief", "expires": "forever", "reason": "griefing"}},
              {{"uuid": "{SECOND_UUID}", "name": "Bad", "created": "not a date",
                "source": "Chief", "expires": "forever", "reason": "griefing"}}
            ]"#
        ),
    );
    write(
        &dir.path().join(BANNED_IPS_FILE_NAME),
        r#"[
          {"ip": "203.0.113.7", "created": "2023-11-14 22:13:20 +0000", "source": "Chief",
           "expires": "forever", "reason": "spam"},
          {"ip": "not-an-address", "created": "2023-11-14 22:13:20 +0000", "source": "Chief",
           "expires": "forever", "reason": "spam"}
        ]"#,
    );

    let bans = BanList::load(dir.path()).expect("one damaged row must not void the table");
    assert_eq!(bans.player_count(), 1, "the intact player row loads");
    assert!(
        bans.is_player_banned_at(LISTED_UUID, now() + Duration::from_secs(4_000_000_000)),
        "and it is still a permanent ban"
    );
    assert!(!bans.is_player_banned_at(SECOND_UUID, now()));
    assert_eq!(bans.ip_count(), 1, "the intact ip row loads");
    assert!(bans.is_ip_banned_at(&"203.0.113.7".parse().expect("test ip"), now()));
}

/// AUDIT-19 G-03: `"FOREVER"` is how a hand-edited file spells a permanent
/// ban. Vanilla reads `expires` case-insensitively, and a row that says
/// `"FOREVER"` must be a permanent ban rather than a row that costs the file.
#[test]
fn forever_is_a_permanent_ban_in_any_case() {
    let dir = TempDir::new("access-rows-forever");
    write(
        &dir.path().join(BANNED_PLAYERS_FILE_NAME),
        &format!(
            r#"[
              {{"uuid": "11111111-1111-1111-1111-111111111111", "name": "Upper", "created": "2023-11-14 22:13:20 +0000",
                "source": "Chief", "expires": "FOREVER", "reason": "griefing"}},
              {{"uuid": "22222222-2222-2222-2222-222222222222", "name": "Mixed", "created": "2023-11-14 22:13:20 +0000",
                "source": "Chief", "expires": "Forever", "reason": "griefing"}},
              {{"uuid": "{LISTED_UUID}", "name": "Lower", "created": "2023-11-14 22:13:20 +0000",
                "source": "Chief", "expires": "forever", "reason": "griefing"}}
            ]"#
        ),
    );

    let bans = BanList::load(dir.path()).expect("a case variant is not a damaged row");
    assert_eq!(bans.player_count(), 3, "every row loads");
    let far_future = now() + Duration::from_secs(4_000_000_000);
    for uuid in [
        "11111111-1111-1111-1111-111111111111",
        "22222222-2222-2222-2222-222222222222",
        LISTED_UUID,
    ] {
        assert_eq!(
            bans.player_ban(uuid).map(|ban| ban.expires),
            Some(None),
            "{uuid} must be a permanent ban"
        );
        assert!(
            bans.is_player_banned_at(uuid, far_future),
            "{uuid} never lapses"
        );
    }
}

// ------------------------------------------------------ never silently empty

#[test]
fn a_truncated_access_file_is_reported_by_name_never_read_as_an_empty_table() {
    // What a crash inside the old `std::fs::write` window left behind: the
    // file exists, starts like a table, and stops mid-row.
    let truncated = format!(r#"[{{"uuid": "{LISTED_UUID}", "na"#);
    let dir = TempDir::new("access-truncated");

    write(&dir.path().join(WHITELIST_FILE_NAME), &truncated);
    let error = Whitelist::load(dir.path()).expect_err("a truncated whitelist must be an error");
    assert!(
        error.to_string().contains(WHITELIST_FILE_NAME),
        "the error names the file: {error}"
    );

    write(&dir.path().join(OPS_FILE_NAME), &truncated);
    let error =
        OperatorList::load(dir.path()).expect_err("a truncated operator file must be an error");
    assert!(error.to_string().contains(OPS_FILE_NAME), "{error}");

    write(&dir.path().join(BANNED_PLAYERS_FILE_NAME), &truncated);
    let error = BanList::load(dir.path()).expect_err("a truncated ban file must be an error");
    assert!(
        error.to_string().contains(BANNED_PLAYERS_FILE_NAME),
        "{error}"
    );
}

/// The row-level half of the same rule: skipping is for a row *among good
/// ones*. When nothing could be read the file is an error, so "bans nobody"
/// is never what a broken file silently means.
#[test]
fn a_file_with_no_readable_row_is_an_error_not_an_empty_table() {
    let dir = TempDir::new("access-all-rows-damaged");

    write(
        &dir.path().join(WHITELIST_FILE_NAME),
        r#"[{"name": "NoUuid"}]"#,
    );
    let error = Whitelist::load(dir.path()).expect_err("no readable row must be an error");
    assert!(error.to_string().contains(WHITELIST_FILE_NAME), "{error}");

    write(&dir.path().join(OPS_FILE_NAME), r#"[{"uuid": "a"}]"#);
    let error = OperatorList::load(dir.path()).expect_err("no readable row must be an error");
    assert!(error.to_string().contains(OPS_FILE_NAME), "{error}");

    write(
        &dir.path().join(BANNED_PLAYERS_FILE_NAME),
        r#"[{"name": "NoUuid"}]"#,
    );
    let error = BanList::load(dir.path()).expect_err("no readable row must be an error");
    assert!(
        error.to_string().contains(BANNED_PLAYERS_FILE_NAME),
        "{error}"
    );

    // ...and an explicitly empty table is still an ordinary empty table.
    write(&dir.path().join(BANNED_PLAYERS_FILE_NAME), "[]");
    let bans = BanList::load(dir.path()).expect("an empty file is a server that bans nobody");
    assert_eq!(bans.player_count(), 0);
}
