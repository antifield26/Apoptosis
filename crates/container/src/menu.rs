//! The menu: slot mappings, validation and click application (P06-01, P06-02).
//!
//! A [`Menu`] is the server's model of "what window does this player have open".
//! It owns one or more [`Container`]s, maps each *menu slot* to a
//! `(container, slot)` pair with a [`SlotRole`], and owns the cursor stack.
//!
//! ## Validation order
//!
//! [`Menu::apply_click`] refuses in this order, and the order matters:
//!
//! 1. **Window id** — a click for another window is not ours.
//! 2. **Slot range** — against *this* menu's slot count.
//! 3. **State id** — a mismatch means the client is out of sync. The click is
//!    *not* applied; [`ClickOutcome::full_resync`] is set so the caller corrects
//!    the client. This is what makes a desynchronised client harmless instead of an
//!    exploiter: it cannot replay an old click against a view it no longer has.
//! 4. **Role and limit** — placement into a computed slot (crafting result,
//!    furnace output) is refused, and every write is clamped to the item's own
//!    maximum.
//!
//! Only after all four does anything mutate.
//!
//! ## Conservation by construction
//!
//! Two private helpers, [`Menu::extract`] and [`Menu::insert`], are the only way a
//! click handler moves items. `extract` removes at most what is there;
//! `insert` places at most what fits and **returns the rest**, which the caller must
//! account for. Because every handler either puts the remainder back or hands it to
//! the caller as a drop, conservation is a property of two functions rather than of
//! seven handlers — and the tests assert it directly by comparing
//! [`Menu::total_items`] before and after.

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::{ItemStack, StackSizeTable};
use std::collections::BTreeSet;

use crate::click::{Click, ClickType, DragStage, DragType, SwapTarget};
use crate::container::{Container, ContainerKind, SlotRole};

/// Which container slot a menu slot exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotMapping {
    /// Index into [`Menu::containers`].
    pub container: u8,
    /// Slot within that container.
    pub slot: u16,
    /// What the slot is for.
    pub role: SlotRole,
    /// Per-slot ceiling on top of the item's own maximum.
    pub max_stack: i32,
}

impl SlotMapping {
    /// A mapping for an ordinary storage slot.
    #[must_use]
    pub const fn storage(container: u8, slot: u16) -> Self {
        Self {
            container,
            slot,
            role: SlotRole::Storage,
            max_stack: mc_entity::stack::DEFAULT_MAX_STACK_SIZE,
        }
    }

    /// A mapping with an explicit role.
    #[must_use]
    pub const fn with_role(container: u8, slot: u16, role: SlotRole) -> Self {
        Self {
            container,
            slot,
            role,
            max_stack: mc_entity::stack::DEFAULT_MAX_STACK_SIZE,
        }
    }

    /// A mapping with an explicit role and slot ceiling.
    #[must_use]
    pub const fn with_limit(container: u8, slot: u16, role: SlotRole, max_stack: i32) -> Self {
        Self {
            container,
            slot,
            role,
            max_stack,
        }
    }

    /// Whether a client may place into this slot.
    #[must_use]
    pub const fn may_place(&self) -> bool {
        self.role.may_place()
    }

    /// Whether a client may take from this slot.
    #[must_use]
    pub const fn may_pickup(&self) -> bool {
        self.role.may_pickup()
    }
}

/// An inclusive-exclusive range of menu slots, used for shift-click routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRange {
    /// First menu slot.
    pub start: u16,
    /// One past the last menu slot.
    pub end: u16,
}

impl SlotRange {
    /// A range over `start..end`.
    #[must_use]
    pub const fn new(start: u16, end: u16) -> Self {
        Self { start, end }
    }

    /// Whether `slot` is inside.
    #[must_use]
    pub const fn contains(&self, slot: u16) -> bool {
        slot >= self.start && slot < self.end
    }

    /// How many slots it covers.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }

    /// Whether it covers nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// How a menu routes a shift-click.
///
/// Vanilla hard-codes "container → player, player → container" per menu. Modelling
/// it as ordered ranges makes the routing *data*, which is what lets a test assert
/// that a shift-click cannot leave items somewhere they would be discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLayout {
    /// Ordered groups. A shift-click from a slot in group *i* tries groups
    /// `i+1, i+2, …, i-1` in that order (wrapping, never its own group last —
    /// which is what makes "shift-click inside the player inventory" move items
    /// between the main rows and the hotbar).
    pub groups: Vec<SlotRange>,
    /// Which container holds the player's own storage.
    ///
    /// Swap routing needs it: "press 3" must find the player's hotbar, and in a
    /// chest menu that is container 1, not 0. Hard-coding 0 (as an earlier draft
    /// did) makes a number-key swap silently do nothing in every container menu.
    pub player_container: u8,
}

impl MenuLayout {
    /// A layout with no routing: shift-click does nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            groups: Vec::new(),
            player_container: 0,
        }
    }

    /// Which group index contains `slot`.
    #[must_use]
    pub fn group_of(&self, slot: u16) -> Option<usize> {
        self.groups.iter().position(|range| range.contains(slot))
    }

    /// The groups to try, in order, for a shift-click from `slot`.
    ///
    /// The source group is deliberately **excluded**: Vanilla never shift-clicks an
    /// item back into the group it came from, and including it would let a
    /// shift-click shuffle items within one group forever instead of moving them.
    #[must_use]
    pub fn transfer_order(&self, slot: u16) -> Vec<SlotRange> {
        let Some(from) = self.group_of(slot) else {
            return self.groups.clone();
        };
        let count = self.groups.len();
        (1..count)
            .map(|offset| self.groups[(from + offset) % count])
            .collect()
    }

    /// The layout for "a container plus the player's inventory": the container
    /// first, then the player's main rows, then the hotbar.
    #[must_use]
    pub fn container_and_player(container_slots: usize) -> Self {
        let base = u16::try_from(container_slots).unwrap_or(u16::MAX);
        Self {
            groups: vec![
                SlotRange::new(0, base),
                SlotRange::new(base, base + 27),
                SlotRange::new(base + 27, base + 36),
            ],
            // The container comes first, so the player's storage is container 1.
            player_container: 1,
        }
    }

    /// The player-inventory menu's layout: main rows then hotbar.
    #[must_use]
    pub fn player_only() -> Self {
        Self {
            groups: vec![SlotRange::new(0, 27), SlotRange::new(27, 36)],
            player_container: 0,
        }
    }
}

/// What changed, so the caller knows which packets to send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClickOutcome {
    /// Menu slots whose contents changed, ascending.
    pub changed_slots: BTreeSet<u16>,
    /// Whether the cursor stack changed.
    pub cursor_changed: bool,
    /// Whether the client must be sent the whole window again.
    pub full_resync: bool,
    /// Items the click removed from the window for dropping into the world.
    pub dropped: Vec<ItemStack>,
}

impl ClickOutcome {
    /// Whether anything happened.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.changed_slots.is_empty()
            && !self.cursor_changed
            && !self.full_resync
            && self.dropped.is_empty()
    }

    /// Fold another outcome into this one.
    pub fn merge(&mut self, other: &Self) {
        self.changed_slots
            .extend(other.changed_slots.iter().copied());
        self.cursor_changed |= other.cursor_changed;
        self.full_resync |= other.full_resync;
        self.dropped.extend(other.dropped.iter().copied());
    }
}

/// An open container window.
#[derive(Debug, Clone)]
pub struct Menu {
    window_id: u8,
    state_id: i32,
    containers: Vec<Container>,
    slots: Vec<SlotMapping>,
    layout: MenuLayout,
    cursor: ItemStack,
    stack_sizes: StackSizeTable,
    /// Whether the player may clone (creative mode). The **only** thing that lets
    /// a click create items.
    creative: bool,
    drag_slots: Vec<u16>,
    drag_type: Option<DragType>,
}

impl Menu {
    /// A menu over `containers` with the given slot mappings.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when a mapping points at a container or slot that
    /// does not exist, when the slot count is outside `1..=MAX_MENU_SLOTS`, or when
    /// a slot ceiling is impossible. Menus are built by the server, so a bad mapping
    /// is a programming error rather than hostile input.
    pub fn new(
        window_id: u8,
        containers: Vec<Container>,
        slots: Vec<SlotMapping>,
        layout: MenuLayout,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if slots.is_empty() || slots.len() > crate::MAX_MENU_SLOTS {
            return Err(ServerError::Invariant(format!(
                "menu with {} slots is outside 1..={}",
                slots.len(),
                crate::MAX_MENU_SLOTS
            )));
        }
        for (index, mapping) in slots.iter().enumerate() {
            let container = containers
                .get(usize::from(mapping.container))
                .ok_or_else(|| {
                    ServerError::Invariant(format!(
                        "menu slot {index} points at container {}, which does not exist",
                        mapping.container
                    ))
                })?;
            if usize::from(mapping.slot) >= container.len() {
                return Err(ServerError::Invariant(format!(
                    "menu slot {index} points at slot {} of a {}-slot container",
                    mapping.slot,
                    container.len()
                )));
            }
            if mapping.max_stack < 1 || mapping.max_stack > mc_entity::stack::HARD_MAX_STACK_SIZE {
                return Err(ServerError::Invariant(format!(
                    "menu slot {index} has an impossible limit of {}",
                    mapping.max_stack
                )));
            }
        }
        Ok(Self {
            window_id,
            state_id: 0,
            containers,
            slots,
            layout,
            cursor: ItemStack::EMPTY,
            stack_sizes,
            creative: false,
            drag_slots: Vec::new(),
            drag_type: None,
        })
    }

    /// Set whether this menu may clone items (creative mode).
    pub fn set_creative(&mut self, creative: bool) {
        self.creative = creative;
    }

    /// Whether cloning is allowed.
    #[must_use]
    pub const fn is_creative(&self) -> bool {
        self.creative
    }

    /// Build the player-inventory menu (`window id 0`) around a player's 41 storage
    /// slots.
    ///
    /// The menu layout matches Vanilla's `InventoryMenu`, verified from the jar in
    /// Phase 04: `0` crafting result, `1..=4` the 2x2 grid, `5..=8` armour,
    /// `9..=35` main inventory, `36..=44` hotbar, `45` offhand. Our player
    /// container stores slots in `PlayerInventory`'s order — `0..=8` hotbar,
    /// `9..=35` main, `36..=39` armour (boots first), `40` offhand — so the mapping
    /// performs that permutation, and the armour sub-order is *forward* because the
    /// jar's `SLOT_IDS` starts at `FEET`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `player_slots` is not 41 slots, or when
    /// [`Menu::new`] rejects the layout.
    pub fn player(
        window_id: u8,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a player inventory container needs 41 slots, got {}",
                player_slots.len()
            )));
        }
        let mut slots = Vec::with_capacity(46);
        // 0: the crafting result, in its own 1-slot container (container 1).
        slots.push(SlotMapping::with_role(1, 0, SlotRole::CraftingResult));
        // 1..=4: the 2x2 crafting grid (container 2).
        for slot in 0..4u16 {
            slots.push(SlotMapping::with_role(2, slot, SlotRole::CraftingInput));
        }
        // 5..=8: armour, boots first (container 0 slots 36..=39).
        for slot in 0..4u16 {
            slots.push(SlotMapping::with_role(0, 36 + slot, SlotRole::Armor));
        }
        // 9..=35: main inventory (container 0 slots 9..=35).
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        // 36..=44: hotbar (container 0 slots 0..=8).
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        // 45: offhand (container 0 slot 40).
        slots.push(SlotMapping::with_role(0, 40, SlotRole::Offhand));

        let result = Container::new(ContainerKind::Crafting, 1)?;
        let grid = Container::new(ContainerKind::Crafting, 4)?;
        let containers = vec![player_slots, result, grid];
        // The player's own storage is one routing group spanning main, hotbar and
        // offhand (menu 9..=45). The crafting grid and armour are deliberately not
        // in a group: shift-clicking an item must not silently equip or craft it.
        let layout = MenuLayout {
            groups: vec![SlotRange::new(9, 45)],
            player_container: 0,
        };
        Self::new(window_id, containers, slots, layout, stack_sizes)
    }

    /// The window id.
    #[must_use]
    pub const fn window_id(&self) -> u8 {
        self.window_id
    }

    /// Which container holds the player's own storage (0 for the player menu,
    /// 1 for a block menu where container 0 is the block).
    #[must_use]
    pub const fn player_container_index(&self) -> usize {
        self.layout.player_container as usize
    }

    /// Build a chest menu: `block_slots` (27 single, 54 double) plus the
    /// player's main rows and hotbar (36 slots).
    ///
    /// The player container is the full 41-slot `Player` container, but only
    /// main (`9..=35`) and hotbar (`0..=8`) are exposed — armour, crafting and
    /// offhand are not part of a vanilla chest window. Shift-click routes
    /// block ↔ player via [`MenuLayout::container_and_player`].
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `block_slots` is not 27 or 54, when
    /// `player_slots` is not 41, or when [`Menu::new`] rejects the layout.
    pub fn chest(
        window_id: u8,
        block_slots: Container,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        let n = block_slots.len();
        if n != 27 && n != 54 {
            return Err(ServerError::Invariant(format!(
                "a chest container needs 27 or 54 slots, got {n}"
            )));
        }
        if block_slots.kind() != ContainerKind::Generic {
            return Err(ServerError::Invariant(format!(
                "a chest container must be Generic, got {}",
                block_slots.kind()
            )));
        }
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a chest menu needs 41 player slots, got {}",
                player_slots.len()
            )));
        }
        let mut slots = Vec::with_capacity(n + 36);
        for slot in 0..n as u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        // Player main 9..=35, then hotbar 0..=8 (storage order, not menu order).
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        let layout = MenuLayout::container_and_player(n);
        Self::new(
            window_id,
            vec![block_slots, player_slots],
            slots,
            layout,
            stack_sizes,
        )
    }

    /// Build a furnace menu: 3 block slots (input, fuel, output) plus 36 player
    /// slots. The output refuses placement via [`SlotRole::FurnaceOutput`].
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] on the same shape mismatches as [`Menu::chest`].
    pub fn furnace(
        window_id: u8,
        block_slots: Container,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if block_slots.len() != 3 {
            return Err(ServerError::Invariant(format!(
                "a furnace container needs 3 slots, got {}",
                block_slots.len()
            )));
        }
        if block_slots.kind() != ContainerKind::Furnace {
            return Err(ServerError::Invariant(format!(
                "a furnace container must be Furnace, got {}",
                block_slots.kind()
            )));
        }
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a furnace menu needs 41 player slots, got {}",
                player_slots.len()
            )));
        }
        let mut slots = Vec::with_capacity(39);
        slots.push(SlotMapping::with_role(0, 0, SlotRole::FurnaceInput));
        slots.push(SlotMapping::with_role(0, 1, SlotRole::FurnaceFuel));
        slots.push(SlotMapping::with_role(0, 2, SlotRole::FurnaceOutput));
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        let layout = MenuLayout::container_and_player(3);
        Self::new(
            window_id,
            vec![block_slots, player_slots],
            slots,
            layout,
            stack_sizes,
        )
    }

    /// Build a hopper menu: 5 block slots plus 36 player slots.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] on the same shape mismatches as [`Menu::chest`].
    pub fn hopper(
        window_id: u8,
        block_slots: Container,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if block_slots.len() != 5 {
            return Err(ServerError::Invariant(format!(
                "a hopper container needs 5 slots, got {}",
                block_slots.len()
            )));
        }
        if block_slots.kind() != ContainerKind::Generic {
            return Err(ServerError::Invariant(format!(
                "a hopper container must be Generic, got {}",
                block_slots.kind()
            )));
        }
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a hopper menu needs 41 player slots, got {}",
                player_slots.len()
            )));
        }
        let mut slots = Vec::with_capacity(41);
        for slot in 0..5u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        let layout = MenuLayout::container_and_player(5);
        Self::new(
            window_id,
            vec![block_slots, player_slots],
            slots,
            layout,
            stack_sizes,
        )
    }

    /// Build a dispenser/dropper menu: 9 block slots plus 36 player slots
    /// (P17-01, vanilla `generic_3x3`).
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] on the same shape mismatches as [`Menu::chest`].
    pub fn dispenser(
        window_id: u8,
        block_slots: Container,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if block_slots.len() != 9 {
            return Err(ServerError::Invariant(format!(
                "a dispenser container needs 9 slots, got {}",
                block_slots.len()
            )));
        }
        if block_slots.kind() != ContainerKind::Generic {
            return Err(ServerError::Invariant(format!(
                "a dispenser container must be Generic, got {}",
                block_slots.kind()
            )));
        }
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a dispenser menu needs 41 player slots, got {}",
                player_slots.len()
            )));
        }
        let mut slots = Vec::with_capacity(45);
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(1, slot));
        }
        let layout = MenuLayout::container_and_player(9);
        Self::new(
            window_id,
            vec![block_slots, player_slots],
            slots,
            layout,
            stack_sizes,
        )
    }

    /// Build a crafting-table menu: result plus the 3×3 grid plus 36 player
    /// slots (P17-02 Step C, vanilla `CraftingMenu` shape).
    ///
    /// Menu `0` is the result, `1..=9` the grid left to right, top to
    /// bottom, `10..=36` main inventory and `37..=45` the hotbar. Container
    /// indices mirror the player menu — 0 player, 1 result, 2 grid — so the
    /// shared recompute/take helpers read the same slots (the width rides
    /// the grid size: 4 is 2×2, 9 is 3×3). The grid is ephemeral — no block
    /// entity backs it — so closing returns it (the server's close path
    /// owns that, not this constructor).
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `grid_slots` is not 9, when
    /// `player_slots` is not 41, or when [`Menu::new`] rejects the layout.
    pub fn crafting_table(
        window_id: u8,
        grid_slots: Container,
        player_slots: Container,
        stack_sizes: StackSizeTable,
    ) -> ServerResult<Self> {
        if grid_slots.len() != 9 {
            return Err(ServerError::Invariant(format!(
                "a crafting grid needs 9 slots, got {}",
                grid_slots.len()
            )));
        }
        if grid_slots.kind() != ContainerKind::Crafting {
            return Err(ServerError::Invariant(format!(
                "a crafting grid must be Crafting, got {}",
                grid_slots.kind()
            )));
        }
        if player_slots.len() != 41 {
            return Err(ServerError::Invariant(format!(
                "a crafting menu needs 41 player slots, got {}",
                player_slots.len()
            )));
        }
        let result = Container::new(ContainerKind::Crafting, 1)?;
        let mut slots = Vec::with_capacity(46);
        // 0: the result (container 1).
        slots.push(SlotMapping::with_role(1, 0, SlotRole::CraftingResult));
        // 1..=9: the 3x3 grid (container 2).
        for slot in 0..9u16 {
            slots.push(SlotMapping::with_role(2, slot, SlotRole::CraftingInput));
        }
        // 10..=36: main inventory (container 0 slots 9..=35).
        for slot in 9..36u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        // 37..=45: hotbar (container 0 slots 0..=8).
        for slot in 0..9u16 {
            slots.push(SlotMapping::storage(0, slot));
        }
        let layout = MenuLayout {
            groups: vec![
                SlotRange::new(0, 10),
                SlotRange::new(10, 37),
                SlotRange::new(37, 46),
            ],
            player_container: 0,
        };
        Self::new(
            window_id,
            vec![player_slots, result, grid_slots],
            slots,
            layout,
            stack_sizes,
        )
    }

    /// The current state id, which the client must echo back.
    #[must_use]
    pub const fn state_id(&self) -> i32 {
        self.state_id
    }

    /// Advance the state id after an accepted mutation.
    pub fn bump_state(&mut self) {
        self.state_id = self.state_id.wrapping_add(1);
    }

    /// How many menu slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The containers this menu owns.
    #[must_use]
    pub fn containers(&self) -> &[Container] {
        &self.containers
    }

    /// Mutable access to the containers, for server-side placement (a block being
    /// broken, a hopper transferring, a block entity being persisted).
    ///
    /// Bypasses role and limit checks by design — it is the path the *server* takes,
    /// not the client — but it cannot break conservation because it still writes
    /// through [`Container::set`].
    pub fn containers_mut(&mut self) -> &mut [Container] {
        &mut self.containers
    }

    /// A container by index.
    #[must_use]
    pub fn container(&self, index: usize) -> Option<&Container> {
        self.containers.get(index)
    }

    /// A container by index, mutably.
    pub fn container_mut(&mut self, index: usize) -> Option<&mut Container> {
        self.containers.get_mut(index)
    }

    /// The slot mappings.
    #[must_use]
    pub fn slots(&self) -> &[SlotMapping] {
        &self.slots
    }

    /// A mapping by menu slot.
    #[must_use]
    pub fn mapping(&self, index: usize) -> Option<&SlotMapping> {
        self.slots.get(index)
    }

    /// The window slot displaying a storage slot, if any.
    ///
    /// Vanilla window numbers differ per menu (player hotbar `h` lives at
    /// window slot `36 + h` on window 0, but after the block slots on a
    /// block window), so callers translating storage indices — hotbar
    /// selection sync, held-slot updates — must ask the menu rather than
    /// add a constant. First match wins; layouts never alias one storage
    /// slot twice.
    #[must_use]
    pub fn window_slot_for(&self, container: usize, slot: u16) -> Option<usize> {
        self.slots
            .iter()
            .position(|mapping| usize::from(mapping.container) == container && mapping.slot == slot)
    }

    /// The layout.
    #[must_use]
    pub const fn layout(&self) -> &MenuLayout {
        &self.layout
    }

    /// The cursor stack.
    #[must_use]
    pub const fn cursor(&self) -> ItemStack {
        self.cursor
    }

    /// The stack shown in a menu slot, or empty when out of range.
    #[must_use]
    pub fn display_stack(&self, index: usize) -> ItemStack {
        let Some(mapping) = self.slots.get(index) else {
            return ItemStack::EMPTY;
        };
        self.containers[usize::from(mapping.container)].get(usize::from(mapping.slot))
    }

    /// The item's own maximum stack size.
    #[must_use]
    pub fn item_limit(&self, stack: ItemStack) -> i32 {
        match stack.item_id() {
            Some(id) => self.stack_sizes.max_stack_size(id),
            None => 0,
        }
    }

    /// The effective limit for a menu slot: the item's maximum, capped by the
    /// slot's own ceiling.
    #[must_use]
    pub fn slot_limit(&self, index: usize, stack: ItemStack) -> i32 {
        let Some(mapping) = self.slots.get(index) else {
            return 0;
        };
        self.item_limit(stack).min(mapping.max_stack)
    }

    /// Total items across every container and the cursor.
    ///
    /// The conservation instrument: only [`ClickType::Throw`] may decrease it and
    /// only creative [`ClickType::Clone`] may increase it.
    #[must_use]
    pub fn total_items(&self) -> i64 {
        self.containers
            .iter()
            .map(Container::total_items)
            .sum::<i64>()
            + i64::from(self.cursor.count())
    }

    /// Write a menu slot, clamping to its limit.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when the index does not exist. A stack above
    /// the limit is clamped rather than refused: clamping is what Vanilla does, and
    /// an over-limit stack can only come from a server-side mistake.
    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> ServerResult<()> {
        let Some(mapping) = self.slots.get(index).copied() else {
            return Err(ServerError::InvalidAction(format!(
                "menu slot {index} does not exist"
            )));
        };
        let limit = self.slot_limit(index, stack);
        let stack = if stack.is_empty() || stack.count() <= limit {
            stack
        } else {
            ItemStack::new(stack.item_id().unwrap_or(0), limit)?
        };
        self.containers[usize::from(mapping.container)].set(usize::from(mapping.slot), stack)
    }

    /// Set the cursor, clamping to the item's own limit.
    pub fn set_cursor(&mut self, mut stack: ItemStack) {
        let limit = self.item_limit(stack);
        if !stack.is_empty() && stack.count() > limit {
            stack.shrink(stack.count() - limit);
        }
        self.cursor = stack;
    }

    /// Put `stack` back on the cursor, merging with what is already there.
    ///
    /// Returns whatever did not fit, so a caller can drop it rather than lose it.
    /// The limit comes from `stack` itself: reading it from the cursor is wrong
    /// whenever the cursor is empty, which is exactly when this is called.
    fn return_to_cursor(&mut self, mut stack: ItemStack) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }
        let limit = self.item_limit(stack);
        let mut cursor = self.cursor;
        let remainder = cursor.merge_capped(&mut stack, limit);
        self.cursor = cursor;
        if remainder > 0 {
            stack
        } else {
            ItemStack::EMPTY
        }
    }

    /// Take up to `count` items out of a menu slot; returns what was taken.
    ///
    /// One of the two functions a click handler uses to move items.
    fn extract(&mut self, index: usize, count: i32) -> ItemStack {
        let mut stack = self.display_stack(index);
        let taken = stack.split(count);
        if !taken.is_empty() {
            // Writing back a strictly smaller stack cannot fail for a valid index.
            let _ = self.set_slot(index, stack);
        }
        taken
    }

    /// Put `stack` into a menu slot, merging with a matching stack; returns what
    /// did **not** fit.
    ///
    /// The other of the two. Every caller either puts the remainder back or reports
    /// it as a drop, which is what makes conservation checkable.
    fn insert(&mut self, index: usize, stack: ItemStack) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::EMPTY;
        }
        let Some(mapping) = self.slots.get(index).copied() else {
            return stack;
        };
        if !mapping.may_place() {
            return stack;
        }
        let existing = self.display_stack(index);
        if !existing.is_empty() && !existing.same_item(&stack) {
            return stack;
        }
        let limit = self.slot_limit(index, stack);
        if existing.is_empty() {
            let mut placed = stack;
            let mut overflow = ItemStack::EMPTY;
            if placed.count() > limit {
                // `split` returns the part taken off the front, so ask for the
                // *excess* and leave `limit` behind. Asking for `limit` would
                // invert the split and destroy everything above it.
                overflow = placed.split(placed.count() - limit);
            }
            // A write to a valid, placement-permitted slot cannot fail.
            let _ = self.set_slot(index, placed);
            return overflow;
        }
        let mut merged = existing;
        let mut incoming = stack;
        let remainder = merged.merge_capped(&mut incoming, limit);
        if merged != existing {
            let _ = self.set_slot(index, merged);
        }
        if remainder > 0 {
            // `remainder` is what did not fit, and `incoming` still holds it.
            incoming
        } else {
            ItemStack::EMPTY
        }
    }

    /// Menu slots whose container slots changed since the last drain, ascending.
    #[must_use]
    pub fn changed_menu_slots(&self) -> BTreeSet<u16> {
        let mut changed = BTreeSet::new();
        for (menu_index, mapping) in self.slots.iter().enumerate() {
            let container = &self.containers[usize::from(mapping.container)];
            if container.changed().contains(&mapping.slot) {
                changed.insert(menu_index as u16);
            }
        }
        changed
    }

    /// Drain the change flags of every container.
    pub fn clear_changed(&mut self) {
        for container in &mut self.containers {
            container.clear_changed();
        }
    }

    /// Drain the changed slots and clear the flags.
    pub fn drain_changed(&mut self) -> BTreeSet<u16> {
        let changed = self.changed_menu_slots();
        self.clear_changed();
        changed
    }

    /// Every menu slot, for a full resync payload.
    #[must_use]
    pub fn full_contents(&self) -> Vec<ItemStack> {
        (0..self.slots.len())
            .map(|index| self.display_stack(index))
            .collect()
    }

    fn mapping_of(&self, index: usize) -> ServerResult<SlotMapping> {
        self.slots.get(index).copied().ok_or_else(|| {
            ServerError::InvalidAction(format!("slot {index} does not exist in this menu"))
        })
    }

    // ---------------------------------------------------------------- clicks

    /// Apply a click.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] for a click that cannot apply to this menu (a
    /// foreign window id, or a slot this menu does not have). A state-id mismatch is
    /// **not** an error: it returns [`ClickOutcome::full_resync`], because a
    /// desynchronised client is an expected condition to correct rather than a
    /// protocol violation to punish.
    pub fn apply_click(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        if click.window_id != self.window_id {
            return Err(ServerError::InvalidAction(format!(
                "click for window {} but this menu is window {}",
                click.window_id, self.window_id
            )));
        }
        if let Some(index) = click.slot_index() {
            self.mapping_of(index)?;
        }
        if click.state_id != self.state_id {
            return Ok(ClickOutcome {
                full_resync: true,
                ..ClickOutcome::default()
            });
        }

        let mut outcome = match click.click_type {
            ClickType::Pickup => self.click_pickup(click),
            ClickType::QuickMove => self.click_quick_move(click),
            ClickType::Swap => self.click_swap(click),
            ClickType::Clone => self.click_clone(click),
            ClickType::Throw => self.click_throw(click),
            ClickType::QuickCraft => self.click_quick_craft(click),
            ClickType::PickupAll => self.click_pickup_all(click),
        }?;
        if !outcome.is_noop() {
            self.bump_state();
        }
        if outcome.full_resync {
            outcome.changed_slots.clear();
            outcome.cursor_changed = false;
        }
        Ok(outcome)
    }

    /// Left/right click: pick up, put down, merge, or swap.
    fn click_pickup(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        let Some(index) = click.slot_index() else {
            // Outside the window: drop one item (right) or the stack (left).
            if !self.cursor.is_empty() {
                let count = if click.secondary {
                    1
                } else {
                    self.cursor.count()
                };
                let dropped = self.cursor.split(count);
                if !dropped.is_empty() {
                    outcome.dropped.push(dropped);
                    outcome.cursor_changed = true;
                }
            }
            return Ok(outcome);
        };

        let mapping = self.mapping_of(index)?;
        let slot_stack = self.display_stack(index);

        if self.cursor.is_empty() {
            if !mapping.may_pickup() || slot_stack.is_empty() {
                return Ok(outcome);
            }
            // Right click takes half, rounded up (Vanilla).
            let count = if click.secondary {
                // Round up: picking up one item with the right button takes it.
                (slot_stack.count() + 1) / 2
            } else {
                slot_stack.count()
            };
            let taken = self.extract(index, count);
            if !taken.is_empty() {
                self.set_cursor(taken);
                outcome.cursor_changed = true;
                outcome.changed_slots.insert(index as u16);
            }
            return Ok(outcome);
        }

        // The cursor holds something.
        if !mapping.may_place() {
            return Ok(outcome);
        }
        if slot_stack.is_empty() {
            let wanted = if click.secondary {
                1
            } else {
                self.cursor.count()
            };
            let offered = self.cursor.split(wanted);
            let overflow = self.insert(index, offered);
            if !overflow.is_empty() {
                // Did not fit: back on the cursor, or dropped if even that cannot
                // hold it (which would be a server bug, not a client one).
                let leftover = self.return_to_cursor(overflow);
                if !leftover.is_empty() {
                    outcome.dropped.push(leftover);
                }
            }
            if self.display_stack(index) != slot_stack {
                outcome.changed_slots.insert(index as u16);
            }
            outcome.cursor_changed = true;
            return Ok(outcome);
        }

        if !slot_stack.same_item(&self.cursor) {
            // Different items: a left click swaps, a right click does nothing.
            if !click.secondary {
                let cursor = self.cursor;
                self.set_slot(index, cursor)?;
                self.set_cursor(slot_stack);
                outcome.changed_slots.insert(index as u16);
                outcome.cursor_changed = true;
            }
            return Ok(outcome);
        }

        // Same item: merge.
        let room = self.slot_limit(index, slot_stack) - slot_stack.count();
        if room <= 0 {
            return Ok(outcome);
        }
        let wanted = if click.secondary { 1.min(room) } else { room };
        let offered = self.cursor.split(wanted);
        let overflow = self.insert(index, offered);
        if !overflow.is_empty() {
            let leftover = self.return_to_cursor(overflow);
            if !leftover.is_empty() {
                outcome.dropped.push(leftover);
            }
        }
        outcome.changed_slots.insert(index as u16);
        outcome.cursor_changed = true;
        Ok(outcome)
    }

    /// Shift-click: move the stack into the layout's next groups.
    fn click_quick_move(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        let Some(index) = click.slot_index() else {
            return Ok(outcome);
        };
        let mapping = self.mapping_of(index)?;
        let stack = self.display_stack(index);
        if stack.is_empty() || !mapping.may_pickup() {
            return Ok(outcome);
        }
        let mut remaining = self.extract(index, stack.count());
        for range in self.layout.transfer_order(index as u16) {
            if remaining.is_empty() {
                break;
            }
            for target in range.start..range.end {
                if remaining.is_empty() {
                    break;
                }
                if usize::from(target) == index {
                    continue;
                }
                remaining = self.insert(usize::from(target), remaining);
                if self.display_stack(usize::from(target)) != ItemStack::EMPTY {
                    outcome.changed_slots.insert(target);
                }
            }
        }
        // Whatever did not fit goes back where it came from.
        let leftover = self.insert(index, remaining);
        if !leftover.is_empty() {
            return Err(ServerError::Invariant(format!(
                "a shift-click could not return {} leftover items to its source slot",
                leftover.count()
            )));
        }
        outcome.changed_slots.insert(index as u16);
        Ok(outcome)
    }

    /// Number-key swap.
    fn click_swap(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        let Some(index) = click.slot_index() else {
            return Ok(outcome);
        };
        let Some(target) = click.swap_target else {
            return Ok(outcome);
        };
        let mapping = self.mapping_of(index)?;
        let Some(partner) = self.swap_partner(target) else {
            return Ok(outcome);
        };
        if partner == index as u16 {
            return Ok(outcome);
        }
        let partner_mapping = self.mapping_of(usize::from(partner))?;
        let a = self.display_stack(index);
        let b = self.display_stack(usize::from(partner));

        if self.cursor.is_empty() {
            // Both directions must be legal: the item that lands must be allowed
            // there and must fit.
            if !b.is_empty() && !mapping.may_place() {
                return Ok(outcome);
            }
            if !a.is_empty() && !partner_mapping.may_place() {
                return Ok(outcome);
            }
            if !b.is_empty() && b.count() > self.slot_limit(index, b) {
                return Ok(outcome);
            }
            if !a.is_empty() && a.count() > self.slot_limit(usize::from(partner), a) {
                return Ok(outcome);
            }
            self.set_slot(index, b)?;
            self.set_slot(usize::from(partner), a)?;
            outcome.changed_slots.insert(index as u16);
            outcome.changed_slots.insert(partner);
        } else if b.is_empty() && mapping.may_place() {
            // With a cursor, an empty partner takes the cursor.
            let cursor = self.cursor;
            let overflow = self.insert(usize::from(partner), cursor);
            if overflow.is_empty() {
                self.set_cursor(ItemStack::EMPTY);
                outcome.changed_slots.insert(partner);
                outcome.cursor_changed = true;
            }
        }
        Ok(outcome)
    }

    /// Middle click: creative duplication — the only click that may create items.
    fn click_clone(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        if !self.creative {
            return Ok(outcome);
        }
        let Some(index) = click.slot_index() else {
            return Ok(outcome);
        };
        self.mapping_of(index)?;
        let stack = self.display_stack(index);
        if stack.is_empty() {
            return Ok(outcome);
        }
        let count = if click.button == 2 {
            self.item_limit(stack)
        } else {
            stack.count()
        };
        let cloned = ItemStack::new(stack.item_id().unwrap_or(0), count)?;
        self.set_cursor(cloned);
        outcome.cursor_changed = true;
        Ok(outcome)
    }

    /// Throw: remove items, hand them to the caller to drop in the world.
    fn click_throw(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        if let Some(index) = click.slot_index() {
            let mapping = self.mapping_of(index)?;
            if !mapping.may_pickup() {
                return Ok(outcome);
            }
            let stack = self.display_stack(index);
            if stack.is_empty() {
                return Ok(outcome);
            }
            let count = if click.secondary { stack.count() } else { 1 };
            let dropped = self.extract(index, count);
            if !dropped.is_empty() {
                outcome.dropped.push(dropped);
                outcome.changed_slots.insert(index as u16);
            }
        } else if !self.cursor.is_empty() {
            let count = if click.secondary {
                self.cursor.count()
            } else {
                1
            };
            let dropped = self.cursor.split(count);
            if !dropped.is_empty() {
                outcome.dropped.push(dropped);
                outcome.cursor_changed = true;
            }
        }
        Ok(outcome)
    }

    /// Drag: start → add slot → end.
    // The three drag types each have their own distribution rule and share one
    // preamble; splitting it would duplicate the stage validation three times.
    #[allow(clippy::too_many_lines)]
    fn click_quick_craft(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        let Some((drag_type, stage)) = click.drag() else {
            return Ok(outcome);
        };
        match stage {
            DragStage::Start => {
                self.drag_slots.clear();
                self.drag_type = Some(drag_type);
                Ok(outcome)
            }
            DragStage::AddSlot => {
                // Only meaningful while a drag of the same type is open.
                if self.drag_type != Some(drag_type) {
                    return Ok(outcome);
                }
                if let Some(index) = click.slot_index() {
                    let mapping = self.mapping_of(index)?;
                    if mapping.may_place() && !self.drag_slots.contains(&(index as u16)) {
                        self.drag_slots.push(index as u16);
                    }
                }
                Ok(outcome)
            }
            DragStage::End => {
                let slots = std::mem::take(&mut self.drag_slots);
                let open = self.drag_type.take();
                if open != Some(drag_type) || self.cursor.is_empty() || slots.is_empty() {
                    return Ok(outcome);
                }
                match drag_type {
                    DragType::One => {
                        for slot in slots {
                            if self.cursor.is_empty() {
                                break;
                            }
                            let one = self.cursor.split(1);
                            let overflow = self.insert(usize::from(slot), one);
                            if !overflow.is_empty() {
                                let leftover = self.return_to_cursor(overflow);
                                if !leftover.is_empty() {
                                    outcome.dropped.push(leftover);
                                }
                            }
                            outcome.changed_slots.insert(slot);
                        }
                        outcome.cursor_changed = true;
                    }
                    DragType::Even => {
                        // Only slots that can accept the item share the cursor.
                        let accepted: Vec<(u16, i32)> = slots
                            .iter()
                            .filter_map(|slot| {
                                let target = self.display_stack(usize::from(*slot));
                                if !target.is_empty() && !target.same_item(&self.cursor) {
                                    return None;
                                }
                                let room = self.slot_limit(usize::from(*slot), self.cursor)
                                    - target.count();
                                (room > 0).then_some((*slot, room))
                            })
                            .collect();
                        if accepted.is_empty() {
                            return Ok(outcome);
                        }
                        let total = self.cursor.count();
                        let per = total / accepted.len() as i32;
                        let mut extra = total % accepted.len() as i32;
                        for (slot, room) in accepted {
                            let mut give = per.min(room);
                            if extra > 0 && give < room {
                                give += 1;
                                extra -= 1;
                            }
                            if give == 0 {
                                continue;
                            }
                            let share = self.cursor.split(give);
                            let overflow = self.insert(usize::from(slot), share);
                            if !overflow.is_empty() {
                                let leftover = self.return_to_cursor(overflow);
                                if !leftover.is_empty() {
                                    outcome.dropped.push(leftover);
                                }
                            }
                            outcome.changed_slots.insert(slot);
                        }
                        outcome.cursor_changed = true;
                    }
                    DragType::Full => {
                        for slot in slots {
                            if self.cursor.is_empty() {
                                break;
                            }
                            let target = self.display_stack(usize::from(slot));
                            let room =
                                self.slot_limit(usize::from(slot), self.cursor) - target.count();
                            if room <= 0 {
                                continue;
                            }
                            let share = self.cursor.split(room);
                            let overflow = self.insert(usize::from(slot), share);
                            if !overflow.is_empty() {
                                let leftover = self.return_to_cursor(overflow);
                                if !leftover.is_empty() {
                                    outcome.dropped.push(leftover);
                                }
                            }
                            outcome.changed_slots.insert(slot);
                        }
                        outcome.cursor_changed = true;
                    }
                }
                Ok(outcome)
            }
        }
    }

    /// Double-click: gather matching stacks into the cursor.
    fn click_pickup_all(&mut self, click: &Click) -> ServerResult<ClickOutcome> {
        let mut outcome = ClickOutcome::default();
        let Some(index) = click.slot_index() else {
            return Ok(outcome);
        };
        let mapping = self.mapping_of(index)?;
        if !mapping.may_pickup() {
            return Ok(outcome);
        }
        let mut gathered = self.cursor;
        if gathered.is_empty() {
            let target = self.display_stack(index);
            if target.is_empty() {
                return Ok(outcome);
            }
            gathered = self.extract(index, target.count());
            outcome.changed_slots.insert(index as u16);
        }
        if gathered.is_empty() {
            return Ok(outcome);
        }
        let limit = self.item_limit(gathered);
        for slot in 0..self.slots.len() {
            if gathered.count() >= limit {
                break;
            }
            if slot == index {
                continue;
            }
            let stack = self.display_stack(slot);
            if stack.is_empty()
                || !stack.same_item(&gathered)
                || !self.mapping_of(slot)?.may_pickup()
            {
                continue;
            }
            let want = limit - gathered.count();
            let moved = self.extract(slot, want);
            if moved.is_empty() {
                continue;
            }
            let mut moved = moved;
            let _ = gathered.merge_capped(&mut moved, limit);
            outcome.changed_slots.insert(slot as u16);
        }
        if gathered != self.cursor {
            self.set_cursor(gathered);
            outcome.cursor_changed = true;
        }
        Ok(outcome)
    }

    /// The menu slot a swap targets (a hotbar slot, or the offhand).
    fn swap_partner(&self, target: SwapTarget) -> Option<u16> {
        let player = self.layout.player_container;
        match target {
            SwapTarget::Hotbar(hotbar) => self
                .slots
                .iter()
                .position(|mapping| {
                    mapping.container == player
                        && mapping.role == SlotRole::Storage
                        && mapping.slot == u16::from(hotbar)
                })
                .map(|index| index as u16),
            SwapTarget::Offhand => self
                .slots
                .iter()
                .position(|mapping| mapping.role == SlotRole::Offhand)
                .map(|index| index as u16),
        }
    }
}

#[cfg(test)]
mod tests;
