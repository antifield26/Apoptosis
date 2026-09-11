//! Status-state packets (server-list ping).

use super::Packet;
use crate::ids::{clientbound, serverbound};
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

/// `minecraft:status_request` (serverbound 0). Empty payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = serverbound::status::STATUS_REQUEST;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if !payload.is_empty() {
            return Err(ServerError::Protocol(
                "status request must be empty".to_owned(),
            ));
        }
        Ok(Self)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// `minecraft:ping_request` (serverbound 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusPing {
    /// Opaque payload echoed back by the server.
    pub payload: i64,
}

impl Packet for StatusPing {
    const ID: i32 = serverbound::status::PING_REQUEST;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            payload: PacketReader::new(payload).read_i64()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.payload);
        Ok(writer.finish())
    }
}

/// `minecraft:status_response` (clientbound 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResponse {
    /// JSON document per the server-list ping schema.
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = clientbound::status::STATUS_RESPONSE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            json: PacketReader::new(payload).read_string(crate::MAX_IDENTIFIER_LEN)?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_string(&self.json)?;
        Ok(writer.finish())
    }
}

/// `minecraft:pong_response` (clientbound 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusPong {
    /// Payload echoed from [`StatusPing`].
    pub payload: i64,
}

impl Packet for StatusPong {
    const ID: i32 = clientbound::status::PONG_RESPONSE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            payload: PacketReader::new(payload).read_i64()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.payload);
        Ok(writer.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::{StatusPing, StatusRequest, StatusResponse};
    use crate::packets::Packet;

    #[test]
    fn status_request_is_empty_and_strict() {
        assert_eq!(StatusRequest.encode().expect("encodes"), Vec::<u8>::new());
        assert!(StatusRequest::decode(&[]).is_ok());
        assert!(StatusRequest::decode(&[0x00]).is_err());
    }

    #[test]
    fn ping_round_trip() {
        let packet = StatusPing {
            payload: 0x0102_0304_0506_0708,
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(StatusPing::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn status_response_round_trip() {
        let packet = StatusResponse {
            json: "{\"version\":{\"name\":\"26.1.2\",\"protocol\":775}}".to_owned(),
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(StatusResponse::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn truncated_ping_is_rejected() {
        assert!(StatusPing::decode(&[0x01, 0x02]).is_err());
    }
}
