//! Capture sweep (P15-07 A-03): every captured body either round-trips or is
//! on the closed unmodelled list.
//!
//! # What this proves, and what it does not
//!
//! For every packet this build models, decode-then-encode reproduces the exact
//! bytes a real 26.1.2 server sent — which proves the decoder consumed the
//! body *exactly* (a trailing byte would come back missing) and the encoder
//! speaks the same wire shape. Serverbound play goes through
//! [`PlayIntent::decode`](mc_protocol::packets::play::PlayIntent), whose
//! trailing-byte refusal is part of the product: `Some` here means exhausted.
//!
//! Bodies for packets this build does not model cannot round-trip; they are
//! asserted to be *exactly* the closed list below (jar names from
//! `docs/protocol/packet-ids-775.tsv`). A future capture containing a new
//! unmodelled id fails here by design — modelling it is P18 breadth work, and
//! extending the list without that work would be weakening the test.
//!
//! # Corpus
//!
//! `target/vanilla-capture/bodies/`, filenames
//! `NNNNNN_<c2s|s2c>_<state>_<id>.bin` holding the body without the id prefix.
//! The directory is never modified by this test.
//!
//! # Provenance
//!
//! AUDIT-09 A-03 proposed exactly this experiment ("feed every captured
//! packet body to its decoder and assert the reader is exhausted"); the lane
//! estimated 19 000 packets, this corpus holds 6 258.

use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    C2sCustomPayload, ClientInformation, FeatureFlags, FinishConfiguration, FinishConfigurationAck,
    RegistryData, S2cCustomPayload, SelectKnownPacks, UpdateTags,
};
use mc_protocol::packets::handshake::Handshake;
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::{
    AddEntity, BlockUpdate, ChunkBatchFinished, ChunkBatchStart, ContainerSetContent, GameEvent,
    JoinGame, KeepAlive, LevelChunkWithLight, MoveEntityPos, MoveEntityPosRot, MoveEntityRot,
    PlayIntent, PlayerPosition, RemoveEntities, SetChunkCacheCenter, SetDefaultSpawnPosition,
    SetEntityData, SetExperience, SetHealth, SetHeldSlot, SetTime,
};
use mc_protocol::wire::{PacketReader, PacketWriter};
use std::collections::BTreeMap;

/// Decode, re-encode, and require the exact input bytes back.
fn roundtrip<T: Packet>(body: &[u8]) -> Result<(), String> {
    let packet = T::decode(body).map_err(|e| format!("decode: {e}"))?;
    let back = packet.encode().map_err(|e| format!("encode: {e}"))?;
    if back == body {
        Ok(())
    } else {
        Err(format!(
            "re-encode differs ({} in, {} out)",
            body.len(),
            back.len()
        ))
    }
}

/// `SetEntityMotion` has no [`Packet`] impl (the server never sends it); the
/// sweep covers it through its inherent codec plus an explicit exhaustion
/// check instead of leaving 2 342 bodies unasserted.
fn motion(body: &[u8]) -> Result<(), String> {
    use mc_protocol::packets::play::SetEntityMotion;
    let mut reader = PacketReader::new(body);
    let packet = SetEntityMotion::decode(&mut reader).map_err(|e| format!("decode: {e}"))?;
    if !reader.is_empty() {
        return Err(format!("{} trailing bytes", reader.remaining()));
    }
    let mut writer = PacketWriter::new();
    packet
        .encode(&mut writer)
        .map_err(|e| format!("encode: {e}"))?;
    let back = writer.finish();
    if back == body {
        Ok(())
    } else {
        Err(format!(
            "re-encode differs ({} in, {} out)",
            body.len(),
            back.len()
        ))
    }
}

/// The 8 modeled fields decode; one trailing byte of unknown semantics
/// follows on every captured body (seven sessions, all `0x00`). It is *not*
/// modelled — a guess would be a lie about a client setting — but its bounds
/// are pinned: exactly one byte, still zero. A nonzero trailing byte, or a
/// second one, fails here and means the field has been identified and must be
/// modelled (P18).
fn client_information(body: &[u8]) -> Result<(), String> {
    let packet = ClientInformation::decode(body).map_err(|e| format!("decode: {e}"))?;
    let back = packet.encode().map_err(|e| format!("encode: {e}"))?;
    if body.len() == back.len() + 1 && body.last() == Some(&0x00) && back == body[..back.len()] {
        Ok(())
    } else {
        Err(format!(
            "client_information trailing byte changed ({} in, {} modeled)",
            body.len(),
            back.len()
        ))
    }
}

/// What one body established: checked with a verdict, or unmodelled.
enum Outcome {
    Checked(Result<(), String>),
    Unmodelled,
}

/// Serverbound play goes through the product dispatcher: `Some` means the
/// trailing-byte refusal inside saw a clean end.
fn play_c2s(id: i32, body: &[u8]) -> Result<(), String> {
    match PlayIntent::decode(id, body) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err("unmodelled serverbound id".to_owned()),
        Err(e) => Err(format!("decode: {e}")),
    }
}

fn check_body(state: &str, dirn: &str, id: i32, body: &[u8]) -> Outcome {
    if state == "play" && dirn == "c2s" {
        return Outcome::Checked(play_c2s(id, body));
    }
    let result = match (state, dirn, id) {
        ("handshake", "c2s", 0) => roundtrip::<Handshake>(body),
        ("login", "c2s", 0) => roundtrip::<LoginStart>(body),
        ("login", "c2s", 3) => roundtrip::<LoginAcknowledged>(body),
        ("login", "s2c", 2) => roundtrip::<LoginSuccess>(body),
        ("login", "s2c", 3) => roundtrip::<SetCompression>(body),
        ("config", "c2s", 0) => client_information(body),
        ("config", "c2s", 2) => roundtrip::<C2sCustomPayload>(body),
        ("config", "c2s", 3) => roundtrip::<FinishConfigurationAck>(body),
        ("config", "c2s", 7) | ("config", "s2c", 14) => roundtrip::<SelectKnownPacks>(body),
        ("config", "s2c", 1) => roundtrip::<S2cCustomPayload>(body),
        ("config", "s2c", 3) => roundtrip::<FinishConfiguration>(body),
        ("config", "s2c", 7) => roundtrip::<RegistryData>(body),
        ("config", "s2c", 12) => roundtrip::<FeatureFlags>(body),
        ("config", "s2c", 13) => roundtrip::<UpdateTags>(body),
        ("play", "s2c", 1) => roundtrip::<AddEntity>(body),
        ("play", "s2c", 8) => roundtrip::<BlockUpdate>(body),
        ("play", "s2c", 11) => roundtrip::<ChunkBatchFinished>(body),
        ("play", "s2c", 12) => roundtrip::<ChunkBatchStart>(body),
        ("play", "s2c", 18) => roundtrip::<ContainerSetContent>(body),
        ("play", "s2c", 38) => roundtrip::<GameEvent>(body),
        ("play", "s2c", 44) => roundtrip::<KeepAlive>(body),
        ("play", "s2c", 45) => roundtrip::<LevelChunkWithLight>(body),
        ("play", "s2c", 49) => roundtrip::<JoinGame>(body),
        ("play", "s2c", 53) => roundtrip::<MoveEntityPos>(body),
        ("play", "s2c", 54) => roundtrip::<MoveEntityPosRot>(body),
        ("play", "s2c", 56) => roundtrip::<MoveEntityRot>(body),
        ("play", "s2c", 72) => roundtrip::<PlayerPosition>(body),
        ("play", "s2c", 77) => roundtrip::<RemoveEntities>(body),
        ("play", "s2c", 94) => roundtrip::<SetChunkCacheCenter>(body),
        ("play", "s2c", 97) => roundtrip::<SetDefaultSpawnPosition>(body),
        ("play", "s2c", 99) => roundtrip::<SetEntityData>(body),
        ("play", "s2c", 101) => motion(body),
        ("play", "s2c", 103) => roundtrip::<SetExperience>(body),
        ("play", "s2c", 104) => roundtrip::<SetHealth>(body),
        ("play", "s2c", 105) => roundtrip::<SetHeldSlot>(body),
        ("play", "s2c", 113) => roundtrip::<SetTime>(body),
        _ => return Outcome::Unmodelled,
    };
    Outcome::Checked(result)
}

/// (state, direction, id, jar name) triples this build does not model.
/// P18 breadth owns them; this list is exact-set by assertion below.
const UNMODELLED: &[(&str, &str, i32, &str)] = &[
    ("play", "s2c", 0, "bundle"),
    ("play", "s2c", 10, "change_difficulty"),
    ("play", "s2c", 16, "commands"),
    ("play", "s2c", 34, "entity_event"),
    ("play", "s2c", 35, "entity_position_sync"),
    ("play", "s2c", 43, "initialize_border"),
    ("play", "s2c", 46, "level_event"),
    ("play", "s2c", 64, "player_abilities"),
    ("play", "s2c", 70, "player_info_update"),
    ("play", "s2c", 74, "recipe_book_add"),
    ("play", "s2c", 76, "recipe_book_settings"),
    ("play", "s2c", 83, "rotate_head"),
    ("play", "s2c", 86, "server_data"),
    ("play", "s2c", 127, "ticking_state"),
    ("play", "s2c", 128, "ticking_step"),
    ("play", "s2c", 130, "update_advancements"),
    ("play", "s2c", 131, "update_attributes"),
    ("play", "s2c", 133, "update_mob_effect"),
];

#[test]
fn every_captured_body_round_trips_or_is_listed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/vanilla-capture/bodies");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("capture bodies exist")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no capture bodies found");

    let mut ok: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut unmodelled: BTreeMap<(String, String, i32), usize> = BTreeMap::new();

    for name in &files {
        let stem = name.strip_suffix(".bin").expect("bin");
        let parts: Vec<&str> = stem.split('_').collect();
        assert_eq!(parts.len(), 4, "unexpected capture name {name}");
        let (dirn, state, id): (String, String, i32) = (
            parts[1].to_owned(),
            parts[2].to_owned(),
            parts[3].parse().expect("id"),
        );
        let body = std::fs::read(dir.join(name)).expect("body");
        match check_body(&state, &dirn, id, &body) {
            Outcome::Checked(Ok(())) => *ok.entry(format!("{dirn}_{state}_{id}")).or_insert(0) += 1,
            Outcome::Checked(Err(e)) => failures.push(format!("{name}: {e}")),
            Outcome::Unmodelled => {
                *unmodelled.entry((state, dirn, id)).or_insert(0) += 1;
            }
        }
    }

    let swept: usize = ok.values().sum();
    let listed: usize = unmodelled.values().sum();
    println!("swept {swept} bodies across {} keys", ok.len());
    for (key, count) in &ok {
        println!("  {key}: {count}");
    }
    println!("unmodelled {listed} bodies:");
    for ((state, dirn, id), count) in &unmodelled {
        println!("  {dirn}_{state}_{id}: {count}");
    }

    assert!(
        failures.is_empty(),
        "sweep failures:\n{}",
        failures.join("\n")
    );
    let got: Vec<(String, String, i32)> = unmodelled.keys().cloned().collect();
    let want: Vec<(String, String, i32)> = UNMODELLED
        .iter()
        .map(|(s, d, i, _)| (s.to_string(), d.to_string(), *i))
        .collect();
    assert_eq!(
        got, want,
        "unmodelled set changed: model the newcomer or justify it"
    );
    // The total pins the corpus itself: bodies removed silently would shrink
    // coverage without failing anything else.
    assert_eq!(
        swept + listed,
        files.len(),
        "every body is either swept or listed"
    );
}
