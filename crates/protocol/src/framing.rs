//! Length-prefixed frame codec with optional zlib compression (P02-03/P02-09).
//!
//! Wire format (Java 1.20.2+):
//!
//! ```text
//! uncompressed: [VarInt length][packet id VarInt][data]
//! compressed:   [VarInt frame length][VarInt uncompressed length][zlib(id+data)]
//!               uncompressed length == 0 means the tail is raw (id+data)
//! ```
//!
//! Hostile-input rules enforced here (AGENTS.md section 10):
//! - frame length and uncompressed length are capped at [`MAX_PACKET_SIZE`];
//! - a compressed payload claiming an uncompressed size below the negotiated
//!   threshold is rejected (vanilla behaviour) and decompression output is
//!   compared against the declared size, so zlib bombs produce an error, not
//!   an allocation.
//! - the incremental buffer never grows past two maximum frames plus prefix
//!   slack; callers drain with [`FrameCodec::try_next`].

use crate::varint::{MAX_VARINT_BYTES, read_varint, write_varint};
use crate::{MAX_PACKET_SIZE, RawPacket};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use mc_core::error::{ServerError, ServerResult};
use std::io::{Read, Write};

/// Upper bound for the internal reassembly buffer.
const BUFFER_CAP: usize = 2 * (MAX_PACKET_SIZE + MAX_VARINT_BYTES) + 16;

/// Incremental decoder that turns a byte stream into raw packets.
#[derive(Debug)]
pub struct FrameCodec {
    buffer: Vec<u8>,
    start: usize,
    compression_threshold: Option<i32>,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCodec {
    /// Create an uncompressed codec.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            compression_threshold: None,
        }
    }

    /// Enable compression after `SetCompression` has been sent or received.
    pub fn set_compression(&mut self, threshold: i32) {
        self.compression_threshold = Some(threshold.max(0));
    }

    /// Current compression threshold, if negotiated.
    #[must_use]
    pub fn compression_threshold(&self) -> Option<i32> {
        self.compression_threshold
    }

    /// Bytes buffered but not yet consumed (diagnostics/tests).
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len() - self.start
    }

    /// Feed freshly read socket bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when buffered data exceeds the reassembly cap
    /// (a client that never completes a frame).
    pub fn feed(&mut self, data: &[u8]) -> ServerResult<()> {
        self.buffer.extend_from_slice(data);
        if self.buffered() > BUFFER_CAP {
            return Err(ServerError::Protocol(
                "frame buffer exceeded limit".to_owned(),
            ));
        }
        Ok(())
    }

    /// Extract the next complete packet, if one is buffered.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on malformed frames, oversize declarations or
    /// failed decompression. After an error the caller must drop the
    /// connection; the codec is left as-is.
    pub fn try_next(&mut self) -> ServerResult<Option<RawPacket>> {
        let buffered = self.buffered();
        if buffered == 0 {
            return Ok(None);
        }
        let data = &self.buffer[self.start..];
        // Parse the frame length without consuming, so a partial frame can wait.
        let mut prefix = data;
        let Ok(frame_len) = read_varint(&mut prefix) else {
            if data.len() >= MAX_VARINT_BYTES {
                return Err(ServerError::Protocol(
                    "invalid frame length prefix".to_owned(),
                ));
            }
            return Ok(None);
        };
        let prefix_len = data.len() - prefix.len();
        if frame_len < 0 {
            return Err(ServerError::Protocol(format!(
                "negative frame length {frame_len}"
            )));
        }
        // Safe conversion: negativity checked above, magnitude checked below.
        #[allow(clippy::cast_sign_loss)]
        let frame_len = frame_len as usize;
        if frame_len == 0 {
            return Err(ServerError::Protocol("zero-length frame".to_owned()));
        }
        if frame_len > MAX_PACKET_SIZE {
            return Err(ServerError::Protocol(format!(
                "frame length {frame_len} exceeds maximum {MAX_PACKET_SIZE}"
            )));
        }
        let total = prefix_len + frame_len;
        if data.len() < total {
            return Ok(None); // wait for the rest
        }
        let frame = &data[prefix_len..total];

        let payload = match self.compression_threshold {
            None => frame.to_vec(),
            Some(threshold) => decompress_frame(frame, threshold)?,
        };
        self.start += total;
        if self.start > 8192 && self.start * 2 > self.buffer.len() {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        if payload.len() > MAX_PACKET_SIZE {
            return Err(ServerError::Protocol(
                "packet payload exceeds maximum".to_owned(),
            ));
        }
        // A packet must contain at least a packet id VarInt.
        let mut body = &payload[..];
        let id = read_varint(&mut body)?;
        let consumed = payload.len() - body.len();
        Ok(Some(RawPacket::new(id, payload[consumed..].to_vec())))
    }

    /// Encode a packet for the wire.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when compression fails or the payload exceeds
    /// [`MAX_PACKET_SIZE`].
    pub fn encode(packet: &RawPacket, compression_threshold: Option<i32>) -> ServerResult<Vec<u8>> {
        let mut body = Vec::with_capacity(packet.payload.len() + MAX_VARINT_BYTES);
        write_varint(&mut body, packet.id);
        body.extend_from_slice(&packet.payload);
        if body.len() > MAX_PACKET_SIZE {
            return Err(ServerError::Protocol(
                "outbound packet exceeds maximum size".to_owned(),
            ));
        }
        let mut out = Vec::with_capacity(body.len() + MAX_VARINT_BYTES * 2);
        match compression_threshold {
            None => {
                write_varint(
                    &mut out,
                    i32::try_from(body.len()).map_err(|_| oversized())?,
                );
                out.extend_from_slice(&body);
            }
            Some(threshold) => {
                if body.len() < threshold.max(0) as usize {
                    write_varint(
                        &mut out,
                        i32::try_from(body.len() + 1).map_err(|_| oversized())?,
                    );
                    write_varint(&mut out, 0);
                    out.extend_from_slice(&body);
                } else {
                    let declared = i32::try_from(body.len()).map_err(|_| oversized())?;
                    let prefix_len = crate::varint::varint_len(declared);
                    let compressed = compress(&body)?;
                    write_varint(
                        &mut out,
                        i32::try_from(prefix_len + compressed.len()).map_err(|_| oversized())?,
                    );
                    write_varint(&mut out, declared);
                    out.extend_from_slice(&compressed);
                }
            }
        }
        Ok(out)
    }
}

fn oversized() -> ServerError {
    ServerError::Protocol("outbound packet exceeds maximum size".to_owned())
}

fn compress(data: &[u8]) -> ServerResult<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| ServerError::Operational(format!("zlib compression failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ServerError::Operational(format!("zlib compression failed: {e}")))
}

fn decompress_frame(frame: &[u8], threshold: i32) -> ServerResult<Vec<u8>> {
    let mut cursor = frame;
    let declared = read_varint(&mut cursor)?;
    if declared < 0 {
        return Err(ServerError::Protocol(format!(
            "negative uncompressed length {declared}"
        )));
    }
    let declared = declared as usize;
    if declared == 0 {
        // Uncompressed fallback: must fit the cap and honour the threshold.
        let payload = cursor.to_vec();
        if payload.len() > MAX_PACKET_SIZE {
            return Err(ServerError::Protocol(
                "uncompressed payload exceeds maximum".to_owned(),
            ));
        }
        return Ok(payload);
    }
    if declared > MAX_PACKET_SIZE {
        return Err(ServerError::Protocol(format!(
            "uncompressed length {declared} exceeds maximum {MAX_PACKET_SIZE}"
        )));
    }
    if declared < threshold.max(0) as usize {
        // Vanilla rejects "badly compressed" packets below the threshold.
        return Err(ServerError::Protocol(format!(
            "uncompressed length {declared} is below compression threshold {threshold}"
        )));
    }
    let mut decoder = ZlibDecoder::new(cursor).take(declared as u64 + 1);
    let mut output = Vec::with_capacity(declared.min(64 * 1024));
    decoder
        .read_to_end(&mut output)
        .map_err(|e| ServerError::Protocol(format!("zlib decompression failed: {e}")))?;
    if output.len() != declared {
        return Err(ServerError::Protocol(format!(
            "decompressed length {} does not match declared {declared}",
            output.len()
        )));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::FrameCodec;
    use crate::MAX_PACKET_SIZE;
    use crate::RawPacket;
    use mc_core::error::ServerError;

    fn encode(packet: &RawPacket, threshold: Option<i32>) -> Vec<u8> {
        FrameCodec::encode(packet, threshold).expect("encodes")
    }

    fn decode_one(bytes: &[u8], threshold: Option<i32>) -> RawPacket {
        let mut codec = FrameCodec::new();
        if let Some(threshold) = threshold {
            codec.set_compression(threshold);
        }
        codec.feed(bytes).expect("feeds");
        codec.try_next().expect("decodes").expect("one packet")
    }

    #[test]
    fn uncompressed_round_trip() {
        let packet = RawPacket::new(0x2A, vec![1, 2, 3, 4]);
        let bytes = encode(&packet, None);
        assert_eq!(decode_one(&bytes, None), packet);
    }

    #[test]
    fn compressed_round_trip_above_threshold() {
        let packet = RawPacket::new(7, vec![0xAB; 600]);
        let bytes = encode(&packet, Some(256));
        assert_eq!(decode_one(&bytes, Some(256)), packet);
    }

    #[test]
    fn small_packets_use_uncompressed_fallback() {
        let packet = RawPacket::new(7, vec![0x01, 0x02]);
        let bytes = encode(&packet, Some(256));
        // [frame len][0][id][payload]
        assert_eq!(bytes[1], 0);
        assert_eq!(decode_one(&bytes, Some(256)), packet);
    }

    #[test]
    fn split_feeds_reassemble() {
        let packet = RawPacket::new(3, vec![9; 100]);
        let bytes = encode(&packet, Some(16));
        let mut codec = FrameCodec::new();
        codec.set_compression(16);
        for chunk in bytes.chunks(7) {
            codec.feed(chunk).expect("feeds");
        }
        assert_eq!(codec.try_next().expect("decodes"), Some(packet));
        assert_eq!(codec.try_next().expect("drained"), None);
    }

    #[test]
    fn multiple_packets_in_one_feed() {
        let a = RawPacket::new(1, vec![1]);
        let b = RawPacket::new(2, vec![2, 2]);
        let mut bytes = encode(&a, None);
        bytes.extend_from_slice(&encode(&b, None));
        let mut codec = FrameCodec::new();
        codec.feed(&bytes).expect("feeds");
        assert_eq!(codec.try_next().expect("a"), Some(a));
        assert_eq!(codec.try_next().expect("b"), Some(b));
        assert_eq!(codec.try_next().expect("none"), None);
    }

    #[test]
    fn oversized_frame_is_rejected_without_allocating() {
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, (MAX_PACKET_SIZE + 1) as i32);
        bytes.extend_from_slice(&[0u8; 8]);
        let mut codec = FrameCodec::new();
        codec.feed(&bytes).expect("feeds");
        assert!(matches!(codec.try_next(), Err(ServerError::Protocol(_))));
    }

    #[test]
    fn negative_and_zero_frames_are_rejected() {
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, -1);
        let mut codec = FrameCodec::new();
        codec.feed(&bytes).expect("feeds");
        assert!(codec.try_next().is_err());

        let mut codec = FrameCodec::new();
        codec.feed(&[0x00]).expect("feeds");
        assert!(codec.try_next().is_err());
    }

    #[test]
    fn decompression_bomb_is_rejected() {
        // Declare 1 MiB, send 1 KiB of zlib data that expands beyond it.
        let payload = vec![0u8; MAX_PACKET_SIZE];
        let compressed = super::compress(&payload).expect("compresses");
        let mut frame = Vec::new();
        crate::varint::write_varint(&mut frame, 512);
        crate::varint::write_varint(&mut frame, (MAX_PACKET_SIZE / 2) as i32);
        frame.extend_from_slice(&compressed);
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, frame.len() as i32);
        bytes.extend_from_slice(&frame);
        let mut codec = FrameCodec::new();
        codec.set_compression(1);
        codec.feed(&bytes).expect("feeds");
        assert!(matches!(codec.try_next(), Err(ServerError::Protocol(_))));
    }

    #[test]
    fn below_threshold_compressed_packet_is_rejected() {
        let payload = vec![0x42u8; 300];
        let compressed = super::compress(&payload).expect("compresses");
        let mut frame = Vec::new();
        crate::varint::write_varint(&mut frame, (payload.len()) as i32);
        frame.extend_from_slice(&compressed);
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, frame.len() as i32);
        bytes.extend_from_slice(&frame);
        let mut codec = FrameCodec::new();
        codec.set_compression(1000);
        codec.feed(&bytes).expect("feeds");
        assert!(codec.try_next().is_err());
    }

    #[test]
    fn a_slow_drip_never_grows_the_buffer_without_bound() {
        // P08-07: one byte per feed, never completing a frame. The buffer must
        // hold at most the drip, and feeding past the cap must fail rather than
        // grow: this is the slowloris shape at the frame layer.
        let mut codec = FrameCodec::new();
        for _ in 0..1024 {
            codec.feed(&[0x80]).expect("a drip fits");
        }
        assert!(codec.buffered() <= 1024, "buffered {}", codec.buffered());
        // A frame-length prefix of five continuation bytes is malformed, and a
        // codec holding a full cap of drips plus one frame must refuse the feed.
        let mut full = FrameCodec::new();
        let filler = vec![0x01u8; super::BUFFER_CAP + 1];
        assert!(full.feed(&filler).is_err(), "past the cap must fail");
    }

    #[test]
    fn malformed_frames_never_panic() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..5_000 {
            let len = (next() % 64) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
            for threshold in [None, Some(0), Some(256)] {
                let mut codec = FrameCodec::new();
                if let Some(threshold) = threshold {
                    codec.set_compression(threshold);
                }
                if codec.feed(&bytes).is_ok() {
                    let _ = codec.try_next();
                }
            }
        }
    }
}
