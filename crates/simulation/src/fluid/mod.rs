//! Fluids: the jar's flow rules and the queue that feeds them (P20-01).
//!
//! ## Where this lives, and why
//!
//! Two questions were open when this module was written, and both are answered
//! here rather than deferred:
//!
//! 1. **The queue is `mc-simulation`'s, not `mc-redstone`'s.** ADR-0009 §1 makes
//!    fluids a **second** `LevelTicks` object beside the block queue, and
//!    [`queue::FluidQueue`] is that object. It sits next to
//!    [`flow`] because a queue belongs with the consumer it exists for, and
//!    `mc-simulation` is the crate that owns a tick's phases — the phase
//!    [`crate::TickPhase::FluidTicks`] drains it. `mc-redstone::UpdateQueue` is
//!    the **precedent for the discipline** (caps, dedup, a budget, a total
//!    order) and [`queue::FluidQueue`] copies that shape deliberately, but the
//!    two queues are separate objects with separate caps, exactly as the jar's
//!    are.
//! 2. **The world is reached through a trait, not through `mc-world`.** Keeping
//!    `mc-simulation`'s dependency list at `mc-core` alone is ADR-0009 §5's "no
//!    new dependency", and it makes the rules testable against a
//!    `BTreeMap`-backed world (see this module's tests) instead of a chunk store.
//!    [`world::FluidWorld`] documents what the server has to answer and which
//!    parts of the jar's answer are approximations here.
//!
//! ## What is implemented, and what is not
//!
//! Implemented, each with the jar method it ports named on the function:
//! water and lava levels 0..15 with the falling encoding, source creation
//! (game-rule gated), the spread delays (water 5; lava 10/30 by the per-dimension
//! `FAST_LAVA` attribute, ×4 when lava climbs), the slope search, the
//! lava/water conversions (stone, cobblestone, obsidian), and `waterlogged`
//! placement into waterloggable blocks.
//!
//! ## The write-notification half is public, and shared
//!
//! `Level.setBlock(pos, state, 3)` does two things besides the chunk write:
//! `BlockState.onPlace` on the written cell and `updateNeighborsAt` over the six
//! faces in `NeighborUpdater.UPDATE_ORDER`. Both are rules of this module
//! ([`on_place`] and [`notify_neighbors`]), and both are required for **any**
//! write, not only an engine write: a player's bucket next to a lava source must
//! convert it in the tick it lands, exactly as the jar does. A cell written
//! outside the engine (bucket, command, structure) is therefore taken through
//! these two entry points by the writer's own adapter —
//! `crates/server`'s block-edit feed — rather than through a second copy of the
//! order or of `shouldSpreadLiquid`.
//!
//! Not implemented, named rather than hidden:
//!
//! - **Item drops from `beforeDestroyingBlock`.** A plant the water reaches is
//!   replaced without dropping its item; no fluid-driven loot path exists yet.
//! - **`fizz` sound and particles** (client-visible, no simulation effect).
//! - **The Nether-only `SOUL_SOIL`+`BLUE_ICE` → basalt arm** of
//!   `LiquidBlock.shouldSpreadLiquid`; one dimension until P21.
//! - **`bubble_column`**, which is a separate block behaviour, not a flow rule.
//! - **Fluid ticks are not persisted.** `mc-world`'s chunk writer emits an empty
//!   `fluid_ticks` list today (`chunk.rs::to_chunk_data`), so a restart loses
//!   queued spread work; water already placed stays where it is.

pub mod flow;
pub mod queue;
pub mod state;
pub mod world;

pub use flow::{
    FluidTickOutcome, get_new_liquid, get_spread, get_spread_delay, notify_neighbors, on_place,
    tick,
};
pub use queue::{
    FluidBudget, FluidDrain, FluidQueue, FluidScheduled, MAX_FLUID_QUEUE_ENTRIES,
    MAX_FLUID_SCHEDULE_DELAY, MAX_FLUID_TICKS_PER_TICK,
};
pub use state::{FluidBody, FluidKind, FluidRules, FluidState};
pub use world::{BlockFacts, Dir, FluidWorld, Pos};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mc_core::random::RandomSource;
    use mc_core::tick::Tick;

    use super::flow::{COBBLESTONE, FluidTickOutcome, OBSIDIAN, STONE};
    use super::queue::{FluidBudget, FluidQueue};
    use super::state::{FluidKind, FluidRules, FluidState};
    use super::world::{BlockFacts, FluidWorld, Pos};

    /// A world of explicitly-listed blocks; anything unlisted is air.
    ///
    /// Deliberately tiny: the flow rules are a pure function of the blocks they
    /// read, so a `BTreeMap` is enough to pin them and a chunk store would only
    /// add ways for a test to fail for the wrong reason.
    #[derive(Debug, Default, Clone)]
    struct TestWorld {
        blocks: BTreeMap<Pos, BlockFacts>,
        /// Blocks written by `set_block_named`, so a test can assert a conversion.
        named: BTreeMap<Pos, String>,
    }

    impl TestWorld {
        fn new() -> Self {
            Self::default()
        }

        fn set(&mut self, pos: Pos, facts: BlockFacts) -> &mut Self {
            self.blocks.insert(pos, facts);
            self
        }

        fn water(&mut self, x: i32, y: i32, z: i32, level: u8) -> &mut Self {
            self.set(Pos::new(x, y, z), Self::water_facts(level))
        }

        fn lava(&mut self, x: i32, y: i32, z: i32, level: u8) -> &mut Self {
            self.set(Pos::new(x, y, z), Self::lava_facts(level))
        }

        /// A dry waterloggable block (stairs-like: not a full cube).
        fn container(&mut self, x: i32, y: i32, z: i32, waterlogged: bool) -> &mut Self {
            self.set(
                Pos::new(x, y, z),
                BlockFacts::container(900, waterlogged, false),
            )
        }

        fn stone(&mut self, x: i32, y: i32, z: i32) -> &mut Self {
            self.set(Pos::new(x, y, z), BlockFacts::solid(1))
        }

        /// A wall of stone under a square region, so water has a floor.
        fn floor(&mut self, y: i32, from: i32, to: i32) -> &mut Self {
            for x in from..=to {
                for z in from..=to {
                    self.stone(x, y, z);
                }
            }
            self
        }

        fn fluid_at(&self, x: i32, y: i32, z: i32) -> FluidState {
            self.facts(Pos::new(x, y, z)).expect("loaded").fluid
        }

        fn water_facts(level: u8) -> BlockFacts {
            BlockFacts::of_fluid(86 + i32::from(level), FluidState::water(level))
        }

        fn lava_facts(level: u8) -> BlockFacts {
            BlockFacts::of_fluid(102 + i32::from(level), FluidState::lava(level))
        }
    }

    impl FluidWorld for TestWorld {
        fn facts(&self, pos: Pos) -> Option<BlockFacts> {
            // Everything is "loaded" in the test world.
            Some(self.blocks.get(&pos).copied().unwrap_or(BlockFacts::air(0)))
        }

        fn place_fluid(&mut self, pos: Pos, fluid: FluidState) -> bool {
            let before = self.facts(pos).expect("loaded");
            if fluid.is_empty() {
                self.blocks.insert(pos, BlockFacts::air(0));
                return !before.fluid.is_empty();
            }
            // The waterlogging rule the server adapter implements: water into a
            // waterloggable block sets its property and keeps the block.
            if before.water_container && fluid.kind() == Some(FluidKind::Water) {
                self.blocks.insert(
                    pos,
                    BlockFacts::container(before.id, true, before.full_cube),
                );
                return !before.waterlogged;
            }
            let id = match fluid.kind() {
                Some(FluidKind::Water) => 86 + i32::from(fluid.block_level()),
                Some(FluidKind::Lava) => 102 + i32::from(fluid.block_level()),
                None => 0,
            };
            self.blocks.insert(pos, BlockFacts::of_fluid(id, fluid));
            before.fluid != fluid
        }

        fn set_block_named(&mut self, pos: Pos, name: &str) -> bool {
            if !matches!(name, STONE | COBBLESTONE | OBSIDIAN) {
                return false;
            }
            let changed = self.facts(pos).expect("loaded").fluid.kind().is_some();
            self.blocks.insert(pos, BlockFacts::solid(1));
            self.named.insert(pos, name.to_owned());
            changed
        }
    }

    fn pos(x: i32, y: i32, z: i32) -> Pos {
        Pos::new(x, y, z)
    }

    fn water_rules() -> FluidRules {
        FluidRules::water(true)
    }

    /// Tick one position, with a fresh queue and a fixed seed.
    fn tick_once(
        world: &mut TestWorld,
        rules: FluidRules,
        at: Pos,
        now: Tick,
        queue: &mut FluidQueue,
    ) -> FluidTickOutcome {
        let mut random = RandomSource::new(0x5EED_0001);
        super::flow::tick(world, rules, at, now, queue, &mut random)
    }

    /// Water written beside a lava source converts it **in that tick**.
    ///
    /// The jar's write path is `Level.setBlock(pos, state, 3)` →
    /// `updateNeighborsAt` → `LiquidBlock.neighborChanged` →
    /// `shouldSpreadLiquid` (`javap`; quoted on `flow::notify_neighbors`), so the
    /// tick water arrives next to a lava cell the lava is already
    /// `minecraft:obsidian` — it does **not** wait for the lava's own fluid tick,
    /// which for overworld lava is thirty ticks later and by then is too late
    /// (the water has overwritten the cell and flowed on). This is the
    /// `lava_meets_water` divergence measured against the real 26.1.2 server:
    /// `(8, 102, 6)` is obsidian there at tick 20, not water at tick 40.
    #[test]
    fn water_written_beside_a_lava_source_converts_it_in_the_same_tick() {
        let mut world = TestWorld::new();
        // A floor wide enough that no direction of the source's slope search can
        // see a hole: all four faces then tie, so the east face — the one the
        // lava is behind — is written in this tick.
        world.floor(63, -8, 8);
        world.water(-2, 64, 0, 0);
        world.lava(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        // One water tick, at tick 0: the source spreads east into (-1, 64, 0),
        // and that write is what notifies the lava.
        let outcome = tick_once(&mut world, water_rules(), pos(-2, 64, 0), 0, &mut queue);

        assert_eq!(
            world.fluid_at(-1, 64, 0),
            FluidState::water(1),
            "the water reached the cell beside the lava"
        );
        assert!(
            outcome.converted,
            "the write converted the neighbouring lava in this tick"
        );
        assert_eq!(
            world.named.get(&pos(0, 64, 0)).map(String::as_str),
            Some(OBSIDIAN),
            "a lava *source* beside water becomes obsidian, not cobblestone"
        );
        assert_eq!(world.fluid_at(0, 64, 0), FluidState::Empty);
        // And it is not queued: the jar's `onPlace` never schedules a `LiquidBlock`
        // that `shouldSpreadLiquid` converted away, and the lava's own tick (thirty
        // ticks out) is what the port must *not* be waiting for.
        let due = queue.drain_due(1_000, FluidBudget::nominal()).due;
        assert!(
            !due.contains(&pos(0, 64, 0)),
            "the converted cell must not still be owed a lava tick: {due:?}"
        );
    }

    /// Lava written beside water converts **itself**, in the same tick.
    ///
    /// `LiquidBlock.onPlace` runs `shouldSpreadLiquid` on the block that was just
    /// written: a *flowing* lava cell beside water becomes `minecraft:cobblestone`
    /// before its own tick could ever run, and is not scheduled. The lava here
    /// also spreads north, south and west, and those cells stay lava — the rule
    /// converts only the cell that has water beside it.
    #[test]
    fn lava_written_beside_water_becomes_cobblestone_in_the_same_tick() {
        let mut world = TestWorld::new();
        world.floor(63, -8, 8);
        world.lava(0, 64, 0, 0);
        world.water(2, 64, 0, 0);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(
            &mut world,
            FluidRules::lava(false, false),
            pos(0, 64, 0),
            0,
            &mut queue,
        );

        assert!(outcome.converted);
        assert_eq!(
            world.named.get(&pos(1, 64, 0)).map(String::as_str),
            Some(COBBLESTONE),
            "flowing lava beside water becomes cobblestone, not obsidian"
        );
        assert_eq!(world.fluid_at(1, 64, 0), FluidState::Empty);
        for (dx, dz) in [(0, -1), (0, 1), (-1, 0)] {
            assert_eq!(
                world.fluid_at(dx, 64, dz),
                FluidState::lava(2),
                "lava spreading away from the water keeps the drop-off level and stays lava"
            );
        }
        let due = queue.drain_due(1_000, FluidBudget::nominal()).due;
        assert!(
            !due.contains(&pos(1, 64, 0)),
            "the converted cell is not owed a fluid tick: {due:?}"
        );
    }

    #[test]
    fn a_water_source_spreads_one_cell_sideways_and_schedules_it_five_ticks_later() {
        // `WaterFluid.getTickDelay` is the constant 5 (javap, quoted in `state`),
        // so the four neighbours are queued for tick now+5 — not now+1 and not now+4.
        let mut world = TestWorld::new();
        world.floor(63, -2, 2);
        world.water(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(&mut world, water_rules(), pos(0, 64, 0), 100, &mut queue);

        assert!(outcome.did_work && !outcome.converted);
        assert_eq!(outcome.spread_writes, 4, "one write per horizontal face");
        for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
            assert_eq!(
                world.fluid_at(dx, 64, dz),
                FluidState::water(1),
                "the neighbour at ({dx}, {dz}) received amount 7, i.e. block level 1"
            );
        }
        assert_eq!(queue.pending(), 4, "each written cell is scheduled");
        assert_eq!(
            queue.scheduled_at(105).map(std::collections::BTreeSet::len),
            Some(4)
        );
        assert!(queue.drain_due(104, FluidBudget::nominal()).is_empty());
        assert_eq!(queue.drain_due(105, FluidBudget::nominal()).len(), 4);
    }

    #[test]
    fn water_falls_rather_than_spreading_sideways() {
        // `spread` tries DOWN first and only runs `spreadToSides` when the cell
        // below is not a hole it can fill (javap offsets 30-130). A source with
        // air below therefore makes a falling column, not a puddle.
        let mut world = TestWorld::new();
        world.floor(60, -2, 2);
        world.water(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);

        assert_eq!(
            world.fluid_at(0, 63, 0),
            FluidState::water(8),
            "the cell below gets the falling form (block level 8)"
        );
        assert!(world.fluid_at(0, 63, 0).is_falling());
        assert_eq!(
            world.fluid_at(1, 64, 0),
            FluidState::Empty,
            "and no sideways spread happens while the fluid can fall"
        );
        assert_eq!(queue.pending(), 1);
    }

    #[test]
    fn water_above_lava_leaves_obsidian_and_flowing_lava_leaves_cobblestone() {
        // `LiquidBlock.shouldSpreadLiquid`: a lava cell with water above or
        // beside it becomes OBSIDIAN when it is a source and COBBLESTONE when it
        // is flowing.
        let mut source_world = TestWorld::new();
        source_world.floor(63, -2, 2);
        source_world.lava(0, 64, 0, 0);
        source_world.water(0, 65, 0, 0);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(
            &mut source_world,
            FluidRules::lava(false, false),
            pos(0, 64, 0),
            0,
            &mut queue,
        );
        assert!(outcome.converted);
        assert_eq!(
            source_world.named.get(&pos(0, 64, 0)).map(String::as_str),
            Some(OBSIDIAN)
        );
        assert_eq!(source_world.fluid_at(0, 64, 0), FluidState::Empty);

        let mut flowing_world = TestWorld::new();
        flowing_world.floor(63, -2, 2);
        flowing_world.lava(0, 64, 0, 1);
        flowing_world.water(0, 65, 0, 0);
        let mut queue = FluidQueue::new();
        tick_once(
            &mut flowing_world,
            FluidRules::lava(false, false),
            pos(0, 64, 0),
            0,
            &mut queue,
        );
        assert_eq!(
            flowing_world.named.get(&pos(0, 64, 0)).map(String::as_str),
            Some(COBBLESTONE)
        );

        // The rule is one-way: water next to lava does *not* convert the water.
        let mut water_world = TestWorld::new();
        water_world.floor(63, -2, 2);
        water_world.water(0, 64, 0, 0);
        water_world.lava(1, 64, 0, 0);
        let mut queue = FluidQueue::new();
        tick_once(
            &mut water_world,
            water_rules(),
            pos(0, 64, 0),
            0,
            &mut queue,
        );
        assert!(water_world.named.is_empty(), "water never converts itself");
    }

    #[test]
    fn lava_flowing_down_into_water_turns_that_water_to_stone() {
        // `LavaFluid.spreadTo`'s DOWN override: lava spreading down into a water
        // block replaces the *water* with STONE (and does not place the lava).
        let mut world = TestWorld::new();
        world.floor(60, -2, 2);
        world.lava(0, 64, 0, 0);
        world.water(0, 63, 0, 0);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(
            &mut world,
            FluidRules::lava(false, false),
            pos(0, 64, 0),
            0,
            &mut queue,
        );

        assert!(outcome.converted);
        assert_eq!(
            world.named.get(&pos(0, 63, 0)).map(String::as_str),
            Some(STONE)
        );
        assert_eq!(
            world.fluid_at(0, 64, 0),
            FluidState::lava(0),
            "the lava cell is untouched by this rule and keeps flowing"
        );
    }

    #[test]
    fn water_into_a_waterloggable_block_waterlogs_it_instead_of_replacing_it() {
        // `FlowingFluid.spreadTo` calls `LiquidBlockContainer.placeLiquid`, and
        // `canHoldSpecificFluid` refuses lava for a water container. The block
        // must survive: a waterlogged stairs is still a stairs.
        let mut world = TestWorld::new();
        world.floor(63, -2, 2);
        world.water(0, 64, 0, 0);
        world.container(1, 64, 0, false);
        let mut queue = FluidQueue::new();
        tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);

        let stairs = world.facts(pos(1, 64, 0)).expect("loaded");
        assert!(stairs.waterlogged, "the block became waterlogged");
        assert_eq!(stairs.id, 900, "and kept its own state id");
        assert!(stairs.fluid.is_source(), "carrying a water source");

        // A waterlogged block is not a container that can take a second fluid.
        assert!(!stairs.can_hold_specific_fluid(FluidKind::Water));
        assert!(!stairs.can_hold_specific_fluid(FluidKind::Lava));
    }

    #[test]
    fn two_neighbouring_sources_make_a_new_source_when_the_rule_allows_it() {
        // `getNewLiquid`: `sourceNeighborCount >= 2 && canConvertToSource`, with a
        // solid block (or a source of the same fluid) below.
        let mut world = TestWorld::new();
        world.floor(63, -2, 2);
        world.water(-1, 64, 0, 0);
        world.water(1, 64, 0, 0);
        world.water(0, 64, 0, 1);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);
        assert!(outcome.level_changed);
        assert_eq!(
            world.fluid_at(0, 64, 0),
            FluidState::water(0),
            "the middle cell became a source"
        );

        // With `water_source_conversion` off, the same cell keeps its level.
        let mut gated = TestWorld::new();
        gated.floor(63, -2, 2);
        gated.water(-1, 64, 0, 0);
        gated.water(1, 64, 0, 0);
        gated.water(0, 64, 0, 1);
        let mut queue = FluidQueue::new();
        let rules = FluidRules::water(false);
        tick_once(&mut gated, rules, pos(0, 64, 0), 0, &mut queue);
        assert_eq!(
            gated.fluid_at(0, 64, 0),
            FluidState::water(1),
            "the game rule is the only thing that changed"
        );
    }

    #[test]
    fn a_flowing_cell_with_no_source_left_runs_out() {
        // `getNewLiquid` ends at `maxAmount - getDropOff <= 0` → EMPTY, and `tick`
        // clears the cell (javap offsets 255-276).
        let mut world = TestWorld::new();
        for x in -1..=1 {
            for z in -1..=1 {
                world.stone(x, 63, z);
            }
        }
        world.water(0, 64, 0, 1);
        let mut queue = FluidQueue::new();
        let outcome = tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);
        assert!(outcome.removed);
        assert_eq!(world.fluid_at(0, 64, 0), FluidState::Empty);
    }

    #[test]
    fn water_flows_towards_the_nearest_hole_and_ignores_the_far_side() {
        // `getSpread` keeps only the directions at the **minimum** slope distance
        // (`if (dist < best) map.clear()`, javap offsets 171-185). A corridor with
        // a hole two cells north must therefore send water north only, even
        // though the south side is equally open.
        let mut world = TestWorld::new();
        for z in -5..=5 {
            world.stone(0, 63, z);
        }
        for z in -5..=5 {
            world.stone(-1, 64, z);
            world.stone(1, 64, z);
        }
        for z in [-5, 6] {
            for x in -1..=1 {
                world.stone(x, 64, z);
            }
        }
        // The hole, two cells north of the source.
        world.set(pos(0, 63, -2), BlockFacts::air(0));
        world.water(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);

        assert_eq!(
            world.fluid_at(0, 64, -1),
            FluidState::water(1),
            "the side that leads to the hole flows"
        );
        assert_eq!(
            world.fluid_at(0, 64, 1),
            FluidState::Empty,
            "the side at a greater slope distance does not, this tick"
        );
        assert_eq!(queue.pending(), 1, "exactly one cell was scheduled");
    }

    #[test]
    fn the_slope_search_stops_at_the_jars_slope_find_distance() {
        // `getSlopeDistance` recurses only while `distance < getSlopeFindDistance`
        // (javap offsets 104-110); water's is 4, and the search starts at the
        // direct neighbour with distance 1. A hole six cells from the source is
        // five from that neighbour, so it is never reached: every direction ties
        // at "unreachable" and the water still flows sideways rather than waiting
        // for a hole it cannot see.
        let mut world = TestWorld::new();
        for z in -8..=8 {
            world.stone(0, 63, z);
        }
        for z in -8..=8 {
            world.stone(-1, 64, z);
            world.stone(1, 64, z);
        }
        for z in [-8, 9] {
            world.stone(0, 64, z);
        }
        // The hole, six cells north: outside the search.
        world.set(pos(0, 63, -6), BlockFacts::air(0));
        world.water(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);

        assert_eq!(
            world.fluid_at(0, 64, -1),
            FluidState::water(1),
            "an unreachable hole must not stop the flow"
        );
        assert_eq!(
            world.fluid_at(0, 64, 1),
            FluidState::water(1),
            "both open directions tie, so both flow"
        );
        assert_eq!(queue.pending(), 2);
    }

    #[test]
    fn the_lava_spread_delay_is_four_times_when_lava_climbs() {
        // `LavaFluid.getSpreadDelay`: ×4 when both states are non-falling and the
        // new height is the greater, unless a 1-in-4 draw suppresses it.
        let mut world = TestWorld::new();
        world.floor(63, -2, 2);
        // `FluidState::lava(level)` takes the **block level**: level 4 is the
        // shallower fluid (amount 4) and level 3 the deeper one (amount 5), so the
        // new state's `getHeight` is the greater one.
        world.lava(0, 64, 0, 4);
        let rules = FluidRules::lava(false, false);
        let mut random = RandomSource::new(7);
        let mut seen_fast = false;
        let mut seen_slow = false;
        for _ in 0..64 {
            let delay = super::flow::get_spread_delay(
                &world,
                rules,
                pos(0, 64, 0),
                FluidState::lava(4),
                FluidState::lava(3),
                &mut random,
            );
            assert!(
                delay == 30 || delay == 120,
                "the overworld lava delay is 30, quadrupled to 120"
            );
            seen_fast |= delay == 30;
            seen_slow |= delay == 120;
        }
        assert!(seen_slow, "three draws in four quadruple the delay");
        assert!(seen_fast, "the fourth does not");
        // The flat case (no height increase) is never quadrupled.
        let flat = super::flow::get_spread_delay(
            &world,
            rules,
            pos(0, 64, 0),
            FluidState::lava(2),
            FluidState::lava(3),
            &mut random,
        );
        assert_eq!(flat, 30);
    }

    #[test]
    fn a_fence_stops_water_but_air_does_not() {
        // The documented approximation in `can_pass_through_wall`: a partial
        // non-waterloggable shape blocks. `minecraft:oak_fence` is such a shape;
        // this pins that the engine treats "not full cube, cannot hold fluid" as
        // a wall rather than as open space.
        let mut world = TestWorld::new();
        world.floor(63, -2, 2);
        world.water(0, 64, 0, 0);
        let mut fence = BlockFacts::air(500);
        fence.air = false;
        fence.can_hold_any_fluid = false;
        world.set(pos(1, 64, 0), fence);
        let mut queue = FluidQueue::new();
        tick_once(&mut world, water_rules(), pos(0, 64, 0), 0, &mut queue);
        assert_eq!(
            world.fluid_at(1, 64, 0),
            FluidState::Empty,
            "water must not pass the fence"
        );
        assert_eq!(
            world.fluid_at(-1, 64, 0),
            FluidState::water(1),
            "the open faces still flow"
        );
    }

    #[test]
    fn an_unloaded_chunk_is_never_written_through() {
        // `facts` returning `None` means "cannot interact"; a fluid must not
        // spread into a chunk that is not loaded (ADR-0009 §2.4).
        #[derive(Default)]
        struct UnloadedWorld {
            inner: TestWorld,
        }
        impl FluidWorld for UnloadedWorld {
            fn facts(&self, pos: Pos) -> Option<BlockFacts> {
                if pos.x > 0 {
                    return None;
                }
                self.inner.facts(pos)
            }
            fn place_fluid(&mut self, pos: Pos, fluid: FluidState) -> bool {
                self.inner.place_fluid(pos, fluid)
            }
            fn set_block_named(&mut self, pos: Pos, name: &str) -> bool {
                self.inner.set_block_named(pos, name)
            }
        }
        let mut world = UnloadedWorld::default();
        world.inner.floor(63, -2, 2);
        world.inner.water(0, 64, 0, 0);
        let mut queue = FluidQueue::new();
        let mut random = RandomSource::new(1);
        super::flow::tick(
            &mut world,
            water_rules(),
            pos(0, 64, 0),
            0,
            &mut queue,
            &mut random,
        );
        assert_eq!(
            world.inner.fluid_at(1, 64, 0),
            FluidState::Empty,
            "the unloaded side is untouched"
        );
        assert_eq!(
            world.inner.fluid_at(-1, 64, 0),
            FluidState::water(1),
            "the loaded side behaves normally"
        );
    }
}
