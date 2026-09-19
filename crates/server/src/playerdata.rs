//! `playerdata/<uuid>.dat`: per-player persistence (P14-09 walk).
//!
//! Leaving used to forget everything but an in-memory copy, so a disconnect
//! within one run was survivable and a restart started fresh at spawn. This
//! module closes the second half with vanilla's own file: the gzip'd,
//! unnamed-root player compound (`Player::to_nbt` / `Player::from_nbt`),
//! written atomically beside the world.
//!
//! Boundaries, stated:
//!
//! - a missing file is not an error (first join has nothing to load);
//! - a corrupt file is `CorruptData` to the caller, and the game turns that
//!   into a fresh spawn with a loud warn — refusing the join would strand
//!   the player with no recourse on a headless Pi, while vanilla itself
//!   resets rather than bricks;
//! - a stored non-positive health rejoins fresh at spawn (disconnecting
//!   while dead is a respawn, not a resurrection);
//! - the write happens on leave; autosave ticks do not rewrite playerdata,
//!   so a crash between autosaves can lose minutes of player state the same
//!   way vanilla's periodic player save can.

use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use std::path::{Path, PathBuf};

/// Subdirectory of the world holding one `<uuid>.dat` per player, matching Vanilla.
pub const PLAYERDATA_DIR: &str = "playerdata";

/// Cap on a decoded player file (1 MiB): a real player compound is a few KiB,
/// so anything larger is a decompression bomb, not a player.
pub const PLAYERDATA_LIMIT: usize = 1024 * 1024;

/// Path of a player's file. The uuid comes from the authenticated profile
/// (hex and dashes only), never from chat input, so no traversal is possible.
#[must_use]
pub fn path_for(world_dir: &Path, uuid: &str) -> PathBuf {
    world_dir.join(PLAYERDATA_DIR).join(format!("{uuid}.dat"))
}

/// Write a player's compound atomically (tmp + rename, no backup copy: the
/// previous revision has no value once the new one is staged).
///
/// # Errors
///
/// [`ServerError::Operational`] when the directory cannot be created or the
/// atomic write fails; [`ServerError::Invariant`] when the tag cannot be
/// encoded (server-authored data).
pub fn save(world_dir: &Path, uuid: &str, tag: &NbtTag) -> ServerResult<()> {
    std::fs::create_dir_all(world_dir.join(PLAYERDATA_DIR)).map_err(|error| {
        ServerError::Operational(format!("cannot create playerdata dir: {error}"))
    })?;
    // Unnamed root, gzip container — vanilla's playerdata layout. (The
    // `save` helpers only do the *named* `level.dat` shape, so the two lines
    // live here rather than behind a wrongly-shared helper.)
    let mut raw = Vec::new();
    mc_nbt::write_unnamed(tag, &mut raw)?;
    let bytes = mc_persistence::compression::Compression::Gzip.compress(&raw)?;
    mc_persistence::save::write_atomic(&path_for(world_dir, uuid), &bytes, false)
}

/// Load a player's compound, or `None` when no file exists yet.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the file is present but does not decode
/// (bad gzip, truncated NBT, wrong root); [`ServerError::Operational`] when
/// the file cannot be read.
pub fn load(world_dir: &Path, uuid: &str) -> ServerResult<Option<NbtTag>> {
    let path = path_for(world_dir, uuid);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| ServerError::Operational(format!("cannot read {uuid}.dat: {error}")))?;
    Ok(Some(decode_player_bytes(&bytes, &path)?))
}

/// Decode player file bytes: gzip container holding the unnamed compound,
/// with a decompression cap so a hostile file cannot inflate memory.
fn decode_player_bytes(bytes: &[u8], path: &Path) -> ServerResult<NbtTag> {
    let raw = mc_persistence::compression::Compression::Gzip.decompress(bytes, PLAYERDATA_LIMIT)?;
    let tag = mc_nbt::read_unnamed(&raw, mc_nbt::Limits::DISK)
        .map_err(|error| ServerError::CorruptData(format!("{}: {error}", path.display())))?;
    if tag.entries().is_none() {
        return Err(ServerError::CorruptData(format!(
            "{}: player root is not a compound",
            path.display()
        )));
    }
    Ok(tag)
}

/// Peek a spawn position out of a decoded player compound: the `Pos` double
/// triple, or `None` when it is missing or malformed (the caller then falls
/// back to the world spawn and the full decode reports the problem).
#[must_use]
pub fn peek_pos(tag: &NbtTag) -> Option<mc_world::Vec3> {
    let entries = tag.entries()?;
    let (_, pos) = entries.iter().find(|(key, _)| key == "Pos")?;
    let mc_nbt::NbtTag::List(items) = pos else {
        return None;
    };
    let mut coords = items.iter().map(mc_nbt::NbtTag::as_f64);
    let (x, y, z) = (coords.next()??, coords.next()??, coords.next()??);
    Some(mc_world::Vec3::new(x, y, z))
}

#[cfg(test)]
mod tests {
    use super::{load, path_for, save};
    use mc_nbt::NbtTag;

    #[test]
    fn missing_file_loads_as_nothing() {
        let dir = std::env::temp_dir().join("mc-playerdata-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        assert!(
            load(&dir, "00000000-0000-0000-0000-000000000000")
                .expect("missing is not an error")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_is_uuid_dot_dat_under_playerdata() {
        let root = std::path::Path::new("/srv/minecraft/world");
        assert_eq!(
            path_for(root, "069a79f4-44e9-4726-a5be-fca90e38aaf5"),
            root.join("playerdata")
                .join("069a79f4-44e9-4726-a5be-fca90e38aaf5.dat")
        );
    }

    #[test]
    fn save_then_load_round_trips_a_compound() {
        let dir = std::env::temp_dir().join("mc-playerdata-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let uuid = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        let tag = NbtTag::Compound(vec![
            ("Health".to_owned(), NbtTag::Float(8.0)),
            (
                "Pos".to_owned(),
                NbtTag::List(vec![
                    NbtTag::Double(100.5),
                    NbtTag::Double(70.0),
                    NbtTag::Double(-40.5),
                ]),
            ),
        ]);
        save(&dir, uuid, &tag).expect("saves");
        assert!(
            path_for(&dir, uuid).is_file(),
            "the file lands as <uuid>.dat under playerdata/"
        );
        assert_eq!(
            load(&dir, uuid).expect("loads"),
            Some(tag),
            "what comes back is what was written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_file_is_corrupt_not_missing() {
        let dir = std::env::temp_dir().join("mc-playerdata-garbage");
        let _ = std::fs::remove_dir_all(&dir);
        let uuid = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        std::fs::create_dir_all(dir.join("playerdata")).expect("scratch dir");
        std::fs::write(dir.join("playerdata").join(format!("{uuid}.dat")), b"nope")
            .expect("garbage");
        let err = load(&dir, uuid).expect_err("garbage must fail");
        assert!(
            matches!(err, mc_core::error::ServerError::CorruptData(_)),
            "garbage is corruption, not a missing file: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
