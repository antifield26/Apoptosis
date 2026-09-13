//! Phase 04 survival vertical-slice end-to-end tests (P04-16, P04-17).
//!
//! These drive a real [`Game`] through the same channel the network layer uses: a
//! `ClientEventKind::Joined` event followed by decoded [`PlayIntent`]s, exactly as
//! `mc-network` produces them. Nothing is stubbed —the packets are encoded by the
//! real codecs, the world is the real `mc_world::World`, and block ids come from
//! the jar-extracted registry.
//!
//! What this proves: a player can join, receive terrain, walk on it, break and
//! place blocks within reach, die, respawn, and have the world survive a restart.
//! What it does **not** prove is real-client acceptance —no 26.1.2 client exists
//! in this environment. That gap is stated in the parity matrix (originally the Phase 04 report)
//! rather than papered over with a self-consistent codec test.

// Each scenario is a single linear narrative (join → act → assert) on purpose;
// splitting one across helpers would hide the ordering that makes it a test.
#![allow(clippy::too_many_lines)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::{CHUNKS_PER_TICK, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// The hotbar slot a harness's player currently has selected.
fn selected(harness: &Harness) -> u8 {
    harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .selected_hotbar()
}

/// Everything a test needs: a live game and a channel to act as the client.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    /// Kept alive so the temporary world directory outlives the game.
    _storage: WorldService,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::new(&storage, 4, rx).expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _storage: storage,
            _dir: dir,
        }
    }

    /// Solid stone under the spawn so the player has ground to stand on.
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

    /// Join a player and return the packets the server queued.
    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, receiver) = OutboundSender::pair(self.id, 8192);
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
        receiver
    }

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn drain_ids(out: &mut InboundReceiver) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Some(raw) = out.try_recv() {
            ids.push(raw.id);
        }
        ids
    }
}

#[test]
fn a_player_joins_and_receives_terrain_and_vitals() {
    let mut harness = Harness::new("p04-join");
    harness.build_floor();
    let mut out = harness.join("Tester");
    assert_eq!(harness.game.player_count(), 1);

    let ids = Harness::drain_ids(&mut out);
    let chunks = ids
        .iter()
        .filter(|id| **id == clientbound::play::LEVEL_CHUNK_WITH_LIGHT)
        .count();
    assert!(chunks > 0, "a join must stream chunks, saw {ids:?}");
    assert!(
        chunks <= CHUNKS_PER_TICK,
        "one tick must not stream more than {CHUNKS_PER_TICK} chunks (got {chunks})"
    );
    assert!(
        ids.contains(&clientbound::play::PLAYER_POSITION),
        "the join must place the player"
    );
    assert!(
        ids.contains(&clientbound::play::SET_HEALTH),
        "the join must sync health"
    );
    assert!(
        ids.contains(&clientbound::play::SET_DEFAULT_SPAWN_POSITION),
        "the join must sync the spawn point"
    );
    assert!(
        ids.contains(&clientbound::play::SYSTEM_CHAT),
        "the join must greet the player"
    );

    // The whole view arrives over subsequent ticks — **as many as the streaming budget needs**, which is why
    // this waits for the count rather than ticking a number of times. Eight ticks sufficed while
    // `CHUNKS_PER_TICK` was 64 and did not when a perturbation lowered it to 8, whereupon this failed with 72
    // of 81 having found no defect: the budget had been baked into an assertion about the view distance.
    let expected = ((2 * harness.game.view_distance() + 1).pow(2)) as usize;
    let mut total = chunks;
    let deadline = 400;
    for _ in 0..deadline {
        if total >= expected {
            break;
        }
        harness.game.tick().expect("tick");
        total += Harness::drain_ids(&mut out)
            .iter()
            .filter(|id| **id == mc_protocol::ids::clientbound::play::LEVEL_CHUNK_WITH_LIGHT)
            .count();
    }
    assert_eq!(
        total, expected,
        "the view distance must be exactly (2r+1)^2 chunks, and {deadline} ticks is long enough at any budget \
         the server ships"
    );

    let player = harness.game.player(harness.id).expect("player");
    let (_, sy, _) = harness.game.spawn();
    assert!(
        (player.position.y - f64::from(sy)).abs() < 1.0,
        "spawn y {} should be near the floor top {sy}",
        player.position.y
    );
}

#[test]
fn movement_is_clamped_by_collision_and_hostile_values_are_refused() {
    let mut harness = Harness::new("p04-move");
    let (sx, sy, sz) = harness.build_floor();
    let _out = harness.join("Walker");

    harness.intent(PlayIntent::MovePlayerPos {
        x: f64::from(sx) + 2.5,
        y: f64::from(sy),
        z: f64::from(sz) + 0.5,
        on_ground: true,
    });
    let moved = harness.game.player(harness.id).expect("player").position;
    assert!(
        (moved.x - (f64::from(sx) + 2.5)).abs() < 0.2,
        "a legal 2-block step should apply, got {moved:?}"
    );

    // A 500-block teleport is refused.
    let before = harness.game.player(harness.id).expect("player").position;
    harness.intent(PlayIntent::MovePlayerPos {
        x: f64::from(sx) + 500.0,
        y: f64::from(sy) + 50.0,
        z: f64::from(sz) + 500.0,
        on_ground: false,
    });
    let after = harness.game.player(harness.id).expect("player").position;
    assert!(
        (after.x - before.x).abs() < 0.001 && (after.z - before.z).abs() < 0.001,
        "a 500-block teleport must be refused ({before:?} -> {after:?})"
    );

    // Non-finite coordinates never reach the world.
    harness.intent(PlayIntent::MovePlayerPos {
        x: f64::NAN,
        y: f64::INFINITY,
        z: f64::from(sz),
        on_ground: true,
    });
    assert!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .position
            .x
            .is_finite(),
        "NaN/Inf must never reach the world"
    );
}

#[test]
fn a_wall_stops_a_player_and_a_fall_lands_on_the_floor() {
    let mut harness = Harness::new("p04-wall");
    let (sx, sy, sz) = harness.build_floor();
    let _out = harness.join("Bumper");
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    for y in sy..(sy + 3) {
        harness
            .game
            .world_mut()
            .set_block(sx + 2, y, sz, stone)
            .expect("wall");
    }

    harness.intent(PlayIntent::MovePlayerPos {
        x: f64::from(sx) + 4.5,
        y: f64::from(sy),
        z: f64::from(sz) + 0.5,
        on_ground: true,
    });
    let after_wall = harness.game.player(harness.id).expect("player").position;
    assert!(
        after_wall.x < f64::from(sx) + 2.0,
        "the wall must stop the player before x={}, got {}",
        sx + 2,
        after_wall.x
    );

    // Climb in 1-block steps (each is under the teleport cap), then descend.
    let mut y = after_wall.y;
    for _ in 0..8 {
        y += 1.0;
        harness.intent(PlayIntent::MovePlayerPos {
            x: after_wall.x,
            y,
            z: after_wall.z,
            on_ground: false,
        });
    }
    let climbed = harness.game.player(harness.id).expect("player").position.y;
    assert!(
        climbed > f64::from(sy) + 6.0,
        "the player should have climbed above {sy}, got {climbed}"
    );

    let mut y = climbed;
    while y > f64::from(sy) {
        y -= 1.0;
        harness.intent(PlayIntent::MovePlayerPos {
            x: after_wall.x,
            y: y.max(f64::from(sy)),
            z: after_wall.z,
            on_ground: false,
        });
    }
    let landed = harness.game.player(harness.id).expect("player");
    assert!(
        landed.position.y < climbed,
        "the player must have fallen back down"
    );
    assert!(landed.on_ground, "landing on the floor reports on_ground");
    assert!(
        landed.is_alive(),
        "an 8-block fall from full health must not be lethal"
    );
}

#[test]
fn breaking_and_placing_blocks_is_validated_and_broadcast() {
    let mut harness = Harness::new("p04-build");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Builder");

    let target = (sx, sy - 1, sz);
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    assert_eq!(
        harness.game.world().get_block(target.0, target.1, target.2),
        stone,
        "the floor starts solid"
    );

    let _ = Harness::drain_ids(&mut out);

    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(target.0, target.1, target.2),
        facing: 1,
        sequence: 0,
    });
    assert_eq!(
        harness.game.world().get_block(target.0, target.1, target.2),
        air,
        "an in-reach dig clears the block"
    );
    assert!(
        Harness::drain_ids(&mut out).contains(&clientbound::play::BLOCK_UPDATE),
        "the break must be broadcast"
    );

    // Out of reach: place a stone block 40 blocks away and confirm the dig is
    // refused. (The floor only spans a few chunks, so the block is created here
    // rather than assumed.)
    let far = (sx + 40, sy - 1, sz);
    harness
        .game
        .world_mut()
        .set_block(far.0, far.1, far.2, stone)
        .expect("far block");
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(far.0, far.1, far.2),
        facing: 1,
        sequence: 0,
    });
    assert_eq!(
        harness.game.world().get_block(far.0, far.1, far.2),
        stone,
        "a dig 40 blocks away must be refused"
    );

    // Placement: hold dirt and click the top face of a floor block next to the
    // player. Whether it succeeds depends on the player's own hitbox, so the test
    // asserts the invariant: the world changed iff an item was consumed.
    let dirt_item = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
    let dirt_block = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt block");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.inventory.select(0).expect("hotbar 0");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(dirt_item, 64).expect("stack"),
            )
            .expect("slot 0");
    }

    let place_at = (sx + 1, sy, sz);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 0,
    });
    let placed = harness
        .game
        .world()
        .get_block(place_at.0, place_at.1, place_at.2);
    let held_count = harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .selected_item()
        .count();
    if placed == dirt_block {
        assert_eq!(held_count, 63, "a successful placement consumes one item");
    } else {
        assert_eq!(placed, air, "the target was air before the attempt");
        assert_eq!(held_count, 64, "a refused placement consumes nothing");
    }

    // A placement into genuinely occupied space never succeeds. The floor is one
    // layer at `sy - 1`, so add a second layer at `sy - 2` and click the floor's
    // downward face: the destination is then solid.
    let below_floor = (sx, sy - 2, sz);
    harness
        .game
        .world_mut()
        .set_block(below_floor.0, below_floor.1, below_floor.2, stone)
        .expect("second floor layer");
    let held_before_refusal = harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .selected_item()
        .count();
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx, sy - 1, sz),
        face: 0,
        cursor_x: 0.5,
        cursor_y: 0.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 0,
    });
    assert_eq!(
        harness
            .game
            .world()
            .get_block(below_floor.0, below_floor.1, below_floor.2),
        stone,
        "a placement into occupied space leaves the world unchanged"
    );
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .inventory
            .selected_item()
            .count(),
        held_before_refusal,
        "a refused placement consumes nothing"
    );
}

#[test]
fn death_and_respawn_restore_the_player() {
    let mut harness = Harness::new("p04-death");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Mortal");

    {
        let player = harness.game.player_mut(harness.id).expect("player");
        let outcome = player.apply_damage(100.0);
        assert!(outcome.died, "100 damage must be lethal");
        assert!(!player.is_alive());
    }

    harness.intent(PlayIntent::MovePlayerPos {
        x: f64::from(sx) + 5.5,
        y: f64::from(sy),
        z: f64::from(sz) + 0.5,
        on_ground: true,
    });
    assert!(
        !harness.game.player(harness.id).expect("player").is_alive(),
        "a dead player must not move"
    );

    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::ClientCommand { action: 0 });
    let player = harness.game.player(harness.id).expect("player");
    assert!(player.is_alive(), "respawn restores the player");
    assert!(player.health > 0.0, "respawn restores health");
    assert!(
        (player.position.x - (f64::from(sx) + 0.5)).abs() < 0.2,
        "respawn returns to spawn, got {:?}",
        player.position
    );
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::RESPAWN),
        "the respawn packet must be sent, saw {ids:?}"
    );
}

#[test]
fn a_hostile_hotbar_index_is_rejected() {
    let mut harness = Harness::new("p04-hotbar");
    harness.build_floor();
    let _out = harness.join("Fuzzer");

    harness.intent(PlayIntent::SetCarriedItem { slot: 9 });
    assert_eq!(selected(&harness), 0, "slot 9 is out of range");
    harness.intent(PlayIntent::SetCarriedItem { slot: -5 });
    assert_eq!(selected(&harness), 0, "a negative slot is out of range");
    harness.intent(PlayIntent::SetCarriedItem { slot: 4 });
    assert_eq!(selected(&harness), 4, "a legal slot is applied");
}

#[test]
fn the_world_survives_a_save_and_reload() {
    // P04-17: the exit-gate clause "survives restart with verified state".
    let dir = TempDir::new("p04-restart");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };

    let (sx, sy, sz, diamond) = {
        let mut storage = WorldService::open(&config).expect("world opens");
        let (_tx, rx) = game_channel(16);
        let mut game = Game::new(&storage, 4, rx).expect("game");
        let diamond = game
            .registries()
            .blocks
            .default_state("minecraft:diamond_block")
            .expect("diamond block");
        let (sx, sy, sz) = game.spawn();
        game.world_mut()
            .set_block(sx, sy, sz, diamond)
            .expect("place");
        game.world_mut()
            .set_block(sx + 1, sy, sz, diamond)
            .expect("place");
        game.save_all(&mut storage).expect("saves");
        storage.close().expect("closes");
        (sx, sy, sz, diamond)
    };

    let mut storage = WorldService::open(&config).expect("world reopens");
    assert!(
        storage.storage().level().is_some(),
        "level.dat survived the restart"
    );
    let registries = mc_registry::Registries::vanilla().expect("registry");
    let chunk_pos = mc_persistence::chunk::ChunkPos::new(sx >> 4, sz >> 4);
    let data = storage
        .storage_mut()
        .read_chunk(&mc_persistence::dimension::Dimension::Overworld, chunk_pos)
        .expect("chunk reads")
        .expect("the edited chunk was saved");
    let runtime =
        mc_world::chunk::Chunk::from_chunk_data(&data, &registries.blocks).expect("converts");
    assert_eq!(
        runtime.get_block(sx, sy, sz),
        diamond,
        "the first placed block survived the restart"
    );
    assert_eq!(
        runtime.get_block(sx + 1, sy, sz),
        diamond,
        "the second placed block survived the restart"
    );
    assert_eq!(
        runtime.get_block(sx, sy - 1, sz),
        registries.blocks.air_id(),
        "a block that was never placed is still air"
    );
}

/// Breaking a block changes the light around it, and the client has to be told (P10-05).
///
/// The block update alone would leave the client rendering the hole with the old light — which is what
/// happened before `light_update` existed, and what placing a torch still looked like: the block changed and
/// the light did not.
#[tokio::test]
async fn breaking_a_block_sends_a_light_update() {
    let mut harness = Harness::new("p10-light-update");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Digger");
    // Everything the join queued: those packets include the chunks themselves, and the light update has to be
    // told apart from them.
    let _ = Harness::drain_ids(&mut out);

    // Dig the block the player stands on, opening a hole the sky light can fall into.
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(sx, sy - 1, sz),
        facing: 1,
        sequence: 0,
    });

    // One tick broads the block change and queues the chunk; the budget is four chunks a tick, so one tick is
    // enough for the single chunk that changed.
    harness.game.tick().expect("tick");

    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::LIGHT_UPDATE),
        "breaking a block must relight its chunk and tell the client; got {ids:?}"
    );
}
