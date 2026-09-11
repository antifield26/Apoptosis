//! NBT writer (P03-03).
//!
//! The writer mirrors the reader exactly: same tag ids, same payload layout,
//! same modified-UTF-8 strings. Two entry points:
//!
//! - [`write_named`] — disk encoding, with the root tag name;
//! - [`write_unnamed`] — network encoding, root name omitted.
//!
//! Writing is fallible for one reason only: Vanilla NBT strings carry a `u16`
//! byte-length prefix, so a string whose modified-UTF-8 form exceeds 65535 bytes
//! cannot be represented. Vanilla throws `UTFDataFormatException`; we return
//! [`ServerError::Operational`] and leave `out` untouched rather than emitting a
//! truncated, unreadable file.

use crate::string::write_modified_utf8;
use crate::value::{NbtTag, tag};
use mc_core::error::ServerResult;

/// Serialize `tag` with a root name (disk encoding).
///
/// # Errors
///
/// [`ServerError::Operational`] when a string exceeds the encodable length.
pub fn write_named(name: &str, tag: &NbtTag, out: &mut Vec<u8>) -> ServerResult<()> {
    let start = out.len();
    out.push(tag.tag_id());
    if let Err(err) = write_modified_utf8(name, out).and_then(|()| write_payload(tag, out)) {
        out.truncate(start);
        return Err(err);
    }
    Ok(())
}

/// Serialize `tag` without a root name (network encoding).
///
/// # Errors
///
/// [`ServerError::Operational`] when a string exceeds the encodable length.
pub fn write_unnamed(tag: &NbtTag, out: &mut Vec<u8>) -> ServerResult<()> {
    let start = out.len();
    out.push(tag.tag_id());
    if let Err(err) = write_payload(tag, out) {
        out.truncate(start);
        return Err(err);
    }
    Ok(())
}

fn write_payload(tag: &NbtTag, out: &mut Vec<u8>) -> ServerResult<()> {
    match tag {
        NbtTag::Byte(value) => out.push(*value as u8),
        NbtTag::Short(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Int(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Long(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Float(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::Double(value) => out.extend_from_slice(&value.to_be_bytes()),
        NbtTag::ByteArray(bytes) => {
            write_len(bytes.len(), "byte array", out)?;
            out.extend_from_slice(bytes);
        }
        NbtTag::String(value) => write_modified_utf8(value, out)?,
        NbtTag::List(items) => {
            // Vanilla writes the list's element type in the header: TAG_End for
            // an empty list, otherwise the type of every element (they are
            // homogeneous by construction).
            let element_id = items.first().map_or(tag::END, NbtTag::tag_id);
            out.push(element_id);
            write_len(items.len(), "list", out)?;
            for item in items {
                write_payload(item, out)?;
            }
        }
        NbtTag::Compound(entries) => {
            for (name, value) in entries {
                out.push(value.tag_id());
                write_modified_utf8(name, out)?;
                write_payload(value, out)?;
            }
            out.push(tag::END);
        }
        NbtTag::IntArray(values) => {
            write_len(values.len(), "int array", out)?;
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
        NbtTag::LongArray(values) => {
            write_len(values.len(), "long array", out)?;
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
    }
    Ok(())
}

/// Write an `i32` collection length, refusing counts the format cannot express.
fn write_len(len: usize, what: &str, out: &mut Vec<u8>) -> ServerResult<()> {
    let Ok(len) = i32::try_from(len) else {
        return Err(mc_core::error::ServerError::Operational(format!(
            "NBT {what} of {len} entries exceeds the i32 length field"
        )));
    };
    out.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

impl NbtTag {
    /// Serialize this value as a nameless network-NBT tag.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a string is too long to encode.
    pub fn write_network(&self, out: &mut Vec<u8>) -> ServerResult<()> {
        write_unnamed(self, out)
    }

    /// Serialize this value with a root name (disk encoding).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a string is too long to encode.
    pub fn write_named_tag(&self, name: &str, out: &mut Vec<u8>) -> ServerResult<()> {
        write_named(name, self, out)
    }
}

#[cfg(test)]
mod tests {
    use super::{write_named, write_unnamed};
    use crate::read::{Limits, TagReader};
    use crate::value::{NbtTag, tag};

    fn round_trip_disk(name: &str, tag: &NbtTag) {
        let mut bytes = Vec::new();
        write_named(name, tag, &mut bytes).expect("encodes");
        let mut reader = TagReader::new(&bytes, Limits::DISK).expect("reader");
        let (read_name, read_tag) = reader.read_named().expect("decodes");
        assert_eq!(read_name, name);
        assert_eq!(&read_tag, tag);
        assert_eq!(
            reader.remaining(),
            0,
            "writer output must be fully consumed"
        );
    }

    fn sample() -> NbtTag {
        NbtTag::Compound(vec![
            ("byte".to_owned(), NbtTag::Byte(-3)),
            ("short".to_owned(), NbtTag::Short(-300)),
            ("int".to_owned(), NbtTag::Int(-70_000)),
            ("long".to_owned(), NbtTag::Long(i64::MIN)),
            ("float".to_owned(), NbtTag::Float(1.5)),
            ("double".to_owned(), NbtTag::Double(-2.25)),
            ("bytes".to_owned(), NbtTag::ByteArray(vec![0, 1, 255])),
            (
                "string".to_owned(),
                NbtTag::String("h\u{00E9}llo \u{1F600}\0".to_owned()),
            ),
            (
                "list".to_owned(),
                NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2), NbtTag::Int(3)]),
            ),
            ("empty_list".to_owned(), NbtTag::List(Vec::new())),
            ("empty_compound".to_owned(), NbtTag::Compound(Vec::new())),
            ("ints".to_owned(), NbtTag::IntArray(vec![1, -2, 3])),
            ("longs".to_owned(), NbtTag::LongArray(vec![1, i64::MAX])),
            (
                "nested".to_owned(),
                NbtTag::Compound(vec![(
                    "deep".to_owned(),
                    NbtTag::List(vec![NbtTag::Compound(vec![(
                        "leaf".to_owned(),
                        NbtTag::Byte(7),
                    )])]),
                )]),
            ),
        ])
    }

    #[test]
    fn every_tag_type_round_trips_on_disk() {
        round_trip_disk("", &sample());
        round_trip_disk("Data", &sample());
        round_trip_disk("n\u{00E9}me", &sample());
    }

    #[test]
    fn every_tag_type_round_trips_on_the_network() {
        let tag = sample();
        let mut bytes = Vec::new();
        write_unnamed(&tag, &mut bytes).expect("encodes");
        let mut reader = TagReader::new(&bytes, Limits::NETWORK).expect("reader");
        let decoded = reader.read_unnamed().expect("decodes");
        assert_eq!(decoded, tag);
        assert_eq!(reader.remaining(), 0);
        assert_eq!(bytes[0], tag::COMPOUND, "nameless root starts with the id");
    }

    #[test]
    fn empty_list_carries_end_element_type() {
        let mut bytes = Vec::new();
        write_unnamed(&NbtTag::List(Vec::new()), &mut bytes).expect("encodes");
        assert_eq!(bytes, [tag::LIST, tag::END, 0, 0, 0, 0]);
    }

    #[test]
    fn disk_root_encodes_name_before_payload() {
        let mut bytes = Vec::new();
        write_named("Data", &NbtTag::Int(1), &mut bytes).expect("encodes");
        assert_eq!(
            bytes,
            [tag::INT, 0x00, 0x04, b'D', b'a', b't', b'a', 0, 0, 0, 1]
        );
    }

    #[test]
    fn unwritable_string_leaves_output_untouched() {
        let tag = NbtTag::Compound(vec![("big".to_owned(), NbtTag::String("a".repeat(70_000)))]);
        let mut bytes = Vec::new();
        assert!(write_named("Data", &tag, &mut bytes).is_err());
        assert!(
            bytes.is_empty(),
            "partial output must not survive a failure"
        );
    }

    #[test]
    fn collection_lengths_are_big_endian_i32() {
        let mut bytes = Vec::new();
        write_unnamed(&NbtTag::IntArray(vec![7]), &mut bytes).expect("encodes");
        assert_eq!(bytes, [tag::INT_ARRAY, 0, 0, 0, 1, 0, 0, 0, 7]);
    }
}
