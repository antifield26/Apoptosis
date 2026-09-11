//! Configuration-state packets.

use super::Packet;
use crate::ids::{clientbound, serverbound};
use crate::nbt::Nbt;
use crate::text::TextComponent;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

/// Vanilla locale cap for [`ClientInformation::locale`].
pub const MAX_LOCALE_CHARS: usize = 16;

/// A pack announcement (`minecraft:select_known_packs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPack {
    /// Pack namespace, e.g. `minecraft`.
    pub namespace: String,
    /// Pack id, e.g. `core`.
    pub id: String,
    /// Pack version string.
    pub version: String,
}

/// `minecraft:select_known_packs` (serverbound 7 / clientbound 14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectKnownPacks {
    /// Announced packs.
    pub packs: Vec<KnownPack>,
}

fn decode_known_packs(payload: &[u8]) -> ServerResult<SelectKnownPacks> {
    let mut reader = PacketReader::new(payload);
    let count = reader.read_varint()?;
    if !(0..=64).contains(&count) {
        return Err(ServerError::Protocol(format!(
            "known-pack count out of range: {count}"
        )));
    }
    let mut packs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        packs.push(KnownPack {
            namespace: reader.read_string(crate::MAX_IDENTIFIER_LEN)?,
            id: reader.read_string(crate::MAX_IDENTIFIER_LEN)?,
            version: reader.read_string(crate::MAX_IDENTIFIER_LEN)?,
        });
    }
    Ok(SelectKnownPacks { packs })
}

fn encode_known_packs(packs: &SelectKnownPacks) -> ServerResult<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(i32::try_from(packs.packs.len()).unwrap_or(i32::MAX));
    for pack in &packs.packs {
        writer.write_string(&pack.namespace)?;
        writer.write_string(&pack.id)?;
        writer.write_string(&pack.version)?;
    }
    Ok(writer.finish())
}

/// Clientbound id of `minecraft:select_known_packs` (same body as serverbound).
pub const SELECT_KNOWN_PACKS_CLIENTBOUND: i32 = clientbound::config::SELECT_KNOWN_PACKS;

impl Packet for SelectKnownPacks {
    const ID: i32 = serverbound::config::SELECT_KNOWN_PACKS;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        decode_known_packs(payload)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        encode_known_packs(self)
    }
}

/// `minecraft:client_information` (configuration serverbound 0, also play 14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation {
    /// Client locale, e.g. `en_us`.
    pub locale: String,
    /// Render distance in chunks.
    pub view_distance: i8,
    /// Chat visibility (0 enabled, 1 commands only, 2 hidden).
    pub chat_mode: i32,
    /// Whether chat formatting is rendered.
    pub chat_colors: bool,
    /// Skin-part bitmask.
    pub skin_parts: u8,
    /// Dominant hand (0 left, 1 right).
    pub main_hand: i32,
    /// Whether text filtering is enabled.
    pub text_filtering: bool,
    /// Whether the player allows server-list display.
    pub server_listing: bool,
}

impl ClientInformation {
    /// Decode the shared body (both configuration and play ids use it).
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on malformed data.
    pub fn decode_body(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        Ok(Self {
            locale: reader.read_string(MAX_LOCALE_CHARS)?,
            view_distance: reader.read_i8()?,
            chat_mode: reader.read_varint()?,
            chat_colors: reader.read_bool()?,
            skin_parts: reader.read_u8()?,
            main_hand: reader.read_varint()?,
            text_filtering: reader.read_bool()?,
            server_listing: reader.read_bool()?,
        })
    }

    /// Encode the shared body (used by both configuration and play ids).
    ///
    /// # Errors
    ///
    /// Same as [`Packet::encode`].
    pub fn encode_body(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_string(&self.locale)?;
        writer.write_i8(self.view_distance);
        writer.write_varint(self.chat_mode);
        writer.write_bool(self.chat_colors);
        writer.write_u8(self.skin_parts);
        writer.write_varint(self.main_hand);
        writer.write_bool(self.text_filtering);
        writer.write_bool(self.server_listing);
        Ok(writer.finish())
    }
}

impl Packet for ClientInformation {
    const ID: i32 = serverbound::config::CLIENT_INFORMATION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Self::decode_body(payload)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        self.encode_body()
    }
}

/// `minecraft:finish_configuration` acknowledgement (serverbound 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FinishConfigurationAck;

impl Packet for FinishConfigurationAck {
    const ID: i32 = serverbound::config::FINISH_CONFIGURATION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(ServerError::Protocol(
                "finish configuration ack must be empty".to_owned(),
            ))
        }
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// `minecraft:finish_configuration` (clientbound 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FinishConfiguration;

impl Packet for FinishConfiguration {
    const ID: i32 = clientbound::config::FINISH_CONFIGURATION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(ServerError::Protocol(
                "finish configuration must be empty".to_owned(),
            ))
        }
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// `minecraft:keep_alive` (configuration, both directions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKeepAlive {
    /// Opaque id echoed by the peer.
    pub id: i64,
}

impl Packet for ConfigKeepAlive {
    const ID: i32 = clientbound::config::KEEP_ALIVE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            id: PacketReader::new(payload).read_i64()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.id);
        Ok(writer.finish())
    }
}

/// `minecraft:pong` (configuration serverbound 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigPong {
    /// Payload echoed from `minecraft:ping`.
    pub id: i32,
}

impl Packet for ConfigPong {
    const ID: i32 = serverbound::config::PONG;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            id: PacketReader::new(payload).read_i32()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i32(self.id);
        Ok(writer.finish())
    }
}

/// `minecraft:update_enabled_features` (clientbound 12).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeatureFlags {
    /// Enabled feature identifiers, e.g. `minecraft:vanilla`.
    pub flags: Vec<String>,
}

impl Packet for FeatureFlags {
    const ID: i32 = clientbound::config::UPDATE_ENABLED_FEATURES;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let count = reader.read_varint()?;
        if !(0..=64).contains(&count) {
            return Err(ServerError::Protocol(format!(
                "feature count out of range: {count}"
            )));
        }
        let mut flags = Vec::with_capacity(count as usize);
        for _ in 0..count {
            flags.push(reader.read_string(crate::MAX_IDENTIFIER_LEN)?);
        }
        Ok(Self { flags })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(i32::try_from(self.flags.len()).unwrap_or(i32::MAX));
        for flag in &self.flags {
            writer.write_string(flag)?;
        }
        Ok(writer.finish())
    }
}

/// One registry entry: an id plus optional element data.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
    /// Entry identifier, e.g. `minecraft:overworld`.
    pub id: String,
    /// Optional element NBT (network NBT, nameless root).
    pub data: Option<Nbt>,
}

/// `minecraft:registry_data` (clientbound 7).
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryData {
    /// Registry identifier, e.g. `minecraft:dimension_type`.
    pub registry: String,
    /// Entries in id order; the index is the numeric id used by `JoinGame`.
    pub entries: Vec<RegistryEntry>,
}

impl Packet for RegistryData {
    const ID: i32 = clientbound::config::REGISTRY_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let registry = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
        let count = reader.read_varint()?;
        if count < 0 {
            return Err(ServerError::Protocol(format!(
                "negative registry count {count}"
            )));
        }
        let count = count as usize;
        // Each entry needs at least an id length byte plus a flag byte.
        if count > reader.remaining() {
            return Err(ServerError::Protocol(format!(
                "registry count {count} exceeds remaining bytes {}",
                reader.remaining()
            )));
        }
        let mut entries = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let id = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
            let data = if reader.read_bool()? {
                let (value, consumed) = {
                    let original = reader.remaining_slice();
                    let mut rest = original;
                    let value = Nbt::read_network(&mut rest)?;
                    (value, original.len() - rest.len())
                };
                reader.advance(consumed)?;
                Some(value)
            } else {
                None
            };
            entries.push(RegistryEntry { id, data });
        }
        Ok(Self { registry, entries })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_string(&self.registry)?;
        writer.write_varint(i32::try_from(self.entries.len()).unwrap_or(i32::MAX));
        for entry in &self.entries {
            writer.write_string(&entry.id)?;
            match &entry.data {
                Some(data) => {
                    writer.write_bool(true);
                    let mut encoded = Vec::new();
                    data.write_network(&mut encoded)?;
                    writer.write_bytes(&encoded);
                }
                None => writer.write_bool(false),
            }
        }
        Ok(writer.finish())
    }
}

/// `minecraft:update_tags` (clientbound 13). Phase 02 sends an empty set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateTags;

impl Packet for UpdateTags {
    const ID: i32 = clientbound::config::UPDATE_TAGS;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let registries = reader.read_varint()?;
        if registries != 0 {
            return Err(ServerError::Protocol(
                "phase-02 update_tags must contain no registries".to_owned(),
            ));
        }
        Ok(Self)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(0);
        Ok(writer.finish())
    }
}

/// `minecraft:disconnect` (configuration clientbound 2). NBT text component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDisconnect {
    /// Reason shown to the client.
    pub reason: TextComponent,
}

impl Packet for ConfigDisconnect {
    const ID: i32 = clientbound::config::DISCONNECT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut rest = payload;
        let nbt = Nbt::read_network(&mut rest)?;
        let reason = text_from_nbt(&nbt)?;
        Ok(Self { reason })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        let mut encoded = Vec::new();
        self.reason.to_nbt().write_network(&mut encoded)?;
        writer.write_bytes(&encoded);
        Ok(writer.finish())
    }
}

/// Extract a literal component from an NBT text tree.
///
/// # Errors
///
/// [`ServerError::Protocol`] when the shape is not a literal component.
pub fn text_from_nbt(nbt: &Nbt) -> ServerResult<TextComponent> {
    match nbt {
        Nbt::Compound(entries) => {
            for (name, value) in entries {
                if name == "text"
                    && let Nbt::String(text) = value
                {
                    return Ok(TextComponent::literal(text.clone()));
                }
            }
            Ok(TextComponent::literal(String::new()))
        }
        Nbt::String(text) => Ok(TextComponent::literal(text.clone())),
        _ => Err(ServerError::Protocol(
            "unsupported text component shape".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientInformation, ConfigDisconnect, ConfigKeepAlive, FeatureFlags, FinishConfiguration,
        RegistryData, RegistryEntry, SELECT_KNOWN_PACKS_CLIENTBOUND, SelectKnownPacks,
    };
    use crate::ids::clientbound;
    use crate::nbt::Nbt;
    use crate::packets::Packet;
    use crate::text::TextComponent;

    #[test]
    fn client_information_round_trip() {
        let packet = ClientInformation {
            locale: "en_us".to_owned(),
            view_distance: 12,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7F,
            main_hand: 1,
            text_filtering: false,
            server_listing: true,
        };
        assert_eq!(
            ClientInformation::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn known_packs_round_trip() {
        let packet = SelectKnownPacks {
            packs: vec![super::KnownPack {
                namespace: "minecraft".to_owned(),
                id: "core".to_owned(),
                version: "26.1.2".to_owned(),
            }],
        };
        assert_eq!(
            SelectKnownPacks::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
        // Same body serves the clientbound variant (id differs at framing).
        assert_eq!(
            SELECT_KNOWN_PACKS_CLIENTBOUND,
            clientbound::config::SELECT_KNOWN_PACKS
        );
    }

    #[test]
    fn feature_flags_round_trip() {
        let packet = FeatureFlags {
            flags: vec!["minecraft:vanilla".to_owned()],
        };
        assert_eq!(
            FeatureFlags::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn registry_data_round_trip_with_nbt() {
        let packet = RegistryData {
            registry: "minecraft:dimension_type".to_owned(),
            entries: vec![RegistryEntry {
                id: "minecraft:overworld".to_owned(),
                data: Some(Nbt::Compound(vec![("height".to_owned(), Nbt::Int(384))])),
            }],
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(RegistryData::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn registry_data_rejects_hostile_count() {
        let mut writer = crate::wire::PacketWriter::new();
        writer
            .write_string("minecraft:dimension_type")
            .expect("writes");
        writer.write_varint(i32::MAX);
        assert!(RegistryData::decode(&writer.finish()).is_err());
    }

    #[test]
    fn disconnect_uses_nbt_text() {
        let packet = ConfigDisconnect {
            reason: TextComponent::literal("bye"),
        };
        let bytes = packet.encode().expect("encodes");
        assert_eq!(ConfigDisconnect::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn empty_packets_are_strict() {
        assert!(FinishConfiguration::decode(&[]).is_ok());
        assert!(FinishConfiguration::decode(&[0x01]).is_err());
    }

    #[test]
    fn config_keepalive_round_trip() {
        let packet = ConfigKeepAlive { id: -5 };
        assert_eq!(
            ConfigKeepAlive::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }
}
