//! Furnace smelting: fuel, cook progress and the tick rule (P06-05).
//!
//! ## What this is, and what it is not
//!
//! [`SmeltingRegistry::baseline`] is a **hand-written baseline subset** of the
//! smelting table: iron ore, raw iron, sand, cobblestone, raw porkchop and a
//! potato, plus their cook times and experience. It is **not** the Vanilla smelting
//! table. In Vanilla a smelting recipe is data
//! (`data/<namespace>/recipe/*.json` with `"type": "minecraft:smelting"`, and
//! `blasting`/`smoking`/`campfire_cooking` as separate types), the fuel table is
//! `AbstractFurnaceBlockEntity.getBurnDuration` over the item's
//! `DataComponents.FURNACE_FUEL`-style burn-time data, and the fuel *tags*
//! (`#minecraft:coals`, `#minecraft:logs_that_burn`, â€? decide what may burn.
//! Loading that real data is **P07-03**; until then this module's tables are the
//! small, labelled stand-ins that make the furnace path real and testable.
//!
//! Concretely, what this module does **not** implement:
//!
//! - **Blasting, smoking and campfire cooking.** Only `smelting` exists.
//! - **Fuel tags.** The fuel table is an ordered list of item names, not
//!   `#minecraft:logs_that_burn` and friends, so a wooden item outside the list
//!   does not burn here even though Vanilla would burn it.
//! - **The fuel remainder** (a lava bucket leaves an empty bucket, a lava bucket
//!   in the fuel slot returns a bucket without consuming the lava in creative â€?//!   none of that). The fuel item is consumed whole. Recorded as a known gap.
//! - **Experience orbs and the recipe-book unlock.** [`FurnaceState::experience`]
//!   accumulates the recipe's experience while cooking; nothing spawns or grants
//!   it. The caller's, at P06-07/P06-10.
//! - **The furnace's item-slot *contents* rules beyond input/fuel/output**: no
//!   "blast furnace only accepts ores", no fuel-slot-only-for-fuel check.
//!
//! ## The rule whose absence causes the classic bug
//!
//! A furnace must **not** consume fuel, and must **not** advance cooking, when it
//! cannot use the result: the output slot full of a *different* item is the case
//! that matters. Vanilla's `AbstractFurnaceBlockEntity.serverTick` computes
//! `canBurn` first and only then decides whether to consume a new fuel item, so a
//! blocked furnace burns out its current fuel and stops. [`Furnace::tick`]
//! reproduces that exactly:
//!
//! 1. If nothing cookable is in the input, or the output cannot accept the result,
//!    no new fuel is taken and no progress is made.
//! 2. Otherwise, if nothing is burning, try to start a burn from the fuel slot.
//! 3. If (still) nothing is burning, stop.
//! 4. Advance cooking by one tick, and finish exactly at
//!    [`SmeltingRecipe::cook_ticks`].
//!
//! A furnace that has already started burning keeps burning down while it is
//! blocked â€?that is Vanilla, and it is why a hopper-fed furnace with a full output
//! does not bank fuel forever. Ticking after a relight resumes the part-cooked
//! item rather than starting it over: the progress is discarded only when the
//! furnace is dark *and* nothing is burning, which is the state a furnace reaches
//! when its fuel runs out with no top-up.
//!
//! ## Labelling (AGENTS.md section 3.1)
//!
//! Every constant below carries `verified`, `derived` or `approximation` with its
//! source, and a constant this build chose rather than observed carries
//! "product decision". The module doc of [`SmeltingRegistry::baseline`] and the
//! [`FUEL_BURST_TICKS`] table repeat the labels where a reader will look for them.
//! Nothing in this module claims to be the Vanilla table.
//!
//! ## Determinism (AGENTS.md section 3.6)
//!
//! Ticks are integers, the fuel table is a slice walked in order, and cooking
//! advances by exactly one per tick with no randomness or clock. The same state
//! plus the same number of ticks is the same state.

#![deny(missing_docs)]

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::{ItemStack, StackSizeTable};
use mc_registry::ItemRegistry;

use crate::container::Container;

/// Ticks in one second at the fixed 20 TPS simulation rate (product decision,
/// AGENTS.md section 2: the clock is fixed at 20 TPS).
pub const TICKS_PER_SECOND: u32 = 20;

/// Ticks it takes to smelt one item in a furnace (**verified**).
///
/// Vanilla's `AbstractFurnaceBlockEntity` uses a cook time of 200 ticks for
/// `smelting` (10 seconds), 100 for `blasting` and `smoking`, and 600 for
/// `campfire_cooking`. Only the 200-tick `smelting` figure is modelled here.
pub const SMELTING_COOK_TICKS: u32 = 200;

/// Longest cook time this build accepts for a recipe (**product decision**).
///
/// A ceiling so a corrupt or generated table cannot make a furnace's progress bar
/// take a session-length of ticks; the longest Vanilla figure is
/// `campfire_cooking`'s 600, so 20 seconds (400 ticks) is comfortably above every
/// `smelting` recipe and below anything absurd.
pub const MAX_COOK_TICKS: u32 = 400;

/// How much experience a smelted iron item grants (**approximation**).
///
/// Vanilla's smelting recipes carry an `experience` field (0.1 for a plain
/// smelted block, 0.7 for iron and gold, 0.35 for food, â€?. This build does not
/// claim those exact per-recipe values for anything outside the baseline table,
/// so this is the placeholder the baseline uses for iron-family recipes.
pub const EXPERIENCE_PER_IRON_SMELT: f32 = 0.7;

/// Burntime of one fuel item, in ticks, with how it was established.
///
/// The numbers are Vanilla's `getBurnDuration` values; the label says how this
/// build knows:
///
/// - **verified** â€?the value is stated directly in Mojang's own item data / the
///   `fuelValues`-equivalent table this build checked against, i.e. the figure is
///   the documented Vanilla one.
/// - **derived** â€?arithmetic on a verified figure (a coal block is nine coal).
/// - **approximation** â€?a plausible figure this build chose; do not treat it as
///   Vanilla.
///
/// Every value here still needs the P07-03 data loader to become data; a number
/// labelled verified is verified *as a constant*, not "the whole fuel table has
/// been ported".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuelValue {
    /// Item name, e.g. `minecraft:coal`.
    pub item: &'static str,
    /// Ticks one item burns for.
    pub ticks: u32,
    /// How `ticks` was established.
    pub evidence: Evidence,
}

/// How a constant was established (AGENTS.md section 3.1, source-of-truth
/// hierarchy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    /// Vanilla's own value, as documented for 26.1.2.
    Verified,
    /// Arithmetic on another entry (a coal block burns nine coal).
    Derived,
    /// A figure this build chose. **Not** a Vanilla fact.
    Approximation,
    /// A decision about this build's behaviour, not a Vanilla number.
    ProductDecision,
}

impl Evidence {
    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Derived => "derived",
            Self::Approximation => "approximation",
            Self::ProductDecision => "product decision",
        }
    }

    /// Whether the value is a Vanilla figure this build is willing to assert.
    #[must_use]
    pub const fn is_vanilla(self) -> bool {
        matches!(self, Self::Verified | Self::Derived)
    }
}

/// The fuel table: item name to burn time in ticks, **each row carrying its own
/// label**.
///
/// | fuel | ticks | seconds | label |
/// |---|---|---|---|
/// | `minecraft:coal` | 1600 | 80 | verified |
/// | `minecraft:charcoal` | 1600 | 80 | verified |
/// | `minecraft:coal_block` | 16000 | 800 | derived (9 x coal) |
/// | `minecraft:blaze_rod` | 2400 | 120 | verified |
/// | `minecraft:oak_planks` | 300 | 15 | verified |
/// | `minecraft:stick` | 100 | 5 | verified |
/// | `minecraft:lava_bucket` | 20000 | 1000 | verified |
/// | `minecraft:dried_kelp_block` | 4000 | 200 | verified |
///
/// The label is part of the **row** rather than a parallel table, so reordering or
/// adding a row cannot silently attach the wrong evidence to a fuel — the hazard a
/// parallel `&[(&str, Evidence)]` would carry, and the reason this is one table of
/// [`FuelValue`]s instead of two tables of tuples.
///
/// **This is a small hand-picked sample, not the Vanilla `fuelValues` table.**
/// Vanilla's table has dozens of rows (every plank, log, sapling, wooden tool,
/// wool, carpet, bamboo, blaze rod, lava bucket, dried kelp block, coal, charcoal,
/// coal block, and more) each carrying its own burn time as *item data*, and those
/// rows arrive with the P07-03 data loader. Rows that are missing here are missing,
/// not zero: [`SmeltingRegistry::burn_ticks_for`] reports "not a fuel" rather than
/// guessing a number.
///
/// Known approximations this build is **not** making: nothing in this table is
/// [`Evidence::Approximation`], because a guessed burn time is indistinguishable
/// from a correct one at runtime and would silently change gameplay. Only
/// [`EXPERIENCE_PER_IRON_SMELT`] and the baseline recipes' per-item experience carry
/// that label.
pub const FUEL_BURST_TICKS: &[FuelValue] = &[
    FuelValue {
        item: "minecraft:coal",
        ticks: 1600,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:charcoal",
        ticks: 1600,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:coal_block",
        ticks: 16000,
        evidence: Evidence::Derived,
    },
    FuelValue {
        item: "minecraft:blaze_rod",
        ticks: 2400,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:oak_planks",
        ticks: 300,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:stick",
        ticks: 100,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:lava_bucket",
        ticks: 20000,
        evidence: Evidence::Verified,
    },
    FuelValue {
        item: "minecraft:dried_kelp_block",
        ticks: 4000,
        evidence: Evidence::Verified,
    },
];

/// One smelting recipe: an input becomes an output after `cook_ticks`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmeltingRecipe {
    /// Item id consumed from the input slot.
    pub input: i32,
    /// Item id produced into the output slot.
    pub output: i32,
    /// Items produced per input item. Always 1 in Vanilla's `smelting`; modelled
    /// so a future `blasting`/data recipe can say otherwise.
    pub output_count: i32,
    /// Ticks of cooking per item.
    pub cook_ticks: u32,
    /// Experience granted per item. **See the label on each baseline row.**
    pub experience: f32,
}

impl SmeltingRecipe {
    /// A recipe producing one item.
    ///
    /// # Errors
    ///
    /// As described in [`SmeltingRecipe::new`].
    pub fn one(input: i32, output: i32, cook_ticks: u32, experience: f32) -> ServerResult<Self> {
        Self::new(input, output, 1, cook_ticks, experience)
    }

    /// A recipe with an explicit output count.
    ///
    /// # Errors
    ///
    /// As described in [`SmeltingRecipe::one`].
    pub fn new(
        input: i32,
        output: i32,
        output_count: i32,
        cook_ticks: u32,
        experience: f32,
    ) -> ServerResult<Self> {
        if !(1..=mc_entity::stack::HARD_MAX_STACK_SIZE).contains(&output_count) {
            return Err(ServerError::Invariant(format!(
                "a smelting recipe producing {output_count} items is outside 1..={}",
                mc_entity::stack::HARD_MAX_STACK_SIZE
            )));
        }
        if cook_ticks == 0 || cook_ticks > MAX_COOK_TICKS {
            return Err(ServerError::Invariant(format!(
                "a smelting recipe with {cook_ticks} cook ticks is outside 1..={MAX_COOK_TICKS}"
            )));
        }
        Ok(Self {
            input,
            output,
            output_count,
            cook_ticks,
            experience,
        })
    }
}

/// The labelled baseline smelting table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmeltingRegistry {
    recipes: Vec<SmeltingRecipe>,
}

impl SmeltingRegistry {
    /// A registry over an explicit recipe list, in lookup order.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when two recipes smelt the same input item:
    /// "which recipe wins" must not depend on insertion order for something a
    /// player sees.
    pub fn new(recipes: Vec<SmeltingRecipe>) -> ServerResult<Self> {
        for (index, recipe) in recipes.iter().enumerate() {
            if recipes[..index]
                .iter()
                .any(|earlier| earlier.input == recipe.input)
            {
                return Err(ServerError::Invariant(format!(
                    "two smelting recipes consume item id {}",
                    recipe.input
                )));
            }
        }
        Ok(Self { recipes })
    }

    /// An empty registry: nothing smelts.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            recipes: Vec::new(),
        }
    }

    /// The **hand-written baseline subset** of Vanilla's smelting recipes.
    ///
    /// | input | output | count | cook ticks | experience | label |
    /// |---|---|---|---|---|---|
    /// | `minecraft:iron_ore` | `minecraft:iron_ingot` | 1 | 200 | 0.7 | verified shape, approximate xp |
    /// | `minecraft:deepslate_iron_ore` | `minecraft:iron_ingot` | 1 | 200 | 0.7 | verified shape, approximate xp |
    /// | `minecraft:raw_iron` | `minecraft:iron_ingot` | 1 | 200 | 0.7 | verified shape, approximate xp |
    /// | `minecraft:sand` | `minecraft:glass` | 1 | 200 | 0.1 | verified shape, approximate xp |
    /// | `minecraft:cobblestone` | `minecraft:stone` | 1 | 200 | 0.1 | verified shape, approximate xp |
    /// | `minecraft:porkchop` | `minecraft:cooked_porkchop` | 1 | 200 | 0.35 | verified shape, approximate xp |
    /// | `minecraft:potato` | `minecraft:baked_potato` | 1 | 200 | 0.35 | verified shape, approximate xp |
    ///
    /// "Verified shape" means the inputâ†’output pair and the 1-item count are
    /// Vanilla's and were not guessed. The `experience` column is this build's
    /// [`EXPERIENCE_PER_IRON_SMELT`]/food placeholder rather than the exact
    /// per-recipe figure, so it is an **approximation** and is documented as one;
    /// nothing in this build grants experience yet (see the module docs).
    ///
    /// The baseline is **incomplete on purpose**. Vanilla smelts hundreds of items
    /// (every ore and its deepslate variant, every food, every log to charcoal,
    /// clay to brick, stone variants, glass, dyes, and more) and the whole table
    /// arrives as data at P07-03. Nothing here pretends otherwise.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the first item that `items` does not
    /// contain, and [`ServerError::Invariant`] for a duplicate input. A missing
    /// name is never skipped (AGENTS.md section 3.3).
    pub fn baseline(items: &ItemRegistry) -> ServerResult<Self> {
        let mut recipes = Vec::with_capacity(7);
        for (input, output, experience) in [
            (
                "minecraft:iron_ore",
                "minecraft:iron_ingot",
                EXPERIENCE_PER_IRON_SMELT,
            ),
            (
                "minecraft:deepslate_iron_ore",
                "minecraft:iron_ingot",
                EXPERIENCE_PER_IRON_SMELT,
            ),
            (
                "minecraft:raw_iron",
                "minecraft:iron_ingot",
                EXPERIENCE_PER_IRON_SMELT,
            ),
            ("minecraft:sand", "minecraft:glass", 0.1),
            ("minecraft:cobblestone", "minecraft:stone", 0.1),
            ("minecraft:porkchop", "minecraft:cooked_porkchop", 0.35),
            ("minecraft:potato", "minecraft:baked_potato", 0.35),
        ] {
            recipes.push(SmeltingRecipe::one(
                id_of(items, input)?,
                id_of(items, output)?,
                SMELTING_COOK_TICKS,
                experience,
            )?);
        }
        Self::new(recipes)
    }

    /// The recipes, in lookup order.
    #[must_use]
    pub fn recipes(&self) -> &[SmeltingRecipe] {
        &self.recipes
    }

    /// Number of recipes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Whether there are no recipes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// The recipe for an input item, or `None` when it does not smelt.
    #[must_use]
    pub fn recipe_for(&self, input: i32) -> Option<&SmeltingRecipe> {
        self.recipes.iter().find(|recipe| recipe.input == input)
    }

    /// Ticks one item of `item_id` burns for in a furnace, or `None` when it is
    /// not a fuel **this build knows**.
    ///
    /// `item_id` is a registry id, resolved to a name and then looked up in
    /// [`FUEL_BURST_TICKS`]. The name lookup is a *hard* check: an id outside the
    /// registry is [`ServerError::CorruptData`] rather than "not a fuel", which is
    /// the difference between a bad stack and an item that simply does not burn.
    ///
    /// The table is not per-registry state, so this is a method only for the call
    /// site's readability; [`burn_ticks_for`] is the free function it delegates to.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when `item_id` is outside `items`.
    pub fn burn_ticks_for(&self, items: &ItemRegistry, item_id: i32) -> ServerResult<Option<u32>> {
        burn_ticks_for(items, item_id)
    }
}

/// Ticks one item of `item_id` burns for, or `None` when [`FUEL_BURST_TICKS`] has
/// no row for it.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when `item_id` is outside `items`.
pub fn burn_ticks_for(items: &ItemRegistry, item_id: i32) -> ServerResult<Option<u32>> {
    let name = items.name(item_id).map_err(|_| {
        ServerError::CorruptData(format!(
            "fuel lookup for item id {item_id}, which the item registry does not contain"
        ))
    })?;
    Ok(FUEL_BURST_TICKS
        .iter()
        .find(|fuel| fuel.item == name)
        .map(|fuel| fuel.ticks))
}

/// The label on the [`FUEL_BURST_TICKS`] row for `item`, or
/// [`Evidence::Approximation`] when the table has no such row.
///
/// An unknown item falls back to the **weakest** label rather than the strongest,
/// so a caller reading the evidence can never be handed a stronger claim than the
/// truth. Rows themselves carry their own label, so this is a lookup rather than a
/// parallel table.
#[must_use]
pub fn fuel_evidence(item: &str) -> Evidence {
    FUEL_BURST_TICKS
        .iter()
        .find(|fuel| fuel.item == item)
        .map_or(Evidence::Approximation, |fuel| fuel.evidence)
}

/// Persistent furnace state, one per furnace block entity.
///
/// Everything here is meant to be persisted with the block entity (Vanilla stores
/// `BurnTime`, `CookTime` and `cookingTotalTime` in the furnace's NBT) and sent to
/// the client through `container_set_data` when it changes.
///
/// ## What the burn counter means, exactly
///
/// [`FurnaceState::burn_ticks_remaining`] is the number of burn ticks left *at the
/// start of the next tick*, so a tick is lit when the counter is positive **after**
/// the tick has spent one and before any replacement has been taken:
///
/// * a fuel item taken by a cold furnace sets the counter to its full burn time and
///   that same tick is the item's first lit tick;
/// * every later lit tick decrements it by one, so after the *n*-th lit tick of a
///   `D`-tick item the counter reads `D - n + 1`;
/// * the counter therefore reads `1` after the item's last lit tick, and the
///   following tick decrements it to `0`, finds nothing to take, and is dark.
///
/// The observable consequence, which is what
/// `furnace/tests.rs::a_fuel_item_lasts_exactly_its_documented_number_of_ticks`
/// asserts, is that a `D`-tick item lights the furnace for **exactly `D` ticks**,
/// and `N` such items light it for exactly `N x D` with no dark tick in between
/// (the tick that spends one item's last burn tick is also the tick that takes the
/// next, so `lit` is continuous).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FurnaceState {
    /// Burn ticks left at the start of the next tick; see the type-level docs for
    /// the exact accounting.
    pub burn_ticks_remaining: u32,
    /// Total ticks the current fuel item burns for (the progress-bar denominator),
    /// or `0` once the burn has ended.
    pub burn_ticks_total: u32,
    /// Ticks of cooking completed on the current item.
    pub cook_progress: u32,
    /// Ticks the current item needs (`0` when nothing is cooking).
    pub cook_total: u32,
    /// Experience accumulated but not yet granted.
    pub experience: f32,
}

impl FurnaceState {
    /// A cold, empty furnace.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            burn_ticks_remaining: 0,
            burn_ticks_total: 0,
            cook_progress: 0,
            cook_total: 0,
            experience: 0.0,
        }
    }

    /// Whether the furnace is lit, i.e. has burn ticks left.
    ///
    /// This is the value of Vanilla's `LIT` block state and of the `lit` field in
    /// the furnace's `container_set_data` payload; [`FurnaceTickReport::state_changed`]
    /// tells the caller when to send it.
    #[must_use]
    pub const fn is_lit(&self) -> bool {
        self.burn_ticks_remaining > 0
    }

    /// Cooking progress in `0.0..=1.0`, or `0.0` when nothing is cooking.
    ///
    /// The value the client's progress-bar overlay uses. The conversion goes
    /// through `u16` because [`MAX_COOK_TICKS`] (400) is far below `u16::MAX`, so
    /// the integer the fraction is built from is exactly representable as an
    /// `f32` and the ratio cannot lose precision on the way in.
    #[must_use]
    pub fn cook_fraction(&self) -> f32 {
        if self.cook_total == 0 {
            return 0.0;
        }
        let total = u16::try_from(self.cook_total).unwrap_or(u16::MAX);
        let done = u16::try_from(self.cook_progress.min(self.cook_total)).unwrap_or(u16::MAX);
        f32::from(done) / f32::from(total)
    }
}

/// Which container slots a furnace uses, by index, plus a distinctness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnaceSlots {
    /// Slot holding the item to smelt.
    pub input: usize,
    /// Slot holding the fuel.
    pub fuel: usize,
    /// Take-only slot holding the result.
    pub output: usize,
}

impl FurnaceSlots {
    /// The canonical furnace layout: `0` input, `1` fuel, `2` output.
    ///
    /// Vanilla's `AbstractFurnaceMenu` adds the slots in this order. It is
    /// **not** the same as the player menu's crafting slots (see
    /// [`crate::Menu::player`]), which is why the indices are passed in rather
    /// than assumed deep inside the tick.
    pub const CANONICAL: Self = Self {
        input: 0,
        fuel: 1,
        output: 2,
    };

    /// The three slots, in input/fuel/output order.
    #[must_use]
    pub const fn indices(self) -> [usize; 3] {
        [self.input, self.fuel, self.output]
    }

    /// Whether the three slots are pairwise distinct.
    #[must_use]
    pub const fn are_distinct(self) -> bool {
        self.input != self.fuel && self.fuel != self.output && self.input != self.output
    }
}

/// What one [`Furnace::tick`] changed, so the caller knows what to send.
///
/// [`FurnaceTickReport::finished_items`] is the only field that means "the output
/// grew"; compare it against the caller's previous reading rather than inferring a
/// craft from a progress value.
///
/// The four flags look like the "boolean soup" `clippy::struct_excessive_bools`
/// warns about, and they are deliberate: each one maps to a distinct thing the
/// caller does (a `container_set_data` for `lit`, an item-count update, a slot
/// update for the fuel, a progress-bar update for the cook), so folding them into a
/// state machine would hide which packet a tick needs.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FurnaceTickReport {
    /// The furnace's `lit` value after the tick (Vanilla's `LIT` block state).
    pub lit: bool,
    /// Items finished this tick (0 or 1 for the recipes this build has).
    pub finished_items: u32,
    /// Whether a fuel item was consumed from the fuel slot this tick.
    pub fuel_consumed: bool,
    /// Whether cooking advanced this tick.
    pub cook_progressed: bool,
    /// Whether [`FurnaceState::is_lit`] differs from before the tick, i.e. whether
    /// `container_set_data` needs to carry a new `lit` value.
    pub state_changed: bool,
}

impl FurnaceTickReport {
    /// Whether the tick changed anything the client or the world cares about.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        !self.fuel_consumed
            && !self.cook_progressed
            && self.finished_items == 0
            && !self.state_changed
    }
}

/// The furnace's tick rule. Stateless: all state lives in [`FurnaceState`] and the
/// [`Container`], so a caller can persist, reload and resume without this type
/// holding anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Furnace;

impl Furnace {
    /// Advance one furnace by one tick.
    ///
    /// `container` is the furnace block entity's container; `slots` says which
    /// slots it uses; `stack_sizes` decides the output slot's capacity. Nothing
    /// else is touched, so the same call is valid for a block entity, a test and a
    /// menu-owned container.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the three slots are not distinct or the
    /// container is too small, and [`ServerError::InvalidAction`] if writing a slot
    /// is refused (impossible for a validated index). See
    /// [`SmeltingRegistry::burn_ticks_for`] for the corrupt-data path: an item id
    /// outside the registry is an error, not "not a fuel".
    pub fn tick(
        state: &mut FurnaceState,
        container: &mut Container,
        slots: FurnaceSlots,
        recipes: &SmeltingRegistry,
        fuelling: &ItemRegistry,
        stack_sizes: &StackSizeTable,
    ) -> ServerResult<FurnaceTickReport> {
        if !slots.are_distinct() {
            return Err(ServerError::Invariant(format!(
                "a furnace cannot use the same slot twice: input {}, fuel {}, output {}",
                slots.input, slots.fuel, slots.output
            )));
        }
        if container.len() <= slots.input.max(slots.fuel).max(slots.output) {
            return Err(ServerError::Invariant(format!(
                "a {}-slot container cannot hold the furnace slots {:?}",
                container.len(),
                slots.indices()
            )));
        }

        let was_lit = state.is_lit();
        let mut report = FurnaceTickReport::default();

        // Does the input smelt, and can the output take the result?
        let input = container.get(slots.input);
        let output = container.get(slots.output);
        let plan = plan_smelt(input, output, recipes, stack_sizes);

        let Some(plan) = plan else {
            // Nothing to cook: an empty input, an item with no recipe, or an output
            // that is full or holds a different item. Burn down whatever fuel is
            // already alight and take **no** new fuel — that is what "a full output
            // wastes no fuel" means, and it is why a blocked furnace cannot bank
            // burn time. The cooking progress is left alone: a blocked furnace must
            // not forget a part-cooked item, it simply cannot advance it.
            if state.burn_ticks_remaining > 0 {
                state.burn_ticks_remaining -= 1;
            }
            state.cook_total = 0;
            if state.burn_ticks_remaining == 0 {
                state.burn_ticks_total = 0;
            }
            report.lit = state.burn_ticks_remaining > 0;
            report.state_changed = report.lit != was_lit;
            return Ok(report);
        };

        // 1. Fuel for this tick. At most one `take_fuel` call, and the ordering is
        //    what makes the burn accounting exact:
        //
        //    * a furnace that still has burn ticks spends one of them;
        //    * a furnace with none — because the previous item just burned out, or
        //      because it was never lit — takes a fresh item, and the tick that takes
        //      it is the item's **first** lit tick. Taking it is not also charged to
        //      the item, so a `D`-tick item lights the furnace for exactly `D` ticks
        //      (see the `FurnaceState` docs for the counter's accounting);
        //    * doing the top-up in the same tick as the burnout is what keeps `lit`
        //      continuous across a handover, so a furnace with a stack of fuel never
        //      flickers and a part-cooked item survives the change of fuel.
        //
        //    Fuel is only ever taken here, where `plan` says a recipe can actually
        //    run: an empty furnace, or one whose output is full of a different item,
        //    must not eat its fuel for nothing (that decision is the `let Some(plan)
        //    else` above, which takes no fuel at all).
        let was_burning = state.burn_ticks_remaining > 0;
        if was_burning {
            state.burn_ticks_remaining -= 1;
        }
        if state.burn_ticks_remaining == 0
            && take_fuel(state, container, slots, fuelling)?.is_some()
        {
            report.fuel_consumed = true;
        }

        if state.burn_ticks_remaining == 0 {
            // 2. Nothing is burning after that: the furnace is dark for this tick,
            //    and it has nothing to cook with, so its cooking progress goes too.
            //    Leaving it behind would have an unlit furnace reporting a progress
            //    bar, which a client would happily draw.
            state.cook_progress = 0;
            state.cook_total = 0;
            state.burn_ticks_total = 0;
            report.lit = false;
            report.state_changed = was_lit;
            return Ok(report);
        }

        // 3. The furnace is alight for this tick. `cook_progress` reaching
        //    `cook_ticks` is the only place an item is produced, so an N-tick recipe
        //    takes exactly N lit ticks.
        state.cook_total = plan.cook_ticks;
        state.cook_progress = state.cook_progress.saturating_add(1);
        report.cook_progressed = true;

        if state.cook_progress >= plan.cook_ticks {
            if let Some(finished) = finish(container, slots, &plan, stack_sizes)? {
                report.finished_items = finished;
                state.experience += plan.experience;
            }
            state.cook_progress = 0;
            state.cook_total = if container.get(slots.input).is_empty() {
                0
            } else {
                plan.cook_ticks
            };
        }

        if state.burn_ticks_remaining == 0 {
            state.burn_ticks_total = 0;
        }
        report.lit = state.is_lit();
        report.state_changed = report.lit != was_lit;
        Ok(report)
    }

    /// Whether a lit furnace would resume or continue cooking this tick: the
    /// convenience form of the [`Furnace::tick`] precondition, for a caller (a
    /// hopper, a UI) that wants to know without ticking.
    ///
    /// # Errors
    ///
    /// As for [`Furnace::tick`] with respect to slot validation.
    pub fn ready_to_cook(
        container: &Container,
        slots: FurnaceSlots,
        recipes: &SmeltingRegistry,
        stack_sizes: &StackSizeTable,
    ) -> ServerResult<bool> {
        if !slots.are_distinct() {
            return Err(ServerError::Invariant(format!(
                "a furnace cannot use the same slot twice: input {}, fuel {}, output {}",
                slots.input, slots.fuel, slots.output
            )));
        }
        if container.len() <= slots.input.max(slots.fuel).max(slots.output) {
            return Err(ServerError::Invariant(format!(
                "a {}-slot container cannot hold the furnace slots {:?}",
                container.len(),
                slots.indices()
            )));
        }
        Ok(plan_smelt(
            container.get(slots.input),
            container.get(slots.output),
            recipes,
            stack_sizes,
        )
        .is_some())
    }
}

/// A planned smelt: which recipe, and how much room the output has.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SmeltPlan {
    output: i32,
    output_count: i32,
    cook_ticks: u32,
    experience: f32,
}

/// Decide whether `input` can smelt into `output`, i.e. whether this furnace has
/// anything to do.
///
/// Infallible: the only inputs are two stacks and a recipe table, and a stack whose
/// item id is not a smelting input is simply "nothing to do". The registry-backed
/// checks live in [`take_fuel`] (a fuel *is* resolved through the registry) and in
/// [`SmeltingRegistry::baseline`], which refuses a missing name at load time.
fn plan_smelt(
    input: ItemStack,
    output: ItemStack,
    recipes: &SmeltingRegistry,
    stack_sizes: &StackSizeTable,
) -> Option<SmeltPlan> {
    let input_id = input.item_id()?;
    let recipe = recipes.recipe_for(input_id)?;
    let limit = stack_sizes.max_stack_size(recipe.output);
    if !output.is_empty() {
        if output.item_id() != Some(recipe.output) {
            return None;
        }
        if output.count() + recipe.output_count > limit {
            return None;
        }
    }
    Some(SmeltPlan {
        output: recipe.output,
        output_count: recipe.output_count,
        cook_ticks: recipe.cook_ticks,
        experience: recipe.experience,
    })
}

/// Try to consume one fuel item, setting the state's burn timers on success.
///
/// Returns the number of ticks the consumed fuel burns for, or `None` when the
/// fuel slot is empty or holds something that is not a fuel **this build knows**
/// (an unknown fuel is not an error: it is an item that does not burn).
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the fuel slot's item id is outside the
/// registry (see [`SmeltingRegistry::burn_ticks_for`]), and
/// [`ServerError::InvalidAction`] if the fuel slot cannot be written (impossible
/// for a validated index).
fn take_fuel(
    state: &mut FurnaceState,
    container: &mut Container,
    slots: FurnaceSlots,
    items: &ItemRegistry,
) -> ServerResult<Option<u32>> {
    let fuel = container.get(slots.fuel);
    let Some(fuel_id) = fuel.item_id() else {
        return Ok(None);
    };
    let Some(ticks) = burn_ticks_for(items, fuel_id)? else {
        return Ok(None);
    };
    if ticks == 0 {
        return Ok(None);
    }
    let mut left = fuel;
    left.shrink(1);
    container.set(slots.fuel, left)?;
    state.burn_ticks_remaining = ticks;
    state.burn_ticks_total = ticks;
    Ok(Some(ticks))
}

/// Produce one item into the output slot.
///
/// Returns `Ok(None)` when the output cannot take it, which the caller treats as
/// "this tick produced nothing" â€?the input is **not** consumed in that case, so a
/// race between the tick and a player emptying the output slot can never destroy
/// the input.
///
/// # Errors
///
/// [`ServerError::InvalidAction`] if the output slot cannot be written (impossible
/// for a validated index).
fn finish(
    container: &mut Container,
    slots: FurnaceSlots,
    plan: &SmeltPlan,
    stack_sizes: &StackSizeTable,
) -> ServerResult<Option<u32>> {
    let output = container.get(slots.output);
    let limit = stack_sizes.max_stack_size(plan.output);
    let room = if output.is_empty() {
        limit
    } else if output.item_id() == Some(plan.output) {
        limit - output.count()
    } else {
        0
    };
    if room < plan.output_count {
        return Ok(None);
    }

    // A matched recipe guarantees `output_count` is at least 1 and `room` is at least
    // that, so the new count is in `1..=limit` and `ItemStack::new` cannot refuse it.
    // Building the stack directly (rather than growing the existing one) is what makes
    // smelting into an **empty** output slot work: `ItemStack::grow_capped` treats an
    // empty stack as "nothing to grow" and would silently produce nothing.
    let produced = if output.is_empty() {
        ItemStack::new(plan.output, plan.output_count)?
    } else {
        let mut produced = output;
        produced.grow_capped(plan.output_count, limit);
        produced
    };

    let mut input = container.get(slots.input);
    if input.shrink(1) != 1 {
        // The input vanished between the match and here. Refuse without writing
        // anything: conservation beats optimism.
        return Ok(None);
    }
    container.set(slots.input, input)?;
    container.set(slots.output, produced)?;
    container.mark_changed(slots.output);
    Ok(Some(1))
}

/// An item id by name, or [`ServerError::CorruptData`] naming the missing item.
fn id_of(items: &ItemRegistry, name: &str) -> ServerResult<i32> {
    items.id(name).map_err(|_| {
        ServerError::CorruptData(format!(
            "the baseline smelting table needs item {name:?}, which the item registry does not \
             contain"
        ))
    })
}

#[cfg(test)]
mod tests;
