//! Chunk payload compression adapters (P03-05).
//!
//! A region file stores, per chunk, `[u32 length][u8 compression id][payload]`
//! where `length` counts the compression byte plus the payload. This module owns
//! the mapping between compression ids and codecs, and the decompression-bomb
//! defence.
//!
//! Measured ids (`RegionFileVersion` constants in the 26.1.2 server jar):
//!
//! | id | name | read | write | note |
//! |---|---|---|---|---|
//! | 1 | `gzip` | yes | yes | legacy pre-1.15 worlds |
//! | 2 | `deflate` | yes | yes | **Vanilla default** (`VERSION_DEFLATE`) |
//! | 3 | `none` | yes | yes | `region-file-compression=none` |
//! | 4 | `lz4` | no | no | refused explicitly, see below |
//! | 127 | `custom` | no | no | datapack-provided codec |
//!
//! LZ4 and custom codecs are **not implemented**. Vanilla only produces them when
//! an operator opts in via `server.properties` or a datapack, so the default
//! interaction path is unaffected; a world that uses them is refused with
//! [`ServerError::Operational`] naming the codec instead of being misread.
//! Recorded in the parity matrix as a known gap.

use flate2::Compression as Flate2Level;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use mc_core::error::{ServerError, ServerResult};
use std::io::{Read, Write};

/// Default maximum decompressed chunk size (16 MiB).
///
/// A real 26.1.2 chunk measured 2.9 KiB..147 KiB; the cap exists to turn a
/// decompression bomb into a clean [`ServerError::CorruptData`] instead of an
/// allocation failure.
pub const DEFAULT_PAYLOAD_LIMIT: usize = 16 * 1024 * 1024;

/// Compression codec of a stored chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Compression {
    /// id 1: gzip container (legacy).
    Gzip,
    /// id 2: raw zlib stream (Vanilla default).
    Zlib,
    /// id 3: payload stored verbatim.
    None,
    /// id 4: LZ4 block stream (recognised, not implemented).
    Lz4,
    /// id 127: datapack codec (recognised, not implemented).
    Custom,
}

impl Compression {
    /// Region-file compression id.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Gzip => 1,
            Self::Zlib => 2,
            Self::None => 3,
            Self::Lz4 => 4,
            Self::Custom => 127,
        }
    }

    /// Map a compression id to a codec, if the id is known.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::Gzip),
            2 => Some(Self::Zlib),
            3 => Some(Self::None),
            4 => Some(Self::Lz4),
            127 => Some(Self::Custom),
            _ => None,
        }
    }

    /// Vanilla's `region-file-compression` option name for this codec.
    #[must_use]
    pub const fn option_name(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zlib => "deflate",
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Custom => "custom",
        }
    }

    /// Whether this build can decode the codec.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Gzip | Self::Zlib | Self::None)
    }

    /// Decompress a chunk payload, refusing anything larger than `limit`.
    ///
    /// # Errors
    ///
    /// - [`ServerError::Operational`] for a recognised but unimplemented codec;
    /// - [`ServerError::CorruptData`] when the stream is invalid or the decoded
    ///   size exceeds `limit` (the decompression-bomb guard).
    pub fn decompress(self, payload: &[u8], limit: usize) -> ServerResult<Vec<u8>> {
        match self {
            Self::None => {
                if payload.len() > limit {
                    return Err(too_large(self, payload.len(), limit));
                }
                Ok(payload.to_vec())
            }
            Self::Zlib => read_limited(ZlibDecoder::new(payload), self, limit),
            Self::Gzip => read_limited(GzDecoder::new(payload), self, limit),
            Self::Lz4 | Self::Custom => Err(ServerError::Operational(format!(
                "region compression {:?} (id {}) is not implemented; \
                 re-save the world with region-file-compression=deflate",
                self.option_name(),
                self.id()
            ))),
        }
    }

    /// Compress a chunk payload.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] for a codec that cannot be written.
    pub fn compress(self, raw: &[u8]) -> ServerResult<Vec<u8>> {
        match self {
            Self::None => Ok(raw.to_vec()),
            Self::Zlib => {
                let mut encoder = ZlibEncoder::new(Vec::new(), Flate2Level::new(6));
                finish(encoder.write_all(raw).and_then(|()| encoder.finish()), self)
            }
            Self::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), Flate2Level::new(6));
                finish(encoder.write_all(raw).and_then(|()| encoder.finish()), self)
            }
            Self::Lz4 | Self::Custom => Err(ServerError::Operational(format!(
                "region compression {:?} (id {}) cannot be written by this build",
                self.option_name(),
                self.id()
            ))),
        }
    }
}

/// Codec used for chunks this server writes: Vanilla's default (`deflate`).
pub const DEFAULT_WRITE: Compression = Compression::Zlib;

fn read_limited<R: Read>(reader: R, codec: Compression, limit: usize) -> ServerResult<Vec<u8>> {
    let mut out = Vec::new();
    // `take` caps the work *and* the allocation: one byte past the limit is
    // enough to detect an oversized payload without decoding all of it.
    let mut limited = reader.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1));
    limited.read_to_end(&mut out).map_err(|e| {
        ServerError::CorruptData(format!("{} chunk payload: {e}", codec.option_name()))
    })?;
    if out.len() > limit {
        return Err(too_large(codec, out.len(), limit));
    }
    Ok(out)
}

fn finish(result: std::io::Result<Vec<u8>>, codec: Compression) -> ServerResult<Vec<u8>> {
    result.map_err(|e| {
        ServerError::Operational(format!("{} compression failed: {e}", codec.option_name()))
    })
}

fn too_large(codec: Compression, size: usize, limit: usize) -> ServerError {
    ServerError::CorruptData(format!(
        "{} chunk payload decodes to {size} bytes, over the {limit} byte limit",
        codec.option_name()
    ))
}

#[cfg(test)]
mod tests {
    use super::{Compression, DEFAULT_WRITE};
    use mc_core::error::ServerError;

    #[test]
    fn ids_match_the_vanilla_region_file_version_table() {
        assert_eq!(Compression::Gzip.id(), 1);
        assert_eq!(Compression::Zlib.id(), 2);
        assert_eq!(Compression::None.id(), 3);
        assert_eq!(Compression::Lz4.id(), 4);
        assert_eq!(Compression::Custom.id(), 127);
        for codec in [
            Compression::Gzip,
            Compression::Zlib,
            Compression::None,
            Compression::Lz4,
            Compression::Custom,
        ] {
            assert_eq!(Compression::from_id(codec.id()), Some(codec));
        }
        assert_eq!(Compression::from_id(0), None);
        assert_eq!(Compression::from_id(5), None);
        assert_eq!(DEFAULT_WRITE, Compression::Zlib, "vanilla default");
    }

    #[test]
    fn round_trip_for_every_writable_codec() {
        let raw: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        for codec in [Compression::Gzip, Compression::Zlib, Compression::None] {
            let packed = codec.compress(&raw).expect("compresses");
            let unpacked = codec
                .decompress(&packed, super::DEFAULT_PAYLOAD_LIMIT)
                .expect("decompresses");
            assert_eq!(unpacked, raw, "{codec:?} round trip");
        }
    }

    #[test]
    fn zlib_and_gzip_actually_shrink_repetitive_data() {
        let raw = vec![0xABu8; 100_000];
        for codec in [Compression::Gzip, Compression::Zlib] {
            let packed = codec.compress(&raw).expect("compresses");
            assert!(packed.len() < raw.len() / 10, "{codec:?} should compress");
        }
    }

    #[test]
    fn decompression_bomb_is_rejected_without_decoding_it_all() {
        // 1 MiB of zeros compresses to about 1 KiB; a 64 KiB limit must reject it.
        let raw = vec![0u8; 1024 * 1024];
        let packed = Compression::Zlib.compress(&raw).expect("compresses");
        assert!(packed.len() < 4096, "bomb must be small on disk");
        let err = Compression::Zlib
            .decompress(&packed, 64 * 1024)
            .expect_err("must be rejected");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
        // The same payload is fine when the limit allows it.
        assert_eq!(
            Compression::Zlib
                .decompress(&packed, 2 * 1024 * 1024)
                .expect("within limit")
                .len(),
            raw.len()
        );
    }

    #[test]
    fn uncompressed_payload_respects_the_limit() {
        let raw = vec![7u8; 100];
        assert!(Compression::None.decompress(&raw, 100).is_ok());
        let err = Compression::None
            .decompress(&raw, 99)
            .expect_err("over limit");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    }

    #[test]
    fn corrupt_streams_are_errors_not_panics() {
        for codec in [Compression::Zlib, Compression::Gzip] {
            for bad in [
                &b""[..],
                &b"not compressed at all"[..],
                &[0x78, 0x9c, 0xff, 0xff, 0xff][..],
            ] {
                let result = codec.decompress(bad, super::DEFAULT_PAYLOAD_LIMIT);
                assert!(result.is_err(), "{codec:?} should reject {bad:?}");
            }
        }
    }

    #[test]
    fn unimplemented_codecs_are_refused_explicitly() {
        for codec in [Compression::Lz4, Compression::Custom] {
            assert!(!codec.is_supported());
            let err = codec.decompress(&[1, 2, 3], 1024).expect_err("unsupported");
            assert!(matches!(err, ServerError::Operational(_)), "{err:?}");
            assert!(
                format!("{err}").contains("not implemented"),
                "error must say what is missing: {err}"
            );
            assert!(codec.compress(&[1, 2, 3]).is_err());
        }
    }

    #[test]
    fn a_truncated_zlib_stream_is_detected() {
        let raw = vec![3u8; 5000];
        let packed = Compression::Zlib.compress(&raw).expect("compresses");
        let truncated = &packed[..packed.len() / 2];
        assert!(
            Compression::Zlib
                .decompress(truncated, 1024 * 1024)
                .is_err()
        );
    }
}
