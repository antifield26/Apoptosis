//! Fixture helpers: locate checked-in fixtures and compare bytes (P01-10).
//!
//! Layout contract (stable from Phase 01 on):
//!
//! ```text
//! crates/test-support/fixtures/<area>/<name>.bin    # golden bytes
//! crates/test-support/fixtures/<area>/<name>.json   # structured cases
//! ```
//!
//! Tests resolve paths via [`fixture_path`] (relative to this crate root, so
//! they work from any workspace member), read with [`read_fixture`], and
//! assert with [`assert_bytes_eq`] which prints a short hex context around
//! the first divergence instead of dumping megabytes.

use std::path::PathBuf;

/// Resolve `crates/test-support/fixtures/<area>/<name>`.
///
/// # Panics
///
/// Panics when the path cannot be constructed (programmer error in the test
/// itself, never player input).
#[must_use]
pub fn fixture_path(area: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(area)
        .join(name)
}

/// Read a fixture file into memory.
///
/// # Errors
///
/// Returns [`mc_core::error::ServerError::Operational`] when the file is
/// missing or unreadable.
pub fn read_fixture(area: &str, name: &str) -> mc_core::error::ServerResult<Vec<u8>> {
    let path = fixture_path(area, name);
    std::fs::read(&path).map_err(|e| {
        mc_core::error::ServerError::Operational(format!("missing fixture {}: {e}", path.display()))
    })
}

/// Read a `.hex` fixture and decode it to bytes.
///
/// Hex text keeps golden wire data diff-friendly in Git while staying exact.
/// `#` starts a comment to end of line; whitespace is ignored; non-hex
/// characters are an error.
///
/// # Errors
///
/// [`mc_core::error::ServerError::Operational`] when the file is missing,
/// unreadable, has an odd number of digits or contains non-hex characters.
pub fn read_hex_fixture(area: &str, name: &str) -> mc_core::error::ServerResult<Vec<u8>> {
    let path = fixture_path(area, name);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        mc_core::error::ServerError::Operational(format!("missing fixture {}: {e}", path.display()))
    })?;
    let compact: String = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect();
    if !compact.len().is_multiple_of(2) {
        return Err(mc_core::error::ServerError::Operational(format!(
            "fixture {} has an odd number of hex digits",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(compact.len() / 2);
    let mut chars = compact.chars();
    while let (Some(high), Some(low)) = (chars.next(), chars.next()) {
        let pair = format!("{high}{low}");
        let byte = u8::from_str_radix(&pair, 16).map_err(|_| {
            mc_core::error::ServerError::Operational(format!(
                "fixture {} contains non-hex data: {pair:?}",
                path.display()
            ))
        })?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Assert two byte strings are equal, with first-divergence context.
///
/// On mismatch the panic message shows the offset, up to 16 bytes of context
/// per side in hex, and both lengths — enough to triage golden failures
/// without flooding logs.
///
/// # Panics
///
/// Panics when `actual != expected`. Intended for tests only.
pub fn assert_bytes_eq(actual: &[u8], expected: &[u8]) {
    if actual == expected {
        return;
    }
    let first = actual
        .iter()
        .zip(expected.iter())
        .position(|(a, b)| a != b)
        .unwrap_or(actual.len().min(expected.len()));
    panic!(
        "fixture bytes differ at offset {first}: actual[..] {} vs expected[..] {} (lengths {} vs {})",
        hex_window(actual, first.min(actual.len())),
        hex_window(expected, first.min(expected.len())),
        actual.len(),
        expected.len()
    );
}

fn hex_window(bytes: &[u8], at: usize) -> String {
    let start = at.saturating_sub(8);
    let end = (at + 8).min(bytes.len());
    bytes[start..end]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A unique temporary directory for tests that touch the filesystem
/// (persistence restart tests in P03). Deleted on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a fresh temp dir under the OS temp area.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be created (broken test environment).
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mc-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&path).expect("test temp dir must be creatable");
        Self { path }
    }

    /// The directory path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::{TempDir, assert_bytes_eq, fixture_path};

    #[test]
    fn byte_compare_passes_on_equal_input() {
        assert_bytes_eq(&[1, 2, 3], &[1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "fixture bytes differ at offset 1")]
    fn byte_compare_reports_first_divergence() {
        assert_bytes_eq(&[0xAA, 0xBB, 0xCC], &[0xAA, 0x00, 0xCC]);
    }

    #[test]
    fn temp_dir_lifecycle() {
        let path = {
            let dir = TempDir::new("lifecycle");
            assert!(dir.path().is_dir());
            std::fs::write(dir.path().join("probe"), b"x").expect("writable");
            dir.path().to_owned()
        };
        assert!(!path.exists(), "TempDir must clean up on drop");
    }

    #[test]
    fn fixture_path_layout_contract() {
        let path = fixture_path("protocol", "handshake.bin");
        let text = path.to_string_lossy().replace('\\', "/");
        assert!(
            text.ends_with("crates/test-support/fixtures/protocol/handshake.bin"),
            "{text}"
        );
    }
}
