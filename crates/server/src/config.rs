//! Server configuration: TOML schema, defaults and validation (P01-06).
//!
//! The file format is TOML (AGENTS.md section 2). Every field has a
//! `Default` so a missing file still boots a sane offline-mode server, and
//! [`ServerConfig::validate`] rejects hostile or nonsensical values with
//! [`ServerError::Operational`] before any socket is bound.
//!
//! ```toml
//! [network]
//! bind = "127.0.0.1:25565"
//! max_players = 10
//! online_mode = false
//!
//! [simulation]
//! view_distance = 8
//!
//! [storage]
//! world_dir = "world"
//! autosave_ticks = 6000
//!
//! [access]
//! whitelist_enforced = false
//!
//! [gameplay]
//! spawn_protection = 16
//! pvp = true
//! idle_timeout_minutes = 0
//! simulation_distance = 8
//! default_gamemode = "survival"
//! hide_online_players = false
//! ```

use mc_core::error::{ServerError, ServerResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Minecraft Java 26.1.2 protocol version served by this server.
///
/// Single-version policy: 26.1.x shares protocol 775
/// (`docs/research/protocol-baseline.md` section 1). Displayed to clients as
/// `"26.1"` on the wire; `"26.1.2"` appears only in status/config surfaces.
pub const PROTOCOL_VERSION: i32 = 775;

/// Version string shown in server-list pings and logs.
pub const VERSION_NAME: &str = "26.1.2";

/// Top-level server configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Networking (bind address, slots, auth mode).
    pub network: NetworkConfig,
    /// Simulation pacing (kept minimal until P05 owns the scheduler).
    pub simulation: SimulationConfig,
    /// World storage paths and save pacing.
    pub storage: StorageConfig,
    /// Data pack paths.
    pub datapacks: DataPackConfig,
    /// Who may join (P19-01; P19-06 owns the rest of the properties).
    pub access: AccessConfig,
    /// Gameplay properties (P19-06; Vanilla `server.properties` analogues).
    pub gameplay: GameplayConfig,
    /// RCON admin protocol (P19-04; off unless configured).
    pub rcon: RconConfig,
}

/// Networking configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// Socket to bind, e.g. `127.0.0.1:25565`.
    pub bind: String,
    /// Concurrent player slots. Product contract: 10 (AGENTS.md section 2).
    pub max_players: u32,
    /// Mojang authentication. Product default: OFF (AGENTS.md section 2);
    /// `true` verifies logins against Mojang's session server (P19-05,
    /// ADR-0008): RSA handshake, `hasJoined` check, encrypted transport.
    pub online_mode: bool,
    /// Compression threshold in bytes; `-1` disables compression.
    pub compression_threshold: i32,
    /// Server-list description shown to clients.
    pub motd: String,
}

/// Simulation pacing configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SimulationConfig {
    /// Chunk view distance in chunks (radius). Vanilla default 10; our Pi-5
    /// default is 8 until the P08 benchmark says otherwise.
    pub view_distance: u32,
}

/// World storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// World directory (relative paths resolve against the process cwd).
    pub world_dir: PathBuf,
    /// Ticks between autosaves; 0 disables the timer (explicit saves only).
    pub autosave_ticks: u64,
    /// World seed for generation, when the operator names one
    /// (AUDIT-18 F-H1, AUDIT-19 D-19-M1).
    ///
    /// `None` (the default) means "no opinion": a world that records a seed
    /// keeps generating from it, and a fresh world generates from seed 0 (the
    /// long-standing default, stated not hidden).
    ///
    /// A recorded seed always wins, and **every world this build creates or
    /// opens records one**: the resolved seed is written into `level.dat` at
    /// creation and on the first boot of a world that records none
    /// (`crate::storage::WorldService`, AUDIT-19 D-19-M1). So setting or
    /// clearing this key cannot fork a world this server has already opened —
    /// it only seeds a world that records no seed at all, i.e. one written by
    /// another tool or by an older build, and only until its first boot here.
    /// A real 26.1 world records that seed in
    /// `data/minecraft/world_gen_settings.dat`, not in `level.dat`
    /// (AUDIT-19 D-19-H1); both are read.
    pub seed: Option<i64>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            simulation: SimulationConfig::default(),
            storage: StorageConfig::default(),
            datapacks: DataPackConfig::default(),
            access: AccessConfig::default(),
            gameplay: GameplayConfig::default(),
            rcon: RconConfig::default(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:25565".to_owned(),
            max_players: 10,
            online_mode: false,
            compression_threshold: 256,
            motd: "A Rust Minecraft Server".to_owned(),
        }
    }
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self { view_distance: 8 }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            world_dir: PathBuf::from("world"),
            autosave_ticks: 6000,
            seed: None,
        }
    }
}

/// Data pack paths.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DataPackConfig {
    /// Path to the vanilla data directory — the jar's `data/minecraft` — when it is available.
    ///
    /// `None` by default, and that is the honest default: the jar data is Mojang's and is **not**
    /// committed to this repository, so a server on a fresh checkout has none. The consequence is
    /// stated rather than hidden: with no vanilla data, no vanilla function, recipe or tag loads,
    /// and `/function` reports unknown names. Setting this to either the `data/minecraft`
    /// directory or a pack root containing it works, because the two are indistinguishable from
    /// outside and getting it wrong has the same symptom as configuring nothing.
    pub vanilla_data: Option<PathBuf>,
}

impl Default for DataPackConfig {
    fn default() -> Self {
        Self { vanilla_data: None }
    }
}

/// Join-access configuration (P19-01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AccessConfig {
    /// Enforce `whitelist.json` at login: a profile that is neither listed
    /// nor an operator is refused with Vanilla's message.
    ///
    /// Vanilla's `white-list` key, default off like Vanilla. `/whitelist
    /// on|off` toggles it live; a restart restores this value.
    pub whitelist_enforced: bool,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            whitelist_enforced: false,
        }
    }
}

/// Default game mode for new players (P19-06).
///
/// Config-side twin of `mc_entity::GameMode` with Vanilla's lowercase
/// names: kept here (rather than deriving serde over there) so `mc-entity`
/// gains no serialization dependency for one config key.
/// [`DefaultGameMode::game_mode`] is the conversion the join uses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultGameMode {
    /// Vanilla Survival (and this build's default).
    #[default]
    Survival,
    /// Vanilla Creative.
    Creative,
    /// Vanilla Adventure.
    Adventure,
    /// Vanilla Spectator.
    Spectator,
}

impl DefaultGameMode {
    /// The entity-layer game mode this key names.
    ///
    /// The two enums are the same four modes by construction; this is the
    /// only place they meet, so a new variant on either side is a compile
    /// error here rather than a silent fallback at join.
    #[must_use]
    pub const fn game_mode(self) -> mc_entity::GameMode {
        match self {
            Self::Survival => mc_entity::GameMode::Survival,
            Self::Creative => mc_entity::GameMode::Creative,
            Self::Adventure => mc_entity::GameMode::Adventure,
            Self::Spectator => mc_entity::GameMode::Spectator,
        }
    }
}

/// Gameplay properties (P19-06; Vanilla `server.properties` analogues).
///
/// Config + Game plumbing: every key reads, writes, validates and has a Game
/// accessor. Enforcement is P20-owned (named per key) with one exception that
/// is live: `default_gamemode`, applied to players who join **without** a
/// stored `playerdata` file (see [`GameplayConfig::default_gamemode`]). The
/// other exception is the exposure warning, which is a startup log, not
/// behaviour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GameplayConfig {
    /// Spawn protection radius in blocks (Vanilla `spawn-protection`,
    /// default 16; 0 disables). Enforcement (non-op edits refused in
    /// radius) is P20-owned.
    pub spawn_protection: u32,
    /// Whether players may damage each other (Vanilla `pvp`, default
    /// true). The damage gate is P20-owned.
    pub pvp: bool,
    /// Minutes of inactivity before disconnect (Vanilla
    /// `player-idle-timeout`, default 0 = disabled). The idle tracker is
    /// P20-owned (it needs per-tick activity timestamps).
    pub idle_timeout_minutes: u32,
    /// Tick radius in chunks (Vanilla `simulation-distance`). Stored now;
    /// ticking honours it in P20 (view distance stays the streaming
    /// radius until then).
    pub simulation_distance: u32,
    /// Game mode for new players (Vanilla `gamemode`).
    ///
    /// **Scope (AUDIT-19 A-07).** Applied at join to a player with no stored
    /// `playerdata` file and no in-memory state to restore — a genuinely new
    /// player. A returning player's own mode wins: a stored file carries the
    /// mode (the key is not consulted at all), and a reconnect inside one run
    /// restores the mode the session had when it left. There is no
    /// `/defaultgamemode` command in this build; changing the key means
    /// editing the config and restarting, and it never touches players who
    /// are already online (`/gamemode <mode>` changes the invoker's own mode
    /// only).
    pub default_gamemode: DefaultGameMode,
    /// Whether the status response hides the player sample (Vanilla
    /// `hide-online-players`, default false). Wiring is P20-owned.
    pub hide_online_players: bool,
}

impl Default for GameplayConfig {
    fn default() -> Self {
        Self {
            spawn_protection: 16,
            pvp: true,
            idle_timeout_minutes: 0,
            simulation_distance: 8,
            default_gamemode: DefaultGameMode::Survival,
            hide_online_players: false,
        }
    }
}

/// RCON admin protocol configuration (P19-04).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RconConfig {
    /// Serve RCON. Default off: an admin protocol that answers commands
    /// must be opted into, never on by accident.
    pub enabled: bool,
    /// Socket to bind. Default loopback (Vanilla's `127.0.0.1:25575`):
    /// RCON authenticates with a bare password, so it stays off the
    /// network unless the operator moves it deliberately.
    pub bind: String,
    /// RCON password. Empty by default, and enabling RCON with an empty
    /// password is refused at validation — an unauthenticated admin
    /// protocol must not boot.
    pub password: String,
}

impl Default for RconConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1:25575".to_owned(),
            password: String::new(),
        }
    }
}

impl ServerConfig {
    /// Parse TOML text into a validated config.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Operational`] on TOML syntax errors, unknown
    /// fields, or validation failures.
    pub fn from_toml(text: &str) -> ServerResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|e| ServerError::Operational(format!("invalid config TOML: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Load and validate a TOML config file.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Operational`] when the file cannot be read,
    /// parsed, or validated.
    pub fn load_from_file(path: &std::path::Path) -> ServerResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ServerError::Operational(format!("cannot read {}: {e}", path.display()))
        })?;
        Self::from_toml(&text)
    }

    /// Check every field against its documented bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Operational`] describing the first violation.
    pub fn validate(&self) -> ServerResult<()> {
        // Bind must be a parseable socket address; unparseable binds fail here
        // instead of panicking the listener task at startup.
        if self.network.bind.parse::<std::net::SocketAddr>().is_err() {
            return Err(ServerError::Operational(format!(
                "network.bind is not a socket address: {:?}",
                self.network.bind
            )));
        }
        if self.network.max_players == 0 || self.network.max_players > 100 {
            return Err(ServerError::Operational(format!(
                "network.max_players out of range (1..=100): {}",
                self.network.max_players
            )));
        }
        if self.simulation.view_distance < 2 || self.simulation.view_distance > 32 {
            return Err(ServerError::Operational(format!(
                "simulation.view_distance out of range (2..=32): {}",
                self.simulation.view_distance
            )));
        }
        if self.network.compression_threshold != -1
            && !(0..=65_536).contains(&self.network.compression_threshold)
        {
            return Err(ServerError::Operational(format!(
                "network.compression_threshold must be -1 or 0..=65536: {}",
                self.network.compression_threshold
            )));
        }
        if self.network.motd.chars().count() > 128 {
            return Err(ServerError::Operational(
                "network.motd must be at most 128 characters".to_owned(),
            ));
        }
        // RCON answers commands with a bare password: enabling it without
        // one must fail here, not boot an open admin port.
        if self.rcon.enabled && self.rcon.password.is_empty() {
            return Err(ServerError::Operational(
                "rcon.enabled refuses an empty rcon.password: set one or disable RCON".to_owned(),
            ));
        }
        // P19-06 gameplay bounds: typos rejected, policy untouched. Zero
        // disables spawn protection and idle timeout alike (Vanilla's
        // convention); the upper bounds reject misplaced digits, not intent.
        if self.gameplay.spawn_protection > 256 {
            return Err(ServerError::Operational(format!(
                "gameplay.spawn_protection out of range (0..=256): {}",
                self.gameplay.spawn_protection
            )));
        }
        if self.gameplay.idle_timeout_minutes > 1440 {
            return Err(ServerError::Operational(format!(
                "gameplay.idle_timeout_minutes out of range (0..=1440): {}",
                self.gameplay.idle_timeout_minutes
            )));
        }
        if self.gameplay.simulation_distance < 2 || self.gameplay.simulation_distance > 32 {
            return Err(ServerError::Operational(format!(
                "gameplay.simulation_distance out of range (2..=32): {}",
                self.gameplay.simulation_distance
            )));
        }
        if self.rcon.enabled && self.rcon.bind.parse::<std::net::SocketAddr>().is_err() {
            return Err(ServerError::Operational(format!(
                "rcon.bind is not a socket address: {:?}",
                self.rcon.bind
            )));
        }
        if self.storage.world_dir.as_os_str().is_empty() {
            return Err(ServerError::Operational(
                "storage.world_dir must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;

    #[test]
    fn defaults_match_product_contract() {
        let config = ServerConfig::default();
        assert_eq!(config.network.max_players, 10);
        assert!(!config.network.online_mode, "online mode defaults OFF");
        config.validate().expect("defaults must validate");
    }

    #[test]
    fn parses_documented_example() {
        let config = ServerConfig::from_toml(
            "[network]\nbind = \"127.0.0.1:25565\"\nmax_players = 10\nonline_mode = false\n\
             [simulation]\nview_distance = 8\n[storage]\nworld_dir = \"world\"\nautosave_ticks = 6000\n",
        )
        .expect("documented example must parse");
        assert_eq!(config.network.bind, "127.0.0.1:25565");
    }

    #[test]
    fn parses_an_explicit_world_seed() {
        let config =
            ServerConfig::from_toml("[storage]\nworld_dir = \"world\"\nseed = 1361882806\n")
                .expect("a seed must parse");
        assert_eq!(config.storage.seed, Some(1_361_882_806));
        let bare = ServerConfig::from_toml("[storage]\nworld_dir = \"world\"\n")
            .expect("missing seed must parse");
        assert_eq!(bare.storage.seed, None, "no seed is the default");
    }

    #[test]
    fn whitelist_enforcement_defaults_off_and_parses() {
        let bare = ServerConfig::default();
        assert!(
            !bare.access.whitelist_enforced,
            "an open server is the default, like Vanilla"
        );
        let config = ServerConfig::from_toml("[access]\nwhitelist_enforced = true\n")
            .expect("the access section must parse");
        assert!(config.access.whitelist_enforced);
    }

    #[test]
    fn gameplay_defaults_match_vanilla_analogues() {
        use super::{DefaultGameMode, GameplayConfig};
        let gameplay = GameplayConfig::default();
        assert_eq!(gameplay.spawn_protection, 16);
        assert!(gameplay.pvp, "Vanilla pvp defaults on");
        assert_eq!(gameplay.idle_timeout_minutes, 0, "0 disables, like Vanilla");
        assert_eq!(gameplay.simulation_distance, 8);
        assert_eq!(gameplay.default_gamemode, DefaultGameMode::Survival);
        assert!(!gameplay.hide_online_players);
        ServerConfig::default()
            .validate()
            .expect("defaults validate");
    }

    #[test]
    fn gameplay_parses_and_round_trips() {
        use super::DefaultGameMode;
        let config = ServerConfig::from_toml(
            "[gameplay]\nspawn_protection = 0\npvp = false\nidle_timeout_minutes = 15\n\
             simulation_distance = 10\ndefault_gamemode = \"creative\"\nhide_online_players = true\n",
        )
        .expect("a full gameplay section must parse");
        assert_eq!(config.gameplay.spawn_protection, 0);
        assert!(!config.gameplay.pvp);
        assert_eq!(config.gameplay.idle_timeout_minutes, 15);
        assert_eq!(config.gameplay.simulation_distance, 10);
        assert_eq!(config.gameplay.default_gamemode, DefaultGameMode::Creative);
        assert!(config.gameplay.hide_online_players);
        // Write path: what parses must serialize back to what parses.
        let text = toml::to_string(&config).expect("serializes");
        let back = ServerConfig::from_toml(&text).expect("round-trips");
        assert_eq!(back.gameplay, config.gameplay);
    }

    #[test]
    fn gameplay_refuses_unknown_keys_and_values() {
        assert!(
            ServerConfig::from_toml("[gameplay]\nevil = 1\n").is_err(),
            "unknown gameplay keys are refused like every other section"
        );
        for bad in [
            "[gameplay]\nspawn_protection = 257\n",
            "[gameplay]\nidle_timeout_minutes = 1441\n",
            "[gameplay]\nsimulation_distance = 1\n",
            "[gameplay]\nsimulation_distance = 33\n",
            "[gameplay]\ndefault_gamemode = \"hardcore\"\n",
        ] {
            assert!(
                ServerConfig::from_toml(bad).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn rcon_refuses_to_enable_without_a_password() {
        let bare = ServerConfig::default();
        assert!(!bare.rcon.enabled, "RCON is off unless configured");
        assert_eq!(bare.rcon.bind, "127.0.0.1:25575");
        let enabled = ServerConfig::from_toml("[rcon]\nenabled = true\npassword = \"s3cret\"\n")
            .expect("a passworded RCON parses");
        assert!(enabled.rcon.enabled);
        assert!(
            ServerConfig::from_toml("[rcon]\nenabled = true\n").is_err(),
            "enabling RCON with no password must fail validation"
        );
        assert!(
            ServerConfig::from_toml(
                "[rcon]\nenabled = true\npassword = \"s3cret\"\nbind = \"nope\"\n"
            )
            .is_err(),
            "an unparsable RCON bind must fail validation"
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = ServerConfig::from_toml("[network]\nbind = \"127.0.0.1:25565\"\nevil = 1\n")
            .expect_err("unknown fields must be rejected");
        assert!(
            matches!(err, super::ServerError::Operational(_)),
            "wrong variant: {err:?}"
        );
    }

    #[test]
    fn rejects_hostile_values() {
        for bad in [
            "[network]\nbind = \"not-an-addr\"\n",
            "[network]\nbind = \"127.0.0.1:25565\"\nmax_players = 0\n",
            "[network]\nbind = \"127.0.0.1:25565\"\nmax_players = 1000000\n",
            "[network]\nbind = \"127.0.0.1:25565\"\ncompression_threshold = 70000\n",
            "[network]\nbind = \"127.0.0.1:25565\"\ncompression_threshold = -2\n",
            "[simulation]\nview_distance = 1\n",
            "[simulation]\nview_distance = 64\n",
            "[storage]\nworld_dir = \"\"\n",
        ] {
            assert!(
                ServerConfig::from_toml(bad).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn accepts_disabled_compression_and_rejects_overlong_motd() {
        let config = ServerConfig::from_toml(
            "[network]\nbind = \"127.0.0.1:25565\"\ncompression_threshold = -1\n",
        )
        .expect("disabled compression is valid");
        assert_eq!(config.network.compression_threshold, -1);

        let long_motd = "x".repeat(129);
        let toml = format!("[network]\nbind = \"127.0.0.1:25565\"\nmotd = \"{long_motd}\"\n");
        assert!(ServerConfig::from_toml(&toml).is_err(), "motd cap enforced");
    }
}
