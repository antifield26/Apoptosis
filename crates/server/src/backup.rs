//! World backup and restore (P08-05).
//!
//! The server already keeps `level.dat_old` beside `level.dat` on every save,
//! but that is crash recovery for one file, not a backup: it covers neither the
//! region files nor an operator error. This module is the documented,
//! operator-facing half:
//!
//! - [`backup_world`] copies a world directory to `<backup_dir>/<name>` while
//!   the server is **stopped**, and writes a [`BackupManifest`] naming what was
//!   copied and when;
//! - [`restore_world`] copies a backup back over a world directory, refusing to
//!   overwrite a world that has changed since the backup unless the operator
//!   passes `overwrite = true`;
//! - [`verify_backup`] re-reads a backup the way the server would open it.
//!
//! Three deliberate limits, stated rather than hidden:
//!
//! 1. **Offline only.** A backup taken while the server runs can catch a region
//!    file mid-flush. Nothing here locks the world; the runbook tells the
//!    operator to `systemctl stop mc-server` first, and the manifest records the
//!    server version so a backup from another build is visible.
//! 2. **Whole-directory copy.** Selective backup (only dirty regions) would need
//!    the save journal this build does not keep. Disk is cheap; correctness is
//!    not the place to optimise.
//! 3. **No compression.** A plain directory copy is verifiable with `ls` and
//!    restorable with `cp -r`. A compressed archive that fails halfway leaves
//!    nothing usable; a copy that fails halfway leaves a manifest that does not
//!    verify, which is the honest signal.

use mc_core::error::{ServerError, ServerResult};
use std::path::{Path, PathBuf};

/// Manifest file written into every backup directory.
pub const MANIFEST_FILE: &str = "mc-backup.json";

/// Server version recorded in the manifest.
pub const SERVER_VERSION: &str = crate::config::VERSION_NAME;

/// What a backup contains, as recorded at backup time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupManifest {
    /// World directory the backup was taken from, as given.
    pub world_dir: PathBuf,
    /// Unix milliseconds the backup finished.
    pub taken_millis: i64,
    /// Server version that wrote it.
    pub server_version: String,
    /// Files copied, relative to the world directory, sorted.
    pub files: Vec<String>,
}

impl BackupManifest {
    /// Read a manifest written by [`backup_world`].
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the file is missing or not a manifest.
    pub fn load(backup_dir: &Path) -> ServerResult<Self> {
        let path = backup_dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            ServerError::CorruptData(format!("cannot read {}: {e}", path.display()))
        })?;
        Self::parse(&text, &path)
    }

    /// Parse manifest text, naming `path` in errors.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the shape is wrong.
    pub fn parse(text: &str, path: &Path) -> ServerResult<Self> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            ServerError::CorruptData(format!("{} is not valid JSON: {e}", path.display()))
        })?;
        let world_dir = value
            .get("world_dir")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServerError::CorruptData(format!("{} has no world_dir", path.display()))
            })?;
        let taken_millis = value
            .get("taken_millis")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                ServerError::CorruptData(format!("{} has no taken_millis", path.display()))
            })?;
        let server_version = value
            .get("server_version")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ServerError::CorruptData(format!("{} has no server_version", path.display()))
            })?;
        let files = value
            .get("files")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ServerError::CorruptData(format!("{} has no files", path.display())))?;
        let mut files: Vec<String> = files
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        files.sort();
        Ok(Self {
            world_dir: PathBuf::from(world_dir),
            taken_millis,
            server_version: server_version.to_owned(),
            files,
        })
    }

    /// Render for [`MANIFEST_FILE`].
    #[must_use]
    pub fn render(&self) -> String {
        serde_json::json!({
            "world_dir": self.world_dir.to_string_lossy(),
            "taken_millis": self.taken_millis,
            "server_version": self.server_version,
            "files": self.files,
        })
        .to_string()
    }
}

/// Copy a stopped world to `<backup_dir>`, and write the manifest.
///
/// Creates `<backup_dir>` when absent; refuses when it already holds a
/// manifest, so a backup never silently merges into an older one.
///
/// # Errors
///
/// [`ServerError::Operational`] when the world cannot be read or the backup
/// cannot be written.
pub fn backup_world(world_dir: &Path, backup_dir: &Path) -> ServerResult<BackupManifest> {
    if !world_dir.is_dir() {
        return Err(ServerError::Operational(format!(
            "world directory {} does not exist",
            world_dir.display()
        )));
    }
    if backup_dir.join(MANIFEST_FILE).exists() {
        return Err(ServerError::Operational(format!(
            "{} already holds a backup; remove it first",
            backup_dir.display()
        )));
    }
    std::fs::create_dir_all(backup_dir).map_err(|e| {
        ServerError::Operational(format!("cannot create {}: {e}", backup_dir.display()))
    })?;
    let files = copy_tree(world_dir, backup_dir)?;
    let manifest = BackupManifest {
        world_dir: world_dir.to_path_buf(),
        taken_millis: mc_persistence::save::unix_millis(std::time::SystemTime::now()),
        server_version: SERVER_VERSION.to_owned(),
        files,
    };
    std::fs::write(backup_dir.join(MANIFEST_FILE), manifest.render()).map_err(|e| {
        ServerError::Operational(format!(
            "cannot write {}: {e}",
            backup_dir.join(MANIFEST_FILE).display()
        ))
    })?;
    tracing::info!(
        world = %world_dir.display(),
        backup = %backup_dir.display(),
        files = manifest.files.len(),
        "world backup complete"
    );
    Ok(manifest)
}

/// Copy a backup back over a world directory.
///
/// Refuses when the world holds files the backup does not know about unless
/// `overwrite` is set: restoring over a newer world without saying so is
/// exactly the data loss a backup exists to prevent.
///
/// # Errors
///
/// [`ServerError::Operational`] on I/O failure; [`ServerError::CorruptData`]
/// when the backup has no manifest.
pub fn restore_world(
    backup_dir: &Path,
    world_dir: &Path,
    overwrite: bool,
) -> ServerResult<BackupManifest> {
    let manifest = BackupManifest::load(backup_dir)?;
    if !overwrite && let Some(stray) = stray_files(world_dir, &manifest)? {
        return Err(ServerError::Operational(format!(
            "{} holds {stray}, which the backup does not contain; pass overwrite to replace it",
            world_dir.display()
        )));
    }
    std::fs::create_dir_all(world_dir).map_err(|e| {
        ServerError::Operational(format!("cannot create {}: {e}", world_dir.display()))
    })?;
    let _ = copy_tree(backup_dir, world_dir)?;
    // The manifest must not leak into the live world: it describes the backup,
    // and a second backup taken later would otherwise inherit stale file lists.
    let _ = std::fs::remove_file(world_dir.join(MANIFEST_FILE));
    tracing::info!(
        backup = %backup_dir.display(),
        world = %world_dir.display(),
        files = manifest.files.len(),
        "world restore complete"
    );
    Ok(manifest)
}

/// Re-read a backup the way the server would open it.
///
/// Checks the manifest parses, every listed file exists, and `level.dat`
/// decodes with a supported `DataVersion`. A backup that verifies is one the
/// server can open; a backup that does not is reported with the reason.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the backup is incomplete or unreadable.
pub fn verify_backup(backup_dir: &Path) -> ServerResult<BackupManifest> {
    let manifest = BackupManifest::load(backup_dir)?;
    let mut missing = Vec::new();
    for file in &manifest.files {
        if !backup_dir.join(file).is_file() {
            missing.push(file.clone());
        }
    }
    if !missing.is_empty() {
        return Err(ServerError::CorruptData(format!(
            "{} is missing {} file(s): {}",
            backup_dir.display(),
            missing.len(),
            missing.join(", ")
        )));
    }
    // `level.dat` is the one file the server cannot boot without.
    if manifest.files.iter().any(|file| file == "level.dat") {
        let bytes = std::fs::read(backup_dir.join("level.dat")).map_err(|e| {
            ServerError::CorruptData(format!("cannot read the backup's level.dat: {e}"))
        })?;
        let tag = mc_persistence::save::decode_gzip_nbt(&bytes)?;
        let level = mc_persistence::level::LevelDat::from_nbt(&tag)?;
        if !mc_persistence::level::LevelDat::is_supported_data_version(level.data_version) {
            return Err(ServerError::CorruptData(format!(
                "the backup's level.dat has DataVersion {}, outside the supported window",
                level.data_version
            )));
        }
    }
    Ok(manifest)
}

/// Copy every file under `from` to `to`, returning sorted relative paths.
///
/// The manifest file itself is never copied: it belongs to the backup
/// directory, not to the world it describes.
fn copy_tree(from: &Path, to: &Path) -> ServerResult<Vec<String>> {
    let mut files = Vec::new();
    copy_tree_inner(from, from, to, &mut files)?;
    files.sort();
    Ok(files)
}

fn copy_tree_inner(
    root: &Path,
    from: &Path,
    to: &Path,
    files: &mut Vec<String>,
) -> ServerResult<()> {
    let entries = std::fs::read_dir(from)
        .map_err(|e| ServerError::Operational(format!("cannot list {}: {e}", from.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            ServerError::Operational(format!("cannot list {}: {e}", from.display()))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| {
            ServerError::Operational(format!(
                "{} is not under {}",
                path.display(),
                root.display()
            ))
        })?;
        if relative.as_os_str() == MANIFEST_FILE {
            continue;
        }
        let target = to.join(relative);
        let file_type = entry.file_type().map_err(|e| {
            ServerError::Operational(format!("cannot stat {}: {e}", path.display()))
        })?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| {
                ServerError::Operational(format!("cannot create {}: {e}", target.display()))
            })?;
            copy_tree_inner(root, &path, to, files)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ServerError::Operational(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::copy(&path, &target).map_err(|e| {
                ServerError::Operational(format!(
                    "cannot copy {} to {}: {e}",
                    path.display(),
                    target.display()
                ))
            })?;
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Files in the live world that the backup does not know about, if any.
fn stray_files(world_dir: &Path, manifest: &BackupManifest) -> ServerResult<Option<String>> {
    if !world_dir.is_dir() {
        return Ok(None);
    }
    let mut live = Vec::new();
    collect_files(world_dir, world_dir, &mut live)?;
    let known: std::collections::BTreeSet<&str> =
        manifest.files.iter().map(String::as_str).collect();
    live.sort();
    let stray: Vec<&str> = live
        .iter()
        .map(String::as_str)
        .filter(|file| *file != MANIFEST_FILE && !known.contains(file))
        .collect();
    Ok(stray.first().map(|file| (*file).to_owned()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> ServerResult<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ServerError::Operational(format!("cannot list {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| ServerError::Operational(format!("cannot list {}: {e}", dir.display())))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| {
            ServerError::Operational(format!("cannot stat {}: {e}", path.display()))
        })?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| {
                ServerError::Operational(format!(
                    "{} is not under {}",
                    path.display(),
                    root.display()
                ))
            })?;
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MANIFEST_FILE, backup_world, restore_world, verify_backup};
    use mc_test_support::fixtures::TempDir;

    fn world_with_level(tag: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new(tag);
        let world = dir.path().join("world");
        let config = crate::config::StorageConfig {
            world_dir: world.clone(),
            autosave_ticks: 0,
        };
        let service = crate::storage::WorldService::open(&config).expect("world opens");
        service.close().expect("closes");
        assert!(world.join("level.dat").is_file(), "a world has level.dat");
        (dir, world)
    }

    #[test]
    fn backup_then_verify_then_restore_round_trips() {
        let (_dir, world) = world_with_level("backup-round-trip");
        let backup_root = TempDir::new("backup-dest");
        let backup = backup_root.path().join("backup-1");
        let manifest = backup_world(&world, &backup).expect("backup");
        assert!(
            manifest.files.contains(&"level.dat".to_owned()),
            "the manifest names level.dat: {:?}",
            manifest.files
        );
        assert!(backup.join(MANIFEST_FILE).is_file());
        let verified = verify_backup(&backup).expect("verifies");
        assert_eq!(verified.files, manifest.files);

        // Restore over a fresh directory.
        let target_root = TempDir::new("backup-restore");
        let target = target_root.path().join("world");
        let restored = restore_world(&backup, &target, false).expect("restore");
        assert_eq!(restored.files, manifest.files);
        assert!(target.join("level.dat").is_file());
        assert!(
            !target.join(MANIFEST_FILE).exists(),
            "the manifest must not leak into the live world"
        );
        // The restored world opens: the files are not just present but valid.
        let config = crate::config::StorageConfig {
            world_dir: target.clone(),
            autosave_ticks: 0,
        };
        crate::storage::WorldService::open(&config).expect("restored world opens");
    }

    #[test]
    fn restore_refuses_to_overwrite_a_changed_world_without_overwrite() {
        let (_dir, world) = world_with_level("backup-guard-source");
        let backup_root = TempDir::new("backup-guard-backup");
        let backup = backup_root.path().join("backup-1");
        backup_world(&world, &backup).expect("backup");

        // The world gains a file the backup never saw.
        std::fs::write(world.join("playerdata-note.txt"), b"newer").expect("write");
        let err = restore_world(&backup, &world, false).expect_err("must refuse");
        assert!(
            err.to_string().contains("overwrite"),
            "the refusal names the remedy: {err}"
        );
        // With the flag it proceeds.
        restore_world(&backup, &world, true).expect("overwrite restores");
    }

    #[test]
    fn a_backup_into_an_existing_backup_is_refused() {
        let (_dir, world) = world_with_level("backup-twice");
        let backup_root = TempDir::new("backup-twice-dest");
        let backup = backup_root.path().join("backup-1");
        backup_world(&world, &backup).expect("first backup");
        let err = backup_world(&world, &backup).expect_err("second backup must refuse");
        assert!(err.to_string().contains("already holds a backup"), "{err}");
    }

    #[test]
    fn verify_reports_a_backup_with_a_missing_file() {
        let (_dir, world) = world_with_level("backup-missing");
        let backup_root = TempDir::new("backup-missing-dest");
        let backup = backup_root.path().join("backup-1");
        let manifest = backup_world(&world, &backup).expect("backup");
        assert!(manifest.files.contains(&"level.dat".to_owned()));
        std::fs::remove_file(backup.join("level.dat")).expect("remove");
        let err = verify_backup(&backup).expect_err("must report the hole");
        assert!(err.to_string().contains("level.dat"), "{err}");
    }

    #[test]
    fn backup_of_a_missing_world_is_an_error_not_an_empty_backup() {
        let dir = TempDir::new("backup-nowhere");
        let err = backup_world(
            &dir.path().join("does-not-exist"),
            &dir.path().join("backup"),
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("does not exist"), "{err}");
    }
}
