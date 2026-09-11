//! Minecraft Java Edition 26.1.2 (protocol 775) wire codec.
//!
//! `mc-protocol` owns packet types, varint/framing codecs and version-specific
//! wire semantics (ADR-0001 D-01/D-05). It does **not** own world state,
//! connections or gameplay: those live in `mc-network` and later crates.
//!
//! Scope for Phase 02:
//! - [`varint`]: VarInt/VarLong with hostile-input limits.
//! - [`wire`]: bounds-checked packet reader/writer primitives.
//! - [`framing`]: length-prefixed frames with optional zlib compression and
//!   decompression-bomb limits.
//! - [`nbt`]: network NBT (nameless root, modified UTF-8 strings), the subset
//!   needed by the configuration-phase registry payload.
//! - [`packets`]: typed packets for handshake, status, login, configuration
//!   and the Phase-02 play plumbing.
//!
//! Version policy: protocol **775 only** (26.1.x shares the number,
//! `docs/research/protocol-baseline.md` section 1). The display name is
//! `26.1.2` on status/config surfaces; the wire version string is `26.1`.

#![forbid(unsafe_code)]
// Wire code converts between integer widths constantly; every conversion site
// either bounds-checks the value first or is lossless by construction. The
// pedantic cast lints add noise here without catching real defects.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

pub mod framing;
pub mod ids;
pub mod nbt;
pub mod packet;
pub mod packets;
pub mod text;
pub mod varint;
pub mod wire;

pub use packet::RawPacket;

/// Maximum size of a single (decompressed) packet payload, in bytes.
///
/// Adopted from the reference protocol libraries
/// (`valence_protocol/src/lib.rs:79-80`) and enforced on both the compressed
/// frame length and the decompressed payload length.
pub const MAX_PACKET_SIZE: usize = 2_097_152;

/// Highest string length accepted for identifier strings, in bytes.
pub const MAX_IDENTIFIER_LEN: usize = 32_767;
