//! P11-08: a chunk's entities ride the save and come back on the next boot.
//!
//! The test builds a **second `Game` on the same `world_dir`**, which is the only
//! way to prove persistence: asserting on the `ChunkData` the first game wrote
//! would prove the encoder agrees with the decoder it was written beside, not
//! that a restart finds anything. The second game is built by the same
//! constructor the lifecycle uses, so the load path is the production one.
//!
//! ## The condition this test has to state out loud
//!
//! This build saves **dirty** chunks, and a generated chunk is deliberately left
//! clean (see `load_or_create_chunk`: the generator is deterministic, so
//! persisting generated terrain buys nothing). Spawning an entity does not by
//! itself mark a chunk dirty, so an entity in a chunk nobody has touched is
//! **not** saved. Every test below modifies a block in the chunk first, and the
//! first one asserts the chunk really is dirty before saving.
//!
//! That is a real divergence from vanilla and it is recorded rather than hidden:
//! see the "entity persistence" row in `docs/vanilla-parity/PARITY-MATRIX.md` and
//! `docs/audits/AUDIT-09-REMEDIATION.md`. What these tests pin is the part that
//! *is* implemented — a saved chunk's entities survive — and they fail loudly if
//! the dirty-chunk rule is ever tightened or loosened.
//!
//! ## What these do not prove
//!
//! That a real client sees the restored mobs (P11-10), and that a *vanilla*
//! server could read our file: the NBT is a modelled subset, stated on
//! `serialize_chunk_entities`.

use mc_entity::stack::ItemStack;
use mc_entity::{EntityBody, MobKind};
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packet::RawPacket;
use mc_protocol::packets::play::AddEntity;
use mc_server::config::StorageConfig;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// A world directory that outlives every game opened on it.
struct World {
    _dir: TempDir,
    config: StorageConfig,
}

impl World {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        Self { _dir: dir, config }
    }

    /// A game that owns its storage, so it can generate and read terrain.
    fn game(&self) -> (Game, tokio::sync::mpsc::Sender<ClientEvent>) {
        let storage = WorldService::open(&self.config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        (game, tx)
    }
}

/// The chunk position holding a block coordinate.
fn chunk_of(x: i32, z: i32) -> ChunkPos {
    ChunkPos::new(x >> 4, z >> 4)
}

fn join(game: &mut Game, events: &tokio::sync::mpsc::Sender<ClientEvent>) -> InboundReceiver {
    let id = ConnectionId(1);
    let (outbound, out) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Returner"),
                outbound,
            },
        })
        .expect("join event queued");
    game.tick().expect("tick");
    out
}

/// Every packet queued so far, in order.
fn drain(out: &mut InboundReceiver) -> Vec<RawPacket> {
    let mut packets = Vec::new();
    while let Some(raw) = out.try_recv() {
        packets.push(raw);
    }
    packets
}

/// The entity ids named by the `add_entity` packets in `packets`.
///
/// Decoded with the real `AddEntity::decode`, so this reads the field the client
/// reads rather than trusting a byte offset the test guessed.
fn announced_entities(packets: &[RawPacket]) -> Vec<i32> {
    packets
        .iter()
        .filter(|packet| packet.id == clientbound::play::ADD_ENTITY)
        .filter_map(|packet| AddEntity::decode(&mut packet.reader()).ok())
        .map(|add| add.entity_id)
        .collect()
}

/// The `(id, kind)` of every mob in the store.
fn stored_mobs(game: &Game) -> Vec<(i32, MobKind)> {
    game.entity_store()
        .iter()
        .filter_map(|entity| {
            let EntityBody::Mob(mob) = &entity.body else {
                return None;
            };
            Some((entity.id.get(), mob.kind))
        })
        .collect()
}

#[test]
fn a_saved_chunk_returns_its_mobs_and_its_drops_to_the_next_game() {
    let world = World::new("p11-persist");
    let (mut first, _events) = world.game();

    // No player joins this game at all, deliberately: with no players the spawn
    // cycle returns immediately, so nothing below can be confused with a
    // naturally spawned entity.
    let (bx, by, bz) = first.spawn();
    let chunk = chunk_of(bx, bz);
    assert!(first.load_chunk(chunk), "the spawn chunk loads");

    // Dirty the chunk. Without a modification there is nothing to save, and this
    // test would be asserting on the generator instead of on persistence.
    let stone = first
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    first
        .world_mut()
        .set_block(bx, by + 4, bz, stone)
        .expect("the marker block is set");

    let zombie_at = mc_world::Vec3::new(
        f64::from(bx) + 3.5,
        f64::from(by) + 4.0,
        f64::from(bz) + 3.5,
    );
    first
        .spawn_mob(MobKind::Zombie, zombie_at)
        .expect("the zombie spawns");
    let diamond = first
        .registries()
        .items
        .id("minecraft:diamond")
        .expect("diamond");
    let item_at = mc_world::Vec3::new(
        f64::from(bx) + 5.5,
        f64::from(by) + 4.0,
        f64::from(bz) + 0.5,
    );
    first
        .spawn_item(ItemStack::new(diamond, 7).expect("seven diamonds"), item_at)
        .expect("the drop spawns");

    // Preconditions, so a failure below is about the round trip and not about a
    // spawn that never happened.
    assert_eq!(
        stored_mobs(&first),
        vec![(1, MobKind::Zombie)],
        "exactly the zombie is in the store before the save"
    );
    assert_eq!(first.dropped_items().len(), 1, "one drop before the save");
    assert!(
        first.world().dirty_chunks().contains(&chunk),
        "the marker block made the chunk dirty, or nothing below is saved"
    );

    first.save_all_owned().expect("the world saves");
    // The region files have to be released before a second service can open the
    // same directory on Windows, so the handle is closed explicitly rather than
    // left to `Drop`.
    first.close_storage().expect("the storage handle closes");
    drop(first);

    // ---- the restart ------------------------------------------------------
    let (mut second, _events2) = world.game();
    assert!(second.load_chunk(chunk), "the saved chunk loads");
    assert!(
        second.world().is_loaded(chunk),
        "the second game has the chunk in its world"
    );

    let mobs = stored_mobs(&second);
    assert_eq!(
        mobs.len(),
        1,
        "the saved mob came back exactly once; saw {mobs:?}"
    );
    assert_eq!(
        mobs[0].1,
        MobKind::Zombie,
        "and as the kind it was saved as"
    );
    let saved = second
        .entity_store()
        .get(mc_entity::EntityId::new(mobs[0].0).expect("a positive id"))
        .expect("the entity is in the store")
        .position;
    assert!(
        (saved.x - zombie_at.x).abs() < 1.0
            && (saved.y - zombie_at.y).abs() < 2.0
            && (saved.z - zombie_at.z).abs() < 1.0,
        "at the position it was saved at (wanted {zombie_at:?}, got {saved:?})"
    );

    let drops = second.dropped_items();
    assert_eq!(drops.len(), 1, "the saved drop came back; saw {drops:?}");
    assert_eq!(
        drops[0].0.item_id(),
        Some(diamond),
        "as the item it was saved as, not as whatever item had the count as its registry id \
         (the reversed-argument defect this assertion exists for)"
    );
    assert_eq!(drops[0].0.count(), 7, "with its count intact");
}

#[test]
fn a_joining_player_is_told_about_a_resident_entity_on_the_join_tick() {
    // The join-visibility half of P11-08, and the reason it is asserted on the
    // **join tick itself**: `entity_announce_packets` in `send_chunk` is what
    // tells a player about entities in a chunk they have just received. The
    // pending-spawn path would also announce the zombie, but only on the tick
    // *after* the chunk is streamed, so a single tick isolates the two. Removing
    // the announcement from `send_chunk` makes this test fail and leaves the
    // pending path green — which is exactly the regression it guards.
    let world = World::new("p11-persist-join");
    let (mut first, _events) = world.game();

    let (bx, by, bz) = first.spawn();
    let chunk = chunk_of(bx, bz);
    assert!(first.load_chunk(chunk));
    let stone = first
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    first
        .world_mut()
        .set_block(bx, by + 3, bz, stone)
        .expect("marker");
    // At the chunk's own centre, so it is in the chunk the joining player's
    // spawn point is in however the seed happens to place that point.
    let cow_at = mc_world::Vec3::new(
        f64::from(chunk.x * 16 + 8),
        f64::from(by) + 3.0,
        f64::from(chunk.z * 16 + 8),
    );
    first
        .spawn_mob(MobKind::Cow, cow_at)
        .expect("the cow spawns");
    first.save_all_owned().expect("the world saves");
    first.close_storage().expect("the storage handle closes");
    drop(first);

    let (mut second, events2) = world.game();
    let mut out = join(&mut second, &events2);

    let mobs = stored_mobs(&second);
    assert_eq!(
        mobs.len(),
        1,
        "the reloaded cow is the only mob, so the id asserted below is unambiguous; saw {mobs:?}"
    );
    assert_eq!(mobs[0].1, MobKind::Cow);
    let cow_id = mobs[0].0;
    let announced = announced_entities(&drain(&mut out));
    assert!(
        announced.contains(&cow_id),
        "the joining player was told about the resident cow (id {cow_id}) in the same tick its \
         chunk was streamed; add_entity named {announced:?}"
    );
}
