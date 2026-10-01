//! AUDIT-19 B-M5 and D-19-L3: two decisions that were live in the code and
//! pinned nowhere.
//!
//! # B-M5 — the item-stack metadata strip
//!
//! An item entity's contents are announced to clients through
//! `MetadataValue::ItemStack { count, item_id }`, which has no field for the
//! data-component patch: a damaged, renamed or enchanted drop is drawn as a
//! plain item until it is picked up and re-synced through the inventory path.
//! That is deliberate (the build does not model the patch on the wire), and the
//! audit's finding is that nothing pinned it.
//!
//! The **real sites** are the three points where a live `ItemStack` becomes
//! that value, all in `crates/server/src/game/tick.rs`:
//! `broadcast_entity_spawns` (`:3751`), `entity_announce_packets` (`:4502`,
//! reached from `send_chunk` for a joining player) and the merge announcement
//! (`:1514`). AUDIT-19 §5 pointed at `tick.rs:4345-4359`, which is chunk
//! streaming and holds no metadata at all; AUDIT-18 pointed at
//! `crates/entity/src/inventory.rs:139`, a `selected_hotbar` field declaration
//! in both of the revisions it cites. The protocol encoder itself is pinned by
//! `crates/protocol/tests/item_stack_metadata.rs`, but that test cannot see
//! whether the server still announces through it.
//!
//! What this file pins: every `set_entity_data` an item entity gets carries the
//! documented shape — count and item id of the entity's own stack, and an
//! **empty** patch — while the store keeps the patch. Extending
//! `MetadataValue::ItemStack` with a patch, or dropping the count, turns it red.
//!
//! # D-19-L3 — the read-first guard on the non-command writers
//!
//! Every writer that is not a command (`start_dig`, placement, the lever, the
//! redstone propagation) reads its target with `get_block_loaded` and returns
//! when the chunk is not loaded. Nothing pinned that: swapping the call for
//! `get_block` (air instead of `None`) left the suite green, because the one
//! writer a test can drive cheaply — digging — reads air and then does nothing.
//! Placement is where the difference becomes a write: with `get_block` the
//! target reads as empty, the block lands, and `World::ensure_chunk` creates a
//! chunk that was not there. If a chunk *was* stored there, the next save
//! writes the placeholder over it, which is the data-loss class P19-08 fixed
//! for the command path.
//!
//! The test below forces the precondition the guard exists for (a chunk inside
//! reach that is not loaded), sends a real placement intent through
//! `Game::tick`, and asserts both halves: no chunk is created in memory, and
//! the stored file is still there after a save.

// The block coordinates below are an `f64` world position floored to a block
// index (the player's feet), the same cast `dig_progress.rs` allows.
#![allow(clippy::cast_possible_truncation)]

use mc_entity::components::{DataComponent, ItemComponents};
use mc_entity::stack::ItemStack;
use mc_nbt::NbtTag;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_persistence::chunk::{BlockState, ChunkData, ChunkPos, SectionData};
use mc_persistence::dimension::Dimension;
use mc_protocol::ids::clientbound;
use mc_protocol::packet::RawPacket;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{MetadataValue, PlayIntent, SetEntityData, block_position};
use mc_server::config::StorageConfig;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// The metadata slot an item entity's stack rides (`tick.rs` writes 8; the
/// capture in `item_stack_metadata.rs` reads it back from a real server).
const ITEM_STACK_SLOT: u8 = 8;

fn config_in(dir: &TempDir) -> StorageConfig {
    StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    }
}

fn join(game: &mut Game, events: &tokio::sync::mpsc::Sender<ClientEvent>) -> InboundReceiver {
    let id = ConnectionId(1);
    let (outbound, out) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Pinner"),
                outbound,
            },
        })
        .expect("join event queued");
    game.tick().expect("the join tick runs");
    out
}

fn drain(out: &mut InboundReceiver) -> Vec<RawPacket> {
    let mut packets = Vec::new();
    while let Some(raw) = out.try_recv() {
        packets.push(raw);
    }
    packets
}

/// The `set_entity_data` body an entity was announced with, decoded by the real
/// decoder — which refuses a non-empty component patch, so a patch that started
/// riding the wire would make this panic rather than pass.
fn announced_stack(packets: &[RawPacket], entity_id: i32) -> Option<(u8, MetadataValue)> {
    for raw in packets
        .iter()
        .filter(|raw| raw.id == clientbound::play::SET_ENTITY_DATA)
    {
        let packet = SetEntityData::decode(&raw.payload).expect("our own body decodes");
        if packet.entity_id != entity_id {
            continue;
        }
        let mut entries = packet.entries.into_iter();
        let first = entries.next();
        assert!(
            entries.next().is_none(),
            "a dropped stack is announced with exactly one metadata entry"
        );
        return first;
    }
    None
}

#[test]
fn a_drop_with_components_is_announced_without_them_and_keeps_them() {
    // B-M5. Both announcement paths a single player can produce are covered by
    // one scenario: an item already in the world when the player joins (the
    // stream/announce path, `entity_announce_packets`) and one spawned after
    // (the pending-spawn path, `broadcast_entity_spawns`). The two produce the
    // same bytes, so the assertion is on the wire shape rather than on which
    // site emitted it — the point is that *no* item announcement carries a
    // patch and that none of them loses the count.
    let dir = TempDir::new("p19-bm5");
    let config = config_in(&dir);
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    let mut game =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    let (bx, by, bz) = game.spawn();
    let diamond = game
        .registries()
        .items
        .id("minecraft:diamond")
        .expect("diamond");
    let mut components = ItemComponents::new();
    components.set(DataComponent::Damage(9));
    components.set(DataComponent::CustomName("Anniversary".to_owned()));
    let stack = ItemStack::with_components(diamond, 7, components).expect("a damaged, named stack");

    let at = |dx: f64, dz: f64| {
        mc_world::Vec3::new(f64::from(bx) + dx, f64::from(by) + 1.0, f64::from(bz) + dz)
    };
    let resident = game
        .spawn_item(stack.clone(), at(0.5, 0.5))
        .expect("the resident drop spawns");
    let mut out = join(&mut game, &events);
    let fresh = game
        .spawn_item(stack.clone(), at(0.5, 2.5))
        .expect("the fresh drop spawns");
    game.tick().expect("the spawn broadcast runs");
    let packets = drain(&mut out);

    for (label, id) in [("resident", resident), ("fresh", fresh)] {
        let announced = announced_stack(&packets, id.get());
        let (slot, value) = announced.unwrap_or_else(|| {
            panic!(
                "the {label} drop (entity {}) was never announced, so the strip is not what \
                 this test measured",
                id.get()
            )
        });
        assert_eq!(slot, ITEM_STACK_SLOT, "the stack rides slot 8");
        match value {
            MetadataValue::ItemStack { count, item_id } => {
                assert_eq!(count, 7, "the {label} drop's count is not stripped");
                assert_eq!(
                    item_id, diamond,
                    "the {label} drop's item id is not stripped"
                );
            }
            other => panic!("expected a patchless item stack, got {other:?}"),
        }
    }

    // The strip is on the wire only: the entity still holds the patch, which is
    // what makes it a documented downgrade rather than data loss.
    let stored = game
        .dropped_items()
        .into_iter()
        .map(|(stack, _)| stack)
        .find(|stack| stack.count() == 7)
        .expect("a drop is still in the world");
    assert_eq!(
        stored.damage(),
        Some(9),
        "the server-side stack keeps its components"
    );
    assert_eq!(stored.custom_name(), Some("Anniversary"));
}

/// A chunk with one section and a marker no placeholder can produce.
fn planted_chunk(pos: ChunkPos) -> ChunkData {
    ChunkData {
        pos,
        data_version: mc_persistence::level::DATA_VERSION_26_1_2,
        status: "minecraft:full".to_owned(),
        min_section_y: -4,
        last_update: 0,
        inhabited_time: 0,
        light_correct: false,
        sections: vec![SectionData::filled(
            4,
            BlockState::new("minecraft:stone"),
            "minecraft:plains",
        )],
        heightmaps: Vec::new(),
        block_entities: Vec::new(),
        entities: Vec::new(),
        block_ticks: Vec::new(),
        fluid_ticks: Vec::new(),
        post_processing: Vec::new(),
        structures: None,
        extra: vec![("_audit19_d19_l3".to_owned(), NbtTag::Byte(1))],
    }
}

/// Whether the stored chunk at `pos` is still the planted one (a placeholder
/// written over it would carry no marker).
fn planted_chunk_survives(config: &StorageConfig, pos: ChunkPos) -> bool {
    let mut storage = WorldService::open(config).expect("world reopens");
    let read = storage
        .storage_mut()
        .read_chunk(&Dimension::Overworld, pos)
        .expect("the chunk reads")
        .is_some_and(|data| data.extra.iter().any(|(key, _)| key == "_audit19_d19_l3"));
    storage.close().expect("the handle closes");
    read
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one scenario from world setup to the on-disk assertion; splitting it would need shared mutable state between the halves"
)]
fn a_placement_into_an_unloaded_chunk_is_refused_before_it_creates_one() {
    // D-19-L3. The guard at `session.rs`'s `place_held_block` reads the target
    // with `get_block_loaded`. Neutralising it (`get_block`, i.e. air) lets the
    // block land in a chunk nobody loaded: the chunk is created, and if a chunk
    // was stored there the next save replaces it with the placeholder.
    let dir = TempDir::new("p19-d19-l3");
    let config = config_in(&dir);
    let (bx, _by, bz) = {
        let storage = WorldService::open(&config).expect("world opens");
        let probe = Game::new(&storage, 2, game_channel(4).1).expect("probe builds");
        probe.spawn()
    };
    let centre = ChunkPos::new(bx >> 4, bz >> 4);
    let neighbours = [
        ChunkPos::new(centre.x + 1, centre.z),
        ChunkPos::new(centre.x - 1, centre.z),
        ChunkPos::new(centre.x, centre.z + 1),
        ChunkPos::new(centre.x, centre.z - 1),
    ];
    // A stored chunk on every side, so whichever neighbour the test picks has
    // real terrain to lose.
    {
        let mut storage = WorldService::open(&config).expect("world opens");
        for pos in neighbours {
            storage
                .storage_mut()
                .queue_chunk_save(&Dimension::Overworld, &planted_chunk(pos))
                .expect("the planted chunk queues");
        }
        storage.storage_mut().flush().expect("it is on disk");
        storage.close().expect("the handle closes");
    }

    let storage = WorldService::open(&config).expect("world reopens");
    let (events, rx) = game_channel(256);
    let mut game =
        Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    let _out = join(&mut game, &events);
    let y = {
        let position = game.player(ConnectionId(1)).expect("the player").position;
        position.y.floor() as i32
    };
    assert!(
        game.world().is_loaded(centre),
        "the joining player's own chunk must be loaded, or the target is not a neighbour"
    );

    // The `+x` neighbour, emptied the way an unload leaves it, inside the
    // player's reach and clickable from the edge of their own chunk.
    // `unload_chunk` is the honest way to reach this state: an unloaded chunk is
    // exactly what the guard's condition names, and it stays unloaded because
    // streaming only sends chunks the session has not been sent (`sent_chunks`)
    // — which is also the production shape of "a chunk inside reach that is not
    // loaded".
    let target_chunk = ChunkPos::new(centre.x + 1, centre.z);
    let clicked = (centre.x * 16 + 15, y, centre.z * 16 + 8);
    let target = (clicked.0 + 1, y, clicked.2);
    let player_at = (
        f64::from(centre.x * 16) + 14.5,
        f64::from(centre.z * 16) + 8.5,
    );
    game.world_mut()
        .unload_chunk(target_chunk)
        .expect("the neighbour was streamed, so it can be unloaded");
    let stone_block = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    game.world_mut()
        .set_block(clicked.0, clicked.1, clicked.2, stone_block)
        .expect("the clicked block exists in the player's own chunk");
    {
        let player = game.player_mut(ConnectionId(1)).expect("the player");
        player.position.x = player_at.0;
        player.position.z = player_at.1;
    }
    let stone_item = game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone item");
    game.player_mut(ConnectionId(1))
        .expect("the player")
        .inventory
        .set_slot(0, ItemStack::new(stone_item, 1).expect("a stack"))
        .expect("slot 0 takes the block");
    assert!(
        !game.world().is_loaded(target_chunk),
        "precondition: the target chunk is not loaded"
    );
    assert_eq!(
        game.world().get_block_loaded(target.0, target.1, target.2),
        None,
        "precondition: the placement target is in the unloaded chunk"
    );

    events
        .try_send(ClientEvent {
            id: ConnectionId(1),
            kind: ClientEventKind::Intent(PlayIntent::UseItemOn {
                hand: 0,
                position: block_position(clicked.0, clicked.1, clicked.2),
                face: 5,
                cursor_x: 0.5,
                cursor_y: 0.5,
                cursor_z: 0.5,
                inside_block: false,
                world_border_hit: false,
                sequence: 0,
            }),
        })
        .expect("the placement intent queues");
    game.tick().expect("the intent tick runs");

    assert!(
        !game.world().is_loaded(target_chunk),
        "a placement into an unloaded chunk must not create the chunk — with `get_block` the \
         target reads as air and the write creates it"
    );
    assert_eq!(
        game.world().get_block_loaded(target.0, target.1, target.2),
        None,
        "and nothing was written into it"
    );
    game.save_all_owned().expect("the world saves");
    game.close_storage().expect("the storage handle closes");
    drop(game);
    assert!(
        planted_chunk_survives(&config, target_chunk),
        "the stored chunk on the other side is still the stored chunk, not a placeholder \
         written over it"
    );
}
