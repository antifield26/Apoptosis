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

pub use blocks::{BlockRegistry, BlockStateRef};
pub use items::ItemRegistry;

use mc_core::error::ServerResult;
use std::path::Path;

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
        Ok(Self {
            blocks: BlockRegistry::load(&dir.join("blocks.tsv"))?,
            items: ItemRegistry::load(&dir.join("items.tsv"))?,
        })
    }

    /// Load the vanilla tables shipped in this repository.
    ///
    /// # Errors
    ///
    /// As for [`Registries::load`].
    pub fn vanilla() -> ServerResult<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Self::load(&root.join(FIXTURE_DIR))
    }
}
