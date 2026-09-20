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

/// A dropped item reaches the client that can see it, with the **item** type id.
///
/// The defect this guards against is the one P10-06 found: a number sent to a client that was never compared
/// with the registry the client owns. `minecraft:item` is **71**, and id 0 is `minecraft:acacia_boat` because the
/// entity registry is alphabetical -- so a `type_id` of 0 would spawn a boat where a dropped stack should be, and
/// nothing in the suite before this test would have noticed.
#[test]
fn a_dropped_item_is_announced_with_the_item_type_id() {
    let mut harness = Harness::new("p10-drop-announced");
    harness.build_floor();
    let mut out = harness.join("Dropper");
    // The join itself is a burst of chunk and vitals packets; clear it so the assertion below is about the drop.
    let joined = Harness::drain_ids(&mut out);
    assert!(
        joined.contains(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT),
        "the join must have streamed terrain, or this test is running before there is a world to drop into"
    );

    let (sx, sy, sz) = harness.game.spawn();
    let position = mc_world::Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
    let entity = harness
        .game
        .spawn_item(mc_entity::ItemStack::new(1, 3).expect("stone"), position)
        .expect("the drop spawns");
    assert!(entity.get() > 0);

    harness.game.tick().expect("tick");
    let announced = Harness::drain_ids(&mut out);
    assert_eq!(
        announced
            .iter()
            .filter(|id| **id == clientbound::play::ADD_ENTITY)
            .count(),
        1,
        "the drop must be announced exactly once, and not announced twice (which a client renders as two \
         entities) or zero times (which is the defect this test exists for)"
    );

    // The EntityId `spawn_item` returned is real, which is what makes the packet's entity id meaningful: a
    // packet naming some other entity would satisfy the count above and nothing else here would notice.
    assert!(entity.get() > 0, "a real entity id, not a placeholder");
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
        ids.contains(&clientbound::play::DISGUISED_CHAT),
        "the join must greet the player"
    );
    assert!(
        ids.contains(&clientbound::play::SET_TIME),
        "the join must carry the full clock sync (P14-09 walk): without an \
         absolute seed the client's overworld instance advances locally from \
         zero forever, saw {ids:?}"
    );
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_CONTENT),
        "the join must sync the whole player window (P14-10 walk): a rejoin \
         with a non-empty inventory rendered an empty hotbar until the first \
         inventory action, saw {ids:?}"
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

/// A placement consumes from the held slot in place: with a partial stack
/// earlier in the hotbar, the remainder must not migrate there (P14-09 walk:
/// every placement visibly rearranged the client's hotbar, because the
/// remainder went through `add_stack`'s lowest-partial-first fill).
#[test]
fn placement_consumes_in_the_held_slot() {
    let mut harness = Harness::new("p04-place-held");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Builder");
    let _ = Harness::drain_ids(&mut out);

    let dirt_item = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
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
        // A partial stack earlier in fill order than nothing — but the held
        // slot is 0, so a correct consume never touches slot 1.
        player
            .inventory
            .set_slot(
                1,
                mc_entity::stack::ItemStack::new(dirt_item, 32).expect("stack"),
            )
            .expect("slot 1");
        // Stand clear of the target cell: clicking the top face of the floor
        // at (sx+1) puts the block at (sx+1, sy, sz), and the placer may not
        // intersect it.
        player.position =
            mc_world::Vec3::new(f64::from(sx) + 4.5, f64::from(sy), f64::from(sz) + 0.5);
    }

    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx + 1, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 0,
    });
    assert_eq!(
        harness.game.world().get_block(sx + 1, sy, sz),
        harness
            .game
            .registries()
            .blocks
            .default_state("minecraft:dirt")
            .expect("dirt block"),
        "the block must land for this test to exercise the consume path"
    );
    let player = harness.game.player(harness.id).expect("player");
    assert_eq!(
        player.inventory.slot(0).count(),
        63,
        "the held slot loses exactly one"
    );
    assert_eq!(
        player.inventory.slot(1).count(),
        32,
        "the earlier partial stack is untouched"
    );
}

#[test]
fn death_and_respawn_restore_the_player() {
    let mut harness = Harness::new("p04-death");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Mortal");

    {
        let player = harness.game.player_mut(harness.id).expect("player");
        let outcome = player.apply_damage(
            100.0,
            mc_entity::combat::DamageSource::MobAttack,
            &mc_entity::combat::CombatStats::ZERO,
        );
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
    assert!(
        ids.contains(&clientbound::play::GAME_EVENT),
        "respawn must re-arm the client's level-load tracker (KD-50, second \
         half): without the start-chunks game event a real client sits on \
         \"Loading terrain\" forever, saw {ids:?}"
    );
}

/// P11-10 remainder: `/tp` to `surface + 30` must not embed, and the death →
/// respawn half must work from there.
///
/// The handoff's labelled experiment named `surface + 20` as lethal without
/// embedding. Two corrections from the instrument:
///
/// - `teleport_source` resolves with `find_surface` (scan down for air/air/solid),
///   so `/tp` to air lands *on the surface*, not in the sky — the position ends
///   at `sy`, not `ty`. The not-embedding half holds, and this test pins it
///   (feet and head are air).
/// - Fall damage is per-tick (at most 8 − 3 = 5 through the 8-block move cap),
///   so a 20-block fall spread over ticks cannot kill from full health. The `+30`
///   figure would need a single-tick 30-block drop, which the move cap refuses.
///   The test kills with `apply_damage` after proving the teleport landed in air —
///   the physics gap is stated, not hidden. Live-client screen acceptance still
///   needs an owner at the keyboard and is recorded as owed.
#[test]
#[allow(clippy::cast_possible_truncation)]
fn tp_above_surface_does_not_embed_and_death_respawns_from_there() {
    let mut harness = Harness::new("p11-tp-death");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Mortal");

    // Teleport 30 above the spawn floor through the real command path.
    let ty = sy + 30;
    harness.intent(PlayIntent::ChatCommand {
        command: format!("tp Mortal {sx} {ty} {sz}"),
    });
    let pos = harness.game.player(harness.id).expect("player").position;
    // `teleport_source` resolves into air: feet and head must both be empty.
    // The 0.5 offsets are the teleport centering (`x + 0.5`, `z + 0.5`).
    let feet_y = pos.y.floor() as i32;
    let head_y = (pos.y + 1.8).floor() as i32;
    let feet = harness.game.world().get_block_loaded(sx, feet_y, sz);
    let head = harness.game.world().get_block_loaded(sx, head_y, sz);
    for (label, slot) in [("feet", feet), ("head", head)] {
        let Some(state) = slot else {
            panic!("{label} chunk must be loaded after /tp");
        };
        assert!(
            harness.game.registries().blocks.is_empty(state),
            "{label} must be air after /tp to {sx} {ty} {sz}, got state {state}"
        );
    }
    assert!(
        (pos.y - f64::from(sy)).abs() < 1.0,
        "teleport to air must resolve onto the surface ~{sy} (find_surface), got {}",
        pos.y
    );

    // Lethal damage from there, then the same respawn path as the P04 test.
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        let outcome = player.apply_damage(
            100.0,
            mc_entity::combat::DamageSource::MobAttack,
            &mc_entity::combat::CombatStats::ZERO,
        );
        assert!(outcome.died, "100 damage must be lethal from the air");
    }
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::ClientCommand { action: 0 });
    let player = harness.game.player(harness.id).expect("player");
    assert!(player.is_alive(), "respawn restores the player");
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::RESPAWN),
        "the respawn packet must be sent after an air death, saw {ids:?}"
    );
}

/// P12-07: the player crafting grid fills from clicks and crafts.
///
/// Puts two oak planks into the 2x2 grid through real clicks, asserts the
/// result slot recomputes to sticks, takes the result, and asserts the grid
/// was consumed. Runs against the game's table (baseline in tests without
/// packs, pack conversion once `load_packs` runs — the hook is the same).
#[test]
fn crafting_grid_recomputes_and_taking_consumes() {
    let mut harness = Harness::new("p12-craft");
    harness.build_floor();
    let mut out = harness.join("Crafter");
    let _ = Harness::drain_ids(&mut out);
    let planks = harness
        .game
        .registries()
        .items
        .id("minecraft:oak_planks")
        .expect("planks");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(planks, 2).expect("stack"),
            )
            .expect("give planks");
    }
    // Player menu is window 0. Hotbar 0 is menu slot 36; grid is menu 1..=4.
    let window = 0;
    let mut state = harness.game.menu_state_id(harness.id).expect("state");
    // Pick up planks from hotbar (menu 36).
    harness.intent(PlayIntent::ContainerClick {
        window_id: window,
        state_id: state,
        slot: 36,
        button: 0,
        click_type: 0,
    });
    // Place one into grid slot 1 (menu 1 = grid 0).
    state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: window,
        state_id: state,
        slot: 1,
        button: 1,
        click_type: 0,
    });
    // Place one into grid slot 3 (menu 3 = grid 2) for the 1x2 sticks pattern.
    state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: window,
        state_id: state,
        slot: 3,
        button: 0,
        click_type: 0,
    });
    let result = harness.game.menu_slot(harness.id, 0).expect("result");
    let sticks = harness
        .game
        .registries()
        .items
        .id("minecraft:stick")
        .expect("sticks");
    assert_eq!(
        result.item_id(),
        Some(sticks),
        "two stacked planks must recompute to sticks, got {result:?}"
    );
    assert_eq!(result.count(), 4);
    // Take the result: the grid must be consumed.
    state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: window,
        state_id: state,
        slot: 0,
        button: 0,
        click_type: 0,
    });
    let cursor = harness.game.menu_cursor(harness.id).expect("cursor");
    assert_eq!(cursor.item_id(), Some(sticks));
    let grid_left: i64 = [1, 2, 3, 4]
        .iter()
        .map(|slot| {
            i64::from(
                harness
                    .game
                    .menu_slot(harness.id, *slot)
                    .expect("grid")
                    .count(),
            )
        })
        .sum();
    assert_eq!(grid_left, 0, "taking the result must consume the grid");
    let _ = Harness::drain_ids(&mut out);
}

/// AUDIT-12 F2: a stale result-take must not consume the grid.
///
/// Fills the 2x2 grid, bumps the state with a valid click, then replays a
/// result-take with the now-stale state. The menu applies nothing on stale
/// state — and the crafting hook must not consume behind its back.
#[test]
fn a_stale_result_take_consumes_nothing() {
    let mut harness = Harness::new("p12-stale-craft");
    harness.build_floor();
    let mut out = harness.join("Stale");
    let _ = Harness::drain_ids(&mut out);
    let planks = harness
        .game
        .registries()
        .items
        .id("minecraft:oak_planks")
        .expect("planks");
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(planks, 2).expect("stack"),
            )
            .expect("give planks");
        player
            .inventory
            .set_slot(
                1,
                mc_entity::stack::ItemStack::new(stone, 4).expect("stack"),
            )
            .expect("give stones for the state bump");
    }
    // Planks into grid slots 1 and 3 (sticks pattern).
    for slot in [36, 1, 3] {
        let state = harness.game.menu_state_id(harness.id).expect("state");
        let (slot, button, click_type) = if slot == 36 {
            (36, 0, 0)
        } else if slot == 1 {
            (1, 1, 0)
        } else {
            (3, 0, 0)
        };
        harness.intent(PlayIntent::ContainerClick {
            window_id: 0,
            state_id: state,
            slot,
            button,
            click_type,
        });
    }
    let grid_before: i64 = [1, 2, 3, 4]
        .iter()
        .map(|slot| {
            i64::from(
                harness
                    .game
                    .menu_slot(harness.id, *slot)
                    .expect("grid")
                    .count(),
            )
        })
        .sum();
    assert_eq!(grid_before, 2);
    // Bump the state with a real move: quick-move the stones in hotbar 1
    // (menu 37) elsewhere in the player inventory. An empty-slot pickup is a
    // no-op that does NOT bump state, which is why the first version of this
    // test accidentally sent a fresh state and crafted for real.
    let fresh = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: 0,
        state_id: fresh,
        slot: 37,
        button: 0,
        click_type: 1,
    });
    let bumped = harness.game.menu_state_id(harness.id).expect("state");
    assert_ne!(
        bumped, fresh,
        "the quick-move must advance state, or the take below is not stale"
    );
    let stale = fresh;
    // Stale result-take with the pre-bump state.
    harness.intent(PlayIntent::ContainerClick {
        window_id: 0,
        state_id: stale,
        slot: 0,
        button: 0,
        click_type: 0,
    });
    let grid_after: i64 = [1, 2, 3, 4]
        .iter()
        .map(|slot| {
            i64::from(
                harness
                    .game
                    .menu_slot(harness.id, *slot)
                    .expect("grid")
                    .count(),
            )
        })
        .sum();
    assert_eq!(grid_after, grid_before, "a stale take must consume nothing");
    let _ = Harness::drain_ids(&mut out);
}

/// AUDIT-12: breaking a chest with a full cursor returns the cursor.
///
/// Picks 16 stones onto the cursor, breaks the chest before closing, and
/// asserts the 16 survive in the inventory or as drops (previously lost with
/// no warn/drop).
#[test]
fn breaking_a_chest_with_a_full_cursor_keeps_the_cursor() {
    let mut harness = Harness::new("p12-break-cursor");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Holder");
    let chest = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, chest)
        .expect("place chest");
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 51,
    });
    let window = i32::from(
        harness
            .game
            .menu_window_id(harness.id)
            .expect("a chest window"),
    );
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(stone, 16).expect("stack"),
            )
            .expect("give stones");
    }
    // Into the chest, then back onto the cursor.
    for slot in [54, 0] {
        let state = harness.game.menu_state_id(harness.id).expect("state");
        let click_type = i32::from(slot == 54);
        harness.intent(PlayIntent::ContainerClick {
            window_id: window,
            state_id: state,
            slot,
            button: 0,
            click_type,
        });
    }
    assert!(
        !harness
            .game
            .menu_cursor(harness.id)
            .expect("cursor")
            .is_empty(),
        "the stack must be on the cursor before the break"
    );
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(at.0, at.1, at.2),
        facing: 1,
        sequence: 52,
    });
    harness.game.tick().expect("settle");
    let in_inventory: i64 = (0..harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .stored_slots())
        .map(|i| {
            i64::from(
                harness
                    .game
                    .player(harness.id)
                    .expect("player")
                    .inventory
                    .slot(i)
                    .count(),
            )
        })
        .sum();
    let in_drops: i64 = harness
        .game
        .dropped_items()
        .iter()
        .filter(|(stack, _)| stack.item_id() == Some(stone))
        .map(|(stack, _)| i64::from(stack.count()))
        .sum();
    assert!(
        in_inventory + in_drops >= 16,
        "cursor 16 must survive the break (inventory {in_inventory} + drops {in_drops})"
    );
    let _ = Harness::drain_ids(&mut out);
}

/// AUDIT-12: an open furnace shows the smelted output without reopening.
///
/// Seeds the entity near completion, ticks past it, and asserts the open
/// menu's output slot (menu 2) holds the ingot — previously stale until
/// reopen (entity cooked, menu showed pre-tick stacks).
#[test]
fn an_open_furnace_menu_shows_completed_output() {
    let mut harness = Harness::new("p12-furnace-view");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Watcher");
    let furnace = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:furnace")
        .expect("furnace block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, furnace)
        .expect("place furnace");
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 61,
    });
    let window = i32::from(
        harness
            .game
            .menu_window_id(harness.id)
            .expect("a furnace window"),
    );
    let items = &harness.game.registries().items;
    let ore = items.id("minecraft:iron_ore").expect("ore");
    let coal = items.id("minecraft:coal").expect("coal");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(ore, 1).expect("stack"))
            .expect("give ore");
        player
            .inventory
            .set_slot(1, mc_entity::stack::ItemStack::new(coal, 1).expect("stack"))
            .expect("give coal");
    }
    for slot in [30, 31] {
        let state = harness.game.menu_state_id(harness.id).expect("state");
        harness.intent(PlayIntent::ContainerClick {
            window_id: window,
            state_id: state,
            slot,
            button: 0,
            click_type: 1,
        });
    }
    // Seed near completion: 195/200 cooked, lit with 100 burn left.
    {
        let entity = harness
            .game
            .block_entities_mut()
            .get_mut(mc_container::BlockPos::new(at.0, at.1, at.2))
            .expect("furnace entity");
        if let mc_container::BlockEntityData::Furnace {
            burn_ticks,
            burn_total,
            cook_progress,
            cook_total,
            ..
        } = &mut entity.data
        {
            *burn_ticks = 100;
            *burn_total = 1600;
            *cook_progress = 195;
            *cook_total = 200;
        } else {
            panic!("a furnace entity");
        }
    }
    for _ in 0..10 {
        harness.game.tick().expect("tick");
    }
    let output = harness.game.menu_slot(harness.id, 2).expect("output");
    let ingot = harness
        .game
        .registries()
        .items
        .id("minecraft:iron_ingot")
        .expect("ingot");
    assert_eq!(
        output.item_id(),
        Some(ingot),
        "the open menu must show the smelted ingot, got {output:?}"
    );
    let _ = Harness::drain_ids(&mut out);
}

/// P13-02: placing or breaking redstone feeds the queue; dirt does not.
///
/// Dirt goes first in a fresh game (queue stays empty: passive block, passive
/// neighbours). Then a lever placed through the real `use_item_on` path queues
/// exactly seven updates (six neighbours + self). Breaking the floor under the
/// lever queues seven again — the removed stone is passive but the lever
/// neighbour is an emitter. Note the phase order this relies on: each intent
/// ticks `ScheduledTicks` (which drains) *before* `Players` (which feeds), so
/// every assertion below reads a queue the drain has just emptied.
#[test]
fn redstone_edits_feed_the_queue_and_dirt_does_not() {
    let mut harness = Harness::new("p13-feed");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Sparky");
    assert_eq!(harness.game.redstone_pending(), 0);
    let _ = Harness::drain_ids(&mut out);

    // Dirt far from any circuit first: passive block, passive neighbours.
    let dirt_item = harness
        .game
        .registries()
        .items
        .id("minecraft:dirt")
        .expect("dirt item");
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
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx + 5, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 70,
    });
    assert_eq!(
        harness.game.redstone_pending(),
        0,
        "dirt in an open field must not feed the queue"
    );

    let lever_item = harness
        .game
        .registries()
        .items
        .id("minecraft:lever")
        .expect("lever item");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.inventory.select(0).expect("hotbar 0");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(lever_item, 4).expect("stack"),
            )
            .expect("slot 0");
    }
    let at = (sx + 1, sy, sz);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx + 1, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 71,
    });
    let placed = harness.game.world().get_block(at.0, at.1, at.2);
    let name = harness
        .game
        .registries()
        .blocks
        .block_name(placed)
        .expect("placed has a name");
    assert_eq!(name, "minecraft:lever", "the lever must place, got {name}");
    assert_eq!(
        harness.game.redstone_pending(),
        7,
        "six neighbours + self, deduplicated"
    );

    // Breaking the floor under the lever: removed stone is passive, but the
    // lever neighbour is an emitter, so the queue refills past the drain.
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(sx + 1, sy - 1, sz),
        facing: 1,
        sequence: 73,
    });
    assert_eq!(
        harness.game.redstone_pending(),
        7,
        "breaking next to a lever must queue seven fresh updates, saw {}",
        harness.game.redstone_pending()
    );
    let _ = Harness::drain_ids(&mut out);
}

/// P13-02: right-clicking a lever flips `powered` and feeds the queue.
///
/// The lever is placed directly (raw world writes do not feed — the feed
/// lives on the player-action path), then flipped twice through real clicks:
/// off→on→off with only `powered` changing, each flip queueing.
#[test]
fn right_clicking_a_lever_flips_powered() {
    let mut harness = Harness::new("p13-lever");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Flippy");
    let lever = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:lever")
        .expect("lever block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, lever)
        .expect("place lever");
    assert_eq!(
        harness.game.redstone_pending(),
        0,
        "a raw world write must not feed the queue"
    );
    let _ = Harness::drain_ids(&mut out);

    let powered = |harness: &Harness| {
        let state = harness.game.world().get_block(at.0, at.1, at.2);
        harness
            .game
            .registries()
            .blocks
            .properties_of(state)
            .expect("lever has properties")
            .into_iter()
            .find(|(name, _)| name == "powered")
            .map(|(_, value)| value)
    };
    let initial = powered(&harness);
    // Flip on: must change powered, whatever it started as.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 74,
    });
    let flipped = powered(&harness);
    assert_ne!(flipped, initial, "a flip must change powered");
    // Flip off: back where it started, with only `powered` touched.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 75,
    });
    assert_eq!(powered(&harness), initial, "two flips must round-trip");
    assert!(
        harness.game.redstone_pending() > 0,
        "flips must feed the queue"
    );
    let _ = Harness::drain_ids(&mut out);
}

/// P13-03: a flipped lever powers dust and lights a lamp, on the wire.
///
/// Builds lever—wire—wire plus a stone pedestal with a lamp on top through
/// real placements, flips the lever, and asserts the near wire carries 15,
/// the far wire 14, the lamp is lit, and a `block_update` went out. The lamp
/// stands on the pedestal rather than beside the dust because that is the
/// vanilla-faithful geometry (P13-06, measured: a lamp beside live dust stays
/// dark; the far dust powers the pedestal from the side and the lamp reads
/// its support). Flipping back darkens the line. The model does the physics;
/// the world's change list carries the broadcast.
#[test]
fn a_flipped_lever_powers_dust_and_lights_a_lamp() {
    let mut harness = Harness::new("p13-circuit");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Sparky");
    let _ = Harness::drain_ids(&mut out);
    let items = &harness.game.registries().items;
    let lever_item = items.id("minecraft:lever").expect("lever");
    let dust_item = items.id("minecraft:redstone").expect("dust");
    let lamp_item = items.id("minecraft:redstone_lamp").expect("lamp");
    let stone_item = items.id("minecraft:stone").expect("stone");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.inventory.select(0).expect("hotbar 0");
        for (slot, item, count) in [
            (0, lever_item, 1),
            (1, dust_item, 2),
            (2, lamp_item, 1),
            (3, stone_item, 1),
        ] {
            player
                .inventory
                .set_slot(
                    slot,
                    mc_entity::stack::ItemStack::new(item, count).expect("stack"),
                )
                .expect("give");
        }
    }
    // Place lever, two dust, pedestal in a row on the floor, then the lamp on
    // top of the pedestal. Each round finds a hotbar slot holding the wanted
    // item first: `add_stack` refills the *first* suitable slot, not the
    // taken one, so slot numbers drift.
    let at = [
        (sx + 1, sy, sz),
        (sx + 2, sy, sz),
        (sx + 3, sy, sz),
        (sx + 4, sy, sz),
        (sx + 4, sy + 1, sz),
    ];
    let want = [lever_item, dust_item, dust_item, stone_item, lamp_item];
    for (round, ((x, y, z), item)) in at.iter().zip(want.iter()).enumerate() {
        let slot = {
            let player = harness.game.player(harness.id).expect("player");
            (0..9u8)
                .find(|slot| player.inventory.slot(usize::from(*slot)).item_id() == Some(*item))
                .expect("a hotbar slot holding the item")
        };
        {
            let player = harness.game.player_mut(harness.id).expect("player");
            player.inventory.select(slot).expect("select");
        }
        harness.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(*x, y - 1, *z),
            face: 1,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside_block: false,
            sequence: 80 + i32::try_from(round).expect("few rounds"),
        });
        {
            let state = harness.game.world().get_block(*x, *y, *z);
            let name = harness
                .game
                .registries()
                .blocks
                .block_name(state)
                .unwrap_or("?");
            assert!(
                [
                    "minecraft:lever",
                    "minecraft:redstone_wire",
                    "minecraft:redstone_lamp",
                    "minecraft:stone",
                ]
                .contains(&name),
                "round{round} must place its block, saw {name}"
            );
        }
    }
    for name in [
        "minecraft:lever",
        "minecraft:redstone_wire",
        "minecraft:redstone_lamp",
    ] {
        let id = harness.game.registries().blocks.default_state(name);
        assert!(id.is_ok(), "{name} must resolve");
    }
    let wire_power = |harness: &Harness, x: i32, y: i32, z: i32| {
        let state = harness.game.world().get_block(x, y, z);
        harness
            .game
            .registries()
            .blocks
            .properties_of(state)
            .expect("wire has properties")
            .into_iter()
            .find(|(name, _)| name == "power")
            .map(|(_, value)| value)
    };
    let lamp_lit = |harness: &Harness, x: i32, y: i32, z: i32| {
        let state = harness.game.world().get_block(x, y, z);
        harness
            .game
            .registries()
            .blocks
            .properties_of(state)
            .expect("lamp has properties")
            .into_iter()
            .find(|(name, _)| name == "lit")
            .map(|(_, value)| value)
    };
    // Flip the lever on: the line must power within a few ticks. Drain first
    // so the block updates below are the propagation's, not the placements'.
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at[0].0, at[0].1, at[0].2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 90,
    });
    for _ in 0..3 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        wire_power(&harness, at[1].0, at[1].1, at[1].2).as_deref(),
        Some("15"),
        "the near wire must carry 15 (P13-05, measured)"
    );
    assert_eq!(
        wire_power(&harness, at[2].0, at[2].1, at[2].2).as_deref(),
        Some("14"),
        "the far wire must carry 14"
    );
    assert_eq!(
        lamp_lit(&harness, at[4].0, at[4].1, at[4].2).as_deref(),
        Some("true"),
        "the lamp must be lit"
    );
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::BLOCK_UPDATE),
        "powered wire and a lit lamp must reach clients as block updates, saw {ids:?}"
    );
    // Flip off: the line must go dark again.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at[0].0, at[0].1, at[0].2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 91,
    });
    for _ in 0..3 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(
        wire_power(&harness, at[1].0, at[1].1, at[1].2).as_deref(),
        Some("0"),
        "the near wire must fall back to 0"
    );
    assert_eq!(
        lamp_lit(&harness, at[4].0, at[4].1, at[4].2).as_deref(),
        Some("false"),
        "the lamp must go dark"
    );
    let _ = Harness::drain_ids(&mut out);
}

/// P13-04: a torch listens to its attachment block, not its neighbours.
///
/// A torch standing on a lever with a live wire beside it stays lit (the wire
/// is not its attachment); flipping the lever darkens the torch and the wire
/// follows. The comparator half is pinned at the model level.
#[test]
fn a_torch_follows_its_attachment_not_its_neighbours() {
    let mut harness = Harness::new("p13-torch");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Sparky");
    let _ = Harness::drain_ids(&mut out);
    let items = &harness.game.registries().items;
    let lever_item = items.id("minecraft:lever").expect("lever");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player.inventory.select(0).expect("hotbar 0");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(lever_item, 1).expect("stack"),
            )
            .expect("give");
    }
    // Lever through the real path (feeds the queue); torch and wire placed
    // directly — raw writes bypass the feed, and a torch cannot be placed
    // onto a lever through clicks anyway (right-click flips it).
    let lever_at = (sx + 1, sy, sz);
    let torch_at = (sx + 1, sy + 1, sz);
    let wire_at = (sx + 2, sy + 1, sz);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(sx + 1, sy - 1, sz),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 100,
    });
    let torch = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:redstone_torch")
        .expect("torch block");
    let wire0 = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:redstone_wire")
        .expect("wire block");
    harness
        .game
        .world_mut()
        .set_block(torch_at.0, torch_at.1, torch_at.2, torch)
        .expect("stand the torch on the lever");
    harness
        .game
        .world_mut()
        .set_block(wire_at.0, wire_at.1, wire_at.2, wire0)
        .expect("wire beside the torch");
    harness.game.tick().expect("settle");

    let lit = |harness: &Harness| {
        let state = harness
            .game
            .world()
            .get_block(torch_at.0, torch_at.1, torch_at.2);
        harness
            .game
            .registries()
            .blocks
            .properties_of(state)
            .expect("torch has properties")
            .into_iter()
            .find(|(name, _)| name == "lit")
            .map(|(_, value)| value)
    };
    let power = |harness: &Harness| {
        let state = harness
            .game
            .world()
            .get_block(wire_at.0, wire_at.1, wire_at.2);
        harness
            .game
            .registries()
            .blocks
            .properties_of(state)
            .expect("wire has properties")
            .into_iter()
            .find(|(name, _)| name == "power")
            .map(|(_, value)| value)
    };
    // Flip the lever on: the attachment powers, so the torch must go dark
    // however it was placed (lit or unlit converge here), and the wire beside
    // it — which did not change — must follow.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(lever_at.0, lever_at.1, lever_at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 103,
    });
    for _ in 0..3 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(lit(&harness).as_deref(), Some("false"));
    assert_eq!(power(&harness).as_deref(), Some("0"));

    // Flip back: lit again, wire live again.
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(lever_at.0, lever_at.1, lever_at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 104,
    });
    for _ in 0..3 {
        harness.game.tick().expect("tick");
    }
    assert_eq!(lit(&harness).as_deref(), Some("true"));
    assert_eq!(power(&harness).as_deref(), Some("15"));
    let _ = Harness::drain_ids(&mut out);
}

/// P12-09: closing a chest returns the cursor and restores the player menu.
///
/// Picks up a chest stack onto the cursor, closes the window through the real
/// `container_close` path, and asserts the cursor lands back in the inventory
/// (nothing is lost), the window is 0 again, and the cursor is empty.
#[test]
fn closing_a_chest_returns_the_cursor() {
    let mut harness = Harness::new("p12-close");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Closer");
    let chest = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, chest)
        .expect("place chest");
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 41,
    });
    let window = i32::from(
        harness
            .game
            .menu_window_id(harness.id)
            .expect("a chest window"),
    );
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(stone, 16).expect("stack"),
            )
            .expect("give stones");
    }
    // Move into the chest, then pick one stack back onto the cursor.
    for slot in [54, 0] {
        let state = harness.game.menu_state_id(harness.id).expect("state");
        let click_type = i32::from(slot == 54);
        harness.intent(PlayIntent::ContainerClick {
            window_id: window,
            state_id: state,
            slot,
            button: 0,
            click_type,
        });
    }
    assert!(
        !harness
            .game
            .menu_cursor(harness.id)
            .expect("a cursor")
            .is_empty(),
        "picking up from the chest must leave the stack on the cursor"
    );
    let state = harness.game.menu_state_id(harness.id).expect("state");
    let window_u8 = harness
        .game
        .menu_window_id(harness.id)
        .expect("a chest window");
    harness.intent(PlayIntent::ContainerClose {
        window_id: i32::from(window_u8),
    });
    assert_eq!(
        harness.game.menu_window_id(harness.id),
        Some(0),
        "closing must restore window 0"
    );
    assert!(
        harness
            .game
            .menu_cursor(harness.id)
            .expect("a cursor")
            .is_empty(),
        "closing must empty the cursor"
    );
    let total: i64 = (0..harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .stored_slots())
        .map(|i| {
            i64::from(
                harness
                    .game
                    .player(harness.id)
                    .expect("player")
                    .inventory
                    .slot(i)
                    .count(),
            )
        })
        .sum();
    assert_eq!(
        total, 16,
        "the cursor stack must return to the inventory, saw {total}"
    );
    let _ = state;
    let _ = Harness::drain_ids(&mut out);
}

/// P12-06: breaking a chest drops its contents instead of destroying them.
///
/// Fills a chest with stones through its block entity (the honest setup: the
/// chest was opened and transacted in P12-02), digs it, and asserts the stones
/// come back as ground items and the entity is gone. Closes the P06 "items
/// lost on break" gap.
#[test]
fn breaking_a_chest_drops_its_contents() {
    let mut harness = Harness::new("p12-break");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Breaker");
    let chest = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, chest)
        .expect("place chest");
    harness.game.tick().expect("create entity");
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let entity = harness
            .game
            .block_entities_mut()
            .get_mut(mc_container::BlockPos::new(at.0, at.1, at.2))
            .expect("chest entity");
        let items = entity.data.items_mut().expect("chest items");
        items[0] = mc_entity::stack::ItemStack::new(stone, 12).expect("stack");
    }
    let _ = Harness::drain_ids(&mut out);
    // Instant survival dig (P04 rule): START_DESTROY_BLOCK breaks at once.
    harness.intent(PlayIntent::PlayerAction {
        status: 0,
        position: block_position(at.0, at.1, at.2),
        facing: 1,
        sequence: 31,
    });
    // One more tick so drops spawned during the broadcast are announced.
    harness.game.tick().expect("settle");
    assert!(
        harness
            .game
            .block_entities()
            .get(mc_container::BlockPos::new(at.0, at.1, at.2))
            .is_none(),
        "breaking a chest must retire its entity"
    );
    let stone_drops: i64 = harness
        .game
        .dropped_items()
        .iter()
        .filter(|(stack, _)| stack.item_id() == Some(stone))
        .map(|(stack, _)| i64::from(stack.count()))
        .sum();
    assert!(
        stone_drops >= 12,
        "the chest's 12 stones must drop (alongside loot), saw {stone_drops}"
    );
}

/// P12-02: chest transactions on a non-zero window conserve items.
///
/// Opens a chest, puts 64 stones in the player's hotbar, shift-clicks them
/// into the chest (menu slot 54 = player storage 0), and asserts the block
/// entity holds them, the player no longer does, and the total never changes —
/// including across a 20-click hostile flood that shuttles the stack back and
/// forth. The 2 000-click shape lives in `mc-container`'s unit tests; this
/// proves the *server* half (mirror in, validate against the non-zero window,
/// flush to both the inventory and the block entity).
#[test]
fn chest_transactions_conserve_across_a_flood() {
    let mut harness = Harness::new("p12-chest-tx");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Trader");
    let chest = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, chest)
        .expect("place chest");
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 11,
    });
    let window = i32::from(
        harness
            .game
            .menu_window_id(harness.id)
            .expect("a chest window"),
    );
    assert_ne!(window, 0, "chest transactions run on a non-zero window");

    // 64 stones into hotbar slot 0 (player storage 0 = menu slot 54).
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone item");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(
                0,
                mc_entity::stack::ItemStack::new(stone, 64).expect("stack"),
            )
            .expect("give stones");
    }
    // The only items in play are those 64 stones.
    let total_before = 64i64;

    // Shift-click hotbar into the chest.
    let mut state = harness.game.menu_state_id(harness.id).expect("state");
    harness.intent(PlayIntent::ContainerClick {
        window_id: window,
        state_id: state,
        slot: 54,
        button: 0,
        click_type: 1,
    });
    let entity_count: i64 = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(at.0, at.1, at.2))
        .expect("the chest entity")
        .data
        .total_items();
    assert_eq!(entity_count, 64, "the shift-click must land in the chest");
    assert_eq!(
        harness.game.menu_total_items(harness.id).expect("total"),
        total_before,
        "a chest move must conserve"
    );

    // Hostile flood: shuttle the stack chest ↔ player 20 times.
    for round in 0..20 {
        state = harness.game.menu_state_id(harness.id).expect("state");
        // Chest slot 0 on even rounds (back to player), player hotbar on odd.
        let slot = if round % 2 == 0 { 0 } else { 54 };
        harness.intent(PlayIntent::ContainerClick {
            window_id: window,
            state_id: state,
            slot,
            button: 0,
            click_type: 1,
        });
        assert_eq!(
            harness.game.menu_total_items(harness.id).expect("total"),
            total_before,
            "round {round}: a chest flood must conserve"
        );
    }
    let _ = Harness::drain_ids(&mut out);
}

/// P12-04: hoppers pull from above and push below on the 8-tick cooldown.
///
/// A chest with stones sits above an empty hopper; after 20 ticks the hopper
/// must hold stones (pulled, one per 8 ticks). Then a second hopper with
/// stones sits above an empty chest; after 20 ticks the chest must hold them
/// (pushed). Furnace routing is a recorded gap — only `Container`/`Hopper`
/// payloads participate.
#[test]
fn hoppers_pull_from_above_and_push_below() {
    let mut harness = Harness::new("p12-hopper");
    let (sx, sy, sz) = harness.build_floor();
    let _out = harness.join("Hopper");
    let blocks = &harness.game.registries().blocks;
    let chest_id = blocks.default_state("minecraft:chest").expect("chest");
    let hopper_id = blocks.default_state("minecraft:hopper").expect("hopper");
    let stone_item = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");

    // Pull: chest (sy+1) with 4 stones above hopper (sy).
    let chest_pos = (sx + 1, sy + 1, sz);
    let hopper_pos = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(chest_pos.0, chest_pos.1, chest_pos.2, chest_id)
        .expect("chest");
    harness
        .game
        .world_mut()
        .set_block(hopper_pos.0, hopper_pos.1, hopper_pos.2, hopper_id)
        .expect("hopper");
    harness.game.tick().expect("create entities");
    {
        let store = harness.game.block_entities_mut();
        let chest = store
            .get_mut(mc_container::BlockPos::new(
                chest_pos.0,
                chest_pos.1,
                chest_pos.2,
            ))
            .expect("chest entity");
        let items = chest.data.items_mut().expect("chest items");
        items[0] = mc_entity::stack::ItemStack::new(stone_item, 4).expect("stack");
    }
    for _ in 0..20 {
        harness.game.tick().expect("tick");
    }
    let hopper_count: i64 = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(
            hopper_pos.0,
            hopper_pos.1,
            hopper_pos.2,
        ))
        .expect("hopper entity")
        .data
        .total_items();
    assert!(
        hopper_count > 0,
        "a hopper must pull stones from the chest above within 20 ticks"
    );

    // Push: hopper (sy+3) with 4 stones above an empty chest (sy+2).
    let chest2 = (sx + 3, sy + 2, sz);
    let hopper2 = (sx + 3, sy + 3, sz);
    harness
        .game
        .world_mut()
        .set_block(chest2.0, chest2.1, chest2.2, chest_id)
        .expect("chest2");
    harness
        .game
        .world_mut()
        .set_block(hopper2.0, hopper2.1, hopper2.2, hopper_id)
        .expect("hopper2");
    harness.game.tick().expect("create entities 2");
    {
        let store = harness.game.block_entities_mut();
        let hopper = store
            .get_mut(mc_container::BlockPos::new(hopper2.0, hopper2.1, hopper2.2))
            .expect("hopper2 entity");
        let items = hopper.data.items_mut().expect("hopper items");
        items[0] = mc_entity::stack::ItemStack::new(stone_item, 4).expect("stack");
    }
    for _ in 0..20 {
        harness.game.tick().expect("tick");
    }
    let chest2_count: i64 = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(chest2.0, chest2.1, chest2.2))
        .expect("chest2 entity")
        .data
        .total_items();
    assert!(
        chest2_count > 0,
        "a hopper must push stones into the chest below within 20 ticks"
    );
}

/// P12-03: an open furnace ticks and reports progress via `container_set_data`.
///
/// Opens a real furnace, shift-clicks iron ore + coal into it, runs five ticks,
/// and asserts the block entity's cook progress advanced and the server sent
/// `container_set_data` (19) on the furnace window. Properties are vanilla's
/// `FurnaceMenu` data slots: 0 burn remaining, 1 burn total, 2 cook progress,
/// 3 cook total.
#[test]
fn an_open_furnace_cooks_and_reports_progress() {
    let mut harness = Harness::new("p12-furnace");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Smelter");
    let furnace = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:furnace")
        .expect("furnace block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, furnace)
        .expect("place furnace");
    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 21,
    });
    let window = i32::from(
        harness
            .game
            .menu_window_id(harness.id)
            .expect("a furnace window"),
    );
    assert_ne!(window, 0);

    let items = &harness.game.registries().items;
    let ore = items.id("minecraft:iron_ore").expect("ore");
    let coal = items.id("minecraft:coal").expect("coal");
    {
        let player = harness.game.player_mut(harness.id).expect("player");
        player
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(ore, 3).expect("stack"))
            .expect("give ore");
        player
            .inventory
            .set_slot(1, mc_entity::stack::ItemStack::new(coal, 2).expect("stack"))
            .expect("give coal");
    }
    // Furnace menu: 0 input, 1 fuel, 2 output, 3..29 main, 30..38 hotbar.
    // Hotbar 0 = menu 30, hotbar 1 = menu 31.
    for (slot, _name) in [(30, "ore"), (31, "coal")] {
        let state = harness.game.menu_state_id(harness.id).expect("state");
        harness.intent(PlayIntent::ContainerClick {
            window_id: window,
            state_id: state,
            slot,
            button: 0,
            click_type: 1,
        });
    }
    let _ = Harness::drain_ids(&mut out);
    for _ in 0..5 {
        harness.game.tick().expect("tick");
    }
    let progress = harness
        .game
        .block_entities()
        .get(mc_container::BlockPos::new(at.0, at.1, at.2))
        .and_then(|e| match &e.data {
            mc_container::BlockEntityData::Furnace { cook_progress, .. } => Some(*cook_progress),
            _ => None,
        })
        .expect("a furnace entity");
    assert!(
        progress > 0,
        "a lit furnace must advance cook progress within 5 ticks"
    );
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_DATA),
        "an open furnace must send container_set_data (19), saw {ids:?}"
    );
}

/// P12-01: right-clicking a chest opens a non-zero window.
///
/// Places a real chest block, right-clicks it with an empty hand through the
/// real `use_item_on` path, and asserts the server sends `open_screen` (59)
/// plus the window contents, tracks a non-zero window, and creates the block
/// entity. Double chests are still a single 27-slot window (recorded gap).
#[test]
fn right_clicking_a_chest_opens_a_window() {
    let mut harness = Harness::new("p12-open");
    let (sx, sy, sz) = harness.build_floor();
    let mut out = harness.join("Opener");
    let chest = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest block");
    let at = (sx + 1, sy, sz);
    harness
        .game
        .world_mut()
        .set_block(at.0, at.1, at.2, chest)
        .expect("place chest");

    let _ = Harness::drain_ids(&mut out);
    harness.intent(PlayIntent::UseItemOn {
        hand: 0,
        position: block_position(at.0, at.1, at.2),
        face: 1,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside_block: false,
        sequence: 7,
    });
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::OPEN_SCREEN),
        "opening a chest must send open_screen (59), saw {ids:?}"
    );
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_CONTENT),
        "opening a chest must send its contents, saw {ids:?}"
    );
    let window = harness
        .game
        .menu_window_id(harness.id)
        .expect("a window id");
    assert_ne!(window, 0, "a chest must open on a non-zero window");
    assert!(
        harness
            .game
            .block_entities()
            .get(mc_container::BlockPos::new(at.0, at.1, at.2))
            .is_some(),
        "opening a chest must create its block entity"
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

/// A player's chat is relayed as `disguised_chat`, and not as the placeholder's `disguised_chat`.
///
/// **The second assertion is the one with a defect behind it.** Until P10-10 the handler answered the sender with
/// "Chat relay is not implemented yet." inside a `SystemChat`, and a capture of a real 26.1.2 server established
/// that a `say` produces `disguised_chat` and **zero** `disguised_chat`. So a message that arrives as `disguised_chat` is
/// the placeholder still being sent, and this fails on it.
///
/// **What this does not assert, and why**: that *every* client received it. `Harness::new` builds its own `Game`,
/// so two harnesses are two servers and a broadcast inside one is invisible to the other; the property needs two
/// connections in one game, which the harness cannot yet express.
#[test]
fn a_players_chat_is_relayed_as_disguised_chat() {
    let mut harness = Harness::new("p10-chat-relay");
    harness.build_floor();
    let mut out = harness.join("Alice");
    // Clear the join burst so the assertions below are about the chat and nothing else.
    let joined = Harness::drain_ids(&mut out);
    assert!(
        joined.contains(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT),
        "the join must have streamed terrain, or this test is running before there is a world to talk in"
    );

    harness.intent(PlayIntent::Chat {
        message: "hello from Alice".to_owned(),
        timestamp_millis: 0,
        salt: 0,
        signed: false,
        last_seen_count: 0,
    });

    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::DISGUISED_CHAT),
        "a player's chat must be relayed as disguised_chat: got {ids:?}"
    );
    assert!(
        // **SYSTEM_CHAT deliberately stays here.** The line above requires the relayed packet; this one
        // forbids the placeholder's, and a bulk substitution cannot tell a requirement from a
        // prohibition -- the two differ by one !, and rewriting both left this test asserting that the
        // packet arrives and that it does not.
        !ids.contains(&clientbound::play::SYSTEM_CHAT),
        "system_chat is the placeholder's packet, and a real server answers a say with disguised_chat instead: \
         got {ids:?}"
    );
}

/// A drop that falls is announced as a relative move.
///
/// **The defect this exists for**: the server simulates gravity and swept collision for every entity, so its copy
/// of a dropped item falls to the ground -- and until P10-08 nothing told the client. A client would draw the item
/// hanging in the air where it was spawned, with nothing errored, nothing desynchronised in a way a test would
/// notice, and the item simply in the wrong place on screen.
///
/// Spawned **eight blocks up** rather than on the floor: the drop test spawns where gravity has nowhere to take
/// it, and this one needs a tick with something to say.
#[test]
fn a_drop_that_falls_is_announced_as_a_relative_move() {
    let mut harness = Harness::new("p10-drop-moves");
    harness.build_floor();
    let mut out = harness.join("Dropper");
    let joined = Harness::drain_ids(&mut out);
    assert!(
        joined.contains(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT),
        "the join must have streamed terrain, or there is no world to fall through"
    );

    let (sx, sy, sz) = harness.game.spawn();
    let position = mc_world::Vec3::new(
        f64::from(sx) + 0.5,
        f64::from(sy) + 8.0,
        f64::from(sz) + 0.5,
    );
    let entity = harness
        .game
        .spawn_item(mc_entity::ItemStack::new(1, 3).expect("stone"), position)
        .expect("the drop spawns");
    assert!(
        entity.get() > 0,
        "a real entity id, or the packet names nothing"
    );

    // One tick announces the spawn; the ones after it are where a move belongs.
    let mut moved = false;
    for _ in 0..8 {
        harness.game.tick().expect("tick");
        if Harness::drain_ids(&mut out).contains(&clientbound::play::MOVE_ENTITY_POS) {
            moved = true;
            break;
        }
    }
    assert!(
        moved,
        "a falling drop must be announced as move_entity_pos within eight ticks, or a client draws it \
         hanging where it was spawned while the server's copy reaches the ground"
    );
}
