//! Minimal configuration-phase registry payload (P02-10).
//!
//! A client cannot enter Play without a `minecraft:dimension_type` entry to
//! resolve the `JoinGame` dimension. Phase 02 ships a single overworld entry and
//! a single plains-like biome, authored from the public 26.1 datapack schema
//! observed in the reference assets (`assets/datapacks/26_1/data/minecraft/
//! {dimension_type/overworld.json,worldgen/biome/plains.json}`).
//!
//! **Status: provisional.** The element schema moves between versions (26.1
//! introduced `attributes`/`timelines`/`default_clock` in the datapack JSON),
//! and this environment has no real 26.1.2 client to verify against. A real
//! client test is required before any compatibility claim; the gap is tracked
//! in `docs/phases/PHASE-02-REPORT.md`. Full registry loading is P04-01/P07-03.

use mc_core::error::ServerResult;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_protocol::nbt::Nbt;
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    FeatureFlags, FinishConfiguration, KnownPack, RegistryData, RegistryEntry, SelectKnownPacks,
    UpdateTags,
};

/// Identifier of the single dimension served in Phase 02.
pub const OVERWORLD: &str = "minecraft:overworld";
/// Identifier of the single biome served in Phase 02.
pub const PLAINS: &str = "minecraft:plains";

/// Minimal `minecraft:dimension_type` element for the overworld.
///
/// Values follow the 26.1 datapack schema (field names are wire facts); the
/// attribute section is deliberately trimmed to the sky colour.
#[must_use]
pub fn overworld_dimension_type() -> Nbt {
    Nbt::Compound(vec![
        ("has_skylight".to_owned(), Nbt::Byte(1)),
        ("has_ceiling".to_owned(), Nbt::Byte(0)),
        ("has_ender_dragon_fight".to_owned(), Nbt::Byte(0)),
        ("coordinate_scale".to_owned(), Nbt::Double(1.0)),
        ("min_y".to_owned(), Nbt::Int(-64)),
        ("height".to_owned(), Nbt::Int(384)),
        ("logical_height".to_owned(), Nbt::Int(384)),
        (
            "infiniburn".to_owned(),
            Nbt::String("#minecraft:infiniburn_overworld".to_owned()),
        ),
        ("ambient_light".to_owned(), Nbt::Float(0.0)),
        (
            "monster_spawn_light_level".to_owned(),
            Nbt::Compound(vec![
                ("min_inclusive".to_owned(), Nbt::Int(0)),
                ("max_inclusive".to_owned(), Nbt::Int(7)),
                (
                    "type".to_owned(),
                    Nbt::String("minecraft:uniform".to_owned()),
                ),
            ]),
        ),
        ("monster_spawn_block_light_limit".to_owned(), Nbt::Int(0)),
        (
            "attributes".to_owned(),
            Nbt::Compound(vec![(
                "minecraft:visual/sky_color".to_owned(),
                Nbt::String("#78a7ff".to_owned()),
            )]),
        ),
        (
            "timelines".to_owned(),
            Nbt::String("#minecraft:in_overworld".to_owned()),
        ),
        (
            "default_clock".to_owned(),
            Nbt::String("minecraft:overworld".to_owned()),
        ),
    ])
}

/// Minimal `minecraft:worldgen/biome` element for plains.
#[must_use]
pub fn plains_biome() -> Nbt {
    Nbt::Compound(vec![
        ("has_precipitation".to_owned(), Nbt::Byte(1)),
        ("temperature".to_owned(), Nbt::Float(0.8)),
        ("downfall".to_owned(), Nbt::Float(0.4)),
        (
            "attributes".to_owned(),
            Nbt::Compound(vec![(
                "minecraft:visual/sky_color".to_owned(),
                Nbt::String("#78a7ff".to_owned()),
            )]),
        ),
        (
            "effects".to_owned(),
            Nbt::Compound(vec![(
                "water_color".to_owned(),
                Nbt::String("#3f76e4".to_owned()),
            )]),
        ),
    ])
}

/// Build the full configuration packet sequence sent after `LoginAcknowledged`.
///
/// # Errors
///
/// [`mc_core::error::ServerError`] on encoding invariants (never wire input).
pub fn configuration_packets(version_name: &str) -> ServerResult<Vec<RawPacket>> {
    let mut packets = Vec::new();

    let known = SelectKnownPacks {
        packs: vec![KnownPack {
            namespace: "minecraft".to_owned(),
            id: "core".to_owned(),
            version: version_name.to_owned(),
        }],
    };
    packets.push(RawPacket::new(
        clientbound::config::SELECT_KNOWN_PACKS,
        known.encode()?,
    ));

    packets.push(
        FeatureFlags {
            flags: vec!["minecraft:vanilla".to_owned()],
        }
        .to_raw()?,
    );

    packets.push(
        RegistryData {
            registry: "minecraft:dimension_type".to_owned(),
            entries: vec![RegistryEntry {
                id: OVERWORLD.to_owned(),
                data: Some(overworld_dimension_type()),
            }],
        }
        .to_raw()?,
    );

    packets.push(
        RegistryData {
            registry: "minecraft:worldgen/biome".to_owned(),
            entries: vec![RegistryEntry {
                id: PLAINS.to_owned(),
                data: Some(plains_biome()),
            }],
        }
        .to_raw()?,
    );

    packets.push(UpdateTags.to_raw()?);
    packets.push(FinishConfiguration.to_raw()?);
    Ok(packets)
}

/// Id of the empty serverbound acknowledgement for reference in dispatch.
pub const FINISH_CONFIGURATION_ACK: i32 = serverbound::config::FINISH_CONFIGURATION;

#[cfg(test)]
mod tests {
    use super::{configuration_packets, overworld_dimension_type, plains_biome};
    use mc_protocol::ids::clientbound;
    use mc_protocol::nbt::Nbt;
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::config::{FinishConfiguration, RegistryData, SelectKnownPacks};

    #[test]
    fn packet_sequence_is_complete_and_ordered() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let ids: Vec<i32> = packets.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                clientbound::config::SELECT_KNOWN_PACKS,
                clientbound::config::UPDATE_ENABLED_FEATURES,
                clientbound::config::REGISTRY_DATA,
                clientbound::config::REGISTRY_DATA,
                clientbound::config::UPDATE_TAGS,
                clientbound::config::FINISH_CONFIGURATION,
            ]
        );
    }

    #[test]
    fn registry_entries_round_trip_through_wire_codec() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let registry_packets: Vec<_> = packets
            .iter()
            .filter(|p| p.id == clientbound::config::REGISTRY_DATA)
            .collect();
        assert_eq!(registry_packets.len(), 2);
        let dimensions = RegistryData::decode(&registry_packets[0].payload).expect("decodes");
        assert_eq!(dimensions.registry, "minecraft:dimension_type");
        assert_eq!(dimensions.entries.len(), 1);
        assert_eq!(dimensions.entries[0].id, "minecraft:overworld");
        assert!(dimensions.entries[0].data.is_some());

        let biomes = RegistryData::decode(&registry_packets[1].payload).expect("decodes");
        assert_eq!(biomes.registry, "minecraft:worldgen/biome");

        let finish = FinishConfiguration::decode(&packets[5].payload).expect("decodes");
        let _ = finish;
    }

    #[test]
    fn select_known_packs_packet_carries_core_version() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let known = SelectKnownPacks::decode(&packets[0].payload).expect("decodes");
        assert_eq!(known.packs.len(), 1);
        assert_eq!(known.packs[0].namespace, "minecraft");
        assert_eq!(known.packs[0].id, "core");
        assert_eq!(known.packs[0].version, "26.1.2");
    }

    #[test]
    fn dimension_type_has_required_fields() {
        let Nbt::Compound(entries) = overworld_dimension_type() else {
            panic!("expected compound");
        };
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        for required in [
            "min_y",
            "height",
            "logical_height",
            "coordinate_scale",
            "has_skylight",
        ] {
            assert!(names.contains(&required), "missing {required}");
        }
        let Nbt::Compound(biome) = plains_biome() else {
            panic!("expected compound");
        };
        assert!(biome.iter().any(|(name, _)| name == "temperature"));
    }
}
