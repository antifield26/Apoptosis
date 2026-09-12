//! `level.dat` (P03-08).
//!
//! Modelled on the file a real 26.1.2 server wrote (`target/vanilla-26.1.2`,
//! dumped in the Phase 03 report (git history, tag `phase-09-final`)):
//!
//! ```text
//! Data: {
//!   `DataVersion`: 4790, Version: {Id: 4790, Name: "26.1.2", Series: "main", Snapshot: 0},
//!   version: 19133, LevelName: "world", LastPlayed: <ms>, GameType: 0, Time: 0,
//!   difficulty_settings: {difficulty: "easy", hardcore: 0, locked: 0},
//!   spawn: {pos: [x, y, z], pitch: 0.0, yaw: 0.0, dimension: "minecraft:overworld"},
//!   ServerBrands: ["vanilla"], WasModded: 0, initialized: 1, allowCommands: 0,
//!   DataPacks: {Enabled: ["vanilla"], Disabled: [...]}
//! }
//! ```
//!
//! Compared with older versions, 26.1 moved several things out of `level.dat`
//! entirely: world-gen settings now live in `data/minecraft/world_gen_settings.dat`,
//! day time in `data/minecraft/world_clocks.dat`, game rules in
//! `data/minecraft/game_rules.dat` and weather in `data/minecraft/weather.dat`.
//! Spawn and difficulty also changed shape — `SpawnX/SpawnY/SpawnZ/SpawnAngle`
//! became a `spawn` compound, and the `Difficulty` byte became a
//! `difficulty_settings` compound with a **string** difficulty. The reader
//! accepts both shapes so worlds written by tools targeting the older schema
//! still load; the writer always emits the 26.1 shape.

use crate::dimension::{Dimension, OVERWORLD_KEY};
use mc_core::error::{ServerError, ServerResult};
use mc_nbt::{Limits, NbtTag};

/// `DataVersion` written by 26.1.2 (measured: `level.dat`, every chunk and every
/// `data/minecraft/*.dat` in a vanilla 26.1.2 world carry this value).
pub const DATA_VERSION_26_1_2: i32 = 4790;

/// `Data.version`, the storage-format generation (measured: 19133).
pub const LEVEL_VERSION_26_1_2: i32 = 19133;

/// Version name recorded in `Data.Version.Name`.
pub const VERSION_NAME_26_1_2: &str = "26.1.2";

/// Version series recorded in `Data.Version.Series`.
pub const VERSION_SERIES: &str = "main";

/// Oldest `DataVersion` whose `level.dat`/chunk schema this crate models.
///
/// Below this value the pre-1.21.9 formats apply (different spawn/difficulty
/// shape, different chunk fields); we refuse rather than guess, because we ship
/// no datafixers.
pub const MIN_READABLE_DATA_VERSION: i32 = 4435;

/// Game difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// No hostile mobs, no hunger.
    Peaceful,
    /// Default for a fresh Vanilla world.
    Easy,
    /// Vanilla default for existing worlds.
    Normal,
    /// Hardcore-adjacent survival difficulty.
    Hard,
}

impl Difficulty {
    /// Key used by the 26.1 `difficulty_settings.difficulty` string field.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Peaceful => "peaceful",
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }

    /// Parse the 26.1 string form.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "peaceful" => Some(Self::Peaceful),
            "easy" => Some(Self::Easy),
            "normal" => Some(Self::Normal),
            "hard" => Some(Self::Hard),
            _ => None,
        }
    }

    /// Legacy numeric form (0..=3) used before 26.1.
    #[must_use]
    pub const fn legacy_id(self) -> i8 {
        match self {
            Self::Peaceful => 0,
            Self::Easy => 1,
            Self::Normal => 2,
            Self::Hard => 3,
        }
    }

    /// Parse the legacy numeric form.
    #[must_use]
    pub const fn from_legacy_id(id: i8) -> Option<Self> {
        match id {
            0 => Some(Self::Peaceful),
            1 => Some(Self::Easy),
            2 => Some(Self::Normal),
            3 => Some(Self::Hard),
            _ => None,
        }
    }
}

/// World spawn point (`Data.spawn` in 26.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnPoint {
    /// Block x.
    pub x: i32,
    /// Block y.
    pub y: i32,
    /// Block z.
    pub z: i32,
    /// Horizontal angle in degrees.
    pub yaw: f32,
    /// Vertical angle in degrees.
    pub pitch: f32,
    /// Dimension key the spawn belongs to.
    pub dimension: String,
}

impl Default for SpawnPoint {
    fn default() -> Self {
        Self {
            x: 0,
            y: 64,
            z: 0,
            yaw: 0.0,
            pitch: 0.0,
            dimension: OVERWORLD_KEY.to_owned(),
        }
    }
}

/// `Data.Version` — the version that last wrote the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    /// Version id (equals `DataVersion` for `main`-series releases).
    pub id: i32,
    /// Version name, e.g. `26.1.2`.
    pub name: String,
    /// Series id, `main` for releases.
    pub series: String,
    /// Whether the world was last written by a snapshot.
    pub snapshot: bool,
}

/// The decoded `level.dat`.
///
/// The boolean fields mirror the file's own flags one-to-one; grouping them
/// into a bitfield would obscure the mapping to `level.dat`.
#[allow(clippy::struct_excessive_bools)]
///
/// `extra` preserves any entry this model does not interpret, so a load/save
/// cycle never drops operator or datapack data (AGENTS.md section 3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelDat {
    /// Schema version of the stored data (4790 for 26.1.2).
    pub data_version: i32,
    /// `Data.Version`, if present.
    pub version: Option<VersionInfo>,
    /// `Data.version`: the storage generation (19133 for 26.1.x).
    pub level_version: i32,
    /// Display name of the world.
    pub level_name: String,
    /// Last play time in milliseconds since the epoch.
    pub last_played: i64,
    /// Vanilla game type (0 survival, 1 creative, 2 adventure, 3 spectator).
    pub game_type: i32,
    /// World age in ticks.
    pub time: i64,
    /// Difficulty.
    pub difficulty: Difficulty,
    /// Hardcore flag.
    pub hardcore: bool,
    /// Whether the difficulty is locked.
    pub difficulty_locked: bool,
    /// Spawn point.
    pub spawn: SpawnPoint,
    /// Brands that have written this world (`["vanilla"]` for a vanilla world).
    pub server_brands: Vec<String>,
    /// `WasModded` flag.
    pub was_modded: bool,
    /// `initialized` flag (spawn area prepared).
    pub initialized: bool,
    /// Whether cheats/commands are allowed.
    pub allow_commands: bool,
    /// Enabled datapacks.
    pub enabled_packs: Vec<String>,
    /// Disabled datapacks.
    pub disabled_packs: Vec<String>,
    /// Uninterpreted `Data` entries, preserved verbatim.
    pub extra: Vec<(String, NbtTag)>,
}

impl LevelDat {
    /// A fresh 26.1.2 `level.dat` for a new world.
    #[must_use]
    pub fn new(level_name: &str, last_played_millis: i64) -> Self {
        Self {
            data_version: DATA_VERSION_26_1_2,
            version: Some(VersionInfo {
                id: DATA_VERSION_26_1_2,
                name: VERSION_NAME_26_1_2.to_owned(),
                series: VERSION_SERIES.to_owned(),
                snapshot: false,
            }),
            level_version: LEVEL_VERSION_26_1_2,
            level_name: level_name.to_owned(),
            last_played: last_played_millis,
            game_type: 0,
            time: 0,
            difficulty: Difficulty::Easy,
            hardcore: false,
            difficulty_locked: false,
            spawn: SpawnPoint::default(),
            server_brands: vec!["vanilla".to_owned()],
            was_modded: false,
            initialized: false,
            allow_commands: false,
            enabled_packs: vec!["vanilla".to_owned()],
            disabled_packs: Vec::new(),
            extra: Vec::new(),
        }
    }

    /// Whether this server can interpret a world at this `DataVersion`.
    ///
    /// We ship no datafixers, so anything older than
    /// [`MIN_READABLE_DATA_VERSION`] or newer than [`DATA_VERSION_26_1_2`] is
    /// refused rather than silently misread.
    #[must_use]
    pub const fn is_supported_data_version(data_version: i32) -> bool {
        data_version >= MIN_READABLE_DATA_VERSION && data_version <= DATA_VERSION_26_1_2
    }

    /// Decode a `level.dat` root tag (the file's root is `{Data: {...}}`).
    ///
    /// Kept as one function on purpose: it is a flat field-by-field mapping and
    /// splitting it would scatter the tolerant-read rules across helpers.
    #[allow(clippy::too_many_lines)]
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when `Data` is missing or not a compound.
    pub fn from_nbt(root: &NbtTag) -> ServerResult<Self> {
        let data = root
            .get_compound("Data")
            .ok_or_else(|| ServerError::CorruptData("level.dat has no Data compound".to_owned()))?;

        let data_version = data
            .get_i32("DataVersion")
            .unwrap_or(MIN_READABLE_DATA_VERSION);
        let version = data.get_compound("Version").map(|tag| VersionInfo {
            id: tag.get_i32("Id").unwrap_or(data_version),
            name: tag
                .get_str("Name")
                .unwrap_or(VERSION_NAME_26_1_2)
                .to_owned(),
            series: tag.get_str("Series").unwrap_or(VERSION_SERIES).to_owned(),
            snapshot: tag.get_bool("Snapshot").unwrap_or(false),
        });

        // 26.1 shape first, then the pre-26.1 shapes.
        let (difficulty, hardcore, difficulty_locked) =
            match data.get_compound("difficulty_settings") {
                Some(settings) => (
                    settings
                        .get_str("difficulty")
                        .and_then(Difficulty::from_key)
                        .or_else(|| {
                            settings
                                .get_i8("difficulty")
                                .and_then(Difficulty::from_legacy_id)
                        })
                        .unwrap_or(Difficulty::Easy),
                    settings.get_bool("hardcore").unwrap_or(false),
                    settings.get_bool("locked").unwrap_or(false),
                ),
                None => (
                    data.get_i8("Difficulty")
                        .and_then(Difficulty::from_legacy_id)
                        .unwrap_or(Difficulty::Easy),
                    data.get_bool("hardcore").unwrap_or(false),
                    data.get_bool("DifficultyLocked").unwrap_or(false),
                ),
            };

        let spawn = match data.get_compound("spawn") {
            Some(spawn) => {
                let pos = spawn.get_list("pos").unwrap_or(&[]);
                let coord = |index: usize, default: i32| {
                    pos.get(index)
                        .and_then(NbtTag::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(default)
                };
                SpawnPoint {
                    x: coord(0, 0),
                    y: coord(1, 64),
                    z: coord(2, 0),
                    // A yaw/pitch outside `f32` range is not representable in the
                    // format either; fall back to the default rather than NaN.
                    yaw: spawn
                        .get_f64("yaw")
                        .filter(|v| v.is_finite())
                        .map_or(0.0, |v| v as f32),
                    pitch: spawn
                        .get_f64("pitch")
                        .filter(|v| v.is_finite())
                        .map_or(0.0, |v| v as f32),
                    dimension: spawn
                        .get_str("dimension")
                        .unwrap_or(OVERWORLD_KEY)
                        .to_owned(),
                }
            }
            None => SpawnPoint {
                x: data.get_i32("SpawnX").unwrap_or(0),
                y: data.get_i32("SpawnY").unwrap_or(64),
                z: data.get_i32("SpawnZ").unwrap_or(0),
                yaw: data
                    .get_f64("SpawnAngle")
                    .filter(|v| v.is_finite())
                    .map_or(0.0, |v| v as f32),
                pitch: 0.0,
                dimension: OVERWORLD_KEY.to_owned(),
            },
        };

        let data_packs = data.get_compound("DataPacks");
        let string_list = |parent: &NbtTag, key: &str| -> Vec<String> {
            parent
                .get_list(key)
                .unwrap_or(&[])
                .iter()
                .filter_map(|item| match item {
                    NbtTag::String(value) => Some(value.clone()),
                    _ => None,
                })
                .collect()
        };

        let known = [
            "DataVersion",
            "Version",
            "version",
            "LevelName",
            "LastPlayed",
            "GameType",
            "Time",
            "difficulty_settings",
            "Difficulty",
            "hardcore",
            "DifficultyLocked",
            "spawn",
            "SpawnX",
            "SpawnY",
            "SpawnZ",
            "SpawnAngle",
            "ServerBrands",
            "WasModded",
            "initialized",
            "allowCommands",
            "DataPacks",
        ];
        let extra = data
            .entries()
            .unwrap_or(&[])
            .iter()
            .filter(|(key, _)| !known.contains(&key.as_str()))
            .cloned()
            .collect();

        Ok(Self {
            data_version,
            version,
            level_version: data.get_i32("version").unwrap_or(LEVEL_VERSION_26_1_2),
            level_name: data.get_str("LevelName").unwrap_or("world").to_owned(),
            last_played: data.get_i64("LastPlayed").unwrap_or(0),
            game_type: data.get_i32("GameType").unwrap_or(0),
            time: data.get_i64("Time").unwrap_or(0),
            difficulty,
            hardcore,
            difficulty_locked,
            spawn,
            server_brands: string_list(data, "ServerBrands"),
            was_modded: data.get_bool("WasModded").unwrap_or(false),
            initialized: data.get_bool("initialized").unwrap_or(false),
            allow_commands: data.get_bool("allowCommands").unwrap_or(false),
            enabled_packs: data_packs.map_or_else(Vec::new, |packs| string_list(packs, "Enabled")),
            disabled_packs: data_packs
                .map_or_else(Vec::new, |packs| string_list(packs, "Disabled")),
            extra,
        })
    }

    /// Decode a `level.dat` file's bytes (gzip-compressed NBT).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the file is not a gzip NBT document or
    /// its `Data` compound is missing.
    pub fn from_bytes(bytes: &[u8]) -> ServerResult<Self> {
        Self::from_nbt(&crate::save::decode_gzip_nbt(bytes)?)
    }

    /// Encode as the 26.1.2 `level.dat` structure (uncompressed NBT bytes).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when a string is too long to encode.
    pub fn to_nbt_bytes(&self) -> ServerResult<Vec<u8>> {
        let mut out = Vec::new();
        mc_nbt::write_named("", &self.to_nbt(), &mut out)?;
        Ok(out)
    }

    /// Encode as the nested `{Data: {...}}` tag Vanilla expects.
    ///
    /// One function, one field order, matching the measured vanilla layout.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn to_nbt(&self) -> NbtTag {
        // Field order follows the measured vanilla file so a round trip is
        // diff-friendly in golden comparisons.
        let mut entries: Vec<(String, NbtTag)> = vec![
            ("DataVersion".to_owned(), NbtTag::Int(self.data_version)),
            (
                "Version".to_owned(),
                NbtTag::compound([
                    (
                        "Snapshot".to_owned(),
                        NbtTag::Byte(i8::from(self.version.as_ref().is_some_and(|v| v.snapshot))),
                    ),
                    (
                        "Series".to_owned(),
                        NbtTag::String(
                            self.version
                                .as_ref()
                                .map_or_else(|| VERSION_SERIES.to_owned(), |v| v.series.clone()),
                        ),
                    ),
                    (
                        "Id".to_owned(),
                        NbtTag::Int(self.version.as_ref().map_or(self.data_version, |v| v.id)),
                    ),
                    (
                        "Name".to_owned(),
                        NbtTag::String(
                            self.version
                                .as_ref()
                                .map_or_else(|| VERSION_NAME_26_1_2.to_owned(), |v| v.name.clone()),
                        ),
                    ),
                ]),
            ),
            ("version".to_owned(), NbtTag::Int(self.level_version)),
            (
                "LevelName".to_owned(),
                NbtTag::String(self.level_name.clone()),
            ),
            ("LastPlayed".to_owned(), NbtTag::Long(self.last_played)),
            ("GameType".to_owned(), NbtTag::Int(self.game_type)),
            ("Time".to_owned(), NbtTag::Long(self.time)),
            (
                "difficulty_settings".to_owned(),
                NbtTag::compound([
                    (
                        "difficulty".to_owned(),
                        NbtTag::String(self.difficulty.key().to_owned()),
                    ),
                    ("hardcore".to_owned(), NbtTag::Byte(i8::from(self.hardcore))),
                    (
                        "locked".to_owned(),
                        NbtTag::Byte(i8::from(self.difficulty_locked)),
                    ),
                ]),
            ),
            (
                "spawn".to_owned(),
                NbtTag::compound([
                    (
                        "pos".to_owned(),
                        NbtTag::List(vec![
                            NbtTag::Int(self.spawn.x),
                            NbtTag::Int(self.spawn.y),
                            NbtTag::Int(self.spawn.z),
                        ]),
                    ),
                    ("pitch".to_owned(), NbtTag::Float(self.spawn.pitch)),
                    (
                        "dimension".to_owned(),
                        NbtTag::String(self.spawn.dimension.clone()),
                    ),
                    ("yaw".to_owned(), NbtTag::Float(self.spawn.yaw)),
                ]),
            ),
            (
                "ServerBrands".to_owned(),
                NbtTag::List(
                    self.server_brands
                        .iter()
                        .map(|b| NbtTag::String(b.clone()))
                        .collect(),
                ),
            ),
            (
                "WasModded".to_owned(),
                NbtTag::Byte(i8::from(self.was_modded)),
            ),
            (
                "initialized".to_owned(),
                NbtTag::Byte(i8::from(self.initialized)),
            ),
            (
                "allowCommands".to_owned(),
                NbtTag::Byte(i8::from(self.allow_commands)),
            ),
            (
                "DataPacks".to_owned(),
                NbtTag::compound([
                    (
                        "Enabled".to_owned(),
                        NbtTag::List(
                            self.enabled_packs
                                .iter()
                                .map(|p| NbtTag::String(p.clone()))
                                .collect(),
                        ),
                    ),
                    (
                        "Disabled".to_owned(),
                        NbtTag::List(
                            self.disabled_packs
                                .iter()
                                .map(|p| NbtTag::String(p.clone()))
                                .collect(),
                        ),
                    ),
                ]),
            ),
        ];
        entries.extend(self.extra.iter().cloned());
        NbtTag::Compound(vec![("Data".to_owned(), NbtTag::Compound(entries))])
    }

    /// The dimension the spawn point lives in.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the stored key is not a valid id.
    pub fn spawn_dimension(&self) -> ServerResult<Dimension> {
        Dimension::parse(&self.spawn.dimension)
    }

    /// Decode from an already-parsed root, with the standard disk budgets.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] / [`ServerError::Protocol`] as for
    /// [`LevelDat::from_bytes`].
    pub fn from_bytes_checked(bytes: &[u8]) -> ServerResult<Self> {
        let (_, root) = mc_nbt::read_named(bytes, Limits::DISK)?;
        Self::from_nbt(&root)
    }
}

#[cfg(test)]
mod tests {
    use super::{DATA_VERSION_26_1_2, Difficulty, LEVEL_VERSION_26_1_2, LevelDat, SpawnPoint};
    use mc_core::error::ServerError;
    use mc_nbt::NbtTag;

    fn sample() -> LevelDat {
        let mut level = LevelDat::new("test world", 1_700_000_000_000);
        level.spawn = SpawnPoint {
            x: -592,
            y: 67,
            z: -384,
            yaw: 0.0,
            pitch: 0.0,
            dimension: "minecraft:overworld".to_owned(),
        };
        level.initialized = true;
        level
    }

    #[test]
    fn new_stamps_the_verified_26_1_2_versions() {
        let level = LevelDat::new("world", 0);
        assert_eq!(level.data_version, 4790);
        assert_eq!(level.level_version, 19133);
        let version = level.version.as_ref().expect("version");
        assert_eq!(version.id, 4790);
        assert_eq!(version.name, "26.1.2");
        assert_eq!(version.series, "main");
        assert!(!version.snapshot);
        assert_eq!(DATA_VERSION_26_1_2, 4790);
        assert_eq!(LEVEL_VERSION_26_1_2, 19133);
    }

    #[test]
    fn round_trip_through_nbt() {
        let level = sample();
        let root = level.to_nbt();
        let decoded = LevelDat::from_nbt(&root).expect("decodes");
        assert_eq!(decoded, level);
        // And through the byte encoding as well.
        let bytes = level.to_nbt_bytes().expect("encodes");
        assert_eq!(
            LevelDat::from_bytes_checked(&bytes).expect("decodes"),
            level
        );
    }

    #[test]
    fn writes_the_26_1_shape_not_the_legacy_one() {
        let root = sample().to_nbt();
        let data = root.get_compound("Data").expect("Data");
        assert!(
            data.contains("difficulty_settings"),
            "26.1 difficulty shape"
        );
        assert!(!data.contains("Difficulty"), "legacy byte must be gone");
        assert!(data.contains("spawn"), "26.1 spawn shape");
        assert!(!data.contains("SpawnX"), "legacy spawn must be gone");
        assert_eq!(
            data.get_compound("difficulty_settings")
                .and_then(|d| d.get_str("difficulty")),
            Some("easy"),
            "difficulty is a string in 26.1"
        );
        let pos = data
            .get_compound("spawn")
            .and_then(|s| s.get_list("pos"))
            .expect("spawn.pos");
        assert_eq!(pos.len(), 3);
    }

    #[test]
    fn reads_legacy_spawn_and_difficulty_shapes() {
        let legacy = NbtTag::Compound(vec![(
            "Data".to_owned(),
            NbtTag::Compound(vec![
                ("DataVersion".to_owned(), NbtTag::Int(4435)),
                ("SpawnX".to_owned(), NbtTag::Int(10)),
                ("SpawnY".to_owned(), NbtTag::Int(70)),
                ("SpawnZ".to_owned(), NbtTag::Int(-20)),
                ("SpawnAngle".to_owned(), NbtTag::Float(90.0)),
                ("Difficulty".to_owned(), NbtTag::Byte(3)),
                ("DifficultyLocked".to_owned(), NbtTag::Byte(1)),
                ("LevelName".to_owned(), NbtTag::String("old".to_owned())),
            ]),
        )]);
        let level = LevelDat::from_nbt(&legacy).expect("decodes");
        assert_eq!((level.spawn.x, level.spawn.y, level.spawn.z), (10, 70, -20));
        assert!((level.spawn.yaw - 90.0).abs() < f32::EPSILON);
        assert_eq!(level.difficulty, Difficulty::Hard);
        assert!(level.difficulty_locked);
        assert_eq!(level.level_name, "old");
        // The legacy fields are interpreted, so they are not duplicated in
        // `extra`; re-encoding upgrades the document to the 26.1 shape.
        assert!(!level.extra.iter().any(|(k, _)| k == "SpawnX"));
        let upgraded = level.to_nbt();
        let data = upgraded.get_compound("Data").expect("Data");
        assert!(data.contains("spawn") && !data.contains("SpawnX"));
        assert!(data.contains("difficulty_settings") && !data.contains("Difficulty"));
    }

    #[test]
    fn unknown_entries_survive_a_round_trip() {
        let mut level = sample();
        level.extra.push((
            "SomeModData".to_owned(),
            NbtTag::String("keep me".to_owned()),
        ));
        let decoded = LevelDat::from_nbt(&level.to_nbt()).expect("decodes");
        assert_eq!(
            decoded
                .extra
                .iter()
                .find(|(k, _)| k == "SomeModData")
                .map(|(_, v)| v.clone()),
            Some(NbtTag::String("keep me".to_owned()))
        );
    }

    #[test]
    fn missing_data_compound_is_an_error() {
        let err = LevelDat::from_nbt(&NbtTag::Compound(vec![])).expect_err("no Data");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
        let wrong_type = NbtTag::Compound(vec![("Data".to_owned(), NbtTag::Int(1))]);
        assert!(LevelDat::from_nbt(&wrong_type).is_err());
    }

    #[test]
    fn data_version_window_is_explicit() {
        assert!(LevelDat::is_supported_data_version(4790));
        assert!(LevelDat::is_supported_data_version(4435));
        assert!(!LevelDat::is_supported_data_version(4434), "too old");
        assert!(!LevelDat::is_supported_data_version(4903), "26.2 is newer");
    }

    #[test]
    fn difficulty_keys_match_the_vanilla_enum() {
        for (difficulty, key, id) in [
            (Difficulty::Peaceful, "peaceful", 0),
            (Difficulty::Easy, "easy", 1),
            (Difficulty::Normal, "normal", 2),
            (Difficulty::Hard, "hard", 3),
        ] {
            assert_eq!(difficulty.key(), key);
            assert_eq!(Difficulty::from_key(key), Some(difficulty));
            assert_eq!(difficulty.legacy_id(), id);
            assert_eq!(Difficulty::from_legacy_id(id), Some(difficulty));
        }
        assert_eq!(Difficulty::from_key("nightmare"), None);
        assert_eq!(Difficulty::from_legacy_id(9), None);
    }
}
