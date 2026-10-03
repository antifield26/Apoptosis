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

/// Stalk rules shared by cane and cactus (jar static init, both classes).
struct CaneCactus;

impl CaneCactus {
    /// No stalk grows past three (`height >= 3` returns, both classes).
    const MAX_STACK: i32 = 3;
    /// The age that grows instead of climbing (`age == 15`, both classes).
    const GROW_AGE: i32 = 15;
}

/// Spread attempts per random tick (jar `SpreadingSnowyBlock`: `4`).
const SPREAD_ATTEMPTS: usize = 4;

/// Full light opacity: dampening `>= 15` starves the cell below (jar
/// `canStayAlive` tail).
const FULL_OPACITY: u8 = 15;

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
            "minecraft:sugar_cane" => self.tick_cane(x, y, z, id),
            "minecraft:cactus" => self.tick_cactus(x, y, z, id),
            "minecraft:grass_block" | "minecraft:mycelium" => self.tick_spread(x, y, z, id, &name),
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

    /// Sugar cane growth (`SugarCaneBlock.randomTick`, jar bytecode).
    ///
    /// Only the stack top runs (guard: the cell above must be air): count
    /// consecutive cane below (capped at 3 — the loop stops counting there,
    /// and `height >= 3` returns); at `age == 15` a fresh cane lands above
    /// and this cell resets to 0, otherwise the age climbs. Growth writes
    /// ride the canonical edit path, so the Broadcast phase streams them.
    fn tick_cane(&mut self, x: i32, y: i32, z: i32, id: i32) -> bool {
        if !self.is_empty_block(x, y + 1, z) {
            return false;
        }
        let mut height = 1;
        for depth in 1..=2 {
            let below = self.world.get_block_loaded(x, y - depth, z);
            let is_cane = below.is_some_and(|cell| {
                self.registries
                    .blocks
                    .block_name(cell)
                    .is_ok_and(|name| name == "minecraft:sugar_cane")
            });
            if !is_cane {
                break;
            }
            height += 1;
        }
        if height >= CaneCactus::MAX_STACK {
            return false;
        }
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let Some(age) = int_property(&properties, "age") else {
            return false;
        };
        if age >= CaneCactus::GROW_AGE {
            let Ok(fresh) = self.registries.blocks.default_state("minecraft:sugar_cane") else {
                return false;
            };
            if self.world.set_block(x, y + 1, z, fresh).is_err() {
                return false;
            }
            self.block_feed(x, y + 1, z, fresh);
            return self.write_property(x, y, z, "minecraft:sugar_cane", "age", 0);
        }
        self.write_property(x, y, z, "minecraft:sugar_cane", "age", age + 1)
    }

    /// Cactus growth (`CactusBlock.randomTick`, jar bytecode, minus the
    /// flower arm).
    ///
    /// Same stack-top shape as cane (air above, height < 3, `age == 15`
    /// grows and resets, else the age climbs). The flower branch — planting
    /// `cactus_flower` above at age with a 0.25/0.10 roll — is a named gap:
    /// the constants (`ATTEMPT_GROW_CACTUS_FLOWER_AGE`, both probabilities)
    /// were read but the arm is not wired, so cacti grow flowerless here.
    fn tick_cactus(&mut self, x: i32, y: i32, z: i32, id: i32) -> bool {
        if !self.is_empty_block(x, y + 1, z) {
            return false;
        }
        let mut height = 1;
        for depth in 1..=2 {
            let below = self.world.get_block_loaded(x, y - depth, z);
            let is_cactus = below.is_some_and(|cell| {
                self.registries
                    .blocks
                    .block_name(cell)
                    .is_ok_and(|name| name == "minecraft:cactus")
            });
            if !is_cactus {
                break;
            }
            height += 1;
        }
        if height >= CaneCactus::MAX_STACK {
            return false;
        }
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let Some(age) = int_property(&properties, "age") else {
            return false;
        };
        if age >= CaneCactus::GROW_AGE {
            let Ok(fresh) = self.registries.blocks.default_state("minecraft:cactus") else {
                return false;
            };
            if self.world.set_block(x, y + 1, z, fresh).is_err() {
                return false;
            }
            self.block_feed(x, y + 1, z, fresh);
            return self.write_property(x, y, z, "minecraft:cactus", "age", 0);
        }
        self.write_property(x, y, z, "minecraft:cactus", "age", age + 1)
    }

    /// Grass/mycelium spread and starvation (`SpreadingSnowyBlock.randomTick`).
    ///
    /// Starving first (jar order): without a survivable roof the cell
    /// reverts to dirt. Otherwise, under bright sky (`>= 9` above), four
    /// attempts land at `nextInt(3)-1, nextInt(5)-3, nextInt(3)-1`; a dirt
    /// target that could itself survive there (and carries no water above)
    /// takes this block's state with its `snowy` flag re-read from the snow
    /// above the *target*. Mycelium spreads onto dirt the same way (its jar
    /// base block is dirt, like grass).
    fn tick_spread(&mut self, x: i32, y: i32, z: i32, id: i32, name: &str) -> bool {
        let Ok(spreader) = self.registries.blocks.block_name(id) else {
            return false;
        };
        let spreader = spreader.to_owned();
        if !self.spread_stays_alive(x, y, z) {
            let Ok(dirt) = self.registries.blocks.default_state("minecraft:dirt") else {
                return false;
            };
            if self.world.set_block(x, y, z, dirt).is_err() {
                return false;
            }
            self.block_feed(x, y, z, dirt);
            return true;
        }
        if self.raw_brightness(x, y + 1, z) < GROWTH_LIGHT {
            return false;
        }
        let mut grew = false;
        for _ in 0..SPREAD_ATTEMPTS {
            let tx = x + self.random.next_i32_bounded(3) - 1;
            let ty = y + self.random.next_i32_bounded(5) - 3;
            let tz = z + self.random.next_i32_bounded(3) - 1;
            let Some(target) = self.world.get_block_loaded(tx, ty, tz) else {
                continue;
            };
            let is_dirt = self
                .registries
                .blocks
                .block_name(target)
                .is_ok_and(|target_name| target_name == "minecraft:dirt");
            if !is_dirt || !self.spread_can_propagate(tx, ty, tz) {
                continue;
            }
            let snowy = self.is_snowy_above(tx, ty, tz);
            let mut props = self.registries.blocks.properties_of(id).unwrap_or_default();
            if let Some(entry) = props.iter_mut().find(|(k, _)| k == "snowy") {
                entry.1 = snowy.to_string();
            }
            let Ok(new_id) = self.registries.blocks.state_id(&spreader, &props) else {
                continue;
            };
            if self.world.set_block(tx, ty, tz, new_id).is_err() {
                continue;
            }
            self.block_feed(tx, ty, tz, new_id);
            grew = true;
        }
        // Keep the borrow checker honest: `name` selects the dispatch arm
        // and `spreader` (the owned copy) does the writing.
        let _ = name;
        grew
    }

    /// Whether the cell at `(x, y, z)` is air-like (the stack-top guard both
    /// stalk rules share with the jar's `isEmptyBlock`).
    fn is_empty_block(&self, x: i32, y: i32, z: i32) -> bool {
        self.world
            .get_block_loaded(x, y, z)
            .is_some_and(|cell| self.registries.blocks.is_empty(cell))
    }

    /// Jar `SpreadingSnowyBlock.canStayAlive`: snow with one layer always
    /// suffices; a full (source) fluid above kills; otherwise the roof's
    /// light dampening must stay below full opacity (15).
    fn spread_stays_alive(&self, x: i32, y: i32, z: i32) -> bool {
        let Some(above) = self.world.get_block_loaded(x, y + 1, z) else {
            return false;
        };
        if self.is_single_snow_layer(above) {
            return true;
        }
        if self.is_water_source(x, y + 1, z) {
            return false;
        }
        self.registries.light.dampening(above) < FULL_OPACITY
    }

    /// Jar `canPropagate`: survivable there, and no water (any) above.
    fn spread_can_propagate(&self, x: i32, y: i32, z: i32) -> bool {
        if !self.spread_stays_alive(x, y, z) {
            return false;
        }
        !matches!(
            self.fluid_state_at(x, y + 1, z),
            mc_simulation::FluidState::Water(_)
        )
    }

    /// Whether the cell holds `minecraft:snow` at `layers == 1`.
    fn is_single_snow_layer(&self, id: i32) -> bool {
        if self
            .registries
            .blocks
            .block_name(id)
            .is_ok_and(|name| name == "minecraft:snow")
        {
            let props = self.registries.blocks.properties_of(id).unwrap_or_default();
            return int_property(&props, "layers").is_some_and(|layers| layers == 1);
        }
        false
    }

    /// Whether the cell holds a water *source* (the jar's `isFull`).
    fn is_water_source(&self, x: i32, y: i32, z: i32) -> bool {
        matches!(
            self.fluid_state_at(x, y, z),
            mc_simulation::FluidState::Water(body) if body.is_source()
        )
    }

    /// Whether snow (block or layer) sits above, for the `snowy` flag (the
    /// jar's `isSnowySetting`, subset to the two snow blocks).
    fn is_snowy_above(&self, x: i32, y: i32, z: i32) -> bool {
        self.world
            .get_block_loaded(x, y + 1, z)
            .and_then(|above| self.registries.blocks.block_name(above).ok())
            .is_some_and(|name| name == "minecraft:snow" || name == "minecraft:snow_block")
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

#[cfg(test)]
mod tests {
    //! Mechanism pins for slice 2b, called directly (TEST-TIME-PLAN §2).
    //!
    //! Each pin below proves what a 400-tick farm run proved, with one
    //! handler call instead of a sweep: no players, no streaming, no
    //! broadcast, no light settle (except where a brightness gate is
    //! asserted — then light is computed for the single farm chunk,
    //! directly). The one full-tick wiring test per area stays in
    //! `tests/spread.rs`; these prove the mechanisms.

    use super::Game;
    use crate::storage::WorldService;
    use mc_world::ChunkPos;

    /// A game with owned storage on a scratch dir (no players, one chunk).
    fn bare_game(tag: &str) -> (Game, mc_test_support::fixtures::TempDir) {
        let dir = mc_test_support::fixtures::TempDir::new(tag);
        let config = crate::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (_tx, rx) = mc_network::bridge::game_channel(64);
        let game = Game::with_seed_and_storage(storage, 2, rx, crate::game::DEFAULT_RANDOM_SEED)
            .expect("game builds");
        (game, dir)
    }

    fn aged(game: &Game, name: &str, age: i32) -> i32 {
        game.registries()
            .blocks
            .state_id(name, &[("age".to_owned(), age.to_string())])
            .expect("aged stalk state")
    }

    fn age_of(game: &Game, x: i32, y: i32, z: i32) -> i32 {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        game.registries()
            .blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "age")
            .and_then(|(_, v)| v.parse().ok())
            .expect("age reads")
    }

    fn stalk_height(game: &Game, name: &str, x: i32, y: i32, z: i32) -> usize {
        let blocks = &game.registries().blocks;
        let mut height = 0_usize;
        for dy in 0..8 {
            let Some(id) = game.world().get_block_loaded(x, y + dy, z) else {
                break;
            };
            if blocks.block_name(id).unwrap_or("") != name {
                break;
            }
            height += 1;
        }
        height
    }

    #[test]
    fn cane_grows_and_resets_on_direct_call() {
        let (mut game, _dir) = bare_game("cane-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let sand = game
            .registries()
            .blocks
            .default_state("minecraft:sand")
            .expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        let stalk = aged(&game, "minecraft:sugar_cane", 15);
        game.world_mut()
            .set_block(8, 121, 8, stalk)
            .expect("cane placed");
        let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 121, 8, id),
            "a max-age tick applies"
        );
        assert_eq!(
            stalk_height(&game, "minecraft:sugar_cane", 8, 121, 8),
            2,
            "one direct call grows one level"
        );
        assert_eq!(age_of(&game, 8, 121, 8), 0, "the grower resets to 0");
    }

    #[test]
    fn cane_climbs_on_direct_call() {
        let (mut game, _dir) = bare_game("cane-climb-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let sand = game
            .registries()
            .blocks
            .default_state("minecraft:sand")
            .expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        let stalk = aged(&game, "minecraft:sugar_cane", 0);
        game.world_mut()
            .set_block(8, 121, 8, stalk)
            .expect("cane placed");
        let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 121, 8, id),
            "a young tick applies"
        );
        assert_eq!(age_of(&game, 8, 121, 8), 1, "the age climbs by one");
        assert_eq!(
            stalk_height(&game, "minecraft:sugar_cane", 8, 121, 8),
            1,
            "climbing does not grow"
        );
    }

    #[test]
    fn cane_cap_holds_on_direct_call() {
        let (mut game, _dir) = bare_game("cane-cap-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let sand = blocks.default_state("minecraft:sand").expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        for dy in 1..=3 {
            let stalk = blocks
                .state_id(
                    "minecraft:sugar_cane",
                    &[("age".to_owned(), "15".to_owned())],
                )
                .expect("max-age cane");
            game.world_mut()
                .set_block(8, 120 + dy, 8, stalk)
                .expect("cane placed");
        }
        let id = game.world().get_block_loaded(8, 123, 8).expect("loaded");
        assert!(
            !game.random_tick_block(8, 123, 8, id),
            "a full stack applies nothing"
        );
        assert_eq!(
            stalk_height(&game, "minecraft:sugar_cane", 8, 121, 8),
            3,
            "a full stack stays full"
        );
    }

    #[test]
    fn cactus_grows_and_resets_on_direct_call() {
        let (mut game, _dir) = bare_game("cactus-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let sand = game
            .registries()
            .blocks
            .default_state("minecraft:sand")
            .expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        let stalk = aged(&game, "minecraft:cactus", 15);
        game.world_mut()
            .set_block(8, 121, 8, stalk)
            .expect("cactus placed");
        let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 121, 8, id),
            "a max-age tick applies"
        );
        assert_eq!(
            stalk_height(&game, "minecraft:cactus", 8, 121, 8),
            2,
            "one direct call grows one level"
        );
        assert_eq!(age_of(&game, 8, 121, 8), 0, "the grower resets to 0");
    }

    #[test]
    fn cactus_climbs_on_direct_call() {
        let (mut game, _dir) = bare_game("cactus-climb-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let sand = game
            .registries()
            .blocks
            .default_state("minecraft:sand")
            .expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        let stalk = aged(&game, "minecraft:cactus", 0);
        game.world_mut()
            .set_block(8, 121, 8, stalk)
            .expect("cactus placed");
        let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 121, 8, id),
            "a young tick applies"
        );
        assert_eq!(age_of(&game, 8, 121, 8), 1, "the age climbs by one");
    }

    #[test]
    fn cactus_cap_holds_on_direct_call() {
        let (mut game, _dir) = bare_game("cactus-cap-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let sand = blocks.default_state("minecraft:sand").expect("sand");
        game.world_mut()
            .set_block(8, 120, 8, sand)
            .expect("sand placed");
        for dy in 1..=3 {
            let stalk = blocks
                .state_id("minecraft:cactus", &[("age".to_owned(), "15".to_owned())])
                .expect("max-age cactus");
            game.world_mut()
                .set_block(8, 120 + dy, 8, stalk)
                .expect("cactus placed");
        }
        let id = game.world().get_block_loaded(8, 123, 8).expect("loaded");
        assert!(
            !game.random_tick_block(8, 123, 8, id),
            "a full stack applies nothing"
        );
        assert_eq!(
            stalk_height(&game, "minecraft:cactus", 8, 121, 8),
            3,
            "a full stack stays full"
        );
    }

    /// Light over exactly the farm chunk (what the brightness gate reads;
    /// computed directly, not settled through ticks).
    fn light_farm(game: &mut Game) {
        let table = game.registries().light.clone();
        let pos = ChunkPos::new(0, 0);
        game.world_mut().invalidate_light_3x3(pos);
        game.world_mut()
            .compute_light(pos, &table)
            .expect("light computes");
    }

    #[test]
    fn grass_starves_on_direct_call() {
        let (mut game, _dir) = bare_game("grass-starve-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let grass = blocks
            .default_state("minecraft:grass_block")
            .expect("grass");
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        game.world_mut()
            .set_block(8, 120, 8, grass)
            .expect("grass placed");
        game.world_mut()
            .set_block(8, 121, 8, stone)
            .expect("roof placed");
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 120, 8, id),
            "a covered tick applies"
        );
        assert_eq!(
            blocks
                .block_name(game.world().get_block_loaded(8, 120, 8).expect("loaded"))
                .expect("registered"),
            "minecraft:dirt",
            "covered grass starves to dirt"
        );
    }

    #[test]
    fn grass_spreads_on_direct_calls() {
        let (mut game, _dir) = bare_game("grass-spread-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let grass = blocks
            .default_state("minecraft:grass_block")
            .expect("grass");
        let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
        game.world_mut()
            .set_block(8, 120, 8, grass)
            .expect("grass placed");
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            game.world_mut()
                .set_block(8 + dx, 120, 8 + dz, dirt)
                .expect("dirt placed");
        }
        light_farm(&mut game);
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        let mut grown = false;
        // Four attempts per call over four dirt neighbours: bounded draws
        // from the fixed seed, so this terminates either way.
        for _ in 0..30 {
            if game.random_tick_block(8, 120, 8, id) {
                grown = true;
                break;
            }
        }
        assert!(grown, "open grass converts neighbouring dirt");
        let converted = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dz)| {
            game.world()
                .get_block_loaded(8 + dx, 120, 8 + dz)
                .and_then(|cell| blocks.block_name(cell).ok())
                .is_some_and(|name| name == "minecraft:grass_block")
        });
        assert!(converted, "a dirt neighbour is grass now");
    }

    #[test]
    fn grass_refuses_water_roofed_dirt() {
        let (mut game, _dir) = bare_game("grass-refuse-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let grass = blocks
            .default_state("minecraft:grass_block")
            .expect("grass");
        let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
        let water = blocks.default_state("minecraft:water").expect("water");
        game.world_mut()
            .set_block(8, 120, 8, grass)
            .expect("grass placed");
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            game.world_mut()
                .set_block(8 + dx, 120, 8 + dz, dirt)
                .expect("dirt placed");
            game.world_mut()
                .set_block(8 + dx, 121, 8 + dz, water)
                .expect("water placed");
        }
        light_farm(&mut game);
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        for _ in 0..30 {
            let _ = game.random_tick_block(8, 120, 8, id);
        }
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let cell = game
                .world()
                .get_block_loaded(8 + dx, 120, 8 + dz)
                .expect("loaded");
            assert_eq!(
                blocks.block_name(cell).expect("registered"),
                "minecraft:dirt",
                "water-roofed dirt never converts"
            );
        }
    }

    #[test]
    fn mycelium_spreads_on_direct_calls() {
        let (mut game, _dir) = bare_game("mycelium-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let mycelium = blocks
            .default_state("minecraft:mycelium")
            .expect("mycelium");
        let dirt = blocks.default_state("minecraft:dirt").expect("dirt");
        game.world_mut()
            .set_block(8, 120, 8, mycelium)
            .expect("mycelium placed");
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            game.world_mut()
                .set_block(8 + dx, 120, 8 + dz, dirt)
                .expect("dirt placed");
        }
        light_farm(&mut game);
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        // Mycelium and grass share the handler; the odd loop below is the
        // same bounded-draw shape as the grass pin (fixed seed, terminates).
        let mut grown = false;
        for _ in 0..30 {
            if game.random_tick_block(8, 120, 8, id) {
                grown = true;
                break;
            }
        }
        assert!(grown, "open mycelium converts neighbouring dirt");
    }
}
