//! Server-authoritative containers and inventory transactions (P06-01..P06-03).
//!
//! ## The rule this crate exists to enforce
//!
//! A client's `container_click` is **intent, never authority** (PHASE-06). The
//! client says "I left-clicked slot 12 in window 0 at state 47"; the server decides
//! what that means, mutates its own state, and tells the client what changed. Every
//! packet the client sends is therefore validated against server state, and a
//! mismatch is resolved by *resynchronising the client*, never by trusting it.
//!
//! Three properties are checked mechanically by the tests in this crate, because
//! "the client cannot dupe or destroy items" is the whole point:
//!
//! 1. **Conservation** — a transfer moves items, it never creates or destroys them.
//!    For every click type except [`ClickType::Throw`] and [`ClickType::Clone`],
//!    the total item count across all containers plus the cursor is invariant.
//! 2. **Bounded** — no stack ever exceeds its slot's limit, and the cursor never
//!    exceeds the item's own maximum. A hostile client cannot claim 64 items of a
//!    stack size of 1.
//! 3. **Validated** — slot indices, the window id and the state id are all checked
//!    against server state before anything mutates.
//!
//! ## Why a menu owns its containers
//!
//! Vanilla's `AbstractContainerMenu` holds references into other objects. Rust
//! makes that awkward, so a [`Menu`] **owns** a `Vec<Container>` and describes each
//! menu slot as `(container index, slot index)` ([`SlotMapping`]). This keeps every
//! mutation in one place (so conservation is checkable in one place), keeps the
//! click handlers free of borrow gymnastics, and makes a whole transaction testable
//! against a `Menu` alone with no world, no socket and no async.
//!
//! ## Determinism (AGENTS.md section 3.6)
//!
//! Every collection a transaction walks is ordered: containers and slots are
//! `Vec`s indexed by position, and the changed-slot sets are `BTreeSet`s, so the
//! set of packets produced for a given click is identical on every run.
//!
//! ## What this crate does not do
//!
//! It knows nothing about the world, chunks, entities or the wire. Persisting a
//! container's contents, turning a `container_set_slot` into bytes, and deciding
//! *which* menu a player has open are all the caller's job (`mc-server`). That
//! boundary is deliberate: the transaction rules are the part that must be
//! exhaustively tested, and they are testable in isolation only if nothing here
//! reaches outward.

#![forbid(unsafe_code)]
// Slot arithmetic mixes wire integers (`i16`, `i8`, `i32`) with indices; every
// conversion is range-checked before use, which is the point of the `Result`
// returns throughout. The casts that remain are of already-validated values.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod block_entity;
pub mod click;
pub mod container;
pub mod crafting;
pub mod furnace;
pub mod hopper;
pub mod menu;
pub mod smelting_data;

pub use block_entity::{BlockEntity, BlockEntityData, BlockEntityKind, BlockEntityStore, BlockPos};
pub use click::{Click, ClickRejection, ClickType, DragStage, DragType, SwapTarget};
pub use container::{Container, ContainerKind, MAX_CONTAINER_SLOTS, SlotRole};
// The gameplay tables (recipes, smelting, fuel) are hand-written baseline subsets,
// **not** the Vanilla data: see each module's docs for exactly what is missing and
// which phase (P07-03) loads the real thing.
pub use crafting::{
    CraftingConversion, Ingredient, MAX_ALTERNATIVES_PER_KEY, MAX_GRID_SLOTS, MAX_GRID_WIDTH,
    Recipe, RecipeKind, RecipeMatch, RecipeRegistry, Shape, ShapedPattern, ShapedRecipe,
    ShapelessRecipe,
};
pub use furnace::{
    Evidence, FUEL_BURST_TICKS, FuelValue, Furnace, FurnaceSlots, FurnaceState, FurnaceTickReport,
    MAX_COOK_TICKS, SMELTING_COOK_TICKS, SmeltingRecipe, SmeltingRegistry,
};
pub use hopper::{HOPPER_TRANSFER_COOLDOWN_TICKS, Hopper, HopperTransfer};
pub use menu::{ClickOutcome, Menu, MenuLayout, SlotMapping, SlotRange};

/// Player-inventory window id (Vanilla `InventoryMenu`).
pub const PLAYER_WINDOW_ID: u8 = 0;

/// Slot index a client sends for "outside the window" (a drop).
pub const SLOT_OUTSIDE: i16 = -1;

/// Upper bound on the menu slot count of any menu this build can build.
///
/// This is a *decode-time* bound only: it lets [`click::Click::new`] reject an
/// absurd slot before any menu exists, without knowing which menu the client
/// means. A real menu still range-checks against its own slot count.
///
/// 46 is the player inventory (0 = result, 1..=4 crafting, 5..=8 armour,
/// 9..=35 main, 36..=44 hotbar, 45 offhand); a large chest is 54 container slots
/// plus those 46, so the ceiling is set by the largest combination we can open.
pub const MAX_MENU_SLOTS: usize = 46 + 54;

/// Largest legal stack count for any item, as a compile-time sanity bound.
///
/// Per-item limits are resolved from [`mc_entity::stack::StackSizeTable`]; this is
/// only the ceiling a decoded packet is checked against before that lookup.
pub const HARD_MAX_COUNT: i32 = mc_entity::stack::HARD_MAX_STACK_SIZE;
