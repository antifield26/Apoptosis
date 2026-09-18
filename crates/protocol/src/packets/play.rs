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
use mc_persistence::packing;

// ---------------------------------------------------------------------------
// Shared validation helpers
// ---------------------------------------------------------------------------

/// Largest palette accepted in a network paletted container.
///
/// A block-state palette cannot exceed one entry per block state in the
/// registry, and `bits` is capped at [`mc_persistence::packing::MAX_BITS`] = 32
/// anyway; the explicit cap keeps a hostile length from being used as an
/// allocation hint before the bits check rejects it.
pub const MAX_PALETTE_LEN: usize = 1 << 16;

/// Largest section count accepted in a `chunk data` blob.
///
/// The 26.1 overworld is 24 sections tall with room for expansion; the cap is
/// generous but keeps the section loop bounded independently of the frame cap.
pub const MAX_CHUNK_SECTIONS: usize = 1024;

/// Largest heightmap count accepted in one chunk.
pub const MAX_HEIGHTMAPS: usize = 16;

/// Largest long count accepted in one heightmap.
///
/// A heightmap holds one column per horizontal position, packed 9 entries to a
/// `long` in vanilla; the cap is far above that but still bounds the loop.
pub const MAX_HEIGHTMAP_LONGS: usize = 4096;

/// Largest block-entity count accepted in one chunk.
pub const MAX_BLOCK_ENTITIES: usize = 4096;

/// Read a `VarInt` count and reject it unless it is inside `0..=max`.
///
/// Every counted wire list goes through here, so a hostile count can never be
/// turned into `Vec::with_capacity` before it has been range-checked.
fn read_count(reader: &mut PacketReader<'_>, what: &str, max: usize) -> ServerResult<usize> {
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
fn packed_len(len: usize) -> ServerResult<i32> {
    i32::try_from(len)
        .map_err(|_| ServerError::Invariant(format!("length {len} does not fit a VarInt")))
}

/// Re-layer a [`mc_persistence::packing`] failure as malformed wire input.
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

// ---------------------------------------------------------------------------
// Block positions
// ---------------------------------------------------------------------------

/// Bits of the y coordinate in a packed block position.
const POSITION_Y_BITS: u32 = 12;

/// Bits of an x/z coordinate in a packed block position.
const POSITION_HORIZONTAL_BITS: u32 = 26;

/// Bit offset of the x field in a packed block position.
const POSITION_X_SHIFT: u32 = 38;

/// Bit offset of the z field in a packed block position.
const POSITION_Z_SHIFT: u32 = 12;

/// Mask selecting an x/z field before it is shifted into place.
const POSITION_HORIZONTAL_MASK: i64 = (1 << POSITION_HORIZONTAL_BITS) - 1;

/// Mask selecting the y field in place.
const POSITION_Y_MASK: i64 = (1 << POSITION_Y_BITS) - 1;

/// Pack a block position the way `net.minecraft.core.BlockPos.asLong` does
/// (`((x & 0x3FFFFFF) << 38) | ((z & 0x3FFFFFF) << 12) | (y & 0xFFF)`).
///
/// The coordinate ranges are ±33 554 431 horizontally and ±2048 vertically;
/// values outside a field's width are truncated into it, exactly as the vanilla
/// bit operations do. Callers validate gameplay-coordinate bounds separately —
/// this is the wire encoding, not a range check.
#[must_use]
pub const fn block_position(x: i32, y: i32, z: i32) -> i64 {
    let x = (x as i64 & POSITION_HORIZONTAL_MASK) << POSITION_X_SHIFT;
    let z = (z as i64 & POSITION_HORIZONTAL_MASK) << POSITION_Z_SHIFT;
    let y = y as i64 & POSITION_Y_MASK;
    x | z | y
}

/// Inverse of [`block_position`], sign-extending each field.
///
/// Shifting a field up to the top of the 64-bit word and back down with an
/// arithmetic right shift *is* the sign extension, so a negative x, y or z
/// comes back negative instead of picking up the neighbouring field's bits.
#[must_use]
pub const fn unpack_block_position(packed: i64) -> (i32, i32, i32) {
    let x = (packed << (64 - POSITION_HORIZONTAL_BITS - POSITION_X_SHIFT))
        >> (64 - POSITION_HORIZONTAL_BITS);
    let z = (packed << (64 - POSITION_HORIZONTAL_BITS - POSITION_Z_SHIFT))
        >> (64 - POSITION_HORIZONTAL_BITS);
    let y = (packed << (64 - POSITION_Y_BITS)) >> (64 - POSITION_Y_BITS);
    (x as i32, y as i32, z as i32)
}

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

/// `minecraft:player_position` / teleport (clientbound 72).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPosition {
    /// Absolute x.
    pub x: f64,
    /// Absolute y.
    pub y: f64,
    /// Absolute z.
    pub z: f64,
    /// Velocity x.
    pub velocity_x: f64,
    /// Velocity y.
    pub velocity_y: f64,
    /// Velocity z.
    pub velocity_z: f64,
    /// Yaw degrees.
    pub yaw: f32,
    /// Pitch degrees.
    pub pitch: f32,
    /// Relative position/rotation flags.
    pub flags: i32,
    /// Teleport id to acknowledge.
    pub teleport_id: i32,
}

impl Packet for PlayerPosition {
    const ID: i32 = clientbound::play::PLAYER_POSITION;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        // The teleport id **leads**. Captured from a vanilla 26.1.2 server: `01` then six `f64`s, two `f32`s
        // and an `i32` of flags, which is exactly the 61 bytes the wire reports (P10-03).
        Ok(Self {
            teleport_id: reader.read_varint()?,
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            velocity_x: reader.read_f64()?,
            velocity_y: reader.read_f64()?,
            velocity_z: reader.read_f64()?,
            yaw: reader.read_f32()?,
            pitch: reader.read_f32()?,
            flags: reader.read_i32()?,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        // Leading, and it matters: a client reads this first. Writing it last made the client consume our
        // first byte of `x` as the id and then report one byte left over.
        writer.write_varint(self.teleport_id);
        writer.write_f64(self.x);
        writer.write_f64(self.y);
        writer.write_f64(self.z);
        writer.write_f64(self.velocity_x);
        writer.write_f64(self.velocity_y);
        writer.write_f64(self.velocity_z);
        writer.write_f32(self.yaw);
        writer.write_f32(self.pitch);
        writer.write_i32(self.flags);
        Ok(writer.finish())
    }
}

/// Block states in one chunk section (16×16×16).
pub const BLOCKS_PER_SECTION: usize = packing::BLOCK_ENTRIES;

/// Biomes in one chunk section (4×4×4).
pub const BIOMES_PER_SECTION: usize = packing::BIOME_ENTRIES;

/// Minimum bit width for a **network** biome palette.
///
/// The disk form uses 1 ([`mc_persistence::packing::BIOME_MIN_BITS`]) but the
/// network `Strategy.createForBiomes` uses **2**
/// (`docs/protocol/chunk-wire-format.md` section 7), so a disk container's width
/// must not be reused verbatim on the wire. Block states agree at 4 in both
/// forms, which is why only the biome side needs its own constant.
pub const NETWORK_BIOME_MIN_BITS: u32 = 2;

/// Bytes in one light array (2048 = 16³ nibbles).
pub const LIGHT_ARRAY_BYTES: usize = 2048;

/// Highest light-section bit a mask may use.
///
/// Bit `i` of a mask corresponds to light section `i - 1` — section 0 is bit 1
/// and bit 0 covers the layer below the world
/// (`docs/protocol/chunk-wire-format.md` section 6). The value is one below the
/// `i32` sign bit so `1 << (MAX_LIGHT_SECTIONS + 1)` is still a defined shift
/// while a full positive `i32` mask cannot be silently truncated.
pub const MAX_LIGHT_SECTIONS: u32 = 30;

/// A paletted container in its **network** form.
///
/// ```text
/// u8   bits_per_entry
/// bits == 0:  VarInt single global palette id      (the whole container)
/// bits != 0:  VarInt palette length
///             VarInt global palette id   × length
///             i64    packed indices      × longs_needed(entries, bits)
/// ```
///
/// Verified from the jar as `PalettedContainer$Data.write`
/// (`docs/protocol/chunk-wire-format.md` section 5). Two details matter and are
/// easy to get wrong:
///
/// - the storage array is `writeFixedSizeLongArray`: **raw longs with no**
///   `VarInt` length prefix. The count is derived from `bits` and the container's
///   cell count (4096 block states, 64 biomes). Only a heightmap's long array
///   carries a length prefix.
/// - `bits == 0` is the single-value form: one global palette id and **no**
///   palette and **no** array. It is not "the minimum width for a 1-entry
///   palette" — `bits_for` would answer 4 there, and a 4-bit container does
///   carry an array.
///
/// `palette` and `values` are the **decompressed** form: exactly
/// [`BLOCKS_PER_SECTION`] (or [`BIOMES_PER_SECTION`]) palette indices, one per
/// cell, which is what the world layer works with. Packing is delegated to
/// [`mc_persistence::packing`] — LSB-first, values never spanning a `long`
/// boundary — rather than reimplemented here, so the disk and network forms
/// cannot drift apart (P03-10).
///
/// [`PalettedContainer::decode`] re-derives the bit width from the palette and
/// rejects a mismatch: nothing in this version lets a peer widen a container.
///
/// The biome minimum on the network is [`NETWORK_BIOME_MIN_BITS`] (2), **not**
/// the disk constant — see that constant's docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedContainer {
    /// Global palette ids present in this container.
    pub palette: Vec<u32>,
    /// One palette index per cell, in vanilla scan order.
    pub values: Vec<u32>,
    /// Bit width used on the wire (`0` selects the single-value form).
    pub bits: u32,
}

impl PalettedContainer {
    /// Wrap an existing `(palette, values)` pair, deriving the bit width.
    ///
    /// A container that is one value repeated — the common case for air, and for
    /// a biome section — is encoded as the **single-value** form: `bits = 0` with
    /// the one global palette id and **no** index array. That form is not a
    /// minimum-width choice, it is a different layout, which is why
    /// [`mc_persistence::packing::bits_for`]'s `max(min_bits, …)` result is
    /// ignored for it: a palette of one still yields a 4-bit width there, and a
    /// 4-bit width *does* carry an array.
    ///
    /// `min_bits` is [`mc_persistence::packing::BLOCK_MIN_BITS`] for block
    /// states and `BIOME_MIN_BITS` for biomes.
    #[must_use]
    pub fn new(palette: Vec<u32>, values: Vec<u32>, min_bits: u32) -> Self {
        let bits = Self::canonical_bits(&palette, min_bits);
        Self {
            palette,
            values,
            bits,
        }
    }

    /// The wire bit width for a palette: `0` for the single-value form,
    /// otherwise the minimum width for the palette length.
    fn canonical_bits(palette: &[u32], min_bits: u32) -> u32 {
        if palette.len() <= 1 {
            0
        } else {
            packing::bits_for(palette.len(), min_bits)
        }
    }

    /// Encode the container, returning the number of bytes written.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the palette or width is not encodable
    /// (server-authored data, so this is a bug rather than bad input).
    pub fn encode(&self, writer: &mut PacketWriter) -> ServerResult<usize> {
        if self.palette.len() > MAX_PALETTE_LEN {
            return Err(ServerError::Invariant(format!(
                "palette of {} entries exceeds the {MAX_PALETTE_LEN}-entry limit",
                self.palette.len()
            )));
        }
        if self.bits == 0 && self.palette.len() > 1 {
            return Err(ServerError::Invariant(format!(
                "single-value container with a {}-entry palette",
                self.palette.len()
            )));
        }
        let before = writer.len();
        writer.write_u8(self.bits as u8);
        if self.bits == 0 {
            // Single-value form: the whole container is one palette id, written
            // once, with no data array at all.
            let first = self.palette.first().copied().unwrap_or(0);
            writer.write_varint(first as i32);
            return Ok(writer.len() - before);
        }
        if self.palette.is_empty() {
            return Err(ServerError::Invariant(
                "indexed container with an empty palette".to_owned(),
            ));
        }
        writer.write_varint(packed_len(self.palette.len())?);
        for entry in &self.palette {
            writer.write_varint(*entry as i32);
        }
        // `writeFixedSizeLongArray`: raw longs, no length prefix.
        let packed = packing::pack(&self.values, self.bits)?;
        for long in &packed {
            writer.write_i64(*long);
        }
        Ok(writer.len() - before)
    }

    /// Decode a container with `entries` cells and `min_bits` minimum width.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for a truncated payload, an out-of-range
    /// palette length or bit width, a bit width that disagrees with the palette,
    /// or an index outside the palette.
    pub fn decode(
        reader: &mut PacketReader<'_>,
        entries: usize,
        min_bits: u32,
    ) -> ServerResult<Self> {
        let bits = u32::from(reader.read_u8()?);
        if bits == 0 {
            // Single-value form: one global palette id, no palette and no index array. `entries` is the whole
            // container, so every slot indexes the one palette entry — **index `0`, not the id it holds**. The
            // two coincide for air, whose id is `0`, which is why handing back the id went unnoticed until a
            // uniform section of something else (`palette=[86]`, `values=[86, ..]`) made `palette[values[i]]`
            // an out-of-range read.
            let value = reader.read_varint()?;
            if value < 0 {
                return Err(ServerError::Protocol(format!(
                    "single-value palette id {value} is negative"
                )));
            }
            return Ok(Self {
                palette: vec![value as u32],
                values: vec![0; entries],
                bits,
            });
        }
        if !(1..=packing::MAX_BITS).contains(&bits) {
            return Err(ServerError::Protocol(format!(
                "paletted container declares {bits} bits per entry"
            )));
        }
        let palette_len = read_count(reader, "palette", MAX_PALETTE_LEN)?;
        let mut palette = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            let id = reader.read_varint()?;
            if id < 0 {
                return Err(ServerError::Protocol(format!(
                    "palette entry {id} is negative"
                )));
            }
            palette.push(id as u32);
        }
        if palette.is_empty() {
            return Err(ServerError::Protocol(
                "paletted container has an empty palette but stores indices".to_owned(),
            ));
        }
        // Any indexed container has a palette of at least two, and its width must
        // be exactly the minimum for that length: `bits` is not an independent
        // field that a peer may choose to widen.
        let derived = Self::canonical_bits(&palette, min_bits);
        if bits != derived {
            return Err(ServerError::Protocol(format!(
                "paletted container declares {bits} bits for a {}-entry palette (expected {derived})",
                palette.len()
            )));
        }
        // `writeFixedSizeLongArray`: the count is not on the wire, it is derived
        // from the width and the container's cell count.
        let longs = packing::longs_needed(entries, bits);
        let mut data = Vec::with_capacity(longs);
        for _ in 0..longs {
            data.push(reader.read_i64()?);
        }
        let values = packing::unpack(&data, bits, entries).map_err(packing_error)?;
        for value in &values {
            if *value as usize >= palette.len() {
                return Err(ServerError::Protocol(format!(
                    "palette index {value} is outside a {}-entry palette",
                    palette.len()
                )));
            }
        }
        Ok(Self {
            palette,
            values,
            bits,
        })
    }
}

/// One 16×16×16 chunk section as it appears inside `chunk data`.
///
/// `LevelChunkSection.write`, verified from the jar
/// (`docs/protocol/chunk-wire-format.md` section 4):
///
/// ```text
/// i16  non-empty block count
/// i16  fluid count
/// <PalettedContainer> block states  (4096 cells)
/// <PalettedContainer> biomes        (64 cells)
/// ```
///
/// The **fluid count** is easy to miss: an earlier brief omitted it, and a
/// section without it desynchronises every following byte of the chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSection {
    /// Non-empty (non-air) block count, `0..=4096`.
    pub block_count: i16,
    /// Non-empty fluid count, `0..=4096`.
    pub fluid_count: i16,
    /// Block states, [`BLOCKS_PER_SECTION`] cells.
    pub block_states: PalettedContainer,
    /// Biomes, [`BIOMES_PER_SECTION`] cells.
    pub biomes: PalettedContainer,
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
    // The two low bits carry `(scale & 3)`; a scale whose low bits are nonzero
    // sets the `| 4` continuation marker, and the rest rides the `VarInt`.
    let has_extra = (magnitude & 3) != 0;
    let low = if has_extra {
        (magnitude & 3) | 4
    } else {
        magnitude
    };
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

/// One heightmap: a `Heightmap.Types` id plus its packed columns.
///
/// The wire carries an integer id, not a name
/// (`ByteBufCodecs.idMapper(BY_ID, Types::id)` → `VarInt.write(buf, Types::id)`),
/// so the constants below are the ids the client resolves. They come from
/// `docs/protocol/heightmap-types.tsv`, read from `Heightmap$Types.class` in
/// `server-26.1.2.jar` (sha1 `83eb1106…`; the `97ccd4c0…` hash in the task
/// header is the *outer* bundler jar, and the TSV records that the inner jar is
/// byte-identical to the bundler's own copy).
///
/// Note there are exactly six types and **no** `LIGHT_BLOCKING`: that identifier
/// exists only as a legacy on-disk key written by `HeightmapRenamingFix`.
/// `kind` is kept as a raw `i32` so an unknown id round-trips instead of being
/// dropped — the client itself decodes out-of-range ids leniently to id 0
/// (`ByIdMap.OutOfBoundsStrategy.ZERO`), so a peer may legally send one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heightmap {
    /// `Heightmap.Types` id, see [`HEIGHTMAP_WORLD_SURFACE`] and friends.
    pub kind: i32,
    /// Packed height columns. The count **is** on the wire for heightmaps
    /// (`ByteBufCodecs.LONG_ARRAY` → `FriendlyByteBuf.writeLongArray`), unlike a
    /// paletted container's storage array.
    pub data: Vec<i64>,
}

/// `Heightmap.Types` id for `WORLD_SURFACE_WG`.
pub const HEIGHTMAP_WORLD_SURFACE_WG: i32 = 0;
/// `Heightmap.Types` id for `WORLD_SURFACE` — the type Phase 04 sends.
pub const HEIGHTMAP_WORLD_SURFACE: i32 = 1;
/// `Heightmap.Types` id for `OCEAN_FLOOR_WG`.
pub const HEIGHTMAP_OCEAN_FLOOR_WG: i32 = 2;
/// `Heightmap.Types` id for `OCEAN_FLOOR`.
pub const HEIGHTMAP_OCEAN_FLOOR: i32 = 3;
/// `Heightmap.Types` id for `MOTION_BLOCKING`.
pub const HEIGHTMAP_MOTION_BLOCKING: i32 = 4;
/// `Heightmap.Types` id for `MOTION_BLOCKING_NO_LEAVES`.
pub const HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES: i32 = 5;

/// Sections in an overworld chunk column (384 blocks / 16).
///
/// The section blob has **no** count prefix, so this is what a decoder needs to
/// know how many sections to read; a taller or shorter dimension supplies its
/// own value to [`LevelChunkWithLight::decode_chunk_data`].
pub const OVERWORLD_SECTIONS: usize = 24;

/// A `LevelChunkWithLight` payload (clientbound, play).
///
/// ```text
/// i32     chunk_x
/// i32     chunk_z
/// VarInt  heightmap count
///           per entry: VarInt Heightmap.Types id
///                      VarInt long count
///                      i64    × count              (raw)
/// VarInt  data size (bytes)
/// byte[]  chunk data:
///           per section (fixed count, no prefix):
///             i16 block_count, i16 fluid_count,
///             block states container, biomes container
/// VarInt  block_entity count
///           per entry: u16 packed xz, u16 y, VarInt type id, NBT data
/// VarInt  sky light mask          \
/// VarInt  block light mask         |  bit i ↔ light section i - 1
/// VarInt  empty sky light mask     |
/// VarInt  empty block light mask  /
/// VarInt  sky light array count
///           per array: VarInt 2048, byte[2048]
/// VarInt  block light array count
///           per array: VarInt 2048, byte[2048]
/// ```
///
/// Verified from the jar — see `docs/protocol/chunk-wire-format.md` for the
/// class and method behind every line. Three things the earlier description got
/// wrong and this implementation follows the jar on: the section blob has no
/// count prefix, each section carries a fluid count after the block count, and
/// the heightmaps are a numeric map rather than an NBT compound.
///
/// The light *masks* are not derivable from the arrays (a set bit with no array
/// means "fully lit", a set bit in an *empty* mask means "fully dark"), so they
/// are carried verbatim and checked for consistency against the array counts:
/// the arrays carry no section index of their own, so a mismatch is
/// unrecoverable rather than cosmetic. Phase 04 sends all four masks and both
/// array counts as `0`, which leaves the chunk dark until a `light_update`
/// arrives — recorded as a known limitation, not hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    /// Chunk x (in chunks).
    pub chunk_x: i32,
    /// Chunk z (in chunks).
    pub chunk_z: i32,
    /// Heightmaps in wire order. A `Map` on the wire, so a repeated id is
    /// possible and preserved here rather than collapsed.
    pub heightmaps: Vec<Heightmap>,
    /// Sections bottom-to-top, including empty ones.
    pub sections: Vec<ChunkSection>,
    /// Block entities in this chunk.
    pub block_entities: Vec<ChunkBlockEntity>,
    /// Light sections whose sky light an array follows for, as **set indices**.
    ///
    /// These four are `java.util.BitSet` on the wire (P10-03, KD-44): a `VarInt` count of longs followed by
    /// that many `i64`s. They were read as `VarInt`s, which survives an empty mask — `0` in both encodings —
    /// and silently misreads any mask with content, which is why every captured vanilla chunk failed to parse.
    ///
    /// Index `i` is light section `i`, which is world section `i - 1`. Sorted, deduplicated.
    pub sky_light_mask: Vec<u32>,
    /// Light sections whose block light an array follows for. See [`Self::sky_light_mask`].
    pub block_light_mask: Vec<u32>,
    /// Light sections that are uniformly sky-lit, so need no array.
    pub empty_sky_light_mask: Vec<u32>,
    /// Light sections that are uniformly dark, so need no array.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

/// Encode a heightmap list (`VarInt` count, then id + length + longs each).
///
/// # Errors
///
/// [`ServerError::Invariant`] when a list length does not fit a `VarInt`.
fn encode_heightmaps(writer: &mut PacketWriter, heightmaps: &[Heightmap]) -> ServerResult<()> {
    writer.write_varint(packed_len(heightmaps.len())?);
    for heightmap in heightmaps {
        writer.write_varint(heightmap.kind);
        writer.write_varint(packed_len(heightmap.data.len())?);
        for long in &heightmap.data {
            writer.write_i64(*long);
        }
    }
    Ok(())
}

/// Decode a heightmap list.
///
/// # Errors
///
/// [`ServerError::Protocol`] on a hostile entry or long count, or truncation.
fn decode_heightmaps(reader: &mut PacketReader<'_>) -> ServerResult<Vec<Heightmap>> {
    let count = read_count(reader, "heightmap", MAX_HEIGHTMAPS)?;
    let mut heightmaps = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = reader.read_varint()?;
        let long_count = read_count(reader, "heightmap long", MAX_HEIGHTMAP_LONGS)?;
        let mut data = Vec::with_capacity(long_count);
        for _ in 0..long_count {
            data.push(reader.read_i64()?);
        }
        heightmaps.push(Heightmap { kind, data });
    }
    Ok(heightmaps)
}

impl LevelChunkWithLight {
    /// Encode the `chunk data` blob (sections only), returning the bytes.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when a section's containers do not have the
    /// exact cell count the format requires, or a count is out of `0..=4096`.
    pub fn encode_chunk_data(sections: &[ChunkSection]) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        for section in sections {
            // A bad count here is server-authored data, so the shared check's
            // `Protocol` verdict is re-labelled as the invariant violation it is.
            validate_section_counts(section)
                .map_err(|error| ServerError::Invariant(error.to_string()))?;
            writer.write_i16(section.block_count);
            writer.write_i16(section.fluid_count);
            section.block_states.encode(&mut writer)?;
            section.biomes.encode(&mut writer)?;
        }
        Ok(writer.finish())
    }

    /// Decode a `chunk data` blob holding exactly `sections_per_chunk` sections.
    ///
    /// The count is a parameter because the blob does not carry one: it is
    /// implied by the dimension's height ([`OVERWORLD_SECTIONS`] for the
    /// overworld). A blob that does not end exactly at the last section is
    /// rejected rather than partially accepted.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on a hostile block/fluid count or container, a
    /// truncated blob, or trailing bytes.
    pub fn decode_chunk_data(
        bytes: &[u8],
        sections_per_chunk: usize,
    ) -> ServerResult<Vec<ChunkSection>> {
        if sections_per_chunk > MAX_CHUNK_SECTIONS {
            return Err(ServerError::Protocol(format!(
                "chunk declares {sections_per_chunk} sections, above the \
                 {MAX_CHUNK_SECTIONS}-section limit"
            )));
        }
        let mut reader = PacketReader::new(bytes);
        let mut sections = Vec::with_capacity(sections_per_chunk);
        for _ in 0..sections_per_chunk {
            let block_count = reader.read_i16()?;
            let fluid_count = reader.read_i16()?;
            let section = ChunkSection {
                block_count,
                fluid_count,
                block_states: PalettedContainer::decode(
                    &mut reader,
                    BLOCKS_PER_SECTION,
                    packing::BLOCK_MIN_BITS,
                )?,
                biomes: PalettedContainer::decode(
                    &mut reader,
                    BIOMES_PER_SECTION,
                    NETWORK_BIOME_MIN_BITS,
                )?,
            };
            validate_section_counts(&section)
                .map_err(|error| ServerError::Protocol(error.to_string()))?;
            sections.push(section);
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "chunk data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(sections)
    }
}

/// Check the two section counters and the container cell counts.
///
/// Used by both directions: the counters are `i16` on the wire but describe a
/// 4096-block volume, so out-of-range values are malformed in either direction.
fn validate_section_counts(section: &ChunkSection) -> ServerResult<()> {
    let max = BLOCKS_PER_SECTION as i16;
    if !(0..=max).contains(&section.block_count) {
        return Err(ServerError::Protocol(format!(
            "section block count {} is outside 0..={max}",
            section.block_count
        )));
    }
    if !(0..=max).contains(&section.fluid_count) {
        return Err(ServerError::Protocol(format!(
            "section fluid count {} is outside 0..={max}",
            section.fluid_count
        )));
    }
    if section.block_states.values.len() != BLOCKS_PER_SECTION {
        return Err(ServerError::Protocol(format!(
            "block states hold {} cells, expected {BLOCKS_PER_SECTION}",
            section.block_states.values.len()
        )));
    }
    if section.biomes.values.len() != BIOMES_PER_SECTION {
        return Err(ServerError::Protocol(format!(
            "biomes hold {} cells, expected {BIOMES_PER_SECTION}",
            section.biomes.values.len()
        )));
    }
    Ok(())
}

impl Packet for LevelChunkWithLight {
    const ID: i32 = clientbound::play::LEVEL_CHUNK_WITH_LIGHT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let chunk_x = reader.read_i32()?;
        let chunk_z = reader.read_i32()?;
        let heightmaps = decode_heightmaps(&mut reader)?;
        let size = reader.read_varint()?;
        if size < 0 {
            return Err(ServerError::Protocol(format!(
                "chunk data size {size} is negative"
            )));
        }
        let data = reader.read_bytes(size as usize)?;
        // The blob has no section count of its own; the dimension supplies it.
        let sections = Self::decode_chunk_data(data, OVERWORLD_SECTIONS)?;
        let block_entity_count = read_count(&mut reader, "block entity", MAX_BLOCK_ENTITIES)?;
        let mut block_entities = Vec::with_capacity(block_entity_count);
        for _ in 0..block_entity_count {
            let packed_xz = reader.read_u16()?;
            let y = reader.read_u16()?;
            let type_id = reader.read_varint()?;
            let mut rest = reader.remaining_slice();
            let data = Nbt::read_network(&mut rest)?;
            let consumed = reader.remaining() - rest.len();
            reader.advance(consumed)?;
            block_entities.push(ChunkBlockEntity {
                packed_xz,
                y,
                type_id,
                data,
            });
        }
        let light = read_light_data(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "level_chunk_with_light has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            heightmaps,
            sections,
            block_entities,
            sky_light_mask: light.sky_light_mask,
            block_light_mask: light.block_light_mask,
            empty_sky_light_mask: light.empty_sky_light_mask,
            empty_block_light_mask: light.empty_block_light_mask,
            sky_light: light.sky_light,
            block_light: light.block_light,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i32(self.chunk_x);
        writer.write_i32(self.chunk_z);
        encode_heightmaps(&mut writer, &self.heightmaps)?;
        let data = Self::encode_chunk_data(&self.sections)?;
        writer.write_varint(packed_len(data.len())?);
        writer.write_bytes(&data);
        encode_block_entities(&mut writer, &self.block_entities)?;
        write_light_data(
            &mut writer,
            &self.sky_light_mask,
            &self.block_light_mask,
            &self.empty_sky_light_mask,
            &self.empty_block_light_mask,
            &self.sky_light,
            &self.block_light,
        )?;
        Ok(writer.finish())
    }
}

/// Append the block-entity list (`count`, then `u16 xz`, `u16 y`, `VarInt` type
/// id and nameless NBT per entry).
fn encode_block_entities(
    writer: &mut PacketWriter,
    block_entities: &[ChunkBlockEntity],
) -> ServerResult<()> {
    writer.write_varint(packed_len(block_entities.len())?);
    let mut nbt_bytes = Vec::new();
    for entity in block_entities {
        writer.write_u16(entity.packed_xz);
        writer.write_u16(entity.y);
        writer.write_varint(entity.type_id);
        nbt_bytes.clear();
        entity.data.write_network(&mut nbt_bytes)?;
        writer.write_bytes(&nbt_bytes);
    }
    Ok(())
}

/// Append a light-array section (`VarInt` count, then `VarInt 2048` + the bytes).
///
/// The `2048` length prefix is redundant but mandatory, and is written
/// explicitly rather than derived from `array.len()` so a caller cannot
/// accidentally emit an array the client will slice wrongly.
fn encode_light_arrays(writer: &mut PacketWriter, arrays: &[Vec<u8>]) -> ServerResult<()> {
    writer.write_varint(packed_len(arrays.len())?);
    for array in arrays {
        if array.len() != LIGHT_ARRAY_BYTES {
            return Err(ServerError::Invariant(format!(
                "light array of {} bytes, expected {LIGHT_ARRAY_BYTES}",
                array.len()
            )));
        }
        writer.write_varint(LIGHT_ARRAY_BYTES as i32);
        writer.write_bytes(array);
    }
    Ok(())
}

/// The light half of both chunk packets: four masks and two array lists.
///
/// Returned owned by `read_light_data`; `write_light_data` borrows the fields instead, so encoding a packet
/// does not clone a single 2 048-byte array.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightData {
    /// Sections an array follows for, by index. See [`LevelChunkWithLight::sky_light_mask`].
    pub sky_light_mask: Vec<u32>,
    /// Sections a block-light array follows for.
    pub block_light_mask: Vec<u32>,
    /// Sections that are uniformly sky-lit.
    pub empty_sky_light_mask: Vec<u32>,
    /// Sections that are uniformly dark.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

/// Write the four masks and two array lists that `level_chunk_with_light` and `light_update` share.
///
/// # Errors
///
/// [`ServerError::Invariant`] for an array that is not `LIGHT_ARRAY_BYTES`.
#[allow(clippy::too_many_arguments)]
pub fn write_light_data(
    writer: &mut PacketWriter,
    sky_light_mask: &[u32],
    block_light_mask: &[u32],
    empty_sky_light_mask: &[u32],
    empty_block_light_mask: &[u32],
    sky_light: &[Vec<u8>],
    block_light: &[Vec<u8>],
) -> ServerResult<()> {
    write_light_mask(writer, sky_light_mask);
    write_light_mask(writer, block_light_mask);
    write_light_mask(writer, empty_sky_light_mask);
    write_light_mask(writer, empty_block_light_mask);
    encode_light_arrays(writer, sky_light)?;
    encode_light_arrays(writer, block_light)
}

/// Read the four masks and two array lists that both chunk packets share.
///
/// # Errors
///
/// [`ServerError::Protocol`] on a malformed mask, a count that disagrees with its mask, or an array that is not
/// `LIGHT_ARRAY_BYTES`.
pub fn read_light_data(reader: &mut PacketReader<'_>) -> ServerResult<LightData> {
    let sky_light_mask = read_light_mask(reader, "sky light")?;
    let block_light_mask = read_light_mask(reader, "block light")?;
    let empty_sky_light_mask = read_light_mask(reader, "empty sky light")?;
    let empty_block_light_mask = read_light_mask(reader, "empty block light")?;
    let sky_light = decode_light_arrays(reader, &sky_light_mask)?;
    let block_light = decode_light_arrays(reader, &block_light_mask)?;
    Ok(LightData {
        sky_light_mask,
        block_light_mask,
        empty_sky_light_mask,
        empty_block_light_mask,
        sky_light,
        block_light,
    })
}

/// Read a light-array section and check the count against the mask that indexes
/// it (the arrays carry no section index of their own).
fn decode_light_arrays(reader: &mut PacketReader<'_>, mask: &[u32]) -> ServerResult<Vec<Vec<u8>>> {
    let count = read_count(reader, "light array", MAX_LIGHT_SECTIONS as usize + 1)?;
    let expected = mask.len();
    if count != expected {
        return Err(ServerError::Protocol(format!(
            "light mask has {expected} sections set but {count} arrays follow"
        )));
    }
    let mut arrays = Vec::with_capacity(count);
    for _ in 0..count {
        let len = reader.read_varint()?;
        if len != LIGHT_ARRAY_BYTES as i32 {
            return Err(ServerError::Protocol(format!(
                "light array declares {len} bytes, expected {LIGHT_ARRAY_BYTES}"
            )));
        }
        let bytes = reader.read_bytes(LIGHT_ARRAY_BYTES)?;
        arrays.push(bytes.to_vec());
    }
    Ok(arrays)
}

/// Upper bound on the longs a light-mask `BitSet` may claim.
///
/// `MAX_LIGHT_SECTIONS + 1` bits need `(MAX_LIGHT_SECTIONS + 1).div_ceil(64)` longs; one more is allowed so a
/// server that pads is still readable. The bound exists so one hostile `VarInt` cannot drive a large
/// allocation, as with every other count in this packet.
const MAX_LIGHT_MASK_LONGS: i32 = 2;

/// Read one light mask: vanilla's `BitSet` form, a `VarInt` count of longs then that many `i64`s (P10-03,
/// KD-44). Returns the **set indices**, sorted.
fn read_light_mask(reader: &mut PacketReader<'_>, what: &str) -> ServerResult<Vec<u32>> {
    let longs = reader.read_varint()?;
    if longs < 0 {
        return Err(ServerError::Protocol(format!(
            "{what} mask claims {longs} longs"
        )));
    }
    if longs > MAX_LIGHT_MASK_LONGS {
        return Err(ServerError::Protocol(format!(
            "{what} mask claims {longs} longs, above the {MAX_LIGHT_MASK_LONGS} a light section range needs"
        )));
    }
    let mut indices = Vec::new();
    for index in 0..longs {
        // The bit pattern is what matters, so the signed read is reinterpreted rather than range-checked.
        #[allow(clippy::cast_sign_loss)]
        let value = reader.read_i64()? as u64;
        for bit in 0..64u32 {
            if value >> bit & 1 == 1 {
                let section = index as u32 * 64 + bit;
                if section > MAX_LIGHT_SECTIONS {
                    return Err(ServerError::Protocol(format!(
                        "{what} mask sets bit {section}, beyond section {MAX_LIGHT_SECTIONS}"
                    )));
                }
                indices.push(section);
            }
        }
    }
    Ok(indices)
}

/// Write one light mask as a `BitSet`: a `VarInt` count of longs then that many `i64`s.
///
/// Trailing zero longs are trimmed, which is what vanilla's `BitSet.toLongArray()` does. Writing a fixed-width
/// integer instead would emit bytes no real server produces — and the client reads a count, so it would read
/// the wrong number of them.
fn write_light_mask(writer: &mut PacketWriter, mask: &[u32]) {
    let Some(highest) = mask.iter().copied().max() else {
        writer.write_varint(0);
        return;
    };
    let longs = highest / 64 + 1;
    #[allow(clippy::cast_possible_wrap)]
    writer.write_varint(longs as i32);
    for index in 0..longs {
        let mut value = 0u64;
        for bit in 0..64 {
            if mask.contains(&(index * 64 + bit)) {
                value |= 1u64 << bit;
            }
        }
        #[allow(clippy::cast_possible_wrap)]
        writer.write_i64(value as i64);
    }
}

/// `minecraft:light_update` (clientbound play 48).
///
/// Sent when a chunk's light changes without its blocks being re-sent — placing a torch, breaking a block that
/// was casting a shadow. It carries **the same light data** as the tail of [`LevelChunkWithLight`], which is
/// why both go through [`write_light_data`] and [`read_light_data`].
///
/// **The coordinates are `VarInt`s.** The chunk packet writes the same two numbers as `i32`; this one does
/// not, and reading them the other way consumes two extra bytes per coordinate and misparses the light data
/// that follows. That difference is the whole reason this packet is not a struct re-use of the chunk one, and
/// it is pinned by a test.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightUpdate {
    /// Chunk x.
    pub chunk_x: i32,
    /// Chunk z.
    pub chunk_z: i32,
    /// Sections an array follows for, by index.
    pub sky_light_mask: Vec<u32>,
    /// Sections a block-light array follows for.
    pub block_light_mask: Vec<u32>,
    /// Sections that are uniformly sky-lit.
    pub empty_sky_light_mask: Vec<u32>,
    /// Sections that are uniformly dark.
    pub empty_block_light_mask: Vec<u32>,
    /// Sky-light arrays, in ascending mask-bit order.
    pub sky_light: Vec<Vec<u8>>,
    /// Block-light arrays, in ascending mask-bit order.
    pub block_light: Vec<Vec<u8>>,
}

impl LightUpdate {
    /// Build one from a computed chunk, taking only what changed.
    ///
    /// Every light section is accounted for, the same way [`LevelChunkWithLight`] does it: a section that is
    /// uniformly the layer's default goes in the matching `empty_*` mask and needs no array, and bit `i` is
    /// light section `i`, i.e. world section `i - 1`.
    #[must_use]
    pub fn from_chunk_light(
        chunk_x: i32,
        chunk_z: i32,
        light: &crate::packets::play::LightData,
    ) -> Self {
        Self {
            chunk_x,
            chunk_z,
            sky_light_mask: light.sky_light_mask.clone(),
            block_light_mask: light.block_light_mask.clone(),
            empty_sky_light_mask: light.empty_sky_light_mask.clone(),
            empty_block_light_mask: light.empty_block_light_mask.clone(),
            sky_light: light.sky_light.clone(),
            block_light: light.block_light.clone(),
        }
    }
}

impl Packet for LightUpdate {
    const ID: i32 = clientbound::play::LIGHT_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        // **`VarInt`, not `i32`** — see the type's documentation. The chunk packet beside it uses `i32` for
        // the same two numbers.
        let chunk_x = reader.read_varint()?;
        let chunk_z = reader.read_varint()?;
        let light = read_light_data(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "light_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            sky_light_mask: light.sky_light_mask,
            block_light_mask: light.block_light_mask,
            empty_sky_light_mask: light.empty_sky_light_mask,
            empty_block_light_mask: light.empty_block_light_mask,
            sky_light: light.sky_light,
            block_light: light.block_light,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.chunk_x);
        writer.write_varint(self.chunk_z);
        write_light_data(
            &mut writer,
            &self.sky_light_mask,
            &self.block_light_mask,
            &self.empty_sky_light_mask,
            &self.empty_block_light_mask,
            &self.sky_light,
            &self.block_light,
        )?;
        Ok(writer.finish())
    }
}

/// Acknowledge the client's predicted block changes up to a sequence
/// (`minecraft:block_changed_ack`, clientbound play 4).
///
/// # Why this packet exists, and what its absence cost
///
/// A 26.x client does not apply a `block_update` straight to its level. Mining
/// opens a **prediction**: `MultiPlayerGameMode.startDestroyBlock` calls
/// `startPrediction`, which raises `BlockStatePredictionHandler.currentSequenceNr`
/// and sends `player_action` carrying that number, having first retained the
/// server-known state for the position.
///
/// The client then routes every server block change through
/// `ClientLevel.setServerVerifiedBlockState`, which is (bytecode, client jar):
///
/// ```text
/// if (!this.blockStatePredictionHandler.updateKnownServerState(pos, state)) {
///     super.setBlock(pos, state, flags, 512);
/// }
/// ```
///
/// `updateKnownServerState` **returns true whenever a prediction is pending at
/// that position** — and then the state is only *stored*, never applied. The
/// pending entry is cleared by exactly one thing: `endPredictionsUpTo(sequence)`,
/// which `ClientPacketListener.handleBlockChangedAck` calls with the sequence
/// from this packet.
///
/// So a server that never sends `block_changed_ack` never lets the client finish a
/// prediction. The block the player mined stays stone on screen, and **every later
/// server change at that position is swallowed too** — a permanent, per-position
/// freeze. That was the real cause of the owner's "survival mining does not break
/// blocks" report; the earlier reading of the capture (that the dig packets were
/// malformed) was an artefact of counting the packet-id byte as part of the body.
///
/// The vanilla server's rule, from `javap -c -p` on
/// `net.minecraft.server.network.ServerGamePacketListenerImpl`:
///
/// ```text
/// ackBlockChangesUpTo(int sequence):
///     if (sequence < 0) throw new IllegalArgumentException("Expected packet sequence nr >= 0");
///     this.ackBlockChangesUpTo = Math.max(sequence, this.ackBlockChangesUpTo);
/// tick():
///     if (this.ackBlockChangesUpTo > -1) {
///         send(new ClientboundBlockChangedAckPacket(this.ackBlockChangesUpTo));
///         this.ackBlockChangesUpTo = -1;
///     }
/// ```
///
/// and it is fed from `handlePlayerAction`, `handleUseItemOn` and `handleUseItem` —
/// the three serverbound packets that carry a sequence. Hence the high-water mark
/// rather than "ack each": the client only ever needs to know how far the server
/// has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChangedAck {
    /// Highest prediction sequence the server has processed for this connection.
    pub sequence: i32,
}

impl Packet for BlockChangedAck {
    /// The clientbound play id, cross-checked against the shipped table: the
    /// client jar's registration order and `docs/protocol/packet-ids-775.tsv`
    /// both put `block_changed_ack` at **4**.
    const ID: i32 = clientbound::play::BLOCK_CHANGED_ACK;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let sequence = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_changed_ack has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { sequence })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        // A single VarInt, and nothing else: the packet is one field wide.
        writer.write_varint(self.sequence);
        Ok(writer.finish())
    }
}

/// A single block change (`minecraft:block_update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpdate {
    /// Packed block position, see [`block_position`].
    pub position: i64,
    /// Global block state id.
    pub block_state: i32,
}

impl Packet for BlockUpdate {
    const ID: i32 = clientbound::play::BLOCK_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let position = reader.read_i64()?;
        let block_state = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "block_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            position,
            block_state,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_i64(self.position);
        writer.write_varint(self.block_state);
        Ok(writer.finish())
    }
}

/// A batch of block changes in one section (`minecraft:section_blocks_update`).
///
/// Vanilla packs many block changes into one packet, which maps cleanly onto a
/// tick's worth of edits to a single section. The leading count is the only
/// structure: every entry has the same two fields as [`BlockUpdate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionBlocksUpdate {
    /// Changes, each as a packed position plus a global block state id.
    pub updates: Vec<BlockUpdate>,
}

/// Largest number of block changes accepted in one [`SectionBlocksUpdate`].
///
/// A section holds [`BLOCKS_PER_SECTION`] blocks, so a larger batch cannot
/// describe a real change set; the cap also bounds the decode loop before the
/// frame-size cap would.
pub const MAX_SECTION_UPDATES: usize = BLOCKS_PER_SECTION;

impl Packet for SectionBlocksUpdate {
    const ID: i32 = clientbound::play::SECTION_BLOCKS_UPDATE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let count = read_count(&mut reader, "section update", MAX_SECTION_UPDATES)?;
        let mut updates = Vec::with_capacity(count);
        for _ in 0..count {
            updates.push(BlockUpdate {
                position: reader.read_i64()?,
                block_state: reader.read_varint()?,
            });
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "section_blocks_update has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { updates })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(packed_len(self.updates.len())?);
        for update in &self.updates {
            writer.write_i64(update.position);
            writer.write_varint(update.block_state);
        }
        Ok(writer.finish())
    }
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
/// A clock reference is the registry id plus one (`ByteBufCodecs$30`: `0`
/// means inline, otherwise `byId(i - 1)`); the overworld clock bootstraps
/// first, so it is id `0` on the wire as `0x01`. A clock state is
/// `total_ticks i64`, `partial_tick f32`, `rate f32` in field order. An empty
/// patch-free map is a single `0x00` — which is exactly what the P10-03
/// capture holds after its `i64` (eighteen packets, all 9 bytes): those
/// captures were an empty clock map, not "one trailing byte".
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
            // Reference encoding: `0` would be an inline clock (never sent),
            // otherwise `byId(i - 1)`.
            let raw = reader.read_varint()?;
            if raw <= 0 {
                return Err(ServerError::Protocol(format!(
                    "set_time clock reference {raw} is not a registry id"
                )));
            }
            clocks.push(ClockState {
                clock_id: raw - 1,
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
            writer.write_varint(clock.clock_id + 1);
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

/// The three relative-move packets, whose layouts a capture settled.
///
/// A real 26.1.2 server sent **11,713** bodies for these three ids and every one is a length the field widths
/// imply, with the two values per packet differing by exactly the one byte a `VarInt` id gains past 127:
///
/// ```text
/// 53  move_entity_pos      id, dx, dy, dz (i16), on_ground                 = 8 / 9
/// 54  move_entity_pos_rot  id, dx, dy, dz (i16), yaw, pitch (i8), on_ground = 10 / 11
/// 56  move_entity_rot      id, yaw, pitch (i8), on_ground                  = 4 / 5
/// ```
///
/// **Deltas, not positions**: vanilla sends these for small movements and `teleport_entity` for large ones, so the
/// units are 1/4096 of a block per axis and the caller decides which packet a movement deserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPos {
    /// Entity id.
    pub entity_id: i32,
    /// X delta, in 1/4096 of a block.
    pub dx: i16,
    /// Y delta, in 1/4096 of a block.
    pub dy: i16,
    /// Z delta, in 1/4096 of a block.
    pub dz: i16,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

/// A relative move with rotation; see [`MoveEntityPos`] for the layout's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPosRot {
    /// Entity id.
    pub entity_id: i32,
    /// X delta, in 1/4096 of a block.
    pub dx: i16,
    /// Y delta, in 1/4096 of a block.
    pub dy: i16,
    /// Z delta, in 1/4096 of a block.
    pub dz: i16,
    /// Yaw, in 1/256 of a degree.
    pub yaw: i8,
    /// Pitch, in 1/256 of a degree.
    pub pitch: i8,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

/// Rotation only; see [`MoveEntityPos`] for the layout's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityRot {
    /// Entity id.
    pub entity_id: i32,
    /// Yaw, in 1/256 of a degree.
    pub yaw: i8,
    /// Pitch, in 1/256 of a degree.
    pub pitch: i8,
    /// Whether the entity is resting on solid ground.
    pub on_ground: bool,
}

impl Packet for MoveEntityPos {
    const ID: i32 = clientbound::play::MOVE_ENTITY_POS;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_pos")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

impl Packet for MoveEntityPosRot {
    const ID: i32 = clientbound::play::MOVE_ENTITY_POS_ROT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            dx: reader.read_i16()?,
            dy: reader.read_i16()?,
            dz: reader.read_i16()?,
            yaw: reader.read_i8()?,
            pitch: reader.read_i8()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_pos_rot")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i16(self.dx);
        writer.write_i16(self.dy);
        writer.write_i16(self.dz);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

impl Packet for MoveEntityRot {
    const ID: i32 = clientbound::play::MOVE_ENTITY_ROT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            yaw: reader.read_i8()?,
            pitch: reader.read_i8()?,
            on_ground: reader.read_bool()?,
        };
        require_exhausted(&reader, "move_entity_rot")?;
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_i8(self.yaw);
        writer.write_i8(self.pitch);
        writer.write_bool(self.on_ground);
        Ok(writer.finish())
    }
}

/// Refuse a body with bytes left over, naming the packet.
///
/// Shared by the three above because a decoder that ignores a tail would accept a longer packet as a shorter one
/// -- which is how a field a future version added goes missing without anything reporting it.
fn require_exhausted(reader: &PacketReader<'_>, packet: &str) -> ServerResult<()> {
    if reader.is_empty() {
        return Ok(());
    }
    Err(ServerError::Protocol(format!(
        "{packet} has {} trailing bytes",
        reader.remaining()
    )))
}

/// Wire type id for [`MetadataValue::Byte`].
pub const METADATA_TYPE_BYTE: i32 = 0;
/// Wire type id for [`MetadataValue::VarInt`].
pub const METADATA_TYPE_VARINT: i32 = 1;
/// Wire type id for [`MetadataValue::Float`].
pub const METADATA_TYPE_FLOAT: i32 = 3;
/// Wire type id for [`MetadataValue::ItemStack`].
///
/// Read off the wire rather than from the serializer table alone: a captured `set_entity_data` for a dropped item
/// carries `08 07` -- index 8, this type -- followed by the stack.
pub const METADATA_TYPE_ITEM_STACK: i32 = 7;

/// Terminator that ends a metadata entry list.
pub const METADATA_TERMINATOR: u8 = 0xFF;

/// The metadata **slot index** a living entity's health rides on: **9**, with [`METADATA_TYPE_FLOAT`].
///
/// Measured, not read off a table: every mob kind Phase 11 spawns (zombie, skeleton, creeper, spider, pig,
/// chicken, sheep, cow, slime) sent its spawn health at index 9 with the float type in the console-summon
/// captures, across several hundred bodies whose walk ended on a clean terminator. The full per-type slot table
/// lives in `crates/test-support/fixtures/registry/entity_metadata.tsv`, and
/// `crates/protocol/tests/entity_metadata_golden.rs` pins both the constant and two whole captured bodies.
pub const METADATA_INDEX_HEALTH: u8 = 9;

/// One entity metadata value.
///
/// Only the three shapes Phase 04 sends are modelled. Vanilla's remaining type
/// ids (2, 4, 5, 6, 7, …: `VarLong`, string, component, item stack, …) are
/// deliberately **not** decoded here: a wrong guess would silently misparse a
/// later entry and desynchronise the rest of the list, so
/// [`SetEntityData::decode`] rejects them with an explicit error instead
/// (AGENTS.md section 3.3). Adding one is a new variant plus its `type_id`
/// arm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetadataValue {
    /// Type id [`METADATA_TYPE_BYTE`]: a signed byte held as its raw `u8`.
    Byte(u8),
    /// Type id [`METADATA_TYPE_VARINT`].
    VarInt(i32),
    /// Type id [`METADATA_TYPE_FLOAT`].
    Float(f32),
    /// Type id [`METADATA_TYPE_ITEM_STACK`]: an item and a count.
    ///
    /// **Data components are not modelled.** The wire carries a patch of added and removed components after the
    /// item id, and a captured stack with none sends `00 00`; this variant writes that empty patch, so a stack
    /// that carries components would be sent as though it carried none. Named here rather than discovered by a
    /// caller, and the reason the fields are the two this build can be honest about.
    ItemStack {
        /// How many items the stack holds.
        count: i32,
        /// Item registry id.
        item_id: i32,
    },
}

impl MetadataValue {
    /// The wire type id written before the value.
    #[must_use]
    pub const fn type_id(self) -> i32 {
        match self {
            Self::Byte(_) => METADATA_TYPE_BYTE,
            Self::VarInt(_) => METADATA_TYPE_VARINT,
            Self::Float(_) => METADATA_TYPE_FLOAT,
            Self::ItemStack { .. } => METADATA_TYPE_ITEM_STACK,
        }
    }

    /// Write the value (not the index or type id) to `writer`.
    pub fn encode(self, writer: &mut PacketWriter) {
        match self {
            Self::Byte(value) => writer.write_u8(value),
            Self::VarInt(value) => writer.write_varint(value),
            Self::Float(value) => writer.write_f32(value),
            Self::ItemStack { count, item_id } => {
                // The component patch: nothing added, nothing removed. A captured stack with no components sends
                // exactly these two bytes, and a dropped item is a stack with no components.
                writer.write_varint(count);
                writer.write_varint(item_id);
                writer.write_varint(0);
                writer.write_varint(0);
            }
        }
    }

    /// Read a value of the given wire type id.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for an unmodelled type id or a truncated value.
    pub fn decode(reader: &mut PacketReader<'_>, type_id: i32) -> ServerResult<Self> {
        match type_id {
            METADATA_TYPE_BYTE => Ok(Self::Byte(reader.read_u8()?)),
            METADATA_TYPE_VARINT => Ok(Self::VarInt(reader.read_varint()?)),
            METADATA_TYPE_FLOAT => Ok(Self::Float(reader.read_f32()?)),
            METADATA_TYPE_ITEM_STACK => {
                let count = reader.read_varint()?;
                let item_id = reader.read_varint()?;
                // The patch this build does not model. Refusing a non-empty one is the alternative to silently
                // discarding it, and nothing in this capture carries one.
                let added = reader.read_varint()?;
                let removed = reader.read_varint()?;
                if added != 0 || removed != 0 {
                    return Err(ServerError::Protocol(format!(
                        "item stack carries {added} added and {removed} removed data components, which this build \
 does not model"
                    )));
                }
                Ok(Self::ItemStack { count, item_id })
            }
            other => Err(ServerError::Protocol(format!(
                "unmodelled entity metadata type id {other}"
            ))),
        }
    }
}

/// `minecraft:set_entity_data` (clientbound play).
///
/// Body: `VarInt` entity id, then metadata entries terminated by
/// [`METADATA_TERMINATOR`]:
///
/// ```text
/// per entry: u8 index, VarInt type id, <value>
/// end:       u8 0xFF
/// ```
///
/// The index is the metadata field slot (health, air, custom name, …), not a
/// sequence number; slots may be sent in any order and a later packet may
/// resend a slot. The terminator is mandatory — a payload that simply runs out
/// of bytes is an error, because a client would keep reading.
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityData {
    /// Entity the entries apply to.
    pub entity_id: i32,
    /// `(index, value)` pairs in wire order.
    pub entries: Vec<(u8, MetadataValue)>,
}

impl Packet for SetEntityData {
    const ID: i32 = clientbound::play::SET_ENTITY_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let entity_id = reader.read_varint()?;
        let mut entries = Vec::new();
        loop {
            let index = reader.read_u8()?;
            if index == METADATA_TERMINATOR {
                break;
            }
            if entries.len() >= MAX_METADATA_ENTRIES {
                return Err(ServerError::Protocol(format!(
                    "more than {MAX_METADATA_ENTRIES} metadata entries without a terminator"
                )));
            }
            let type_id = reader.read_varint()?;
            entries.push((index, MetadataValue::decode(&mut reader, type_id)?));
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_entity_data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { entity_id, entries })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        for (index, value) in &self.entries {
            writer.write_u8(*index);
            writer.write_varint(value.type_id());
            value.encode(&mut writer);
        }
        writer.write_u8(METADATA_TERMINATOR);
        Ok(writer.finish())
    }
}

/// Cap on metadata entries in one [`SetEntityData`].
///
/// Vanilla's entity data table has well under a hundred slots; the cap only
/// exists so a payload that never reaches its terminator cannot loop forever.
pub const MAX_METADATA_ENTRIES: usize = 256;

/// An item stack as it appears in inventory packets (protocol 775).
///
/// ```text
/// VarInt item id        (0 = empty; nothing follows)
/// if id != 0:
///   VarInt count
///   VarInt number of data components
///   per component: VarInt type id, then component data
/// ```
///
/// Protocol 775 does **not** carry the pre-1.20.5 `slot` / `change count` bytes
/// after the count. Data component payloads are not modelled here: each is
/// carried as pre-encoded bytes, which keeps the encoder honest about the
/// framing it *does* know (id, count, component count, component type id)
/// without inventing component codecs for Phase 04. The empty stack is
/// `item_id == 0`, and then `count`/`components` must be empty too.
/// One optional slot on the wire, exactly as 26.1.2's
/// `ItemStack.createOptionalStreamCodec` reads it (`ItemStack$1` +
/// `DataComponentPatch$3`, bytecode-read from the jar):
///
/// `count VarInt`; a count `<= 0` is the whole stack (bare `0x00`, the only
/// bytes an empty slot ever occupies). Otherwise `item id VarInt`, then the
/// component patch: `added-count VarInt`, that many `(type, value)` entries,
/// `removed-count VarInt`, that many bare type ids.
///
/// Two consequences this crate got wrong before the P14-09 walk (a picked-up
/// cobblestone was the first non-empty stack a real client ever had to
/// decode from us, and it failed): the count comes **first**, not the id,
/// and an empty patch is two zero bytes (`00 00`), not one. Empty slots encode
/// identically either way (`0x00`), which is why every inventory sync before
/// that walk looked fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    /// Item registry id; `0` means "empty stack".
    pub item_id: i32,
    /// Stack size; ignored when `item_id` is 0.
    pub count: i32,
    /// Added `(component type id, pre-encoded component data)` pairs.
    /// Removed components are unmodelled: this crate never sends any and
    /// refuses to receive any rather than dropping them silently.
    pub components: Vec<(i32, Vec<u8>)>,
}

impl ItemStack {
    /// The empty stack (`item_id == 0`), written as a bare `VarInt 0`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            item_id: 0,
            count: 0,
            components: Vec::new(),
        }
    }

    /// Whether this stack encodes as a bare `VarInt 0`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.item_id == 0
    }

    /// A stack with `count` items and no data components.
    #[must_use]
    pub const fn simple(item_id: i32, count: i32) -> Self {
        Self {
            item_id,
            count,
            components: Vec::new(),
        }
    }

    /// Encode the stack, returning the number of bytes written.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] for a negative count or a component count
    /// that does not fit a `VarInt` (server-authored data).
    pub fn encode(&self, writer: &mut PacketWriter) -> ServerResult<usize> {
        let before = writer.len();
        if self.count < 0 {
            return Err(ServerError::Invariant(format!(
                "item stack count {} is negative",
                self.count
            )));
        }
        // Vanilla reads the count first and stops at `<= 0`; an id-0 stack
        // is air by registry lookup, so both spellings encode as bare zero.
        if self.count == 0 || self.item_id == 0 {
            writer.write_varint(0);
            return Ok(writer.len() - before);
        }
        writer.write_varint(self.count);
        writer.write_varint(self.item_id);
        writer.write_varint(packed_len(self.components.len())?);
        for (type_id, data) in &self.components {
            writer.write_varint(*type_id);
            writer.write_bytes(data);
        }
        // Removed components: always empty on send (unmodelled).
        writer.write_varint(0);
        Ok(writer.len() - before)
    }

    /// Decode one stack.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for a negative item id, an absurd component
    /// count, a removed-component list (unmodelled — refused, never dropped),
    /// or a truncated payload.
    pub fn decode(reader: &mut PacketReader<'_>) -> ServerResult<Self> {
        let count = reader.read_varint()?;
        if count <= 0 {
            return Ok(Self::empty());
        }
        let item_id = reader.read_varint()?;
        if item_id < 0 {
            return Err(ServerError::Protocol(format!(
                "item stack id {item_id} is negative"
            )));
        }
        if item_id == 0 {
            // `Item.byId(0)` is air: a positive count of nothing is nothing.
            return Ok(Self::empty());
        }
        let added = read_count(reader, "item component", MAX_ITEM_COMPONENTS)?;
        // Component payload lengths are not self-describing in a way this crate
        // models, so each component owns the bytes it declares. Without a
        // length field the only safe assumption is "the rest of the packet",
        // which is why this decoder accepts at most zero added components and
        // rejects anything else rather than guessing.
        if added > 0 {
            return Err(ServerError::Protocol(format!(
                "{added} item data components present but component \
                 payload framing is unmodelled"
            )));
        }
        let removed = read_count(reader, "removed item component", MAX_ITEM_COMPONENTS)?;
        if removed > 0 {
            return Err(ServerError::Protocol(format!(
                "{removed} removed item components present but removed \
                 components are unmodelled"
            )));
        }
        Ok(Self {
            item_id,
            count,
            components: Vec::new(),
        })
    }
}

/// Cap on data components in one [`ItemStack`].
///
/// Only `0` is currently decodable (see [`ItemStack::decode`]); the constant
/// exists so the rejection message is a range check rather than a magic number.
pub const MAX_ITEM_COMPONENTS: usize = 1024;

/// `minecraft:container_set_slot` (clientbound play).
///
/// Body: container-id `VarInt`, state-id `VarInt`, `i16` slot index, then the
/// [`ItemStack`]. The container id is a `VarInt` on the wire
/// (`FriendlyByteBuf.readContainerId`, bytecode-read): window `0` is the
/// player inventory, `-1` the cursor. The state id is echoed by the client in
/// `container_click` so a stale click can be detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetSlot {
    /// Window the slot belongs to.
    pub window_id: i32,
    /// Inventory state id the client last acknowledged.
    pub state_id: i32,
    /// Slot index within the window.
    pub slot: i16,
    /// New contents of the slot.
    pub item: ItemStack,
}

impl Packet for ContainerSetSlot {
    const ID: i32 = clientbound::play::CONTAINER_SET_SLOT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let state_id = reader.read_varint()?;
        let slot = reader.read_i16()?;
        let item = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_slot has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            state_id,
            slot,
            item,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_i16(self.slot);
        self.item.encode(&mut writer)?;
        Ok(writer.finish())
    }
}

/// `minecraft:container_set_content` (clientbound play).
///
/// Body: container-id `VarInt`, state-id `VarInt`, `VarInt` slot count, that
/// many [`ItemStack`]s, then the carried (cursor) stack. The carried stack is
/// **not** part of the count — it is written unconditionally, even when empty —
/// so an off-by-one here shifts every remaining stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetContent {
    /// Window being filled.
    pub window_id: i32,
    /// Inventory state id the client last acknowledged.
    pub state_id: i32,
    /// Contents of the window's slots, in slot-index order.
    pub slots: Vec<ItemStack>,
    /// Stack held on the cursor.
    pub carried: ItemStack,
}

/// Largest slot count accepted in one [`ContainerSetContent`].
///
/// A large chest holds 54 slots and the player inventory 46; the cap leaves
/// room for any container vanilla currently defines.
pub const MAX_CONTAINER_SLOTS: usize = 256;

impl Packet for ContainerSetContent {
    const ID: i32 = clientbound::play::CONTAINER_SET_CONTENT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let state_id = reader.read_varint()?;
        let count = read_count(&mut reader, "container slot", MAX_CONTAINER_SLOTS)?;
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            slots.push(ItemStack::decode(&mut reader)?);
        }
        let carried = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_content has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            state_id,
            slots,
            carried,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_varint(packed_len(self.slots.len())?);
        for slot in &self.slots {
            slot.encode(&mut writer)?;
        }
        self.carried.encode(&mut writer)?;
        Ok(writer.finish())
    }
}

/// Vanilla `MenuType` registry ids for the windows P12 opens.
///
/// Source: `javap -c -p` on `net.minecraft.world.inventory.MenuType` from the
/// official 26.1.2 server jar — the static initialiser registers in this order,
/// and `BuiltInRegistries.MENU` assigns 0..N in registration order:
///
/// ```text
/// 0 generic_9x1, 1 generic_9x2, 2 generic_9x3, 3 generic_9x4, 4 generic_9x5,
/// 5 generic_9x6, 6 generic_3x3, 7 crafter_3x3, 8 anvil, 9 beacon,
/// 10 blast_furnace, 11 brewing_stand, 12 crafting, 13 enchantment, 14 furnace,
/// 15 grindstone, 16 hopper, 17 lectern, 18 loom, 19 merchant, 20 shulker_box,
/// 21 smithing, 22 smoker, 23 cartography_table, 24 stonecutter
/// ```
///
/// Only the four P12 needs are named here; adding a fifth menu is adding a
/// constant, not re-deriving the table.
pub const MENU_GENERIC_9X3: i32 = 2;
/// Double chest (54 slots).
pub const MENU_GENERIC_9X6: i32 = 5;
/// Furnace (3 slots: input, fuel, output).
pub const MENU_FURNACE: i32 = 14;
/// Hopper (5 slots).
pub const MENU_HOPPER: i32 = 16;

/// `minecraft:open_screen` (clientbound play 59).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundOpenScreenPacket` (26.1.2):
/// `containerId` via `ByteBufCodecs.CONTAINER_ID` (a `VarInt`), then `type` via
/// `ByteBufCodecs.registry(Registries.MENU)` (a `VarInt` registry id), then
/// `title` via `ComponentSerialization.TRUSTED_STREAM_CODEC` (network NBT —
/// the same encoding [`SystemChat`] uses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenScreen {
    /// Window id the client will use in `container_click` (non-zero).
    pub window_id: i32,
    /// Vanilla `MenuType` registry id (see `MENU_*` above).
    pub menu_type: i32,
    /// Window title.
    pub title: TextComponent,
}

impl Packet for OpenScreen {
    const ID: i32 = clientbound::play::OPEN_SCREEN;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let menu_type = reader.read_varint()?;
        let rest = reader.take_remaining();
        let mut rest = rest;
        let nbt = Nbt::read_network(&mut rest)?;
        if !rest.is_empty() {
            return Err(ServerError::Protocol(format!(
                "open_screen has {} trailing bytes",
                rest.len()
            )));
        }
        let title = super::config::text_from_nbt(&nbt)?;
        Ok(Self {
            window_id,
            menu_type,
            title,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.menu_type);
        let mut bytes = Vec::new();
        self.title.to_nbt().write_network(&mut bytes)?;
        writer.write_bytes(&bytes);
        Ok(writer.finish())
    }
}

/// `minecraft:container_set_data` (clientbound play 19).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundContainerSetDataPacket`:
/// `readContainerId` (`VarInt`), then `readShort` property, then `readShort`
/// value. Furnace progress rides this: property 0 = burn remaining,
/// 1 = burn total, 2 = cook progress, 3 = cook total (vanilla
/// `FurnaceMenu` data slots, asserted by the furnace tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerSetData {
    /// Window id.
    pub window_id: i32,
    /// Property id.
    pub property: i16,
    /// Property value.
    pub value: i16,
}

impl Packet for ContainerSetData {
    const ID: i32 = clientbound::play::CONTAINER_SET_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let property = reader.read_i16()?;
        let value = reader.read_i16()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            property,
            value,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_i16(self.property);
        writer.write_i16(self.value);
        Ok(writer.finish())
    }
}

/// `minecraft:container_close` (clientbound play 17).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundContainerClosePacket`:
/// a single `readContainerId` (`VarInt`). Tells the client to close a window
/// the server no longer tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerClose {
    /// Window id to close.
    pub window_id: i32,
}

impl Packet for ContainerClose {
    const ID: i32 = clientbound::play::CONTAINER_CLOSE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_close has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { window_id })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        Ok(writer.finish())
    }
}

/// `minecraft:set_cursor_item` (clientbound play 96).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundSetCursorItemPacket`:
/// a single `ItemStack.OPTIONAL_STREAM_CODEC` — the same optional stack
/// encoding [`ItemStack`] already implements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCursorItem {
    /// Stack on the cursor.
    pub item: ItemStack,
}

impl Packet for SetCursorItem {
    const ID: i32 = clientbound::play::SET_CURSOR_ITEM;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let item = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_cursor_item has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { item })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        self.item.encode(&mut writer)?;
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
        /// Window id (0 is the player inventory).
        window_id: u8,
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
                window_id: reader.read_u8()?,
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
            serverbound::play::CLIENT_TICK_END => {
                if !reader.is_empty() {
                    return Err(ServerError::Protocol(
                        "client_tick_end must be empty".to_owned(),
                    ));
                }
                Some(Self::ClientTickEnd)
            }
            _ => None,
        };
        Ok(intent)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_MAX_CHARS, ConfigurationAcknowledged, ContainerClose, ContainerSetData,
        ForgetLevelChunk, JoinGame, KeepAlive, LightData, LightUpdate, MENU_FURNACE,
        MENU_GENERIC_9X3, MENU_GENERIC_9X6, MENU_HOPPER, OpenScreen, PlayDisconnect, PlayIntent,
        PlayPingRequest, PlayPong, PlayerPosition, SIGNATURE_LEN, SetChunkCacheCenter,
        SetChunkCacheRadius, SetCursorItem,
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
        // world age 6000, one clock: overworld (id 0 → wire 1), 20000 ticks,
        // no partial, normal rate.
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
                0x01, // overworld reference (id + 1)
                0xA0, 0x9C, 0x01, // 20000 as VarLong
                0x00, 0x00, 0x00, 0x00, // partial 0.0
                0x3F, 0x80, 0x00, 0x00, // rate 1.0
            ]
        );
        assert_eq!(SetTime::decode(&body).expect("decodes"), packet);
        // An inline clock (reference 0) is refused, not guessed at.
        let mut inline = body.clone();
        inline[9] = 0x00;
        assert!(SetTime::decode(&inline).is_err());
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
        SystemChat, block_position, packing, unpack_block_position,
    };
    use crate::nbt::Nbt;
    use crate::wire::PacketWriter;

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
            super::decode_light_arrays(&mut crate::wire::PacketReader::new(&writer.finish()), &[0])
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
            super::decode_light_arrays(
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
            super::read_light_mask(&mut crate::wire::PacketReader::new(&negative), "sky").is_err()
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
            super::read_light_mask(&mut crate::wire::PacketReader::new(&beyond), "sky").is_err()
        );

        // A hostile long count is refused before anything is allocated.
        let mut greedy = PacketWriter::new();
        greedy.write_varint(99);
        let greedy = greedy.finish();
        assert!(
            super::read_light_mask(&mut crate::wire::PacketReader::new(&greedy), "sky").is_err()
        );

        // **The empty mask is a single `0x00` byte in both encodings**, which is the whole of KD-44: reading
        // the masks as `VarInt`s agrees with the real wire for an empty mask and disagrees for any other, so
        // the defect survived until a server sent one with content.
        assert_eq!(
            super::read_light_mask(&mut crate::wire::PacketReader::new(&[0x00]), "sky")
                .expect("empty mask"),
            Vec::<u32>::new()
        );

        // A mask with content decodes to its set indices.
        let mut two = PacketWriter::new();
        two.write_varint(1);
        two.write_i64(0b110);
        let two = two.finish();
        assert_eq!(
            super::read_light_mask(&mut crate::wire::PacketReader::new(&two), "sky")
                .expect("two sections"),
            vec![1, 2]
        );

        // And it survives a round trip through the writer, including the trailing-zero trim that makes the
        // output byte-identical to vanilla's `BitSet.toLongArray()`.
        let mut round_trip = PacketWriter::new();
        super::write_light_mask(&mut round_trip, &[1, 2]);
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
}
