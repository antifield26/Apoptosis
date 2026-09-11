//! Raw packet representation shared by codec layers.

use crate::wire::PacketReader;

/// A decoded packet: the protocol id plus the payload after the id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacket {
    /// Protocol packet id for the current connection state.
    pub id: i32,
    /// Payload bytes after the id.
    pub payload: Vec<u8>,
}

impl RawPacket {
    /// Create a packet from an id and payload.
    #[must_use]
    pub fn new(id: i32, payload: Vec<u8>) -> Self {
        Self { id, payload }
    }

    /// Reader over the payload.
    #[must_use]
    pub fn reader(&self) -> PacketReader<'_> {
        PacketReader::new(&self.payload)
    }
}
