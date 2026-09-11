//! Item stacks.
//!
//! An [`ItemStack`] is the unit the inventory, the container payload and the
//! player's `playerdata/<uuid>.dat` all agree on: a numeric item id plus a
//! count. Two rules drive the whole type:
//!
//! - **`item_id == 0` is `minecraft:air`** and means "nothing"; the registry's
//!   own table confirms it (`items.tsv` line 1 is `0\tminecraft:air\t-`). An
//!   empty stack's count is meaningless and is forced to `0`.
//! - **A stack you did not check is not an item id.** [`ItemStack::item_id`]
//!   returns `Option<i32>` and the fields are private, so a caller cannot feed
//!   an empty stack into an item lookup without going through
//!   [`ItemStack::is_empty`] first.
//!
//! ## Stack size limits, and why they are a table *and* a lookup
//!
//! Vanilla's default is `Item.Properties.stacksTo(64)`; 205 items override it
//! down to 16 (buckets, snowballs, eggs, signs, hanging signs) or 1 (tools,
//! spears, armour, boats, minecarts, potions, …). Those overrides are keyed by
//! item **name** here ([`STACK_SIZE_1`], [`STACK_SIZE_16`]), so the table is
//! auditable against `items.tsv` and a test can cross-check every row.
//!
//! Because the table is keyed by name, an **id**-keyed limit requires a
//! registry lookup, which this crate does once at startup:
//! [`StackSizeTable::resolve`] turns the name table into a sorted id table, and
//! [`StackSizeTable::max_stack_size`] is then a binary search with no
//! allocation. Per-item limits are enforced through that value, so
//! [`ItemStack::new`] only refuses counts that are illegal for *every* item;
//! the inventory applies the per-item cap when it accepts a stack.
//!
//! ## Known gaps (do not read this as vanilla-complete)
//!
//! - `stacksTo` is **data**: a datapack can change it, and per-stack
//!   `components` (`max_stack_size`) can override it per item. This crate
//!   models the hard-coded 26.1.2 vanilla table only.
//! - The table covers the families the task named (tools, armour, buckets,
//!   potions, ender pearls, snowballs, eggs, signs, minecarts, boats) plus the
//!   other vanilla stack-1 consumables/containers (stews, pies, cake, golden
//!   apples, saddle, written books, totem, bundle). It is **not** a port of
//!   every `stacksTo` call site in the game.
//! - Item **durability**, enchantments and components are not modelled at all:
//!   a stack here is an identity plus a count (AGENTS.md section 3.3).

use mc_core::error::{ServerError, ServerResult};
use mc_registry::ItemRegistry;
use std::fmt;

/// Item id of `minecraft:air`, i.e. "this stack is empty".
pub const AIR_ITEM_ID: i32 = 0;

/// Vanilla's default maximum stack size (`Item.Properties.stacksTo(64)`).
pub const DEFAULT_MAX_STACK_SIZE: i32 = 64;

/// Maximum size of a stack of buckets, snowballs, eggs and signs.
pub const MAX_STACK_SIZE_16: i32 = 16;

/// Maximum size of a stack of tools, armour, boats, minecarts and potions.
pub const MAX_STACK_SIZE_1: i32 = 1;

/// Highest count any vanilla item can hold, i.e. the hard ceiling
/// [`ItemStack::new`] enforces without a registry.
pub const HARD_MAX_STACK_SIZE: i32 = DEFAULT_MAX_STACK_SIZE;

/// A count of items of one type.
///
/// Guarantees: `item_id >= 0`, `0 <= count <= 64`, and `item_id == 0` implies
/// `count == 0`. The per-item cap from the stack-size table is applied by the
/// owner of the table (see [`StackSizeTable`]), because a stack on its own has
/// no registry to consult.
///
/// `PartialOrd`/`Ord` order by `(item_id, count)` so a container payload can be
/// sorted deterministically for golden comparisons (AGENTS.md section 3.6).
///
/// ```
/// # use mc_entity::ItemStack;
/// let mut stack = ItemStack::new(1, 64).expect("stone stacks to 64");
/// assert_eq!(stack.count(), 64);
/// assert_eq!(stack.grow(10), 10, "a full stack cannot take more");
/// assert_eq!(stack.shrink(4), 4);
/// assert_eq!(stack.count(), 60);
/// assert!(ItemStack::EMPTY.is_empty());
/// assert_eq!(ItemStack::EMPTY.item_id(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemStack {
    /// `minecraft:air` (0) for an empty stack, otherwise a registry id.
    item_id: i32,
    /// Number of items, `0` for an empty stack.
    count: i32,
}

impl ItemStack {
    /// The empty stack (`minecraft:air`, count 0).
    pub const EMPTY: Self = Self {
        item_id: AIR_ITEM_ID,
        count: 0,
    };

    /// Build a stack of `item_id` with `count` items.
    ///
    /// `count <= 0`, or `item_id == 0`, yields [`ItemStack::EMPTY`]: vanilla
    /// treats a non-positive count as "no items", and `air` is not a carryable
    /// item, so neither is an error.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `item_id < 0` (no registry id can be
    /// negative, and the caller is usually decoding client input) or when
    /// `count > 64` (`HARD_MAX_STACK_SIZE`; beyond every legal stack, so it is
    /// attack input rather than a rounding accident).
    pub fn new(item_id: i32, count: i32) -> ServerResult<Self> {
        if item_id < 0 {
            return Err(ServerError::InvalidAction(format!(
                "negative item id {item_id}"
            )));
        }
        if count > HARD_MAX_STACK_SIZE {
            return Err(ServerError::InvalidAction(format!(
                "item count {count} for item id {item_id} exceeds any legal stack"
            )));
        }
        if item_id == AIR_ITEM_ID || count <= 0 {
            return Ok(Self::EMPTY);
        }
        Ok(Self { item_id, count })
    }

    /// Build a stack **without** any limit check, for tests that need to
    /// reproduce an oversized or impossible stack.
    ///
    /// `#[cfg(test)]` only: the public API cannot build a stack that violates
    /// [`ItemStack::is_valid`], which is exactly the invariant the inventory's
    /// per-item check has to be tested against (a spoofed `64` buckets, say).
    /// Exposing this outside tests would be a way to defeat that check.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn from_raw_parts(item_id: i32, count: i32) -> Self {
        Self { item_id, count }
    }

    /// Replace this stack with [`ItemStack::EMPTY`], yielding what was held.
    #[must_use]
    pub fn take(&mut self) -> Self {
        std::mem::replace(self, Self::EMPTY)
    }

    /// Whether the stack holds no items.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.item_id == AIR_ITEM_ID || self.count <= 0
    }

    /// Registry id of the item, or `None` for an empty stack.
    ///
    /// Deliberately an `Option`: the type system then makes "use an `ItemStack`
    /// as an item id" impossible without checking emptiness.
    #[must_use]
    pub const fn item_id(&self) -> Option<i32> {
        if self.item_id == AIR_ITEM_ID {
            None
        } else {
            Some(self.item_id)
        }
    }

    /// Number of items in the stack (`0` when empty).
    #[must_use]
    pub const fn count(&self) -> i32 {
        if self.item_id == AIR_ITEM_ID {
            0
        } else {
            self.count
        }
    }

    /// Whether `other` is a non-empty stack of the same item.
    #[must_use]
    pub fn same_item(&self, other: &Self) -> bool {
        !self.is_empty() && self.item_id == other.item_id
    }

    /// Whether this stack is within the limits [`ItemStack::new`] enforces.
    ///
    /// The inventory additionally checks the per-item cap before accepting a
    /// stack, so a persisted or decoded stack cannot smuggle an oversized count
    /// past an id-only check.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.item_id >= AIR_ITEM_ID && self.count >= 0 && self.count <= HARD_MAX_STACK_SIZE
    }

    /// Split off up to `n` items, leaving the remainder behind.
    ///
    /// A non-positive `n` splits nothing: "take 0 items" is a no-op request,
    /// not a protocol violation.
    #[must_use]
    pub fn split(&mut self, n: i32) -> Self {
        if n <= 0 || self.is_empty() {
            return Self::EMPTY;
        }
        let taken = n.min(self.count);
        self.count -= taken;
        let out = Self {
            item_id: self.item_id,
            count: taken,
        };
        if self.count <= 0 {
            *self = Self::EMPTY;
        }
        out
    }

    /// Add up to `n` items, capped at `limit` (the item's maximum stack size).
    ///
    /// Returns how many did **not** fit: vanilla's `ItemStack.grow` also clamps
    /// and simply does not report it, so a caller that must not lose items has
    /// to look at the return value. Growing an empty stack, or growing by a
    /// non-positive amount, changes nothing.
    pub fn grow_capped(&mut self, n: i32, limit: i32) -> i32 {
        if n <= 0 || self.is_empty() {
            return n.max(0);
        }
        let limit = limit.clamp(1, HARD_MAX_STACK_SIZE);
        let room = (limit - self.count).max(0);
        let added = n.min(room);
        self.count += added;
        n - added
    }

    /// [`ItemStack::grow_capped`] with the vanilla default limit of 64.
    pub fn grow(&mut self, n: i32) -> i32 {
        self.grow_capped(n, DEFAULT_MAX_STACK_SIZE)
    }

    /// Remove up to `n` items; returns how many were actually removed.
    ///
    /// Removing more than the stack holds empties it and reports the real
    /// number, so a caller that needs an exact count (dropping items, refunding
    /// a transaction) can detect the shortfall.
    pub fn shrink(&mut self, n: i32) -> i32 {
        if n <= 0 || self.is_empty() {
            return 0;
        }
        let removed = n.min(self.count);
        self.count -= removed;
        if self.count <= 0 {
            *self = Self::EMPTY;
        }
        removed
    }

    /// Add `other` onto this stack under `limit`, returning what did not fit.
    ///
    /// Only stacks of the same item merge; different items (and two empties)
    /// merge nothing. An **empty `self` takes `other` whole** when it fits or
    /// under `limit`, so this is also the copy-into-an-empty-slot operation —
    /// otherwise a caller would need a second code path for "place a stack in an
    /// empty slot", and forgetting it silently drops items.
    ///
    /// On a partial merge `other` keeps what did not fit, and that remainder is
    /// the return value, so a caller that ignores it still has not lost the
    /// items. When everything fits, `other` is empty and `0` is returned —
    /// vanilla's `ItemStack.merge`/`Inventory.add` contract.
    pub fn merge_capped(&mut self, other: &mut Self, limit: i32) -> i32 {
        if other.is_empty() {
            return 0;
        }
        if !self.is_empty() && !self.same_item(other) {
            return other.count();
        }
        let limit = limit.clamp(1, HARD_MAX_STACK_SIZE);
        let room = if self.is_empty() {
            limit
        } else {
            (limit - self.count).max(0)
        };
        // `split` returns the taken part and leaves the remainder behind in
        // `other`, which is exactly the contract this function needs.
        let taken = other.split(room);
        if self.is_empty() {
            *self = taken;
        } else {
            self.count += taken.count;
        }
        other.count
    }

    /// [`ItemStack::merge_capped`] with the vanilla default limit of 64.
    pub fn merge(&mut self, other: &mut Self) -> i32 {
        self.merge_capped(other, DEFAULT_MAX_STACK_SIZE)
    }
}

impl Default for ItemStack {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl fmt::Display for ItemStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.item_id() {
            Some(id) => write!(f, "item {id} x{}", self.count),
            None => f.write_str("empty"),
        }
    }
}

/// One row of the stack-size exception table: a registry item name and the
/// limit vanilla's `Item.Properties.stacksTo` gives it.
type StackSizeRule = (&'static str, i32);

/// Items that stack to **1**: everything with durability, plus containers whose
/// contents live in the item itself.
///
/// Generated from `crates/test-support/fixtures/registry/items.tsv` by taking
/// every item whose name matches one of the families below and then adding the
/// individually named items; the workspace test that walks the whole fixture
/// re-checks both halves, so a registry regeneration that adds an item to a
/// family fails the test until the table is updated.
///
/// Families: `_pickaxe`, `_shovel`, `_axe`, `_hoe`, `_sword`, `_spear` (new in
/// 26.1), `_helmet`, `_chestplate`, `_leggings`, `_boots`, `_horse_armor`,
/// `_nautilus_armor` (new in 26.1), `_boat`, `_chest_boat`, `_raft`,
/// `_chest_raft`, `_harness` (new in 26.1), every `<colour>_bundle`, and the
/// named remainder (bow, crossbow, trident, shears, shield, elytra, fishing rod,
/// flint and steel, the two `*_on_a_stick`s, the minecarts, saddle, wolf armour,
/// the stews, soups and pies, cake, the golden apples, the container potions,
/// the books and the totem).
///
/// Sorted by name so [`max_stack_size_for_name`] can binary-search it and so the
/// table stays diff-stable when reviewed.
const STACK_SIZE_1: &[StackSizeRule] = &[
    ("minecraft:acacia_boat", 1),
    ("minecraft:acacia_chest_boat", 1),
    ("minecraft:bamboo_chest_raft", 1),
    ("minecraft:bamboo_raft", 1),
    ("minecraft:beetroot_soup", 1),
    ("minecraft:birch_boat", 1),
    ("minecraft:birch_chest_boat", 1),
    ("minecraft:black_bundle", 1),
    ("minecraft:black_harness", 1),
    ("minecraft:blue_bundle", 1),
    ("minecraft:blue_harness", 1),
    ("minecraft:bow", 1),
    ("minecraft:brown_bundle", 1),
    ("minecraft:brown_harness", 1),
    ("minecraft:bundle", 1),
    ("minecraft:cake", 1),
    ("minecraft:carrot_on_a_stick", 1),
    ("minecraft:chainmail_boots", 1),
    ("minecraft:chainmail_chestplate", 1),
    ("minecraft:chainmail_helmet", 1),
    ("minecraft:chainmail_leggings", 1),
    ("minecraft:cherry_boat", 1),
    ("minecraft:cherry_chest_boat", 1),
    ("minecraft:chest_minecart", 1),
    ("minecraft:command_block_minecart", 1),
    ("minecraft:copper_axe", 1),
    ("minecraft:copper_boots", 1),
    ("minecraft:copper_chestplate", 1),
    ("minecraft:copper_helmet", 1),
    ("minecraft:copper_hoe", 1),
    ("minecraft:copper_horse_armor", 1),
    ("minecraft:copper_leggings", 1),
    ("minecraft:copper_nautilus_armor", 1),
    ("minecraft:copper_pickaxe", 1),
    ("minecraft:copper_shovel", 1),
    ("minecraft:copper_spear", 1),
    ("minecraft:copper_sword", 1),
    ("minecraft:crossbow", 1),
    ("minecraft:cyan_bundle", 1),
    ("minecraft:cyan_harness", 1),
    ("minecraft:dark_oak_boat", 1),
    ("minecraft:dark_oak_chest_boat", 1),
    ("minecraft:diamond_axe", 1),
    ("minecraft:diamond_boots", 1),
    ("minecraft:diamond_chestplate", 1),
    ("minecraft:diamond_helmet", 1),
    ("minecraft:diamond_hoe", 1),
    ("minecraft:diamond_horse_armor", 1),
    ("minecraft:diamond_leggings", 1),
    ("minecraft:diamond_nautilus_armor", 1),
    ("minecraft:diamond_pickaxe", 1),
    ("minecraft:diamond_shovel", 1),
    ("minecraft:diamond_spear", 1),
    ("minecraft:diamond_sword", 1),
    ("minecraft:elytra", 1),
    ("minecraft:enchanted_golden_apple", 1),
    ("minecraft:fishing_rod", 1),
    ("minecraft:flint_and_steel", 1),
    ("minecraft:furnace_minecart", 1),
    ("minecraft:golden_apple", 1),
    ("minecraft:golden_axe", 1),
    ("minecraft:golden_boots", 1),
    ("minecraft:golden_chestplate", 1),
    ("minecraft:golden_helmet", 1),
    ("minecraft:golden_hoe", 1),
    ("minecraft:golden_horse_armor", 1),
    ("minecraft:golden_leggings", 1),
    ("minecraft:golden_nautilus_armor", 1),
    ("minecraft:golden_pickaxe", 1),
    ("minecraft:golden_shovel", 1),
    ("minecraft:golden_spear", 1),
    ("minecraft:golden_sword", 1),
    ("minecraft:gray_bundle", 1),
    ("minecraft:gray_harness", 1),
    ("minecraft:green_bundle", 1),
    ("minecraft:green_harness", 1),
    ("minecraft:hopper_minecart", 1),
    ("minecraft:iron_axe", 1),
    ("minecraft:iron_boots", 1),
    ("minecraft:iron_chestplate", 1),
    ("minecraft:iron_helmet", 1),
    ("minecraft:iron_hoe", 1),
    ("minecraft:iron_horse_armor", 1),
    ("minecraft:iron_leggings", 1),
    ("minecraft:iron_nautilus_armor", 1),
    ("minecraft:iron_pickaxe", 1),
    ("minecraft:iron_shovel", 1),
    ("minecraft:iron_spear", 1),
    ("minecraft:iron_sword", 1),
    ("minecraft:jungle_boat", 1),
    ("minecraft:jungle_chest_boat", 1),
    ("minecraft:leather_boots", 1),
    ("minecraft:leather_chestplate", 1),
    ("minecraft:leather_helmet", 1),
    ("minecraft:leather_horse_armor", 1),
    ("minecraft:leather_leggings", 1),
    ("minecraft:light_blue_bundle", 1),
    ("minecraft:light_blue_harness", 1),
    ("minecraft:light_gray_bundle", 1),
    ("minecraft:light_gray_harness", 1),
    ("minecraft:lime_bundle", 1),
    ("minecraft:lime_harness", 1),
    ("minecraft:lingering_potion", 1),
    ("minecraft:magenta_bundle", 1),
    ("minecraft:magenta_harness", 1),
    ("minecraft:mangrove_boat", 1),
    ("minecraft:mangrove_chest_boat", 1),
    ("minecraft:minecart", 1),
    ("minecraft:mushroom_stew", 1),
    ("minecraft:netherite_axe", 1),
    ("minecraft:netherite_boots", 1),
    ("minecraft:netherite_chestplate", 1),
    ("minecraft:netherite_helmet", 1),
    ("minecraft:netherite_hoe", 1),
    ("minecraft:netherite_horse_armor", 1),
    ("minecraft:netherite_leggings", 1),
    ("minecraft:netherite_nautilus_armor", 1),
    ("minecraft:netherite_pickaxe", 1),
    ("minecraft:netherite_shovel", 1),
    ("minecraft:netherite_spear", 1),
    ("minecraft:netherite_sword", 1),
    ("minecraft:oak_boat", 1),
    ("minecraft:oak_chest_boat", 1),
    ("minecraft:orange_bundle", 1),
    ("minecraft:orange_harness", 1),
    ("minecraft:pale_oak_boat", 1),
    ("minecraft:pale_oak_chest_boat", 1),
    ("minecraft:pink_bundle", 1),
    ("minecraft:pink_harness", 1),
    ("minecraft:potion", 1),
    ("minecraft:pumpkin_pie", 1),
    ("minecraft:purple_bundle", 1),
    ("minecraft:purple_harness", 1),
    ("minecraft:rabbit_stew", 1),
    ("minecraft:red_bundle", 1),
    ("minecraft:red_harness", 1),
    ("minecraft:saddle", 1),
    ("minecraft:shears", 1),
    ("minecraft:shield", 1),
    ("minecraft:splash_potion", 1),
    ("minecraft:spruce_boat", 1),
    ("minecraft:spruce_chest_boat", 1),
    ("minecraft:stone_axe", 1),
    ("minecraft:stone_hoe", 1),
    ("minecraft:stone_pickaxe", 1),
    ("minecraft:stone_shovel", 1),
    ("minecraft:stone_spear", 1),
    ("minecraft:stone_sword", 1),
    ("minecraft:suspicious_stew", 1),
    ("minecraft:tnt_minecart", 1),
    ("minecraft:totem_of_undying", 1),
    ("minecraft:trident", 1),
    ("minecraft:turtle_helmet", 1),
    ("minecraft:warped_fungus_on_a_stick", 1),
    ("minecraft:white_bundle", 1),
    ("minecraft:white_harness", 1),
    ("minecraft:wolf_armor", 1),
    ("minecraft:wooden_axe", 1),
    ("minecraft:wooden_hoe", 1),
    ("minecraft:wooden_pickaxe", 1),
    ("minecraft:wooden_shovel", 1),
    ("minecraft:wooden_spear", 1),
    ("minecraft:wooden_sword", 1),
    ("minecraft:writable_book", 1),
    ("minecraft:written_book", 1),
    ("minecraft:yellow_bundle", 1),
    ("minecraft:yellow_harness", 1),
];

/// Items that stack to **16**: every `_bucket`, the three throwables (egg, ender
/// pearl, snowball) and every `_sign`/`_hanging_sign`.
///
/// Same sorted-by-name shape as [`STACK_SIZE_1`].
const STACK_SIZE_16: &[StackSizeRule] = &[
    ("minecraft:acacia_hanging_sign", 16),
    ("minecraft:acacia_sign", 16),
    ("minecraft:axolotl_bucket", 16),
    ("minecraft:bamboo_hanging_sign", 16),
    ("minecraft:bamboo_sign", 16),
    ("minecraft:birch_hanging_sign", 16),
    ("minecraft:birch_sign", 16),
    ("minecraft:bucket", 16),
    ("minecraft:cherry_hanging_sign", 16),
    ("minecraft:cherry_sign", 16),
    ("minecraft:cod_bucket", 16),
    ("minecraft:crimson_hanging_sign", 16),
    ("minecraft:crimson_sign", 16),
    ("minecraft:dark_oak_hanging_sign", 16),
    ("minecraft:dark_oak_sign", 16),
    ("minecraft:egg", 16),
    ("minecraft:ender_pearl", 16),
    ("minecraft:jungle_hanging_sign", 16),
    ("minecraft:jungle_sign", 16),
    ("minecraft:lava_bucket", 16),
    ("minecraft:mangrove_hanging_sign", 16),
    ("minecraft:mangrove_sign", 16),
    ("minecraft:milk_bucket", 16),
    ("minecraft:oak_hanging_sign", 16),
    ("minecraft:oak_sign", 16),
    ("minecraft:pale_oak_hanging_sign", 16),
    ("minecraft:pale_oak_sign", 16),
    ("minecraft:powder_snow_bucket", 16),
    ("minecraft:pufferfish_bucket", 16),
    ("minecraft:salmon_bucket", 16),
    ("minecraft:snowball", 16),
    ("minecraft:spruce_hanging_sign", 16),
    ("minecraft:spruce_sign", 16),
    ("minecraft:tadpole_bucket", 16),
    ("minecraft:tropical_fish_bucket", 16),
    ("minecraft:warped_hanging_sign", 16),
    ("minecraft:warped_sign", 16),
    ("minecraft:water_bucket", 16),
];

/// Maximum stack size of an item **name**: 16 or 1 for the documented
/// exceptions, 64 for everything else.
///
/// An unknown name also gets 64 — "the default unless overridden" — so a
/// caller cannot mistake a typo for a limit of 1. Use
/// [`StackSizeTable::resolve`] when the name must be proven to exist.
#[must_use]
pub fn max_stack_size_for_name(item_name: &str) -> i32 {
    if let Ok(index) = STACK_SIZE_16.binary_search_by(|(name, _)| name.cmp(&item_name)) {
        return STACK_SIZE_16[index].1;
    }
    if let Ok(index) = STACK_SIZE_1.binary_search_by(|(name, _)| name.cmp(&item_name)) {
        return STACK_SIZE_1[index].1;
    }
    DEFAULT_MAX_STACK_SIZE
}

/// The vanilla stack-size table resolved against one [`ItemRegistry`].
///
/// This is the id-keyed form of [`max_stack_size_for_name`]: build it once at
/// startup and keep it beside the registry. Lookups are a binary search over
/// ~165 entries with no allocation, so it is safe to call per inventory slot.
///
/// ```
/// # use mc_entity::{ItemStack, StackSizeTable};
/// # use mc_registry::ItemRegistry;
/// # use std::path::Path;
/// # let items = ItemRegistry::load(
/// #     &Path::new(env!("CARGO_MANIFEST_DIR"))
/// #         .join("../../crates/test-support/fixtures/registry/items.tsv")).expect("fixture");
/// let sizes = StackSizeTable::resolve(&items).expect("table resolves");
/// let bucket = items.id("minecraft:bucket").expect("bucket");
/// assert_eq!(sizes.max_stack_size(bucket), 16);
/// assert_eq!(sizes.max_stack_size(items.id("minecraft:stone").expect("stone")), 64);
/// // A 16-count stack of buckets: only 6 of 10 further buckets fit.
/// let mut stack = ItemStack::new(bucket, 10).expect("stack");
/// assert_eq!(stack.grow_capped(10, sizes.max_stack_size(bucket)), 4);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackSizeTable {
    /// `(item id, limit)`, sorted by id.
    by_id: Vec<(i32, i32)>,
}

impl Default for StackSizeTable {
    /// An **empty** table: every item id gets the vanilla default of 64.
    ///
    /// This is deliberately not the vanilla exception table — that requires a
    /// registry to resolve names to ids. A server must use
    /// [`StackSizeTable::resolve`]; an empty table silently stacks tools by 64,
    /// which only matters for gameplay, not for memory safety.
    fn default() -> Self {
        Self { by_id: Vec::new() }
    }
}

impl StackSizeTable {
    /// Resolve both exception tables against `items`.
    ///
    /// Resolution is a hard check on purpose: every table name must exist, so
    /// "the registry was regenerated and a name moved" fails at startup instead
    /// of silently turning a tool into a 64-stack item (AGENTS.md section 3.3).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when a table name is absent from `items`.
    pub fn resolve(items: &ItemRegistry) -> ServerResult<Self> {
        let mut by_id = Vec::with_capacity(STACK_SIZE_1.len() + STACK_SIZE_16.len());
        for (name, limit) in STACK_SIZE_1.iter().chain(STACK_SIZE_16) {
            let id = items.id(name).map_err(|_| {
                ServerError::CorruptData(format!(
                    "stack-size table names {name:?}, which the item registry does not contain"
                ))
            })?;
            by_id.push((id, *limit));
        }
        by_id.sort_unstable();
        Ok(Self { by_id })
    }

    /// Maximum stack size of a resolved item id (64 when not an exception).
    #[must_use]
    pub fn max_stack_size(&self, item_id: i32) -> i32 {
        self.by_id
            .binary_search_by_key(&item_id, |(id, _)| *id)
            .map_or(DEFAULT_MAX_STACK_SIZE, |index| self.by_id[index].1)
    }

    /// Number of exception entries (always 165 for the 26.1.2 vanilla table).
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the table has no entries (never true for a resolved table).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    // Health, hunger and experience are exact values in this crate's ranges
    // (0.0..=20.0, 0.0..=5.0, the level costs and their exact fractions), and the
    // tests assert them exactly on purpose: an epsilon would hide the rounding
    // bug the assertions exist to catch, because these are equality checks on
    // values that must be bit-identical.
    #![allow(clippy::float_cmp)]
    use super::{
        DEFAULT_MAX_STACK_SIZE, ItemStack, STACK_SIZE_1, STACK_SIZE_16, StackSizeTable,
        max_stack_size_for_name,
    };
    use mc_core::error::ServerError;
    use mc_registry::ItemRegistry;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/test-support/fixtures/registry")
            .join(name)
    }

    fn registry() -> ItemRegistry {
        ItemRegistry::load(&fixture("items.tsv")).expect("items fixture loads")
    }

    #[test]
    fn the_exception_table_names_real_registry_items() {
        let items = registry();
        let sizes = StackSizeTable::resolve(&items).expect("every table name exists in items.tsv");
        assert_eq!(sizes.len(), STACK_SIZE_1.len() + STACK_SIZE_16.len());
        let bucket = items.id("minecraft:bucket").expect("bucket");
        let pickaxe = items.id("minecraft:diamond_pickaxe").expect("pickaxe");
        let stone = items.id("minecraft:stone").expect("stone");
        assert_eq!(sizes.max_stack_size(bucket), 16);
        assert_eq!(sizes.max_stack_size(pickaxe), 1);
        assert_eq!(sizes.max_stack_size(stone), 64);
        assert_eq!(sizes.max_stack_size(0), 64, "air is not an exception");
        assert_eq!(
            sizes.max_stack_size(items.id("minecraft:oak_sign").expect("sign")),
            16
        );
        assert_eq!(
            sizes.max_stack_size(items.id("minecraft:oak_boat").expect("boat")),
            1
        );
    }

    #[test]
    fn a_table_name_missing_from_the_registry_is_an_error() {
        // A registry containing only air must fail resolution rather than
        // silently reporting 64 for every tool.
        let tiny = ItemRegistry::parse("0\tminecraft:air\t-\n").expect("parses");
        let err = StackSizeTable::resolve(&tiny).expect_err("names cannot resolve");
        assert!(matches!(err, ServerError::CorruptData(_)), "{err:?}");
    }

    #[test]
    fn both_tables_are_sorted_so_the_binary_search_is_sound() {
        for table in [STACK_SIZE_1, STACK_SIZE_16] {
            for pair in table.windows(2) {
                assert!(
                    pair[0].0 < pair[1].0,
                    "{} must sort before {}",
                    pair[0].0,
                    pair[1].0
                );
            }
        }
    }

    /// The fixture is the whole 26.1.2 item registry, so cross-checking its rows
    /// against the name families catches both a missing and an extra entry.
    #[test]
    fn name_pattern_rule_agrees_with_the_whole_registry() {
        for line in std::fs::read_to_string(fixture("items.tsv"))
            .expect("fixture readable")
            .lines()
        {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let name = fields[1];
            let id: i32 = fields[0].parse().expect("id");
            if id == 0 {
                continue;
            }
            let simple = name.strip_prefix("minecraft:").expect("namespaced");
            let pattern_says_1 = simple.ends_with("_pickaxe")
                || simple.ends_with("_shovel")
                || simple.ends_with("_axe")
                || simple.ends_with("_hoe")
                || simple.ends_with("_sword")
                || simple.ends_with("_spear")
                || simple.ends_with("_helmet")
                || simple.ends_with("_chestplate")
                || simple.ends_with("_leggings")
                || simple.ends_with("_boots")
                || simple.ends_with("_horse_armor")
                || simple.ends_with("_nautilus_armor")
                || simple.ends_with("_boat")
                || simple.ends_with("_chest_boat")
                || simple.ends_with("_raft")
                || simple.ends_with("_chest_raft")
                || simple.ends_with("_harness")
                || simple.ends_with("_bundle")
                || simple == "bundle";
            let pattern_says_16 = simple.ends_with("_bucket") || simple.ends_with("_sign");
            let actual = max_stack_size_for_name(name);
            if pattern_says_1 {
                assert_eq!(actual, 1, "{name} must stack to 1");
            } else if pattern_says_16 {
                assert_eq!(actual, 16, "{name} must stack to 16");
            }
        }
        // The patterns are exactly the tool/armour/vehicle/bucket/sign families;
        // everything else must fall through to the default.
        assert_eq!(
            max_stack_size_for_name("minecraft:stone"),
            DEFAULT_MAX_STACK_SIZE
        );
        assert_eq!(
            max_stack_size_for_name("minecraft:not_an_item"),
            DEFAULT_MAX_STACK_SIZE
        );
    }

    /// Stack-1/16 items that no name family covers: without an explicit row they
    /// would silently become 64-stacks. Every one of these is a vanilla
    /// `stacksTo(1)` item — a container (bundle, potion, stew, cake), a
    /// durability item (bow, shears, flint and steel, the two `*_on_a_stick`s),
    /// a vehicle (the minecart family) or a one-off (saddle, totem, wolf armour).
    #[test]
    fn named_exceptions_are_covered() {
        for name in [
            "minecraft:potion",
            "minecraft:splash_potion",
            "minecraft:lingering_potion",
            "minecraft:golden_apple",
            "minecraft:enchanted_golden_apple",
            "minecraft:mushroom_stew",
            "minecraft:rabbit_stew",
            "minecraft:beetroot_soup",
            "minecraft:suspicious_stew",
            "minecraft:pumpkin_pie",
            "minecraft:cake",
            "minecraft:saddle",
            "minecraft:elytra",
            "minecraft:shield",
            "minecraft:bow",
            "minecraft:crossbow",
            "minecraft:trident",
            "minecraft:shears",
            "minecraft:fishing_rod",
            "minecraft:flint_and_steel",
            "minecraft:carrot_on_a_stick",
            "minecraft:warped_fungus_on_a_stick",
            "minecraft:totem_of_undying",
            "minecraft:bundle",
            "minecraft:white_bundle",
            "minecraft:black_bundle",
            "minecraft:writable_book",
            "minecraft:written_book",
            "minecraft:ender_pearl",
            "minecraft:snowball",
            "minecraft:egg",
            "minecraft:minecart",
            "minecraft:tnt_minecart",
            "minecraft:chest_minecart",
            "minecraft:furnace_minecart",
            "minecraft:hopper_minecart",
            "minecraft:command_block_minecart",
            "minecraft:wolf_armor",
            "minecraft:turtle_helmet",
            "minecraft:white_harness",
            "minecraft:black_harness",
        ] {
            let size = max_stack_size_for_name(name);
            assert!(
                size == 1 || size == 16,
                "{name} must be an explicit exception, got {size}"
            );
        }
    }

    /// The whole table resolved against the real registry: every exception is
    /// honoured and every non-exception keeps the 64 default.
    #[test]
    fn resolution_covers_the_whole_registry() {
        let items = registry();
        let sizes = StackSizeTable::resolve(&items).expect("resolves");
        assert_eq!(sizes.len(), 205, "167 stack-1 plus 38 stack-16 entries");
        for line in std::fs::read_to_string(fixture("items.tsv"))
            .expect("fixture readable")
            .lines()
        {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let name = fields[1];
            let id: i32 = fields[0].parse().expect("id");
            assert_eq!(
                sizes.max_stack_size(id),
                max_stack_size_for_name(name),
                "{name} (id {id}) must resolve to the same limit as its name"
            );
        }
    }

    #[test]
    fn new_rejects_impossible_input_and_normalises_empty() {
        assert_eq!(ItemStack::new(1, 64).expect("in range").count(), 64);
        assert!(matches!(
            ItemStack::new(1, 65),
            Err(ServerError::InvalidAction(_))
        ));
        assert!(matches!(
            ItemStack::new(1, i32::MAX),
            Err(ServerError::InvalidAction(_))
        ));
        assert!(matches!(
            ItemStack::new(-1, 1),
            Err(ServerError::InvalidAction(_))
        ));
        assert_eq!(
            ItemStack::new(0, 64).expect("air is empty"),
            ItemStack::EMPTY
        );
        assert_eq!(ItemStack::new(1, 0).expect("zero count"), ItemStack::EMPTY);
        assert_eq!(
            ItemStack::new(1, -5).expect("negative count"),
            ItemStack::EMPTY
        );
    }

    #[test]
    fn empty_never_exposes_an_item_id() {
        assert!(ItemStack::EMPTY.is_empty());
        assert_eq!(ItemStack::EMPTY.item_id(), None);
        assert_eq!(ItemStack::EMPTY.count(), 0);
        let air = ItemStack::new(0, 0).expect("air");
        assert_eq!(air.item_id(), None);
        assert!(air.is_empty());
    }

    #[test]
    fn split_boundaries() {
        let mut stack = ItemStack::new(1, 10).expect("stack");
        assert!(stack.split(0).is_empty(), "splitting 0 takes nothing");
        assert_eq!(stack.count(), 10);
        let all = stack.split(10);
        assert_eq!(all.count(), 10);
        assert_eq!(all.item_id(), Some(1));
        assert!(stack.is_empty(), "splitting everything empties the source");

        let mut stack = ItemStack::new(1, 10).expect("stack");
        assert_eq!(
            stack.split(999).count(),
            10,
            "cannot take more than present"
        );
        assert!(stack.is_empty());
        assert!(ItemStack::new(1, 10).expect("stack").split(-3).is_empty());
        let mut empty = ItemStack::EMPTY;
        assert!(empty.split(5).is_empty());
    }

    #[test]
    fn grow_and_shrink_report_the_shortfall() {
        let mut stack = ItemStack::new(1, 60).expect("stack");
        assert_eq!(stack.grow(10), 6, "only 4 fit into 64");
        assert_eq!(stack.count(), 64);
        assert_eq!(stack.grow(1), 1, "a full stack has no room");
        assert_eq!(stack.grow(0), 0);
        assert_eq!(stack.shrink(64), 64);
        assert!(stack.is_empty());
        assert_eq!(stack.shrink(5), 0, "nothing left to remove");
        assert_eq!(stack.grow(5), 5, "growing an empty stack does nothing");
        assert_eq!(ItemStack::new(1, 5).expect("stack").shrink(-1), 0);
    }

    #[test]
    fn grow_capped_honours_a_per_item_limit() {
        // Item id 1013 is `minecraft:bucket` (16); element limits are what the
        // inventory passes in from `StackSizeTable`.
        let mut buckets = ItemStack::new(1013, 10).expect("buckets");
        assert_eq!(buckets.grow_capped(10, 16), 4, "only 6 fit into 16");
        assert_eq!(buckets.count(), 16);
        let mut tools = ItemStack::new(942, 1).expect("sword");
        assert_eq!(tools.grow_capped(1, 1), 1, "a tool is a stack of one");
        assert_eq!(tools.count(), 1);
    }

    #[test]
    fn merge_only_merges_the_same_item() {
        let mut target = ItemStack::new(1, 60).expect("stone");
        let mut source = ItemStack::new(1, 20).expect("stone");
        // Only 4 fit; the other 16 stay in the source and are also the return
        // value, so a caller that ignores it still cannot lose track of them.
        assert_eq!(target.merge(&mut source), 16);
        assert_eq!(target.count(), 64);
        assert_eq!(source.count(), 16, "the leftover stays in the source");

        // A merge that fits entirely empties the source.
        let mut target = ItemStack::new(1, 60).expect("stone");
        let mut source = ItemStack::new(1, 4).expect("stone");
        assert_eq!(target.merge(&mut source), 0);
        assert_eq!(target.count(), 64);
        assert!(source.is_empty());

        let mut other = ItemStack::new(2, 5).expect("dirt");
        assert_eq!(target.merge(&mut other), 5, "different items never merge");
        assert_eq!(other.count(), 5);

        // An empty stack takes the whole stack: this is the "place into an empty
        // slot" case, so it must move the items rather than refuse an empty
        // destination.
        let mut empty = ItemStack::EMPTY;
        let mut source = ItemStack::new(1, 3).expect("stone");
        assert_eq!(empty.merge(&mut source), 0);
        assert_eq!(empty.count(), 3);
        assert!(source.is_empty());

        // Two empties merge nothing.
        let mut empty = ItemStack::EMPTY;
        let mut also_empty = ItemStack::EMPTY;
        assert_eq!(empty.merge(&mut also_empty), 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn same_item_ignores_empties() {
        let stone = ItemStack::new(1, 1).expect("stone");
        assert!(stone.same_item(&ItemStack::new(1, 64).expect("stone")));
        assert!(!stone.same_item(&ItemStack::new(2, 1).expect("dirt")));
        assert!(!stone.same_item(&ItemStack::EMPTY));
        assert!(!ItemStack::EMPTY.same_item(&ItemStack::EMPTY));
    }

    #[test]
    fn validity_tracks_the_constructor_limits() {
        assert!(ItemStack::new(1, 64).expect("stack").is_valid());
        assert!(ItemStack::EMPTY.is_valid());
        assert_eq!(ItemStack::default(), ItemStack::EMPTY);
        assert_eq!(
            ItemStack::new(1, 3).expect("stack").to_string(),
            "item 1 x3"
        );
        assert_eq!(ItemStack::EMPTY.to_string(), "empty");
    }

    #[test]
    fn take_empties_the_slot() {
        let mut stack = ItemStack::new(1, 5).expect("stone");
        let taken = stack.take();
        assert_eq!(taken.count(), 5);
        assert!(stack.is_empty());
        assert_eq!(stack.take(), ItemStack::EMPTY);
    }
}
