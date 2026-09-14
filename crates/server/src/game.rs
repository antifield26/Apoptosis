//! The game loop: world, players, entities and the network bridge
//! (P04-03..P04-15, P05-01..P05-03).
//!
//! One [`Game`] owns the world, the connected players, the entity store and the
//! receiving end of the network bridge. [`crate::lifecycle::Server::run`] drives
//! it: each tick runs the six phases of [`mc_simulation::PHASE_ORDER`] through
//! [`mc_simulation::Scheduler`], which times each phase and folds the result into
//! [`mc_simulation::TickMetrics`].
//!
//! ## The six phases, and what each one does here
//!
//! | Phase | This crate's work | Status |
//! |---|---|---|
//! | [`TickPhase::Network`] | drain the inbound channel (bounded) and *queue* the work: player intents go to a pending buffer, joins/leaves apply inline | implemented |
//! | [`TickPhase::ScheduledTicks`] | none — block/fluid scheduled ticks are P05-05/P05-06 | **documented no-op** |
//! | [`TickPhase::Entities`] | per-entity timers, gravity + swept collision, landing/fall damage, and the AI hook | physics implemented, AI is a **documented no-op** (P05-11..14) |
//! | [`TickPhase::Players`] | apply the queued intents in arrival order, then player timers and physics | implemented |
//! | [`TickPhase::BlockEntities`] | none — block-entity behaviour is P06 | **documented no-op** |
//! | [`TickPhase::Broadcast`] | block changes, chunk streaming, world time, entity-removal sweep, chunk unloading | implemented |
//!
//! ### Why intents are queued instead of applied in the Network phase
//!
//! [`Game`]'s `PhaseRunner` implementation only *decodes and queues* during
//! [`TickPhase::Network`]: a movement or interaction intent is pushed onto the
//! pending-intent buffer and applied later, in the Players phase. The reason is
//! ordering, not tidiness. An intent that ran during Network would resolve against
//! the world as it stood at the *end of the previous* tick, so a block placed by
//! this tick's entity or scheduled-tick work would be invisible to a player action
//! that arrived in the same tick — and the same packet stream would then produce
//! different results depending on where the phase boundary happened to fall.
//! Applying intents in Players means every one of them resolves against the world
//! state this tick's earlier phases produced, which is the same state a client
//! observes in the Broadcast phase.
//!
//! Joins and leaves are still handled inline in Network: they are connection
//! lifecycle, not gameplay actions, and a join has to exist before the Players
//! phase can tick it.
//!
//! ## Threading (AGENTS.md section 8)
//!
//! - **Owner**: the tick thread owns `Game` outright; nothing else touches it.
//!   Phases run synchronously inside [`Game::tick`] — no `async`, no locks, no
//!   spawned work.
//! - **Primitive**: `tokio::sync::mpsc`, one bounded channel per direction, plus
//!   the per-connection outbound queue.
//! - **World storage**: `Game` may *own* the world handle — the tick thread hands
//!   it over with [`Game::with_seed_and_storage`] — or borrow it for the duration
//!   of a call ([`Game::save_all`]). One or the other, never both. The owned handle
//!   is a plain field reached through `&mut self`, not a lock and not a `RefCell`:
//!   there is exactly one thread, and the boundary being enforced is aliasing, so
//!   the compiler is the whole synchronisation story. While a save has the handle
//!   checked out, `Game` deliberately has no storage for that call and chunk loads
//!   fall back to the in-memory/placeholder path.
//! - **Ordering**: events apply in arrival order within a tick, and queued intents
//!   keep that same arrival order through the Players phase. Packets are queued per
//!   player, so two players observing the same change see it in the same tick and
//!   in the same relative order. Entity iteration is ascending by
//!   [`mc_entity::EntityId`].
//! - **Backpressure**: outbound uses `try_send`. A full queue disconnects that
//!   player rather than growing memory without bound; inbound drops are counted by
//!   the bridge and never block the connection task. The inbound drain, the
//!   per-tick intent budget and the per-tick chunk budget are all bounded, so one
//!   busy client cannot own a tick.
//! - **Failure**: a failed send affects one player only. Client-caused errors are
//!   refused inside the phase that saw them; a phase error means a programmer
//!   invariant broke and aborts the tick, which the lifecycle treats as fatal.
//! - **Shutdown**: [`Game::save_all`] flushes the world through a borrowed handle;
//!   [`Game::save_all_owned`] and [`Game::close_storage`] do the same for an owned
//!   one. Per-player data and entities are **not** persisted (see
//!   [`Game::save_all`]).
//!
//! ## Deterministic simulation (AGENTS.md section 3.6)
//!
//! Same state + same ordered inputs + same tick count ⇒ same result. Four things
//! hold that in place, and all four are requirements rather than accidents:
//!
//! 1. the **phase order** comes from [`mc_simulation::PHASE_ORDER`], a `const`
//!    array in a crate that knows nothing about gameplay;
//! 2. **entities are ticked in ascending [`mc_entity::EntityId`] order** (the store
//!    is a `BTreeMap`), collected into a `Vec` before the loop so a phase can
//!    mutate the store while iterating it;
//! 3. **queued intents are applied in arrival order**, the order the bridge
//!    drained them from the bounded channel;
//! 4. **chunk streaming is sorted by (distance, x, z)** before it is truncated to
//!    the per-tick budget.
//!
//! Randomness comes from a single seeded [`mc_simulation::RandomSource`] owned by
//! `Game` and chosen through [`Game::with_seed`]; nothing here draws from the
//! clock or from any other source. `tests/entity_lifecycle.rs` asserts that two
//! games built with the same seed and fed the same intents produce the same
//! [`TickReport`] sequence.
//!
//! ## Entity ownership: one source of truth, one projection
//!
//! [`Session::player`] (an [`mc_entity::Player`]) is the **authoritative** player
//! state: health, food, inventory, experience, game mode. The
//! [`mc_entity::Entity`] with `EntityBody::Player` in the entity store is a
//! **projection** of it — position, yaw/pitch and `on_ground`, refreshed once per
//! tick at the end of the Players phase. It exists so radius queries, packet work
//! and tests can see players through the same [`mc_entity::EntityStore`] as every
//! other entity, and so an entity id exists for the player on the wire.
//!
//! Nothing reads gameplay state *back* out of the projection: a query that needs
//! health, food or inventory goes through [`Game::player`]. The projection carries
//! no velocity of its own, because player movement is resolved from the client's
//! reported position and a second velocity would be a second, disagreeing source
//! of truth. Two writers for one field is the bug this split exists to prevent.
//!
//! ## Chunk loading and the data-loss rule
//!
//! A chunk is read from disk before it is ever *created*:
//! [`Game::load_or_create_chunk`] loads a stored chunk **clean**, and an all-air
//! placeholder is created only when nothing is stored. A chunk that was not read
//! and not written is never saved, so streaming a view over existing terrain
//! cannot overwrite that terrain with air. Chunks outside the view distance are
//! unloaded again (never while dirty), so a long walk does not accumulate memory.
//!
//! ## Simulated vs. not
//!
//! Simulated: movement with swept collision (players and non-player entities),
//! fall damage, block break and place with reach/targeting validation,
//! health/hunger/experience, death and respawn, chunk streaming (join and
//! chunk-border crossings) from memory *and* from disk, world time, entity
//! timers/gravity/despawn, chunk unloading outside the view distance.
//!
//! **Not** simulated, and therefore not claimed: mob AI, mob spawning, block and
//! fluid scheduled ticks, block-entity behaviour, redstone, fluids, lighting,
//! world generation, chat relay, commands, item pickup and merging, entity
//! persistence, per-player data persistence, and the `add_entity`/`remove_entities`
//! packets that would make non-player entities visible to a client. Each is
//! recorded in `docs/vanilla-parity/PARITY-MATRIX.md` and the phase reports, and
//! every no-op in this file says so at its definition.

// Simulation narrows and widens constantly: protocol fields are `f64`/`i32`, world
// coordinates are `i32` blocks, and block ids are `i32` while wire palettes are
// `u32`. Every site validates its range first (teleport caps, world bounds,
// registry lookups) or is lossless by construction; the same exemption is
// documented in the other binary-format crates.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use crate::storage::WorldService;
use mc_core::error::{ServerError, ServerResult};
use mc_core::tick::Tick;
use mc_entity::entity::{EntityBody, EntityId, EntityKind, EntityStore};
use mc_entity::inventory::Hand;
use mc_entity::item_entity::ItemEntity;
use mc_entity::player::{DamageOutcome, GameMode, Player};
use mc_entity::stack::ItemStack;
use mc_network::bridge::{ClientEvent, ClientEventKind, ConnectionId, GameEvents, OutboundSender};
use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::dimension::Dimension;
use mc_protocol::RawPacket;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BIOMES_PER_SECTION, BlockUpdate, ChunkSection, ContainerSetContent, ContainerSetSlot,
    GameEvent, HEIGHTMAP_WORLD_SURFACE, Heightmap, LevelChunkWithLight, LightUpdate,
    NETWORK_BIOME_MIN_BITS, PalettedContainer as WireContainer, PlayDisconnect, PlayIntent,
    PlayerPosition, Respawn, SetDefaultSpawnPosition, SetExperience, SetHealth, SetHeldSlot,
    SetTime, SystemChat, block_position, unpack_block_position,
};
use mc_protocol::text::TextComponent;
use mc_registry::Registries;
use mc_simulation::{PhaseRunner, RandomSource, Scheduler, TickMetrics, TickPhase};
use mc_world::chunk::Chunk;
use mc_world::light::LightArray;
use mc_world::{Aabb, Vec3, World};
use std::collections::{BTreeMap, BTreeSet};
use tracing::{debug, info, trace, warn};

/// A chunk's light as the wire wants it: four masks and two array lists.
///
/// Both `level_chunk_with_light` and `light_update` carry exactly this, so it is built once here rather than
/// twice. A drift between two copies would be invisible — both packets stay well-formed, and the client renders
/// whichever arrived last.
struct LightFields {
    sky_mask: Vec<u32>,
    block_mask: Vec<u32>,
    empty_sky_mask: Vec<u32>,
    empty_block_mask: Vec<u32>,
    sky: Vec<Vec<u8>>,
    block: Vec<Vec<u8>>,
}

/// Split computed light into the masks and arrays the wire carries (P10-05).
///
/// Every light section is accounted for: light section `i` is world section `i - 1`, so there is one below the
/// world and one above it, and both hold no blocks — full sky, no block light. A section that is uniformly the
/// layer's default (15 for sky, 0 for block) goes in the matching `empty_*` mask and needs no array; anything
/// else gets a 2 048-byte array and a bit in the matching mask. That is what a real server does and what keeps
/// the packet small.
///
/// # Errors
///
/// [`ServerError::Invariant`] for more light sections than a mask can index.
fn light_fields(
    light: &mc_world::light::ChunkLight,
    section_count: usize,
) -> ServerResult<LightFields> {
    let light_sections = section_count + 2;
    let mut fields = LightFields {
        sky_mask: Vec::new(),
        block_mask: Vec::new(),
        empty_sky_mask: Vec::new(),
        empty_block_mask: Vec::new(),
        sky: Vec::new(),
        block: Vec::new(),
    };
    for index in 0..light_sections {
        let outside = index == 0 || index == light_sections - 1;
        let (sky, block) = if outside {
            (
                LightArray::filled(mc_world::light::MAX_LIGHT),
                LightArray::filled(0),
            )
        } else {
            (light.sky[index - 1].clone(), light.block[index - 1].clone())
        };
        let index = u32::try_from(index).map_err(|_| {
            ServerError::Invariant("more light sections than a mask can index".to_owned())
        })?;
        if sky.uniform() == Some(mc_world::light::MAX_LIGHT) {
            fields.empty_sky_mask.push(index);
        } else {
            fields.sky_mask.push(index);
            fields.sky.push(sky.as_bytes().to_vec());
        }
        if block.uniform() == Some(0) {
            fields.empty_block_mask.push(index);
        } else {
            fields.block_mask.push(index);
            fields.block.push(block.as_bytes().to_vec());
        }
    }
    Ok(fields)
}

/// How far the spawn search looks, in blocks, and how finely.
///
/// A step of eight blocks can miss a one-block island, which does not matter: the search is looking for
/// somewhere to start a player, not for the closest dry block. The radius bounds the work — the whole spiral is
/// about four thousand height samples, each a few noise evaluations, and it runs once at startup.
const SPAWN_SEARCH_RADIUS: i32 = 256;
const SPAWN_SEARCH_STEP: i32 = 8;

/// The nearest column at or above sea level, searched outward in rings from `from_x, from_z`.
///
/// Returns `None` when the whole radius is water, in which case the caller keeps the stored spawn: an ocean
/// start is better than no answer, and the radius is deliberately large enough that it will not happen on the
/// shipped generator.
fn find_land_spawn(
    generator: &mc_worldgen::TerrainGenerator,
    from_x: i32,
    from_z: i32,
) -> Option<(i32, i32)> {
    let mut radius = SPAWN_SEARCH_STEP;
    while radius <= SPAWN_SEARCH_RADIUS {
        // Walk the ring: the four edges, stepping by the search step, so a ring costs 8 * radius / step probes.
        let mut offset = -radius;
        while offset <= radius {
            for (x, z) in [
                (from_x + offset, from_z - radius),
                (from_x + offset, from_z + radius),
                (from_x - radius, from_z + offset),
                (from_x + radius, from_z + offset),
            ] {
                if generator.surface_height(x, z) >= mc_worldgen::OVERWORLD_SEA_LEVEL {
                    return Some((x, z));
                }
            }
            offset += SPAWN_SEARCH_STEP;
        }
        radius += SPAWN_SEARCH_STEP;
    }
    None
}

/// `ClientboundGameEventPacket.Type.LEVEL_CHUNKS_LOAD_START` on the wire.
///
/// The server sends it to say "chunks are about to arrive", and a 26.x client's loading screen does not lift
/// until it has. **Measured, not inferred**: a real vanilla server's join sequence carries
/// `game_event` (clientbound play 38) with the body `26 0d 00000000` — id 38, event **13**, value `0.0`.
pub const GAME_EVENT_LEVEL_CHUNKS_LOAD_START: u8 = 13;

/// How many chunks a tick may relight and announce.
///
/// Each one is a full recompute of the chunk plus a packet of a few kilobytes, so this bounds the tick's cost
/// by the clock rather than by what a player did. Four is enough that a hand-placed block is announced the
/// same tick, and small enough that a burst cannot stall the server.
pub const LIGHT_UPDATES_PER_TICK: usize = 4;

/// How far a player can reach to break or place a block (Vanilla survival).
pub const REACH: f64 = 4.5;

/// Seed used when a caller does not pick one ([`Game::new`]).
///
/// A fixed constant rather than a clock reading, because AGENTS.md section 3.6
/// requires reproducibility: two servers started from the same world draw the same
/// random sequence. Callers that want a different sequence pick a seed with
/// [`Game::with_seed`].
pub const DEFAULT_RANDOM_SEED: i64 = 0;

/// Ticks between periodic metric summaries (30 s at 20 TPS).
///
/// [`crate::lifecycle`] logs one line per interval; the constant lives here because
/// the interval is a property of the simulation, not of the logger.
pub const METRICS_LOG_INTERVAL_TICKS: u64 = 600;

/// Chunks streamed per player per tick.
///
/// A join wants `(2r+1)²` chunks; sending them all in one tick would queue a burst
/// no socket drains instantly, so the view fills over a few ticks.
pub const CHUNKS_PER_TICK: usize = 64;

/// Player intents drained from the network per tick.
///
/// The Network phase drains up to this many events; the Players phase applies
/// everything that produced. The bound is what stops one client's packet flood from
/// owning a tick (AGENTS.md section 10) without dropping the excess, which stays
/// queued in the channel for the next tick.
const PENDING_INTENT_BUDGET: usize = 256;

/// `player_action` status: start digging.
const ACTION_START_DESTROY_BLOCK: i32 = 0;
/// `player_action` status: finish digging (creative instant break, survival result).
const ACTION_FINISH_DESTROY_BLOCK: i32 = 2;
/// Whether a `player_action` status carries a block position that must be in reach.
///
/// `0` start-destroy, `1` abort-destroy and `2` finish-destroy all name the block
/// being mined. Every other status (drop, drop-all, release-use, swap-with-offhand)
/// is about the player's own hands and carries `BlockPos.ZERO`.
#[must_use]
const fn targets_a_block(status: i32) -> bool {
    matches!(
        status,
        ACTION_START_DESTROY_BLOCK | ACTION_ABORT_DESTROY_BLOCK | ACTION_FINISH_DESTROY_BLOCK
    )
}

/// `player_action` status: abort digging (the client changed its mind).
const ACTION_ABORT_DESTROY_BLOCK: i32 = 1;
/// `player_action` status: drop the held item.
const ACTION_DROP_ITEM: i32 = 3;
/// `player_action` status: swap the held item with the offhand.
const ACTION_SWAP_ITEM_WITH_OFFHAND: i32 = 6;

/// `client_command` action: perform respawn.
const CLIENT_COMMAND_RESPAWN: i32 = 0;

/// Blocks a player (or any other entity) falls before damage starts (Vanilla: 3).
const FALL_DAMAGE_THRESHOLD: f64 = 3.0;

/// What the structure decorator has done.
///
/// Counted rather than logged because "no structure appeared" and "a structure appeared and blends with
/// the terrain" look identical from outside a chunk, and only the decorator can tell them apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StructureStats {
    /// Chunks the decorator looked at.
    pub considered: usize,
    /// Chunks the selection picked a template for.
    pub selected: usize,
    /// Blocks those templates wrote.
    pub blocks_written: usize,
    /// Selections the placement policy refused.
    pub refused: usize,
}

/// Player cap before the lifecycle supplies the configured one.
///
/// Vanilla's `server.properties` default, so a game built without a config (tests, and
/// the `Game::new` path) reports a plausible number rather than a placeholder zero.
pub const DEFAULT_MAX_PLAYERS: u32 = 20;

/// Ticks between food/regeneration steps (Vanilla's `foodTickTimer` period).
///
/// 80 ticks is 4 seconds, which is the interval Vanilla uses for both natural
/// regeneration and starvation damage.
pub const FOOD_TICK_INTERVAL: u64 = 80;

/// Ticks of damage immunity granted after a hit (Vanilla's 10-tick window).
///
/// Read from `Entity::invulnerable_ticks`, which [`Game::damage_entity`] now
/// consults; before Audit 03 the field was only decremented, so an entity in fire
/// took a hit every tick.
pub const INVULNERABLE_TICKS: u32 = 10;

/// Chunks kept loaded beyond the view distance before a chunk is unloaded.
///
/// Unloading exactly at the view distance would thrash: a player walking along a
/// chunk border would drop and re-stream the same chunk every few ticks. Two chunks
/// of hysteresis costs ~768 KiB per player and removes that entirely.
const UNLOAD_MARGIN_CHUNKS: i32 = 2;

/// How close to a block boundary an entity's feet must be to count as resting.
///
/// The collision solver binary-searches to within a fraction of a block, so a box
/// that came to rest on a surface can be a few millionths of a block above or below
/// it. `1e-3` accepts that and still rejects an entity that is genuinely suspended
/// inside the air block above the floor — which is the case this constant exists to
/// separate from "standing on the floor".
const REST_EPSILON: f64 = 1e-3;

/// Downward velocity added to a non-player entity each tick, in blocks/tick².
///
/// The Vanilla value for a non-living entity that is not a boat or a minecart
/// (`Entity.getGravity`). Dropped items do **not** use it: an item owns its own
/// constant pair in [`mc_entity::item_entity`] and is stepped through
/// [`ItemEntity::tick_physics`]. Living mobs use `0.08`; that arrives with the mob
/// code in P05-11..14 and is deliberately not guessed here.
const ENTITY_GRAVITY: f64 = 0.04;

/// Biome id for `minecraft:plains` in the 26.1.2 biome registry.
///
/// Phase 04 streams one biome for the whole world; the id comes from the
/// jar-extracted registry table (`docs/research/provenance.md`). P07 replaces this
/// with real biome data.
/// `minecraft:plains` in the registry **the client is sent**, which is not 0.
///
/// The client resolves a chunk's biome ids against the registry this server hands it \u2014 a verbatim replay of
/// vanilla's \u2014 and in that registry id 0 is `minecraft:badlands`. This constant was `0` while being named for
/// plains, so every column of every chunk was painted as badlands: red sand and orange terracotta under a hazy
/// sky, wherever the player stood, with the terrain and the light entirely correct.
///
/// Measured, not guessed: `crates/network/src/registry_data/config-payload.bin` is the exact byte sequence the
/// client receives, and the identifier run after `minecraft:worldgen/biome` is **65 names in alphabetical
/// order** ending at `minecraft:chat_type`, the next registry. `minecraft:plains` is the 41st of them.
///
/// **Per-column biomes are still not modelled.** `Biome::index()` is this crate's own six-biome slot, a
/// different numbering from the client's registry, so sending it would be a new defect rather than a fix. Every
/// cell is plains, as the comment always claimed, and now that is what the number means.
/// Public so a test can hold it against the registry it claims to index \u2014 see
/// `crates/server/tests/registry_ids.rs`.
pub const PLAINS_BIOME_ID: u32 = 40;

/// One connected player's server-side state.
/// One connected player's simulation state.
///
/// pub(crate) because the command handlers are a sibling module: they read a player's
/// name, position and permission to build a command source. The individual fields are
/// crate-visible for the same reason, and nothing outside the crate sees any of it.
pub(crate) struct Session {
    /// The connection this session belongs to.
    pub(crate) id: ConnectionId,
    /// **Authoritative** player state; the entity-store entry is a projection of
    /// this (see the module docs).
    pub(crate) player: Player,
    /// The entity this connection controls, chosen by the entity store so ids are
    /// unique across every entity and never reused.
    entity: EntityId,
    outbound: OutboundSender,
    /// Chunks already sent, so streaming is incremental.
    sent_chunks: BTreeSet<ChunkPos>,
    /// Position at the start of this tick, for fall-damage accounting.
    tick_start_y: f64,
    /// What this connection is permitted to do.
    ///
    /// Set at join from `ops.json` (listed uuids hold their file level,
    /// everyone else [`mc_command::PermissionLevel::All`]) and never raised
    /// afterwards: there is no in-game grant path, because `/op` does not
    /// write the file. A stale comment here once claimed no storage was read;
    /// `ops_e2e` proves the join path reads it (P08-08 review).
    pub(crate) permission: mc_command::PermissionLevel,
    /// The container window this player has open.
    ///
    /// Every player always has the player-inventory menu (`window 0`) open; other
    /// menus (chests, furnaces) are P06-03's job. The menu is the **authority** for
    /// item placement: `container_click` is decoded, validated against this state
    /// and applied here, never trusted.
    menu: mc_container::Menu,
    /// Whether this player may receive world packets.
    ready: bool,
}

impl Session {
    fn aabb(&self) -> Aabb {
        Aabb::player(to_world(self.player.position))
    }

    /// The chunk this player currently stands in.
    fn chunk(&self) -> ChunkPos {
        chunk_of(self.player.position.x, self.player.position.z)
    }
}

/// The validated half of a join: everything `join` needs before it allocates an
/// entity id for the newcomer.
///
/// Grouped so the admission check and the session construction cannot drift
/// apart: both take the same two fields the refusal paths validated.
struct PreparedJoin {
    profile: mc_entity::GameProfile,
    inventory: mc_entity::PlayerInventory,
}

/// The five raw integers of a `container_click`, grouped so the handler takes one
/// argument instead of five positional ones that are trivially swappable.
#[derive(Debug, Clone, Copy)]
struct RawClick {
    window_id: i32,
    state_id: i32,
    slot: i16,
    button: i8,
    click_type: i32,
}

/// A read-only view of a session, for tests and diagnostics.
///
/// Deliberately not a `&Session`: the menu is full of mutable state that a caller
/// has no business reaching into during a tick. This exposes exactly the reads a
/// test needs.
#[derive(Clone, Copy)]
pub struct SessionView<'a> {
    session: &'a Session,
}

impl SessionView<'_> {
    /// The menu slot count.
    #[must_use]
    pub fn menu_slot_count(&self) -> usize {
        self.session.menu.slot_count()
    }

    /// The window id.
    #[must_use]
    pub fn menu_window_id(&self) -> u8 {
        self.session.menu.window_id()
    }

    /// The revision counter.
    #[must_use]
    pub fn menu_state_id(&self) -> i32 {
        self.session.menu.state_id()
    }

    /// The stack in a menu slot, empty when out of range.
    #[must_use]
    pub fn menu_slot(&self, slot: usize) -> mc_entity::stack::ItemStack {
        self.session.menu.display_stack(slot)
    }

    /// The cursor stack.
    #[must_use]
    pub fn menu_cursor(&self) -> mc_entity::stack::ItemStack {
        self.session.menu.cursor()
    }

    /// Total items in the player\'s own inventory, excluding the cursor.
    ///
    /// The instrument the duplication regression tests assert against: a drop or a
    /// placement must move this number, and a no-op click must not.
    #[must_use]
    pub fn inventory_total(&self) -> i64 {
        let slots = self.session.player.inventory.stored_slots();
        (0..slots)
            .map(|index| i64::from(self.session.player.inventory.slot(index).count()))
            .sum()
    }
}

/// Result of one tick (tests and telemetry).
///
/// This is a **single tick's** delta, not a running total: every counter starts at
/// zero at the beginning of the tick that produced it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    /// Events drained from the network this tick.
    pub events: usize,
    /// Block changes broadcast to at least one player.
    pub block_changes: usize,
    /// Light updates sent this tick (`light_update` packets).
    ///
    /// Counted because a light update that stops being sent is invisible: the client keeps rendering stale
    /// light and reports nothing.
    pub light_updates: usize,
    /// Chunk packets queued this tick.
    pub chunks_sent: usize,
    /// Packets queued this tick.
    pub packets: usize,
    /// Players disconnected this tick.
    pub disconnects: usize,
    /// Connections whose outbound queue overflowed and who must be dropped.
    ///
    /// This lives on the report rather than on `Game` because the send helpers take
    /// `&self` (they are called while a session is borrowed), so they cannot push to
    /// a `Game` field. The Network phase drains this list, which is what actually
    /// makes the "a full queue disconnects that player" contract in the module docs
    /// true; before that it was a counter for a disconnect that never happened.
    pub overflowed: Vec<ConnectionId>,
    /// Non-player entities ticked by the Entities phase this tick.
    pub entities_ticked: usize,
    /// Block entities retired this tick because their block changed.
    pub block_entities_changed: usize,
    /// Entities swept out of the store by the Broadcast phase this tick.
    pub removed_entities: usize,
    /// Ids swept this tick, ascending.
    ///
    /// Carried so tests can name what disappeared and so the P05 packet work has
    /// the batch it needs. **No `remove_entities` packet is encoded yet**: clients
    /// are not told, which is stated here rather than implied away.
    pub removed_ids: Vec<EntityId>,
}

/// The simulation.
pub struct Game {
    registries: Registries,
    /// The open world, when this `Game` owns it (the production tick path).
    ///
    /// `None` for the borrowed-storage constructors, where a [`WorldService`] is
    /// passed per call instead.
    storage: Option<WorldService>,
    world: World,
    /// Live sessions, keyed by connection.
    ///
    /// `pub(crate)` because the command handlers are a sibling module: `/list` and
    /// `/say` iterate every session, and `/tp` resolves a name to one. Making the whole
    /// struct crate-visible would expose far more than that; this exposes exactly the map.
    pub(crate) sessions: BTreeMap<ConnectionId, Session>,
    /// Every live entity in the dimension (P05-03).
    entities: EntityStore,
    /// Which entity each connection controls.
    entity_ids: BTreeMap<ConnectionId, EntityId>,
    events: GameEvents,
    view_distance: i32,
    /// The seed every random draw in this simulation derives from.
    random_seed: i64,
    /// The live source; its state advances with the work done, so a replay from the
    /// same seed takes the same draws.
    random: RandomSource,
    /// Runs and times the six phases of a tick.
    scheduler: Scheduler,
    /// Intents drained this tick, applied by the Players phase in arrival order.
    pending_intents: Vec<(ConnectionId, PlayIntent)>,
    /// Chunks whose light changed and whose `light_update` has not been sent yet.
    ///
    /// A queue rather than an immediate send because the cost per chunk is a full recompute and the traffic is
    /// kilobytes; a burst of block changes would otherwise stall the tick. Nothing is dropped — a chunk stays
    /// here until it is sent — so a burst is delayed rather than lost.
    pending_light: BTreeSet<ChunkPos>,
    /// This tick's counters, shared by every phase while it runs.
    report: TickReport,
    tick: u64,
    /// Retained for the tests that exercise the disconnect path directly; the
    /// production path carries overflow on the `TickReport` (see its field docs).
    overflowed: Vec<ConnectionId>,
    /// The terrain generator, consulted only when nothing is stored for a chunk.
    ///
    /// `None` when the registry lacks a palette block, in which case the all-air
    /// placeholder is used and a warning was logged at construction: a server that cannot
    /// generate must still start, because refusing to boot over a terrain palette would
    /// take a working world offline.
    generator: Option<mc_worldgen::TerrainGenerator>,
    /// Data functions this game has loaded, for `/function`.
    ///
    /// Empty until [`Game::load_functions_from`] is called, which is what a server with no data
    /// pack legitimately has.
    pub(crate) functions: mc_data::function::FunctionRegistry,
    /// Structure templates loaded from the packs, for decoration.
    structures: mc_worldgen::structures::StructureRegistry,
    /// How many chunks the structure decorator has looked at, and what it found.
    ///
    /// A counter rather than a log line: "no structure appeared" and "a structure appeared and blends
    /// with the terrain" are indistinguishable from the outside, and only the decorator knows which
    /// happened. Exposed so a test can assert the difference.
    structure_stats: StructureStats,
    /// Running total from the oak-tree pass, so a world that has none is visible without standing in it.
    tree_stats: mc_worldgen::features::TreeStats,
    /// The placement rule, derived **once** when the templates are installed.
    ///
    /// `StructureSet::from_registry` sorts the names and caps the selectable slice, which is O(n log n)
    /// over the pack. Deriving it per chunk would make a chunk's cost depend on the installed pack
    /// size, which is exactly what the cap exists to prevent — so it is derived here instead.
    structure_set: mc_worldgen::structures::StructureSet,
    /// Who may run operator commands, loaded from `ops.json` at construction.
    ///
    /// Loaded once rather than per login, matching Vanilla's startup read. A change to the
    /// file therefore needs a restart — stated because the alternative (reloading on every
    /// login) is a different feature that would let a grant take effect with no audit trail.
    ///
    /// An empty list is the common case and is not an error; a **malformed** file is reported
    /// by `Server::open_world` and leaves this empty, because refusing to boot over an
    /// operator file would take a working world offline.
    operators: crate::ops::OperatorList,
    /// The configured player cap, for `/list`.
    ///
    /// One value rather than the whole `ServerConfig`: the alternative was a new
    /// parameter on four constructors and every call site, including six tests, to serve
    /// a single line of output. The lifecycle sets it from the real config; the default
    /// is Vanilla's own.
    max_players: u32,
    /// Offset applied to the time-of-day broadcast, so `/time set` survives the
    /// per-second recomputation from the tick counter.
    time_offset: i64,
    /// Set by `/stop`; the lifecycle drains it and shuts down.
    ///
    /// `Game` records the request and the *lifecycle* acts on it, which keeps the
    /// ownership direction one-way: the server drives the game, never the reverse.
    shutdown_requested: bool,
    /// Every loaded block entity, keyed by position.
    ///
    /// Owned here rather than in `mc-world` because a block entity is *state*, not
    /// geometry: the world answers "what block is at x,y,z", this answers "what does
    /// that block hold". `mc-container` owns the model, this owns the instance.
    block_entities: mc_container::BlockEntityStore,
    /// Chunks that became placeholders while this game could not read storage.
    ///
    /// A game with a borrowed `WorldService` cannot tell "nothing stored" from "I
    /// cannot look", so a placeholder it creates must never be written back: that is
    /// exactly how a placeholder used to overwrite real terrain (Audit 03). These
    /// positions are skipped by `save_all` and reported instead.
    placeholder_without_storage: BTreeSet<ChunkPos>,
}

impl Game {
    /// Build a game around an open world, taking the spawn from `level.dat`.
    ///
    /// The storage handle is **borrowed for this call only**: the game keeps no
    /// reference to it, so chunk loads fall back to the in-memory/placeholder path
    /// and saving goes through [`Game::save_all`]. The deterministic RNG seed
    /// defaults to [`DEFAULT_RANDOM_SEED`]; use [`Game::with_seed`] to choose one.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the registry tables cannot be read.
    pub fn new(
        storage: &WorldService,
        view_distance: i32,
        events: GameEvents,
    ) -> ServerResult<Self> {
        Self::with_seed(storage, view_distance, events, DEFAULT_RANDOM_SEED)
    }

    /// Build a game with an explicit RNG seed.
    ///
    /// Two games built with the same seed, the same world and the same ordered
    /// input sequence produce the same [`TickReport`] sequence (AGENTS.md
    /// section 3.6); `tests/entity_lifecycle.rs` asserts exactly that.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the registry tables cannot be read.
    pub fn with_seed(
        storage: &WorldService,
        view_distance: i32,
        events: GameEvents,
        seed: i64,
    ) -> ServerResult<Self> {
        Self::build(Some(storage), None, view_distance, events, seed)
    }

    /// Build a game that **owns** the world handle, loading chunks from disk on
    /// demand.
    ///
    /// This is the production tick-thread constructor: `Game` keeps the
    /// [`WorldService`] so the Broadcast phase can read a chunk from disk before
    /// creating a placeholder for it, and the world spawn comes from its
    /// `level.dat`. The tick thread must then drive shutdown through
    /// [`Game::close_storage`] (or take the handle back with
    /// [`Game::into_storage`]) instead of calling [`WorldService::close`] itself.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the registry tables cannot be read.
    pub fn with_seed_and_storage(
        storage: WorldService,
        view_distance: i32,
        events: GameEvents,
        seed: i64,
    ) -> ServerResult<Self> {
        Self::build(None, Some(storage), view_distance, events, seed)
    }

    fn build(
        borrowed: Option<&WorldService>,
        owned: Option<WorldService>,
        view_distance: i32,
        events: GameEvents,
        seed: i64,
    ) -> ServerResult<Self> {
        Self::build_with_operators(
            borrowed,
            owned,
            view_distance,
            events,
            seed,
            crate::ops::OperatorList::new(),
        )
    }

    /// As [`Game::build`], with an explicit operator list.
    ///
    /// A separate function rather than a sixth parameter on the constructors: `ops.json` is
    /// loaded by the lifecycle, which knows the world directory, and a test that wants
    /// operators can say so here. Threading it through `new`/`with_seed`/
    /// `with_seed_and_storage` would add a parameter to four signatures and every call site
    /// to serve one production caller.
    ///
    /// # Errors
    ///
    /// As for [`Game::build`].
    pub fn build_with_operators(
        borrowed: Option<&WorldService>,
        owned: Option<WorldService>,
        view_distance: i32,
        events: GameEvents,
        seed: i64,
        operators: crate::ops::OperatorList,
    ) -> ServerResult<Self> {
        let registries = Registries::vanilla()?;
        let mut world = World::new(Dimension::Overworld, registries.blocks.clone());
        // The spawn comes from whichever handle this game was given; a game with
        // neither (only reachable from a test that passes no storage) keeps the
        // world's own default.
        let spawn = borrowed
            .and_then(|storage| storage.storage().level())
            .or_else(|| owned.as_ref().and_then(|storage| storage.storage().level()))
            .map_or((0, 64, 0), |level| {
                (level.spawn.x, level.spawn.y, level.spawn.z)
            });
        // A level.dat written by another tool (or hand-edited) may name a y outside
        // the world; clamp it rather than dropping the player into the void.
        let min_y = i32::from(world.min_section_y()) * mc_world::SECTION_HEIGHT;
        let max_y = (i32::from(world.min_section_y()) + world.section_count() as i32)
            * mc_world::SECTION_HEIGHT;
        let y = spawn.1.clamp(min_y + 1, max_y - 2);
        world.set_spawn(spawn.0, y, spawn.2);
        // Terrain generation is a *simulation* concern: `World` is the chunk container
        // and stays constructible without a generator, which a large part of the test
        // suite relies on. A registry that lacks a palette block leaves this `None` with a
        // warning rather than failing construction — refusing to boot would take a working
        // *stored* world offline over a terrain feature it may never need.
        let generator = {
            // A `WorldSeed` is a label rather than a quantity, so every bit pattern of the
            // simulation seed is legal and no validation is needed here.
            let context =
                mc_worldgen::WorldgenContext::overworld(mc_worldgen::WorldSeed::from_raw(seed));
            match mc_worldgen::TerrainGenerator::new(context, &registries.blocks) {
                Ok(generator) => Some(generator),
                Err(error) => {
                    warn!(%error, "terrain generation disabled: the palette could not resolve");
                    None
                }
            }
        };

        // **A spawn on land.** A fresh world's `level.dat` names `(0, 64, 0)`, and roughly two columns in five
        // of this generator are ocean — so the default dropped the player into open sea, where every chunk in
        // view is ocean floor: no grass, no trees, no biome variety, and a sea bed that is *correctly* dark
        // because water attenuates sky light. Vanilla searches for a suitable spawn for the same reason.
        //
        // Only a spawn that is **in water** is moved: a stored world's own choice is its own.
        if let Some(generator) = generator.as_ref() {
            let (sx, _, sz) = world.spawn();
            if generator.surface_height(sx, sz) < mc_worldgen::OVERWORLD_SEA_LEVEL
                && let Some((lx, lz)) = find_land_spawn(generator, sx, sz)
            {
                let y = generator
                    .surface_height(lx, lz)
                    .saturating_add(1)
                    .clamp(min_y + 1, max_y - 2);
                world.set_spawn(lx, y, lz);
                info!(
                    from_x = sx,
                    from_z = sz,
                    to_x = lx,
                    to_z = lz,
                    y,
                    "the stored spawn is under water; moved to the nearest land"
                );
            }
        }

        Ok(Self {
            registries,
            storage: owned,
            world,
            sessions: BTreeMap::new(),
            entities: EntityStore::new(),
            entity_ids: BTreeMap::new(),
            events,
            view_distance: view_distance.clamp(2, 16),
            random_seed: seed,
            random: RandomSource::new(seed),
            scheduler: Scheduler::new(),
            pending_intents: Vec::new(),
            pending_light: BTreeSet::new(),
            report: TickReport::default(),
            tick: 0,
            overflowed: Vec::new(),
            generator,
            functions: mc_data::function::FunctionRegistry::new(),
            structures: mc_worldgen::structures::StructureRegistry::new(),
            structure_stats: StructureStats::default(),
            tree_stats: mc_worldgen::features::TreeStats::default(),
            // Derived from an empty registry: the rule for "no templates", which is the correct
            // initial state. `set_structures` re-derives it when a pack loads.
            structure_set: mc_worldgen::structures::StructureSet::from_registry(
                &mc_worldgen::structures::StructureRegistry::new(),
            ),
            operators,
            max_players: DEFAULT_MAX_PLAYERS,
            time_offset: 0,
            shutdown_requested: false,
            block_entities: mc_container::BlockEntityStore::new(),
            placeholder_without_storage: BTreeSet::new(),
        })
    }

    /// The world (tests, diagnostics, admin tooling).
    #[must_use]
    pub const fn world(&self) -> &World {
        &self.world
    }

    /// Mutable world access.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The terrain generator, when this game has one.
    ///
    /// Exposed for the spawn check: whether a world starts a player on land is a property of the generator and
    /// the stored spawn together, and there is no other way to ask from outside.
    #[must_use]
    pub const fn terrain_generator(&self) -> Option<&mc_worldgen::TerrainGenerator> {
        self.generator.as_ref()
    }

    /// The registry tables.
    #[must_use]
    pub const fn registries(&self) -> &Registries {
        &self.registries
    }

    /// Connected player count.
    #[must_use]
    pub fn player_count(&self) -> usize {
        self.sessions.len()
    }

    /// Whether a connection id is an active player.
    #[must_use]
    pub fn has_player(&self, id: ConnectionId) -> bool {
        self.sessions.contains_key(&id)
    }

    /// A player's state.
    #[must_use]
    pub fn player(&self, id: ConnectionId) -> Option<&Player> {
        self.sessions.get(&id).map(|session| &session.player)
    }

    /// Mutable player state (tests and admin actions).
    ///
    /// Changing the position here does not move the entity projection; the next
    /// Players phase republishes it (see the module docs on the two sources).
    pub fn player_mut(&mut self, id: ConnectionId) -> Option<&mut Player> {
        self.sessions
            .get_mut(&id)
            .map(|session| &mut session.player)
    }

    /// Active connection ids, in deterministic order.
    pub fn players(&self) -> impl Iterator<Item = ConnectionId> + '_ {
        self.sessions.keys().copied()
    }

    /// Every live entity, ascending by id.
    #[must_use]
    pub const fn entity_store(&self) -> &EntityStore {
        &self.entities
    }

    /// Mutable entity store access (tests, the P08 benchmark's workload setup and
    /// admin tooling).
    ///
    /// This is a raw door into the store: an entity spawned here is ticked by the
    /// Entities phase from the next tick onwards, exactly as one spawned by
    /// [`Game::spawn_item`] is, but nothing validates the position or the body. A
    /// caller that needs the invariants (a non-empty stack, a position inside the
    /// world) should go through the typed helpers.
    pub const fn entity_store_mut(&mut self) -> &mut EntityStore {
        &mut self.entities
    }

    /// The entity a connection controls, if it is connected.
    #[must_use]
    pub fn entity_id_of(&self, id: ConnectionId) -> Option<EntityId> {
        self.entity_ids.get(&id).copied()
    }

    /// The seed this simulation draws randomness from.
    #[must_use]
    pub const fn random_seed(&self) -> i64 {
        self.random_seed
    }

    /// The live random source for this simulation.
    ///
    /// **Nothing draws from it yet.** The sequence matters as soon as mob spawning
    /// (P05-11) and AI decisions exist; fixing the seeding policy now — a field
    /// with a documented default plus [`Game::with_seed`] — means those systems
    /// inherit reproducibility instead of having it retrofitted. Exposing the
    /// source is also what lets a test assert two runs drew the same sequence once
    /// there is a consumer.
    #[must_use]
    pub const fn random(&self) -> &RandomSource {
        &self.random
    }

    /// Current tick number. The loop entry point is [`Game::tick`], so the getter
    /// needs a distinct name.
    #[must_use]
    pub const fn tick_count(&self) -> u64 {
        self.tick
    }

    /// The most recent tick's report, as returned by [`Game::tick`].
    ///
    /// Empty before the first tick. Used by the lifecycle heartbeat, which needs
    /// this tick's counts rather than a running total.
    #[must_use]
    pub const fn report(&self) -> &TickReport {
        &self.report
    }

    /// Rolling tick metrics: percentiles, overruns and per-phase means.
    ///
    /// The samples are the scheduler's: one per tick that ran (including a tick
    /// that aborted in a phase), with the six phase costs recorded separately.
    #[must_use]
    pub const fn metrics(&self) -> &TickMetrics {
        self.scheduler.metrics()
    }

    /// Spawn point.
    #[must_use]
    pub fn spawn(&self) -> (i32, i32, i32) {
        self.world.spawn()
    }

    /// View distance in chunks.
    #[must_use]
    pub const fn view_distance(&self) -> i32 {
        self.view_distance
    }

    /// Spawn a dropped-item entity holding `stack`.
    ///
    /// This closes the Phase 04 gap where a dropped stack was taken out of the
    /// inventory and then discarded. The physics live in [`mc_entity::item_entity`];
    /// this crate runs them (Entities phase) and reaps the entity when the despawn
    /// timer fires.
    ///
    /// What is **not** implemented, and is not faked: item pickup, item merging and
    /// the `add_entity` packet that would make the drop visible to a client. A
    /// dropped item exists on the server and in radius queries; no client is told
    /// about it yet (P05-15 owns the entity spawn/despawn packets).
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] for an empty stack (an invisible, unkillable
    /// entity), and [`ServerError::Invariant`] when the entity cap is reached.
    pub fn spawn_item(&mut self, stack: ItemStack, position: Vec3) -> ServerResult<EntityId> {
        self.spawn_item_owned(stack, position, None)
    }

    /// Spawn a dropped item with an owner, for the player-drop path.
    fn spawn_item_owned(
        &mut self,
        stack: ItemStack,
        position: Vec3,
        owner: Option<EntityId>,
    ) -> ServerResult<EntityId> {
        if stack.is_empty() {
            return Err(ServerError::InvalidAction(
                "refusing to spawn an item entity for an empty stack".to_owned(),
            ));
        }
        self.entities.spawn(
            EntityBody::Item(ItemEntity::new(stack, owner)),
            to_entity(position),
        )
    }

    /// Give the owned world handle back (shutdown, or handing it to a save worker).
    ///
    /// Returns `None` when this game never owned one.
    #[must_use]
    pub fn into_storage(self) -> Option<WorldService> {
        self.storage
    }

    /// The owned world handle, when there is one.
    #[must_use]
    pub const fn storage(&self) -> Option<&WorldService> {
        self.storage.as_ref()
    }

    /// Mutable access to the owned world handle (autosave, admin tooling).
    pub fn storage_mut(&mut self) -> Option<&mut WorldService> {
        self.storage.as_mut()
    }

    /// Flush the owned world handle and close it.
    ///
    /// A no-op for a game that does not own storage. Afterwards the game has no
    /// storage: further chunk loads use the in-memory/placeholder path, and another
    /// save needs [`Game::save_all`] with a borrowed handle.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a chunk cannot be encoded or `level.dat`
    /// cannot be written.
    pub fn close_storage(&mut self) -> ServerResult<()> {
        let Some(mut service) = self.storage.take() else {
            return Ok(());
        };
        // `queue_dirty_chunks` now reports how many placeholders it skipped, which
        // `close_storage` has no use for — the warning is emitted inside `save_all`, and a
        // shutdown path that also logged it would say the same thing twice.
        let result = self
            .queue_dirty_chunks(&mut service)
            .and_then(|_skipped| service.close().map(|_| ()));
        if let Err(error) = &result {
            warn!(%error, "closing the world failed");
        }
        result
    }

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
                self.tick_scheduled();
                Ok(())
            }
            TickPhase::Entities => {
                self.phase_entities(report);
                Ok(())
            }
            TickPhase::Players => self.phase_players(report),
            TickPhase::BlockEntities => {
                self.tick_block_entities();
                Ok(())
            }
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
    // Takes `&mut self` because that is the phase's shape, not because this body
    // needs it: the queue it will drain lives on `Game`.
    #[allow(clippy::unused_self)]
    fn tick_scheduled(&mut self) {}

    /// Phase 5: **documented no-op**.
    ///
    /// Block-entity behaviour (furnaces, hoppers, chests, spawners, signs) is P06:
    /// it needs the block-entity registry, per-chunk block-entity maps in
    /// [`mc_world::chunk::Chunk`] and the container transaction model in
    /// `mc-entity`. None of those exist yet, so there is nothing to tick.
    // See `tick_scheduled` for why the receiver is part of the signature.
    #[allow(clippy::unused_self)]
    fn tick_block_entities(&mut self) {}

    /// Phase 3: tick every non-player entity, ascending by id.
    ///
    /// Infallible today: [`Game::tick_entity`] has no error path left (a refused
    /// world write is logged and skipped). It gains a `ServerResult` when entity AI
    /// lands and starts making fallible world queries.
    fn phase_entities(&mut self, report: &mut TickReport) {
        // Collected first: the loop mutates the store, so it cannot hold the
        // iterator. `ids()` is ascending because the store is a `BTreeMap`.
        let ids: Vec<EntityId> = self.entities.ids().collect();
        for id in ids {
            // The AI hook runs first, as Vanilla orders `tick` before the move. It
            // is a no-op today; see `tick_entity_ai`.
            self.tick_entity_ai(id);
            if self.tick_entity(id) {
                report.entities_ticked += 1;
            }
        }
    }

    /// Per-entity AI and behaviour. **Documented no-op.**
    ///
    /// Mob AI — goal selectors, pathfinding, target acquisition, breeding,
    /// panic/flee — is P05-11..P05-14 and belongs to `mc-entity`'s AI modules,
    /// which are written in parallel with this change. The hook exists so the phase
    /// order is already right when that lands: an AI that moves an entity *before*
    /// that entity's own physics step is what Vanilla does, and wiring it in later
    /// should be a body change here, not a reordering of the phases.
    // The receiver is unused today and is the whole point of the hook: the AI will
    // read the world and mutate the entity through `self`.
    #[allow(clippy::unused_self)]
    fn tick_entity_ai(&mut self, id: EntityId) {
        let _ = id;
    }

    /// Timers, gravity, collision, landing and fall damage for one entity.
    ///
    /// Returns whether the entity was ticked. Players are not: their state lives in
    /// [`Session::player`] and is handled by the Players phase.
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
            } else {
                velocity.y -= ENTITY_GRAVITY;
                entity.velocity = velocity;
            }
        }

        // 2. Integrate against the world. `move_with_collision` sweeps the box, so
        //    a fast or badly-framed step cannot tunnel through a floor.
        let result = self.world.move_with_collision(hitbox, to_world(velocity));
        let applied = to_world(position).plus(result.delta);
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
            entity.position = to_entity(applied);
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
            let died = self.damage_entity(id, damage);
            debug!(%id, damage, died, "entity fall damage");
        }
        true
    }

    /// Apply damage to a living entity, flagging it removed when it dies.
    ///
    /// Returns whether this hit was lethal. Non-living entities are immune rather
    /// than silently damaged: an item entity has no health to reduce.
    fn damage_entity(&mut self, id: EntityId, amount: f32) -> bool {
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
        // Resistance and the other damage modifiers, which nothing applied before.
        let effects: Vec<mc_entity::effect::ActiveEffect> =
            entity.effects.values().copied().collect();
        let multiplier = mc_entity::effect::damage_taken_multiplier(&effects);
        let amount = if amount.is_finite() && amount > 0.0 {
            amount * multiplier as f32
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
        if entity.health > 0.0 {
            return false;
        }
        entity.removed = true;
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
        self.stream_all(report)?;
        self.send_world_time(report, tick)?;
        self.sweep_entity_removals(report);
        self.unload_distant_chunks();
        Ok(())
    }

    /// Take every entity flagged for removal out of the store, recording the batch
    /// on the report.
    fn sweep_entity_removals(&mut self, report: &mut TickReport) {
        let removed = self.entities.sweep_removed();
        if removed.is_empty() {
            return;
        }
        // A player's own entity going away means the connection is gone; drop the
        // mapping so `entity_id_of` cannot hand out a dead id.
        let dead: BTreeSet<EntityId> = removed.iter().copied().collect();
        self.entity_ids.retain(|_, id| !dead.contains(id));
        debug!(count = removed.len(), "entities swept");
        report.removed_entities += removed.len();
        // No `remove_entities` packet yet: clients are told nothing, so a despawned
        // item would linger on screen. P05-15 owns entity spawn/despawn packets,
        // which is why the batch is carried on the report instead.
        report.removed_ids = removed;
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

    fn broadcast_block_changes(&mut self, report: &mut TickReport) -> ServerResult<()> {
        let changes = self.world.take_block_changes();
        for change in changes {
            let pos = mc_container::BlockPos::new(change.x, change.y, change.z);
            if let Some(retired) = self.block_entities.remove(pos) {
                let dropped = retired.data.total_items();
                if dropped > 0 {
                    // The items themselves are not spawned one-by-one here: doing so
                    // needs a per-item position and is P06-08's hopper/drop work.
                    // Reporting the count keeps the loss visible rather than silent.
                    info!(
                        %pos,
                        kind = retired.kind().name(),
                        items = dropped,
                        "block entity retired with contents; they are not yet dropped"
                    );
                }
                report.block_entities_changed += 1;
            }
            let packet = BlockUpdate {
                position: block_position(change.x, change.y, change.z),
                block_state: change.new_id,
            }
            .to_raw()?;
            if self.broadcast_chunk(change.pos, &packet, report) > 0 {
                report.block_changes += 1;
            }
            // The changed chunk *and* the neighbours `World::set_block` invalidated, since the light the
            // client holds for those changed too.
            self.pending_light.insert(change.pos);
            let local_x = change.x.rem_euclid(16);
            let local_z = change.z.rem_euclid(16);
            if local_x == 0 {
                self.pending_light
                    .insert(ChunkPos::new(change.pos.x - 1, change.pos.z));
            }
            if local_x == 15 {
                self.pending_light
                    .insert(ChunkPos::new(change.pos.x + 1, change.pos.z));
            }
            if local_z == 0 {
                self.pending_light
                    .insert(ChunkPos::new(change.pos.x, change.pos.z - 1));
            }
            if local_z == 15 {
                self.pending_light
                    .insert(ChunkPos::new(change.pos.x, change.pos.z + 1));
            }
        }
        Ok(())
    }

    /// The loaded block entities.
    #[must_use]
    pub const fn block_entities(&self) -> &mc_container::BlockEntityStore {
        &self.block_entities
    }

    /// Mutable block-entity access, for the server-side paths that create or fill a
    /// container (a chest being opened, a furnace being lit).
    pub const fn block_entities_mut(&mut self) -> &mut mc_container::BlockEntityStore {
        &mut self.block_entities
    }

    /// Place a block entity, returning whatever it displaced.
    ///
    /// The caller must handle the displaced entity: a container replaced by another
    /// still holds its items, and dropping them silently is the bug this return value
    /// exists to prevent.
    pub fn place_block_entity(
        &mut self,
        pos: mc_container::BlockPos,
        kind: mc_container::BlockEntityKind,
    ) -> Option<mc_container::BlockEntity> {
        self.block_entities
            .insert(mc_container::BlockEntity::new(pos, kind))
    }

    /// Send the world time once a second.
    fn send_world_time(&self, report: &mut TickReport, tick: Tick) -> ServerResult<()> {
        if !tick.is_multiple_of(20) {
            return Ok(());
        }
        // **26.1.2 removed `time_of_day` from the wire** (P10-03, KD-43): the captured vanilla payload is
        // `i64` + one byte, and the client derives the time of day from the `world_clock` registry instead.
        // So this packet can no longer carry the server's time of day, and `/time set` no longer moves a
        // real client's sky. That is not a regression from this change — the old encoding carried
        // `time_of_day` and a real client rejected the whole packet — but it is a capability the server does
        // not have until 26.1's clock mechanism is implemented.
        //
        // `flag` mirrors the captured value rather than interpreting it.
        let packet = SetTime {
            world_age: tick as i64,
            flag: 0,
        }
        .to_raw()?;
        self.broadcast_all(&packet, report);
        Ok(())
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
            ClientEventKind::Left => self.leave(event.id),
        }
        Ok(())
    }

    /// Remove a connection's player and mark its entity for the sweep.
    fn leave(&mut self, id: ConnectionId) {
        let Some(mut session) = self.sessions.remove(&id) else {
            return;
        };
        // Whatever the player was carrying on the cursor goes back to their
        // inventory. Dropping it with the session would destroy items on every
        // disconnect (Audit 04 A2), and the cursor is not part of the inventory, so
        // `write_back_inventory` alone would not recover it.
        let carried = session.menu.cursor();
        if !carried.is_empty() {
            let leftover = session.player.inventory.add_stack(carried);
            if leftover.is_empty() {
                debug!(id = %id, items = carried.count(), "returned the cursor to the inventory");
            } else {
                // The inventory was full. Report the loss rather than hiding it; a
                // drop entity needs a position and this path has already released the
                // session, so it is recorded as a warn.
                warn!(
                    id = %id,
                    lost = leftover.count(),
                    "the inventory was full, so cursor items were lost on disconnect"
                );
            }
        }
        // Flag rather than remove: the Broadcast phase sweeps once per tick, so a
        // departure produces one batch rather than a removal per event.
        if let Some(entity) = self.entities.get_mut(session.entity) {
            entity.removed = true;
        }
        self.entity_ids.remove(&id);
        info!(
            id = %id,
            name = %session.player.profile.name,
            entity = %session.entity,
            "player left"
        );
    }

    // One linear admission: the cap, then the profile, then the inventory, then
    // the entity id, then the menu. Splitting it would put the refusal ordering
    // (which the full-server test depends on) behind a call for no benefit.
    #[allow(clippy::too_many_lines)]
    fn join(
        &mut self,
        id: ConnectionId,
        profile: &mc_network::auth::GameProfile,
        outbound: OutboundSender,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        // The cap is enforced here and not at the socket: the network layer admits
        // connections with headroom for status pings and handshakes, and the game
        // loop is where "a player" exists. An operator whose entry sets
        // `bypassesPlayerLimit` joins anyway (P08-06). Every refusal below
        // disconnects *that* client rather than failing the tick, which the
        // lifecycle treats as fatal (AGENTS.md sections 9 and 10).
        if self.is_full_for(&profile.id.to_string()) {
            warn!(id = %id, name = %profile.name, "refused a join: the server is full");
            refuse_join(&outbound, "The server is full");
            return Ok(());
        }
        let Some(prepared) = self.prepare_join(id, profile, &outbound) else {
            return Ok(());
        };
        // One expression for the join position so the entity projection, the
        // authoritative `Player` and the `player_position` packet cannot disagree.
        let (sx, sy, sz) = self.world.spawn();
        let at = Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
        // The entity store allocates the id, so it is unique across every entity in
        // the dimension and never reused — the property the wire protocol needs.
        let Ok(entity) = self.entities.spawn(EntityBody::Player, to_entity(at)) else {
            warn!(id = %id, "refused a join: the entity store is full");
            refuse_join(&outbound, "Server entity limit reached");
            return Ok(());
        };
        let entity_id = entity.get();

        let mut player = Player::new(
            prepared.profile,
            entity_id,
            "minecraft:overworld",
            prepared.inventory,
        );
        player.position = to_entity(at);
        player.game_mode = GameMode::Survival;

        // The menu is the authority for item placement, so it is built here and the
        // player's inventory is mirrored into it. `mirror_inventory` is one of the
        // two places the two models meet; `write_back_inventory` is the other.
        let mut menu = match self.new_player_menu() {
            Ok(menu) => menu,
            Err(error) => {
                warn!(id = %id, %error, "could not build the player menu");
                refuse_join(&outbound, "The server could not open your inventory.");
                return Ok(());
            }
        };
        menu.set_creative(player.game_mode.is_creative());
        mirror_inventory(&mut menu, &player.inventory);

        self.entity_ids.insert(id, entity);
        info!(
            id = %id,
            name = %profile.name,
            entity_id,
            x = sx,
            y = sy,
            z = sz,
            "player joined"
        );
        self.sessions.insert(
            id,
            Session {
                id,
                player,
                entity,
                outbound,
                sent_chunks: BTreeSet::new(),
                tick_start_y: f64::from(sy),
                // The operator list is the authority: a listed uuid gets its file level,
                // and anyone else is level 0. `ops.json` stores the hyphenated uuid, which is
                // what `Uuid`'s `Display` produces — the conversion is a real one, since the
                // in-memory id is a `Uuid`.
                permission: self.operators.level_for(&profile.id.to_string()),
                menu,
                ready: false,
            },
        );

        // The network layer already sent JoinGame; this is the world-side
        // continuation: the level-load signal, position, spawn marker, vitals, then terrain.
        //
        // **This one is not optional, and nothing complains when it is missing.** A 26.x client's loading
        // screen dismisses on `LevelLoadTracker.isLevelReady()`, which only becomes true once
        // `loadingPacketsReceived()` has moved the tracker out of `WaitingForServer` — and the only caller of
        // that is `ClientPacketListener.handleGameEvent`. So the client sits on "Loading terrain" forever,
        // with every packet involved well-formed and no error anywhere (KD-50).
        //
        // The values are quoted from a real server's capture rather than from the enum, whose numeric ids I
        // could not read: `game_event` (clientbound play 38), body `26 0d 00000000` — id 38, event **13**,
        // value `0.0`.
        self.send(
            id,
            &GameEvent {
                event: GAME_EVENT_LEVEL_CHUNKS_LOAD_START,
                value: 0.0,
            },
            report,
        )?;
        self.send(
            id,
            &PlayerPosition {
                x: f64::from(sx) + 0.5,
                y: f64::from(sy),
                z: f64::from(sz) + 0.5,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: 0.0,
                pitch: 0.0,
                flags: 0,
                teleport_id: 1,
            },
            report,
        )?;
        self.send(
            id,
            &SetDefaultSpawnPosition {
                // The dimension leads the packet since 26.1; a real client rejects a body without it
                // (P10-03, KD-40, captured from a vanilla server). The server is overworld-only, so this is
                // the same identifier `registry_data::OVERWORLD` names.
                dimension: "minecraft:overworld".to_owned(),
                position: block_position(sx, sy, sz),
                yaw: 0.0,
                pitch: 0.0,
            },
            report,
        )?;
        self.send_vitals(id, report)?;
        if let Some(session) = self.sessions.get_mut(&id) {
            session.ready = true;
        }
        self.send(
            id,
            &SystemChat {
                content: TextComponent::literal("Welcome to the Rust Minecraft server."),
                overlay: false,
            },
            report,
        )?;
        // The terrain itself goes out through `stream_all` later in this same tick,
        // which shares the per-tick chunk budget.
        Ok(())
    }

    /// The profile and inventory a join needs, or `None` when the join is refused.
    ///
    /// Split out of `join` so the admission reads as one screen: the profile the
    /// entity model rejects and the registry that cannot build an inventory both
    /// disconnect *that* client, and both refusals happen before an entity id is
    /// allocated for them.
    fn prepare_join(
        &self,
        id: ConnectionId,
        profile: &mc_network::auth::GameProfile,
        outbound: &OutboundSender,
    ) -> Option<PreparedJoin> {
        let Ok(entity_profile) = mc_entity::GameProfile::new(profile.id.to_string(), &profile.name)
        else {
            warn!(id = %id, name = %profile.name, "refused a join with an unusable profile");
            refuse_join(outbound, "Invalid player profile");
            return None;
        };
        match mc_entity::inventory::inventory_for_registry(&self.registries.items) {
            Ok(inventory) => Some(PreparedJoin {
                profile: entity_profile,
                inventory,
            }),
            Err(error) => {
                warn!(id = %id, %error, "refused a join: the registry cannot build an inventory");
                refuse_join(outbound, "Server inventory registry unavailable");
                None
            }
        }
    }

    // One flat dispatch over the play intents, in wire-id order. Splitting it would
    // put the per-intent validation behind a jump, and this is the function a reader
    // checks against the protocol.
    #[allow(clippy::too_many_lines)]
    fn apply_intent(
        &mut self,
        id: ConnectionId,
        intent: PlayIntent,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let Some(session) = self.sessions.get(&id) else {
            return Ok(());
        };
        // Dead and spectator players do not act (Vanilla behaviour) -- except that
        // a dead player may ask to respawn, which is the one action that has to
        // work while dead.
        let respawn_request = matches!(intent, PlayIntent::ClientCommand { action } if action == CLIENT_COMMAND_RESPAWN);
        if !respawn_request
            && (session.player.game_mode == GameMode::Spectator || !session.player.is_alive())
        {
            return Ok(());
        }
        match intent {
            PlayIntent::MovePlayerPos { x, y, z, on_ground } => {
                self.move_player(id, x, y, z, None, on_ground);
            }
            PlayIntent::MovePlayerPosRot {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground,
            } => self.move_player(id, x, y, z, Some((yaw, pitch)), on_ground),
            PlayIntent::MovePlayerRot {
                yaw,
                pitch,
                on_ground,
            } => {
                // Rotation is client-authoritative in Vanilla, but a non-finite
                // value would poison every later look-vector computation, so it is
                // rejected the same way position is (Audit 03).
                if !yaw.is_finite() || !pitch.is_finite() {
                    debug!(id = %id, "refusing a non-finite rotation");
                    return Ok(());
                }
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.player.yaw = yaw;
                    session.player.pitch = pitch;
                    session.player.on_ground = on_ground;
                }
            }
            PlayIntent::MovePlayerStatusOnly { on_ground } => {
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.player.on_ground = on_ground;
                }
            }
            PlayIntent::PlayerAction {
                status,
                position,
                facing,
                ..
            } => self.apply_player_action(id, status, position, facing, report)?,
            PlayIntent::UseItemOn {
                position,
                face,
                hand,
                ..
            } => self.apply_use_item_on(id, position, face, hand, report),
            PlayIntent::SetCarriedItem { slot } => self.apply_hotbar(id, slot, report)?,
            PlayIntent::ClientCommand { action } => {
                self.apply_client_command(id, action, report)?;
            }
            PlayIntent::Chat { message, .. } => {
                info!(id = %id, %message, "player chat (relay lands in P07)");
                self.send(
                    id,
                    &SystemChat {
                        content: TextComponent::literal("Chat relay is not implemented yet."),
                        overlay: false,
                    },
                    report,
                )?;
            }
            PlayIntent::ChatCommand { command } => {
                // The dispatcher validates the name, the permission and the arguments
                // before any handler runs, so a handler cannot be reached with a command
                // the source may not use or arguments that do not fit (P07-05).
                self.dispatch_command(id, &command, report)?;
            }
            PlayIntent::ContainerClick {
                window_id,
                state_id,
                slot,
                button,
                click_type,
            } => {
                self.apply_container_click(
                    id,
                    RawClick {
                        window_id,
                        state_id,
                        slot,
                        button,
                        click_type,
                    },
                    report,
                );
            }
            PlayIntent::ContainerClose { window_id } => {
                // Closing the player's own inventory is a no-op: it cannot be
                // closed. Closing anything else is P06-03's job (other menus do not
                // exist yet), and saying so beats a silent success.
                debug!(id = %id, window_id, "container close (only the player menu exists)");
            }
            PlayIntent::Swing { .. }
            | PlayIntent::AcceptTeleportation { .. }
            | PlayIntent::UseItem { .. } => {}
        }
        Ok(())
    }

    /// The operator list this game loaded.
    ///
    /// Exposed so a caller can report which operators a running server knows about, and so a
    /// test can assert that the *list* and a session's level agree — a session-only check
    /// cannot catch a level that came from somewhere other than the file.
    #[must_use]
    pub const fn operators(&self) -> &crate::ops::OperatorList {
        &self.operators
    }

    /// The configured maximum player count, for `/list`.
    #[must_use]
    pub const fn max_players(&self) -> u32 {
        self.max_players
    }

    /// Whether a uuid's operator entry asks for a player-limit bypass.
    ///
    /// The check is its own method so the join path names the rule it enforces:
    /// a listed operator with `bypassesPlayerLimit` joins a full server rather
    /// than being refused with it. An absent or malformed entry grants nothing.
    #[must_use]
    pub fn player_limit_bypass(&self, uuid: &str) -> bool {
        self.operators
            .get(uuid)
            .is_some_and(|entry| entry.bypasses_player_limit)
    }

    /// Whether `uuid` must be refused because the server is full.
    ///
    /// Split out of `join` so the four-way refusal fits the line budget: the
    /// rule is one predicate (at cap and no bypass entry), not inline
    /// arithmetic at the call site.
    fn is_full_for(&self, uuid: &str) -> bool {
        self.sessions.len() >= self.max_players as usize && !self.player_limit_bypass(uuid)
    }

    /// Set the player cap. The lifecycle is the authority for this value.
    pub const fn set_max_players(&mut self, max_players: u32) {
        self.max_players = max_players;
    }

    /// Every online player's name, ascending.
    #[must_use]
    pub fn player_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .sessions
            .values()
            .map(|session| session.player.profile.name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// The session for a connection, for tests and diagnostics.
    #[must_use]
    pub fn session(&self, id: ConnectionId) -> Option<SessionView<'_>> {
        let session = self.sessions.get(&id)?;
        Some(SessionView { session })
    }

    /// The menu slot count of a player's open window.
    #[must_use]
    pub fn menu_slot_count(&self, id: ConnectionId) -> Option<usize> {
        self.sessions.get(&id).map(|s| s.menu.slot_count())
    }

    /// The window id of a player's open menu.
    #[must_use]
    pub fn menu_window_id(&self, id: ConnectionId) -> Option<u8> {
        self.sessions.get(&id).map(|s| s.menu.window_id())
    }

    /// The revision counter of a player's open menu.
    #[must_use]
    pub fn menu_state_id(&self, id: ConnectionId) -> Option<i32> {
        self.sessions.get(&id).map(|s| s.menu.state_id())
    }

    /// Items across a player's whole window plus their cursor.
    ///
    /// The conservation instrument for tests: a click may move items between these
    /// places but must not change the total (except throw and creative clone).
    #[must_use]
    pub fn menu_total_items(&self, id: ConnectionId) -> Option<i64> {
        self.sessions.get(&id).map(|s| s.menu.total_items())
    }

    /// The time-of-day offset applied on top of the tick counter.
    #[must_use]
    pub const fn time_offset(&self) -> i64 {
        self.time_offset
    }

    /// Set the time-of-day offset.
    pub fn set_time_offset(&mut self, offset: i64) {
        self.time_offset = offset.rem_euclid(24_000);
    }

    /// Whether a command has asked the server to stop.
    #[must_use]
    pub const fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Record a shutdown request from a command.
    pub fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    /// Move the invoking source to a resolved block position.
    ///
    /// The destination is resolved against collision the same way a gameplay move is, so
    /// a teleport into a wall lands on top of it rather than inside it.
    ///
    /// # Errors
    ///
    /// Returns the reason as a message, because a command's failure is a message to a
    /// player rather than an error condition.
    pub(crate) fn teleport_source(
        &mut self,
        source: &mc_command::CommandSource,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<(), String> {
        if !self.in_build_range(y) {
            return Err(format!(
                "Cannot teleport to y={y}: outside the world's build range."
            ));
        }
        let target = Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
        // Find the connection whose name matches, since a command source carries a name
        // rather than an id (the framework has no notion of connections).
        let Some(id) = self
            .sessions
            .iter()
            .find(|(_, session)| session.player.profile.name == source.name)
            .map(|(id, _)| *id)
        else {
            return Err("That player is no longer online.".to_owned());
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return Err("That player is no longer online.".to_owned());
        };
        // A landed-on-surface resolution: the destination, or the first free spot above
        // it when it is inside a block.
        let resolved = self.world.find_surface(x, z, y).map_or(target, |surface| {
            Vec3::new(f64::from(x) + 0.5, f64::from(surface), f64::from(z) + 0.5)
        });
        session.player.position = mc_entity::player::Vec3::new(resolved.x, resolved.y, resolved.z);
        session.player.on_ground = false;
        session.tick_start_y = resolved.y;
        // Terrain may not be streamed for the destination yet; `stream_for` picks it up
        // on the next tick because the chunk it computes from has changed.
        debug!(name = %source.name, x, y, z, "teleported by command");
        Ok(())
    }

    /// Re-mirror the menu from the authoritative inventory and tell the client.
    ///
    /// Called after any action that mutated the inventory without going through the
    /// menu (drop, offhand swap, block placement). Without it the menu keeps a stale
    /// view until the next click and the client is never told, which is half of the
    /// dual-copy problem Audit 04 found.
    ///
    /// Deliberately sends **per-slot** updates rather than a whole-window resync: the
    /// action touched one or two slots, and a full `container_set_content` on every
    /// block placement would be a packet per placed block.
    fn sync_menu_from_inventory(&mut self, id: ConnectionId, report: &mut TickReport) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        // Record what the client currently believes before overwriting it.
        let before: Vec<mc_entity::stack::ItemStack> = (0..session.menu.slot_count())
            .map(|slot| session.menu.display_stack(slot))
            .collect();
        mirror_inventory(&mut session.menu, &session.player.inventory);

        let mut changed: Vec<(i16, mc_protocol::packets::play::ItemStack)> = Vec::new();
        for (slot, previous) in before.iter().enumerate() {
            let now = session.menu.display_stack(slot);
            if now != *previous {
                changed.push((slot as i16, wire_stack(now)));
            }
        }
        let state = session.menu.state_id();
        let window = session.menu.window_id().cast_signed();
        if changed.is_empty() {
            return;
        }
        // The state id advances because the window the client is looking at changed;
        // a client that clicks afterwards must carry the new revision.
        session.menu.bump_state();
        let state = session.menu.state_id().max(state);

        for (slot, item) in changed {
            let packet = ContainerSetSlot {
                window_id: window,
                state_id: state,
                slot,
                item,
            };
            if let Err(error) = self.send(id, &packet, report) {
                debug!(id = %id, %error, "could not send a post-action slot update");
            }
        }
    }

    /// Whether the menu and the authoritative inventory agree, and by how much
    /// they differ.
    ///
    /// Returns the number of slots whose contents differ, summed across the player's
    /// 41 storage slots; `None` when there is no session. A non-zero value means the
    /// two views have diverged, which is the condition that produced item
    /// duplication (Audit 04 A1). Exposed for the regression tests, and cheap enough
    /// to be worth asserting in a debug build.
    #[must_use]
    pub fn menu_inventory_divergence(&self, id: ConnectionId) -> Option<usize> {
        let session = self.sessions.get(&id)?;
        let Some(container) = session.menu.container(0) else {
            // No player container, so there is nothing to compare; reporting a
            // divergence would be wrong, so this is a distinct answer.
            return None;
        };
        let slots = container.len().min(session.player.inventory.stored_slots());
        let mut differing = 0;
        for index in 0..slots {
            if container.get(index) != session.player.inventory.slot(index) {
                differing += 1;
            }
        }
        Some(differing)
    }

    /// What a player currently holds on the cursor.
    #[must_use]
    pub fn menu_cursor(&self, id: ConnectionId) -> Option<mc_entity::stack::ItemStack> {
        self.sessions.get(&id).map(|s| s.menu.cursor())
    }

    /// The stack in a menu slot.
    #[must_use]
    pub fn menu_slot(&self, id: ConnectionId, slot: usize) -> Option<mc_entity::stack::ItemStack> {
        self.sessions
            .get(&id)
            .and_then(|s| (slot < s.menu.slot_count()).then(|| s.menu.display_stack(slot)))
    }

    /// Put `count` of a named item into a player's menu and inventory.
    ///
    /// A test and administration helper: there is no creative inventory or `give`
    /// command yet (P07), so this is how a test establishes a known starting stack.
    /// It goes through the menu's own container so the two views stay mirrored, and
    /// it refuses an unknown item name rather than silently doing nothing.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the name is not in the item registry;
    /// [`ServerError::InvalidAction`] when the session does not exist.
    pub fn grant_item(
        &mut self,
        id: ConnectionId,
        item_name: &str,
        count: i32,
    ) -> ServerResult<()> {
        let item_id = self.registries.items.id(item_name)?;
        let stack = mc_entity::stack::ItemStack::new(item_id, count)?;
        // `Menu::set_slot` clamps to the slot limit, so a 64-count grant of a stack-1
        // item would silently become 1 and lose 63 (Audit 04 B4). Refusing is the
        // honest behaviour for a *server-authored* value: it is a caller bug.
        let allowed = mc_entity::stack::StackSizeTable::resolve(&self.registries.items)?;
        let limit = allowed.max_stack_size(item_id);
        if count > limit {
            return Err(ServerError::InvalidAction(format!(
                "{count} {item_name} exceeds that item's stack limit of {limit}"
            )));
        }
        let Some(session) = self.sessions.get_mut(&id) else {
            return Err(ServerError::InvalidAction(format!(
                "no session for {id} to grant an item to"
            )));
        };
        // Mirror first, so a grant starts from the authoritative inventory rather
        // than replacing whatever the menu last saw (the same direction as a click).
        mirror_inventory(&mut session.menu, &session.player.inventory);
        // Menu slot 36 is hotbar slot 0 in the player layout.
        session.menu.set_slot(36, stack)?;
        write_back_inventory(&session.menu, &mut session.player.inventory);
        Ok(())
    }

    /// Build the player-inventory menu with the resolved stack-size table.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the stack-size table cannot be resolved
    /// (a registry problem, i.e. startup-time corruption), or an invariant error
    /// from [`mc_container::Menu::player`], which would mean the layout constants
    /// disagree with the container size.
    fn new_player_menu(&self) -> ServerResult<mc_container::Menu> {
        let stack_sizes = mc_entity::stack::StackSizeTable::resolve(&self.registries.items)?;
        let slots = mc_entity::inventory::PlayerInventory::new(stack_sizes.clone()).stored_slots();
        let container = mc_container::Container::new(mc_container::ContainerKind::Player, slots)?;
        mc_container::Menu::player(mc_container::PLAYER_WINDOW_ID, container, stack_sizes)
    }

    /// Decode, validate and apply a `container_click`.
    ///
    /// Every step can refuse without mutating: a bad window id, an unknown click
    /// type, a slot this menu does not have, or a stale state id. A stale state id
    /// is not an error — it resynchronises the client, which is the whole point of
    /// the state id.
    fn apply_container_click(&mut self, id: ConnectionId, raw: RawClick, report: &mut TickReport) {
        let Ok(click) = mc_container::Click::new(
            raw.window_id,
            raw.state_id,
            raw.slot,
            raw.button,
            raw.click_type,
        ) else {
            debug!(id = %id, "refusing a malformed container click");
            return;
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        // **Authority direction.** `PlayerInventory` is the authoritative state and
        // the menu is a per-click transaction view over it, so the inventory is
        // mirrored *in* before the click is applied. Without this, a player action
        // that mutated only the inventory (Q-drop, offhand swap, block placement)
        // would be reverted by the unconditional write-back below — which was a
        // client-reachable item duplication (Audit 04 A1).
        mirror_inventory(&mut session.menu, &session.player.inventory);
        let outcome = match session.menu.apply_click(&click) {
            Ok(outcome) => outcome,
            Err(error) => {
                debug!(id = %id, %error, "container click refused");
                return;
            }
        };

        if outcome.full_resync {
            // The client's view is stale: send the whole window again. This is the
            // correction path, so it is not an error and is not rate-limited away.
            let contents = session.menu.full_contents();
            let state = session.menu.state_id();
            // The menu addresses windows as u8 (its own bound); the wire uses i8.
            let wire_window = session.menu.window_id().cast_signed();
            let packet = ContainerSetContent {
                window_id: wire_window,
                state_id: state,
                slots: contents.iter().copied().map(wire_stack).collect(),
                carried: wire_stack(session.menu.cursor()),
            };
            // The borrow of `session` ends here; the send needs `&self`.
            if let Err(error) = self.send(id, &packet, report) {
                debug!(id = %id, %error, "could not send a container resync");
            }
            return;
        }

        // Per-slot deltas for everything that changed.
        let mut updates: Vec<(i16, mc_protocol::packets::play::ItemStack)> = Vec::new();
        for menu_slot in &outcome.changed_slots {
            updates.push((
                *menu_slot as i16,
                wire_stack(session.menu.display_stack(usize::from(*menu_slot))),
            ));
        }
        let cursor = session.menu.cursor();
        let state = session.menu.state_id();
        let window = session.menu.window_id().cast_signed();

        for (menu_slot, stack) in updates {
            let packet = ContainerSetSlot {
                window_id: window,
                state_id: state,
                slot: menu_slot,
                item: stack,
            };
            if let Err(error) = self.send(id, &packet, report) {
                debug!(id = %id, %error, "could not send a container slot update");
            }
        }

        // Dropped items become real entities, which is what makes a throw visible
        // rather than a silent deletion.
        if !outcome.dropped.is_empty() {
            let position = self
                .sessions
                .get(&id)
                .map_or(mc_entity::player::Vec3::default(), |session| {
                    session.player.position
                });
            // A thrown item appears just in front of the thrower's feet, which is
            // where Vanilla drops it from.
            let position = to_world(position);
            for stack in &outcome.dropped {
                if let Err(error) = self.spawn_item(*stack, position) {
                    warn!(id = %id, %error, "a dropped item could not be spawned");
                }
            }
        }

        // Flush the accepted transaction back onto the authoritative inventory.
        // This is the other half of the pair: the menu was mirrored *in* at the top of
        // this function, so the two can only disagree for the duration of one click,
        // during which nothing else runs.
        if let Some(session) = self.sessions.get_mut(&id) {
            write_back_inventory(&session.menu, &mut session.player.inventory);
        }

        // The cursor is not part of the window payload, so a change to it needs its
        // own update: Vanilla sends it as slot -1 in the player window.
        if outcome.cursor_changed {
            let packet = ContainerSetSlot {
                window_id: window,
                state_id: state,
                slot: -1,
                item: wire_stack(cursor),
            };
            if let Err(error) = self.send(id, &packet, report) {
                debug!(id = %id, %error, "could not send the cursor update");
            }
        }
    }

    // ------------------------------------------------------------- movement

    /// Apply a client-reported position, resolved against collision.
    ///
    /// The client is authoritative about where it *is* (that is how Vanilla
    /// movement works), but the server decides whether the move is *possible*: the
    /// requested position becomes a step from the last accepted one, clipped by
    /// solid blocks. A client therefore cannot walk through walls or fly, and any
    /// disagreement produces a correction packet.
    fn move_player(
        &mut self,
        id: ConnectionId,
        x: f64,
        y: f64,
        z: f64,
        rotation: Option<(f32, f32)>,
        on_ground: bool,
    ) {
        let Some(session) = self.sessions.get(&id) else {
            return;
        };
        if !x.is_finite() || !y.is_finite() || !z.is_finite() {
            debug!(id = %id, "rejected non-finite movement");
            return;
        }
        let current = Vec3::new(
            session.player.position.x,
            session.player.position.y,
            session.player.position.z,
        );
        let requested = Vec3::new(x, y, z);
        let delta = requested.minus(current);
        if delta.x.abs() > 8.0 || delta.y.abs() > 8.0 || delta.z.abs() > 8.0 {
            // Legitimate per-packet moves are well under a block. Anything larger
            // is a teleport attempt: refuse it and re-anchor the client.
            debug!(id = %id, "rejected movement jump of {delta:?}");
            self.correct_position(id, current, rotation, None);
            return;
        }

        let started_on_ground = session.player.on_ground;
        let start_y = session.tick_start_y;
        let creative = session.player.game_mode.is_creative();
        let result = self.world.move_with_collision(Aabb::player(current), delta);
        let applied = current.plus(result.delta);
        let moved_less = (applied.x - requested.x).abs() > 0.001
            || (applied.y - requested.y).abs() > 0.001
            || (applied.z - requested.z).abs() > 0.001;

        // Grounded means "there is solid ground under my feet", not only "the last
        // downward move was stopped". A player who lands *exactly* on a surface
        // never collides (touching faces do not overlap), so relying on the
        // collision result alone would report `on_ground = false` to someone
        // standing still on the floor.
        let feet_x = floor_to_i32(applied.x);
        let feet_y = floor_to_i32(applied.y);
        let feet_z = floor_to_i32(applied.z);
        let grounded = result.on_ground || self.world.is_solid(feet_x, feet_y - 1, feet_z);

        let mut fell_damage = 0.0f32;
        {
            let Some(session) = self.sessions.get_mut(&id) else {
                return;
            };
            session.player.position = mc_entity::player::Vec3::new(applied.x, applied.y, applied.z);
            if let Some((yaw, pitch)) = rotation {
                session.player.yaw = yaw;
                session.player.pitch = pitch;
            }
            session.player.on_ground = grounded || on_ground;
            // Fall damage: measured from where the fall started, ignoring the first
            // three blocks (Vanilla's rule).
            if grounded && !started_on_ground && !creative {
                let fallen = (start_y - applied.y).floor();
                if fallen > FALL_DAMAGE_THRESHOLD {
                    fell_damage = (fallen - FALL_DAMAGE_THRESHOLD) as f32;
                }
            }
        }

        if fell_damage > 0.0 {
            // The session was checked above and nothing in between removes one, so
            // the fallback is unreachable rather than a second failure path.
            let outcome = self.sessions.get_mut(&id).map_or(
                DamageOutcome {
                    applied: false,
                    died: false,
                    dealt: 0.0,
                    health: 0.0,
                },
                |session| session.player.apply_damage(fell_damage),
            );
            debug!(id = %id, damage = fell_damage, health = outcome.health, "fall damage");
            self.after_damage(id, outcome);
        }
        if moved_less {
            self.correct_position(id, applied, rotation, None);
        }
    }

    /// Copy authoritative player state into the entity projection.
    fn project_player_entities(&mut self) {
        let updates: Vec<(EntityId, mc_entity::player::Vec3, bool, f32, f32)> = self
            .sessions
            .values()
            .map(|session| {
                (
                    session.entity,
                    session.player.position,
                    session.player.on_ground,
                    session.player.yaw,
                    session.player.pitch,
                )
            })
            .collect();
        for (entity, position, on_ground, yaw, pitch) in updates {
            let Some(projection) = self.entities.get_mut(entity) else {
                continue;
            };
            // Position is the feet centre for both the `Player` and the entity, so
            // this is a field move and not a correction.
            projection.position = position;
            projection.on_ground = on_ground;
            projection.yaw = yaw;
            projection.pitch = pitch;
            // The projection keeps no velocity of its own: player movement is
            // resolved from the client's reported position, so a velocity here would
            // be a second, disagreeing source of truth. Zero is also what the
            // `player_position` packet tells the client. Knockback and server-side
            // player velocity (P05) will replace this with a real value.
            projection.velocity = mc_entity::player::Vec3::ZERO;
        }
    }

    /// Send the client a teleport back to the server's position.
    ///
    /// `report` is optional so callers that have no tick report (the movement
    /// path's early refusals) do not have to fabricate one; the packet is queued
    /// either way.
    fn correct_position(
        &self,
        id: ConnectionId,
        position: Vec3,
        rotation: Option<(f32, f32)>,
        report: Option<&mut TickReport>,
    ) {
        let (yaw, pitch) = rotation.unwrap_or_else(|| {
            self.sessions
                .get(&id)
                .map_or((0.0, 0.0), |s| (s.player.yaw, s.player.pitch))
        });
        let packet = PlayerPosition {
            x: position.x,
            y: position.y,
            z: position.z,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw,
            pitch,
            flags: 0,
            teleport_id: self.tick.wrapping_add(2) as i32,
        };
        let mut local = TickReport::default();
        let _ = self.send(id, &packet, report.unwrap_or(&mut local));
    }

    // ------------------------------------------------------ block actions

    /// Dig/drop/swap, validated against reach, load state and the registry.
    fn apply_player_action(
        &mut self,
        id: ConnectionId,
        status: i32,
        position: i64,
        _facing: u8,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let (x, y, z) = unpack_block_position(position);
        // Only the block-targeting statuses carry a position that means anything.
        // Vanilla sends `BlockPos.ZERO` for drop, swap and release, so applying the
        // reach check to those refused every Q-drop by any player not standing at the
        // origin (found by `inventory_duplication`). The list is explicit rather than
        // "everything else", so a new status cannot silently inherit the wrong rule.
        if targets_a_block(status) && !self.within_reach(id, x, y, z) {
            debug!(id = %id, x, y, z, status, "rejected action outside reach");
            return Ok(());
        }
        match status {
            ACTION_START_DESTROY_BLOCK | ACTION_FINISH_DESTROY_BLOCK => {
                if !self.in_build_range(y) {
                    debug!(id = %id, y, "rejected dig outside the world height");
                    return Ok(());
                }
                let Some(current) = self.world.get_block_loaded(x, y, z) else {
                    debug!(id = %id, "rejected dig in an unloaded chunk");
                    return Ok(());
                };
                if self.registries.blocks.is_empty(current) {
                    return Ok(()); // already air: nothing to do
                }
                let creative = self
                    .sessions
                    .get(&id)
                    .is_some_and(|s| s.player.game_mode.is_creative());
                if !creative && self.registries.blocks.block_name(current)? == "minecraft:bedrock" {
                    debug!(id = %id, "refused to break bedrock in survival");
                    return Ok(());
                }
                let air = self.registries.blocks.air_id();
                // `set_block` cannot fail here: the y range was checked above, and
                // an out-of-range y is its only error. Refusing to propagate keeps a
                // hostile coordinate from ending the tick (AGENTS.md section 9).
                if let Err(error) = self.world.set_block(x, y, z, air) {
                    debug!(id = %id, x, y, z, %error, "block break was refused");
                    return Ok(());
                }
                debug!(id = %id, x, y, z, "block broken");
                self.sync_menu_from_inventory(id, report);
            }
            ACTION_DROP_ITEM => {
                // The held stack leaves the inventory and becomes a dropped-item
                // entity at roughly eye height. Item pickup, merging and the
                // `add_entity` packet that would show it are still P05-15; the
                // entity itself is real and is ticked from the next tick onwards.
                let (dropped, owner, at) = {
                    let Some(session) = self.sessions.get_mut(&id) else {
                        return Ok(());
                    };
                    let dropped = session.player.inventory.take_held(Hand::Main);
                    let at = Vec3::new(
                        session.player.position.x,
                        session.player.position.y + 1.2,
                        session.player.position.z,
                    );
                    (dropped, session.entity, at)
                };
                if dropped.is_empty() {
                    return Ok(());
                }
                match self.spawn_item_owned(dropped, at, Some(owner)) {
                    Ok(entity) => debug!(id = %id, %entity, "dropped an item entity"),
                    // The stack has already left the inventory at this point, so a
                    // refusal is a real loss and is logged rather than swallowed.
                    Err(error) => warn!(id = %id, %error, "could not spawn a dropped item"),
                }
                // The slot the client is looking at just emptied, so the menu is
                // re-mirrored and the update sent. Without this the menu kept the
                // pre-drop view until the next click.
                self.sync_menu_from_inventory(id, report);
            }
            ACTION_SWAP_ITEM_WITH_OFFHAND => {
                if let Some(session) = self.sessions.get_mut(&id) {
                    let main = session.player.inventory.take_held(Hand::Main);
                    let off = session.player.inventory.take_held(Hand::Off);
                    let _ = session.player.inventory.replace_held(Hand::Main, off);
                    let _ = session.player.inventory.replace_held(Hand::Off, main);
                }
                self.sync_menu_from_inventory(id, report);
            }
            other => debug!(id = %id, status = other, "unhandled player action"),
        }
        Ok(())
    }

    /// Whether a `y` is inside the world's build range.
    ///
    /// This guard is what keeps a client-chosen coordinate away from
    /// [`World::set_block`], whose only error is an out-of-range `y`. Without it a
    /// hostile (or merely desynchronised) packet would return an error out of
    /// [`Game::tick`], and [`crate::lifecycle::Server::run`] treats a tick error as
    /// fatal — one client could stop the server.
    pub(crate) fn in_build_range(&self, y: i32) -> bool {
        let min_y = i32::from(self.world.min_section_y()) * mc_world::SECTION_HEIGHT;
        let max_y = (i32::from(self.world.min_section_y()) + self.world.section_count() as i32)
            * mc_world::SECTION_HEIGHT;
        (min_y..max_y).contains(&y)
    }

    /// Right-click a block: place the held block item if the target is legal.
    fn apply_use_item_on(
        &mut self,
        id: ConnectionId,
        position: i64,
        face: i32,
        hand: i32,
        report: &mut TickReport,
    ) {
        let (x, y, z) = unpack_block_position(position);
        if !self.within_reach(id, x, y, z) {
            debug!(id = %id, "rejected placement outside reach");
            return;
        }
        let hand = Hand::from_id(u8::try_from(hand).unwrap_or(0)).unwrap_or(Hand::Main);
        let Some(session) = self.sessions.get(&id) else {
            return;
        };
        let held = session.player.inventory.held_item(hand);
        let Some(item_id) = held.item_id() else {
            return; // empty hand
        };
        let block = match self.registries.items.block_of(item_id) {
            Ok(Some(block)) => block.to_owned(),
            Ok(None) => return, // not a placeable item
            Err(error) => {
                debug!(id = %id, item_id, %error, "held item is missing from the registry");
                return;
            }
        };
        let (dx, dy, dz) = face_offset(face);
        let (tx, ty, tz) = (x + dx, y + dy, z + dz);
        // A placement target 100 000 blocks up is hostile input, not a bug in this
        // server: refuse it before it reaches the world (AGENTS.md section 9).
        if !self.in_build_range(ty) {
            debug!(id = %id, y = ty, "rejected placement outside the world height");
            return;
        }
        let Some(target) = self.world.get_block_loaded(tx, ty, tz) else {
            debug!(id = %id, "rejected placement into an unloaded chunk");
            return;
        };
        if !self.registries.blocks.is_empty(target) {
            debug!(id = %id, "refused to place inside an occupied block");
            return;
        }
        // A block must not be placed inside any player, including the placer.
        let box_ = Aabb::block(tx, ty, tz);
        if self
            .sessions
            .values()
            .any(|other| other.aabb().intersects(box_))
        {
            debug!(id = %id, "refused to place a block inside a player");
            return;
        }
        // `state_id` with no properties yields the block's first state. A block
        // whose default needs a facing (stairs, logs) is placed with that first
        // state rather than a guessed one; the parity matrix records this.
        let block_id = match self.registries.blocks.state_id(&block, &[]) {
            Ok(id) => id,
            Err(error) => {
                debug!(id = %id, %block, %error, "cannot resolve a block state");
                return;
            }
        };
        if let Err(error) = self.world.set_block(tx, ty, tz, block_id) {
            // Unreachable while `in_build_range` above holds; kept so a future
            // change to the world's own validation cannot turn a client action into
            // a fatal tick error.
            debug!(id = %id, x = tx, y = ty, z = tz, %error, "placement was refused");
            return;
        }
        let survival = self
            .sessions
            .get(&id)
            .is_some_and(|s| s.player.game_mode == GameMode::Survival);
        if survival {
            {
                let Some(session) = self.sessions.get_mut(&id) else {
                    return;
                };
                let mut stack = session.player.inventory.take_held(hand);
                stack.shrink(1);
                let leftover = session.player.inventory.add_stack(stack);
                debug_assert!(leftover.is_empty(), "a shrunk stack must fit back");
            }
            // One path for "an action changed my items": mirror the inventory into the
            // menu, advance the revision and send the per-slot updates. The bespoke
            // single-slot send this replaces used a hard-coded `state_id: 0`, so the
            // client was handed a revision the server did not have (Audit 04 A6).
            self.sync_menu_from_inventory(id, report);
        }
        debug!(id = %id, block = %block, x = tx, y = ty, z = tz, "block placed");
    }

    /// Hostile or malformed hotbar indices are dropped, never applied.
    fn apply_hotbar(
        &mut self,
        id: ConnectionId,
        slot: i16,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let Ok(slot) = u8::try_from(slot) else {
            debug!(id = %id, slot, "rejected negative hotbar index");
            return Ok(());
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return Ok(());
        };
        if session.player.inventory.select(slot).is_err() {
            debug!(id = %id, slot, "rejected out-of-range hotbar index");
            return Ok(());
        }
        let selected = i32::from(session.player.inventory.selected_hotbar());
        let held = session.player.inventory.selected_item();
        self.send(id, &SetHeldSlot { slot: selected }, report)?;
        // Sync the newly held slot so the client's hotbar matches the server's.
        let packet = mc_protocol::packets::play::ContainerSetSlot {
            window_id: 0,
            state_id: 0,
            slot: selected as i16,
            item: wire_stack(held),
        };
        self.send(id, &packet, report)?;
        Ok(())
    }

    fn apply_client_command(
        &mut self,
        id: ConnectionId,
        action: i32,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        if action != CLIENT_COMMAND_RESPAWN {
            return Ok(());
        }
        let dead = self
            .sessions
            .get(&id)
            .is_some_and(|session| !session.player.is_alive());
        if !dead {
            debug!(id = %id, "ignored a respawn request from a living player");
            return Ok(());
        }
        let (sx, sy, sz) = self.world.spawn();
        let (game_mode, dropped) = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return Ok(());
            };
            // `keep_inventory` is a game rule we do not model yet; Vanilla's default
            // is `false`, so items are dropped. The stacks are returned to the
            // caller and discarded here: they are dropped at the *death* position,
            // which this path no longer knows, and item entities for a death drop
            // land with the rest of P05-15. Stated, not implied.
            let dropped = session.player.respawn(false);
            session.player.position = mc_entity::player::Vec3::new(
                f64::from(sx) + 0.5,
                f64::from(sy),
                f64::from(sz) + 0.5,
            );
            session.tick_start_y = f64::from(sy);
            session.sent_chunks.clear();
            (session.player.game_mode.id(), dropped.len())
        };
        if dropped > 0 {
            debug!(id = %id, dropped, "death drops discarded (item entities land in P05)");
        }
        info!(id = %id, "player respawned");
        self.send(
            id,
            &Respawn {
                dimension_type_id: 0,
                dimension_name: mc_network::registry_data::OVERWORLD.to_owned(),
                hashed_seed: 0,
                game_mode,
                previous_game_mode: -1,
                is_debug: false,
                is_flat: false,
                data_kept: 0,
                sea_level: 63,
            },
            report,
        )?;
        self.correct_position(
            id,
            Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5),
            Some((0.0, 0.0)),
            Some(report),
        );
        self.send_vitals(id, report)?;
        self.stream_for(id, report)?;
        Ok(())
    }

    // ------------------------------------------------------------------ tick

    // Every player is ticked the same way and nothing here can fail: the food step
    // and the void guard both produce outcomes rather than errors. Returning a
    // `Result` that is always `Ok` would invite a caller to `?` it and hide that.
    fn tick_players(&mut self) {
        let min_y = i32::from(self.world.min_section_y()) * mc_world::SECTION_HEIGHT;
        let spawn = self.world.spawn();
        let mut messages: Vec<(ConnectionId, String)> = Vec::new();
        for session in self.sessions.values_mut() {
            session.tick_start_y = session.player.position.y;
            // Vanilla heals on a 4-second timer (`foodTickTimer`), not every tick.
            // Calling this every tick made regeneration ~20x too fast and meant
            // exhaustion never accrued, so food never depleted in play (Audit 03).
            // The exhaustion cost of movement/actions is P05-14's table; until it
            // exists this passes 0, which is why a player never gets hungry yet.
            // An off-tick changed nothing, so the outcome is "no damage".
            let outcome = if self.tick.is_multiple_of(FOOD_TICK_INTERVAL) {
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
            // Nothing below the world is standable. Void damage is P05; until then
            // a player who ends up there is returned to spawn instead of falling
            // forever.
            if session.player.position.y < f64::from(min_y - 8) {
                warn!(id = %session.id, "player fell out of the world; returning to spawn");
                session.player.position = mc_entity::player::Vec3::new(
                    f64::from(spawn.0) + 0.5,
                    f64::from(spawn.1),
                    f64::from(spawn.2) + 0.5,
                );
                session.tick_start_y = f64::from(spawn.1);
                session.sent_chunks.clear();
            }
        }
        for (id, message) in messages {
            self.send_message(id, &message);
        }
    }

    /// Push vitals after damage and tell the player if they died.
    ///
    /// Takes no `TickReport`: it runs from the movement path, where the caller
    /// already has one and a second borrow is impossible. The packets are queued
    /// regardless; only the per-tick counters miss them.
    fn after_damage(&mut self, id: ConnectionId, outcome: DamageOutcome) {
        let mut local = TickReport::default();
        if outcome.applied {
            let _ = self.send_vitals(id, &mut local);
        }
        if outcome.died {
            let _ = self.send(
                id,
                &SystemChat {
                    content: TextComponent::literal("You died! Use the respawn button."),
                    overlay: false,
                },
                &mut local,
            );
        }
    }

    /// Whether a block position is within the player's reach.
    fn within_reach(&self, id: ConnectionId, x: i32, y: i32, z: i32) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        // Vanilla measures from the eye (feet + 1.62) to the *closest point* of the
        // target block's box, so a straight-line distance is the right test. Using
        // per-axis comparisons instead would accept a block far away on two axes
        // when it is close on the third.
        let eye = Vec3::new(
            session.player.position.x,
            session.player.position.y + 1.62,
            session.player.position.z,
        );
        let block = Aabb::block(x, y, z);
        // Per-axis distance from the eye to the box (0 when the eye is inside that
        // axis' span).
        let dx = (block.min_x - eye.x).max(0.0).max(eye.x - block.max_x);
        let dy = (block.min_y - eye.y).max(0.0).max(eye.y - block.max_y);
        let dz = (block.min_z - eye.z).max(0.0).max(eye.z - block.max_z);
        (dx * dx + dy * dy + dz * dz).sqrt() <= REACH
    }

    // ------------------------------------------------------------- streaming

    /// Load a chunk into the world, from disk when one is stored.
    ///
    /// The order matters and is the reason this method exists at all:
    ///
    /// 1. already loaded → nothing to do;
    /// 2. stored on disk → convert and load it, then mark it **clean**. A chunk
    ///    that was loaded and not edited must never be written back: an all-air
    ///    placeholder saved over real terrain is the data loss this ordering
    ///    prevents;
    /// 3. nothing stored → create the all-air placeholder (generation is P07) and
    ///    mark it clean for the same reason — "we have no terrain for this" is not
    ///    the same as "this is empty terrain", and only a real edit makes it dirty;
    /// 4. read failed → placeholder, logged, and left clean: a failed read must
    ///    never license overwriting a file.
    ///
    /// Reads happen on the tick thread and stop at the per-tick chunk budget, so the
    /// worst case per tick is [`CHUNKS_PER_TICK`] chunk decodes. Moving them to a
    /// worker is P08-11 and would change the determinism story, not just the
    /// threading, so it is deliberately left on the tick thread here.
    fn load_or_create_chunk(&mut self, pos: ChunkPos) {
        if self.world.is_loaded(pos) {
            return;
        }
        let mut loaded = false;
        match self.read_stored_chunk(pos) {
            Ok(Some(data)) => match Chunk::from_chunk_data(&data, &self.registries.blocks) {
                Ok(chunk) => {
                    self.world.load_chunk(chunk);
                    loaded = true;
                }
                Err(error) => {
                    warn!(?pos, %error, "stored chunk could not be converted; using a placeholder");
                }
            },
            Ok(None) => {}
            Err(error) => {
                warn!(?pos, %error, "chunk read failed; using a placeholder (it will not be saved)");
            }
        }
        if !loaded {
            // Generation requires knowing that **nothing is stored**, and only a game that
            // owns storage can know that: `read_stored_chunk` returns `Ok(None)` both for
            // "no chunk here" and for "I have no handle to look with". Treating the second
            // as the first would generate terrain over a saved world, which is unrecoverable
            // — so a borrowing game keeps the placeholder and its do-not-persist mark
            // instead. Found by the worldgen E2E test; see the fix's commit message.
            //
            // This is the **only** place generation happens, which is what makes "an
            // existing world is never regenerated" a structural property rather than a
            // promise.
            let may_generate = self.can_read_stored_chunks();
            match self
                .generator
                .as_ref()
                .filter(|_| may_generate)
                .map(|generator| {
                    // The trait must be in scope for the method to resolve; naming it here
                    // rather than at the top of the file keeps the import next to its only use.
                    use mc_worldgen::ChunkGenerator as _;
                    generator
                        .generate_chunk(pos, &self.registries.blocks)
                        .map_err(|error| error.to_string())
                }) {
                Some(Ok(mut chunk)) => {
                    // Left **clean**, which is the non-obvious part: this generator is
                    // deterministic, so a generated chunk is reproducible byte for byte
                    // from `(seed, pos)` at any later time. Persisting it buys nothing, and
                    // a dirty chunk cannot be unloaded — so marking it dirty would make
                    // generated chunks accumulate forever. What must be persisted is a
                    // *modification*, and `set_block` marks the chunk dirty for that.
                    //
                    // An earlier version of this path marked it dirty "so the world keeps
                    // it", which broke chunk unloading for every generated chunk and (via a
                    // matching change to the clean-marking below) let placeholders be
                    // written back over real terrain. Two existing tests caught both.
                    //
                    // Structures decorate the terrain, so they run **after** it and read the same
                    // height field the terrain pass used. A refusal is counted rather than fatal: a
                    // missing decoration is strictly better than losing the terrain a player stands
                    // on.
                    self.decorate_with_structures(pos, &mut chunk);
                    // **The oak features.** `generate_chunk` is terrain only — `decorate` is a separate pass,
                    // exactly as vanilla separates them — and **nothing called it**, so every world this
                    // server generated had no trees in it at all. The biome surface blocks were right (grass
                    // over dirt over stone, podzol and coarse dirt for taiga, sand and water for ocean), which
                    // is why the world read as terrain stripped of its features rather than as broken terrain.
                    //
                    // Last, so a tree is not planted through a structure placed a line earlier.
                    if self.generator.is_some() {
                        // Rebuilt from the seed for the same reason `decorate_with_structures` rebuilds its
                        // context: one source of truth for the seed, and no borrow of `self` held across the
                        // mutable use of `chunk`.
                        let context = mc_worldgen::WorldgenContext::overworld(
                            mc_worldgen::WorldSeed::from_raw(self.random_seed),
                        );
                        match mc_worldgen::TerrainGenerator::new(context, &self.registries.blocks) {
                            Ok(generator) => {
                                let stats =
                                    generator.decorate(&mut chunk, pos, &self.registries.blocks);
                                self.tree_stats.record(stats);
                            }
                            Err(error) => {
                                warn!(?pos, %error, "a chunk could not be decorated with trees");
                            }
                        }
                    }
                    self.world.load_chunk(chunk);
                }
                Some(Err(error)) => {
                    warn!(?pos, %error, "chunk generation failed; using a placeholder");
                    self.world.ensure_chunk(pos);
                    if !self.can_read_stored_chunks() {
                        self.placeholder_without_storage.insert(pos);
                    }
                }
                None => {
                    // No generator, or a game that cannot tell "absent" from "unreadable":
                    // a placeholder is only safe to *persist* when this game could have read
                    // the real chunk and found nothing. Without storage it cannot know, so it
                    // keeps the placeholder but refuses to let it be written back by marking
                    // it "not saved yet" rather than clean.
                    self.world.ensure_chunk(pos);
                    if !may_generate {
                        self.placeholder_without_storage.insert(pos);
                    }
                }
            }
        }
        // Every path ends clean, and the three origins reach that state for three
        // different reasons — which is why the line is unconditional:
        //
        //   * a **stored** chunk is unmodified;
        //   * a **generated** chunk is reproducible from the seed;
        //   * a **placeholder** is a stand-in, and `ensure_chunk` marks it dirty by
        //     construction, so without this it would be written back over real terrain.
        //
        // The third is the one an earlier version broke by making this conditional, which
        // `a_stored_chunk_is_loaded_from_disk_and_never_overwritten_by_a_placeholder` caught.
        self.mark_chunk_clean(pos);
    }

    /// Install the structure templates and derive the placement rule.
    ///
    /// The rule is derived **here**, once, rather than per chunk: `StructureSet::from_registry` sorts
    /// the names and caps the selectable slice, so deriving it per chunk would make one chunk's cost
    /// depend on the installed pack size — which is exactly what the cap exists to prevent.
    pub fn set_structures(&mut self, structures: mc_worldgen::structures::StructureRegistry) {
        // The set is derived from the templates the **build policy can place**, not from all of them.
        //
        // `StructureSet::from_registry` takes the first 64 names in sorted order, and alphabetically
        // those are all `ancient_city/*` — large multi-chunk pieces — while
        // `StructureBuild::default()` is `CrossChunk::SingleChunk`, which refuses anything that does not
        // fit. Selection and placement therefore disagreed on **every** chunk: `selected: 2,
        // blocks_written: 0, refused: 2`, so a server generated no structures at all with the pack fully
        // loaded. Found by a test that generates through the server rather than through the library.
        //
        // Deriving from `fitting_in_one_chunk` makes the two consistent by construction. The cost is
        // real and recorded: the selectable population is the single-chunk subset, so large structures
        // never generate under a `SingleChunk` policy.
        self.structure_set = mc_worldgen::structures::StructureSet::from_registry(
            &structures.fitting_in_one_chunk(),
        );
        self.structures = structures;
    }

    /// The placement rule the server uses for structural decoration.
    ///
    /// Exposed so a test can assert that every name the selection can pick is one the build policy can
    /// place — the invariant whose violation made a server generate no structures at all while the pack
    /// was fully loaded.
    #[must_use]
    pub const fn structure_set(&self) -> &mc_worldgen::structures::StructureSet {
        &self.structure_set
    }

    /// What the structure decorator has done so far.
    #[must_use]
    pub const fn structure_stats(&self) -> StructureStats {
        self.structure_stats
    }

    /// What the oak-tree decorator has done so far.
    ///
    /// Counted because "no trees" is invisible: the chunks are well-formed, the light is right, and a world
    /// with no features in it looks exactly like a working one until somebody stands in it.
    #[must_use]
    pub const fn tree_stats(&self) -> mc_worldgen::features::TreeStats {
        self.tree_stats
    }

    /// The structure templates this game has loaded.
    #[must_use]
    pub const fn structures(&self) -> &mc_worldgen::structures::StructureRegistry {
        &self.structures
    }

    /// Place whatever structure this chunk's selection picks, if any.
    ///
    /// A no-op when no templates are loaded, so a server without a data pack pays nothing. The
    /// selection is a pure function of `(seed, chunk_pos)`, so a chunk decorated here and one decorated
    /// after a restart get the same structure — which is what makes regeneration reproducible.
    fn decorate_with_structures(&mut self, pos: ChunkPos, chunk: &mut Chunk) {
        if self.structures.is_empty() {
            return;
        }
        self.structure_stats.considered += 1;
        if self.generator.is_none() {
            // No generator means no terrain, and a structure on a placeholder would be decoration on
            // nothing. Skipped rather than placed.
            return;
        }
        // Rebuilt from the seed rather than stored: `WorldgenContext::overworld` is the only
        // constructor the server uses, and deriving it here keeps one source of truth for the seed.
        let context = mc_worldgen::WorldgenContext::overworld(mc_worldgen::WorldSeed::from_raw(
            self.random_seed,
        ));
        let surface_height = |x: i32, z: i32| {
            self.generator
                .as_ref()
                .map_or(0, |generator| generator.surface_height(x, z))
        };
        let request = mc_worldgen::structures::StructureGenRequest {
            pos,
            context: &context,
            registry: &self.structures,
            set: &self.structure_set,
            build: mc_worldgen::structures::StructureBuild::default(),
            blocks: &self.registries.blocks,
            surface_height: &surface_height,
        };
        let report = mc_worldgen::structures::generate_structures(chunk, &request);
        if report.selected {
            self.structure_stats.selected += 1;
            self.structure_stats.blocks_written += report.blocks_written;
            if report.refused.is_some() {
                self.structure_stats.refused += 1;
            }
        }
        if report.selected && report.blocks_written == 0 {
            // Selected but wrote nothing: either it was refused or every block was air under
            // `IgnoreAir`. Logged because it is the difference between "no structure here" and
            // "a structure that did not appear", which is otherwise invisible.
            debug!(
                ?pos,
                name = ?report.name,
                outside = report.blocks_outside,
                out_of_world = report.blocks_out_of_world,
                refused = ?report.refused,
                "a structure was selected but wrote no blocks"
            );
        }
    }

    /// Load a chunk, generating or reading it as appropriate.
    ///
    /// The same path the chunk streamer uses, exposed because it is genuinely useful
    /// outside it: a test needs to place a chunk deterministically, and a future
    /// `/forceload` needs to load one that no player is near. It is deliberately **not** a
    /// test-only shortcut — a second loading path would be a second place for the
    /// "prefer stored over generated" ordering to be got wrong.
    ///
    /// Returns whether a chunk is loaded afterwards, which is `false` only when generation
    /// failed *and* storage could not supply one.
    pub fn load_chunk(&mut self, pos: ChunkPos) -> bool {
        self.load_or_create_chunk(pos);
        self.world.is_loaded(pos)
    }

    /// Read a chunk from the owned storage, when there is one.
    ///
    /// `Ok(None)` means "nothing is stored here", which is different from an error
    /// and is what decides between a real load and a placeholder.
    ///
    /// # Errors
    ///
    /// Propagates the storage layer's error so the caller can log it. The caller
    /// degrades to a placeholder; it never fails the tick.
    fn read_stored_chunk(&mut self, pos: ChunkPos) -> ServerResult<Option<ChunkData>> {
        let Some(storage) = self.storage.as_mut() else {
            return Ok(None);
        };
        storage.storage_mut().read_chunk(&Dimension::Overworld, pos)
    }

    /// Whether this game can read the world's stored chunks.
    ///
    /// A game built with a *borrowed* `WorldService` (`Game::new`, `Game::with_seed`)
    /// has no storage of its own, so it cannot tell "no chunk stored here" from "I
    /// cannot look". Treating that as "no chunk stored here" is what let a test
    /// placeholder be saved over real terrain, so the difference is now explicit and
    /// the two callers behave differently.
    #[must_use]
    const fn can_read_stored_chunks(&self) -> bool {
        self.storage.is_some()
    }

    fn mark_chunk_clean(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.world.chunk_mut(pos) {
            chunk.mark_clean();
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
    fn stream_for(&mut self, id: ConnectionId, report: &mut TickReport) -> ServerResult<()> {
        let Some(session) = self.sessions.get(&id) else {
            return Ok(());
        };
        let centre = session.chunk();
        let radius = self.view_distance;
        let mut wanted: Vec<ChunkPos> = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let candidate = ChunkPos::new(centre.x + dx, centre.z + dz);
                if !session.sent_chunks.contains(&candidate) {
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
        } else {
            debug!(?pos, "chunk packet dropped; it will be retried next tick");
        }
        Ok(())
    }

    /// Unload chunks no player can see any more.
    ///
    /// Without this, every chunk a player walks past stays resident (a chunk is
    /// ~384 KiB), so a long walk is an unbounded memory leak. The rule is simple and
    /// deliberately conservative:
    ///
    /// - keep anything within `view_distance + UNLOAD_MARGIN_CHUNKS` of any player;
    /// - never unload a **dirty** chunk — it holds edits that are not on disk yet,
    ///   and only [`Game::save_all`] persists those;
    /// - drop the position from every player's `sent_chunks`, so walking back
    ///   re-streams the chunk instead of leaving a hole in the client's view.
    ///
    /// Nothing else holds a chunk index, so unloading cannot dangle: block changes
    /// carry their own coordinates and are re-resolved when broadcast.
    fn unload_distant_chunks(&mut self) {
        if self.sessions.is_empty() {
            // Nobody to keep chunks for. This is the headless/test shape, where
            // unloading would silently discard the world a test just built.
            return;
        }
        let radius = self.view_distance.saturating_add(UNLOAD_MARGIN_CHUNKS);
        // Player chunk centres, collected before the loop so the sessions can be
        // mutated (their `sent_chunks`) inside it.
        let centres: Vec<(i32, i32)> = self
            .sessions
            .values()
            .map(|session| {
                let centre = session.chunk();
                (centre.x, centre.z)
            })
            .collect();
        let loaded: Vec<ChunkPos> = self.world.chunk_positions().collect();
        let mut removed = 0usize;
        for pos in loaded {
            if self.world.chunk(pos).is_some_and(|chunk| chunk.dirty) {
                trace!(?pos, "keeping a dirty chunk outside the view distance");
                continue;
            }
            let wanted = centres
                .iter()
                .any(|(x, z)| (pos.x - x).abs() <= radius && (pos.z - z).abs() <= radius);
            if wanted {
                continue;
            }
            if self.world.unload_chunk(pos).is_some() {
                removed += 1;
            }
            for session in self.sessions.values_mut() {
                session.sent_chunks.remove(&pos);
            }
        }
        if removed > 0 {
            trace!(removed, "unloaded chunks outside the view distance");
        }
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
                WireContainer::new(palette, values, mc_persistence::packing::BLOCK_MIN_BITS);
            // One plains biome fills every cell, and the id is the one the client's own registry gives
            // plains \u2014 see [`PLAINS_BIOME_ID`]. Per-column biomes are not modelled yet, and the values array
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

    // ---------------------------------------------------------------- sending

    /// Encode and queue a packet; reports whether it was queued.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the packet cannot be encoded, which is a
    /// server-side bug rather than client input.
    pub(crate) fn send<T: Packet>(
        &self,
        id: ConnectionId,
        packet: &T,
        report: &mut TickReport,
    ) -> ServerResult<bool> {
        let raw = packet.to_raw()?;
        Ok(self.send_raw(id, raw, report))
    }

    /// Queue a packet, reporting whether it actually reached the connection.
    ///
    /// Returns `false` when the player is gone or the queue is full. Callers that
    /// track delivery (chunk streaming) must honour it: a dropped packet that was
    /// already recorded as sent is a hole the client never recovers from.
    fn send_raw(&self, id: ConnectionId, raw: RawPacket, report: &mut TickReport) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        if session.outbound.try_send(raw).is_err() {
            warn!(id = %id, "outbound queue full; the player will be disconnected");
            // Recorded here and enforced by the Network phase, which owns `&mut self`.
            if !report.overflowed.contains(&id) {
                report.overflowed.push(id);
            }
            report.packets += 1;
            return false;
        }
        report.packets += 1;
        true
    }

    /// Queue a system chat line.
    ///
    /// Takes no report: it is called from paths that already hold one (borrow
    /// conflict) or from tests. The packet is queued either way.
    fn send_message(&self, id: ConnectionId, text: &str) {
        let mut local = TickReport::default();
        // A chat line is best-effort: a full queue is already handled by the
        // overflow path, so the queued flag is deliberately discarded here.
        if self
            .send(
                id,
                &SystemChat {
                    content: TextComponent::literal(text),
                    overlay: false,
                },
                &mut local,
            )
            .is_err()
        {
            debug!(id = %id, "a system chat line could not be encoded");
        }
    }

    fn send_vitals(&self, id: ConnectionId, report: &mut TickReport) -> ServerResult<()> {
        let Some(session) = self.sessions.get(&id) else {
            return Ok(());
        };
        self.send(
            id,
            &SetHealth {
                health: session.player.health,
                food: session.player.food,
                saturation: session.player.saturation,
            },
            report,
        )?;
        self.send(
            id,
            &SetExperience {
                progress: session.player.experience_progress(),
                level: session.player.level,
                total: session.player.total_experience,
            },
            report,
        )?;
        Ok(())
    }

    /// Queue a block-change packet for every player who has the chunk.
    ///
    /// Returns how many players received it, which is what distinguishes "a change
    /// nobody can see" from "a visible change" in [`TickReport::block_changes`].
    fn broadcast_chunk(&self, pos: ChunkPos, raw: &RawPacket, report: &mut TickReport) -> usize {
        let mut sent = 0;
        for session in self.sessions.values() {
            if session.sent_chunks.contains(&pos) {
                self.send_raw(session.id, raw.clone(), report);
                sent += 1;
            }
        }
        sent
    }

    fn broadcast_all(&self, raw: &RawPacket, report: &mut TickReport) {
        for session in self.sessions.values() {
            if session.ready {
                self.send_raw(session.id, raw.clone(), report);
            }
        }
    }

    /// Queue one packet for a player (tests and admin actions).
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when the id is not a player,
    /// [`ServerError::Operational`] when the queue is full.
    pub fn send_to(&self, id: ConnectionId, packet: &impl Packet) -> ServerResult<()> {
        let raw = packet.to_raw()?;
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| ServerError::InvalidAction(format!("{id} is not a player")))?;
        session
            .outbound
            .try_send(raw)
            .map_err(|_| ServerError::Operational(format!("{id} outbound queue is full")))
    }

    /// Drop players whose outbound queue overflowed.
    fn enforce_overflow(&mut self, ids: Vec<ConnectionId>) -> usize {
        let mut count = 0;
        for id in ids {
            if let Some(session) = self.sessions.remove(&id) {
                let packet = PlayDisconnect {
                    reason: TextComponent::literal("Outbound queue overflow"),
                }
                .to_raw();
                if let Ok(raw) = packet {
                    let _ = session.outbound.try_send(raw);
                }
                // Same treatment as a clean leave: the entity goes on the sweep.
                if let Some(entity) = self.entities.get_mut(session.entity) {
                    entity.removed = true;
                }
                self.entity_ids.remove(&id);
                warn!(id = %id, "disconnected after an outbound queue overflow");
                count += 1;
            }
        }
        count
    }

    /// Record that a player must be dropped next tick.
    pub fn request_disconnect(&mut self, id: ConnectionId) {
        self.overflowed.push(id);
    }

    /// Persist the world through a caller-supplied handle (shutdown path).
    ///
    /// Persists: **dirty** chunks and `level.dat`. Does **not** persist per-player
    /// data —`playerdata/<uuid>.dat` needs the player-file layout, which Phase 04
    /// does not implement — nor entities: mob and item persistence arrives with the
    /// entity chunk sections in P05-16, so a dropped item does not survive a
    /// restart. Both gaps are listed in the phase reports rather than implied away.
    ///
    /// Only chunks the world reports as dirty are written. A chunk that was loaded
    /// from disk and then edited is dirty; a chunk that was only *streamed* to a
    /// player is not, which is what stops an all-air placeholder from overwriting
    /// real terrain.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a chunk cannot be encoded or the flush
    /// fails.
    pub fn save_all(&mut self, storage: &mut WorldService) -> ServerResult<()> {
        let skipped = self.queue_dirty_chunks(storage)?;
        if skipped > 0 {
            // Reported rather than silent: a save that deliberately left chunks out is
            // different from one that saved everything, and an operator should be able to
            // tell which happened.
            warn!(
                skipped,
                "placeholder chunks were not saved: this game cannot read storage, so it \
                 cannot tell an empty chunk from an unreadable one"
            );
        }
        let report = storage.storage_mut().flush()?;
        if report.is_clean() {
            self.world.clear_dirty();
        } else {
            // Keep the dirty flags. Clearing them here would forget a failed write
            // entirely: the chunk would never be retried and the loss would show up
            // only as missing terrain after a restart (Audit 03).
            warn!(
                failed = report.chunks_failed,
                errors = ?report.errors,
                "world flush reported failures; dirty chunks keep their flags for retry"
            );
        }
        Ok(())
    }

    /// [`Game::save_all`] against the handle this game owns.
    ///
    /// **Silently does nothing when this game does not own storage**, and returns `Ok(())`
    /// when it does so. That is deliberate — a game built with `Game::new` or
    /// `Game::with_seed` holds a *borrow* and legitimately has no handle to flush, and
    /// [`crate::lifecycle::Server::run`] calls this on every autosave tick regardless —
    /// but it is a sharp edge worth naming: a caller who pairs it with a borrowing
    /// constructor gets a save that reports success and writes nothing, and the loss only
    /// appears as missing terrain after a restart. Use [`Game::save_all`] with the service
    /// you borrowed from instead.
    ///
    /// # Errors
    ///
    /// As for [`Game::save_all`].
    pub fn save_all_owned(&mut self) -> ServerResult<()> {
        let Some(mut service) = self.storage.take() else {
            return Ok(());
        };
        let result = self.save_all(&mut service);
        self.storage = Some(service);
        result
    }

    /// Encode and queue every dirty chunk, **except** the do-not-persist placeholders.
    ///
    /// The skip is the whole point of [`Game::placeholder_without_storage`], and until this
    /// was written nothing consulted that set: the field was populated on every placeholder
    /// path and read by nobody, so a placeholder was still written over real terrain. A
    /// generated world made it visible — the borrowing-game test placed a marker and watched
    /// it vanish — but the defect predates generation, and the doc comment on the field was
    /// more confident than the code.
    fn queue_dirty_chunks(&self, storage: &mut WorldService) -> ServerResult<usize> {
        let mut skipped = 0usize;
        for pos in self.world.dirty_chunks() {
            if self.placeholder_without_storage.contains(&pos) {
                // Belt to the clean-flag's braces. A placeholder is cleaned when it is
                // created, so a placeholder should never *be* dirty — but the consequence of
                // being wrong here is destroying a saved world, so the check stays and is
                // reported. (Before this check existed, the field was populated on every
                // placeholder path and read by nobody, which is how a placeholder once
                // overwrote real terrain.)
                skipped += 1;
                continue;
            }
            if let Some(chunk) = self.world.chunk(pos) {
                let data = chunk.to_chunk_data(&self.registries.blocks)?;
                storage
                    .storage_mut()
                    .queue_chunk_save(&Dimension::Overworld, &data)?;
            }
        }
        Ok(skipped)
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

/// Refuse a join that cannot be completed, without failing the tick.
///
/// The connection gets a `play_disconnect` and never enters the session map, so a
/// client cannot turn "the server is out of entity ids" into a stopped server
/// (AGENTS.md sections 9 and 10). There is no `Session` yet, which is why the
/// packet bypasses [`Game::send`] and goes straight to the connection's own queue.
fn refuse_join(outbound: &OutboundSender, reason: &str) {
    let packet = PlayDisconnect {
        reason: TextComponent::literal(reason),
    };
    if let Ok(raw) = packet.to_raw() {
        let _ = outbound.try_send(raw);
    }
}

/// Floor a coordinate to the block index containing it.
///
/// Rust defines a float-to-int `as` cast as saturating (and NaN to zero), so a
/// hostile coordinate — `f64::INFINITY`, `f64::NAN`, `1e300` — becomes a world
/// coordinate that is merely out of range rather than undefined behaviour. Every
/// caller then treats it as an ordinary out-of-range block, which fails closed:
/// collision reads it as air, and [`Game::in_build_range`] refuses to write there.
fn floor_to_i32(value: f64) -> i32 {
    value.floor() as i32
}

/// The entity crate's vector, from the world crate's.
///
/// `mc-entity` and `mc-world` each own a `Vec3` and neither may depend on the
/// other (the dependency runs world ← entity), so the conversion belongs here —
/// the one place that needs both. The two are structurally identical, so this is
/// a field move.
fn to_entity(vector: Vec3) -> mc_entity::player::Vec3 {
    mc_entity::player::Vec3::new(vector.x, vector.y, vector.z)
}

/// The world crate's vector, from the entity crate's.
///
/// See [`to_entity`] for why the conversion lives at this boundary.
fn to_world(vector: mc_entity::player::Vec3) -> Vec3 {
    Vec3::new(vector.x, vector.y, vector.z)
}

/// The chunk a world position falls in.
fn chunk_of(x: f64, z: f64) -> ChunkPos {
    ChunkPos::new(floor_to_i32(x) >> 4, floor_to_i32(z) >> 4)
}

/// Wrap an item stack for the wire.
///
/// `ItemStack::simple` covers the Phase 04 subset: an id and a count, no data
/// components. Component payloads are unmodelled, so anything with components would
/// need a real codec —see `mc_protocol::packets::play::ItemStack`.
/// Copy a player's inventory into a menu's player container.
///
/// Called at join and again before every click, so the menu always starts a
/// transaction from the authoritative state (Audit 04 A1).
///
/// Slot order is identical in both models (`PlayerInventory`'s storage order: hotbar
/// `0..=8`, main `9..=35`, armour `36..=39` boots-first, offhand `40`), so the copy
/// is positional and needs no permutation. That identity is asserted by
/// `the_menu_layout_matches_the_inventory_storage_order`.
///
/// A mismatch is a programming error rather than a runtime condition, so the
/// function clamps to the smaller size instead of panicking: losing the tail of an
/// inventory is recoverable, a panic in the join path is not.
fn mirror_inventory(
    menu: &mut mc_container::Menu,
    inventory: &mc_entity::inventory::PlayerInventory,
) {
    let Some(container) = menu.container_mut(0) else {
        return;
    };
    let slots = container.len().min(inventory.stored_slots());
    for index in 0..slots {
        // `set` only fails past the end, which `min` above prevents.
        let _ = container.set(index, inventory.slot(index));
    }
}

/// Copy a menu's player container back into a player's inventory.
///
/// The return direction of the pair; see [`mirror_inventory`] for why the inventory
/// is the authoritative side.
fn write_back_inventory(
    menu: &mc_container::Menu,
    inventory: &mut mc_entity::inventory::PlayerInventory,
) {
    let Some(container) = menu.container(0) else {
        return;
    };
    let slots = container.len().min(inventory.stored_slots());
    for index in 0..slots {
        let _ = inventory.set_slot(index, container.get(index));
    }
}

fn wire_stack(stack: mc_entity::stack::ItemStack) -> mc_protocol::packets::play::ItemStack {
    match stack.item_id() {
        Some(id) if !stack.is_empty() => {
            mc_protocol::packets::play::ItemStack::simple(id, stack.count())
        }
        _ => mc_protocol::packets::play::ItemStack::empty(),
    }
}

/// Offset applied to a targeted block for the face the client clicked.
///
/// Face ids are Vanilla's: 0 = -Y, 1 = +Y, 2 = -Z, 3 = +Z, 4 = -X, 5 = +X.
#[must_use]
pub fn face_offset(face: i32) -> (i32, i32, i32) {
    match face {
        0 => (0, -1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        // 1 is +Y; anything unknown defaults to the top face rather than refusing
        // the action, matching how the client sends 0..5 only.
        _ => (0, 1, 0),
    }
}
