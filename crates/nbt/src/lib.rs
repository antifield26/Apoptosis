//! NBT (Named Binary Tag) model, reader and writer — P03-01..P03-04.
//!
//! One implementation serves both encodings Vanilla uses:
//!
//! | Encoding | Root tag name | Container | Used by |
//! |---|---|---|---|
//! | disk | present | gzip (level.dat) / zlib (region chunks) | [`read_named`], [`write_named`] |
//! | network | omitted ("nameless") | inside the packet frame | [`NbtTag::read_network`], [`NbtTag::write_network`] |
//!
//! Everything else is shared: tag ids, payload layout, and Java **modified
//! UTF-8** strings with a `u16` byte-length prefix.
//!
//! ## Evidence for modified UTF-8 on disk
//!
//! Vanilla writes NBT strings through `DataOutput.writeUTF`
//! (`net.minecraft.nbt.StringTag.write` bytecode → `DataOutput.writeUTF`), which
//! is Java's modified UTF-8, not standard UTF-8. Measured on the 26.1.2 runtime
//! in this project's evidence run (`target/vanilla-26.1.2`, JDK 25):
//!
//! ```text
//! writeUTF(\u0000)          = 00 02 C0 80
//! writeUTF(\u00E9)          = 00 02 C3 A9
//! writeUTF(\uD83D\uDE00)    = 00 06 ED A0 BD ED B8 80   (CESU-8 surrogate pair)
//! ```
//!
//! So a supplementary character costs two 3-byte sequences (a surrogate pair),
//! and NUL is `C0 80` — the same rules the network codec already implemented in
//! Phase 02, now shared with the disk path.
//!
//! ## Hostile input
//!
//! World files are attacker-influenced data (a server may be handed a world
//! directory by an operator, or a region file may be corrupted). The reader is
//! therefore bounded by [`Limits`]: recursion depth, total input bytes and total
//! tag count. Every collection length must also fit in the remaining input,
//! which stops an inflated `i32` count from triggering a large allocation.
//! Nothing in this crate panics on malformed input (AGENTS.md section 10).

#![forbid(unsafe_code)]
// The NBT format is a dense binary encoding: tag ids, lengths and payloads are
// constantly narrowed (u16 payload length, u8 tag ids, i8 bytes) and every
// conversion site either bounds-checks the value first or is lossless by
// construction. The pedantic cast lints would add noise without catching real
// defects, matching the exemption already documented in `mc-protocol`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

mod read;
mod string;
mod value;
mod write;

pub use read::{Limits, TagReader, read_named, read_unnamed};
pub use string::{read_modified_utf8, write_modified_utf8};
pub use value::{NbtTag, tag};
pub use write::{write_named, write_unnamed};

/// Re-exported for callers that match on failure classes.
pub use mc_core::error::{ServerError, ServerResult};
