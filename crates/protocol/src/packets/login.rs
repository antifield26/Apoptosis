//! Login-state packets.

use super::Packet;
use crate::ids::{clientbound, serverbound};
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::ServerResult;

/// Vanilla username cap.
pub const MAX_USERNAME_CHARS: usize = 16;

/// `minecraft:hello` / `LoginStart` (serverbound 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStart {
    /// Requested username.
    pub name: String,
    /// Client-announced profile UUID (real value derived by the server in
    /// offline mode, see `mc-network::auth`).
    pub uuid: uuid::Uuid,
}

impl Packet for LoginStart {
    const ID: i32 = serverbound::login::HELLO;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let name = reader.read_string(MAX_USERNAME_CHARS)?;
        let uuid = reader.read_uuid()?;
        Ok(Self { name, uuid })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_string(&self.name)?;
        writer.write_uuid(&self.uuid);
        Ok(writer.finish())
    }
}

/// `minecraft:login_acknowledged` (serverbound 3). Empty payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoginAcknowledged;

impl Packet for LoginAcknowledged {
    const ID: i32 = serverbound::login::LOGIN_ACKNOWLEDGED;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if !payload.is_empty() {
            return Err(mc_core::error::ServerError::Protocol(
                "login acknowledged must be empty".to_owned(),
            ));
        }
        Ok(Self)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// A profile property (skin/cape data); empty for offline profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileProperty {
    /// Property name, e.g. `textures`.
    pub name: String,
    /// Property value.
    pub value: String,
    /// Optional Mojang signature.
    pub signature: Option<String>,
}

/// `minecraft:login_finished` (clientbound 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    /// Assigned profile UUID.
    pub uuid: uuid::Uuid,
    /// Accepted username.
    pub name: String,
    /// Profile properties.
    pub properties: Vec<ProfileProperty>,
}

impl Packet for LoginSuccess {
    const ID: i32 = clientbound::login::LOGIN_FINISHED;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let uuid = reader.read_uuid()?;
        let name = reader.read_string(MAX_USERNAME_CHARS)?;
        let count = reader.read_varint()?;
        if !(0..=64).contains(&count) {
            return Err(mc_core::error::ServerError::Protocol(format!(
                "profile property count out of range: {count}"
            )));
        }
        let mut properties = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = reader.read_string(64)?;
            let value = reader.read_string(1024)?;
            let signature = if reader.read_bool()? {
                Some(reader.read_string(1024)?)
            } else {
                None
            };
            properties.push(ProfileProperty {
                name,
                value,
                signature,
            });
        }
        Ok(Self {
            uuid,
            name,
            properties,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_uuid(&self.uuid);
        writer.write_string(&self.name)?;
        writer.write_varint(i32::try_from(self.properties.len()).unwrap_or(i32::MAX));
        for property in &self.properties {
            writer.write_string(&property.name)?;
            writer.write_string(&property.value)?;
            match &property.signature {
                Some(signature) => {
                    writer.write_bool(true);
                    writer.write_string(signature)?;
                }
                None => writer.write_bool(false),
            }
        }
        Ok(writer.finish())
    }
}

/// `minecraft:login_compression` (clientbound 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCompression {
    /// Threshold in bytes; payloads below it stay uncompressed.
    pub threshold: i32,
}

impl Packet for SetCompression {
    const ID: i32 = clientbound::login::LOGIN_COMPRESSION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            threshold: PacketReader::new(payload).read_varint()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.threshold);
        Ok(writer.finish())
    }
}

/// `minecraft:login_disconnect` (clientbound 0). JSON-encoded reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDisconnect {
    /// JSON text component.
    pub json: String,
}

impl Packet for LoginDisconnect {
    const ID: i32 = clientbound::login::LOGIN_DISCONNECT;

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

#[cfg(test)]
mod tests {
    use super::{
        LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess, ProfileProperty,
        SetCompression,
    };
    use crate::packets::Packet;

    #[test]
    fn login_start_round_trip() {
        let packet = LoginStart {
            name: "Notch".to_owned(),
            uuid: uuid::Uuid::from_u128(42),
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(LoginStart::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn login_start_rejects_overlong_name() {
        let mut writer = crate::wire::PacketWriter::new();
        writer
            .write_string("this_name_is_way_too_long")
            .expect("writes");
        writer.write_uuid(&uuid::Uuid::nil());
        assert!(LoginStart::decode(&writer.finish()).is_err());
    }

    #[test]
    fn login_success_round_trip_with_properties() {
        let packet = LoginSuccess {
            uuid: uuid::Uuid::from_u128(7),
            name: "Steve".to_owned(),
            properties: vec![ProfileProperty {
                name: "textures".to_owned(),
                value: "abc".to_owned(),
                signature: Some("sig".to_owned()),
            }],
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(LoginSuccess::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn set_compression_round_trip() {
        let packet = SetCompression { threshold: 256 };
        assert_eq!(
            SetCompression::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn disconnect_round_trip() {
        let packet = LoginDisconnect {
            json: "{\"text\":\"nope\"}".to_owned(),
        };
        assert_eq!(
            LoginDisconnect::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn login_acknowledged_is_empty() {
        assert_eq!(
            LoginAcknowledged.encode().expect("encodes"),
            Vec::<u8>::new()
        );
        assert!(LoginAcknowledged::decode(&[0x00]).is_err());
    }
}
