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
//! Slice 2b added cane, cactus, grass and mycelium spread on the same
//! dispatch. Slice 2c (this slice) adds oak saplings (stage, then the
//! worldgen oak feature grown at runtime), leaf decay with on-demand
//! distance repair, and the `UseItemOn` bone-meal arm in `session.rs`.
//! Named gaps inside slice 2c: only oak grows (the only tree feature);
//! bone meal advances crops one stage (the jar grows several — the exact
//! constants were not re-read here) and does not flora-spread grass;
//! saplings skip the soil check, in the same gap class as crop survival.
//!
//! ## Light (a consequence, not a new gap)
//!
//! The brightness gate reads the chunk's cached light, which is Vanilla's own
//! rule — but our light model is static (no day/night dimming, README): crops
//! therefore grow through the night here. That follows from the recorded
//! lighting boundary; dimming arrives with the light model, not with growth.

use super::Game;
use mc_world::ChunkPos;

/// Jar default for `random_tick_speed`, pinned: `javap -c GameRules` registers
/// it as `registerInteger("random_tick_speed", UPDATES, 3, 0)`.
///
/// The sweep reads the live stored value (`Game::rules.tick_speed()`); this
/// constant pins the default the rate tests assume, so a default change fails
/// loudly here instead of silently halving every farm.
pub(crate) const RANDOM_TICK_SPEED_DEFAULT: usize = 3;

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

/// Sapling draws: the jar's `SaplingBlock.randomTick` grows on 1-in-7.
const SAPLING_TICK_ONE_IN: i32 = 7;

/// Leaf distance that decays (`LeavesBlock`: `!persistent && distance == 7`).
const LEAF_DECAY_DISTANCE: i32 = 7;

/// Support-search radius for the on-demand leaf repair (a log farther than
/// this cannot sustain the cell, same bound as the decay rule above).
const LEAF_SUPPORT_RADIUS: i32 = 7;

/// Documented vanilla drop rates for oak-family leaves, *not* jar-read:
/// 1-in-20 the leaf's sapling, 1-in-50 a stick. They ride these constants so
/// a loot-table wiring (which would replace them) fails loudly here.
const SAPLING_DROP_ONE_IN: i32 = 20;
const STICK_DROP_ONE_IN: i32 = 50;

/// The six face neighbours, for the leaf-support search.
const LEAF_STEPS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

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
            "minecraft:oak_sapling" => self.tick_sapling(x, y, z, id),
            name if name.ends_with("_leaves") => self.tick_leaves(x, y, z, id, name),
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
    pub(crate) fn write_property(
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
    /// The `mob_griefing` gate lives at the callers (P20-05): the jar checks
    /// the rule for non-players only, so the player path calls straight in.
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

    /// Oak sapling growth (`SaplingBlock.randomTick` shape, P20-02 slice 2c).
    ///
    /// Brightness `>= 9` and a 1-in-7 draw, then stage 0 advances to 1 and a
    /// stage-1 sapling attempts the worldgen oak feature at runtime. Only oak
    /// grows: it is the only tree feature this build has (worldgen docs list
    /// the rest as not implemented).
    fn tick_sapling(&mut self, x: i32, y: i32, z: i32, id: i32) -> bool {
        if self.raw_brightness(x, y, z) < GROWTH_LIGHT {
            return false;
        }
        if self.random.next_i32_bounded(SAPLING_TICK_ONE_IN) != 0 {
            return false;
        }
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let Some(stage) = int_property(&properties, "stage") else {
            return false;
        };
        if stage < 1 {
            return self.write_property(x, y, z, "minecraft:oak_sapling", "stage", stage + 1);
        }
        self.grow_oak_tree(x, y, z)
    }

    /// Grow the worldgen oak feature where a stage-1 sapling stands (slice 2c).
    ///
    /// The trunk base is the sapling cell (soil = the cell below). The volume
    /// is pre-checked — trunk cells must be air except the sapling's own, the
    /// canopy must be air or leaves — because `place_oak_at` overwrites
    /// whatever it is given and a runtime growth must not eat a build the way
    /// generation-time placement may reshape terrain. Refusals (straddling,
    /// vertical, occupied) keep the sapling for a later tick.
    ///
    /// Changed cells ride the canonical edit path cell by cell, so the growth
    /// dirties, feeds and broadcasts like player edits. Leaf distances repair
    /// lazily on sampling (`tick_leaves`), so no fixup pass runs here.
    pub(crate) fn grow_oak_tree(&mut self, x: i32, y: i32, z: i32) -> bool {
        use mc_worldgen::features::{
            CANOPY_LAYERS, CANOPY_RADIUS, MAX_TRUNK_HEIGHT, MIN_TRUNK_HEIGHT,
        };
        let span = MAX_TRUNK_HEIGHT - MIN_TRUNK_HEIGHT + 1;
        let trunk_height = MIN_TRUNK_HEIGHT + self.random.next_i32_bounded(span);
        let surface_y = y - 1;
        let trunk_top = surface_y + trunk_height;
        // The trunk column must be free except the sapling's own cell.
        for ty in surface_y + 1..=trunk_top {
            if ty == y {
                continue;
            }
            let free = self
                .world
                .get_block_loaded(x, ty, z)
                .is_some_and(|cell| self.registries.blocks.is_empty(cell));
            if !free {
                return false;
            }
        }
        // The canopy must be air or leaves (never a build).
        for layer in 0..=CANOPY_LAYERS {
            let radius = (CANOPY_RADIUS - layer).max(0);
            let cy = trunk_top + 1 + layer;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if layer == 0 && dx.abs() == radius && dz.abs() == radius {
                        continue;
                    }
                    let free =
                        self.world
                            .get_block_loaded(x + dx, cy, z + dz)
                            .is_some_and(|cell| {
                                self.registries.blocks.is_empty(cell)
                                    || self
                                        .registries
                                        .blocks
                                        .block_name(cell)
                                        .is_ok_and(|name| name.ends_with("_leaves"))
                            });
                    if !free {
                        return false;
                    }
                }
            }
        }
        // Snapshot the affected box, place at chunk level, then feed every
        // cell that changed.
        let mut before = Vec::new();
        for by in surface_y + 1..=trunk_top + 1 + CANOPY_LAYERS {
            for dx in -CANOPY_RADIUS..=CANOPY_RADIUS {
                for dz in -CANOPY_RADIUS..=CANOPY_RADIUS {
                    if let Some(cell) = self.world.get_block_loaded(x + dx, by, z + dz) {
                        before.push(((x + dx, by, z + dz), cell));
                    }
                }
            }
        }
        let Ok(palette) = mc_worldgen::terrain::BlockPalette::resolve(&self.registries.blocks)
        else {
            return false;
        };
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk) = self.world.chunk_mut(chunk_pos) else {
            return false;
        };
        match mc_worldgen::features::place_oak_at(
            chunk,
            &self.registries.blocks,
            &palette,
            x,
            surface_y,
            z,
            trunk_height,
        ) {
            mc_worldgen::features::OakPlacement::Placed { .. } => {}
            _ => return false,
        }
        for ((cx, cy, cz), old) in before {
            let Some(current) = self.world.get_block_loaded(cx, cy, cz) else {
                continue;
            };
            if current != old {
                self.block_feed(cx, cy, cz, current);
            }
        }
        true
    }

    /// Leaf decay with on-demand distance repair (P20-02 slice 2c).
    ///
    /// The jar's `LeavesBlock.randomTick` decays `!persistent && distance ==
    /// 7`; distance itself arrives over neighbour updates in vanilla. Nothing
    /// here propagates distances on write (no `updateShape` analogue), so the
    /// sampled cell measures its own support instead: a bounded BFS through
    /// leaves to the nearest log. A supported cell writes its true distance
    /// and lives; an unreachable one (`distance == 7`) decays with the
    /// documented drop rates. Generated canopies therefore repair rather than
    /// rot — the fixture default is `distance=7`, which a blind decay arm
    /// would evaporate.
    ///
    /// Drops (documented vanilla rates, not jar-read: 1-in-20 the leaf's
    /// sapling, 1-in-50 a stick) arrive as item entities; unmapped leaf kinds
    /// drop sticks only.
    fn tick_leaves(&mut self, x: i32, y: i32, z: i32, id: i32, name: &str) -> bool {
        let properties = self.registries.blocks.properties_of(id).unwrap_or_default();
        let persistent = properties
            .iter()
            .find(|(k, _)| k == "persistent")
            .is_some_and(|(_, v)| v == "true");
        if persistent {
            return false;
        }
        let distance = leaf_support_distance(self, x, y, z);
        let stored = int_property(&properties, "distance").unwrap_or(LEAF_DECAY_DISTANCE);
        if distance != stored {
            return self.write_property(x, y, z, name, "distance", distance);
        }
        if distance != LEAF_DECAY_DISTANCE {
            return false;
        }
        // Unsupported: remove on the canonical path, then roll the drops.
        let air = self.registries.blocks.air_id();
        if self.world.set_block(x, y, z, air).is_err() {
            return false;
        }
        self.block_feed(x, y, z, air);
        if self.random.next_i32_bounded(SAPLING_DROP_ONE_IN) == 0
            && let Some(sapling) = leaf_sapling_item(name)
            && let Ok(item_id) = self.registries.items.id(sapling)
            && let Ok(stack) = mc_entity::stack::ItemStack::new(item_id, 1)
        {
            let at =
                mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
            let _ = self.spawn_item_owned(stack, at, None);
        }
        if self.random.next_i32_bounded(STICK_DROP_ONE_IN) == 0
            && let Ok(item_id) = self.registries.items.id("minecraft:stick")
            && let Ok(stack) = mc_entity::stack::ItemStack::new(item_id, 1)
        {
            let at =
                mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5);
            let _ = self.spawn_item_owned(stack, at, None);
        }
        true
    }

    /// Sweep helper: the loaded chunks inside the ticking radius around loaded    /// entity positions (ADR-0009 §2.4; `simulation_distance` arrives with
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

/// Shortest leaf-distance from `(x, y, z)` to a log, through leaves only.
///
/// `0` would be a log cell itself (never queried — only leaves call this);
/// leaves chain at `parent + 1`, capped at [`LEAF_DECAY_DISTANCE`]. Anything
/// unloaded bounds the search like a solid cell: an unloaded neighbour
/// carries no support the loader has not proven.
fn leaf_support_distance(game: &Game, x: i32, y: i32, z: i32) -> i32 {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let blocks = &game.registries.blocks;
    let cell_kind = |cx: i32, cy: i32, cz: i32| -> Option<bool> {
        // `Some(true)` = log, `Some(false)` = leaves, `None` = anything else.
        let id = game.world.get_block_loaded(cx, cy, cz)?;
        let name = blocks.block_name(id).ok()?;
        if name.ends_with("_log") {
            Some(true)
        } else if name.ends_with("_leaves") {
            Some(false)
        } else {
            None
        }
    };
    // Seed with every log in the cube: multi-source BFS assigns minimums.
    let mut distance = BTreeMap::new();
    let mut queue = VecDeque::new();
    let mut seen = BTreeSet::new();
    for dx in -LEAF_SUPPORT_RADIUS..=LEAF_SUPPORT_RADIUS {
        for dy in -LEAF_SUPPORT_RADIUS..=LEAF_SUPPORT_RADIUS {
            for dz in -LEAF_SUPPORT_RADIUS..=LEAF_SUPPORT_RADIUS {
                let (cx, cy, cz) = (x + dx, y + dy, z + dz);
                if cell_kind(cx, cy, cz) == Some(true) {
                    distance.insert((cx, cy, cz), 0);
                    queue.push_back((cx, cy, cz));
                    seen.insert((cx, cy, cz));
                }
            }
        }
    }
    while let Some((cx, cy, cz)) = queue.pop_front() {
        let here = distance[&(cx, cy, cz)];
        if here >= LEAF_DECAY_DISTANCE {
            continue;
        }
        for (dx, dy, dz) in LEAF_STEPS {
            let next = (cx + dx, cy + dy, cz + dz);
            if !seen.insert(next) {
                continue;
            }
            // Logs seed the search; only leaves propagate through it.
            if cell_kind(next.0, next.1, next.2) == Some(false) {
                distance.insert(next, here + 1);
                queue.push_back(next);
            }
        }
    }
    distance
        .get(&(x, y, z))
        .copied()
        .unwrap_or(LEAF_DECAY_DISTANCE)
}

/// The sapling item a leaf kind drops (`None` when the kind has no mapped
/// item — sticks still drop).
fn leaf_sapling_item(leaves: &str) -> Option<&'static str> {
    match leaves {
        "minecraft:oak_leaves" => Some("minecraft:oak_sapling"),
        "minecraft:dark_oak_leaves" => Some("minecraft:dark_oak_sapling"),
        "minecraft:pale_oak_leaves" => Some("minecraft:pale_oak_sapling"),
        "minecraft:birch_leaves" => Some("minecraft:birch_sapling"),
        "minecraft:spruce_leaves" => Some("minecraft:spruce_sapling"),
        "minecraft:jungle_leaves" => Some("minecraft:jungle_sapling"),
        "minecraft:acacia_leaves" => Some("minecraft:acacia_sapling"),
        "minecraft:cherry_leaves" => Some("minecraft:cherry_sapling"),
        "minecraft:mangrove_leaves" => Some("minecraft:mangrove_propagule"),
        _ => None,
    }
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

    /// Install hand-built growth tags (the test seam; production resolves
    /// the pack tags in `load_packs`): every farmland moisture state grows
    /// crops, every wheat age state holds dry soil.
    fn install_growth_tags(game: &mut Game) {
        use std::collections::BTreeSet;
        let blocks = &game.registries().blocks;
        let mut grows = BTreeSet::new();
        let mut maintains = BTreeSet::new();
        for level in 0..8 {
            grows.insert(
                blocks
                    .state_id(
                        "minecraft:farmland",
                        &[("moisture".to_owned(), level.to_string())],
                    )
                    .expect("farmland moisture state"),
            );
        }
        for age in 0..8 {
            maintains.insert(
                blocks
                    .state_id("minecraft:wheat", &[("age".to_owned(), age.to_string())])
                    .expect("wheat age state"),
            );
        }
        game.set_growth_tags(grows, maintains);
    }

    /// One `crop` seedling at `(x, y+1, z)` on a **3×3 patch** of moist
    /// farmland at `y`.
    ///
    /// The patch is the point, not scenery: `getGrowthSpeed` scores the centre
    /// cell 1.0 + 3.0 + eight quartered neighbours = 10.0, which is the
    /// arithmetic every pin below quotes. A lone soil column scores 4.0 (its
    /// neighbours are air), so planting the crop without the patch silently
    /// changes the rate under test — the first draft did exactly that and its
    /// rate pins failed at 4.0.
    fn plant_crop(game: &mut Game, x: i32, y: i32, z: i32, crop: &str, age: i32) {
        let blocks = game.registries().blocks.clone();
        let soil = blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "7".to_owned())],
            )
            .expect("moist farmland");
        let seedling = blocks
            .state_id(crop, &[("age".to_owned(), age.to_string())])
            .expect("seedling crop");
        for dx in -1..=1 {
            for dz in -1..=1 {
                game.world_mut()
                    .set_block(x + dx, y, z + dz, soil)
                    .expect("soil placed");
            }
        }
        game.world_mut()
            .set_block(x, y + 1, z, seedling)
            .expect("crop placed");
    }

    /// One wheat seedling on a 3×3 moist-farmland patch (see [`plant_crop`]).
    fn plant_wheat(game: &mut Game, x: i32, y: i32, z: i32, age: i32) {
        plant_crop(game, x, y, z, "minecraft:wheat", age);
    }

    fn crop_age(game: &Game, x: i32, y: i32, z: i32) -> i32 {
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

    /// `moisture` of the farmland at `(x, y, z)`.
    fn moisture_of(game: &Game, x: i32, y: i32, z: i32) -> i32 {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        game.registries()
            .blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "moisture")
            .and_then(|(_, v)| v.parse().ok())
            .expect("moisture reads")
    }

    #[test]
    fn wheat_grows_on_direct_calls() {
        let (mut game, _dir) = bare_game("wheat-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        plant_wheat(&mut game, 8, 120, 8, 0);
        light_farm(&mut game);
        // Moist isolated wheat: speed 10.0, bound 3, so ~10 gains in 30
        // calls from the fixed seed; the floor below is less than half.
        // Each call re-reads the id: `random_tick_block` trusts the id it is
        // handed (the sweep always hands a fresh read of the sampled cell), so
        // a cached pre-growth id would keep rewriting the same age step.
        for _ in 0..30 {
            let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
            let _ = game.random_tick_block(8, 121, 8, id);
        }
        let age = crop_age(&game, 8, 121, 8);
        assert!(
            (4..=7).contains(&age),
            "wheat must grow decisively in 30 calls, got age {age}"
        );
    }

    /// A ripe crop applies nothing **and consumes no randomness**: the jar's
    /// `age < maxAge` guard returns before the growth roll.
    ///
    /// The state assertion alone cannot carry this pin. `write_property` refuses
    /// the out-of-band `wheat[age=8]` state, so a crop at max age stays at 7
    /// whether or not the guard is there — measured, not assumed: neutralising
    /// the guard left the first draft of this pin green. The draw counter can
    /// tell the two apart, so the pin compares the seeded source either side of
    /// the calls.
    #[test]
    fn wheat_respects_max_age_on_direct_call() {
        let (mut game, _dir) = bare_game("wheat-max-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        plant_wheat(&mut game, 8, 120, 8, 7);
        light_farm(&mut game);
        let untouched = game.random.clone();
        for _ in 0..30 {
            let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
            assert!(
                !game.random_tick_block(8, 121, 8, id),
                "a ripe crop applies nothing"
            );
        }
        assert_eq!(crop_age(&game, 8, 121, 8), 7, "ripe wheat never exceeds 7");
        assert_eq!(
            game.random, untouched,
            "a ripe crop consumes no draws: the max-age guard returns before the roll"
        );
    }

    /// The `fallOn` threshold: a fall that cannot beat `fall - 0.5` never
    /// tramples.
    ///
    /// Deterministic without a seed: at 0.5 blocks the roll is `< 0.0`, which
    /// `next_f64` (range `[0, 1)`) can never satisfy. This is the arm the
    /// tick-level "standing still" control cannot reach — an idle player never
    /// produces a landing at all — so the threshold is pinned here rather than
    /// implied there.
    #[test]
    fn short_falls_do_not_trample() {
        let (mut game, _dir) = bare_game("trample-threshold-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let soil = game
            .registries()
            .blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "7".to_owned())],
            )
            .expect("moist farmland");
        game.world_mut()
            .set_block(8, 120, 8, soil)
            .expect("soil placed");
        for fall in [0.0, 0.4, 0.5] {
            game.trample_on_landing(8, 120, 8, fall);
            assert_eq!(
                game.registries()
                    .blocks
                    .block_name(game.world().get_block_loaded(8, 120, 8).expect("loaded"))
                    .expect("registered"),
                "minecraft:farmland",
                "a {fall}-block fall is under the threshold (roll < {} never passes)",
                fall - 0.5
            );
        }
    }

    /// Beetroot's `nextInt(3)` pre-gate makes it grow at a third of wheat's
    /// rate under identical soil.
    ///
    /// Rates are compared as **hit counts over a fixed number of rounds**, not
    /// as final ages. A single crop per side saturates: both reach their max age
    /// long before the run ends, so the first draft of this pin (two crops, 60
    /// calls each) stayed green with the pre-gate deleted — measured, then
    /// fixed. The fields below stay far from the cap (per-cell expectations are
    /// 2.0 for wheat and 0.67 for beetroot, against a cap of 7).
    #[test]
    fn beetroot_grows_slower_on_direct_calls() {
        let (mut game, _dir) = bare_game("beet-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        // Two equal fields inside one chunk, cells 2 apart in both axes so no
        // cell has an orthogonal same-crop neighbour (which would halve its
        // growth speed). 15 wheat + 15 beetroot over six rounds: wheat expects
        // ≈30 hits, beetroot ≈10, and no cell comes near the age cap.
        let mut wheat_cells = Vec::new();
        let mut beet_cells = Vec::new();
        for row in 0..5 {
            for col in 0..3 {
                let (x, z) = (1 + col * 2, 1 + row * 2);
                plant_crop(&mut game, x, 120, z, "minecraft:wheat", 0);
                wheat_cells.push((x, 121, z));
                let (x, z) = (9 + col * 2, 1 + row * 2);
                plant_crop(&mut game, x, 120, z, "minecraft:beetroots", 0);
                beet_cells.push((x, 121, z));
            }
        }
        light_farm(&mut game);
        let hits = |cells: &[(i32, i32, i32)], game: &mut Game| -> usize {
            let mut hits = 0;
            for _ in 0..6 {
                for &(x, y, z) in cells {
                    // Fresh id per call, as in the sweep (a cached pre-growth
                    // id would re-write the same age step).
                    let id = game.world().get_block_loaded(x, y, z).expect("loaded");
                    hits += usize::from(game.random_tick_block(x, y, z, id));
                }
            }
            hits
        };
        let wheat_hits = hits(&wheat_cells, &mut game);
        let beet_hits = hits(&beet_cells, &mut game);
        assert!(beet_hits > 0, "beetroot still grows, only slower");
        assert!(
            beet_hits * 2 < wheat_hits,
            "the pre-gate must put beetroot below half of wheat's rate: \
             wheat {wheat_hits} hits vs beetroot {beet_hits} over the same rounds"
        );
    }

    #[test]
    fn dark_wheat_never_grows_on_direct_calls() {
        let (mut game, _dir) = bare_game("dark-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        let stone = game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        // A sealed stone box: settled light inside is 0 by construction.
        for y in 118..=124 {
            for z in 6..=10 {
                for x in 6..=10 {
                    game.world_mut()
                        .set_block(x, y, z, stone)
                        .expect("box sealed");
                }
            }
        }
        plant_wheat(&mut game, 8, 120, 8, 0);
        light_farm(&mut game);
        let id = game.world().get_block_loaded(8, 121, 8).expect("loaded");
        for _ in 0..20 {
            assert!(
                !game.random_tick_block(8, 121, 8, id),
                "darkness gates every call"
            );
        }
        assert_eq!(crop_age(&game, 8, 121, 8), 0, "nothing grows without light");
    }

    /// The dry arm: moisture 1 ticks down to 0, and an uncovered moisture-0
    /// soil turns to dirt on the next call.
    ///
    /// The premises are asserted, not assumed. "A call applied" is not proof of
    /// the dry branch — the wet branch applies too (it writes moisture 7) — so a
    /// plot that happened to stand within water's reach would have read the same
    /// and the next call would find `moisture >= 7` and refuse. The intermediate
    /// state is what separates the two branches.
    #[test]
    fn dry_farmland_dries_then_dirts_on_direct_calls() {
        let (mut game, _dir) = bare_game("dry-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        let dry = game
            .registries()
            .blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "1".to_owned())],
            )
            .expect("dry farmland");
        game.world_mut()
            .set_block(8, 120, 8, dry)
            .expect("soil placed");
        assert!(!game.farmland_is_near_water(8, 120, 8), "no water in reach");
        assert!(!game.is_raining_at(8, 121, 8), "not raining");
        assert!(game.is_empty_block(8, 121, 8), "uncovered: air above");
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        assert!(game.random_tick_block(8, 120, 8, id), "drying applies");
        assert_eq!(
            moisture_of(&game, 8, 120, 8),
            0,
            "one dry tick takes moisture 1 to 0"
        );
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        assert!(game.random_tick_block(8, 120, 8, id), "dirting applies");
        assert_eq!(
            game.registries()
                .blocks
                .block_name(game.world().get_block_loaded(8, 120, 8).expect("loaded"))
                .expect("registered"),
            "minecraft:dirt",
            "dry uncovered soil becomes dirt in two calls"
        );
    }

    #[test]
    fn wet_farmland_wets_on_direct_call() {
        let (mut game, _dir) = bare_game("wet-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        let blocks = game.registries().blocks.clone();
        let dry = blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "0".to_owned())],
            )
            .expect("dry farmland");
        let water = blocks.default_state("minecraft:water").expect("water");
        game.world_mut()
            .set_block(8, 120, 8, dry)
            .expect("soil placed");
        game.world_mut()
            .set_block(9, 120, 8, water)
            .expect("water placed");
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        assert!(game.random_tick_block(8, 120, 8, id), "wetting applies");
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        let moisture = blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "moisture")
            .and_then(|(_, v)| v.parse::<i32>().ok())
            .expect("moisture reads");
        assert_eq!(moisture, 7, "water wets straight to 7");
    }

    /// Farmland wetting is not light-gated: soil beside water in a sealed dark
    /// box still wets. This is the moisture half of the deleted
    /// `crops_do_not_grow_in_the_dark` tick test ("dark soil is still watered,
    /// not dried"), kept because the split would otherwise drop it.
    #[test]
    fn dark_farmland_still_wets_on_direct_call() {
        let (mut game, _dir) = bare_game("dark-wet-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        let blocks = game.registries().blocks.clone();
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        let dry = blocks
            .state_id(
                "minecraft:farmland",
                &[("moisture".to_owned(), "3".to_owned())],
            )
            .expect("dry farmland");
        let water = blocks.default_state("minecraft:water").expect("water");
        // A sealed stone box: light inside is 0 by construction.
        for y in 118..=124 {
            for z in 6..=10 {
                for x in 6..=10 {
                    game.world_mut()
                        .set_block(x, y, z, stone)
                        .expect("box sealed");
                }
            }
        }
        game.world_mut()
            .set_block(8, 120, 8, dry)
            .expect("soil placed");
        game.world_mut()
            .set_block(9, 120, 8, water)
            .expect("water placed");
        light_farm(&mut game);
        assert_eq!(
            game.raw_brightness(8, 120, 8),
            0,
            "the box really is dark, so the wetting below cannot be light-driven"
        );
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 120, 8, id),
            "wetting applies in the dark"
        );
        let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
        let moisture = blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "moisture")
            .and_then(|(_, v)| v.parse::<i32>().ok())
            .expect("moisture reads");
        assert_eq!(moisture, 7, "dark soil beside water is watered, not dried");
    }

    /// The jar's `getGrowthSpeed` arithmetic, on the shapes the rate pins use.
    ///
    /// The comparison is epsilon-based, not `==`: the workspace denies
    /// `clippy::float_cmp`, and the values are sums of quarters, not literals.
    #[test]
    fn growth_speed_matches_the_jar_arithmetic() {
        let (mut game, _dir) = bare_game("speed-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        install_growth_tags(&mut game);
        // Isolated moist wheat: 1.0 + centre 3.0 + eight neighbours × 0.75.
        plant_wheat(&mut game, 8, 120, 8, 0);
        let speed = game.crop_growth_speed(8, 121, 8);
        assert!(
            (speed - 10.0).abs() < f32::EPSILON,
            "isolated moist wheat scores 10.0, got {speed}"
        );
        // A 2×2 block halves it (both row axes share the crop).
        plant_wheat(&mut game, 9, 120, 8, 0);
        plant_wheat(&mut game, 8, 120, 9, 0);
        plant_wheat(&mut game, 9, 120, 9, 0);
        let speed = game.crop_growth_speed(8, 121, 8);
        assert!(
            (speed - 5.0).abs() < f32::EPSILON,
            "a 2×2 wheat block halves the speed, got {speed}"
        );
    }

    // Slice-2c pins, same direct-call shape (TEST-TIME-PLAN §2).
    fn staged(game: &Game, name: &str, stage: i32) -> i32 {
        game.registries()
            .blocks
            .state_id(name, &[("stage".to_owned(), stage.to_string())])
            .expect("staged sapling state")
    }

    fn stage_of(game: &Game, x: i32, y: i32, z: i32) -> i32 {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        game.registries()
            .blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "stage")
            .and_then(|(_, v)| v.parse().ok())
            .expect("stage reads")
    }

    fn name_of(game: &Game, x: i32, y: i32, z: i32) -> String {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        game.registries()
            .blocks
            .block_name(id)
            .expect("registered")
            .to_owned()
    }

    /// Dirt at `(x, 119, z)`, open sky above: the sapling pad.
    fn sapling_pad(game: &mut Game, x: i32, z: i32) {
        let dirt = game
            .registries()
            .blocks
            .default_state("minecraft:dirt")
            .expect("dirt");
        game.world_mut()
            .set_block(x, 119, z, dirt)
            .expect("pad placed");
    }

    fn leaf_state(game: &Game, distance: i32, persistent: bool) -> i32 {
        game.registries()
            .blocks
            .state_id(
                "minecraft:oak_leaves",
                &[
                    ("distance".to_owned(), distance.to_string()),
                    (
                        "persistent".to_owned(),
                        if persistent { "true" } else { "false" }.to_owned(),
                    ),
                    ("waterlogged".to_owned(), "false".to_owned()),
                ],
            )
            .expect("leaf state")
    }

    fn distance_of(game: &Game, x: i32, y: i32, z: i32) -> i32 {
        let id = game.world().get_block_loaded(x, y, z).expect("loaded");
        game.registries()
            .blocks
            .properties_of(id)
            .expect("properties")
            .iter()
            .find(|(k, _)| k == "distance")
            .and_then(|(_, v)| v.parse().ok())
            .expect("distance reads")
    }

    #[test]
    fn sapling_advances_stage_on_direct_calls() {
        let (mut game, _dir) = bare_game("sapling-stage-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        sapling_pad(&mut game, 8, 8);
        let seedling = staged(&game, "minecraft:oak_sapling", 0);
        game.world_mut()
            .set_block(8, 120, 8, seedling)
            .expect("sapling placed");
        light_farm(&mut game);
        assert!(
            game.raw_brightness(8, 120, 8) >= 9,
            "open sky really is bright"
        );
        // The 1-in-7 gate refuses most calls; a hundred always lands one
        // ((6/7)^100 is ~1e-7, and the seed fixes the draws anyway).
        for _ in 0..100 {
            let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
            if game.random_tick_block(8, 120, 8, id) {
                break;
            }
        }
        assert_eq!(
            stage_of(&game, 8, 120, 8),
            1,
            "a stage-0 sapling advances (or the dispatch never reached it)"
        );
    }

    #[test]
    fn sapling_grows_a_tree_on_direct_calls() {
        let (mut game, _dir) = bare_game("sapling-tree-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        sapling_pad(&mut game, 8, 8);
        let grown = staged(&game, "minecraft:oak_sapling", 1);
        game.world_mut()
            .set_block(8, 120, 8, grown)
            .expect("sapling placed");
        light_farm(&mut game);
        for _ in 0..200 {
            let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
            if name_of(&game, 8, 120, 8) != "minecraft:oak_sapling" {
                break;
            }
            game.random_tick_block(8, 120, 8, id);
        }
        assert_eq!(
            name_of(&game, 8, 120, 8),
            "minecraft:oak_log",
            "the trunk base replaces the sapling"
        );
        assert_eq!(
            name_of(&game, 8, 121, 8),
            "minecraft:oak_log",
            "the trunk rises"
        );
        // The canopy sits above the top log: some leaves within two of the
        // trunk top, none of them the trunk itself.
        let mut leaves = 0;
        for y in 121..=132 {
            for dx in -2..=2 {
                for dz in -2..=2 {
                    if name_of(&game, 8 + dx, y, 8 + dz) == "minecraft:oak_leaves" {
                        leaves += 1;
                    }
                }
            }
        }
        assert!(leaves >= 10, "a canopy grew, saw {leaves} leaves");
    }

    #[test]
    fn sapling_refuses_without_headroom() {
        let (mut game, _dir) = bare_game("sapling-roof-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let stone = blocks.default_state("minecraft:stone").expect("stone");
        sapling_pad(&mut game, 8, 8);
        let grown = staged(&game, "minecraft:oak_sapling", 1);
        game.world_mut()
            .set_block(8, 120, 8, grown)
            .expect("sapling placed");
        game.world_mut()
            .set_block(8, 123, 8, stone)
            .expect("ceiling placed");
        light_farm(&mut game);
        for _ in 0..100 {
            let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
            game.random_tick_block(8, 120, 8, id);
        }
        assert_eq!(
            name_of(&game, 8, 120, 8),
            "minecraft:oak_sapling",
            "an occupied canopy keeps its sapling (neutralise the space check and a trunk lands)"
        );
    }

    #[test]
    fn other_saplings_stay_saplings() {
        let (mut game, _dir) = bare_game("sapling-other-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        sapling_pad(&mut game, 8, 8);
        let exotic = staged(&game, "minecraft:dark_oak_sapling", 1);
        game.world_mut()
            .set_block(8, 120, 8, exotic)
            .expect("sapling placed");
        light_farm(&mut game);
        for _ in 0..20 {
            let id = game.world().get_block_loaded(8, 120, 8).expect("loaded");
            assert!(
                !game.random_tick_block(8, 120, 8, id),
                "no handler answers a non-oak sapling"
            );
        }
        assert_eq!(
            name_of(&game, 8, 120, 8),
            "minecraft:dark_oak_sapling",
            "only oak grows until more features exist"
        );
    }

    #[test]
    fn supported_leaves_repair_on_direct_call() {
        let (mut game, _dir) = bare_game("leaf-repair-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let blocks = game.registries().blocks.clone();
        let log = blocks.default_state("minecraft:oak_log").expect("log");
        game.world_mut()
            .set_block(8, 120, 8, log)
            .expect("log placed");
        // A leaf column with stale all-7 distances (the fixture default).
        let stale = leaf_state(&game, 7, false);
        for y in 121..=125 {
            game.world_mut()
                .set_block(8, y, 8, stale)
                .expect("leaf placed");
        }
        let id = game.world().get_block_loaded(8, 124, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 124, 8, id),
            "a stale distance rewrites"
        );
        assert_eq!(
            distance_of(&game, 8, 124, 8),
            4,
            "four up from the log reads four, not rot"
        );
        assert_eq!(
            name_of(&game, 8, 124, 8),
            "minecraft:oak_leaves",
            "supported leaves live (a blind decay arm would take the canopy)"
        );
    }

    #[test]
    fn unsupported_leaves_decay_on_direct_call() {
        let (mut game, _dir) = bare_game("leaf-decay-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        // A cleared cube: no log within the support radius, so the verdict
        // cannot be borrowed from generated terrain.
        let air = game.registries().blocks.air_id();
        for y in 130..=144 {
            for z in 1..=15 {
                for x in 1..=15 {
                    game.world_mut().set_block(x, y, z, air).expect("cleared");
                }
            }
        }
        let lone = leaf_state(&game, 7, false);
        game.world_mut()
            .set_block(8, 137, 8, lone)
            .expect("leaf placed");
        let id = game.world().get_block_loaded(8, 137, 8).expect("loaded");
        assert!(
            game.random_tick_block(8, 137, 8, id),
            "an unreachable tick applies"
        );
        assert_eq!(
            name_of(&game, 8, 137, 8),
            "minecraft:air",
            "unsupported leaves decay (neutralise the repair and nothing rots)"
        );
    }

    #[test]
    fn persistent_leaves_never_decay() {
        let (mut game, _dir) = bare_game("leaf-keep-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let air = game.registries().blocks.air_id();
        for y in 130..=144 {
            for z in 1..=15 {
                for x in 1..=15 {
                    game.world_mut().set_block(x, y, z, air).expect("cleared");
                }
            }
        }
        let kept = leaf_state(&game, 7, true);
        game.world_mut()
            .set_block(8, 137, 8, kept)
            .expect("leaf placed");
        let id = game.world().get_block_loaded(8, 137, 8).expect("loaded");
        assert!(
            !game.random_tick_block(8, 137, 8, id),
            "player-placed leaves are refused by the decay arm"
        );
        assert_eq!(
            name_of(&game, 8, 137, 8),
            "minecraft:oak_leaves",
            "persistent leaves stay"
        );
    }

    #[test]
    fn decay_drops_saplings_over_many_trials() {
        let (mut game, _dir) = bare_game("leaf-drops-unit");
        assert!(game.load_chunk(ChunkPos::new(0, 0)), "field loads");
        let air = game.registries().blocks.air_id();
        for y in 130..=144 {
            for z in 1..=15 {
                for x in 1..=15 {
                    game.world_mut().set_block(x, y, z, air).expect("cleared");
                }
            }
        }
        // 200 decays at 1-in-20: a dry run is ~4e-5 by the binomial, and the
        // seed fixes every draw, so this is a pin rather than a gamble.
        let lone = leaf_state(&game, 7, false);
        for _ in 0..200 {
            game.world_mut()
                .set_block(8, 137, 8, lone)
                .expect("leaf placed");
            let id = game.world().get_block_loaded(8, 137, 8).expect("loaded");
            assert!(
                game.random_tick_block(8, 137, 8, id),
                "every unreachable tick applies"
            );
        }
        let drops = game
            .entities
            .iter()
            .filter(|entity| matches!(entity.body, mc_entity::EntityBody::Item(_)))
            .count();
        assert!(
            drops > 0,
            "two hundred decays drop something (neutralise the drop rolls and the store stays empty)"
        );
    }

    #[test]
    fn leaf_kind_maps_to_its_sapling() {
        assert_eq!(
            super::leaf_sapling_item("minecraft:oak_leaves"),
            Some("minecraft:oak_sapling")
        );
        assert_eq!(
            super::leaf_sapling_item("minecraft:cherry_leaves"),
            Some("minecraft:cherry_sapling")
        );
        assert_eq!(super::leaf_sapling_item("minecraft:stone"), None);
    }
}
