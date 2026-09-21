//! Block entities: block-attached state that persists (P06-07).
//!
//! A block entity is state a *block* owns rather than a *chunk position*: a chest's
//! contents, a furnace's burn timer, a sign's text. Vanilla stores them in the
//! chunk's `block_entities` list, each with an `id`, `x`/`y`/`z` and its own payload.
//!
//! ## The boundary this module draws
//!
//! It owns the **model and the lookup**, not the storage or the transport:
//!
//! - reading and writing the chunk NBT list is `mc-persistence`'s job (it already
//!   round-trips `block_entities` opaquely — see `ChunkData`), because a block entity
//!   payload is NBT and this crate must not depend on the NBT encoding;
//! - deciding *when* a block entity ticks is the caller's, using
//!   [`mc_simulation::TickPhase::BlockEntities`], which the server drives for
//!   furnaces and hoppers (P12-03/04);
//! - sending `block_entity_data` to clients is not implemented (see the gap list).
//!
//! So this module is the part that is testable in isolation and that the game loop
//! can hold: a position-keyed map of typed payloads, with the invariants that make it
//! safe to keep in sync with the world.
//!
//! ## What is not implemented, explicitly (AGENTS.md section 3.3)
//!
//! - **Persistence lives in the server**: the payload type stays NBT-free on
//!   purpose; `Game::serialize/load_chunk_block_entities` (P12-05) converts at
//!   the storage boundary.
//! - **Ticking lives in the server**: furnaces cook and hoppers transfer on the
//!   8-tick cooldown (P12-03/04); this crate holds the transfer primitive and
//!   the tick pure functions.
//! - **No client sync**: no `block_entity_data`, so a chest's contents are invisible
//!   to a client even though the server holds them.
//! - **No `remove` on block break**: the caller must call [`BlockEntityStore::remove`]
//!   when a block changes; nothing does that automatically, and a stale entry is
//!   detectable with [`BlockEntityStore::audit_against`].

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::ItemStack;
use std::collections::BTreeMap;

/// A block position.
///
/// `Copy + Ord` so it can key an ordered map: every walk over block entities is then
/// deterministic (AGENTS.md section 3.6). `mc-world` and `mc-persistence` each have
/// their own position types for their own reasons (chunk-local versus global), so
/// this is the third and smallest: three `i32`s, used only as a map key here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockPos {
    /// X.
    pub x: i32,
    /// Y.
    pub y: i32,
    /// Z.
    pub z: i32,
}

impl BlockPos {
    /// A position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Which chunk column this position belongs to.
    #[must_use]
    pub const fn chunk(self) -> (i32, i32) {
        (self.x >> 4, self.z >> 4)
    }
}

impl std::fmt::Display for BlockPos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

/// The kind of a block entity.
///
/// The set is deliberately the one whose *state* Phase 06 actually models. Every
/// variant here has a payload below; there is no variant kept for a name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockEntityKind {
    /// A chest, barrel or shulker box: a slot list.
    Container,
    /// A furnace, smoker or blast furnace: a slot list plus burn/cook state.
    Furnace,
    /// A hopper: a slot list plus a transfer cooldown.
    Hopper,
    /// A dispenser or dropper: a nine-slot list (P17-01).
    Dispenser,
    /// A sign or hanging sign: text lines.
    Sign,
}

impl BlockEntityKind {
    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Furnace => "furnace",
            Self::Hopper => "hopper",
            Self::Dispenser => "dispenser",
            Self::Sign => "sign",
        }
    }

    /// How many item slots this kind holds.
    #[must_use]
    pub const fn slot_count(self) -> usize {
        match self {
            Self::Container => 27,
            Self::Furnace => 3,
            // A hopper has exactly five slots in Vanilla.
            Self::Hopper => 5,
            // A dispenser or dropper has exactly nine slots in Vanilla.
            Self::Dispenser => 9,
            Self::Sign => 0,
        }
    }

    /// Whether this kind has an inventory.
    #[must_use]
    pub const fn has_inventory(self) -> bool {
        !matches!(self, Self::Sign)
    }
}

impl std::fmt::Display for BlockEntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A block entity's payload.
///
/// Kept free of NBT on purpose: the tag encoding is `mc-nbt`'s, and a payload that
/// carried tags would drag that dependency (and the disk format) into this model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEntityData {
    /// A slot list.
    Items(Vec<ItemStack>),
    /// A slot list plus furnace state.
    Furnace {
        /// The three slots: input, fuel, output.
        items: Vec<ItemStack>,
        /// Ticks of burn left on the current fuel item.
        burn_ticks: u32,
        /// Total burn time of the current fuel item, for the client's flame meter.
        burn_total: u32,
        /// Cook progress in ticks.
        cook_progress: u32,
        /// Ticks the current recipe needs.
        cook_total: u32,
    },
    /// A hopper's slot list plus its transfer cooldown.
    Hopper {
        /// The five slots.
        items: Vec<ItemStack>,
        /// Ticks until the next transfer attempt.
        cooldown: u32,
    },
    /// A dispenser or dropper's nine slots (P17-01).
    Dispenser {
        /// The nine slots, row-major like the 3×3 window.
        items: Vec<ItemStack>,
    },
    /// A sign's four text lines.
    Sign {
        /// The lines, in order.
        lines: [String; 4],
    },
}

impl BlockEntityData {
    /// An empty payload for a kind.
    #[must_use]
    pub fn empty(kind: BlockEntityKind) -> Self {
        match kind {
            BlockEntityKind::Container => Self::Items(vec![ItemStack::EMPTY; 27]),
            BlockEntityKind::Furnace => Self::Furnace {
                items: vec![ItemStack::EMPTY; 3],
                burn_ticks: 0,
                burn_total: 0,
                cook_progress: 0,
                cook_total: 0,
            },
            BlockEntityKind::Hopper => Self::Hopper {
                items: vec![ItemStack::EMPTY; 5],
                cooldown: 0,
            },
            BlockEntityKind::Dispenser => Self::Dispenser {
                items: vec![ItemStack::EMPTY; 9],
            },
            BlockEntityKind::Sign => Self::Sign {
                lines: std::array::from_fn(|_| String::new()),
            },
        }
    }

    /// Which kind this payload belongs to.
    #[must_use]
    pub const fn kind(&self) -> BlockEntityKind {
        match self {
            Self::Items(_) => BlockEntityKind::Container,
            Self::Furnace { .. } => BlockEntityKind::Furnace,
            Self::Hopper { .. } => BlockEntityKind::Hopper,
            Self::Dispenser { .. } => BlockEntityKind::Dispenser,
            Self::Sign { .. } => BlockEntityKind::Sign,
        }
    }

    /// The slot list, when this payload has one.
    #[must_use]
    pub fn items(&self) -> Option<&[ItemStack]> {
        match self {
            Self::Items(items)
            | Self::Furnace { items, .. }
            | Self::Hopper { items, .. }
            | Self::Dispenser { items } => Some(items),
            Self::Sign { .. } => None,
        }
    }

    /// The slot list, mutably.
    pub fn items_mut(&mut self) -> Option<&mut Vec<ItemStack>> {
        match self {
            Self::Items(items)
            | Self::Furnace { items, .. }
            | Self::Hopper { items, .. }
            | Self::Dispenser { items } => Some(items),
            Self::Sign { .. } => None,
        }
    }

    /// Total items held, for conservation checks.
    #[must_use]
    pub fn total_items(&self) -> i64 {
        self.items()
            .map_or(0, |items| items.iter().map(|s| i64::from(s.count())).sum())
    }

    /// Whether the slot list is the right length for its kind.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        match self {
            Self::Items(items) => items.len() == BlockEntityKind::Container.slot_count(),
            Self::Furnace { items, .. } => items.len() == BlockEntityKind::Furnace.slot_count(),
            Self::Hopper { items, .. } => items.len() == BlockEntityKind::Hopper.slot_count(),
            Self::Dispenser { items } => items.len() == BlockEntityKind::Dispenser.slot_count(),
            Self::Sign { .. } => true,
        }
    }
}

/// One block entity: its kind, its position and its payload.
///
/// The three are kept consistent by construction — [`BlockEntity::new`] builds the
/// payload from the kind, so a `Container` cannot hold furnace state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntity {
    /// The position it is attached to.
    pub pos: BlockPos,
    /// The payload, whose kind is the entity's kind.
    pub data: BlockEntityData,
}

impl BlockEntity {
    /// A block entity of `kind` at `pos`, with an empty payload.
    #[must_use]
    pub fn new(pos: BlockPos, kind: BlockEntityKind) -> Self {
        Self {
            pos,
            data: BlockEntityData::empty(kind),
        }
    }

    /// Its kind, derived from the payload so the two cannot disagree.
    #[must_use]
    pub const fn kind(&self) -> BlockEntityKind {
        self.data.kind()
    }
}

/// Every loaded block entity, keyed by position.
///
/// `BTreeMap` so iteration is by ascending `(x, y, z)`: a tick that walks this map
/// does so in the same order every run.
#[derive(Debug, Clone, Default)]
pub struct BlockEntityStore {
    entities: BTreeMap<BlockPos, BlockEntity>,
}

impl BlockEntityStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: BTreeMap::new(),
        }
    }

    /// How many block entities are loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Place a block entity, replacing any at the same position.
    ///
    /// Returns the block entity that was displaced, which the caller must handle: a
    /// chest replaced by a furnace has contents that would otherwise vanish silently
    /// (the caller decides whether to drop them).
    pub fn insert(&mut self, entity: BlockEntity) -> Option<BlockEntity> {
        self.entities.insert(entity.pos, entity)
    }

    /// Borrow by position.
    #[must_use]
    pub fn get(&self, pos: BlockPos) -> Option<&BlockEntity> {
        self.entities.get(&pos)
    }

    /// Mutably borrow by position.
    pub fn get_mut(&mut self, pos: BlockPos) -> Option<&mut BlockEntity> {
        self.entities.get_mut(&pos)
    }

    /// Remove, returning what was there.
    ///
    /// The caller must call this when a block is broken, or the entry outlives its
    /// block. [`BlockEntityStore::audit_against`] finds entries that were missed.
    pub fn remove(&mut self, pos: BlockPos) -> Option<BlockEntity> {
        self.entities.remove(&pos)
    }

    /// Every block entity, ascending by position.
    pub fn iter(&self) -> impl Iterator<Item = &BlockEntity> {
        self.entities.values()
    }

    /// Every position, ascending.
    pub fn positions(&self) -> impl Iterator<Item = BlockPos> + '_ {
        self.entities.keys().copied()
    }

    /// Block entities of one kind, ascending by position.
    #[must_use]
    pub fn of_kind(&self, kind: BlockEntityKind) -> Vec<BlockPos> {
        self.entities
            .values()
            .filter(|entity| entity.kind() == kind)
            .map(|entity| entity.pos)
            .collect()
    }

    /// Total items held across every block entity that has an inventory.
    #[must_use]
    pub fn total_items(&self) -> i64 {
        self.entities.values().map(|e| e.data.total_items()).sum()
    }

    /// Whether every payload matches its kind's slot count.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.entities.values().all(|e| e.data.is_well_formed())
    }

    /// Clear everything (world unload).
    pub fn clear(&mut self) {
        self.entities.clear();
    }

    /// Positions whose block is no longer the block that owns them.
    ///
    /// `is_expected` is the caller's predicate — it has the world, this store does
    /// not. An entry whose position fails the predicate is a leaked block entity:
    /// the block was broken or replaced and nobody called [`BlockEntityStore::remove`].
    /// Returning them rather than deleting them keeps the repair a decision.
    #[must_use]
    pub fn audit_against<F>(&self, is_expected: F) -> Vec<BlockPos>
    where
        F: Fn(BlockPos, BlockEntityKind) -> bool,
    {
        self.entities
            .values()
            .filter(|entity| !is_expected(entity.pos, entity.kind()))
            .map(|entity| entity.pos)
            .collect()
    }

    /// Remove everything [`BlockEntityStore::audit_against`] reported, returning the
    /// positions dropped.
    pub fn prune<F>(&mut self, is_expected: F) -> Vec<BlockPos>
    where
        F: Fn(BlockPos, BlockEntityKind) -> bool,
    {
        let stale = self.audit_against(is_expected);
        for pos in &stale {
            self.entities.remove(pos);
        }
        stale
    }
}

/// Open a block entity's inventory as a slot list, refusing a mismatch.
///
/// # Errors
///
/// [`ServerError::Invariant`] when the payload has no inventory (a sign) or a slot
/// count that does not match its kind, which would mean a store built by hand rather
/// than through [`BlockEntity::new`].
pub fn inventory_of(entity: &BlockEntity) -> ServerResult<&[ItemStack]> {
    let items = entity
        .data
        .items()
        .ok_or_else(|| ServerError::Invariant(format!("a {} has no inventory", entity.kind())))?;
    let expected = entity.kind().slot_count();
    if items.len() != expected {
        return Err(ServerError::Invariant(format!(
            "a {} at {} holds {} slots, expected {expected}",
            entity.kind(),
            entity.pos,
            items.len()
        )));
    }
    Ok(items)
}

#[cfg(test)]
mod tests;
