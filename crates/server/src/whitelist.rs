//! The server whitelist, `whitelist.json` (P19-01).
//!
//! Vanilla's shape, beside `ops.json`: a JSON array of `{uuid, name}`
//! entries. Matching is by uuid, exactly like operators — names change, and
//! matching by name would let anyone take a listed identity by taking their
//! name. Policy mirrors [`crate::ops`]:
//!
//! - **A missing file is not an error.** A server that never listed anyone
//!   is ordinary; its absence only matters when enforcement is on, and then
//!   it means "nobody is listed", not "the file is broken".
//! - **One damaged row does not void the table.** A row that cannot be
//!   parsed is skipped with a `warn!` naming the file and the row, and its
//!   siblings still load (AUDIT-19 G-02/G-03). A file that is not a JSON
//!   array at all, or whose *every* row is damaged, is still an error from
//!   [`Whitelist::load`] and the *caller* decides: an empty list must never
//!   be indistinguishable from a file nobody could read.
//!   [`crate::lifecycle`] logs it and boots with an empty list, because
//!   taking a working world offline over a comma is the worse failure.
//! - **The file is replaced atomically.** A sibling temp file is written,
//!   fsynced and renamed over the target — the repository's
//!   `mc_persistence::save::write_atomic` protocol, the same one `level.dat`
//!   uses. `std::fs::write` truncates in place, so a crash inside the write
//!   window left a truncated `whitelist.json`, which reads as "nobody is
//!   listed" (AUDIT-19 G-02/G-03). A failed write leaves the previous file
//!   live, so the caller's rollback of the in-memory list cannot diverge.
//! - **The file stays Vanilla-pure.** No extra keys are written, so a
//!   Vanilla server boots on our file and keeps every entry (P19-01
//!   acceptance) — enforcement state lives in config + memory, never here.
//! - **The `uuid` field is a uuid.** A row whose uuid is a typo (or a name)
//!   would load and then never match a joining player, silently: the listed
//!   player would simply be refused, or a ban would never apply
//!   (AUDIT-19 G-10). The format is validated on load through
//!   [`crate::access_uuid`] and a bad row is skipped with the same
//!   warning-names-the-row policy as every other damaged field. Both shapes
//!   Vanilla reads are accepted: the canonical dashed uuid and the 32-digit
//!   undashed one.
//! - **Operators bypass the whitelist.** Vanilla exempts ops from the
//!   check; the exemption is evaluated at the join gate, not stored here.

use mc_core::error::{ServerError, ServerResult};
use mc_data::json::{Limits, read_json};
use mc_persistence::save::write_atomic;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name beside `ops.json` (the world's parent directory).
pub const WHITELIST_FILE_NAME: &str = "whitelist.json";

/// Size and depth limits for the file.
///
/// A whitelist is a handful of lines, so these are generous. Stated rather
/// than relying on `mc_data::json`'s defaults so a future change to *those*
/// defaults cannot widen what this file accepts without anyone noticing.
pub const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_depth: 16,
};

/// One whitelisted profile, as the file states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whitelisted {
    /// Profile uuid, and the key matching uses.
    pub uuid: String,
    /// The name as recorded. For messages; **not** used for matching.
    pub name: String,
}

/// Who may join when enforcement is on, loaded from `whitelist.json`.
///
/// Ordered by uuid so iteration is reproducible (AGENTS.md §3.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Whitelist {
    by_uuid: BTreeMap<String, Whitelisted>,
}

impl Whitelist {
    /// An empty list: nobody is listed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            by_uuid: BTreeMap::new(),
        }
    }

    /// Load `whitelist.json` from a directory.
    ///
    /// A missing file yields an empty list, which is an ordinary server. A
    /// damaged **row** is skipped and logged, its siblings still load; a
    /// **file** that is not a JSON array, or whose every row is damaged, is
    /// an error returned rather than an empty list — the policy for the
    /// error belongs to the caller, and `crate::lifecycle` logs it and boots
    /// with an empty list.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem when it
    /// exists but cannot be understood, and [`ServerError::Io`] when it
    /// exists and cannot be read.
    pub fn load(directory: &Path) -> ServerResult<Self> {
        let path = directory.join(WHITELIST_FILE_NAME);
        // Same split as operators: "not there" is a different answer from
        // "there and unreadable", and only the second is reported.
        if !path.exists() {
            return Ok(Self::new());
        }
        let value = read_json(&path, LIMITS).map_err(ServerError::from)?;
        Self::from_value(&value, &path)
    }

    /// Build a list from an already-parsed JSON value.
    ///
    /// A damaged row is skipped and logged, its siblings still load. A file
    /// whose every row is damaged is an error (the first row's problem,
    /// naming the file) rather than an empty list: "lists nobody" and
    /// "nobody could be read" must not look alike.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem when the
    /// value is not an array, when no row could be read, or when a uuid
    /// repeats.
    pub fn from_value(value: &Value, path: &Path) -> ServerResult<Self> {
        let Some(entries) = value.as_array() else {
            return Err(ServerError::CorruptData(format!(
                "{}: a whitelist must be a JSON array of entries, found {}",
                path.display(),
                json_type_name(value)
            )));
        };

        let mut list = Self::new();
        let mut first_problem: Option<ServerError> = None;
        for (index, entry) in entries.iter().enumerate() {
            let whitelisted = match parse_entry(entry, path, index) {
                Ok(whitelisted) => whitelisted,
                Err(error) => {
                    // One damaged row must not void the table: the siblings
                    // still load. The line names the file and the row, so a
                    // skipped entry is never silent (AUDIT-19 G-02/G-03).
                    tracing::warn!(
                        path = %path.display(),
                        row = index,
                        %error,
                        "skipping a damaged whitelist row"
                    );
                    if first_problem.is_none() {
                        first_problem = Some(error);
                    }
                    continue;
                }
            };
            // A duplicated uuid is a file that cannot mean what it says, so
            // it is refused rather than resolved by last-wins.
            if list.by_uuid.contains_key(&whitelisted.uuid) {
                return Err(ServerError::CorruptData(format!(
                    "{}: entry {index} repeats uuid {}",
                    path.display(),
                    whitelisted.uuid
                )));
            }
            list.by_uuid.insert(whitelisted.uuid.clone(), whitelisted);
        }
        if list.is_empty() && !entries.is_empty() {
            return Err(first_problem.unwrap_or_else(|| {
                ServerError::CorruptData(format!(
                    "{}: none of the {} entries could be read",
                    path.display(),
                    entries.len()
                ))
            }));
        }
        Ok(list)
    }

    /// List `uuid` under `name`. Returns whether the list changed.
    ///
    /// Re-listing the same uuid updates the recorded name rather than
    /// adding a second entry, so the file can never hold two rows for one
    /// profile.
    pub fn insert(&mut self, uuid: &str, name: &str) -> bool {
        let uuid = normalise_uuid(uuid);
        let changed = self
            .by_uuid
            .get(&uuid)
            .is_none_or(|current| current.name != name);
        self.by_uuid.insert(
            uuid.clone(),
            Whitelisted {
                uuid,
                name: name.to_owned(),
            },
        );
        changed
    }

    /// Unlist `uuid`. Returns whether an entry was removed.
    pub fn remove(&mut self, uuid: &str) -> bool {
        self.by_uuid.remove(&normalise_uuid(uuid)).is_some()
    }

    /// Restore a previously removed entry verbatim.
    ///
    /// `pub(crate)` for the failed-`save` rollback, mirroring operators:
    /// a failed write must put back *exactly* what was there.
    pub(crate) fn restore(&mut self, whitelisted: Whitelisted) {
        self.by_uuid.insert(whitelisted.uuid.clone(), whitelisted);
    }

    /// Write the list back to `whitelist.json` in `directory`, Vanilla's shape.
    ///
    /// Entries ascending by uuid (the map order), pretty-printed like
    /// Vanilla's own writer, with exactly the two keys Vanilla reads —
    /// nothing extra, so a Vanilla server keeps every entry. A missing
    /// directory is created; anything else that goes wrong is an error the
    /// caller reports rather than a list change the caller pretends happened.
    ///
    /// The replacement is **atomic** (AUDIT-19 G-02/G-03): a sibling temp
    /// file, fsynced, then renamed over the target by
    /// [`mc_persistence::save::write_atomic`] — the protocol `level.dat`
    /// already uses. Before this, `std::fs::write` truncated the live file,
    /// so a crash inside the write window left a truncated whitelist; a
    /// failed write now leaves the previous file byte-identical.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the directory cannot be created or
    /// the file cannot be written or serialised.
    pub fn save(&self, directory: &Path) -> ServerResult<()> {
        let entries: Vec<serde_json::Value> = self
            .by_uuid
            .values()
            .map(|whitelisted| {
                serde_json::json!({
                    "uuid": whitelisted.uuid,
                    "name": whitelisted.name,
                })
            })
            .collect();
        let text = serde_json::to_string_pretty(&entries).map_err(|error| {
            ServerError::Operational(format!(
                "{}: cannot serialise: {error}",
                directory.display()
            ))
        })?;
        std::fs::create_dir_all(directory).map_err(|error| {
            ServerError::Operational(format!(
                "{}: cannot create directory: {error}",
                directory.display()
            ))
        })?;
        write_atomic(&directory.join(WHITELIST_FILE_NAME), text.as_bytes(), false).map_err(
            |error| {
                ServerError::Operational(format!(
                    "{}: cannot write {WHITELIST_FILE_NAME}: {error}",
                    directory.display()
                ))
            },
        )
    }

    /// The entry for a uuid, normalising the key the way `parse_entry` does.
    ///
    /// **Every accessor goes through here**: lower-casing on the way in but
    /// not on the way out would make a listed profile invisible to an
    /// upper-case query, with "not white-listed" as the silent symptom.
    #[must_use]
    pub fn get(&self, uuid: &str) -> Option<&Whitelisted> {
        self.by_uuid.get(&normalise_uuid(uuid))
    }

    /// Whether a profile is listed.
    #[must_use]
    pub fn contains(&self, uuid: &str) -> bool {
        self.get(uuid).is_some()
    }

    /// How many profiles are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_uuid.len()
    }

    /// Whether nobody is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_uuid.is_empty()
    }

    /// Every listed name, ascending by uuid (the map order).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.by_uuid
            .values()
            .map(|whitelisted| whitelisted.name.as_str())
            .collect()
    }
}

/// A uuid as this module keys on it: trimmed and lower-case.
///
/// The rule lives in [`crate::access_uuid::normalise`], shared with the
/// operators and bans readers, because "how a uuid is keyed" must not be three
/// answers: a mismatch here is invisible — the profile simply appears not to be
/// listed.
#[must_use]
fn normalise_uuid(uuid: &str) -> String {
    crate::access_uuid::normalise(uuid)
}

/// Parse one array entry.
fn parse_entry(entry: &Value, path: &Path, index: usize) -> ServerResult<Whitelisted> {
    let Some(object) = entry.as_object() else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} is {}, not an object",
            path.display(),
            json_type_name(entry)
        )));
    };

    // AUDIT-19 G-10: the uuid is validated, not merely required to be a
    // string. A typo used to load as a row that parses, lists and then never
    // matches anybody, with no warning anywhere.
    let uuid = crate::access_uuid::from_row(object, path, index, "listed profile")?;

    // A name is optional in practice: Vanilla always writes one, but matching
    // is by uuid, so a missing name is a cosmetic loss rather than a reason
    // to refuse the whole file.
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();

    Ok(Whitelisted { uuid, name })
}

/// The JSON type name for refusal messages.
fn json_type_name(value: &Value) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "a boolean"
    } else if value.is_number() {
        "a number"
    } else if value.is_string() {
        "a string"
    } else if value.is_array() {
        "an array"
    } else {
        "an object"
    }
}

#[cfg(test)]
mod tests {
    use super::{WHITELIST_FILE_NAME, Whitelist};
    use mc_test_support::fixtures::TempDir;
    use std::path::Path;

    fn dir(tag: &str) -> TempDir {
        TempDir::new(tag)
    }

    #[test]
    fn missing_file_is_an_empty_list() {
        let dir = dir("whitelist-missing");
        let list = Whitelist::load(dir.path()).expect("missing file loads");
        assert!(list.is_empty());
    }

    #[test]
    fn round_trip_keeps_uuid_and_name() {
        let dir = dir("whitelist-round-trip");
        let mut list = Whitelist::new();
        assert!(list.insert("069a79f4-44e9-4726-a5be-f4a7b64ac909", "Notch"));
        assert!(!list.insert("069a79f4-44e9-4726-a5be-f4a7b64ac909", "Notch"));
        list.save(dir.path()).expect("saves");
        let back = Whitelist::load(dir.path()).expect("reloads");
        assert_eq!(back, list);
        assert!(back.contains("069a79f4-44e9-4726-a5be-f4a7b64ac909"));
        assert_eq!(back.names(), vec!["Notch"]);
    }

    #[test]
    fn matching_ignores_uuid_case() {
        let mut list = Whitelist::new();
        list.insert("069A79F4-44E9-4726-A5BE-F4A7B64AC909", "Notch");
        assert!(
            list.contains("069a79f4-44e9-4726-a5be-f4a7b64ac909"),
            "an upper-case file entry must still match"
        );
    }

    #[test]
    fn duplicate_uuid_is_refused() {
        let value = serde_json::json!([
            {"uuid": "069a79f4-44e9-4726-a5be-f4a7b64ac909", "name": "Notch"},
            {"uuid": "069a79f4-44e9-4726-a5be-f4a7b64ac909", "name": "Notch2"},
        ]);
        assert!(
            Whitelist::from_value(&value, Path::new("whitelist.json")).is_err(),
            "a duplicated uuid cannot mean what it says"
        );
    }

    #[test]
    fn malformed_files_are_refused_by_name() {
        for value in [
            serde_json::json!({"uuid": "x"}),
            serde_json::json!([{"name": "NoUuid"}]),
            serde_json::json!([{"uuid": ""}]),
            serde_json::json!(["a string entry"]),
        ] {
            assert!(
                Whitelist::from_value(&value, Path::new("whitelist.json")).is_err(),
                "refused: {value}"
            );
        }
    }

    #[test]
    fn a_typoed_uuid_is_a_damaged_row_not_a_row_that_never_matches() {
        // AUDIT-19 G-10: `{"uuid": "not-a-uuid-at-all"}` used to be accepted
        // into the list (the live probe loaded `listed=2`), where it could
        // never match a joining player. It is now a damaged row: skipped with
        // a warning that names the file and the row, siblings still load.
        let value = serde_json::json!([
            {"uuid": "069a79f4-44e9-4726-a5be-f4a7b64ac909", "name": "Notch"},
            {"uuid": "not-a-uuid-at-all", "name": "Typo"},
        ]);
        let list = Whitelist::from_value(&value, Path::new("whitelist.json"))
            .expect("one bad row does not void the table");
        assert_eq!(list.len(), 1, "only the valid row is listed");
        assert_eq!(list.names(), vec!["Notch"]);
        assert!(
            !list.contains("not-a-uuid-at-all"),
            "the typo is not a live row"
        );

        // Every row damaged is still the file-level error path, naming the
        // first row's problem rather than booting as "nobody is listed".
        let only_bad = serde_json::json!([{"uuid": "oops", "name": "Typo"}]);
        let error = Whitelist::from_value(&only_bad, Path::new("whitelist.json"))
            .expect_err("a file whose every row is damaged is an error");
        let text = error.to_string();
        assert!(text.contains("whitelist.json"), "{text}");
        assert!(text.contains("uuid"), "{text}");

        // Both shapes a Vanilla file may carry still load, so the validation
        // cannot lock out a legitimate file. The undashed form keeps its
        // spelling (matching is exact-string, so normalising shapes is a
        // separate change), which is why each case is looked up as written.
        for uuid in [
            "069a79f4-44e9-4726-a5be-f4a7b64ac909",
            "069a79f444e94726a5bef4a7b64ac909",
        ] {
            let value = serde_json::json!([{"uuid": uuid, "name": "Notch"}]);
            let list = Whitelist::from_value(&value, Path::new("whitelist.json"))
                .unwrap_or_else(|error| panic!("{uuid} must load: {error}"));
            assert_eq!(list.len(), 1, "{uuid} is a real uuid");
            assert!(list.contains(uuid), "{uuid} is the row that loaded");
        }
    }

    #[test]
    fn vanilla_shaped_file_loads_and_stays_vanilla_shaped() {
        // What a Vanilla server writes: uuid + name only. Loading keeps both,
        // and saving writes exactly those two keys back.
        let dir = dir("whitelist-vanilla");
        std::fs::write(
            dir.path().join(WHITELIST_FILE_NAME),
            r#"[{"uuid": "069a79f4-44e9-4726-a5be-f4a7b64ac909", "name": "Notch"}]"#,
        )
        .expect("fixture");
        let list = Whitelist::load(dir.path()).expect("vanilla file loads");
        assert_eq!(list.len(), 1);
        assert_eq!(list.names(), vec!["Notch"]);
        list.save(dir.path()).expect("saves");
        let text = std::fs::read_to_string(dir.path().join(WHITELIST_FILE_NAME)).expect("reads");
        let back: serde_json::Value = serde_json::from_str(&text).expect("json");
        for entry in back.as_array().expect("array") {
            let mut keys: Vec<&str> = entry
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec!["name", "uuid"],
                "only Vanilla's two keys are written, saw {keys:?}"
            );
        }
    }
}
