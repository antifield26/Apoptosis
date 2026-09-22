//! The tick: phases, entity simulation, broadcast (P15-03).
//!
//! Mechanical split of `super`: every item here moved byte-identical.
//! No logic changed; `super` keeps the struct, the sessions and the
//! persistence.

use mc_core::error::ServerResult;
use mc_core::tick::Tick;
use mc_entity::combat::{
    Attacker, BASE_MELEE_KNOCKBACK, CombatStats, DamageSource, armor_absorb, worn_stats,
};
use mc_entity::entity::{EntityBody, EntityId, EntityKind};
use mc_entity::mob::{
    ATTACK_RANGE, Mob, MobAttackStyle, MobGoal, MobKind, MobObservation, MobSighting,
};
use mc_entity::pathfind::{BlockView, SearchLimits, find_path};

use mc_entity::player::DamageOutcome;
use mc_network::bridge::{ClientEvent, ClientEventKind, ConnectionId};
use mc_persistence::chunk::ChunkPos;
use mc_persistence::level::Difficulty;
use mc_protocol::RawPacket;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BIOMES_PER_SECTION, BlockChangedAck, BlockUpdate, ChunkSection, ContainerSetContent,
    ContainerSetData, ForgetLevelChunk, HEIGHTMAP_WORLD_SURFACE, Heightmap, HurtAnimation,
    LevelChunkWithLight, LightUpdate, NETWORK_BIOME_MIN_BITS, PalettedContainer as WireContainer,
    SetChunkCacheCenter, SetChunkCacheRadius, SetTime, block_position,
};
use mc_protocol::text::TextComponent;
use mc_simulation::{PhaseRunner, TickPhase};
use mc_world::Vec3;
use mc_world::World;
use mc_world::chunk::Chunk;
use mc_world::collision::Aabb;
use std::collections::{BTreeMap, BTreeSet};
use tracing::{debug, trace, warn};

use super::{
    AiRng, CHAT_TYPE_CHAT, CHUNKS_PER_TICK, ENTITY_GRAVITY, EYE_HEIGHT, FALL_DAMAGE_THRESHOLD,
    FOOD_TICK_INTERVAL, Game, GroundItem, INVULNERABLE_TICKS, ITEM_MERGE_RADIUS_SQR,
    ITEM_PICKUP_RADIUS_SQR, LIGHT_UPDATES_PER_TICK, MOB_LOOKAHEAD_BLOCKS, NO_BLOCK_CHANGE_SEQUENCE,
    OpenKind, PENDING_INTENT_BUDGET, PLAINS_BIOME_ID, REST_EPSILON, TickReport,
    UNLOAD_MARGIN_CHUNKS, block_reach, chunk_of, floor_to_i32, is_container_block, light_fields,
    mark_block_dirty, mirror_inventory, open_kind_for, wire_angle, wire_stack,
};

/// The live world as a pathfinding view: solidity is movement-blocking
/// (exactly [`BlockView`]'s contract), unloaded cells read back unknown and
/// therefore solid, so no path routes through ungenerated void.
struct WorldView<'a>(&'a World);

impl BlockView for WorldView<'_> {
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.0.is_solid(x, y, z)
    }
}

/// Hunger and status effects for one session per tick (P11-06, P16-03).
///
/// Extracted from `tick_players` when the dig events pushed it over the
/// line budget; behaviour byte-identical: food on the 4-second timer,
/// poison/wither/regen beside it, death messages collected.
fn tick_session_vitals(
    session: &mut super::session::Session,
    tick: u64,
    items: &mc_registry::ItemRegistry,
    messages: &mut Vec<(ConnectionId, String)>,
    effect_events: &mut Vec<(ConnectionId, i32, Vec<i32>)>,
) {
    // Vanilla heals on a 4-second timer (`foodTickTimer`), not every tick.
    // Calling this every tick made regeneration ~20x too fast and meant
    // exhaustion never accrued, so food never depleted in play (Audit 03).
    // The exhaustion cost of movement/actions is P05-14's table; until it
    // exists this passes 0, which is why a player never gets hungry yet.
    // An off-tick changed nothing, so the outcome is "no damage".
    let outcome = if tick.is_multiple_of(FOOD_TICK_INTERVAL) {
        session.player.tick_food(0.0)
    } else {
        mc_entity::player::DamageOutcome {
            applied: false,
            died: false,
            dealt: 0.0,
            health: session.player.health,
        }
    };
    if outcome.died && !session.player.is_alive() {
        messages.push((session.id, "You died!".to_owned()));
    }
    // Status effects tick down here, beside hunger: poison and wither
    // deal through the shared damage path (armour-aware, floored at
    // half a heart for poison), regeneration heals, and expired ids
    // come back for their removal packets below.
    let was_alive = session.player.is_alive();
    let armor = mc_entity::combat::worn_stats(&session.player.inventory, items);
    let expired = session.player.tick_effects(tick, &armor);
    if !expired.is_empty() {
        effect_events.push((session.id, session.entity.get(), expired));
    }
    if was_alive && !session.player.is_alive() {
        messages.push((session.id, "You died!".to_owned()));
    }
}

/// Advance one session's survival dig by a tick (P16-05).
///
/// Pure accumulation plus event collection: breaks mutate the world and
/// stages broadcast, so both happen after the session loop from the
/// collected vectors (the `tick_players` collect-then-apply shape, which
/// also keeps this function borrow-clean: session mutably, world and
/// registries shared).
///
/// Anything that invalidates the dig (target gone or changed, held item
/// swapped, walked out of reach, creative switch) clears the state; a clear
/// overlay goes out only when a stage was announced.
fn tick_session_dig(
    session: &mut super::session::Session,
    world: &World,
    registries: &mc_registry::Registries,
    dig_breaks: &mut Vec<(ConnectionId, i32, i32, i32, i32)>,
    dig_stages: &mut Vec<(i32, i32, i32, i32, i8)>,
    dig_clears: &mut Vec<(i32, i32, i32, i32)>,
) {
    let Some(dig) = session.dig else {
        return;
    };
    let id = session.id;
    let entity_id = session.entity.get();
    let announced = dig.stage >= 0;
    // Creative breaks are instant through their own path: a survival dig
    // that survives a gamemode flip ends here.
    if session.player.game_mode.is_creative() {
        session.dig = None;
        if announced {
            dig_clears.push((entity_id, dig.x, dig.y, dig.z));
        }
        return;
    }
    let current = world.get_block_loaded(dig.x, dig.y, dig.z);
    let changed = current.is_none_or(|block| block != dig.block);
    let held = session.player.inventory.selected_item().item_id();
    let swapped = held != dig.held;
    let eye = Vec3::new(
        session.player.position.x,
        session.player.position.y + EYE_HEIGHT,
        session.player.position.z,
    );
    let reach = block_reach(false);
    let walked_away = Aabb::block(dig.x, dig.y, dig.z).distance_to_sqr(eye) >= reach * reach;
    if changed || swapped || walked_away {
        session.dig = None;
        if announced {
            dig_clears.push((entity_id, dig.x, dig.y, dig.z));
        }
        return;
    }
    let Some(block) = current else {
        return;
    };
    let name = registries
        .blocks
        .block_name(block)
        .map(str::to_owned)
        .unwrap_or_default();
    let held_name = held.and_then(|held| registries.items.name(held).ok().map(str::to_owned));
    match mc_registry::dig_rate(
        &registries.blocks,
        &registries.items,
        held_name.as_deref(),
        &name,
        session.player.on_ground,
    ) {
        mc_registry::DigRate::Unbreakable | mc_registry::DigRate::Instant => {
            // Unreachable for a tracked dig (START refuses/breaks those),
            // so a changed table cancels rather than breaks: breaking here
            // would spend a dig the START arm refused.
            session.dig = None;
            if announced {
                dig_clears.push((entity_id, dig.x, dig.y, dig.z));
            }
        }
        mc_registry::DigRate::PerTick(rate) => {
            let progress = dig.progress + rate;
            if progress >= 1.0 {
                session.dig = None;
                dig_breaks.push((id, dig.x, dig.y, dig.z, dig.block));
                return;
            }
            let stage = i8::try_from(floor_to_i32(f64::from(progress) * 10.0).min(9)).unwrap_or(9);
            if let Some(state) = session.dig.as_mut() {
                state.progress = progress;
                if stage != dig.stage {
                    state.stage = stage;
                    dig_stages.push((entity_id, dig.x, dig.y, dig.z, stage));
                }
            }
        }
    }
}

impl Game {
    /// Run one tick: the six phases, in order, each timed.
    ///
    /// # Errors
    ///
    /// Only a genuine invariant failure escapes: every client-controlled path
    /// refuses bad input and returns `Ok`. A phase error aborts the tick and is
    /// recorded in [`Game::metrics`] before it propagates.
    pub fn tick(&mut self) -> ServerResult<TickReport> {
        self.tick = self.tick.saturating_add(1);
        let tick = self.tick;
        // This tick's counters start empty; the phases fill them in.
        self.report = TickReport::default();
        // Borrow split: `Scheduler::run_tick` needs `&mut Scheduler` *and* a `&mut`
        // reference to the runner, which is this same `Game`. Moving the scheduler
        // out for the duration of the call is a move of a few kilobytes, not a
        // copy, and the placeholder is dropped straight afterwards.
        let mut scheduler = std::mem::take(&mut self.scheduler);
        let outcome = scheduler.run_tick(tick, self);
        self.scheduler = scheduler;
        outcome?;
        Ok(self.report.clone())
    }

    // ---------------------------------------------------------------- phases

    /// One phase's body, with this tick's report already separated out of `self`.
    fn run_phase_inner(
        &mut self,
        tick: Tick,
        phase: TickPhase,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        match phase {
            TickPhase::Network => self.phase_network(report),
            TickPhase::ScheduledTicks => {
                self.tick_scheduled(tick, report);
                Ok(())
            }
            TickPhase::Entities => {
                // The budget is above the phase body: items-before-statements.
                const ENTITY_MOVES_PER_TICK: usize = 128;
                // **Positions before the phase, compared after it.** The position update lives in a per-entity
                // helper that has no `report`, so the signal has to be raised here, on the boundary.
                // Yaw joins the snapshot (P11-03): a mob that turned faces its
                // target on the client, which a position-only delta cannot say.
                let before: Vec<_> = self
                    .entities
                    .ids()
                    .filter_map(|id| self.entities.get(id).map(|e| (id, e.position, e.yaw)))
                    .collect();
                self.phase_entities(report);
                // The budget (P11-03): vanilla streams every moved entity every
                // tick, and at this server's mob counts that is far below the
                // cap; the cap exists so a pathological crowd degrades by
                // *deferred* movement packets — the rotating cursor means a
                // deferred entity is first in line next tick — rather than by
                // unbounded queue growth.
                let mut before: Vec<_> = before;
                let population = before.len();
                if population > ENTITY_MOVES_PER_TICK {
                    let start = (self.entity_move_cursor % population as u64) as usize;
                    before.rotate_left(start);
                    before.truncate(ENTITY_MOVES_PER_TICK);
                    self.entity_move_cursor = (self.entity_move_cursor
                        + ENTITY_MOVES_PER_TICK as u64)
                        % population as u64;
                }
                for (id, was, was_yaw) in before {
                    let Some(entity) = self.entities.get(id) else {
                        // Removed during the phase; its removal is announced where removals are swept.
                        continue;
                    };
                    let now = entity.position;
                    // Deltas are 1/4096 of a block, which is what the packet carries. A movement too small for
                    // that is one no client could be told about, so **the comparison is between the values that
                    // would go on the wire rather than between the floats** -- which is both the honest test and
                    // the one clippy does not have to distrust. A movement too *large* for `i16` is clamped:
                    // vanilla teleports instead, and until that exists the client catches up on its next chunk.
                    // A let, not a const: an item declares itself from the start of its scope, so one written after
                    // statements reads as though it came later than it does (clippy::items_after_statements).
                    let scale = 4096.0;
                    let clamp = |value: f64| {
                        let scaled = (value * scale).round();
                        i16::try_from(scaled.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i64)
                            .unwrap_or(i16::MAX)
                    };
                    let (dx, dy, dz) = (
                        clamp(now.x - was.x),
                        clamp(now.y - was.y),
                        clamp(now.z - was.z),
                    );
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let (on_ground, yaw, pitch) = (entity.on_ground, entity.yaw, entity.pitch);
                    // A turned mob rides the rotation variant of the same
                    // packet; an unchanged heading keeps the cheaper form.
                    let packet = if wire_angle(yaw) == wire_angle(was_yaw) {
                        mc_protocol::packets::play::MoveEntityPos {
                            entity_id: id.get(),
                            dx,
                            dy,
                            dz,
                            on_ground,
                        }
                        .to_raw()?
                    } else {
                        mc_protocol::packets::play::MoveEntityPosRot {
                            entity_id: id.get(),
                            dx,
                            dy,
                            dz,
                            yaw: wire_angle(yaw),
                            pitch: wire_angle(pitch),
                            on_ground,
                        }
                        .to_raw()?
                    };
                    self.broadcast_chunk(chunk_of(now.x, now.z), &packet, report);
                }
                Ok(())
            }
            TickPhase::Players => self.phase_players(report),
            TickPhase::BlockEntities => self.tick_block_entities(report),
            TickPhase::Broadcast => self.phase_broadcast(report, tick),
        }
    }

    /// Phase 1: drain the inbound channel (bounded) and decode the work.
    ///
    /// Intents are queued rather than applied; see the module docs for why.
    fn phase_network(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let events = self.events.drain(PENDING_INTENT_BUDGET);
        report.events = events.len();
        for event in events {
            self.apply_event(event, report)?;
        }
        let overflowed = std::mem::take(&mut report.overflowed);
        report.disconnects += self.enforce_overflow(overflowed);
        Ok(())
    }

    /// Phase 2: **documented no-op**.
    ///
    /// Scheduled block, fluid and entity ticks are P05-05 (fluids) and P05-06
    /// (block ticks): a per-position due queue ordered by `(tick, position)` and
    /// the redstone/`randomTick` hooks land there. Nothing here pretends to run
    /// them: an empty tick list and a missing scheduler look identical from the
    /// outside, so the absence is stated rather than stubbed with a placeholder
    /// that would make [`Game::metrics`] look busy.
    /// Phase 2: drain due block ticks, then drive the redstone model (P13-01/03).
    ///
    /// The drain keeps P13-01's counting (fired/pending on the report). The
    /// drive mirrors `run_block_tick`'s orchestration — prepare each due
    /// position, run `propagate` under the nominal budget, reschedule live
    /// wires — with the drain count retained for the report, which the library
    /// body does not return. `World::set_block` writes land in the world's
    /// change list, so the Broadcast phase sends them as `block_update`s with
    /// no redstone-specific packet path.
    fn tick_scheduled(&mut self, tick: Tick, report: &mut TickReport) {
        let budget = mc_redstone::UpdateBudget::nominal();
        let drain = self.scheduled_ticks.drain_due_block_ticks(tick, budget);
        report.scheduled_ticks_fired = drain.len();
        report.scheduled_ticks_pending = drain.scheduled_later;
        for pos in &drain.positions {
            // Due component ticks act first so the recompute below sees the
            // post-action state (an observer going dark, a dispenser one item
            // lighter).
            self.fire_due_component_tick(*pos);
            mc_redstone::propagation::prepare(&mut self.scheduled_ticks, *pos);
            mc_redstone::propagation::prepare_self(&mut self.scheduled_ticks, *pos);
        }
        let table = mc_redstone::EmitterTable::new(&self.registries.blocks);
        let propagation = mc_redstone::propagation::propagate(
            &mut self.world,
            &mut self.scheduled_ticks,
            table,
            budget,
        );
        report.redstone_updates = propagation.updates_processed;
        report.redstone_changed = propagation.blocks_changed;
        let changed: Vec<mc_redstone::BlockPos> = propagation
            .changes
            .iter()
            .map(|change| change.pos)
            .chain(drain.positions)
            .collect();
        mc_redstone::propagation::schedule_wire_recheck(
            &self.world,
            table,
            &mut self.scheduled_ticks,
            tick,
            &changed,
        );
        // Pulse-driven components watch state changes, not recomputes: an
        // observer fires when its watched block changes, a dispenser latches
        // on a rising edge. Both are edge events the propagation loop does
        // not carry, so they are scanned here off the change list (and in
        // `redstone_feed` for player edits, which queue before propagate).
        let triggered: Vec<(i32, i32, i32)> = propagation
            .changes
            .iter()
            .map(|change| (change.pos.x, change.pos.y, change.pos.z))
            .collect();
        for (x, y, z) in triggered {
            self.trigger_neighbors_of_change(x, y, z);
        }
    }

    /// Schedule a block tick for `(x, y, z)`, `delay` ticks from now (P13-01).
    ///
    /// The scheduling half of the queue the `ScheduledTicks` phase drains; the
    /// world feed of P13-02 calls this when a placed block needs one. Delays
    /// past [`mc_redstone::MAX_SCHEDULE_DELAY`] or overflowing the tick counter
    /// are refused as hostile input, never clamped silently — use
    /// `schedule_clamped` on the queue for wire/file values.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when the delay is out of range or the due
    /// tick overflows.
    pub fn schedule_block_tick(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        delay: u32,
    ) -> ServerResult<mc_redstone::Inserted> {
        let now = self.tick;
        self.scheduled_ticks
            .schedule(now, mc_redstone::BlockPos::new(x, y, z), delay)
    }

    /// Write one boolean property, returning whether anything changed.
    ///
    /// Shared by the observer pulse and the dispenser latch: reads the
    /// current assignment, no-ops when it already matches (so the write,
    /// the feed and the schedule below all stay edge-triggered), otherwise
    /// resolves, writes, feeds and reports true. A state that stops
    /// resolving fails safe as "no change" rather than as a stuck latch.
    fn set_block_flag(&mut self, x: i32, y: i32, z: i32, key: &str, value: bool) -> bool {
        let Some(id) = self.world.get_block_loaded(x, y, z) else {
            return false;
        };
        let Ok(name) = self.registries.blocks.block_name(id) else {
            return false;
        };
        let name = name.to_owned();
        let Ok(props) = self.registries.blocks.properties_of(id) else {
            return false;
        };
        if props
            .iter()
            .any(|(prop, val)| prop == key && val == &value.to_string())
        {
            return false;
        }
        let mut rewritten = props.clone();
        let Some(entry) = rewritten.iter_mut().find(|(prop, _)| prop == key) else {
            return false;
        };
        entry.1 = value.to_string();
        let Ok(new_id) = self.registries.blocks.state_id(&name, &rewritten) else {
            return false;
        };
        if new_id == id {
            return false;
        }
        if self.world.set_block(x, y, z, new_id).is_err() {
            return false;
        }
        self.redstone_feed(x, y, z, new_id);
        true
    }

    /// Pulse observers and latch dispensers around a changed block (P17-01).
    ///
    /// For each of the six neighbors: an observer whose front faces the
    /// change powers up with an off-tick in 2 (pumpkin `ObserverBlock` —
    /// schedule only from unpowered, so the powered check doubles as the
    /// scheduled check); a dispenser/dropper latches `triggered` with a
    /// dispense in 4 on a rising edge and unlatches on the fall (pumpkin
    /// `DispenserBlock.on_neighbor_update`, including the above-cell power
    /// check). Reads only, except on a genuine edge.
    pub(crate) fn trigger_neighbors_of_change(&mut self, x: i32, y: i32, z: i32) {
        const OFFSETS: [(i32, i32, i32); 6] = [
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
            (-1, 0, 0),
            (1, 0, 0),
        ];
        for (dx, dy, dz) in OFFSETS {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            let Some(id) = self.world.get_block_loaded(nx, ny, nz) else {
                continue;
            };
            let Ok(name) = self.registries.blocks.block_name(id) else {
                continue;
            };
            if name == mc_redstone::OBSERVER {
                // Front offset of the observer must be the change: the
                // change sits at (observer + facing).
                let facing = self
                    .registries
                    .blocks
                    .properties_of(id)
                    .ok()
                    .and_then(|props| {
                        props
                            .iter()
                            .find(|(key, _)| key == "facing")
                            .map(|(_, value)| value.clone())
                    })
                    .unwrap_or_else(|| "north".to_owned());
                let (fx, fy, fz) = mc_redstone::blocks::facing_offset(&facing);
                if (nx + fx, ny + fy, nz + fz) != (x, y, z) {
                    continue;
                }
                if self.set_block_flag(nx, ny, nz, "powered", true)
                    && let Err(error) = self.schedule_block_tick(nx, ny, nz, 2)
                {
                    debug!(%error, "observer off-tick refused");
                }
            } else if name == mc_redstone::DISPENSER || name == mc_redstone::DROPPER {
                let powered = self.powered_at(nx, ny, nz) || self.powered_at(nx, ny + 1, nz);
                let triggered = self.registries.blocks.properties_of(id).is_ok_and(|props| {
                    props
                        .iter()
                        .any(|(key, value)| key == "triggered" && value == "true")
                });
                if powered && !triggered {
                    if self.set_block_flag(nx, ny, nz, "triggered", true)
                        && let Err(error) = self.schedule_block_tick(nx, ny, nz, 4)
                    {
                        debug!(%error, "dispenser tick refused");
                    }
                } else if !powered && triggered {
                    self.set_block_flag(nx, ny, nz, "triggered", false);
                }
            }
        }
    }

    /// Run a due component tick: observers go dark, dispensers dispense.
    ///
    /// Due positions are drained by `tick_scheduled`, which recomputes them
    /// right after — so this acts first, letting the recompute see the
    /// post-action state.
    fn fire_due_component_tick(&mut self, pos: mc_redstone::BlockPos) {
        let Some(id) = self.world.get_block_loaded(pos.x, pos.y, pos.z) else {
            return;
        };
        let Ok(name) = self.registries.blocks.block_name(id) else {
            return;
        };
        if name == mc_redstone::OBSERVER {
            self.set_block_flag(pos.x, pos.y, pos.z, "powered", false);
        } else if name == mc_redstone::DISPENSER || name == mc_redstone::DROPPER {
            let triggered = self.registries.blocks.properties_of(id).is_ok_and(|props| {
                props
                    .iter()
                    .any(|(key, value)| key == "triggered" && value == "true")
            });
            if triggered {
                self.dispense_from(pos.x, pos.y, pos.z);
            }
        }
    }

    /// Dispense one item from a triggered dispenser/dropper (P17-01).
    ///
    /// A random occupied slot (seeded game RNG, so replays agree), one item
    /// out the front face with a small forward velocity. Every item drops
    /// as an entity: arrows do not shoot, fluids do not place, tools do not
    /// till — each special is a named gap, not a silent refusal (the item
    /// still leaves, exactly one per pulse). An empty inventory fires
    /// silently (no click sound exists anywhere in this build).
    fn dispense_from(&mut self, x: i32, y: i32, z: i32) {
        let Some(id) = self.world.get_block_loaded(x, y, z) else {
            return;
        };
        let facing = self
            .registries
            .blocks
            .properties_of(id)
            .ok()
            .and_then(|props| {
                props
                    .iter()
                    .find(|(key, _)| key == "facing")
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_else(|| "north".to_owned());
        let (fx, fy, fz) = mc_redstone::blocks::facing_offset(&facing);
        let pos = mc_container::BlockPos::new(x, y, z);
        let taken = self
            .block_entities
            .get_mut(pos)
            .and_then(|entity| entity.data.items_mut())
            .and_then(|items| {
                let occupied: Vec<usize> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, stack)| !stack.is_empty())
                    .map(|(index, _)| index)
                    .collect();
                if occupied.is_empty() {
                    return None;
                }
                let pick = usize::try_from(
                    self.random
                        .next_i32_bounded(i32::try_from(occupied.len()).unwrap_or(1)),
                )
                .unwrap_or(0);
                let index = occupied[pick.min(occupied.len() - 1)];
                Some(items[index].split(1))
            });
        let Some(one) = taken else {
            return;
        };
        if one.is_empty() {
            return;
        }
        mark_block_dirty(&mut self.world, x, z);
        let at = mc_world::Vec3::new(
            f64::from(x + fx) + 0.5,
            f64::from(y + fy) + 0.5,
            f64::from(z + fz) + 0.5,
        );
        match self.spawn_item(one, at) {
            Ok(entity) => {
                if let Some(entity) = self.entities.get_mut(entity) {
                    entity.velocity = mc_world::Vec3::new(
                        f64::from(fx) * 0.3,
                        f64::from(fy) * 0.3,
                        f64::from(fz) * 0.3,
                    );
                }
            }
            Err(error) => {
                warn!(%error, "a dispensed item could not be spawned; restored");
                if let Some(entity) = self.block_entities.get_mut(pos)
                    && let Some(items) = entity.data.items_mut()
                {
                    for stack in items.iter_mut() {
                        if stack.item_id() == one.item_id() && !stack.is_empty() {
                            stack.grow(1);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Phase 5: furnaces cook, hoppers transfer (P12-03/04).
    ///
    /// Each furnace block entity advances one tick through `Furnace::tick` on the
    /// hand-written baseline (P12-08 retires it for pack data); when a player has
    /// that furnace open the changed `container_set_data` properties (0 burn
    /// remaining, 1 burn total, 2 cook progress, 3 cook total — vanilla
    /// `FurnaceMenu` data slots) are sent on its window. Hopper ticking lives
    /// here too (see `tick_hoppers`); the phase stays deterministic by walking
    /// block positions ascending.
    #[allow(clippy::too_many_lines)]
    fn tick_block_entities(&mut self, report: &mut TickReport) -> ServerResult<()> {
        // Resolve once per tick: the table is small and furnaces are few. The
        // table itself is the pack conversion once packs load (P12-08);
        // fuel stays the jar-verified baseline.
        let stack_sizes = mc_entity::stack::StackSizeTable::resolve(&self.registries.items)?;
        let slots = mc_container::FurnaceSlots::CANONICAL;

        // Phase one: advance every furnace, recording data-slot deltas.
        // Collected first so the mutable walk over block entities ends before
        // any `&self` send below.
        let mut deltas: Vec<(mc_container::BlockPos, [i16; 4])> = Vec::new();
        // Furnaces whose *items* changed need the hopper-style content resync
        // below (data slots alone leave the open menu showing pre-tick stacks,
        // which the next click would flush back over the smelted output).
        let mut furnace_items_changed: Vec<mc_container::BlockPos> = Vec::new();
        let positions: Vec<mc_container::BlockPos> = self.block_entities.positions().collect();
        for pos in positions {
            let Some(entity) = self.block_entities.get_mut(pos) else {
                continue;
            };
            let mc_container::BlockEntityData::Furnace {
                items,
                burn_ticks,
                burn_total,
                cook_progress,
                cook_total,
            } = &mut entity.data
            else {
                continue;
            };
            let before = [*burn_ticks, *burn_total, *cook_progress, *cook_total];
            let before_items = items.clone();
            // Bridge the NBT-free payload into the tick's container + state.
            let Ok(mut container) =
                mc_container::Container::new(mc_container::ContainerKind::Furnace, 3)
            else {
                continue;
            };
            for (index, stack) in items.iter().enumerate().take(3) {
                let _ = container.set(index, *stack);
            }
            let mut state = mc_container::FurnaceState {
                burn_ticks_remaining: *burn_ticks,
                burn_ticks_total: *burn_total,
                cook_progress: *cook_progress,
                cook_total: *cook_total,
                experience: 0.0,
            };
            let ticked = mc_container::Furnace::tick(
                &mut state,
                &mut container,
                slots,
                &self.smelting_furnace,
                &self.registries.items,
                &stack_sizes,
            );
            if ticked.is_err() {
                continue;
            }
            for (index, stack) in items.iter_mut().enumerate().take(3) {
                *stack = container.get(index);
            }
            *burn_ticks = state.burn_ticks_remaining;
            *burn_total = state.burn_ticks_total;
            *cook_progress = state.cook_progress;
            *cook_total = state.cook_total;
            let after = [*burn_ticks, *burn_total, *cook_progress, *cook_total];
            if before != after || *items != before_items {
                mark_block_dirty(&mut self.world, pos.x, pos.z);
            }
            if *items != before_items {
                furnace_items_changed.push(pos);
            }
            if before != after {
                let mut narrow = [0i16; 4];
                let mut changed = false;
                for (index, (was, now)) in before.iter().zip(after.iter()).enumerate() {
                    // Cook/burn totals stay under `u16::MAX` (400-tick cooks,
                    // second-scale fuels); saturate rather than wrap on absurd data.
                    let narrow_now = u16::try_from(*now).unwrap_or(u16::MAX) as i16;
                    narrow[index] = narrow_now;
                    changed |= was != now;
                }
                if changed {
                    deltas.push((pos, narrow));
                }
            }
        }

        // Phase 1b: hoppers transfer every 8 game ticks when busy (P12-04).
        //
        // Vanilla pulls from above then pushes below in the same tick; this does
        // push-then-pull with one item per side so a hopper chain advances one
        // slot per cooldown. Positions ascend for determinism; mutations go
        // through temp containers so two map entries are never borrowed together.
        //
        // P17-02: the push target follows the hopper's `facing` (pumpkin ejects
        // into `position.offset(facing)`), and furnaces take part with
        // face-aware routing — pull takes the output slot only (input/fuel
        // refuse hopper extraction), a top entry feeds the input slot with
        // smeltables only, a side entry feeds the fuel slot with fuels only.
        // Furnace XP on extract and the `enabled` redstone lock stay named gaps.
        let mut hopper_touched: Vec<mc_container::BlockPos> = Vec::new();
        let hopper_positions: Vec<mc_container::BlockPos> = self
            .block_entities
            .positions()
            .filter(|pos| {
                self.block_entities
                    .get(*pos)
                    .is_some_and(|e| e.kind() == mc_container::BlockEntityKind::Hopper)
            })
            .collect();
        for pos in hopper_positions {
            // Cooldown gate.
            let cooled = match self.block_entities.get_mut(pos) {
                Some(entity) => match &mut entity.data {
                    mc_container::BlockEntityData::Hopper { cooldown, .. } => {
                        if *cooldown > 0 {
                            *cooldown -= 1;
                            false
                        } else {
                            true
                        }
                    }
                    _ => continue,
                },
                None => continue,
            };
            if !cooled {
                continue;
            }
            // Push target follows `facing` (down = below, else the horizontal
            // neighbour); unreadable states fail safe to below, which is the
            // pre-P17-02 behaviour. The pull source stays the cell above.
            let dest_below = self.hopper_push_offset(pos);
            let dest = mc_container::BlockPos::new(
                pos.x + dest_below.0,
                pos.y + dest_below.1,
                pos.z + dest_below.2,
            );
            let above = mc_container::BlockPos::new(pos.x, pos.y + 1, pos.z);
            let mut moved = false;
            // Push into the facing cell first, then pull from above.
            for (source_pos, dest_pos) in [(pos, dest), (above, pos)] {
                // Furnace routing (P17-02): a furnace source yields its output
                // slot only (input/fuel refuse hopper extraction); a furnace
                // destination takes smeltables into the input on top entry and
                // fuels into the fuel slot on side entry, checked per item.
                let source_is_furnace = self
                    .block_entities
                    .get(source_pos)
                    .is_some_and(|e| e.kind() == mc_container::BlockEntityKind::Furnace);
                let dest_is_furnace = self
                    .block_entities
                    .get(dest_pos)
                    .is_some_and(|e| e.kind() == mc_container::BlockEntityKind::Furnace);
                if dest_is_furnace && source_pos == pos {
                    if self.hopper_feed_furnace(pos, dest_pos, dest_below.1 == -1) {
                        moved = true;
                        hopper_touched.push(source_pos);
                        hopper_touched.push(dest_pos);
                    }
                    if moved {
                        break;
                    }
                    continue;
                }
                if source_is_furnace && dest_pos == pos {
                    if self.hopper_take_furnace_output(source_pos, dest_pos) {
                        moved = true;
                        hopper_touched.push(source_pos);
                        hopper_touched.push(dest_pos);
                    }
                    if moved {
                        break;
                    }
                    continue;
                }
                // Merged double-chest views (P17-02 Step B): a half reads the
                // pair as one 54-slot inventory, like vanilla.
                let (Some((source_items, source_halves)), Some((dest_items, dest_halves))) = (
                    self.hopper_inventory(source_pos),
                    self.hopper_inventory(dest_pos),
                ) else {
                    continue;
                };
                let Ok(mut source) = mc_container::Container::new(
                    mc_container::ContainerKind::Generic,
                    source_items.len(),
                ) else {
                    continue;
                };
                let Ok(mut dest) = mc_container::Container::new(
                    mc_container::ContainerKind::Generic,
                    dest_items.len(),
                ) else {
                    continue;
                };
                for (index, stack) in source_items.iter().enumerate() {
                    let _ = source.set(index, *stack);
                }
                for (index, stack) in dest_items.iter().enumerate() {
                    let _ = dest.set(index, *stack);
                }
                let source_roles = vec![mc_container::SlotRole::Storage; source.len()];
                let dest_roles = vec![mc_container::SlotRole::Storage; dest.len()];
                let Ok(transfer) = mc_container::Hopper::transfer(
                    &mut source,
                    &source_roles,
                    &mut dest,
                    &dest_roles,
                    1,
                ) else {
                    continue;
                };
                if transfer.moved == 0 {
                    continue;
                }
                // Write back through the same views (doubles split 27/27).
                // Both views validated at read and nothing runs between, so
                // a refused write here is unreachable single-threaded; it
                // still counts as no-move rather than a half-applied one.
                let back_source: Vec<mc_entity::stack::ItemStack> =
                    (0..source.len()).map(|i| source.get(i)).collect();
                let back_dest: Vec<mc_entity::stack::ItemStack> =
                    (0..dest.len()).map(|i| dest.get(i)).collect();
                if !self.store_hopper_inventory(source_pos, source_halves, &back_source)
                    || !self.store_hopper_inventory(dest_pos, dest_halves, &back_dest)
                {
                    continue;
                }
                moved = true;
                // Touch both halves of any double, so viewers of either half
                // resync and both chunks mark dirty.
                for half in [source_halves, dest_halves].into_iter().flatten() {
                    hopper_touched.push(mc_container::BlockPos::new(half.0.0, half.0.1, half.0.2));
                    hopper_touched.push(mc_container::BlockPos::new(half.1.0, half.1.1, half.1.2));
                }
                hopper_touched.push(source_pos);
                hopper_touched.push(dest_pos);
                // One side per cooldown, like vanilla's single transfer per wake.
                break;
            }
            if moved
                && let Some(entity) = self.block_entities.get_mut(pos)
                && let mc_container::BlockEntityData::Hopper { cooldown, .. } = &mut entity.data
            {
                *cooldown = mc_container::HOPPER_TRANSFER_COOLDOWN_TICKS;
            }
        }

        // Phase two: send deltas to whoever holds the matching window.
        for (pos, values) in deltas {
            // Find sessions before sending so `&self` sends do not borrow
            // alongside the session walk.
            let viewers: Vec<(ConnectionId, i32)> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.open_block == Some(pos))
                .map(|(id, s)| (*id, i32::from(s.menu.window_id())))
                .collect();
            for (id, window) in viewers {
                for (property, value) in values.iter().enumerate() {
                    let _ = self.send(
                        id,
                        &ContainerSetData {
                            window_id: window,
                            property: property as i16,
                            value: *value,
                        },
                        report,
                    );
                }
            }
        }
        // Hopper and furnace-item viewers get a full resync: a transfer or a
        // smelt moved items, not progress bars, so the window contents (not
        // data slots) changed. Deduped because one transfer touches two
        // positions that may share a viewer; each viewer gets one resync per
        // tick at most. The menu's block half is refreshed from the entity
        // first — otherwise the resync would replay the pre-transfer contents
        // the menu still holds.
        hopper_touched.sort();
        hopper_touched.dedup();
        furnace_items_changed.sort();
        furnace_items_changed.dedup();
        for pos in &hopper_touched {
            mark_block_dirty(&mut self.world, pos.x, pos.z);
        }
        let mut content_touched = hopper_touched;
        for pos in furnace_items_changed {
            if !content_touched.contains(&pos) {
                content_touched.push(pos);
            }
        }
        for pos in content_touched {
            let viewers: Vec<ConnectionId> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.open_block == Some(pos))
                .map(|(id, _)| *id)
                .collect();
            for id in viewers {
                // Refresh the block half from the entity. A 54-slot double
                // menu refreshes from the merged pair (P17-02 Step B).
                let refreshed = {
                    let (entity_items, menu_len) = match (
                        self.block_entities.get(pos).and_then(|e| e.data.items()),
                        self.sessions
                            .get(&id)
                            .and_then(|s| s.menu.container(0).map(mc_container::Container::len)),
                    ) {
                        (Some(items), Some(len)) if items.len() == len => (items.to_vec(), true),
                        (_, Some(54)) => match self.chest_merged_items(pos.x, pos.y, pos.z) {
                            Some(merged) => (merged, true),
                            None => (Vec::new(), false),
                        },
                        _ => (Vec::new(), false),
                    };
                    if menu_len {
                        match self.sessions.get_mut(&id) {
                            Some(session) => match session.menu.container_mut(0) {
                                Some(container) => {
                                    for (index, stack) in entity_items.iter().enumerate() {
                                        let _ = container.set(index, *stack);
                                    }
                                    true
                                }
                                None => false,
                            },
                            None => false,
                        }
                    } else {
                        false
                    }
                };
                if !refreshed {
                    continue;
                }
                let (contents, state, cursor, window) = match self.sessions.get(&id) {
                    Some(session) => (
                        session
                            .menu
                            .full_contents()
                            .iter()
                            .copied()
                            .map(wire_stack)
                            .collect::<Vec<_>>(),
                        session.menu.state_id(),
                        session.menu.cursor(),
                        i32::from(session.menu.window_id()),
                    ),
                    None => continue,
                };
                let _ = self.send(
                    id,
                    &ContainerSetContent {
                        window_id: window,
                        state_id: state,
                        slots: contents,
                        carried: wire_stack(cursor),
                    },
                    report,
                );
            }
        }
        Ok(())
    }

    /// A hopper's push offset from its `facing` property (P17-02).
    ///
    /// Down pushes below, horizontal facings push sideways (pumpkin ejects
    /// into `position.offset(facing)`). Unreadable states fail safe to below
    /// — the pre-P17-02 behaviour — rather than refusing the transfer.
    fn hopper_push_offset(&self, pos: mc_container::BlockPos) -> (i32, i32, i32) {
        let facing = self
            .world
            .get_block_loaded(pos.x, pos.y, pos.z)
            .and_then(|id| self.registries.blocks.properties_of(id).ok())
            .and_then(|props| {
                props
                    .iter()
                    .find(|(key, _)| key == "facing")
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_else(|| "down".to_owned());
        if facing == "down" {
            (0, -1, 0)
        } else {
            let (fx, fy, fz) = mc_redstone::blocks::facing_offset(&facing);
            if fy != 0 { (0, -1, 0) } else { (fx, 0, fz) }
        }
    }

    /// Push one item from a hopper into a furnace, face-aware (P17-02).
    ///
    /// `from_top` (hopper directly above) feeds the input slot with smeltable
    /// items only; a side entry feeds the fuel slot with fuels only — vanilla's
    /// sided rule, checked per item against the pack smelting table and the
    /// jar-verified fuel table, so coal from above stays put and food from the
    /// side stays put. Hopper slots are tried in order; the first valid stack
    /// with room in the target moves one item. Unknown items (failed lookups)
    /// fail safe as "not valid" rather than as fuel.
    fn hopper_feed_furnace(
        &mut self,
        hopper: mc_container::BlockPos,
        furnace: mc_container::BlockPos,
        from_top: bool,
    ) -> bool {
        // Slot 0 on top entry, slot 1 on side entry.
        let slot = usize::from(!from_top);
        let Some(hopper_items) = self
            .block_entities
            .get(hopper)
            .and_then(|e| e.data.items())
            .map(<[mc_entity::stack::ItemStack]>::to_vec)
        else {
            return false;
        };
        let Some(furnace_items) = self
            .block_entities
            .get(furnace)
            .and_then(|e| e.data.items())
            .map(<[mc_entity::stack::ItemStack]>::to_vec)
        else {
            return false;
        };
        if furnace_items.len() <= slot {
            return false;
        }
        for (index, stack) in hopper_items.iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            let Some(item_id) = stack.item_id() else {
                continue;
            };
            let valid = if from_top {
                self.smelting_furnace.recipe_for(item_id).is_some()
            } else {
                self.smelting_furnace
                    .burn_ticks_for(&self.registries.items, item_id)
                    .unwrap_or(None)
                    .is_some()
            };
            if !valid {
                continue;
            }
            let dest = furnace_items[slot];
            let room = if dest.is_empty() {
                true
            } else if dest.item_id() == Some(item_id) {
                let limit = self.sessions.values().next().map_or(64, |session| {
                    session
                        .player
                        .inventory
                        .stack_sizes()
                        .max_stack_size(item_id)
                });
                dest.count() < limit
            } else {
                false
            };
            if !room {
                return false;
            }
            let mut taken = *stack;
            let one = taken.split(1);
            if one.is_empty() {
                continue;
            }
            let mut into = dest;
            if into.is_empty() {
                into = one;
            } else {
                into.grow(1);
            }
            if let Some(entity) = self.block_entities.get_mut(hopper)
                && let Some(items) = entity.data.items_mut()
                && let Some(slot_stack) = items.get_mut(index)
            {
                *slot_stack = taken;
            } else {
                return false;
            }
            if let Some(entity) = self.block_entities.get_mut(furnace)
                && let Some(items) = entity.data.items_mut()
                && let Some(slot_stack) = items.get_mut(slot)
            {
                *slot_stack = into;
            } else {
                return false;
            }
            mark_block_dirty(&mut self.world, hopper.x, hopper.z);
            return true;
        }
        false
    }

    /// Pull one item from a furnace's output slot into a hopper (P17-02).
    ///
    /// Input and fuel never leave through a hopper (vanilla sided rule; the
    /// refusal lives in [`mc_container::SlotRole::may_hopper_extract`], this
    /// only supplies the furnace role slice). The hopper half takes anything
    /// with room, exactly like the generic push path.
    fn hopper_take_furnace_output(
        &mut self,
        furnace: mc_container::BlockPos,
        hopper: mc_container::BlockPos,
    ) -> bool {
        let (Some(source_items), Some(dest_items)) = (
            self.block_entities
                .get(furnace)
                .and_then(|e| e.data.items())
                .map(<[mc_entity::stack::ItemStack]>::to_vec),
            self.block_entities
                .get(hopper)
                .and_then(|e| e.data.items())
                .map(<[mc_entity::stack::ItemStack]>::to_vec),
        ) else {
            return false;
        };
        let Ok(mut source) =
            mc_container::Container::new(mc_container::ContainerKind::Generic, source_items.len())
        else {
            return false;
        };
        let Ok(mut dest) =
            mc_container::Container::new(mc_container::ContainerKind::Generic, dest_items.len())
        else {
            return false;
        };
        for (index, stack) in source_items.iter().enumerate() {
            let _ = source.set(index, *stack);
        }
        for (index, stack) in dest_items.iter().enumerate() {
            let _ = dest.set(index, *stack);
        }
        let roles = [
            mc_container::SlotRole::FurnaceInput,
            mc_container::SlotRole::FurnaceFuel,
            mc_container::SlotRole::FurnaceOutput,
        ];
        let dest_roles = vec![mc_container::SlotRole::Storage; dest.len()];
        let Ok(transfer) =
            mc_container::Hopper::transfer(&mut source, &roles, &mut dest, &dest_roles, 1)
        else {
            return false;
        };
        if transfer.moved == 0 {
            return false;
        }
        if let Some(entity) = self.block_entities.get_mut(furnace)
            && let Some(items) = entity.data.items_mut()
        {
            for (index, slot) in items.iter_mut().enumerate() {
                *slot = source.get(index);
            }
        }
        if let Some(entity) = self.block_entities.get_mut(hopper)
            && let Some(items) = entity.data.items_mut()
        {
            for (index, slot) in items.iter_mut().enumerate() {
                *slot = dest.get(index);
            }
        }
        mark_block_dirty(&mut self.world, furnace.x, furnace.z);
        true
    }

    /// Phase 3: tick every non-player entity, ascending by id.
    ///
    /// Infallible today: [`Game::tick_entity`] has no error path left (a refused
    /// world write is logged and skipped). It gains a `ServerResult` when entity AI
    /// lands and starts making fallible world queries.
    fn phase_entities(&mut self, report: &mut TickReport) {
        // Natural spawning runs at the head of the Entities phase, every tick,
        // where vanilla's spawn cycle sits relative to entity ticking.
        self.run_spawn_cycle();
        // Collected first: the loop mutates the store, so it cannot hold the
        // iterator. `ids()` is ascending because the store is a `BTreeMap`.
        let ids: Vec<EntityId> = self.entities.ids().collect();
        for id in ids {
            // The AI hook runs first, as Vanilla orders `tick` before the move. It
            // is a no-op today; see `tick_entity_ai`.
            self.tick_entity_ai(id);
            self.tick_mob_despawn(id);
            if self.tick_entity(id) {
                report.entities_ticked += 1;
            }
            // Arrows check their hits after moving, like items check pickup
            // after theirs: same phase, same ordering rationale.
            self.resolve_projectile_hit(id);
        }
        self.merge_and_collect_items();
    }

    /// Merge stacks and collect them into players (P11-05, P11-09).
    ///
    /// **Merging**: two ground stacks of the same item within half a block
    /// combine into the older entity, up to the item's stack limit (vanilla's
    /// merge rule, radius simplified from the box-overlap shape); the younger
    /// entity keeps whatever did not fit and is removed when empty, which the
    /// sweep announces. **Pickup**: a stack whose pickup delay has expired and
    /// that a ready player's cell overlaps (vanilla's radius, as a 1.0-block
    /// distance) goes into that player's inventory through `add_stack`, whose
    /// leftover stays on the ground when the inventory is full; the client sees
    /// the new slot contents through the inventory re-mirror, and the removal
    /// through the entity sweep.
    fn merge_and_collect_items(&mut self) {
        self.merge_ground_stacks();
        self.collect_items_into_players();
        self.merge_orbs();
        self.collect_orbs_into_players();
    }

    /// Merge adjacent same-item ground stacks into the older entity (P11-09).
    fn merge_ground_stacks(&mut self) {
        let ground: Vec<GroundItem> = self
            .entities
            .iter()
            .filter_map(|entity| {
                let EntityBody::Item(item) = &entity.body else {
                    return None;
                };
                Some(GroundItem {
                    id: entity.id,
                    item: item.item_id(),
                    position: entity.position,
                    age: entity.age,
                    ready: item.pickup_delay == 0,
                })
            })
            .collect();
        // Merging: same item, half a block apart, older survives. One pass per
        // pair is enough at these populations; a full binning pass is worth it
        // only when item counts make the O(n^2) observable.
        for a in 0..ground.len() {
            for b in (a + 1)..ground.len() {
                let (first, second) = (ground[a], ground[b]);
                if first.item.is_none() || first.item != second.item {
                    continue;
                }
                let dx = first.position.x - second.position.x;
                let dy = first.position.y - second.position.y;
                let dz = first.position.z - second.position.z;
                if dx * dx + dy * dy + dz * dz > ITEM_MERGE_RADIUS_SQR {
                    continue;
                }
                let (keep, drop) = if first.age <= second.age {
                    (first.id, second.id)
                } else {
                    (second.id, first.id)
                };
                // The player inventory's own limit table governs merges the
                // same way it governs inserts, so the two cannot disagree.
                let item_id = self
                    .entities
                    .get(keep)
                    .and_then(|entity| match &entity.body {
                        EntityBody::Item(item) => item.item_id(),
                        _ => None,
                    });
                let max_stack = self.sessions.values().next().map_or(
                    mc_entity::stack::DEFAULT_MAX_STACK_SIZE,
                    |session| {
                        item_id.map_or(mc_entity::stack::DEFAULT_MAX_STACK_SIZE, |id| {
                            session.player.inventory.stack_sizes().max_stack_size(id)
                        })
                    },
                );
                // The store cannot hand out two mutable borrows at once, so the
                // younger stack is copied out, merged into the keeper, and the
                // remainder written back.
                let Some(drop_entity) = self.entities.get(drop) else {
                    continue;
                };
                let EntityBody::Item(drop_item) = &drop_entity.body else {
                    continue;
                };
                if drop_item.stack.is_empty() {
                    continue;
                }
                let mut incoming = drop_item.stack;
                let Some(keep_entity) = self.entities.get_mut(keep) else {
                    continue;
                };
                let EntityBody::Item(keep_item) = &mut keep_entity.body else {
                    continue;
                };
                let before = keep_item.stack.count();
                keep_item.stack.merge_capped(&mut incoming, max_stack);
                let _merged = keep_item.stack.count() > before;
                let Some(drop_entity) = self.entities.get_mut(drop) else {
                    continue;
                };
                let EntityBody::Item(drop_item) = &mut drop_entity.body else {
                    continue;
                };
                drop_item.stack = incoming;
                if drop_item.stack.is_empty() {
                    drop_entity.removed = true;
                }
            }
        }
    }

    /// Give every ready, overdue ground stack to the nearest player within the
    /// pickup radius (P11-05), mirroring the new slot contents to that client.
    fn collect_items_into_players(&mut self) {
        let players: Vec<(EntityId, ConnectionId, mc_world::Vec3)> = self
            .sessions
            .values()
            .filter(|session| session.ready)
            .map(|session| (session.entity, session.id, session.player.position))
            .collect();
        let ground: Vec<GroundItem> = self
            .entities
            .iter()
            .filter_map(|entity| {
                let EntityBody::Item(item) = &entity.body else {
                    return None;
                };
                Some(GroundItem {
                    id: entity.id,
                    item: item.item_id(),
                    position: entity.position,
                    age: entity.age,
                    ready: item.pickup_delay == 0,
                })
            })
            .collect();
        for item in ground {
            if !item.ready {
                continue;
            }
            let Some((_, connection, _player_position)) = players
                .iter()
                .copied()
                .min_by(|a, b| {
                    let da = (a.2.x - item.position.x).powi(2)
                        + (a.2.y - item.position.y).powi(2)
                        + (a.2.z - item.position.z).powi(2);
                    let db = (b.2.x - item.position.x).powi(2)
                        + (b.2.y - item.position.y).powi(2)
                        + (b.2.z - item.position.z).powi(2);
                    da.total_cmp(&db)
                })
                .filter(|(_, _, position)| {
                    let dx = position.x - item.position.x;
                    let dy = position.y - item.position.y;
                    let dz = position.z - item.position.z;
                    dx * dx + dy * dy + dz * dz <= ITEM_PICKUP_RADIUS_SQR
                })
            else {
                continue;
            };
            let Some(entity) = self.entities.get_mut(item.id) else {
                continue;
            };
            let EntityBody::Item(ground_stack) = &mut entity.body else {
                continue;
            };
            let stack = ground_stack.stack;
            let leftover = match self.sessions.get_mut(&connection) {
                Some(session) => session.player.inventory.add_stack(stack),
                None => continue,
            };
            if leftover.is_empty() {
                entity.removed = true;
                debug!(item = ?item.item, player = %connection, "item picked up");
            } else {
                let EntityBody::Item(ground_stack) = &mut entity.body else {
                    continue;
                };
                ground_stack.stack = leftover;
            }
            self.sync_menu_from_inventory(connection, &mut TickReport::default());
        }
    }

    /// Merge adjacent orbs into the older entity, summing values (P16-02).
    ///
    /// Same contact scale as drops (half a block); orb-specific merge radii
    /// are unmeasured, and inventing a second constant for the same contact
    /// would be guessing. Values only grow, so no cap applies.
    fn merge_orbs(&mut self) {
        let orbs: Vec<(EntityId, i32, mc_world::Vec3, u32)> = self
            .entities
            .iter()
            .filter_map(|entity| match &entity.body {
                EntityBody::Orb(orb) => Some((entity.id, orb.value, entity.position, orb.age)),
                _ => None,
            })
            .collect();
        for a in 0..orbs.len() {
            for b in (a + 1)..orbs.len() {
                let (first, second) = (orbs[a], orbs[b]);
                let dx = first.2.x - second.2.x;
                let dy = first.2.y - second.2.y;
                let dz = first.2.z - second.2.z;
                if dx * dx + dy * dy + dz * dz > ITEM_MERGE_RADIUS_SQR {
                    continue;
                }
                let (keep, drop) = if first.3 <= second.3 {
                    (first.0, second.0)
                } else {
                    (second.0, first.0)
                };
                let Some(drop_entity) = self.entities.get(drop) else {
                    continue;
                };
                let EntityBody::Orb(drop_orb) = &drop_entity.body else {
                    continue;
                };
                let incoming = drop_orb.value;
                let Some(keep_entity) = self.entities.get_mut(keep) else {
                    continue;
                };
                let EntityBody::Orb(keep_orb) = &mut keep_entity.body else {
                    continue;
                };
                keep_orb.value = keep_orb.value.saturating_add(incoming);
                let Some(drop_entity) = self.entities.get_mut(drop) else {
                    continue;
                };
                drop_entity.removed = true;
            }
        }
    }

    /// Give every orb a ready player overlaps to that player (P16-02).
    ///
    /// Same one-block reach as drops. Orbs carry no pickup delay of their
    /// own; instead each picker throttles to one orb per two ticks (vanilla's
    /// per-player rule), and a pickup pushes vitals so the client sees the
    /// new level at once.
    fn collect_orbs_into_players(&mut self) {
        let players: Vec<(EntityId, ConnectionId, mc_world::Vec3)> = self
            .sessions
            .values()
            .filter(|session| session.ready)
            .map(|session| (session.entity, session.id, session.player.position))
            .collect();
        let orbs: Vec<(EntityId, i32, mc_world::Vec3)> = self
            .entities
            .iter()
            .filter_map(|entity| match &entity.body {
                EntityBody::Orb(orb) => Some((entity.id, orb.value, entity.position)),
                _ => None,
            })
            .collect();
        for orb in orbs {
            let Some((_, connection, _)) = players
                .iter()
                .copied()
                .min_by(|a, b| {
                    let da = (a.2.x - orb.2.x).powi(2)
                        + (a.2.y - orb.2.y).powi(2)
                        + (a.2.z - orb.2.z).powi(2);
                    let db = (b.2.x - orb.2.x).powi(2)
                        + (b.2.y - orb.2.y).powi(2)
                        + (b.2.z - orb.2.z).powi(2);
                    da.total_cmp(&db)
                })
                .filter(|(_, _, position)| {
                    let dx = position.x - orb.2.x;
                    let dy = position.y - orb.2.y;
                    let dz = position.z - orb.2.z;
                    dx * dx + dy * dy + dz * dz <= ITEM_PICKUP_RADIUS_SQR
                })
            else {
                continue;
            };
            let Some(session) = self.sessions.get_mut(&connection) else {
                continue;
            };
            if session.xp_pickup_cooldown > 0 {
                continue;
            }
            session.xp_pickup_cooldown = 2;
            let gained = session.player.add_experience(orb.1);
            debug!(player = %connection, value = orb.1, gained, "experience orb picked up");
            let Some(entity) = self.entities.get_mut(orb.0) else {
                continue;
            };
            entity.removed = true;
            let _ = self.send_vitals(connection, &mut TickReport::default());
        }
    }

    /// The natural spawn cycle (P11-01): sampled candidate positions around
    /// each player, the measured light/solid/biome rules, and the category
    /// caps. The rules and every constant's provenance live in
    /// [`crate::spawn`]; this is the orchestration only.
    ///
    /// Spawned mobs go through [`Game::pending_entity_spawns`] like drops, so
    /// the Broadcast phase announces them with their per-type registry id and
    /// their spawn health.
    fn run_spawn_cycle(&mut self) {
        let players: Vec<mc_world::Vec3> = self
            .sessions
            .values()
            .filter(|session| session.ready)
            .map(|session| session.player.position)
            .collect();
        if players.is_empty() {
            return;
        }
        let darken = crate::spawn::sky_darken(crate::spawn::time_of_day(
            self.tick as i64,
            self.time_offset,
        ));
        // Per-category counts over the world; the cap scope is a named
        // simplification (see `crate::spawn`'s module docs).
        let mut counts = [0_i32; 2];
        for entity in self.entities.iter() {
            if let EntityBody::Mob(mob) = &entity.body {
                counts[crate::spawn::category_of(mob.kind) as usize] += 1;
            }
        }
        // Vanilla sweeps the whole chunk ring every tick; so does this, with
        // the per-position randomness in the block coordinates.
        let half = crate::spawn::SPAWN_DISTANCE_CHUNK;
        for player in &players {
            let player_chunk_x = (player.x as i64).div_euclid(16) as i32;
            let player_chunk_z = (player.z as i64).div_euclid(16) as i32;
            for dx in -half..=half {
                for dz in -half..=half {
                    let chunk_x = player_chunk_x + dx;
                    let chunk_z = player_chunk_z + dz;
                    let x = chunk_x * 16 + self.random.next_i32_bounded(16);
                    let z = chunk_z * 16 + self.random.next_i32_bounded(16);
                    let centre = (f64::from(x) + 0.5, f64::from(z) + 0.5);
                    if players.iter().any(|other| {
                        let ox = other.x - centre.0;
                        let oz = other.z - centre.1;
                        ox * ox + oz * oz < crate::spawn::MIN_SPAWN_DISTANCE_SQR
                    }) {
                        continue;
                    }
                    let bounds = {
                        let Some(chunk) =
                            self.world.chunk(mc_world::ChunkPos::new(chunk_x, chunk_z))
                        else {
                            continue;
                        };
                        (chunk.min_y(), chunk.sections.len() as i32 * 16)
                    };
                    let y = bounds.0
                        + self
                            .random
                            .next_i32_bounded(bounds.1.saturating_sub(2).max(1));
                    self.try_spawn_pack(x, y, z, darken, &mut counts);
                }
            }
        }
    }

    /// Validate one candidate position and, if its rules pass, spawn a pack of
    /// the biome table's chosen kind, updating the live cap counts.
    ///
    /// The counts thread through as `&mut`, so a cap reached mid-cycle blocks
    /// later attempts in the *same* tick — vanilla updates its spawn state
    /// after every pack, and a by-value copy would let one tick's 289
    /// positions each land a pack before the next count.
    fn try_spawn_pack(&mut self, x: i32, y: i32, z: i32, darken: i32, counts: &mut [i32; 2]) {
        // A solid block below and two air cells to stand in.
        let below = self.world.get_block(x, y - 1, z);
        let feet = self.world.get_block(x, y, z);
        let head = self.world.get_block(x, y + 1, z);
        if !mc_world::collision::is_solid_or_unknown(&self.registries.blocks, below)
            || feet != 0
            || head != 0
        {
            return;
        }
        // The biome's tables decide category and kind. A biome with no modeled
        // rows in its category (the ocean's creature list, say) rejects here,
        // as does an unloaded generator.
        // One biome for the whole world is what the generator models
        // (`PLAINS_BIOME_ID`'s docs), so the tables are keyed once; when
        // per-column biomes land this lookup moves to the position.
        let Some(generator) = self.generator.as_ref() else {
            return;
        };
        let biome_name = mc_worldgen::terrain::ChunkGenerator::biome_at(generator, x, z).id();
        let Some(tables) = self.spawn_tables.biomes.get(biome_name) else {
            return;
        };
        // Vanilla attempts **each category independently** per position
        // (`spawnCategoryForPosition` runs for every `SPAWNING_CATEGORIES`
        // entry), so both rows are drawn up front: the choice phase reads only
        // the tables and the RNG, and the light and spawn work below needs
        // `self` mutable, so the table borrow has to end first. A category at
        // its cap, or with no modeled row in this biome, draws nothing.
        let picked = {
            let mut monster = None;
            let mut creature = None;
            if counts[crate::spawn::MobCategory::Monster as usize]
                < crate::spawn::MobCategory::Monster.max_instances()
            {
                monster = tables.pick(crate::spawn::MobCategory::Monster, &mut self.random);
            }
            if counts[crate::spawn::MobCategory::Creature as usize]
                < crate::spawn::MobCategory::Creature.max_instances()
            {
                creature = tables.pick(crate::spawn::MobCategory::Creature, &mut self.random);
            }
            (monster.copied(), creature.copied())
        };
        let (monster, creature) = picked;
        {
            let Some((block_light, sky_light)) = self.light_at(x, y, z) else {
                return;
            };
            // Monsters first, vanilla's category order; the creature row is
            // the fallback when the monster rule rejects the position.
            if let Some(row) = monster
                && !matches!(self.difficulty, Difficulty::Peaceful)
                && crate::spawn::monster_spawn_allowed(
                    block_light,
                    sky_light,
                    darken,
                    &mut self.random,
                )
            {
                counts[crate::spawn::MobCategory::Monster as usize] += 1;
                self.spawn_pack(row, x, y, z);
                return;
            }
            if let Some(row) = creature {
                // The animal rule reads raw brightness with no darkening, and
                // the tag below the position is grass only.
                let below_name = self.registries.blocks.block_name(below).unwrap_or_default();
                if crate::spawn::animal_spawn_allowed(
                    block_light.max(sky_light),
                    below_name == "minecraft:grass_block",
                ) {
                    counts[crate::spawn::MobCategory::Creature as usize] += 1;
                    self.spawn_pack(row, x, y, z);
                }
            }
        }
    }

    /// Spawn one pack on the already-validated cell.
    ///
    /// Vanilla spreads the members over nearby cells and re-validates each;
    /// this build lands every member on the one validated cell, which keeps
    /// the position accounting honest until P11-02's movement spreads them.
    fn spawn_pack(&mut self, row: crate::spawn::SpawnRow, x: i32, y: i32, z: i32) {
        let span = (row.max_count - row.min_count + 1) as i32;
        let count = row.min_count as i32 + self.random.next_i32_bounded(span);
        for _ in 0..count {
            let position =
                mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
            let Ok(id) = self
                .entities
                .spawn(EntityBody::Mob(Mob::new(row.kind)), position)
            else {
                return;
            };
            self.pending_entity_spawns.push(id);
        }
    }

    /// Sky and block light at one world position, from the chunk's cached
    /// computation. A chunk without light computes it; a chunk that cannot be
    /// lit reports absence rather than guessing zero.
    fn light_at(&mut self, x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
        let pos = mc_world::ChunkPos::new(x >> 4, z >> 4);
        if self.world.cached_light(pos).is_none() {
            self.world.compute_light(pos, &self.registries.light).ok()?;
        }
        let light = self.world.cached_light(pos)?;
        let min_y = {
            let chunk = self.world.chunk(pos)?;
            chunk.min_y()
        };
        let section = usize::try_from((y - min_y).div_euclid(16)).ok()?;
        let sky = light.sky.get(section)?.get(x & 15, y & 15, z & 15);
        let block = light.block.get(section)?.get(x & 15, y & 15, z & 15);
        Some((block, sky))
    }

    /// The despawn pass for one mob, every tick (`Mob.checkDespawn`).
    ///
    /// The idle counter climbs outside the no-despawn ring and resets inside
    /// it; the distance rule applies to monsters only, because creatures are
    /// persistent (`Animal.removeWhenFarAway` returns false). Items and
    /// players are not mobs and are skipped.
    fn tick_mob_despawn(&mut self, id: EntityId) {
        let Some(entity) = self.entities.get(id) else {
            return;
        };
        if entity.removed {
            return;
        }
        let EntityBody::Mob(mob) = &entity.body else {
            return;
        };
        let category = crate::spawn::category_of(mob.kind);
        let position = entity.position;
        let Some(distance_sq) = self.nearest_player_distance_sq(position) else {
            return;
        };
        let Some(entity) = self.entities.get_mut(id) else {
            return;
        };
        let EntityBody::Mob(mob) = &mut entity.body else {
            return;
        };
        if distance_sq < crate::spawn::NO_DESPAWN_DISTANCE_SQR {
            mob.no_action_ticks = 0;
            return;
        }
        // The increment lives in the AI hook (goal activity resets the counter
        // there); this pass only reads it and applies the roll.
        let no_action = mob.no_action_ticks;
        if crate::spawn::despawn(category, distance_sq, no_action, &mut self.random) {
            entity.removed = true;
        }
    }

    /// Squared distance from `position` to the nearest ready player, if any.
    fn nearest_player_distance_sq(&self, position: mc_world::Vec3) -> Option<f64> {
        self.sessions
            .values()
            .filter(|session| session.ready)
            .map(|session| {
                let p = session.player.position;
                let dx = p.x - position.x;
                let dz = p.z - position.z;
                dx * dx + dz * dz
            })
            .min_by(f64::total_cmp)
    }

    /// Per-entity AI and behaviour — live as of P11-02.
    ///
    /// The AI itself is `mc_entity`'s [`MobAi`], a pure function of its own
    /// state and an observation; this hook builds the observation, calls
    /// [`MobAi::decide`] **once per tick per mob in ascending entity id** (the
    /// loop's order, which the AI's calling convention requires), and resolves
    /// the goal: horizontal steering for movement, a melee swing for an
    /// in-range [`MobGoal::Attack`], and the idle counter the despawn pass
    /// reads.
    ///
    /// [`MobGoal::Attack`] intents dispatch by [`MobAttackStyle`]: melee
    /// swings, skeleton bow shots, and the creeper fuse (P16-04). The
    /// creeper's damage figure stays the explosion value, never a melee one
    /// (see `mob.rs`'s gap list).
    fn tick_entity_ai(&mut self, id: EntityId) {
        // Observation first: these read through `&self`, so no entity borrow is
        // live when `decide` needs `&mut self.entities`.
        let Some(entity) = self.entities.get(id) else {
            return;
        };
        if entity.removed {
            return;
        }
        let EntityBody::Mob(mob) = &entity.body else {
            return;
        };
        let kind = mob.kind;
        let position = entity.position;
        let health_fraction = if kind.max_health() > 0.0 {
            entity.health / kind.max_health()
        } else {
            1.0
        };
        let sighting = self
            .nearest_ready_player(position)
            .map(|(player, distance)| MobSighting::new(player, distance));
        let observation = MobObservation::new(
            (
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            ),
            sighting,
            health_fraction,
            self.tick,
        );
        // `self.entities` and `self.random` are disjoint fields, so the AI can
        // draw from the game's own source while mutating the mob.
        let goal = {
            let Some(entity) = self.entities.get_mut(id) else {
                return;
            };
            let EntityBody::Mob(mob) = &mut entity.body else {
                return;
            };
            let mut rng = AiRng(&mut self.random);
            mob.ai.decide(kind, observation, &mut rng)
        };
        self.resolve_mob_goal(id, kind, goal, position);
    }

    /// The nearest ready player as `(entity id, distance in blocks)`, or `None`.
    ///
    /// Three-dimensional: the AI's radii are block distances and the mob can be
    /// above or below its target. The AI applies its own range filtering, so the
    /// true nearest player is passed regardless of distance.
    fn nearest_ready_player(&self, position: mc_world::Vec3) -> Option<(EntityId, f64)> {
        self.sessions
            .values()
            .filter(|session| session.ready)
            .map(|session| {
                let p = session.player.position;
                let dx = p.x - position.x;
                let dy = p.y - position.y;
                let dz = p.z - position.z;
                (session.entity, (dx * dx + dy * dy + dz * dz).sqrt())
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Steer a chase along its A* path, head-first.
    ///
    /// Computes a path when the current one is empty and falls back to direct
    /// steering when the search fails (P16-04).
    ///
    /// Waypoints are feet cells; a waypoint within 0.6 blocks is consumed.
    /// A path whose final cell drifted more than two blocks from the target's
    /// cell is dropped, so a kiting target re-searches instead of trailing
    /// stale cells. The per-step passability check downstream still applies,
    /// so a path through a freshly placed block degrades to standing, not to
    /// phasing.
    fn chase_steer(
        &mut self,
        id: EntityId,
        kind: MobKind,
        position: mc_world::Vec3,
        target: mc_world::Vec3,
    ) -> (f64, f64, i32) {
        let from = (
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        );
        let goal = (
            target.x.floor() as i32,
            target.y.floor() as i32,
            target.z.floor() as i32,
        );
        let mut direct = (target.x - position.x, target.z - position.z, 0);
        let Some(entity) = self.entities.get_mut(id) else {
            return direct;
        };
        let EntityBody::Mob(mob) = &mut entity.body else {
            return direct;
        };
        while let Some(&(wx, _, wz)) = mob.ai.path.first() {
            let dx = f64::from(wx) + 0.5 - position.x;
            let dz = f64::from(wz) + 0.5 - position.z;
            if dx * dx + dz * dz < 0.36 {
                mob.ai.path.remove(0);
            } else {
                break;
            }
        }
        if let Some(&(lx, ly, lz)) = mob.ai.path.last() {
            let drift = (lx - goal.0).abs() + (ly - goal.1).abs() + (lz - goal.2).abs();
            if drift > 2 {
                mob.ai.path.clear();
            }
        }
        if mob.ai.path.is_empty() {
            let view = WorldView(&self.world);
            let mut limits = SearchLimits::new();
            limits.clearance = kind.clearance();
            if let Some(path) = find_path(&view, from, goal, limits) {
                mob.ai.path = path;
            }
        }
        if let Some(&(wx, _, wz)) = mob.ai.path.first() {
            direct = (
                f64::from(wx) + 0.5 - position.x,
                f64::from(wz) + 0.5 - position.z,
                0,
            );
        }
        direct
    }

    /// Act on the goal `decide` returned.
    ///
    /// The idle counter climbs on an [`MobGoal::Idle`] and resets on any other
    /// goal — the meaning P11-01's despawn pass documented for it once goal
    /// activity existed. Movement is **direct steering**, except for
    /// [`MobGoal::Chase`], which follows an A* path when the search finds one
    /// (P16-04): the horizontal velocity points at the goal at the kind's walk
    /// speed and the yaw faces it, with the existing physics phase integrating
    /// and colliding.
    ///
    /// ## The one-cell lookahead (M-4)
    ///
    /// Steering is still direct for wander and flee — **that is not
    /// pathfinding** — but it is no longer blind. Before a steer is applied, the cell the mob's centre would reach after
    /// [`MOB_LOOKAHEAD_BLOCKS`] at that heading is checked for passability
    /// ([`Self::mob_step_is_passable`]): a solid block at the mob's feet or head, or
    /// a fluid, refuses the step. The owner's acceptance round saw mobs walk into
    /// water and grind through walls, and this is the cheap half of that
    /// complaint —
    ///
    /// * a **blocked wander stops and re-targets**: the walk is abandoned, so the
    ///   AI rolls a new destination at its next decision boundary instead of
    ///   pressing into the same wall until the walk's timer expires;
    /// * a **blocked flee just stops**: it has nothing to re-target
    ///   towards, and grinding at a wall while facing the player is worse than
    ///   standing still (a blocked chase searches instead — see below).
    ///
    /// What it deliberately does **not** do, so that the gap does not read as
    /// solved:
    ///
    /// * no path around an obstacle for wander/flee — a wall between a
    ///   wandering mob and its target stops it, it does not route it (chase
    ///   alone searches);
    /// * no ledge or fall handling. Refusing a step with no ground under it would
    ///   stop mobs walking down any hill, which vanilla mobs do constantly, so the
    ///   check only refuses *occupancy*, not *support*;
    /// * no step-up assist beyond what the collision pass already does, and no
    ///   diagonal corner negotiation (the check is one cell at the heading, not the
    ///   swept body).
    fn resolve_mob_goal(
        &mut self,
        id: EntityId,
        kind: MobKind,
        goal: MobGoal,
        position: mc_world::Vec3,
    ) {
        {
            let Some(entity) = self.entities.get_mut(id) else {
                return;
            };
            let EntityBody::Mob(mob) = &mut entity.body else {
                return;
            };
            if matches!(goal, MobGoal::Idle) {
                mob.no_action_ticks += 1;
            } else {
                mob.no_action_ticks = 0;
            }
        }
        // Horizontal steering as `(dx, dz)`, normalised by the caller of the
        // velocity write; `None` stands still.
        let steer: Option<(f64, f64, i32)> = match &goal {
            // Idle stands; an in-range attack also stands and swings.
            MobGoal::Idle | MobGoal::Attack { .. } => None,
            MobGoal::Wander { target } => Some((
                f64::from(target.0) + 0.5 - position.x,
                f64::from(target.2) + 0.5 - position.z,
                0,
            )),
            MobGoal::Chase { target } => {
                let Some(other) = self.entities.get(*target) else {
                    return;
                };
                // A gone target is the end of the goal this tick; `decide`
                // re-chooses next tick.
                let target_pos = other.position;
                Some(self.chase_steer(id, kind, position, target_pos))
            }
            MobGoal::Flee { from: target } => {
                let Some(other) = self.entities.get(*target) else {
                    return;
                };
                // Fleeing has no destination to path to; direct steering
                // stands, as documented above.
                Some((
                    (position.x - other.position.x),
                    (position.z - other.position.z),
                    0,
                ))
            }
        };
        if let Some((dx, dz, _)) = steer {
            let length = (dx * dx + dz * dz).sqrt();
            if length > 1.0e-6 {
                let (dir_x, dir_z) = (dx / length, dz / length);
                let speed = kind.movement_speed();
                // Vanilla's yaw convention: 0 faces +Z, increasing clockwise,
                // so `yaw = degrees(atan2(-x, z))` — pinned by a unit test.
                let yaw = (-dir_x).atan2(dir_z).to_degrees();
                if self.mob_step_is_passable(position, dir_x, dir_z) {
                    if let Some(entity) = self.entities.get_mut(id) {
                        entity.velocity.x = dir_x * speed;
                        entity.velocity.z = dir_z * speed;
                        entity.yaw = yaw as f32;
                    }
                } else {
                    // M-4: the step is refused, so the mob stops this tick. A
                    // wander additionally abandons its destination, which is what
                    // makes "re-target" happen rather than "stand still for the
                    // rest of the walk's timer".
                    debug!(%id, %kind, "mob step blocked; stopping");
                    let re_target = matches!(goal, MobGoal::Wander { .. });
                    if let Some(entity) = self.entities.get_mut(id) {
                        entity.velocity.x = 0.0;
                        entity.velocity.z = 0.0;
                        if re_target && let EntityBody::Mob(mob) = &mut entity.body {
                            mob.ai.wander_target = None;
                            mob.ai.cooldown = 0;
                            mob.ai.goal = MobGoal::Idle;
                        }
                    }
                    if re_target {
                        // Nothing to swing at: the goal the caller would act on is
                        // the walk the AI just gave up on, so the attack arm below
                        // must not fire for it.
                        return;
                    }
                }
            }
        } else if matches!(goal, MobGoal::Idle) {
            // An idle mob halts; its friction then does the rest.
            if let Some(entity) = self.entities.get_mut(id) {
                entity.velocity.x = 0.0;
                entity.velocity.z = 0.0;
            }
        }
        if let MobGoal::Attack {
            target,
            cooldown: 0,
        } = goal
        {
            // Style dispatch lives here so each arm reads as its own rule;
            // `resolve_mob_melee` keeps its internal melee check as
            // defense in depth.
            match kind.attack_style() {
                Some(MobAttackStyle::Melee) => {
                    self.resolve_mob_melee(id, kind, target, position);
                }
                Some(MobAttackStyle::Ranged) => {
                    self.resolve_mob_ranged(id, kind, target, position);
                }
                Some(MobAttackStyle::Explosive) | None => {}
            }
        }
        // The fuse runs every tick regardless of goal: a creeper lights at
        // three blocks even before the 2-block Attack goal engages.
        self.tick_creeper_fuse(id, kind, position);
    }

    /// Whether a mob standing at `position` and heading `(dir_x, dir_z)` may take
    /// this tick's step (M-4).
    ///
    /// The probe is **one cell at the heading**: the block position the mob's
    /// centre would occupy after [`MOB_LOOKAHEAD_BLOCKS`], tested at the mob's feet
    /// and at head height. A step is refused when either level is a solid block or
    /// a fluid.
    ///
    /// Three deliberate properties:
    ///
    /// * **The mob's own cell is not a verdict.** When the lookahead lands in the
    ///   cell the mob already occupies there is no information in it — and a mob
    ///   that has already waded into water would otherwise be frozen there for
    ///   ever. A same-cell probe returns `true` and the check simply applies once
    ///   the mob has moved far enough to look into the next cell.
    /// * **An unloaded chunk is impassable.** [`mc_world::World::get_block_loaded`]
    ///   returns `None` for a chunk nobody has loaded; treating that as air would
    ///   walk mobs into a void the collision pass cannot resolve.
    /// * **A fluid is impassable, not fatal.** Nothing here pushes a mob *out* of
    ///   water it is already in: this refuses to *enter*, which is the whole of
    ///   M-4's water half.
    fn mob_step_is_passable(&self, position: mc_world::Vec3, dir_x: f64, dir_z: f64) -> bool {
        let here = (
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        );
        let ahead = (
            (position.x + dir_x * MOB_LOOKAHEAD_BLOCKS).floor() as i32,
            here.1,
            (position.z + dir_z * MOB_LOOKAHEAD_BLOCKS).floor() as i32,
        );
        if (ahead.0, ahead.2) == (here.0, here.2) {
            // Nothing ahead of us yet.
            return true;
        }
        for y in [ahead.1, ahead.1 + 1] {
            let Some(block) = self.world.get_block_loaded(ahead.0, y, ahead.2) else {
                debug!(
                    x = ahead.0,
                    y,
                    z = ahead.2,
                    "mob step blocked by an unloaded chunk"
                );
                return false;
            };
            if mc_world::collision::is_solid_or_unknown(&self.registries.blocks, block) {
                return false;
            }
            if mc_world::collision::is_liquid(&self.registries.blocks, block) {
                debug!(x = ahead.0, y, z = ahead.2, "mob step blocked by a fluid");
                return false;
            }
        }
        true
    }

    /// Resolve one bow shot against a player (P16-04).
    ///
    /// Skeletons only (the one Ranged kind): within 15 blocks with line of
    /// sight, an arrow flies at 1.6 toward the player's torso with
    /// difficulty-scaled spread, dealing power-scaled damage on contact. The
    /// Attack cooldown (20 ticks) paces shots; a blind or distant skeleton
    /// holds fire rather than wasting arrows. Draw timing is folded into the
    /// cooldown (vanilla draws ~20 ticks, then fires on roughly the same
    /// rhythm), and criticals/arrow pickup stay unmodelled.
    fn resolve_mob_ranged(
        &mut self,
        id: EntityId,
        kind: MobKind,
        target: EntityId,
        position: mc_world::Vec3,
    ) {
        // The eye the arrow leaves from (skeleton torso height, documented
        // approximation — exact per-kind eyes are unmodelled).
        const SHOOTER_EYE: f64 = 1.5;
        // Bow range, one flat number (pumpkin `BowAttackGoal::new` range).
        const BOW_RANGE: f64 = 15.0;
        // Cooldown between shots in ticks (same source).
        const BOW_COOLDOWN: u32 = 20;
        // Arrow speed in blocks per tick (same source).
        const ARROW_SPEED: f64 = 1.6;
        let Some(session_id) = self
            .sessions
            .values()
            .find(|session| session.entity == target)
            .map(|session| session.id)
        else {
            return;
        };
        let (target_pos, target_eye) = match self.sessions.get(&session_id) {
            Some(session) => {
                let p = session.player.position;
                (p, p.y + 0.6)
            }
            None => return,
        };
        let dx = target_pos.x - position.x;
        let dy = target_eye - (position.y + SHOOTER_EYE);
        let dz = target_pos.z - position.z;
        let horizontal = (dx * dx + dz * dz).sqrt();
        if horizontal > BOW_RANGE {
            return;
        }
        let from = Vec3::new(position.x, position.y + SHOOTER_EYE, position.z);
        let to = Vec3::new(target_pos.x, target_eye, target_pos.z);
        if !self.world.has_line_of_sight(from, to) {
            return;
        }
        // Damage: power-scaled with a triangular draw, mirroring vanilla
        // `AbstractSkeleton::performRangedAttack` via pumpkin
        // (`set_base_damage_from_mob`): `power * 2 + triangular(diff * 0.11,
        // 0.57425)`, drawn from the seeded source for determinism.
        let difficulty = self.difficulty.legacy_id() as f64;
        let r1 = self.random.next_f64();
        let r2 = self.random.next_f64();
        let damage = (ARROW_SPEED * 2.0 + (difficulty * 0.11 + (r1 - r2) * 0.57425)) as f32;
        // Spread: per-axis triangular deflection scaled by
        // `14 - difficulty * 4` (same source), then normalized to speed.
        let divergence = 14.0 - difficulty * 4.0;
        let r3 = self.random.next_f64();
        let r4 = self.random.next_f64();
        let r5 = self.random.next_f64();
        let r6 = self.random.next_f64();
        let r7 = self.random.next_f64();
        let r8 = self.random.next_f64();
        let jitter = 0.0075 * divergence;
        let mut vx = dx + (r3 - r4) * jitter;
        let mut vy = horizontal.mul_add(0.2, dy) + (r5 - r6) * jitter;
        let mut vz = dz + (r7 - r8) * jitter;
        let len = (vx * vx + vy * vy + vz * vz).sqrt().max(1e-6);
        vx = vx / len * ARROW_SPEED;
        vy = vy / len * ARROW_SPEED;
        vz = vz / len * ARROW_SPEED;
        let yaw = (-vx).atan2(vz).to_degrees() as f32;
        let pitch = (-vy / (vx * vx + vz * vz).sqrt().max(1e-6))
            .atan()
            .to_degrees() as f32;
        let Ok(arrow) = self.entities.spawn(
            EntityBody::Projectile(mc_entity::Projectile {
                kind: mc_entity::ProjectileKind::Arrow,
                owner: Some(id),
                life: mc_entity::ProjectileKind::Arrow.max_lifetime(),
                damage,
            }),
            from,
        ) else {
            return;
        };
        if let Some(entity) = self.entities.get_mut(arrow) {
            entity.velocity = Vec3::new(vx, vy, vz);
            entity.yaw = yaw;
            entity.pitch = pitch;
        }
        self.pending_entity_spawns.push(arrow);
        // The bow is spent for this cycle whether or not anything was hit.
        if let Some(entity) = self.entities.get_mut(id)
            && let EntityBody::Mob(mob) = &mut entity.body
            && kind == mob.kind
        {
            mob.ai.goal = MobGoal::Attack {
                target,
                cooldown: BOW_COOLDOWN,
            };
        }
        debug!(%id, target = %target, damage, "skeleton looses an arrow");
    }

    /// Resolve one flying arrow's hits after its move (P16-04).
    ///
    /// Arrows only (snowballs stay harmless): the arrow's lifetime counts
    /// down and it despawns at zero; an arrow inside a solid block sticks
    /// and despawns without further hits (a one-tick simplification of the
    /// stuck-arrow state). Otherwise the first overlapping victim takes the
    /// arrow's release-time damage through [`DamageSource::Arrow`] — players
    /// through the authoritative `Player` plus vitals, mobs through
    /// [`Self::damage_entity`] with the shooter as the knockback attacker —
    /// and the arrow is consumed. Owner immunity holds: a shot never hits
    /// the mob that loosed it. A victim inside its hurt window is skipped
    /// and the arrow flies on rather than being eaten by invulnerability.
    /// Flight integration stays the shared entity gravity: the per-kind
    /// drag/gravity model in `mc_entity::projectile` is not what moves
    /// arrows yet (approximation, stated).
    fn resolve_projectile_hit(&mut self, id: EntityId) {
        let (position, damage, owner, is_arrow) = match self.entities.get(id) {
            Some(entity) => match &entity.body {
                EntityBody::Projectile(projectile) => (
                    entity.position,
                    projectile.damage,
                    projectile.owner,
                    projectile.kind == mc_entity::ProjectileKind::Arrow,
                ),
                _ => return,
            },
            None => return,
        };
        if !is_arrow {
            return;
        }
        if self.tick_arrow_lifetime(id, position) {
            return;
        }
        if self.hit_player_with_arrow(id, position, damage, owner) {
            return;
        }
        self.hit_mob_with_arrow(id, position, damage, owner);
    }

    /// One lifetime tick for an arrow: expiry or burial removes it.
    ///
    /// Returns `true` when the arrow was removed and the caller is done.
    fn tick_arrow_lifetime(&mut self, id: EntityId, position: mc_world::Vec3) -> bool {
        let stuck_or_spent = match self.entities.get_mut(id) {
            Some(entity) => {
                let EntityBody::Projectile(projectile) = &mut entity.body else {
                    return true;
                };
                projectile.tick();
                if projectile.is_expired() {
                    true
                } else {
                    let cell = (
                        position.x.floor() as i32,
                        position.y.floor() as i32,
                        position.z.floor() as i32,
                    );
                    self.world
                        .get_block_loaded(cell.0, cell.1, cell.2)
                        .is_some_and(|block| {
                            mc_world::collision::is_solid_or_unknown(&self.registries.blocks, block)
                        })
                }
            }
            None => return true,
        };
        if stuck_or_spent {
            if let Some(entity) = self.entities.get_mut(id) {
                entity.removed = true;
            }
            return true;
        }
        false
    }

    /// Arrow-vs-player hit test: first overlapping ready session takes the
    /// damage through the authoritative `Player` plus vitals.
    ///
    /// Returns `true` when an arrow was consumed by a hit (or skipped past a
    /// hurt window, in which case the arrow flies on and the caller is still
    /// done for this tick).
    fn hit_player_with_arrow(
        &mut self,
        id: EntityId,
        position: mc_world::Vec3,
        damage: f32,
        owner: Option<EntityId>,
    ) -> bool {
        // The skeleton arm only targets players, so the player hit is the
        // common case and the falsification probe's shape.
        let hit_player = self
            .sessions
            .values()
            .filter(|session| Some(session.entity) != owner && session.ready)
            .map(|session| (session.id, session.entity, session.player.position))
            .find(|(_, _, at)| {
                mc_world::collision::Aabb::player(*at).distance_to_sqr(position) == 0.0
            });
        let Some((session_id, _, _)) = hit_player else {
            return false;
        };
        if self
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.hurt_invuln_ticks > 0)
        {
            return true;
        }
        let outcome = self.sessions.get_mut(&session_id).map(|session| {
            session.hurt_invuln_ticks = INVULNERABLE_TICKS;
            let armor = worn_stats(&session.player.inventory, &self.registries.items);
            session
                .player
                .apply_damage(damage, DamageSource::Arrow, &armor)
        });
        if let Some(outcome) = outcome {
            if outcome.applied {
                debug!(arrow = %id, dealt = outcome.dealt, "arrow hit a player");
            }
            self.after_damage(session_id, outcome);
        }
        if let Some(entity) = self.entities.get_mut(id) {
            entity.removed = true;
        }
        true
    }

    /// Arrow-vs-mob hit test (friendly fire is real: a skeleton can hit a
    /// zombie) through [`Self::damage_entity`] with the shooter as the
    /// knockback attacker.
    fn hit_mob_with_arrow(
        &mut self,
        id: EntityId,
        position: mc_world::Vec3,
        damage: f32,
        owner: Option<EntityId>,
    ) {
        let hit_mob = self
            .entities
            .iter()
            .filter(|entity| {
                entity.id != id
                    && Some(entity.id) != owner
                    && !entity.removed
                    && matches!(&entity.body, EntityBody::Mob(_))
            })
            .find(|entity| entity.hitbox().distance_to_sqr(position) == 0.0)
            .map(|entity| entity.id);
        if let Some(victim) = hit_mob {
            let attacker = owner
                .and_then(|shooter| self.entities.get(shooter))
                .map(|shooter| Attacker {
                    pos: shooter.position,
                    yaw: shooter.yaw,
                })
                .or(Some(Attacker {
                    pos: position,
                    yaw: 0.0,
                }));
            self.damage_entity(victim, damage, DamageSource::Arrow, attacker);
            if let Some(entity) = self.entities.get_mut(id) {
                entity.removed = true;
            }
        }
    }

    /// Tick a creeper's fuse and detonate at the end of it (P16-04).
    ///
    /// Lit when a ready player is within [`CREEPER_IGNITE_RANGE`] (3 blocks,
    /// pumpkin `CreeperIgniteGoal`); a lit fuse counts up every tick and
    /// stands down one tick at a time past [`CREEPER_DEFUSE_RANGE`] (7
    /// blocks), mirroring vanilla's fuse-speed `-1`. At
    /// [`CREEPER_FUSE_TICKS`] (30) the creeper detonates: itself removed with
    /// no loot (a suicide drops nothing — consistent with the no-credit XP
    /// rule), blast damage to players and mobs inside
    /// [`CREEPER_BLAST_RADIUS`] with linear falloff from the kind's
    /// point-blank figure, gated by line of sight (binary exposure: a wall
    /// between blocks the blast entirely, where vanilla scales it — stated,
    /// not hidden). No blocks break and no crater forms (gap).
    fn tick_creeper_fuse(&mut self, id: EntityId, kind: MobKind, position: mc_world::Vec3) {
        use mc_entity::mob::{CREEPER_DEFUSE_RANGE, CREEPER_FUSE_TICKS, CREEPER_IGNITE_RANGE};
        if kind != MobKind::Creeper {
            return;
        }
        let nearest = self
            .nearest_ready_player(position)
            .map(|(_, distance)| distance);
        let mut detonate = false;
        let (mut fuse_before, mut fuse_now) = (0, 0);
        if let Some(entity) = self.entities.get_mut(id) {
            let EntityBody::Mob(mob) = &mut entity.body else {
                return;
            };
            fuse_before = mob.ai.fuse;
            match nearest {
                Some(distance) if distance <= CREEPER_IGNITE_RANGE => {
                    if mob.ai.fuse == 0 {
                        mob.ai.fuse = 1;
                    } else if mob.ai.fuse < CREEPER_FUSE_TICKS {
                        mob.ai.fuse += 1;
                    }
                }
                Some(distance) if distance > CREEPER_DEFUSE_RANGE && mob.ai.fuse > 0 => {
                    mob.ai.fuse -= 1;
                }
                _ => {
                    if mob.ai.fuse > 0 && mob.ai.fuse < CREEPER_FUSE_TICKS {
                        mob.ai.fuse += 1;
                    }
                }
            }
            fuse_now = mob.ai.fuse;
            detonate = mob.ai.fuse >= CREEPER_FUSE_TICKS;
        }
        // Announce flash-state transitions only (lit, stood down, gone): the
        // icon's lifetime is bounded by these packets, and a per-tick
        // re-send would spam every client for every unlit creeper.
        if detonate || (fuse_before == 0) != (fuse_now == 0) {
            // Lit, stood down, or gone: the client tracks the flash state.
            let state = i32::from(detonate || fuse_now > 0);
            let packet = mc_protocol::packets::play::SetEntityData {
                entity_id: id.get(),
                entries: vec![(
                    mc_protocol::packets::play::METADATA_INDEX_CREEPER_FUSE,
                    mc_protocol::packets::play::MetadataValue::VarInt(state),
                )],
            }
            .to_raw();
            if let Ok(raw) = packet {
                let mut report = TickReport::default();
                self.broadcast_all(&raw, &mut report);
            }
        }
        if detonate {
            self.explode_creeper(id, position);
        }
    }

    /// Detonate a lit creeper: blast damage with falloff, then self-removal.
    fn explode_creeper(&mut self, id: EntityId, position: mc_world::Vec3) {
        use mc_entity::mob::CREEPER_BLAST_RADIUS;
        let centre = Vec3::new(position.x, position.y + 0.5, position.z);
        let base = MobKind::Creeper.attack_damage();
        // Players: torso distance for falloff, eye line for the LOS gate.
        let victims: Vec<(ConnectionId, f32)> = self
            .sessions
            .values()
            .filter(|session| session.ready)
            .filter_map(|session| {
                let p = session.player.position;
                let torso = Vec3::new(p.x, p.y + 0.9, p.z);
                let dx = torso.x - centre.x;
                let dy = torso.y - centre.y;
                let dz = torso.z - centre.z;
                let distance = (dx * dx + dy * dy + dz * dz).sqrt();
                if distance > CREEPER_BLAST_RADIUS {
                    return None;
                }
                let eye = Vec3::new(p.x, p.y + 1.62, p.z);
                if !self.world.has_line_of_sight(centre, eye) {
                    return None;
                }
                let amount = base * (1.0 - distance as f32 / CREEPER_BLAST_RADIUS as f32);
                (amount > 0.0).then_some((session.id, amount))
            })
            .collect();
        for (session_id, amount) in victims {
            if self
                .sessions
                .get(&session_id)
                .is_some_and(|session| session.hurt_invuln_ticks > 0)
            {
                continue;
            }
            let outcome = self.sessions.get_mut(&session_id).map(|session| {
                session.hurt_invuln_ticks = INVULNERABLE_TICKS;
                let armor = worn_stats(&session.player.inventory, &self.registries.items);
                session
                    .player
                    .apply_damage(amount, DamageSource::Explosion, &armor)
            });
            if let Some(outcome) = outcome {
                self.after_damage(session_id, outcome);
            }
        }
        // Mobs in the blast (the creeper excluded): same falloff and LOS.
        let mob_victims: Vec<(EntityId, f32)> = self
            .entities
            .iter()
            .filter(|entity| {
                entity.id != id && !entity.removed && matches!(&entity.body, EntityBody::Mob(_))
            })
            .filter_map(|entity| {
                let at = entity.position;
                let torso = Vec3::new(at.x, at.y + 0.5, at.z);
                let dx = torso.x - centre.x;
                let dy = torso.y - centre.y;
                let dz = torso.z - centre.z;
                let distance = (dx * dx + dy * dy + dz * dz).sqrt();
                if distance > CREEPER_BLAST_RADIUS {
                    return None;
                }
                if !self.world.has_line_of_sight(centre, torso) {
                    return None;
                }
                let amount = base * (1.0 - distance as f32 / CREEPER_BLAST_RADIUS as f32);
                (amount > 0.0).then_some((entity.id, amount))
            })
            .collect();
        for (victim, amount) in mob_victims {
            self.damage_entity(victim, amount, DamageSource::Explosion, None);
        }
        if let Some(entity) = self.entities.get_mut(id) {
            entity.removed = true;
        }
        debug!(%id, "creeper detonated");
    }

    /// Resolve one melee swing against a player.
    ///
    /// Only [`MobAttackStyle::Melee`] reaches here (the style dispatch in
    /// [`Self::tick_entity_ai`] routes ranged and explosive intents to their
    /// own arms). The swing re-checks
    /// the range at resolution time — the target may have moved since the
    /// decision — and the attack cooldown restarts through
    /// [`MobAi::note_attack_landed`] whether or not the hit landed, because the
    /// swing itself was spent.
    ///
    /// The rate limit is the AI's own [`ATTACK_COOLDOWN_TICKS`]; the shared
    /// 10-tick hurt window on the player side is P11-06's combat work.
    fn resolve_mob_melee(
        &mut self,
        attacker: EntityId,
        kind: MobKind,
        target: EntityId,
        position: mc_world::Vec3,
    ) {
        if kind.attack_style() != Some(MobAttackStyle::Melee) {
            return;
        }
        // Range re-check against the live position.
        let Some(target_entity) = self.entities.get(target) else {
            return;
        };
        let dx = target_entity.position.x - position.x;
        let dy = target_entity.position.y - position.y;
        let dz = target_entity.position.z - position.z;
        if (dx * dx + dy * dy + dz * dz).sqrt() > ATTACK_RANGE {
            return;
        }
        // The AI only targets players; find the session whose projection is
        // the target, damage the authoritative `Player`, and push vitals.
        let session_id = self
            .sessions
            .values()
            .find(|session| session.entity == target)
            .map(|session| session.id);
        let Some(session_id) = session_id else {
            return;
        };
        // The shared hurt window: any mob hit inside it is refused, the same
        // 10-tick rule `damage_entity` applies to non-players.
        if self
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.hurt_invuln_ticks > 0)
        {
            return;
        }
        let outcome = self.sessions.get_mut(&session_id).map(|session| {
            session.hurt_invuln_ticks = INVULNERABLE_TICKS;
            // Armour is the victim's worn set (P16-01); knockback does not
            // apply — projections carry no velocity under client-driven
            // motion, so there is nowhere honest to put the shove.
            let armor = worn_stats(&session.player.inventory, &self.registries.items);
            session
                .player
                .apply_damage(kind.attack_damage(), DamageSource::MobAttack, &armor)
        });
        let Some(outcome) = outcome else {
            return;
        };
        if outcome.applied {
            debug!(attacker = %attacker, kind = kind.name(), dealt = outcome.dealt, "mob melee hit");
        }
        self.after_damage(session_id, outcome);
        // The swing is spent whether or not it landed.
        if let Some(entity) = self.entities.get_mut(attacker)
            && let EntityBody::Mob(mob) = &mut entity.body
        {
            mob.ai.note_attack_landed();
        }
    }

    /// Timers, gravity, collision, landing and fall damage for one entity.
    ///
    /// Returns whether the entity was ticked. Players are not: their state lives in
    /// [`Session::player`] and is handled by the Players phase.
    /// Collect one tick of mob effect damage over time (P16-03): poison and
    /// wither hits summed, whether any wither contributed (it wins the damage
    /// flags, mirroring the player path), and regeneration amplifiers.
    /// Multiple simultaneous dots merge into one hit — the single-window
    /// model would eat the second.
    fn mob_effect_ticks(&self, id: EntityId) -> (f32, bool, Vec<i32>) {
        let Some(entity) = self.entities.get(id) else {
            return (0.0, false, Vec::new());
        };
        let EntityBody::Mob(_) = &entity.body else {
            return (0.0, false, Vec::new());
        };
        let mut dot = 0.0f32;
        let mut dot_wither = false;
        let mut regen_amps = Vec::new();
        for effect in entity.effects.values() {
            let hit = mc_entity::effect::damage_over_time(effect, self.tick);
            if hit > 0.0 {
                dot += hit;
                dot_wither =
                    dot_wither || effect.kind() == Some(mc_entity::effect::EffectKind::Wither);
            }
            if effect.kind() == Some(mc_entity::effect::EffectKind::Regeneration) {
                regen_amps.push(effect.amplifier);
            }
        }
        (dot, dot_wither, regen_amps)
    }

    /// Apply one tick of collected mob effects: damage through
    /// [`Self::damage_entity`], regeneration straight to health.
    fn apply_mob_effect_ticks(
        &mut self,
        id: EntityId,
        dot: f32,
        dot_wither: bool,
        regen_amps: Vec<i32>,
    ) {
        if dot > 0.0 {
            let source = if dot_wither {
                DamageSource::Wither
            } else {
                DamageSource::Poison
            };
            let mut hit = dot;
            if !dot_wither && let Some(entity) = self.entities.get(id) {
                hit = hit.min((entity.health - 1.0).max(0.0));
            }
            if hit > 0.0 {
                self.damage_entity(id, hit, source, None);
            }
        }
        if !regen_amps.is_empty()
            && let Some(entity) = self.entities.get_mut(id)
            && let EntityBody::Mob(mob) = &entity.body
        {
            let max = mob.kind.max_health();
            for amplifier in regen_amps {
                let shift = u32::try_from(amplifier.max(0)).unwrap_or(0);
                let interval = (50u64 >> shift.min(6)).max(1);
                if self.tick.is_multiple_of(interval) {
                    entity.health = (entity.health + 1.0).min(max);
                }
            }
        }
    }

    fn tick_entity(&mut self, id: EntityId) -> bool {
        let Some(entity) = self.entities.get(id) else {
            return false;
        };
        // An entity already flagged for removal is left alone until the sweep.
        if entity.removed || entity.kind() == EntityKind::Player {
            return false;
        }
        let start_y = entity.position.y;
        let position = entity.position;
        let on_ground = entity.on_ground;
        let hitbox = entity.hitbox();
        let mut velocity = entity.velocity;

        // 1. Per-kind velocity step, then the shared timers.
        self.mob_effect_ticks(id);
        {
            let Some(entity) = self.entities.get_mut(id) else {
                return false;
            };
            entity.tick_timers();
            if let EntityBody::Item(item) = &mut entity.body {
                // Vanilla's item tick applies gravity and drag, then its own
                // timers, and only then moves.
                item.on_ground = on_ground;
                velocity = item.tick_physics(velocity);
                item.tick_timers();
                entity.velocity = velocity;
                if item.should_despawn() {
                    entity.removed = true;
                    return true;
                }
            } else if let EntityBody::Orb(orb) = &mut entity.body {
                // Orbs ride the same motion shape with their own gravity
                // constant (P16-02); despawn and sweep like drops.
                orb.on_ground = on_ground;
                velocity = orb.tick_physics(velocity);
                orb.tick_age();
                entity.velocity = velocity;
                if orb.should_despawn() {
                    entity.removed = true;
                    return true;
                }
            } else {
                velocity.y -= ENTITY_GRAVITY;
                entity.velocity = velocity;
            }
        }

        // Knockback channel: fold the impulse in after steering and
        // before the move (see `Entity::fold_knockback` for why the
        // channel exists).
        {
            let Some(entity) = self.entities.get_mut(id) else {
                return false;
            };
            entity.fold_knockback(&mut velocity);
        }

        // Effect ticks (P16-03): damage through the shared path (the hurt
        // window, armour-zero for mobs, loot/XP on death all apply), with
        // the poison floor at half a heart; regeneration heals toward max.
        let (dot, dot_wither, regen_amps) = self.mob_effect_ticks(id);
        self.apply_mob_effect_ticks(id, dot, dot_wither, regen_amps);

        // 2. Integrate against the world. `move_with_collision` sweeps the box, so
        //    a fast or badly-framed step cannot tunnel through a floor.
        //    Living movers (players are handled by `move_player`; here: mobs)
        //    auto-step up to STEP_HEIGHT; drops, orbs and arrows move exactly
        //    as before (NO_STEP_UP).
        let step = if self
            .entities
            .get(id)
            .is_some_and(|entity| entity.kind().is_living())
        {
            mc_world::STEP_HEIGHT
        } else {
            mc_world::NO_STEP_UP
        };
        let result = self.world.move_with_collision(hitbox, velocity, step);
        let applied = position.plus(result.delta);
        // Grounded, for an entity that *falls on its own*, is deliberately
        // narrower than the player rule below. A falling entity must not be
        // declared grounded while it is still inside the air block above a floor:
        // doing so zeroes its velocity and leaves it hovering a fraction of a block
        // up. So "solid ground below the feet" only counts when the box is actually
        // resting on the surface, and the collision result is what says so.
        let feet = applied.y;
        let resting = velocity.y >= 0.0
            && (feet - feet.floor()).abs() < REST_EPSILON
            && self.world.is_solid(
                floor_to_i32(applied.x),
                floor_to_i32(feet) - 1,
                floor_to_i32(applied.z),
            );
        // `collided[1]` also covers "the box already overlaps geometry, so the
        // solver refused the downward step", which is how a box that landed a
        // hair inside the floor is recognised as standing on it.
        let blocked_down = velocity.y < 0.0 && result.collided[1];
        let grounded = result.on_ground || blocked_down || resting;

        let mut damage = 0.0f32;
        {
            let Some(entity) = self.entities.get_mut(id) else {
                return false;
            };
            entity.position = applied;
            entity.on_ground = grounded;
            if result.collided[1] && velocity.y < 0.0 {
                velocity.y = 0.0;
            }
            if result.collided[0] {
                velocity.x = 0.0;
            }
            if result.collided[2] {
                velocity.z = 0.0;
            }
            entity.velocity = velocity;
            // Fall damage, measured from where the fall started and ignoring the
            // first three blocks (Vanilla's rule). Only living entities take it.
            if grounded && !on_ground && start_y > applied.y && entity.kind().is_living() {
                let fallen = (start_y - applied.y).floor();
                if fallen > FALL_DAMAGE_THRESHOLD {
                    damage = (fallen - FALL_DAMAGE_THRESHOLD) as f32;
                }
            }
        }
        if damage > 0.0 {
            let died = self.damage_entity(id, damage, mc_entity::combat::DamageSource::Fall, None);
            debug!(%id, damage, died, "entity fall damage");
        }
        true
    }

    /// The red hurt flash for one applied hit: `hurt_animation` (clientbound
    /// play 42) to every ready client, lethal hits included.
    ///
    /// Owner session: landed hits showed no red, because the server sent
    /// `entity_event` 2 — which is not the flash on a modern client (jar
    /// `LivingEntity` never broadcasts 2 on the hurt path; its constants
    /// there are 3 = death, 35 = totem, 46, 60, 67). The red comes from this
    /// packet (jar `ClientboundHurtAnimationPacket`: `VarInt` id + `f32`
    /// yaw; `ServerPlayer.indicateDamage` sends exactly it, Pumpkin's damage
    /// path broadcasts it to tracking players). The yaw mirrors
    /// vanilla/Pumpkin: the damage direction in degrees minus the victim's
    /// yaw, 0.0 with no attacker.
    fn broadcast_hurt_animation(&self, id: EntityId, attacker: Option<Attacker>) {
        let Some(victim) = self.entities.get(id) else {
            return;
        };
        let yaw = attacker.map_or(0.0, |atk| {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "hurt yaw is a visual tilt; f64 direction math narrowed to the f32 wire field"
            )]
            let directed = (atk.pos.z - victim.position.z)
                .atan2(atk.pos.x - victim.position.x)
                .to_degrees() as f32
                - victim.yaw;
            directed
        });
        let packet = HurtAnimation {
            entity_id: id.get(),
            yaw,
        };
        if let Ok(raw) = packet.to_raw() {
            let mut report = TickReport::default();
            self.broadcast_all(&raw, &mut report);
        }
    }

    /// Apply damage to a living entity, flagging it removed when it dies.
    ///
    /// Returns whether this hit was lethal. Non-living entities are immune rather
    /// than silently damaged: an item entity has no health to reduce. `source`
    /// drives armour bypass and knockback (P16-01); `attacker` carries the
    /// knockback direction. Mobs wear no armour; a player-body victim reads the
    /// owning session's worn set, because projections carry no inventory.
    pub(crate) fn damage_entity(
        &mut self,
        id: EntityId,
        raw: f32,
        source: DamageSource,
        attacker: Option<Attacker>,
    ) -> bool {
        let Some(entity) = self.entities.get_mut(id) else {
            return false;
        };
        if !entity.kind().is_living() || !entity.is_alive() {
            return false;
        }
        // Invulnerability frames. Without this a mob standing in fire would take a
        // hit every tick and die instantly, and the parity matrix's claim that the
        // 10-tick window is *applied* would be false (Audit 03 found exactly that).
        if entity.invulnerable_ticks > 0 {
            return false;
        }
        // Knockback past the window (vanilla order: the shove lands before
        // armour and resistance are figured), mobs only — player projections
        // carry no velocity under client-driven motion, so shoving one would
        // be bytes into a field nothing reads.
        if source.applies_knockback()
            && let (Some(atk), EntityBody::Mob(_)) = (attacker, &entity.body)
        {
            // Mobs wear nothing, so no resistance scales this; the
            // constant is vanilla's hurt-path base (see combat docs).
            entity.apply_knockback(atk.pos, atk.yaw, BASE_MELEE_KNOCKBACK);
        }
        // Armour before resistance (vanilla order). Mobs wear none; a
        // player-body victim borrows its owner's worn set.
        let stats = match &entity.body {
            EntityBody::Player => self
                .sessions
                .values()
                .find(|session| session.entity == id)
                .map_or(CombatStats::ZERO, |session| {
                    worn_stats(&session.player.inventory, &self.registries.items)
                }),
            _ => CombatStats::ZERO,
        };
        let armored = if source.bypasses_armor() {
            raw
        } else {
            armor_absorb(raw, stats.armor, stats.toughness)
        };
        // Resistance and the other damage modifiers, which nothing applied before.
        let effects: Vec<mc_entity::effect::ActiveEffect> =
            entity.effects.values().copied().collect();
        let multiplier = mc_entity::effect::damage_taken_multiplier(&effects);
        let amount = if armored.is_finite() && armored > 0.0 {
            armored * multiplier as f32
        } else {
            return false;
        };
        if amount <= 0.0 {
            // Full resistance: the hit lands but deals nothing, and still consumes
            // the invulnerability window so the client is not spammed.
            entity.invulnerable_ticks = INVULNERABLE_TICKS;
            return false;
        }
        entity.health = (entity.health - amount).max(0.0);
        entity.invulnerable_ticks = INVULNERABLE_TICKS;
        let died = entity.health <= 0.0;
        self.broadcast_hurt_animation(id, attacker);
        if !died {
            return false;
        }
        let Some(entity) = self.entities.get_mut(id) else {
            return false;
        };
        let body = &entity.body;
        let kind = match body {
            EntityBody::Mob(mob) => Some(mob.kind),
            _ => None,
        };
        let position = entity.position;
        entity.removed = true;
        // Experience scatter (P16-02): vanilla awards XP only for player
        // kills. This function's player-driven caller is the swing path
        // (`attacker.is_some()`); falls and any future non-player caller
        // scatter nothing. A mob finished off by the environment after a
        // player hit keeps its XP — no hurt-credit tracking exists to award
        // it, and that limitation is stated, not hidden.
        if let (Some(kind), Some(_)) = (kind, attacker) {
            let reward = kind.experience_reward();
            if reward > 0 {
                self.scatter_experience(position, reward);
            }
        }
        // P11-04: a dead mob's loot table is the drop authority, same as
        // blocks. Looting-enchanted bonuses are not modelled (the level map is
        // empty), so a looting-gated rare pool reads level 0 and stays closed.
        if let Some(kind) = kind {
            let stem = kind.name();
            if let Ok(table_id) =
                mc_core::ids::ResourceId::parse(&format!("minecraft:entities/{stem}"))
            {
                // Owner decision (§2 of the audit handoff): a bare hand is a
                // **known, unenchanted** tool, not an unknown one. `None` means
                // "tool unknown", which turned every enchantment-gated condition
                // into a refusal and made a dead cow drop nothing; an empty map
                // is "known, level 0", which the module's own docs define, so
                // the conditions *evaluate* instead.
                let context = mc_data::loot::LootContext {
                    enchantment_levels: Some(BTreeMap::new()),
                    survives_explosion: None,
                    block_properties: None,
                    luck: None,
                };
                let centre = mc_world::Vec3::new(position.x, position.y + 0.5, position.z);
                self.spawn_loot_table(&table_id, centre, &context);
            }
        }
        true
    }

    /// Phase 4: apply the queued intents in arrival order, then tick players.
    fn phase_players(&mut self, report: &mut TickReport) -> ServerResult<()> {
        self.apply_pending_intents(report)?;
        self.tick_players();
        // Publish player state into the entity store so queries and packets see
        // players through the same store as everything else. This is a projection:
        // `Session::player` stays authoritative (module docs).
        self.project_player_entities();
        Ok(())
    }

    /// Apply this tick's queued intents, oldest first.
    ///
    /// The queue is bounded upstream rather than here: the Network phase drains at
    /// most [`PENDING_INTENT_BUDGET`] events per tick and is the only thing that
    /// pushes, so this loop cannot be handed an unbounded backlog. Anything a tick
    /// does not drain stays in the channel and arrives next tick, in order.
    fn apply_pending_intents(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let pending = std::mem::take(&mut self.pending_intents);
        for (id, intent) in pending {
            self.apply_intent(id, intent, report)?;
        }
        Ok(())
    }

    /// Phase 6: flush everything a client should see about this tick.
    fn phase_broadcast(&mut self, report: &mut TickReport, tick: Tick) -> ServerResult<()> {
        self.broadcast_block_changes(report)?;
        self.broadcast_light_updates(report)?;
        self.broadcast_entity_spawns(report)?;
        self.send_block_change_acks(report)?;
        self.stream_all(report)?;
        self.send_world_time(report, tick)?;
        self.sweep_entity_removals(report)?;
        self.unload_distant_chunks(report)?;
        Ok(())
    }

    /// Tell each player whose block prediction is still open how far the server
    /// has got, then close it.
    ///
    /// This runs **after** the block changes of the same tick have been queued, and
    /// in the same per-connection order, so a client sees `block_update` and then
    /// the ack that lets it apply. That ordering is the whole reason this is a
    /// broadcast-phase step rather than part of the Players phase: acking before
    /// the update would have the client close its prediction on the *old* state and
    /// re-apply the old block.
    ///
    /// One packet per player per tick at most, exactly like vanilla's
    /// `ServerGamePacketListenerImpl.tick`, which sends and then resets the
    /// high-water mark to `-1`.
    fn send_block_change_acks(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let pending: Vec<(ConnectionId, i32)> = self
            .sessions
            .iter_mut()
            .filter_map(|(id, session)| {
                let sequence = std::mem::replace(
                    &mut session.ack_block_changes_up_to,
                    NO_BLOCK_CHANGE_SEQUENCE,
                );
                (sequence > NO_BLOCK_CHANGE_SEQUENCE).then_some((*id, sequence))
            })
            .collect();
        for (id, sequence) in pending {
            self.send(id, &BlockChangedAck { sequence }, report)?;
            report.block_change_acks += 1;
        }
        Ok(())
    }

    /// Raise the pending block-change acknowledgement for `id` to at least
    /// `sequence`, ignoring anything that is not a sequence.
    ///
    /// Vanilla throws on a negative sequence; a hostile client must not be able to
    /// do that here, so a negative value is dropped. The high-water mark is what
    /// vanilla keeps (`Math.max`), not a queue: the client only needs to learn how
    /// far the server has processed.
    pub(crate) fn note_block_change_sequence(&mut self, id: ConnectionId, sequence: i32) {
        if sequence < 0 {
            debug!(id = %id, sequence, "ignored a negative block-change sequence");
            return;
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.ack_block_changes_up_to = session.ack_block_changes_up_to.max(sequence);
        }
    }

    /// Take every entity flagged for removal out of the store, recording the batch
    /// on the report.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the batch cannot be framed, which for a list of ids means never.
    fn sweep_entity_removals(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let removed = self.entities.sweep_removed();
        if removed.is_empty() {
            return Ok(());
        }
        // A player's own entity going away means the connection is gone; drop the
        // mapping so `entity_id_of` cannot hand out a dead id.
        let dead: BTreeSet<EntityId> = removed.iter().copied().collect();
        self.entity_ids.retain(|_, id| !dead.contains(id));
        debug!(count = removed.len(), "entities swept");
        report.removed_entities += removed.len();
        // **Broadcast to every ready session**, not to the players who were tracking each entity. The sweep has
        // already taken them out of the store, so their positions are gone and `broadcast_chunk` has nothing to
        // aim at; per-player entity visibility is not something this build has. A client ignores a
        // `remove_entities` for an id it does not hold, so sending it everywhere is correct rather than
        // approximate -- and cheaper than tracking who was told what, which is worth building when there is
        // something to gain from it and there is not yet.
        let ids: Vec<i32> = removed.iter().map(|id| id.get()).collect();
        let packet = mc_protocol::packets::play::RemoveEntities { entity_ids: ids }.to_raw()?;
        self.broadcast_all(&packet, report);
        report.removed_ids = removed;
        Ok(())
    }

    /// Broadcast this tick's block changes to the players who have the chunk.
    ///
    /// Also retires the block entity at each changed position. A block entity is
    /// state its *block* owns, so a block that changed no longer owns it; leaving the
    /// entry behind would make it reappear if the same block were placed again.
    ///
    /// **A retired entity's items are currently lost.** The count is logged and the
    /// retirement is counted on the tick report, but nothing spawns them: an item drop
    /// needs a per-item position, which is P06-08's caller. The previous comment here
    /// claimed they were dropped, which the log a few lines below contradicts
    /// (Audit 05).
    /// Spend the per-tick light budget: recompute the light of queued chunks and tell the clients that hold
    /// them.
    ///
    /// Bounded on purpose. Recomputing one chunk is three passes over 124 320 cells and the packet is
    /// kilobytes, so the rate is fixed per tick and the queue absorbs whatever exceeds it — a burst is sent a
    /// little later rather than dropped.
    fn broadcast_light_updates(&mut self, report: &mut TickReport) -> ServerResult<()> {
        for _ in 0..LIGHT_UPDATES_PER_TICK {
            let Some(pos) = self.pending_light.iter().next().copied() else {
                break;
            };
            self.pending_light.remove(&pos);
            // A chunk that has been unloaded has nothing to light and nobody to tell.
            if !self.world.is_loaded(pos) {
                continue;
            }
            self.world.compute_light(pos, &self.registries.light)?;
            let Some(light) = self.world.cached_light(pos) else {
                continue;
            };
            let Some(chunk) = self.world.chunk(pos) else {
                continue;
            };
            let fields = light_fields(light, chunk.sections.len())?;
            let update = LightUpdate {
                chunk_x: pos.x,
                chunk_z: pos.z,
                sky_light_mask: fields.sky_mask,
                block_light_mask: fields.block_mask,
                empty_sky_light_mask: fields.empty_sky_mask,
                empty_block_light_mask: fields.empty_block_mask,
                sky_light: fields.sky,
                block_light: fields.block,
            };
            // Only to clients that have the chunk: one that has never been sent it has the light it needs
            // coming with the chunk itself. The ids are collected first because `send` needs the whole `self`
            // while the iteration borrows `self.sessions`.
            let holders: Vec<ConnectionId> = self
                .sessions
                .values()
                .filter(|session| session.sent_chunks.contains(&pos))
                .map(|session| session.id)
                .collect();
            for id in holders {
                self.send(id, &update, report)?;
            }
            report.light_updates += 1;
        }
        Ok(())
    }

    /// Announce this tick's dropped items to the players who can see them.
    ///
    /// The type id comes from `self.registries.entities`, which is the table extracted from the jar by
    /// `EntityTypeProbe` -- **not** the config payload, where `minecraft:entity_type` is a tag directory. A
    /// dropped stack is `minecraft:item`, which is id **71** and not id 0: the registry is alphabetical, so id 0
    /// is `minecraft:acacia_boat`. See P10-06.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the entity type table has no `minecraft:item`, which a table that
    /// loaded cannot happen to: the parser rejects a table with gaps.
    ///
    /// [`ServerError::Protocol`] when the packet cannot be framed, which for these field types means never.
    fn broadcast_entity_spawns(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let pending = std::mem::take(&mut self.pending_entity_spawns);
        if pending.is_empty() {
            return Ok(());
        }
        let item = self.registries.entities.id(mc_registry::entities::ITEM)?;
        for id in pending {
            // The entity can be gone already -- reaped, or removed by a command in the same tick -- and an
            // identity for something that is not there is not something to send. Skipping is the honest answer
            // rather than a packet describing nothing.
            let (Some(entity), Some(uuid)) = (self.entities.get(id), self.entities.uuid(id)) else {
                continue;
            };
            // The registry id the client's own entity-type table maps back to a
            // model: a drop's 71, or the mob kind's row from `entity_types.tsv`
            // (P10-06). An unmodeled id would be sent as nothing recognizable.
            let type_id = match &entity.body {
                mc_entity::EntityBody::Mob(mob) => {
                    // `entity_types.tsv` stores the full resource id, and
                    // `MobKind::name` is the bare suffix.
                    let full = format!("minecraft:{}", mob.kind.name());
                    self.registries.entities.id(&full)?
                }
                mc_entity::EntityBody::Orb(_) => self
                    .registries
                    .entities
                    .id(mc_registry::entities::EXPERIENCE_ORB)?,
                mc_entity::EntityBody::Projectile(projectile) => match projectile.kind {
                    mc_entity::ProjectileKind::Arrow => {
                        self.registries.entities.id(mc_registry::entities::ARROW)?
                    }
                    mc_entity::ProjectileKind::Snowball => item,
                },
                mc_entity::EntityBody::Item(_) | mc_entity::EntityBody::Player => item,
            };
            let position = entity.position;
            let (yaw, pitch) = (entity.yaw, entity.pitch);
            let packet = mc_protocol::packets::play::AddEntity {
                entity_id: id.get(),
                uuid,
                type_id,
                x: position.x,
                y: position.y,
                z: position.z,
                pitch: wire_angle(pitch),
                yaw: wire_angle(yaw),
                head_yaw: wire_angle(yaw),
                // A dropped stack has no variant fields, and our entities
                // spawn at rest: vanilla's `LpVec3` encodes a stationary
                // movement as one zero byte.
                data: 0,
                movement: (0.0, 0.0, 0.0),
            }
            .to_raw()?;
            if self.broadcast_chunk(chunk_of(position.x, position.z), &packet, report) > 0 {
                report.entities_spawned += 1;
            }
            // **The stack, as a second packet**, which is how a capture shows a real server doing it: a drop is
            // announced by `add_entity` and then given its contents by `set_entity_data` at index 8 with
            // serializer type 7. An item entity with no metadata is one a client draws as an empty-looking drop.
            //
            // The chain rather than nested `if`s: two conditions, one body, and `clippy::collapsible_if` is right
            // that the flat form says it better.
            match &entity.body {
                mc_entity::EntityBody::Item(item) => {
                    if let Some(item_id) = item.item_id() {
                        let contents = mc_protocol::packets::play::SetEntityData {
                            entity_id: id.get(),
                            entries: vec![(
                                8,
                                mc_protocol::packets::play::MetadataValue::ItemStack {
                                    count: item.count(),
                                    item_id,
                                },
                            )],
                        }
                        .to_raw()?;
                        self.broadcast_all(&contents, report);
                    }
                }
                // **A spawn health, as the second packet** -- the slot every
                // captured mob kind sends at spawn (P10-07's measured table:
                // index 9, float serializer). A default-variant mob omits every
                // other slot, so health alone matches vanilla's spawn shape.
                mc_entity::EntityBody::Mob(mob) => {
                    let contents = mc_protocol::packets::play::SetEntityData {
                        entity_id: id.get(),
                        entries: vec![(
                            mc_protocol::packets::play::METADATA_INDEX_HEALTH,
                            mc_protocol::packets::play::MetadataValue::Float(mob.kind.max_health()),
                        )],
                    }
                    .to_raw()?;
                    self.broadcast_all(&contents, report);
                }
                mc_entity::EntityBody::Orb(orb) => {
                    let contents = mc_protocol::packets::play::SetEntityData {
                        entity_id: id.get(),
                        entries: vec![(
                            mc_protocol::packets::play::METADATA_INDEX_ORB_VALUE,
                            mc_protocol::packets::play::MetadataValue::VarInt(orb.value),
                        )],
                    }
                    .to_raw()?;
                    self.broadcast_all(&contents, report);
                }
                mc_entity::EntityBody::Player | mc_entity::EntityBody::Projectile(_) => {}
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn broadcast_block_changes(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let changes = self.world.take_block_changes();
        for change in changes {
            let pos = mc_container::BlockPos::new(change.x, change.y, change.z);
            // A change *into* a container (placing a chest) must not retire the
            // entity the open path just created in the same tick: the placement
            // queues first, the open runs in the Network phase, and this
            // broadcast runs last. Only a change *away* from containers retires.
            let new_is_container = self
                .registries
                .blocks
                .block_name(change.new_id)
                .is_ok_and(is_container_block);
            if new_is_container
                && self.block_entities.get(pos).is_none()
                && let Some(kind) = self
                    .registries
                    .blocks
                    .block_name(change.new_id)
                    .ok()
                    .and_then(open_kind_for)
            {
                let entity_kind = match kind {
                    OpenKind::Chest => Some(mc_container::BlockEntityKind::Container),
                    OpenKind::Furnace => Some(mc_container::BlockEntityKind::Furnace),
                    OpenKind::Hopper => Some(mc_container::BlockEntityKind::Hopper),
                    OpenKind::Dispenser => Some(mc_container::BlockEntityKind::Dispenser),
                    // Crafting tables never reach this path (no block
                    // entity); skipping keeps the mapping exhaustive.
                    OpenKind::Crafting => None,
                };
                if let Some(entity_kind) = entity_kind {
                    self.block_entities
                        .insert(mc_container::BlockEntity::new(pos, entity_kind));
                }
            } else if !new_is_container && let Some(retired) = self.block_entities.remove(pos) {
                // P12-06: breaking a container drops its contents (closes the P06
                // "items lost on break" gap). Each non-empty stack becomes a ground
                // item at the block centre, alongside the loot-table drops the
                // break path already spawned.
                if let Some(items) = retired.data.items() {
                    let centre = Vec3::new(
                        f64::from(change.x) + 0.5,
                        f64::from(change.y) + 0.5,
                        f64::from(change.z) + 0.5,
                    );
                    for stack in items.iter().filter(|s| !s.is_empty()) {
                        if let Err(error) = self.spawn_item(*stack, centre) {
                            warn!(%pos, %error, "a broken container's item could not be spawned");
                        }
                    }
                }
                report.block_entities_changed += 1;
                // Viewers of the broken block get their window closed and their
                // player menu restored: the block half they were transacting
                // against no longer exists.
                let viewers: Vec<(ConnectionId, i32)> = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| s.open_block == Some(pos))
                    .map(|(id, s)| (*id, i32::from(s.menu.window_id())))
                    .collect();
                for (id, window) in viewers {
                    // Carry the old cursor across the rebuild: it lives only in
                    // the discarded menu, and dropping it would lose the held
                    // stack with no warn/drop (AUDIT-12). Returned below like a
                    // close (inventory first, feet on overflow).
                    let carried = self
                        .sessions
                        .get(&id)
                        .map_or(mc_entity::stack::ItemStack::EMPTY, |s| s.menu.cursor());
                    // Rebuild the player menu first so the close below cannot
                    // strand the session without a window.
                    let rebuilt = self.new_player_menu();
                    match rebuilt {
                        Ok(mut menu) => {
                            if let Some(session) = self.sessions.get_mut(&id) {
                                mirror_inventory(&mut menu, &session.player.inventory);
                                menu.set_creative(session.player.game_mode.is_creative());
                                session.menu = menu;
                                session.open_block = None;
                            }
                        }
                        Err(error) => {
                            debug!(id = %id, %error, "could not rebuild the player menu after a break");
                            continue;
                        }
                    }
                    if !carried.is_empty() {
                        let leftover = match self.sessions.get_mut(&id) {
                            Some(session) => session.player.inventory.add_stack(carried),
                            None => carried,
                        };
                        if !leftover.is_empty() {
                            let position = self
                                .sessions
                                .get(&id)
                                .map_or(mc_world::Vec3::default(), |s| s.player.position);
                            let _ = self.spawn_item(leftover, position);
                        }
                    }
                    let _ = self.send(
                        id,
                        &mc_protocol::packets::play::ContainerClose { window_id: window },
                        report,
                    );
                    // And the restored player contents, so the client is not left
                    // showing the chest it just lost.
                    if let Some(session) = self.sessions.get(&id) {
                        let contents: Vec<mc_protocol::packets::play::ItemStack> = session
                            .menu
                            .full_contents()
                            .iter()
                            .copied()
                            .map(wire_stack)
                            .collect();
                        let state = session.menu.state_id();
                        let cursor = session.menu.cursor();
                        let _ = self.send(
                            id,
                            &ContainerSetContent {
                                window_id: 0,
                                state_id: state,
                                slots: contents,
                                carried: wire_stack(cursor),
                            },
                            report,
                        );
                    }
                }
            }
            let packet = BlockUpdate {
                position: block_position(change.x, change.y, change.z),
                block_state: change.new_id,
            }
            .to_raw()?;
            if self.broadcast_chunk(change.pos, &packet, report) > 0 {
                report.block_changes += 1;
            }
            // The changed chunk *and* every neighbour whose one-block light margin
            // reads across the border the block sits within one block of, since the
            // light the client holds for those changed too. The rule is
            // `mc_world::chunks_a_block_can_light` rather than a second copy of the
            // four edge tests: the copy that used to live here could not express the
            // diagonal case at a corner, so a corner change left the diagonal chunk
            // unqueued and the client drawing its old light (AUDIT-09 B-05).
            for affected in mc_world::chunks_a_block_can_light(change.pos, change.x, change.z) {
                self.pending_light.insert(affected);
            }
        }
        Ok(())
    }

    /// Send the world time once a second.
    fn send_world_time(&self, report: &mut TickReport, tick: Tick) -> ServerResult<()> {
        if !tick.is_multiple_of(20) {
            return Ok(());
        }
        // Vanilla's steady state is an empty clock map
        // (`forceGameTimeSynchronization`); the client advances its clock
        // instances locally from the join-time full sync and the immediate
        // single-entry broadcast a `/time` mutation sends. A full entry every
        // second would also work, but the empty map is what eighteen captured
        // vanilla packets all carry.
        let packet = SetTime {
            world_age: tick as i64,
            clocks: Vec::new(),
        }
        .to_raw()?;
        self.broadcast_all(&packet, report);
        Ok(())
    }

    /// The overworld clock entry at `tick`, carrying the `/time` offset.
    pub(crate) fn overworld_clock_entry(
        &self,
        tick: Tick,
    ) -> mc_protocol::packets::play::ClockState {
        mc_protocol::packets::play::ClockState {
            clock_id: mc_protocol::packets::play::WORLD_CLOCK_OVERWORLD,
            total_ticks: tick as i64 + self.time_offset,
            partial_tick: 0.0,
            rate: 1.0,
        }
    }

    // ---------------------------------------------------------------- events

    fn apply_event(&mut self, event: ClientEvent, report: &mut TickReport) -> ServerResult<()> {
        match event.kind {
            ClientEventKind::Joined { profile, outbound } => {
                self.join(event.id, &profile, outbound, report)?;
            }
            ClientEventKind::Intent(intent) => {
                // Queued, not applied: see the module docs on phase ordering.
                self.pending_intents.push((event.id, intent));
            }
            ClientEventKind::Unmodelled { packet_id } => {
                debug!(id = %event.id, packet_id, "unmodelled play packet");
            }
            ClientEventKind::ViewDistance { distance } => {
                self.apply_view_distance(event.id, distance, report)?;
            }
            ClientEventKind::Left => self.leave(event.id),
        }
        Ok(())
    }

    // Every player is ticked the same way and nothing here can fail: the food step
    // and the void guard both produce outcomes rather than errors. Returning a
    // `Result` that is always `Ok` would invite a caller to `?` it and hide that.
    fn tick_players(&mut self) {
        let min_y = i32::from(self.world.min_section_y()) * mc_world::SECTION_HEIGHT;
        let spawn = self.world.spawn();
        let mut messages: Vec<(ConnectionId, String)> = Vec::new();
        let mut effect_events: Vec<(ConnectionId, i32, Vec<i32>)> = Vec::new();
        // Dig outcomes, applied after the loop (breaks mutate the world;
        // stages broadcast). Breaks carry the connection plus the target
        // snapshot (the state is already cleared when the event is
        // collected); stages and clears carry the overlay key (miner
        // entity, position) plus the stage.
        let mut dig_breaks: Vec<(ConnectionId, i32, i32, i32, i32)> = Vec::new();
        let mut dig_stages: Vec<(i32, i32, i32, i32, i8)> = Vec::new();
        let mut dig_clears: Vec<(i32, i32, i32, i32)> = Vec::new();
        for session in self.sessions.values_mut() {
            session.tick_start_y = session.player.position.y;
            session.hurt_invuln_ticks = session.hurt_invuln_ticks.saturating_sub(1);
            session.xp_pickup_cooldown = session.xp_pickup_cooldown.saturating_sub(1);
            tick_session_vitals(
                session,
                self.tick,
                &self.registries.items,
                &mut messages,
                &mut effect_events,
            );
            // Nothing below the world is standable. Void damage is P05; until then
            // a player who ends up there is returned to spawn instead of falling
            // forever.
            if session.player.position.y < f64::from(min_y - 8) {
                warn!(id = %session.id, "player fell out of the world; returning to spawn");
                session.player.position = mc_world::Vec3::new(
                    f64::from(spawn.0) + 0.5,
                    f64::from(spawn.1),
                    f64::from(spawn.2) + 0.5,
                );
                session.tick_start_y = f64::from(spawn.1);
                session.sent_chunks.clear();
            }
            // Survival digging accumulates here, beside hunger: one mining
            // tick per game tick (see `tick_session_dig` for the rules).
            tick_session_dig(
                session,
                &self.world,
                &self.registries,
                &mut dig_breaks,
                &mut dig_stages,
                &mut dig_clears,
            );
        }
        for (id, message) in messages {
            self.send_message(id, &message);
        }
        let mut report = TickReport::default();
        for (id, entity_id, expired) in effect_events {
            for effect_id in expired {
                let Some(wire_id) = mc_entity::effect::wire_id_of(effect_id) else {
                    continue;
                };
                let packet = mc_protocol::packets::play::RemoveMobEffect {
                    entity_id,
                    effect_id: wire_id,
                };
                let _ = self.send(id, &packet, &mut report);
            }
        }
        // Dig outcomes, in collection order: breaks, stage advances, clears.
        self.apply_dig_events(dig_breaks, dig_stages, dig_clears, &mut report);
    }

    /// Apply collected dig outcomes: breaks through the shared path with a
    /// fresh harvest judgment, stages and clears as overlay packets.
    ///
    /// The harvest judgment re-runs here rather than riding the event: the
    /// held item may have changed between collection and application, and
    /// the break must judge what is held now. (A swap mid-tick cancels the
    /// dig next tick via the held check, so this normally sees an unchanged
    /// hand — defense in depth.)
    fn apply_dig_events(
        &mut self,
        dig_breaks: Vec<(ConnectionId, i32, i32, i32, i32)>,
        dig_stages: Vec<(i32, i32, i32, i32, i8)>,
        dig_clears: Vec<(i32, i32, i32, i32)>,
        report: &mut TickReport,
    ) {
        for (id, x, y, z, block) in dig_breaks {
            let Some(current) = self.world.get_block_loaded(x, y, z) else {
                continue;
            };
            if current != block || self.registries.blocks.is_empty(current) {
                continue;
            }
            let Ok(name) = self.registries.blocks.block_name(current) else {
                continue;
            };
            let name = name.to_owned();
            let held = self
                .sessions
                .get(&id)
                .and_then(|session| session.player.inventory.selected_item().item_id());
            let held_name =
                held.and_then(|held| self.registries.items.name(held).ok().map(str::to_owned));
            let harvest = mc_registry::can_harvest(
                &self.registries.blocks,
                &self.registries.items,
                held_name.as_deref(),
                &name,
            );
            self.break_block_now(
                id,
                super::session::BreakTarget {
                    x,
                    y,
                    z,
                    block: current,
                },
                false,
                harvest,
                report,
            );
        }
        for (entity_id, x, y, z, stage) in dig_stages {
            let packet = mc_protocol::packets::play::BlockDestruction {
                entity_id,
                position: block_position(x, y, z),
                stage,
            };
            if let Ok(raw) = packet.to_raw() {
                self.broadcast_all(&raw, report);
            }
        }
        for (entity_id, x, y, z) in dig_clears {
            let packet = mc_protocol::packets::play::BlockDestruction {
                entity_id,
                position: block_position(x, y, z),
                stage: -1,
            };
            if let Ok(raw) = packet.to_raw() {
                self.broadcast_all(&raw, report);
            }
        }
    }

    /// Push vitals after damage and tell the player if they died.
    ///
    /// Takes no `TickReport`: it runs from the movement path, where the caller
    /// already has one and a second borrow is impossible. The packets are queued
    /// regardless; only the per-tick counters miss them.
    pub(crate) fn after_damage(&mut self, id: ConnectionId, outcome: DamageOutcome) {
        let mut local = TickReport::default();
        if outcome.applied {
            let _ = self.send_vitals(id, &mut local);
        }
        if outcome.died {
            // P11-07: the inventory becomes ground entities at the death
            // position, right now, which is when vanilla drops it. The later
            // `respawn` call finds an empty inventory and drops nothing twice.
            // Experience orbs are not an entity kind here (named gap), so the
            // experience reset happens at respawn with nothing on the ground.
            let (position, stacks, level) = match self.sessions.get_mut(&id) {
                Some(session) => {
                    let p = session.player.position;
                    let position = mc_world::Vec3::new(p.x, p.y, p.z);
                    // M-1: the death point rides `Respawn`'s `lastDeathLocation`.
                    // Rounded down to the block the player died in, which is what a
                    // `GlobalPos` is.
                    session.last_death_location = Some((
                        mc_network::registry_data::OVERWORLD.to_owned(),
                        mc_protocol::packets::play::block_position(
                            p.x.floor() as i32,
                            p.y.floor() as i32,
                            p.z.floor() as i32,
                        ),
                    ));
                    (
                        position,
                        session.player.inventory.drain_all(),
                        session.player.level,
                    )
                }
                None => return,
            };
            for stack in stacks {
                match self.spawn_item_owned(stack, position, None) {
                    Ok(entity) => debug!(id = %id, %entity, "death drop spawned"),
                    Err(error) => warn!(id = %id, %error, "could not spawn a death drop"),
                }
            }
            // Experience scatter (P16-02): vanilla drops min(7 * level, 100)
            // points as orbs. No keepInventory gamerule exists here, so the
            // scatter is unconditional — and the later `respawn` still resets
            // the bar, which is now correct instead of lossy.
            let dropped = u32::try_from(level.max(0))
                .unwrap_or(0)
                .saturating_mul(7)
                .min(100);
            if dropped > 0 {
                self.scatter_experience(position, dropped);
            }
        }
        if outcome.died {
            let _ = self.send(
                id,
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal("You died! Use the respawn button."),
                    chat_type: CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                &mut local,
            );
        }
    }

    fn stream_all(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let ids: Vec<ConnectionId> = self.sessions.keys().copied().collect();
        for id in ids {
            self.stream_for(id, report)?;
        }
        Ok(())
    }

    /// Send the chunks a player is missing, nearest first, bounded per tick.
    pub(crate) fn stream_for(
        &mut self,
        id: ConnectionId,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        // The client's cache centre must lead the chunks, not trail them: it
        // culls and waits on the centre it was last told (P14-04 follow-up).
        let centre = {
            let Some(session) = self.sessions.get(&id) else {
                return Ok(());
            };
            session.chunk()
        };
        let stale = self
            .sessions
            .get(&id)
            .is_some_and(|session| session.center != centre);
        if stale {
            let packet = SetChunkCacheCenter {
                x: centre.x,
                z: centre.z,
            };
            self.send(id, &packet, report)?;
            if let Some(session) = self.sessions.get_mut(&id) {
                session.center = centre;
            }
        }
        let radius = self
            .sessions
            .get(&id)
            .map_or(self.view_distance, |session| session.view_distance);
        let mut wanted: Vec<ChunkPos> = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let candidate = ChunkPos::new(centre.x + dx, centre.z + dz);
                let sent = self
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.sent_chunks.contains(&candidate));
                if !sent {
                    wanted.push(candidate);
                }
            }
        }
        // Deterministic order (distance, then coordinates) so two runs stream in the
        // same sequence (AGENTS.md section 3.6).
        wanted.sort_by_key(|candidate| {
            let dx = candidate.x - centre.x;
            let dz = candidate.z - centre.z;
            (dx * dx + dz * dz, candidate.x, candidate.z)
        });
        // Bound the whole tick, not this call: a join streams once on join and again
        // via `stream_all`, and both would otherwise send a full batch.
        let budget = CHUNKS_PER_TICK.saturating_sub(report.chunks_sent);
        wanted.truncate(budget);
        for pos in wanted {
            self.send_chunk(id, pos, report)?;
        }
        Ok(())
    }

    fn send_chunk(
        &mut self,
        id: ConnectionId,
        pos: ChunkPos,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        // Disk first, placeholder second; see `load_or_create_chunk`.
        self.load_or_create_chunk(pos);
        // Light before the borrow below, not inside the packet builder: the builder takes `&self` and reaches
        // the world through its argument, so it cannot fill the cache. Computing here means each chunk is lit
        // once rather than on every send (KD-45).
        self.world.compute_light(pos, &self.registries.light)?;
        // The packet is built from a borrow rather than a clone: a chunk is
        // ~384 KiB, and this is the per-chunk hot path of a join. Both borrows are
        // immutable, so the compiler accepts them together.
        let Some(chunk) = self.world.chunk(pos) else {
            // Unreachable: the call above guarantees a chunk exists at `pos`.
            debug!(?pos, "chunk vanished between load and send");
            return Ok(());
        };
        let packet = self.vanilla_chunk_packet(chunk)?;
        // Record only a packet that actually reached the queue. Marking it sent
        // first (which an earlier version did) means a dropped packet leaves a hole
        // the client never gets: `stream_for` skips anything already in
        // `sent_chunks`, so the chunk would never be re-sent.
        let queued = self.send(id, &packet, report)?;
        if queued {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.sent_chunks.insert(pos);
            }
            report.chunks_sent += 1;
            // P11-08/P11-10: entities already in the streamed chunk are
            // announced to **this** player — the pending-spawn path only
            // reaches players who held the chunk at spawn time, so a joining
            // player would otherwise never learn what lives here.
            let residents: Vec<EntityId> = self
                .entities
                .ids()
                .filter(|entity| {
                    self.entities.get(*entity).is_some_and(|entity| {
                        !entity.removed
                            && entity.kind() != EntityKind::Player
                            && chunk_of(entity.position.x, entity.position.z) == pos
                    })
                })
                .collect();
            for entity in residents {
                for packet in self.entity_announce_packets(entity)? {
                    // `packet` is already framed, so it goes through the raw
                    // sender rather than the Packet-implementing path.
                    self.send_raw(id, packet, report);
                }
            }
        } else {
            debug!(?pos, "chunk packet dropped; it will be retried next tick");
        }
        Ok(())
    }

    /// The packets that introduce one entity: `add_entity`, then the body
    /// packet — the item stack for a drop, the spawn health for a mob.
    ///
    /// Shared by the broadcast phase (to every player holding the chunk) and
    /// the chunk stream (to the player that just received the chunk).
    fn entity_announce_packets(&self, id: EntityId) -> ServerResult<Vec<RawPacket>> {
        let Some(entity) = self.entities.get(id) else {
            return Ok(Vec::new());
        };
        let Some(uuid) = self.entities.uuid(id) else {
            return Ok(Vec::new());
        };
        let type_id = match &entity.body {
            mc_entity::EntityBody::Mob(mob) => {
                // `entity_types.tsv` stores the full resource id, and
                // `MobKind::name` is the bare suffix.
                let full = format!("minecraft:{}", mob.kind.name());
                self.registries.entities.id(&full)?
            }
            mc_entity::EntityBody::Orb(_) => self
                .registries
                .entities
                .id(mc_registry::entities::EXPERIENCE_ORB)?,
            mc_entity::EntityBody::Projectile(projectile) => match projectile.kind {
                // Arrows announce as arrows; snowballs keep their historical
                // item-type announcement (pre-existing quirk, out of scope).
                mc_entity::ProjectileKind::Arrow => {
                    self.registries.entities.id(mc_registry::entities::ARROW)?
                }
                mc_entity::ProjectileKind::Snowball => {
                    self.registries.entities.id(mc_registry::entities::ITEM)?
                }
            },
            _ => self.registries.entities.id(mc_registry::entities::ITEM)?,
        };
        let position = entity.position;
        let (yaw, pitch) = (entity.yaw, entity.pitch);
        let add = mc_protocol::packets::play::AddEntity {
            entity_id: id.get(),
            uuid,
            type_id,
            x: position.x,
            y: position.y,
            z: position.z,
            pitch: wire_angle(pitch),
            yaw: wire_angle(yaw),
            head_yaw: wire_angle(yaw),
            data: 0,
            movement: (0.0, 0.0, 0.0),
        }
        .to_raw()?;
        let mut out = vec![add];
        match &entity.body {
            mc_entity::EntityBody::Item(item) => {
                if let Some(item_id) = item.item_id() {
                    out.push(
                        mc_protocol::packets::play::SetEntityData {
                            entity_id: id.get(),
                            entries: vec![(
                                8,
                                mc_protocol::packets::play::MetadataValue::ItemStack {
                                    count: item.count(),
                                    item_id,
                                },
                            )],
                        }
                        .to_raw()?,
                    );
                }
            }
            mc_entity::EntityBody::Mob(mob) => {
                out.push(
                    mc_protocol::packets::play::SetEntityData {
                        entity_id: id.get(),
                        entries: vec![(
                            mc_protocol::packets::play::METADATA_INDEX_HEALTH,
                            mc_protocol::packets::play::MetadataValue::Float(mob.kind.max_health()),
                        )],
                    }
                    .to_raw()?,
                );
            }
            mc_entity::EntityBody::Orb(orb) => {
                // The orb's value rides the tracked slot, mirroring the
                // item-contents second packet above.
                out.push(
                    mc_protocol::packets::play::SetEntityData {
                        entity_id: id.get(),
                        entries: vec![(
                            mc_protocol::packets::play::METADATA_INDEX_ORB_VALUE,
                            mc_protocol::packets::play::MetadataValue::VarInt(orb.value),
                        )],
                    }
                    .to_raw()?,
                );
            }
            mc_entity::EntityBody::Player | mc_entity::EntityBody::Projectile(_) => {}
        }
        Ok(out)
    }

    /// Unload chunks no player can see any more.
    ///
    /// Without this, every chunk a player walks past stays resident (a chunk is
    /// ~384 KiB), so a long walk is an unbounded memory leak. The rule is simple and
    /// deliberately conservative:
    ///
    /// - keep anything within `view_distance + UNLOAD_MARGIN_CHUNKS` of any player
    ///   (each session's own radius, since P14-04);
    /// - never unload a **dirty** chunk — it holds edits that are not on disk yet,
    ///   and only [`Game::save_all`] persists those;
    /// - tell every player that had the chunk to forget it
    ///   (`forget_level_chunk`, P14-04), and drop it from their `sent_chunks`,
    ///   so walking back re-streams the chunk instead of leaving the client's
    ///   stale copy — or a hole — in the view.
    ///
    /// Nothing else holds a chunk index, so unloading cannot dangle: block changes
    /// carry their own coordinates and are re-resolved when broadcast.
    fn unload_distant_chunks(&mut self, report: &mut TickReport) -> ServerResult<()> {
        if self.sessions.is_empty() {
            // Nobody to keep chunks for. This is the headless/test shape, where
            // unloading would silently discard the world a test just built.
            return Ok(());
        }
        // Player chunk centres with their own radii, collected before the loop
        // so the sessions can be mutated (their `sent_chunks`) inside it.
        let centres: Vec<(i32, i32, i32)> = self
            .sessions
            .values()
            .map(|session| {
                let centre = session.chunk();
                (
                    centre.x,
                    centre.z,
                    session.view_distance.saturating_add(UNLOAD_MARGIN_CHUNKS),
                )
            })
            .collect();
        let loaded: Vec<ChunkPos> = self.world.chunk_positions().collect();
        let mut removed = 0usize;
        // (session, chunk) pairs the client must forget, collected before any
        // send borrows the sessions.
        let mut forgets: Vec<(ConnectionId, ChunkPos)> = Vec::new();
        for pos in loaded {
            if self.world.chunk(pos).is_some_and(|chunk| chunk.dirty) {
                trace!(?pos, "keeping a dirty chunk outside the view distance");
                continue;
            }
            let wanted = centres
                .iter()
                .any(|(x, z, radius)| (pos.x - x).abs() <= *radius && (pos.z - z).abs() <= *radius);
            if wanted {
                continue;
            }
            if self.world.unload_chunk(pos).is_some() {
                removed += 1;
                // AUDIT-09 B-06: the do-not-persist mark belongs to a *loaded*
                // placeholder. Holding it after the chunk is gone leaked one entry
                // per chunk a player ever visited (the set was only ever inserted
                // into, never cleared), and it bought nothing: a chunk that is not
                // loaded cannot be in `world.dirty_chunks()`, and a reload marks
                // itself again through the same two placeholder paths. Dropping it
                // here is what keeps the mark's meaning "this live chunk must not
                // be written".
                self.placeholder_without_storage.remove(&pos);
            }
            for session in self.sessions.values_mut() {
                if session.sent_chunks.remove(&pos) {
                    forgets.push((session.id, pos));
                }
            }
        }
        for (id, pos) in forgets {
            let packet = ForgetLevelChunk { x: pos.x, z: pos.z };
            self.send(id, &packet, report)?;
        }
        if removed > 0 {
            trace!(removed, "unloaded chunks outside the view distance");
        }
        Ok(())
    }

    /// Apply a client's view-distance setting (P14-04).
    ///
    /// Clamped to `2..=server maximum`: Vanilla lets the client ask for less
    /// than the server runs, never more. A change is confirmed with
    /// `set_chunk_cache_radius` (the client renders to the confirmed radius)
    /// and takes effect on the next stream/unload pass, which both read the
    /// session radius. An unknown session is ignored — settings can arrive
    /// before the join is applied.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the confirm packet cannot be encoded,
    /// like every other send path.
    fn apply_view_distance(
        &mut self,
        id: ConnectionId,
        distance: i8,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let radius = i32::from(distance).clamp(2, self.view_distance);
        let Some(session) = self.sessions.get_mut(&id) else {
            return Ok(());
        };
        if session.view_distance == radius {
            return Ok(());
        }
        session.view_distance = radius;
        let packet = SetChunkCacheRadius { radius };
        self.send(id, &packet, report)?;
        Ok(())
    }

    /// Build the `level_chunk_with_light` packet for a runtime chunk.
    ///
    /// Light is computed by `mc_world::light` over the chunk plus a one-block margin, so it crosses chunk
    /// borders, and every light section is then accounted for in a mask (P10-05):
    ///
    /// * a section that is uniformly the layer's default — 15 for sky, 0 for block — goes in the matching
    ///   `empty_*` mask, which is what a real server does and what keeps the packet small;
    /// * anything else gets a 2048-byte array and a bit in the matching mask.
    ///
    /// Bit `i` is light section `i`, which is world section `i - 1`, so the two sections outside the world
    /// (below and above) are included: they contain no blocks, so both are full sky and no block light. The
    /// earlier version of this function sent four **empty masks and no arrays**, which is why a real client
    /// rendered an unlit world.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when a block id is not in the registry —the
    /// alternative would be silently sending a wrong block.
    pub fn vanilla_chunk_packet(&self, chunk: &Chunk) -> ServerResult<LevelChunkWithLight> {
        let mut sections = Vec::with_capacity(chunk.sections.len());
        for section in &chunk.sections {
            // Build the `(palette, values)` pair once. A container that is one value
            // repeated is encoded by the wire codec in the `bits == 0` single-value
            // form; `PalettedContainer::new` derives that from the palette length, so
            // the same construction covers both shapes.
            let mut palette: Vec<u32> = Vec::new();
            let mut values: Vec<u32> = Vec::with_capacity(section.blocks.len());
            for id in &section.blocks {
                let wire_id = u32::try_from(*id).unwrap_or(0);
                let index = if let Some(found) = palette.iter().position(|entry| *entry == wire_id)
                {
                    found
                } else {
                    palette.push(wire_id);
                    palette.len() - 1
                };
                values.push(index as u32);
            }
            let block_states =
                WireContainer::new(palette, values, mc_core::packing::BLOCK_MIN_BITS);
            // One plains biome fills every cell, and the id is the one the client's own registry gives
            // plains — see [`PLAINS_BIOME_ID`]. Per-column biomes are not modelled yet, and the values array
            // is still the full cell count because the encoder validates the container's geometry.
            let biomes = WireContainer::new(
                vec![PLAINS_BIOME_ID],
                vec![0u32; BIOMES_PER_SECTION],
                NETWORK_BIOME_MIN_BITS,
            );
            sections.push(ChunkSection {
                block_count: section.non_empty_block_count,
                fluid_count: 0,
                block_states,
                biomes,
            });
        }
        // Light, over the chunk plus a margin so it crosses borders. An unloaded neighbour reads as `None`,
        // which `compute_chunk_light` treats as air — the same assumption the client makes about ungenerated
        // space.
        //
        // Read from the cache where it exists — `send_chunk` fills it before calling this — and compute
        // without keeping the result otherwise. This takes `&self`, so it cannot fill the cache itself; a
        // caller that forgets to pre-warm gets a correct packet at the old cost rather than a wrong one.
        let computed;
        let light = if let Some(cached) = self.world.cached_light(chunk.pos) {
            cached
        } else {
            computed = mc_world::light::compute_chunk_light(
                &self.registries.light,
                chunk.pos.x,
                chunk.pos.z,
                chunk.min_y(),
                chunk.sections.len(),
                |x, y, z| self.world.get_block_loaded(x, y, z),
            )?;
            &computed
        };

        let light = light_fields(light, chunk.sections.len())?;

        Ok(LevelChunkWithLight {
            chunk_x: chunk.pos.x,
            chunk_z: chunk.pos.z,
            heightmaps: vec![Heightmap {
                kind: HEIGHTMAP_WORLD_SURFACE,
                data: chunk.heightmap_long_array(),
            }],
            sections,
            block_entities: Vec::new(),
            sky_light_mask: light.sky_mask,
            block_light_mask: light.block_mask,
            empty_sky_light_mask: light.empty_sky_mask,
            empty_block_light_mask: light.empty_block_mask,
            sky_light: light.sky,
            block_light: light.block,
        })
    }
}
impl PhaseRunner for Game {
    fn run_phase(&mut self, tick: Tick, phase: TickPhase) -> ServerResult<()> {
        // This tick's report lives in `self`, but a phase needs `&mut self` for the
        // world, players and entities at the same time. Moving it out for the call
        // is a move of a handful of counters and keeps every phase signature
        // uniform.
        let mut report = std::mem::take(&mut self.report);
        let result = self.run_phase_inner(tick, phase, &mut report);
        self.report = report;
        result
    }
}
