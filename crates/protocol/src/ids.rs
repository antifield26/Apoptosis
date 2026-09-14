//! Protocol constants and packet ids for Java 26.1 (protocol 775).
//!
//! **Authoritative source**: `docs/protocol/packet-ids-775.tsv`, machine-extracted
//! from the official 26.1.2 server jar. Vanilla registers packets with
//! `ProtocolInfoBuilder.addPacket` from a static initialiser and each call takes
//! the next index, so the order of the `getstatic PacketTypes.<NAME>`
//! instructions in that initialiser *is* the id table
//! (`target/vanilla-26.1.2/packets_from_jar.py`; method and jar hash in
//! `docs/research/provenance.md`).
//!
//! The Phase 02 table was transcribed from a reference snapshot and was **wrong
//! for `chat_command`** (8 instead of 7 — a real 26.1.2 client would have had
//! `/commands` decoded as a chat message). `crates/protocol/tests/packet_ids.rs`
//! now asserts every constant below against the extracted table, so a typo fails
//! the build instead of reaching the wire.

/// Network protocol version served by this crate.
pub const PROTOCOL_VERSION: i32 = 775;

/// Version string used on status/config wire surfaces.
pub const VERSION_NAME: &str = "26.1.2";

/// Version string advertised to clients in the server list (matches the
/// 26.1.x family; protocol compatibility is what clients gate on).
pub const WIRE_VERSION_NAME: &str = "26.1";

/// Connection states of the modern login lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// First packet only: the intention/handshake.
    Handshake,
    /// Server-list ping flow.
    Status,
    /// Mojang/offline authentication and compression negotiation.
    Login,
    /// Registry/tag synchronisation before entering the world.
    Configuration,
    /// In-world play state.
    Play,
}

/// Packet direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Client to server.
    Serverbound,
    /// Server to client.
    Clientbound,
}

/// Serverbound (client → server) packet ids.
pub mod serverbound {
    /// Handshake state packets.
    pub mod handshake {
        /// `minecraft:intention`
        pub const INTENTION: i32 = 0;
    }

    /// Status state packets.
    pub mod status {
        /// `minecraft:status_request`
        pub const STATUS_REQUEST: i32 = 0;
        /// `minecraft:ping_request`
        pub const PING_REQUEST: i32 = 1;
    }

    /// Login state packets.
    pub mod login {
        /// `minecraft:hello` (`LoginStart`)
        pub const HELLO: i32 = 0;
        /// `minecraft:key` (`EncryptionResponse`, online mode only)
        pub const KEY: i32 = 1;
        /// `minecraft:custom_query_answer`
        pub const CUSTOM_QUERY_ANSWER: i32 = 2;
        /// `minecraft:login_acknowledged`
        pub const LOGIN_ACKNOWLEDGED: i32 = 3;
        /// `minecraft:cookie_response`
        pub const COOKIE_RESPONSE: i32 = 4;
    }

    /// Configuration state packets.
    pub mod config {
        /// `minecraft:client_information`
        pub const CLIENT_INFORMATION: i32 = 0;
        /// `minecraft:cookie_response`
        pub const COOKIE_RESPONSE: i32 = 1;
        /// `minecraft:custom_payload`
        pub const CUSTOM_PAYLOAD: i32 = 2;
        /// `minecraft:finish_configuration` (acknowledgement)
        pub const FINISH_CONFIGURATION: i32 = 3;
        /// `minecraft:keep_alive`
        pub const KEEP_ALIVE: i32 = 4;
        /// `minecraft:pong`
        pub const PONG: i32 = 5;
        /// `minecraft:resource_pack`
        pub const RESOURCE_PACK: i32 = 6;
        /// `minecraft:select_known_packs`
        pub const SELECT_KNOWN_PACKS: i32 = 7;
        /// `minecraft:custom_click_action`
        pub const CUSTOM_CLICK_ACTION: i32 = 8;
        /// `minecraft:accept_code_of_conduct`
        pub const ACCEPT_CODE_OF_CONDUCT: i32 = 9;
    }

    /// Play state packets (subset modelled by this server).
    pub mod play {
        /// `minecraft:accept_teleportation`
        pub const ACCEPT_TELEPORTATION: i32 = 0;
        /// `minecraft:attack`
        pub const ATTACK: i32 = 1;
        /// `minecraft:chat_command` (unsigned command, no leading slash)
        pub const CHAT_COMMAND: i32 = 7;
        /// `minecraft:chat_command_signed` — unmodelled; the id is recorded so
        /// the gap between `chat_command` and `chat` is explicit.
        pub const CHAT_COMMAND_SIGNED: i32 = 8;
        /// `minecraft:chat`
        pub const CHAT: i32 = 9;
        /// `minecraft:client_tick_end` — a client closing its own tick.
        ///
        /// **Not an intent**, and the reason is worth stating: a server that does not wait on a client's tick
        /// may drop this, which is why it went unmodelled for so long -- nothing failed. What it did do was
        /// log `unmodelled play packet` **about fourteen times a second per player**, found in a real client's
        /// session (P10-11) and by nothing the suite runs. The id is named here so that fixing the log is
        /// written against a name rather than a literal 13.
        pub const CLIENT_TICK_END: i32 = 13;
        /// `minecraft:client_command` (respawn, stats, …)
        pub const CLIENT_COMMAND: i32 = 12;
        /// `minecraft:client_information`
        pub const CLIENT_INFORMATION: i32 = 14;
        /// `minecraft:command_suggestion`
        pub const COMMAND_SUGGESTION: i32 = 15;
        /// `minecraft:configuration_acknowledged` (play → configuration)
        pub const CONFIGURATION_ACKNOWLEDGED: i32 = 16;
        /// `minecraft:container_click`
        pub const CONTAINER_CLICK: i32 = 18;
        /// `minecraft:container_close`
        pub const CONTAINER_CLOSE: i32 = 19;
        /// `minecraft:interact`
        pub const INTERACT: i32 = 26;
        /// `minecraft:keep_alive`
        pub const KEEP_ALIVE: i32 = 28;
        /// `minecraft:move_player_pos`
        pub const MOVE_PLAYER_POS: i32 = 30;
        /// `minecraft:move_player_pos_rot`
        pub const MOVE_PLAYER_POS_ROT: i32 = 31;
        /// `minecraft:move_player_rot`
        pub const MOVE_PLAYER_ROT: i32 = 32;
        /// `minecraft:move_player_status_only`
        pub const MOVE_PLAYER_STATUS_ONLY: i32 = 33;
        /// `minecraft:ping_request`
        pub const PING_REQUEST: i32 = 38;
        /// `minecraft:player_action` (dig/place/drop/swap)
        pub const PLAYER_ACTION: i32 = 41;
        /// `minecraft:player_command`
        pub const PLAYER_COMMAND: i32 = 42;
        /// `minecraft:player_input`
        pub const PLAYER_INPUT: i32 = 43;
        /// `minecraft:player_loaded`
        pub const PLAYER_LOADED: i32 = 44;
        /// `minecraft:pong`
        pub const PONG: i32 = 45;
        /// `minecraft:set_carried_item` (hotbar selection)
        pub const SET_CARRIED_ITEM: i32 = 53;
        /// `minecraft:set_creative_mode_slot` — unmodelled; recorded because a
        /// creative client may send it and we must ignore, not misread, it.
        pub const SET_CREATIVE_MODE_SLOT: i32 = 56;
        /// `minecraft:swing` (arm animation)
        pub const SWING: i32 = 63;
        /// `minecraft:use_item_on` (block placement)
        pub const USE_ITEM_ON: i32 = 66;
        /// `minecraft:use_item` (item use)
        pub const USE_ITEM: i32 = 67;
    }
}

/// Clientbound (server → client) packet ids.
pub mod clientbound {
    /// Status state packets.
    pub mod status {
        /// `minecraft:status_response`
        pub const STATUS_RESPONSE: i32 = 0;
        /// `minecraft:pong_response`
        pub const PONG_RESPONSE: i32 = 1;
    }

    /// Login state packets.
    pub mod login {
        /// `minecraft:login_disconnect`
        pub const LOGIN_DISCONNECT: i32 = 0;
        /// `minecraft:hello` (`EncryptionRequest`, online mode only)
        pub const HELLO: i32 = 1;
        /// `minecraft:login_finished`
        pub const LOGIN_FINISHED: i32 = 2;
        /// `minecraft:login_compression`
        pub const LOGIN_COMPRESSION: i32 = 3;
        /// `minecraft:custom_query`
        pub const CUSTOM_QUERY: i32 = 4;
        /// `minecraft:cookie_request`
        pub const COOKIE_REQUEST: i32 = 5;
    }

    /// Configuration state packets.
    pub mod config {
        /// `minecraft:cookie_request`
        pub const COOKIE_REQUEST: i32 = 0;
        /// `minecraft:custom_payload`
        pub const CUSTOM_PAYLOAD: i32 = 1;
        /// `minecraft:disconnect`
        pub const DISCONNECT: i32 = 2;
        /// `minecraft:finish_configuration`
        pub const FINISH_CONFIGURATION: i32 = 3;
        /// `minecraft:keep_alive`
        pub const KEEP_ALIVE: i32 = 4;
        /// `minecraft:ping`
        pub const PING: i32 = 5;
        /// `minecraft:reset_chat`
        pub const RESET_CHAT: i32 = 6;
        /// `minecraft:registry_data`
        pub const REGISTRY_DATA: i32 = 7;
        /// `minecraft:transfer`
        pub const TRANSFER: i32 = 11;
        /// `minecraft:update_enabled_features`
        pub const UPDATE_ENABLED_FEATURES: i32 = 12;
        /// `minecraft:update_tags`
        pub const UPDATE_TAGS: i32 = 13;
        /// `minecraft:select_known_packs`
        pub const SELECT_KNOWN_PACKS: i32 = 14;
        /// `minecraft:code_of_conduct`
        pub const CODE_OF_CONDUCT: i32 = 19;
    }

    /// Play state packets (subset modelled by this server).
    pub mod play {
        /// `minecraft:add_entity` — spawn an entity, carrying its registry type id.
        ///
        /// **The type id is a claim about the client's own registry.** `entity_type` is a *built-in* registry
        /// compiled into the client jar, not one this server sends, so the ids come from
        /// `crates/test-support/fixtures/registry/entity_types.tsv` (extracted by `EntityTypeProbe`) and **not**
        /// from the config payload — where `minecraft:entity_type` is a tag directory in `update_tags` rather
        /// than a registry. See P10-06.
        pub const ADD_ENTITY: i32 = 1;
        /// `minecraft:block_entity_data` — the contents of a block entity, on placement and on
        /// change.
        ///
        /// **The type id it carries is a built-in registry claim**: block entity types are compiled into the
        /// client jar, so their ids come from a jar extraction and not from the config payload — see
        /// P10-06 for why those two instruments are not interchangeable.
        pub const BLOCK_ENTITY_DATA: i32 = 6;
        /// `minecraft:block_update` (single block change)
        pub const BLOCK_UPDATE: i32 = 8;
        /// `minecraft:chunk_batch_finished`
        pub const CHUNK_BATCH_FINISHED: i32 = 11;
        /// `minecraft:chunk_batch_start`
        pub const CHUNK_BATCH_START: i32 = 12;
        /// `minecraft:container_set_content`
        pub const CONTAINER_SET_CONTENT: i32 = 18;
        /// `minecraft:container_set_slot`
        pub const CONTAINER_SET_SLOT: i32 = 20;
        /// `minecraft:disconnect`
        pub const DISCONNECT: i32 = 32;
        /// `minecraft:game_event` (weather, gamemode, …)
        pub const GAME_EVENT: i32 = 38;
        /// `minecraft:keep_alive`
        pub const KEEP_ALIVE: i32 = 44;
        /// `minecraft:level_chunk_with_light`
        pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 45;
        /// `minecraft:light_update`
        ///
        /// Carries the same light data as the tail of `level_chunk_with_light`, but with **`VarInt`**
        /// coordinates where that packet uses `i32` — settled with `javap` on the jar, not by analogy.
        pub const LIGHT_UPDATE: i32 = 48;
        /// `minecraft:login` (`JoinGame`)
        pub const LOGIN: i32 = 49;
        /// `minecraft:ping`
        pub const PING: i32 = 61;
        /// `minecraft:pong_response`
        pub const PONG_RESPONSE: i32 = 62;
        /// `minecraft:player_position` (teleport)
        pub const PLAYER_POSITION: i32 = 72;
        /// `minecraft:respawn`
        pub const RESPAWN: i32 = 82;
        /// `minecraft:section_blocks_update` (multi-block change)
        pub const SECTION_BLOCKS_UPDATE: i32 = 84;
        /// `minecraft:set_chunk_cache_center`
        pub const SET_CHUNK_CACHE_CENTER: i32 = 94;
        /// `minecraft:remove_entities` — despawn one or more entities by id.
        pub const REMOVE_ENTITIES: i32 = 77;
        /// `minecraft:set_chunk_cache_radius`
        pub const SET_CHUNK_CACHE_RADIUS: i32 = 95;
        /// `minecraft:set_default_spawn_position`
        pub const SET_DEFAULT_SPAWN_POSITION: i32 = 97;
        /// `minecraft:set_entity_data` (metadata, e.g. health/air)
        pub const SET_ENTITY_DATA: i32 = 99;
        /// `minecraft:move_entity_pos` — a relative move, deltas only.
        pub const MOVE_ENTITY_POS: i32 = 53;
        /// `minecraft:move_entity_pos_rot` — a relative move with rotation.
        pub const MOVE_ENTITY_POS_ROT: i32 = 54;
        /// `minecraft:move_entity_rot` — rotation only.
        pub const MOVE_ENTITY_ROT: i32 = 56;
        /// `minecraft:set_entity_motion` — set an entity's velocity.
        pub const SET_ENTITY_MOTION: i32 = 101;
        /// `minecraft:set_experience`
        pub const SET_EXPERIENCE: i32 = 103;
        /// `minecraft:set_health`
        pub const SET_HEALTH: i32 = 104;
        /// `minecraft:set_held_slot`
        pub const SET_HELD_SLOT: i32 = 105;
        /// `minecraft:set_player_inventory` (creative-mode inventory)
        pub const SET_PLAYER_INVENTORY: i32 = 108;
        /// `minecraft:set_time`
        pub const SET_TIME: i32 = 113;
        /// `minecraft:start_configuration`
        pub const START_CONFIGURATION: i32 = 118;
        /// `minecraft:disguised_chat` — a server message attributed to a player.
        ///
        /// **What a 26.1.2 console `say` produces.** A capture of a real server showed two `say` commands
        /// producing two `disguised_chat` and **zero** `system_chat`, so a server that answers chat with
        /// `system_chat` alone diverges from the client's expectation. See P10-10.
        pub const DISGUISED_CHAT: i32 = 33;
        /// `minecraft:player_chat` — a player's own message, broadcast.
        ///
        /// Carries the sender's UUID, index and signature rather than a preformatted line, which is why it
        /// is a different packet from the two above and not a variant of them.
        pub const PLAYER_CHAT: i32 = 65;
        /// `minecraft:system_chat`
        pub const SYSTEM_CHAT: i32 = 121;
    }
}

#[cfg(test)]
mod tests {
    use super::{PROTOCOL_VERSION, clientbound, serverbound};

    #[test]
    fn protocol_is_26_1_family() {
        assert_eq!(PROTOCOL_VERSION, 775);
    }

    /// Every constant is checked against the jar-extracted table in
    /// `tests/packet_ids.rs`; these spot values exist so a reader of this file
    /// sees the shape without opening the table.
    #[test]
    fn spot_check_ids_against_the_jar_table() {
        assert_eq!(serverbound::login::LOGIN_ACKNOWLEDGED, 3);
        assert_eq!(clientbound::login::LOGIN_FINISHED, 2);
        assert_eq!(clientbound::config::REGISTRY_DATA, 7);
        assert_eq!(clientbound::config::UPDATE_ENABLED_FEATURES, 12);
        assert_eq!(clientbound::play::LOGIN, 49);
        assert_eq!(clientbound::play::START_CONFIGURATION, 118);
        assert_eq!(serverbound::play::CONFIGURATION_ACKNOWLEDGED, 16);
        assert_eq!(serverbound::play::KEEP_ALIVE, 28);
        // The constant Phase 02 got wrong.
        assert_eq!(serverbound::play::CHAT_COMMAND, 7);
    }
}
