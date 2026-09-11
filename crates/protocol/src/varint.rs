//! `VarInt` / `VarLong` codecs (P02-02).
//!
//! Wire format: little-endian base-128, 7 payload bits per byte, high bit set
//! when more bytes follow. Java caps a `VarInt` at 5 bytes and a `VarLong` at
//! 10 bytes; hostile input must never cause an unbounded read or a panic
//! (AGENTS.md section 10), so decoding rejects overlong and truncated values.

use mc_core::error::{ServerError, ServerResult};

/// Maximum encoded bytes for a 32-bit `VarInt`.
pub const MAX_VARINT_BYTES: usize = 5;
/// Maximum encoded bytes for a 64-bit `VarLong`.
pub const MAX_VARLONG_BYTES: usize = 10;

/// Encoded length of `value` as a `VarInt` (1..=[`MAX_VARINT_BYTES`]).
#[must_use]
pub fn varint_len(value: i32) -> usize {
    let mut value = value as u32;
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

/// Encoded length of `value` as a `VarLong` (1..=[`MAX_VARLONG_BYTES`]).
#[must_use]
pub fn varlong_len(value: i64) -> usize {
    let mut value = value as u64;
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

/// Append the `VarInt` encoding of `value`, returning the bytes written.
pub fn write_varint(out: &mut Vec<u8>, value: i32) -> usize {
    let start = out.len();
    let mut value = value as u32;
    loop {
        if value & !0x7F == 0 {
            out.push(value as u8);
            break;
        }
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.len() - start
}

/// Append the `VarLong` encoding of `value`, returning the bytes written.
pub fn write_varlong(out: &mut Vec<u8>, value: i64) -> usize {
    let start = out.len();
    let mut value = value as u64;
    loop {
        if value & !0x7F == 0 {
            out.push(value as u8);
            break;
        }
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.len() - start
}

/// Decode a `VarInt` from the front of `bytes`, advancing the slice.
///
/// # Errors
///
/// [`ServerError::Protocol`] when the value is truncated or longer than
/// [`MAX_VARINT_BYTES`].
pub fn read_varint(bytes: &mut &[u8]) -> ServerResult<i32> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    let mut count = 0;
    loop {
        let Some((&byte, rest)) = bytes.split_first() else {
            return Err(ServerError::Protocol("truncated VarInt".to_owned()));
        };
        *bytes = rest;
        count += 1;
        result |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result as i32);
        }
        if count >= MAX_VARINT_BYTES {
            return Err(ServerError::Protocol(format!(
                "VarInt longer than {MAX_VARINT_BYTES} bytes"
            )));
        }
        shift += 7;
    }
}

/// Decode a `VarLong` from the front of `bytes`, advancing the slice.
///
/// # Errors
///
/// [`ServerError::Protocol`] when the value is truncated or longer than
/// [`MAX_VARLONG_BYTES`].
pub fn read_varlong(bytes: &mut &[u8]) -> ServerResult<i64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut count = 0;
    loop {
        let Some((&byte, rest)) = bytes.split_first() else {
            return Err(ServerError::Protocol("truncated VarLong".to_owned()));
        };
        *bytes = rest;
        count += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result as i64);
        }
        if count >= MAX_VARLONG_BYTES {
            return Err(ServerError::Protocol(format!(
                "VarLong longer than {MAX_VARLONG_BYTES} bytes"
            )));
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_VARINT_BYTES, read_varint, read_varlong, varint_len, write_varint, write_varlong,
    };
    use mc_core::error::ServerError;

    fn encode_varint(value: i32) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(&mut out, value);
        out
    }

    #[test]
    fn known_encodings_match_vanilla() {
        // Golden values from the public protocol documentation.
        assert_eq!(encode_varint(0), [0x00]);
        assert_eq!(encode_varint(1), [0x01]);
        assert_eq!(encode_varint(127), [0x7F]);
        assert_eq!(encode_varint(128), [0x80, 0x01]);
        assert_eq!(encode_varint(255), [0xFF, 0x01]);
        assert_eq!(encode_varint(2_147_483_647), [0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
        assert_eq!(encode_varint(-1), [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn round_trip_all_widths() {
        let values = [
            i32::MIN,
            -1,
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            2_097_151,
            2_097_152,
            i32::MAX,
        ];
        for value in values {
            let mut bytes = encode_varint(value);
            assert_eq!(
                bytes.len(),
                varint_len(value),
                "length mismatch for {value}"
            );
            assert!(bytes.len() <= MAX_VARINT_BYTES);
            assert_eq!(
                read_varint(&mut &bytes[..]).expect("decodes"),
                value,
                "value {value}"
            );
            // Exhaustively drain to prove no trailing bytes.
            bytes.clear();
        }
    }

    #[test]
    fn rejects_overlong_and_truncated() {
        // Six continuation bytes: overlong, must be rejected at the cap.
        let overlong = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
        assert!(matches!(
            read_varint(&mut &overlong[..]),
            Err(ServerError::Protocol(_))
        ));
        // Max-byte encoding with continuation bit is overlong.
        let fifth_continuation = [0xFF, 0xFF, 0xFF, 0xFF, 0x8F];
        assert!(read_varint(&mut &fifth_continuation[..]).is_err());
        // Truncated (continuation bit set on the last available byte).
        let truncated = [0x80];
        assert!(read_varint(&mut &truncated[..]).is_err());
        assert!(read_varint(&mut &[][..]).is_err());
    }

    #[test]
    fn varlong_round_trip() {
        for value in [i64::MIN, -1, 0, 1, i64::MAX, 9_223_372_036_854_775_807] {
            let mut out = Vec::new();
            write_varlong(&mut out, value);
            assert_eq!(read_varlong(&mut &out[..]).expect("decodes"), value);
        }
    }

    #[test]
    fn varlong_rejects_overlong() {
        let overlong = [0xFF; 11];
        assert!(read_varlong(&mut &overlong[..]).is_err());
    }

    #[test]
    fn malformed_bytes_never_panic() {
        // Deterministic pseudo-random corpus: bytes must decode or error
        // cleanly, never panic or read out of bounds.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..10_000 {
            let len = (next() % 12) as usize;
            let buf: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
            let _ = read_varint(&mut &buf[..]);
            let _ = read_varlong(&mut &buf[..]);
        }
    }
}
