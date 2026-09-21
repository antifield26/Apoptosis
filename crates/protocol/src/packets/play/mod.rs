//! Play-state packets (Phase-02 plumbing subset plus the Phase-04 world codecs).
//!
//! ## Where these wire shapes come from
//!
//! Packet **ids** are not transcribed here: every `impl Packet` names a
//! constant from [`crate::ids`], whose values are machine-extracted from the
//! official 26.1.2 server jar's `ProtocolInfoBuilder` registration order
//! (`docs/protocol/packet-ids-775.tsv`, asserted by
//! `crates/protocol/tests/packet_ids.rs`).
//!
//! **Field order** for the Phase-04 packets below (chunk, block, entity,
//! inventory, respawn, chat) was verified against the official 26.1.2 server
//! jar in this project; the shapes are reproduced field by field in the doc
//! comment of each packet. Every one of them is covered by an encode → decode →
//! compare round trip plus a hostile-input test in the module test block, so a
//! drift in either direction fails the build rather than reaching the wire.
//!
//! Two caveats are recorded rather than implied away (AGENTS.md section 3.3):
//!
//! - there is no real 26.1.2 client in this environment
//!   (`docs/protocol/26.1.2-wire-notes.md` section 7), so "the client accepts
//!   it" is *not* what these tests establish — only that our encoder and
//!   decoder agree and that malformed input is refused;
//! - item **data component** payloads and entity metadata values beyond
//!   [`MetadataValue`] are unmodelled. Both are carried as opaque
//!   pre-encoded bytes/types and are rejected with an explicit error instead of
//!   being guessed at.

use super::Packet;
use crate::ids::{clientbound, serverbound};
use crate::nbt::Nbt;
use crate::text::TextComponent;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

/// Read a `VarInt` count and reject it unless it is inside `0..=max`.
///
/// Every counted wire list goes through here, so a hostile count can never be
/// turned into `Vec::with_capacity` before it has been range-checked.
pub(crate) fn read_count(
    reader: &mut PacketReader<'_>,
    what: &str,
    max: usize,
) -> ServerResult<usize> {
    let raw = reader.read_varint()?;
    if raw < 0 || raw as usize > max {
        return Err(ServerError::Protocol(format!(
            "{what} count {raw} is outside 0..={max}"
        )));
    }
    Ok(raw as usize)
}

/// Convert a server-authored collection length into the wire `VarInt`.
///
/// # Errors
///
/// [`ServerError::Invariant`] when the length exceeds `i32::MAX`.
pub(crate) fn packed_len(len: usize) -> ServerResult<i32> {
    i32::try_from(len)
        .map_err(|_| ServerError::Invariant(format!("length {len} does not fit a VarInt")))
}

/// Re-layer a [`mc_core::packing`] failure as malformed wire input.
///
/// The packing module reports `CorruptData` because its primary caller reads
/// disk chunks; the same condition arriving here came off the network, and
/// `ServerError::Protocol` is what the connection layer acts on (AGENTS.md
/// section 9).
fn packing_error(error: ServerError) -> ServerError {
    match error {
        ServerError::CorruptData(message) => ServerError::Protocol(message),
        other => other,
    }
}

mod chunk;
mod inventory;
mod light;
mod position;

pub use self::chunk::{
    BIOMES_PER_SECTION, BLOCKS_PER_SECTION, BlockChangedAck, BlockDestruction, BlockUpdate,
    ChunkBatchFinished, ChunkBatchStart, ChunkSection, HEIGHTMAP_MOTION_BLOCKING,
    HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES, HEIGHTMAP_OCEAN_FLOOR, HEIGHTMAP_OCEAN_FLOOR_WG,
    HEIGHTMAP_WORLD_SURFACE, HEIGHTMAP_WORLD_SURFACE_WG, Heightmap, LevelChunkWithLight,
    MAX_BLOCK_ENTITIES, MAX_CHUNK_SECTIONS, MAX_HEIGHTMAP_LONGS, MAX_HEIGHTMAPS, MAX_PALETTE_LEN,
    MAX_SECTION_UPDATES, NETWORK_BIOME_MIN_BITS, OVERWORLD_SECTIONS, PalettedContainer,
    SectionBlocksUpdate,
};
pub use self::inventory::{
    ContainerClose, ContainerSetContent, ContainerSetData, ContainerSetSlot, ItemStack,
    MAX_CONTAINER_SLOTS, MAX_ITEM_COMPONENTS, MAX_METADATA_ENTRIES, MENU_FURNACE, MENU_GENERIC_9X3,
    MENU_GENERIC_9X6, MENU_HOPPER, METADATA_INDEX_CREEPER_FUSE, METADATA_INDEX_HEALTH,
    METADATA_INDEX_ORB_VALUE, METADATA_TERMINATOR, METADATA_TYPE_BYTE, METADATA_TYPE_FLOAT,
    METADATA_TYPE_INT, METADATA_TYPE_ITEM_STACK, METADATA_TYPE_VARIANTS, METADATA_TYPE_VARINT,
    MOB_EFFECT_FLAG_AMBIENT, MOB_EFFECT_FLAG_ICON, MOB_EFFECT_FLAG_PARTICLES, MetadataValue,
    OpenScreen, RemoveMobEffect, SetCursorItem, SetEntityData, UpdateMobEffect,
};
pub use self::light::{
    LIGHT_ARRAY_BYTES, LightData, LightUpdate, MAX_LIGHT_SECTIONS, read_light_data,
    write_light_data,
};
pub use self::position::{
    MoveEntityPos, MoveEntityPosRot, MoveEntityRot, PlayerPosition, block_position,
    unpack_block_position,
};

// ---------------------------------------------------------------------------
// Shared validation helpers
// ---------------------------------------------------------------------------

/// `minecraft:login` / `JoinGame` (clientbound 49).
///
/// Field order verified against the 26.1 reference writer
/// (`pumpkin-protocol/src/java/client/play/login.rs:218-378`, version gates for
/// `>= 1.26.2` excluded because protocol 775 predates them).
#[allow(clippy::struct_excessive_bools)] // The vanilla packet really is this boolean-heavy.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinGame {
    /// Entity id assigned to the player.
    pub entity_id: i32,
    /// Hardcore flag.
    pub hardcore: bool,
    /// All dimension names known to the server.
    pub dimension_names: Vec<String>,
    /// Advertised player slot count.
    pub max_players: i32,
    /// Chunk view distance.
    pub view_distance: i32,
    /// Entity simulation distance.
    pub simulation_distance: i32,
    /// Hide F3 debug details.
    pub reduced_debug_info: bool,
    /// Show the respawn screen on death.
    pub enable_respawn_screen: bool,
    /// Limited crafting (recipe book gating).
    pub limited_crafting: bool,
    /// Index into the synced `minecraft:dimension_type` registry.
    pub dimension_type_id: i32,
    /// Key of the dimension the player spawns in.
    pub dimension_name: String,
    /// Hashed world seed shown to the client.
    pub hashed_seed: i64,
    /// Game mode id (0 survival, 1 creative, ...).
    pub game_mode: u8,
    /// Previous game mode id (-1 for none).
    pub previous_game_mode: i8,
    /// Debug world flag.
    pub is_debug: bool,
    /// Flat world flag.
    pub is_flat: bool,
    /// Last death location (dimension, packed block position).
    pub death_location: Option<(String, i64)>,
    /// Respawn-anchor cooldown in ticks.
    pub portal_cooldown: i32,
    /// Sea level used for rendering.
    pub sea_level: i32,
    /// Enforce signed chat.
    pub enforce_secure_chat: bool,
}

impl Packet for JoinGame {
    const ID: i32 = clientbound::play::LOGIN;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let entity_id = reader.read_i32()?;
        let hardcore = reader.read_bool()?;
        let dimension_count = reader.read_varint()?;
        if !(0..=64).contains(&dimension_count) {
            return Err(ServerError::Protocol(format!(
                "dimension count out of range: {dimension_count}"
            )));
        }
        let mut dimension_names = Vec::with_capacity(dimension_count as usize);
        for _ in 0..dimension_count {
            dimension_names.push(reader.read_string(crate::MAX_IDENTIFIER_LEN)?);
        }
        let max_players = reader.read_varint()?;
        let view_distance = reader.read_varint()?;
        let simulation_distance = reader.read_varint()?;
        let reduced_debug_info = reader.read_bool()?;
        let enable_respawn_screen = reader.read_bool()?;
        let limited_crafting = reader.read_bool()?;
        let dimension_type_id = reader.read_varint()?;
        let dimension_name = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
        let hashed_seed = reader.read_i64()?;
        let game_mode = reader.read_u8()?;
        let previous_game_mode = reader.read_i8()?;
        let is_debug = reader.read_bool()?;
        let is_flat = reader.read_bool()?;
        let death_location = if reader.read_bool()? {
            let dimension = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
            let position = reader.read_i64()?;
            Some((dimension, position))
        } else {
            None
        };
        let portal_cooldown = reader.read_varint()?;
        let sea_level = reader.read_varint()?;
        let enforce_secure_chat = reader.read_bool()?;
        Ok(Self {
            entity_id,
            hardcore,
            dimension_names,
            max_players,
            view_distance,
            simulation_distance,
            reduced_debug_info,
            enable_respawn_screen,
            limited_crafting,
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            enforce_secure_chat,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i32(self.entity_id);
        writer.write_bool(self.hardcore);
        writer.write_varint(i32::try_from(self.dimension_names.len()).unwrap_or(i32::MAX));
        for name in &self.dimension_names {
            writer.write_string(name)?;
        }
        writer.write_varint(self.max_players);
        writer.write_varint(self.view_distance);
        writer.write_varint(self.simulation_distance);
        writer.write_bool(self.reduced_debug_info);
        writer.write_bool(self.enable_respawn_screen);
        writer.write_bool(self.limited_crafting);
        writer.write_varint(self.dimension_type_id);
        writer.write_string(&self.dimension_name)?;
        writer.write_i64(self.hashed_seed);
        writer.write_u8(self.game_mode);
        writer.write_i8(self.previous_game_mode);
        writer.write_bool(self.is_debug);
        writer.write_bool(self.is_flat);
        match &self.death_location {
            Some((dimension, position)) => {
                writer.write_bool(true);
                writer.write_string(dimension)?;
                writer.write_i64(*position);
            }
            None => writer.write_bool(false),
        }
        writer.write_varint(self.portal_cooldown);
        writer.write_varint(self.sea_level);
        writer.write_bool(self.enforce_secure_chat);
        Ok(writer.finish())
    }
}

/// `minecraft:keep_alive` (clientbound 44 / serverbound 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAlive {
    /// Opaque id echoed by the peer.
    pub id: i64,
}

impl Packet for KeepAlive {
    const ID: i32 = clientbound::play::KEEP_ALIVE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            id: PacketReader::new(payload).read_i64()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.id);
        Ok(writer.finish())
    }
}

/// `minecraft:disconnect` (play clientbound 32). NBT text component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayDisconnect {
    /// Reason shown to the client.
    pub reason: TextComponent,
}

impl Packet for PlayDisconnect {
    const ID: i32 = clientbound::play::DISCONNECT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut rest = payload;
        let nbt = Nbt::read_network(&mut rest)?;
        Ok(Self {
            reason: super::config::text_from_nbt(&nbt)?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        let mut encoded = Vec::new();
        self.reason.to_nbt().write_network(&mut encoded)?;
        writer.write_bytes(&encoded);
        Ok(writer.finish())
    }
}

/// `minecraft:set_chunk_cache_center` (clientbound 94).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetChunkCacheCenter {
    /// Chunk x.
    pub x: i32,
    /// Chunk z.
    pub z: i32,
}

impl Packet for SetChunkCacheCenter {
    const ID: i32 = clientbound::play::SET_CHUNK_CACHE_CENTER;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        Ok(Self {
            x: reader.read_varint()?,
            z: reader.read_varint()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.x);
        writer.write_varint(self.z);
        Ok(writer.finish())
    }
}

/// `minecraft:set_chunk_cache_radius` (clientbound 95).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetChunkCacheRadius {
    /// View distance in chunks.
    pub radius: i32,
}
impl Packet for SetChunkCacheRadius {
    const ID: i32 = clientbound::play::SET_CHUNK_CACHE_RADIUS;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            radius: PacketReader::new(payload).read_varint()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.radius);
        Ok(writer.finish())
    }
}

/// `minecraft:forget_level_chunk` (clientbound 37).
///
/// One packed long, big-endian: the chunk x in the low 32 bits, z in the
/// high 32 (`ChunkPos.pack`, bytecode-read from the 26.1.2 jar, P14-04).
/// Tells the client to drop a chunk the server unloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetLevelChunk {
    /// Chunk x.
    pub x: i32,
    /// Chunk z.
    pub z: i32,
}

impl ForgetLevelChunk {
    /// Pack chunk coordinates the way the jar's `ChunkPos.pack` does.
    #[must_use]
    pub const fn pack(x: i32, z: i32) -> i64 {
        ((x as u64 & 0xFFFF_FFFF) | ((z as u64 & 0xFFFF_FFFF) << 32)) as i64
    }
}

impl Packet for ForgetLevelChunk {
    const ID: i32 = clientbound::play::FORGET_LEVEL_CHUNK;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packed = reader.read_i64()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "forget_level_chunk has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            x: packed as i32,
            z: (packed >> 32) as i32,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(Self::pack(self.x, self.z));
        Ok(writer.finish())
    }
}

/// `minecraft:configuration_acknowledged` (serverbound 16). Empty payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConfigurationAcknowledged;

impl Packet for ConfigurationAcknowledged {
    const ID: i32 = serverbound::play::CONFIGURATION_ACKNOWLEDGED;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(ServerError::Protocol(
                "configuration acknowledged must be empty".to_owned(),
            ))
        }
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// `minecraft:start_configuration` (clientbound 118). Empty payload.
///
/// Sent when the server moves a play connection back into the configuration
/// state (Play → Configuration re-entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartConfiguration;

impl Packet for StartConfiguration {
    const ID: i32 = clientbound::play::START_CONFIGURATION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(ServerError::Protocol(
                "start configuration must be empty".to_owned(),
            ))
        }
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        Ok(PacketWriter::new().finish())
    }
}

/// `minecraft:ping_request` (serverbound play 38).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayPingRequest {
    /// Payload echoed back by the server.
    pub id: i32,
}

impl Packet for PlayPingRequest {
    const ID: i32 = serverbound::play::PING_REQUEST;
    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            id: PacketReader::new(payload).read_i32()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i32(self.id);
        Ok(writer.finish())
    }
}

/// `minecraft:pong_response` (clientbound 62).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayPong {
    /// Payload echoed from [`PlayPingRequest`].
    pub id: i32,
}

impl Packet for PlayPong {
    const ID: i32 = clientbound::play::PONG_RESPONSE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        Ok(Self {
            id: PacketReader::new(payload).read_i32()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i32(self.id);
        Ok(writer.finish())
    }
}

/// `minecraft:add_entity` — spawn an entity, carrying its registry type id.
///
/// # The type id is a claim about a registry the client owns
///
/// `entity_type` is a **built-in** registry, compiled into the client jar, not a datapack registry this server
/// sends. So the id comes from `crates/test-support/fixtures/registry/entity_types.tsv` — extracted from the jar
/// by `tools/vanilla-probe/EntityTypeProbe.java` — and **not** from the config payload, where
/// `minecraft:entity_type` is a tag directory in `update_tags`. See P10-06 for why the two are not
/// interchangeable.
///
/// # Layout, and how it was checked
///
/// A `VarInt` entity id, a 16-byte UUID, a `VarInt` type id, three `f64` coordinates, three `i8` angles (in 1/256 of
/// a degree), a `VarInt` data field and three `i16` velocities (in 1/8000 of a block per tick). **Those widths sum
/// to 52 bytes**, and the first `add_entity` body a real 26.1.2 server sent through the capture rig is exactly
/// 52 bytes: `crates/test-support/fixtures/protocol/add_entity_slime.hex`, whose type id is 117, which the table
/// names `minecraft:slime`. The movement field rides vanilla's `LpVec3` — see [`write_lp_vec3`].
#[derive(Debug, Clone, PartialEq)]
pub struct AddEntity {
    /// Entity id, unique within the connection.
    pub entity_id: i32,
    /// The entity's UUID.
    pub uuid: uuid::Uuid,
    /// Entity **type** registry id, from the client's built-in registry.
    pub type_id: i32,
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate.
    pub z: f64,
    /// Pitch, in 1/256 of a degree.
    pub pitch: i8,
    /// Yaw, in 1/256 of a degree.
    pub yaw: i8,
    /// Head yaw, in 1/256 of a degree.
    pub head_yaw: i8,
    /// Object data: the variant fields of a non-living entity, `0` for a living one.
    pub data: i32,
    /// The entity's movement, encoded at vanilla's lower precision — a real
    /// 26.1.2 client **refused our packet when this carried the old velocity
    /// triple**, "found 5 bytes extra".
    pub movement: (f64, f64, f64),
}

/// Vanilla's `LpVec3`: a 26.x movement vector at "lower precision".
///
/// Bytecode-read from the client jar (`net.minecraft.network.LpVec3`, per the
/// `Vec3.LP_STREAM_CODEC` bootstrap): a stationary vector is **one byte** of
/// zero; anything else writes scale bits, three 15-bit quantized deltas and a
/// `VarInt` magnitude tail. The rounding, the threshold and the bit layout are
/// the bytecode's own; a plausible-looking approximation here would be another
/// wire rejection.
///
/// # Errors
///
/// [`ServerError::Protocol`] never for these field types; the signature
/// matches the neighbouring writers.
pub fn write_lp_vec3(writer: &mut PacketWriter, movement: &(f64, f64, f64)) -> ServerResult<()> {
    // `sanitize`: NaN becomes zero; everything else is clamped into the range
    // the 44-bit packing can represent.
    let sanitize = |value: f64| {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(-1.717_986_918_3e10, 1.717_986_918_3e10)
        }
    };
    let (x, y, z) = (
        sanitize(movement.0),
        sanitize(movement.1),
        sanitize(movement.2),
    );
    let abs_max = x.abs().max(y.abs()).max(z.abs());
    // The stationary threshold: `2^-15`, exactly the bytecode's
    // `3.051944088384301E-5`.
    if abs_max < 3.051_944_088_384_301e-5 {
        writer.write_u8(0);
        return Ok(());
    }
    let magnitude = abs_max.ceil() as i64;
    // The two low bits carry `(scale & 3)`; the rest rides the `VarInt` when
    // nonzero. `has_extra` is *not* `(magnitude & 3) != 0`: a magnitude of 1..3
    // fits the low bits with nothing left for the tail, and writing an empty
    // `0x00` tail emits bytes vanilla never sends (6258-body capture sweep,
    // P15-07 A-03). Worse, a multiple of 4 with no tail sets bit 2 while the
    // tail is absent, so a decoder reads the pitch as a VarInt.
    let low_bits = magnitude & 3;
    let has_extra = (magnitude >> 2) != 0;
    let low = if has_extra { low_bits | 4 } else { low_bits };
    // `pack`: 15 bits of `[-1, 1]` as `round((d * 0.5 + 0.5) * 32766)`.
    let pack = |value: f64| ((value * 0.5 + 0.5) * 32_766.0).round() as i64;
    let combined = low
        | (pack(x / abs_max.ceil()) << 3)
        | (pack(y / abs_max.ceil()) << 18)
        | (pack(z / abs_max.ceil()) << 33);
    writer.write_u8((combined & 0xFF) as u8);
    writer.write_u8(((combined >> 8) & 0xFF) as u8);
    // The low 32 bits of the 48-bit composition, as a bit pattern: the top
    // bit is set for magnitudes whose scale reaches into it, so the signed
    // conversion must preserve the pattern rather than the value.
    let high = ((combined >> 16) & 0xFFFF_FFFF) as u32 as i32;
    writer.write_i32(high);
    if has_extra {
        writer.write_varint(i32::try_from((magnitude >> 2) & 0x3FFF_FFFF).unwrap_or(0));
    }
    Ok(())
}

/// The read half of [`write_lp_vec3`], for tests that decode a captured body
/// and re-encode it.
///
/// # Errors
///
/// [`ServerError::Protocol`] when the input ends early, a `VarInt` is
/// malformed, or a nonzero movement carries a zero magnitude.
#[allow(
    clippy::cast_precision_loss // 15-bit values into f64; the loss is unreachable
)]
pub fn read_lp_vec3(reader: &mut PacketReader<'_>) -> ServerResult<(f64, f64, f64)> {
    let unpack = |packed: i64| {
        let value = packed & 0x7FFF;
        let value = value.min(32_766);
        value as f64 * 2.0 / 32_766.0 - 1.0
    };
    let low = reader.read_u8()?;
    if low == 0 {
        return Ok((0.0, 0.0, 0.0));
    }
    let next = reader.read_u8()?;
    let high = reader.read_i32()?;
    let combined =
        i64::from(low) | (i64::from(next) << 8) | ((i64::from(high) & 0xFFFF_FFFF) << 16);
    let magnitude_low = combined & 7;
    let scale = if magnitude_low & 4 != 0 {
        let rest = i64::from(reader.read_varint()?);
        ((rest << 2) & 0x3FFF_FFFC) | (magnitude_low & 3)
    } else {
        magnitude_low & 3
    };
    if scale == 0 {
        return Err(ServerError::Protocol(
            "lp_vec3: a nonzero movement carries a zero scale".to_owned(),
        ));
    }
    #[allow(clippy::cast_precision_loss)]
    let scale_f = scale as f64;
    let x = unpack((combined >> 3) & 0x7FFF) * scale_f;
    let y = unpack((combined >> 18) & 0x7FFF) * scale_f;
    let z = unpack((combined >> 33) & 0x7FFF) * scale_f;
    Ok((x, y, z))
}

impl AddEntity {
    /// Encode the body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when a field cannot be written, which for these types means never; the
    /// signature matches its neighbours rather than being infallible in isolation.
    /// Write the body into an existing writer, for the [`Packet`] impl and for tests that frame it
    /// themselves.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when a field cannot be written, which for these types means never.
    pub fn encode_into(&self, writer: &mut PacketWriter) -> ServerResult<()> {
        writer.write_varint(self.entity_id);
        writer.write_uuid(&self.uuid);
        writer.write_varint(self.type_id);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        // The movement rides vanilla's `LpVec3`, between the position and the
        // rotations — exactly the bytecode's order.
        write_lp_vec3(writer, &self.movement)?;
        writer.write_i8(self.pitch);
        writer.write_i8(self.yaw);
        writer.write_i8(self.head_yaw);
        writer.write_varint(self.data);
        Ok(())
    }

    /// Decode a body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the input ends early or a `VarInt` is malformed or too long.
    pub fn decode(reader: &mut PacketReader<'_>) -> ServerResult<Self> {
        let entity_id = reader.read_varint()?;
        let uuid = reader.read_uuid()?;
        let type_id = reader.read_varint()?;
        let x = reader.read_f64()?;
        let y = reader.read_f64()?;
        let z = reader.read_f64()?;
        let movement = read_lp_vec3(reader)?;
        Ok(Self {
            entity_id,
            uuid,
            type_id,
            x,
            y,
            z,
            movement,
            pitch: reader.read_i8()?,
            yaw: reader.read_i8()?,
            head_yaw: reader.read_i8()?,
            data: reader.read_varint()?,
        })
    }
}

/// `minecraft:remove_entities` — despawn entities by id.
///
/// # Layout, and how it was checked
///
/// A `VarInt` count followed by that many `VarInt` entity ids. **That is one byte of count plus one byte per id
/// for a single removal**, and all **twelve** `remove_entities` bodies a real 26.1.2 server sent through the
/// capture rig are exactly **two bytes**:
///
/// ```text
/// 001788_s2c_play_77.bin: 01 0f   -> count 1, entity id 15
/// 002890_s2c_play_77.bin: 01 4d   -> count 1, entity id 77
/// 003812_s2c_play_77.bin: 01 36   -> count 1, entity id 54
/// ```
///
/// The same arithmetic check `AddEntity` got, on a shorter packet: the widths the shape implies and the lengths a
/// real server produced agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveEntities {
    /// Entity ids to despawn.
    pub entity_ids: Vec<i32>,
}

impl Packet for AddEntity {
    const ID: i32 = crate::ids::clientbound::play::ADD_ENTITY;

    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the body is malformed or has trailing bytes.
    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let decoded = Self::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "add_entity has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(decoded)
    }

    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the body cannot be encoded, which for these field types means never.
    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        self.encode_into(&mut writer)?;
        Ok(writer.finish())
    }
}

impl RemoveEntities {
    /// Encode the body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the count does not fit a `VarInt`, which needs more than two billion ids.
    /// Write the body into an existing writer, for the [`Packet`] impl.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the count does not fit a `VarInt`.
    pub fn encode_into(&self, writer: &mut PacketWriter) -> ServerResult<()> {
        let count = i32::try_from(self.entity_ids.len()).map_err(|_| {
            ServerError::Protocol("remove_entities carries more ids than a VarInt count".to_owned())
        })?;
        writer.write_varint(count);
        for id in &self.entity_ids {
            writer.write_varint(*id);
        }
        Ok(())
    }

    /// Decode a body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the count is negative, or claims more ids than the body holds. **A hostile
    /// count is refused rather than reserved**: a body that says four billion ids and carries one must not make
    /// the decoder allocate for four billion.
    pub fn decode(reader: &mut PacketReader<'_>) -> ServerResult<Self> {
        let count = reader.read_varint()?;
        let count = usize::try_from(count).map_err(|_| {
            ServerError::Protocol(format!("remove_entities count {count} is negative"))
        })?;
        let mut entity_ids = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            entity_ids.push(reader.read_varint()?);
        }
        Ok(Self { entity_ids })
    }
}

/// `minecraft:set_entity_motion` — set an entity's velocity.
///
/// # Layout, and how it was checked
///
/// A `VarInt` entity id followed by three `i16` velocities, **big-endian** as every multi-byte field in this
/// protocol is, in **1/8000 of a block per tick**. That is
/// `1 + 3 * 2 = 7` bytes, and **all 2942** `set_entity_motion` bodies a real 26.1.2 server sent through the
/// capture rig are exactly seven bytes:
///
/// ```text
/// 000143: 4e 49 f9 7c 92 eb ed   -> id 78, velocities 18937, 31890, -5139  -> 2.37, 3.99, -0.64
/// 000214: 43 e9 0e 78 54 ec ae   -> id 67, velocities -5874, 30804, -4946 -> -0.73, 3.85, -0.62
/// ```
///
/// **The decoded values are plausible velocities**, which a length check cannot establish on its own: an `i16`
/// read at the wrong offset is still an `i16`, and would not give three small numbers that look like something
/// walking around a world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetEntityMotion {
    /// Entity id.
    pub entity_id: i32,
    /// Velocity X, in 1/8000 of a block per tick.
    pub velocity_x: i16,
    /// Velocity Y, in 1/8000 of a block per tick.
    pub velocity_y: i16,
    /// Velocity Z, in 1/8000 of a block per tick.
    pub velocity_z: i16,
}

impl SetEntityMotion {
    /// Encode the body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when a field cannot be written, which for these types means never; the
    /// signature matches its neighbours rather than being infallible in isolation.
    pub fn encode(&self, writer: &mut PacketWriter) -> ServerResult<()> {
        writer.write_varint(self.entity_id);
        writer.write_i16(self.velocity_x);
        writer.write_i16(self.velocity_y);
        writer.write_i16(self.velocity_z);
        Ok(())
    }

    /// Decode a body.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the input ends early or the `VarInt` is malformed or too long.
    pub fn decode(reader: &mut PacketReader<'_>) -> ServerResult<Self> {
        Ok(Self {
            entity_id: reader.read_varint()?,
            velocity_x: reader.read_i16()?,
            velocity_y: reader.read_i16()?,
            velocity_z: reader.read_i16()?,
        })
    }

    /// The velocity in blocks per tick, which is what a caller sets rather than the wire's 1/8000ths.
    #[must_use]
    pub fn velocity(&self) -> (f64, f64, f64) {
        const PER_BLOCK: f64 = 8000.0;
        (
            f64::from(self.velocity_x) / PER_BLOCK,
            f64::from(self.velocity_y) / PER_BLOCK,
            f64::from(self.velocity_z) / PER_BLOCK,
        )
    }

    /// Build from a velocity in blocks per tick, rounding to the wire's resolution.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when a component is not finite or would not fit an `i16` at this
    /// resolution, which is a caller passing a teleport rather than a velocity.
    pub fn from_velocity(entity_id: i32, x: f64, y: f64, z: f64) -> ServerResult<Self> {
        fn to_wire(axis: &str, value: f64) -> ServerResult<i16> {
            if !value.is_finite() {
                return Err(ServerError::InvalidAction(format!(
                    "velocity {axis} is {value}, which is not a number"
                )));
            }
            let scaled = (value * 8000.0).round();
            if scaled < f64::from(i16::MIN) || scaled > f64::from(i16::MAX) {
                return Err(ServerError::InvalidAction(format!(
                    "velocity {axis} is {value} blocks/tick, beyond what the wire can carry"
                )));
            }
            Ok(scaled as i16)
        }
        Ok(Self {
            entity_id,
            velocity_x: to_wire("x", x)?,
            velocity_y: to_wire("y", y)?,
            velocity_z: to_wire("z", z)?,
        })
    }
}

impl Packet for RemoveEntities {
    const ID: i32 = crate::ids::clientbound::play::REMOVE_ENTITIES;

    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the body is malformed or has trailing bytes.
    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let decoded = Self::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "remove_entities has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(decoded)
    }

    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the id count does not fit a `VarInt`.
    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        self.encode_into(&mut writer)?;
        Ok(writer.finish())
    }
}

/// One block entity inside a chunk.
///
/// `BlockEntityInfo.LIST_STREAM_CODEC`: `packedXZ` and `y` are each a 16-bit
/// field, then a `VarInt` type id and the payload NBT. Both coordinates are
/// modelled as the raw `u16` the wire carries; `packed_xz` is `x << 4 | z` with
/// both in `0..=15`, so its sign bit is never set, and `y` is reinterpreted by
/// the world layer with the dimension's own range.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBlockEntity {
    /// Packed horizontal offset: `x << 4 | z`, each `0..=15`.
    pub packed_xz: u16,
    /// Block entity y.
    pub y: u16,
    /// Block entity type registry id.
    pub type_id: i32,
    /// Block entity payload (nameless network NBT compound).
    pub data: Nbt,
}

/// `minecraft:set_default_spawn_position` (clientbound play).
///
/// Body: packed block position, `f32` spawn angle. The angle is what the client
/// uses to orient a new player; it is sent for all game modes even though it
/// only matters for adventure/spawn-facing behaviour.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDefaultSpawnPosition {
    /// Dimension this spawn belongs to, e.g. `minecraft:overworld`.
    ///
    /// **26.1.2 leads with this**, and a `pitch` follows the yaw. Both were established by capturing the
    /// packet from a vanilla 26.1.2 server rather than by inference (P10-03, KD-40): the body is 37 bytes —
    /// `Identifier` (1 + 19) + `BlockPos` (8) + `f32` yaw (4) + `f32` pitch (4). Before that this packet sent
    /// only `BlockPos + f32`, and a real client rejected it with
    /// `readerIndex(10) + length(4) exceeds writerIndex(13)` — the payload was one field short, which is
    /// precisely what shape-inferred encoding cannot notice.
    pub dimension: String,
    /// Packed block position, see [`block_position`].
    pub position: i64,
    /// Spawn yaw in degrees.
    pub yaw: f32,
    /// Spawn pitch in degrees.
    pub pitch: f32,
}

impl Packet for SetDefaultSpawnPosition {
    const ID: i32 = clientbound::play::SET_DEFAULT_SPAWN_POSITION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let dimension = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
        let position = reader.read_i64()?;
        let yaw = reader.read_f32()?;
        let pitch = reader.read_f32()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_default_spawn_position has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            dimension,
            position,
            yaw,
            pitch,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_string(&self.dimension)?;
        writer.write_i64(self.position);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        Ok(writer.finish())
    }
}

/// `minecraft:set_health` (clientbound play).
///
/// Body: `f32` health, `VarInt` food level, `f32` saturation. The client does
/// not simulate any of the three — they are pure display state — so they are
/// sent as-is and never derived from one another here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetHealth {
    /// Current health (`0.0..=max_health`).
    pub health: f32,
    /// Food level (`0..=20`).
    pub food: i32,
    /// Food saturation (`0.0..=food`).
    pub saturation: f32,
}

impl Packet for SetHealth {
    const ID: i32 = clientbound::play::SET_HEALTH;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let health = reader.read_f32()?;
        let food = reader.read_varint()?;
        let saturation = reader.read_f32()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_health has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            health,
            food,
            saturation,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_f32(self.health);
        writer.write_varint(self.food);
        writer.write_f32(self.saturation);
        Ok(writer.finish())
    }
}

/// `minecraft:set_experience` (clientbound play).
///
/// Body: `f32` bar progress (`0.0..1.0`), `VarInt` level, `VarInt` total
/// experience. The three are independent fields on the wire: the client
/// renders the bar from `progress` directly, so the server must keep them
/// consistent itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetExperience {
    /// Progress towards the next level (`0.0..1.0`).
    pub progress: f32,
    /// Experience level.
    pub level: i32,
    /// Total experience accumulated.
    pub total: i32,
}

impl Packet for SetExperience {
    const ID: i32 = clientbound::play::SET_EXPERIENCE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let progress = reader.read_f32()?;
        let level = reader.read_varint()?;
        let total = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_experience has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            progress,
            level,
            total,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_f32(self.progress);
        writer.write_varint(self.level);
        writer.write_varint(self.total);
        Ok(writer.finish())
    }
}

/// `minecraft:set_time` (clientbound play).
///
/// 26.1 replaced the old `(world_age, day_time, do_daylight_cycle)` triple
/// with `(world_age, clock_updates)`: an `i64` plus a map of
/// `WorldClock → ClockNetworkState`, bytecode-read from the jar
/// (`ClientboundSetTimePacket`: `LONG` then a composite of
/// `WorldClock.STREAM_CODEC` and `ClockNetworkState.STREAM_CODEC`).
///
/// A clock reference is the **raw registry id** (`ByteBufCodecs$29`: `VarInt`
/// in, `byId` out — no plus-one, no inline form). The overworld clock
/// bootstraps first, so it is id `0` on the wire as `0x00`. An earlier
/// revision wrote id-plus-one (confusing this with the `holder` convention
/// of `ByteBufCodecs$30`); every update then landed on the wrong instance,
/// the overworld instance advanced locally from zero forever, and `/time`
/// moved the server's mobs without ever moving the client's sky — with no
/// error anywhere, because the shape stayed valid (P14-09 walk).
///
/// Rhythm, also from the jar: the per-second broadcast (`MinecraftServer.
/// forceGameTimeSynchronization`) always carries an **empty** map; a changed
/// clock is broadcast immediately (`ServerClockManager.modifyClock` builds a
/// single-entry map and pushes it); joins get the full sync. So per-second
/// empties are the steady state, not a degenerate case.
///
/// A clock state is `total_ticks VarLong`, `partial_tick f32`, `rate f32` in
/// field order. An empty map is a single `0x00` — which is exactly what the
/// P10-03 capture holds after its `i64` (eighteen packets, all 9 bytes).
///
/// We previously sent `i64 world_age` + `i64 time_of_day` + `bool`, 8 bytes
/// too many, which a real client reported as `was larger than I expected`.
#[derive(Debug, Clone, PartialEq)]
pub struct SetTime {
    /// Ticks the world has existed.
    pub world_age: i64,
    /// Clock updates, normally the single overworld entry. Empty decodes and
    /// encodes as the P10-03 capture (map size zero).
    pub clocks: Vec<ClockState>,
}

/// One `WorldClock → ClockNetworkState` map entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockState {
    /// Registry id of the clock (`WorldClocks` bootstrap order: overworld first).
    pub clock_id: i32,
    /// The clock's total ticks — what the client's sky renders. A `VarLong`
    /// on the wire (`ByteBufCodecs.VAR_LONG` inside the composite), not a
    /// fixed long: writing 8 bytes here was the P14-09 field-length error.
    pub total_ticks: i64,
    /// Sub-tick interpolation, normally `0.0`.
    pub partial_tick: f32,
    /// Tick rate multiplier, normally `1.0`.
    pub rate: f32,
}

/// The overworld day clock's registry id (`WorldClocks.bootstrap` registers
/// it first).
pub const WORLD_CLOCK_OVERWORLD: i32 = 0;

/// Cap on clock entries in one `set_time`; vanilla sends one per loaded
/// clockable dimension, and a hostile count must not drive a large loop.
pub const MAX_CLOCK_UPDATES: usize = 8;

impl Packet for SetTime {
    const ID: i32 = clientbound::play::SET_TIME;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let world_age = reader.read_i64()?;
        let count = read_count(&mut reader, "clock update", MAX_CLOCK_UPDATES)?;
        let mut clocks = Vec::with_capacity(count);
        for _ in 0..count {
            // Raw registry id, no offset: overworld is `0x00`.
            let clock_id = reader.read_varint()?;
            if clock_id < 0 {
                return Err(ServerError::Protocol(format!(
                    "set_time clock id {clock_id} is negative"
                )));
            }
            clocks.push(ClockState {
                clock_id,
                total_ticks: reader.read_varlong()?,
                partial_tick: reader.read_f32()?,
                rate: reader.read_f32()?,
            });
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_time has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { world_age, clocks })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.world_age);
        writer.write_varint(packed_len(self.clocks.len())?);
        for clock in &self.clocks {
            if clock.clock_id < 0 {
                return Err(ServerError::Invariant(format!(
                    "clock id {} is negative",
                    clock.clock_id
                )));
            }
            writer.write_varint(clock.clock_id);
            writer.write_varlong(clock.total_ticks);
            writer.write_f32(clock.partial_tick);
            writer.write_f32(clock.rate);
        }
        Ok(writer.finish())
    }
}

/// `minecraft:set_held_slot` (clientbound play).
///
/// Body: `VarInt` hotbar slot (`0..=8`). Sent when the server changes the
/// selection itself (after a respawn, or a swap); the client's own
/// `set_carried_item` is serverbound and modelled as
/// [`PlayIntent::SetCarriedItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetHeldSlot {
    /// Selected hotbar slot.
    pub slot: i32,
}

impl Packet for SetHeldSlot {
    const ID: i32 = clientbound::play::SET_HELD_SLOT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let slot = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_held_slot has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { slot })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.slot);
        Ok(writer.finish())
    }
}

/// `minecraft:game_event` (clientbound play).
///
/// Body: `u8` event id, `f32` value. One packet carries weather transitions
/// (rain/thunder start and stop) and game-mode changes; the value is a
/// game-mode id for the mode events and an unused `0.0` for most others.
/// A `u8` id means a reader must not treat an unknown id as fatal — new event
/// ids are additive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    /// Event id (`0` no respawn block available, `1` begin raining, …).
    pub event: u8,
    /// Event payload; `f32` because the mode-change events reuse the field as
    /// a game-mode id.
    pub value: f32,
}

impl Packet for GameEvent {
    const ID: i32 = clientbound::play::GAME_EVENT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let event = reader.read_u8()?;
        let value = reader.read_f32()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "game_event has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { event, value })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_u8(self.event);
        writer.write_f32(self.value);
        Ok(writer.finish())
    }
}

/// `minecraft:respawn` (clientbound play).
///
/// Body: a whole `CommonPlayerSpawnInfo`, then one trailing byte.
///
/// ```text
/// CommonPlayerSpawnInfo:
///   VarInt      dimension type id      (registry-friendly holder)
///   Identifier  dimension name         (ResourceKey)
///   i64         hashed seed
///   u8          game mode
///   i8          previous game mode     (-1 for none)
///   bool        is debug
///   bool        is flat
///   Optional    last death location    (bool + Identifier + i64 packed BlockPos)
///   VarInt      portal cooldown
///   VarInt      sea level
/// u8            data kept flags        <- AFTER the spawn info, not inside it
/// ```
///
/// # Why this shape, and what it replaced
///
/// An earlier version ended the body at `is_flat`, then wrote `data kept` and
/// `sea level`. That is not the 26.1.2 shape: the client's
/// `CommonPlayerSpawnInfo` read constructor consumes **ten** fields and the
/// packet's own reader consumes an eleventh byte after them, so the client read
/// past the end of our body and refused the packet — a real client could not
/// respawn after dying (owner's P11-10 acceptance round; `AUDIT-10-FINDINGS.md`
/// M-1).
///
/// Both halves are bytecode-verified against the client jar (`javap -c -p`):
///
/// * `CommonPlayerSpawnInfo`'s read constructor is
///   `DimensionType.STREAM_CODEC.decode`, `readResourceKey(Registries.DIMENSION)`,
///   `readLong`, `readByte`, `readByte`, `readBoolean`, `readBoolean`,
///   `readOptional(<GlobalPos codec>)`, `readVarInt`, `readVarInt`, and it calls
///   `<init>(Holder, ResourceKey, J, GameType, GameType, Z, Z, Optional, I, I)`;
///   the optional's codec is `FriendlyByteBuf.readGlobalPos`, i.e.
///   `readResourceKey` + `readBlockPos`;
/// * `ClientboundRespawnPacket.write` is
///   `commonPlayerSpawnInfo.write(buf)` then `buf.writeByte(dataToKeep)`.
///
/// These are **the same ten fields the client already accepts in [`JoinGame`]**,
/// which is why the two packets are written by the same code shape: the bug was
/// never the field *layout*, it was that `Respawn` stopped early and put the
/// trailing byte in the middle.
#[allow(clippy::struct_excessive_bools)] // Mirrors JoinGame, which is genuinely boolean-heavy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respawn {
    /// Index into the synced `minecraft:dimension_type` registry.
    pub dimension_type_id: i32,
    /// Key of the dimension the player respawns into.
    pub dimension_name: String,
    /// Hashed world seed shown to the client.
    pub hashed_seed: i64,
    /// Game mode id (0 survival, 1 creative, …).
    pub game_mode: u8,
    /// Previous game mode id (`-1` for none).
    pub previous_game_mode: i8,
    /// Debug world flag.
    pub is_debug: bool,
    /// Flat world flag.
    pub is_flat: bool,
    /// Where the player last died, as `(dimension key, packed block position)`.
    ///
    /// `Optional<GlobalPos>` on the wire: written as `false` when `None`. Vanilla
    /// sends the death position here so the client can offer "respawn at the death
    /// point" and place the death-screen locator.
    pub death_location: Option<(String, i64)>,
    /// Respawn-anchor cooldown in ticks; `0` when no charged anchor is held.
    pub portal_cooldown: i32,
    /// Sea level used for rendering.
    pub sea_level: i32,
    /// Which player metadata the client keeps across the respawn.
    ///
    /// The client reads it as a bit mask (`ClientboundRespawnPacket.shouldKeep`),
    /// so `0` keeps nothing — what a plain death respawn sends.
    pub data_kept: u8,
}

impl Packet for Respawn {
    const ID: i32 = clientbound::play::RESPAWN;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let dimension_type_id = reader.read_varint()?;
        let dimension_name = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
        let hashed_seed = reader.read_i64()?;
        let game_mode = reader.read_u8()?;
        let previous_game_mode = reader.read_i8()?;
        let is_debug = reader.read_bool()?;
        let is_flat = reader.read_bool()?;
        // `Optional<GlobalPos>`: a presence flag, then the dimension key and the
        // packed block position only when present.
        let death_location = if reader.read_bool()? {
            let dimension = reader.read_string(crate::MAX_IDENTIFIER_LEN)?;
            let position = reader.read_i64()?;
            Some((dimension, position))
        } else {
            None
        };
        let portal_cooldown = reader.read_varint()?;
        let sea_level = reader.read_varint()?;
        let data_kept = reader.read_u8()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "respawn has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            death_location,
            portal_cooldown,
            sea_level,
            data_kept,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.dimension_type_id);
        writer.write_string(&self.dimension_name)?;
        writer.write_i64(self.hashed_seed);
        writer.write_u8(self.game_mode);
        writer.write_i8(self.previous_game_mode);
        writer.write_bool(self.is_debug);
        writer.write_bool(self.is_flat);
        // Absent death location is a single `false` byte, which is what a player
        // who has not died yet (or who died in another dimension) sends.
        match &self.death_location {
            Some((dimension, position)) => {
                writer.write_bool(true);
                writer.write_string(dimension)?;
                writer.write_i64(*position);
            }
            None => writer.write_bool(false),
        }
        writer.write_varint(self.portal_cooldown);
        writer.write_varint(self.sea_level);
        // The packet's own trailing byte, after the whole spawn info.
        writer.write_u8(self.data_kept);
        Ok(writer.finish())
    }
}

/// `minecraft:system_chat` (clientbound play).
///
/// Body: a nameless network-NBT text component, then `bool` overlay. When
/// `overlay` is set the client draws the message above the hotbar (the vanilla
/// "action bar") and it is *not* added to chat history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemChat {
    /// Component to display.
    pub content: TextComponent,
    /// Draw above the hotbar instead of in the chat box.
    pub overlay: bool,
}

impl Packet for SystemChat {
    const ID: i32 = clientbound::play::SYSTEM_CHAT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut rest = payload;
        let nbt = Nbt::read_network(&mut rest)?;
        let content = super::config::text_from_nbt(&nbt)?;
        let overlay = PacketReader::new(rest).read_bool()?;
        Ok(Self { content, overlay })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        let mut nbt_bytes = Vec::new();
        self.content.to_nbt().write_network(&mut nbt_bytes)?;
        writer.write_bytes(&nbt_bytes);
        writer.write_bool(self.overlay);
        Ok(writer.finish())
    }
}

/// `minecraft:disguised_chat` — a server message attributed to a player.
///
/// **What a 26.1.2 console `say` produces**, which a capture of a real server confirmed: two `say` commands produced
/// two `disguised_chat` and **zero** `system_chat`. A server that answers chat with [`SystemChat`] alone therefore
/// diverges from the client's expectation, which is what P10-10 exists to fix.
///
/// # Format, from a captured payload
///
/// ```text
/// 08 00 03 62 79 65 | 05 | 08 00 06 53 65 72 76 65 72 | 00
///    6 bytes          1        9 bytes                    1   = 17
/// ```
///
/// * `08` is `TAG_String`, and **the network form omits the tag's name**, so `00 03` is the length and `62 79 65`
///   is `"bye"`;
/// * `05` is a `VarInt`: the **chat type**, a registry id, and the third place in this phase where a number is a
///   claim about a registry the client owns;
/// * `08 00 06` and `53 65 72 76 65 72` are the sender's name, `"Server"`;
/// * `00` is `TAG_End` — an **absent** optional target name, written rather than omitted.
///
/// The last point cost a round. A decoder that reads the terminator without consuming it leaves one byte behind and
/// fails its own trailing-byte check; one that treats an empty remainder as absence accepts a truncated packet.
/// **Absence is written, so reading it has to consume it.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisguisedChat {
    /// The message to display.
    pub message: TextComponent,
    /// Chat type registry id: how the client decorates and colours the line.
    pub chat_type: i32,
    /// Who the message is attributed to.
    pub sender_name: TextComponent,
    /// The target's name, for a message about someone.
    pub target_name: Option<TextComponent>,
}

impl Packet for DisguisedChat {
    const ID: i32 = clientbound::play::DISGUISED_CHAT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut rest = payload;
        let message = super::config::text_from_nbt(&Nbt::read_network(&mut rest)?)?;
        let mut reader = PacketReader::new(rest);
        let chat_type = reader.read_varint()?;
        rest = reader.take_remaining();
        let sender_name = super::config::text_from_nbt(&Nbt::read_network(&mut rest)?)?;

        let target_name = if rest == [0x00] {
            // Consumed, not merely recognised: leaving the terminator behind is what made the trailing-byte check
            // fire on a payload that was correct.
            rest = &rest[1..];
            None
        } else if rest.is_empty() {
            return Err(ServerError::Protocol(
                "disguised_chat ended before its target name field".to_owned(),
            ));
        } else {
            Some(super::config::text_from_nbt(&Nbt::read_network(
                &mut rest,
            )?)?)
        };

        if !rest.is_empty() {
            return Err(ServerError::Protocol(format!(
                "disguised_chat has {} trailing bytes",
                rest.len()
            )));
        }
        Ok(Self {
            message,
            chat_type,
            sender_name,
            target_name,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        let mut bytes = Vec::new();
        self.message.to_nbt().write_network(&mut bytes)?;
        writer.write_bytes(&bytes);
        writer.write_varint(self.chat_type);
        let mut bytes = Vec::new();
        self.sender_name.to_nbt().write_network(&mut bytes)?;
        writer.write_bytes(&bytes);
        let mut bytes = Vec::new();
        match &self.target_name {
            Some(name) => name.to_nbt().write_network(&mut bytes)?,
            None => bytes.push(0x00),
        }
        writer.write_bytes(&bytes);
        Ok(writer.finish())
    }
}

/// `minecraft:block_entity_data` — the contents of a block entity.
///
/// Sent when a block entity is placed and when its contents change, so a client can render a chest's items or a
/// sign's text as soon as it exists rather than when the chunk is next re-sent.
///
/// # Shape, from two precedents rather than from a guess
///
/// `BlockUpdate` carries its position as a **packed `i64`** and reads it with `read_i64`; [`ChunkBlockEntity`]
/// carries a **block entity type registry id** and a **network NBT payload**. This packet is those three fields,
/// and the trait machinery is the same.
///
/// **The type id is a registry claim of the kind this phase has twice found wrong elsewhere.** A block entity
/// type is a **built-in** registry compiled into the client jar, so its ids come from a jar extraction and not
/// from the config payload.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityData {
    /// Packed block position, as `BlockUpdate` carries it.
    pub position: i64,
    /// Block entity type registry id.
    pub type_id: i32,
    /// The block entity's payload, as network NBT.
    pub data: Nbt,
}

impl Packet for BlockEntityData {
    const ID: i32 = clientbound::play::BLOCK_ENTITY_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let position = reader.read_i64()?;
        let type_id = reader.read_varint()?;
        let mut rest = reader.take_remaining();
        let data = Nbt::read_network(&mut rest)?;
        if !rest.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_entity_data has {} trailing bytes",
                rest.len()
            )));
        }
        Ok(Self {
            position,
            type_id,
            data,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.position);
        writer.write_varint(self.type_id);
        let mut bytes = Vec::new();
        self.data.write_network(&mut bytes)?;
        writer.write_bytes(&bytes);
        Ok(writer.finish())
    }
}

/// Longest chat message Vanilla accepts (`writeUtf(message, 256)` in
/// `ServerboundChatPacket.write`, verified from the 26.1.2 jar).
pub const CHAT_MAX_CHARS: usize = 256;

/// Longest command string; Vanilla reads it with the default `writeUtf` cap.
pub const COMMAND_MAX_CHARS: usize = 32_500;

/// Length of a chat `MessageSignature` (a 256-byte salt signature).
pub const SIGNATURE_LEN: usize = 256;

/// Upper bound on acknowledged last-seen messages in one chat packet.
///
/// Vanilla's `LastSeenMessages.Update` holds at most 20 entries; the cap keeps a
/// hostile count from driving a large loop.
pub const LAST_SEEN_MAX: i32 = 20;

/// Player hand (`0` main, `1` off).
pub const HAND_MAIN: i32 = 0;
/// Off hand.
pub const HAND_OFF: i32 = 1;

/// Decoded serverbound play intents.
///
/// Phase 02 recognised these but did not simulate them; Phase 04 feeds the
/// movement/interaction variants into the world and answers the rest.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayIntent {
    /// Player movement (position only).
    MovePlayerPos {
        /// Absolute x.
        x: f64,
        /// Feet y.
        y: f64,
        /// Absolute z.
        z: f64,
        /// Whether the client claims to be on the ground.
        on_ground: bool,
    },
    /// Player movement (position + rotation).
    MovePlayerPosRot {
        /// Absolute x.
        x: f64,
        /// Feet y.
        y: f64,
        /// Absolute z.
        z: f64,
        /// Yaw degrees.
        yaw: f32,
        /// Pitch degrees.
        pitch: f32,
        /// Whether the client claims to be on the ground.
        on_ground: bool,
    },
    /// Player rotation only.
    MovePlayerRot {
        /// Yaw degrees.
        yaw: f32,
        /// Pitch degrees.
        pitch: f32,
        /// Whether the client claims to be on the ground.
        on_ground: bool,
    },
    /// Ground state only.
    MovePlayerStatusOnly {
        /// Whether the client claims to be on the ground.
        on_ground: bool,
    },
    /// Teleport acknowledgement.
    AcceptTeleportation {
        /// Acknowledged teleport id.
        teleport_id: i32,
    },
    /// A player action (dig/place intent).
    PlayerAction {
        /// Action id.
        status: i32,
        /// Packed block position.
        position: i64,
        /// Face id.
        facing: u8,
        /// Action sequence number.
        sequence: i32,
    },
    /// A client command (e.g. respawn).
    ClientCommand {
        /// Command action id.
        action: i32,
    },
    /// Chat message body.
    ///
    /// 1.19+ `serverbound:chat` carries a signature payload after the string
    /// (timestamp, salt, signature, last-seen update). Phase 04 does not process
    /// signed chat, but the trailing bytes are consumed and validated so a
    /// malformed payload is rejected here instead of being silently truncated.
    Chat {
        /// Raw message text.
        message: String,
        /// Unix milliseconds the client says it sent the message.
        timestamp_millis: i64,
        /// Signature salt.
        salt: i64,
        /// Whether a 256-byte signature followed.
        signed: bool,
        /// Number of acknowledged last-seen messages.
        last_seen_count: i32,
    },
    /// Chat command without the leading slash.
    ChatCommand {
        /// Raw command text.
        command: String,
    },
    /// Arm swing (main or off hand).
    Swing {
        /// Hand id: 0 main, 1 off.
        hand: i32,
    },
    /// Player interaction with an entity (serverbound `minecraft:interact`).
    ///
    /// Wire type 1 is an **attack**, 0 an interaction, 2 an interaction at a
    /// coordinate. All three decode here so a body is never silently half-read;
    /// the server only *acts* on the attack.
    Interact {
        /// Entity being attacked or used.
        entity: i32,
        /// Wire interaction type: 0 interact, 1 attack, 2 interact-at.
        kind: i32,
    },
    /// Block placement / item use on a block.
    UseItemOn {
        /// Hand id: 0 main, 1 off.
        hand: i32,
        /// Packed block position.
        position: i64,
        /// Face clicked (0..5, `-1` for inside).
        face: i32,
        /// Cursor position within the block, per axis (0.0..1.0).
        cursor_x: f32,
        /// Cursor y.
        cursor_y: f32,
        /// Cursor z.
        cursor_z: f32,
        /// Whether the click landed inside the block shape.
        inside_block: bool,
        /// Action sequence number (echoed back in block-change acks).
        sequence: i32,
    },
    /// Item use in air (no block target).
    UseItem {
        /// Hand id: 0 main, 1 off.
        hand: i32,
        /// Action sequence number.
        sequence: i32,
        /// Player yaw at use time.
        yaw: f32,
        /// Player pitch at use time.
        pitch: f32,
    },
    /// Hotbar slot selection.
    SetCarriedItem {
        /// Newly selected hotbar slot (0..8).
        slot: i16,
    },
    /// Container/window close.
    ContainerClose {
        /// Window id (0 is the player inventory), vanilla `CONTAINER_ID`
        /// (`VarInt`, same codec as the clientbound close).
        window_id: i32,
    },
    /// A click inside an open container window.
    ///
    /// Carries the **raw wire integers**; interpreting them is
    /// `mc_container::Click`'s job, not this crate's, so that the protocol layer
    /// stays free of gameplay dependencies and the validation lives next to the
    /// state it validates against.
    ///
    /// The packet also carries two trailing fields (`changedSlots` and
    /// `carriedItem`, each a `HashedStack`) that the client fills with a *prediction*
    /// of the slots it believes it changed. They are **not decoded**: they are a
    /// desync-detection optimisation, not the authority mechanism (the state id is),
    /// and the frame is length-delimited so the unread bytes are discarded safely.
    /// Recorded as a parity gap rather than guessed at.
    ContainerClick {
        /// Window id (`0` is the player inventory).
        window_id: i32,
        /// The server revision the client believes it is looking at.
        state_id: i32,
        /// Menu slot, or `-1` for a click outside the window.
        slot: i16,
        /// Button; its meaning depends on `click_type`.
        button: i8,
        /// `ContainerInput` ordinal: 0 pickup, 1 quick-move, 2 swap, 3 clone,
        /// 4 throw, 5 quick-craft, 6 pickup-all.
        click_type: i32,
    },
    /// A real client's own end-of-tick marker (serverbound play 13; 911
    /// captured bodies, every one empty). The server's tick is its own clock,
    /// so this is decoded and deliberately unacted.
    ClientTickEnd,
    /// Serverbound keep-alive response (play 28): the `i64` the server sent.
    /// This server never sends keep-alives, so a response is decoded (so the
    /// trailing-byte guard covers it) and deliberately unacted.
    KeepAlive {
        /// The echoed keep-alive id.
        id: i64,
    },
    /// The client's chunk-batch flow-control ack (serverbound play 11):
    /// desired chunks per tick. Decoded so the capture sweep covers it;
    /// unacted — chunk sending is not batched.
    ChunkBatchReceived {
        /// Desired chunks per tick.
        desired_chunks_per_tick: f32,
    },
    /// The client finished loading (serverbound play 44, empty). Decoded and
    /// deliberately unacted.
    PlayerLoaded,
}

impl PlayIntent {
    /// Decode a serverbound play packet by id; `None` for unmodelled ids.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when a *recognized* packet is malformed.
    // One flat dispatch over every serverbound play packet. Splitting it would put
    // the per-packet field order — the part that must match the jar — behind a jump,
    // and the whole point of this function is that one can read a packet's layout
    // top to bottom.
    #[allow(clippy::too_many_lines)]
    pub fn decode(id: i32, payload: &[u8]) -> ServerResult<Option<Self>> {
        let mut reader = PacketReader::new(payload);
        let intent = match id {
            serverbound::play::MOVE_PLAYER_POS => Some(Self::MovePlayerPos {
                x: reader.read_f64()?,
                y: reader.read_f64()?,
                z: reader.read_f64()?,
                on_ground: reader.read_bool()?,
            }),
            serverbound::play::MOVE_PLAYER_POS_ROT => Some(Self::MovePlayerPosRot {
                x: reader.read_f64()?,
                y: reader.read_f64()?,
                z: reader.read_f64()?,
                yaw: reader.read_f32()?,
                pitch: reader.read_f32()?,
                on_ground: reader.read_bool()?,
            }),
            serverbound::play::MOVE_PLAYER_ROT => Some(Self::MovePlayerRot {
                yaw: reader.read_f32()?,
                pitch: reader.read_f32()?,
                on_ground: reader.read_bool()?,
            }),
            serverbound::play::MOVE_PLAYER_STATUS_ONLY => Some(Self::MovePlayerStatusOnly {
                on_ground: reader.read_bool()?,
            }),
            serverbound::play::ACCEPT_TELEPORTATION => Some(Self::AcceptTeleportation {
                teleport_id: reader.read_varint()?,
            }),
            serverbound::play::PLAYER_ACTION => Some(Self::PlayerAction {
                status: reader.read_varint()?,
                position: reader.read_i64()?,
                facing: reader.read_u8()?,
                sequence: reader.read_varint()?,
            }),
            serverbound::play::CLIENT_COMMAND => Some(Self::ClientCommand {
                action: reader.read_varint()?,
            }),
            serverbound::play::CHAT => {
                // Full 1.19+ shape: `message`, `timestamp`, `salt`,
                // `signature` (optional 256-byte array), `last_seen` update.
                let message = reader.read_string(CHAT_MAX_CHARS)?;
                let timestamp_millis = reader.read_i64()?;
                let salt = reader.read_i64()?;
                let signed = match reader.read_u8()? {
                    1 => {
                        reader.read_bytes(SIGNATURE_LEN)?;
                        true
                    }
                    _ => false,
                };
                // `LastSeenMessages.Update`: a VarInt count, then that many entries
                // of (VarInt id, 256-byte signature). Bounded like everything else.
                let last_seen_count = reader.read_varint()?;
                if !(0..=LAST_SEEN_MAX).contains(&last_seen_count) {
                    return Err(ServerError::Protocol(format!(
                        "chat last_seen count {last_seen_count} out of range"
                    )));
                }
                for _ in 0..last_seen_count {
                    let _ = reader.read_varint()?;
                    reader.read_bytes(SIGNATURE_LEN)?;
                }
                Some(Self::Chat {
                    message,
                    timestamp_millis,
                    salt,
                    signed,
                    last_seen_count,
                })
            }
            serverbound::play::CHAT_COMMAND => Some(Self::ChatCommand {
                command: reader.read_string(COMMAND_MAX_CHARS)?,
            }),
            serverbound::play::SWING => Some(Self::Swing {
                hand: reader.read_varint()?,
            }),
            serverbound::play::INTERACT => {
                let entity = reader.read_varint()?;
                let kind = reader.read_varint()?;
                // Types 0 and 2 carry a target coordinate and a hand after the
                // entity id; type 1 (attack) stops after `sneaking`. Reading
                // each shape fully is what keeps a body from being half-decoded.
                match kind {
                    1 => {
                        let _sneaking = reader.read_bool()?;
                    }
                    2 => {
                        let _x = reader.read_f32()?;
                        let _y = reader.read_f32()?;
                        let _z = reader.read_f32()?;
                        let _hand = reader.read_varint()?;
                        let _sneaking = reader.read_bool()?;
                    }
                    _ => {
                        let _hand = reader.read_varint()?;
                        let _sneaking = reader.read_bool()?;
                    }
                }
                Some(Self::Interact { entity, kind })
            }
            serverbound::play::USE_ITEM_ON => Some(Self::UseItemOn {
                hand: reader.read_varint()?,
                position: reader.read_i64()?,
                face: reader.read_varint()?,
                cursor_x: reader.read_f32()?,
                cursor_y: reader.read_f32()?,
                cursor_z: reader.read_f32()?,
                inside_block: reader.read_bool()?,
                sequence: reader.read_varint()?,
            }),
            serverbound::play::USE_ITEM => Some(Self::UseItem {
                hand: reader.read_varint()?,
                sequence: reader.read_varint()?,
                yaw: reader.read_f32()?,
                pitch: reader.read_f32()?,
            }),
            serverbound::play::SET_CARRIED_ITEM => Some(Self::SetCarriedItem {
                slot: reader.read_i16()?,
            }),
            serverbound::play::CONTAINER_CLOSE => Some(Self::ContainerClose {
                window_id: reader.read_varint()?,
            }),
            // Field order verified from the jar (see the variant's doc comment).
            // The two trailing `HashedStack` fields are intentionally left unread.
            serverbound::play::CONTAINER_CLICK => Some(Self::ContainerClick {
                window_id: reader.read_varint()?,
                state_id: reader.read_varint()?,
                slot: reader.read_i16()?,
                button: reader.read_i8()?,
                click_type: reader.read_varint()?,
            }),
            // A real client sends this at the end of each of its own ticks
            // (911 captured bodies, every one empty). The server's tick is its
            // own clock, so the intent is deliberately unacted -- modelled so
            // the per-tick arrival is silent instead of a debug flood.
            serverbound::play::CLIENT_TICK_END => Some(Self::ClientTickEnd),
            serverbound::play::KEEP_ALIVE => Some(Self::KeepAlive {
                id: reader.read_i64()?,
            }),
            serverbound::play::CHUNK_BATCH_RECEIVED => Some(Self::ChunkBatchReceived {
                desired_chunks_per_tick: reader.read_f32()?,
            }),
            serverbound::play::PLAYER_LOADED => Some(Self::PlayerLoaded),
            _ => None,
        };
        // AUDIT-09 A-03: a recognized packet must be consumed exactly. Any
        // trailing byte is either a hostile probe or a field this build does
        // not model — both are refused rather than silently accepted. The one
        // exemption is `CONTAINER_CLICK`, whose two trailing `HashedStack`
        // fields are a documented parity gap (see the variant's docs): the
        // frame is length-delimited, so the unread bytes are discarded safely.
        if intent.is_some() && id != serverbound::play::CONTAINER_CLICK && !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "serverbound play packet {id} has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(intent)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockDestruction, COMMAND_MAX_CHARS, ConfigurationAcknowledged, ContainerClose,
        ContainerSetData, ForgetLevelChunk, JoinGame, KeepAlive, LightData, LightUpdate,
        MENU_FURNACE, MENU_GENERIC_9X3, MENU_GENERIC_9X6, MENU_HOPPER, OpenScreen, PlayDisconnect,
        PlayIntent, PlayPingRequest, PlayPong, PlayerPosition, RemoveMobEffect, SIGNATURE_LEN,
        SetChunkCacheCenter, SetChunkCacheRadius, SetCursorItem, UpdateMobEffect,
    };
    use crate::packets::Packet;
    use crate::text::TextComponent;

    fn sample_join_game() -> JoinGame {
        JoinGame {
            entity_id: 1,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld".to_owned()],
            max_players: 10,
            view_distance: 8,
            simulation_distance: 8,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            dimension_type_id: 0,
            dimension_name: "minecraft:overworld".to_owned(),
            hashed_seed: 12345,
            game_mode: 0,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: false,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            enforce_secure_chat: false,
        }
    }

    /// The exact payload a **vanilla 26.1.2 server** sends for `set_default_spawn_position`.
    ///
    /// Captured through the P10-01 rig (P10-03, KD-40). 36 payload bytes after the id:
    ///
    /// ```text
    /// 13 6d696e6563726166743a6f766572776f726c64   "minecraft:overworld"
    /// 0000000000000fc4                            i64 4036 = packed BlockPos(0, -60, 0)
    /// 00000000 00000000                           f32 yaw, f32 pitch
    /// ```
    ///
    /// Ours sent 12 bytes (`BlockPos` + one `f32`) and a real client rejected it with
    /// `readerIndex(10) + length(4) exceeds writerIndex(13)`. Pinning the captured bytes is what stops that
    /// returning: the shape was inferred once and never checked against anything real.
    /// The exact payload a **vanilla 26.1.2 server** sends for `player_position`.
    ///
    /// Captured through the P10-01 rig (P10-03). 61 payload bytes after the id:
    ///
    /// ```text
    /// 01                 VarInt teleport id          <- leading
    /// bfe0000000000000   f64 x = -0.5
    /// c04e000000000000   f64 y = -60.0
    /// bfe0000000000000   f64 z = -0.5
    /// 0000000000000000   f64 dx, dy, dz
    /// 00000000 00000000  f32 yaw, f32 pitch
    /// 00000000           i32 flags
    /// ```
    ///
    /// We wrote the same fields with the teleport id **last**, so a client read the top byte of `x` as the id
    /// and reported `found 1 bytes extra` — a symptom that reads like a width problem and is actually an
    /// ordering one.
    /// The exact payload a **vanilla 26.1.2 server** sends for `set_time`.
    ///
    /// Captured through the P10-01 rig (P10-03, KD-43). Nine payload bytes:
    ///
    /// ```text
    /// 0000000000001cf2   i64 world age = 7410
    /// 00                 one trailing byte, 0x00 in all eighteen captured packets
    /// ```
    ///
    /// Eighteen packets at this id are all nine bytes, with the `i64` incrementing by exactly 20 — one second
    /// of ticks, which is this packet's send rate. We previously sent `i64` + `i64` + `bool` (17 bytes) and a
    /// real client reported `was larger than I expected`.
    /// The light half of `light_update` must be **byte-identical** to the light half of
    /// `level_chunk_with_light` for the same light.
    ///
    /// The two packets share `write_light_data`, so this is a statement about the code as much as the bytes —
    /// but the bytes are what a client reads, and a real client already accepts the chunk packet's half on
    /// every chunk it is sent. Pinning the equality isolates the one thing that genuinely differs between the
    /// packets: the two coordinates, `VarInt` here and `i32` there (KD-49).
    #[test]
    fn the_light_half_of_both_packets_is_byte_identical() {
        let light = LightData {
            sky_light_mask: vec![1],
            block_light_mask: vec![2],
            empty_sky_light_mask: vec![0],
            empty_block_light_mask: Vec::new(),
            sky_light: vec![vec![0x5A; 2048]],
            block_light: vec![vec![0xA5; 2048]],
        };
        let chunk = LevelChunkWithLight {
            chunk_x: 1,
            chunk_z: 2,
            heightmaps: Vec::new(),
            sections: Vec::new(),
            block_entities: Vec::new(),
            sky_light_mask: light.sky_light_mask.clone(),
            block_light_mask: light.block_light_mask.clone(),
            empty_sky_light_mask: light.empty_sky_light_mask.clone(),
            empty_block_light_mask: light.empty_block_light_mask.clone(),
            sky_light: light.sky_light.clone(),
            block_light: light.block_light.clone(),
        };
        let update = LightUpdate {
            chunk_x: 1,
            chunk_z: 2,
            sky_light_mask: light.sky_light_mask.clone(),
            block_light_mask: light.block_light_mask.clone(),
            empty_sky_light_mask: light.empty_sky_light_mask.clone(),
            empty_block_light_mask: light.empty_block_light_mask.clone(),
            sky_light: light.sky_light.clone(),
            block_light: light.block_light.clone(),
        };

        let chunk_bytes = chunk.encode().expect("encodes");
        let update_bytes = update.encode().expect("encodes");

        // The chunk packet's header before the light half, with no heightmaps, no sections and no block
        // entities: two `i32` coordinates and three zero counts, so eleven bytes. `light_update`'s is two
        // `VarInt` coordinates, so two.
        assert_eq!(
            &chunk_bytes[11..],
            &update_bytes[2..],
            "the light a client reads must be the same bytes in both packets"
        );
        assert_eq!(chunk_bytes.len() - 11, update_bytes.len() - 2);
        // Which also says the two headers really are those lengths, so the slices above are the light halves
        // and not accidentally equal for some other reason.
        assert_eq!(
            &chunk_bytes[..2],
            &[0x00, 0x00],
            "the chunk packet's x is i32, high bytes first"
        );
        assert_eq!(
            &update_bytes[..2],
            &[0x01, 0x02],
            "light_update's coordinates are one VarInt byte each"
        );
    }

    #[test]
    fn light_update_round_trips_its_light_data() {
        let packet = LightUpdate {
            chunk_x: -3,
            chunk_z: 7,
            // A sky array for light section 1 and an empty-block declaration for section 0: one of each mask
            // kind, so the round trip exercises both branches.
            sky_light_mask: vec![1],
            block_light_mask: Vec::new(),
            empty_sky_light_mask: Vec::new(),
            empty_block_light_mask: vec![0],
            sky_light: vec![vec![0xAB; 2048]],
            block_light: Vec::new(),
        };
        let encoded = packet.encode().expect("encodes");
        assert_eq!(
            LightUpdate::decode(&encoded).expect("decodes"),
            packet,
            "the light data and the coordinates must survive the wire"
        );
    }

    /// **`light_update` writes its coordinates as `VarInt`; `level_chunk_with_light` writes them as `i32`.**
    ///
    /// The two packets carry identical light data, so "they are shaped alike" is the natural assumption and it
    /// is wrong for exactly these two fields. Reading them the other way consumes six extra bytes and misparses
    /// everything after — settled with `javap` on the jar rather than by analogy, and pinned here so a later
    /// reader cannot quietly unify them.
    #[test]
    fn light_update_coordinates_are_varints_where_the_chunk_packet_uses_i32() {
        let update = LightUpdate {
            chunk_x: 1,
            chunk_z: 2,
            ..LightUpdate::default()
        };
        let encoded = update.encode().expect("encodes");
        assert_eq!(
            &encoded[..2],
            &[0x01, 0x02],
            "one byte each: the coordinates are VarInts here"
        );

        let chunk = LevelChunkWithLight {
            chunk_x: 1,
            chunk_z: 2,
            heightmaps: Vec::new(),
            sections: Vec::new(),
            block_entities: Vec::new(),
            sky_light_mask: Vec::new(),
            block_light_mask: Vec::new(),
            empty_sky_light_mask: Vec::new(),
            empty_block_light_mask: Vec::new(),
            sky_light: Vec::new(),
            block_light: Vec::new(),
        };
        let chunk_encoded = chunk.encode().expect("encodes");
        assert_eq!(
            &chunk_encoded[..4],
            &[0x00, 0x00, 0x00, 0x01],
            "four bytes each: the chunk packet's are i32"
        );
        assert_ne!(
            &encoded[..2],
            &chunk_encoded[..2],
            "if these ever match, one of the two encodings has changed"
        );
    }

    #[test]
    fn the_captured_vanilla_set_time_is_reproduced() {
        // Eighteen packets, all 9 bytes: world age plus an *empty clock map*.
        // What P10-03 read as "one trailing byte" was the map size zero.
        let captured: [u8; 9] = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0xf2, // world age 7410
            0x00, // zero clock updates
        ];
        let packet = SetTime {
            world_age: 7410,
            clocks: Vec::new(),
        };
        let encoded = packet.encode().expect("encodes");
        assert_eq!(
            encoded, captured,
            "the captured vanilla bytes must be reproduced exactly"
        );
        assert_eq!(SetTime::decode(&encoded).expect("decodes"), packet);
    }

    #[test]
    fn set_time_carries_the_overworld_clock() {
        // world age 6000, one clock: overworld id 0 on the wire as `0x00`,
        // 20000 ticks, no partial, normal rate. (An earlier revision wrote
        // id-plus-one here; the golden pins the raw id.)
        let packet = SetTime {
            world_age: 6000,
            clocks: vec![super::ClockState {
                clock_id: super::WORLD_CLOCK_OVERWORLD,
                total_ticks: 20_000,
                partial_tick: 0.0,
                rate: 1.0,
            }],
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(
            body,
            [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x17, 0x70, // world age
                0x01, // one clock update
                0x00, // overworld, raw registry id
                0xA0, 0x9C, 0x01, // 20000 as VarLong
                0x00, 0x00, 0x00, 0x00, // partial 0.0
                0x3F, 0x80, 0x00, 0x00, // rate 1.0
            ]
        );
        assert_eq!(SetTime::decode(&body).expect("decodes"), packet);
        // A negative clock id is refused, not guessed at.
        let mut bad = body[..9].to_vec();
        bad.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert!(SetTime::decode(&bad).is_err());
    }

    #[test]
    fn the_captured_vanilla_player_position_is_reproduced() {
        let captured: [u8; 61] = [
            0x01, // teleport id, leading
            0xbf, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // x = -0.5
            0xc0, 0x4e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // y = -60.0
            0xbf, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // z = -0.5
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dx
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dy
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dz
            0x00, 0x00, 0x00, 0x00, // yaw
            0x00, 0x00, 0x00, 0x00, // pitch
            0x00, 0x00, 0x00, 0x00, // flags
        ];
        let packet = PlayerPosition {
            teleport_id: 1,
            x: -0.5,
            y: -60.0,
            z: -0.5,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            flags: 0,
        };
        let encoded = packet.encode().expect("encodes");
        assert_eq!(
            encoded.len(),
            captured.len(),
            "a real client accepted exactly this many bytes"
        );
        assert_eq!(
            encoded, captured,
            "the captured vanilla bytes must be reproduced exactly"
        );
        assert_eq!(PlayerPosition::decode(&encoded).expect("decodes"), packet);
    }

    #[test]
    fn the_captured_vanilla_payload_is_reproduced() {
        let captured: [u8; 36] = [
            0x13, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'o', b'v', b'e',
            b'r', b'w', b'o', b'r', b'l', b'd', // Identifier "minecraft:overworld"
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0f, 0xc4, // packed BlockPos(0, -60, 0)
            0x00, 0x00, 0x00, 0x00, // yaw
            0x00, 0x00, 0x00, 0x00, // pitch
        ];
        let packet = SetDefaultSpawnPosition {
            dimension: "minecraft:overworld".to_owned(),
            position: block_position(0, -60, 0),
            yaw: 0.0,
            pitch: 0.0,
        };
        let encoded = packet.encode().expect("encodes");
        assert_eq!(
            encoded.len(),
            captured.len(),
            "the payload must be the length a real client accepted"
        );
        assert_eq!(
            encoded, captured,
            "the captured vanilla bytes must be reproduced exactly"
        );
        // And it must decode back to the same packet, not merely to the same length.
        assert_eq!(
            SetDefaultSpawnPosition::decode(&encoded).expect("decodes"),
            packet
        );
    }

    #[test]
    fn join_game_round_trip() {
        let packet = sample_join_game();
        let bytes = packet.encode().expect("encodes");
        assert_eq!(JoinGame::decode(&bytes).expect("decodes"), packet);
    }

    #[test]
    fn join_game_rejects_hostile_dimension_count() {
        let mut writer = crate::wire::PacketWriter::new();
        writer.write_i32(1);
        writer.write_bool(false);
        writer.write_varint(1000);
        assert!(JoinGame::decode(&writer.finish()).is_err());
    }

    #[test]
    fn keep_alive_round_trip() {
        let packet = KeepAlive { id: -1 };
        assert_eq!(
            KeepAlive::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn play_disconnect_round_trip() {
        let packet = PlayDisconnect {
            reason: TextComponent::literal("kicked"),
        };
        assert_eq!(
            PlayDisconnect::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn cache_packets_round_trip() {
        let center = SetChunkCacheCenter { x: -3, z: 7 };
        assert_eq!(
            SetChunkCacheCenter::decode(&center.encode().expect("encodes")).expect("decodes"),
            center
        );
        let radius = SetChunkCacheRadius { radius: 8 };
        assert_eq!(
            SetChunkCacheRadius::decode(&radius.encode().expect("encodes")).expect("decodes"),
            radius
        );
    }

    #[test]
    fn forget_level_chunk_packs_x_low_z_high() {
        use crate::ids::clientbound;
        assert_eq!(
            ForgetLevelChunk::ID,
            37,
            "jar game_clientbound table row 37"
        );
        assert_eq!(ForgetLevelChunk::ID, clientbound::play::FORGET_LEVEL_CHUNK);
        // Bytecode-read `ChunkPos.pack`: x in the low 32 bits, z in the high
        // 32, one big-endian long. (1, 2) -> 00 00 00 02 00 00 00 01.
        let packet = ForgetLevelChunk { x: 1, z: 2 };
        assert_eq!(
            packet.encode().expect("encodes"),
            vec![0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01]
        );
        assert_eq!(
            ForgetLevelChunk::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
        // Negative coordinates survive the pack (sign-extended halves).
        for (x, z) in [(-1, -1), (-33, 17), (i32::MIN, i32::MAX)] {
            let packet = ForgetLevelChunk { x, z };
            assert_eq!(
                ForgetLevelChunk::decode(&packet.encode().expect("encodes")).expect("decodes"),
                packet,
                "({x}, {z}) must round-trip"
            );
        }
        // The packet is one long wide: trailing bytes are refused, like every
        // other fixed-shape clientbound decode (AUDIT-14, A-03 recheck).
        let mut padded = packet.encode().expect("encodes");
        padded.push(0x00);
        assert!(
            ForgetLevelChunk::decode(&padded).is_err(),
            "a trailing byte must not decode"
        );
    }

    #[test]
    fn configuration_acknowledged_is_empty() {
        assert_eq!(
            ConfigurationAcknowledged.encode().expect("encodes"),
            Vec::<u8>::new()
        );
        assert!(ConfigurationAcknowledged::decode(&[0x01]).is_err());
    }

    #[test]
    fn ping_pong_round_trip() {
        let request = PlayPingRequest { id: 42 };
        let bytes = request.encode().expect("encodes");
        assert_eq!(PlayPingRequest::decode(&bytes).expect("decodes"), request);

        let answer = PlayPong { id: 42 };
        let bytes = answer.encode().expect("encodes");
        assert_eq!(PlayPong::decode(&bytes).expect("decodes"), answer);
    }

    #[test]
    fn player_position_round_trip() {
        let packet = PlayerPosition {
            x: 1.5,
            y: 64.0,
            z: -7.25,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw: 90.0,
            pitch: 10.0,
            flags: 0,
            teleport_id: 3,
        };
        assert_eq!(
            PlayerPosition::decode(&packet.encode().expect("encodes")).expect("decodes"),
            packet
        );
    }

    #[test]
    fn play_intent_decodes_movement_and_ignores_unknown() {
        let mut writer = crate::wire::PacketWriter::new();
        writer.write_f64(0.5);
        writer.write_f64(64.0);
        writer.write_f64(-0.5);
        writer.write_f32(45.0);
        writer.write_f32(-10.0);
        writer.write_bool(true);
        let bytes = writer.finish();
        let intent = PlayIntent::decode(crate::ids::serverbound::play::MOVE_PLAYER_POS_ROT, &bytes)
            .expect("decodes")
            .expect("recognized");
        assert!(matches!(intent, PlayIntent::MovePlayerPosRot { .. }));

        assert!(
            PlayIntent::decode(999, &[])
                .expect("unknown is ignored")
                .is_none()
        );
        assert!(
            PlayIntent::decode(crate::ids::serverbound::play::MOVE_PLAYER_POS, &[0x00]).is_err()
        );
    }

    #[test]
    // AUDIT-15: the close window id is vanilla `CONTAINER_ID` (`VarInt`,
    // same codec as the clientbound close). A two-byte id must decode to
    // 128, not read one byte and drop the other.
    fn container_close_reads_a_varint_window_id() {
        let intent = PlayIntent::decode(
            crate::ids::serverbound::play::CONTAINER_CLOSE,
            &[0x80, 0x01],
        )
        .expect("decodes")
        .expect("recognized");
        assert!(
            matches!(intent, PlayIntent::ContainerClose { window_id: 128 }),
            "{intent:?}"
        );
        assert_eq!(
            PlayIntent::decode(crate::ids::serverbound::play::CONTAINER_CLOSE, &[0x05])
                .expect("decodes")
                .expect("recognized"),
            PlayIntent::ContainerClose { window_id: 5 }
        );
    }

    #[test]
    // The real shape, from 911 captured bodies of a real 26.1.2 client (the
    // chat-capture and vanilla-capture sessions): every one was empty.
    fn a_real_clients_tick_end_is_empty_and_recognized() {
        assert!(
            matches!(
                PlayIntent::decode(crate::ids::serverbound::play::CLIENT_TICK_END, &[])
                    .expect("decodes")
                    .expect("recognized"),
                PlayIntent::ClientTickEnd
            ),
            "the captured shape is an empty payload"
        );
        // A payload that is not what the captures carry is a protocol error,
        // not a silent truncation.
        assert!(
            PlayIntent::decode(crate::ids::serverbound::play::CLIENT_TICK_END, &[0x00]).is_err()
        );
    }

    #[test]
    fn chat_decodes_the_full_signed_payload() {
        use crate::wire::PacketWriter;
        let mut writer = PacketWriter::new();
        writer.write_string("hello world").expect("string");
        writer.write_i64(1_700_000_000_000);
        writer.write_i64(-42);
        writer.write_u8(0); // no signature
        writer.write_varint(0); // no last-seen entries
        let bytes = writer.finish();
        let intent = PlayIntent::decode(crate::ids::serverbound::play::CHAT, &bytes)
            .expect("decodes")
            .expect("recognized");
        assert_eq!(
            intent,
            PlayIntent::Chat {
                message: "hello world".to_owned(),
                timestamp_millis: 1_700_000_000_000,
                salt: -42,
                signed: false,
                last_seen_count: 0,
            }
        );

        // With a signature and two last-seen entries.
        let mut writer = PacketWriter::new();
        writer.write_string("signed").expect("string");
        writer.write_i64(1);
        writer.write_i64(2);
        writer.write_u8(1);
        writer.write_bytes(&[0xAB; SIGNATURE_LEN]);
        writer.write_varint(2);
        for _ in 0..2 {
            writer.write_varint(7);
            writer.write_bytes(&[0xCD; SIGNATURE_LEN]);
        }
        let bytes = writer.finish();
        let intent = PlayIntent::decode(crate::ids::serverbound::play::CHAT, &bytes)
            .expect("decodes")
            .expect("recognized");
        assert_eq!(
            intent,
            PlayIntent::Chat {
                message: "signed".to_owned(),
                timestamp_millis: 1,
                salt: 2,
                signed: true,
                last_seen_count: 2,
            }
        );
    }

    #[test]
    fn chat_rejects_a_hostile_last_seen_count() {
        use crate::wire::PacketWriter;
        let mut writer = PacketWriter::new();
        writer.write_string("x").expect("string");
        writer.write_i64(0);
        writer.write_i64(0);
        writer.write_u8(0);
        writer.write_varint(1_000_000); // absurd
        let bytes = writer.finish();
        assert!(
            PlayIntent::decode(crate::ids::serverbound::play::CHAT, &bytes).is_err(),
            "an out-of-range last-seen count must be rejected, not looped over"
        );
    }

    #[test]
    fn chat_command_allows_vanilla_length_commands() {
        use crate::wire::PacketWriter;
        let command = "x".repeat(COMMAND_MAX_CHARS);
        let mut writer = PacketWriter::new();
        writer.write_string(&command).expect("string");
        let bytes = writer.finish();
        let intent = PlayIntent::decode(crate::ids::serverbound::play::CHAT_COMMAND, &bytes)
            .expect("decodes")
            .expect("recognized");
        assert_eq!(
            intent,
            PlayIntent::ChatCommand {
                command: command.clone()
            }
        );
        assert_eq!(command.chars().count(), COMMAND_MAX_CHARS);
    }

    #[test]
    fn interaction_intents_decode() {
        use crate::ids::serverbound::play as ids;
        use crate::wire::PacketWriter;

        let mut writer = PacketWriter::new();
        writer.write_varint(0);
        writer.write_i64(1234);
        writer.write_varint(1);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        writer.write_f32(0.5);
        writer.write_bool(false);
        writer.write_varint(9);
        let bytes = writer.finish();
        let intent = PlayIntent::decode(ids::USE_ITEM_ON, &bytes)
            .expect("decodes")
            .expect("recognized");
        assert_eq!(
            intent,
            PlayIntent::UseItemOn {
                hand: 0,
                position: 1234,
                face: 1,
                cursor_x: 0.5,
                cursor_y: 0.5,
                cursor_z: 0.5,
                inside_block: false,
                sequence: 9,
            }
        );

        let mut writer = PacketWriter::new();
        writer.write_varint(1);
        let bytes = writer.finish();
        assert_eq!(
            PlayIntent::decode(ids::SWING, &bytes)
                .expect("decodes")
                .expect("recognized"),
            PlayIntent::Swing { hand: 1 }
        );

        let mut writer = PacketWriter::new();
        writer.write_i16(4);
        let bytes = writer.finish();
        assert_eq!(
            PlayIntent::decode(ids::SET_CARRIED_ITEM, &bytes)
                .expect("decodes")
                .expect("recognized"),
            PlayIntent::SetCarriedItem { slot: 4 }
        );

        // Truncated payloads are errors, not panics.
        assert!(PlayIntent::decode(ids::USE_ITEM_ON, &[0x00]).is_err());
        assert!(PlayIntent::decode(ids::SET_CARRIED_ITEM, &[0x01]).is_err());
    }

    // -----------------------------------------------------------------------
    // Phase 04: block positions, chunks/blocks, entity + inventory state
    // -----------------------------------------------------------------------

    use super::{
        BIOMES_PER_SECTION, BLOCKS_PER_SECTION, BlockUpdate, ChunkBlockEntity, ChunkSection,
        ContainerSetContent, ContainerSetSlot, GameEvent, HEIGHTMAP_MOTION_BLOCKING, Heightmap,
        ItemStack, LevelChunkWithLight, METADATA_TERMINATOR, MetadataValue, NETWORK_BIOME_MIN_BITS,
        OVERWORLD_SECTIONS, PalettedContainer, Respawn, SectionBlocksUpdate,
        SetDefaultSpawnPosition, SetEntityData, SetExperience, SetHealth, SetHeldSlot, SetTime,
        SystemChat, block_position, unpack_block_position,
    };
    use crate::nbt::Nbt;
    use crate::wire::PacketWriter;
    use mc_core::packing;

    /// A section whose whole volume is one block state and one biome, i.e. the
    /// single-value paletted form (`bits == 0`, no data array).
    fn uniform_section(block_state: u32, biome: u32) -> ChunkSection {
        ChunkSection {
            block_count: 0,
            fluid_count: 0,
            // One palette entry, so every slot holds the index **`0`** — not the id the entry carries. The
            // two are the same number only for air, which is why this fixture and the decoder it was written
            // against could both be wrong and the suite stay green.
            block_states: PalettedContainer::new(
                vec![block_state],
                vec![0; BLOCKS_PER_SECTION],
                packing::BLOCK_MIN_BITS,
            ),
            biomes: PalettedContainer::new(
                vec![biome],
                vec![0; BIOMES_PER_SECTION],
                NETWORK_BIOME_MIN_BITS,
            ),
        }
    }

    /// A full overworld column: one real section followed by 23 empty ones,
    /// because the blob carries no count and a decoder always reads 24.
    fn overworld_sections() -> Vec<ChunkSection> {
        let mut sections: Vec<ChunkSection> = (0..OVERWORLD_SECTIONS)
            .map(|_| uniform_section(0, 1))
            .collect();
        sections[0] = uniform_section(9, 1);
        sections
    }
    /// A chunk with a full section column, one block entity, one sky-light
    /// array for light section 0 (mask bit 1) and no block light.
    fn sample_chunk() -> LevelChunkWithLight {
        LevelChunkWithLight {
            chunk_x: -3,
            chunk_z: 12,
            heightmaps: vec![Heightmap {
                kind: super::HEIGHTMAP_WORLD_SURFACE,
                data: vec![7, 0, 1],
            }],
            sections: overworld_sections(),
            block_entities: vec![ChunkBlockEntity {
                packed_xz: 5,
                y: 70,
                type_id: 4,
                data: Nbt::Compound(vec![(
                    "id".to_owned(),
                    Nbt::String("minecraft:chest".to_owned()),
                )]),
            }],
            sky_light_mask: vec![1],
            block_light_mask: Vec::new(),
            empty_sky_light_mask: Vec::new(),
            empty_block_light_mask: Vec::new(),
            sky_light: vec![vec![0xAA; super::LIGHT_ARRAY_BYTES]],
            block_light: Vec::new(),
        }
    }

    #[test]
    fn block_position_round_trips_positive_and_negative_coordinates() {
        for (x, y, z) in [
            (0, 0, 0),
            (1, 64, -1),
            (-1, -1, -1),
            (33_554_431, 2047, 33_554_431),
            (-33_554_432, -2048, -33_554_432),
            (-1000, 255, 1000),
        ] {
            assert_eq!(
                unpack_block_position(block_position(x, y, z)),
                (x, y, z),
                "({x}, {y}, {z}) must survive the packed form"
            );
        }
    }

    #[test]
    fn block_position_matches_the_vanilla_bit_layout() {
        // ((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF).
        // For (1, 2, 3): (1 << 38) | (3 << 12) | 2
        //              = 274_877_906_944 + 12_288 + 2
        //              = 274_877_919_234.
        assert_eq!(block_position(1, 2, 3), 274_877_919_234);
        // Each field lands only in its own bits, so the three fields cannot
        // bleed into one another.
        assert_eq!(block_position(1, 0, 0), 1_i64 << 38);
        assert_eq!(block_position(0, 0, 1), 1_i64 << 12);
        assert_eq!(block_position(0, 1, 0), 1);
        // Field boundaries, each checked against the exact bit it must set. The
        // x field is 26 bits at 38..63 and z is 26 bits at 12..37, so each
        // field's own sign bit is 25 bits above its base; y is 12 bits at 0..11.
        assert_eq!(block_position(-33_554_432, 0, 0), i64::MIN, "x sign bit");
        assert_eq!(block_position(33_554_431, 0, 0), 0x1FF_FFFF_i64 << 38);
        assert_eq!(block_position(0, -2048, 0), 1 << 11, "y sign bit");
        assert_eq!(block_position(0, 2047, 0), 0x7FF);
        assert_eq!(block_position(0, 0, -33_554_432), 1 << 37, "z sign bit");
        assert_eq!(block_position(0, 0, 33_554_431), 0x1FF_FFFF_i64 << 12);
        // A container id of -1 fills every field, which is the all-ones word.
        assert_eq!(block_position(-1, -1, -1) as u64, u64::MAX);
        // Negative coordinates sign-extend, not zero-extend.
        assert_eq!(
            unpack_block_position(block_position(-1, -1, -1)),
            (-1, -1, -1)
        );
        assert_eq!(unpack_block_position(0), (0, 0, 0));
    }

    #[test]
    fn block_update_golden_bytes() {
        // block_position(1, 64, -1) = (1 << 38) | ((-1 & 0x3FFFFFF) << 12) | 64
        //                           = 274_877_906_944 + 274_877_902_848 + 64
        //                           = 549_755_809_856
        //                           = 0x0000_007F_FFFF_F040.
        assert_eq!(block_position(1, 64, -1), 549_755_809_856);
        let packet = BlockUpdate {
            position: block_position(1, 64, -1),
            block_state: 14,
        };
        let body = packet.encode().expect("encodes");
        let expected = [
            0x00, 0x00, 0x00, 0x7F, 0xFF, 0xFF, 0xF0, 0x40, // packed position
            0x0E, // VarInt 14
        ];
        assert_eq!(body, expected);
        assert_eq!(BlockUpdate::decode(&expected).expect("decodes"), packet);

        // The packed form decomposes back to the coordinates it came from, so
        // the golden bytes above are not merely self-consistent.
        assert_eq!(unpack_block_position(549_755_809_856), (1, 64, -1));

        // Framing pairs the body with the id from the jar-extracted table.
        let raw = packet.to_raw().expect("raw");
        assert_eq!(raw.id, BlockUpdate::ID);
        assert_eq!(raw.id, 8);
        assert_eq!(raw.payload, expected);
    }

    #[test]
    fn block_destruction_round_trip() {
        // P16-05: VarInt entity, packed position, i8 stage — the shape
        // pumpkin's CSetBlockDestroyStage corroborates. 0xFF decodes as -1,
        // the overlay-clearing stage, which the dig path sends on abort.
        let packet = BlockDestruction {
            entity_id: 7,
            position: block_position(1, 64, -1),
            stage: 4,
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(body.len(), 1 + 8 + 1);
        assert_eq!(BlockDestruction::decode(&body).expect("decodes"), packet);
        let clearing = BlockDestruction {
            entity_id: 7,
            position: block_position(1, 64, -1),
            stage: -1,
        };
        let clearing_body = clearing.encode().expect("encodes");
        assert_eq!(clearing_body[clearing_body.len() - 1], 0xFF);
        assert_eq!(
            BlockDestruction::decode(&clearing_body).expect("decodes"),
            clearing
        );
        assert!(BlockDestruction::decode(&[0x07]).is_err());
        let raw = packet.to_raw().expect("raw");
        assert_eq!(raw.id, BlockDestruction::ID);
        assert_eq!(raw.id, 5);
    }

    #[test]
    fn section_blocks_update_round_trip() {
        let packet = SectionBlocksUpdate {
            updates: vec![
                BlockUpdate {
                    position: block_position(0, 64, 0),
                    block_state: 1,
                },
                BlockUpdate {
                    position: block_position(15, 65, 15),
                    block_state: 4096,
                },
            ],
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(body[0], 0x02, "leading count is 2");
        assert_eq!(SectionBlocksUpdate::decode(&body).expect("decodes"), packet);
        // One position plus one VarInt per entry after the count.
        assert_eq!(body.len(), 1 + (8 + 1) + (8 + 2));

        assert!(SectionBlocksUpdate::decode(&[]).is_err(), "no count");
        assert!(
            SectionBlocksUpdate::decode(&[0x02, 0x00]).is_err(),
            "count larger than the entries present"
        );
    }

    /// Golden bytes for the single-value form. A uniform container is *not*
    /// written as a 4-bit palette of one with an index array: `bits = 0` means
    /// one global palette id and nothing else.
    #[test]
    fn paletted_container_single_value_golden_bytes() {
        // blocks = minecraft:air (state 0), biomes = minecraft:plains (id 1).
        let container = PalettedContainer::new(
            vec![0],
            vec![0; BLOCKS_PER_SECTION],
            packing::BLOCK_MIN_BITS,
        );
        assert_eq!(container.bits, 0);
        let mut writer = PacketWriter::new();
        container.encode(&mut writer).expect("encodes");
        assert_eq!(writer.finish(), [0x00, 0x00]);

        let mut reader = crate::wire::PacketReader::new(&[0x00, 0x00]);
        let decoded =
            PalettedContainer::decode(&mut reader, BLOCKS_PER_SECTION, packing::BLOCK_MIN_BITS)
                .expect("decodes");
        assert_eq!(decoded, container);
        assert!(reader.is_empty());
    }

    /// Golden bytes for the multi-entry form: an all-zero 40-cell container on a
    /// 2-entry palette packs into 3 longs at the 4-bit minimum (16 values per
    /// long, `ceil(40 / 16) = 3`) and, per `writeFixedSizeLongArray`, those longs
    /// follow the palette with **no length prefix**. Pinned as bits, palette,
    /// then exactly 24 zero bytes, so both a stray length prefix and a
    /// byte-order regression fail here.
    #[test]
    fn paletted_container_multi_entry_golden_bytes() {
        let palette = vec![0, 5];
        let container =
            PalettedContainer::new(palette.clone(), vec![0; 40], packing::BLOCK_MIN_BITS);
        assert_eq!(container.bits, 4);
        let mut writer = PacketWriter::new();
        container.encode(&mut writer).expect("encodes");
        let body = writer.finish();
        assert_eq!(
            &body[..4],
            &[
                0x04, // 4 bits per entry
                0x02, // palette length
                0x00, 0x05, // palette ids
            ]
        );
        assert_eq!(
            body.len(),
            4 + 3 * 8,
            "the 3 storage longs are raw: no VarInt length in front of them"
        );
        assert!(
            body[4..].iter().all(|byte| *byte == 0),
            "an all-index-0 container must pack to zero longs"
        );

        let mut reader = crate::wire::PacketReader::new(&body);
        let decoded =
            PalettedContainer::decode(&mut reader, 40, packing::BLOCK_MIN_BITS).expect("decodes");
        assert_eq!(decoded.palette, palette);
        assert_eq!(decoded.values, vec![0; 40]);
        assert!(reader.is_empty());
    }

    #[test]
    fn paletted_container_round_trips_a_multi_entry_palette() {
        let palette = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let values: Vec<u32> = (0..BLOCKS_PER_SECTION)
            .map(|index| (index % palette.len()) as u32)
            .collect();
        let container = PalettedContainer::new(palette, values, packing::BLOCK_MIN_BITS);
        assert_eq!(container.bits, 5, "17 palette entries need 5 bits");
        let mut writer = PacketWriter::new();
        container.encode(&mut writer).expect("encodes");
        let body = writer.finish();

        let mut reader = crate::wire::PacketReader::new(&body);
        let decoded =
            PalettedContainer::decode(&mut reader, BLOCKS_PER_SECTION, packing::BLOCK_MIN_BITS)
                .expect("decodes");
        assert_eq!(decoded, container);
        assert!(reader.is_empty());
    }

    #[test]
    fn biome_minimum_bit_width_is_two_on_the_network() {
        // A 2-entry biome palette at the network minimum: 2 bits, 32 values per
        // long, `ceil(64 / 32) = 2` longs. At the disk minimum of 1 it would be
        // 1 bit and 1 long, so this pins the network-specific value.
        let palette = vec![1, 2];
        let container = PalettedContainer::new(
            palette.clone(),
            vec![1; BIOMES_PER_SECTION],
            NETWORK_BIOME_MIN_BITS,
        );
        assert_eq!(container.bits, 2);
        let mut writer = PacketWriter::new();
        container.encode(&mut writer).expect("encodes");
        let body = writer.finish();
        assert_eq!(&body[..4], &[0x02, 0x02, 0x01, 0x02]);
        assert_eq!(body.len(), 4 + 2 * 8);

        let mut reader = crate::wire::PacketReader::new(&body);
        let decoded =
            PalettedContainer::decode(&mut reader, BIOMES_PER_SECTION, NETWORK_BIOME_MIN_BITS)
                .expect("decodes");
        assert_eq!(decoded, container);

        // The same bytes read with the disk minimum are rejected rather than
        // silently reinterpreted.
        let mut reader = crate::wire::PacketReader::new(&body);
        assert!(
            PalettedContainer::decode(&mut reader, BIOMES_PER_SECTION, packing::BIOME_MIN_BITS)
                .is_err()
        );
    }

    #[test]
    fn paletted_container_rejects_the_single_value_id_zero_and_one() {
        // bits = 0, single palette id 1: 64 biome cells, every one of them **index 0** into that one-entry
        // palette. Holding the id here instead was the decoder's bug, restated as an expectation.
        let mut reader = crate::wire::PacketReader::new(&[0x00, 0x01]);
        let container =
            PalettedContainer::decode(&mut reader, BIOMES_PER_SECTION, NETWORK_BIOME_MIN_BITS)
                .expect("decodes");
        assert_eq!(container.values, vec![0; BIOMES_PER_SECTION]);
        assert_eq!(container.bits, 0);
    }

    #[test]
    fn chunk_data_round_trips_and_matches_the_wire_shape() {
        let sections = vec![uniform_section(0, 1)];
        let blob = LevelChunkWithLight::encode_chunk_data(&sections).expect("encodes");
        assert_eq!(
            blob,
            [
                0x00, 0x00, // block count 0
                0x00, 0x00, // fluid count 0
                0x00, 0x00, // block states: bits 0, palette id 0
                0x00, 0x01, // biomes: bits 0, palette id 1
            ]
        );
        assert_eq!(
            LevelChunkWithLight::decode_chunk_data(&blob, 1).expect("decodes"),
            sections
        );
    }

    #[test]
    fn chunk_data_has_no_section_count_prefix() {
        // Two sections cost exactly twice one, and the first four bytes of the
        // blob are the first section's counters: if a count prefix were written
        // the leading byte would be 0x02 instead of the block count.
        let one =
            LevelChunkWithLight::encode_chunk_data(&[uniform_section(0, 1)]).expect("encodes");
        let two =
            LevelChunkWithLight::encode_chunk_data(&[uniform_section(0, 1), uniform_section(0, 1)])
                .expect("encodes");
        assert_eq!(two.len(), 2 * one.len());
        assert_eq!(&two[..one.len()], &one[..]);
        assert_eq!(
            two[0], 0x00,
            "first byte is the block count, not a count prefix"
        );
    }

    #[test]
    fn level_chunk_with_light_round_trip() {
        let packet = sample_chunk();
        let body = packet.encode().expect("encodes");
        assert_eq!(body[0..4], [0xFF, 0xFF, 0xFF, 0xFD], "chunk x is -3");
        assert_eq!(body[4..8], [0x00, 0x00, 0x00, 0x0C], "chunk z is 12");
        // Heightmaps: one entry, id HEIGHTMAP_WORLD_SURFACE (1), 3 longs.
        assert_eq!(&body[8..11], &[0x01, 0x01, 0x03]);
        assert_eq!(LevelChunkWithLight::decode(&body).expect("decodes"), packet);

        let raw = packet.to_raw().expect("raw");
        assert_eq!(raw.id, LevelChunkWithLight::ID);
        assert_eq!(raw.id, 45);
    }

    #[test]
    fn level_chunk_with_light_light_mask_carries_full_array_count() {
        let mut packet = sample_chunk();
        packet.sky_light_mask = vec![0, 2];
        packet.sky_light = vec![
            vec![0x00; super::LIGHT_ARRAY_BYTES],
            vec![0xFF; super::LIGHT_ARRAY_BYTES],
        ];
        let body = packet.encode().expect("encodes");
        assert_eq!(LevelChunkWithLight::decode(&body).expect("decodes"), packet);
        // The tail is: sky array count, then each array as `VarInt 2048` + the
        // 2048 bytes, then the block array count. Every length prefix is
        // explicit rather than derived from the array.
        let sky_count = 1;
        let count_len = 2; // VarInt 2048
        let tail_len = sky_count + 2 * (count_len + super::LIGHT_ARRAY_BYTES) + 1;
        let tail = &body[body.len() - tail_len..];
        assert_eq!(tail[0], 0x02, "two sky-light arrays follow");
        assert_eq!(&tail[1..3], &[0x80, 0x10], "VarInt 2048");
        assert!(
            tail[3..3 + super::LIGHT_ARRAY_BYTES]
                .iter()
                .all(|b| *b == 0x00)
        );
        let second = 3 + super::LIGHT_ARRAY_BYTES;
        assert_eq!(&tail[second..second + 2], &[0x80, 0x10], "VarInt 2048");
        assert!(
            tail[second + 2..second + 2 + super::LIGHT_ARRAY_BYTES]
                .iter()
                .all(|b| *b == 0xFF)
        );
        assert_eq!(
            *tail.last().expect("non-empty"),
            0x00,
            "no block-light arrays"
        );
    }

    #[test]
    fn level_chunk_with_light_rejects_a_truncated_light_array() {
        let mut packet = sample_chunk();
        packet.sky_light = vec![vec![0xAA; super::LIGHT_ARRAY_BYTES - 1]];
        assert!(packet.encode().is_err(), "a short array is a server bug");

        // And on the wire: correct mask and count, an array that ends early.
        let mut writer = PacketWriter::new();
        writer.write_varint(1);
        writer.write_varint(super::LIGHT_ARRAY_BYTES as i32);
        writer.write_bytes(&[0xAA; super::LIGHT_ARRAY_BYTES - 1]);
        assert!(
            super::light::decode_light_arrays(
                &mut crate::wire::PacketReader::new(&writer.finish()),
                &[0]
            )
            .is_err()
        );
    }

    #[test]
    fn level_chunk_with_light_rejects_a_light_mask_count_mismatch() {
        // Mask says two sections have arrays; one array follows.
        let mut writer = PacketWriter::new();
        writer.write_varint(1);
        writer.write_varint(super::LIGHT_ARRAY_BYTES as i32);
        writer.write_bytes(&[0x00; super::LIGHT_ARRAY_BYTES]);
        assert!(
            super::light::decode_light_arrays(
                &mut crate::wire::PacketReader::new(&writer.finish()),
                &[0, 1]
            )
            .is_err(),
            "arrays carry no section index, so the count must match the mask"
        );
    }

    #[test]
    fn level_chunk_with_light_hostile_inputs_are_errors_not_panics() {
        let body = sample_chunk().encode().expect("encodes");
        // Every truncation of a valid packet must fail, never panic.
        for len in 0..body.len() {
            assert!(
                LevelChunkWithLight::decode(&body[..len]).is_err(),
                "a payload truncated to {len} bytes must be rejected"
            );
        }
    }

    #[test]
    fn chunk_decode_rejects_an_absurd_palette_length() {
        // One section: block count 0, fluid count 0, then block states declaring
        // a 4-bit-wide palette of 100_000 entries with nothing behind it.
        let blob = [
            0x00, 0x00, // block count 0
            0x00, 0x00, // fluid count 0
            0x04, // bits = 4
            0xA0, 0x8D, 0x06, // VarInt 100_000 palette entries
        ];
        assert!(
            LevelChunkWithLight::decode_chunk_data(&blob, 1).is_err(),
            "an absurd palette length must be refused, not allocated for"
        );
    }

    #[test]
    fn chunk_decode_rejects_a_truncated_long_array() {
        // A self-consistent 4-bit header for a 2-entry block-state palette.
        // `longs_needed(4096, 4)` is 256, so a container cut short of that is
        // malformed — and the count is not on the wire to be trusted instead.
        let header = [
            0x00, 0x00, // block count 0
            0x00, 0x00, // fluid count 0
            0x04, // 4 bits per entry
            0x02, // palette length 2
            0x00, 0x05, // palette ids
        ];
        let mut one_long = header.to_vec();
        one_long.extend_from_slice(&0_i64.to_be_bytes());
        assert!(
            LevelChunkWithLight::decode_chunk_data(&one_long, 1).is_err(),
            "a 4096-cell container cannot be indexed by a single long"
        );

        // The same header with exactly the right array, then a biome container,
        // decodes — so the rejection above is about the array length only.
        let mut valid = header.to_vec();
        valid.extend(std::iter::repeat_n(0_u8, 256 * 8));
        valid.extend_from_slice(&[0x00, 0x01]); // biomes: single-value id 1
        assert_eq!(
            LevelChunkWithLight::decode_chunk_data(&valid, 1).expect("decodes"),
            vec![ChunkSection {
                block_count: 0,
                fluid_count: 0,
                block_states: PalettedContainer {
                    palette: vec![0, 5],
                    values: vec![0; BLOCKS_PER_SECTION],
                    bits: 4,
                },
                biomes: PalettedContainer {
                    palette: vec![1],
                    // **Index `0`, not the id.** The golden bytes carry a single-value palette of id `1`, so
                    // every slot indexes that one entry; this expectation used to hold `1` in every slot, which
                    // is the same misunderstanding the decoder had, and it is why the suite stayed green.
                    values: vec![0; BIOMES_PER_SECTION],
                    bits: 0,
                },
            }]
        );
    }

    #[test]
    fn chunk_decode_rejects_out_of_range_section_counts() {
        // A section whose non-air count exceeds the 4096 blocks it can hold.
        let over = [0x10, 0x01, 0x00, 0x00]; // i16 0x1001 = 4097
        assert!(
            LevelChunkWithLight::decode_chunk_data(&over, 1).is_err(),
            "block count 4097 is outside 0..=4096"
        );
        // And the fluid count is checked the same way: 0x1001 in the second
        // short, with a legal block count in front of it.
        let fluid_over = [0x00, 0x00, 0x10, 0x01];
        assert!(
            LevelChunkWithLight::decode_chunk_data(&fluid_over, 1).is_err(),
            "fluid count 4097 is outside 0..=4096"
        );
        // The largest legal counters still parse (and then fail for the missing
        // containers, not for the counters).
        let legal = [0x10, 0x00, 0x10, 0x00];
        assert!(LevelChunkWithLight::decode_chunk_data(&legal, 1).is_err());
    }

    #[test]
    fn chunk_decode_rejects_trailing_bytes() {
        let mut blob =
            LevelChunkWithLight::encode_chunk_data(&[uniform_section(0, 1)]).expect("encodes");
        assert!(LevelChunkWithLight::decode_chunk_data(&blob, 1).is_ok());
        blob.push(0x00);
        assert!(
            LevelChunkWithLight::decode_chunk_data(&blob, 1).is_err(),
            "a self-contained blob must be consumed exactly"
        );
        // A blob for one section read as two runs out of bytes.
        let one =
            LevelChunkWithLight::encode_chunk_data(&[uniform_section(0, 1)]).expect("encodes");
        assert!(LevelChunkWithLight::decode_chunk_data(&one, 2).is_err());
    }

    #[test]
    fn level_chunk_with_light_rejects_a_hostile_heightmap_length() {
        // One heightmap whose long count is absurd, with no longs behind it.
        let mut writer = PacketWriter::new();
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_varint(1); // one heightmap
        writer.write_varint(HEIGHTMAP_MOTION_BLOCKING);
        writer.write_varint(i32::MAX);
        assert!(LevelChunkWithLight::decode(&writer.finish()).is_err());

        // An absurd heightmap *count* is refused before any allocation.
        let mut writer = PacketWriter::new();
        writer.write_i32(0);
        writer.write_i32(0);
        writer.write_varint(1_000_000);
        assert!(LevelChunkWithLight::decode(&writer.finish()).is_err());
    }

    #[test]
    fn level_chunk_with_light_accepts_the_phase_04_zero_light_form() {
        // What Phase 04 actually sends: four zero masks and no arrays.
        let mut packet = sample_chunk();
        packet.sky_light_mask = Vec::new();
        packet.block_light_mask = Vec::new();
        packet.empty_sky_light_mask = Vec::new();
        packet.empty_block_light_mask = Vec::new();
        packet.sky_light = Vec::new();
        packet.block_light = Vec::new();
        let body = packet.encode().expect("encodes");
        assert_eq!(LevelChunkWithLight::decode(&body).expect("decodes"), packet);
        // ...ends with five zero VarInts: four masks and two array counts.
        let tail = &body[body.len() - 6..];
        assert_eq!(tail, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn level_chunk_with_light_rejects_a_hostile_light_mask() {
        // A negative mask is not a valid bitset for this field: the five bytes
        // are the 5-byte VarInt form of -1.
        let negative = [0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        assert!(
            super::light::read_light_mask(&mut crate::wire::PacketReader::new(&negative), "sky")
                .is_err()
        );

        // A mask that sets bits above the last light section is refused too.
        // A `BitSet` that sets a bit beyond the light section range is refused. Under the wire model the mask
        // is a long count plus longs (KD-44), so the offending bit lives in a long rather than an integer.
        let mut beyond = PacketWriter::new();
        beyond.write_varint(2);
        beyond.write_i64(0);
        beyond.write_i64(1 << 40); // section 64 + 40 = 104
        let beyond = beyond.finish();
        assert!(
            super::light::read_light_mask(&mut crate::wire::PacketReader::new(&beyond), "sky")
                .is_err()
        );

        // A hostile long count is refused before anything is allocated.
        let mut greedy = PacketWriter::new();
        greedy.write_varint(99);
        let greedy = greedy.finish();
        assert!(
            super::light::read_light_mask(&mut crate::wire::PacketReader::new(&greedy), "sky")
                .is_err()
        );

        // **The empty mask is a single `0x00` byte in both encodings**, which is the whole of KD-44: reading
        // the masks as `VarInt`s agrees with the real wire for an empty mask and disagrees for any other, so
        // the defect survived until a server sent one with content.
        assert_eq!(
            super::light::read_light_mask(&mut crate::wire::PacketReader::new(&[0x00]), "sky")
                .expect("empty mask"),
            Vec::<u32>::new()
        );

        // A mask with content decodes to its set indices.
        let mut two = PacketWriter::new();
        two.write_varint(1);
        two.write_i64(0b110);
        let two = two.finish();
        assert_eq!(
            super::light::read_light_mask(&mut crate::wire::PacketReader::new(&two), "sky")
                .expect("two sections"),
            vec![1, 2]
        );

        // And it survives a round trip through the writer, including the trailing-zero trim that makes the
        // output byte-identical to vanilla's `BitSet.toLongArray()`.
        let mut round_trip = PacketWriter::new();
        super::light::write_light_mask(&mut round_trip, &[1, 2]);
        let round_trip = round_trip.finish();
        assert_eq!(round_trip, two, "a mask must encode exactly as it was read");
    }

    /// Pins the six `Heightmap.Types` ids to `docs/protocol/heightmap-types.tsv`.
    ///
    /// These are the whole reason heights can render wrongly without any error:
    /// the wire carries an integer, so a wrong constant is silent. There are
    /// exactly six types and no `LIGHT_BLOCKING`.
    #[test]
    fn heightmap_ids_match_the_extracted_table() {
        assert_eq!(super::HEIGHTMAP_WORLD_SURFACE_WG, 0);
        assert_eq!(super::HEIGHTMAP_WORLD_SURFACE, 1);
        assert_eq!(super::HEIGHTMAP_OCEAN_FLOOR_WG, 2);
        assert_eq!(super::HEIGHTMAP_OCEAN_FLOOR, 3);
        assert_eq!(super::HEIGHTMAP_MOTION_BLOCKING, 4);
        assert_eq!(super::HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES, 5);
    }

    #[test]
    fn heightmaps_round_trip_with_an_explicit_long_count() {
        let packet = LevelChunkWithLight {
            heightmaps: vec![
                Heightmap {
                    kind: super::HEIGHTMAP_WORLD_SURFACE,
                    data: vec![-1, 0, 1],
                },
                Heightmap {
                    kind: super::HEIGHTMAP_MOTION_BLOCKING,
                    data: Vec::new(),
                },
            ],
            ..sample_chunk()
        };
        let body = packet.encode().expect("encodes");
        // Two entries: id 1 with 3 longs, then id 4 with 0 longs.
        assert_eq!(&body[8..12], &[0x02, 0x01, 0x03, 0xFF]);
        assert_eq!(LevelChunkWithLight::decode(&body).expect("decodes"), packet);

        // An unknown id survives the round trip rather than being dropped: the
        // client decodes out-of-range ids leniently, so a peer may send one.
        let unknown = LevelChunkWithLight {
            heightmaps: vec![Heightmap {
                kind: 99,
                data: vec![0],
            }],
            ..sample_chunk()
        };
        assert_eq!(
            LevelChunkWithLight::decode(&unknown.encode().expect("encodes")).expect("decodes"),
            unknown
        );
    }

    #[test]
    fn set_health_set_experience_and_set_time_round_trip() {
        let health = SetHealth {
            health: 19.5,
            food: 20,
            saturation: 5.0,
        };
        assert_eq!(
            SetHealth::decode(&health.encode().expect("encodes")).expect("decodes"),
            health
        );

        let experience = SetExperience {
            progress: 0.25,
            level: 30,
            total: 1395,
        };
        assert_eq!(
            SetExperience::decode(&experience.encode().expect("encodes")).expect("decodes"),
            experience
        );

        let time = SetTime {
            world_age: 12_345,
            clocks: Vec::new(),
        };
        assert_eq!(
            SetTime::decode(&time.encode().expect("encodes")).expect("decodes"),
            time
        );
        assert!(
            SetTime::decode(&time.encode().expect("encodes")[..8]).is_err(),
            "a payload cut mid-field is an error"
        );
    }

    #[test]
    fn set_held_slot_game_event_and_spawn_position_round_trip() {
        let held = SetHeldSlot { slot: 8 };
        assert_eq!(
            SetHeldSlot::decode(&held.encode().expect("encodes")).expect("decodes"),
            held
        );

        let event = GameEvent {
            event: 1,
            value: 0.0,
        };
        assert_eq!(
            GameEvent::decode(&event.encode().expect("encodes")).expect("decodes"),
            event
        );
        assert_eq!(
            event.encode().expect("encodes"),
            [0x01, 0x00, 0x00, 0x00, 0x00],
            "u8 id then f32 value"
        );

        let spawn = SetDefaultSpawnPosition {
            dimension: "minecraft:overworld".to_owned(),
            position: block_position(0, -60, 0),
            yaw: 0.0,
            pitch: 0.0,
        };
        assert_eq!(
            SetDefaultSpawnPosition::decode(&spawn.encode().expect("encodes")).expect("decodes"),
            spawn
        );
        assert!(SetDefaultSpawnPosition::decode(&[0x00; 4]).is_err());
    }

    #[test]
    fn respawn_round_trip_and_rejects_truncation() {
        for death_location in [
            None,
            Some((
                "minecraft:overworld".to_owned(),
                block_position(-12, 66, -6),
            )),
        ] {
            let packet = Respawn {
                dimension_type_id: 0,
                dimension_name: "minecraft:overworld".to_owned(),
                hashed_seed: -4_242_424_242,
                game_mode: 0,
                previous_game_mode: -1,
                is_debug: false,
                is_flat: false,
                death_location,
                portal_cooldown: 0,
                sea_level: 63,
                data_kept: 0,
            };
            let body = packet.encode().expect("encodes");
            assert_eq!(Respawn::decode(&body).expect("decodes"), packet);

            for len in 0..body.len() {
                assert!(
                    Respawn::decode(&body[..len]).is_err(),
                    "truncation at {len} bytes must be rejected"
                );
            }
        }
    }

    /// The client's own reader for `ClientboundRespawnPacket`, transcribed from
    /// the 26.1.2 client jar's bytecode rather than from our encoder.
    ///
    /// This is the test that would have caught M-1. A round trip through
    /// [`Respawn::decode`] cannot: both halves were wrong in the same way, so
    /// they agreed. This reader is derived from the *client's* instructions, so
    /// it fails when our bytes stop matching what a real client consumes.
    ///
    /// `javap -c -p net.minecraft.network.protocol.game.CommonPlayerSpawnInfo`
    /// (read constructor) and `…ClientboundRespawnPacket` (`write`), both on
    /// `26.1.2.jar`:
    ///
    /// ```text
    /// CommonPlayerSpawnInfo(RegistryFriendlyByteBuf):
    ///   DimensionType.STREAM_CODEC.decode(buf)          -> Holder
    ///   buf.readResourceKey(Registries.DIMENSION)       -> ResourceKey
    ///   buf.readLong()                                  -> seed
    ///   buf.readByte()  -> GameType.byId
    ///   buf.readByte()  -> GameType.getNullableId
    ///   buf.readBoolean()                               -> isDebug
    ///   buf.readBoolean()                               -> isFlat
    ///   buf.readOptional(FriendlyByteBuf::readGlobalPos) -> lastDeathLocation
    ///   buf.readVarInt()                                -> portalCooldown
    ///   buf.readVarInt()                                -> seaLevel
    /// ClientboundRespawnPacket(RegistryFriendlyByteBuf):
    ///   new CommonPlayerSpawnInfo(buf); buf.readByte()  -> dataToKeep
    /// ```
    struct ClientReader<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl ClientReader<'_> {
        fn take(&mut self, n: usize) -> &[u8] {
            let slice = &self.bytes[self.at..self.at + n];
            self.at += n;
            slice
        }

        fn varint(&mut self) -> i32 {
            let mut value = 0i32;
            let mut shift = 0;
            loop {
                let byte = self.take(1)[0];
                value |= i32::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return value;
                }
                shift += 7;
            }
        }

        /// `FriendlyByteBuf.readIdentifier`: a `VarInt` length then UTF-8 bytes.
        fn identifier(&mut self) -> String {
            let len = self.varint() as usize;
            String::from_utf8(self.take(len).to_vec()).expect("identifier is UTF-8")
        }

        fn bool_byte(&mut self) -> bool {
            self.take(1)[0] != 0
        }

        fn i64(&mut self) -> i64 {
            i64::from_be_bytes(self.take(8).try_into().expect("8 bytes"))
        }

        fn u8(&mut self) -> u8 {
            self.take(1)[0]
        }

        fn i8(&mut self) -> i8 {
            self.take(1)[0] as i8
        }
    }

    /// Drive the client-shaped reader over our bytes and report the ten spawn-info
    /// fields plus the trailing byte, asserting the reader consumed everything.
    fn read_as_the_client_does(
        body: &[u8],
    ) -> (i32, String, i64, u8, i8, bool, bool, bool, i32, i32, u8) {
        let mut reader = ClientReader { bytes: body, at: 0 };
        // 1. dimension type: the registry-friendly holder writes a VarInt id.
        let dimension_type_id = reader.varint();
        // 2. dimension: `readResourceKey` -> `writeResourceKey` -> `writeIdentifier`.
        let dimension_name = reader.identifier();
        // 3-7.
        let hashed_seed = reader.i64();
        let game_mode = reader.u8();
        let previous_game_mode = reader.i8();
        let is_debug = reader.bool_byte();
        let is_flat = reader.bool_byte();
        // 8. `readOptional(readGlobalPos)`: presence bool, then a ResourceKey and
        //    a packed BlockPos long when present.
        let has_death_location = reader.bool_byte();
        if has_death_location {
            let _dimension = reader.identifier();
            let _position = reader.i64();
        }
        // 9-10.
        let portal_cooldown = reader.varint();
        let sea_level = reader.varint();
        // The packet's own trailing byte.
        let data_kept = reader.u8();
        assert_eq!(
            reader.at,
            body.len(),
            "the client read {} bytes of a {}-byte body",
            reader.at,
            body.len()
        );
        (
            dimension_type_id,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            has_death_location,
            portal_cooldown,
            sea_level,
            data_kept,
        )
    }

    #[test]
    fn respawn_is_exactly_what_the_client_reads() {
        let death_position = block_position(-12, 66, -6);
        let packet = Respawn {
            dimension_type_id: 7,
            dimension_name: "minecraft:overworld".to_owned(),
            hashed_seed: 12_345,
            game_mode: 0,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: false,
            death_location: Some(("minecraft:overworld".to_owned(), death_position)),
            portal_cooldown: 0,
            sea_level: 63,
            data_kept: 3,
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(
            read_as_the_client_does(&body),
            (
                7,
                "minecraft:overworld".to_owned(),
                12_345,
                0,
                -1,
                false,
                false,
                true,
                0,
                63,
                3
            ),
            "the client's read order must land on exactly our field values"
        );

        // And the absent case stays one byte shorter, with the flag false.
        let without = Respawn {
            death_location: None,
            ..packet.clone()
        };
        let without_body = without.encode().expect("encodes");
        assert_eq!(without_body.len() + 1 + 19 + 8, body.len());
        assert!(!read_as_the_client_does(&without_body).7);
    }

    /// The body this packet used to produce: the dimension/seed/mode block, then
    /// `data kept` where `sea level` belongs and the sea level after it.
    ///
    /// Built here rather than reconstructed from the encoder, because the encoder
    /// no longer produces it — that is the point of the two tests below.
    fn old_truncated_respawn_body() -> Vec<u8> {
        let mut writer = crate::wire::PacketWriter::new();
        writer.write_varint(0);
        writer.write_string("minecraft:overworld").expect("writes");
        writer.write_i64(0);
        writer.write_u8(0);
        writer.write_i8(-1);
        writer.write_bool(false);
        writer.write_bool(false);
        writer.write_u8(0); // `data kept`, in the old, wrong position
        writer.write_varint(63);
        writer.finish()
    }

    /// **The falsification anchor for M-1, part one: the client is driven off the
    /// rails rather than merely reaching the wrong values.**
    ///
    /// The old body is 35 bytes and the client's reader consumes all 35 before it
    /// has finished: after reading what it takes for `portalCooldown`, it still
    /// needs `seaLevel` **and** the packet's trailing `dataToKeep` byte, and there
    /// are none left. A real client reports a decoder exception and refuses the
    /// packet, which is what the owner saw as "cannot respawn".
    ///
    /// This is a `#[should_panic]` rather than an assertion because the failure
    /// *is* an out-of-bytes read, and the test says so in its name.
    #[test]
    #[should_panic(expected = "out of range")]
    fn respawn_rejects_the_old_truncated_spawn_info() {
        read_as_the_client_does(&old_truncated_respawn_body());
    }

    /// **The falsification anchor for M-1, part two: where the misalignment lands.**
    ///
    /// Not a duplicate of the test above — that one stops at the point the client
    /// runs out of bytes, this one names *which* of our fields the client reads as
    /// which of its own before it does. Without that, "the packet was too short"
    /// is a reader's inference; with it, the first wrong field is on the record:
    /// the old `data kept` becomes "no death location", and the old `sea level`
    /// becomes the portal cooldown.
    #[test]
    fn the_old_respawn_shape_misaligns_the_clients_fields() {
        let old_shape = old_truncated_respawn_body();
        let mut reader = ClientReader {
            bytes: &old_shape,
            at: 0,
        };
        let _dimension_type_id = reader.varint();
        let _dimension_name = reader.identifier();
        let _seed = reader.i64();
        let _game_mode = reader.u8();
        let _previous = reader.i8();
        let _debug = reader.bool_byte();
        let _flat = reader.bool_byte();
        let has_death_location = reader.bool_byte(); // our old `data kept` = 0
        assert!(
            !has_death_location,
            "the old shape's `data kept` byte is read as the death-location flag"
        );
        let portal_cooldown = reader.varint(); // our old `sea level` = 63
        assert_eq!(
            portal_cooldown, 63,
            "the old shape's sea level is read as the portal cooldown"
        );
        // And the two fields the client still wants are not there.
        assert_eq!(
            old_shape.len() - reader.at,
            0,
            "the client must still read a sea level and the trailing byte; \
             the old shape has neither left"
        );
    }

    #[test]
    fn system_chat_round_trip_and_hostile_input() {
        let packet = SystemChat {
            content: TextComponent::literal("hello"),
            overlay: false,
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(SystemChat::decode(&body).expect("decodes"), packet);

        let action_bar = SystemChat {
            content: TextComponent::literal("+5 XP"),
            overlay: true,
        };
        assert_eq!(
            SystemChat::decode(&action_bar.encode().expect("encodes")).expect("decodes"),
            action_bar
        );

        assert!(SystemChat::decode(&[]).is_err(), "missing NBT root");
        assert!(
            SystemChat::decode(&body[..body.len() - 1]).is_err(),
            "missing overlay flag"
        );
    }

    #[test]
    fn set_entity_data_round_trip_and_golden_bytes() {
        let packet = SetEntityData {
            entity_id: 7,
            entries: vec![
                (8, MetadataValue::Byte(3)),
                (9, MetadataValue::VarInt(300)),
                (10, MetadataValue::Float(1.0)),
            ],
        };
        let body = packet.encode().expect("encodes");
        let expected = [
            0x07, // entity id
            0x08,
            0x00,
            0x03, // index 8, type 0 (byte), value 3
            0x09,
            0x01,
            0xAC,
            0x02, // index 9, type 1 (VarInt), value 300
            0x0A,
            0x03,
            0x3F,
            0x80,
            0x00,
            0x00, // index 10, type 3 (float), 1.0
            METADATA_TERMINATOR,
        ];
        assert_eq!(body, expected);
        assert_eq!(SetEntityData::decode(&expected).expect("decodes"), packet);
    }

    #[test]
    fn set_entity_data_rejects_unterminated_and_unmodelled_payloads() {
        // Missing terminator: the reader runs out of bytes instead of looping.
        let unterminated = [0x07, 0x08, 0x00, 0x03];
        assert!(SetEntityData::decode(&unterminated).is_err());

        // Type id 2 (VarLong) is not modelled: reject rather than misparse.
        let unmodelled = [0x07, 0x08, 0x02, 0x00, METADATA_TERMINATOR];
        assert!(SetEntityData::decode(&unmodelled).is_err());

        // An empty entry list is legal (just the id and the terminator).
        assert_eq!(
            SetEntityData::decode(&[0x07, METADATA_TERMINATOR]).expect("decodes"),
            SetEntityData {
                entity_id: 7,
                entries: Vec::new()
            }
        );
    }

    #[test]
    fn metadata_entry_indices_keep_the_high_bit_clear() {
        // 0xFF is the terminator, so index 255 cannot be encoded as an index.
        let packet = SetEntityData {
            entity_id: 1,
            entries: vec![(0xFE, MetadataValue::Byte(0x80))],
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(body.last(), Some(&METADATA_TERMINATOR));
        assert_eq!(
            SetEntityData::decode(&body).expect("decodes").entries[0].0,
            0xFE
        );
    }

    #[test]
    fn item_stack_empty_and_simple_round_trip() {
        let empty = ItemStack::empty();
        let mut writer = PacketWriter::new();
        assert_eq!(empty.encode(&mut writer).expect("encodes"), 1);
        assert_eq!(writer.finish(), [0x00], "empty is a bare VarInt 0");

        let simple = ItemStack::simple(5, 3);
        let mut writer = PacketWriter::new();
        simple.encode(&mut writer).expect("encodes");
        let body = writer.finish();
        // Vanilla Slot order: count 3, id 5, zero added, zero removed.
        assert_eq!(body, [0x03, 0x05, 0x00, 0x00]);
        let mut reader = crate::wire::PacketReader::new(&body);
        assert_eq!(ItemStack::decode(&mut reader).expect("decodes"), simple);
        assert!(reader.is_empty());
    }

    #[test]
    fn item_stack_encodes_unmodelled_components_and_refuses_to_guess_them() {
        let with_component = ItemStack {
            item_id: 5,
            count: 1,
            components: vec![(7, vec![0xAA, 0xBB])],
        };
        let mut writer = PacketWriter::new();
        with_component.encode(&mut writer).expect("encodes");
        let body = writer.finish();
        assert_eq!(body, [0x01, 0x05, 0x01, 0x07, 0xAA, 0xBB, 0x00]);

        let mut reader = crate::wire::PacketReader::new(&body);
        assert!(
            ItemStack::decode(&mut reader).is_err(),
            "component payload framing is unmodelled, so it must be refused"
        );
        assert!(ItemStack::decode(&mut crate::wire::PacketReader::new(&[0x05])).is_err());
    }

    #[test]
    fn container_set_slot_round_trip() {
        let packet = ContainerSetSlot {
            window_id: 0,
            state_id: 1,
            slot: 36,
            item: ItemStack::simple(5, 3),
        };
        let body = packet.encode().expect("encodes");
        // window 0, state 1, slot 36, then the Slot: count 3, id 5, no patch.
        assert_eq!(body, [0x00, 0x01, 0x00, 0x24, 0x03, 0x05, 0x00, 0x00]);
        assert_eq!(ContainerSetSlot::decode(&body).expect("decodes"), packet);

        // The P14-09 walk's failing bytes, pinned: one cobblestone (item 35)
        // into slot 5 — count first, patch pair after the id.
        let cobble = ContainerSetSlot {
            window_id: 0,
            state_id: 7,
            slot: 5,
            item: ItemStack::simple(35, 1),
        };
        assert_eq!(
            cobble.encode().expect("encodes"),
            [0x00, 0x07, 0x00, 0x05, 0x01, 0x23, 0x00, 0x00]
        );
        assert_eq!(
            ContainerSetSlot::decode(&cobble.encode().expect("encodes")).expect("decodes"),
            cobble
        );

        let empty = ContainerSetSlot {
            window_id: -1,
            state_id: 0,
            slot: -1,
            item: ItemStack::empty(),
        };
        let empty_body = empty.encode().expect("encodes");
        // The container id is a VarInt, not a byte: -1 is five bytes, and an
        // i8 encoding here would have shifted the whole packet.
        assert_eq!(
            empty_body,
            [0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x00, 0xFF, 0xFF, 0x00]
        );
        assert_eq!(
            ContainerSetSlot::decode(&empty_body).expect("decodes"),
            empty
        );
        assert!(ContainerSetSlot::decode(&[0x00, 0x01]).is_err());
    }

    #[test]
    fn container_set_content_round_trip_and_carried_stack_is_outside_the_count() {
        let packet = ContainerSetContent {
            window_id: 0,
            state_id: 2,
            slots: vec![ItemStack::empty(), ItemStack::simple(1, 64)],
            carried: ItemStack::simple(2, 1),
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(
            body,
            [
                0x00, // window id
                0x02, // state id
                0x02, // two slots
                0x00, // slot 0 empty
                0x40, 0x01, 0x00, 0x00, // slot 1: count 64, id 1, empty patch
                0x01, 0x02, 0x00, 0x00, // carried: count 1, id 2, empty patch
            ]
        );
        assert_eq!(ContainerSetContent::decode(&body).expect("decodes"), packet);

        // A payload that stops after the counted slots has no carried stack.
        assert!(ContainerSetContent::decode(&body[..6]).is_err());
        // And a count that exceeds the frame is refused before allocating.
        assert!(ContainerSetContent::decode(&[0x00, 0x02, 0xFF, 0xFF, 0x7F]).is_err());
    }

    #[test]
    fn open_screen_round_trip_and_menu_type_is_a_varint_registry_id() {
        // Single chest: window 1, menu 2 (generic_9x3), title "Chest".
        // Wire: VarInt 1, VarInt 2, then network NBT TAG_String "Chest"
        // (0x08 tag, 0x00 0x05 length, bytes) — the same Component encoding
        // SystemChat uses, verified against the jar's TRUSTED_STREAM_CODEC slot.
        let packet = OpenScreen {
            window_id: 1,
            menu_type: MENU_GENERIC_9X3,
            title: TextComponent::literal("Chest"),
        };
        assert_eq!(MENU_GENERIC_9X3, 2);
        assert_eq!(MENU_GENERIC_9X6, 5);
        assert_eq!(MENU_FURNACE, 14);
        assert_eq!(MENU_HOPPER, 16);
        let body = packet.encode().expect("encodes");
        assert_eq!(
            body,
            [0x01, 0x02, 0x08, 0x00, 0x05, b'C', b'h', b'e', b's', b't']
        );
        assert_eq!(OpenScreen::decode(&body).expect("decodes"), packet);
        assert!(OpenScreen::decode(&[0x01]).is_err());
        assert!(OpenScreen::decode(&[0x01, 0x02]).is_err());
    }

    #[test]
    fn container_set_data_round_trip_and_short_widths() {
        // Furnace cook progress: window 1, property 2, value 100.
        // Wire: VarInt 1, i16-be 2, i16-be 100 — readShort/writeShort in the jar.
        let packet = ContainerSetData {
            window_id: 1,
            property: 2,
            value: 100,
        };
        let body = packet.encode().expect("encodes");
        assert_eq!(body, [0x01, 0x00, 0x02, 0x00, 0x64]);
        assert_eq!(ContainerSetData::decode(&body).expect("decodes"), packet);
        assert!(ContainerSetData::decode(&[0x01, 0x00]).is_err());
        assert!(ContainerSetData::decode(&[0x01, 0x00, 0x02, 0x00]).is_err());
    }

    #[test]
    fn container_close_and_set_cursor_round_trip() {
        let close = ContainerClose { window_id: 1 };
        let body = close.encode().expect("encodes");
        assert_eq!(body, [0x01]);
        assert_eq!(ContainerClose::decode(&body).expect("decodes"), close);
        assert!(ContainerClose::decode(&[]).is_err());

        let cursor = SetCursorItem {
            item: ItemStack::simple(5, 3),
        };
        let body = cursor.encode().expect("encodes");
        assert_eq!(body, [0x03, 0x05, 0x00, 0x00]);
        assert_eq!(SetCursorItem::decode(&body).expect("decodes"), cursor);
        assert!(SetCursorItem::decode(&[0x05]).is_err());
    }

    #[test]
    fn mob_effect_packets_round_trip() {
        // P16-03: field order per pumpkin's CUpdateMobEffect/CRemoveMobEffect
        // (entity, effect, amplifier, duration, flags); no capture exists for
        // either id, so the pin is shape-exactness through our own codec plus
        // the trailing-byte refusal both decoders share.
        let update = UpdateMobEffect {
            entity_id: 7,
            effect_id: 19,
            amplifier: 1,
            duration: 600,
            flags: UpdateMobEffect::flags_for(false),
        };
        let body = update.encode().expect("encodes");
        assert_eq!(body, [0x07, 0x13, 0x01, 0xD8, 0x04, 0x06]);
        assert_eq!(UpdateMobEffect::decode(&body).expect("decodes"), update);
        assert!(UpdateMobEffect::decode(&[0x07, 0x13]).is_err());
        let mut padded = body.clone();
        padded.push(0x00);
        assert!(UpdateMobEffect::decode(&padded).is_err());

        let remove = RemoveMobEffect {
            entity_id: 7,
            effect_id: 19,
        };
        let body = remove.encode().expect("encodes");
        assert_eq!(body, [0x07, 0x13]);
        assert_eq!(RemoveMobEffect::decode(&body).expect("decodes"), remove);
        assert!(RemoveMobEffect::decode(&[0x07]).is_err());
    }
}
