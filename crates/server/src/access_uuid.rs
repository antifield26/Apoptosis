//! The `uuid` field of an access file, validated (AUDIT-19 G-10).
//!
//! `ops.json`, `whitelist.json` and `banned-players.json` all key their rows on
//! a uuid string, and every one of them used to accept any string at all. A
//! typo — `069a79f4-44e9-4726-a5be-f4a7b64ac90` (31 hex digits), a name pasted
//! into the field, a stray space inside — then became a row that parses,
//! counts, lists and **never matches anybody**: no error, no warning, and the
//! only symptom is a permission, a whitelist entry or a ban that silently does
//! not apply. Vanilla refuses the whole file in that case
//! (`StoredUserList` → `UUID.fromString`), which is worse than what these
//! modules want, so the row is refused instead and its siblings still load
//! through the same row-level policy the row-level fixes established.
//!
//! The accepted shapes are what Vanilla's own parser accepts and what its
//! writers emit: the canonical 8-4-4-4-12 dashed form and the 32-digit
//! undashed form. A Vanilla server reads both, and `uuid::Uuid::parse_str`
//! parses exactly these two, so this module is a thin, named seam over the
//! workspace's uuid crate rather than a second parser.

use std::path::Path;
use uuid::Uuid;

/// The uuid field of whichever access file is being read.
///
/// Shared by `ops.json` (field: operator), `whitelist.json` (listed profile)
/// and `banned-players.json` (banned profile) as well as by the three
/// `normalise_uuid` helpers they already had — one place decides what a row's
/// `uuid` may say, so the four readers cannot drift.
pub const FIELD: &str = "uuid";

/// Read the `uuid` field of one access-file row, refusing a damaged one.
///
/// `path` and `index` are only used to name the row in the error, which is
/// what makes a skipped row visible: the caller logs it and drops the row, the
/// same row-level policy the access files already use for every other damaged
/// field (AUDIT-19 G-10).
///
/// # Errors
///
/// [`ServerError::CorruptData`] naming the file, the row and `what` (the kind
/// of entry) when the field is missing, empty, or not a uuid.
pub fn from_row(
    object: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
    index: usize,
    what: &str,
) -> mc_core::error::ServerResult<String> {
    use mc_core::error::ServerError;
    let raw = object
        .get(FIELD)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{}: entry {index} has no string `{FIELD}`, which is the field that identifies \
                 a {what}",
                path.display()
            ))
        })?;
    let uuid = normalise(raw);
    if uuid.is_empty() {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} has an empty `{FIELD}`",
            path.display()
        )));
    }
    if !is_valid(&uuid) {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} has `{FIELD}` {uuid:?}, which is not a uuid — a row like this \
             would load and then never match a player; skipped",
            path.display()
        )));
    }
    Ok(uuid)
}

/// A uuid as the access modules key on it: trimmed and lower-case.
///
/// Files in the wild are not consistent about case, and a mismatch here is
/// invisible — the profile simply appears not to be listed.
#[must_use]
pub fn normalise(uuid: &str) -> String {
    uuid.trim().to_ascii_lowercase()
}

/// Uuid `text` in one of the shapes an access file may hold.
///
/// Callers trim and lower-case first ([`normalise`]); this is purely about the
/// digits and dashes. Lower-case is not required here (`A` is a hex digit),
/// but the normalised form is what the modules store.
#[must_use]
pub fn parse(text: &str) -> Option<Uuid> {
    let trimmed = text.trim();
    // Cheap shape gate before the parser: exactly 32 hex digits, optionally
    // with dashes, and at most the four a canonical uuid carries.
    // `Uuid::parse_str` is lenient about *hyphen placement* (it counts hex
    // digits), and a wrong-but-parseable uuid is exactly the silent mismatch
    // this module exists to refuse, so the shape is checked here rather than
    // inferred from the parser's success.
    let dashes = trimmed.chars().filter(|c| *c == '-').count();
    let hexdigits = trimmed.chars().filter(char::is_ascii_hexdigit).count();
    let shaped = hexdigits == 32
        && dashes <= 4
        && trimmed.len() == hexdigits + dashes
        && trimmed.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    if !shaped {
        return None;
    }
    Uuid::parse_str(trimmed).ok()
}

/// Whether `text` is a uuid an access-file row can match on.
#[must_use]
pub fn is_valid(text: &str) -> bool {
    parse(text).is_some()
}

#[cfg(test)]
mod tests {
    use super::{is_valid, parse};

    #[test]
    fn canonical_and_undashed_shapes_both_parse() {
        // What a Vanilla server writes (dashed) and what Mojang's own
        // `hasJoined` answers with (undashed) — both must keep working.
        let dashed = "069a79f4-44e9-4726-a5be-f4a7b64ac909";
        let undashed = "069a79f444e94726a5bef4a7b64ac909";
        assert_eq!(
            parse(dashed).expect("dashed").to_string(),
            dashed,
            "the dashed form round-trips"
        );
        assert_eq!(
            parse(undashed).expect("undashed").to_string(),
            dashed,
            "the undashed form names the same profile"
        );
        assert_eq!(
            parse(&dashed.to_ascii_uppercase()).map(|id| id.to_string()),
            Some(dashed.to_owned()),
            "case never mattered, in either direction"
        );
    }

    #[test]
    fn a_typo_is_refused_instead_of_becoming_a_row_that_never_matches() {
        // AUDIT-19 G-10: each of these used to load as a live row and then
        // silently never match a joining player.
        for bad in [
            "",                                      // empty
            "not-a-uuid-at-all",                     // a name pasted in
            "069a79f4-44e9-4726-a5be-f4a7b64ac90",   // 31 digits: one short
            "069a79f4-44e9-4726-a5be-f4a7b64ac9090", // 33 digits: one long
            "069a79f4-44e9-4726-a5be-f4a7b64ac90g",  // a non-hex digit
            "069a79f444e94726a5bef4a7b64ac90",       // 31 digits, no dashes
            "069a79f4_44e9_4726_a5be_f4a7b64ac909",  // underscores
            "069a79f4-44e9-4726-a5be-f4a7b64ac9-09", // five dashes
            "  ",
        ] {
            assert!(!is_valid(bad), "{bad:?} must be refused");
            assert!(parse(bad).is_none(), "{bad:?} must not parse");
        }
        // Forgiving where it should be: surrounding whitespace is trimmed by
        // the access modules before they ask, and case never mattered.
        assert!(is_valid("069A79F4-44E9-4726-A5BE-F4A7B64AC909"));
    }

    #[test]
    fn the_nil_uuid_is_still_a_uuid() {
        // All-zero is a legal uuid and must not be confused with "missing".
        assert!(is_valid("00000000-0000-0000-0000-000000000000"));
        assert!(is_valid("00000000000000000000000000000000"));
    }
}
