//! Entity lifecycle, phase scheduling and chunk-durability tests (P05-01..P05-03).
//!
//! These drive a real [`Game`] through the same channel the network layer uses.
//! Four things are being pinned down here:
//!
//! 1. **entity wiring** — a join creates an entity, the entity tracks the player,
//!    a leave reaps it, and a drop becomes an item entity;
//! 2. **entity physics** — an entity with velocity falls and lands on a floor;
//! 3. **determinism** (AGENTS.md section 3.6) — two games with the same seed and
//!    the same ordered intents produce the same `TickReport` sequence;
//! 4. **durability** — a chunk read from storage is not overwritten by the all-air
//!    placeholder that streaming would otherwise create, and a chunk nobody can
//!    see is unloaded rather than kept forever.
//!
//! What this does **not** prove: mob AI, item pickup or the entity packets that
//! would make a non-player entity visible to a client. Those are P05-11..P05-15
//! and are listed as gaps in `docs/phases/PHASE-04-REPORT.md` rather than implied
//! by a passing test here.

// Chunk arithmetic narrows world coordinates to chunk indices; the crate root
// documents the same cast exemption.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use mc_entity::entity::{EntityBody, EntityKind};
use mc_entity::player::Vec3 as EntityVec3;
use mc_entity::stack::ItemStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_persistence::dimension::Dimension;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_registry::Registries;
use mc_server::game::{Game, TickReport};
use mc_server::storage::WorldService;
use mc_simulation::TickPhase;
use mc_test_support::fixtures::TempDir;
use mc_world::chunk::Chunk;
use mc_world::{ChunkPos, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT};
use std::time::Duration;

/// Everything a test needs: a live game and a channel to act as the client.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    out: InboundReceiver,
    /// Moved into the join event; `None` once the player has joined.
    outbound: Option<OutboundSender>,
    _storage: WorldService,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str, view_distance: i32) -> Self {
        Self::with_seed(tag, view_distance, mc_server::game::DEFAULT_RANDOM_SEED)
    }

    fn with_seed(tag: &str, view_distance: i32, seed: i64) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (events, rx) = game_channel(256);
        let game = Game::with_seed(&storage, view_distance, rx, seed).expect("game builds");
        let (outbound, out) = OutboundSender::pair(ConnectionId(1), 8192);
        Self {
            game,
            events,
            id: ConnectionId(1),
            out,
            outbound: Some(outbound),
            _storage: storage,
            _dir: dir,
        }
    }

    /// Solid stone under the spawn so a player has ground to stand on.
    fn build_floor(&mut self) -> (i32, i32, i32) {
        let (sx, sy, sz) = self.game.spawn();
        let stone = self
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        for x in (sx - 8)..=(sx + 12) {
            for z in (sz - 8)..=(sz + 12) {
                self.game
                    .world_mut()
                    .set_block(x, sy - 1, z, stone)
                    .expect("floor");
            }
        }
        (sx, sy, sz)
    }

    /// Join a player and run the tick that applies the join.
    fn join(&mut self, name: &str) {
        let outbound = self.outbound.take().expect("one join per harness");
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
    }

    /// Queue one intent and run the tick that applies it.
    fn intent(&mut self, intent: PlayIntent) -> TickReport {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick")
    }

    /// The player's authoritative position.
    fn position(&self) -> EntityVec3 {
        self.game.player(self.id).expect("player").position
    }
}

/// Number of chunk packets queued for the player this tick.
fn chunk_packets(harness: &mut Harness) -> usize {
    let mut count = 0;
    while let Some(raw) = harness.out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::LEVEL_CHUNK_WITH_LIGHT {
            count += 1;
        }
    }
    count
}

#[test]
fn a_joined_player_has_an_entity_that_tracks_it() {
    let mut harness = Harness::new("p05-entity-join", 4);
    let (sx, sy, sz) = harness.build_floor();
    harness.join("Tracker");

    let entity_id = harness
        .game
        .entity_id_of(harness.id)
        .expect("a joined player controls an entity");
    let entity = harness
        .game
        .entity_store()
        .get(entity_id)
        .expect("the entity is in the store");
    assert_eq!(entity.kind(), EntityKind::Player);
    assert_eq!(entity.body, EntityBody::Player);
    assert!(entity.is_alive());

    let player = harness.position();
    assert!(
        (entity.position.x - player.x).abs() < 1e-9
            && (entity.position.y - player.y).abs() < 1e-9
            && (entity.position.z - player.z).abs() < 1e-9,
        "the projection must start where the player is: {:?} vs {player:?}",
        entity.position
    );

    // Walk one legal step: the authoritative state moves, and the projection
    // follows it on the same tick.
    let report = harness.intent(PlayIntent::MovePlayerPos {
        x: f64::from(sx) + 3.5,
        y: f64::from(sy),
        z: f64::from(sz) + 0.5,
        on_ground: true,
    });
    let moved = harness.position();
    assert!(
        (moved.x - (f64::from(sx) + 3.5)).abs() < 0.2,
        "the step should apply, got {moved:?}"
    );
    let entity = harness
        .game
        .entity_store()
        .get(entity_id)
        .expect("still there");
    assert!(
        (entity.position.x - moved.x).abs() < 1e-9 && (entity.position.z - moved.z).abs() < 1e-9,
        "the projection must follow the player after a move: {:?} vs {moved:?}",
        entity.position
    );
    assert!(entity.on_ground, "the floor is under the player");
    let _ = report;
}

#[test]
fn leaving_marks_the_entity_removed_and_a_tick_reaps_it() {
    let mut harness = Harness::new("p05-entity-leave", 4);
    harness.build_floor();
    harness.join("Leaver");
    let entity_id = harness.game.entity_id_of(harness.id).expect("entity");
    assert!(harness.game.entity_store().get(entity_id).is_some());

    harness
        .events
        .try_send(ClientEvent {
            id: harness.id,
            kind: ClientEventKind::Left,
        })
        .expect("leave queued");
    let report = harness.game.tick().expect("tick");

    assert_eq!(harness.game.player_count(), 0, "the session is gone");
    assert_eq!(
        harness.game.entity_id_of(harness.id),
        None,
        "the connection no longer controls an entity"
    );
    assert!(
        harness.game.entity_store().get(entity_id).is_none(),
        "the sweep must have removed the entity from the store"
    );
    assert_eq!(
        report.removed_entities, 1,
        "the report counts the reaped entity"
    );
    assert_eq!(report.removed_ids, vec![entity_id]);
    // The reap is a *tick* action: the same tick that saw the leave swept it.
    assert_eq!(harness.game.entity_store().len(), 0);
}

#[test]
fn spawn_item_creates_an_item_entity_and_refuses_an_empty_stack() {
    let mut harness = Harness::new("p05-item-spawn", 4);
    let (sx, sy, sz) = harness.build_floor();
    let dirt = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
    let stack = ItemStack::new(dirt, 5).expect("five dirt");

    let id = harness
        .game
        .spawn_item(
            stack,
            mc_world::Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5),
        )
        .expect("an item entity spawns");
    let items = harness.game.entity_store().of_kind(EntityKind::Item);
    assert_eq!(items, vec![id], "the drop is the only item entity");
    let entity = harness.game.entity_store().get(id).expect("item entity");
    match &entity.body {
        EntityBody::Item(item) => {
            assert_eq!(item.item_id(), Some(dirt));
            assert_eq!(item.count(), 5);
            assert_eq!(item.owner, None, "a world drop has no owner");
        }
        other => panic!("expected an item body, got {other:?}"),
    }

    // An empty stack would be an invisible, unkillable entity: refused.
    assert!(
        harness
            .game
            .spawn_item(ItemStack::EMPTY, mc_world::Vec3::new(0.0, 64.0, 0.0))
            .is_err(),
        "an empty stack must not become an entity"
    );
}

#[test]
fn dropping_the_held_item_spawns_an_item_entity() {
    let mut harness = Harness::new("p05-item-drop", 4);
    let (sx, sy, sz) = harness.build_floor();
    harness.join("Dropper");
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.inventory.select(0).expect("hotbar 0");
        player
            .inventory
            .set_slot(0, ItemStack::new(stone, 3).expect("three stone"))
            .expect("slot 0");
    }
    assert!(
        harness
            .game
            .entity_store()
            .of_kind(EntityKind::Item)
            .is_empty()
    );

    // `player_action` status 3 is the drop; the target position only has to be
    // inside reach, exactly as the client's own packet is.
    let report = harness.intent(PlayIntent::PlayerAction {
        status: 3,
        position: block_position(sx, sy - 1, sz),
        facing: 1,
        sequence: 0,
    });

    let items = harness.game.entity_store().of_kind(EntityKind::Item);
    assert_eq!(items.len(), 1, "the drop must become an item entity");
    let entity = harness.game.entity_store().get(items[0]).expect("item");
    match &entity.body {
        EntityBody::Item(item) => {
            assert_eq!(item.count(), 3, "the whole stack is dropped");
            assert_eq!(
                item.owner,
                harness.game.entity_id_of(harness.id),
                "the drop remembers who let it go"
            );
        }
        other => panic!("expected an item body, got {other:?}"),
    }
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .inventory
            .selected_item()
            .count(),
        0,
        "the stack left the inventory"
    );
    // The item is not ticked in the same phase that spawned it: it is created by
    // the Players phase, which the Entities phase has already passed this tick.
    assert_eq!(report.entities_ticked, 0);
    let next = harness.game.tick().expect("tick");
    assert_eq!(next.entities_ticked, 1, "the item is ticked from then on");
}

#[test]
fn an_entity_falls_under_gravity_and_lands_on_a_solid_floor() {
    let mut harness = Harness::new("p05-gravity", 4);
    let (sx, sy, sz) = harness.build_floor();
    let dirt = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
    // Three blocks above the floor top: an item falls exactly as far as the block
    // arithmetic says, so the assertion can name the number.
    let start_y = f64::from(sy) + 3.0;
    let id = harness
        .game
        .spawn_item(
            ItemStack::new(dirt, 1).expect("one dirt"),
            mc_world::Vec3::new(f64::from(sx) + 0.5, start_y, f64::from(sz) + 0.5),
        )
        .expect("spawn");

    // Items fall slowly (0.04 blocks/tick² against 0.98 drag): 60 ticks is far
    // past the ~13 ticks the closed form needs for three blocks.
    for _ in 0..60 {
        harness.game.tick().expect("tick");
    }
    let landed = harness.game.entity_store().get(id).expect("item entity");
    assert!(
        landed.position.y < start_y,
        "gravity must have moved it down: {} from {start_y}",
        landed.position.y
    );
    assert!(
        (landed.position.y - f64::from(sy)).abs() < 0.05,
        "it must rest exactly on the floor top ({sy}), got {}",
        landed.position.y
    );
    assert!(landed.on_ground, "a resting entity reports ground");
    assert!(
        landed.velocity.y.abs() < 0.01,
        "a grounded item loses its vertical velocity, got {}",
        landed.velocity.y
    );
    // It must not have sunk into the floor.
    assert!(
        !harness.game.world().is_solid(
            landed.position.x.floor() as i32,
            landed.position.y.floor() as i32,
            landed.position.z.floor() as i32
        ),
        "the entity must not end up inside the floor block"
    );
}

#[test]
fn the_six_phases_run_and_the_metrics_show_their_cost() {
    let mut harness = Harness::new("p05-phases", 4);
    let (sx, sy, sz) = harness.build_floor();
    harness.join("Phased");
    // A dig gives the Broadcast phase a block change to encode, and the drop gives
    // the Entities phase something to tick.
    let dirt = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
    harness
        .game
        .spawn_item(
            ItemStack::new(dirt, 1).expect("one dirt"),
            mc_world::Vec3::new(
                f64::from(sx) + 2.5,
                f64::from(sy) + 1.0,
                f64::from(sz) + 0.5,
            ),
        )
        .expect("spawn");
    let report = harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(sx, sy - 1, sz),
        facing: 1,
        sequence: 0,
    });
    assert_eq!(report.block_changes, 1, "the break is broadcast once");
    assert!(report.entities_ticked >= 1, "the item was ticked");
    assert!(report.packets > 0, "the tick queued packets");

    let metrics = harness.game.metrics();
    assert!(metrics.started(), "the scheduler recorded ticks");
    assert_eq!(
        metrics.tick_count(),
        harness.game.tick_count(),
        "one sample per tick"
    );
    assert!(
        metrics.percentile(0.95) > Duration::ZERO,
        "a tick that encoded a chunk and a block change cannot cost nothing"
    );
    // Network (drained events) and Broadcast (encoding) do real work every tick
    // this test drives, so both must have a non-zero mean.
    assert!(
        metrics.phase_mean(TickPhase::Network) > Duration::ZERO,
        "the Network phase must be measured"
    );
    assert!(
        metrics.phase_mean(TickPhase::Broadcast) > Duration::ZERO,
        "the Broadcast phase must be measured"
    );
    assert!(
        metrics.busiest_phase().is_some(),
        "some phase must have cost something"
    );
    // The two documented no-op phases (ScheduledTicks, BlockEntities) are still
    // *run and timed* — their means may legitimately round to zero, which is why
    // they are not asserted non-zero here. What is asserted is that every phase's
    // mean is within the whole-tick mean times six plus a tick, i.e. no phase is
    // reporting a fabricated cost.
    let budget = metrics.mean().saturating_mul(6) + Duration::from_millis(1);
    for phase in TickPhase::all() {
        assert!(
            metrics.phase_mean(*phase) <= budget,
            "{phase} mean {:?} exceeds the whole-tick scale {budget:?}",
            metrics.phase_mean(*phase)
        );
    }
}

#[test]
fn the_same_seed_replays_the_same_tick_reports() {
    // AGENTS.md section 3.6: same state + same ordered inputs + same tick count
    // ⇒ same result. The `TickReport` is the observable summary of a tick, and it
    // covers every counter the phases touch.
    const TICKS: usize = 24;
    const SEED: i64 = 424_242;

    let script = |harness: &mut Harness| -> Vec<TickReport> {
        let (sx, sy, sz) = harness.build_floor();
        harness.join("Replayer");
        // Something to drop, so the script exercises the entity path as well as
        // movement and blocks.
        let stone = harness
            .game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        {
            let player = harness.game.player_mut(harness.id).expect("player");
            player.inventory.select(0).expect("hotbar 0");
            // Every hotbar slot, because the script also exercises hotbar
            // switching: a drop from an empty slot would spawn nothing.
            for slot in 0..9 {
                player
                    .inventory
                    .set_slot(slot, ItemStack::new(stone, 64).expect("stack"))
                    .expect("hotbar slot");
            }
        }
        let mut reports = Vec::with_capacity(TICKS);
        for step in 0..TICKS {
            // A small repeating offset (0..3) — exactly representable in f64.
            let offset = f64::from(u8::try_from(step % 4).expect("0..3"));
            let report = match step % 6 {
                0 | 1 => harness.intent(PlayIntent::MovePlayerPos {
                    x: f64::from(sx) + 0.5 + offset,
                    y: f64::from(sy),
                    z: f64::from(sz) + 0.5 + offset,
                    on_ground: true,
                }),
                2 => harness.intent(PlayIntent::SetCarriedItem {
                    slot: (step % 9) as i16,
                }),
                3 => harness.intent(PlayIntent::PlayerAction {
                    status: 0,
                    position: block_position(sx + step as i32 % 3, sy - 1, sz),
                    facing: 1,
                    sequence: 0,
                }),
                4 => harness.intent(PlayIntent::MovePlayerRot {
                    yaw: (offset * 30.0) as f32,
                    pitch: (-offset * 10.0) as f32,
                    on_ground: true,
                }),
                _ => harness.intent(PlayIntent::PlayerAction {
                    status: 3,
                    position: block_position(sx, sy - 1, sz),
                    facing: 1,
                    sequence: 0,
                }),
            };
            reports.push(report);
        }
        reports
    };

    let mut first = Harness::with_seed("p05-determinism-a", 4, SEED);
    let mut second = Harness::with_seed("p05-determinism-b", 4, SEED);
    let first_reports = script(&mut first);
    let second_reports = script(&mut second);

    assert_eq!(
        first_reports.len(),
        TICKS,
        "the script must produce one report per tick"
    );
    assert_eq!(
        first_reports, second_reports,
        "the same seed and the same ordered inputs must replay identically"
    );
    // The replay is not accidentally empty: the game did real work in both runs.
    assert!(
        first_reports.iter().any(|report| report.chunks_sent > 0),
        "the script streamed chunks"
    );
    assert!(
        first_reports
            .iter()
            .any(|report| report.entities_ticked > 0 || report.removed_entities > 0),
        "the script touched entities"
    );
    assert_eq!(
        first.game.metrics().tick_count(),
        second.game.metrics().tick_count()
    );
    assert_eq!(
        first.position(),
        second.position(),
        "the authoritative player state must match too"
    );
    assert_eq!(
        first.game.entity_store().len(),
        second.game.entity_store().len()
    );

    // A different seed must not be silently ignored either: the field is read by
    // `Game::random`, and the two runs agree on it.
    assert_eq!(first.game.random_seed(), SEED);
    assert_eq!(first.game.random(), second.game.random());
}

#[test]
fn chunks_outside_the_view_are_unloaded_and_re_streamed_on_return() {
    let mut harness = Harness::new("p05-unload", 4);
    // Spawn in chunk (0, 0) without a floor: the player does not move on its own,
    // because player physics is driven by client intents.
    harness.game.world_mut().set_spawn(0, 64, 0);
    harness.join("Walker");
    for _ in 0..6 {
        harness.game.tick().expect("tick");
    }
    let home = ChunkPos::new(0, 0);
    assert!(
        harness.game.world().is_loaded(home),
        "the join must stream the chunk the player stands in"
    );
    assert!(
        chunk_packets(&mut harness) > 0,
        "the join streamed chunk packets"
    );

    // Move the authoritative player state 37 chunks away, as a teleport would.
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .position = EntityVec3::new(600.5, 64.0, 0.5);
    for _ in 0..12 {
        harness.game.tick().expect("tick");
    }
    assert!(
        harness.game.world().is_loaded(ChunkPos::new(37, 0)),
        "the new surroundings must stream"
    );
    assert!(
        !harness.game.world().is_loaded(home),
        "a clean chunk no player can see must be unloaded, not kept forever"
    );

    // Coming back must re-stream it: the unload drops the `sent_chunks` entry, so
    // a hole in the client's view cannot survive.
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .position = EntityVec3::new(0.5, 64.0, 0.5);
    for _ in 0..12 {
        harness.game.tick().expect("tick");
    }
    assert!(
        harness.game.world().is_loaded(home),
        "returning must re-stream the chunk (only `send_chunk` loads one)"
    );
    assert!(
        chunk_packets(&mut harness) > 0,
        "the return trip re-sent chunks"
    );
}

#[test]
fn a_stored_chunk_is_loaded_from_disk_and_never_overwritten_by_a_placeholder() {
    // The Phase 04 gap this pins down: streaming used to create an all-air chunk
    // for every position in view and mark it dirty, so the next save wrote that
    // air over real terrain. A player's build did not survive a restart.
    let dir = TempDir::new("p05-chunk-reload");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let registries = Registries::vanilla().expect("registry");
    let stone = registries
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let diamond = registries
        .blocks
        .default_state("minecraft:diamond_block")
        .expect("diamond block");
    let (bx, by, bz) = (3, 64, 5);
    let home = ChunkPos::new(bx >> 4, bz >> 4);

    // ---- run 1: put real terrain in one chunk, through the storage layer ----
    {
        let mut storage = WorldService::open(&config).expect("world opens");
        let mut chunk = Chunk::air(
            home,
            OVERWORLD_MIN_SECTION_Y,
            OVERWORLD_SECTION_COUNT,
            &registries.blocks,
        );
        chunk
            .set_block(bx, by, bz, diamond, &registries.blocks)
            .expect("diamond")
            .expect("the block changed");
        let data = chunk.to_chunk_data(&registries.blocks).expect("encodes");
        storage
            .storage_mut()
            .queue_chunk_save(&Dimension::Overworld, &data)
            .expect("queued");
        storage.storage_mut().flush().expect("flushed");
        storage.close().expect("closes");
    }

    // ---- run 2: the game must read it back, and must not erase it ----
    {
        let storage = WorldService::open(&config).expect("world reopens");
        let (events, rx) = game_channel(64);
        // The production shape: the game owns the world handle so it can read a
        // chunk before creating a placeholder for it.
        let mut game = Game::with_seed_and_storage(storage, 4, rx, 7).expect("game builds");
        game.world_mut().set_spawn(bx, by, bz);
        let (outbound, _out) = OutboundSender::pair(ConnectionId(1), 8192);
        events
            .try_send(ClientEvent {
                id: ConnectionId(1),
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile("Reloader"),
                    outbound,
                },
            })
            .expect("join queued");
        for _ in 0..4 {
            game.tick().expect("tick");
        }

        assert!(
            game.world().is_loaded(home),
            "the view must have reached the stored chunk"
        );
        assert_eq!(
            game.world().get_block(bx, by, bz),
            diamond,
            "the stored block must be visible in the running world"
        );

        // An edit to that chunk is still saved, so the durability fix did not turn
        // the save path off: only *unmodified* chunks are protected.
        game.world_mut()
            .set_block(bx, by - 1, bz, stone)
            .expect("edit");
        game.save_all_owned().expect("saves");

        // The chunks that were only ever placeholders are clean, so nothing was
        // queued for them; a save that wrote them would erase terrain on the next
        // start, which run 3 checks from disk.
        let storage = game.into_storage().expect("the game owns the world");
        storage.close().expect("closes");
    }

    // ---- run 3: the terrain is still there, and the placeholders were not ----
    let mut storage = WorldService::open(&config).expect("world reopens");
    let data = storage
        .storage_mut()
        .read_chunk(&Dimension::Overworld, home)
        .expect("reads")
        .expect("the chunk is still stored");
    let runtime = Chunk::from_chunk_data(&data, &registries.blocks).expect("converts");
    assert_eq!(
        runtime.get_block(bx, by, bz),
        diamond,
        "the stored block survived a streaming run and a save"
    );
    assert_eq!(
        runtime.get_block(bx, by - 1, bz),
        stone,
        "a real edit is still persisted"
    );
    // A neighbour that only ever existed as a placeholder must not have been
    // created on disk.
    let neighbour = ChunkPos::new(home.x + 1, home.z);
    assert!(
        storage
            .storage_mut()
            .read_chunk(&Dimension::Overworld, neighbour)
            .expect("reads")
            .is_none(),
        "a chunk that was only streamed as air must not be written back"
    );
    storage.close().expect("closes");
}
