//! Configuration-phase registry payload, replayed from a captured vanilla 26.1.2 server (P10-03).
//!
//! ## What is sent, and why it is not what you would expect
//!
//! A vanilla server sends **entry ids only**. Captured through the P10-01 rig: 382 entries across 28
//! registries in 8 781 bytes, with every `has_data` flag false, followed by 32 316 bytes of tags. The element
//! content is *not* transmitted because the client already has it — it declared `minecraft:core = 26.1.2`
//! under `select_known_packs`, so the server sends the registry shape and no more.
//!
//! That is why this module replays bytes instead of constructing them. An earlier version converted the jar's
//! data-pack JSON into NBT and sent full element data, which failed twice over: it could not express
//! `enchantment`'s dispatch codecs or `villager_trade`'s mixed-type arrays, **and** it was sending data the
//! protocol never asks for, which the client then tried to parse. Both were self-inflicted.
//!
//! ## The dependency this creates
//!
//! An ids-only payload is correct only for a client that has `minecraft:core`. Every 26.1.2 client does, and
//! [`configuration_packets`] advertises it, but the dependency is real: see
//! `the_payload_is_ids_only_and_therefore_requires_the_known_pack`.

use mc_core::error::{ServerError, ServerResult};
use mc_protocol::RawPacket;
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    FeatureFlags, FinishConfiguration, KnownPack, SelectKnownPacks,
};

/// Identifier of the single dimension this server serves.
pub const OVERWORLD: &str = "minecraft:overworld";
/// Identifier of the biome the world is made of.
pub const PLAINS: &str = "minecraft:plains";

/// The namespace/id/version advertised to clients, which is what licenses an ids-only payload.
pub const KNOWN_PACK: (&str, &str, &str) = ("minecraft", "core", "26.1.2");

/// The captured clientbound packets, in wire order.
///
/// Built by `tools/vanilla-probe/build_config_payload.py` from bodies captured off a real vanilla 26.1.2
/// server through the capture rig. Format:
///
/// ```text
/// magic  b"MCRP1"
/// u32    packet_count
/// per packet: u32 id, u32 body length, body
/// ```
const CONFIG_PAYLOAD: &[u8] = include_bytes!("config-payload.bin");

/// Magic marking a captured payload blob.
const MAGIC: &[u8; 5] = b"MCRP1";

/// Read a big-endian `u32` at `at`, or fail.
fn u32_at(bytes: &[u8], at: usize) -> ServerResult<u32> {
    let slice = bytes.get(at..at + 4).ok_or_else(|| {
        ServerError::Protocol(format!("config-payload.bin is truncated at byte {at}"))
    })?;
    let mut value = [0u8; 4];
    value.copy_from_slice(slice);
    Ok(u32::from_be_bytes(value))
}

/// The captured clientbound configuration packets, verbatim and in order: 28 `registry_data` then one
/// `update_tags`.
///
/// # Errors
///
/// [`ServerError::Protocol`] if the embedded blob is malformed. It is compiled in, so this is a build-time
/// invariant rather than wire input.
pub fn captured_payload() -> ServerResult<Vec<RawPacket>> {
    if CONFIG_PAYLOAD.get(..5) != Some(MAGIC.as_slice()) {
        return Err(ServerError::Protocol(
            "config-payload.bin does not start with the expected magic".to_owned(),
        ));
    }
    let count = u32_at(CONFIG_PAYLOAD, 5)? as usize;
    let mut packets = Vec::with_capacity(count);
    let mut at = 9usize;
    for index in 0..count {
        let id = u32_at(CONFIG_PAYLOAD, at)?;
        let length = u32_at(CONFIG_PAYLOAD, at + 4)? as usize;
        let body = CONFIG_PAYLOAD.get(at + 8..at + 8 + length).ok_or_else(|| {
            ServerError::Protocol(format!(
                "config-payload.bin packet {index} runs past the end"
            ))
        })?;
        packets.push(RawPacket::new(
            i32::try_from(id).map_err(|_| {
                ServerError::Protocol(format!("packet id {id} does not fit an i32"))
            })?,
            body.to_vec(),
        ));
        at += 8 + length;
    }
    if at != CONFIG_PAYLOAD.len() {
        return Err(ServerError::Protocol(format!(
            "config-payload.bin has {} trailing bytes",
            CONFIG_PAYLOAD.len() - at
        )));
    }
    Ok(packets)
}

/// Every registry the payload carries, in wire order, as `(registry name, entry count)`.
///
/// The order **is** the numeric id mapping the client uses, and the tags index into it, so it is read from the
/// payload rather than duplicated in a list that could drift.
///
/// # Errors
///
/// [`ServerError::Protocol`] if a registry body cannot be parsed.
pub fn registry_names() -> ServerResult<Vec<(String, i32)>> {
    let mut out = Vec::new();
    for packet in captured_payload()? {
        if packet.id != clientbound::config::REGISTRY_DATA {
            continue;
        }
        let mut reader = mc_protocol::wire::PacketReader::new(&packet.payload);
        let name = reader.read_string(mc_protocol::MAX_IDENTIFIER_LEN)?;
        let count = reader.read_varint()?;
        out.push((name, count));
    }
    Ok(out)
}

/// Build the configuration packet sequence sent after `LoginAcknowledged`.
///
/// Our own `select_known_packs` and `update_enabled_features` lead, then the captured registry and tag
/// payloads verbatim, then `finish_configuration`.
///
/// # Errors
///
/// [`ServerError`] on encoding invariants or a malformed embedded payload.
pub fn configuration_packets(version_name: &str) -> ServerResult<Vec<RawPacket>> {
    let mut packets = Vec::new();

    let (namespace, id, version) = KNOWN_PACK;
    // **Not** `to_raw()`: `SelectKnownPacks`'s `Packet::ID` is the *serverbound* id, because the type models
    // the reply a client sends. Using it here emitted a clientbound packet carrying id 7. Where the two
    // directions share a packet name the id has to be chosen explicitly.
    packets.push(RawPacket::new(
        clientbound::config::SELECT_KNOWN_PACKS,
        SelectKnownPacks {
            packs: vec![KnownPack {
                namespace: namespace.to_owned(),
                id: id.to_owned(),
                version: if version_name.is_empty() {
                    version.to_owned()
                } else {
                    version_name.to_owned()
                },
            }],
        }
        .encode()?,
    ));

    packets.push(
        FeatureFlags {
            flags: vec!["minecraft:vanilla".to_owned()],
        }
        .to_raw()?,
    );

    packets.extend(captured_payload()?);
    packets.push(FinishConfiguration.to_raw()?);
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_PACK, captured_payload, configuration_packets, registry_names};
    use mc_protocol::ids::clientbound;
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::config::{FinishConfiguration, SelectKnownPacks};
    use mc_protocol::wire::PacketReader;

    #[test]
    fn packet_sequence_is_complete_and_ordered() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let ids: Vec<i32> = packets.iter().map(|packet| packet.id).collect();

        assert_eq!(ids.first(), Some(&clientbound::config::SELECT_KNOWN_PACKS));
        assert_eq!(
            ids.get(1),
            Some(&clientbound::config::UPDATE_ENABLED_FEATURES)
        );
        assert_eq!(ids.last(), Some(&clientbound::config::FINISH_CONFIGURATION));

        let tags_at = ids
            .iter()
            .position(|id| *id == clientbound::config::UPDATE_TAGS)
            .expect("a tags packet is sent");
        let last_registry = ids
            .iter()
            .rposition(|id| *id == clientbound::config::REGISTRY_DATA)
            .expect("registries are sent");
        assert!(
            last_registry < tags_at,
            "tags index into the registries, so they must follow them"
        );
        assert_eq!(
            tags_at,
            ids.len() - 2,
            "only finish_configuration follows the tags"
        );
    }

    /// The registries a real client requires must be present. `timeline` and `world_clock` are the two the
    /// overworld `dimension_type` *references*, and their absence was KD-39.
    #[test]
    fn the_registries_a_real_client_needs_are_present() {
        let names: Vec<String> = registry_names()
            .expect("names")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for required in [
            "minecraft:dimension_type",
            "minecraft:worldgen/biome",
            "minecraft:world_clock",
            "minecraft:timeline",
            "minecraft:damage_type",
            // The two the old shape-based converter could not encode and had to exclude.
            "minecraft:enchantment",
        ] {
            assert!(
                names.contains(&required.to_owned()),
                "missing {required} in {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            28,
            "the captured payload carries 28 registries"
        );
    }

    /// The premise of an ids-only payload, asserted rather than assumed.
    #[test]
    fn the_payload_is_ids_only_and_therefore_requires_the_known_pack() {
        // Every entry must have its `has_data` flag false. If a future capture includes element data, this
        // test is where that is noticed, because the known-pack dependency would no longer be the reason.
        let mut entries = 0usize;
        for packet in captured_payload().expect("payload") {
            if packet.id != clientbound::config::REGISTRY_DATA {
                continue;
            }
            let mut reader = PacketReader::new(&packet.payload);
            let _registry = reader
                .read_string(mc_protocol::MAX_IDENTIFIER_LEN)
                .expect("name");
            let count = reader.read_varint().expect("count");
            for _ in 0..count {
                let _id = reader
                    .read_string(mc_protocol::MAX_IDENTIFIER_LEN)
                    .expect("entry id");
                assert!(
                    !reader.read_bool().expect("has_data"),
                    "a captured entry carries element data, so the known-pack negotiation is no longer \
                     the reason the payload is small"
                );
                entries += 1;
            }
        }
        assert_eq!(entries, 382, "the capture holds 382 entries");

        // And the pack is advertised, which is what makes omitting the data legitimate.
        let packets = configuration_packets("26.1.2").expect("builds");
        assert_eq!(
            packets[0].id,
            clientbound::config::SELECT_KNOWN_PACKS,
            "the clientbound id, not the serverbound one the type defaults to"
        );
        let known = SelectKnownPacks::decode(&packets[0].payload).expect("decodes");
        assert_eq!(known.packs.len(), 1);
        assert_eq!(
            (
                known.packs[0].namespace.as_str(),
                known.packs[0].id.as_str()
            ),
            (KNOWN_PACK.0, KNOWN_PACK.1)
        );
    }

    /// `join_game` sends `dimension_type_id: 0`, so entry 0 of that registry must be the overworld.
    #[test]
    fn the_overworld_is_dimension_type_zero() {
        let packet = captured_payload()
            .expect("payload")
            .into_iter()
            .find(|packet| {
                if packet.id != clientbound::config::REGISTRY_DATA {
                    return false;
                }
                let mut reader = PacketReader::new(&packet.payload);
                reader
                    .read_string(mc_protocol::MAX_IDENTIFIER_LEN)
                    .is_ok_and(|name| name == "minecraft:dimension_type")
            })
            .expect("the dimension_type registry is present");
        let mut reader = PacketReader::new(&packet.payload);
        let _registry = reader
            .read_string(mc_protocol::MAX_IDENTIFIER_LEN)
            .expect("name");
        let _count = reader.read_varint().expect("count");
        let first = reader
            .read_string(mc_protocol::MAX_IDENTIFIER_LEN)
            .expect("first entry");
        assert_eq!(
            first,
            super::OVERWORLD,
            "entry 0 must be the overworld, because join_game references dimension_type id 0"
        );
    }

    /// The tags must survive being replayed, and must not be empty — an unbound tag was half of KD-39.
    #[test]
    fn the_captured_tags_are_present_and_non_empty() {
        let tags = captured_payload()
            .expect("payload")
            .into_iter()
            .find(|packet| packet.id == clientbound::config::UPDATE_TAGS)
            .expect("a tags packet is captured");
        let mut reader = PacketReader::new(&tags.payload);
        let registries = reader.read_varint().expect("registry count");
        assert!(registries > 0, "the captured tags packet is empty");
    }

    #[test]
    fn finish_configuration_round_trips() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let last = packets.last().expect("a last packet");
        FinishConfiguration::decode(&last.payload).expect("decodes");
    }
}
