//! Weather, slice 1: state, the jar's cycle, packets, `/weather`, persistence
//! (P20-03).
//!
//! ## Jar truth (26.1.2, `javap -c` on the server jar, read 2026-10-02)
//!
//! - `WeatherData` (`world/level/saveddata`, file `data/minecraft/weather.dat`
//!   shaped `{DataVersion, data: {rain_time, raining, thundering,
//!   thunder_time, clear_weather_time}}` — measured on the evidence world):
//!   five fields, no more.
//! - `ServerLevel.advanceWeatherCycle`: gated on `canHaveWeather` and the
//!   `ADVANCE_WEATHER` rule; `clearWeatherTime > 0` decrements it and forces
//!   both times to `isX ? 0 : 1` with both flags false; else each timer
//!   decrements and flips its flag at zero, or samples its provider at zero:
//!   `RAIN_DELAY (12000, 180000)`, `RAIN_DURATION (12000, 24000)`,
//!   `THUNDER_DELAY (12000, 180000)`, `THUNDER_DURATION (3600, 15600)`
//!   (all `UniformInt`, static init). Levels ease `±0.01`/tick toward
//!   `raining/thundering ? 1 : 0`, clamped to `[0, 1]`.
//! - Packets, also bytecode-read: `RAIN_LEVEL_CHANGE`/`THUNDER_LEVEL_CHANGE`
//!   whenever `oLevel != level` (every easing tick), plus `STOP_RAINING` or
//!   `START_RAINING` when the raining flag flipped (all in
//!   `setWeatherParameters`, which also always re-sends both levels).
//!   Event ids: 1 start, 2 stop, 7 rain level, 8 thunder level.
//! - `setWeatherParameters(clear, rain, raining, thundering)` writes
//!   `clearWeatherTime = clear`, `rainTime = thunderTime = rain`,
//!   both flags; `resetWeatherCycle` zeroes the four rain/thunder fields
//!   (sleep calls it when raining — P20-04 owns that call).
//! - `WeatherCommand`: level 2; `clear [t]` → `(RAIN_DELAY.sample or t, 0,
//!   false, false)`, `rain [t]` → `(0, RAIN_DURATION.sample or t, true,
//!   false)`, `thunder [t]` → `(0, THUNDER_DURATION.sample or t, true, true)`;
//!   absent duration samples the provider, it is never a fixed 6000.
//! - `prepareWeather`: boot with raining sets `rainLevel = 1` (and thunder
//!   likewise) — no 100-tick ease-in after a restart.
//!
//! ## What this slice does and does not do
//!
//! Done: the five-field state on `Game`, the cycle at the head of
//! `ScheduledTicks` (the jar's `tickTime` slot, ADR-0009 §2.2), the four
//! packets above, `/weather`, `weather.dat` round-trip, `is_raining_at` for
//! the farmland rain arm (which P20-02 left false) and beds (P20-04 reads the
//! same flags). Slice 2 adds lightning strikes: a per-chunk roll while
//! thundering spawns a bolt entity that damages the strike box once and is
//! swept after its visual fuse. Named gaps: `ADVANCE_WEATHER` reads the
//! stored rule since P20-05; thunder darkness in the spawn-light rule is
//! untouched; `canHaveWeather` is always true (one dimension); fire, creeper
//! charging and villager/witch conversion on strike are unmodelled (no fire
//! model, no powered state, unmodelled mobs).

use super::Game;
use mc_core::error::ServerResult;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::GameEvent;

/// `ServerLevel.RAIN_DELAY`: `UniformInt.of(12000, 180000)` (jar static init).
pub(crate) const RAIN_DELAY_MIN: i32 = 12_000;
/// See [`RAIN_DELAY_MIN`].
pub(crate) const RAIN_DELAY_MAX: i32 = 180_000;
/// `ServerLevel.RAIN_DURATION`: `UniformInt.of(12000, 24000)`.
pub(crate) const RAIN_DURATION_MIN: i32 = 12_000;
/// See [`RAIN_DURATION_MIN`].
pub(crate) const RAIN_DURATION_MAX: i32 = 24_000;
/// `ServerLevel.THUNDER_DELAY`: `UniformInt.of(12000, 180000)`.
pub(crate) const THUNDER_DELAY_MIN: i32 = 12_000;
/// See [`THUNDER_DELAY_MIN`].
pub(crate) const THUNDER_DELAY_MAX: i32 = 180_000;
/// `ServerLevel.THUNDER_DURATION`: `UniformInt.of(3600, 15600)`.
pub(crate) const THUNDER_DURATION_MIN: i32 = 3_600;
/// See [`THUNDER_DURATION_MIN`].
pub(crate) const THUNDER_DURATION_MAX: i32 = 15_600;

/// Per-tick level ease (`±0.01`, jar `advanceWeatherCycle`).
const LEVEL_STEP: f32 = 0.01;

/// `game_event` ids (jar `ClientboundGameEventPacket$Type` static init).
const EVENT_START_RAINING: u8 = 1;
const EVENT_STOP_RAINING: u8 = 2;
const EVENT_RAIN_LEVEL_CHANGE: u8 = 7;
const EVENT_THUNDER_LEVEL_CHANGE: u8 = 8;

/// Strike roll: one in this many per chunk per tick while thundering.
///
/// Vanilla's `tickThunder` shape, confirmed in the local reference
/// (`Pumpkin-master .../world/mod.rs`: `is_raining && is_thundering &&
/// rng 0..100_000 == 0` per spawning chunk; GPL-3.0, shape only).
pub(crate) const LIGHTNING_CHUNK_ONE_IN: i32 = 100_000;

/// Damage one strike deals to every living entity in its box (vanilla shape
/// via Pumpkin `EntityBase::on_lightning_strike`; the 8-second fire there is
/// a named gap here).
pub(crate) const LIGHTNING_DAMAGE: f32 = 5.0;

/// Strike box half-extents around the strike point (Pumpkin's
/// `damage_box`: `x±3, y-3..y+9, z±3`).
const LIGHTNING_DX: f64 = 3.0;
const LIGHTNING_DY_LO: f64 = 3.0;
const LIGHTNING_DY_HI: f64 = 9.0;
const LIGHTNING_DZ: f64 = 3.0;

/// The five `WeatherData` fields plus the two client-visible levels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WeatherState {
    /// Ticks of forced clear weather left (jar `clearWeatherTime`).
    pub clear_weather_time: i32,
    /// Ticks until the rain flag flips (jar `rainTime`).
    pub rain_time: i32,
    /// Ticks until the thunder flag flips (jar `thunderTime`).
    pub thunder_time: i32,
    /// Whether it is raining (jar `raining`).
    pub raining: bool,
    /// Whether it is thundering — always with rain here, as in the jar's
    /// command paths (jar `thundering`).
    pub thundering: bool,
    /// Eased client level 0..1 (jar `rainLevel`).
    pub rain_level: f32,
    /// Eased client level 0..1 (jar `thunderLevel`).
    pub thunder_level: f32,
}

impl Default for WeatherState {
    /// A fresh world's weather: clear skies, zeroed timers — exactly what the
    /// jar's own fresh `weather.dat` records.
    fn default() -> Self {
        Self {
            clear_weather_time: 0,
            rain_time: 0,
            thunder_time: 0,
            raining: false,
            thundering: false,
            rain_level: 0.0,
            thunder_level: 0.0,
        }
    }
}

impl WeatherState {
    /// Read the five fields out of a `weather.dat` document.
    ///
    /// Missing keys default like a fresh file (tolerant read: vanilla omits
    /// untouched fields, and so do we on write — only the live values round-
    /// trip, never invented ones).
    pub(crate) fn from_document(document: &mc_nbt::NbtTag) -> Self {
        let data = document
            .get_compound("data")
            .or_else(|| document.get_compound("Data"));
        let get_int = |key: &str| {
            data.and_then(|d| d.get(key))
                .and_then(mc_nbt::NbtTag::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0)
        };
        let get_bool = |key: &str| {
            data.and_then(|d| d.get(key))
                .is_some_and(|v| !matches!(v, mc_nbt::NbtTag::Byte(0) | mc_nbt::NbtTag::Int(0)))
        };
        let mut state = Self {
            clear_weather_time: get_int("clear_weather_time"),
            rain_time: get_int("rain_time"),
            thunder_time: get_int("thunder_time"),
            raining: get_bool("raining"),
            thundering: get_bool("thundering"),
            rain_level: 0.0,
            thunder_level: 0.0,
        };
        // `prepareWeather`: a boot into rain starts at full level.
        if state.raining {
            state.rain_level = 1.0;
        }
        if state.thundering {
            state.thunder_level = 1.0;
        }
        state
    }

    /// Write the five live fields as a `weather.dat` document.
    pub(crate) fn to_document(&self) -> mc_nbt::NbtTag {
        let byte = |flag: bool| {
            if flag {
                mc_nbt::NbtTag::Byte(1)
            } else {
                mc_nbt::NbtTag::Byte(0)
            }
        };
        mc_nbt::NbtTag::compound([
            (
                "DataVersion".to_owned(),
                mc_nbt::NbtTag::Int(mc_persistence::level::DATA_VERSION_26_1_2),
            ),
            (
                "data".to_owned(),
                mc_nbt::NbtTag::compound([
                    (
                        "clear_weather_time".to_owned(),
                        mc_nbt::NbtTag::Int(self.clear_weather_time),
                    ),
                    ("rain_time".to_owned(), mc_nbt::NbtTag::Int(self.rain_time)),
                    ("raining".to_owned(), byte(self.raining)),
                    ("thundering".to_owned(), byte(self.thundering)),
                    (
                        "thunder_time".to_owned(),
                        mc_nbt::NbtTag::Int(self.thunder_time),
                    ),
                ]),
            ),
        ])
    }
}

impl Game {
    /// Advance world time and the weather timers (head of `ScheduledTicks`).
    ///
    /// The jar puts `tickTime()` before both tick queues (ADR-0009 §2.2); this
    /// is the same slot. The `advance_weather` rule gates the call itself
    /// (P20-05, in `run_phase_inner`): this body always advances when it runs.
    pub(crate) fn tick_weather(&mut self, report: &mut super::TickReport) {
        let was_raining = self.weather.raining;
        // Timers (the jar's `advanceWeatherCycle` with `ADVANCE_WEATHER` true
        // and `canHaveWeather` true for the single dimension).
        if self.weather.clear_weather_time > 0 {
            self.weather.clear_weather_time -= 1;
            self.weather.thunder_time = i32::from(!self.weather.thundering);
            self.weather.rain_time = i32::from(!self.weather.raining);
            self.weather.thundering = false;
            self.weather.raining = false;
        } else {
            if self.weather.thunder_time > 0 {
                self.weather.thunder_time -= 1;
                if self.weather.thunder_time == 0 {
                    self.weather.thundering = !self.weather.thundering;
                }
            } else if self.weather.thundering {
                self.weather.thunder_time =
                    self.sample_uniform(THUNDER_DURATION_MIN, THUNDER_DURATION_MAX);
            } else {
                self.weather.thunder_time =
                    self.sample_uniform(THUNDER_DELAY_MIN, THUNDER_DELAY_MAX);
            }
            if self.weather.rain_time > 0 {
                self.weather.rain_time -= 1;
                if self.weather.rain_time == 0 {
                    self.weather.raining = !self.weather.raining;
                }
            } else if self.weather.raining {
                self.weather.rain_time = self.sample_uniform(RAIN_DURATION_MIN, RAIN_DURATION_MAX);
            } else {
                self.weather.rain_time = self.sample_uniform(RAIN_DELAY_MIN, RAIN_DELAY_MAX);
            }
        }
        // Levels ease toward the flags (jar `±0.01`, clamped).
        let prev_rain = self.weather.rain_level;
        let prev_thunder = self.weather.thunder_level;
        if self.weather.thundering {
            self.weather.thunder_level = (self.weather.thunder_level + LEVEL_STEP).min(1.0);
        } else {
            self.weather.thunder_level = (self.weather.thunder_level - LEVEL_STEP).max(0.0);
        }
        if self.weather.raining {
            self.weather.rain_level = (self.weather.rain_level + LEVEL_STEP).min(1.0);
        } else {
            self.weather.rain_level = (self.weather.rain_level - LEVEL_STEP).max(0.0);
        }
        // Broadcasts: START/STOP on a rain flip, level packets on any change
        // (the jar sends both level packets from `setWeatherParameters` too,
        // so a command lands on clients the same tick).
        if self.weather.raining != was_raining {
            let event = if self.weather.raining {
                EVENT_START_RAINING
            } else {
                EVENT_STOP_RAINING
            };
            self.broadcast_game_event(event, 0.0, report);
        }
        // Exact comparison, as the jar does (`fcmpl` on `oRainLevel !=
        // rainLevel`): levels move only in ±0.01 steps and clamp exactly at
        // the ends, so any difference at all is a change worth broadcasting.
        // A margin here would swallow a real easing step.
        #[allow(clippy::float_cmp, reason = "exact change detection, jar fcmpl")]
        if self.weather.rain_level != prev_rain {
            self.broadcast_game_event(EVENT_RAIN_LEVEL_CHANGE, self.weather.rain_level, report);
        }
        #[allow(clippy::float_cmp, reason = "exact change detection, jar fcmpl")]
        if self.weather.thunder_level != prev_thunder {
            self.broadcast_game_event(
                EVENT_THUNDER_LEVEL_CHANGE,
                self.weather.thunder_level,
                report,
            );
        }
    }

    /// `setWeatherParameters(clear, rain, raining, thundering)`: the jar sets
    /// `thunderTime` from the same `rain` int — `/weather thunder` therefore
    /// storms exactly as long as it rains.
    pub(crate) fn set_weather_parameters(
        &mut self,
        clear_time: i32,
        rain_time: i32,
        raining: bool,
        thundering: bool,
        report: &mut super::TickReport,
    ) {
        let was_raining = self.weather.raining;
        self.weather.clear_weather_time = clear_time;
        self.weather.rain_time = rain_time;
        self.weather.thunder_time = rain_time;
        self.weather.raining = raining;
        self.weather.thundering = thundering;
        if self.weather.raining != was_raining {
            let event = if self.weather.raining {
                EVENT_START_RAINING
            } else {
                EVENT_STOP_RAINING
            };
            self.broadcast_game_event(event, 0.0, report);
        }
        self.broadcast_game_event(EVENT_RAIN_LEVEL_CHANGE, self.weather.rain_level, report);
        self.broadcast_game_event(
            EVENT_THUNDER_LEVEL_CHANGE,
            self.weather.thunder_level,
            report,
        );
    }

    /// Whether rain falls on `(x, y, z)`: raining with sky above.
    ///
    /// The jar's `isRainingAt` also consults biomes and heightmaps; this
    /// build has one dimension and a `WORLD_SURFACE` heightmap, so "no
    /// non-air block above" is the honest approximation (water overhead
    /// counts as cover here — named, since the jar tests opacity).
    pub(crate) fn is_raining_at(&self, x: i32, y: i32, z: i32) -> bool {
        if !self.weather.raining {
            return false;
        }
        self.world.highest_block(x, z).is_none_or(|top| top < y)
    }

    /// Uniform `[min, max]` sample from the game's seeded source (the jar's
    /// `UniformInt.sample`: inclusive on both ends).
    pub(crate) fn sample_uniform(&mut self, min: i32, max: i32) -> i32 {
        let span = max.saturating_sub(min).saturating_add(1).max(1);
        min.saturating_add(self.random.next_i32_bounded(span))
    }

    /// One `game_event` to every session (weather transitions and levels).
    fn broadcast_game_event(&self, event: u8, value: f32, report: &mut super::TickReport) {
        let Ok(packet) = (GameEvent { event, value }).to_raw() else {
            return;
        };
        self.broadcast_all(&packet, report);
    }

    /// Read `weather.dat` into the game (boot path; missing file is clear
    /// skies, a corrupt file warns and starts clear — weather re-randomizes,
    /// so unlike a seed there is nothing to fork).
    pub fn load_weather(&mut self, world_root: &std::path::Path) {
        let document = match mc_persistence::level::read_weather(world_root) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(%error, "unreadable weather document; starting clear");
                None
            }
        };
        if let Some(document) = document {
            self.weather = WeatherState::from_document(&document);
        }
    }

    /// Test seam (same pattern as `set_ores_and_carvers`): install weather.
    ///
    /// Plain parameters rather than a `WeatherState`: the state type stays
    /// inside the crate, so the public surface gains no new names.
    pub fn set_weather(
        &mut self,
        clear_weather_time: i32,
        rain_time: i32,
        thunder_time: i32,
        raining: bool,
        thundering: bool,
    ) {
        self.weather.clear_weather_time = clear_weather_time;
        self.weather.rain_time = rain_time;
        self.weather.thunder_time = thunder_time;
        self.weather.raining = raining;
        self.weather.thundering = thundering;
        // Levels follow the flags (the jar's `prepareWeather`): a test that
        // installs a storm reads full levels at once, not a 100-tick ease-in.
        self.weather.rain_level = if raining { 1.0 } else { 0.0 };
        self.weather.thunder_level = if thundering { 1.0 } else { 0.0 };
    }

    /// Strike lightning at the cell `(x, y, z)` (P20-03 slice 2).
    ///
    /// Spawns the bolt visual (announced through the pending-spawn path with
    /// the jar's `minecraft:lightning_bolt` type) and deals 5.0 to every
    /// living entity in the strike box, once, through the shared damage path
    /// (hurt window, armour, death and loot all apply). The bolt itself is
    /// excluded, and so is everything non-living — drops and orbs weather
    /// the storm like vanilla. Fire, creeper charging and mob conversion on
    /// strike are named gaps (see the module docs).
    ///
    /// Public like `set_weather` and `spawn_mob`: the scheduler below calls
    /// it live, and tests call the same entry instead of a second path.
    ///
    /// # Errors
    ///
    /// When the entity store is full — the bolt is refused rather than
    /// displacing a live entity.
    pub fn strike_lightning(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
    ) -> ServerResult<mc_entity::EntityId> {
        let at = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
        let bolt = self
            .entities
            .spawn(mc_entity::EntityBody::Bolt(mc_entity::Bolt::new()), at)?;
        self.pending_entity_spawns.push(bolt);
        let victims: Vec<mc_entity::EntityId> = self
            .entities
            .iter()
            .filter(|entity| {
                !entity.removed
                    && entity.id != bolt
                    && entity.kind().is_living()
                    && (entity.position.x - at.x).abs() <= LIGHTNING_DX
                    && entity.position.y >= at.y - LIGHTNING_DY_LO
                    && entity.position.y <= at.y + LIGHTNING_DY_HI
                    && (entity.position.z - at.z).abs() <= LIGHTNING_DZ
            })
            .map(|entity| entity.id)
            .collect();
        for victim in victims {
            self.damage_entity(
                victim,
                LIGHTNING_DAMAGE,
                mc_entity::combat::DamageSource::Lightning,
                None,
            );
        }
        Ok(bolt)
    }

    /// One chunk's thunderstorm roll (P20-03 slice 2).
    ///
    /// The jar's `tickThunder` shape, called per loaded chunk in the ticking
    /// radius (here: from the `RandomTicks` sweep, which already walks those
    /// chunks): thundering, then 1-in-`LIGHTNING_CHUNK_ONE_IN`, then a random
    /// column whose top must be rained on. A miss is silent — most ticks
    /// strike nothing anywhere.
    ///
    /// Public like `strike_lightning`: the sweep calls it live, and the
    /// no-thunder gate is pinned by calling it directly a thousand times.
    pub fn maybe_strike_lightning(&mut self, chunk: mc_world::ChunkPos) {
        if !self.weather.thundering {
            return;
        }
        if self.random.next_i32_bounded(LIGHTNING_CHUNK_ONE_IN) != 0 {
            return;
        }
        let x = chunk.x * 16 + self.random.next_i32_bounded(mc_world::SECTION_WIDTH);
        let z = chunk.z * 16 + self.random.next_i32_bounded(mc_world::SECTION_WIDTH);
        let Some(top) = self.world.highest_block(x, z) else {
            return;
        };
        let y = top + 1;
        if !self.is_raining_at(x, y, z) {
            return;
        }
        let _ = self.strike_lightning(x, y, z);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RAIN_DELAY_MAX, RAIN_DELAY_MIN, RAIN_DURATION_MAX, RAIN_DURATION_MIN, THUNDER_DELAY_MAX,
        THUNDER_DELAY_MIN, THUNDER_DURATION_MAX, THUNDER_DURATION_MIN, WeatherState,
    };

    /// The jar's `UniformInt` bounds, pinned: `ServerLevel` static init reads
    /// `RAIN_DELAY (12000, 180000)`, `RAIN_DURATION (12000, 24000)`,
    /// `THUNDER_DELAY (12000, 180000)`, `THUNDER_DURATION (3600, 15600)`.
    /// A drifted constant reschedules every storm on every world.
    #[test]
    fn duration_bounds_match_the_jar() {
        assert_eq!((RAIN_DELAY_MIN, RAIN_DELAY_MAX), (12_000, 180_000));
        assert_eq!((RAIN_DURATION_MIN, RAIN_DURATION_MAX), (12_000, 24_000));
        assert_eq!((THUNDER_DELAY_MIN, THUNDER_DELAY_MAX), (12_000, 180_000));
        assert_eq!(
            (THUNDER_DURATION_MIN, THUNDER_DURATION_MAX),
            (3_600, 15_600)
        );
    }

    /// The strike roll runs at the jar's rate: one in 100 000 draws hits.
    ///
    /// A million draws off the seeded source, asserting a band around the
    /// expected ten (sd ~3.2, so 2..=18 is ±2.5σ; the seed fixes the draws,
    /// so this is a pin rather than a gamble). A zeroed bound or a dropped
    /// roll lands outside the band.
    #[test]
    fn strike_roll_matches_the_jar_rate() {
        use mc_core::random::RandomSource;
        let mut random = RandomSource::new(super::super::DEFAULT_RANDOM_SEED);
        let mut strikes = 0;
        for _ in 0..1_000_000 {
            if random.next_i32_bounded(super::LIGHTNING_CHUNK_ONE_IN) == 0 {
                strikes += 1;
            }
        }
        assert!(
            (2..=18).contains(&strikes),
            "one-in-100 000 over a million draws lands ~10, saw {strikes}"
        );
    }

    /// `to_document` round-trips through `from_document`, including the
    /// `prepareWeather` levels and the tolerant read of a fresh file.
    #[test]
    fn the_document_round_trips() {
        let storm = WeatherState {
            clear_weather_time: 0,
            rain_time: 5_000,
            thunder_time: 3_000,
            raining: true,
            thundering: true,
            rain_level: 1.0,
            thunder_level: 1.0,
        };
        let back = WeatherState::from_document(&storm.to_document());
        assert_eq!(back, storm);
        // A fresh/empty document is clear skies, not an error.
        let fresh = WeatherState::from_document(&mc_nbt::NbtTag::compound(Vec::new()));
        assert_eq!(fresh, WeatherState::default());
    }
}
