//! Fluid state, its block-state encoding, and the jar-sourced rule numbers (P20-01).
//!
//! ## Why a domain type and not the block state
//!
//! The world stores a fluid as a **block state**: `minecraft:water` with
//! `level=0..15` (fixture `registry/blocks.tsv`, state ids 86..101 and 102..117).
//! The jar keeps a *fluid* state beside the block state — `FluidState` with a
//! `LEVEL` property (1..8) and a `FALLING` boolean — and every flow rule reads
//! that form, not the block's. This module is that form, plus the one conversion
//! between them.
//!
//! ## The encoding, quoted from the jar
//!
//! `javap -p -c -classpath target/vanilla-26.1.2/server-26.1.2.jar
//! net.minecraft.world.level.material.FlowingFluid` (2026-10-01), method
//! `getLegacyLevel(FluidState)`:
//!
//! ```text
//!  0: aload_0 / 1: invokevirtual FluidState.isSource()  -> if source: iconst_0; ireturn
//!  9: bipush 8 ; FluidState.getAmount() ; bipush 8 ; Math.min(II)I ; isub      // 8 - min(amount, 8)
//! 21: FluidState.getValue(FALLING) ? bipush 8 : iconst_0 ; iadd                // + 8 when falling
//! ```
//!
//! so `level = 0` for a source, `level = 8 - amount` for a flowing fluid and
//! `level = 16 - amount` for a falling one. `WaterFluid$Source.getAmount` is the
//! constant `8` and `WaterFluid$Source.isSource` returns `true`;
//! `WaterFluid$Flowing.getAmount` returns `FluidState.getValue(LEVEL)` and
//! `isSource` returns `false` (`javap` on both inner classes, same date). And
//! `WaterFluid.createLegacyBlock(FluidState)` is exactly
//! `Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, getLegacyLevel(state))`
//! — the same for `LavaFluid`. The 16 `level` values in our fixture therefore
//! carry all of it: 0 is the source, 1..7 are flowing, 8..15 are falling.
//!
//! ## The rule numbers, and where each comes from
//!
//! | rule | water | lava | jar evidence (`javap -c`, 2026-10-01) |
//! |---|---|---|---|
//! | `getTickDelay` | `5` | `isFastLava ? 10 : 30` | `WaterFluid.getTickDelay` is `iconst_5; ireturn`; `LavaFluid.getTickDelay` branches on `isFastLava(levelReader)` |
//! | `getSlopeFindDistance` | `4` | `isFastLava ? 4 : 2` | `WaterFluid`/`LavaFluid.getSlopeFindDistance` |
//! | `getDropOff` | `1` | `isFastLava ? 1 : 2` | `WaterFluid`/`LavaFluid.getDropOff` |
//! | `canConvertToSource` | game rule `water_source_conversion` | game rule `lava_source_conversion` | `WaterFluid.canConvertToSource` reads `GameRules.WATER_SOURCE_CONVERSION`; `LavaFluid.canConvertToSource` reads `LAVA_SOURCE_CONVERSION` |
//!
//! `isFastLava(LevelReader)` is
//! `levelReader.environmentAttributes().getDimensionValue(EnvironmentAttributes.FAST_LAVA)`
//! — a **per-dimension attribute**, which is what "the lava delay is 10 in the
//! Nether and 30 elsewhere" means in 26.1.2 (it used to be a hard-coded
//! `dimensionType().ultraWarm()` check). This project has one dimension as of
//! P20-01, so the server passes `fast = false` (overworld); P21-00a's
//! multi-dimension runtime is what will pass `true` for the Nether, and this
//! type already carries the parameter so that is not a rewrite.
//!
//! `getSpreadDelay` is **not** in the table because it is not a constant:
//! `LavaFluid` overrides it (quoted in full in [`crate::fluid::flow`]).

/// Which fluid, without its level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FluidKind {
    /// `minecraft:water`, registry state ids 86..=101.
    Water,
    /// `minecraft:lava`, registry state ids 102..=117.
    Lava,
}

impl FluidKind {
    /// Stable name for logs and test failure messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Water => "water",
            Self::Lava => "lava",
        }
    }

    /// The block whose `level` property encodes this fluid.
    #[must_use]
    pub const fn block_name(self) -> &'static str {
        match self {
            Self::Water => "minecraft:water",
            Self::Lava => "minecraft:lava",
        }
    }

    /// The fluid this one is not.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Water => Self::Lava,
            Self::Lava => Self::Water,
        }
    }
}

/// The level, falling flag and source flag of a present fluid.
///
/// `amount` is the jar's `FluidState.getAmount()`: `1..=8`, where `8` is a full
/// block. A source is always `amount == 8` (`WaterFluid$Source.getAmount`), so
/// the constructor refuses to build a contradictory value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FluidBody {
    /// `1..=8`; `8` is full.
    amount: u8,
    falling: bool,
    source: bool,
}

impl FluidBody {
    /// A fluid of `amount` units (`1..=8`, clamped), not falling, not a source.
    #[must_use]
    pub const fn flowing(amount: u8) -> Self {
        Self {
            amount: if amount == 0 {
                1
            } else if amount > 8 {
                8
            } else {
                amount
            },
            falling: false,
            source: false,
        }
    }

    /// The falling form: `getFlowing(8, true)`, whose block level is 8.
    #[must_use]
    pub const fn falling() -> Self {
        Self {
            amount: 8,
            falling: true,
            source: false,
        }
    }

    /// A source block: `getSource(false)`.
    #[must_use]
    pub const fn source() -> Self {
        Self {
            amount: 8,
            falling: false,
            source: true,
        }
    }

    /// The jar's `FluidState.getAmount()`.
    #[must_use]
    pub const fn amount(self) -> u8 {
        self.amount
    }

    /// The jar's `FALLING` property.
    #[must_use]
    pub const fn is_falling(self) -> bool {
        self.falling
    }

    /// The jar's `FluidState.isSource()`.
    #[must_use]
    pub const fn is_source(self) -> bool {
        self.source
    }

    /// The same body one drop-off lower, or `None` when that would be empty.
    #[must_use]
    pub const fn lowered_by(self, drop_off: u8) -> Option<Self> {
        match self.amount.checked_sub(drop_off) {
            Some(0) | None => None,
            Some(amount) => Some(Self {
                amount,
                falling: false,
                source: false,
            }),
        }
    }
}

/// A fluid occupying a block: empty, or one of the two fluids at some level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, PartialOrd, Ord)]
pub enum FluidState {
    /// No fluid (`Fluids.EMPTY.defaultFluidState()`).
    #[default]
    Empty,
    /// Water at a level.
    Water(FluidBody),
    /// Lava at a level.
    Lava(FluidBody),
}

impl FluidState {
    /// The fluid a `level=` block-state property carries.
    ///
    /// The inverse of [`FluidState::block_level`]; see the module docs for the
    /// jar's `getLegacyLevel` this mirrors.
    #[must_use]
    pub const fn from_block_level(kind: FluidKind, level: u8) -> Self {
        let level = if level > 15 { 15 } else { level };
        let body = if level == 0 {
            FluidBody::source()
        } else if level < 8 {
            FluidBody {
                amount: 8 - level,
                falling: false,
                source: false,
            }
        } else {
            FluidBody {
                amount: 16 - level,
                falling: true,
                source: false,
            }
        };
        match kind {
            FluidKind::Water => Self::Water(body),
            FluidKind::Lava => Self::Lava(body),
        }
    }

    /// Whether this is the empty fluid.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The kind, or `None` when empty.
    #[must_use]
    pub const fn kind(self) -> Option<FluidKind> {
        match self {
            Self::Empty => None,
            Self::Water(_) => Some(FluidKind::Water),
            Self::Lava(_) => Some(FluidKind::Lava),
        }
    }

    /// The body, or `None` when empty.
    #[must_use]
    pub const fn body(self) -> Option<FluidBody> {
        match self {
            Self::Empty => None,
            Self::Water(body) | Self::Lava(body) => Some(body),
        }
    }

    /// The jar's `FluidState.getAmount()`.
    #[must_use]
    pub const fn amount(self) -> u8 {
        match self.body() {
            Some(body) => body.amount(),
            None => 0,
        }
    }

    /// The jar's `FluidState.isSource()`.
    #[must_use]
    pub const fn is_source(self) -> bool {
        matches!(self.body(), Some(body) if body.is_source())
    }

    /// The jar's `FluidState.getValue(FALLING)`.
    #[must_use]
    pub const fn is_falling(self) -> bool {
        matches!(self.body(), Some(body) if body.is_falling())
    }

    /// `Fluid.isSame`: same fluid *type*, whatever the level.
    #[must_use]
    pub const fn is_same(self, other: Self) -> bool {
        match (self.kind(), other.kind()) {
            (Some(a), Some(b)) => a as u8 == b as u8,
            _ => false,
        }
    }

    /// `isSourceBlockOfThisType`: same type *and* a source.
    #[must_use]
    pub const fn is_source_of(self, other: Self) -> bool {
        self.is_same(other) && self.is_source()
    }

    /// `isSourceBlockOfThisType` against a fluid *kind* rather than a state.
    #[must_use]
    pub const fn is_source_of_kind(self, kind: FluidKind) -> bool {
        matches!(self.kind(), Some(k) if k as u8 == kind as u8) && self.is_source()
    }

    /// The block-state `level` property this fluid writes.
    ///
    /// `FlowingFluid.getLegacyLevel`, quoted in the module docs: 0 for a source,
    /// `8 - amount` flowing, `16 - amount` falling.
    #[must_use]
    pub const fn block_level(self) -> u8 {
        match self.body() {
            None => 0,
            Some(body) => {
                if body.is_source() {
                    0
                } else if body.is_falling() {
                    16 - body.amount()
                } else {
                    8 - body.amount()
                }
            }
        }
    }

    /// Water at `level`, for tests and the world adapter.
    #[must_use]
    pub const fn water(level: u8) -> Self {
        Self::from_block_level(FluidKind::Water, level)
    }

    /// Lava at `level`, for tests and the world adapter.
    #[must_use]
    pub const fn lava(level: u8) -> Self {
        Self::from_block_level(FluidKind::Lava, level)
    }
}

/// The numbers a flow rule needs, per fluid and per dimension.
///
/// Every field is jar-sourced; see the module docs table for the `javap` quote
/// behind each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidRules {
    /// Which fluid these rules describe.
    pub kind: FluidKind,
    /// `FAST_LAVA` for this dimension: `true` in the Nether, `false` elsewhere.
    pub fast: bool,
    /// `canConvertToSource`: the `*_source_conversion` game rule's value.
    pub convert_to_source: bool,
}

impl FluidRules {
    /// Overworld water.
    #[must_use]
    pub const fn water(convert_to_source: bool) -> Self {
        Self {
            kind: FluidKind::Water,
            fast: false,
            convert_to_source,
        }
    }

    /// Lava in a dimension with `FAST_LAVA = fast`.
    #[must_use]
    pub const fn lava(fast: bool, convert_to_source: bool) -> Self {
        Self {
            kind: FluidKind::Lava,
            fast,
            convert_to_source,
        }
    }

    /// `getTickDelay(LevelReader)`: 5 for water, 10/30 for lava.
    #[must_use]
    pub const fn tick_delay(self) -> u32 {
        match self.kind {
            FluidKind::Water => 5,
            FluidKind::Lava => {
                if self.fast {
                    10
                } else {
                    30
                }
            }
        }
    }

    /// `getSlopeFindDistance(LevelReader)`: 4 for water, 4/2 for lava.
    #[must_use]
    pub const fn slope_find_distance(self) -> u8 {
        match self.kind {
            FluidKind::Water => 4,
            FluidKind::Lava => {
                if self.fast {
                    4
                } else {
                    2
                }
            }
        }
    }

    /// `getDropOff(LevelReader)`: 1 for water, 1/2 for lava.
    #[must_use]
    pub const fn drop_off(self) -> u8 {
        match self.kind {
            FluidKind::Water => 1,
            FluidKind::Lava => {
                if self.fast {
                    1
                } else {
                    2
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FluidBody, FluidKind, FluidRules, FluidState};

    #[test]
    fn the_block_level_encoding_round_trips_for_both_fluids() {
        // The property is the authority: every level 0..=15 must decode and
        // re-encode to itself, or a fluid would change level by being read.
        for kind in [FluidKind::Water, FluidKind::Lava] {
            for level in 0..=15u8 {
                let state = FluidState::from_block_level(kind, level);
                assert_eq!(state.block_level(), level, "{kind:?} level {level}");
                assert_eq!(state.kind(), Some(kind));
            }
        }
    }

    #[test]
    fn level_zero_is_a_source_and_eight_and_above_is_falling() {
        // The three regimes getLegacyLevel produces, quoted in the module docs.
        let source = FluidState::water(0);
        assert!(source.is_source());
        assert_eq!(source.amount(), 8);
        assert!(!source.is_falling());
        assert_eq!(FluidState::water(1).amount(), 7);
        assert!(!FluidState::water(1).is_falling());
        assert_eq!(FluidState::water(7).amount(), 1);
        let falling = FluidState::water(8);
        assert!(!falling.is_source());
        assert!(falling.is_falling());
        assert_eq!(falling.amount(), 8);
        assert_eq!(FluidState::water(15).amount(), 1);
        assert!(FluidState::water(15).is_falling());
        assert_eq!(FluidState::Empty.block_level(), 0);
        assert_eq!(FluidState::Empty.amount(), 0);
        assert!(FluidState::Empty.is_empty());
    }

    #[test]
    fn the_jar_sourced_rule_numbers_are_what_the_jar_says() {
        // Each of these is quoted with its javap evidence in the module docs; the
        // assertion is here so a silent edit to a constant fails a named test.
        let water = FluidRules::water(true);
        assert_eq!(water.tick_delay(), 5, "WaterFluid.getTickDelay = 5");
        assert_eq!(water.slope_find_distance(), 4, "WaterFluid slope = 4");
        assert_eq!(water.drop_off(), 1, "WaterFluid drop off = 1");

        let overworld_lava = FluidRules::lava(false, false);
        assert_eq!(overworld_lava.tick_delay(), 30, "LavaFluid, no FAST_LAVA");
        assert_eq!(overworld_lava.slope_find_distance(), 2);
        assert_eq!(overworld_lava.drop_off(), 2);

        let nether_lava = FluidRules::lava(true, true);
        assert_eq!(nether_lava.tick_delay(), 10, "LavaFluid, FAST_LAVA");
        assert_eq!(nether_lava.slope_find_distance(), 4);
        assert_eq!(nether_lava.drop_off(), 1);
    }

    #[test]
    fn fluid_identity_is_type_equality_not_level_equality() {
        assert!(FluidState::water(0).is_same(FluidState::water(7)));
        assert!(FluidState::water(7).is_same(FluidState::water(0)));
        assert!(!FluidState::water(0).is_same(FluidState::lava(0)));
        assert!(!FluidState::Empty.is_same(FluidState::Empty));
        assert!(FluidState::water(0).is_source_of(FluidState::water(3)));
        assert!(!FluidState::water(3).is_source_of(FluidState::water(0)));
        assert!(!FluidState::lava(0).is_source_of(FluidState::water(0)));
        assert!(FluidState::water(0).is_source_of_kind(FluidKind::Water));
        assert!(!FluidState::water(4).is_source_of_kind(FluidKind::Water));
        assert!(!FluidState::lava(0).is_source_of_kind(FluidKind::Water));
        assert_eq!(FluidKind::Water.other(), FluidKind::Lava);
        assert_eq!(FluidKind::Lava.block_name(), "minecraft:lava");
        assert_eq!(FluidBody::falling().amount(), 8);
        assert!(FluidBody::falling().is_falling());
        assert_eq!(FluidBody::flowing(0).amount(), 1, "amount is clamped up");
        assert_eq!(FluidBody::flowing(200).amount(), 8, "and down");
        assert_eq!(
            FluidBody::flowing(5).lowered_by(1).map(FluidBody::amount),
            Some(4)
        );
        assert!(FluidBody::flowing(1).lowered_by(1).is_none());
    }
}
