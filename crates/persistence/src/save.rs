//! Atomic and ordered save semantics (P03-13).
//!
//! Two guarantees, both testable:
//!
//! **1. `level.dat` is replaced atomically.** Vanilla keeps `level.dat` plus a
//! `level.dat_old` backup. The sequence here is
//!
//! ```text
//! write level.dat.tmp  ->  fsync tmp  ->  copy level.dat to level.dat_old
//!                      ->  rename tmp over level.dat  ->  fsync directory
//! ```
//!
//! so at every instant at least one complete `level.dat` exists on disk: before
//! the rename the old file is intact, after it the new one is. A crash between
//! the copy and the rename leaves the *old* file live and the *older* one in
//! `_old`, which is exactly Vanilla's recovery window.
//!
//! **2. Region writes are ordered so the header entry is the commit point.**
//! [`crate::region::RegionFile::write_chunk`] writes the payload, then the
//! timestamp, then the location word. An interrupted save can lose a new chunk
//! but cannot make an old one unreadable. [`save_region_files`] then fsyncs each
//! touched file once, instead of once per chunk.
//!
//! Failure is never silent: every failed unit is recorded in
//! [`SaveReport::errors`] and remains dirty so the next flush retries it
//! (AGENTS.md section 3.3, "no fake completeness").

use mc_core::error::{ServerError, ServerResult};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::warn;

/// gzip magic bytes; `level.dat` is always a gzip-compressed NBT document.
const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];

/// Append `.tmp` to a path (the atomic-replacement staging name).
#[must_use]
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Append `_old` to a path (Vanilla's backup name, e.g. `level.dat_old`).
#[must_use]
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("_old");
    PathBuf::from(name)
}

/// Read a gzip-compressed NBT document (`level.dat`).
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the file is not gzip or does not decompress.
pub fn decode_gzip_nbt(bytes: &[u8]) -> ServerResult<mc_nbt::NbtTag> {
    if bytes.len() < GZIP_MAGIC.len() || bytes[..2] != GZIP_MAGIC {
        return Err(ServerError::CorruptData(
            "level.dat is not gzip-compressed".to_owned(),
        ));
    }
    let raw =
        crate::compression::Compression::Gzip.decompress(bytes, mc_nbt::Limits::DISK.max_bytes)?;
    let (_, tag) = mc_nbt::read_named(&raw, mc_nbt::Limits::DISK)?;
    Ok(tag)
}

/// Encode an NBT document the way `level.dat` is stored (gzip).
///
/// # Errors
///
/// [`ServerError::Operational`] when encoding or compression fails.
pub fn encode_gzip_nbt(name: &str, tag: &mc_nbt::NbtTag) -> ServerResult<Vec<u8>> {
    let mut raw = Vec::new();
    mc_nbt::write_named(name, tag, &mut raw)?;
    crate::compression::Compression::Gzip.compress(&raw)
}

/// Write `bytes` to `path` atomically, keeping the previous content in
/// `path_old`.
///
/// # Errors
///
/// [`ServerError::Operational`] when the temporary file cannot be written or the
/// rename fails. In that case `path` still holds its previous content.
pub fn write_atomic(path: &Path, bytes: &[u8], keep_backup: bool) -> ServerResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            ServerError::Operational(format!("cannot create {}: {e}", parent.display()))
        })?;
    }
    let temp = temp_path(path);
    {
        let mut file = fs::File::create(&temp).map_err(|e| {
            ServerError::Operational(format!("cannot create {}: {e}", temp.display()))
        })?;
        file.write_all(bytes).map_err(|e| {
            ServerError::Operational(format!("cannot write {}: {e}", temp.display()))
        })?;
        // Durability point: the staged file must be complete before it can
        // replace the live one.
        file.sync_all()
            .map_err(|e| ServerError::Operational(format!("fsync {}: {e}", temp.display())))?;
    }

    if keep_backup && path.exists() {
        let backup = backup_path(path);
        // Copy rather than rename: a rename would leave no `level.dat` at all
        // if the following rename failed.
        fs::copy(path, &backup).map_err(|e| {
            ServerError::Operational(format!(
                "cannot back up {} to {}: {e}",
                path.display(),
                backup.display()
            ))
        })?;
        if let Ok(file) = fs::File::open(&backup) {
            let _ = file.sync_all();
        }
    }

    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        ServerError::Operational(format!(
            "cannot move {} into place at {}: {e}",
            temp.display(),
            path.display()
        ))
    })?;
    sync_directory(path.parent());
    Ok(())
}

/// Best-effort directory fsync so the rename itself survives a power loss.
///
/// Not supported on every platform (Windows cannot fsync a directory handle);
/// a failure is logged at debug level rather than treated as a save failure.
fn sync_directory(dir: Option<&Path>) {
    let Some(dir) = dir else {
        return;
    };
    match fs::File::open(dir) {
        Ok(handle) => {
            if let Err(e) = handle.sync_all() {
                tracing::debug!(directory = %dir.display(), error = %e, "directory fsync unavailable");
            }
        }
        Err(e) => {
            tracing::debug!(directory = %dir.display(), error = %e, "directory fsync unavailable");
        }
    }
}

/// Outcome counters for one flush.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveReport {
    /// Chunks written successfully.
    pub chunks_written: usize,
    /// Chunks whose write failed (they stay dirty).
    pub chunks_failed: usize,
    /// Region files synced.
    pub regions_synced: usize,
    /// Whether `level.dat` was written.
    pub level_written: bool,
    /// Whether chunks were skipped because their data was unchanged.
    pub chunks_skipped: usize,
    /// Human-readable failures, newest last.
    pub errors: Vec<String>,
}

impl SaveReport {
    /// Whether the flush had no failures.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.chunks_failed == 0
    }

    /// Whether nothing at all was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks_written == 0
            && self.chunks_failed == 0
            && self.regions_synced == 0
            && !self.level_written
            && self.chunks_skipped == 0
    }

    /// Fold another report into this one.
    pub fn merge(&mut self, other: Self) {
        self.chunks_written += other.chunks_written;
        self.chunks_failed += other.chunks_failed;
        self.regions_synced += other.regions_synced;
        self.chunks_skipped += other.chunks_skipped;
        self.level_written |= other.level_written;
        self.errors.extend(other.errors);
    }

    /// Convert a failed report into an error, for callers that treat any
    /// failure as fatal.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the report contains failures.
    pub fn into_result(self) -> ServerResult<Self> {
        if self.is_clean() {
            Ok(self)
        } else {
            Err(ServerError::Operational(format!(
                "save completed with {} failure(s): {}",
                self.errors.len(),
                self.errors.join("; ")
            )))
        }
    }
}

/// How many seconds since the Unix epoch, for region timestamps.
///
/// A clock before 1970 (or a platform clock error) yields 0 rather than a
/// negative timestamp, which is what vanilla stores for "unknown".
#[must_use]
pub fn unix_seconds(now: std::time::SystemTime) -> i32 {
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i32::try_from(duration.as_secs()).unwrap_or(i32::MAX),
        Err(e) => {
            warn!(error = %e, "system clock is before the Unix epoch");
            0
        }
    }
}

/// Milliseconds since the Unix epoch, for `level.dat`'s `LastPlayed`.
#[must_use]
pub fn unix_millis(now: std::time::SystemTime) -> i64 {
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SaveReport, backup_path, decode_gzip_nbt, encode_gzip_nbt, temp_path, unix_millis,
        unix_seconds, write_atomic,
    };
    use mc_core::error::ServerError;
    use mc_nbt::NbtTag;
    use mc_test_support::fixtures::TempDir;
    use std::path::Path;

    #[test]
    fn atomic_write_keeps_a_backup_and_leaves_no_temp_file() {
        let dir = TempDir::new("save-atomic");
        let path = dir.path().join("level.dat");
        write_atomic(&path, b"first", true).expect("first write");
        assert_eq!(std::fs::read(&path).expect("read"), b"first");
        assert!(
            !backup_path(&path).exists(),
            "no backup before the first save"
        );

        write_atomic(&path, b"second", true).expect("second write");
        assert_eq!(std::fs::read(&path).expect("read"), b"second");
        assert_eq!(
            std::fs::read(backup_path(&path)).expect("backup"),
            b"first",
            "the previous version is preserved as level.dat_old"
        );
        assert!(!temp_path(&path).exists(), "staging file must be gone");
    }

    #[test]
    fn atomic_write_without_backup() {
        let dir = TempDir::new("save-nobackup");
        let path = dir.path().join("world_gen_settings.dat");
        write_atomic(&path, b"one", false).expect("write");
        write_atomic(&path, b"two", false).expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn atomic_write_creates_missing_directories() {
        let dir = TempDir::new("save-mkdir");
        let path = dir
            .path()
            .join("data")
            .join("minecraft")
            .join("weather.dat");
        write_atomic(&path, b"rain", true).expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"rain");
    }

    #[test]
    fn failed_write_leaves_the_previous_file_intact_and_reports() {
        let dir = TempDir::new("save-fail");
        let path = dir.path().join("level.dat");
        write_atomic(&path, b"good", true).expect("write");
        // Occupy the temp path with a directory so creating the staging file fails.
        std::fs::create_dir(temp_path(&path)).expect("create blocking dir");
        let err = write_atomic(&path, b"never", true).expect_err("must fail");
        assert!(matches!(err, ServerError::Operational(_)), "{err:?}");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"good",
            "live file must be untouched"
        );
    }

    #[test]
    fn gzip_nbt_round_trip() {
        let tag = NbtTag::Compound(vec![(
            "Data".to_owned(),
            NbtTag::Compound(vec![("DataVersion".to_owned(), NbtTag::Int(4790))]),
        )]);
        let bytes = encode_gzip_nbt("", &tag).expect("encodes");
        assert_eq!(&bytes[..2], &[0x1F, 0x8B], "gzip magic");
        assert_eq!(decode_gzip_nbt(&bytes).expect("decodes"), tag);
    }

    #[test]
    fn non_gzip_level_data_is_rejected() {
        let err = decode_gzip_nbt(b"plain nbt, not gzipped").expect_err("must fail");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
        assert!(decode_gzip_nbt(&[]).is_err());
    }

    #[test]
    fn report_cleanliness_and_result_conversion() {
        let mut report = SaveReport::default();
        assert!(report.is_clean() && report.is_empty());
        report.chunks_written = 3;
        assert!(report.is_clean() && !report.is_empty());
        assert!(report.clone().into_result().is_ok());

        report.chunks_failed = 1;
        report.errors.push("boom".to_owned());
        assert!(!report.is_clean());
        let err = report.into_result().expect_err("must fail");
        assert!(format!("{err}").contains("boom"));
    }

    #[test]
    fn report_merge_accumulates() {
        let mut first = SaveReport {
            chunks_written: 1,
            level_written: true,
            ..SaveReport::default()
        };
        let second = SaveReport {
            chunks_written: 2,
            chunks_failed: 1,
            regions_synced: 2,
            errors: vec!["x".to_owned()],
            ..SaveReport::default()
        };
        first.merge(second);
        assert_eq!(first.chunks_written, 3);
        assert_eq!(first.chunks_failed, 1);
        assert_eq!(first.regions_synced, 2);
        assert!(first.level_written);
        assert_eq!(first.errors, vec!["x".to_owned()]);
    }

    #[test]
    fn timestamp_helpers_are_saturating() {
        let epoch = std::time::UNIX_EPOCH;
        assert_eq!(unix_seconds(epoch), 0);
        assert_eq!(unix_millis(epoch), 0);
        let later = epoch + std::time::Duration::from_millis(1_500);
        assert_eq!(unix_seconds(later), 1);
        assert_eq!(unix_millis(later), 1_500);
        // Before the epoch (clock skew) clamps instead of going negative.
        let earlier = epoch - std::time::Duration::from_secs(10);
        assert_eq!(unix_seconds(earlier), 0);
        assert_eq!(unix_millis(earlier), 0);
    }

    #[test]
    fn temp_and_backup_paths_are_vanilla_named() {
        let path = Path::new("world/level.dat");
        assert_eq!(temp_path(path), Path::new("world/level.dat.tmp"));
        assert_eq!(backup_path(path), Path::new("world/level.dat_old"));
    }
}
