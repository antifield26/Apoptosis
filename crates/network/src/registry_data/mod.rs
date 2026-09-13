//! Configuration-phase registry payload (P02-10, P10-03).
//!
//! A client cannot enter Play without a `minecraft:dimension_type` entry to resolve the `JoinGame` dimension,
//! and a **26.1.2** client additionally requires the registries that entry *references*:
//!
//! | The entry declares | Which requires |
//! |---|---|
//! | `timelines: "#minecraft:in_overworld"` | the `minecraft:timeline` registry, and that tag bound in `update_tags` |
//! | `default_clock: "minecraft:overworld"` | the `minecraft:world_clock` registry, containing that value |
//!
//! Until P10-03 neither was sent, and `UpdateTags` carried nothing at all. A real client refused the session in
//! its registry loader and named both references, which is KD-39. The dimension and biome elements below are
//! hand-authored from the datapack schema; the timeline and clock content is **extracted from the jar's own
//! pack** by `tools/vanilla-probe/extract_synced_registries.py` and embedded at compile time, the same
//! pipeline that produces `blocks.tsv` and `items.tsv`.

use mc_core::error::{ServerError, ServerResult};
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_protocol::nbt::Nbt;
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    FeatureFlags, FinishConfiguration, KnownPack, RegistryData, RegistryEntry, SelectKnownPacks,
    Tag, TagRegistry, UpdateTags,
};
use serde_json::Value;

/// Identifier of the single dimension served.
pub const OVERWORLD: &str = "minecraft:overworld";
/// Identifier of the single biome served.
pub const PLAINS: &str = "minecraft:plains";

/// The registries and tags a 26.1.2 client requires, extracted from the jar's pack.
const SYNCED_REGISTRIES: &str = include_str!("synced-registries.json");

/// Minimal `minecraft:dimension_type` element for the overworld.
///
/// Values follow the 26.1 datapack schema (field names are wire facts); the attribute section is deliberately
/// trimmed to the sky colour. `timelines` and `default_clock` are **references**, and P10-03 is what made the
/// registries behind them exist.
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

/// Convert one JSON value from the fixture into network NBT.
///
/// # Derived rules, not verified facts
///
/// * **Non-integer numbers become `Float`.** JSON has one number type; Minecraft's codec has two, and every
///   non-integer in the vanilla timelines (track `value`s and `cubic_bezier` coefficients) is float-typed. A
///   wrong guess here is exactly the kind of failure a real client reports and names, which is how this rule
///   is adjudicated rather than assumed.
/// * **Booleans become `Byte` 0/1**, because NBT has no boolean and Vanilla writes bytes.
/// * **Integers become `Int`**, widening to `Long` only if they do not fit.
///
/// # Errors
///
/// [`ServerError::Protocol`] for a JSON shape NBT cannot represent (`null`, or a number that is neither).
fn json_to_nbt(value: &Value) -> ServerResult<Nbt> {
    Ok(match value {
        Value::Bool(flag) => Nbt::Byte(i8::from(*flag)),
        Value::String(text) => Nbt::String(text.clone()),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                match i32::try_from(integer) {
                    Ok(narrow) => Nbt::Int(narrow),
                    Err(_) => Nbt::Long(integer),
                }
            } else if let Some(float) = number.as_f64() {
                // `as f64 -> f32` loses precision, and that is intended: the target field is a float.
                #[allow(clippy::cast_possible_truncation)]
                Nbt::Float(float as f32)
            } else {
                return Err(ServerError::Protocol(format!(
                    "registry number {number} is neither an integer nor a float"
                )));
            }
        }
        Value::Array(items) => {
            // NBT lists are **homogeneous**: `mc_nbt` writes the element type once, from the first element,
            // and then bare payloads. Building a list from mixed JSON would declare one type and write
            // another, which decodes as garbage — 44 tests caught exactly that when `villager_trade` was
            // added, because vanilla encodes `number_of_dyes.summands` with a dispatch codec (a number *or*
            // an object) and JSON shape cannot express that.
            //
            // So a mixed array is refused by name. Guessing the dispatch encoding is what this project does
            // not do; the probe refuses these at extraction time for the same reason.
            let mut element_kind: Option<std::mem::Discriminant<Nbt>> = None;
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                let value = json_to_nbt(item)?;
                let kind = std::mem::discriminant(&value);
                match element_kind {
                    None => element_kind = Some(kind),
                    Some(seen) if seen == kind => {}
                    Some(_) => {
                        return Err(ServerError::Protocol(
                            "a registry array mixes element types; NBT lists are homogeneous, so this needs \
                             the field's schema rather than its shape"
                                .to_owned(),
                        ));
                    }
                }
                converted.push(value);
            }
            Nbt::List(converted)
        }
        Value::Object(fields) => {
            let mut converted = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                converted.push((name.clone(), json_to_nbt(field)?));
            }
            Nbt::Compound(converted)
        }
        Value::Null => {
            return Err(ServerError::Protocol(
                "a registry element contains null, which NBT cannot represent".to_owned(),
            ));
        }
    })
}

/// The fixture, parsed once per call.
///
/// # Errors
///
/// [`ServerError::Protocol`] if the embedded fixture is not the expected shape. It is compiled in, so this is
/// a build-time invariant rather than wire input.
fn fixture() -> ServerResult<Value> {
    serde_json::from_str(SYNCED_REGISTRIES)
        .map_err(|error| ServerError::Protocol(format!("synced-registries.json: {error}")))
}

/// Registry elements from the fixture, in the fixture's (sorted) order.
///
/// The order is the **wire id order**, and the tags below refer to entries by that id, so it must not change
/// without the tags changing with it. It is sorted, which matches the order the probe emitted.
///
/// # Errors
///
/// [`ServerError::Protocol`] if the fixture is malformed.
pub fn synced_registry(registry: &str) -> ServerResult<Vec<RegistryEntry>> {
    let document = fixture()?;
    let entries = document
        .get(registry)
        .and_then(Value::as_object)
        .ok_or_else(|| ServerError::Protocol(format!("fixture has no {registry} registry")))?;
    let mut out = Vec::with_capacity(entries.len());
    for (id, element) in entries {
        out.push(RegistryEntry {
            id: id.clone(),
            data: Some(json_to_nbt(element)?),
        });
    }
    Ok(out)
}

/// The registry section names in the fixture, sorted — which is the wire id order.
///
/// The server sends whatever the fixture holds rather than a list of its own: the probe already owns that
/// list, and duplicating it here would go stale the first time the probe grew.
///
/// # Errors
///
/// [`ServerError::Protocol`] if the fixture is malformed.
pub fn synced_registry_names() -> ServerResult<Vec<String>> {
    let document = fixture()?;
    let sections = document
        .as_object()
        .ok_or_else(|| ServerError::Protocol("fixture is not an object".to_owned()))?;
    Ok(sections
        .keys()
        .filter(|name| name.as_str() != "tags")
        .cloned()
        .collect())
}

/// The `update_tags` payload for the synced registries.
///
/// Tag members are **numeric ids** into the registry, so each name is mapped through the entry order from
/// [`synced_registry`]. The fixture already resolved `#minecraft:universal` into its members, because a tag
/// cannot be nested inside a tag on the wire.
///
/// # Errors
///
/// [`ServerError::Protocol`] if a tag names an entry the registry does not contain — which would be a fixture
/// bug, not wire input.
pub fn synced_tags() -> ServerResult<UpdateTags> {
    let document = fixture()?;
    let tags_by_registry = document
        .get("tags")
        .and_then(Value::as_object)
        .ok_or_else(|| ServerError::Protocol("fixture has no tags section".to_owned()))?;

    let mut registries = Vec::with_capacity(tags_by_registry.len());
    for (registry, tags) in tags_by_registry {
        // The fixture keys registry sections by their bare pack directory name (`timeline`) and the tags
        // section by the protocol's namespaced registry name (`minecraft:timeline`). Reconciling the two here
        // is explicit because getting it wrong is silent: the lookup simply finds nothing.
        let bare = registry.strip_prefix("minecraft:").unwrap_or(registry);
        let entries = synced_registry(bare)?;
        let index: std::collections::BTreeMap<&str, i32> = entries
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                #[allow(clippy::cast_possible_truncation)]
                (entry.id.as_str(), position as i32)
            })
            .collect();

        let tags = tags.as_object().ok_or_else(|| {
            ServerError::Protocol(format!("tags for {registry} are not an object"))
        })?;
        let mut out = Vec::with_capacity(tags.len());
        for (name, members) in tags {
            let members = members
                .as_array()
                .ok_or_else(|| ServerError::Protocol(format!("tag {name} is not an array")))?;
            let mut ids = Vec::with_capacity(members.len());
            for member in members {
                let member = member.as_str().ok_or_else(|| {
                    ServerError::Protocol(format!("tag {name} has a non-string member"))
                })?;
                let id = index.get(member).ok_or_else(|| {
                    ServerError::Protocol(format!(
                        "tag {name} names {member}, which is not in {registry}"
                    ))
                })?;
                ids.push(*id);
            }
            out.push(Tag {
                // The pack stores tags without a namespace directory, so they are all `minecraft:`.
                name: format!("minecraft:{name}"),
                entries: ids,
            });
        }
        registries.push(TagRegistry {
            // Already namespaced in the fixture; left as-is rather than re-prefixed.
            registry: registry.clone(),
            tags: out,
        });
    }

    Ok(UpdateTags { registries })
}

/// Build the full configuration packet sequence sent after `LoginAcknowledged`.
///
/// # Errors
///
/// [`ServerError`] on encoding invariants or a malformed embedded fixture.
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

    // Every registry the fixture holds, in its sorted order.
    //
    // The first two added were the ones the overworld dimension *references* (`timelines` and `default_clock`),
    // which a real client refuses to resolve when absent — KD-39. A second real-client run then named thirteen
    // more with "Registry must be non-empty", so the list now comes from the fixture rather than from the
    // server, and the probe owns it.
    for registry in synced_registry_names()? {
        packets.push(
            RegistryData {
                registry: format!("minecraft:{registry}"),
                entries: synced_registry(&registry)?,
            }
            .to_raw()?,
        );
    }

    // Tags must follow the registries they index.
    packets.push(synced_tags()?.to_raw()?);
    packets.push(FinishConfiguration.to_raw()?);
    Ok(packets)
}

/// Id of the empty serverbound acknowledgement for reference in dispatch.
pub const FINISH_CONFIGURATION_ACK: i32 = serverbound::config::FINISH_CONFIGURATION;

#[cfg(test)]
mod tests {
    use super::{
        PLAINS, configuration_packets, json_to_nbt, overworld_dimension_type, plains_biome,
        synced_registry, synced_registry_names, synced_tags,
    };
    use mc_protocol::ids::clientbound;
    use mc_protocol::nbt::Nbt;
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::config::{
        FinishConfiguration, RegistryData, SelectKnownPacks, UpdateTags,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Every registry packet in the payload, keyed by registry name.
    fn registries() -> BTreeMap<String, RegistryData> {
        configuration_packets("26.1.2")
            .expect("builds")
            .iter()
            .filter(|packet| packet.id == clientbound::config::REGISTRY_DATA)
            .map(|packet| {
                let decoded = RegistryData::decode(&packet.payload).expect("decodes");
                (decoded.registry.clone(), decoded)
            })
            .collect()
    }

    #[test]
    fn packet_sequence_is_complete_and_ordered() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let ids: Vec<i32> = packets.iter().map(|packet| packet.id).collect();

        // Shape rather than an exact vector: an exact list would need editing on every registry added and would
        // say nothing about the ordering constraint that actually matters.
        assert_eq!(ids.first(), Some(&clientbound::config::SELECT_KNOWN_PACKS));
        assert_eq!(
            ids.get(1),
            Some(&clientbound::config::UPDATE_ENABLED_FEATURES)
        );
        assert_eq!(ids.last(), Some(&clientbound::config::FINISH_CONFIGURATION));

        let registry_count = synced_registry_names().expect("names").len() + 2;
        let registry_packets = ids
            .iter()
            .filter(|id| **id == clientbound::config::REGISTRY_DATA)
            .count();
        assert_eq!(
            registry_packets, registry_count,
            "the two hand-authored registries plus every registry in the fixture"
        );

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
            "and only finish_configuration follows the tags"
        );
    }

    /// The KD-39 invariant: a reference in one part of the payload must resolve in another.
    #[test]
    fn every_reference_the_overworld_declares_is_actually_sent() {
        let sent = registries();
        let dimension = sent
            .get("minecraft:dimension_type")
            .expect("the dimension registry is sent");
        let Nbt::Compound(fields) = dimension.entries[0]
            .data
            .as_ref()
            .expect("the dimension has an element")
        else {
            panic!("the dimension element should be a compound");
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
        };

        // `timelines` is a tag reference: `#minecraft:in_overworld`.
        let Nbt::String(timelines) = field("timelines").expect("timelines is declared") else {
            panic!("timelines should be a string");
        };
        let tag_name = timelines
            .strip_prefix('#')
            .expect("a tag reference starts with '#'");
        let tags = synced_tags().expect("tags build");
        let timeline_tags = tags
            .registries
            .iter()
            .find(|registry| registry.registry == "minecraft:timeline")
            .expect("timeline tags are sent");
        let timeline_entries = synced_registry("timeline").expect("timeline registry builds");
        let in_overworld = timeline_tags
            .tags
            .iter()
            .find(|tag| {
                tag.name == format!("minecraft:{}", tag_name.trim_start_matches("minecraft:"))
            })
            .unwrap_or_else(|| {
                panic!(
                    "the dimension references {timelines}, which is not among the sent tags: {:?}",
                    timeline_tags
                        .tags
                        .iter()
                        .map(|tag| &tag.name)
                        .collect::<Vec<_>>()
                )
            });
        assert!(
            !in_overworld.entries.is_empty(),
            "a bound but empty tag would still leave the reference meaningless"
        );
        for id in &in_overworld.entries {
            assert!(
                (*id as usize) < timeline_entries.len(),
                "tag member id {id} is outside the {} timeline entries sent — the tag indexes a registry \
                 the client does not have that shape of",
                timeline_entries.len()
            );
        }

        // `default_clock` is a value reference into `world_clock`.
        let Nbt::String(clock) = field("default_clock").expect("default_clock is declared") else {
            panic!("default_clock should be a string");
        };
        let clocks = sent
            .get("minecraft:world_clock")
            .expect("the world_clock registry is sent");
        assert!(
            clocks.entries.iter().any(|entry| &entry.id == clock),
            "the dimension declares default_clock {clock}, which the world_clock registry does not contain: \
             {:?}",
            clocks
                .entries
                .iter()
                .map(|entry| &entry.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_synced_registries_carry_the_jars_content() {
        let clocks = synced_registry("world_clock").expect("builds");
        let clock_ids: Vec<&str> = clocks.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(clock_ids, vec!["minecraft:overworld", "minecraft:the_end"]);

        let timelines = synced_registry("timeline").expect("builds");
        let ids: Vec<&str> = timelines.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "minecraft:day",
                "minecraft:early_game",
                "minecraft:moon",
                "minecraft:villager_schedule",
            ],
            "the entry order is the wire id order, and the tags depend on it"
        );
        // The content is real, not a stub: `day` carries its time markers and period.
        let Nbt::Compound(day) = timelines[0].data.as_ref().expect("day has data") else {
            panic!("a timeline element should be a compound");
        };
        assert!(
            day.iter().any(|(name, _)| name == "time_markers"),
            "a hand-stubbed timeline would not have time_markers"
        );
    }

    #[test]
    fn the_in_overworld_tag_is_resolved_not_nested() {
        // The pack writes `in_overworld` containing `#minecraft:universal`. The wire carries numeric ids, so a
        // nested tag has to be resolved before sending; sending the reference itself is not representable.
        let tags = synced_tags().expect("builds");
        let timeline = tags
            .registries
            .iter()
            .find(|registry| registry.registry == "minecraft:timeline")
            .expect("timeline tags");
        let by_name: BTreeMap<&str, &Vec<i32>> = timeline
            .tags
            .iter()
            .map(|tag| (tag.name.as_str(), &tag.entries))
            .collect();
        let in_overworld = by_name
            .get("minecraft:in_overworld")
            .expect("in_overworld is sent");
        let universal = by_name
            .get("minecraft:universal")
            .expect("universal is sent");
        assert_eq!(
            in_overworld.len(),
            4,
            "in_overworld is #universal plus day, moon and early_game"
        );
        assert_eq!(universal.len(), 1, "universal is villager_schedule");
        for member in *universal {
            assert!(
                in_overworld.contains(member),
                "resolving the nested tag must keep its members"
            );
        }
    }

    #[test]
    fn tags_round_trip_through_the_wire_codec() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let tags_packet = packets
            .iter()
            .find(|packet| packet.id == clientbound::config::UPDATE_TAGS)
            .expect("a tags packet is sent");
        let decoded = UpdateTags::decode(&tags_packet.payload).expect("decodes");
        assert!(
            !decoded.registries.is_empty(),
            "tags must not be empty (KD-39)"
        );
        let original = synced_tags().expect("builds");
        assert_eq!(decoded, original, "the tags must survive the wire codec");
    }

    #[test]
    fn registry_entries_round_trip_through_wire_codec() {
        let packets = configuration_packets("26.1.2").expect("builds");
        let registry_packets: Vec<_> = packets
            .iter()
            .filter(|packet| packet.id == clientbound::config::REGISTRY_DATA)
            .collect();
        assert_eq!(
            registry_packets.len(),
            synced_registry_names().expect("names").len() + 2
        );

        let dimensions = RegistryData::decode(&registry_packets[0].payload).expect("decodes");
        assert_eq!(dimensions.registry, "minecraft:dimension_type");
        assert_eq!(dimensions.entries.len(), 1);
        assert_eq!(dimensions.entries[0].id, "minecraft:overworld");
        assert!(dimensions.entries[0].data.is_some());

        let biomes = RegistryData::decode(&registry_packets[1].payload).expect("decodes");
        assert_eq!(biomes.registry, "minecraft:worldgen/biome");
        assert_eq!(biomes.entries[0].id, PLAINS);

        // Every fixture registry follows, and every one must be non-empty: a real client refuses an empty
        // synced registry, which is what the second first-contact run reported.
        let mut seen = Vec::new();
        for packet in &registry_packets[2..] {
            let decoded = RegistryData::decode(&packet.payload).expect("decodes");
            assert!(
                !decoded.entries.is_empty(),
                "{} is sent empty, which a real client refuses",
                decoded.registry
            );
            seen.push(decoded.registry.clone());
        }
        for expected in [
            "minecraft:world_clock",
            "minecraft:timeline",
            "minecraft:painting_variant",
        ] {
            assert!(
                seen.contains(&expected.to_owned()),
                "missing {expected} in {seen:?}"
            );
        }

        let finish = packets.last().expect("a last packet");
        FinishConfiguration::decode(&finish.payload).expect("decodes");
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
            "timelines",
            "default_clock",
        ] {
            assert!(names.contains(&required), "missing {required}");
        }
        let Nbt::Compound(biome) = plains_biome() else {
            panic!("expected compound");
        };
        assert!(biome.iter().any(|(name, _)| name == "temperature"));
    }

    #[test]
    fn json_maps_to_nbt_by_the_documented_rules() {
        // The float rule is a derivation, not a verified fact: JSON has one number type and Minecraft's codec
        // has two. Pinning it here means a change is deliberate, and the real client adjudicates it.
        assert_eq!(json_to_nbt(&json!(true)).expect("bool"), Nbt::Byte(1));
        assert_eq!(json_to_nbt(&json!(false)).expect("bool"), Nbt::Byte(0));
        assert_eq!(json_to_nbt(&json!(24000)).expect("int"), Nbt::Int(24000));
        assert_eq!(json_to_nbt(&json!(0.5)).expect("float"), Nbt::Float(0.5));
        assert_eq!(
            json_to_nbt(&json!(3_000_000_000_i64)).expect("wide"),
            Nbt::Long(3_000_000_000),
            "an integer too wide for i32 widens rather than truncating"
        );
        assert_eq!(
            json_to_nbt(&json!("minecraft:overworld")).expect("string"),
            Nbt::String("minecraft:overworld".to_owned())
        );
        assert_eq!(
            json_to_nbt(&json!([1, 2])).expect("array"),
            Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])
        );
        assert_eq!(
            json_to_nbt(&json!({"a": true})).expect("object"),
            Nbt::Compound(vec![("a".to_owned(), Nbt::Byte(1))])
        );
        assert!(
            json_to_nbt(&json!(null)).is_err(),
            "null has no NBT representation, so it is refused rather than guessed"
        );
    }
}
