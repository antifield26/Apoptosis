//! Java **modified UTF-8** string codec (P03-01).
//!
//! Shared by the disk and network encodings. Rules (verified against the JDK and
//! against `net.minecraft.nbt.StringTag.write` → `DataOutput.writeUTF`, see the
//! crate docs):
//!
//! - length prefix is a **`u16` byte count** of the encoded payload,
//! - NUL encodes as `C0 80` (never a bare `00`),
//! - characters outside the BMP encode as a CESU-8 surrogate pair (6 bytes),
//! - a payload longer than 65535 bytes cannot be represented at all; Vanilla
//!   throws `UTFDataFormatException`, and so do we ([`ServerError::Operational`]).

use mc_core::error::{ServerError, ServerResult};

/// Maximum encoded payload length (`u16` length prefix).
pub const MAX_ENCODED_LEN: usize = u16::MAX as usize;

/// Encode `value` as Java modified UTF-8 with a `u16` length prefix.
///
/// # Errors
///
/// [`ServerError::Operational`] when the encoded payload exceeds
/// [`MAX_ENCODED_LEN`]; truncating the length field instead would silently
/// corrupt the file, so this is a hard failure.
pub fn write_modified_utf8(value: &str, out: &mut Vec<u8>) -> ServerResult<()> {
    let start = out.len();
    // Reserve the length prefix, then backfill it once the size is known.
    out.extend_from_slice(&[0, 0]);
    encode_body(value, out);
    let len = out.len() - start - 2;
    if len > MAX_ENCODED_LEN {
        out.truncate(start);
        return Err(ServerError::Operational(format!(
            "NBT string encodes to {len} bytes, over the {MAX_ENCODED_LEN}-byte limit"
        )));
    }
    out[start..start + 2].copy_from_slice(&(len as u16).to_be_bytes());
    Ok(())
}

fn encode_body(value: &str, out: &mut Vec<u8>) {
    for unit in value.encode_utf16() {
        match unit {
            0x0000 => out.extend_from_slice(&[0xC0, 0x80]),
            0x0001..=0x007F => out.push(unit as u8),
            0x0080..=0x07FF => {
                out.push(0xC0 | ((unit >> 6) as u8));
                out.push(0x80 | ((unit & 0x3F) as u8));
            }
            _ => {
                out.push(0xE0 | ((unit >> 12) as u8));
                out.push(0x80 | (((unit >> 6) & 0x3F) as u8));
                out.push(0x80 | ((unit & 0x3F) as u8));
            }
        }
    }
}

/// Decode Java modified UTF-8 with a `u16` length prefix.
///
/// # Errors
///
/// [`ServerError::Protocol`] on truncation or malformed byte sequences,
/// including lone surrogates: Rust strings cannot hold an unpaired surrogate, so
/// such a name (only producible by non-Vanilla tooling) is rejected rather than
/// silently replaced.
pub fn read_modified_utf8(bytes: &mut &[u8]) -> ServerResult<String> {
    let len = read_u16(bytes)? as usize;
    let payload = take(bytes, len)?;
    decode(payload)
}

/// Decode a modified UTF-8 payload with no length prefix.
pub(crate) fn decode(payload: &[u8]) -> ServerResult<String> {
    let mut units: Vec<u16> = Vec::with_capacity(payload.len());
    let mut i = 0;
    while i < payload.len() {
        let b0 = payload[i];
        match b0 {
            0x01..=0x7F => {
                units.push(u16::from(b0));
                i += 1;
            }
            0xC0..=0xDF => {
                let Some(&b1) = payload.get(i + 1) else {
                    return Err(truncated());
                };
                if b1 & 0xC0 != 0x80 {
                    return Err(malformed());
                }
                units.push((u16::from(b0 & 0x1F) << 6) | u16::from(b1 & 0x3F));
                i += 2;
            }
            0xE0..=0xEF => {
                let Some(&b1) = payload.get(i + 1) else {
                    return Err(truncated());
                };
                let Some(&b2) = payload.get(i + 2) else {
                    return Err(truncated());
                };
                if b1 & 0xC0 != 0x80 || b2 & 0xC0 != 0x80 {
                    return Err(malformed());
                }
                units.push(
                    (u16::from(b0 & 0x0F) << 12)
                        | (u16::from(b1 & 0x3F) << 6)
                        | u16::from(b2 & 0x3F),
                );
                i += 3;
            }
            // 0x00 (standard-UTF-8 NUL) and stray continuation bytes (0x80..0xBF)
            // are not valid modified UTF-8.
            _ => return Err(malformed()),
        }
    }
    String::from_utf16(&units).map_err(|_| {
        ServerError::Protocol("modified UTF-8 contains an unpaired surrogate".to_owned())
    })
}

fn truncated() -> ServerError {
    ServerError::Protocol("modified UTF-8 string truncated".to_owned())
}

fn malformed() -> ServerError {
    ServerError::Protocol("malformed modified UTF-8".to_owned())
}

pub(crate) fn read_u16(bytes: &mut &[u8]) -> ServerResult<u16> {
    let raw = take(bytes, 2)?;
    Ok(u16::from_be_bytes([raw[0], raw[1]]))
}
pub(crate) fn take<'a>(bytes: &mut &'a [u8], n: usize) -> ServerResult<&'a [u8]> {
    if bytes.len() < n {
        return Err(truncated());
    }
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::{MAX_ENCODED_LEN, ServerError, read_modified_utf8, write_modified_utf8};

    #[test]
    fn round_trip_including_supplementary_characters() {
        for value in [
            "",
            "ascii",
            "nul\0inside",
            "\u{00E9}\u{4E2D}",
            "emoji \u{1F600}",
            "\u{10FFFF}",
        ] {
            let mut out = Vec::new();
            write_modified_utf8(value, &mut out).expect("encodes");
            let mut slice = &out[..];
            let decoded = read_modified_utf8(&mut slice).expect("decodes");
            assert_eq!(decoded, value);
            assert!(slice.is_empty());
        }
    }

    #[test]
    fn matches_measured_java_encoding() {
        // Values measured from the 26.1.2 runtime (see crate docs).
        let cases: &[(&str, &[u8])] = &[
            ("\0", &[0x00, 0x02, 0xC0, 0x80]),
            ("\u{00E9}", &[0x00, 0x02, 0xC3, 0xA9]),
            (
                "\u{1F600}",
                &[0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80],
            ),
            ("a\0b", &[0x00, 0x04, 0x61, 0xC0, 0x80, 0x62]),
        ];
        for (value, expected) in cases {
            let mut out = Vec::new();
            write_modified_utf8(value, &mut out).expect("encodes");
            assert_eq!(out, *expected, "encoding {value:?}");
        }
    }

    #[test]
    fn rejects_malformed_sequences() {
        // Stray continuation byte.
        assert!(read_modified_utf8(&mut &[0x00, 0x01, 0x80][..]).is_err());
        // Standard-UTF-8 NUL is not modified UTF-8.
        assert!(read_modified_utf8(&mut &[0x00, 0x01, 0x00][..]).is_err());
        // Truncated payload.
        assert!(read_modified_utf8(&mut &[0x00, 0x04, 0x41][..]).is_err());
        // Truncated length prefix.
        assert!(read_modified_utf8(&mut &[0x00][..]).is_err());
        // Unpaired high surrogate: `encode_utf16` of a Rust string never yields
        // one, so build the CESU-8 bytes by hand. A high surrogate followed by
        // nothing decodes back to an invalid Rust string and must be rejected.
        let mut out = Vec::new();
        out.extend_from_slice(&[0xED, 0xA0, 0xBD]);
        let mut framed = (out.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&out);
        assert!(read_modified_utf8(&mut &framed[..]).is_err());
    }

    #[test]
    fn over_long_strings_fail_instead_of_truncating() {
        let huge = "a".repeat(MAX_ENCODED_LEN + 1);
        let mut out = Vec::new();
        let err = write_modified_utf8(&huge, &mut out).expect_err("must fail");
        assert!(matches!(err, ServerError::Operational(_)), "{err:?}");
        assert!(out.is_empty(), "no partial output: {out:?}");
        // Exactly at the limit still works and round-trips.
        let exact = "a".repeat(MAX_ENCODED_LEN);
        let mut out = Vec::new();
        write_modified_utf8(&exact, &mut out).expect("encodes at the limit");
        assert_eq!(out.len(), MAX_ENCODED_LEN + 2);
        assert_eq!(
            read_modified_utf8(&mut &out[..]).expect("decodes"),
            exact,
            "round trip at the limit"
        );
    }
}
