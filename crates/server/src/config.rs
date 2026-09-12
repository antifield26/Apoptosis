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
    /// `true` selects the online-mode provider boundary (P02-08, validated P08).
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            simulation: SimulationConfig::default(),
            storage: StorageConfig::default(),
            datapacks: DataPackConfig::default(),
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
