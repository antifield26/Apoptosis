//! Random-tick growth, slice 1: crops and farmland moisture (P20-02).
//!
//! ## Jar truth (26.1.2, `javap -c` on the server jar, read 2026-10-02)
//!
//! - `CropBlock.randomTick`: `getRawBrightness(pos, 0) >= 9`, `age < maxAge`,
//!   then `nextInt((int)(25.0 / growthSpeed) + 1) == 0` grows one age step
//!   (`setBlock` flag 2).
//! - `CropBlock.getGrowthSpeed`: `1.0` plus, over the 3×3 soil columns below,
//!   `1.0` per column in `GROWS_CROPS` (farmland is its only member in the
//!   26.1.2 pack), `3.0` when that column's `MOISTURE > 0`, quartered for the
//!   eight neighbours; then halved for same-crop rows (both axes, or any
//!   diagonal).
//! - `BeetrootBlock.randomTick`: `nextInt(3) == 0` gates the shared path —
//!   beetroots grow at a third of the rate by construction.
//! - `FarmlandBlock.randomTick`: near water (any `WATER`-tagged fluid in
//!   `pos + (-4,0,-4)..=(4,1,4)`) **or** rain above dries nothing and wets to
//!   7 at once; dry soil ticks moisture down, and moisture-0 soil with no
//!   `MAINTAINS_FARMLAND` block above turns to dirt.
//!
//! ## What this slice does and does not do
//!
//! Done: the sweep draws real positions (P20-01 only counted them), wheat,
//! carrots, potatoes and beetroots grow by the rules above, farmland wets and
//! dries by the rule above. Named gaps, each owned by a later slice or phase:
//! rain falls through it since P20-03 (`is_raining_at` reads live sky);
//! hoe tilling and trampling
//! are player/entity hooks, not random ticks (slice 2); saplings, grass
//! spread, leaf decay, cane, cactus and bone meal are further handlers on the
//! same dispatch (slice 2); `turnToDirt` does not push entities up; crop
//! survival (light/soil checks on place) is not modelled, so a crop floats
//! where Vanilla would pop it.
//!
//! ## Light (a consequence, not a new gap)
//!
//! The brightness gate reads the chunk's cached light, which is Vanilla's own
//! rule — but our light model is static (no day/night dimming, README): crops
//! therefore grow through the night here. That follows from the recorded
//! lighting boundary; dimming arrives with the light model, not with growth.

use super::Game;
use mc_world::ChunkPos;

/// Game-rule default until P20-05 stores it: `javap -c GameRules` registers
/// `random_tick_speed` as `registerInteger("random_tick_speed", UPDATES, 3, 0)`.
pub(crate) const RANDOM_TICK_SPEED: usize = 3;

/// Crops grown by the shared `CropBlock.randomTick` path (slice 1).
///
/// Names are Vanilla ids; the fixture pins the age bands below.
const CROP_WHEAT: &str = "minecraft:wheat";
const CROP_CARROTS: &str = "minecraft:carrots";
const CROP_POTATOES: &str = "minecraft:potatoes";
const CROP_BEETROOTS: &str = "minecraft:beetroots";

/// Max ages, jar-sourced via the fixture's state tables (P20-02 pins assert
/// these against the registry, so a fixture change fails loudly here).
const WHEAT_MAX_AGE: i32 = 7;
const CARROTS_MAX_AGE: i32 = 7;
const POTATOES_MAX_AGE: i32 = 7;
const BEETROOTS_MAX_AGE: i32 = 3;

/// `FarmlandBlock.MAX_MOISTURE` (jar static): hydration wets straight to this.
const MAX_MOISTURE: i32 = 7;

/// Brightness gate both crop rules share (`getRawBrightness(pos, 0) >= 9`).
const GROWTH_LIGHT: u8 = 9;

/// Half-extent of the `isNearWater` scan (jar: `(-4,0,-4)..=(4,1,4)`).
const WATER_DX: i32 = 4;
const WATER_DY_LO: i32 = 0;
const WATER_DY_HI: i32 = 1;

impl Game {
    /// One random-tick sample at `(x, y, z)`: dispatch to the block's handler.
    ///
    /// Returns whether the sample applied a change (the `random_ticks_applied`
    /// counter). Unknown blocks are silent no-ops — the sweep's early-out —
    /// and an unresolvable name is a skip, never a panic (AGENTS.md §9).
    pub(crate) fn random_tick_block(&mut self, x: i32, y: i32, z: i32, id: i32) -> bool {
        let Ok(name) = self.registries.blocks.block_name(id) else {
            return false;
        };
        // Owned before any `&mut self` call: the name borrows the registry,
        // and the handlers below need the whole game mutably.
        let name = name.to_owned();
        match name.as_str() {
            CROP_WHEAT | CROP_CARROTS | CROP_POTATOES | CROP_BEETROOTS => {
                self.tick_crop(x, y, z, id, &name)
            }
            "minecraft:farmland" => self.tick_farmland(x, y, z, id),
            _ => false,
        }
    }

    /// The shared crop path (`CropBlock.randomTick` + the beetroot pre-gate).
    fn tick_crop(&mut self, x: i32, y: i32, z: i32, id: i32, name: &str) -> bool {
        // Beetroot's override runs the shared path only a third of the time.
        if name == CROP_BEETROOTS && self.random.next_i32_bounded(3) != 0 {
            return false;
        }
        if self.raw_brightness(x, y, z) < GROWTH_LIGHT {
            return false;
        }
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let Some(age) = int_property(&properties, "age") else {
            return false;
        };
        if age >= max_age_for(name) {
            return false;
        }
        let speed = self.crop_growth_speed(x, y, z);
        // `(int)(25.0F / f)`: truncation, and `f >= 1.0` always, so the bound
        // is at least 2 — `nextInt` never sees a hostile zero from here.
        let bound = (25.0_f32 / speed) as i32 + 1;
        if self.random.next_i32_bounded(bound) != 0 {
            return false;
        }
        self.write_property(x, y, z, name, "age", age + 1)
    }

    /// The jar's `getGrowthSpeed` for the crop at `(x, y, z)`.
    fn crop_growth_speed(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut speed = 1.0_f32;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let mut point = 0.0_f32;
                if let Some(below) = self.world.get_block_loaded(x + dx, y - 1, z + dz)
                    && self.grows_crops.contains(&below)
                {
                    point = 1.0;
                    let props = self
                        .registries
                        .blocks
                        .properties_of(below)
                        .unwrap_or_default();
                    if int_property(&props, "moisture").is_some_and(|m| m > 0) {
                        point = 3.0;
                    }
                }
                if dx != 0 || dz != 0 {
                    point /= 4.0;
                }
                speed += point;
            }
        }
        let name = self
            .world
            .get_block_loaded(x, y, z)
            .and_then(|id| self.registries.blocks.block_name(id).ok())
            .unwrap_or("");
        let same = |nx: i32, nz: i32| {
            self.world
                .get_block_loaded(nx, y, nz)
                .and_then(|id| self.registries.blocks.block_name(id).ok())
                .is_some_and(|n| n == name)
        };
        let x_row = same(x - 1, z) || same(x + 1, z);
        let z_row = same(x, z - 1) || same(x, z + 1);
        if (x_row && z_row)
            || same(x - 1, z - 1)
            || same(x + 1, z - 1)
            || same(x + 1, z + 1)
            || same(x - 1, z + 1)
        {
            speed /= 2.0;
        }
        speed
    }

    /// `FarmlandBlock.randomTick`: wet to 7 near water, dry down otherwise,
    /// dirt when dry and uncovered.
    fn tick_farmland(&mut self, x: i32, y: i32, z: i32, id: i32) -> bool {
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let Some(moisture) = int_property(&properties, "moisture") else {
            return false;
        };
        // The rain arm reads live sky since P20-03.
        let wet = self.farmland_is_near_water(x, y, z) || self.is_raining_at(x, y + 1, z);
        if wet {
            if moisture >= MAX_MOISTURE {
                return false;
            }
            return self.write_property(x, y, z, "minecraft:farmland", "moisture", MAX_MOISTURE);
        }
        if moisture > 0 {
            return self.write_property(x, y, z, "minecraft:farmland", "moisture", moisture - 1);
        }
        if self.maintains_farmland_above(x, y, z) {
            return false;
        }
        self.turn_to_dirt(x, y, z)
    }

    /// The jar's `isNearWater`: any `WATER` fluid in `(-4,0,-4)..=(4,1,4)`.
    ///
    /// Reads through [`Game::fluid_state_at`], the same reading the flow
    /// engine uses, so hydration and flow cannot disagree about water.
    fn farmland_is_near_water(&self, x: i32, y: i32, z: i32) -> bool {
        for dx in -WATER_DX..=WATER_DX {
            for dy in WATER_DY_LO..=WATER_DY_HI {
                for dz in -WATER_DX..=WATER_DX {
                    if matches!(
                        self.fluid_state_at(x + dx, y + dy, z + dz),
                        mc_simulation::FluidState::Water(_)
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The block above is in `MAINTAINS_FARMLAND` (crops hold dry soil).
    fn maintains_farmland_above(&self, x: i32, y: i32, z: i32) -> bool {
        self.world
            .get_block_loaded(x, y + 1, z)
            .is_some_and(|above| self.maintains_farmland.contains(&above))
    }

    /// `getRawBrightness(pos, 0)`: the brighter of sky and block light.
    ///
    /// An unlit chunk reads 0 (no growth in unknown light, never a guess);
    /// every sampled y sits inside the world, so the jar's above-world 15 has
    /// no call site here.
    fn raw_brightness(&mut self, x: i32, y: i32, z: i32) -> u8 {
        self.light_at(x, y, z)
            .map_or(0, |(block, sky)| block.max(sky))
    }

    /// Rewrite one integer property of the block at `(x, y, z)` on the
    /// canonical edit path (`set_block` + `block_feed`, as bucket and session
    /// edits do), so growth dirties the chunk, feeds neighbours and
    /// broadcasts like any other edit.
    ///
    /// The assignment is re-read rather than passed in: seven arguments is
    /// the lint ceiling, and the lookup is one table probe.
    fn write_property(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        name: &str,
        key: &str,
        value: i32,
    ) -> bool {
        let Some(current) = self.world.get_block_loaded(x, y, z) else {
            return false;
        };
        let mut rewritten = self
            .registries
            .blocks
            .properties_of(current)
            .unwrap_or_default();
        let Some(entry) = rewritten.iter_mut().find(|(k, _)| k == key) else {
            return false;
        };
        entry.1 = value.to_string();
        let Ok(new_id) = self.registries.blocks.state_id(name, &rewritten) else {
            return false;
        };
        if self.world.set_block(x, y, z, new_id).is_err() {
            return false;
        }
        self.block_feed(x, y, z, new_id);
        true
    }

    /// Turn farmland to dirt on the canonical edit path (jar
    /// `FarmlandBlock.turnToDirt`, minus `pushEntitiesUp`: entities inside the
    /// cell stay where they are, a named gap since slice 1).
    ///
    /// Shared by dry-out and trampling — every caller names the jar rule it
    /// implements.
    pub(crate) fn turn_to_dirt(&mut self, x: i32, y: i32, z: i32) -> bool {
        let Ok(dirt) = self.registries.blocks.default_state("minecraft:dirt") else {
            return false;
        };
        if self.world.set_block(x, y, z, dirt).is_err() {
            return false;
        }
        self.block_feed(x, y, z, dirt);
        true
    }

    /// Roll trampling when something lands on farmland (jar
    /// `FarmlandBlock.fallOn`): the roll is `nextFloat < fallDistance - 0.5`
    /// with no per-tick minimum — a ≥1.5-block fall always qualifies, here
    /// and in vanilla, while shorter hops inherit the documented P05-segment
    /// interaction (our fall distance is per tick, vanilla's accumulates).
    pub(crate) fn trample_on_landing(&mut self, x: i32, y: i32, z: i32, fall: f64) {
        let is_farmland = self
            .world
            .get_block_loaded(x, y, z)
            .and_then(|cell| self.registries.blocks.block_name(cell).ok())
            .is_some_and(|name| name == "minecraft:farmland");
        if is_farmland && self.random.next_f64() < fall - 0.5 {
            self.turn_to_dirt(x, y, z);
        }
    }

    /// Sweep helper: the loaded chunks inside the ticking radius around loaded
    /// entity positions (ADR-0009 §2.4; `simulation_distance` arrives with
    /// P20-05, until then the loader's `view_distance` is the radius).
    pub(crate) fn random_tick_chunks(&self) -> std::collections::BTreeSet<ChunkPos> {
        let radius = self.view_distance.clamp(2, 16);
        let mut centres: Vec<ChunkPos> = self
            .entity_ids
            .values()
            .filter_map(|id| self.entities.get(*id).map(|entity| entity.position))
            .map(|position| super::chunk_of(position.x, position.z))
            .collect();
        centres.sort_unstable();
        centres.dedup();
        let mut chunks = std::collections::BTreeSet::new();
        for centre in centres {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let pos = ChunkPos::new(centre.x + dx, centre.z + dz);
                    if self.world.is_loaded(pos) {
                        chunks.insert(pos);
                    }
                }
            }
        }
        chunks
    }
}

/// Max age for a slice-1 crop (fixture-pinned; see the growth tests).
fn max_age_for(name: &str) -> i32 {
    match name {
        "minecraft:wheat" => WHEAT_MAX_AGE,
        "minecraft:carrots" => CARROTS_MAX_AGE,
        "minecraft:potatoes" => POTATOES_MAX_AGE,
        "minecraft:beetroots" => BEETROOTS_MAX_AGE,
        _ => 0,
    }
}

/// Parse one integer property out of a `properties_of` assignment.
fn int_property(properties: &[(String, String)], key: &str) -> Option<i32> {
    properties
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.parse().ok())
}
