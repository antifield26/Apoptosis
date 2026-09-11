//! Bounds-checked packet reader/writer primitives (P02-03).
//!
//! Every read returns [`ServerError::Protocol`] instead of panicking, so a
//! malicious payload can only ever drop a connection. Strings are length
//! capped and UTF-8 validated; identifiers go through [`mc_core::ids::ResourceId`].

use crate::varint::{read_varint, write_varint};
use mc_core::error::{ServerError, ServerResult};
use mc_core::ids::ResourceId;

/// Cursor over a packet payload.
#[derive(Debug, Clone)]
pub struct PacketReader<'a> {
    data: &'a [u8],
}

impl<'a> PacketReader<'a> {
    /// Wrap a byte slice.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len()
    }

    /// Whether the reader is fully consumed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// All remaining bytes, consuming the reader.
    pub fn take_remaining(&mut self) -> &'a [u8] {
        let rest = self.data;
        self.data = &[];
        rest
    }

    /// The remaining bytes without consuming them.
    #[must_use]
    pub fn remaining_slice(&self) -> &'a [u8] {
        self.data
    }

    /// Skip `n` already-validated bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than `n` bytes remain.
    pub fn advance(&mut self, n: usize) -> ServerResult<()> {
        self.ensure(n)?;
        self.data = &self.data[n..];
        Ok(())
    }

    fn ensure(&self, n: usize) -> ServerResult<()> {
        if self.data.len() < n {
            return Err(ServerError::Protocol(format!(
                "packet truncated: need {n} bytes, have {}",
                self.data.len()
            )));
        }
        Ok(())
    }

    /// Read a single unsigned byte.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 1 byte remains.
    pub fn read_u8(&mut self) -> ServerResult<u8> {
        self.ensure(1)?;
        let byte = self.data[0];
        self.data = &self.data[1..];
        Ok(byte)
    }

    /// Read a signed byte.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 1 byte remains.
    pub fn read_i8(&mut self) -> ServerResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Read a boolean (nonzero is true, matching the Java client).
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 1 byte remains.
    pub fn read_bool(&mut self) -> ServerResult<bool> {
        Ok(self.read_u8()? != 0)
    }

    /// Read a big-endian `u16`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 2 bytes remain.
    pub fn read_u16(&mut self) -> ServerResult<u16> {
        self.ensure(2)?;
        let value = u16::from_be_bytes([self.data[0], self.data[1]]);
        self.data = &self.data[2..];
        Ok(value)
    }

    /// Read a big-endian `i16`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 2 bytes remain.
    pub fn read_i16(&mut self) -> ServerResult<i16> {
        Ok(self.read_u16()? as i16)
    }

    /// Read a big-endian `i32`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 4 bytes remain.
    pub fn read_i32(&mut self) -> ServerResult<i32> {
        self.ensure(4)?;
        let value = i32::from_be_bytes([self.data[0], self.data[1], self.data[2], self.data[3]]);
        self.data = &self.data[4..];
        Ok(value)
    }

    /// Read a big-endian `i64`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 8 bytes remain.
    pub fn read_i64(&mut self) -> ServerResult<i64> {
        self.ensure(8)?;
        let value = i64::from_be_bytes([
            self.data[0],
            self.data[1],
            self.data[2],
            self.data[3],
            self.data[4],
            self.data[5],
            self.data[6],
            self.data[7],
        ]);
        self.data = &self.data[8..];
        Ok(value)
    }

    /// Read a big-endian `f32`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 4 bytes remain.
    pub fn read_f32(&mut self) -> ServerResult<f32> {
        Ok(f32::from_bits(self.read_i32()? as u32))
    }

    /// Read a big-endian `f64`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 8 bytes remain.
    pub fn read_f64(&mut self) -> ServerResult<f64> {
        Ok(f64::from_bits(self.read_i64()? as u64))
    }

    /// Read a `VarInt`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on truncated/overlong data.
    pub fn read_varint(&mut self) -> ServerResult<i32> {
        read_varint(&mut self.data)
    }

    /// Read a `VarLong`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on truncated/overlong data.
    pub fn read_varlong(&mut self) -> ServerResult<i64> {
        crate::varint::read_varlong(&mut self.data)
    }

    /// Read exactly `n` bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than `n` bytes remain.
    pub fn read_bytes(&mut self, n: usize) -> ServerResult<&'a [u8]> {
        self.ensure(n)?;
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    /// Read a 16-byte UUID.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when fewer than 16 bytes remain.
    pub fn read_uuid(&mut self) -> ServerResult<uuid::Uuid> {
        let bytes = self.read_bytes(16)?;
        let mut raw = [0u8; 16];
        raw.copy_from_slice(bytes);
        Ok(uuid::Uuid::from_bytes(raw))
    }

    /// Read a VarInt-length-prefixed UTF-8 string.
    ///
    /// `max_chars` is the vanilla character limit for the field (for example
    /// 16 for usernames); the byte length is additionally capped so a hostile
    /// length prefix cannot trigger a huge allocation.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on negative/oversized lengths, invalid UTF-8,
    /// or char-count overflow.
    pub fn read_string(&mut self, max_chars: usize) -> ServerResult<String> {
        let len = self.read_varint()?;
        if len < 0 {
            return Err(ServerError::Protocol(format!(
                "negative string length {len}"
            )));
        }
        let len = usize::try_from(len)
            .map_err(|_| ServerError::Protocol("string length overflow".to_owned()))?;
        let max_bytes = max_chars.saturating_mul(4);
        if len > max_bytes {
            return Err(ServerError::Protocol(format!(
                "string of {len} bytes exceeds limit of {max_bytes}"
            )));
        }
        let bytes = self.read_bytes(len)?;
        let text = std::str::from_utf8(bytes)
            .map_err(|e| ServerError::Protocol(format!("string is not valid UTF-8: {e}")))?;
        if text.chars().count() > max_chars {
            return Err(ServerError::Protocol(format!(
                "string of {} chars exceeds limit of {max_chars}",
                text.chars().count()
            )));
        }
        Ok(text.to_owned())
    }

    /// Read a `namespace:value` identifier with the vanilla field cap.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the string is malformed or the identifier
    /// fails [`ResourceId`] validation.
    pub fn read_identifier(&mut self) -> ServerResult<ResourceId> {
        let text = self.read_string(crate::MAX_IDENTIFIER_LEN)?;
        ResourceId::parse(&text)
    }
}

/// Growable packet payload writer. All primitives are infallible except
/// string writes, which enforce the wire length cap.
#[derive(Debug, Default, Clone)]
pub struct PacketWriter {
    buf: Vec<u8>,
}

impl PacketWriter {
    /// Create an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Create a writer containing the packet id (the payload prefix).
    #[must_use]
    pub fn with_id(id: i32) -> Self {
        let mut writer = Self::new();
        writer.write_varint(id);
        writer
    }

    /// Bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Consume the writer, returning the payload bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Borrow the written bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Write one byte.
    pub fn write_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    /// Write one signed byte.
    pub fn write_i8(&mut self, value: i8) {
        self.buf.push(value as u8);
    }

    /// Write a boolean as `0`/`1`.
    pub fn write_bool(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    /// Write a big-endian `u16`.
    pub fn write_u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `i16`.
    pub fn write_i16(&mut self, value: i16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `i32`.
    pub fn write_i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `i64`.
    pub fn write_i64(&mut self, value: i64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `f32`.
    pub fn write_f32(&mut self, value: f32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `f64`.
    pub fn write_f64(&mut self, value: f64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a `VarInt`.
    pub fn write_varint(&mut self, value: i32) {
        write_varint(&mut self.buf, value);
    }

    /// Write a `VarLong`.
    pub fn write_varlong(&mut self, value: i64) {
        crate::varint::write_varlong(&mut self.buf, value);
    }

    /// Write raw bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Write a UUID as 16 raw bytes.
    pub fn write_uuid(&mut self, value: &uuid::Uuid) {
        self.buf.extend_from_slice(value.as_bytes());
    }

    /// Write a UTF-8 string with a `VarInt` byte-length prefix.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the string exceeds the wire cap.
    /// Callers pass server-authored strings, so this is an invariant check,
    /// not a routine failure path.
    pub fn write_string(&mut self, value: &str) -> ServerResult<()> {
        if value.len() > crate::MAX_IDENTIFIER_LEN {
            return Err(ServerError::Operational(format!(
                "string of {} bytes exceeds wire limit",
                value.len()
            )));
        }
        let len = i32::try_from(value.len())
            .map_err(|_| ServerError::Operational("string too long".to_owned()))?;
        self.write_varint(len);
        self.buf.extend_from_slice(value.as_bytes());
        Ok(())
    }

    /// Write an identifier string.
    ///
    /// # Errors
    ///
    /// Same as [`PacketWriter::write_string`].
    pub fn write_identifier(&mut self, value: &ResourceId) -> ServerResult<()> {
        self.write_string(&value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{PacketReader, PacketWriter};
    use mc_core::error::ServerError;

    #[test]
    fn primitives_round_trip() {
        let mut writer = PacketWriter::with_id(0x2A);
        writer.write_u8(0xFF);
        writer.write_i16(-2);
        writer.write_i32(0x0102_0304);
        writer.write_i64(-9);
        writer.write_f32(1.5);
        writer.write_f64(-2.25);
        writer.write_bool(true);
        writer.write_string("hello").expect("writes");
        writer.write_uuid(&uuid::Uuid::from_u128(7));

        let bytes = writer.finish();
        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.read_varint().expect("id"), 0x2A);
        assert_eq!(reader.read_u8().expect("u8"), 0xFF);
        assert_eq!(reader.read_i16().expect("i16"), -2);
        assert_eq!(reader.read_i32().expect("i32"), 0x0102_0304);
        assert_eq!(reader.read_i64().expect("i64"), -9);
        assert!((reader.read_f32().expect("f32") - 1.5).abs() < f32::EPSILON);
        assert!((reader.read_f64().expect("f64") + 2.25).abs() < f64::EPSILON);
        assert!(reader.read_bool().expect("bool"));
        assert_eq!(reader.read_string(32).expect("string"), "hello");
        assert_eq!(reader.read_uuid().expect("uuid"), uuid::Uuid::from_u128(7));
        assert!(reader.is_empty());
    }

    #[test]
    fn reader_rejects_truncation() {
        let mut reader = PacketReader::new(&[0x01, 0x02]);
        assert!(reader.read_i32().is_err());
        assert!(matches!(
            PacketReader::new(&[]).read_u8(),
            Err(ServerError::Protocol(_))
        ));
    }

    #[test]
    fn string_length_caps_are_enforced() {
        // 17-byte string read with a 16-char limit.
        let mut payload = PacketWriter::new();
        payload.write_string("abcdefghijklmnopq").expect("writes");
        let bytes = payload.finish();
        let mut reader = PacketReader::new(&bytes);
        assert!(reader.read_string(16).is_err());
    }

    #[test]
    fn string_char_count_is_enforced_not_only_bytes() {
        // 17 two-byte chars = 34 bytes; byte cap allows it, char cap must not.
        let value: String = "\u{00E9}".repeat(17);
        let mut payload = PacketWriter::new();
        payload.write_string(&value).expect("writes");
        let bytes = payload.finish();
        let mut reader = PacketReader::new(&bytes);
        assert!(reader.read_string(16).is_err());
    }

    #[test]
    fn hostile_length_prefix_does_not_allocate() {
        // Prefix claims 2 GiB but no bytes follow: must error immediately.
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, i32::MAX);
        let mut reader = PacketReader::new(&bytes);
        assert!(reader.read_string(16).is_err());
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut bytes = Vec::new();
        crate::varint::write_varint(&mut bytes, 2);
        bytes.extend_from_slice(&[0xC3, 0x28]);
        let mut reader = PacketReader::new(&bytes);
        assert!(reader.read_string(16).is_err());
    }

    #[test]
    fn identifier_validation_applies() {
        let mut payload = PacketWriter::new();
        payload.write_string("minecraft:stone").expect("writes");
        payload.write_string("../evil").expect("writes");
        let bytes = payload.finish();
        let mut reader = PacketReader::new(&bytes);
        assert!(reader.read_identifier().is_ok());
        assert!(reader.read_identifier().is_err());
    }
}
