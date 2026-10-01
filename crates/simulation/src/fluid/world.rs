//! The world surface the flow rules read and write (P20-01).
//!
//! ## Why a trait rather than `mc_world::World`
//!
//! `mc-simulation` owns the shape of a tick and depends on nothing but
//! `mc-core`; `mc-redstone` is the precedent for a subsystem crate that owns
//! *both* a world-facing trait and the algorithm over it
//! ([`mc_redstone::propagation`]). Putting the fluid engine here keeps the
//! dependency graph unchanged (ADR-0009 §5: "no new dependency") and keeps the
//! rules testable against a ten-block in-memory world instead of a chunk store.
//! The server implements [`FluidWorld`] over `mc_world::World` plus the block
//! registry in `crates/server/src/game/fluids.rs`.
//!
//! ## What the implementor has to answer, and why each question exists
//!
//! | method | jar rule it answers |
//! |---|---|
//! | [`FluidWorld::facts`] | `BlockState.getFluidState`, `blocksMotion`, `canHoldAnyFluid`, `canHoldSpecificFluid` |
//! | [`FluidWorld::place_fluid`] | `FlowingFluid.spreadTo` (`LiquidBlockContainer.placeLiquid` else `setBlock(createLegacyBlock)`) |
//! | [`FluidWorld::set_block_named`] | the lava/water conversions' `Blocks.STONE`/`COBBLESTONE`/`OBSIDIAN` writes |
//!
//! ## Approximations this trait makes explicit
//!
//! - **`can_hold_any_fluid` is derived from our collision data, not from
//!   `LiquidBlockContainer`.** The jar asks `state.getBlock() instanceof
//!   LiquidBlockContainer` (true for every waterloggable block) and otherwise
//!   `!state.blocksMotion()`. We have the second half exactly (`NON_SOLID` +
//!   collision shapes) and approximate the first by "the block has a
//!   `WATERLOGGED` property", which the registry fixture lists for all 410
//!   waterloggable blocks. The two agree on every block this phase tests; a
//!   block that is a container without a `WATERLOGGED` property would be
//!   misread, and none is known.
//! - **`can_pass_through_wall`** needs per-face occlusion
//!   (`Shapes.mergedFaceOccludes`), which this project does not model. The
//!   engine refuses a move into any block whose collision shape is a full cube
//!   and allows it between two empty shapes; a partial non-waterloggable shape
//!   (a fence, a wall) is treated as **blocking**, which is conservative — it
//!   can delay a spread the jar would allow, never the reverse.
//! - **An unloaded chunk is not simulated.** `facts` returns `None` and every
//!   rule treats it as "cannot pass". The jar force-loads; we do not, which is
//!   the ADR-0009 §2.4 boundary rather than a rule of the flow model.
//! - **`beforeDestroyingBlock` does not drop items.** Vanilla breaks a plant
//!   the fluid reaches and drops its item; `place_fluid` replaces the block and
//!   the drop is a named gap of P20-01 (no fluid-driven loot path exists yet).

use super::state::{FluidKind, FluidState};

/// A block position in the fluid engine's own coordinate type.
///
/// `mc-world` passes loose `(i32, i32, i32)` and `mc-redstone` defines its own
/// `BlockPos` for the same reason; nothing in this module depends on the derived
/// order beyond it being total, fixed and independent of insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pos {
    /// World block x.
    pub x: i32,
    /// World block y.
    pub y: i32,
    /// World block z.
    pub z: i32,
}

impl Pos {
    /// A position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The position `dir` away, saturating at the `i32` bounds.
    ///
    /// Saturating rather than wrapping, for the reason `mc_redstone::BlockPos`
    /// gives: a wrapping `i32::MAX + 1` would land inside the world at
    /// `i32::MIN` and let a fluid escape to a block nobody named (AGENTS.md §10).
    #[must_use]
    pub const fn offset(self, dir: Dir) -> Self {
        let (dx, dy, dz) = dir.delta();
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            z: self.z.saturating_add(dz),
        }
    }
}

/// The six faces, in the jar's `Direction` declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dir {
    /// `-Y`.
    Down,
    /// `+Y`.
    Up,
    /// `-Z`.
    North,
    /// `+Z`.
    South,
    /// `-X`.
    West,
    /// `+X`.
    East,
}

impl Dir {
    /// `Direction.Plane.HORIZONTAL`, in the jar's iteration order.
    ///
    /// `javap -p -c net.minecraft.core.Direction$Plane` (2026-10-01) shows the
    /// static initializer building `HORIZONTAL` from `NORTH, EAST, SOUTH, WEST`.
    /// The order is part of the determinism contract, not an incidental detail:
    /// when two neighbours tie on slope distance, the first one wins the map and
    /// the tie-break is observable.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    /// The `(dx, dy, dz)` of this face.
    #[must_use]
    pub const fn delta(self) -> (i32, i32, i32) {
        match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }

    /// The opposite face.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    /// Stable name, matching the jar's enum constants.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }
}

/// What the flow rules need to know about one block.
///
/// Every field is a *derived* answer the implementor computes from its registry;
/// the engine never reads a block name. They are deliberately separate flags
/// because the jar's predicates are separate: `isAir`, `blocksMotion`,
/// `canHoldAnyFluid`, `canPlaceLiquid` and the `WATERLOGGED` property answer
/// different questions and disagree on real blocks (a waterlogged stairs is a
/// container, not a full cube, and not air).
#[allow(
    clippy::struct_excessive_bools,
    reason = "the jar's predicates are independent booleans too; an enum would hide that"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockFacts {
    /// Registry state id, passed through so a caller can log or diff it.
    pub id: i32,
    /// The fluid this block carries. A waterlogged block carries water at the
    /// source level; `minecraft:water level=3` carries water at level 3.
    pub fluid: FluidState,
    /// `BlockState.isAir()`.
    pub air: bool,
    /// `blocksMotion()`: our proxy is a full collision cube.
    pub full_cube: bool,
    /// `canHoldAnyFluid(state)`: a fluid may be placed here at all.
    pub can_hold_any_fluid: bool,
    /// The block is a `LiquidBlockContainer` with `WATERLOGGED` — the jar's
    /// `SimpleWaterloggedBlock` case of `canPlaceLiquid`.
    pub water_container: bool,
    /// The block's own `WATERLOGGED` property is `true`.
    pub waterlogged: bool,
}

impl BlockFacts {
    /// An empty (air-like) block that holds fluid.
    #[must_use]
    pub const fn air(id: i32) -> Self {
        Self {
            id,
            fluid: FluidState::Empty,
            air: true,
            full_cube: false,
            can_hold_any_fluid: true,
            water_container: false,
            waterlogged: false,
        }
    }

    /// A full cube that carries no fluid.
    #[must_use]
    pub const fn solid(id: i32) -> Self {
        Self {
            id,
            fluid: FluidState::Empty,
            air: false,
            full_cube: true,
            can_hold_any_fluid: false,
            water_container: false,
            waterlogged: false,
        }
    }

    /// A fluid block: what `Level.getFluidState(pos)` returns here.
    #[must_use]
    pub const fn of_fluid(id: i32, fluid: FluidState) -> Self {
        Self {
            id,
            fluid,
            air: false,
            full_cube: false,
            can_hold_any_fluid: true,
            water_container: false,
            waterlogged: false,
        }
    }

    /// A waterloggable block: `canHoldAnyFluid` true through the container path.
    #[must_use]
    pub const fn container(id: i32, waterlogged: bool, full_cube: bool) -> Self {
        Self {
            id,
            fluid: if waterlogged {
                // `SimpleWaterloggedBlock.getFluidState` returns a full water
                // source when the property is true.
                FluidState::Water(super::state::FluidBody::source())
            } else {
                FluidState::Empty
            },
            air: false,
            full_cube,
            can_hold_any_fluid: true,
            water_container: true,
            waterlogged,
        }
    }

    /// `FlowingFluid.canHoldSpecificFluid(state, fluid)`.
    ///
    /// `LiquidBlockContainer.canPlaceLiquid(null, level, pos, state, fluid)` for
    /// a waterloggable block is "the fluid is water **and** the block is not
    /// already waterlogged"; every other block answers `true`.
    #[must_use]
    pub const fn can_hold_specific_fluid(self, kind: FluidKind) -> bool {
        if !self.water_container {
            return true;
        }
        matches!(kind, FluidKind::Water) && !self.waterlogged
    }

    /// `FlowingFluid.canHoldFluid(state, fluid)`.
    #[must_use]
    pub const fn can_hold_fluid(self, kind: FluidKind) -> bool {
        self.can_hold_any_fluid && self.can_hold_specific_fluid(kind)
    }
}

/// The world the fluid rules act on.
pub trait FluidWorld {
    /// Facts about the block at `pos`, or `None` when its chunk is not loaded.
    ///
    /// `None` is "cannot interact", never "air": an unloaded chunk must not be
    /// written through, and a fluid must not spread into one (ADR-0009 §2.4).
    fn facts(&self, pos: Pos) -> Option<BlockFacts>;

    /// Write `fluid` at `pos`, returning whether anything changed.
    ///
    /// This is `FlowingFluid.spreadTo` plus `LiquidBlockContainer.placeLiquid`:
    /// writing water into a waterloggable block sets its `WATERLOGGED` property
    /// and keeps the block; every other case writes the fluid's legacy block
    /// (`minecraft:water`/`minecraft:lava` at `fluid.block_level()`).
    /// [`FluidState::Empty`] removes the fluid, leaving air.
    ///
    /// The vanilla `beforeDestroyingBlock` drop is **not** performed; see the
    /// module docs.
    fn place_fluid(&mut self, pos: Pos, fluid: FluidState) -> bool;

    /// Set `pos` to the named block, for the conversion rules (`minecraft:stone`,
    /// `minecraft:cobblestone`, `minecraft:obsidian`).
    ///
    /// Returns whether anything changed. An unknown name or an unloaded chunk is
    /// `false`, never a panic.
    fn set_block_named(&mut self, pos: Pos, name: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::{BlockFacts, Dir, Pos};
    use crate::fluid::state::{FluidKind, FluidState};

    #[test]
    fn positions_saturate_and_faces_are_the_jars() {
        assert_eq!(Pos::new(0, 64, 0).offset(Dir::Up), Pos::new(0, 65, 0));
        assert_eq!(Pos::new(0, 64, 0).offset(Dir::North), Pos::new(0, 64, -1));
        assert_eq!(Pos::new(0, 64, 0).offset(Dir::East), Pos::new(1, 64, 0));
        assert_eq!(
            Pos::new(i32::MAX, 0, 0).offset(Dir::East).x,
            i32::MAX,
            "must not wrap into the far side of the world"
        );
        assert_eq!(Dir::Down.opposite(), Dir::Up);
        assert_eq!(Dir::North.opposite(), Dir::South);
        assert_eq!(Dir::West.name(), "west");
        // The jar's Plane.HORIZONTAL order, quoted in `Dir::HORIZONTAL`.
        assert_eq!(
            Dir::HORIZONTAL,
            [Dir::North, Dir::East, Dir::South, Dir::West]
        );
    }

    #[test]
    fn container_can_hold_specific_fluid_matches_can_place_liquid() {
        let dry_stairs = BlockFacts::container(7, false, false);
        assert!(dry_stairs.can_hold_specific_fluid(FluidKind::Water));
        assert!(!dry_stairs.can_hold_specific_fluid(FluidKind::Lava));
        let wet_stairs = BlockFacts::container(7, true, false);
        assert!(!wet_stairs.can_hold_specific_fluid(FluidKind::Water));
        assert!(!wet_stairs.can_hold_fluid(FluidKind::Water));
        assert_eq!(wet_stairs.fluid, FluidState::water(0));
        assert!(BlockFacts::air(0).can_hold_fluid(FluidKind::Lava));
        assert!(!BlockFacts::solid(1).can_hold_fluid(FluidKind::Water));
    }
}
