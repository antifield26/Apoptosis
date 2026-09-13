//! Vanilla block-state and item registry lookup (P04-01).
//!
//! Since 1.13 the client identifies blocks by a **numeric global block-state id**
//! on the wire (`level_chunk_with_light` palettes, `block_update`), while the
//! *disk* format identifies them by name + properties. Bridging the two is what
//! this crate does, and it is a hard requirement for chunk streaming: an id that
//! does not match the client's registry renders the wrong block.
//!
//! ## Where the table comes from
//!
//! Not from a reference implementation and not hand-written:
//! `target/vanilla-26.1.2/reports/DumpRegistries.java` boots the **official
//! 26.1.2 server jar's own registry** (`SharedConstants.tryDetectVersion()` +
//! `Bootstrap.bootStrap()`, the same calls the dedicated server makes) and walks
//! `Block.BLOCK_STATE_REGISTRY`, whose index *is* the wire id. The dump is then
//! compressed into `crates/test-support/fixtures/registry/blocks.tsv` by
//! `target/vanilla-26.1.2/compact_blocks.py`, which **verifies every one of the
//! 29 873 states round-trips to its exact id** before writing the file.
//!
//! The table is read at startup, not compiled in, so the same binary works
//! against a refreshed table when the version moves (P07 replaces this with
//! data-driven loading from an operator-supplied jar).
//!
//! ## Limits
//!
//! - 1 168 blocks / 29 873 states / 1 506 items — the *entire* vanilla registry,
//!   not a curated subset. A vanilla world chunk can therefore be streamed
//!   without dropping blocks.
//! - Unknown names produce [`ServerError::CorruptData`] rather than a silent
//!   fallback to air: a wrong block is worse than a refused chunk.

#![forbid(unsafe_code)]
// Registry ids are indices: they are `usize` while a table is being built and
// `i32` on the wire (`VarInt` block-state ids). Every conversion site is bounded
// by a table length that is far below `i32::MAX` (29 873 states), and the same
// exemption is documented in the other binary-format crates.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

mod blocks;
mod items;
pub mod light;

pub use blocks::{BlockRegistry, BlockStateRef};
pub use items::ItemRegistry;
pub use light::LightTable;

use mc_core::error::{ServerError, ServerResult};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Registry data shipped with the test-support fixtures.
///
/// Resolved relative to the `mc-registry` crate so it works from any workspace
/// member (same contract as [`mc_test_support::fixtures`]).
pub const FIXTURE_DIR: &str = "crates/test-support/fixtures/registry";

/// Both registries, loaded from a directory containing `blocks.tsv` and
/// `items.tsv`.
#[derive(Debug, Clone)]
pub struct Registries {
    /// Block states.
    pub blocks: BlockRegistry,
    /// Items.
    pub items: ItemRegistry,
    /// Per-state light properties, consumed by the light engine in `mc-world`.
    pub light: LightTable,
}

impl Registries {
    /// Load both tables from `dir`.
    ///
    /// # Errors
    ///
    /// [`mc_core::error::ServerError::Operational`] when a file is missing or
    /// unreadable, [`mc_core::error::ServerError::CorruptData`] when its contents
    /// are malformed.
    pub fn load(dir: &Path) -> ServerResult<Self> {
        // The block registry first: the light table is indexed by state id, so the registry is what sizes it.
        let blocks = BlockRegistry::load(&dir.join("blocks.tsv"))?;
        let state_count = blocks.state_count();
        Ok(Self {
            items: ItemRegistry::load(&dir.join("items.tsv"))?,
            light: LightTable::load(&dir.join("block_light.tsv"), state_count)?,
            blocks,
        })
    }

    /// Load the vanilla tables shipped in this repository.
    ///
    /// Search order (first directory holding `blocks.tsv` wins):
    /// `$MC_FIXTURE_DIR` (operator override), `fixtures/registry` next to the
    /// running executable (the deployed layout — `/srv/mc-server/fixtures/registry`),
    /// the build tree two levels above the executable (a binary run in-place
    /// under `target/release`), and finally the compiled-in workspace path
    /// (dev builds and tests).
    ///
    /// # Errors
    ///
    /// As for [`Registries::load`], or [`ServerError::Operational`] naming every
    /// directory tried when no candidate holds the tables.
    pub fn vanilla() -> ServerResult<Self> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let override_dir = std::env::var("MC_FIXTURE_DIR").ok();
        let mut tried = String::new();
        for dir in candidate_dirs(exe_dir.as_deref(), override_dir.as_deref()) {
            if dir.join("blocks.tsv").is_file() {
                return Self::load(&dir);
            }
            let _ = write!(tried, "\n  tried {}", dir.display());
        }
        Err(ServerError::Operational(format!(
            "registry fixtures not found;{tried}"
        )))
    }
}

/// The search list behind [`Registries::vanilla`], exposed for testing.
/// `override_dir` is the `$MC_FIXTURE_DIR` value, passed in so the precedence
/// is testable without touching process-global state (Audit 08, M2).
fn candidate_dirs(exe_dir: Option<&Path>, override_dir: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = override_dir {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = exe_dir {
        dirs.push(exe_dir.join("fixtures/registry"));
        dirs.push(exe_dir.join("../../crates/test-support/fixtures/registry"));
    }
    dirs.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(FIXTURE_DIR),
    );
    dirs
}

#[cfg(test)]
mod tests {
    use super::{FIXTURE_DIR, candidate_dirs};
    use std::path::Path;

    // The Pi acceptance run (P09) applied deploy/mc-server.service for the first
    // time and the server could not start: the compiled-in path pointed back
    // into the build tree, unreadable by the service user. This pins the order
    // that makes the deployed layout win before the build-tree fallback.
    #[test]
    fn the_fixture_search_prefers_the_executable_side_over_the_build_tree() {
        let dirs = candidate_dirs(Some(Path::new("/srv/mc-server")), None);
        assert_eq!(
            dirs[0],
            Path::new("/srv/mc-server/fixtures/registry"),
            "the deployed layout must be searched first"
        );
        assert!(
            dirs.iter().any(|d| d.ends_with(FIXTURE_DIR)),
            "the compiled-in path must remain the last resort: {dirs:?}"
        );
    }

    #[test]
    fn the_fixture_search_works_without_an_executable() {
        let dirs = candidate_dirs(None, None);
        assert_eq!(dirs.len(), 1, "no exe means only the compiled-in path");
        assert!(dirs[0].ends_with(FIXTURE_DIR));
    }

    // Audit 08 (M2): the `$MC_FIXTURE_DIR` override had no coverage at all —
    // the branch existed but nothing read it. This pins the precedence: the
    // operator override is searched before everything else.
    #[test]
    fn the_operator_override_is_searched_first() {
        let dirs = candidate_dirs(Some(Path::new("/srv/mc-server")), Some("/data/registry"));
        assert_eq!(dirs[0], Path::new("/data/registry"));
        assert_eq!(dirs[1], Path::new("/srv/mc-server/fixtures/registry"));
    }
}
