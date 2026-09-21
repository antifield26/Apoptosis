//! The redstone power model: signal strength, power kinds and power sources (P06-09).
//!
//! There are three ideas in this module and it is worth keeping them apart,
//! because Vanilla itself keeps them apart:
//!
//! 1. **Strength** — an integer in `0..=15` ([`PowerLevel`]). 0 is "off"; every
//!    non-zero strength "activates" a mechanism equally, so strength only matters
//!    for transmission ([`SignalKind`] and attenuation).
//! 2. **Kind** — [`SignalKind::Strong`] versus [`SignalKind::Weak`]. A block can be
//!    powered strongly or weakly, and the difference is whether *redstone dust
//!    adjacent to that block* is powered.
//! 3. **Source** — [`PowerSource`], the blocks that generate a signal in the first
//!    place.
//!
//! ## Evidence labels
//!
//! This crate follows the project rule that a claim about Vanilla needs evidence
//! (AGENTS.md §3.1), and that an *unverified* number must never be presented as a
//! Vanilla fact. Every constant and rule below carries one of these labels:
//!
//! - **verified** — stated explicitly by a source of truth, cited inline.
//! - **derived** — follows by construction from something verified, with the
//!   derivation written out.
//! - **approximation** — deliberately simpler than Vanilla; the deviation is named.
//! - **product decision** — a bound or shape this project chose; not a Vanilla
//!   numeric value at all.
//!
//! What this module claims to be *verified* is narrow, and the module docs on
//! [`PowerState`] spell out exactly which part of the strong/weak rule is verified
//! and which part is not. Do not read "the enum compiles" as "the numbers are
//! Vanilla's".
//!
//! ## What is not modelled here
//!
//! Conductivity (which blocks can be powered at all), quasi-connectivity, the
//! block-update/shape-update distinction and the full component set are **not**
//! implemented. See the crate docs for the complete list.

use mc_core::error::{ServerError, ServerResult};
use std::fmt;

/// Maximum redstone signal strength: a signal is an integer in `0..=15`.
///
/// **verified** — minecraft.wiki, *Redstone mechanics* §"Redstone signal": "A
/// redstone signal has a 'signal strength', which is an integer between 1 and 15",
/// and *Redstone circuits* §"Transmission": "Redstone dust can thus transmit a
/// signal up to 15 blocks by itself."
pub const MAX_POWER: u8 = 15;

/// A redstone signal strength in `0..=15`.
///
/// Values above [`MAX_POWER`] are refused by [`PowerLevel::new`] rather than
/// wrapped or truncated: a wrapped strength produces a circuit that is silently
/// wrong in a way no test would notice, whereas a refusal is a caller bug that
/// surfaces immediately. [`PowerLevel::from_raw`] exists for the hostile-input
/// case where clamping is the only safe option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PowerLevel(u8);

impl PowerLevel {
    /// Off.
    pub const ZERO: Self = Self(0);
    /// The strongest signal ([`MAX_POWER`]).
    pub const MAX: Self = Self(MAX_POWER);

    /// A level from a signal strength, or [`ServerError::InvalidAction`] when it is
    /// above [`MAX_POWER`].
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `value > 15`. The error names the value, so a
    /// log line identifies the offending input without the caller adding context. The
    /// function is `const` so the crate's own constants can use it in a `const` context;
    /// the error path allocates a `String`, which is why a refusal cannot be a `const`
    /// evaluation.
    pub fn new(value: u8) -> ServerResult<Self> {
        if value > MAX_POWER {
            return Err(ServerError::InvalidAction(format!(
                "redstone power level {value} is above the maximum of {MAX_POWER}"
            )));
        }
        Ok(Self(value))
    }

    /// A level from a possibly hostile value, clamped into `0..=15`.
    ///
    /// Use this for anything that came off the wire, out of a file or out of a
    /// plugin; use [`PowerLevel::new`] for values this crate produced itself.
    #[must_use]
    pub const fn from_raw(value: u8) -> Self {
        if value > MAX_POWER {
            Self::MAX
        } else {
            Self(value)
        }
    }

    /// The raw signal strength.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Whether the signal is non-zero.
    ///
    /// Note that "powered" here means "carries a signal", which is *not* the same
    /// as "activates a mechanism": a mechanism activates on any non-zero signal, so
    /// for activation this is the right predicate, but a block can also be
    /// *powered* (Vanilla's meaning) while activating nothing.
    #[must_use]
    pub const fn is_powered(self) -> bool {
        self.0 != 0
    }

    /// Attenuate by `amount`, saturating at 0.
    ///
    /// **derived** — this is the "decreases by 1 for every block" rule of
    /// minecraft.wiki *Redstone mechanics* §"Signal transmission" applied to a
    /// single step; see [`crate::propagation`] for the rule and its label.
    #[must_use]
    pub const fn saturating_sub(self, amount: u8) -> Self {
        Self(self.0.saturating_sub(amount))
    }
}

impl TryFrom<u8> for PowerLevel {
    type Error = ServerError;

    /// Same contract as [`PowerLevel::new`].
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when the value is above [`MAX_POWER`].
    fn try_from(value: u8) -> ServerResult<Self> {
        Self::new(value)
    }
}

impl From<PowerLevel> for u8 {
    fn from(level: PowerLevel) -> Self {
        level.0
    }
}

impl fmt::Display for PowerLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which of the two ways a block can be powered.
///
/// **measured on a real 26.1.2 server (P13-06)** — the conductivity matrix
/// (`target/p13-wire/vanilla`, scripts `target/p13_wire_run.py` /
/// `target/p13_wire_read.py`) replaces the wiki paragraph this documentation
/// used to quote, which the measurements refute in three places (a torch does
/// not power the block it is attached to; dust beside a lit torch carries 15;
/// a redstone block never powers an adjacent solid). The rule that holds:
///
/// - A solid fed by a torch below it or by an attached on-lever is **strong**:
///   dust off it reads the full level (side and top probes read 15).
/// - A solid fed by dust above or beside it is **weak**: dust off it steps
///   down one more (probes read 14 off a 15-fed stone), while mechanisms read
///   the full level.
/// - A redstone block never powers an adjacent solid in any direction, though
///   adjacent dust and torches read the block itself directly.
///
/// Which kind a solid carries is decided positionally in
/// [`crate::propagation::solid_power`], not by [`PowerSource::is_strong_source`]:
/// that flag names the kind of a *direct emission* (a block of redstone emits
/// strongly to adjacent dust), which is a different question from what kind a
/// stone conducts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SignalKind {
    /// Powers an adjacent mechanism at full level; adjacent dust steps down
    /// one more (a dust-fed stone conducts weakly).
    Weak,
    /// Additionally powers adjacent redstone dust at the full level (a
    /// torch/lever-fed stone conducts strongly).
    Strong,
}

impl SignalKind {
    /// Whether this kind powers adjacent redstone dust.
    ///
    /// **verified** — the wiki sentence quoted on [`SignalKind`]: strong powers
    /// dust, weak does not.
    #[must_use]
    pub const fn powers_dust(self) -> bool {
        matches!(self, Self::Strong)
    }

    /// Stable name for logs and test failures.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Weak => "weak",
            Self::Strong => "strong",
        }
    }
}

impl fmt::Display for SignalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The power a block emits, in both kinds at once.
///
/// The two fields answer two different questions, and conflating them is the usual
/// way a redstone model goes subtly wrong:
///
/// - `strong` is what the block emits **through itself** to a neighbour on its far
///   side. A lever attached to a stone block strongly powers that stone;
///   redstone dust *beside* the stone then reads the full level, because the
///   stone passes the signal through. **measured (P13-06)**: a floor lever's
///   stone reads 15 on a side probe, and so does a wall lever's mount.
/// - `weak` is what the block emits **to its immediate neighbours only**. Dust
///   *on top of* a dust-fed stone reads one less than the stone carries
///   (**measured**: 14 off a 15-fed stone), while a lamp on the same stone
///   reads the full level.
///
/// ## What is *not* verified here
///
/// This project has **not** verified, against a 26.1.2 baseline or against Mojang
/// material:
///
/// - that a consumer reads `max(weak, strong)` rather than testing the two kinds
///   separately;
/// - the exact set of blocks each [`PowerSource`] variant powers in each direction,
///   beyond the P13-06 matrix (torch below only; lever attachment only; dust
///   above and beside only; redstone block never);
/// - whether a powered block passes on the *strength* it received or always passes
///   15. This model passes the received strength through (a dust-fed stone
///   carries the dust's level), which matches the one attenuated case measured
///   (15 in, 14 out) and is an assumption below 15.
///
/// The `max` rule is therefore implemented and tested as a **product decision**
/// (test name: `our_consumer_reads_the_maximum_of_weak_and_strong`), not as a
/// Vanilla claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerState {
    /// Power delivered to immediate neighbours only.
    pub weak: PowerLevel,
    /// Power delivered through the block to the far side.
    pub strong: PowerLevel,
}

impl PowerState {
    /// Off.
    pub const OFF: Self = Self {
        weak: PowerLevel::ZERO,
        strong: PowerLevel::ZERO,
    };

    /// A state with explicit levels.
    #[must_use]
    pub const fn new(weak: PowerLevel, strong: PowerLevel) -> Self {
        Self { weak, strong }
    }

    /// A state that emits `level` weakly only.
    #[must_use]
    pub const fn weak_only(level: PowerLevel) -> Self {
        Self {
            weak: level,
            strong: PowerLevel::ZERO,
        }
    }

    /// A state that emits `level` weakly and strongly.
    #[must_use]
    pub const fn strong_at(level: PowerLevel) -> Self {
        Self {
            weak: level,
            strong: level,
        }
    }

    /// The level a consumer reads: `max(weak, strong)`.
    ///
    /// **product decision**, not verified — see the type docs. The reasoning for
    /// choosing it anyway: a mechanism activates on *any* signal, so a consumer must
    /// consider both kinds, and `max` is the only choice that cannot make an
    /// activating signal smaller. It is also deliberately the *only* place the two
    /// kinds are merged, so the split stays visible everywhere else.
    #[must_use]
    pub const fn effective(self) -> PowerLevel {
        if self.weak.get() >= self.strong.get() {
            self.weak
        } else {
            self.strong
        }
    }

    /// Component-wise maximum, for OR-ing several inputs together.
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        Self {
            weak: if self.weak.get() >= other.weak.get() {
                self.weak
            } else {
                other.weak
            },
            strong: if self.strong.get() >= other.strong.get() {
                self.strong
            } else {
                other.strong
            },
        }
    }

    /// Whether either kind carries a signal.
    #[must_use]
    pub const fn is_powered(self) -> bool {
        self.weak.is_powered() || self.strong.is_powered()
    }

    /// Attenuate both kinds by `amount`, saturating at 0.
    #[must_use]
    pub const fn attenuate(self, amount: u8) -> Self {
        Self {
            weak: self.weak.saturating_sub(amount),
            strong: self.strong.saturating_sub(amount),
        }
    }
}

/// A block that generates a redstone signal.
///
/// The variants are the power components this pass models, nothing more. Pistons,
/// observers, hoppers, doors, rails, sculk sensors, daylight detectors, target
/// blocks, trapped chests, detector rails and tripwire hooks are **not** implemented
/// — see the crate docs.
///
/// ## Evidence
///
/// **derived from Mojang material** — this set of blocks emitting a signal is the
/// set whose classes report a signal in the decompiled 26.1.2 hierarchy: a power
/// source is a `Block` subclass whose `isSignalSource()` returns `true` and whose
/// `getSignal()`/`getDirectSignal()` can return a non-zero level (for example
/// `RedStoneBlock`, `RedstoneTorchBlock`, `LeverBlock`, `ButtonBlock`,
/// `BasePressurePlateBlock`, `DiodeBlock` for the repeater, `ComparatorBlock`, and
/// `LightningRod` for the struck state). The *variants* are therefore traceable to a
/// source; the *numbers* attached to them are labelled individually below.
///
/// **approximation** — all directional and state-specific detail is dropped. Two
/// known deviations:
///
/// - A redstone torch in Vanilla powers only the block **above** it (never the
///   block it is attached to, never sideways) — **measured P13-06** — while its
///   emission to adjacent dust is kept direction-agnostic here (dust beside a
///   lit torch reads 15, measured; above and below assumed). Component
///   orientation is not modelled.
/// - A comparator's output depends on its back and side inputs rather than being a
///   constant; [`PowerSource::Comparator`] therefore responds to neighbours like
///   redstone dust does (max neighbour level, no attenuation) instead of using
///   [`PowerSource::base_power`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PowerSource {
    /// Block of redstone: permanently active while placed.
    RedstoneBlock,
    /// Redstone torch (standing or wall): active while `lit=true`.
    Torch,
    /// Lever: active while `powered=true`.
    Lever,
    /// Button (any wood type, stone, polished blackstone): active while
    /// `powered=true`.
    Button,
    /// Pressure plate (any type): active while `powered=true`.
    PressurePlate,
    /// Repeater: active while `powered=true`; output is a fixed level, not the input.
    Repeater,
    /// Comparator: active while `powered=true`; output is the input level in
    /// `compare` mode and the difference in `subtract` mode.
    Comparator,
    /// Observer: active while `powered=true`; emits 15 from its back face
    /// only (P17-01 — directional, see the gather rule).
    Observer,
    /// Lightning rod: active while `powered=true` (struck by lightning).
    LightningRod,
}

impl PowerSource {
    /// The level this source emits while active.
    ///
    /// - `RedstoneBlock`, `Torch`, `Lever`, `Button` → 15.
    ///   **approximation**: 15 is what these components are universally understood to
    ///   emit, and it is consistent with the one number the wiki states outright
    ///   (*Redstone mechanics* §"Signal transmission": "When a redstone repeater
    ///   receives a redstone signal of any strength, it outputs a signal of strength
    ///   15"), but this project has **not** verified each source's base strength
    ///   against the 26.1.2 code or a live baseline. Treat 15 as assumed-until-tested.
    /// - `PressurePlate` → 15. **approximation**: a plain (stone/wood) pressure plate
    ///   has only `powered=true|false` in the block-state table, so 15 is the only
    ///   value its state can imply. Weighted plates carry a `power=0..15` property and
    ///   are **not** modelled by this variant; a caller that has the property should
    ///   use the raw level instead of this function.
    /// - `Repeater` → 15. **approximation**, but the same value the wiki states for a
    ///   repeater's output; the repeater's own behaviour lives in
    ///   [`crate::components::Repeater`].
    /// - `Comparator` → 15. **approximation and known-wrong as an output**: a
    ///   comparator outputs its input level or a difference, never "15" in general.
    ///   This function is the level the source emits *when this crate treats it as a
    ///   constant source*; the correct behaviour is in
    ///   [`crate::components::Comparator`] and in the propagation rule for
    ///   [`PowerSource::Comparator`].
    /// - `LightningRod` → 15. **approximation**: the rod emits while `powered=true`
    ///   (8 game ticks after a strike in Vanilla); neither the level nor the duration
    ///   is verified here.
    #[must_use]
    pub const fn base_power(self) -> PowerLevel {
        // Every variant is 15 today. The match is exhaustive on purpose: adding a
        // variant must force a decision here rather than inherit a default.
        match self {
            Self::RedstoneBlock
            | Self::Torch
            | Self::Lever
            | Self::Button
            | Self::PressurePlate
            | Self::Repeater
            | Self::Comparator
            | Self::Observer
            | Self::LightningRod => PowerLevel::MAX,
        }
    }

    /// Whether this source's direct emission is strong.
    ///
    /// This flag is about the emission to immediately adjacent dust and
    /// mechanisms — a redstone block emits strongly there (dust on it reads
    /// 15, **measured P13-06**) — and must not be read as "powers adjacent
    /// solids": the block never powers a solid (P13-06, all directions), while
    /// the torch and lever, weak emitters here, strongly power a stone through
    /// position (torch below, lever attachment). That positional rule lives in
    /// [`crate::propagation::solid_power`].
    ///
    /// Whether a *repeater's or comparator's output* strongly powers the block
    /// in front of it is **unverified**, and the conservative choice is to
    /// model both as weak until a baseline test says otherwise. Observers
    /// are strong: pumpkin returns 15 for both weak and strong queries, and
    /// vanilla observers drive dust directly.
    #[must_use]
    pub const fn is_strong_source(self) -> bool {
        matches!(self, Self::RedstoneBlock | Self::Observer)
    }

    /// The state this source emits while active.
    #[must_use]
    pub const fn active_state(self) -> PowerState {
        let level = self.base_power();
        if self.is_strong_source() {
            PowerState::strong_at(level)
        } else {
            PowerState::weak_only(level)
        }
    }

    /// Stable name, matching the block name's stem where there is one.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RedstoneBlock => "redstone_block",
            Self::Torch => "redstone_torch",
            Self::Lever => "lever",
            Self::Button => "button",
            Self::PressurePlate => "pressure_plate",
            Self::Repeater => "repeater",
            Self::Comparator => "comparator",
            Self::Observer => "observer",
            Self::LightningRod => "lightning_rod",
        }
    }

    /// The registered block name this source resolves states through.
    ///
    /// **derived from the registry fixture** — the names are the registry's own, and
    /// [`crate::blocks::SOURCE_BLOCKS`] maps every variant to the blocks that carry it.
    /// A variant with several blocks (buttons, pressure plates, lightning rods) reports
    /// its representative block here; [`crate::blocks::primary_block_name`] is the
    /// lookup, and this method is the same table.
    #[must_use]
    pub const fn block_name(self) -> &'static str {
        match self {
            Self::RedstoneBlock => "minecraft:redstone_block",
            Self::Torch => "minecraft:redstone_torch",
            Self::Lever => "minecraft:lever",
            Self::Button => "minecraft:stone_button",
            Self::PressurePlate => "minecraft:stone_pressure_plate",
            Self::Repeater => "minecraft:repeater",
            Self::Comparator => "minecraft:comparator",
            Self::Observer => "minecraft:observer",
            Self::LightningRod => "minecraft:lightning_rod",
        }
    }
}

impl fmt::Display for PowerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_POWER, PowerLevel, PowerSource, PowerState, SignalKind};

    #[test]
    fn power_level_accepts_the_whole_valid_range() {
        for value in 0..=MAX_POWER {
            let level = PowerLevel::new(value).expect("0..=15 is valid");
            assert_eq!(level.get(), value);
            // Round-trips through both conversions.
            assert_eq!(u8::from(level), value);
            assert_eq!(PowerLevel::from_raw(value), level);
            assert_eq!(PowerLevel::try_from(value).expect("valid"), level);
        }
        assert_eq!(PowerLevel::ZERO.get(), 0);
        assert_eq!(PowerLevel::MAX.get(), 15);
        assert_eq!(PowerLevel::default(), PowerLevel::ZERO);
    }

    #[test]
    fn power_level_refuses_anything_above_fifteen() {
        // The whole point: a value Vanilla can never produce is refused instead of
        // wrapping to a plausible-looking 0 or 15.
        for value in [16u8, 17, 100, 254, 255] {
            assert!(
                PowerLevel::new(value).is_err(),
                "{value} must be refused, not wrapped"
            );
            assert!(PowerLevel::try_from(value).is_err());
        }
        assert!(matches!(
            PowerLevel::new(255),
            Err(mc_core::error::ServerError::InvalidAction(_))
        ));
        // `from_raw` is the hostile-input door and clamps rather than failing.
        assert_eq!(PowerLevel::from_raw(255), PowerLevel::MAX);
        assert_eq!(PowerLevel::from_raw(16), PowerLevel::MAX);
        assert_eq!(PowerLevel::from_raw(0), PowerLevel::ZERO);
    }

    #[test]
    fn is_powered_means_non_zero() {
        assert!(!PowerLevel::ZERO.is_powered());
        for value in 1..=MAX_POWER {
            assert!(
                PowerLevel::new(value).expect("valid").is_powered(),
                "{value}"
            );
        }
    }

    #[test]
    fn attenuation_saturates_at_zero() {
        let max = PowerLevel::MAX;
        assert_eq!(max.saturating_sub(1).get(), 14);
        assert_eq!(max.saturating_sub(15).get(), 0);
        // A negative result is impossible rather than a wrap to 255.
        assert_eq!(max.saturating_sub(16), PowerLevel::ZERO);
        assert_eq!(max.saturating_sub(255), PowerLevel::ZERO);
        assert_eq!(PowerLevel::ZERO.saturating_sub(1), PowerLevel::ZERO);
    }

    #[test]
    fn our_consumer_reads_the_maximum_of_weak_and_strong() {
        // Named "our_" because the max rule is a product decision for this crate,
        // not a verified Vanilla rule (see `PowerState`'s docs).
        let weak = PowerLevel::new(7).expect("valid");
        let strong = PowerLevel::new(3).expect("valid");
        assert_eq!(PowerState::new(weak, strong).effective(), weak);
        assert_eq!(PowerState::new(strong, weak).effective(), weak);
        assert_eq!(PowerState::weak_only(weak).effective(), weak);
        assert_eq!(PowerState::strong_at(strong).effective(), strong);
        assert_eq!(PowerState::OFF.effective(), PowerLevel::ZERO);
        // An off weak with a live strong still reads live.
        assert_eq!(
            PowerState::new(PowerLevel::ZERO, strong).effective(),
            strong
        );
        assert!(PowerState::new(PowerLevel::ZERO, strong).is_powered());
        assert!(!PowerState::OFF.is_powered());
    }

    #[test]
    fn combining_inputs_is_a_component_wise_maximum() {
        // Four blocks feeding the same wire: the result keeps the stronger of each
        // kind independently, so a weak 15 is not lost to a strong 4.
        let mut combined = PowerState::OFF;
        combined = combined.max(PowerState::weak_only(PowerLevel::MAX));
        combined = combined.max(PowerState::strong_at(PowerLevel::new(4).expect("valid")));
        assert_eq!(combined.weak.get(), 15);
        assert_eq!(combined.strong.get(), 4);
        // Commutative and idempotent, which the propagation order relies on.
        assert_eq!(combined.max(PowerState::OFF), combined);
        assert_eq!(PowerState::OFF.max(combined), combined);
        assert_eq!(combined.max(combined), combined);
    }

    #[test]
    fn states_attenuate_both_kinds_together() {
        let state = PowerState::new(
            PowerLevel::new(9).expect("valid"),
            PowerLevel::new(2).expect("valid"),
        )
        .attenuate(1);
        assert_eq!(state.weak.get(), 8);
        assert_eq!(state.strong.get(), 1);
        let floored = state.attenuate(15);
        assert_eq!(floored, PowerState::OFF);
    }

    #[test]
    fn strong_powers_dust_and_weak_does_not() {
        // The one rule this module does claim as verified.
        assert!(SignalKind::Strong.powers_dust());
        assert!(!SignalKind::Weak.powers_dust());
        assert_eq!(SignalKind::Strong.to_string(), "strong");
        assert_eq!(SignalKind::Weak.name(), "weak");
    }

    #[test]
    fn only_the_redstone_block_and_the_observer_are_strong_sources() {
        // Written as an exhaustive table so that adding a variant without deciding
        // its strength fails here rather than silently taking a default.
        // The observer joins the redstone block: pumpkin answers 15 to both
        // weak and strong queries, and vanilla observers drive dust directly.
        let expected = [
            (PowerSource::RedstoneBlock, true),
            (PowerSource::Torch, false),
            (PowerSource::Lever, false),
            (PowerSource::Button, false),
            (PowerSource::PressurePlate, false),
            (PowerSource::Repeater, false),
            (PowerSource::Comparator, false),
            (PowerSource::Observer, true),
            (PowerSource::LightningRod, false),
        ];
        for (source, strong) in expected {
            assert_eq!(source.is_strong_source(), strong, "{source}");
            assert_eq!(source.base_power(), PowerLevel::MAX, "{source}");
            let state = source.active_state();
            assert_eq!(state.weak, PowerLevel::MAX, "{source} weak");
            if strong {
                assert_eq!(state.strong, PowerLevel::MAX, "{source} strong");
            } else {
                assert_eq!(state.strong, PowerLevel::ZERO, "{source} strong");
            }
            assert!(state.is_powered(), "{source} active means powered");
            assert!(!source.name().is_empty());
        }
    }

    #[test]
    fn strong_sources_and_names_are_unique() {
        let all = [
            PowerSource::RedstoneBlock,
            PowerSource::Torch,
            PowerSource::Lever,
            PowerSource::Button,
            PowerSource::PressurePlate,
            PowerSource::Repeater,
            PowerSource::Comparator,
            PowerSource::Observer,
            PowerSource::LightningRod,
        ];
        assert_eq!(
            all.iter()
                .filter(|source| source.is_strong_source())
                .count(),
            2
        );
        let mut names: Vec<&str> = all.iter().map(|source| source.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "source names must be unique");
        assert_eq!(PowerSource::Lever.to_string(), "lever");
    }
}
