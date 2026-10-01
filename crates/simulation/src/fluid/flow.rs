//! The flow rules: a port of the 26.1.2 `FlowingFluid` algorithm (P20-01).
//!
//! ## What this is
//!
//! Every function below is the body of a named method of the 26.1.2 server jar's
//! `net.minecraft.world.level.material.FlowingFluid`, `WaterFluid`, `LavaFluid`
//! or `net.minecraft.world.level.block.LiquidBlock`, read with
//! `javap -p -c -classpath target/vanilla-26.1.2/server-26.1.2.jar <class>` on
//! 2026-10-01. The port keeps the jar's *shape* — the same branches in the same
//! order — because the order is observable: which neighbour wins a slope-distance
//! tie, whether a conversion happens before or after a spread, and whether a cell
//! is rescheduled at all decide the tick a player sees water arrive.
//!
//! | function here | jar method |
//! |---|---|
//! | [`tick`] | `FlowingFluid.tick(ServerLevel, BlockPos, BlockState, FluidState)` |
//! | [`get_new_liquid`] | `FlowingFluid.getNewLiquid(ServerLevel, BlockPos, BlockState)` |
//! | `spread` | `FlowingFluid.spread(ServerLevel, BlockPos, BlockState, FluidState)` |
//! | `spread_to_sides` | `FlowingFluid.spreadToSides(ServerLevel, BlockPos, FluidState, BlockState)` |
//! | `spread_to` | `FlowingFluid.spreadTo(LevelAccessor, BlockPos, BlockState, Direction, FluidState)` and `LavaFluid`'s override |
//! | `get_spread` | `FlowingFluid.getSpread(ServerLevel, BlockPos, BlockState)` |
//! | `get_slope_distance` | `FlowingFluid.getSlopeDistance(LevelReader, BlockPos, int, Direction, BlockState, SpreadContext)` |
//! | `is_water_hole` | `FlowingFluid.isWaterHole(BlockGetter, BlockPos, BlockState, BlockPos, BlockState)` |
//! | `can_pass_through_wall` / `can_maybe_pass_through` / `can_pass_through` | the three same-named `FlowingFluid` predicates |
//! | `should_spread_liquid` | `LiquidBlock.shouldSpreadLiquid(Level, BlockPos, BlockState)` |
//! | `notify_neighbors` | `Level.setBlock`'s `updateNeighborsAt(BlockPos, Block)` + `LiquidBlock.neighborChanged` |
//! | `get_spread_delay` | `FlowingFluid.getSpreadDelay` (default) and `LavaFluid`'s override |
//!
//! ## The pieces that are not a literal port, and why
//!
//! - **`can_pass_through_wall`.** The jar asks `Shapes.mergedFaceOccludes`, which
//!   needs per-face occlusion from the block's collision shape. We have shapes
//!   but not face occlusion, so the rule here is: a full cube on either side
//!   blocks, and otherwise both sides must be blocks that can hold a fluid (air,
//!   a fluid, a waterloggable block). A partial non-waterloggable shape (fence,
//!   wall) therefore blocks, where the jar might allow it — conservative, never
//!   the other way. A named test pins the cases this phase relies on.
//! - **The negative-cache thread local** (`OCCLUSION_CACHE`) is not ported: it is
//!   a performance cache, not a rule.
//! - **`beforeDestroyingBlock` does not drop items.** The call is in the jar's
//!   `spreadTo`; the drop is a named gap of P20-01 (see [`super::world`]).
//! - **`fizz` (sound + particles) is not ported.** It is client-visible but has no
//!   simulation effect, and this server has no fluid particle path.
//! - **The `SOUL_SOIL` + `BLUE_ICE` → basalt rule is not ported.** It is
//!   Nether-only (`javap` shows it guarded by `soulSoilBelow`) and this server
//!   has one dimension until P21; the omission is named in the phase report.
//! - **`SpreadContext` is ported as a per-call cache** rather than the jar's
//!   allocation-reusing object with a depth budget. Our recursion is already
//!   bounded by `getSlopeFindDistance` (2 or 4 levels), and the cache exists so
//!   the same neighbour is not classified twice inside one spread computation.
//! - **The neighbour notification is one level deep, and its second half is not
//!   ported.** `Level.setBlock(pos, state, 3)` notifies the six neighbours
//!   ([`notify_neighbors`]), and each neighbour's `LiquidBlock.neighborChanged`
//!   does two things: `shouldSpreadLiquid` (ported — this is what converts lava
//!   beside newly written water) and, when that returns true,
//!   `level.scheduleTick(pos, type, getTickDelay(level))`. The reschedule is
//!   **not** ported: it can only add a tick for a cell that is already stable, and
//!   a stable cell re-ticked recomputes its own level and writes nothing, so no
//!   compared cell can see it — while P20-01's queue-count tests do pin the
//!   numbers it would change. The notification the *conversion's own*
//!   `setBlockAndUpdate` triggers (the jar queues it through
//!   `CollectingNeighborUpdater`) is not chained either; a lava cell converts
//!   only while water is beside it, and every water write is itself notified, so
//!   one level reaches every cell the jar's chain would convert.
//!
//! ## Randomness
//!
//! `LavaFluid.getSpreadDelay` draws `level.getRandom().nextInt(4)`. This port
//! takes a [`RandomSource`] from the caller, so the tick is deterministic for a
//! given seed (AGENTS.md §3.6) instead of reading a hidden generator.

use std::collections::BTreeMap;

use mc_core::random::RandomSource;
use mc_core::tick::Tick;

use super::queue::FluidQueue;
use super::state::{FluidBody, FluidKind, FluidRules, FluidState};
use super::world::{BlockFacts, Dir, FluidWorld, Pos};

/// `Blocks.STONE` — what lava flowing down into water becomes (`LavaFluid.spreadTo`).
pub const STONE: &str = "minecraft:stone";
/// `Blocks.COBBLESTONE` — what *flowing* lava becomes beside water (`LiquidBlock.shouldSpreadLiquid`).
pub const COBBLESTONE: &str = "minecraft:cobblestone";
/// `Blocks.OBSIDIAN` — what a lava *source* becomes beside water.
pub const OBSIDIAN: &str = "minecraft:obsidian";

/// `LavaFluid.canBeReplacedWith` compares the existing fluid's height against
/// this constant: `0.44444445f`, i.e. 4/9 (a fluid of `amount >= 4`).
const LAVA_REPLACEABLE_HEIGHT: f32 = 0.444_444_45;

/// The `Direction`s `LiquidBlock.shouldSpreadLiquid` probes, with
/// `pos.relative(dir.getOpposite())` — every face except `UP`, so the cells it
/// actually inspects are the four horizontal neighbours plus the cell **above**.
///
/// Read from the jar's `POSSIBLE_FLOW_DIRECTIONS` field; the order is the
/// declaration order `DOWN, NORTH, SOUTH, WEST, EAST` that the same field lists.
const POSSIBLE_FLOW_DIRECTIONS: [Dir; 5] =
    [Dir::Down, Dir::North, Dir::South, Dir::West, Dir::East];

/// `NeighborUpdater.UPDATE_ORDER` — the order a written block's neighbours are
/// notified in.
///
/// Read from the jar's static initializer (`javap -p -c
/// net.minecraft.world.level.redstone.NeighborUpdater`, 2026-10-01):
/// `UPDATE_ORDER = {WEST, EAST, DOWN, UP, NORTH, SOUTH}` — **not**
/// `Direction.values()`'s declaration order, and `updateNeighborsAtExceptFromFacing`
/// walks exactly this array, calling `neighborChanged(pos.relative(dir), ...)`.
const UPDATE_ORDER: [Dir; 6] = [
    Dir::West,
    Dir::East,
    Dir::Down,
    Dir::Up,
    Dir::North,
    Dir::South,
];

/// What one fluid tick did, for the tick report and for tests.
///
/// Four independent flags rather than one enum: a single tick can re-level the
/// cell, write several neighbours and convert a lava cell, and the report wants
/// each fact separately. They are not mutually exclusive states, which is what
/// the lint assumes.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent outcomes of one tick, not states of one value"
)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FluidTickOutcome {
    /// The fluid's own level changed and it was rescheduled.
    pub level_changed: bool,
    /// The fluid ran out and its cell was cleared.
    pub removed: bool,
    /// Blocks this tick wrote while spreading.
    pub spread_writes: usize,
    /// A lava/water conversion ran here (obsidian, cobblestone or stone).
    pub converted: bool,
    /// Whether anything at all happened.
    pub did_work: bool,
}

/// `FlowingFluid.tick`: one due fluid tick at `pos`.
///
/// Reads the fluid at `pos` (the jar is handed it, we look it up), re-derives its
/// own level unless it is a source, reschedules with `getSpreadDelay` when that
/// level changed, then spreads to its neighbours.
///
/// Returns without side effects when the cell holds no fluid: a queued tick whose
/// block was replaced by a player is a no-op, not a panic (AGENTS.md §9).
pub fn tick<W: FluidWorld>(
    world: &mut W,
    rules: FluidRules,
    pos: Pos,
    now: Tick,
    queue: &mut FluidQueue,
    random: &mut RandomSource,
) -> FluidTickOutcome {
    let mut outcome = FluidTickOutcome::default();
    let Some(mut facts) = world.facts(pos) else {
        return outcome;
    };
    let mut fluid = facts.fluid;
    if fluid.is_empty() {
        return outcome;
    }
    // `LiquidBlock.onPlace` / `neighborChanged` run `shouldSpreadLiquid` before the
    // fluid does anything else, and in this engine both of those arrive here: a
    // lava cell that water has reached converts instead of flowing (module docs).
    if !should_spread_liquid(world, rules.kind, pos, &mut outcome) {
        // The conversion is a `Level.setBlockAndUpdate` of its own, so the cell's
        // neighbours re-evaluate before this tick ends.
        notify_neighbors(world, pos, &mut outcome);
        outcome.did_work = true;
        return outcome;
    }
    if !fluid.is_source() {
        let new_liquid = get_new_liquid(world, rules, pos, facts);
        let delay = get_spread_delay(world, rules, pos, fluid, new_liquid, random);
        if new_liquid.is_empty() {
            outcome.removed = true;
            fluid = new_liquid;
            world.place_fluid(pos, FluidState::Empty);
            notify_neighbors(world, pos, &mut outcome);
            if let Some(after) = world.facts(pos) {
                facts = after;
            }
        } else if new_liquid != fluid {
            outcome.level_changed = true;
            fluid = new_liquid;
            world.place_fluid(pos, new_liquid);
            notify_neighbors(world, pos, &mut outcome);
            if let Some(after) = world.facts(pos) {
                facts = after;
            }
            queue.schedule(now, pos, delay);
        }
    }
    spread(world, rules, pos, facts, fluid, now, queue, &mut outcome);
    outcome.did_work = outcome.did_work
        || outcome.level_changed
        || outcome.removed
        || outcome.converted
        || outcome.spread_writes != 0;
    outcome
}

/// `FlowingFluid.getNewLiquid`: the level this cell should hold, from its
/// neighbours.
///
/// Quoted structure (offsets from `javap -c FlowingFluid.getNewLiquid`): the four
/// horizontal neighbours raise `maxAmount` (`Math.max` at 118) and count sources
/// (102-108); two or more sources with `canConvertToSource` make a source when the
/// cell below is solid or a source of the same fluid (134-184); a same-fluid cell
/// above with a passable wall (`Direction.UP`, 188-251) makes it falling with
/// amount 8; otherwise the result is `maxAmount - getDropOff` (255-262), empty at
/// or below zero.
#[must_use]
pub fn get_new_liquid<W: FluidWorld>(
    world: &W,
    rules: FluidRules,
    pos: Pos,
    facts: BlockFacts,
) -> FluidState {
    let mut max_amount = 0u8;
    let mut sources = 0u32;
    for dir in Dir::HORIZONTAL {
        let npos = pos.offset(dir);
        let Some(nfacts) = world.facts(npos) else {
            continue;
        };
        if !same_kind(nfacts.fluid, rules.kind) {
            continue;
        }
        if !can_pass_through_wall(facts, nfacts) {
            continue;
        }
        if nfacts.fluid.is_source() {
            sources += 1;
        }
        max_amount = max_amount.max(nfacts.fluid.amount());
    }
    if sources >= 2 && rules.convert_to_source {
        let below = world.facts(pos.offset(Dir::Down));
        if let Some(below) = below
            && (below.full_cube || below.fluid.is_source_of_kind(rules.kind))
        {
            return source_state(rules.kind);
        }
    }
    if let Some(above) = world.facts(pos.offset(Dir::Up))
        && !above.fluid.is_empty()
        && same_kind(above.fluid, rules.kind)
        && can_pass_through_wall(facts, above)
    {
        return falling_state(rules.kind);
    }
    let reduced = i16::from(max_amount) - i16::from(rules.drop_off());
    if reduced <= 0 {
        return FluidState::Empty;
    }
    flowing_state(rules.kind, u8::try_from(reduced).unwrap_or(8))
}

/// `FlowingFluid.getSpreadDelay` / `LavaFluid.getSpreadDelay`.
///
/// The lava override, quoted in full from `javap -c LavaFluid.getSpreadDelay`:
/// `i = getTickDelay(level)`; then, only when both the old and the new state are
/// non-empty, neither is `FALLING`, the new state's `getHeight` is greater than
/// the old one's, and `level.getRandom().nextInt(4) != 0` (three times in four),
/// `i *= 4`. Lava therefore takes 4× as long to spread into a *higher* level
/// unless the 1-in-4 draw passes: 120 ticks in the overworld, 40 in the Nether.
#[must_use]
pub fn get_spread_delay<W: FluidWorld>(
    world: &W,
    rules: FluidRules,
    pos: Pos,
    old: FluidState,
    new: FluidState,
    random: &mut RandomSource,
) -> u32 {
    let mut delay = rules.tick_delay();
    if rules.kind == FluidKind::Lava
        && !old.is_empty()
        && !new.is_empty()
        && !old.is_falling()
        && !new.is_falling()
        && fluid_height(world, new, pos) > fluid_height(world, old, pos)
        && random.next_i32_bounded(4) != 0
    {
        delay = delay.saturating_mul(4);
    }
    delay
}

/// `FluidState.getHeight(level, pos)`: `getOwnHeight` unless the fluid above is
/// the same fluid, in which case the cell is full.
#[must_use]
pub fn fluid_height<W: FluidWorld>(world: &W, fluid: FluidState, pos: Pos) -> f32 {
    let same_above = world
        .facts(pos.offset(Dir::Up))
        .is_some_and(|above| same_kind(above.fluid, fluid.kind().unwrap_or(FluidKind::Water)));
    if same_above {
        return 1.0;
    }
    f32::from(fluid.amount()) / 9.0
}

/// `FlowingFluid.spread`: down first when it can, then sideways.
#[allow(
    clippy::too_many_arguments,
    reason = "the jar's signature plus the queue and report handles this project needs"
)]
fn spread<W: FluidWorld>(
    world: &mut W,
    rules: FluidRules,
    pos: Pos,
    facts: BlockFacts,
    fluid: FluidState,
    now: Tick,
    queue: &mut FluidQueue,
    outcome: &mut FluidTickOutcome,
) {
    if fluid.is_empty() {
        return;
    }
    let below_pos = pos.offset(Dir::Down);
    if let Some(below) = world.facts(below_pos)
        && can_maybe_pass_through(rules, facts, below)
    {
        let new_liquid = get_new_liquid(world, rules, below_pos, below);
        if can_be_replaced_with(world, below.fluid, new_liquid.kind(), Dir::Down, below_pos)
            && can_hold_specific(below, new_liquid.kind())
        {
            spread_to(
                world,
                rules,
                below_pos,
                below,
                Dir::Down,
                new_liquid,
                now,
                queue,
                outcome,
            );
            if source_neighbor_count(world, rules, pos) >= 3 {
                spread_to_sides(world, rules, pos, facts, fluid, now, queue, outcome);
            }
            return;
        }
    }
    if fluid.is_source() || !is_water_hole(world, rules.kind, pos, facts) {
        spread_to_sides(world, rules, pos, facts, fluid, now, queue, outcome);
    }
}

/// `FlowingFluid.spreadToSides`: the sideways spread, at `amount - dropOff`.
#[allow(
    clippy::too_many_arguments,
    reason = "the jar's signature plus the queue and report handles this project needs"
)]
fn spread_to_sides<W: FluidWorld>(
    world: &mut W,
    rules: FluidRules,
    pos: Pos,
    facts: BlockFacts,
    fluid: FluidState,
    now: Tick,
    queue: &mut FluidQueue,
    outcome: &mut FluidTickOutcome,
) {
    let mut amount = i16::from(fluid.amount()) - i16::from(rules.drop_off());
    if fluid.is_falling() {
        amount = 7;
    }
    if amount <= 0 {
        return;
    }
    let mut context = SpreadContext::new(rules.kind);
    for (dir, new_fluid) in get_spread(world, rules, pos, facts, &mut context) {
        let target = pos.offset(dir);
        let Some(tfacts) = world.facts(target) else {
            continue;
        };
        spread_to(
            world, rules, target, tfacts, dir, new_fluid, now, queue, outcome,
        );
    }
}

/// `FlowingFluid.spreadTo`, with `LavaFluid.spreadTo`'s DOWN override, and the
/// two things `Level.setBlock` does after a write.
///
/// The override, quoted from `javap -c LavaFluid.spreadTo`: when the direction is
/// `DOWN`, this fluid is lava and the fluid at the target is water, then if the
/// target's block is a `LiquidBlock` it becomes `Blocks.STONE` — a waterlogged
/// block is not a `LiquidBlock` and is left alone — `fizz` runs and the method
/// returns without placing the fluid.
///
/// The jar reaches the target through `Level.setBlock(pos, state, 3)`, which
/// after the chunk write runs `BlockState.onPlace` on the new state and then
/// `updateNeighborsAt`; both are reproduced here (see [`notify_neighbors`]),
/// because `lava_meets_water` measures exactly that: water written *beside* a
/// lava source converts it in the writing tick.
#[allow(clippy::too_many_arguments)]
fn spread_to<W: FluidWorld>(
    world: &mut W,
    rules: FluidRules,
    pos: Pos,
    facts: BlockFacts,
    dir: Dir,
    fluid: FluidState,
    now: Tick,
    queue: &mut FluidQueue,
    outcome: &mut FluidTickOutcome,
) {
    if rules.kind == FluidKind::Lava
        && dir == Dir::Down
        && facts.fluid.kind() == Some(FluidKind::Water)
        && !facts.water_container
    {
        if world.set_block_named(pos, STONE) {
            outcome.converted = true;
        }
        notify_neighbors(world, pos, outcome);
        return;
    }
    if world.place_fluid(pos, fluid) {
        outcome.spread_writes += 1;
    }
    if !fluid.is_empty() {
        // `LiquidBlock.onPlace`: the block just written schedules its own first
        // tick — unless it is a lava cell water has reached, which converts
        // itself away here instead (and is then not scheduled at all).
        if should_spread_liquid(world, rules.kind, pos, outcome) {
            queue.schedule(now, pos, rules.tick_delay());
        }
    }
    notify_neighbors(world, pos, outcome);
}

/// `Level.setBlock`'s `updateNeighborsAt(BlockPos, Block)` half, with
/// `LiquidBlock.neighborChanged`'s reaction.
///
/// Every write this engine makes stands in for `Level.setBlock(pos, state, 3)`
/// (`javap -c` offsets 375-383: `if ((flags & 1) != 0) updateNeighborsAt(pos,
/// oldState.getBlock())`), and the jar then calls `neighborChanged` on the six
/// neighbours in [`UPDATE_ORDER`]. A neighbour holding a fluid re-runs
/// `LiquidBlock.neighborChanged`, whose first act is `shouldSpreadLiquid` — so a
/// lava cell water has just been written beside becomes `OBSIDIAN`/`COBBLESTONE`
/// **in that tick**, not at its own scheduled tick (for overworld lava, thirty
/// ticks later, by which time the measured server has already converted it).
///
/// Only the neighbour's own `shouldSpreadLiquid` is run: the module docs name the
/// reschedule the jar performs when it returns `true` as deliberately not ported.
///
/// Public because the notification is not the flow engine's private business: a
/// cell written by something *outside* the engine — a player's bucket, a
/// `/setblock`, a structure — goes through the same `Level.setBlock` in the jar
/// and must go through the same notification here. `crates/server`'s block-edit
/// feed calls this and [`on_place`] for exactly that reason, so `UPDATE_ORDER`
/// exists once rather than once per crate.
pub fn notify_neighbors<W: FluidWorld>(world: &mut W, pos: Pos, outcome: &mut FluidTickOutcome) {
    for dir in UPDATE_ORDER {
        let neighbor = pos.offset(dir);
        let Some(kind) = world.facts(neighbor).and_then(|facts| facts.fluid.kind()) else {
            continue;
        };
        should_spread_liquid(world, kind, neighbor, outcome);
    }
}

/// `FlowingFluid.getSpread`: which horizontal neighbours this cell may flow into,
/// and with what level.
///
/// Only the neighbours at the **smallest slope distance** are kept (a source
/// neighbour wins outright: `i = 0` and the map is cleared). The tie handling is
/// the jar's: `if (dist < best) map.clear(); if (dist <= best) { ...put...; best = dist; }`.
#[must_use]
pub fn get_spread<W: FluidWorld>(
    world: &W,
    rules: FluidRules,
    pos: Pos,
    facts: BlockFacts,
    context: &mut SpreadContext,
) -> Vec<(Dir, FluidState)> {
    let mut best = 1000u32;
    let mut map: Vec<(Dir, FluidState)> = Vec::new();
    for dir in Dir::HORIZONTAL {
        let npos = pos.offset(dir);
        let Some(nfacts) = context.facts(world, npos) else {
            continue;
        };
        if !can_maybe_pass_through(rules, facts, nfacts) {
            continue;
        }
        let new_fluid = get_new_liquid(world, rules, npos, nfacts);
        if !can_hold_specific(nfacts, new_fluid.kind()) {
            continue;
        }
        let distance = if context.is_hole(world, npos) {
            0
        } else {
            get_slope_distance(world, rules, npos, 1, dir.opposite(), nfacts, context)
        };
        if distance < best {
            map.clear();
        }
        if distance <= best {
            if can_be_replaced_with(world, nfacts.fluid, new_fluid.kind(), dir, npos) {
                map.push((dir, new_fluid));
            }
            best = distance;
        }
    }
    map
}

/// `FlowingFluid.getSlopeDistance`: how far the fluid must travel sideways to
/// find a hole, bounded by `getSlopeFindDistance`.
fn get_slope_distance<W: FluidWorld>(
    world: &W,
    rules: FluidRules,
    pos: Pos,
    distance: u32,
    from: Dir,
    facts: BlockFacts,
    context: &mut SpreadContext,
) -> u32 {
    let mut best = 1000u32;
    for dir in Dir::HORIZONTAL {
        if dir == from {
            continue;
        }
        let npos = pos.offset(dir);
        let Some(nfacts) = context.facts(world, npos) else {
            continue;
        };
        if !can_pass_through(rules, facts, nfacts) {
            continue;
        }
        if context.is_hole(world, npos) {
            return distance;
        }
        if distance < u32::from(rules.slope_find_distance()) {
            let found = get_slope_distance(
                world,
                rules,
                npos,
                distance + 1,
                dir.opposite(),
                nfacts,
                context,
            );
            if found < best {
                best = found;
            }
        }
    }
    best
}

/// `FlowingFluid.isWaterHole`: the cell below is reachable **and** can take this
/// fluid (or already holds it).
#[must_use]
pub fn is_water_hole<W: FluidWorld>(
    world: &W,
    kind: FluidKind,
    pos: Pos,
    facts: BlockFacts,
) -> bool {
    let Some(below) = world.facts(pos.offset(Dir::Down)) else {
        return false;
    };
    if !can_pass_through_wall(facts, below) {
        return false;
    }
    same_kind(below.fluid, kind) || below.can_hold_fluid(kind)
}

/// `FlowingFluid.canMaybePassThrough`.
#[must_use]
fn can_maybe_pass_through(rules: FluidRules, facts: BlockFacts, neighbor: BlockFacts) -> bool {
    !neighbor.fluid.is_source_of_kind(rules.kind)
        && neighbor.can_hold_any_fluid
        && can_pass_through_wall(facts, neighbor)
}

/// `FlowingFluid.canPassThrough`.
#[must_use]
fn can_pass_through(rules: FluidRules, facts: BlockFacts, neighbor: BlockFacts) -> bool {
    can_maybe_pass_through(rules, facts, neighbor) && neighbor.can_hold_specific_fluid(rules.kind)
}

/// `FlowingFluid.canPassThroughWall`, approximated; see the module docs.
#[must_use]
fn can_pass_through_wall(facts: BlockFacts, neighbor: BlockFacts) -> bool {
    if facts.full_cube || neighbor.full_cube {
        return false;
    }
    // Neither side is a full cube. The jar resolves this with
    // `Shapes.mergedFaceOccludes`; we allow the pair only when both blocks are
    // ones a fluid can occupy at all, which covers air, fluids and
    // waterloggable blocks and conservatively blocks partial solid shapes.
    facts.can_hold_any_fluid && neighbor.can_hold_any_fluid
}

/// `FlowingFluid.sourceNeighborCount`: horizontal neighbours that are sources of
/// this fluid.
fn source_neighbor_count<W: FluidWorld>(world: &W, rules: FluidRules, pos: Pos) -> u32 {
    let mut count = 0;
    for dir in Dir::HORIZONTAL {
        if let Some(facts) = world.facts(pos.offset(dir))
            && facts.fluid.is_source_of_kind(rules.kind)
        {
            count += 1;
        }
    }
    count
}

/// `FluidState.canBeReplacedWith` for the fluid already at the target cell.
///
/// `WaterFluid`: `direction == DOWN && !new.is(WATER)` — water yields downward to
/// anything that is not water. `LavaFluid`: `getHeight(level, pos) >= 0.44444445f
/// && new.is(WATER)` — lava yields to water only when it is at least 4/9 high.
/// The base `Fluid` implementation returns `true` for the empty fluid.
fn can_be_replaced_with<W: FluidWorld>(
    world: &W,
    existing: FluidState,
    new_kind: Option<FluidKind>,
    dir: Dir,
    pos: Pos,
) -> bool {
    match existing.kind() {
        Some(FluidKind::Water) => dir == Dir::Down && new_kind != Some(FluidKind::Water),
        Some(FluidKind::Lava) => {
            fluid_height(world, existing, pos) >= LAVA_REPLACEABLE_HEIGHT
                && new_kind == Some(FluidKind::Water)
        }
        None => true,
    }
}

/// `canHoldSpecificFluid` with the jar's `Fluid` argument, which may be `EMPTY`.
fn can_hold_specific(facts: BlockFacts, kind: Option<FluidKind>) -> bool {
    match kind {
        Some(kind) => facts.can_hold_specific_fluid(kind),
        // The empty fluid is not water, so a waterloggable container refuses it.
        None => !facts.water_container,
    }
}

/// `LiquidBlock.onPlace`: the other half of `Level.setBlock`'s post-write work,
/// run on the cell that was just written.
///
/// The jar's `setBlock` runs `BlockState.onPlace` on the **new** state before it
/// notifies anything, and for a `LiquidBlock` that is
/// `if (shouldSpreadLiquid(level, pos, state)) level.scheduleTick(pos, type,
/// getTickDelay(level))`. A lava block a player places beside water therefore
/// converts *itself* here — `OBSIDIAN` as a source, `COBBLESTONE` as a flowing
/// cell — and is never scheduled.
///
/// Returns the `shouldSpreadLiquid` answer, i.e. whether the cell may keep
/// flowing and should be scheduled: `false` means it converted itself away. A
/// cell that holds no fluid has no `LiquidBlock.onPlace` to run and answers
/// `true`; the caller's own "is this a fluid block?" check is what decides the
/// scheduling of such a cell.
///
/// Public for the same reason as [`notify_neighbors`]: an edit made outside the
/// engine (bucket, command, structure) is a `Level.setBlock` too, and its
/// written cell must get this chance before anything schedules it.
pub fn on_place<W: FluidWorld>(world: &mut W, pos: Pos, outcome: &mut FluidTickOutcome) -> bool {
    let Some(kind) = world.facts(pos).and_then(|facts| facts.fluid.kind()) else {
        return true;
    };
    should_spread_liquid(world, kind, pos, outcome)
}

/// `LiquidBlock.shouldSpreadLiquid`: `true` when the cell may keep flowing,
/// `false` when it converted.
///
/// Quoted from `javap -c LiquidBlock.shouldSpreadLiquid`: only a **lava** cell
/// runs this; it probes `pos.relative(dir.getOpposite())` for each of
/// [`POSSIBLE_FLOW_DIRECTIONS`] (so: the four horizontal neighbours and the cell
/// above) and, on finding water, replaces the cell with `Blocks.OBSIDIAN` when
/// the lava is a source and `Blocks.COBBLESTONE` when it is flowing, then fizzes
/// and returns false. The Nether-only `SOUL_SOIL`/`BLUE_ICE` → basalt arm is not
/// ported (module docs).
///
/// It is called with the *cell's own* kind rather than a whole [`FluidRules`],
/// because the jar's method reads nothing else: [`notify_neighbors`] hands it a
/// neighbour's kind, which may differ from the fluid that did the writing.
fn should_spread_liquid<W: FluidWorld>(
    world: &mut W,
    kind: FluidKind,
    pos: Pos,
    outcome: &mut FluidTickOutcome,
) -> bool {
    if kind != FluidKind::Lava {
        return true;
    }
    let Some(facts) = world.facts(pos) else {
        return true;
    };
    if facts.fluid.kind() != Some(FluidKind::Lava) {
        return true;
    }
    let water_adjacent = POSSIBLE_FLOW_DIRECTIONS.iter().any(|dir| {
        world
            .facts(pos.offset(dir.opposite()))
            .is_some_and(|probe| probe.fluid.kind() == Some(FluidKind::Water))
    });
    if !water_adjacent {
        return true;
    }
    let target = if facts.fluid.is_source() {
        OBSIDIAN
    } else {
        COBBLESTONE
    };
    if world.set_block_named(pos, target) {
        outcome.converted = true;
    }
    false
}

/// The `SpreadContext` cache the jar threads through `getSpread`.
#[derive(Debug)]
pub struct SpreadContext {
    kind: FluidKind,
    facts: BTreeMap<Pos, Option<BlockFacts>>,
    holes: BTreeMap<Pos, bool>,
}

impl SpreadContext {
    /// A fresh cache for one spread computation.
    #[must_use]
    pub fn new(kind: FluidKind) -> Self {
        Self {
            kind,
            facts: BTreeMap::new(),
            holes: BTreeMap::new(),
        }
    }

    /// The cached `BlockState` at `pos`.
    fn facts<W: FluidWorld>(&mut self, world: &W, pos: Pos) -> Option<BlockFacts> {
        if let Some(cached) = self.facts.get(&pos) {
            return *cached;
        }
        let value = world.facts(pos);
        self.facts.insert(pos, value);
        value
    }

    /// The cached `isWaterHole` answer for `pos`.
    fn is_hole<W: FluidWorld>(&mut self, world: &W, pos: Pos) -> bool {
        if let Some(cached) = self.holes.get(&pos) {
            return *cached;
        }
        let value = match self.facts(world, pos) {
            Some(facts) => is_water_hole(world, self.kind, pos, facts),
            None => false,
        };
        self.holes.insert(pos, value);
        value
    }
}

/// Whether `fluid` is the given kind (`Fluid.isSame`).
#[must_use]
fn same_kind(fluid: FluidState, kind: FluidKind) -> bool {
    fluid.kind() == Some(kind)
}

fn source_state(kind: FluidKind) -> FluidState {
    match kind {
        FluidKind::Water => FluidState::Water(FluidBody::source()),
        FluidKind::Lava => FluidState::Lava(FluidBody::source()),
    }
}

fn falling_state(kind: FluidKind) -> FluidState {
    match kind {
        FluidKind::Water => FluidState::Water(FluidBody::falling()),
        FluidKind::Lava => FluidState::Lava(FluidBody::falling()),
    }
}

fn flowing_state(kind: FluidKind, amount: u8) -> FluidState {
    match kind {
        FluidKind::Water => FluidState::Water(FluidBody::flowing(amount)),
        FluidKind::Lava => FluidState::Lava(FluidBody::flowing(amount)),
    }
}
