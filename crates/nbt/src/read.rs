//! Bounded NBT reader (P03-02, P03-04).
//!
//! The reader is a recursive-descent parser driven by a [`Cursor`] over a byte
//! slice. Three independent budgets stop hostile or corrupt input from turning
//! into unbounded work or memory (AGENTS.md section 10):
//!
//! 1. **Depth** — a nesting limit, so recursion cannot overflow the stack.
//! 2. **Input bytes** — an absolute size cap checked once, before parsing.
//! 3. **Tag count** — a cap on created tags, so a file full of tiny values
//!    cannot amplify into gigabytes of `NbtTag`s.
//!
//! On top of that, every declared collection length must fit in the remaining
//! input (the largest possible element still occupies at least one byte), which
//! is what prevents an `i32::MAX` element count from pre-allocating.

use crate::value::{NbtTag, tag};
use mc_core::error::{ServerError, ServerResult};

/// Resource budgets for one parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum nesting depth of compound/list tags.
    pub max_depth: usize,
    /// Maximum size of the input buffer accepted by one parse.
    pub max_bytes: usize,
    /// Maximum number of tags the parse may create.
    pub max_elements: usize,
}

impl Limits {
    /// Limits for world files: a chunk's decompressed NBT is normally well
    /// under 1 MiB (measured 2.9 KiB..147 KiB on a real 26.1.2 world), with
    /// 16 MiB as headroom for entity-heavy chunks.
    pub const DISK: Self = Self {
        max_depth: 512,
        max_bytes: 16 * 1024 * 1024,
        max_elements: 4 * 1024 * 1024,
    };

    /// Limits for network NBT inside a packet frame (2 MiB frame cap).
    pub const NETWORK: Self = Self {
        max_depth: 512,
        max_bytes: 2 * 1024 * 1024,
        max_elements: 1024 * 1024,
    };

    /// Limits with a specific input-size cap, keeping the other budgets.
    #[must_use]
    pub const fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            ..Self::DISK
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DISK
    }
}

/// A bounds-checked cursor over an NBT byte slice.
#[derive(Debug)]
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn u8(&mut self) -> ServerResult<u8> {
        let Some(&byte) = self.bytes.get(self.pos) else {
            return Err(truncated("tag id"));
        };
        self.pos += 1;
        Ok(byte)
    }

    fn i16(&mut self) -> ServerResult<i16> {
        let raw = self.take(2)?;
        Ok(i16::from_be_bytes([raw[0], raw[1]]))
    }

    fn i32(&mut self) -> ServerResult<i32> {
        let raw = self.take(4)?;
        Ok(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn i64(&mut self) -> ServerResult<i64> {
        let raw = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(raw);
        Ok(i64::from_be_bytes(array))
    }

    fn take(&mut self, n: usize) -> ServerResult<&'a [u8]> {
        if self.remaining() < n {
            return Err(truncated("value"));
        }
        let start = self.pos;
        self.pos += n;
        Ok(&self.bytes[start..self.pos])
    }

    /// Read a `u16`-length-prefixed modified UTF-8 string.
    fn string(&mut self) -> ServerResult<String> {
        let len = self.i16()?;
        if len < 0 {
            return Err(ServerError::Protocol(format!(
                "negative NBT string length {len}"
            )));
        }
        let payload = self.take(len as usize)?;
        crate::string::decode(payload)
    }

    /// Read a signed 32-bit collection length that must fit in the remaining
    /// input (every element occupies at least one byte on the wire).
    fn bounded_len(&mut self, what: &str) -> ServerResult<usize> {
        let len = self.i32()?;
        if len < 0 {
            return Err(ServerError::Protocol(format!(
                "negative NBT {what} length {len}"
            )));
        }
        let len = len as usize;
        if len > self.remaining() {
            return Err(ServerError::Protocol(format!(
                "NBT {what} length {len} exceeds {} remaining bytes",
                self.remaining()
            )));
        }
        Ok(len)
    }
}

/// Streaming NBT reader with explicit budgets.
///
/// ```no_run
/// # use mc_nbt::{Limits, TagReader};
/// # fn demo(bytes: &[u8]) -> mc_core::error::ServerResult<()> {
/// let mut reader = TagReader::new(bytes, Limits::DISK)?;
/// let (root_name, root) = reader.read_named()?;
/// assert_eq!(reader.remaining(), 0, "file must not have trailing bytes");
/// # let _ = (root_name, root);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct TagReader<'a> {
    cursor: Cursor<'a>,
    limits: Limits,
    elements: usize,
}

impl<'a> TagReader<'a> {
    /// Create a reader over `bytes`.
    ///
    /// The byte budget is enforced here: an oversized buffer is rejected before
    /// any parsing work happens.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when `bytes` exceeds `limits.max_bytes`.
    pub fn new(bytes: &'a [u8], limits: Limits) -> ServerResult<Self> {
        if bytes.len() > limits.max_bytes {
            return Err(ServerError::CorruptData(format!(
                "NBT payload of {} bytes exceeds the {} byte limit",
                bytes.len(),
                limits.max_bytes
            )));
        }
        Ok(Self {
            cursor: Cursor::new(bytes),
            limits,
            elements: 0,
        })
    }

    /// Bytes consumed so far.
    #[must_use]
    pub fn position(&self) -> usize {
        self.cursor.pos
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.cursor.remaining()
    }

    /// Read a named root tag (disk encoding): `id`, name, payload.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] for a null (`TAG_End`) root, structural
    /// truncation, budget exhaustion or malformed strings.
    pub fn read_named(&mut self) -> ServerResult<(String, NbtTag)> {
        let id = self.cursor.u8()?;
        if id == tag::END {
            return Err(ServerError::CorruptData(
                "NBT has a null (TAG_End) root tag".to_owned(),
            ));
        }
        let name = self.cursor.string()?;
        let value = self.read_payload(id, 0)?;
        Ok((name, value))
    }

    /// Read a nameless root tag (network encoding): `id`, payload.
    ///
    /// # Errors
    ///
    /// Same classes as [`TagReader::read_named`].
    pub fn read_unnamed(&mut self) -> ServerResult<NbtTag> {
        let id = self.cursor.u8()?;
        if id == tag::END {
            return Err(ServerError::Protocol(
                "network NBT has a null (TAG_End) root tag".to_owned(),
            ));
        }
        self.read_payload(id, 0)
    }

    fn budget(&mut self) -> ServerResult<()> {
        self.elements += 1;
        if self.elements > self.limits.max_elements {
            return Err(ServerError::CorruptData(format!(
                "NBT exceeds the {} tag budget",
                self.limits.max_elements
            )));
        }
        Ok(())
    }

    fn depth(&self, depth: usize) -> ServerResult<()> {
        if depth > self.limits.max_depth {
            return Err(ServerError::CorruptData(format!(
                "NBT nesting exceeds the {} level limit",
                self.limits.max_depth
            )));
        }
        Ok(())
    }

    fn read_payload(&mut self, id: u8, depth: usize) -> ServerResult<NbtTag> {
        self.budget()?;
        match id {
            tag::BYTE => Ok(NbtTag::Byte(self.cursor.u8()? as i8)),
            tag::SHORT => Ok(NbtTag::Short(self.cursor.i16()?)),
            tag::INT => Ok(NbtTag::Int(self.cursor.i32()?)),
            tag::LONG => Ok(NbtTag::Long(self.cursor.i64()?)),
            tag::FLOAT => Ok(NbtTag::Float(f32::from_bits(self.cursor.i32()? as u32))),
            tag::DOUBLE => Ok(NbtTag::Double(f64::from_bits(self.cursor.i64()? as u64))),
            tag::BYTE_ARRAY => {
                let len = self.cursor.bounded_len("byte array")?;
                Ok(NbtTag::ByteArray(self.cursor.take(len)?.to_vec()))
            }
            tag::STRING => Ok(NbtTag::String(self.cursor.string()?)),
            tag::LIST => {
                self.depth(depth)?;
                let element_id = self.cursor.u8()?;
                let count = self.cursor.bounded_len("list")?;
                if element_id == tag::END && count > 0 {
                    return Err(ServerError::Protocol(format!(
                        "NBT list declares {count} elements of TAG_End"
                    )));
                }
                if element_id > tag::LONG_ARRAY {
                    return Err(ServerError::Protocol(format!(
                        "unknown NBT tag id {element_id} in list header"
                    )));
                }
                let mut items = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    items.push(self.read_payload(element_id, depth + 1)?);
                }
                Ok(NbtTag::List(items))
            }
            tag::COMPOUND => {
                self.depth(depth)?;
                let mut entries = Vec::new();
                loop {
                    let entry_id = self.cursor.u8()?;
                    if entry_id == tag::END {
                        break;
                    }
                    if entry_id > tag::LONG_ARRAY {
                        return Err(ServerError::Protocol(format!(
                            "unknown NBT tag id {entry_id} in compound"
                        )));
                    }
                    let name = self.cursor.string()?;
                    let value = self.read_payload(entry_id, depth + 1)?;
                    entries.push((name, value));
                }
                Ok(NbtTag::Compound(entries))
            }
            tag::INT_ARRAY => {
                let len = self.cursor.bounded_len("int array")?;
                if len > self.cursor.remaining() / 4 {
                    return Err(ServerError::Protocol(format!(
                        "NBT int array of {len} entries exceeds the remaining input"
                    )));
                }
                let mut values = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    values.push(self.cursor.i32()?);
                }
                Ok(NbtTag::IntArray(values))
            }
            tag::LONG_ARRAY => {
                let len = self.cursor.bounded_len("long array")?;
                if len > self.cursor.remaining() / 8 {
                    return Err(ServerError::Protocol(format!(
                        "NBT long array of {len} entries exceeds the remaining input"
                    )));
                }
                let mut values = Vec::with_capacity(len.min(4096));
                for _ in 0..len {
                    values.push(self.cursor.i64()?);
                }
                Ok(NbtTag::LongArray(values))
            }
            tag::END => Err(ServerError::Protocol(
                "unexpected TAG_End where a value was expected".to_owned(),
            )),
            other => Err(ServerError::Protocol(format!("unknown NBT tag id {other}"))),
        }
    }
}

/// Read one named tag from `bytes` (disk encoding).
///
/// # Errors
///
/// See [`TagReader::read_named`].
pub fn read_named(bytes: &[u8], limits: Limits) -> ServerResult<(String, NbtTag)> {
    TagReader::new(bytes, limits)?.read_named()
}

/// Read one nameless tag from `bytes` (network encoding).
///
/// # Errors
///
/// See [`TagReader::read_unnamed`].
pub fn read_unnamed(bytes: &[u8], limits: Limits) -> ServerResult<NbtTag> {
    TagReader::new(bytes, limits)?.read_unnamed()
}

impl NbtTag {
    /// Deserialize a nameless network-NBT tag, advancing `bytes` past it.
    ///
    /// The `&mut &[u8]` shape matches the Phase 02 network decoder, where a
    /// packet payload is consumed field by field.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on truncated, oversized or malformed data.
    pub fn read_network(bytes: &mut &[u8]) -> ServerResult<Self> {
        Self::read_network_with(bytes, Limits::NETWORK)
    }

    /// [`NbtTag::read_network`] with explicit budgets.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on truncated, oversized or malformed data.
    pub fn read_network_with(bytes: &mut &[u8], limits: Limits) -> ServerResult<Self> {
        let mut reader = TagReader::new(bytes, limits)?;
        let tag = reader.read_unnamed()?;
        let consumed = reader.position();
        *bytes = &bytes[consumed..];
        Ok(tag)
    }

    /// Deserialize a named (disk) NBT tag, advancing `bytes` past it.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] on a null root or budget exhaustion,
    /// [`ServerError::Protocol`] on truncated or malformed data.
    pub fn read_named_tag(bytes: &mut &[u8], limits: Limits) -> ServerResult<(String, Self)> {
        let mut reader = TagReader::new(bytes, limits)?;
        let (name, tag) = reader.read_named()?;
        let consumed = reader.position();
        *bytes = &bytes[consumed..];
        Ok((name, tag))
    }
}

fn truncated(what: &str) -> ServerError {
    ServerError::Protocol(format!("NBT data truncated while reading {what}"))
}
