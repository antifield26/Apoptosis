//! Natural mob spawning — the measured rules, the tables they read, and the
//! despawn pass (P11-01).
//!
//! # Where every number comes from
//!
//! This module is deliberately cluttered with provenance: a spawn rule that is
//! "roughly vanilla" is a spawn rule nobody can debug. Each constant below
//! carries its source, and the sources are of exactly three kinds:
//!
//! 1. **Jar bytecode** — `javap -c`/`-v` on the Mojang-mapped server jar in
//!    `target/vanilla-capture/versions/26.1.2/server-26.1.2.jar`. Method
//!    *shapes* are in the named method's bytecode; compile-time constants live
//!    in the class file's `ConstantValue` attributes, which `javap -c` inlines
//!    and `javap -v` prints.
//! 2. **Jar data pack** — the JSON files under `data/minecraft/` in the same
//!    jar, which are vanilla's own datapack. The per-biome spawn tables
//!    (`data/minecraft/worldgen/biome/*.json`), the overworld dimension type's
//!    spawn-light parameters and the day timeline's sky-light track are all
//!    extracted, not quoted from memory.
//! 3. **Named simplifications** — places this build's model is smaller than
//!    vanilla's, written as such with the vanilla shape described, so the gap
//!    is a decision on the record rather than a guess in the code.
//!
//! # The rules, in vanilla's own words
//!
//! **Monsters** (`Monster.checkMonsterSpawnRules` + `isDarkEnoughToSpawn`,
//! both read off the bytecode): difficulty is not peaceful; a sky-lit position
//! passes only if `sky_light <= random.nextInt(32)` (the 1-in-32 "let a few
//! through" roll); block light must be at most the dimension's
//! `monster_spawn_block_light_limit` (**0** for the overworld, from
//! `data/minecraft/dimension_type/overworld.json` — a torch forbids spawns);
//! and finally `max(block_light, sky_light - sky_darken)` must be at most a
//! `uniform(0, 7)` sample (the dimension's `monster_spawn_light_level`).
//! `sky_darken` itself is `15 - sky_light_level` where `sky_light_level` is
//! the day timeline's `gameplay/sky_light_level` track — keyframes measured
//! from `data/minecraft/timeline/day.json` — truncated to an int exactly as
//! `updateSkyBrightness`'s `f2i` does.
//!
//! **Animals** (`Animal.checkAnimalSpawnRules` + `isBrightEnoughToSpawn`):
//! the raw brightness (block and sky, no darken) must exceed **8**, and the
//! block below must be in the `ANIMALS_SPAWNABLE_ON` block tag, which 26.1.2's
//! `data/minecraft/tags/block/animals_spawnable_on.json` says is exactly
//! `minecraft:grass_block`.
//!
//! **Caps and distances** (`MobCategory`'s enum constructor arguments and
//! `NaturalSpawner`'s `ConstantValue`s): monsters cap at **70**, creatures at
//! **10**; the despawn distance is **128** blocks for both; the no-despawn
//! ring is **32**; and no mob spawns within **24** blocks of a player
//! (`MIN_SPAWN_DISTANCE`), inside a chunk ring of **8** chunks
//! (`SPAWN_DISTANCE_CHUNK`). **Despawn** (`Mob.checkDespawn`): beyond the
//! despawn distance a mob is discarded unless `removeWhenFarAway` says no —
//! creatures are persistent (`isPersistent = true`) and return false there,
//! so only monsters despawn by distance; inside it, a mob whose
//! `noActionTime` exceeds **600** is discarded on a **1-in-800** per-tick roll
//! unless it is within the 32-block ring, which resets the counter.
//!
//! # Named simplifications
//!
//! - **Cap scope**: vanilla's caps are per spawn cycle per player group; this
//!   build counts per world, which for the single-figure player counts this
//!   server supports is the same thing in practice and is called out here in
//!   case that stops being true.
//! - **Pack geometry**: vanilla spreads a pack across nearby cells and
//!   re-validates each member; this build lands every member on the one
//!   validated cell. Movement (P11-02) spreads them.
//! - **Category order per position**: vanilla draws one category per chunk
//!   attempt from its spawning list; this build tries monsters before
//!   creatures at every position, which biases attempt counts toward
//!   monsters when both categories are near their caps.

// The sky-darken curve replicates vanilla's own f32 arithmetic (a timeline
// multiplier applied to 15.0f, then `f2i`), so the casts here are the measured
// semantics, not accidents; the same exemption is documented in `game.rs`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use mc_entity::MobKind;
use mc_simulation::RandomSource;

/// `NaturalSpawner.SPAWN_DISTANCE_CHUNK` minus one, doubled, plus one: the
/// full chunk ring a cycle sweeps, exactly vanilla's shape.
pub const RING_CHUNKS: i32 = 2 * SPAWN_DISTANCE_CHUNK + 1;

/// `NaturalSpawner.MIN_SPAWN_DISTANCE` — no spawn within 24 blocks of a player.
pub const MIN_SPAWN_DISTANCE_SQR: f64 = 24.0 * 24.0;

/// `NaturalSpawner.SPAWN_DISTANCE_CHUNK` — the chunk ring radius around a player.
pub const SPAWN_DISTANCE_CHUNK: i32 = 8;

/// `MobCategory.MONSTER`'s per-cycle instance cap (its enum constructor argument).
pub const MONSTER_MAX_INSTANCES: i32 = 70;

/// `MobCategory.CREATURE`'s per-cycle instance cap (its enum constructor argument).
pub const CREATURE_MAX_INSTANCES: i32 = 10;

/// `MobCategory`'s `despawnDistance` — both modeled categories carry 128.
pub const DESPAWN_DISTANCE_SQR: f64 = 128.0 * 128.0;

/// `MobCategory`'s `noDespawnDistance` — 32 for every category (set inside the
/// constructor, shared by all constants).
pub const NO_DESPAWN_DISTANCE_SQR: f64 = 32.0 * 32.0;

/// `Mob.checkDespawn`'s idle threshold for the random despawn roll.
pub const NO_ACTION_LIMIT: u64 = 600;

/// `Mob.checkDespawn`'s random despawn roll: 1-in-800 per tick.
pub const DESPAWN_ROLL: i32 = 800;

/// `isDarkEnoughToSpawn`'s sky pass-through: a sky-lit position is rejected
/// unless `sky_light <= nextInt(32)`.
pub const SKY_PASS_ROLL: i32 = 32;

/// The overworld dimension type's `monster_spawn_block_light_limit`.
pub const MONSTER_BLOCK_LIGHT_LIMIT: u8 = 0;

/// The overworld dimension type's `monster_spawn_light_level` upper bound — a
/// uniform sample over 0..=7 replaces the brightness comparison.
pub const MONSTER_LIGHT_TEST_MAX: u8 = 7;

/// `Animal.isBrightEnoughToSpawn`'s threshold: raw brightness strictly above 8.
pub const ANIMAL_MIN_RAW_BRIGHTNESS: u8 = 8;

/// One day of the overworld clock, from `data/minecraft/timeline/day.json`'s
/// `period_ticks`.
pub const DAY_PERIOD_TICKS: i64 = 24_000;

/// The sky-light multiplier's night value, from the same timeline's
/// `gameplay/sky_light_level` track: 4/15, i.e. moonlight is sky light 4.
const NIGHT_SKY_LIGHT: f32 = 4.0 / 15.0;

/// The overworld clock's `time_markers`, from `day.json`: day begins 1000,
/// night 13000 — the two this build's tests pin.
pub const DAY_START_TICK: i64 = 1_000;
/// The overworld clock's `night` marker, from `day.json`.
pub const NIGHT_START_TICK: i64 = 13_000;

/// Time of day on the overworld clock.
///
/// The wire no longer carries it (26.1.2 removed `time_of_day`; see
/// `send_world_time`), so the server keeps its own: `(world_age + offset)`
/// modulo the clock's 24 000-tick period.
#[must_use]
pub fn time_of_day(world_age: i64, offset: i64) -> i64 {
    (world_age + offset).rem_euclid(DAY_PERIOD_TICKS)
}

/// The overworld's sky darkening at `time_of_day` — `15 - sky_light_level`.
///
/// `sky_light_level` is 15 multiplied by the day timeline's
/// `gameplay/sky_light_level` track, whose keyframes are the measured
/// `(133, 1.0), (11867, 1.0), (13670, 4/15), (22330, 4/15)`: full light
/// through the day, moonlight (4/15) through the night, linear ramps between.
/// The subtraction truncates like `updateSkyBrightness`'s `f2i`.
#[must_use]
pub fn sky_darken(time_of_day: i64) -> i32 {
    let t = time_of_day.rem_euclid(DAY_PERIOD_TICKS);
    // The track is piecewise linear between its keyframes, wrapping at the
    // period. `ticks` here are the track's own, not shifted by anything.
    let multiplier = match t {
        133..=11_866 => 1.0_f32,
        // The ramp spans the keyframes 11867 -> 13670, so the denominator is
        // their distance (1803), not 1802: at 13669 the multiplier is one
        // f32 step short of the night value, and 13670 lands on it.
        11_867..13_670 => {
            let ticks = (t - 11_867) as f64;
            let f = ticks / 1_803.0;
            (1.0 + (f64::from(NIGHT_SKY_LIGHT) - 1.0) * f) as f32
        }
        13_670..=22_329 => NIGHT_SKY_LIGHT,
        _ => {
            // 22_330..=132: the night-to-day ramp, wrapping over the period.
            let ticks = if t >= 22_330 {
                (t - 22_330) as f64
            } else {
                (t + DAY_PERIOD_TICKS - 22_330) as f64
            };
            let f = ticks / 1_803.0;
            (f64::from(NIGHT_SKY_LIGHT) + (1.0 - f64::from(NIGHT_SKY_LIGHT)) * f) as f32
        }
    };
    (15.0 - 15.0 * multiplier) as i32
}

/// Which vanilla `MobCategory` a modeled kind spawns under.
///
/// The four hostiles are `MONSTER`, the four passives `CREATURE`; nothing
/// else is modeled.
#[must_use]
pub const fn category_of(kind: MobKind) -> MobCategory {
    match kind {
        MobKind::Zombie | MobKind::Skeleton | MobKind::Spider | MobKind::Creeper => {
            MobCategory::Monster
        }
        MobKind::Cow | MobKind::Pig | MobKind::Sheep | MobKind::Chicken => MobCategory::Creature,
    }
}

/// The vanilla spawn categories this build models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobCategory {
    /// `MobCategory.MONSTER`: capped at 70, despawns by distance.
    Monster,
    /// `MobCategory.CREATURE`: capped at 10, persistent.
    Creature,
}

impl MobCategory {
    /// The category's per-cycle instance cap.
    #[must_use]
    pub const fn max_instances(self) -> i32 {
        match self {
            Self::Monster => MONSTER_MAX_INSTANCES,
            Self::Creature => CREATURE_MAX_INSTANCES,
        }
    }

    /// Whether a mob of this category despawns by distance: monsters do,
    /// creatures are persistent (`removeWhenFarAway` returns false for them,
    /// as `Animal.removeWhenFarAway`'s bytecode shows).
    #[must_use]
    pub const fn despawns_by_distance(self) -> bool {
        matches!(self, Self::Monster)
    }
}

/// One row of a biome's spawn table: a kind, its weight, and its pack size.
#[derive(Debug, Clone, Copy)]
pub struct SpawnRow {
    /// The mob kind. Only the eight modeled kinds load; the rest are counted.
    pub kind: MobKind,
    /// The row's vanilla weight.
    pub weight: u32,
    /// The pack's minimum size.
    pub min_count: u32,
    /// The pack's maximum size.
    pub max_count: u32,
}

/// One biome's spawn tables, per modeled category.
#[derive(Debug, Default)]
pub struct BiomeSpawners {
    /// `monster` rows for modeled kinds.
    pub monster: Vec<SpawnRow>,
    /// `creature` rows for modeled kinds.
    pub creature: Vec<SpawnRow>,
    /// How many rows named kinds this build does not model. A biome whose
    /// table is all-unmodeled keeps this count and nothing else, which is how
    /// a silently dropped row stays visible.
    pub skipped: usize,
}

impl BiomeSpawners {
    /// The rows for `category`.
    #[must_use]
    pub fn rows(&self, category: MobCategory) -> &[SpawnRow] {
        match category {
            MobCategory::Monster => &self.monster,
            MobCategory::Creature => &self.creature,
        }
    }

    /// Weighted pick, one `nextInt(total)` — the shape vanilla's
    /// `WeighedRandomItem` uses.
    pub(crate) fn pick(
        &self,
        category: MobCategory,
        random: &mut RandomSource,
    ) -> Option<&SpawnRow> {
        let rows = self.rows(category);
        let total: u32 = rows.iter().map(|row| row.weight).sum();
        if total == 0 {
            return None;
        }
        let mut roll = random
            .next_i32_bounded(i32::try_from(total).unwrap_or(i32::MAX))
            .max(0)
            .cast_unsigned();
        for row in rows {
            if roll < row.weight {
                return Some(row);
            }
            roll -= row.weight;
        }
        None
    }
}

/// The per-biome spawn tables, parsed from the committed fixture.
///
/// [`Self::vanilla`] embeds
/// `crates/test-support/fixtures/registry/biome_spawners.tsv`, which is a
/// verbatim extraction of 26.1.2's per-biome spawner data for this build's six
/// biomes (see the fixture's header for the `mountains` → `windswept_hills`
/// rename).
#[derive(Debug, Default)]
pub struct SpawnTables {
    /// Keyed by vanilla biome id, e.g. `minecraft:plains`.
    pub biomes: std::collections::BTreeMap<&'static str, BiomeSpawners>,
}

impl SpawnTables {
    /// Parse the fixture text. Comment and blank lines are skipped; a row
    /// naming an unmodeled kind counts as skipped for its biome rather than
    /// disappearing.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for a malformed row: wrong field count,
    /// non-numeric fields, or an unknown category. The fixture is committed,
    /// so a failure is a defect in it, not in runtime data.
    pub fn parse(text: &'static str) -> ServerResult<Self> {
        let mut tables = Self::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let row = index + 1;
            let [biome, category, mob, weight, min, max] = fields.as_slice() else {
                return Err(ServerError::Protocol(format!(
                    "biome_spawners.tsv line {row}: expected 6 tab-separated fields, got {}",
                    fields.len()
                )));
            };
            let category = match *category {
                "monster" => MobCategory::Monster,
                "creature" => MobCategory::Creature,
                other => {
                    return Err(ServerError::Protocol(format!(
                        "biome_spawners.tsv line {row}: unknown category {other:?}"
                    )));
                }
            };
            let Ok(weight) = weight.parse() else {
                return Err(ServerError::Protocol(format!(
                    "biome_spawners.tsv line {row}: weight {weight:?} is not a number"
                )));
            };
            let Ok(min) = min.parse() else {
                return Err(ServerError::Protocol(format!(
                    "biome_spawners.tsv line {row}: min count {min:?} is not a number"
                )));
            };
            let Ok(max) = max.parse() else {
                return Err(ServerError::Protocol(format!(
                    "biome_spawners.tsv line {row}: max count {max:?} is not a number"
                )));
            };
            let Some(kind) = MobKind::from_name(mob.strip_prefix("minecraft:").unwrap_or(mob))
            else {
                // An unmodeled kind: counted, not dropped on the floor.
                let entry = tables.biomes.entry(*biome).or_default();
                entry.skipped += 1;
                continue;
            };
            let entry = tables.biomes.entry(*biome).or_default();
            let list = match category {
                MobCategory::Monster => &mut entry.monster,
                MobCategory::Creature => &mut entry.creature,
            };
            list.push(SpawnRow {
                kind,
                weight,
                min_count: min,
                max_count: max,
            });
        }
        Ok(tables)
    }

    /// The committed fixture.
    ///
    /// # Panics
    ///
    /// Never in a correct build: the fixture is committed and
    /// [`SpawnTables::parse`] rejects a malformed row. A panic here is the
    /// parse error surfacing at startup rather than a biome spawning nothing.
    #[must_use]
    pub fn vanilla() -> Self {
        Self::parse(include_str!(
            "../../test-support/fixtures/registry/biome_spawners.tsv"
        ))
        .expect("the committed spawn-table fixture parses")
    }
}

/// `Monster.isDarkEnoughToSpawn`, read off its bytecode.
///
/// `block_light`/`sky_light` are the light levels at the candidate position,
/// `sky_darken` the overworld's at the current time, and `random` the game's
/// own source (the 1-in-32 roll and the 0..=7 sample both draw from it).
#[must_use]
pub fn monster_spawn_allowed(
    block_light: u8,
    sky_light: u8,
    sky_darken: i32,
    random: &mut RandomSource,
) -> bool {
    // `if getBrightness(SKY, pos) > random.nextInt(32) return false`: a
    // sky-lit spot is mostly rejected, in vanilla's exact order of the
    // comparison and the roll.
    if i32::from(sky_light) > random.next_i32_bounded(SKY_PASS_ROLL) {
        return false;
    }
    // `if blockLightLimit < 15 && blockLight > limit return false`, with the
    // overworld's limit of 0: any block light forbids the spawn.
    if MONSTER_BLOCK_LIGHT_LIMIT < 15 && block_light > MONSTER_BLOCK_LIGHT_LIMIT {
        return false;
    }
    // `getMaxLocalRawBrightness(pos) <= monsterSpawnLightTest.sample(random)`:
    // the raw brightness under the current sky darkening against a uniform
    // 0..=7 sample.
    let darkened = u8::try_from(sky_darken.clamp(0, 15)).unwrap_or(15);
    let raw = u8::max(block_light, sky_light.saturating_sub(darkened));
    let sample = random.next_i32_bounded(i32::from(MONSTER_LIGHT_TEST_MAX) + 1);
    raw <= u8::try_from(sample.max(0)).unwrap_or(u8::MAX)
}

/// `Animal.checkAnimalSpawnRules`, read off its bytecode.
///
/// `raw_brightness` is `max(block_light, sky_light)` — the animal rule reads
/// brightness with no sky darkening — and `below_is_spawnable` whether the
/// block below is in the `ANIMALS_SPAWNABLE_ON` tag (26.1.2: exactly
/// `minecraft:grass_block`).
#[must_use]
pub const fn animal_spawn_allowed(raw_brightness: u8, below_is_spawnable: bool) -> bool {
    below_is_spawnable && raw_brightness > ANIMAL_MIN_RAW_BRIGHTNESS
}

/// `Mob.checkDespawn` for one mob, at cycle granularity.
///
/// `distance_sq` is the squared distance to the nearest player;
/// `no_action_ticks` the mob's idle counter. Returns `true` when vanilla
/// discards the mob. The 1-in-800 roll draws only when the idle counter has
/// crossed the limit, matching vanilla's ordering of the checks.
#[must_use]
pub fn despawn(
    category: MobCategory,
    distance_sq: f64,
    no_action_ticks: u64,
    random: &mut RandomSource,
) -> bool {
    if distance_sq > DESPAWN_DISTANCE_SQR && category.despawns_by_distance() {
        return true;
    }
    no_action_ticks > NO_ACTION_LIMIT
        && random.next_i32_bounded(DESPAWN_ROLL) == 0
        && distance_sq > NO_DESPAWN_DISTANCE_SQR
}

use mc_core::error::{ServerError, ServerResult};

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    /// The measured sky-light keyframes, pinned at their own ticks.
    #[test]
    fn the_sky_darken_curve_matches_the_day_timeline_keyframes() {
        assert_eq!(sky_darken(133), 0, "the first keyframe is full day");
        assert_eq!(sky_darken(6_000), 0, "noon is full day");
        assert_eq!(
            sky_darken(11_867),
            0,
            "full day through the last day keyframe"
        );
        assert_eq!(
            sky_darken(13_670),
            11,
            "the night keyframe: 15 - 15*(4/15) = 11"
        );
        assert_eq!(sky_darken(18_000), 11, "midnight is full moonlight");
        assert_eq!(
            sky_darken(22_330),
            11,
            "moonlight through the last night keyframe"
        );
    }

    #[test]
    fn the_sky_darken_ramps_are_strictly_between_the_plateaus() {
        // The two ramps: sunset (11867..13670) and the wrapped sunrise
        // (22330..period+133). Values inside them are strictly between 0 and 11
        // and monotonic, so the transition is a transition and not a jump.
        let sunset: Vec<i32> = (11_867..=13_670).step_by(100).map(sky_darken).collect();
        assert_eq!(*sunset.first().expect("non-empty"), 0);
        // The last sample lands on 13667, one f32 step shy of the 13670
        // keyframe (which the keyframe test pins at 11): truncation keeps it
        // at 10, which is the data's own arithmetic and not an off-by-one.
        assert_eq!(*sunset.last().expect("non-empty"), 10);
        for pair in sunset.windows(2) {
            assert!(pair[1] >= pair[0], "sunset never brightens: {sunset:?}");
        }
        // The plateau ends one tick before the night keyframe lands, so the
        // last ramp sample is 10 and the keyframe itself is 11 -- both pinned
        // by the keyframe test above.
        let mut sunrise: Vec<i32> = (22_330..DAY_PERIOD_TICKS)
            .step_by(97)
            .map(sky_darken)
            .collect();
        sunrise.extend((0..=133).map(sky_darken));
        assert!(
            sunrise.len() > 4,
            "the wrapped ramp is sampled on both sides of the wrap"
        );
        // Sunrise brightens the sky, so the darkening falls monotonically
        // from 11 to 0 across the wrap.
        for pair in sunrise.windows(2) {
            assert!(pair[1] <= pair[0], "sunrise never brightens: {sunrise:?}");
        }
    }

    #[test]
    fn time_of_day_wraps_at_the_clock_period() {
        assert_eq!(time_of_day(0, 0), 0);
        assert_eq!(time_of_day(24_000, 0), 0);
        assert_eq!(time_of_day(25_000, 0), 1_000);
        assert_eq!(
            time_of_day(0, 25_000),
            1_000,
            "the offset is applied before the wrap"
        );
        assert_eq!(
            time_of_day(-500, 0),
            23_500,
            "negative ages wrap, like rem_euclid says"
        );
    }

    /// A fixed seed makes every rule decision replayable; these booleans are
    /// the ones this seed produces, so a rule change is a visible change.
    const SEED: i64 = 0x5EED_2612;

    #[test]
    fn a_torch_forbids_every_monster_spawn_regardless_of_the_roll() {
        // Block light 1 exceeds the overworld's limit of 0; the rule returns
        // false before any other check can rescue it.
        for draw in 0..200_u32 {
            let mut random = RandomSource::new(SEED + i64::from(draw));
            assert!(
                !monster_spawn_allowed(1, 0, 0, &mut random),
                "draw {draw}: a torch forbids spawns"
            );
        }
    }

    #[test]
    fn a_pitch_dark_hole_always_admits_a_monster() {
        // Block 0 and sky 0: raw brightness 0 is at most every 0..=7 sample,
        // and sky 0 fails the `sky > roll` rejection outright.
        for draw in 0..200_u32 {
            let mut random = RandomSource::new(SEED + i64::from(draw));
            assert!(
                monster_spawn_allowed(0, 0, 11, &mut random),
                "draw {draw}: darkness admits spawns"
            );
        }
    }

    #[test]
    fn a_sky_lit_surface_at_noon_never_admits_a_monster() {
        // Day: darken 0, sky 15 -> raw 15, which exceeds every 0..=7 sample.
        // The pass-through roll only delays the rejection, never prevents it.
        for draw in 0..200_u32 {
            let mut random = RandomSource::new(SEED + i64::from(draw));
            assert!(
                !monster_spawn_allowed(0, 15, 0, &mut random),
                "draw {draw}: noon surface refuses monsters"
            );
        }
    }

    #[test]
    fn a_sky_lit_surface_at_night_admits_monsters_at_the_measured_rate() {
        // Night: darken 11, sky 15 -> raw 4, which passes when the 0..=7
        // sample lands at 4 or more (4 of 8), after the sky pass-through lets
        // the attempt through at all (sky 15 survives rolls of 15..=31: 17 of
        // 32). The combined rate is therefore 17/32 x 4/8 = 17/64, about
        // 0.266 -- the sky-lit night surface is where vanilla's spawn
        // starvation lives. Counted over 8000 draws; the tolerance is generous
        // because the draws are sequential, not independent, but a wrong
        // threshold, a missing roll or an inverted comparison cannot land
        // inside it.
        let mut random = RandomSource::new(SEED);
        let mut allowed = 0_u32;
        for _ in 0..8000 {
            if monster_spawn_allowed(0, 15, 11, &mut random) {
                allowed += 1;
            }
        }
        assert!(
            (1_800..=2_500).contains(&allowed),
            "night surface admissions {allowed}/8000 should be near the measured 17/64"
        );
    }

    #[test]
    fn the_animal_rule_is_the_grass_and_brightness_conjunction() {
        assert!(animal_spawn_allowed(15, true), "day grass admits animals");
        assert!(animal_spawn_allowed(9, true), "brightness strictly above 8");
        assert!(
            !animal_spawn_allowed(8, true),
            "brightness 8 is not above 8"
        );
        assert!(!animal_spawn_allowed(0, true), "darkness refuses animals");
        assert!(!animal_spawn_allowed(15, false), "the tag below decides");
    }

    #[test]
    fn despawn_distance_applies_to_monsters_and_not_to_creatures() {
        let mut random = RandomSource::new(SEED);
        assert!(
            despawn(
                MobCategory::Monster,
                DESPAWN_DISTANCE_SQR + 1.0,
                0,
                &mut random
            ),
            "a monster beyond 128 blocks is discarded whatever its idle counter"
        );
        assert!(
            !despawn(
                MobCategory::Creature,
                DESPAWN_DISTANCE_SQR + 1.0,
                0,
                &mut random
            ),
            "a creature is persistent and never despawns by distance"
        );
        assert!(
            !despawn(
                MobCategory::Monster,
                DESPAWN_DISTANCE_SQR - 1.0,
                0,
                &mut random
            ),
            "a monster inside the ring with a fresh counter stays"
        );
        assert!(
            !despawn(
                MobCategory::Monster,
                DESPAWN_DISTANCE_SQR - 1.0,
                NO_ACTION_LIMIT,
                &mut random
            ),
            "the idle roll does not fire at or below the 600-tick limit"
        );
        assert!(
            !despawn(
                MobCategory::Monster,
                NO_DESPAWN_DISTANCE_SQR - 1.0,
                NO_ACTION_LIMIT + 1,
                &mut random
            ),
            "inside the 32-block ring nothing despawns"
        );
    }

    #[test]
    fn every_biome_of_the_six_loads_with_the_measured_row_counts() {
        let tables = SpawnTables::vanilla();
        // (biome, monster rows, creature rows, skipped rows) as extracted from
        // the data pack: the modeled kinds load, the rest are counted.
        let expected: &[(&str, usize, usize, usize)] = &[
            // plains: slime/enderman/witch/zombie_villager/zombie_horse and
            // horse/donkey are unmodeled; desert's creature table (rabbit,
            // camel) is entirely unmodeled; taiga loses wolf/rabbit/fox.
            ("minecraft:plains", 4, 4, 7),
            ("minecraft:desert", 4, 0, 8),
            ("minecraft:forest", 4, 4, 5),
            ("minecraft:ocean", 4, 0, 5),
            ("minecraft:windswept_hills", 4, 4, 5),
            ("minecraft:taiga", 4, 4, 7),
        ];
        assert_eq!(tables.biomes.len(), expected.len(), "all six biomes load");
        for (biome, monsters, creatures, skipped) in expected {
            let entry = tables
                .biomes
                .get(biome)
                .unwrap_or_else(|| panic!("{biome} loads"));
            assert_eq!(entry.monster.len(), *monsters, "{biome} monster rows");
            assert_eq!(entry.creature.len(), *creatures, "{biome} creature rows");
            assert_eq!(
                entry.skipped, *skipped,
                "{biome} unmodeled rows are counted"
            );
            for row in entry.monster.iter().chain(entry.creature.iter()) {
                assert!(row.weight > 0, "{biome} rows carry positive weight");
                assert!(row.min_count >= 1, "{biome} packs have at least one member");
                assert!(
                    row.max_count >= row.min_count,
                    "{biome} pack range is ordered"
                );
            }
        }
    }

    #[test]
    fn the_table_pick_replays_and_never_names_an_unmodeled_kind() {
        let tables = SpawnTables::vanilla();
        let plains = &tables.biomes["minecraft:plains"];
        let mut first = RandomSource::new(SEED);
        let mut second = RandomSource::new(SEED);
        for _ in 0..100 {
            let a = plains
                .pick(MobCategory::Monster, &mut first)
                .expect("a row");
            let b = plains
                .pick(MobCategory::Monster, &mut second)
                .expect("a row");
            assert_eq!(a.kind, b.kind, "the same seed replays the same pick");
            assert_eq!(a.weight, b.weight);
        }
        // The desert's creature table (rabbit, camel) is entirely unmodeled:
        // the pick reports absence instead of a neighboring biome's row.
        let desert = &tables.biomes["minecraft:desert"];
        assert!(
            desert.pick(MobCategory::Creature, &mut first).is_none(),
            "an all-unmodeled category picks nothing"
        );
        // The taiga's creature table holds the four passives; every pick is
        // one of them and the seeded sequence replays.
        let taiga = &tables.biomes["minecraft:taiga"];
        let modeled = [MobKind::Sheep, MobKind::Pig, MobKind::Chicken, MobKind::Cow];
        for _ in 0..50 {
            let row = taiga
                .pick(MobCategory::Creature, &mut first)
                .expect("a row");
            assert!(
                modeled.contains(&row.kind),
                "the taiga picks a modeled passive, not {:?}",
                row.kind
            );
        }
    }

    #[test]
    fn every_modeled_kind_round_trips_through_its_name() {
        for kind in MobKind::ALL {
            assert_eq!(MobKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(
            MobKind::from_name("zombie_villager"),
            None,
            "unmodeled kinds stay unmodeled"
        );
        assert_eq!(MobKind::from_name(""), None);
    }

    #[test]
    fn the_categories_match_the_jar_s_enum_arguments() {
        use MobCategory::{Creature, Monster};
        assert_eq!(category_of(MobKind::Zombie), Monster);
        assert_eq!(category_of(MobKind::Skeleton), Monster);
        assert_eq!(category_of(MobKind::Spider), Monster);
        assert_eq!(category_of(MobKind::Creeper), Monster);
        assert_eq!(category_of(MobKind::Cow), Creature);
        assert_eq!(category_of(MobKind::Pig), Creature);
        assert_eq!(category_of(MobKind::Sheep), Creature);
        assert_eq!(category_of(MobKind::Chicken), Creature);
        assert_eq!(Monster.max_instances(), 70);
        assert_eq!(Creature.max_instances(), 10);
        assert!(Monster.despawns_by_distance());
        assert!(!Creature.despawns_by_distance());
    }
}
