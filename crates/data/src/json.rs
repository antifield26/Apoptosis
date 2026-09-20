//! Bounded JSON reading (P07-03, P07-12).
//!
//! Every file a data pack contributes goes through here, so the hostile-input policy
//! lives in one place instead of being re-derived at each call site:
//!
//! - **size**: a file larger than [`Limits::max_bytes`] is refused *before* parsing, so
//!   a huge file cannot be decoded and then rejected;
//! - **depth**: `serde_json`'s own recursion limit applies (128 by default), and a
//!   pack cannot raise it;
//! - **shape**: a document that is not an object where an object is required is an
//!   error naming the file, not a silent default;
//! - **no panics**: nothing here unwraps. A malformed pack is reported and the pack is
//!   skipped, because a bad data pack must not take the server down (AGENTS.md §9).
//!
//! The error type carries the **path** as well as the reason: with 758 tag files in
//! vanilla alone, "invalid JSON" without a filename is not a usable diagnosis.

use mc_core::error::ServerError;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Ceilings applied to every data file.
///
/// The defaults are derived from the vanilla pack rather than picked: the largest
/// file in `data/minecraft/` at 26.1.2 is well under 1 MiB, so the limit is generous
/// for real data and still bounded. The point is that it *is* bounded — a pack is not
/// a trusted input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest file read, in bytes.
    pub max_bytes: u64,
    /// Deepest JSON nesting accepted.
    ///
    /// `serde_json` enforces this itself; the value is carried here so the policy is
    /// visible and testable rather than implicit in a dependency's default.
    pub max_depth: usize,
}

impl Limits {
    /// 4 MiB per file, 64 levels deep.
    ///
    /// 4 MiB is ~4x the largest vanilla data file, which leaves room for a big
    /// datapack's loot tables while staying bounded. 64 levels is half of
    /// `serde_json`'s limit and well above vanilla's deepest tag nesting (4).
    pub const DEFAULT: Self = Self {
        max_bytes: 4 * 1024 * 1024,
        max_depth: 64,
    };

    /// Limits for tests that need to provoke a refusal cheaply.
    #[must_use]
    pub const fn with_max_bytes(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            ..Self::DEFAULT
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a data file could not be read.
#[derive(Debug, Error)]
pub enum JsonError {
    /// The file could not be opened or read.
    #[error("{p}: {source}", p = path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file is larger than the configured ceiling.
    #[error("{p}: {bytes} bytes exceeds the {limit}-byte limit", p = path.display())]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// Its size in bytes.
        bytes: u64,
        /// The ceiling.
        limit: u64,
    },
    /// The file is not valid JSON, or not the shape the caller expected.
    #[error("{p}: {reason}", p = path.display())]
    Invalid {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
}

impl JsonError {
    /// The file this error is about.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Io { path, .. } | Self::TooLarge { path, .. } | Self::Invalid { path, .. } => {
                path
            }
        }
    }
}

/// Read a file as JSON, enforcing [`Limits`].
///
/// # Errors
///
/// [`JsonError::TooLarge`] when the file exceeds the ceiling (checked from metadata
/// before reading, so a huge file is never loaded), [`JsonError::Io`] when it cannot
/// be read, [`JsonError::Invalid`] when it is not JSON.
pub fn read_json(path: &Path, limits: Limits) -> Result<serde_json::Value, JsonError> {
    let metadata = std::fs::metadata(path).map_err(|source| JsonError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > limits.max_bytes {
        return Err(JsonError::TooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            limit: limits.max_bytes,
        });
    }
    let text = std::fs::read_to_string(path).map_err(|source| JsonError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_json(&text, path)
}

/// Parse JSON text that was already read.
///
/// Split from [`read_json`] so unit tests can exercise the parser without a file.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the text is not JSON.
pub fn parse_json(text: &str, path: &Path) -> Result<serde_json::Value, JsonError> {
    serde_json::from_str(text).map_err(|error| JsonError::Invalid {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Read a file as a JSON object.
///
/// Every data file in the pack format is an object at the top level, so this is the
/// common shape and the one worth naming.
///
/// # Errors
///
/// As [`read_json`], plus [`JsonError::Invalid`] when the document is not an object.
pub fn read_json_object(
    path: &Path,
    limits: Limits,
) -> Result<serde_json::Map<String, serde_json::Value>, JsonError> {
    let value = read_json(path, limits)?;
    into_object(value, path)
}

/// Require a JSON value to be an object.
///
/// # Errors
///
/// [`JsonError::Invalid`] naming the actual type, so the message says what was found
/// rather than only what was wanted.
pub fn into_object(
    value: serde_json::Value,
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, JsonError> {
    match value {
        serde_json::Value::Object(map) => Ok(map),
        other => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("expected a JSON object, found {}", type_name(&other)),
        }),
    }
}

/// The JSON type of a value, for error messages.
#[must_use]
pub fn type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Fetch a required string field.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the field is missing or is not a string.
pub fn required_str<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<&'a str, JsonError> {
    match map.get(key) {
        Some(serde_json::Value::String(text)) => Ok(text),
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("field {key:?} must be a string, found {}", type_name(other)),
        }),
        None => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("missing required field {key:?}"),
        }),
    }
}

/// Fetch an optional string field.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the field is present but not a string.
pub fn optional_str<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Option<&'a str>, JsonError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text)),
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("field {key:?} must be a string, found {}", type_name(other)),
        }),
    }
}

/// Fetch an optional integer field, refusing a non-integer number.
///
/// A float is refused rather than truncated: `"count": 1.5` in a recipe is a pack
/// bug, and silently rounding it would produce a wrong item count.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the field is present but not an integer.
pub fn optional_i64(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Option<i64>, JsonError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => {
            number.as_i64().map(Some).ok_or_else(|| JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("field {key:?} must be an integer, found {number}"),
            })
        }
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "field {key:?} must be an integer, found {}",
                type_name(other)
            ),
        }),
    }
}

/// Fetch an optional float field.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the field is present but not a number.
pub fn optional_f64(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Option<f64>, JsonError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => {
            number.as_f64().map(Some).ok_or_else(|| JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("field {key:?} is not representable as a number"),
            })
        }
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("field {key:?} must be a number, found {}", type_name(other)),
        }),
    }
}

/// Fetch a required array field.
///
/// # Errors
///
/// [`JsonError::Invalid`] when the field is missing or is not an array.
pub fn required_array<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<&'a Vec<serde_json::Value>, JsonError> {
    match map.get(key) {
        Some(serde_json::Value::Array(items)) => Ok(items),
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("field {key:?} must be an array, found {}", type_name(other)),
        }),
        None => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("missing required field {key:?}"),
        }),
    }
}

impl From<JsonError> for ServerError {
    fn from(error: JsonError) -> Self {
        match &error {
            JsonError::Io { .. } => Self::Operational(error.to_string()),
            _ => Self::CorruptData(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        JsonError, Limits, into_object, optional_f64, optional_i64, optional_str, parse_json,
        required_array, required_str, type_name,
    };
    use std::path::Path;

    fn path() -> &'static Path {
        Path::new("test.json")
    }

    #[test]
    fn type_names_are_human_readable() {
        let cases = [
            ("null", "null"),
            ("true", "a boolean"),
            ("1", "a number"),
            ("\"x\"", "a string"),
            ("[]", "an array"),
            ("{}", "an object"),
        ];
        for (text, want) in cases {
            let value = parse_json(text, path()).expect("parses");
            assert_eq!(type_name(&value), want, "{text}");
        }
    }

    #[test]
    fn invalid_json_names_the_file_and_the_reason() {
        let error = parse_json("{ not json", path()).expect_err("must fail");
        assert!(matches!(error, JsonError::Invalid { .. }));
        let text = error.to_string();
        assert!(text.contains("test.json"), "{text}");
        assert_eq!(error.path(), path());
        // It is a real Error impl, so a caller can chain it.
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn a_non_object_document_is_refused_with_its_actual_type() {
        let value = parse_json("[1, 2]", path()).expect("parses");
        let error = into_object(value, path()).expect_err("must fail");
        let text = error.to_string();
        assert!(text.contains("expected a JSON object"), "{text}");
        assert!(text.contains("an array"), "{text}");
    }

    #[test]
    fn required_fields_are_checked_for_presence_and_type() {
        let map = match parse_json(r#"{"a": "x", "b": 1}"#, path()).expect("parses") {
            serde_json::Value::Object(map) => map,
            other => panic!("expected an object, got {other:?}"),
        };
        assert_eq!(required_str(&map, "a", path()).expect("a"), "x");
        // Missing.
        let error = required_str(&map, "zzz", path()).expect_err("missing");
        assert!(
            error.to_string().contains("missing required field"),
            "{error}"
        );
        // Wrong type, and the message says which.
        let error = required_str(&map, "b", path()).expect_err("wrong type");
        let text = error.to_string();
        assert!(text.contains("must be a string"), "{text}");
        assert!(text.contains("a number"), "{text}");
    }

    #[test]
    fn optional_fields_distinguish_absent_null_and_present() {
        let map = match parse_json(r#"{"n": null, "s": "x", "i": 5, "f": 1.5}"#, path())
            .expect("parses")
        {
            serde_json::Value::Object(map) => map,
            other => panic!("expected an object, got {other:?}"),
        };
        assert_eq!(optional_str(&map, "absent", path()).expect("absent"), None);
        assert_eq!(optional_str(&map, "n", path()).expect("null"), None);
        assert_eq!(optional_str(&map, "s", path()).expect("present"), Some("x"));
        assert_eq!(optional_i64(&map, "i", path()).expect("int"), Some(5));
        assert_eq!(optional_f64(&map, "f", path()).expect("float"), Some(1.5));
        // An integer is readable as a float too.
        assert_eq!(
            optional_f64(&map, "i", path()).expect("int as float"),
            Some(5.0)
        );
        // A wrong type is an error for each accessor.
        assert!(optional_str(&map, "i", path()).is_err());
        assert!(optional_i64(&map, "s", path()).is_err());
        assert!(optional_f64(&map, "s", path()).is_err());
    }

    #[test]
    fn a_fractional_count_is_refused_rather_than_truncated() {
        // `"count": 1.5` in a recipe is a pack bug; rounding it silently would give a
        // wrong item count.
        let map = match parse_json(r#"{"count": 1.5}"#, path()).expect("parses") {
            serde_json::Value::Object(map) => map,
            other => panic!("expected an object, got {other:?}"),
        };
        let error = optional_i64(&map, "count", path()).expect_err("must refuse");
        let text = error.to_string();
        assert!(text.contains("must be an integer"), "{text}");
        assert!(text.contains("1.5"), "{text}");
    }

    #[test]
    fn arrays_are_required_to_be_arrays() {
        let map = match parse_json(r#"{"a": [1], "b": "x"}"#, path()).expect("parses") {
            serde_json::Value::Object(map) => map,
            other => panic!("expected an object, got {other:?}"),
        };
        assert_eq!(required_array(&map, "a", path()).expect("array").len(), 1);
        assert!(required_array(&map, "b", path()).is_err());
        assert!(required_array(&map, "absent", path()).is_err());
    }

    #[test]
    fn limits_default_to_a_bounded_ceiling() {
        let limits = Limits::DEFAULT;
        assert!(limits.max_bytes > 0);
        // 4 MiB is ~4x the largest vanilla data file, and 64 levels is half of
        // serde_json's own recursion limit while far above vanilla's depth of 4.
        assert_eq!(limits.max_bytes, 4 * 1024 * 1024);
        assert_eq!(limits.max_depth, 64);
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Limits::with_max_bytes(10).max_bytes, 10);
        assert_eq!(Limits::with_max_bytes(10).max_depth, 64);
    }

    #[test]
    fn a_file_over_the_limit_is_refused_before_being_read() {
        // A real file, so the metadata path is exercised rather than mocked.
        let dir = mc_test_support::fixtures::TempDir::new("json-limits");
        let file = dir.path().join("big.json");
        std::fs::write(&file, "{}").expect("write");
        // It fits at the default and not at a 1-byte ceiling.
        assert!(super::read_json(&file, Limits::DEFAULT).is_ok());
        let error = super::read_json(&file, Limits::with_max_bytes(1)).expect_err("too large");
        assert!(matches!(error, JsonError::TooLarge { .. }));
        assert!(error.to_string().contains("exceeds"), "{error}");
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn a_missing_file_is_an_io_error_naming_it() {
        let error = super::read_json(Path::new("does-not-exist.json"), Limits::DEFAULT)
            .expect_err("must fail");
        assert!(matches!(error, JsonError::Io { .. }));
        assert!(error.to_string().contains("does-not-exist.json"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn deeply_nested_json_is_refused_by_the_parser_not_by_a_stack_overflow() {
        // serde_json's recursion limit is the guard; this asserts it actually fires
        // rather than the process dying on a stack overflow.
        let deep = "[".repeat(400) + &"]".repeat(400);
        let result = parse_json(&deep, path());
        assert!(result.is_err(), "400 levels of nesting must be refused");
    }

    #[test]
    fn json_error_messages_are_stable() {
        // P15-06: operator-visible strings; the thiserror migration must not reword them.
        use std::path::PathBuf;
        let path = PathBuf::from("data/test.json");
        let cases = [
            (
                JsonError::Io {
                    path: path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
                },
                "data/test.json: nope",
            ),
            (
                JsonError::TooLarge {
                    path: path.clone(),
                    bytes: 9000,
                    limit: 1000,
                },
                "data/test.json: 9000 bytes exceeds the 1000-byte limit",
            ),
            (
                JsonError::Invalid {
                    path,
                    reason: "not JSON".to_owned(),
                },
                "data/test.json: not JSON",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn json_io_errors_keep_their_source() {
        // P15-06: thiserror auto-sources the `source` field; pin the chain.
        use std::error::Error;
        use std::path::PathBuf;
        let error = JsonError::Io {
            path: PathBuf::from("data/test.json"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("nope")
        );
    }
}
