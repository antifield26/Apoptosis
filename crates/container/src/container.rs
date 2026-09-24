//! Containers: a bounded, ordered list of item stacks (P06-03, P06-07).
//!
//! A [`Container`] is deliberately dumb: it stores stacks, remembers which slots
//! changed, and reports its total item count. It does **not** enforce stack limits
//! (that needs the item registry, so it lives in [`crate::Menu`]) and it does not
//! know what a click is. Keeping it dumb is what makes the transaction rules
//! testable: conservation is a property of the *menu*, and it can be checked by
//! summing [`Container::total_items`] over every container plus the cursor.

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::ItemStack;
use std::collections::BTreeSet;

/// Largest container this build can represent (a double chest).
pub const MAX_CONTAINER_SLOTS: usize = 54;

/// What a container *is*.
///
/// The kind decides two things and nothing else: how many slots a fresh one has,
/// and how [`crate::Menu`] resolves a slot's role. Behaviour that needs the item
/// registry, a world or a tick count lives one layer up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContainerKind {
    /// A player's inventory: 36 main slots (hotbar first, matching
    /// [`mc_entity::inventory::PlayerInventory`]'s storage order), then 4 armour,
    /// then the offhand.
    Player,
    /// A block container with uniform slots: chest, barrel, dispenser, hopper.
    Generic,
    /// A crafting grid plus its result slot.
    Crafting,
    /// A furnace: input, fuel, output.
    Furnace,
}

impl ContainerKind {
    /// Slots a fresh container of this kind holds, or `None` for a sized kind.
    #[must_use]
    pub const fn default_slots(self) -> Option<usize> {
        match self {
            Self::Player => Some(41),
            Self::Generic => None,
            Self::Crafting => Some(10),
            Self::Furnace => Some(3),
        }
    }

    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Generic => "generic",
            Self::Crafting => "crafting",
            Self::Furnace => "furnace",
        }
    }

    /// Whether this kind can be persisted as a block entity.
    ///
    /// A player inventory belongs to `playerdata`, a crafting grid belongs to
    /// nobody (it is transient), and the rest are block entities.
    #[must_use]
    pub const fn is_block_entity(self) -> bool {
        matches!(self, Self::Generic | Self::Furnace)
    }
}

impl std::fmt::Display for ContainerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The role a slot plays, which decides who may take from it and who may put into
/// it.
///
/// This is the model behind "the client cannot put items into a furnace's output
/// slot" and "the client cannot take the crafting result without crafting".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotRole {
    /// Ordinary storage: take and put freely.
    Storage,
    /// Armour slot: only armour belongs here, and equipping is not modelled, so
    /// only a direct placement is allowed.
    Armor,
    /// The offhand slot.
    Offhand,
    /// A crafting-grid slot: take and put freely, and changing it recomputes the
    /// result.
    CraftingInput,
    /// The crafting result: **take only**. It is computed from the grid, so a
    /// client that could place into it would be creating items.
    CraftingResult,
    /// A furnace's input slot.
    FurnaceInput,
    /// A furnace's fuel slot.
    FurnaceFuel,
    /// A furnace's output: **take only**, for the same reason as the crafting
    /// result.
    FurnaceOutput,
}

impl SlotRole {
    /// Whether a client may place a stack into a slot with this role.
    #[must_use]
    pub const fn may_place(self) -> bool {
        !matches!(self, Self::CraftingResult | Self::FurnaceOutput)
    }

    /// Whether a client may take a stack out of a slot with this role.
    ///
    /// Everything can be taken from; the asymmetric case is placement.
    #[must_use]
    pub const fn may_pickup(self) -> bool {
        true
    }

    /// Whether a hopper may pull a stack out of a slot with this role.
    ///
    /// Hoppers extract furnace output only: the input and fuel slots refuse,
    /// everything else allows. That is vanilla's sided-inventory rule on the
    /// down face (pumpkin `HopperBlockEntity::suck_in_items` walks every slot
    /// of the container above, and only slot 2 of a furnace is a legal take).
    /// Separate from [`Self::may_pickup`] because the client *may* take fuel
    /// back out by hand — the refusal is the hopper's, not the slot's.
    #[must_use]
    pub const fn may_hopper_extract(self) -> bool {
        !matches!(self, Self::FurnaceInput | Self::FurnaceFuel)
    }

    /// Whether this role is computed by the server rather than stored by a player.
    #[must_use]
    pub const fn is_computed(self) -> bool {
        matches!(self, Self::CraftingResult | Self::FurnaceOutput)
    }

    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Armor => "armor",
            Self::Offhand => "offhand",
            Self::CraftingInput => "crafting_input",
            Self::CraftingResult => "crafting_result",
            Self::FurnaceInput => "furnace_input",
            Self::FurnaceFuel => "furnace_fuel",
            Self::FurnaceOutput => "furnace_output",
        }
    }
}

/// A fixed-size list of item stacks with change tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    kind: ContainerKind,
    slots: Vec<ItemStack>,
    changed: BTreeSet<u16>,
}

impl Container {
    /// A container of `size` empty slots.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `size` is 0 or above
    /// [`MAX_CONTAINER_SLOTS`]: a container size is chosen by the server, so an
    /// impossible one is a programming error rather than hostile input.
    pub fn new(kind: ContainerKind, size: usize) -> ServerResult<Self> {
        if size == 0 || size > MAX_CONTAINER_SLOTS {
            return Err(ServerError::Invariant(format!(
                "container of {size} slots is outside 1..={MAX_CONTAINER_SLOTS}"
            )));
        }
        Ok(Self {
            kind,
            slots: vec![ItemStack::EMPTY; size],
            changed: BTreeSet::new(),
        })
    }

    /// A container of this kind's default size.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] for a sized kind with no default.
    pub fn with_default_size(kind: ContainerKind) -> ServerResult<Self> {
        let Some(size) = kind.default_slots() else {
            return Err(ServerError::Invariant(format!(
                "{kind} needs an explicit size"
            )));
        };
        Self::new(kind, size)
    }

    /// This container's kind.
    #[must_use]
    pub const fn kind(&self) -> ContainerKind {
        self.kind
    }

    /// How many slots it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether it has no slots. Never true for a constructed container; present
    /// so the type satisfies the `len`/`is_empty` lint pair honestly.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The stack in `index`, or empty for an out-of-range index.
    ///
    /// Indexing a container is not an error path here: a caller reading past the
    /// end is asking about a slot that does not exist, and `EMPTY` is the honest
    /// answer. Mutation is the fallible direction.
    #[must_use]
    pub fn get(&self, index: usize) -> ItemStack {
        self.slots.get(index).cloned().unwrap_or(ItemStack::EMPTY)
    }

    /// Overwrite `index`.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `index` is out of range — this is the
    /// path a menu slot mapping reaches, so it must be checked rather than
    /// silently ignored.
    pub fn set(&mut self, index: usize, stack: ItemStack) -> ServerResult<()> {
        let slot = self
            .slots
            .get_mut(index)
            .ok_or_else(|| ServerError::InvalidAction(format!("slot {index} is out of range")))?;
        if *slot != stack {
            *slot = stack;
            // `index` came from a `Vec` lookup, so it fits a `u16` for any
            // representable container (`MAX_CONTAINER_SLOTS`).
            self.changed.insert(index as u16);
        }
        Ok(())
    }

    /// Every slot, in order.
    #[must_use]
    pub fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    /// Slots whose contents changed since the last [`Container::clear_changed`].
    #[must_use]
    pub const fn changed(&self) -> &BTreeSet<u16> {
        &self.changed
    }

    /// Forget which slots changed (after telling the client).
    pub fn clear_changed(&mut self) {
        self.changed.clear();
    }

    /// Mark a slot as changed without altering it.
    ///
    /// Used when the server recomputes a slot (a crafting result, a furnace
    /// output) to a value that happens to be identical, so the client is still
    /// told and cannot drift.
    pub fn mark_changed(&mut self, index: usize) {
        if index < self.slots.len() {
            self.changed.insert(index as u16);
        }
    }

    /// Total number of items across every slot.
    ///
    /// `i64` so summing many containers cannot overflow and silently make a
    /// conservation check pass. Negative values are impossible.
    #[must_use]
    pub fn total_items(&self) -> i64 {
        self.slots
            .iter()
            .map(|stack| i64::from(stack.count()))
            .sum()
    }

    /// Index and stack of every non-empty slot, in order.
    pub fn non_empty(&self) -> impl Iterator<Item = (usize, ItemStack)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(index, stack)| (index, stack.clone()))
    }

    /// Swap two of this container's slots.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when either index is out of range.
    pub fn swap(&mut self, a: usize, b: usize) -> ServerResult<()> {
        if a >= self.slots.len() || b >= self.slots.len() {
            return Err(ServerError::InvalidAction(format!(
                "cannot swap slots {a} and {b} in a {}-slot container",
                self.slots.len()
            )));
        }
        if a == b {
            return Ok(());
        }
        self.slots.swap(a, b);
        self.changed.insert(a as u16);
        self.changed.insert(b as u16);
        Ok(())
    }

    /// Empty every slot.
    pub fn clear(&mut self) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if !slot.is_empty() {
                *slot = ItemStack::EMPTY;
                self.changed.insert(index as u16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Container, ContainerKind, MAX_CONTAINER_SLOTS, SlotRole};
    use mc_entity::stack::ItemStack;

    const STONE: i32 = 1;
    const DIRT: i32 = 2;

    fn stack(item: i32, count: i32) -> ItemStack {
        ItemStack::new(item, count).expect("valid stack")
    }

    #[test]
    fn sizes_are_bounded_and_kinds_have_defaults() {
        assert!(Container::new(ContainerKind::Generic, 0).is_err());
        assert!(Container::new(ContainerKind::Generic, MAX_CONTAINER_SLOTS + 1).is_err());
        assert!(Container::new(ContainerKind::Generic, MAX_CONTAINER_SLOTS).is_ok());
        assert_eq!(
            Container::with_default_size(ContainerKind::Player)
                .expect("player")
                .len(),
            41
        );
        assert_eq!(
            Container::with_default_size(ContainerKind::Furnace)
                .expect("furnace")
                .len(),
            3
        );
        assert_eq!(
            Container::with_default_size(ContainerKind::Crafting)
                .expect("crafting")
                .len(),
            10
        );
        // A generic container has no default size and says so.
        assert!(Container::with_default_size(ContainerKind::Generic).is_err());
    }

    #[test]
    fn kind_names_and_roles_are_consistent() {
        for kind in [
            ContainerKind::Player,
            ContainerKind::Generic,
            ContainerKind::Crafting,
            ContainerKind::Furnace,
        ] {
            assert!(!kind.name().is_empty());
        }
        assert!(ContainerKind::Generic.is_block_entity());
        assert!(ContainerKind::Furnace.is_block_entity());
        assert!(!ContainerKind::Player.is_block_entity());
        assert!(!ContainerKind::Crafting.is_block_entity());

        // Only computed slots refuse placement.
        assert!(!SlotRole::CraftingResult.may_place());
        assert!(!SlotRole::FurnaceOutput.may_place());
        for role in [
            SlotRole::Storage,
            SlotRole::Armor,
            SlotRole::Offhand,
            SlotRole::CraftingInput,
            SlotRole::FurnaceInput,
            SlotRole::FurnaceFuel,
        ] {
            assert!(role.may_place(), "{role:?} should accept placement");
        }
        for role in [
            SlotRole::Storage,
            SlotRole::Armor,
            SlotRole::Offhand,
            SlotRole::CraftingInput,
            SlotRole::CraftingResult,
            SlotRole::FurnaceInput,
            SlotRole::FurnaceFuel,
            SlotRole::FurnaceOutput,
        ] {
            assert!(role.may_pickup(), "{role:?} should allow pickup");
            assert!(!role.name().is_empty());
        }
        assert!(SlotRole::CraftingResult.is_computed());
        assert!(SlotRole::FurnaceOutput.is_computed());
        assert!(!SlotRole::Storage.is_computed());
    }

    #[test]
    fn reading_out_of_range_is_empty_and_writing_is_an_error() {
        let mut container = Container::new(ContainerKind::Generic, 3).expect("container");
        assert!(container.get(99).is_empty());
        assert!(container.set(99, stack(STONE, 1)).is_err());
        // A rejected write must not have marked anything changed.
        assert!(container.changed().is_empty());
    }

    #[test]
    fn change_tracking_records_only_real_changes() {
        let mut container = Container::new(ContainerKind::Generic, 4).expect("container");
        container.set(1, stack(STONE, 5)).expect("set");
        assert_eq!(
            container.changed().iter().copied().collect::<Vec<_>>(),
            vec![1]
        );
        // Writing the same value again is not a change.
        container.set(1, stack(STONE, 5)).expect("set");
        assert_eq!(container.changed().len(), 1);
        container.clear_changed();
        container.set(1, stack(STONE, 5)).expect("set");
        assert!(
            container.changed().is_empty(),
            "identical write is not a change"
        );
        // But an explicit mark still records it.
        container.mark_changed(1);
        assert_eq!(
            container.changed().iter().copied().collect::<Vec<_>>(),
            vec![1]
        );
        // Marking out of range is a no-op, not a panic.
        container.mark_changed(99);
        assert_eq!(container.changed().len(), 1);
    }

    #[test]
    fn totals_and_iteration_agree() {
        let mut container = Container::new(ContainerKind::Generic, 4).expect("container");
        assert_eq!(container.total_items(), 0);
        container.set(0, stack(STONE, 64)).expect("set");
        container.set(2, stack(DIRT, 7)).expect("set");
        assert_eq!(container.total_items(), 71);
        let listed: Vec<usize> = container.non_empty().map(|(index, _)| index).collect();
        assert_eq!(listed, vec![0, 2], "iteration is in slot order");
        container.clear();
        assert_eq!(container.total_items(), 0);
        assert!(container.non_empty().next().is_none());
    }

    #[test]
    fn swap_moves_both_slots_and_marks_them() {
        let mut container = Container::new(ContainerKind::Generic, 3).expect("container");
        container.set(0, stack(STONE, 1)).expect("set");
        container.set(2, stack(DIRT, 2)).expect("set");
        container.clear_changed();
        container.swap(0, 2).expect("swap");
        assert_eq!(container.get(0), stack(DIRT, 2));
        assert_eq!(container.get(2), stack(STONE, 1));
        assert_eq!(
            container.changed().iter().copied().collect::<Vec<_>>(),
            vec![0, 2]
        );
        // Out-of-range swaps are refused; the same index is a no-op.
        assert!(container.swap(0, 9).is_err());
        assert!(container.swap(9, 0).is_err());
        container.clear_changed();
        container.swap(1, 1).expect("no-op swap");
        assert!(container.changed().is_empty());
    }

    #[test]
    fn total_items_cannot_overflow_for_a_full_double_chest() {
        // 54 slots of 64 items is 3456; the point is that the sum is an i64 so a
        // conservation check can never wrap.
        let mut container = Container::new(ContainerKind::Generic, 54).expect("container");
        for index in 0..54 {
            container.set(index, stack(STONE, 64)).expect("set");
        }
        assert_eq!(container.total_items(), 54 * 64);
    }
}
