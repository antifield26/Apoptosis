//! `ops.json`: who may run operator commands (P07-04).
//!
//! ## What this closes
//!
//! Before this module, permission levels existed and were enforced but nothing ever granted
//! one: a player was level 0 for their whole session, so `/op` and `/stop` were unreachable
//! from a client. That was recorded as an honest limitation, and this is the other half.
//!
//! The file is Vanilla's: an array of entries beside `server.properties`, each naming a
//! player by **uuid** and giving a numeric `level`:
//!
//! ```json
//! [ { "uuid": "069a79f4-44e9-4726-a5be-fca90e38aaf5",
//!     "name": "Notch", "level": 4, "bypassesPlayerLimit": false } ]
//! ```
//!
//! ## The decisions, each of which could reasonably go the other way
//!
//! - **Read at startup, written by `/op` and `/deop` (P14-02).** Vanilla
//!   rewrites the file on every grant and revoke; so does this build now —
//!   the in-memory change rolls back when the write fails, so the file and
//!   the list never disagree about who is an operator.
//! - **A missing file is not an error.** Vanilla creates one on first run; a server with no
//!   operators is a normal server.
//! - **A malformed file is an error from [`OperatorList::load`], and the *caller*
//!   decides.** The load itself refuses to guess: it names the file and the problem
//!   and returns an error rather than returning a half-parsed list. What the server
//!   does with that error is a separate decision, made at the call site
//!   ([`crate::lifecycle`]): it logs the error at `error!` and boots with **no
//!   operators**, because taking a working world offline over a comma in an operator
//!   file is the worse failure, and the operator can read the message and fix it.
//!   The two halves used to be described as one policy here ("it stops the load"),
//!   which contradicted the call site (AUDIT-09 C-04); Vanilla's
//!   `StoredUserList.load()` likewise propagates and leaves the choice to its
//!   caller (`javap -c`: it declares `throws IOException` and catches nothing).
//! - **A uuid is the identity, a name is not.** Names change; the file's `name` field is
//!   carried for messages and matching is by uuid. An entry with a valid uuid and a stale
//!   name still grants — which is Vanilla's behaviour and the reason the uuid is the key.
//!   The reverse (matching by name) would let anyone take an operator's identity by taking
//!   their name.
//! - **An unknown level is refused, not clamped.** Level 5 is a typo or a different game's
//!   file; treating it as 4 would grant more authority than the file asks for.
//! - **`bypassesPlayerLimit` is enforced at the join gate** (`Game::is_full_for`):
//!   a listed operator with the flag joins a full server; everyone else is
//!   refused with a disconnect. Parse + report + enforce, all three, each with
//!   a test (`ops::tests::*bypass*`, `ops_e2e::a_full_server_*`).

use mc_command::PermissionLevel;
use mc_core::error::{ServerError, ServerResult};
use mc_data::json::{Limits, read_json};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's name beside `server.properties` and the world directory.
pub const OPS_FILE_NAME: &str = "ops.json";

/// Size and depth limits for the file.
///
/// An operator list is a handful of lines, so these are generous. They are stated rather than
/// relying on `mc_data::json`'s defaults so a future change to *those* defaults cannot widen
/// what this file accepts without anyone noticing.
pub const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_depth: 16,
};

/// One operator entry, as the file states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operator {
    /// Profile uuid, and the key matching uses.
    pub uuid: String,
    /// The name as recorded. For messages; **not** used for matching.
    pub name: String,
    /// Permission level from the file.
    pub level: PermissionLevel,
    /// Whether the file grants a player-limit bypass.
    ///
    /// Enforced by [`crate::game::Game::is_full_for`]: a listed operator with
    /// this flag joins a full server rather than being refused with it.
    pub bypasses_player_limit: bool,
}

/// Who may run what, loaded from `ops.json`.
///
/// Ordered by uuid so iteration is reproducible (AGENTS.md §3.6) and so a diagnostic prints
/// the same list every run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperatorList {
    by_uuid: BTreeMap<String, Operator>,
}

impl OperatorList {
    /// An empty list: nobody is an operator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            by_uuid: BTreeMap::new(),
        }
    }

    /// Load `ops.json` from a directory.
    ///
    /// A missing file yields an empty list, which is a normal server. A **malformed** file is
    /// an error, and this function returns it rather than a partial list — the *policy* for
    /// what to do about that error belongs to the caller, and `crate::lifecycle` logs it and
    /// boots with no operators. The module documentation states both halves and why they are
    /// separate (AUDIT-09 C-04: this doc and the call site disagreed about which it was).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem when it exists but cannot
    /// be understood, and [`ServerError::Io`] when it exists and cannot be read.
    pub fn load(directory: &Path) -> ServerResult<Self> {
        let path = directory.join(OPS_FILE_NAME);
        // Vanilla creates the file on first run. A server with no operators is ordinary, so
        // its absence is not a problem to report — and it is checked *before* the JSON
        // boundary because "not there" is a different answer from "there and unreadable".
        if !path.exists() {
            return Ok(Self::new());
        }
        // `read_json` checks the size **from metadata before reading**, enforces the depth
        // limit, and carries the path in every error — so the whole policy is applied by one
        // call. Using it rather than `serde_json` directly is deliberate: this is an
        // operator-supplied file, and the project has one policy for those.
        let value = read_json(&path, LIMITS).map_err(|error| corrupt(&error))?;
        Self::from_value(&value, &path)
    }

    /// Parse the file's contents.
    ///
    /// # Errors
    ///
    /// As for [`OperatorList::load`], with `path` used only to name the file in the message.
    pub fn parse(text: &str, path: &Path) -> ServerResult<Self> {
        // No size check here: the text is already in memory, so `parse_json` is the right
        // entry point and the limit belongs to `load`, which is where a file is involved.
        let value = mc_data::json::parse_json(text, path).map_err(|error| corrupt(&error))?;
        Self::from_value(&value, path)
    }

    /// Build a list from an already-parsed JSON value.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem.
    pub fn from_value(value: &Value, path: &Path) -> ServerResult<Self> {
        let Some(entries) = value.as_array() else {
            return Err(ServerError::CorruptData(format!(
                "{}: an operator list must be a JSON array of entries, found {}",
                path.display(),
                json_type_name(value)
            )));
        };

        let mut list = Self::new();
        for (index, entry) in entries.iter().enumerate() {
            let operator = parse_entry(entry, path, index)?;
            // A duplicated uuid is a file that cannot mean what it says, so it is refused
            // rather than resolved by last-wins: the two entries could name different levels
            // and the authority granted would depend on file order.
            if let Some(previous) = list.by_uuid.get(&operator.uuid) {
                return Err(ServerError::CorruptData(format!(
                    "{}: entry {index} repeats uuid {} (level {} and level {})",
                    path.display(),
                    operator.uuid,
                    previous.level.level(),
                    operator.level.level()
                )));
            }
            list.by_uuid.insert(operator.uuid.clone(), operator);
        }
        Ok(list)
    }

    /// Grant (or re-grant) `uuid` at `level`, recording `name` for messages.
    ///
    /// Matching stays by uuid: granting "Notch" twice updates the one entry
    /// rather than adding a second, so the file can never hold two levels for
    /// the same operator (the load path refuses duplicates for the same
    /// reason). Returns whether the list changed.
    pub fn insert(&mut self, uuid: &str, name: &str, level: PermissionLevel) -> bool {
        let uuid = normalise_uuid(uuid);
        let changed = self.by_uuid.get(&uuid).is_none_or(|current| {
            current.level != level || current.name != name || current.bypasses_player_limit
        });
        // A re-grant keeps a previously granted bypass: revoking a flag the
        // operator holds is `/deop`'s opposite and is not this command's job.
        let bypasses_player_limit = self
            .by_uuid
            .get(&uuid)
            .is_some_and(|current| current.bypasses_player_limit);
        self.by_uuid.insert(
            uuid.clone(),
            Operator {
                uuid,
                name: name.to_owned(),
                level,
                bypasses_player_limit,
            },
        );
        changed
    }

    /// Revoke `uuid`. Returns whether an entry was removed.
    pub fn remove(&mut self, uuid: &str) -> bool {
        self.by_uuid.remove(&normalise_uuid(uuid)).is_some()
    }

    /// Restore a previously removed entry verbatim, bypass flag included.
    ///
    /// `pub(crate)` because it exists for one caller: a failed `save` rolling
    /// back a revoke must put back *exactly* what was there, and `insert`
    /// deliberately never restores a bypass (a grant is not a restore).
    pub(crate) fn restore(&mut self, operator: Operator) {
        self.by_uuid.insert(operator.uuid.clone(), operator);
    }

    /// Write the list back to `ops.json` in `directory`, Vanilla's shape.
    ///
    /// Entries ascending by uuid (the map order), pretty-printed like
    /// Vanilla's own writer. A missing directory is created; anything else
    /// that goes wrong is an error the caller reports rather than a grant
    /// the caller pretends happened.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the directory cannot be created or
    /// the file cannot be written or serialised.
    pub fn save(&self, directory: &Path) -> ServerResult<()> {
        let entries: Vec<serde_json::Value> = self
            .by_uuid
            .values()
            .map(|operator| {
                serde_json::json!({
                    "uuid": operator.uuid,
                    "name": operator.name,
                    "level": operator.level.level(),
                    "bypassesPlayerLimit": operator.bypasses_player_limit,
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
        std::fs::write(directory.join(OPS_FILE_NAME), text).map_err(|error| {
            ServerError::Operational(format!(
                "{}: cannot write {OPS_FILE_NAME}: {error}",
                directory.display()
            ))
        })
    }

    /// The entry for a uuid, normalising the key the way `parse_entry` does.
    ///
    /// **Every accessor goes through here**, because lower-casing on the way in but not on the
    /// way out makes a listed operator invisible to an upper-case query — silently, and with
    /// the symptom being "permissions stopped working". A test caught exactly that.
    #[must_use]
    pub fn get(&self, uuid: &str) -> Option<&Operator> {
        self.by_uuid.get(&normalise_uuid(uuid))
    }

    /// The level a profile holds, or [`PermissionLevel::All`] when it is not an operator.
    #[must_use]
    pub fn level_for(&self, uuid: &str) -> PermissionLevel {
        self.get(uuid)
            .map_or(PermissionLevel::All, |operator| operator.level)
    }

    /// Whether a profile is an operator at all.
    #[must_use]
    pub fn is_operator(&self, uuid: &str) -> bool {
        self.get(uuid).is_some()
    }

    /// How many operators are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_uuid.len()
    }

    /// Whether nobody is an operator.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_uuid.is_empty()
    }

    /// Every entry, ascending by uuid.
    pub fn operators(&self) -> impl Iterator<Item = &Operator> {
        self.by_uuid.values()
    }

    /// How many entries ask for a player-limit bypass (enforced at the join gate).
    #[must_use]
    pub fn bypass_count(&self) -> usize {
        self.by_uuid
            .values()
            .filter(|operator| operator.bypasses_player_limit)
            .count()
    }
}

/// A uuid as this module keys on it: trimmed and lower-case.
///
/// A uuid is conventionally lower-case but files in the wild are not consistent, and a
/// mismatch here is invisible — the operator simply appears not to be listed.
#[must_use]
fn normalise_uuid(uuid: &str) -> String {
    uuid.trim().to_ascii_lowercase()
}

/// The directory `ops.json` lives in: the world's **parent**, matching Vanilla.
///
/// Vanilla puts `ops.json` beside `server.properties`, which is the directory *containing* the
/// world, not the world directory itself. Getting this wrong would silently find no operators,
/// which is exactly the failure this module exists to make impossible, so it is a named
/// function with a test rather than an inline `parent()`.
#[must_use]
pub fn ops_directory(world_dir: &Path) -> PathBuf {
    match world_dir.parent() {
        // `Path::new("world").parent()` is `Some("")`, not `None`, so an empty parent has to
        // be handled explicitly. Both `""` and `"."` behave as the current directory, but `""`
        // prints badly and is a confusing thing to return.
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Parse one array entry.
fn parse_entry(entry: &Value, path: &Path, index: usize) -> ServerResult<Operator> {
    let Some(object) = entry.as_object() else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} is {}, not an object",
            path.display(),
            json_type_name(entry)
        )));
    };

    let uuid = object
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{}: entry {index} has no string `uuid`, which is the field that identifies \
                 an operator",
                path.display()
            ))
        })?;
    let uuid = normalise_uuid(uuid);
    if uuid.is_empty() {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} has an empty `uuid`",
            path.display()
        )));
    }

    // A name is optional in practice: Vanilla always writes one, but matching is by uuid, so
    // a missing name is a cosmetic loss rather than a reason to refuse the whole file.
    // (`uuid` was normalised by `normalise_uuid` above.)
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();

    // Vanilla's field is `level`. A file without one cannot express authority, so this is
    // refused rather than defaulted to 4 — defaulting to the highest level would grant more
    // than the file asks for, and defaulting to 0 would make a listed operator a no-op.
    let Some(raw_level) = object.get("level").and_then(serde_json::Value::as_i64) else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} ({uuid}) has no integer `level`",
            path.display()
        )));
    };
    let Ok(level_number) = u8::try_from(raw_level) else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} ({uuid}) has level {raw_level}, which is not 0-4",
            path.display()
        )));
    };
    let Some(level) = PermissionLevel::from_level(level_number) else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} ({uuid}) has level {raw_level}; Vanilla's levels are 0-4 and an \
             unknown one is refused rather than clamped, because clamping would grant \
             authority the file does not ask for",
            path.display()
        )));
    };

    // Optional: absent means false, which is Vanilla's default.
    let bypasses_player_limit = object
        .get("bypassesPlayerLimit")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    Ok(Operator {
        uuid,
        name,
        level,
        bypasses_player_limit,
    })
}

/// Turn a JSON-boundary error into a server error naming the file.
///
/// A file that cannot be **read** is the environment failing, which is `Operational`; a file
/// that cannot be **understood** is the data being wrong, which is `CorruptData`. The JSON
/// boundary distinguishes the two and this preserves the distinction rather than flattening
/// both into one variant.
fn corrupt(error: &mc_data::json::JsonError) -> ServerError {
    match error {
        mc_data::json::JsonError::Io { .. } => ServerError::Operational(error.to_string()),
        _ => ServerError::CorruptData(error.to_string()),
    }
}

/// A JSON value's type, for a message that says what was found.
///
/// Delegates to `mc_data::json`, so there is one description of a JSON type in the project
/// rather than two that can drift.
fn json_type_name(value: &Value) -> &'static str {
    mc_data::json::type_name(value)
}

#[cfg(test)]
mod tests;
