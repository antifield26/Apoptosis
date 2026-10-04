//! Game rules: storage, `/gamerule`, and wiring (P20-05).
//!
//! ## Rule names and defaults
//!
//! Names are the jar's `snake_case` keys (26.x), *not* the old wiki camelCase:
//! `randomTickSpeed`/`doMobSpawning` are parse errors here. Two in-repo sources
//! agree on the overlapping names — the fluid-differential driver lists
//! `random_tick_speed`, `spawn_mobs`, `advance_time`, `advance_weather` and
//! `fire_spread_radius_around_player`
//! (`crates/test-support/fixtures/fluids/scenarios.toml`), and the jar's own
//! `GameRules` registration quotes `lava_source_conversion = false` and
//! `water_source_conversion = true` (`game/fluids.rs`). The full table
//! (including `keep_inventory`, `mob_griefing`, `players_sleeping_percentage`
//! and `pvp`) is quoted from Pumpkin's 26.x `assets/game_rules.json`
//! (`OpenSourceMinecraftServer/Pumpkin-master`, GPL-3.0 — names and defaults
//! only, no code; Pumpkin targets 26.2 while this server targets 26.1.2, so
//! the table is names-plus-defaults, not a wire claim).
//!
//! ## Document shape
//!
//! `data/minecraft/game_rules.dat`, gzip NBT
//! `{data: {"minecraft:<rule>": Byte|Int, ...}}` with `DataVersion` carried
//! *inside* `data` — Pumpkin's measured shape, unlike `weather.dat` (root).
//! The reader is tolerant: `data` or `Data`, `DataVersion` at either level,
//! unknown keys ignored, missing or mistyped keys defaulted (a corrupt-typed
//! operator value restarts as the default rather than refusing the boot).
//!
//! ## Wiring
//!
//! Every rule with a mechanism reads live from [`GameRules`]; nothing caches
//! a copy. `fire_spread_radius_around_player` is stored and listed but inert —
//! there is no fire model (PARITY), and `/gamerule` says so when it is set.

use mc_persistence::level::DATA_VERSION_26_1_2;

/// A game-rule value as `/gamerule` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleValue {
    /// A boolean rule.
    Bool(bool),
    /// An integer rule.
    Int(i32),
}

impl std::fmt::Display for RuleValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(flag) => write!(f, "{flag}"),
            Self::Int(value) => write!(f, "{value}"),
        }
    }
}

/// Whether `/gamerule` accepts `name` with or without the `minecraft:` prefix.
fn canonical_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// The live game rules.
///
/// Twelve rules: nine boolean, three integer. The booleans stay individual
/// fields (one accessor each, no bitset) because every rule is read by name
/// at a different call site — packing them would trade clarity for nothing.
/// Defaults are the jar's (see module docs).
#[allow(
    clippy::struct_excessive_bools,
    reason = "the rule table is nine named booleans by jar shape; one accessor each"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRules {
    /// Whether the daylight clock advances (`tickTime` gate).
    pub advance_time: bool,
    /// Whether the weather cycle advances (`advanceWeatherCycle` gate).
    pub advance_weather: bool,
    /// Whether death keeps inventory and experience.
    pub keep_inventory: bool,
    /// Whether mobs (and their landings) may grief blocks.
    pub mob_griefing: bool,
    /// Whether creature packs may spawn naturally.
    pub spawn_mobs: bool,
    /// Whether monster packs may spawn naturally.
    pub spawn_monsters: bool,
    /// Whether water forms new sources.
    pub water_source_conversion: bool,
    /// Whether lava forms new sources.
    pub lava_source_conversion: bool,
    /// Whether players may damage each other.
    pub pvp: bool,
    /// Random-tick samples per section per tick; 0 disables the sweep's draws.
    pub random_tick_speed: i32,
    /// Percent of active players that must sleep through to skip the night.
    pub players_sleeping_percentage: i32,
    /// Fire-spread radius. Stored only: no fire model reads it.
    pub fire_spread_radius_around_player: i32,
}

impl Default for GameRules {
    /// The jar's defaults (see module docs for provenance).
    fn default() -> Self {
        Self {
            advance_time: true,
            advance_weather: true,
            keep_inventory: false,
            mob_griefing: true,
            spawn_mobs: true,
            spawn_monsters: true,
            water_source_conversion: true,
            lava_source_conversion: false,
            pvp: true,
            random_tick_speed: super::growth::RANDOM_TICK_SPEED_DEFAULT as i32,
            players_sleeping_percentage: super::sleep::SLEEPING_PERCENTAGE_DEFAULT,
            fire_spread_radius_around_player: 128,
        }
    }
}

impl GameRules {
    /// All rule names, stable order (also the `/gamerule` query-all order).
    pub const NAMES: &[&str] = &[
        "advance_time",
        "advance_weather",
        "fire_spread_radius_around_player",
        "keep_inventory",
        "lava_source_conversion",
        "mob_griefing",
        "players_sleeping_percentage",
        "pvp",
        "random_tick_speed",
        "spawn_mobs",
        "spawn_monsters",
        "water_source_conversion",
    ];

    /// Read one rule by name (with or without the `minecraft:` prefix).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<RuleValue> {
        match canonical_name(name) {
            "advance_time" => Some(RuleValue::Bool(self.advance_time)),
            "advance_weather" => Some(RuleValue::Bool(self.advance_weather)),
            "fire_spread_radius_around_player" => {
                Some(RuleValue::Int(self.fire_spread_radius_around_player))
            }
            "keep_inventory" => Some(RuleValue::Bool(self.keep_inventory)),
            "lava_source_conversion" => Some(RuleValue::Bool(self.lava_source_conversion)),
            "mob_griefing" => Some(RuleValue::Bool(self.mob_griefing)),
            "players_sleeping_percentage" => Some(RuleValue::Int(self.players_sleeping_percentage)),
            "pvp" => Some(RuleValue::Bool(self.pvp)),
            "random_tick_speed" => Some(RuleValue::Int(self.random_tick_speed)),
            "spawn_mobs" => Some(RuleValue::Bool(self.spawn_mobs)),
            "spawn_monsters" => Some(RuleValue::Bool(self.spawn_monsters)),
            "water_source_conversion" => Some(RuleValue::Bool(self.water_source_conversion)),
            _ => None,
        }
    }

    /// Whether `name` is a rule this build actually honours.
    ///
    /// Only `fire_spread_radius_around_player` is inert (no fire model);
    /// everything else has a live reader. Unknown names report false — the
    /// command refuses those before asking.
    #[must_use]
    pub fn is_wired(name: &str) -> bool {
        !matches!(canonical_name(name), "fire_spread_radius_around_player")
            && Self::default().get(name).is_some()
    }

    /// Set one rule from its `/gamerule` text value.
    ///
    /// Booleans take `true`/`false` only; integers must parse as `i32` and be
    /// non-negative (the jar's integer rules bottom out at 0). Returns the new
    /// value, or a message refusing the set.
    pub fn set(&mut self, name: &str, value: &str) -> Result<RuleValue, String> {
        let set_bool = |slot: &mut bool| -> RuleValue {
            *slot = value == "true";
            RuleValue::Bool(*slot)
        };
        match canonical_name(name) {
            "advance_time" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.advance_time))
            }
            "advance_weather" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.advance_weather))
            }
            "keep_inventory" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.keep_inventory))
            }
            "mob_griefing" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.mob_griefing))
            }
            "spawn_mobs" if matches!(value, "true" | "false") => Ok(set_bool(&mut self.spawn_mobs)),
            "spawn_monsters" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.spawn_monsters))
            }
            "water_source_conversion" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.water_source_conversion))
            }
            "lava_source_conversion" if matches!(value, "true" | "false") => {
                Ok(set_bool(&mut self.lava_source_conversion))
            }
            "pvp" if matches!(value, "true" | "false") => Ok(set_bool(&mut self.pvp)),
            "random_tick_speed"
            | "players_sleeping_percentage"
            | "fire_spread_radius_around_player" => match value.parse::<i32>() {
                Ok(number) if number >= 0 => {
                    let slot = match canonical_name(name) {
                        "random_tick_speed" => &mut self.random_tick_speed,
                        "players_sleeping_percentage" => &mut self.players_sleeping_percentage,
                        _ => &mut self.fire_spread_radius_around_player,
                    };
                    *slot = number;
                    Ok(RuleValue::Int(number))
                }
                _ => Err(format!("{value} is not a non-negative integer for {name}")),
            },
            _ if Self::default().get(name).is_some() => {
                Err(format!("{value} is not true or false for {name}"))
            }
            _ => Err(format!("unknown game rule {name}")),
        }
    }

    /// Samples per section per tick, floored at zero (a corrupt file's
    /// negative value disables rather than exploding the sweep bound).
    #[must_use]
    pub fn tick_speed(&self) -> usize {
        self.random_tick_speed.max(0) as usize
    }

    /// Read the stored rules out of a `game_rules.dat` document.
    ///
    /// Tolerant like weather's reader: `data` or `Data`, `minecraft:`-prefixed
    /// keys, missing or mistyped keys fall back to the default.
    pub fn from_document(document: &mc_nbt::NbtTag) -> Self {
        let data = document
            .get_compound("data")
            .or_else(|| document.get_compound("Data"));
        let mut rules = Self::default();
        let Some(data) = data else {
            return rules;
        };
        let get_bool = |key: &str| {
            data.get(key)
                .map(|v| !matches!(v, mc_nbt::NbtTag::Byte(0) | mc_nbt::NbtTag::Int(0)))
        };
        let get_int = |key: &str| {
            data.get(key)
                .and_then(mc_nbt::NbtTag::as_i64)
                .and_then(|v| i32::try_from(v).ok())
        };
        // Each rule keeps its default unless the document carries a readable
        // value: a vanilla file omits untouched rules, and so does our writer.
        for (key, slot) in [
            ("advance_time", &mut rules.advance_time),
            ("advance_weather", &mut rules.advance_weather),
            ("keep_inventory", &mut rules.keep_inventory),
            ("mob_griefing", &mut rules.mob_griefing),
            ("spawn_mobs", &mut rules.spawn_mobs),
            ("spawn_monsters", &mut rules.spawn_monsters),
            (
                "water_source_conversion",
                &mut rules.water_source_conversion,
            ),
            ("lava_source_conversion", &mut rules.lava_source_conversion),
            ("pvp", &mut rules.pvp),
        ] {
            if let Some(value) = get_bool(&format!("minecraft:{key}")) {
                *slot = value;
            }
        }
        for (key, slot) in [
            ("random_tick_speed", &mut rules.random_tick_speed),
            (
                "players_sleeping_percentage",
                &mut rules.players_sleeping_percentage,
            ),
            (
                "fire_spread_radius_around_player",
                &mut rules.fire_spread_radius_around_player,
            ),
        ] {
            if let Some(value) = get_int(&format!("minecraft:{key}")) {
                *slot = value;
            }
        }
        rules
    }

    /// Write the live rules as a `game_rules.dat` document (Pumpkin's shape:
    /// `DataVersion` inside `data`).
    pub fn to_document(&self) -> mc_nbt::NbtTag {
        let byte = |flag: bool| {
            if flag {
                mc_nbt::NbtTag::Byte(1)
            } else {
                mc_nbt::NbtTag::Byte(0)
            }
        };
        let rule = |key: &str, tag: mc_nbt::NbtTag| (format!("minecraft:{key}"), tag);
        mc_nbt::NbtTag::compound([(
            "data".to_owned(),
            mc_nbt::NbtTag::compound([
                (
                    "DataVersion".to_owned(),
                    mc_nbt::NbtTag::Int(DATA_VERSION_26_1_2),
                ),
                rule("advance_time", byte(self.advance_time)),
                rule("advance_weather", byte(self.advance_weather)),
                rule(
                    "fire_spread_radius_around_player",
                    mc_nbt::NbtTag::Int(self.fire_spread_radius_around_player),
                ),
                rule("keep_inventory", byte(self.keep_inventory)),
                rule("lava_source_conversion", byte(self.lava_source_conversion)),
                rule("mob_griefing", byte(self.mob_griefing)),
                rule(
                    "players_sleeping_percentage",
                    mc_nbt::NbtTag::Int(self.players_sleeping_percentage),
                ),
                rule("pvp", byte(self.pvp)),
                rule(
                    "random_tick_speed",
                    mc_nbt::NbtTag::Int(self.random_tick_speed),
                ),
                rule("spawn_mobs", byte(self.spawn_mobs)),
                rule("spawn_monsters", byte(self.spawn_monsters)),
                rule(
                    "water_source_conversion",
                    byte(self.water_source_conversion),
                ),
            ]),
        )])
    }
}

impl super::Game {
    /// Read `game_rules.dat` into the game (boot path).
    ///
    /// Missing file keeps the jar defaults; a corrupt file warns and keeps
    /// them too — rules re-derive from defaults, so unlike a seed there is
    /// nothing to fork. Returns whether a document was loaded, so the
    /// lifecycle can seed fresh-world intent (config `pvp`) only when no
    /// stored rules exist — stored rules always win on an existing world.
    pub fn load_game_rules(&mut self, world_root: &std::path::Path) -> bool {
        let document = match mc_persistence::level::read_game_rules(world_root) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(%error, "unreadable game-rule document; starting from defaults");
                None
            }
        };
        if let Some(document) = document {
            self.rules = GameRules::from_document(&document);
            true
        } else {
            false
        }
    }

    /// The daylight clock reading with `advance_time` honoured.
    ///
    /// Daylight time derives from the ever-advancing tick counter, so a
    /// stopped clock is the captured value in `time_frozen_at` (maintained
    /// each tick beside the weather call). Pure read: every clock consumer
    /// calls this instead of `self.tick`.
    #[must_use]
    pub(crate) fn world_time(&self) -> i64 {
        self.time_frozen_at.unwrap_or(self.tick.cast_signed())
    }

    /// Capture or release the frozen clock reading (head of `ScheduledTicks`).
    ///
    /// The freeze starts the first tick the rule reads false — capturing here
    /// rather than in the `/gamerule` handler keeps direct rule writes (and
    /// tests) on the same path as the command.
    pub(crate) fn maintain_time_freeze(&mut self) {
        if self.rules.advance_time {
            self.time_frozen_at = None;
        } else if self.time_frozen_at.is_none() {
            self.time_frozen_at = Some(self.tick.cast_signed());
        }
    }

    /// The flow rules with this world's game-rule values fed in.
    ///
    /// The engine is unchanged (P20-01's contract): only these two arguments
    /// move from the jar defaults to the stored rules.
    #[must_use]
    pub(crate) fn fluid_rules(
        &self,
        kind: mc_simulation::fluid::FluidKind,
    ) -> mc_simulation::fluid::FluidRules {
        Self::fluid_rules_for(
            kind,
            self.rules.water_source_conversion,
            self.rules.lava_source_conversion,
        )
    }

    /// [`Game::fluid_rules`] without the borrow: the fluid-tick drain holds
    /// `&mut` world state while it needs the rules, so the two flags are
    /// read first and fed here (a method call would borrow all of `self`
    /// across the engine call).
    #[must_use]
    pub(crate) fn fluid_rules_for(
        kind: mc_simulation::fluid::FluidKind,
        water_conversion: bool,
        lava_conversion: bool,
    ) -> mc_simulation::fluid::FluidRules {
        use mc_simulation::fluid::FluidKind;
        match kind {
            FluidKind::Water => mc_simulation::fluid::FluidRules::water(water_conversion),
            // Overworld lava: `FAST_LAVA` is false outside the Nether
            // (fluids.rs per-dimension note, unchanged here).
            FluidKind::Lava => mc_simulation::fluid::FluidRules::lava(false, lava_conversion),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_jar_table() {
        let rules = GameRules::default();
        assert!(rules.advance_time);
        assert!(rules.advance_weather);
        assert!(!rules.keep_inventory);
        assert!(rules.mob_griefing);
        assert!(rules.spawn_mobs);
        assert!(rules.spawn_monsters);
        assert!(rules.water_source_conversion);
        assert!(!rules.lava_source_conversion);
        assert!(rules.pvp);
        assert_eq!(rules.random_tick_speed, 3);
        assert_eq!(rules.players_sleeping_percentage, 100);
        assert_eq!(rules.fire_spread_radius_around_player, 128);
        assert_eq!(GameRules::NAMES.len(), 12);
        // The sweep and the skip-night count read these live; the pinned
        // constants are the same defaults, so a default change fails here
        // instead of silently rescheduling every farm and vote.
        assert_eq!(
            rules.tick_speed(),
            super::super::growth::RANDOM_TICK_SPEED_DEFAULT
        );
        assert_eq!(
            rules.players_sleeping_percentage,
            super::super::sleep::SLEEPING_PERCENTAGE_DEFAULT
        );
    }

    #[test]
    fn document_round_trip_is_stable() {
        let rules = GameRules {
            keep_inventory: true,
            random_tick_speed: 0,
            pvp: false,
            ..GameRules::default()
        };
        let back = GameRules::from_document(&rules.to_document());
        assert_eq!(back, rules);
    }

    #[test]
    fn missing_keys_default_and_unknown_keys_are_ignored() {
        let document = mc_nbt::NbtTag::compound([(
            "data".to_owned(),
            mc_nbt::NbtTag::compound([(
                "minecraft:keep_inventory".to_owned(),
                mc_nbt::NbtTag::Byte(1),
            )]),
        )]);
        let rules = GameRules::from_document(&document);
        assert!(rules.keep_inventory);
        assert_eq!(rules.random_tick_speed, 3);
        let padded = mc_nbt::NbtTag::compound([(
            "data".to_owned(),
            mc_nbt::NbtTag::compound([
                (
                    "minecraft:keep_inventory".to_owned(),
                    mc_nbt::NbtTag::Byte(1),
                ),
                ("minecraft:not_a_rule".to_owned(), mc_nbt::NbtTag::Byte(1)),
                // A Byte where an Int belongs still reads (tolerant buddy:
                // `as_i64` widens the small ints). Strictness here would turn
                // a hand-edited file into a silent default.
                (
                    "minecraft:random_tick_speed".to_owned(),
                    mc_nbt::NbtTag::Byte(1),
                ),
            ]),
        )]);
        let rules = GameRules::from_document(&padded);
        assert!(rules.keep_inventory);
        assert_eq!(rules.random_tick_speed, 1);
        // And a document with no `data` at all is the defaults.
        let bare = mc_nbt::NbtTag::compound(Vec::<(String, mc_nbt::NbtTag)>::new());
        assert_eq!(GameRules::from_document(&bare), GameRules::default());
    }

    #[test]
    fn set_accepts_typed_values_and_refuses_the_rest() {
        let mut rules = GameRules::default();
        assert_eq!(
            rules.set("keep_inventory", "true"),
            Ok(RuleValue::Bool(true))
        );
        assert_eq!(
            rules.set("minecraft:pvp", "false"),
            Ok(RuleValue::Bool(false))
        );
        assert_eq!(rules.set("random_tick_speed", "7"), Ok(RuleValue::Int(7)));
        assert!(rules.set("random_tick_speed", "-1").is_err());
        assert!(rules.set("random_tick_speed", "fast").is_err());
        assert!(rules.set("pvp", "yes").is_err());
        assert!(rules.set("doDaylightCycle", "false").is_err());
        assert!(rules.set("keepInventory", "true").is_err());
        assert!(!GameRules::is_wired("fire_spread_radius_around_player"));
        assert!(GameRules::is_wired("keep_inventory"));
        assert!(!GameRules::is_wired("doDaylightCycle"));
    }

    #[test]
    fn negative_stored_speed_disables_rather_than_exploding() {
        let rules = GameRules {
            random_tick_speed: -4,
            ..GameRules::default()
        };
        assert_eq!(rules.tick_speed(), 0);
    }
}
