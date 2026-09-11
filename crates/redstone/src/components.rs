//! The redstone components this pass implements, as pure functions (P06-11).
//!
//! ## Scope, and what is deliberately missing
//!
//! Implemented: **lever**, **redstone torch**, **repeater**, **comparator**.
//!
//! **Not implemented**, and not stubbed behind a working-looking API:
//!
//! - pistons and sticky pistons (including quasi-connectivity),
//! - observers,
//! - hoppers and other container-driven outputs (comparator *reading* a container),
//! - doors, trapdoors, fence gates,
//! - rails of every kind (powered, detector, activator),
//! - redstone lamps, note blocks, dispensers, droppers, TNT, copper bulbs,
//!   sculk sensors, daylight detectors, target blocks, trapped chests,
//!   tripwire hooks, lightning-rod strike timing,
//! - redstone dust as a *component* (its wire behaviour is in [`crate::propagation`]),
//! - torch burn-out, the 8-tick torch delay, torch "too many flips" suppression,
//!   repeater locking by a powered comparator on its side, comparator
//!   container-signal reading.
//!
//! A component that is not listed as implemented has no entry point here; calling
//! code cannot accidentally get a placeholder result.
//!
//! ## Why pure functions
//!
//! Every component's behaviour is a function
//! `(state, input) -> PowerState` with no world access, no clock and no randomness.
//! That is what makes the golden tests in `tests/golden_circuits.rs` hand-checkable:
//! the expected number in the test is the number the component computes, not a value
//! observed from a run.
//!
//! ## Evidence and deviations
//!
//! Each component's docs name what is verified and what is not. Two deviations
//! apply to all of them and are stated once here:
//!
//! - **No direction.** A component here reads one aggregate [`PowerState`] and writes
//!   one. Vanilla's repeater and comparator are directional (input at the back, sides
//!   for the comparator, output at the front) and the torch's emission is
//!   direction-dependent. Orientation is not modelled: [`ComponentState::output_power`]
//!   takes the input already gathered from the faces the caller considers the input,
//!   and `crate::propagation` treats the result as emitted to all neighbours.
//! - **No source/emission selection.** A lever here emits to every neighbour;
//!   Vanilla's lever emits only in the directions its attachment allows.

use crate::power::{PowerLevel, PowerSource, PowerState, SignalKind};

/// The number of faces a component can be attached to.
///
/// **derived** — a block has six face-adjacent neighbours (`Direction.values()`), the
/// same six [`crate::update::NeighbourSet`] enumerates.
pub const FACING_COUNT: usize = 6;

/// Clamp a repeater's delay into the supported range, in game ticks.
///
/// **derived from project fixtures** — the state table in
/// `crates/test-support/fixtures/registry/blocks.tsv` (dumped from the 26.1.2 data)
/// declares `minecraft:repeater` with `delay=1|2|3|4`, so those are the only four
/// legal values and anything else is out of range. The clamp is deliberately silent
/// on the *low* side (0 becomes 1) and saturating on the high side, because a delay is
/// caller-supplied and a delay of 0 or 999 must not panic (AGENTS.md §9).
#[must_use]
pub const fn clamp_repeater_delay(delay: u32) -> u32 {
    // `u32::clamp` is not const-stable, hence the explicit floor/ceiling.
    if delay < Repeater::MIN_DELAY {
        Repeater::MIN_DELAY
    } else if delay > Repeater::MAX_DELAY {
        Repeater::MAX_DELAY
    } else {
        delay
    }
}

/// A lever: the simplest power component, on or off.
///
/// **verified** — a lever is a power component and the block state table gives it
/// exactly `powered=true|false` plus `face` and `facing`
/// (`crates/test-support/fixtures/registry/blocks.tsv`, dumped from the 26.1.2 data:
/// `minecraft:lever 6771 24 face=floor|wall|ceiling;facing=north|south|west|east;powered=true|false`).
/// Toggling it is the "on demand" power generation minecraft.wiki *Redstone mechanics*
/// §"Signal generation" describes.
///
/// **approximation** — the lever's output level is taken from
/// [`PowerSource::Lever::base_power`], whose 15 is labelled assumed-until-tested
/// there. A lever in Vanilla also only powers the faces its attachment permits; that
/// directionality is not modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lever {
    /// Whether the lever is thrown.
    pub on: bool,
}

impl Lever {
    /// An off lever.
    pub const OFF: Self = Self { on: false };

    /// A lever in the given position.
    #[must_use]
    pub const fn new(on: bool) -> Self {
        Self { on }
    }

    /// Throw the lever, returning the previous position.
    ///
    /// Toggling is a player action; it is infallible and cannot panic.
    pub const fn toggle(&mut self) -> bool {
        let previous = self.on;
        self.on = !self.on;
        previous
    }

    /// The kind of signal this lever emits while on.
    ///
    /// **approximation** — weak, because a lever's effect on its attachment block is
    /// the wiki's example of weak power. Whether a lever *also* strongly powers
    /// something is not modelled; see [`PowerSource::is_strong_source`].
    #[must_use]
    pub const fn signal_kind(&self) -> SignalKind {
        SignalKind::Weak
    }

    /// The *state* properties of this lever: `powered`.
    ///
    /// **derived** — `powered` is the only property a lever's power depends on. The
    /// `face`/`facing` properties the block also declares are supplied by
    /// [`ComponentState::properties`], which documents the defaults used.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        vec![("powered".to_owned(), self.on.to_string())]
    }
}

/// A redstone torch: lit until its attachment block is powered.
///
/// **verified** — the block state table gives a torch `lit=true|false`
/// (`minecraft:redstone_torch 6885 2 lit=true|false`), and the inversion is the
/// canonical NOT gate: minecraft.wiki *Redstone circuits* §"Logic circuit" — "A NOT
/// gate (aka 'inverter') is on if its input is off. The simplest NOT gate is an input
/// signal with a redstone torch attached."
///
/// **approximation** — three things Vanilla does that this does not:
///
/// - the 1-redstone-tick (2 game tick) delay before a torch changes state;
/// - burn-out: a torch that flips too often within a short window stops emitting;
/// - the direction rule (a torch powers the block above it and its attachment block,
///   not dust to its sides or below).
///
/// `lit` is stored rather than derived from `input`, so the type can represent the
/// state *between* the input changing and the torch reacting — that is the state the
/// scheduler will hold once the delayed tick is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedstoneTorch {
    /// Whether the torch is currently emitting.
    pub lit: bool,
}

impl RedstoneTorch {
    /// A lit torch (the state of a freshly placed one).
    pub const LIT: Self = Self { lit: true };

    /// A torch in the given state.
    #[must_use]
    pub const fn new(lit: bool) -> Self {
        Self { lit }
    }

    /// The state a torch reacts to `input` with, ignoring delay.
    ///
    /// Infallible by construction: inverting a signal has no failure mode, so this is
    /// a plain constructor rather than a `Result`.
    #[must_use]
    pub const fn react(input: PowerState) -> Self {
        Self {
            lit: !input.is_powered(),
        }
    }

    /// The signal a *lit* torch emits.
    ///
    /// **approximation** — weak, and direction-agnostic here; see the type docs.
    #[must_use]
    pub const fn output(&self) -> PowerState {
        if self.lit {
            PowerState::weak_only(PowerSource::Torch.base_power())
        } else {
            PowerState::OFF
        }
    }

    /// The block-state property assignment for this torch.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        vec![("lit".to_owned(), self.lit.to_string())]
    }
}

/// A repeater: a directional diode with a configurable delay.
///
/// **verified** — the block state table gives `minecraft:repeater` the properties
/// `delay=1|2|3|4`, `facing`, `locked` and `powered`
/// (`crates/test-support/fixtures/registry/blocks.tsv`, dumped from the 26.1.2 data),
/// and minecraft.wiki *Redstone mechanics* §"Signal transmission" states the output
/// rule outright: "When a redstone repeater receives a redstone signal of any
/// strength, it outputs a signal of strength 15." That is the whole input behaviour,
/// and it is exhaustive over the input space.
///
/// **approximation** — the delay is *stored*, not enforced: nothing here waits
/// `delay` ticks. Enforcing it is the scheduler's job (schedule an
/// [`crate::update::UpdateKind::ScheduledTick`] for `delay` and run
/// [`ComponentState::output_power`] then); `propagate` in this pass changes the
/// repeater's output in the same tick as its input.
///
/// **unverified** — whether a repeater's output strongly powers the block in front of
/// it is not verified here; [`PowerSource::Repeater`] is treated as a weak source. See
/// [`PowerSource::is_strong_source`] for why the conservative choice was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeater {
    /// Delay in game ticks, in `1..=4`.
    pub delay: u32,
    /// Whether the output is currently powered (the `powered` block state).
    pub powered: bool,
    /// Whether a powered comparator on a side is locking the repeater (`locked`).
    pub locked: bool,
}

impl Repeater {
    /// Shortest supported delay, in game ticks.
    ///
    /// **derived** — the minimum of the `delay=1|2|3|4` property in the block state
    /// table.
    pub const MIN_DELAY: u32 = 1;
    /// Longest supported delay, in game ticks.
    ///
    /// **derived** — the maximum of the `delay=1|2|3|4` property in the block state
    /// table. Note that this is a **game tick** count; the community "redstone tick" is
    /// two game ticks (minecraft.wiki *Redstone mechanics* §"Redstone tick"), so a
    /// delay of 4 is 0.2 s, not 0.4 s.
    pub const MAX_DELAY: u32 = 4;

    /// A repeater with the given delay, out of range values clamped.
    #[must_use]
    pub const fn new(delay: u32) -> Self {
        Self {
            delay: clamp_repeater_delay(delay),
            powered: false,
            locked: false,
        }
    }

    /// The delay in game ticks, always inside `1..=4`.
    #[must_use]
    pub const fn delay(&self) -> u32 {
        clamp_repeater_delay(self.delay)
    }

    /// Whether the repeater is delivering power.
    #[must_use]
    pub const fn is_powered(&self) -> bool {
        self.powered
    }

    /// The signal the repeater emits while powered.
    #[must_use]
    pub const fn output(&self) -> PowerState {
        if self.powered {
            PowerState::weak_only(PowerSource::Repeater.base_power())
        } else {
            PowerState::OFF
        }
    }

    /// The block-state property assignment for this repeater.
    ///
    /// `facing` is not included: orientation is not modelled (see the module docs).
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        vec![
            ("delay".to_owned(), self.delay().to_string()),
            ("locked".to_owned(), self.locked.to_string()),
            ("powered".to_owned(), self.powered.to_string()),
        ]
    }
}

/// How a comparator combines its inputs.
///
/// **verified** — the block state table declares `mode=compare|subtract` on
/// `minecraft:comparator` (`crates/test-support/fixtures/registry/blocks.tsv`), and
/// minecraft.wiki *Redstone circuits* §"Comparator" describes both modes: "Comparing
/// mode compares the signal strength from the side with the input; if the side signal
/// is stronger than the input signal, the output is off, otherwise it is on. In
/// Subtracting mode, the comparator subtracts the side signal from the input."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparatorMode {
    /// Output the back input, but only if it is at least the strongest side input.
    Compare,
    /// Output the back input minus the strongest side input, floored at 0.
    Subtract,
}

impl ComparatorMode {
    /// Stable name, matching the block-state property value.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compare => "compare",
            Self::Subtract => "subtract",
        }
    }
}

/// A comparator: compares or subtracts a side input from a back input.
///
/// Note the difference from [`Repeater`]: a comparator **passes the input level
/// through** rather than emitting a fixed 15, which the wiki states for the back
/// input ("When a redstone comparator receives a signal from its back input, it
/// outputs the same signal") and which the compare mode sentence above implies.
///
/// **approximation** — the side input is a single aggregate [`PowerLevel`] (the
/// strongest side), because orientation is not modelled. Vanilla takes the maximum of
/// the two sides, so collapsing them into one value here is faithful to that rule, but
/// it means a caller must be the one to gather the sides.
///
/// **not implemented** — reading a container behind the comparator
/// (`getAnalogOutputSignal`), and the `powered` state's 2-game-tick settling delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparator {
    /// Compare or subtract.
    pub mode: ComparatorMode,
    /// Whether the comparator is currently delivering power (the `powered` state).
    pub powered: bool,
}

impl Comparator {
    /// A comparator in the given mode, currently off.
    #[must_use]
    pub const fn new(mode: ComparatorMode) -> Self {
        Self {
            mode,
            powered: false,
        }
    }

    /// The level the comparator emits, given its back input and its strongest side
    /// input.
    ///
    /// This is the pure combination rule and it holds regardless of `powered`, which
    /// is a *reported* state rather than an input to the arithmetic:
    ///
    /// - [`ComparatorMode::Compare`] → `back` if `back >= side`, else 0.
    /// - [`ComparatorMode::Subtract`] → `back - side`, saturating at 0.
    ///
    /// Both rules are the wiki's, quoted on [`ComparatorMode`]. Note that compare mode
    /// is **not** "output the larger of the two": a stronger side switches the output
    /// off entirely.
    #[must_use]
    pub const fn level(&self, back: PowerLevel, side: PowerLevel) -> PowerLevel {
        match self.mode {
            ComparatorMode::Compare => {
                if back.get() >= side.get() {
                    back
                } else {
                    PowerLevel::ZERO
                }
            }
            ComparatorMode::Subtract => back.saturating_sub(side.get()),
        }
    }

    /// The signal the comparator emits for these inputs.
    ///
    /// **approximation** — emitted weakly, as for the repeater; whether a comparator's
    /// output is a strong source is not verified here.
    #[must_use]
    pub const fn output(&self, back: PowerLevel, side: PowerLevel) -> PowerState {
        PowerState::weak_only(self.level(back, side))
    }

    /// The block-state property assignment for this comparator.
    ///
    /// `facing` is not included: orientation is not modelled.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        vec![
            ("mode".to_owned(), self.mode.name().to_owned()),
            ("powered".to_owned(), self.powered.to_string()),
        ]
    }
}

/// The state of one implemented redstone component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentState {
    /// A lever.
    Lever(Lever),
    /// A redstone torch.
    Torch(RedstoneTorch),
    /// A repeater.
    Repeater(Repeater),
    /// A comparator.
    Comparator(Comparator),
}

impl ComponentState {
    /// The output this component produces, given the input it reads.
    ///
    /// **Pure**: no world access, no clock, no randomness, and the same inputs always
    /// give the same output. Exhaustive truth tables over the (small) input spaces are
    /// in the tests below.
    ///
    /// ## What `input` means, component by component
    ///
    /// The block state is the source of truth for what a component emits, and `input` is
    /// used only where Vanilla's emission genuinely depends on it:
    ///
    /// - [`Lever`] — **ignored**. A lever is a manual source: its state *is* its output.
    /// - [`RedstoneTorch`] — ignored, for the same reason: `lit` is the stored state.
    ///   [`RedstoneTorch::react`] is the rule that decides what `lit` becomes when the
    ///   attachment block changes; the caller applies it once per reaction (a scheduled
    ///   tick in a future pass).
    /// - [`Repeater`] — ignored. Any non-zero input sets `powered`, and a powered repeater
    ///   emits 15 (**verified**: minecraft.wiki, *Redstone Mechanics* §"Signal
    ///   transmission" — "When a redstone repeater receives a redstone signal of any
    ///   strength, it outputs a signal of strength 15"). So the emission follows the
    ///   *stored* `powered` field, which is what the block state carries; the input decides
    ///   when that field changes, not what it means.
    /// - [`Comparator`] — **used**. A comparator's output is the level it receives, so
    ///   unlike the others it cannot be read off its own state. The strongest neighbour is
    ///   taken as its back input; the side input is
    ///   [`ComponentState::output_power_with_side`].
    ///
    /// This split is the thing a caller must know: turning a lever or a repeater on is a
    /// *state write*, while a comparator recomputes from its input every time.
    #[must_use]
    pub fn output_power(self, input: PowerState) -> PowerState {
        match self {
            Self::Lever(lever) => {
                if lever.on {
                    PowerState::weak_only(PowerSource::Lever.base_power())
                } else {
                    PowerState::OFF
                }
            }
            Self::Torch(torch) => torch.output(),
            Self::Repeater(repeater) => repeater.output(),
            Self::Comparator(comparator) => comparator.output(input.effective(), PowerLevel::ZERO),
        }
    }

    /// The output of a component that has a second input, for the components that
    /// need one.
    ///
    /// Only [`ComponentState::Comparator`] uses `side`; every other component ignores
    /// it and this is exactly [`ComponentState::output_power`] with `input` as the
    /// back input.
    #[must_use]
    pub fn output_power_with_side(self, input: PowerState, side: PowerLevel) -> PowerState {
        match self {
            Self::Comparator(comparator) => comparator.output(input.effective(), side),
            other => other.output_power(input),
        }
    }

    /// The power source this component is, for the propagation rule.
    #[must_use]
    pub const fn source(self) -> PowerSource {
        match self {
            Self::Lever(_) => PowerSource::Lever,
            Self::Torch(_) => PowerSource::Torch,
            Self::Repeater(_) => PowerSource::Repeater,
            Self::Comparator(_) => PowerSource::Comparator,
        }
    }

    /// The component's state as a **complete** block-state property assignment, suitable
    /// for [`mc_registry::BlockRegistry::state_id`].
    ///
    /// Complete matters: the registry rejects a property list that omits a property the
    /// block declares (it cannot compute a mixed-radix index without one), so a list
    /// carrying only `powered` would fail to resolve a lever. The properties this crate
    /// does not model are therefore filled with a canonical default and **named here**, so
    /// a reader can see exactly what is being assumed:
    ///
    /// - `facing` is `north` for every component that has it. **approximation**: the
    ///   registry fixture (dumped from the 26.1.2 data) lists `facing=north|south|west|east`
    ///   with `north` first, so `north` is also the first-state default for the blocks
    ///   declared in that order; it is never verified against a placed block.
    /// - a wall torch's `facing` is also `north`, and the standing
    ///   [`ComponentState::Torch`] resolves through `minecraft:redstone_torch` rather than
    ///   the wall variant.
    /// - a lever's or button's `face` is `floor`.
    /// - nothing else (power, delay, mode, locked) is defaulted: those are the properties
    ///   the model actually computes.
    ///
    /// A caller that needs the real orientation must supply it; this model has no
    /// orientation.
    #[must_use]
    pub fn properties(self) -> Vec<(String, String)> {
        let mut properties = match self {
            Self::Lever(lever) => lever.properties(),
            Self::Torch(torch) => torch.properties(),
            Self::Repeater(repeater) => repeater.properties(),
            Self::Comparator(comparator) => comparator.properties(),
        };
        if self.has_facing() {
            properties.push(("facing".to_owned(), "north".to_owned()));
        }
        if self.has_face() {
            properties.push(("face".to_owned(), "floor".to_owned()));
        }
        properties
    }

    /// Whether this component's block declares a `facing` property.
    ///
    /// **derived from the registry fixture**: every modelled component except the standing
    /// torch and the plain pressure plates is declared with `facing=north|south|west|east`.
    #[must_use]
    const fn has_facing(self) -> bool {
        matches!(
            self,
            Self::Lever(_) | Self::Repeater(_) | Self::Comparator(_)
        )
    }

    /// Whether this component's block declares a `face` property.
    ///
    /// **derived from the registry fixture**: levers and buttons are declared with
    /// `face=floor|wall|ceiling`.
    #[must_use]
    const fn has_face(self) -> bool {
        matches!(self, Self::Lever(_))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Comparator, ComparatorMode, ComponentState, FACING_COUNT, Lever, RedstoneTorch, Repeater,
        clamp_repeater_delay,
    };
    use crate::power::{MAX_POWER, PowerLevel, PowerState};

    /// Every power level, for building exhaustive input spaces.
    fn all_levels() -> Vec<PowerLevel> {
        (0..=MAX_POWER)
            .map(|value| PowerLevel::new(value).expect("0..=15 is valid"))
            .collect()
    }

    /// Every weak/strong pair, for building exhaustive `PowerState` inputs.
    fn all_states() -> Vec<PowerState> {
        let mut states = Vec::new();
        for weak in all_levels() {
            for strong in all_levels() {
                states.push(PowerState::new(weak, strong));
            }
        }
        states
    }

    fn level(value: u8) -> PowerLevel {
        PowerLevel::new(value).expect("0..=15 is valid")
    }

    #[test]
    fn lever_truth_table_is_exhaustive_over_its_whole_input_space() {
        // A lever's input space is (on, input power): the input is ignored, so the
        // table is 2 rows and it is checked against every possible input state.
        for on in [false, true] {
            let mut lever = Lever::new(on);
            for input in all_states() {
                let out = ComponentState::Lever(lever).output_power(input);
                if on {
                    assert_eq!(out, PowerState::weak_only(PowerLevel::MAX), "on");
                } else {
                    assert_eq!(out, PowerState::OFF, "off");
                }
            }
            // Toggling flips it and reports the previous position.
            assert_eq!(lever.toggle(), on);
            assert_eq!(lever.on, !on);
        }
        assert_eq!(Lever::OFF, Lever::new(false));
        assert_eq!(Lever::default(), Lever::OFF);
        assert_eq!(
            Lever::new(true).properties(),
            vec![("powered".to_owned(), "true".to_owned())]
        );
    }

    #[test]
    fn torch_truth_table_is_exhaustive_over_lit_and_every_input_state() {
        // 2 x 17 x 17 = 578 cases: the whole input space, not a sample.
        for lit in [false, true] {
            for input in all_states() {
                let out = ComponentState::Torch(RedstoneTorch::new(lit)).output_power(input);
                if lit {
                    assert_eq!(out, PowerState::weak_only(PowerLevel::MAX), "lit");
                } else {
                    assert_eq!(out, PowerState::OFF, "unlit");
                }
            }
        }
        // The inversion rule itself: any non-zero input (in *either* kind) turns the
        // torch off, and zero input lights it.
        for input in all_states() {
            let reacted = RedstoneTorch::react(input);
            assert_eq!(
                reacted.lit,
                !input.is_powered(),
                "input {input:?} should invert"
            );
        }
        assert!(
            !RedstoneTorch::react(PowerState::new(PowerLevel::ZERO, level(4))).lit,
            "a strong-only input must still invert the torch"
        );
        assert!(RedstoneTorch::react(PowerState::OFF).lit);
        assert_eq!(RedstoneTorch::LIT, RedstoneTorch::new(true));
        assert_eq!(RedstoneTorch::new(false), RedstoneTorch { lit: false });
        assert_eq!(
            RedstoneTorch::new(false).properties(),
            vec![("lit".to_owned(), "false".to_owned())]
        );
    }

    #[test]
    fn repeater_truth_table_is_exhaustive_over_delay_powered_locked_and_input() {
        // 4 delays x 2 powered x 2 locked x 17 x 17 input states. The emission follows the
        // *stored* `powered` field, so the input does not change the answer — that is the
        // property the loop checks, over the whole input space.
        for delay in 1..=Repeater::MAX_DELAY {
            for powered in [false, true] {
                for locked in [false, true] {
                    let repeater = Repeater {
                        delay,
                        powered,
                        locked,
                    };
                    let expected = if powered {
                        PowerState::weak_only(PowerLevel::MAX)
                    } else {
                        PowerState::OFF
                    };
                    for input in all_states() {
                        assert_eq!(
                            ComponentState::Repeater(repeater).output_power(input),
                            expected,
                            "delay={delay} powered={powered} locked={locked} input={input:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_repeater_emits_from_its_stored_state_not_from_its_input() {
        // Regression: an earlier version derived `powered` from the input inside
        // `output_power`, which made a *powered* repeater sitting in a world emit nothing
        // whenever its input was read as clear. The state is the source of truth.
        let powered = ComponentState::Repeater(Repeater {
            delay: 1,
            powered: true,
            locked: false,
        });
        assert_eq!(
            powered.output_power(PowerState::OFF),
            PowerState::weak_only(PowerLevel::MAX)
        );
        let unpowered = ComponentState::Repeater(Repeater::new(1));
        assert_eq!(
            unpowered.output_power(PowerState::weak_only(PowerLevel::MAX)),
            PowerState::OFF
        );
    }

    #[test]
    fn a_powered_repeater_outputs_fifteen_whatever_the_input_strength() {
        // The verified wiki rule, stated as its own test: a powered repeater's output does not
        // depend on the input's strength. The *input* is what turned `powered` on; how strong
        // it was is not observable in the output.
        let repeater = Repeater {
            delay: 2,
            powered: true,
            locked: false,
        };
        for value in 0..=MAX_POWER {
            let input = PowerState::weak_only(level(value));
            assert_eq!(
                ComponentState::Repeater(repeater).output_power(input),
                PowerState::weak_only(PowerLevel::MAX),
                "input strength {value} must come out as 15"
            );
        }
        // An unpowered repeater emits nothing, however strong its input is: the input has not
        // been latched into `powered` yet, and that latch is the delay.
        for value in 0..=MAX_POWER {
            let input = PowerState::weak_only(level(value));
            assert_eq!(
                ComponentState::Repeater(Repeater::new(2)).output_power(input),
                PowerState::OFF,
                "input strength {value} on an unpowered repeater"
            );
        }
    }

    #[test]
    fn repeater_delays_are_clamped_into_one_to_four() {
        assert_eq!(clamp_repeater_delay(0), 1);
        assert_eq!(clamp_repeater_delay(1), 1);
        assert_eq!(clamp_repeater_delay(4), 4);
        assert_eq!(clamp_repeater_delay(5), 4);
        assert_eq!(clamp_repeater_delay(u32::MAX), 4);
        for delay in [0, 1, 2, 3, 4, 5, 100, u32::MAX] {
            let repeater = Repeater::new(delay);
            assert!(
                (Repeater::MIN_DELAY..=Repeater::MAX_DELAY).contains(&repeater.delay()),
                "delay({delay}) = {} is out of range",
                repeater.delay()
            );
        }
        assert!(!Repeater::new(2).is_powered());
    }

    #[test]
    fn comparator_compare_mode_is_exhaustive_over_the_whole_input_space() {
        // 17 x 17 = 289 cases, written as the rule so a reader can check each one
        // against the wiki sentence quoted on `ComparatorMode`.
        let comparator = Comparator::new(ComparatorMode::Compare);
        for back in all_levels() {
            for side in all_levels() {
                let expected = if back.get() >= side.get() {
                    back
                } else {
                    PowerLevel::ZERO
                };
                assert_eq!(
                    comparator.level(back, side),
                    expected,
                    "compare back={back} side={side}"
                );
                // The key non-obvious case: a stronger side switches the output off
                // entirely rather than passing the side value through.
                if side.get() > back.get() {
                    assert_eq!(comparator.level(back, side), PowerLevel::ZERO);
                }
            }
        }
    }

    #[test]
    fn comparator_subtract_mode_is_exhaustive_over_the_whole_input_space() {
        let comparator = Comparator::new(ComparatorMode::Subtract);
        for back in all_levels() {
            for side in all_levels() {
                let expected = back.saturating_sub(side.get());
                assert_eq!(
                    comparator.level(back, side),
                    expected,
                    "subtract back={back} side={side}"
                );
                // Never negative, always in range.
                let got = comparator.level(back, side).get();
                assert!(got <= MAX_POWER);
                assert_eq!(got, back.get().saturating_sub(side.get()));
            }
        }
    }

    #[test]
    fn comparator_passes_the_back_level_through_when_no_side_input_exists() {
        // A comparator with a clear side is the "same signal out" case the wiki states.
        for mode in [ComparatorMode::Compare, ComparatorMode::Subtract] {
            let comparator = Comparator::new(mode);
            for back in all_levels() {
                assert_eq!(
                    comparator.level(back, PowerLevel::ZERO),
                    back,
                    "{mode:?} must pass the back level through"
                );
            }
            assert_eq!(Comparator::new(mode).properties()[0].1, mode.name());
        }
        assert_eq!(ComparatorMode::Compare.name(), "compare");
        assert_eq!(ComparatorMode::Subtract.name(), "subtract");
    }

    #[test]
    fn the_single_and_dual_input_entry_points_differ_only_for_the_comparator() {
        // `output_power` is the single-input form; `output_power_with_side` is the same
        // function with the second input ignored, *except* on a comparator, which is the
        // only component whose behaviour depends on a side input.
        let ignores_the_side = [
            ComponentState::Lever(Lever::new(true)),
            ComponentState::Lever(Lever::OFF),
            ComponentState::Torch(RedstoneTorch::LIT),
            ComponentState::Torch(RedstoneTorch::new(false)),
            ComponentState::Repeater(Repeater::new(3)),
        ];
        for component in ignores_the_side {
            for input in all_states() {
                for side in all_levels() {
                    assert_eq!(
                        component.output_power_with_side(input, side),
                        component.output_power(input),
                        "{component:?} input={input:?} side={side}"
                    );
                }
            }
        }
        // The comparator uses the side: subtract mode loses `side` strength, compare mode
        // switches off when the side is stronger.
        let subtract = ComponentState::Comparator(Comparator::new(ComparatorMode::Subtract));
        let input = PowerState::weak_only(level(10));
        assert_eq!(
            subtract.output_power_with_side(input, level(3)),
            PowerState::weak_only(level(7))
        );
        assert_ne!(
            subtract.output_power_with_side(input, level(3)),
            subtract.output_power(input)
        );
        let compare = ComponentState::Comparator(Comparator::new(ComparatorMode::Compare));
        assert_eq!(
            compare.output_power_with_side(input, level(4)),
            PowerState::weak_only(level(10))
        );
        assert_eq!(
            compare.output_power_with_side(input, level(11)),
            PowerState::OFF,
            "a stronger side switches compare mode off entirely"
        );
        // With a clear side, the two entry points agree for the comparator too.
        assert_eq!(
            compare.output_power_with_side(input, PowerLevel::ZERO),
            compare.output_power(input)
        );
    }

    #[test]
    fn every_component_maps_to_a_power_source_and_reports_properties() {
        let components = [
            (
                ComponentState::Lever(Lever::new(true)),
                crate::power::PowerSource::Lever,
            ),
            (
                ComponentState::Torch(RedstoneTorch::LIT),
                crate::power::PowerSource::Torch,
            ),
            (
                ComponentState::Repeater(Repeater::new(2)),
                crate::power::PowerSource::Repeater,
            ),
            (
                ComponentState::Comparator(Comparator::new(ComparatorMode::Compare)),
                crate::power::PowerSource::Comparator,
            ),
        ];
        for (component, source) in components {
            assert_eq!(component.source(), source, "{component:?}");
            let properties = component.properties();
            assert!(
                !properties.is_empty(),
                "{component:?} must report properties"
            );
            for (name, value) in properties {
                assert!(!name.is_empty() && !value.is_empty());
            }
        }
        assert_eq!(FACING_COUNT, 6);
    }
}
