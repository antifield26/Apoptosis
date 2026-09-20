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
//! | [`TickPhase::ScheduledTicks`] | drain due block scheduled ticks from the redstone update queue (P13-01; fluids out of scope, no producers yet) | implemented (queue wired) |
//! | [`TickPhase::Entities`] | the natural spawn cycle, then per-entity AI, despawn, timers, gravity + swept collision, landing/fall damage | implemented (AI live as of P11-02) |
//! | [`TickPhase::Players`] | apply the queued intents in arrival order, then player timers and physics | implemented |
//! | [`TickPhase::BlockEntities`] | furnaces cook (`container_set_data`), hoppers transfer on the 8-tick cooldown, viewers resync | implemented (P12-03/04) |
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
//! chunk-border crossings) from memory *and* from disk, world time, natural mob
//! spawning with the measured vanilla rules (`crate::spawn`), mob AI driving
//! movement and melee, mob despawn, entity timers/gravity, chunk unloading
//! outside the view distance, chat relay, commands, and the
//! `add_entity`/`remove_entities`/`set_entity_data` packets that make entities
//! visible to a client. World *generation* lives in `mc-worldgen` and is wired.
//!
//! **Not** simulated, and therefore not claimed: fluid scheduled ticks,
//! redstone (model only — block scheduled ticks drain since P13-01, with no
//! producers or mechanism reactions yet), per-player data persistence, and the
//! projectile and explosion attack styles. Each is recorded in
//! `docs/vanilla-parity/PARITY-MATRIX.md`, and every no-op in this file says so
//! at its definition.

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
use mc_entity::entity::{EntityBody, EntityId, EntityStore};
use mc_entity::item_entity::ItemEntity;
use mc_entity::mob::Mob;

/// Adapter: the AI reads randomness through `mc_entity`'s one-method [`Rng`]
/// trait, and `mc-entity` cannot depend on `mc-simulation` to implement it for
/// [`RandomSource`] itself. This forwards to the game's own source, so the AI's
/// draws join the same deterministic stream as everything else.
struct AiRng<'a>(&'a mut RandomSource);

impl mc_entity::mob::Rng for AiRng<'_> {
    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        self.0.next_i32_bounded(bound)
    }
}

/// Adapter for `mc-data`'s loot [`Rng`](mc_data::loot::Rng) onto the game's own
/// source: `nextInt()`'s full 32 bits, which is exactly the `next(32)` the
/// loot conditions' float defaults reshape.
struct LootRng<'a>(&'a mut RandomSource);

impl mc_data::loot::Rng for LootRng<'_> {
    fn next_u32(&mut self) -> u32 {
        u32::from_ne_bytes(self.0.next_i32().to_ne_bytes())
    }
}
use mc_entity::player::Player;
use mc_entity::stack::ItemStack;
use mc_network::bridge::{ConnectionId, GameEvents, OutboundSender};
use mc_persistence::chunk::ChunkPos;
use mc_persistence::dimension::Dimension;
use mc_persistence::level::Difficulty;
use mc_protocol::RawPacket;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{PlayDisconnect, PlayIntent, SetExperience, SetHealth};
use mc_protocol::text::TextComponent;
use mc_registry::Registries;
use mc_simulation::{RandomSource, Scheduler, TickMetrics};
use mc_world::light::LightArray;
use mc_world::{Vec3, World};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use tracing::{debug, info, warn};

mod persist;
mod session;
mod tick;

use self::session::Session;

/// A chunk's light as the wire wants it: four masks and two array lists.
///
/// Both `level_chunk_with_light` and `light_update` carry exactly this, so it is built once here rather than
/// twice. A drift between two copies would be invisible — both packets stay well-formed, and the client renders
/// whichever arrived last.
#[derive(Debug, Clone, Default)]
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
/// Light section `i` is world section `i - 1`, so there is one below the world and one above it. Sections are
/// classified three ways, and **which three matters**: an all-dark section goes in the matching `empty_*` mask
/// (the client reads that as "no light here"), a partially lit section carries its 2 048-byte array, and a
/// **fully lit sky section is left unmentioned** — the client defaults an unmentioned sky section to fully
/// lit, while the empty mask would tell it the section is dark. That distinction is the dead-black surface
/// finding from the acceptance session; marking fully lit sections empty painted patches of the surface dark.
/// (Block light has no such default, so all-dark block sections are marked empty honestly.)
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
        push_light_section(&mut fields, index, &sky, &block);
    }
    Ok(fields)
}

/// Classify one light section into the masks and arrays the wire carries.
///
/// The three ways the client understands a light section: an **all-dark**
/// section goes in the matching empty mask, which tells the client the section
/// holds no light; a **partially lit** section carries its full 2 048-byte
/// array; a **fully lit sky** section is left *unmentioned* -- the client's
/// default for a sky section it has no data for is fully lit, and marking one
/// "empty" instead tells it the section is dark, which painted the dead-black
/// surface patches the acceptance session found (the air cells a surface face
/// samples live in the section above). Vanilla's own chunk packets leave
/// fully-lit sections unmentioned too: its captured chunk (0, 0) carries sky
/// data only for the two terrain sections, marks the bedrock section empty,
/// and says nothing about the twenty-one fully-lit air sections above it.
fn push_light_section(fields: &mut LightFields, index: u32, sky: &LightArray, block: &LightArray) {
    match sky.uniform() {
        Some(0) => fields.empty_sky_mask.push(index),
        Some(mc_world::light::MAX_LIGHT) => {}
        _ => {
            fields.sky_mask.push(index);
            fields.sky.push(sky.as_bytes().to_vec());
        }
    }
    if block.uniform() == Some(0) {
        fields.empty_block_mask.push(index);
    } else {
        fields.block_mask.push(index);
        fields.block.push(block.as_bytes().to_vec());
    }
}

#[cfg(test)]
mod light_fields_tests {
    use super::push_light_section;
    use super::{LightArray, LightFields};
    use mc_world::light::MAX_LIGHT;

    fn fields() -> LightFields {
        LightFields {
            sky_mask: Vec::new(),
            block_mask: Vec::new(),
            empty_sky_mask: Vec::new(),
            empty_block_mask: Vec::new(),
            sky: Vec::new(),
            block: Vec::new(),
        }
    }

    // The acceptance finding, pinned at the encoding layer: a fully-lit sky
    // section must be **unmentioned** (the client's default for a sky section
    // it has no data for is fully lit), not marked "empty" -- the empty mask
    // tells the client the section is dark, and a surface whose face samples
    // such a section rendered dead black in the live acceptance session.
    #[test]
    fn a_fully_lit_sky_section_is_left_unmentioned() {
        let mut fields = fields();
        push_light_section(
            &mut fields,
            7,
            &LightArray::filled(MAX_LIGHT),
            &LightArray::filled(0),
        );
        assert!(
            fields.empty_sky_mask.is_empty(),
            "a fully lit section must not be marked empty: {fields:?}"
        );
        assert!(
            fields.sky_mask.is_empty(),
            "a uniform section needs no data array: {fields:?}"
        );
        assert!(fields.sky.is_empty());
        // the block side of the same section is all-dark, which is empty
        assert!(fields.empty_block_mask.contains(&7), "{fields:?}");
    }

    #[test]
    fn an_all_dark_sky_section_is_marked_empty() {
        let mut fields = fields();
        push_light_section(
            &mut fields,
            7,
            &LightArray::filled(0),
            &LightArray::filled(0),
        );
        assert!(fields.empty_sky_mask.contains(&7), "{fields:?}");
    }

    #[test]
    fn a_partially_lit_sky_section_carries_its_array() {
        let mut fields = fields();
        let mut sky = LightArray::filled(MAX_LIGHT);
        sky.set(0, 0, 0, 3);
        push_light_section(&mut fields, 7, &sky, &LightArray::filled(0));
        assert!(fields.sky_mask.contains(&7), "{fields:?}");
        assert_eq!(fields.sky.len(), 1, "one data array follows the mask");
        assert!(fields.empty_sky_mask.is_empty(), "{fields:?}");
    }
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

/// The player's eye height above the feet, in blocks.
///
/// Every reach check vanilla makes is measured from `getEyePosition()`, which for a
/// standing player is the feet plus this. 1.62 is the standing eye height in the
/// jar's own entity dimensions (the same figure `mc_world::collision`'s module doc
/// records for the player hitbox, 0.6 x 1.8 x 0.6). It is **not** modelled as
/// varying with pose: a sneaking or swimming player's eye is lower in vanilla, and
/// this build has no pose.
pub const EYE_HEIGHT: f64 = 1.62;

/// Base block-interaction range, from the jar's own attribute default.
///
/// `net.minecraft.world.entity.player.Player.DEFAULT_BLOCK_INTERACTION_RANGE = 4.5f`
/// (`javap -constants` on the Mojang-mapped 26.1.2 server jar). The live value is
/// the `minecraft:block_interaction_range` **attribute**, which this build does not
/// model beyond the creative modifier below; `Player.blockInteractionRange()`
/// returns `getAttributeValue(Attributes.BLOCK_INTERACTION_RANGE)`.
pub const BLOCK_INTERACTION_RANGE: f64 = 4.5;

/// Block-interaction range for a creative player: the base plus the jar's modifier.
///
/// `ServerPlayer`'s static initialiser builds
/// `AttributeModifier(Identifier.withDefaultNamespace("creative_mode_block_range"),
/// 0.5d, Operation.ADD_VALUE)` and `ServerPlayer.setGameMode` adds it while
/// `isCreative()` — so 4.5 + 0.5 = 5.0, bytecode-read rather than assumed.
pub const CREATIVE_BLOCK_INTERACTION_RANGE: f64 = 5.0;

/// The tolerance vanilla adds to every **block** reach check.
///
/// `ServerPlayer.BLOCK_INTERACTION_DISTANCE_VERIFICATION_BUFFER = 1.0d`, passed by
/// both callers that matter here: `ServerPlayerGameMode.handleBlockBreakAction`
/// (`dconst_1`) and `ServerGamePacketListenerImpl.handleUseItemOn` (`dconst_1`).
/// The effective survival reach is therefore **5.5** blocks from the eye, not the
/// attribute's 4.5 — the server is deliberately more permissive than the client's
/// own raycast, because the two compute the player's position at slightly
/// different moments.
pub const BLOCK_INTERACTION_DISTANCE_VERIFICATION_BUFFER: f64 = 1.0;

/// Base entity-interaction range: `Entity`'s `DEFAULT_ENTITY_INTERACTION_RANGE`
/// (3.0), read from the jar's own `Avatar`/`Player` attribute defaults.
pub const ENTITY_INTERACTION_RANGE: f64 = 3.0;

/// The tolerance vanilla adds to every **entity** reach check.
///
/// `ServerPlayer.ENTITY_INTERACTION_DISTANCE_VERIFICATION_BUFFER = 3.0d`, passed by
/// `ServerGamePacketListenerImpl.handleInteract` (`ldc2_w 3.0d` →
/// `isWithinEntityInteractionRange(aabb, 3.0)`, taken **before** the packet's
/// action is branched on, so it covers a swing and a use alike). Effective reach
/// 6.0 from the eye.
pub const ENTITY_INTERACTION_DISTANCE_VERIFICATION_BUFFER: f64 = 3.0;

/// The effective block reach for a game mode, in blocks from the eye.
///
/// [`BLOCK_INTERACTION_RANGE`] (or [`CREATIVE_BLOCK_INTERACTION_RANGE`]) plus
/// [`BLOCK_INTERACTION_DISTANCE_VERIFICATION_BUFFER`].
///
/// Before AUDIT-11 the server used the bare 4.5 for everyone, so a survival dig
/// between 4.5 and 5.5 blocks from the eye was refused even though the jar accepts
/// it.
#[must_use]
pub const fn block_reach(creative: bool) -> f64 {
    let base = if creative {
        CREATIVE_BLOCK_INTERACTION_RANGE
    } else {
        BLOCK_INTERACTION_RANGE
    };
    base + BLOCK_INTERACTION_DISTANCE_VERIFICATION_BUFFER
}

/// The effective entity reach, in blocks from the eye.
#[must_use]
pub const fn entity_reach() -> f64 {
    ENTITY_INTERACTION_RANGE + ENTITY_INTERACTION_DISTANCE_VERIFICATION_BUFFER
}

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
/// The bare-hand attack damage (vanilla 1.0); a held sword's bonus is not
/// modelled, because the item table carries no damage column (named gap).
const FIST_ATTACK_DAMAGE: f32 = 1.0;
/// One ground item's snapshot for the merge/pickup passes (P11-05, P11-09).
#[derive(Clone, Copy)]
struct GroundItem {
    id: EntityId,
    item: Option<i32>,
    position: mc_entity::player::Vec3,
    age: u64,
    ready: bool,
}

/// Ground stacks of the same item within half a block merge (P11-09), squared.
const ITEM_MERGE_RADIUS_SQR: f64 = 0.25;
/// A ground stack is collected when a player is within one block (P11-05), squared.
const ITEM_PICKUP_RADIUS_SQR: f64 = 1.0;
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

/// "No block-change sequence is waiting to be acknowledged."
///
/// Vanilla's `ServerGamePacketListenerImpl` initialises `ackBlockChangesUpTo` to
/// `-1` and uses `> -1` as the "there is something to send" test, which is also
/// what keeps `0` — a perfectly legal sequence — from being read as "nothing".
const NO_BLOCK_CHANGE_SEQUENCE: i32 = -1;

/// How far ahead of a mob's centre the step check looks, in blocks (M-4).
///
/// One block puts the probe in the next cell along the heading whenever the mob is
/// more than half a block from the far edge, and — because a mob's centre is what
/// is tested — it never looks past the cell the mob's own body would enter. Longer
/// lookaheads start refusing steps a mob could still legally take (a diagonal
/// approach clips a corner the body does not touch), which turns "do not walk into
/// walls" into "do not walk near them".
const MOB_LOOKAHEAD_BLOCKS: f64 = 1.0;

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
/// The client resolves a chunk's biome ids against the registry this server hands it — a verbatim replay of
/// vanilla's — and in that registry id 0 is `minecraft:badlands`. This constant was `0` while being named for
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
/// Public so a test can hold it against the registry it claims to index — see
/// `crates/server/tests/registry_ids.rs`.
pub const PLAINS_BIOME_ID: u32 = 40;

/// The `chat_type` **wire** value a plain message uses: **1**.
///
/// The wire encoding of a registry-friendly chat type is **1-based** -- the
/// payload index plus one, with 0 meaning "absent". The evidence, from two
/// independent instruments that agree: (a) a real 26.1.2 server's console
/// `say` went out as **5** while `minecraft:say_command` sits at payload index
/// **4** of the `chat_type` registry both servers send, and (b) our first sends
/// used **0**, and every joined real client failed with `DecoderException` on
/// the welcome message (two independent sessions) until the offset landed.
/// The earlier retraction of this fix was based on misdating that first
/// session's evidence: its `[CHAT] Welcome` line predates the `DisguisedChat`
/// conversion, so it says nothing about the `disguised_chat` id.
///
/// **Resolved from the payload this server sends**, because `minecraft:chat_type` is one of the registries in it
/// -- the registry order is "ending at `minecraft:chat_type`". So a jar extraction would be the
/// wrong instrument here: **the rule is that a number sent to a client is a claim about a registry the client
/// owns, and which instrument settles it depends on who owns the registry**, not on the kind of number it is.
///
/// `crates/server/tests/registry_ids.rs` resolves it the way it resolves [`PLAINS_BIOME_ID`], which is the check
/// that keeps this from being another unverified constant.
pub const CHAT_TYPE_CHAT: i32 = 1;

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
    /// Entities announced to at least one player this tick.
    ///
    /// Counted rather than assumed: a spawn broadcast to nobody -- an item dropped where no player is
    /// watching -- is a different event from one nobody sent, and a tick report that cannot tell them
    /// apart is the kind of silence this phase has been removing.
    pub entities_spawned: usize,
    /// Ids swept this tick, ascending.
    ///
    /// Carried so tests can name what disappeared; the Broadcast phase turns
    /// the batch into the `remove_entities` packet clients see.
    pub removed_ids: Vec<EntityId>,
    /// `block_changed_ack` packets queued this tick.
    ///
    /// Counted because the packet is invisible in every other instrument: nothing
    /// in this server's logs or state changes when it is missing, and the only
    /// symptom is on a real client's screen (M-2).
    pub block_change_acks: usize,
    /// Neighbour updates processed by the redstone propagation this tick (P13-03).
    pub redstone_updates: usize,
    /// Blocks whose state the redstone propagation changed this tick (P13-03).
    ///
    /// Each one rides the normal block-change broadcast, so a powered wire or a
    /// lit lamp reaches clients as a `block_update` like any other edit.
    pub redstone_changed: usize,
    /// Block scheduled ticks that came due this tick (P13-01).
    ///
    /// Counted because a tick that stops draining is silent: the queue keeps the
    /// work, so nothing errors, and circuits just freeze.
    pub scheduled_ticks_fired: usize,
    /// Scheduled ticks still queued for later ticks after this tick's drain.
    pub scheduled_ticks_pending: usize,
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
    /// Last live player state by profile uuid, for rejoin-within-a-run.
    ///
    /// `leave` stores the whole [`Player`] (position, health, inventory, mode);
    /// a rejoin restores it when it was alive. This survives disconnects, not
    /// restarts: `playerdata/<uuid>.dat` files are the P16 item, and a fresh
    /// process starts every player at spawn (declared, and the restart test
    /// pins it).
    remembered: BTreeMap<String, Player>,
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
    /// The per-biome natural-spawn tables (`crate::spawn`), loaded from the committed
    /// fixture extracted from vanilla's data pack.
    spawn_tables: crate::spawn::SpawnTables,
    /// The loaded loot tables (P11-04), keyed by resource id; the drop
    /// authority for block breaks. Empty until `load_packs` runs.
    loot: mc_data::loot::LootTables,
    /// The crafting table (P12-07): baseline until `load_packs` replaces it
    /// with the pack conversion. The player menu's result slot recomputes
    /// from this after every grid change.
    crafting_registry: mc_container::RecipeRegistry,
    /// The furnace table (P12-08): baseline until `load_packs` replaces it
    /// with the pack conversion. `tick_block_entities` cooks from this.
    /// Fuel values stay the jar-verified baseline (`FUEL_BURST_TICKS`).
    smelting_furnace: mc_container::SmeltingRegistry,
    /// Rotating cursor into the entity list for the movement-broadcast budget
    /// (P11-03): which entity starts this tick's slice, so a capped tick never
    /// starves the same tail every time.
    entity_move_cursor: u64,
    /// Chunks that are **stored but unreadable** this session (AUDIT-09 B-01):
    /// generation over them is refused, because a clean generated chunk plus
    /// one edit would replace real terrain at the next autosave.
    unreadable_chunks: BTreeSet<ChunkPos>,
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
    /// Items dropped this tick, queued for the Broadcast phase to announce.
    ///
    /// Queued rather than sent where they spawn because `spawn_item` has no `TickReport` and
    /// `broadcast_chunk` needs one -- the same deferral `pending_light` performs. Until this existed a
    /// dropped item was real on the server and invisible to every client.
    pending_entity_spawns: Vec<EntityId>,
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
    /// Where `ops.json` lives, when the lifecycle told us (P14-02).
    ///
    /// `None` until then — and always in tests that never set it — in which
    /// case `/op` reports that it cannot persist rather than failing. The
    /// directory is the world's parent (Vanilla puts the file beside
    /// `server.properties`), resolved once by [`crate::ops::ops_directory`].
    ops_directory: Option<PathBuf>,
    /// World difficulty (P14-01).
    ///
    /// Read from `level.dat` when storage is present, `Normal` otherwise
    /// (Vanilla's default for existing worlds). `/difficulty` reports and
    /// sets this; setting also writes `level.dat` back when storage is
    /// present. The one runtime consumer is the monster-spawn gate
    /// (peaceful spawns no hostiles); damage numbers stay the hardcoded
    /// Normal values, which the parity matrix records.
    difficulty: Difficulty,
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
    /// Block scheduled ticks keyed by due tick, then position (P13-01).
    ///
    /// Owned here, drained every tick by the `ScheduledTicks` phase. `mc-redstone`
    /// owns the queue mechanics (caps, dedup, budget, order); the game owns when
    /// it drains. P13-02 attaches the world feed (what schedules) and P13-03 the
    /// mechanism reactions: due ticks prepare neighbours, `propagate` runs, and
    /// live wires reschedule (see `tick_scheduled`).
    scheduled_ticks: mc_redstone::UpdateQueue,
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
    ///
    /// # Panics
    ///
    /// Never in practice: the hand-written loot baseline parses, and its unit
    /// tests fail the build if it ever stops doing so.
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

        // Baselines until `load_packs` replaces them (P12-07/08). Built here
        // because the struct literal below moves `registries`.
        let crafting_registry = mc_container::RecipeRegistry::baseline(&registries.items)?;
        let smelting_furnace = mc_container::SmeltingRegistry::baseline(&registries.items)?;
        // Difficulty comes from `level.dat` when storage is present (P14-01);
        // computed here because the literal below moves `owned`.
        let difficulty = borrowed
            .and_then(|storage| storage.storage().level())
            .or_else(|| owned.as_ref().and_then(|storage| storage.storage().level()))
            .map_or(Difficulty::Normal, |level| level.difficulty);

        Ok(Self {
            registries,
            storage: owned,
            world,
            sessions: BTreeMap::new(),
            remembered: BTreeMap::new(),
            entities: EntityStore::new(),
            entity_ids: BTreeMap::new(),
            events,
            view_distance: view_distance.clamp(2, 16),
            random_seed: seed,
            random: RandomSource::new(seed),
            spawn_tables: crate::spawn::SpawnTables::vanilla(),
            loot: mc_data::loot::LootTables::baseline().expect(
                "the hand-written loot baseline parses; a failure here is a programmer error",
            ),
            // Baselines until `load_packs` replaces them with pack conversions
            // (P12-07/08). Failing construction here would refuse to boot over
            // a registry problem, which is startup-time corruption.
            crafting_registry,
            smelting_furnace,
            entity_move_cursor: 0,
            unreadable_chunks: BTreeSet::new(),
            scheduler: Scheduler::new(),
            pending_intents: Vec::new(),
            pending_light: BTreeSet::new(),
            pending_entity_spawns: Vec::new(),
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
            ops_directory: None,
            difficulty,
            max_players: DEFAULT_MAX_PLAYERS,
            time_offset: 0,
            shutdown_requested: false,
            block_entities: mc_container::BlockEntityStore::new(),
            scheduled_ticks: mc_redstone::UpdateQueue::new(),
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
    /// Every mob in the world as `(kind, position)`, ascending by entity id.
    ///
    /// Read-only view for tests, metrics and the admin tooling; mutation goes
    /// through the tick pipeline like every other entity change.
    pub fn mobs(&self) -> Vec<(mc_entity::mob::MobKind, mc_entity::Vec3)> {
        self.entities
            .iter()
            .filter_map(|entity| {
                let mc_entity::EntityBody::Mob(mob) = &entity.body else {
                    return None;
                };
                Some((mob.kind, entity.position))
            })
            .collect()
    }

    /// Every dropped-item entity as `(stack, position)`, ascending by entity id.
    ///
    /// The same kind of read-only view as [`Self::mobs`], and for the same reason:
    /// a test that asserts on what is on the ground needs to see the stack, and
    /// iterating the raw entity store would make every such test re-implement the
    /// `EntityBody::Item` match. Mutation still goes through the tick pipeline.
    #[must_use]
    pub fn dropped_items(&self) -> Vec<(ItemStack, mc_entity::Vec3)> {
        self.entities
            .iter()
            .filter_map(|entity| {
                let mc_entity::EntityBody::Item(item) = &entity.body else {
                    return None;
                };
                Some((item.stack, entity.position))
            })
            .collect()
    }

    /// The world spawn point, as `(x, y, z)` block coordinates.
    #[must_use]
    pub fn spawn(&self) -> (i32, i32, i32) {
        self.world.spawn()
    }

    /// View distance in chunks.
    #[must_use]
    pub const fn view_distance(&self) -> i32 {
        self.view_distance
    }

    /// How many loaded chunks are currently marked "never persist me".
    ///
    /// Exposed for tests and diagnostics because the set's only other reader is
    /// [`Game::queue_dirty_chunks`], so a set that grows without bound has no
    /// visible symptom: AUDIT-09 B-06 was exactly that — the field was inserted
    /// into on two paths and never cleared, and nothing could see it. This is the
    /// same reason [`mc_world::World::light_cache_len`] exists.
    #[must_use]
    pub fn placeholder_chunk_count(&self) -> usize {
        self.placeholder_without_storage.len()
    }

    /// Spawn a dropped-item entity holding `stack`.
    ///
    /// This closes the Phase 04 gap where a dropped stack was taken out of the
    /// inventory and then discarded. The physics live in [`mc_entity::item_entity`];
    /// this crate runs them (Entities phase) and reaps the entity when the despawn
    /// timer fires.
    ///
    /// What a drop gets now that P11-04..08 have landed: the `add_entity` +
    /// `set_entity_data` pair in the Broadcast phase (so a client renders it), the
    /// merge and pickup passes in the Entities phase
    /// ([`Self::merge_and_collect_items`]), and a place in its chunk's saved
    /// entity list ([`Self::serialize_chunk_entities`]). What it still does
    /// **not** get, and does not fake: a per-drop pickup delay or despawn timer
    /// other than [`mc_entity::item_entity::ItemEntity::new`]'s defaults, and the
    /// owner-only pickup rule (the `owner` argument is carried and never read).
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
        let id = self.entities.spawn(
            EntityBody::Item(ItemEntity::new(stack, owner)),
            to_entity(position),
        )?;
        // Announced in the Broadcast phase; see the field's docs for why not here.
        self.pending_entity_spawns.push(id);
        Ok(id)
    }

    /// Spawn one mob of `kind` at `position`, announcing it like a drop.
    ///
    /// The spawn cycle's packs come through here, and so do tests and admin
    /// tooling; the broadcast phase turns the pending id into an `add_entity`
    /// with the kind's registry id and its spawn health.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the entity store refuses (its cap or id
    /// space), exactly as [`Self::spawn_item`] reports.
    pub fn spawn_mob(
        &mut self,
        kind: mc_entity::mob::MobKind,
        position: mc_entity::player::Vec3,
    ) -> ServerResult<EntityId> {
        let id = self
            .entities
            .spawn(EntityBody::Mob(Mob::new(kind)), position)?;
        // Announced in the Broadcast phase; see the field's docs for why not here.
        self.pending_entity_spawns.push(id);
        Ok(id)
    }

    /// The owned world handle, when there is one.
    #[must_use]
    pub const fn storage(&self) -> Option<&WorldService> {
        self.storage.as_ref()
    }

    /// The redstone classifier over the block registry (P13-02).
    ///
    /// Built per call because it is `Copy` over a reference: there is nothing
    /// to cache, and a stored table could outlive a registry swap.
    fn redstone_table(&self) -> mc_redstone::EmitterTable<'_> {
        mc_redstone::EmitterTable::new(&self.registries.blocks)
    }

    /// Feed a block change at `(x, y, z)` into the redstone model (P13-02).
    ///
    /// Queues neighbour updates for the six neighbours plus the position
    /// itself — but only when the changed block or at least one neighbour is
    /// redstone-relevant (wire or emitter). An unconditional feed would turn
    /// every dirt placement into seven queue entries the propagation then has
    /// to classify and discard, which is a hostile-amplification path: each
    /// placement is player-rate-limited, yet junk would still eat the
    /// 1024-per-tick neighbour budget real circuits need. Deduplication and
    /// the 4096-entry cap live in the queue itself.
    fn redstone_feed(&mut self, x: i32, y: i32, z: i32, changed_id: i32) {
        let table = self.redstone_table();
        let relevant = |id: i32| !matches!(table.classify(id), mc_redstone::BlockRole::Passive);
        if !relevant(changed_id) {
            let pos = mc_redstone::BlockPos::new(x, y, z);
            let mut any_neighbour = false;
            for neighbour in mc_redstone::NeighbourSet::of(pos) {
                if let Some(id) = self
                    .world
                    .get_block_loaded(neighbour.x, neighbour.y, neighbour.z)
                    && relevant(id)
                {
                    any_neighbour = true;
                    break;
                }
            }
            if !any_neighbour {
                return;
            }
        }
        let pos = mc_redstone::BlockPos::new(x, y, z);
        mc_redstone::propagation::prepare(&mut self.scheduled_ticks, pos);
        mc_redstone::propagation::prepare_self(&mut self.scheduled_ticks, pos);
    }

    /// Queued redstone work (neighbour updates + scheduled ticks) still waiting.
    ///
    /// Test hook: the `ScheduledTicks` phase drains scheduled ticks, while
    /// neighbour updates wait for the propagation call P13-03 adds.
    #[must_use]
    pub fn redstone_pending(&self) -> usize {
        self.scheduled_ticks.len()
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

    /// Flip a lever's `powered` property, preserving the rest (P13-02).
    ///
    /// Only `powered` changes: facing/face stay whatever placement gave them,
    /// because guessing orientation would move the lever's attachment. A lever
    /// with no `powered` property (registry drift) is left alone rather than
    /// rewritten.
    fn flip_lever(&mut self, id: ConnectionId, x: i32, y: i32, z: i32) {
        let Some(state) = self.world.get_block_loaded(x, y, z) else {
            return;
        };
        let Ok(mut props) = self.registries.blocks.properties_of(state) else {
            debug!(id = %id, "lever state is missing from the registry");
            return;
        };
        let on = if let Some(powered) = props.iter_mut().find(|(name, _)| name == "powered") {
            if powered.1 == "true" {
                "false".clone_into(&mut powered.1);
            } else {
                "true".clone_into(&mut powered.1);
            }
            powered.1.clone()
        } else {
            debug!(id = %id, "lever has no powered property; left alone");
            return;
        };
        let Ok(new_id) = self.registries.blocks.state_id("minecraft:lever", &props) else {
            debug!(id = %id, "flipped lever state does not resolve");
            return;
        };
        if new_id == state {
            return;
        }
        if self.world.set_block(x, y, z, new_id).is_err() {
            return;
        }
        debug!(id = %id, x, y, z, powered = on, "lever flipped");
        self.redstone_feed(x, y, z, new_id);
    }

    /// World difficulty (P14-01).
    #[must_use]
    pub const fn difficulty(&self) -> Difficulty {
        self.difficulty
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

    /// Set the player cap. The lifecycle is the authority for this value.
    pub const fn set_max_players(&mut self, max_players: u32) {
        self.max_players = max_players;
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

    /// Install the loaded loot tables (P11-04).
    ///
    /// Called once from the pack loader; a game that never loads packs keeps
    /// the empty registry, whose only honest reading is "no table drops
    /// anything" — vanilla's own behaviour for a block with no loot table.
    pub fn set_loot(&mut self, loot: mc_data::loot::LootTables) {
        self.loot = loot;
    }

    /// The loaded loot tables.
    #[must_use]
    pub const fn loot(&self) -> &mc_data::loot::LootTables {
        &self.loot
    }

    /// Install the pack-converted crafting table (P12-07).
    pub fn set_crafting_registry(&mut self, registry: mc_container::RecipeRegistry) {
        self.crafting_registry = registry;
    }

    /// The crafting table (baseline until packs load).
    #[must_use]
    pub const fn crafting_registry(&self) -> &mc_container::RecipeRegistry {
        &self.crafting_registry
    }

    /// Install the pack-converted furnace table (P12-08).
    pub fn set_smelting_furnace(&mut self, registry: mc_container::SmeltingRegistry) {
        self.smelting_furnace = registry;
    }

    /// The furnace table (baseline until packs load).
    #[must_use]
    pub const fn smelting_furnace(&self) -> &mc_container::SmeltingRegistry {
        &self.smelting_furnace
    }

    /// Roll a block's loot table and drop the stacks at the block centre.
    ///
    /// The table name is `minecraft:blocks/<stem>`; a block with no table (or
    /// an unmodelled one) drops nothing, which is vanilla's rule. The context
    /// carries **no enchantments** — tool enchants are not modelled, so every
    /// enchantment-dependent condition refuses, which is the no-silk-touch
    /// reading of a bare hand — and `survives_explosion: true`, because a
    /// player break is not an explosion.
    fn spawn_block_drops(&mut self, block_id: i32, x: i32, y: i32, z: i32) {
        let Ok(name) = self.registries.blocks.block_name(block_id) else {
            return;
        };
        // Own the name so the mutable work below cannot see the registry borrow.
        let name = name.to_owned();
        let stem = name.strip_prefix("minecraft:").unwrap_or(&name);
        let Ok(table_id) = mc_core::ids::ResourceId::parse(&format!("minecraft:blocks/{stem}"))
        else {
            return;
        };
        // Same owner decision as the death context above: a player's hand is
        // known and unenchanted, so `match_tool` reads level 0 instead of
        // refusing the whole table — which is how a bare-handed stone break
        // reaches the cobblestone child of its `alternatives`.
        let context = mc_data::loot::LootContext {
            enchantment_levels: Some(BTreeMap::new()),
            survives_explosion: Some(true),
            block_properties: None,
            luck: None,
        };
        let centre =
            mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
        self.spawn_loot_table(&table_id, centre, &context);
    }

    /// Roll one loot table and drop the stacks at `centre`.
    ///
    /// Shared by the block-break and mob-death paths; a missing table drops
    /// nothing (vanilla's rule for a block or entity without one), and a
    /// refused roll drops nothing rather than half-issuing.
    fn spawn_loot_table(
        &mut self,
        table_id: &mc_core::ids::ResourceId,
        centre: mc_world::Vec3,
        context: &mc_data::loot::LootContext,
    ) {
        let Some(table) = self.loot.by_name(table_id) else {
            debug!(%table_id, "no loot table");
            return;
        };
        let mut rng = LootRng(&mut self.random);
        // The scoped roll (owner decision 2): an unmodelled construct refuses
        // its pool, not the table, so a table with one branch this build cannot
        // execute still drops what it can. Every skipped pool is logged with
        // its reason — a silently reduced drop is the failure mode this
        // replaces, and the log is where it becomes visible.
        let outcome = mc_data::loot::roll_scoped(table, &self.loot, &mut rng, context);
        let stacks = match outcome {
            Ok(outcome) => {
                for refusal in &outcome.skipped {
                    info!(%table_id, refusal = %refusal, "a pool of the loot table was skipped");
                }
                outcome.stacks
            }
            Err(error) => {
                debug!(%table_id, %error, "the loot roll refused; nothing drops");
                return;
            }
        };
        for stack in stacks {
            let Ok(item_id) = self.registries.items.id(&stack.item.to_string()) else {
                warn!(item = %stack.item, "the loot table named an item the registry does not hold");
                continue;
            };
            // `(item_id, count)`, in that order: see the same call in
            // `load_chunk_entities`. Reversed, every loot drop named the item
            // whose registry id equalled the intended count, so a broken stone
            // block dropped 35x `minecraft:stone` instead of 1x
            // `minecraft:cobblestone` (found by
            // `loot_and_pickup::a_survival_break_drops_what_the_loot_table_says`).
            let Ok(stack) = mc_entity::stack::ItemStack::new(item_id, stack.count) else {
                // A count above the hard stack limit is the only way this fails,
                // and it drops the stack, so it is logged rather than swallowed.
                warn!(
                    item = %stack.item,
                    count = stack.count,
                    "a loot table asked for a stack no inventory could hold; the drop is skipped"
                );
                continue;
            };
            match self.spawn_item_owned(stack, centre, None) {
                Ok(entity) => debug!(%entity, %table_id, "loot drop spawned"),
                Err(error) => warn!(%error, "could not spawn a block drop"),
            }
        }
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
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(text),
                    chat_type: CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
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
    let player_idx = menu.player_container_index();
    let Some(container) = menu.container_mut(player_idx) else {
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
    let player_idx = menu.player_container_index();
    let Some(container) = menu.container(player_idx) else {
        return;
    };
    let slots = container.len().min(inventory.stored_slots());
    for index in 0..slots {
        let _ = inventory.set_slot(index, container.get(index));
    }
}

/// Copy a block menu's block container (index 0) back into its block entity.
///
/// The player half goes through [`write_back_inventory`]; this is the block
/// half. A missing entity or a size mismatch is a no-op rather than a panic:
/// the menu still holds the items, so the next open replays them.
fn write_back_block(menu: &mc_container::Menu, entity: &mut mc_container::BlockEntity) {
    let Some(container) = menu.container(0) else {
        return;
    };
    let Some(items) = entity.data.items_mut() else {
        return;
    };
    if items.len() != container.len() {
        return;
    }
    for (index, slot) in items.iter_mut().enumerate() {
        *slot = container.get(index);
    }
}

/// Recompute the player menu's crafting result from the grid (P12-07).
///
/// Reads grid container 2 (4 slots at width 2), writes result container 1.
/// Returns `Ok(true)` when the result slot changed.
fn recompute_crafting_result(
    menu: &mut mc_container::Menu,
    registry: &mc_container::RecipeRegistry,
    items: &mc_registry::ItemRegistry,
    sizes: &mc_entity::stack::StackSizeTable,
) -> mc_core::error::ServerResult<bool> {
    let grid: Vec<mc_entity::stack::ItemStack> = match menu.container(2) {
        Some(container) => (0..container.len()).map(|i| container.get(i)).collect(),
        None => return Ok(false),
    };
    let result = registry.recompute_result(&grid, 2, items, sizes)?;
    let changed = match menu.container(1) {
        Some(container) => container.get(0) != result,
        None => false,
    };
    if changed && let Some(container) = menu.container_mut(1) {
        let _ = container.set(0, result);
    }
    Ok(changed)
}

/// Consume one craft from the player menu's grid (P12-07).
///
/// Returns `Ok(true)` when a recipe matched and the grid was consumed.
/// `Ok(false)` means the grid matches nothing — the result take was stale.
fn take_craft_result(
    menu: &mut mc_container::Menu,
    registry: &mc_container::RecipeRegistry,
    items: &mc_registry::ItemRegistry,
) -> mc_core::error::ServerResult<bool> {
    let mut grid: Vec<mc_entity::stack::ItemStack> = match menu.container(2) {
        Some(container) => (0..container.len()).map(|i| container.get(i)).collect(),
        None => return Ok(false),
    };
    if registry.craft(&mut grid, 2, items)?.is_none() {
        return Ok(false);
    }
    if let Some(container) = menu.container_mut(2) {
        for (index, stack) in grid.iter().enumerate() {
            let _ = container.set(index, *stack);
        }
    }
    Ok(true)
}

fn wire_stack(stack: mc_entity::stack::ItemStack) -> mc_protocol::packets::play::ItemStack {
    match stack.item_id() {
        Some(id) if !stack.is_empty() => {
            mc_protocol::packets::play::ItemStack::simple(id, stack.count())
        }
        _ => mc_protocol::packets::play::ItemStack::empty(),
    }
}

/// An NBT int field that tolerates Byte/Short/Int/Long (vanilla writes Short
/// for some of these; our writer uses Int/Byte). Returns `None` for anything
/// else, so the caller logs and skips rather than guessing.
fn nbt_int(tag: Option<&mc_nbt::NbtTag>) -> Option<i32> {
    match tag {
        Some(mc_nbt::NbtTag::Byte(v)) => Some(i32::from(*v)),
        Some(mc_nbt::NbtTag::Short(v)) => Some(i32::from(*v)),
        Some(mc_nbt::NbtTag::Int(v)) => Some(*v),
        Some(mc_nbt::NbtTag::Long(v)) => i32::try_from(*v).ok(),
        _ => None,
    }
}

/// Mark the chunk holding `(x, z)` dirty so the next autosave persists the
/// block-entity change there. World edits mark through `set_block`; container
/// transactions, furnace cooks and hopper moves do not touch the world, so
/// without this an entity in an otherwise clean chunk would never save —
/// the same divergence P11-08 records for entities.
fn mark_block_dirty(world: &mut World, x: i32, z: i32) {
    let pos = mc_world::ChunkPos {
        x: x >> 4,
        z: z >> 4,
    };
    if let Some(chunk) = world.chunk_mut(pos) {
        chunk.dirty = true;
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

/// Which block window to open. Double chests are deliberately a single 27-slot
/// window here: merging two block entities is recorded as a gap, not guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Chest,
    Furnace,
    Hopper,
}

/// Classify a block name for the right-click-open path, or `None` to place.
fn open_kind_for(name: &str) -> Option<OpenKind> {
    match name {
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel" => Some(OpenKind::Chest),
        "minecraft:furnace" => Some(OpenKind::Furnace),
        "minecraft:hopper" => Some(OpenKind::Hopper),
        _ => None,
    }
}

/// Whether right-click opens rather than places.
fn is_container_block(name: &str) -> bool {
    open_kind_for(name).is_some()
}

/// Degrees to the wire's 1/256 of a degree, signed.
///
/// The truncation is the wire's resolution rather than an accident: the client reads a byte and multiplies by
/// 360/256, so anything finer would be lost anyway. A NaN or an infinity -- which a caller should not produce --
/// becomes 0 rather than saturating, because building a spawn packet is allowed to carry a wrong angle but is not
/// allowed to panic.
fn wire_angle(degrees: f32) -> i8 {
    if !degrees.is_finite() {
        return 0;
    }
    let steps = (degrees * 256.0 / 360.0).round();
    if steps <= f32::from(i8::MIN) {
        i8::MIN
    } else if steps >= f32::from(i8::MAX) {
        i8::MAX
    } else {
        steps as i8
    }
}
