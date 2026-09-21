//! Player sessions: connection state, join/leave, intents (P15-03).
//!
//! Mechanical split of `super` (the 7.8k-line `game.rs`): every item here
//! moved byte-identical except `Session`-family field visibility, widened
//! to `pub(crate)` because the rest of `game` reads them. No logic changed;
//! `super` keeps the struct, the tick and the persistence.

use mc_core::error::{ServerError, ServerResult};
use mc_entity::combat::FIST_DAMAGE;
use mc_entity::entity::{EntityBody, EntityId};
use mc_entity::inventory::Hand;

use mc_entity::player::{DamageOutcome, GameMode, Player};
use mc_network::bridge::{ConnectionId, OutboundSender};
use mc_persistence::chunk::ChunkPos;
use mc_persistence::level::Difficulty;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockDestruction, ContainerSetContent, ContainerSetSlot, GameEvent, MENU_FURNACE,
    MENU_GENERIC_9X3, MENU_HOPPER, OpenScreen, PlayIntent, PlayerPosition, Respawn,
    SetDefaultSpawnPosition, SetHeldSlot, SetTime, block_position, unpack_block_position,
};
use mc_protocol::text::TextComponent;
use mc_world::{Aabb, Vec3};
use std::collections::BTreeSet;
use tracing::{debug, info, warn};

use super::{
    ACTION_ABORT_DESTROY_BLOCK, ACTION_DROP_ITEM, ACTION_FINISH_DESTROY_BLOCK,
    ACTION_START_DESTROY_BLOCK, ACTION_SWAP_ITEM_WITH_OFFHAND, CHAT_TYPE_CHAT,
    CLIENT_COMMAND_RESPAWN, EYE_HEIGHT, FALL_DAMAGE_THRESHOLD, GAME_EVENT_LEVEL_CHUNKS_LOAD_START,
    Game, NO_BLOCK_CHANGE_SEQUENCE, OpenKind, TickReport, block_reach, chunk_of, entity_reach,
    face_offset, floor_to_i32, is_container_block, mark_block_dirty, mirror_inventory,
    open_kind_for, recompute_crafting_result, refuse_join, take_craft_result, targets_a_block,
    wire_stack, write_back_block, write_back_inventory,
};

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
    pub(crate) entity: EntityId,
    pub(crate) outbound: OutboundSender,
    /// Chunks already sent, so streaming is incremental.
    pub(crate) sent_chunks: BTreeSet<ChunkPos>,
    /// The chunk centre the client was last told (P14-04 follow-up).
    ///
    /// The connection tells the client `(0, 0)` at enter-play; the game sets
    /// the join chunk here and re-sends on every chunk crossing, before the
    /// chunks themselves stream. Without it the client culls and waits on a
    /// stale centre: walking far shows nothing new, and respawning far away
    /// sticks on "Loading terrain".
    pub(crate) center: ChunkPos,
    /// The view distance this session streams at (P14-04).
    ///
    /// Starts at the server maximum and follows the client's
    /// `client_information` setting down from there (never up: the server
    /// maximum caps it, like Vanilla). Streaming and unloading both read
    /// this, so a client that turns its distance down stops receiving far
    /// chunks and is told to forget the ones it had.
    pub(crate) view_distance: i32,
    /// Position at the start of this tick, for fall-damage accounting.
    pub(crate) tick_start_y: f64,
    /// What this connection is permitted to do.
    ///
    /// Set at join from `ops.json` (listed uuids hold their file level,
    /// everyone else [`mc_command::PermissionLevel::All`]) and raised or
    /// lowered afterwards only by `/op` and `/deop`, which persist the file
    /// first (P14-02). A stale comment here once claimed no storage was read;
    /// `ops_e2e` proves the join path reads it (P08-08 review).
    pub(crate) permission: mc_command::PermissionLevel,
    /// The name this player joined with.
    ///
    /// Kept because the value arrives once, in the login handshake, and chat needs it afterwards: the join
    /// log could print it and nothing else could reach it. **"The server knows this" and "the server can say
    /// this" are different states**, and this field is the difference.
    pub(crate) name: String,
    /// The profile uuid, hyphenated, as `ops.json` keys on it (P14-02).
    ///
    /// Matching operators by name would let anyone take an operator's identity
    /// by taking their name, so grants resolve through this instead.
    pub(crate) uuid: String,
    /// The container window this player has open.
    ///
    /// Window 0 is always the player inventory; opening a chest, furnace or
    /// hopper replaces it with a block menu on a non-zero window (P12-01).
    /// The menu is the **authority** for item placement: `container_click` is
    /// decoded, validated against this state and applied here, never trusted.
    pub(crate) menu: mc_container::Menu,
    /// Next non-zero window id to hand out (1..=127, wrapping, never 0).
    pub(crate) next_window: u8,
    /// Which block entity the open window belongs to, when it is a block menu.
    pub(crate) open_block: Option<mc_container::BlockPos>,
    /// Whether this player may receive world packets.
    pub(crate) ready: bool,
    /// Ticks of shared hurt invulnerability left (P11-06): a mob hit inside the
    /// window is refused, exactly like `Entity::invulnerable_ticks` for
    /// non-players, which the authoritative `Player` does not carry.
    pub(crate) hurt_invuln_ticks: u32,
    /// Ticks until this session may pick up another experience orb (vanilla's
    /// per-player 2-tick throttle; orbs themselves carry no pickup delay).
    pub(crate) xp_pickup_cooldown: u8,
    /// Where this player last died, as `(dimension key, packed block position)`.
    ///
    /// Recorded in [`Game::after_damage`] and sent in [`Respawn`]'s
    /// `lastDeathLocation` field, which is what vanilla's `ServerPlayer` carries
    /// for the same purpose. Without it the packet would have to claim "no death
    /// location" for a player who plainly died somewhere. What the 26.1.2 client
    /// *does* with the field is not verified on this build; the death screen's own
    /// buttons do not read it (`javap` on `DeathScreen` shows no reference), so
    /// this is fidelity to the wire contract rather than a feature.
    pub(crate) last_death_location: Option<(String, i64)>,
    /// Highest client block-prediction sequence this connection has sent, or `-1`
    /// for "nothing to acknowledge yet".
    ///
    /// Vanilla's `ServerGamePacketListenerImpl.ackBlockChangesUpTo` is the same
    /// high-water mark, sent once per tick as `block_changed_ack` and then reset to
    /// `-1`. It is not an optimisation: until the client receives this, its
    /// pending prediction at that position swallows every server block change
    /// there (see [`mc_protocol::packets::play::BlockChangedAck`]).
    pub(crate) ack_block_changes_up_to: i32,
    /// Block being dug in survival, if any (P16-05): target, accumulated
    /// progress, last announced crack stage, and the held item the dig
    /// started with. Creative never sets this (instant breaks); abort, held
    /// change, target loss and walking out of reach clear it.
    pub(crate) dig: Option<DigState>,
}

/// One survival dig in progress.
///
/// Progress accumulates per tick from the mining tables; the block breaks at
/// 1.0. `stage` is the last announced crack overlay (`-1` = none sent yet);
/// `held` is the held item id (`None` = empty hand) the dig started with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DigState {
    /// Target block position.
    pub x: i32,
    /// Target block position.
    pub y: i32,
    /// Target block position.
    pub z: i32,
    /// Block-state id digging started on; any change cancels the dig.
    pub block: i32,
    /// Accumulated progress; breaks at 1.0.
    pub progress: f32,
    /// Last announced overlay stage, or -1 when none was sent.
    pub stage: i8,
    /// Held item when digging started (`None` = empty hand).
    pub held: Option<i32>,
}

/// A block about to break: position plus the state id in play.
///
/// Groups the four positional values `break_block_now` took separately, so
/// the call sites cannot swap a coordinate with the block id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BreakTarget {
    /// Target block position.
    pub x: i32,
    /// Target block position.
    pub y: i32,
    /// Target block position.
    pub z: i32,
    /// Block-state id at the target.
    pub block: i32,
}

impl Session {
    pub(crate) fn aabb(&self) -> Aabb {
        Aabb::player(self.player.position)
    }

    /// The chunk this player currently stands in.
    pub(crate) fn chunk(&self) -> ChunkPos {
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
    pub(crate) session: &'a Session,
}

impl SessionView<'_> {
    /// The menu slot count.
    #[must_use]
    pub fn menu_slot_count(self) -> usize {
        self.session.menu.slot_count()
    }

    /// The window id.
    #[must_use]
    pub fn menu_window_id(self) -> u8 {
        self.session.menu.window_id()
    }

    /// The revision counter.
    #[must_use]
    pub fn menu_state_id(self) -> i32 {
        self.session.menu.state_id()
    }

    /// The stack in a menu slot, empty when out of range.
    #[must_use]
    pub fn menu_slot(self, slot: usize) -> mc_entity::stack::ItemStack {
        self.session.menu.display_stack(slot)
    }

    /// The cursor stack.
    #[must_use]
    pub fn menu_cursor(self) -> mc_entity::stack::ItemStack {
        self.session.menu.cursor()
    }

    /// Total items in the player\'s own inventory, excluding the cursor.
    ///
    /// The instrument the duplication regression tests assert against: a drop or a
    /// placement must move this number, and a no-op click must not.
    #[must_use]
    pub fn inventory_total(self) -> i64 {
        let slots = self.session.player.inventory.stored_slots();
        (0..slots)
            .map(|index| i64::from(self.session.player.inventory.slot(index).count()))
            .sum()
    }
}

impl Game {
    /// Remove a connection's player and mark its entity for the sweep.
    pub(crate) fn leave(&mut self, id: ConnectionId) {
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
        // Remember the live state for a rejoin within this run (P14-09 walk:
        // disconnects put the player back at spawn). The cursor merge above
        // already ran, so the inventory stored here is complete.
        self.remembered
            .insert(session.uuid.clone(), session.player.clone());
        // And persist it to `playerdata/<uuid>.dat` so a *restart* also
        // restores it (P14-10 walk). A failed write warns and keeps the
        // in-memory copy as the fallback rather than failing the leave.
        if let Some(root) = self.playerdata_root() {
            match session.player.to_nbt(&self.registries.items) {
                Ok(tag) => {
                    if let Err(error) = crate::playerdata::save(&root, &session.uuid, &tag) {
                        warn!(id = %id, %error, "playerdata save failed; memory copy kept");
                    }
                }
                Err(error) => {
                    warn!(id = %id, %error, "playerdata encode failed; memory copy kept");
                }
            }
        }
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
    pub(crate) fn join(
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
        let mut at = Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
        // `playerdata/<uuid>.dat` outranks memory: files survive restarts,
        // memory only disconnects. A corrupt file warns here and rejoins
        // fresh; the position peek decides where the entity spawns, so a
        // stored position is honoured before anything else reads it.
        let mut file_tag: Option<mc_nbt::NbtTag> = None;
        if let Some(root) = self.playerdata_root() {
            match crate::playerdata::load(&root, &profile.id.to_string()) {
                Ok(tag) => {
                    if let Some(ref tag) = tag
                        && let Some(pos) = crate::playerdata::peek_pos(tag)
                    {
                        at = pos;
                    }
                    file_tag = tag;
                }
                Err(error) => {
                    warn!(id = %id, %error, "corrupt playerdata; starting fresh");
                }
            }
        }
        // The in-memory copy covers disconnects whose file write failed;
        // the file above already decided `at`.
        let restored = self
            .remembered
            .get(&profile.id.to_string())
            .filter(|former| former.is_alive())
            .cloned();
        if file_tag.is_none()
            && let Some(ref former) = restored
        {
            at = former.position;
        }
        // The entity store allocates the id, so it is unique across every entity in
        // the dimension and never reused — the property the wire protocol needs.
        let Ok(entity) = self.entities.spawn(EntityBody::Player, at) else {
            warn!(id = %id, "refused a join: the entity store is full");
            refuse_join(&outbound, "Server entity limit reached");
            return Ok(());
        };
        let entity_id = entity.get();

        // A stored file rebuilds the whole player (position, health, inventory,
        // mode); a stored death rejoins fresh at spawn. Otherwise the fresh
        // player below, with the in-memory restore after it.
        let from_file = file_tag.is_some();
        let mut player = match file_tag {
            Some(tag) => {
                match Player::from_nbt(
                    &tag,
                    prepared.profile.clone(),
                    entity_id,
                    &self.registries.items,
                ) {
                    Ok(loaded) if loaded.health > 0.0 => loaded,
                    Ok(_) => {
                        debug!(id = %id, "stored death rejoins fresh at spawn");
                        Player::new(
                            prepared.profile,
                            entity_id,
                            "minecraft:overworld",
                            prepared.inventory,
                        )
                    }
                    Err(error) => {
                        warn!(id = %id, %error, "playerdata decode failed; starting fresh");
                        Player::new(
                            prepared.profile,
                            entity_id,
                            "minecraft:overworld",
                            prepared.inventory,
                        )
                    }
                }
            }
            None => Player::new(
                prepared.profile,
                entity_id,
                "minecraft:overworld",
                prepared.inventory,
            ),
        };
        if !from_file {
            player.position = at;
            player.game_mode = GameMode::Survival;
            if let Some(former) = restored {
                // The menu below mirrors this inventory, and the position packet
                // below reports `at`, so restoring here keeps every copy honest.
                player.yaw = former.yaw;
                player.pitch = former.pitch;
                player.health = former.health;
                player.food = former.food;
                player.inventory = former.inventory;
                player.game_mode = former.game_mode;
                // Active effects ride along too, or a rejoin would keep the
                // server-side timers (fresh player has none — wrong either
                // way) while the join sync below found nothing to announce.
                player.effects = former.effects;
            }
        }

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
                center: chunk_of(at.x, at.z),
                tick_start_y: at.y,
                // The operator list is the authority: a listed uuid gets its file level,
                // and anyone else is level 0. `ops.json` stores the hyphenated uuid, which is
                // what `Uuid`'s `Display` produces — the conversion is a real one, since the
                // in-memory id is a `Uuid`.
                permission: self.operators.level_for(&profile.id.to_string()),
                name: profile.name.clone(),
                uuid: profile.id.to_string(),
                view_distance: self.view_distance,
                menu,
                next_window: 1,
                open_block: None,
                ready: false,
                hurt_invuln_ticks: 0,
                xp_pickup_cooldown: 0,
                last_death_location: None,
                ack_block_changes_up_to: NO_BLOCK_CHANGE_SEQUENCE,
                dig: None,
            },
        );

        // The client renders an empty hotbar until told otherwise: a rejoin
        // with a non-empty inventory showed nothing until the first
        // inventory action touched it (P14-10 walk). Sync the whole player
        // window here, with every other full-state packet, while the menu
        // was just mirrored from the authoritative inventory above.
        {
            let Some(session) = self.sessions.get(&id) else {
                warn!(id = %id, "join failed before the inventory sync");
                return Ok(());
            };
            let packet = ContainerSetContent {
                window_id: i32::from(session.menu.window_id()),
                state_id: session.menu.state_id(),
                slots: session
                    .menu
                    .full_contents()
                    .iter()
                    .copied()
                    .map(wire_stack)
                    .collect(),
                carried: wire_stack(session.menu.cursor()),
            };
            self.send(id, &packet, report)?;
        }

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
        // A rejoin restores the look direction too (P14-10 walk: the exit
        // yaw/pitch is saved, but the join teleport snapped every view to
        // 0/0). Fresh joins keep 0/0, which is what a new player has.
        let (yaw, pitch) = self.sessions.get(&id).map_or((0.0, 0.0), |session| {
            (session.player.yaw, session.player.pitch)
        });
        self.send(
            id,
            &PlayerPosition {
                x: at.x,
                y: at.y,
                z: at.z,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw,
                pitch,
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
        // Active effects ride the join like every other HUD state (P16-03):
        // without this a rejoining player keeps the server-side effect but
        // loses the icon until something re-sends it.
        if let Some(session) = self.sessions.get(&id) {
            for (effect_id, effect) in &session.player.effects {
                self.send(
                    id,
                    &mc_protocol::packets::play::UpdateMobEffect {
                        entity_id: session.entity.get(),
                        effect_id: *effect_id,
                        amplifier: effect.amplifier,
                        duration: effect.duration,
                        flags: mc_protocol::packets::play::UpdateMobEffect::flags_for(
                            effect.ambient,
                        ),
                    },
                    report,
                )?;
            }
        }
        // The join-time full clock sync (`sendLevelInfo` on vanilla): without
        // an absolute seed the client's overworld instance advances locally
        // from zero forever, and no later entry can fix a sky that never
        // learned where it started. The per-second broadcast carries an empty
        // map from here on.
        self.send(
            id,
            &SetTime {
                world_age: self.tick as i64,
                clocks: vec![self.overworld_clock_entry(self.tick)],
            },
            report,
        )?;
        if let Some(session) = self.sessions.get_mut(&id) {
            session.ready = true;
        }
        self.send(
            id,
            &mc_protocol::packets::play::DisguisedChat {
                message: TextComponent::literal("Welcome to the Rust Minecraft server."),
                chat_type: CHAT_TYPE_CHAT,
                sender_name: TextComponent::literal("Server"),
                target_name: None,
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
    pub(crate) fn apply_intent(
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
            PlayIntent::Interact { entity, kind } => {
                // Only the attack acts; a use/interact-at on an entity needs the
                // interaction surfaces (villagers, boats), which are not modelled.
                // The wire id is this server's own `EntityId` value --
                // `AddEntity` is written from `id.get()` with no base offset, so
                // the id a client echoes back is the id the store holds (checked
                // against the encoder, not assumed).
                if kind == 1 {
                    // `EntityId::new` validates; the wire id came from this
                    // server's own `add_entity`, so an invalid one is a hostile
                    // packet, refused rather than reconstructed.
                    if let Ok(target) = EntityId::new(entity) {
                        // Vanilla gates `handleInteract` on
                        // `isWithinEntityInteractionRange(aabb, 3.0)` **before** it
                        // branches on the packet's action, so a swing from across
                        // the map is refused there and must be refused here. This
                        // closes the recorded divergence "a swing outside the
                        // interaction range is not refused" (AUDIT-09).
                        if !self.within_entity_reach(id, target) {
                            debug!(id = %id, target = %target, "rejected attack outside entity reach");
                            return Ok(());
                        }
                        // Held-item damage (P16-01): fist plus the weapon's
                        // bonus, resolved against the registry like every
                        // other item lookup on this path.
                        let damage = self
                            .sessions
                            .get(&id)
                            .map_or(FIST_DAMAGE, |session| {
                                mc_entity::combat::held_damage(
                                    &session.player.inventory,
                                    &self.registries.items,
                                )
                            });
                        let attacker = self.sessions.get(&id).map(|session| {
                            mc_entity::combat::Attacker {
                                pos: session.player.position,
                                yaw: session.player.yaw,
                            }
                        });
                        let died = self.damage_entity(
                            target,
                            damage,
                            mc_entity::combat::DamageSource::PlayerAttack,
                            attacker,
                        );
                        debug!(id = %id, target = %target, damage, died, "player attack");
                    }
                }
            }
            PlayIntent::PlayerAction {
                status,
                position,
                facing,
                sequence,
            } => self.apply_player_action(id, status, position, facing, sequence, report)?,
            PlayIntent::UseItemOn {
                position,
                face,
                hand,
                sequence,
                ..
            } => {
                self.note_block_change_sequence(id, sequence);
                self.apply_use_item_on(id, position, face, hand, report);
            }
            PlayIntent::UseItem { sequence, .. } => {
                // The item use itself stays the no-op it was (no consumable, no
                // projectile, no bucket is modelled). Its **sequence** is not a
                // no-op, though: vanilla feeds it into the same high-water mark
                // from `handleUseItem`, because the client may have predicted a
                // block change the use caused, and an unacked prediction freezes
                // that position on the client.
                self.note_block_change_sequence(id, sequence);
            }
            PlayIntent::SetCarriedItem { slot } => self.apply_hotbar(id, slot, report)?,
            PlayIntent::ClientCommand { action } => {
                self.apply_client_command(id, action, report)?;
            }
            PlayIntent::Chat { message, .. } => {
                info!(id = %id, %message, "player chat");
                // **To everyone, attributed to its sender.** Vanilla sends `player_chat` (65) for a
                // player's own message, which carries the sender's UUID, chat index and signature; this build
                // has no chat signing, so `disguised_chat` -- the same message attributed to a name -- is what
                // it can honestly send. Recorded as a divergence rather than passed off as parity.
                let name = self
                    .sessions
                    .get(&id)
                    .map_or_else(|| "Player".to_owned(), |session| session.name.clone());
                // `chat_type` is resolved, not assumed: `CHAT_TYPE_CHAT` is read out of the
                // `minecraft:chat_type` registry this server sends in `registry_data` (the entry named
                // `minecraft:chat`), pinned by `the_chat_type_this_server_sends_is_the_one_named_chat`.
                // A real 26.1.2 capture answers a console `say` with this same packet and chat type.
                let packet = mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(&message),
                    chat_type: CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal(&name),
                    target_name: None,
                }
                .to_raw()?;
                self.broadcast_all(&packet, report);
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
                // Closing window 0 is a no-op (the player inventory cannot be
                // closed). Closing a block window flushes its block half,
                // returns the cursor to the inventory (dropping what does not
                // fit at the player's feet), and restores the player menu.
                // A stale or foreign window id is ignored after the flush
                // check: the close is idempotent, like vanilla's.
                let open = self.sessions.get(&id).and_then(|s| s.open_block);
                let current_window = self.sessions.get(&id).map(|s| s.menu.window_id());
                if open.is_none() || current_window == Some(0) {
                    return Ok(());
                }
                if let Some(expected) = current_window
                    && window_id != i32::from(expected)
                {
                    debug!(id = %id, window_id, "ignoring a close for a stale window");
                    return Ok(());
                }
                // Flush the block half before discarding the menu.
                if let Some(pos) = open {
                    // `open` came out of this same session map lines above, so
                    // absence here is a desync bug, surfaced as a typed error
                    // rather than the `expect` this replaced (AGENTS.md §9).
                    let menu = &self
                        .sessions
                        .get(&id)
                        .ok_or_else(|| {
                            ServerError::Invariant(format!(
                                "container close for {id:?} with no session"
                            ))
                        })?
                        .menu;
                    if let Some(entity) = self.block_entities.get_mut(pos) {
                        write_back_block(menu, entity);
                        mark_block_dirty(&mut self.world, pos.x, pos.z);
                    }
                }
                // Cursor first, so a failed inventory write can still drop it.
                let cursor = self
                    .sessions
                    .get(&id)
                    .map_or(mc_entity::stack::ItemStack::EMPTY, |s| s.menu.cursor());
                let leftover = if cursor.is_empty() {
                    mc_entity::stack::ItemStack::EMPTY
                } else if let Some(session) = self.sessions.get_mut(&id) {
                    session.player.inventory.add_stack(cursor)
                } else {
                    cursor
                };
                if !leftover.is_empty() {
                    let position = self
                        .sessions
                        .get(&id)
                        .map_or(mc_world::Vec3::default(), |s| s.player.position);
                    let _ = self.spawn_item(leftover, position);
                }
                // Restore the player menu.
                match self.new_player_menu() {
                    Ok(mut menu) => {
                        if let Some(session) = self.sessions.get_mut(&id) {
                            mirror_inventory(&mut menu, &session.player.inventory);
                            menu.set_creative(session.player.game_mode.is_creative());
                            // The cursor lived outside the window; it is empty now.
                            menu.set_cursor(mc_entity::stack::ItemStack::EMPTY);
                            session.menu = menu;
                            session.open_block = None;
                        }
                    }
                    Err(error) => {
                        debug!(id = %id, %error, "could not restore the player menu on close");
                    }
                }
            }
            // A real client closes each of its own ticks with `client_tick_end`
            // (911 captured bodies, all empty). The server's tick is its own
            // clock, so there is nothing to act on -- it is modelled so per-tick
            // arrival is silent instead of two debug lines a tick per player.
            // The three arms beside it are silent for their own reasons.
            PlayIntent::Swing { .. }
            | PlayIntent::AcceptTeleportation { .. }
            | PlayIntent::ClientTickEnd
            // A-03 additions, all decoded-but-unacted: this server sends no
            // keep-alives to answer, batches no chunks to throttle, and needs
            // no signal for load completion. Modelling them keeps the capture
            // sweep total without changing what the game does.
            | PlayIntent::KeepAlive { .. }
            | PlayIntent::ChunkBatchReceived { .. }
            | PlayIntent::PlayerLoaded => {}
        }
        Ok(())
    }

    /// Set the directory `ops.json` lives in (P14-02).
    ///
    /// The lifecycle calls this with [`crate::ops::ops_directory`] once it
    /// knows the world directory; until then `/op` reports that it cannot
    /// persist rather than failing.
    pub fn set_ops_directory(&mut self, directory: std::path::PathBuf) {
        self.ops_directory = Some(directory);
    }

    /// World root for `playerdata/<uuid>.dat`, or `None` when this game has
    /// no storage (some unit harnesses): without a world directory there is
    /// nowhere to write, and the in-memory copy is the whole story.
    fn playerdata_root(&self) -> Option<std::path::PathBuf> {
        self.storage
            .as_ref()
            .map(|service| service.root().to_path_buf())
    }

    /// Grant `target` operator status at `level`, persisting `ops.json` (P14-02).
    ///
    /// The grant applies to the live session *and* the file, in that order of
    /// rollback: the in-memory insert happens first, then the save, and a
    /// failed save rolls the insert back — so memory and file never disagree
    /// about who is an operator. Only online players can be granted (their
    /// uuid comes from the session; matching by name would let anyone take
    /// an identity). Vanilla's default grant level is 4.
    ///
    /// Returns the granted name and whether the list changed, or `None` when
    /// no session holds `target` or no ops directory was ever set (nothing is
    /// changed then).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the `ops.json` write fails, after
    /// rolling the in-memory grant back.
    pub(crate) fn grant_operator(
        &mut self,
        target: mc_network::bridge::ConnectionId,
        level: mc_command::PermissionLevel,
    ) -> ServerResult<Option<(String, bool)>> {
        let Some(directory) = self.ops_directory.clone() else {
            return Ok(None);
        };
        let (uuid, name) = {
            let Some(session) = self.sessions.get(&target) else {
                return Ok(None);
            };
            (session.uuid.clone(), session.name.clone())
        };
        let changed = self.operators.insert(&uuid, &name, level);
        if changed && let Err(error) = self.operators.save(&directory) {
            self.operators.remove(&uuid);
            return Err(error);
        }
        if let Some(session) = self.sessions.get_mut(&target) {
            session.permission = level;
        }
        Ok(Some((name, changed)))
    }

    /// Revoke `target`'s operator status, persisting `ops.json` (P14-02).
    ///
    /// Same rollback contract as [`Game::grant_operator`]: a failed save
    /// restores the removed entry. A live session is demoted to level 0 on
    /// success. Returns the revoked name, or `None` when the uuid was never
    /// listed (nothing is changed then) — or when no session holds `target`
    /// or no ops directory was ever set.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the `ops.json` write fails, after
    /// restoring the entry.
    pub(crate) fn revoke_operator(
        &mut self,
        target: mc_network::bridge::ConnectionId,
    ) -> ServerResult<Option<String>> {
        let Some(directory) = self.ops_directory.clone() else {
            return Ok(None);
        };
        let (uuid, name) = {
            let Some(session) = self.sessions.get(&target) else {
                return Ok(None);
            };
            (session.uuid.clone(), session.name.clone())
        };
        let Some(previous) = self.operators.get(&uuid).cloned() else {
            return Ok(None);
        };
        self.operators.remove(&uuid);
        if let Err(error) = self.operators.save(&directory) {
            self.operators.restore(previous);
            return Err(error);
        }
        if let Some(session) = self.sessions.get_mut(&target) {
            session.permission = mc_command::PermissionLevel::All;
        }
        Ok(Some(name))
    }

    /// The connection id of the session named `name`, if one is online.
    ///
    /// Case-insensitive like `/tp`'s target check: the name arrives from the
    /// login handshake with its original capitalisation, and a command that
    /// refused `steve` for `Steve` would be refusing a typo, not an attack.
    pub(crate) fn session_id_by_name(
        &self,
        name: &str,
    ) -> Option<mc_network::bridge::ConnectionId> {
        self.sessions.values().find_map(|session| {
            session
                .name
                .eq_ignore_ascii_case(name)
                .then_some(session.id)
        })
    }

    /// Whether `level.dat` locks the difficulty (P14-01).
    ///
    /// Without storage there is no file to lock, so nothing is locked. With
    /// storage the answer comes from the file, which is also what a fresh
    /// boot would read — the field and the file cannot disagree about this.
    #[must_use]
    pub fn difficulty_locked(&self) -> bool {
        self.storage
            .as_ref()
            .and_then(|service| service.storage().level())
            .is_some_and(|level| level.difficulty_locked)
    }

    /// Set the difficulty, writing `level.dat` back when storage is present.
    ///
    /// The field always updates (it is what the spawn gate reads); the file
    /// write makes a restart keep it. Without storage there is no file, so
    /// only the field updates — stated, because a test game otherwise looks
    /// like it persists.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the `level.dat` write fails. A locked
    /// difficulty is refused by the *caller* (`/difficulty` answers it), not
    /// here, because a lock is normal input rather than a failure.
    pub fn set_difficulty(&mut self, difficulty: Difficulty) -> ServerResult<()> {
        self.difficulty = difficulty;
        // `WorldService::open` creates `level.dat` when missing, so a stored
        // world always has a level to edit; a `None` here is defensive only.
        if let Some(service) = self.storage_mut()
            && let Some(mut level) = service.storage().level().cloned()
        {
            level.difficulty = difficulty;
            service.storage_mut().save_level(level)?;
        }
        Ok(())
    }

    /// Set a player's game mode and mirror it into their menu (P14-01).
    ///
    /// The menu is the authority for item placement, so its creative flag
    /// follows the mode the way the join and respawn paths already do.
    /// Returns whether a session was found. Broadcasting the change to other
    /// clients (player-info abilities) is not modelled; the parity matrix
    /// records the gap.
    pub(crate) fn set_player_game_mode(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        mode: GameMode,
    ) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        session.player.game_mode = mode;
        session.menu.set_creative(mode.is_creative());
        true
    }

    /// Give `count` of `item` to a player, dropping what does not fit (P14-01).
    ///
    /// Vanilla's `/give` drops the remainder at the player's feet rather than
    /// refusing or deleting it, and `add_stack` spreads across slots rather
    /// than dropping — so the remainder of `add_stack` is spawned, mirroring
    /// the close-rebuild path. Returns `(placed, dropped)`, or `None` when no
    /// session holds `id`.
    pub(crate) fn give_player_item(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        item: i32,
        count: i32,
    ) -> Option<(i32, i32)> {
        let stack = mc_entity::stack::ItemStack::new(item, count).ok()?;
        let leftover = {
            let session = self.sessions.get_mut(&id)?;
            session.player.inventory.add_stack(stack)
        };
        let dropped = leftover.count();
        if !leftover.is_empty() {
            let position = self
                .sessions
                .get(&id)
                .map_or(mc_world::Vec3::default(), |s| s.player.position);
            let _ = self.spawn_item(leftover, position);
        }
        Some((count - dropped, dropped))
    }

    /// Kill a player through the damage path, bypassing invulnerability (P14-01).
    ///
    /// Vanilla's `/kill` works in creative mode; [`Player::kill`] is not
    /// ordinary damage for the same reason. Drops and the death message run
    /// through [`Game::after_damage`] like any other lethal hit. Returns the
    /// outcome, or `None` when no session holds `id`.
    pub(crate) fn kill_player(
        &mut self,
        id: mc_network::bridge::ConnectionId,
    ) -> Option<DamageOutcome> {
        let outcome = self.sessions.get_mut(&id)?.player.kill();
        self.after_damage(id, outcome);
        Some(outcome)
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
        session.player.position = mc_world::Vec3::new(resolved.x, resolved.y, resolved.z);
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
    pub(crate) fn sync_menu_from_inventory(&mut self, id: ConnectionId, report: &mut TickReport) {
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
        let window = i32::from(session.menu.window_id());
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
        // The player container is index 1 in block menus (0 is the block);
        // hard-coding 0 compares the chest against the inventory (AUDIT-12).
        let Some(container) = session
            .menu
            .container(session.menu.player_container_index())
        else {
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
    pub(crate) fn new_player_menu(&self) -> ServerResult<mc_container::Menu> {
        let stack_sizes = mc_entity::stack::StackSizeTable::resolve(&self.registries.items)?;
        let slots = mc_entity::inventory::PlayerInventory::new(stack_sizes.clone()).stored_slots();
        let container = mc_container::Container::new(mc_container::ContainerKind::Player, slots)?;
        mc_container::Menu::player(mc_container::PLAYER_WINDOW_ID, container, stack_sizes)
    }

    /// Block name at a loaded position, for the container-open check.
    fn clicked_block_name(&self, x: i32, y: i32, z: i32) -> Option<String> {
        let state = self.world.get_block_loaded(x, y, z)?;
        self.registries
            .blocks
            .block_name(state)
            .ok()
            .map(str::to_owned)
    }

    /// Open the container at `(x, y, z)` for `id`: ensure its block entity,
    /// build a block menu on a fresh non-zero window, and send `open_screen`
    /// plus the full contents. A second open replaces the first window's menu
    /// (the old block half was already flushed on every click).
    #[allow(clippy::too_many_lines)]
    fn open_container(
        &mut self,
        id: ConnectionId,
        x: i32,
        y: i32,
        z: i32,
        report: &mut TickReport,
    ) {
        let Some(name) = self.clicked_block_name(x, y, z) else {
            return;
        };
        let Some(kind) = open_kind_for(&name) else {
            return;
        };
        let pos = mc_container::BlockPos::new(x, y, z);
        // Ensure the block entity exists so the menu has somewhere to flush to.
        if self.block_entities.get(pos).is_none() {
            let entity_kind = match kind {
                OpenKind::Chest => mc_container::BlockEntityKind::Container,
                OpenKind::Furnace => mc_container::BlockEntityKind::Furnace,
                OpenKind::Hopper => mc_container::BlockEntityKind::Hopper,
            };
            self.block_entities
                .insert(mc_container::BlockEntity::new(pos, entity_kind));
            mark_block_dirty(&mut self.world, x, z);
        }
        let items: Vec<mc_entity::stack::ItemStack> = self
            .block_entities
            .get(pos)
            .and_then(|e| e.data.items())
            .map_or(Vec::new(), <[mc_entity::stack::ItemStack]>::to_vec);

        // Window ids 1..=127 stay positive in the legacy `i8` window fields of
        // `container_set_content/slot`; `open_screen` itself is a `VarInt`.
        let window = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return;
            };
            let window = session.next_window;
            session.next_window = if window >= 127 { 1 } else { window + 1 };
            window
        };
        let stack_sizes = match mc_entity::stack::StackSizeTable::resolve(&self.registries.items) {
            Ok(sizes) => sizes,
            Err(error) => {
                debug!(id = %id, %error, "could not resolve stack sizes for a container");
                return;
            }
        };
        let player_inventory_slots = match self.sessions.get(&id) {
            Some(session) => session.player.inventory.stored_slots(),
            None => return,
        };
        let player_container = match mc_container::Container::new(
            mc_container::ContainerKind::Player,
            player_inventory_slots,
        ) {
            Ok(container) => container,
            Err(error) => {
                debug!(id = %id, %error, "could not build the player half of a container");
                return;
            }
        };
        let (mut menu, menu_type, title) = match kind {
            OpenKind::Chest => {
                let mut block =
                    match mc_container::Container::new(mc_container::ContainerKind::Generic, 27) {
                        Ok(container) => container,
                        Err(error) => {
                            debug!(id = %id, %error, "could not build a chest container");
                            return;
                        }
                    };
                for (index, stack) in items.iter().enumerate().take(27) {
                    let _ = block.set(index, *stack);
                }
                let menu =
                    match mc_container::Menu::chest(window, block, player_container, stack_sizes) {
                        Ok(menu) => menu,
                        Err(error) => {
                            debug!(id = %id, %error, "could not build a chest menu");
                            return;
                        }
                    };
                (menu, MENU_GENERIC_9X3, "Chest")
            }
            OpenKind::Furnace => {
                let mut block =
                    match mc_container::Container::new(mc_container::ContainerKind::Furnace, 3) {
                        Ok(container) => container,
                        Err(error) => {
                            debug!(id = %id, %error, "could not build a furnace container");
                            return;
                        }
                    };
                for (index, stack) in items.iter().enumerate().take(3) {
                    let _ = block.set(index, *stack);
                }
                let menu =
                    match mc_container::Menu::furnace(window, block, player_container, stack_sizes)
                    {
                        Ok(menu) => menu,
                        Err(error) => {
                            debug!(id = %id, %error, "could not build a furnace menu");
                            return;
                        }
                    };
                (menu, MENU_FURNACE, "Furnace")
            }
            OpenKind::Hopper => {
                let mut block =
                    match mc_container::Container::new(mc_container::ContainerKind::Generic, 5) {
                        Ok(container) => container,
                        Err(error) => {
                            debug!(id = %id, %error, "could not build a hopper container");
                            return;
                        }
                    };
                for (index, stack) in items.iter().enumerate().take(5) {
                    let _ = block.set(index, *stack);
                }
                let menu = match mc_container::Menu::hopper(
                    window,
                    block,
                    player_container,
                    stack_sizes,
                ) {
                    Ok(menu) => menu,
                    Err(error) => {
                        debug!(id = %id, %error, "could not build a hopper menu");
                        return;
                    }
                };
                (menu, MENU_HOPPER, "Hopper")
            }
        };

        let (contents, state, cursor, wire_window, content_window) = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return;
            };
            // Mirror the authoritative inventory into the player half before showing.
            mirror_inventory(&mut menu, &session.player.inventory);
            session.menu = menu;
            session.open_block = Some(pos);

            let contents: Vec<mc_protocol::packets::play::ItemStack> = session
                .menu
                .full_contents()
                .iter()
                .copied()
                .map(wire_stack)
                .collect();
            (
                contents,
                session.menu.state_id(),
                session.menu.cursor(),
                i32::from(session.menu.window_id()),
                i32::from(session.menu.window_id()),
            )
        };
        if let Err(error) = self.send(
            id,
            &OpenScreen {
                window_id: wire_window,
                menu_type,
                title: TextComponent::literal(title),
            },
            report,
        ) {
            debug!(id = %id, %error, "could not send open_screen");
            return;
        }
        if let Err(error) = self.send(
            id,
            &ContainerSetContent {
                window_id: content_window,
                state_id: state,
                slots: contents,
                carried: wire_stack(cursor),
            },
            report,
        ) {
            debug!(id = %id, %error, "could not send the opened window contents");
        }
    }

    /// Decode, validate and apply a `container_click`.
    ///
    /// Every step can refuse without mutating: a bad window id, an unknown click
    /// type, a slot this menu does not have, or a stale state id. A stale state id
    /// is not an error — it resynchronises the client, which is the whole point of
    /// the state id.
    #[allow(clippy::too_many_lines)]
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
        let mut outcome = match session.menu.apply_click(&click) {
            Ok(outcome) => outcome,
            Err(error) => {
                debug!(id = %id, %error, "container click refused");
                return;
            }
        };

        // P12-07: player crafting grid. The menu owns the grid/result slots but
        // knows no recipes; the server recomputes the result from its table
        // after every grid change and consumes the grid when the result is
        // taken. A stale result take (no match) resyncs instead of duplicating.
        if session.menu.window_id() == mc_container::PLAYER_WINDOW_ID {
            let is_result_take = raw.slot == 0
                && raw.click_type == mc_container::ClickType::Pickup.id()
                // A stale click applied nothing (`full_resync`): consuming the
                // grid now would lose ingredients for no result (AUDIT-12 F2).
                && !outcome.full_resync;
            let touches_crafting =
                outcome.changed_slots.iter().any(|s| (0..=4).contains(s)) || is_result_take;
            if touches_crafting {
                // Snapshot result/grid menu slots to extend the delta set below.
                let before: Vec<mc_entity::stack::ItemStack> = (0..5)
                    .map(|slot| session.menu.display_stack(slot))
                    .collect();
                let mut resync = false;
                if is_result_take {
                    // Consume one set from the grid; `None` means the result was
                    // stale (no match), so the pickup must not stand.
                    match take_craft_result(
                        &mut session.menu,
                        &self.crafting_registry,
                        &self.registries.items,
                    ) {
                        Ok(consumed) => {
                            if !consumed {
                                resync = true;
                            }
                        }
                        Err(error) => {
                            debug!(id = %id, %error, "craft consumption refused");
                            resync = true;
                        }
                    }
                }
                if !resync {
                    match mc_entity::stack::StackSizeTable::resolve(&self.registries.items) {
                        Ok(sizes) => {
                            if let Err(error) = recompute_crafting_result(
                                &mut session.menu,
                                &self.crafting_registry,
                                &self.registries.items,
                                &sizes,
                            ) {
                                debug!(id = %id, %error, "craft recompute refused");
                                resync = true;
                            }
                        }
                        Err(error) => {
                            debug!(id = %id, %error, "craft sizes refused");
                            resync = true;
                        }
                    }
                }
                if resync {
                    // Stale result: send the whole window again like a state
                    // mismatch, so the client drops the duplicated stack.
                    let contents = session.menu.full_contents();
                    let state = session.menu.state_id();
                    let wire_window = i32::from(session.menu.window_id());
                    let packet = ContainerSetContent {
                        window_id: wire_window,
                        state_id: state,
                        slots: contents.iter().copied().map(wire_stack).collect(),
                        carried: wire_stack(session.menu.cursor()),
                    };
                    if let Err(error) = self.send(id, &packet, report) {
                        debug!(id = %id, %error, "could not send a craft resync");
                    }
                    return;
                }
                for (menu_slot, was) in before.iter().enumerate() {
                    let now = session.menu.display_stack(menu_slot);
                    if now != *was {
                        outcome.changed_slots.insert(menu_slot as u16);
                    }
                }
            }
        }

        if outcome.full_resync {
            // The client's view is stale: send the whole window again. This is the
            // correction path, so it is not an error and is not rate-limited away.
            let contents = session.menu.full_contents();
            let state = session.menu.state_id();
            // The menu addresses windows as u8 (its own bound); the wire uses i8.
            let wire_window = i32::from(session.menu.window_id());
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
        let window = i32::from(session.menu.window_id());

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
                .map_or(mc_world::Vec3::default(), |session| session.player.position);
            // A thrown item appears just in front of the thrower's feet, which is
            // where Vanilla drops it from.
            for stack in &outcome.dropped {
                if let Err(error) = self.spawn_item(*stack, position) {
                    warn!(id = %id, %error, "a dropped item could not be spawned");
                }
            }
        }

        // Flush the accepted transaction back onto the authoritative inventory.
        // This is the other half of the pair: the menu was mirrored *in* at the top of
        // this function, so the two can only disagree for the duration of one click,
        // during which nothing else runs. When a block window is open its block
        // half is flushed into the block entity the same way.
        if let Some(session) = self.sessions.get_mut(&id) {
            write_back_inventory(&session.menu, &mut session.player.inventory);
        }
        if let Some(pos) = self.sessions.get(&id).and_then(|s| s.open_block) {
            // The session was present two statements up (`get_mut` wrote the
            // inventory back through it); if it vanished since, there is no
            // menu left to flush, so skipping beats the `expect` this
            // replaced — and the signature stays `()` (AGENTS.md §9).
            if let Some(session) = self.sessions.get(&id) {
                let menu = &session.menu;
                if let Some(entity) = self.block_entities.get_mut(pos) {
                    write_back_block(menu, entity);
                    mark_block_dirty(&mut self.world, pos.x, pos.z);
                }
            }
        }

        // The cursor is not part of the window payload, so a change to it needs
        // its own update: vanilla's `set_cursor_item` (clientbound 96, a single
        // optional stack — jar-verified). The old slot -1 form is not sent.
        if outcome.cursor_changed {
            let packet = mc_protocol::packets::play::SetCursorItem {
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
        let result =
            self.world
                .move_with_collision(Aabb::player(current), delta, mc_world::STEP_HEIGHT);
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
            session.player.position = mc_world::Vec3::new(applied.x, applied.y, applied.z);
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
                // Falls bypass armour (vanilla `bypasses_armor` tag), so the
                // zero set documents the bypass rather than measuring kit.
                |session| {
                    session.player.apply_damage(
                        fell_damage,
                        mc_entity::combat::DamageSource::Fall,
                        &mc_entity::combat::CombatStats::ZERO,
                    )
                },
            );
            debug!(id = %id, damage = fell_damage, health = outcome.health, "fall damage");
            self.after_damage(id, outcome);
        }
        if moved_less {
            self.correct_position(id, applied, rotation, None);
        }
    }

    /// Copy authoritative player state into the entity projection.
    pub(crate) fn project_player_entities(&mut self) {
        let updates: Vec<(EntityId, mc_world::Vec3, bool, f32, f32)> = self
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
            projection.velocity = mc_world::Vec3::ZERO;
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
        sequence: i32,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        // Before any validation: vanilla acknowledges the sequence the client sent
        // even when the action itself is refused (`handlePlayerAction` calls
        // `ackBlockChangesUpTo` on the way in). A client whose prediction is never
        // closed is stuck with a stale block on screen, and *that* is a worse
        // failure than a refused dig.
        self.note_block_change_sequence(id, sequence);
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
            ACTION_START_DESTROY_BLOCK => {
                self.start_dig(id, x, y, z, report)?;
            }
            ACTION_ABORT_DESTROY_BLOCK => {
                self.abort_dig(id, report);
            }
            ACTION_FINISH_DESTROY_BLOCK => {
                self.finish_dig(id, x, y, z, report)?;
            }
            ACTION_DROP_ITEM => {
                // The held stack leaves the inventory and becomes a dropped-item
                // entity at roughly eye height. The entity is announced by the
                // Broadcast phase with its stack metadata, and the Entities
                // phase's merge and pickup passes (P11-05/P11-09) take it from
                // there.
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

    /// Held item name for the mining tables, or `None` for an empty hand.
    ///
    /// An unresolvable id degrades to a hand rather than refusing the dig:
    /// the inventory validates every write, so reaching this means memory
    /// corruption rather than player input — and a hand digs slowly instead
    /// of erroring the tick.
    fn held_item_name(&self, id: ConnectionId) -> Option<String> {
        let held = self
            .sessions
            .get(&id)?
            .player
            .inventory
            .selected_item()
            .item_id()?;
        self.registries.items.name(held).ok().map(str::to_owned)
    }

    /// Whether the player's feet report ground contact (mid-air digging runs
    /// at a fifth of the speed).
    fn on_ground(&self, id: ConnectionId) -> bool {
        self.sessions
            .get(&id)
            .is_some_and(|session| session.player.on_ground)
    }

    /// Announce one dig overlay stage to every ready player.
    ///
    /// Stages run 0–9 with progress; -1 clears. Broadcast, not chunk-scoped
    /// (the creeper-fuse packet made the same call): at most ten packets
    /// per dig, and only on stage change.
    fn send_dig_stage(
        &self,
        entity_id: i32,
        x: i32,
        y: i32,
        z: i32,
        stage: i8,
        report: &mut TickReport,
    ) {
        let packet = BlockDestruction {
            entity_id,
            position: block_position(x, y, z),
            stage,
        };
        if let Ok(raw) = packet.to_raw() {
            self.broadcast_all(&raw, report);
        }
    }

    /// Break a block right now: air, redstone wake, drops, menu sync.
    ///
    /// Shared by creative instant breaks, zero-hardness survival breaks, and
    /// progress completions (intent or tick). `drops` is false in creative
    /// and when the harvest judgment fails — vanilla drops nothing then.
    pub(crate) fn break_block_now(
        &mut self,
        id: ConnectionId,
        target: BreakTarget,
        creative: bool,
        harvest: bool,
        report: &mut TickReport,
    ) {
        let air = self.registries.blocks.air_id();
        // `set_block` cannot fail here: the y range was checked above, and
        // an out-of-range y is its only error. Refusing to propagate keeps a
        // hostile coordinate from ending the tick (AGENTS.md section 9).
        if let Err(error) = self.world.set_block(target.x, target.y, target.z, air) {
            debug!(id = %id, x = target.x, y = target.y, z = target.z, %error, "block break was refused");
            return;
        }
        debug!(id = %id, x = target.x, y = target.y, z = target.z, "block broken");
        // P13-02: breaking a component — or a block next to one — wakes
        // the model with the *removed* id, so dust re-evaluates.
        self.redstone_feed(target.x, target.y, target.z, target.block);
        if !creative && harvest {
            // P11-04: the loot table is the drop authority in survival.
            self.spawn_block_drops(target.block, target.x, target.y, target.z, harvest);
        }
        self.sync_menu_from_inventory(id, report);
    }

    /// Open a survival dig (P16-05): instant, timed or refused by hardness.
    ///
    /// Creative breaks instantly through the shared path. A START on a new
    /// target replaces the old dig (clearing its overlay), and a repeated
    /// START restarts — vanilla's restart semantics, not progress banking.
    fn start_dig(
        &mut self,
        id: ConnectionId,
        x: i32,
        y: i32,
        z: i32,
        report: &mut TickReport,
    ) -> ServerResult<()> {
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
        let target = BreakTarget {
            x,
            y,
            z,
            block: current,
        };
        let creative = self
            .sessions
            .get(&id)
            .is_some_and(|s| s.player.game_mode.is_creative());
        if creative {
            self.break_block_now(id, target, true, true, report);
            return Ok(());
        }
        // Bedrock is refused here by value (-1.0), not by name: the old name
        // check covered one block while barriers, command blocks and end
        // portal frames dug straight through.
        let name = self.registries.blocks.block_name(current)?.to_owned();
        match mc_registry::dig_rate(
            &self.registries.blocks,
            &self.registries.items,
            self.held_item_name(id).as_deref(),
            &name,
            self.on_ground(id),
        ) {
            mc_registry::DigRate::Unbreakable => {
                debug!(id = %id, name, "refused to break unbreakable block in survival");
                return Ok(());
            }
            mc_registry::DigRate::Instant => {
                let harvest = mc_registry::can_harvest(
                    &self.registries.blocks,
                    &self.registries.items,
                    self.held_item_name(id).as_deref(),
                    &name,
                );
                self.break_block_now(id, target, false, harvest, report);
                return Ok(());
            }
            mc_registry::DigRate::PerTick(_) => {}
        }
        let replaced = self.sessions.get_mut(&id).map(|session| {
            let held = session.player.inventory.selected_item().item_id();
            session.dig.replace(DigState {
                x,
                y,
                z,
                block: current,
                progress: 0.0,
                stage: -1,
                held,
            })
        });
        if let Some(Some(old)) = replaced
            && old.stage >= 0
            && let Some(session) = self.sessions.get(&id)
        {
            self.send_dig_stage(session.entity.get(), old.x, old.y, old.z, -1, report);
        }
        Ok(())
    }

    /// Drop a dig: progress dies, and an announced overlay is cleared.
    ///
    /// No block, no drops, no menu sync — the abort half of P16-05.
    fn abort_dig(&mut self, id: ConnectionId, report: &mut TickReport) {
        let cleared = self.sessions.get_mut(&id).map(|session| session.dig.take());
        if let Some(Some(dig)) = cleared
            && dig.stage >= 0
            && let Some(session) = self.sessions.get(&id)
        {
            self.send_dig_stage(session.entity.get(), dig.x, dig.y, dig.z, -1, report);
        }
    }

    /// Finish a dig: break now only when the server agrees the progress is
    /// complete (P16-05).
    ///
    /// Anything else is a prediction the sequence ack already closed —
    /// ignoring it is what makes a too-early FINISH harmless instead of a
    /// duplicate break, and what makes START+FINISH spam unable to skip the
    /// accumulation (each START restarts it). The kept dig still completes
    /// on the server's own ticks, absorbing the client's one-tick lead.
    fn finish_dig(
        &mut self,
        id: ConnectionId,
        x: i32,
        y: i32,
        z: i32,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let ready = self
            .sessions
            .get(&id)
            .and_then(|session| session.dig)
            .is_some_and(|dig| dig.x == x && dig.y == y && dig.z == z && dig.progress >= 1.0);
        if !ready {
            return Ok(());
        }
        let Some(current) = self.world.get_block_loaded(x, y, z) else {
            return Ok(());
        };
        if self.registries.blocks.is_empty(current) {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.dig = None;
            }
            return Ok(());
        }
        let name = self.registries.blocks.block_name(current)?.to_owned();
        let harvest = mc_registry::can_harvest(
            &self.registries.blocks,
            &self.registries.items,
            self.held_item_name(id).as_deref(),
            &name,
        );
        if let Some(session) = self.sessions.get_mut(&id) {
            session.dig = None;
        }
        self.break_block_now(
            id,
            BreakTarget {
                x,
                y,
                z,
                block: current,
            },
            false,
            harvest,
            report,
        );
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

    /// Right-click a block: open a container when the target is one, otherwise
    /// place the held block item if the target is legal.
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
        // Containers open on right-click before any placement: a chest does
        // something even with an empty hand, and holding a placeable block
        // must not place *through* a chest (vanilla opens unless sneaking,
        // and sneaking is not modelled — opening wins).
        if self
            .clicked_block_name(x, y, z)
            .is_some_and(|n| is_container_block(&n))
        {
            self.open_container(id, x, y, z, report);
            return;
        }
        // Levers flip on right-click (P13-02): the toggle *is* the power source
        // changing state, so it feeds the model like a placement. Before the
        // held-item check so an empty hand flips and a held block does not
        // place through the lever.
        if self.clicked_block_name(x, y, z).as_deref() == Some("minecraft:lever") {
            self.flip_lever(id, x, y, z);
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
        // P13-02: a placed wire, torch, lever or neighbour of one wakes the model.
        self.redstone_feed(tx, ty, tz, block_id);
        let survival = self
            .sessions
            .get(&id)
            .is_some_and(|s| s.player.game_mode == GameMode::Survival);
        if survival {
            {
                let Some(session) = self.sessions.get_mut(&id) else {
                    return;
                };
                // Consume in place: `take` + `shrink` + write back to the
                // *same* slot. Routing the remainder through `add_stack`
                // merged it into the lowest partial stack first, so placing
                // from a full held stack emptied the held slot and grew an
                // earlier one — the client's hotbar visibly rearranged on
                // every placement (P14-09 walk).
                let mut stack = session.player.inventory.take_held(hand);
                stack.shrink(1);
                let _ = session.player.inventory.replace_held(hand, stack);
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
        let (game_mode, dropped, death_location) = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return Ok(());
            };
            // `keep_inventory` is a game rule we do not model yet; Vanilla's default
            // is `false`, so items are dropped. The stacks are returned to the
            // caller and discarded here: they are dropped at the *death* position,
            // which this path no longer knows, and item entities for a death drop
            // land with the rest of P05-15. Stated, not implied.
            let dropped = session.player.respawn(false);
            session.player.position =
                mc_world::Vec3::new(f64::from(sx) + 0.5, f64::from(sy), f64::from(sz) + 0.5);
            session.tick_start_y = f64::from(sy);
            session.sent_chunks.clear();
            (
                session.player.game_mode.id(),
                dropped.len(),
                session.last_death_location.clone(),
            )
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
                death_location,
                // No respawn anchors yet: the cooldown field exists, and 0 is the
                // truthful value for a player who has never charged one.
                portal_cooldown: 0,
                sea_level: 63,
                data_kept: 0,
            },
            report,
        )?;
        // KD-50, second half: the join path sends this and respawn did not.
        // The client's loading screen dismisses on `LevelLoadTracker`
        // becoming ready, which only the start-chunks game event drives — a
        // respawned client sat on "Loading terrain" forever with every packet
        // well-formed and no error anywhere. Vanilla shows the same dirt
        // screen momentarily on respawn, so this matches rather than invents.
        self.send(
            id,
            &GameEvent {
                event: GAME_EVENT_LEVEL_CHUNKS_LOAD_START,
                value: 0.0,
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

    /// Whether a block position is within the player's reach.
    ///
    /// The rule is vanilla's, bytecode-read from the Mojang-mapped 26.1.2 server
    /// jar rather than adapted:
    ///
    /// ```text
    /// Player.isWithinBlockInteractionRange(BlockPos pos, double buffer):
    ///     double range = this.blockInteractionRange() + buffer;   // attribute + buffer
    ///     AABB box = new AABB(pos);                               // the block's unit cube
    ///     return box.distanceToSqr(this.getEyePosition()) < range * range;   // STRICT <
    /// ServerPlayerGameMode.handleBlockBreakAction:  isWithinBlockInteractionRange(pos, 1.0)
    /// ServerGamePacketListenerImpl.handleUseItemOn: isWithinBlockInteractionRange(pos, 1.0)
    /// ```
    ///
    /// Three details are load-bearing and each was wrong or absent before AUDIT-11:
    ///
    /// * **the buffer.** The check is `range + 1.0`, so a survival player reaches
    ///   5.5 blocks, not the 4.5 the attribute alone names. Measuring from the
    ///   *closest point of the block's box* (which is what `AABB.distanceToSqr`
    ///   does, and what this method did already) is correct — the box, not the
    ///   centre, is what vanilla uses;
    /// * **strict `<`**, so exactly at the boundary is refused;
    /// * **the creative modifier**, `+0.5` (`creative_mode_block_range`), giving a
    ///   creative player 6.0.
    ///
    /// What this is *for*: without it any client can break any block anywhere, which
    /// is what AUDIT-11's N-1 was about. The check has been here since Phase 04 —
    /// AUDIT-11's claim that the path had none was refuted by reading this call site
    /// and the 40-block refusal test — but it refused the 4.5-to-5.5 band a real
    /// client is entitled to, so the landings' own acceptance rounds could hit it.
    fn within_reach(&self, id: ConnectionId, x: i32, y: i32, z: i32) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        let eye = Vec3::new(
            session.player.position.x,
            session.player.position.y + EYE_HEIGHT,
            session.player.position.z,
        );
        let reach = block_reach(session.player.game_mode.is_creative());
        Aabb::block(x, y, z).distance_to_sqr(eye) < reach * reach
    }

    /// Whether an entity is within the player's entity reach.
    ///
    /// [`Player.isWithinEntityInteractionRange(AABB, double)`] with the buffer
    /// `ServerGamePacketListenerImpl.handleInteract` passes (`ldc2_w 3.0d`):
    /// `AABB(entity).distanceToSqr(eye) < (3.0 + 3.0)^2`. The check sits **before**
    /// the packet's action is branched on in vanilla, so it covers a swing and a
    /// use alike; this build only acts on the swing, and applies the same gate.
    ///
    /// This closed a recorded open divergence — "a swing outside the interaction
    /// range is not refused" — which was the same hostile-client class as N-1's
    /// concern: the attack path applied damage at any distance at all.
    fn within_entity_reach(&self, id: ConnectionId, target: EntityId) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        let Some(entity) = self.entities.get(target) else {
            return false;
        };
        let eye = Vec3::new(
            session.player.position.x,
            session.player.position.y + EYE_HEIGHT,
            session.player.position.z,
        );
        // The held item's `attack_range` component extends this gate once
        // components land (P18); until then the hook reports zero for every
        // item and the gate is byte-for-byte today's.
        let held = session.player.inventory.selected_item();
        let bonus = held
            .item_id()
            .and_then(|held_id| self.registries.items.name(held_id).ok())
            .map_or(0.0, mc_entity::combat::attack_range_bonus);
        let reach = entity_reach() + bonus;
        entity.hitbox().distance_to_sqr(eye) < reach * reach
    }

    // ------------------------------------------------------------- streaming
}
