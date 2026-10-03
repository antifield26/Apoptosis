//! Fluid simulation wiring (P20-01).
//!
//! `mc-simulation` proves the flow rules against a bare map
//! (`fluid::tests`, `fluid::queue::tests`). This file proves the
//! **integration**: the game owns a second scheduled-tick queue for fluids, the
//! `FluidTicks` phase drains it under the ADR-0009 cap, the flow rules write
//! through [`mc_world::World`] so the Broadcast phase can send the changes, and
//! the `RandomTicks` phase reports its sample count (since P20-02 it also
//! applies growth through `game::growth`; the no-player stone-floor pin below
//! still applies nothing, which is what makes it a control).
//!
//! These are this task's own tests and deliberately **not** the frozen
//! differential scenarios (`spring_flow`, `falling_column`, `lava_meets_water`,
//! `waterlogged_stairs`, `bucket_place`) — those compare cell-by-cell against a
//! vanilla server and are a separate deliverable.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_simulation::MAX_FLUID_TICKS_PER_TICK;
use mc_test_support::fixtures::TempDir;

/// A game with one connected player, for the paths that arrive as an intent
/// (the bucket, P20-01). Same shape as `mechanisms.rs`'s harness.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(64);
        let game =
            Game::with_seed_and_storage(storage, 2, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 4096);
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
        out
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

    /// Stand the player in the block at `(x, y, z)`, so reach checks pass.
    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }

    /// Put one `item` in the held slot (hotbar slot 0).
    fn give(&mut self, item: &str) {
        let id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(id, 1).expect("stack"))
            .expect("slot 0 takes it");
    }

    fn held_name(&mut self) -> Option<String> {
        let held = self
            .game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .selected_item()
            .item_id()?;
        self.game
            .registries()
            .items
            .name(held)
            .ok()
            .map(str::to_owned)
    }

    fn click(&mut self, x: i32, y: i32, z: i32, face: i32) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }
}

fn game(tag: &str) -> (Game, WorldService, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = game_channel(64);
    let game = Game::new(&storage, 3, rx).expect("game builds");
    (game, storage, dir)
}

/// A stone floor at `y = 63` under the square `-2..=2`, so water has something
/// to sit on. Written straight into the world: terrain is not what these tests
/// are about, and the fluid feed is exercised by the schedule calls instead.
fn stone_floor(game: &mut Game) {
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone is registered");
    for x in -2..=2 {
        for z in -2..=2 {
            game.world_mut()
                .set_block(x, 63, z, stone)
                .expect("floor writes");
        }
    }
}

/// The registry name of the block at `(x, y, z)`.
fn block_name(game: &Game, x: i32, y: i32, z: i32) -> String {
    let id = game.world().get_block_loaded(x, y, z).expect("loaded");
    game.registries()
        .blocks
        .block_name(id)
        .expect("registered")
        .to_owned()
}

/// The `level` property of the block at `(x, y, z)`, if it has one.
fn block_level(game: &Game, x: i32, y: i32, z: i32) -> Option<u8> {
    let id = game.world().get_block_loaded(x, y, z)?;
    game.registries()
        .blocks
        .properties_of(id)
        .ok()?
        .iter()
        .find(|(key, _)| key == "level")
        .and_then(|(_, value)| value.parse::<u8>().ok())
}

/// Write a water source at `(x, y, z)` without the block feed, then schedule it
/// by hand — the same public call the feed uses.
/// Write a water source at `(x, y, z)` and feed the change the way a player
/// placement does, so the delay comes from the fluid's own rules rather than
/// from a number this test chose.
fn place_water_source(game: &mut Game, x: i32, y: i32, z: i32) {
    let water = game
        .registries()
        .blocks
        .state_id("minecraft:water", &[("level".to_owned(), "0".to_owned())])
        .expect("water level=0 resolves");
    game.world_mut()
        .set_block(x, y, z, water)
        .expect("water writes");
    game.feed_block_change(x, y, z);
}

#[test]
fn a_water_source_spreads_after_its_five_tick_delay_and_not_before() {
    // WaterFluid.getTickDelay is the constant 5 (javap; quoted in
    // `mc_simulation::fluid::state`). The integration claim: the game's fluid
    // queue really holds the work for five ticks, and the phase really runs it.
    let (mut game, _storage, _dir) = game("p20-fluid-delay");
    stone_floor(&mut game);
    place_water_source(&mut game, 0, 64, 0);

    // The source was scheduled for tick 5; ticks 1..=4 must not run it.
    for _ in 0..4 {
        let report = game.tick().expect("tick");
        assert_eq!(report.fluid_ticks_fired, 0, "not due yet");
        assert_eq!(report.fluid_ticks_pending, 1, "but still owed");
    }
    assert_eq!(
        block_name(&game, 1, 64, 0),
        "minecraft:air",
        "water has not spread yet"
    );

    // Tick 5 runs the source: four neighbours, each written and scheduled.
    let report = game.tick().expect("tick");
    assert_eq!(report.fluid_ticks_fired, 1);
    assert_eq!(
        report.fluid_blocks_written, 4,
        "one write per horizontal face"
    );
    for (x, z) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        assert_eq!(block_name(&game, x, 64, z), "minecraft:water");
        assert_eq!(
            block_level(&game, x, 64, z),
            Some(1),
            "a source spreads at level 1 (amount 7)"
        );
    }
    assert_eq!(
        report.fluid_ticks_pending, 4,
        "the written cells are queued"
    );
}

#[test]
fn the_phase_cap_spills_fluid_work_instead_of_dropping_it() {
    // ADR-0009 §2.3: due fluid updates beyond MAX_FLUID_TICKS_PER_TICK stay
    // queued. Fired + pending must equal what was scheduled, on every tick.
    let (mut game, _storage, _dir) = game("p20-fluid-cap");
    stone_floor(&mut game);
    let scheduled = MAX_FLUID_TICKS_PER_TICK + 128;
    for x in 0..i32::try_from(scheduled).expect("fits") {
        game.schedule_fluid_tick(x, 64, 0, 0);
    }
    let report = game.tick().expect("tick");
    assert_eq!(
        report.fluid_ticks_fired, MAX_FLUID_TICKS_PER_TICK,
        "the cap bounds one tick"
    );
    assert_eq!(
        report.fluid_ticks_fired + report.fluid_ticks_pending,
        scheduled,
        "whatever did not run is still queued, not lost"
    );
    // And the spilled work runs on the next tick, oldest first.
    let next = game.tick().expect("tick");
    assert_eq!(next.fluid_ticks_fired, 128);
    assert_eq!(next.fluid_ticks_pending, 0);
}

#[test]
fn a_water_source_flows_downward_into_a_falling_column() {
    // `spread` tries DOWN first, so a source over a drop makes a falling column
    // (block level 8) instead of a puddle. Through the phase, not the unit API.
    let (mut game, _storage, _dir) = game("p20-fluid-fall");
    stone_floor(&mut game);
    // The source sits one cell above the floor, so the cell below it is air and
    // the down-spread has somewhere to go.
    place_water_source(&mut game, 0, 65, 0);
    for _ in 0..5 {
        game.tick().expect("tick");
    }
    // The source ticked; the cell below was empty and is now falling water.
    let below = game.world().get_block_loaded(0, 64, 0).expect("loaded");
    let below_name = game
        .registries()
        .blocks
        .block_name(below)
        .expect("registered")
        .to_owned();
    assert_eq!(below_name, "minecraft:water");
    assert_eq!(
        block_level(&game, 0, 64, 0),
        Some(8),
        "the falling form sits at level 8"
    );
}

#[test]
fn water_meeting_lava_in_the_world_leaves_obsidian() {
    // `LiquidBlock.shouldSpreadLiquid`: a lava *source* with water above it turns
    // to obsidian. The water is placed above and both cells are scheduled; the
    // lava's own tick converts it.
    let (mut game, _storage, _dir) = game("p20-fluid-lava");
    stone_floor(&mut game);
    let lava = game
        .registries()
        .blocks
        .state_id("minecraft:lava", &[("level".to_owned(), "0".to_owned())])
        .expect("lava level=0 resolves");
    game.world_mut()
        .set_block(0, 64, 0, lava)
        .expect("lava writes");
    game.schedule_fluid_tick(0, 64, 0, 0);
    place_water_source(&mut game, 0, 65, 0);
    game.schedule_fluid_tick(0, 65, 0, 0);

    for _ in 0..6 {
        game.tick().expect("tick");
    }
    assert_eq!(
        block_name(&game, 0, 64, 0),
        "minecraft:obsidian",
        "the lava source became obsidian"
    );
}

#[test]
fn water_fills_a_waterloggable_block_instead_of_replacing_it() {
    // `FlowingFluid.spreadTo` calls `LiquidBlockContainer.placeLiquid`, which for
    // a `SimpleWaterloggedBlock` sets `waterlogged=true` and keeps the block.
    let (mut game, _storage, _dir) = game("p20-fluid-waterlogged");
    stone_floor(&mut game);
    let stairs = game
        .registries()
        .blocks
        .default_state("minecraft:oak_stairs")
        .expect("oak_stairs is registered");
    game.world_mut()
        .set_block(1, 64, 0, stairs)
        .expect("stairs write");
    place_water_source(&mut game, 0, 64, 0);
    for _ in 0..6 {
        game.tick().expect("tick");
    }

    let id = game.world().get_block_loaded(1, 64, 0).expect("loaded");
    assert_eq!(
        game.registries().blocks.block_name(id).expect("registered"),
        "minecraft:oak_stairs",
        "the block survives the water"
    );
    let properties = game
        .registries()
        .blocks
        .properties_of(id)
        .expect("properties");
    assert!(
        properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true"),
        "and it is waterlogged: {properties:?}"
    );
}

#[test]
fn the_random_tick_phase_counts_its_samples_and_applies_nothing() {
    // The sweep's shape and its counters (P20-01); the growth handlers are
    // P20-02, and this fixture — no player, stone floor — still applies
    // nothing, which is what makes it the control both phases share. Two
    // claims, both falsifiable: with no player there is no ticking radius,
    // so no samples; and nothing in a run of ticks is ever *applied*.
    let (mut game, _storage, _dir) = game("p20-random-tick");
    stone_floor(&mut game);
    let mut applied = 0usize;
    for _ in 0..3 {
        let report = game.tick().expect("tick");
        assert_eq!(
            report.random_tick_samples, 0,
            "the sweep is centred on players; with none there is nothing to sweep"
        );
        applied += report.random_ticks_applied;
    }
    assert_eq!(
        applied, 0,
        "stone has no random-tick handler (P20-02 growth)"
    );
}

#[test]
fn a_water_bucket_places_a_source_and_becomes_an_empty_bucket() {
    // The bucket path through `UseItemOn`: a filled bucket aimed at the top face
    // of a block places its fluid in the cell above and is exchanged for the
    // empty bucket in the same slot.
    let mut harness = Harness::new("p20-bucket-place");
    harness.join("Pourer");
    stone_floor(&mut harness.game);
    harness.stand(0, 64, 0);
    harness.give("minecraft:water_bucket");
    harness.click(1, 63, 0, 1);

    assert_eq!(
        block_name(&harness.game, 1, 64, 0),
        "minecraft:water",
        "the fluid landed on the clicked face"
    );
    assert_eq!(
        block_level(&harness.game, 1, 64, 0),
        Some(0),
        "and it is a source, not a flowing level"
    );
    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:bucket"),
        "the bucket was emptied into the world"
    );
}

#[test]
fn a_players_water_bucket_converts_the_lava_source_beside_it_in_that_tick() {
    // The player path, not `world_mut().set_block`: the water arrives as an
    // intent, the way a client's `UseItemOn` does. The jar converts the lava
    // *inside* that placement — `Level.setBlock` runs `LiquidBlock.onPlace` on
    // the written cell and `updateNeighborsAt` in `NeighborUpdater.UPDATE_ORDER`
    // (quoted on `mc_simulation::fluid::notify_neighbors`), whose
    // `LiquidBlock.neighborChanged` runs `shouldSpreadLiquid`. So the source is
    // already obsidian when the tick that applied the use ends: **the placement
    // tick, not the lava's own scheduled tick.**
    let mut harness = Harness::new("p20-bucket-lava");
    harness.join("Pourer");
    stone_floor(&mut harness.game);
    // The lava source goes in through the game's own edit path (raw set, then the
    // same feed every editing path makes), so it is *also* owed its own tick
    // thirty ticks out. The assertion below cannot be explained by that tick
    // running early: `fed_at` pins how far off it still is.
    let lava = harness
        .game
        .registries()
        .blocks
        .state_id("minecraft:lava", &[("level".to_owned(), "0".to_owned())])
        .expect("lava level=0 resolves");
    harness
        .game
        .world_mut()
        .set_block(2, 64, 0, lava)
        .expect("lava writes");
    harness.game.feed_block_change(2, 64, 0);
    let fed_at = harness.game.tick_count();

    harness.stand(0, 64, 0);
    harness.give("minecraft:water_bucket");
    // Face 1 (up) of the floor cell under the air cell west of the lava: the
    // bucket empties into that air cell, which is the lava's neighbour.
    harness.click(1, 63, 0, 1);

    assert_eq!(
        block_name(&harness.game, 1, 64, 0),
        "minecraft:water",
        "the bucket emptied beside the lava"
    );
    assert_eq!(
        block_level(&harness.game, 1, 64, 0),
        Some(0),
        "as a source, not a flowing level"
    );
    assert!(
        harness.game.tick_count() - fed_at < 30,
        "the lava's own tick is thirty ticks out; this test must be asserting \
         inside the window where only the write's notification can have converted it"
    );
    assert_eq!(
        block_name(&harness.game, 2, 64, 0),
        "minecraft:obsidian",
        "the lava source converted in the tick the player's bucket was emptied \
         (tick {}); with the feed's notification neutralised it waits for its own \
         tick thirty ticks later and is still lava here",
        harness.game.tick_count()
    );
}

#[test]
fn a_players_lava_bucket_beside_water_converts_itself_in_that_tick() {
    // The mirror half: `LiquidBlock.onPlace` runs `shouldSpreadLiquid` on the
    // block that was just *written*, so a lava source a player pours next to
    // water becomes obsidian itself, in the placement tick, and is not scheduled.
    let mut harness = Harness::new("p20-bucket-lava-self");
    harness.join("Pourer");
    stone_floor(&mut harness.game);
    let water = harness
        .game
        .registries()
        .blocks
        .state_id("minecraft:water", &[("level".to_owned(), "0".to_owned())])
        .expect("water level=0 resolves");
    harness
        .game
        .world_mut()
        .set_block(2, 64, 0, water)
        .expect("water writes");
    harness.game.feed_block_change(2, 64, 0);

    harness.stand(0, 64, 0);
    harness.give("minecraft:lava_bucket");
    // The same click geometry: the lava lands in the air cell beside the water.
    harness.click(1, 63, 0, 1);

    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:bucket"),
        "the lava bucket emptied"
    );
    assert_eq!(
        block_name(&harness.game, 1, 64, 0),
        "minecraft:obsidian",
        "the lava the player poured converted itself in the placement tick"
    );
    assert_eq!(
        block_name(&harness.game, 2, 64, 0),
        "minecraft:water",
        "and the water it met is untouched: the rule converts lava, never water"
    );
}

#[test]
fn an_empty_bucket_takes_a_water_source_and_refuses_flowing_water() {
    // The fill half: `LiquidBlock.pickupBlock` takes a source (`level == 0`) and
    // refuses anything flowing, which is what stops a bucket being filled from a
    // stream rather than from the block the stream came from.
    let mut harness = Harness::new("p20-bucket-fill");
    harness.join("Filler");
    stone_floor(&mut harness.game);
    harness.stand(0, 64, 0);

    // A flowing cell (level 1) is refused outright.
    let flowing = harness
        .game
        .registries()
        .blocks
        .state_id("minecraft:water", &[("level".to_owned(), "1".to_owned())])
        .expect("water level=1 resolves");
    harness
        .game
        .world_mut()
        .set_block(1, 64, 0, flowing)
        .expect("flowing water writes");
    harness.give("minecraft:bucket");
    harness.click(1, 64, 0, 1);
    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:bucket"),
        "a stream cannot fill a bucket"
    );
    assert_eq!(block_name(&harness.game, 1, 64, 0), "minecraft:water");

    // The source is taken, and the cell becomes air.
    place_water_source(&mut harness.game, 1, 64, 0);
    harness.click(1, 64, 0, 1);
    assert_eq!(
        harness.held_name().as_deref(),
        Some("minecraft:water_bucket"),
        "the source fills the bucket"
    );
    assert_eq!(
        block_name(&harness.game, 1, 64, 0),
        "minecraft:air",
        "and is removed from the world"
    );
}

#[test]
fn a_mob_under_water_loses_its_air_and_drowns_after_the_jars_delay() {
    // The air supply and the 20-tick hit cadence: vanilla drains 300 ticks of
    // air (`AIR_TICKS`), counts 20 more below zero, then deals 2.0 and resets.
    // A cow's eye at y = feet + 1.62 is inside the upper water layer.
    let (mut game, _storage, _dir) = game("p20-drown");
    stone_floor(&mut game);
    for y in 64..=65 {
        for x in -1..=1 {
            for z in -1..=1 {
                place_water_source(&mut game, x, y, z);
            }
        }
    }
    let id = game
        .spawn_mob(
            mc_entity::mob::MobKind::Cow,
            mc_world::Vec3::new(0.5, 64.0, 0.5),
        )
        .expect("the cow spawns");
    let full_health = game.entity(id).expect("entity").health;
    assert_eq!(
        game.entity(id).expect("entity").air_ticks,
        mc_entity::AIR_TICKS
    );

    // 299 ticks: the air drains, the cow is untouched.
    for _ in 0..299 {
        game.tick().expect("tick");
    }
    let entity = game.entity(id).expect("entity");
    assert_eq!(entity.air_ticks, 1, "one tick of air left");
    assert!(
        (entity.health - full_health).abs() < f32::EPSILON,
        "drowning has not started: {} vs {full_health}",
        entity.health
    );

    // Air hits zero at 300, and the first hit lands 20 ticks later.
    for _ in 0..21 {
        game.tick().expect("tick");
    }
    let entity = game.entity(id).expect("entity");
    assert!(
        (entity.health - (full_health - mc_entity::DROWN_DAMAGE)).abs() < f32::EPSILON,
        "one drowning hit, not one per tick: {} vs {}",
        entity.health,
        full_health - mc_entity::DROWN_DAMAGE
    );

    // Out of water the supply refills at 4 a tick.
    let air = game.registries().blocks.air_id();
    for y in 64..=65 {
        for x in -1..=1 {
            for z in -1..=1 {
                game.world_mut().set_block(x, y, z, air).expect("drained");
            }
        }
    }
    game.tick().expect("tick");
    let entity = game.entity(id).expect("entity");
    assert_eq!(
        entity.air_ticks, 4,
        "out of water the supply refills 4 a tick from zero"
    );
    // And it fills back up to the cap rather than growing without bound.
    for _ in 0..80 {
        game.tick().expect("tick");
    }
    assert_eq!(
        game.entity(id).expect("entity").air_ticks,
        mc_entity::AIR_TICKS,
        "the supply is capped at 15 seconds"
    );
}

#[test]
fn a_fluid_tick_for_a_position_that_no_longer_holds_fluid_is_a_no_op() {
    // The jar hands `FlowingFluid.tick` the *current* state, so a queued tick for
    // a block a player has mined does nothing. Ours re-reads for the same reason:
    // a stale entry must not resurrect water.
    let (mut game, _storage, _dir) = game("p20-fluid-stale");
    stone_floor(&mut game);
    place_water_source(&mut game, 0, 64, 0);
    let air = game.registries().blocks.air_id();
    game.world_mut().set_block(0, 64, 0, air).expect("mined");
    // Let the queued tick come due; the water is gone by then.
    let mut fired = 0usize;
    let mut written = 0usize;
    for _ in 0..5 {
        let report = game.tick().expect("tick");
        fired += report.fluid_ticks_fired;
        written += report.fluid_blocks_written;
    }
    assert_eq!(fired, 1, "the queued tick still came due");
    assert_eq!(written, 0, "and did nothing");
    assert_eq!(
        block_name(&game, 1, 64, 0),
        "minecraft:air",
        "no water was resurrected"
    );
}
