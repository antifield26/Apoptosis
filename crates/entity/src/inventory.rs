//! The player inventory: 46 slots, and the client-facing container payload.
//!
//! ## Slot layout (vanilla `Inventory` + `PlayerInventory`)
//!
//! ```text
//!   0..=8    hotbar          (client slots 36..=44)
//!   9..=35   main inventory  (three rows of nine, client slots 9..=35)
//!  36..=39   armour          (boots, leggings, chestplate, helmet)
//!  40        offhand
//!  41..=44   crafting grid   (2x2, transient, client slots 1..=4)
//!  45        crafting result (transient, client slot 0)
//! ```
//!
//! Slots 0..=40 are **stored**; 41..=45 exist only in the window the client
//! sees, so [`PlayerInventory::set_slot`] rejects them with
//! [`ServerError::InvalidAction`]. [`PlayerInventory::to_container_payload`]
//! emits all 46 with 41..=45 empty.
//!
//! ## Hostile input
//!
//! Everything here is reachable from client packets: the hotbar index arrives
//! in `set_carried_item`, the hand id in `use_item`/`swing`, the slot index in
//! every click packet, and item counts arrive through those same paths. No
//! accessor panics, no index wraps, and every rejection is a typed error
//! (AGENTS.md section 10).
//!
//! ## What is *not* here
//!
//! No container transaction model: `click_container`, drag handling, shift-click
//! distribution, the cursor (carried) stack, crafting, and the
//! client-slot-vs-server-slot remapping of an open window belong to the
//! container system, not to player state (AGENTS.md section 3.3).

use crate::stack::{ItemStack, StackSizeTable};
use mc_core::error::{ServerError, ServerResult};
use mc_registry::ItemRegistry;

/// Number of hotbar slots.
pub const HOTBAR_SLOTS: usize = 9;

/// Highest valid hotbar index (0-based, as [`PlayerInventory::select`] takes it).
pub const HOTBAR_LAST: u8 = 8;

/// First main-inventory slot (three rows of nine, client slots 9..=35).
pub const MAIN_START: usize = 9;

/// Number of main-inventory slots (three rows of nine).
pub const MAIN_SLOTS: usize = 27;

/// First armour slot.
///
/// The stored order inside `ARMOR_START..ARMOR_START + ARMOR_SLOTS` is **boots,
/// leggings, chestplate, helmet**, which is the order the client's window lists
/// them in (menu indices 5..=8). Verified from the jar: `InventoryMenu`'s
/// constructor loops `SLOT_IDS = [FEET, LEGS, CHEST, HEAD]` starting at menu
/// index 5, and `InventoryMenu.ARMOR_SLOT_START = 5`.
pub const ARMOR_START: usize = 36;

/// Number of armour slots.
pub const ARMOR_SLOTS: usize = 4;

/// First armour slot **as the client's window numbers it** (`ARMOR_SLOT_START`).
///
/// Distinct from [`ARMOR_START`], which is our storage index. The two happen to
/// have the same sub-order, which is why the payload mapping is forward.
pub const ARMOR_MENU_START: usize = 5;

/// Offhand slot in this type's storage.
pub const OFFHAND_SLOT: usize = 40;

/// Highest slot index this type stores.
pub const LAST_STORED_SLOT: usize = OFFHAND_SLOT;

/// First crafting-grid slot (client slots 1..=4). Not stored, always empty.
pub const CRAFTING_START: usize = 41;

/// Crafting result slot in the container payload. Not stored, always empty.
pub const CRAFTING_RESULT_SLOT: usize = 45;

/// The offhand's index **in the client's window** (`container_set_content`).
pub const OFFHAND_MENU_SLOT: usize = 45;

/// Slots the client's player-inventory window (`window id 0`) contains.
pub const CONTAINER_SLOT_COUNT: usize = 46;

/// Window id of the player's own inventory, as sent by `container_set_content`.
pub const PLAYER_WINDOW_ID: u8 = 0;

/// Which hand a use/interact action refers to.
///
/// The ids are the vanilla `InteractionHand` ordinals: `MAIN_HAND = 0`,
/// `OFF_HAND = 1`. They arrive from the client as a raw `VarInt`, so
/// [`Hand::from_id`] returns `Option` and the caller turns `None` into an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hand {
    /// The main hand: the currently selected hotbar slot.
    Main,
    /// The offhand slot (40).
    Off,
}

impl Hand {
    /// Both hands, main first, for deterministic iteration.
    pub const ALL: [Self; 2] = [Self::Main, Self::Off];

    /// Vanilla `InteractionHand` ordinal (`MAIN_HAND = 0`, `OFF_HAND = 1`).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Main => 0,
            Self::Off => 1,
        }
    }

    /// Parse a vanilla `InteractionHand` ordinal.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Main),
            1 => Some(Self::Off),
            _ => None,
        }
    }
}

/// The 41 stored player slots plus the selected hotbar index.
///
/// Carries the resolved [`StackSizeTable`] so the per-item cap is applied on
/// every insertion: a client that claims 64 buckets in one slot is rejected, not
/// stored. The table is ~165 entries and immutable after resolution; a player map
/// can share one behind an `Arc` outside this type, or let each player own a copy
/// (~2.6 KiB). Owning a copy is deliberate simplicity here, not a measured
/// performance choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInventory {
    /// Slots 0..=40; 41..=45 are not stored.
    slots: [ItemStack; LAST_STORED_SLOT + 1],
    /// Selected hotbar index, `0..=8`.
    selected_hotbar: u8,
    /// Per-item stack limits, resolved from the item registry at startup.
    stack_sizes: StackSizeTable,
}

impl PlayerInventory {
    /// An empty inventory with hotbar slot 0 selected.
    #[must_use]
    pub fn new(stack_sizes: StackSizeTable) -> Self {
        Self {
            slots: [ItemStack::EMPTY; LAST_STORED_SLOT + 1],
            selected_hotbar: 0,
            stack_sizes,
        }
    }

    /// Number of stored slots (41).
    #[must_use]
    pub const fn stored_slots(&self) -> usize {
        LAST_STORED_SLOT + 1
    }

    /// The stack in a stored slot, or [`ItemStack::EMPTY`] when `index` is out
    /// of range.
    ///
    /// Total on purpose: reading a bogus index must not panic, and `EMPTY` is
    /// the honest answer for a slot this type does not have.
    #[must_use]
    pub fn slot(&self, index: usize) -> ItemStack {
        self.slots.get(index).copied().unwrap_or(ItemStack::EMPTY)
    }

    /// Write a stored slot.
    ///
    /// # Errors
    ///
    /// - [`ServerError::InvalidAction`] when `index > 40`, which includes the
    ///   transient crafting slots 41..=45 this type does not own; or
    /// - [`ServerError::InvalidAction`] when `stack` exceeds the item's maximum
    ///   stack size from the resolved table (inventory spoofing, AGENTS.md
    ///   section 10).
    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> ServerResult<()> {
        if index > LAST_STORED_SLOT {
            return Err(ServerError::InvalidAction(format!(
                "inventory slot {index} is out of range (0..={LAST_STORED_SLOT})"
            )));
        }
        let limit = self.limit_for(stack);
        if stack.count() > limit {
            return Err(ServerError::InvalidAction(format!(
                "slot {index}: item {} x{} exceeds the maximum stack size {limit}",
                stack.item_id().unwrap_or(0),
                stack.count()
            )));
        }
        self.slots[index] = stack;
        Ok(())
    }

    /// Selected hotbar index (`0..=8`).
    #[must_use]
    pub const fn selected_hotbar(&self) -> u8 {
        self.selected_hotbar
    }

    /// Select a hotbar slot.
    ///
    /// Directly client-controlled by `set_carried_item`, so an index outside
    /// `0..=8` is refused: it must never wrap, clamp silently or panic. This is
    /// an intentional divergence from vanilla, whose
    /// `ServerboundSetCarriedItemPacket` handler writes whatever byte it
    /// received into a nine-element array; refusing is the safe direction, and
    /// it makes every later read of the selection in range by construction.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `slot > 8`.
    pub fn select(&mut self, slot: u8) -> ServerResult<()> {
        if slot > HOTBAR_LAST {
            return Err(ServerError::InvalidAction(format!(
                "hotbar slot {slot} is out of range (0..={HOTBAR_LAST})"
            )));
        }
        self.selected_hotbar = slot;
        Ok(())
    }

    /// Set the selected hotbar index while loading persisted data.
    ///
    /// A `SelectedItemSlot` outside `0..=8` in a `playerdata` file is treated as
    /// "hotbar 0" rather than as a load failure: the rest of the file is still
    /// readable, and normalising one cosmetic field beats refusing the player's
    /// whole inventory.
    pub(crate) fn set_selected_hotbar_tolerant(&mut self, slot: i32) {
        self.selected_hotbar = u8::try_from(slot)
            .ok()
            .filter(|slot| *slot <= HOTBAR_LAST)
            .unwrap_or(0);
    }

    /// The stack in the main hand, i.e. the selected hotbar slot.
    #[must_use]
    pub fn selected_item(&self) -> ItemStack {
        self.slot(usize::from(self.selected_hotbar))
    }

    /// The stack in a hand: the selected hotbar slot for [`Hand::Main`], slot 40
    /// for [`Hand::Off`].
    #[must_use]
    pub fn held_item(&self, hand: Hand) -> ItemStack {
        match hand {
            Hand::Main => self.selected_item(),
            Hand::Off => self.slot(OFFHAND_SLOT),
        }
    }

    /// Take the stack out of a hand, leaving its slot empty.
    ///
    /// Infallible: both hands map to a stored slot (`0..=8` or `40`) by
    /// construction, and the selected index is validated by
    /// [`PlayerInventory::select`].
    #[must_use]
    pub fn take_held(&mut self, hand: Hand) -> ItemStack {
        let index = Self::hand_slot(hand, self.selected_hotbar);
        self.slots[index].take()
    }

    /// Write into a hand's slot, returning the stack it replaced.
    ///
    /// Infallible for the same reason as [`PlayerInventory::take_held`]. The
    /// replacement is **not** stack-size validated: a caller building a held
    /// stack has already gone through [`PlayerInventory::add_stack`] or
    /// [`PlayerInventory::set_slot`].
    pub fn replace_held(&mut self, hand: Hand, stack: ItemStack) -> ItemStack {
        let index = Self::hand_slot(hand, self.selected_hotbar);
        std::mem::replace(&mut self.slots[index], stack)
    }

    /// Insert a stack, merging into partial stacks of the same item first.
    ///
    /// Returns the leftover, exactly like vanilla's `Inventory.add` returning
    /// false when the stack could not be fully placed. The fill order is hotbar
    /// then main inventory, lowest index first, which keeps a given sequence of
    /// insertions reproducible (AGENTS.md section 3.6).
    ///
    /// Armour and offhand slots are deliberately skipped: vanilla fills those
    /// through right-click equip logic, which is a click-handler concern rather
    /// than an `add` concern. A stack larger than one slot holds is spread
    /// across slots rather than dropped.
    /// Remove and return every stored stack, leaving the inventory empty.
    ///
    /// The death-drop path uses this: the stacks become ground entities at the
    /// death position, and the later `respawn` call then finds nothing to drop
    /// a second time.
    #[must_use]
    pub fn drain_all(&mut self) -> Vec<ItemStack> {
        let mut out = Vec::new();
        for slot in &mut self.slots {
            if !slot.is_empty() {
                out.push(*slot);
                *slot = ItemStack::EMPTY;
            }
        }
        out
    }

    /// Insert `stack`, filling partial stacks first, and return what did not fit.
    pub fn add_stack(&mut self, mut stack: ItemStack) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }
        let limit = self.limit_for(stack);
        let candidates = (0..HOTBAR_SLOTS).chain(MAIN_START..ARMOR_START);
        // Pass 1: top up stacks of the same item. `merge_capped` is called on
        // the *slot*, so the slot is filled and `stack` keeps what did not fit.
        for index in candidates.clone() {
            if !self.slots[index].same_item(&stack) {
                continue;
            }
            let mut target = self.slots[index];
            target.merge_capped(&mut stack, limit);
            self.slots[index] = target;
            if stack.is_empty() {
                return ItemStack::EMPTY;
            }
        }
        // Pass 2: place what is left into the first empty slots, again filling
        // the slot from `stack`.
        for index in candidates {
            if !self.slots[index].is_empty() {
                continue;
            }
            let mut target = ItemStack::EMPTY;
            target.merge_capped(&mut stack, limit);
            self.slots[index] = target;
            if stack.is_empty() {
                return ItemStack::EMPTY;
            }
        }
        stack
    }

    /// Remove up to `count` items from a stored slot.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `index > 40` or `count <= 0`. An
    /// empty slot is not an error: it simply yields [`ItemStack::EMPTY`].
    pub fn remove_from_slot(&mut self, index: usize, count: i32) -> ServerResult<ItemStack> {
        if index > LAST_STORED_SLOT {
            return Err(ServerError::InvalidAction(format!(
                "inventory slot {index} is out of range (0..={LAST_STORED_SLOT})"
            )));
        }
        if count <= 0 {
            return Err(ServerError::InvalidAction(format!(
                "cannot remove {count} items from slot {index}"
            )));
        }
        Ok(self.slots[index].split(count))
    }

    /// The 46 stacks the client's player-inventory window (`window id 0`)
    /// expects, in the order `container_set_content` sends them.
    ///
    /// Index map, verified against the official jar rather than recalled:
    /// `0` = crafting result, `1..=4` = the 2x2 crafting grid, `5..=8` = armour,
    /// `9..=35` = main inventory, `36..=44` = hotbar, `45` = offhand.
    ///
    /// The armour sub-order is the non-obvious part. `javap -c` on
    /// `net.minecraft.world.inventory.InventoryMenu` shows the constructor looping
    /// `i = 0..3` over `SLOT_IDS = [FEET, LEGS, CHEST, HEAD]` and adding
    /// `ArmorSlot(inventory, owner, SLOT_IDS[i], 39 - i, ..)`, so
    /// **menu index `5 + i` holds equipment slot `SLOT_IDS[i]`**:
    ///
    /// | menu index | equipment slot | this type's stored index |
    /// |---|---|---|
    /// | 5 | FEET (boots) | 36 |
    /// | 6 | LEGS (leggings) | 37 |
    /// | 7 | CHEST (chestplate) | 38 |
    /// | 8 | HEAD (helmet) | 39 |
    ///
    /// The `InventoryMenu` constants agree: `ARMOR_SLOT_START = 5`,
    /// `ARMOR_SLOT_END = 9`, `USE_ROW_SLOT_START = 36` (hotbar) and the offhand is
    /// a separate `InventoryMenu$1` slot at menu index 45.
    ///
    /// Audit 02 read this as an inverted permutation; the reversal would in fact
    /// have introduced the bug it reported, because this type stores boots first
    /// (`ARMOR_START`) while the menu lists them first as well (`5`). The mapping
    /// below is therefore forward, and
    /// `the_container_permutation_is_the_documented_one` locks it.
    ///
    /// The five transient slots at `0` and `1..=4` are **always empty**: this
    /// type does not model a crafting grid, so a window opened with this payload
    /// shows an empty grid and an empty result. Sending an empty result is
    /// correct-but-incomplete; crafting is unimplemented (AGENTS.md section
    /// 3.3), not silently faked.
    #[must_use]
    pub fn to_container_payload(&self) -> Vec<ItemStack> {
        let mut payload = vec![ItemStack::EMPTY; CONTAINER_SLOT_COUNT];
        // Hotbar -> client 36..=44.
        for index in 0..HOTBAR_SLOTS {
            payload[MAIN_START + MAIN_SLOTS + index] = self.slots[index];
        }
        // Main inventory -> client 9..=35 (the same indices).
        payload[MAIN_START..ARMOR_START].copy_from_slice(&self.slots[MAIN_START..ARMOR_START]);
        // Armour: stored boots..helmet (36..=39) -> menu 5..=8 in the same order,
        // because the jar's `SLOT_IDS` loop starts at FEET. See the doc comment
        // above for the bytecode this comes from. Offhand (40) -> menu 45.
        payload[ARMOR_MENU_START..ARMOR_MENU_START + ARMOR_SLOTS]
            .copy_from_slice(&self.slots[ARMOR_START..ARMOR_START + ARMOR_SLOTS]);
        payload[OFFHAND_MENU_SLOT] = self.slots[OFFHAND_SLOT];
        payload
    }

    /// The resolved stack-size table this inventory validates against.
    #[must_use]
    pub const fn stack_sizes(&self) -> &StackSizeTable {
        &self.stack_sizes
    }

    /// Maximum stack size to apply to `stack` (1 for an empty stack, which is
    /// never inserted anyway).
    fn limit_for(&self, stack: ItemStack) -> i32 {
        stack
            .item_id()
            .map_or(1, |id| self.stack_sizes.max_stack_size(id))
    }

    /// Slot index a hand refers to. In range by construction because
    /// `selected_hotbar <= HOTBAR_LAST` is enforced by [`PlayerInventory::select`]
    /// and by [`PlayerInventory::set_selected_hotbar_tolerant`].
    const fn hand_slot(hand: Hand, selected_hotbar: u8) -> usize {
        match hand {
            Hand::Main => selected_hotbar as usize,
            Hand::Off => OFFHAND_SLOT,
        }
    }
}

/// Build an empty inventory from an [`ItemRegistry`] by resolving the stack-size
/// table.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the stack-size table names an item the
/// registry does not contain.
pub fn inventory_for_registry(items: &ItemRegistry) -> ServerResult<PlayerInventory> {
    Ok(PlayerInventory::new(StackSizeTable::resolve(items)?))
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
        ARMOR_MENU_START, ARMOR_SLOTS, ARMOR_START, CONTAINER_SLOT_COUNT, CRAFTING_RESULT_SLOT,
        CRAFTING_START, HOTBAR_LAST, HOTBAR_SLOTS, Hand, MAIN_START, OFFHAND_MENU_SLOT,
        OFFHAND_SLOT, PLAYER_WINDOW_ID, PlayerInventory, inventory_for_registry,
    };
    use crate::stack::{ItemStack, StackSizeTable};
    use mc_core::error::ServerError;
    use mc_registry::ItemRegistry;
    use std::path::Path;

    /// Item ids used by the tests, each checked against the fixture below.
    const STONE: i32 = 1;
    const OTHER_BLOCK: i32 = 2;
    const DIAMOND_PICKAXE: i32 = 939;
    const BUCKET: i32 = 1013;

    fn registry() -> ItemRegistry {
        ItemRegistry::load(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/test-support/fixtures/registry/items.tsv"),
        )
        .expect("items fixture loads")
    }

    fn empty_inventory() -> PlayerInventory {
        let items = registry();
        assert_eq!(items.id("minecraft:stone").expect("stone"), STONE);
        assert_eq!(items.id("minecraft:granite").expect("granite"), OTHER_BLOCK);
        assert_eq!(items.id("minecraft:bucket").expect("bucket"), BUCKET);
        assert_eq!(
            items.id("minecraft:diamond_pickaxe").expect("pickaxe"),
            DIAMOND_PICKAXE
        );
        inventory_for_registry(&items).expect("table resolves")
    }

    fn stack(item_id: i32, count: i32) -> ItemStack {
        ItemStack::new(item_id, count).expect("legal stack")
    }

    #[test]
    fn slot_bounds_are_rejected() {
        let mut inventory = empty_inventory();
        for bad in [41usize, 45, 46, 999, usize::MAX] {
            let err = inventory
                .set_slot(bad, stack(STONE, 1))
                .expect_err("out of range");
            assert!(matches!(err, ServerError::InvalidAction(_)), "{err:?}");
            let err = inventory
                .remove_from_slot(bad, 1)
                .expect_err("out of range");
            assert!(matches!(err, ServerError::InvalidAction(_)), "{err:?}");
            assert!(
                inventory.slot(bad).is_empty(),
                "a rejected slot stays empty and does not panic"
            );
        }
        for good in [0usize, 8, 9, 35, 36, 40] {
            inventory
                .set_slot(good, stack(STONE, 1))
                .expect("stored slot");
            assert_eq!(inventory.slot(good).count(), 1);
        }
        for transient in CRAFTING_START..=CRAFTING_RESULT_SLOT {
            assert!(
                inventory.set_slot(transient, stack(STONE, 1)).is_err(),
                "crafting slot {transient} is not player state"
            );
        }
    }

    #[test]
    fn hostile_stack_sizes_are_rejected_per_item() {
        let mut inventory = empty_inventory();
        // A bucket really holds 16: the constructor and the slot agree.
        inventory.set_slot(0, stack(BUCKET, 16)).expect("legal");
        assert_eq!(inventory.slot(0).count(), 16);
        // A tool really holds one.
        inventory
            .set_slot(1, stack(DIAMOND_PICKAXE, 1))
            .expect("legal");
        // Blocks keep the vanilla default.
        inventory.set_slot(2, stack(STONE, 64)).expect("legal");
        // 65 is beyond every legal stack, so the stack itself cannot exist: the
        // hostile count is refused at construction, before a slot sees it.
        assert!(matches!(
            ItemStack::new(STONE, 65),
            Err(ServerError::InvalidAction(_))
        ));
        // An oversized stack cannot be built through the public API, so the
        // inventory's per-item check is exercised against a hand-built stack
        // through a test-only accessor. This is the "spoofed 64 buckets" case.
        let spoofed = ItemStack::from_raw_parts(BUCKET, 65);
        assert!(
            !spoofed.is_valid(),
            "the raw accessor must be able to build what the constructor refuses"
        );
        let err = inventory
            .set_slot(3, spoofed)
            .expect_err("the table says 16");
        assert!(matches!(err, ServerError::InvalidAction(_)), "{err:?}");
        assert!(
            inventory.slot(3).is_empty(),
            "a rejected stack must not be stored"
        );
    }

    #[test]
    fn hotbar_selection_bounds_are_rejected() {
        let mut inventory = empty_inventory();
        for good in 0..=HOTBAR_LAST {
            inventory.select(good).expect("legal hotbar index");
            assert_eq!(inventory.selected_hotbar(), good);
        }
        for bad in [9u8, 10, 100, u8::MAX] {
            let err = inventory.select(bad).expect_err("out of range");
            assert!(matches!(err, ServerError::InvalidAction(_)), "{err:?}");
            assert_eq!(
                inventory.selected_hotbar(),
                HOTBAR_LAST,
                "a rejected selection does not change state"
            );
        }
    }

    #[test]
    fn held_item_follows_the_selection_and_the_hand() {
        let mut inventory = empty_inventory();
        inventory.set_slot(3, stack(STONE, 5)).expect("stone");
        inventory
            .set_slot(OFFHAND_SLOT, stack(BUCKET, 1))
            .expect("bucket");
        assert!(inventory.held_item(Hand::Main).is_empty());
        inventory.select(3).expect("select 3");
        assert_eq!(inventory.held_item(Hand::Main), stack(STONE, 5));
        assert_eq!(inventory.held_item(Hand::Off), stack(BUCKET, 1));
        assert_eq!(inventory.selected_item(), stack(STONE, 5));

        let taken = inventory.take_held(Hand::Main);
        assert_eq!(taken, stack(STONE, 5));
        assert!(inventory.slot(3).is_empty());
        let taken = inventory.take_held(Hand::Off);
        assert_eq!(taken, stack(BUCKET, 1));
        assert!(inventory.slot(OFFHAND_SLOT).is_empty());
        assert!(inventory.take_held(Hand::Off).is_empty());
    }

    #[test]
    fn hand_ids_match_vanilla_and_reject_anything_else() {
        assert_eq!(Hand::Main.id(), 0);
        assert_eq!(Hand::Off.id(), 1);
        assert_eq!(Hand::from_id(0), Some(Hand::Main));
        assert_eq!(Hand::from_id(1), Some(Hand::Off));
        for bad in 2..=255u8 {
            assert_eq!(Hand::from_id(bad), None, "hand {bad} is not a hand");
        }
        assert_eq!(Hand::ALL, [Hand::Main, Hand::Off]);
    }

    #[test]
    fn add_stack_fills_partial_stacks_before_empty_ones() {
        let mut inventory = empty_inventory();
        inventory.set_slot(9, stack(STONE, 60)).expect("partial");
        inventory
            .set_slot(10, stack(OTHER_BLOCK, 64))
            .expect("full, wrong item");
        let leftover = inventory.add_stack(stack(STONE, 10));
        assert!(
            leftover.is_empty(),
            "all 10 fit: 4 into slot 9, 6 into slot 0"
        );
        assert_eq!(inventory.slot(9).count(), 64);
        assert_eq!(
            inventory.slot(10).count(),
            64,
            "a different item is never overwritten"
        );
        assert_eq!(inventory.slot(0).count(), 6, "hotbar 0 is the first empty");
        assert_eq!(
            inventory.slot(1).count(),
            0,
            "and nothing beyond that was touched"
        );
    }

    #[test]
    fn add_stack_returns_the_leftover_that_does_not_fit() {
        // The partial stack comes first: 4 of the 10 fit into slot 9 and the
        // other 6 go to the first empty slot.
        let mut inventory = empty_inventory();
        inventory.set_slot(9, stack(STONE, 60)).expect("partial");
        assert!(inventory.add_stack(stack(STONE, 10)).is_empty());
        assert_eq!(inventory.slot(9).count(), 64);
        assert_eq!(inventory.slot(0).count(), 6);

        // A full inventory holds nothing more.
        let mut inventory = empty_inventory();
        for index in (0..=HOTBAR_LAST as usize).chain(MAIN_START..36) {
            inventory.set_slot(index, stack(STONE, 64)).expect("full");
        }
        let leftover = inventory.add_stack(stack(STONE, 30));
        assert_eq!(leftover.count(), 30, "a full inventory holds nothing more");
        // Freeing slot 20 leaves 10 items there with room for 54, so all 30 fit
        // and nothing is left over.
        let removed = inventory.remove_from_slot(20, 54).expect("makes room");
        assert_eq!(removed.count(), 54);
        assert_eq!(inventory.slot(20).count(), 10);
        let leftover = inventory.add_stack(stack(STONE, 30));
        assert!(leftover.is_empty(), "the freed slot absorbed all 30");
        assert_eq!(inventory.slot(20).count(), 40);
    }

    #[test]
    fn add_stack_spreads_a_stack_larger_than_one_slot() {
        let mut inventory = empty_inventory();
        // A full stack lands whole in the first empty slot.
        let leftover = inventory.add_stack(stack(STONE, 64));
        assert!(leftover.is_empty());
        assert_eq!(inventory.slot(0).count(), 64);
        // And a second one lands in the next slot, not on top of the first.
        let leftover = inventory.add_stack(stack(STONE, 64));
        assert!(leftover.is_empty());
        assert_eq!(inventory.slot(0).count(), 64);
        assert_eq!(inventory.slot(1).count(), 64);
    }

    #[test]
    fn remove_from_slot_reports_shortfalls_and_rejects_hostile_counts() {
        let mut inventory = empty_inventory();
        inventory.set_slot(4, stack(STONE, 10)).expect("stone");
        assert_eq!(
            inventory.remove_from_slot(4, 4).expect("removes").count(),
            4
        );
        assert_eq!(inventory.slot(4).count(), 6);
        assert_eq!(
            inventory.remove_from_slot(4, 100).expect("clamps").count(),
            6,
            "removing more than present yields the real amount"
        );
        assert!(inventory.slot(4).is_empty());
        assert_eq!(
            inventory
                .remove_from_slot(4, 1)
                .expect("empty slot")
                .count(),
            0
        );
        for bad in [0, -1, i32::MIN] {
            assert!(
                inventory.remove_from_slot(4, bad).is_err(),
                "count {bad} must be rejected"
            );
        }
    }

    #[test]
    fn container_payload_matches_the_client_window() {
        let mut inventory = empty_inventory();
        inventory.set_slot(0, stack(STONE, 1)).expect("hotbar 0");
        inventory.set_slot(8, stack(STONE, 2)).expect("hotbar 8");
        inventory
            .set_slot(MAIN_START, stack(OTHER_BLOCK, 3))
            .expect("main 9");
        inventory
            .set_slot(35, stack(OTHER_BLOCK, 4))
            .expect("main 35");
        inventory
            .set_slot(36, stack(BUCKET, 1))
            .expect("boots slot");
        inventory
            .set_slot(39, stack(BUCKET, 2))
            .expect("helmet slot");
        inventory
            .set_slot(OFFHAND_SLOT, stack(DIAMOND_PICKAXE, 1))
            .expect("offhand");

        let payload = inventory.to_container_payload();
        assert_eq!(payload.len(), CONTAINER_SLOT_COUNT);
        assert_eq!(payload.len(), 46, "the client window is 46 slots");
        assert_eq!(PLAYER_WINDOW_ID, 0, "player inventory is window 0");
        // Crafting grid + result are exposed but empty, and say so.
        assert_eq!(payload[0], ItemStack::EMPTY, "crafting result");
        assert_eq!(&payload[1..=4], &[ItemStack::EMPTY; 4], "2x2 grid");
        // Armour: client 5..=8 in the same order the slots are stored, i.e.
        // boots, leggings, chestplate, helmet.
        assert_eq!(payload[5], stack(BUCKET, 1), "boots");
        assert_eq!(payload[6], ItemStack::EMPTY, "no leggings stored");
        assert_eq!(payload[7], ItemStack::EMPTY, "no chestplate stored");
        assert_eq!(payload[8], stack(BUCKET, 2), "helmet");
        // Main inventory: client slot 9..=35 is the same index.
        assert_eq!(payload[MAIN_START], stack(OTHER_BLOCK, 3), "main slot 9");
        assert_eq!(payload[35], stack(OTHER_BLOCK, 4), "main slot 35");
        // Hotbar: client 36..=44.
        assert_eq!(payload[36], stack(STONE, 1), "hotbar 0");
        assert_eq!(payload[44], stack(STONE, 2), "hotbar 8");
        assert!(payload[43].is_empty(), "an untouched hotbar slot");
        // Offhand is last.
        assert_eq!(payload[45], stack(DIAMOND_PICKAXE, 1));
    }

    /// The permutation must be exactly the client's: an off-by-one would put a
    /// helmet on the player's feet, which the real client would then report back
    /// as a desync. Every stored slot is filled with a distinguishable stack and
    /// each payload index is checked by hand.
    #[test]
    fn the_container_permutation_is_the_documented_one() {
        let mut inventory = empty_inventory();
        for index in 0..inventory.stored_slots() {
            // Distinct per slot: stone for hotbar/main, granite elsewhere, and
            // counts encode the slot index so a mismatch is identifiable.
            let item = if (MAIN_START..ARMOR_START).contains(&index) {
                OTHER_BLOCK
            } else {
                STONE
            };
            let count = i32::try_from(index).expect("small") % 64 + 1;
            inventory.set_slot(index, stack(item, count)).expect("fits");
        }
        let payload = inventory.to_container_payload();
        assert!(payload[0].is_empty(), "crafting result is never stored");
        for (grid, stack) in payload.iter().enumerate().take(5).skip(1) {
            assert!(stack.is_empty(), "crafting grid slot {grid}");
        }
        // The expected menu index is spelled out per storage region rather than
        // recomputed with the implementation's own formula, so this test can fail:
        // a tautology would pass whatever the mapping did.
        for stored in 0..inventory.stored_slots() {
            let client = if stored < HOTBAR_SLOTS {
                // Hotbar 0..=8 -> menu 36..=44 (USE_ROW_SLOT_START = 36).
                36 + stored
            } else if stored < ARMOR_START {
                // Main inventory 9..=35 -> menu 9..=35 (INV_SLOT_START = 9, identity).
                stored
            } else if stored == OFFHAND_SLOT {
                // Offhand -> menu 45 (the dedicated InventoryMenu$1 slot).
                OFFHAND_MENU_SLOT
            } else {
                // Armour 36..=39 -> menu 5..=8, same sub-order (jar: SLOT_IDS starts
                // at FEET and ARMOR_SLOT_START = 5).
                ARMOR_MENU_START + (stored - ARMOR_START)
            };
            assert_eq!(
                payload[client],
                inventory.slot(stored),
                "stored slot {stored} must appear at menu index {client}"
            );
        }

        // The armour sub-order specifically: boots (stored 36) is menu 5 and the
        // helmet (stored 39) is menu 8. This is the pair Audit 02 read backwards.
        assert_eq!(
            payload[ARMOR_MENU_START],
            inventory.slot(ARMOR_START),
            "stored 36 (boots) belongs at menu 5, the FEET slot"
        );
        assert_eq!(
            payload[ARMOR_MENU_START + ARMOR_SLOTS - 1],
            inventory.slot(ARMOR_START + ARMOR_SLOTS - 1),
            "stored 39 (helmet) belongs at menu 8, the HEAD slot"
        );
    }

    #[test]
    fn an_empty_stack_table_keeps_the_vanilla_defaults() {
        // An empty table gives every item the vanilla default of 64, which is
        // wrong for buckets and tools; the server must resolve the real table.
        let mut inventory = PlayerInventory::new(StackSizeTable::default());
        inventory
            .set_slot(0, stack(BUCKET, 64))
            .expect("no table, default 64");
        assert_eq!(inventory.slot(0).count(), 64);
        assert!(inventory.stack_sizes().is_empty());
        let resolved = empty_inventory();
        assert_eq!(
            resolved.stack_sizes().len(),
            205,
            "the 26.1.2 exception table has 167 stack-1 and 38 stack-16 entries"
        );
    }

    #[test]
    fn an_inventory_round_trips_through_clone_and_replace_held() {
        let mut inventory = empty_inventory();
        inventory.select(2).expect("select");
        inventory.set_slot(2, stack(STONE, 7)).expect("stone");
        let mut copy = inventory.clone();
        assert_eq!(copy, inventory);
        let replaced = copy.replace_held(Hand::Main, stack(OTHER_BLOCK, 3));
        assert_eq!(replaced, stack(STONE, 7));
        assert_eq!(copy.slot(2), stack(OTHER_BLOCK, 3));
        assert_eq!(
            inventory.slot(2),
            stack(STONE, 7),
            "the original is untouched"
        );
        assert_eq!(copy.stored_slots(), 41);
    }
}
