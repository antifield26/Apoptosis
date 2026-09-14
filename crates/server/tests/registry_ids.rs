//! The numbers this server sends a client are claims about the client's registries.
//!
//! ## Why
//!
//! KD-65: `PLAINS_BIOME_ID = 0` while id 0 in the registry the client is sent is `minecraft:badlands`. Every
//! chunk of every world was painted red sand and orange terracotta, wherever the player stood, with the terrain,
//! the blocks and the light entirely correct. Nothing errored, and nothing in the repository could contradict it.
//!
//! KD-56 is the same shape one registry over: a block's default state assumed to be its lowest id, wrong for 642
//! of 1168 blocks, so every log lay on its side and every leaf held water.
//!
//! **The rule from the sweep:** of the five registry ids this server puts on the wire, the two with nothing
//! checking them were both wrong and the three with something checking them are all right. This is the check for
//! the biome id, and it reads the exact bytes the client is given rather than a copy of them.

use mc_server::game::{CHAT_TYPE_CHAT, PLAINS_BIOME_ID};

/// The registry blob the config phase sends, uncompressed.
///
/// **Read from the fixture rather than through `captured_payload()`.** That accessor parses the same blob into
/// packets whose payloads, concatenated, come to 41 097 bytes containing `minecraft:` and **not** the biome
/// registry's key — so it is not the uncompressed registry bytes, whatever the reason. Rather than detour
/// through that, this says plainly where its bytes come from: the same committed blob the packet builder embeds,
/// which makes it the source of the copy rather than a copy of a copy.
///
/// A test that asks the server what it sends would be stronger. This one knows that it does not, and says so.
fn payload() -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read(dir.join("../network/src/registry_data/config-payload.bin"))
        .expect("the committed config payload is readable")
}

/// The identifiers in the payload after one registry's key, in the order they appear.
///
/// Stops at the first repeat, which is where a registry's own list ends and the next one's names begin.
fn identifiers_after(payload: &[u8], key: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(payload);
    let Some(at) = text.find(key) else {
        panic!("the payload does not carry {key:?} at all");
    };
    let mut names: Vec<String> = Vec::new();
    let mut search = &text[at + key.len()..];
    while let Some(offset) = search.find("minecraft:") {
        let tail = &search[offset..];
        let end = tail
            .find(|c: char| {
                !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/' || c == ':')
            })
            .unwrap_or(tail.len());
        let name = &tail[..end];
        if names.iter().any(|seen| seen == name) {
            break;
        }
        names.push(name.to_owned());
        search = &tail[end..];
    }
    names
}

#[test]
fn the_biome_id_this_server_sends_names_the_biome_it_claims() {
    let payload = payload();

    // The biome registry's names come out alphabetical and stop where the next registry breaks the order, so the
    // longest strictly-increasing prefix is the registry itself. If that ever stops being true the prefix
    // collapses, and the assertion below fails with a message that says which of the two it was.
    let names = identifiers_after(&payload, "minecraft:worldgen/biome");
    assert!(
        names.len() > 8,
        "only {} identifiers followed the biome key, so this is not reading the registry: {names:?}",
        names.len()
    );
    let mut prefix = vec![names[0].clone()];
    for name in &names[1..] {
        if name <= prefix.last().expect("non-empty") {
            break;
        }
        prefix.push(name.clone());
    }
    assert!(
        prefix.len() > 8,
        "the biome registry is no longer alphabetical in the payload ({} of {} names sorted), so the id cannot \
         be read as an index: {names:?}",
        prefix.len(),
        names.len()
    );

    let index = usize::try_from(PLAINS_BIOME_ID).expect("a biome id fits in usize");
    let at_id = prefix.get(index).unwrap_or_else(|| {
        panic!(
            "PLAINS_BIOME_ID is {index}, beyond the {} biomes the registry carries",
            prefix.len()
        )
    });
    assert_eq!(
        at_id, "minecraft:plains",
        "PLAINS_BIOME_ID is {index}, and the registry the client is sent gives that id to {at_id:?}. \
         Every chunk would be painted as {at_id:?}: see KD-65."
    );
}

#[test]
fn the_dimension_type_id_this_server_sends_is_the_overworld() {
    let payload = payload();
    let names = identifiers_after(&payload, "minecraft:dimension_type");
    assert_eq!(
        names.first().map(String::as_str),
        Some("minecraft:overworld"),
        "join_game sends dimension_type_id 0, so entry 0 of the registry must be the overworld: {names:?}"
    );
}

/// The chat type the six message sites send is the one the payload names `minecraft:chat`.
///
/// **The same shape as the biome-id check above**, and for the same reason: `chat_type` decides how a client
/// decorates a line, the server sent a literal `0` for it, and nothing anywhere could contradict that. It is a
/// **datapack registry** — one this server sends — so the instrument is the payload rather than the jar, and a
/// capture of a real server sending `5` says only that the number is not free.
#[test]
fn the_chat_type_this_server_sends_is_the_one_named_chat() {
    let names = identifiers_after(&payload(), "minecraft:chat_type");
    assert!(
        !names.is_empty(),
        "the payload carries no chat_type identifiers, so this test is checking nothing"
    );
    let at = names
        .iter()
        .position(|name| name == "minecraft:chat")
        .unwrap_or_else(|| panic!("no `minecraft:chat` among the chat types: {names:?}"));
    // A checked conversion rather than s: clippy is right that the cast can truncate, and refusing beats
    // wrapping silently. It cannot fail for a registry this size, which is the point of saying so here.
    let at = i32::try_from(at).expect("a registry index fits in an i32");
    assert_eq!(
        at, CHAT_TYPE_CHAT,
        "the payload gives `minecraft:chat` the id {at}, and the server sends {CHAT_TYPE_CHAT}. Registered: {names:?}"
    );
}
