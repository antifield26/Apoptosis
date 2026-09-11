//! Handshake-state packets.

use super::Packet;
use crate::ids::serverbound::handshake as ids;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

/// Vanilla address cap (`BungeeCord` forwarding exceeds this; not supported in
/// early phases, see AGENTS.md section 2).
pub const MAX_ADDRESS_LEN: usize = 255;

/// The intent declared in the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeIntent {
    /// Server-list ping.
    Status,
    /// Login.
    Login,
    /// Transfer (treated as login; `docs/research/protocol-baseline.md` §4).
    Transfer,
}

impl HandshakeIntent {
    /// Decode the wire value.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for intents outside `1..=3`.
    pub fn from_wire(value: i32) -> ServerResult<Self> {
        match value {
            1 => Ok(Self::Status),
            2 => Ok(Self::Login),
            3 => Ok(Self::Transfer),
            other => Err(ServerError::Protocol(format!(
                "unknown handshake intent {other}"
            ))),
        }
    }

    /// The wire value.
    #[must_use]
    pub fn to_wire(self) -> i32 {
        match self {
            Self::Status => 1,
            Self::Login => 2,
            Self::Transfer => 3,
        }
    }
}

/// `minecraft:intention` (serverbound 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// Protocol version announced by the client.
    pub protocol_version: i32,
    /// Hostname used to connect.
    pub server_address: String,
    /// Port used to connect.
    pub server_port: u16,
    /// Requested next state.
    pub intent: HandshakeIntent,
}

impl Packet for Handshake {
    const ID: i32 = ids::INTENTION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let protocol_version = reader.read_varint()?;
        let server_address = reader.read_string(MAX_ADDRESS_LEN)?;
        let server_port = reader.read_u16()?;
        let intent = HandshakeIntent::from_wire(reader.read_varint()?)?;
        Ok(Self {
            protocol_version,
            server_address,
            server_port,
            intent,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.protocol_version);
        writer.write_string(&self.server_address)?;
        writer.write_u16(self.server_port);
        writer.write_varint(self.intent.to_wire());
        Ok(writer.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::{Handshake, HandshakeIntent};
    use crate::packets::Packet;

    #[test]
    fn round_trip_login_intent() {
        let packet = Handshake {
            protocol_version: 775,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(Handshake::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn golden_login_handshake_bytes() {
        // 775, "localhost", 25565, intent 2 -?hand-assembled from the wire spec.
        let packet = Handshake {
            protocol_version: 775,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        let body = packet.encode().expect("encodes");
        let expected = [
            0x87, 0x06, // VarInt 775
            0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', // address
            0x63, 0xDD, // port 25565
            0x02, // login
        ];
        assert_eq!(body, expected);

        // Framing pairs the body with id 0.
        let raw = packet.to_raw().expect("raw");
        assert_eq!(raw.id, Handshake::ID);
        assert_eq!(raw.payload, expected);
    }

    #[test]
    fn rejects_bad_intent_and_oversized_address() {
        let mut oversized = crate::wire::PacketWriter::new();
        oversized.write_varint(775);
        oversized
            .write_string("a".repeat(300).as_str())
            .expect("writes");
        oversized.write_u16(25565);
        oversized.write_varint(2);
        assert!(Handshake::decode(&oversized.finish()).is_err());

        let mut bad_intent = crate::wire::PacketWriter::new();
        bad_intent.write_varint(775);
        bad_intent.write_string("localhost").expect("writes");
        bad_intent.write_u16(25565);
        bad_intent.write_varint(9);
        assert!(Handshake::decode(&bad_intent.finish()).is_err());
    }
}
