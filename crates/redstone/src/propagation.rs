//! The propagation algorithm: recompute what changed, in a fixed order, within a
//! budget (P06-10).
//!
//! ## The rule this implements
//!
//! 1. A change to a block is announced by pushing a neighbour update for that block
//!    (see [`prepare`], or push it yourself).
//! 2. Processing an update recomputes the block's own [`PowerState`] from its six
//!    neighbours.
//! 3. If the new state differs from the old one, the world is written and a neighbour
//!    update is queued for each of the six neighbours.
//! 4. Repeat until the queue is empty or the [`UpdateBudget`] is exhausted.
//!
//! Wire power is the one number the algorithm computes:
//!
//! ```text
//! wire_power(pos) = max over the six neighbours of emitted_power(neighbour) - 1
//! ```
//!
//! with the subtraction saturating at 0, so a wire with no powered neighbour carries 0.
//! [`WIRE_ATTENUATION_PER_BLOCK`] labels that "1".
//!
//! ## Both directions propagate, and that is a tested property
//!
//! A change pushes neighbour updates for the six faces of the position that changed, **whatever the
//! change was** — an increase, a decrease, or a removal. Nothing here tests whether the new state is
//! itself an emitter, because doing so would break *decreases*: a destroyed redstone block or a lever
//! switched off emits nothing, so a model that only pushes from live sources can never carry the
//! falling edge, and the line stays lit forever. That is the classic "redstone stays on after you
//! break the source" defect.
//!
//! `tests/propagation.rs` pins both directions down, over a whole line rather than one hop:
//! `removing_the_source_drops_every_wire_in_the_line_to_zero_in_one_pass` and
//! `switching_a_lever_off_drops_the_line_and_on_lights_it_again`.
//!
//! ## Evidence, and what is *not* reproduced
//!
//! **verified** - minecraft.wiki, *Redstone mechanics* section "Signal transmission":
//!
//! > Redstone dust transmits power to adjacent redstone dust, but the signal strength
//! > decreases by 1 for every block of redstone dust that the signal travels. Redstone
//! > dust can thus transmit a signal up to 15 blocks by itself.
//!
//! That is the attenuation rule implemented here. It is also the *only* number in the
//! wire calculation, since the source levels themselves are labelled in
//! [`crate::power`].
//!
//! **not reproduced, explicitly** - none of the following is modelled, and no claim is
//! made that a circuit here behaves like the same circuit in Vanilla:
//!
//! - **Vanilla's update order.** Vanilla does not process a global queue of
//!   `UpdateKind`s; it walks a per-level collection of block positions in its own order
//!   and raises block updates, shape updates and (for some blocks) comparator updates
//!   through distinct code paths. This crate processes positions in the deterministic
//!   order of [`crate::update::UpdateQueue`], which is *a* total order with a documented
//!   rule, **not** Vanilla's. For a circuit whose result depends on update order (a
//!   "locational" circuit), the two will differ.
//! - **The block-update versus shape-update distinction.** Vanilla's `updateShape` and
//!   `neighborChanged` are separate events with separate recipients; here every update
//!   is one `NeighborChanged` recomputation. A wire's *connection* shape
//!   (`north=side|up|none`) is written but never recomputed, so a wire does not
//!   re-orient when its neighbours change.
//! - **Comparator updates as a separate channel.**
//!   [`crate::update::UpdateKind::ComparatorUpdate`] exists as a kind but is processed
//!   identically to a neighbour update.
//! - **The 1-redstone-tick delay of redstone dust**, the 2-game-tick reaction of a
//!   torch, and the configured delay of a repeater. This pass changes a component's
//!   output in the same tick as its input; only *wire* rescheduling is modelled, via
//!   [`schedule_wire_recheck`].
//! - **Conductivity.** A solid block never becomes powered here, so a circuit that
//!   relies on powering a block rather than dust does not work. See [`BlockRole`].
//! - **Quasi-connectivity** (a piston activated by the space above it), and the
//!   mechanism component set (pistons, doors, dispensers, ...) other than the
//!   redstone lamp. An unimplemented mechanism block is [`BlockRole::Passive`]
//!   and therefore never reacts - it is absent, not silently substituted.
//! - **Wire burn-out, wire re-orientation when unsupported, and wire breaking when its
//!   support is removed.** A dangling wire here is simply wire.
//! - **Ordering across chunk and dimension boundaries.** An update for an unloaded
//!   chunk is a no-op read ([`BlockView::get_state`] returns `None`), never a chunk
//!   load, so a circuit that spans a chunk boundary behaves as if the far side were
//!   absent - and a redstone update can never make the server generate terrain.

use crate::blocks::{REDSTONE_LAMP, REDSTONE_WIRE, source_for_name};
use crate::components::{
    Comparator, ComparatorMode, ComponentState, Lever, RedstoneTorch, Repeater,
};
use crate::power::{MAX_POWER, PowerLevel, PowerSource, PowerState, SignalKind};
use crate::update::{BlockPos, Inserted, NeighbourSet, UpdateBudget, UpdateKind, UpdateQueue};
use mc_core::error::ServerResult;
use mc_registry::BlockRegistry;

/// Strength lost per block of redstone dust travelled.
///
/// **verified** - minecraft.wiki, *Redstone mechanics* section "Signal transmission",
/// quoted in the module docs: the signal "decreases by 1 for every block of redstone
/// dust that the signal travels".
pub const WIRE_ATTENUATION_PER_BLOCK: u8 = 1;

/// The strength a wire needs to be a signal rather than an off wire.
///
/// **derived** - a redstone signal is an integer in `1..=15` (minecraft.wiki,
/// *Redstone mechanics* section "Redstone signal"), so 1 is the weakest non-off value
/// and 0 is off.
pub const MIN_LIVE_WIRE_POWER: u8 = 1;

/// How many wire blocks in a straight line away from a source carry a non-zero signal, in
/// this model.
///
/// **derived**, and deliberately *one less* than the wiki's phrasing, which is spelled out
/// here rather than hidden:
///
/// - minecraft.wiki, *Redstone mechanics* section "Signal transmission" says "Redstone dust
///   can thus transmit a signal up to 15 blocks by itself".
/// - The rule this crate applies to a wire is "the strongest neighbouring emission minus
///   [`WIRE_ATTENUATION_PER_BLOCK`]", so the wire immediately next to a 15-strength source
///   carries 14, not 15. The line then reads 14, 13, ... 1 and the 15th wire block is at 0.
/// - That is `MAX_POWER - WIRE_ATTENUATION_PER_BLOCK` live blocks, which is 14.
///
/// The off-by-one is a real difference from the wiki's sentence, not a rounding artefact: this
/// model charges the first wire block one attenuation step, while the wiki's sentence counts
/// the source's own strength as the first block. Which of the two matches 26.1.2 is **not
/// verified here**; the implemented number is 14, `tests/propagation.rs` asserts 14, and this
/// constant exists so the number has one name.
///
/// **derived** arithmetic: `MAX_POWER / WIRE_ATTENUATION_PER_BLOCK - 1 = 14`.
pub const WIRE_LIVE_BLOCKS: u32 =
    MAX_POWER as u32 / WIRE_ATTENUATION_PER_BLOCK as u32 - WIRE_ATTENUATION_PER_BLOCK as u32;

/// A readable and writable view of block states, as the algorithm needs it.
///
/// The trait exists so the algorithm can be tested against a flat array
/// ([`FlatWorld`]) without a loaded world, and so `mc-world`'s `World` can implement it
/// directly (it does: see the `impl` at the bottom of this file). It is deliberately
/// tiny: a read, a write, and nothing else. No chunk loading, no lighting, no block
/// entities.
pub trait BlockView {
    /// The block-state id at `pos`, or `None` when the position is not loaded.
    ///
    /// `None` is not an error and not air: it means "this crate must not reason about
    /// this position". An unloaded position makes its own recomputation a no-op and
    /// contributes nothing to its neighbours' inputs.
    fn get_state(&self, pos: BlockPos) -> Option<i32>;

    /// Write a block-state id.
    ///
    /// Returns whether a change happened. A `false` return must mean nothing changed:
    /// the propagation loop treats a refused write as "the block could not be updated"
    /// and stops propagating from it rather than assuming the write landed.
    fn set_state(&mut self, pos: BlockPos, state: i32) -> bool;
}

/// The three roles a block can play in this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRole {
    /// Redstone dust: takes the strongest neighbouring emission, minus attenuation, and
    /// stores it in its `power` property.
    Wire {
        /// The power level currently stored in the block state.
        stored: PowerLevel,
    },
    /// A power component: what it emits is decided by its **own stored state** (a lever with
    /// `powered=false` emits nothing, a torch with `lit=false` emits nothing) and, for a
    /// comparator, by its input.
    ///
    /// [`PowerSource`] alone is not enough, because it names the *kind* of component and not
    /// whether this particular one is switched on. [`EmitterTable::emitted_for_id`] therefore
    /// reads the state: [`EmitterTable::component_of`] turns the block-state id into a
    /// [`ComponentState`], and [`ComponentState::output_power`] is the pure truth table.
    Emitter {
        /// Which kind of component this is, for state lookups and reporting.
        source: PowerSource,
    },
    /// Everything else.
    ///
    /// **approximation** - Vanilla would call some of these conductive and power them
    /// weakly, which is how a lever powers dust *through* a block. This model has no
    /// conductivity table, so a passive block is never powered and never passes power
    /// on. One visible consequence: "lever attached to a block, dust on the far side"
    /// does not work here, while "lever next to dust" does.
    Passive,
    /// A mechanism the circuit drives (P13-03).
    ///
    /// Only the redstone lamp so far: it lights when powered from any side, which
    /// needs no facing. A mechanism emits nothing itself — neighbours read it as
    /// dark either way — so this role exists for the *reaction*, not the emission.
    Mechanism,
}

/// The power one block emits, attributed to its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitterOutput {
    /// The state being emitted.
    pub state: PowerState,
    /// Which kind of signal it is.
    pub kind: SignalKind,
}

impl EmitterOutput {
    /// Off, and weak (an off signal has no kind; weak is the harmless default).
    pub const OFF: Self = Self {
        state: PowerState::OFF,
        kind: SignalKind::Weak,
    };

    /// An emission of `state`. The kind follows the state: a non-zero `strong` field
    /// means the block is strongly powered, which is the only thing the strong/weak
    /// distinction exists for.
    #[must_use]
    pub const fn of(state: PowerState) -> Self {
        let kind = if state.strong.get() > 0 {
            SignalKind::Strong
        } else {
            SignalKind::Weak
        };
        Self { state, kind }
    }

    /// The level a consumer reads.
    #[must_use]
    pub const fn effective(&self) -> PowerLevel {
        self.state.effective()
    }

    /// Whether this is a strong emission, i.e. one that powers adjacent dust.
    #[must_use]
    pub const fn is_strong(&self) -> bool {
        matches!(self.kind, SignalKind::Strong)
    }
}

/// One block's six neighbour emissions, gathered in the canonical face order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInputs {
    /// Emissions of `down, up, north(-z), south(+z), west(-x), east(+x)`.
    pub faces: [EmitterOutput; 6],
}

impl BlockInputs {
    /// No input from any face.
    pub const NONE: Self = Self {
        faces: [EmitterOutput::OFF; 6],
    };

    /// The strongest emission across the six faces.
    ///
    /// Ties keep the earlier face, which is the canonical `down, up, ...` order, so the
    /// result is a function of the inputs alone.
    #[must_use]
    pub fn strongest(&self) -> EmitterOutput {
        let mut best = EmitterOutput::OFF;
        for output in self.faces {
            if output.effective().get() > best.effective().get() {
                best = output;
            }
        }
        best
    }

    /// The aggregate input a component reads, as a [`PowerState`].
    ///
    /// **product decision** - the aggregate is `max(weak, strong)` per face and then the
    /// maximum across faces. `max` is the same rule [`PowerState::effective`] applies to
    /// one face; see its docs for why that rule is not claimed as verified.
    #[must_use]
    pub fn strongest_state(&self) -> PowerState {
        let mut best = PowerState::OFF;
        for output in self.faces {
            best = best.max(PowerState::weak_only(output.effective()));
        }
        best
    }

    /// The number of faces that carry a signal.
    #[must_use]
    pub fn live_faces(&self) -> usize {
        self.faces
            .iter()
            .filter(|output| output.state.is_powered())
            .count()
    }

    /// Whether any face carries a signal.
    #[must_use]
    pub fn any_live(&self) -> bool {
        self.live_faces() != 0
    }
}

/// Attributes a block-state id to a [`BlockRole`], and resolves the states the
/// algorithm reads and writes.
///
/// This is the only place the algorithm consults block *names*, which keeps the inner
/// loop free of string matching and keeps the name list in one visible place
/// ([`crate::blocks`]).
///
/// The type is `Copy` and one machine word - it is a borrowed registry - so it is passed
/// by value.
#[derive(Debug, Clone, Copy)]
pub struct EmitterTable<'a> {
    registry: &'a BlockRegistry,
}

impl<'a> EmitterTable<'a> {
    /// A table over a block registry.
    #[must_use]
    pub const fn new(registry: &'a BlockRegistry) -> Self {
        Self { registry }
    }

    /// The registry.
    #[must_use]
    pub const fn registry(&self) -> &'a BlockRegistry {
        self.registry
    }

    /// The registered name of `id`.
    ///
    /// Returns `None` for an id the registry does not know. That is not an error here: a
    /// world read can return a stale or corrupt id, and the propagation loop's answer is
    /// to treat the block as [`BlockRole::Passive`] rather than to fail a tick
    /// (AGENTS.md section 9).
    #[must_use]
    pub fn name_of(&self, id: i32) -> Option<&'a str> {
        self.registry.block_name(id).ok()
    }

    /// The role of a block-state id.
    ///
    /// **approximation** - the three buckets stand in for Vanilla's block classes:
    ///
    /// - **wire** is [`REDSTONE_WIRE`] with a readable `power` property, and nothing
    ///   else, so the "cut" and "connect vertically" rules of *Conductivity* do not
    ///   exist here;
    /// - **emitter** is any block named in [`crate::blocks::SOURCE_BLOCKS`];
    /// - **mechanism** is [`crate::blocks::REDSTONE_LAMP`], the one driven block;
    /// - **passive** is everything else, *excluding* the implemented mechanism: other
    ///   mechanisms (pistons, doors, dispensers, …) stay passive and never react —
    ///   they are not silently treated as something that works.
    ///
    /// An unreadable id, an id with unreadable properties, or a wire whose `power`
    /// property cannot be parsed is passive: an unreadable state is not guessed at.
    ///
    /// An *inactive* component (a lever with `powered=false`, a torch with `lit=false`)
    /// is still [`BlockRole::Emitter`], because the reason to recompute it is its own
    /// state rather than its input, and because that keeps "this block is a redstone
    /// component" separate from "this block is currently emitting". Whether it is emitting is
    /// answered by [`EmitterTable::emitted_for_id`], not by the role.
    #[must_use]
    pub fn classify(&self, id: i32) -> BlockRole {
        let Ok(name) = self.registry.block_name(id) else {
            return BlockRole::Passive;
        };
        if name == REDSTONE_WIRE {
            return match self.wire_power(id) {
                Some(stored) => BlockRole::Wire { stored },
                None => BlockRole::Passive,
            };
        }
        if name == REDSTONE_LAMP {
            return BlockRole::Mechanism;
        }
        match source_for_name(name) {
            Some(source) => BlockRole::Emitter { source },
            None => BlockRole::Passive,
        }
    }

    /// The power stored in a redstone-wire state, or `None` when the state is not a
    /// readable wire.
    ///
    /// The property is `power`, matching the registry fixture's
    /// `minecraft:redstone_wire ... power=0|1|...|15`. The value is parsed and clamped
    /// through [`PowerLevel::from_raw`]: a corrupt property is clamped into range rather
    /// than failing, because a world read must not be able to abort a tick.
    #[must_use]
    pub fn wire_power(&self, id: i32) -> Option<PowerLevel> {
        let properties = self.registry.properties_of(id).ok()?;
        let (_, value) = properties.iter().find(|(name, _)| name == "power")?;
        value.parse::<u8>().ok().map(PowerLevel::from_raw)
    }

    /// The state id of a redstone wire carrying `power`.
    ///
    /// The four side-connection properties are written as `side`, the shape of a wire
    /// with no upward connection. **approximation**: shape is never recomputed, so a
    /// wire that should connect upward keeps a `side` shape. That is a shape and render
    /// difference, not a power difference.
    ///
    /// # Errors
    ///
    /// `ServerError::CorruptData` when the registry has no `minecraft:redstone_wire`
    /// state for these properties - a broken registry, not a hostile input.
    pub fn wire_state(&self, power: PowerLevel) -> ServerResult<i32> {
        let properties = [
            ("east".to_owned(), "side".to_owned()),
            ("north".to_owned(), "side".to_owned()),
            ("power".to_owned(), power.get().to_string()),
            ("south".to_owned(), "side".to_owned()),
            ("west".to_owned(), "side".to_owned()),
        ];
        self.registry.state_id(REDSTONE_WIRE, &properties)
    }

    /// The state id of a known component state.
    ///
    /// Only the power-relevant properties are supplied ([`ComponentState::properties`]),
    /// so a component's `facing`/`face` keeps the registry's first value - the
    /// orientation this crate does not model.
    ///
    /// # Errors
    ///
    /// `ServerError::CorruptData` when the registry has no state for that block and
    /// those properties.
    pub fn component_state(&self, component: ComponentState) -> ServerResult<i32> {
        self.registry.state_id(
            crate::blocks::primary_block_name(component.source()),
            &component.properties(),
        )
    }

    /// The component state a block id represents, when it is one this crate models.
    ///
    /// # Errors
    ///
    /// `ServerError::CorruptData` when the id is not a modelled component state, or the
    /// registry cannot read it. A source this model recognises but has no component type
    /// for (a button, a pressure plate, a block of redstone, a lightning rod) is an error
    /// rather than an approximation by a nearby component, so a caller cannot mistake a
    /// button for a lever.
    pub fn component_of(&self, id: i32) -> ServerResult<ComponentState> {
        let source = match self.classify(id) {
            BlockRole::Emitter { source } => source,
            BlockRole::Wire { .. } | BlockRole::Passive | BlockRole::Mechanism => {
                return Err(mc_core::error::ServerError::CorruptData(format!(
                    "block state {id} is not a modelled redstone component"
                )));
            }
        };
        let properties = self.registry.properties_of(id)?;
        let powered = |property: &str| {
            properties
                .iter()
                .any(|(key, value)| key == property && value == "true")
        };
        Ok(match source {
            PowerSource::Lever => ComponentState::Lever(Lever::new(powered("powered"))),
            PowerSource::Torch => ComponentState::Torch(RedstoneTorch::new(powered("lit"))),
            PowerSource::Repeater => {
                let delay = properties
                    .iter()
                    .find(|(key, _)| key == "delay")
                    .and_then(|(_, value)| value.parse::<u32>().ok())
                    .unwrap_or(Repeater::MIN_DELAY);
                ComponentState::Repeater(Repeater {
                    delay: crate::components::clamp_repeater_delay(delay),
                    powered: powered("powered"),
                    locked: powered("locked"),
                })
            }
            PowerSource::Comparator => {
                let mode = properties.iter().find(|(key, _)| key == "mode").map_or(
                    ComparatorMode::Compare,
                    |(_, value)| {
                        if value == "subtract" {
                            ComparatorMode::Subtract
                        } else {
                            ComparatorMode::Compare
                        }
                    },
                );
                ComponentState::Comparator(Comparator {
                    mode,
                    powered: powered("powered"),
                })
            }
            PowerSource::RedstoneBlock
            | PowerSource::Button
            | PowerSource::PressurePlate
            | PowerSource::LightningRod => {
                return Err(mc_core::error::ServerError::CorruptData(format!(
                    "{} has no modelled component state",
                    source.name()
                )));
            }
        })
    }

    /// The power a block emits, given its role and its neighbours' inputs.
    ///
    /// - [`BlockRole::Emitter`] -> the emission of the component's own state, except for
    ///   a comparator, whose output is its back input. Every other emitter ignores
    ///   `inputs`, which is what makes a repeater emit 15 regardless of input strength.
    ///   **approximation**: a direction-less comparator takes its "back" input to be the
    ///   strongest neighbour and its side inputs to be zero, so it behaves as a
    ///   compare-mode pass-through. Its real side inputs are the two perpendicular faces,
    ///   which need orientation.
    /// - [`BlockRole::Wire`] -> the strongest neighbour emission minus
    ///   [`WIRE_ATTENUATION_PER_BLOCK`], saturating at 0, emitted **weakly**. A wire never
    ///   emits strongly, which is this model's form of "a block becomes weakly powered
    ///   when it is powered only by redstone dust".
    /// - [`BlockRole::Passive`] -> off.
    ///
    /// **Prefer [`EmitterTable::emitted_for_id`]**, which resolves the component's live state
    /// from the block id. This method cannot tell an inactive component from an active one —
    /// a role of `Emitter { source: Torch }` says "this is a torch", not "this torch is lit" —
    /// so it is only correct for a role the caller has separately confirmed is active. It is
    /// public for callers that already hold a [`ComponentState`].
    #[must_use]
    pub fn emitted(&self, role: BlockRole, inputs: &BlockInputs) -> EmitterOutput {
        match role {
            BlockRole::Wire { .. } => {
                let strongest = inputs.strongest().effective();
                EmitterOutput::of(PowerState::weak_only(
                    strongest.saturating_sub(WIRE_ATTENUATION_PER_BLOCK),
                ))
            }
            BlockRole::Emitter { source } => {
                let state = match source {
                    PowerSource::Comparator => {
                        let back = inputs.strongest_state().effective();
                        Comparator::new(ComparatorMode::Compare).output(back, PowerLevel::ZERO)
                    }
                    other => other.active_state(),
                };
                EmitterOutput::of(state)
            }
            // A mechanism emits nothing itself; its reaction lives in
            // [`EmitterTable::new_state`].
            BlockRole::Mechanism | BlockRole::Passive => EmitterOutput::OFF,
        }
    }

    /// The power the block state `id` emits, given its neighbours' inputs.
    ///
    /// This is the function the propagation loop uses, and it is the one that reads the state:
    /// a lever with `powered=false` and a torch with `lit=false` emit nothing, because their
    /// stored state says so. [`EmitterTable::emitted`] cannot see that, which is exactly the
    /// error this method exists to prevent.
    ///
    /// An unreadable component emits nothing rather than being guessed at. The four sources
    /// this model recognises but has no component type for ([`PowerSource::RedstoneBlock`],
    /// [`PowerSource::Button`], [`PowerSource::PressurePlate`],
    /// [`PowerSource::LightningRod`]) emit their [`PowerSource::active_state`], because for
    /// those the block state carries no property this model reads — so no state is being
    /// ignored.
    #[must_use]
    pub fn emitted_for_id(&self, id: i32, inputs: &BlockInputs) -> EmitterOutput {
        let role = self.classify(id);
        match role {
            BlockRole::Passive | BlockRole::Mechanism => EmitterOutput::OFF,
            BlockRole::Wire { .. } => self.emitted(role, inputs),
            BlockRole::Emitter { .. } => match self.component_of(id) {
                Ok(component) => {
                    EmitterOutput::of(component.output_power(inputs.strongest_state()))
                }
                Err(_) => self.emitted(role, &BlockInputs::NONE),
            },
        }
    }

    /// The new block-state id for a block with `role`, or `None` when the block cannot
    /// be recomputed.
    ///
    /// A wire's new state encodes the power this algorithm computed. A mechanism's
    /// new state encodes its reaction: the lamp lights when any neighbour emits
    /// (lamps are side-agnostic, so no facing is needed). Every other block
    /// keeps its id: this pass does not write `powered`/`lit` back into
    /// emitters, because nothing in it *drives* those properties from a circuit
    /// yet — lever flips arrive as player actions, torch/repeater/comparator
    /// state changes need P13-04's orientation work. The model's computed power
    /// values otherwise live in wire states only.
    #[must_use]
    pub fn new_state(&self, role: BlockRole, inputs: &BlockInputs, current_id: i32) -> Option<i32> {
        match role {
            BlockRole::Wire { .. } => {
                let power = self.emitted(role, inputs).effective();
                self.wire_state(power).ok()
            }
            BlockRole::Mechanism => {
                let lit = inputs.strongest_state().is_powered();
                let properties = self.registry.properties_of(current_id).ok()?;
                let current = properties
                    .iter()
                    .find(|(name, _)| name == "lit")
                    .is_some_and(|(_, value)| value == "true");
                if lit == current {
                    return Some(current_id);
                }
                self.registry
                    .state_id(
                        crate::blocks::REDSTONE_LAMP,
                        &[("lit".to_owned(), lit.to_string())],
                    )
                    .ok()
            }
            BlockRole::Emitter { .. } | BlockRole::Passive => Some(current_id),
        }
    }
}

/// Read the current inputs and emitted output of one block.
///
/// Returns `None` when the position is not loaded ([`BlockView::get_state`] returned
/// `None`), which makes the block un-updatable rather than air.
#[must_use]
pub fn read_block<V: BlockView + ?Sized>(
    world: &V,
    table: EmitterTable<'_>,
    pos: BlockPos,
) -> Option<(BlockRole, BlockInputs, EmitterOutput)> {
    let id = world.get_state(pos)?;
    let role = table.classify(id);
    let inputs = gather_inputs(world, table, pos);
    let output = table.emitted_for_id(id, &inputs);
    Some((role, inputs, output))
}

/// A block's inputs: what each of its six neighbours currently emits.
///
/// A wire's emission depends on *its* neighbours' emissions, so this walks exactly one
/// step further and stops. Three things keep that finite and cheap:
///
/// - a wire's own stored `power` is only used for a *neighbouring* wire, never for
///   itself, so a wire cannot cite itself;
/// - [`BlockRole::Passive`] emits nothing, terminating most walks immediately;
/// - a neighbouring wire is read from its stored `power` property rather than by
///   recursing again, so a dust loop terminates instead of recursing forever.
///
/// The result is that a wire's power here is always "one attenuation step from what its
/// neighbours currently emit", which is the same rule the propagation loop applies when it
/// recomputes that wire directly. They agree by construction, and
/// `tests/golden_circuits.rs` checks the numbers.
#[must_use]
pub fn gather_inputs<V: BlockView + ?Sized>(
    world: &V,
    table: EmitterTable<'_>,
    pos: BlockPos,
) -> BlockInputs {
    let mut faces = [EmitterOutput::OFF; 6];
    for (index, neighbour) in NeighbourSet::of(pos).into_iter().enumerate() {
        let Some(neighbour_id) = world.get_state(neighbour) else {
            continue;
        };
        faces[index] = match table.classify(neighbour_id) {
            BlockRole::Wire { stored } => {
                let inner = gather_wire_inputs(world, table, neighbour);
                table.emitted(BlockRole::Wire { stored }, &inner)
            }
            // A component's emission is read from its own state, which is why this goes
            // through the id: an unlit torch must emit nothing here.
            BlockRole::Emitter { .. } => table.emitted_for_id(neighbour_id, &BlockInputs::NONE),
            // Mechanisms emit nothing (a lit lamp does not power neighbours).
            BlockRole::Mechanism | BlockRole::Passive => EmitterOutput::OFF,
        };
    }
    BlockInputs { faces }
}

/// The inputs of a wire's own neighbours, one level deep and then read from state.
fn gather_wire_inputs<V: BlockView + ?Sized>(
    world: &V,
    table: EmitterTable<'_>,
    wire: BlockPos,
) -> BlockInputs {
    let mut faces = [EmitterOutput::OFF; 6];
    for (index, neighbour) in NeighbourSet::of(wire).into_iter().enumerate() {
        let Some(neighbour_id) = world.get_state(neighbour) else {
            continue;
        };
        faces[index] = match table.classify(neighbour_id) {
            // A wire next to a wire cites the stored level instead of recursing, so a
            // dust loop reads its own previous state rather than recursing forever.
            BlockRole::Wire { stored } => EmitterOutput::of(PowerState::weak_only(stored)),
            BlockRole::Emitter { .. } => table.emitted_for_id(neighbour_id, &BlockInputs::NONE),
            BlockRole::Mechanism | BlockRole::Passive => EmitterOutput::OFF,
        };
    }
    BlockInputs { faces }
}

/// One block that changed, in the order it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropagatedChange {
    /// Where.
    pub pos: BlockPos,
    /// The update that caused it.
    pub cause: UpdateKind,
    /// Block-state id before.
    pub old_state: i32,
    /// Block-state id after.
    pub new_state: i32,
    /// The power the block emits after the change.
    pub emitted: EmitterOutput,
}

/// What one [`propagate`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropagationReport {
    /// Neighbour updates processed.
    pub updates_processed: usize,
    /// Blocks whose state actually changed.
    pub blocks_changed: usize,
    /// Whether **the budget** stopped this call: it was fully consumed with positions still
    /// serviceable.
    ///
    /// It means "redstone is doing more work than a tick is allowed", which is the operator-facing
    /// signal that a circuit is starving the tick. A call that consumed its whole budget and finished
    /// reports `false`, and so does a call that finished early.
    pub budget_exhausted: bool,
    /// Every change, in processing order.
    ///
    /// The order is the evidence for determinism: `tests/determinism.rs` compares this
    /// whole vector between two identical runs, not just the final state.
    pub changes: Vec<PropagatedChange>,
    /// Neighbour updates **refused** because the queue hit its entry cap.
    ///
    /// A refused push is for a position not already pending, and it is not guaranteed
    /// to be re-derived: a later change to one of its neighbours would raise a fresh
    /// push, but a circuit that has settled will not produce one. So a non-zero value
    /// means redstone may be stale somewhere, and it is reported rather than
    /// discarded (Audit 04 A5). Reaching it needs >4 096 distinct simultaneously
    /// pending positions, which a normal circuit does not approach.
    pub updates_refused: usize,
}

impl PropagationReport {
    /// An empty report.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            updates_processed: 0,
            blocks_changed: 0,
            budget_exhausted: false,
            changes: Vec::new(),
            updates_refused: 0,
        }
    }

    /// Whether any update was processed.
    #[must_use]
    pub fn did_work(&self) -> bool {
        self.updates_processed != 0
    }
}

/// Seed the queue with the neighbour updates a change at `pos` must raise.
///
/// This is step 1 of the algorithm and it is *not* done by [`propagate`], because only the
/// code that changed the block knows it changed. Returns the positions newly queued.
pub fn prepare(queue: &mut UpdateQueue, pos: BlockPos) -> usize {
    let positions: Vec<BlockPos> = NeighbourSet::of(pos).into_iter().collect();
    queue.push_neighbours(&positions)
}

/// Queue an update for `pos` itself, so its own state is recomputed.
///
/// A block that was just replaced needs this: nothing else will recompute it. A wire needs
/// it when the *shape* of the circuit changed rather than a neighbour's power.
pub fn prepare_self(queue: &mut UpdateQueue, pos: BlockPos) -> Inserted {
    queue.push_neighbour(pos)
}

/// Process queued updates until the queue is settled or the budget runs out.
///
/// `budget.neighbour_updates_per_tick` bounds this call **exactly**: the loop drains at most what is
/// left of the budget and processes everything it drained, so
/// `report.updates_processed <= budget.neighbour_updates_per_tick` always holds with no truncation
/// and nothing to put back. `0` is legal and means "do nothing", which reports
/// [`PropagationReport::budget_exhausted`] only when there was in fact work to do.
///
/// ## Nothing is lost, and the loop terminates
///
/// Each iteration drains at most the remaining budget, so the loop either makes progress or stops:
///
/// - **Budget spent.** Everything the drain did not take stays in the queue. So calling `propagate`
///   once per tick with a small budget reaches exactly the final state an unlimited budget reaches,
///   only later — which is what `tests/budget_exhaustion.rs` asserts on a line long enough to force a
///   split.
/// - **Nothing left.** The queue is empty and the circuit has settled at its fixed point. Under a
///   budget large enough for the circuit, that happens in a single call: a change's neighbours are
///   re-queued immediately and serviced as the sweep cursor reaches them, so an increase travels the
///   whole line and so does a **decrease** — which is what stops a destroyed source from leaving the
///   far end of a line lit (`tests/propagation.rs`).
///
/// The loop condition is `next_pending`, i.e. "is anything queued", and the drain is capped at the
/// remaining budget, so the loop cannot spin past its budget.
///
/// The order is stable across calls and across ticks: the queue is ordered by position, and its sweep
/// cursor advances only past work that was actually taken, so a caller can call again and continue
/// from where this call stopped.
pub fn propagate<V: BlockView + ?Sized>(
    world: &mut V,
    queue: &mut UpdateQueue,
    table: EmitterTable<'_>,
    budget: UpdateBudget,
) -> PropagationReport {
    let mut report = PropagationReport::empty();

    while report.updates_processed < budget.neighbour_updates_per_tick
        && queue.next_pending().is_some()
    {
        // Drain exactly what is left of the budget. The cap must be the number this call will
        // actually process, or the queue's sweep cursor would advance past work that was never done
        // and the rotation would stop being fair.
        let remaining = budget.neighbour_updates_per_tick - report.updates_processed;
        let drained = queue.drain_at_most(remaining);
        if drained.is_empty() {
            break;
        }
        for pos in drained.positions {
            report.updates_processed += 1;
            if let Some(change) = process_one(world, table, pos, &mut report.changes) {
                report.blocks_changed += 1;
                // The changed block's emission changed, so each neighbour is a candidate — pushed
                // regardless of whether the change was an increase or a decrease, which is what makes
                // breaking a source propagate. A neighbour whose recomputation is a no-op costs one
                // budget unit and changes nothing, which is what keeps the queue from growing on a
                // settled circuit.
                for neighbour in NeighbourSet::of(pos) {
                    if queue.push_neighbour(neighbour) == Inserted::Refused {
                        report.updates_refused += 1;
                    }
                }
                debug_assert_ne!(change.old_state, change.new_state);
            }
        }
    }

    // `budget_exhausted` means **the budget stopped this call**: it was fully consumed with positions
    // still queued. A call that consumed its whole budget and then finished, and a call that finished
    // early, both report false — which is what makes the flag usable as "redstone is starving the
    // tick" rather than "redstone did anything at all".
    report.budget_exhausted = queue.next_pending().is_some()
        && report.updates_processed >= budget.neighbour_updates_per_tick;
    report
}

/// Recompute one position and write it if it changed.
fn process_one<V: BlockView + ?Sized>(
    world: &mut V,
    table: EmitterTable<'_>,
    pos: BlockPos,
    changes: &mut Vec<PropagatedChange>,
) -> Option<PropagatedChange> {
    let id = world.get_state(pos)?;
    let role = table.classify(id);
    let inputs = gather_inputs(world, table, pos);
    let emitted = table.emitted_for_id(id, &inputs);
    let new_state = table.new_state(role, &inputs, id)?;
    tracing::trace!(
        %pos,
        ?role,
        ?inputs,
        ?emitted,
        id,
        ?new_state,
        "redstone recompute"
    );
    if new_state == id {
        return None;
    }
    if !world.set_state(pos, new_state) {
        // A refused write means the block could not be updated. Propagating from a state
        // that was never written would put the model out of step with the world, so the
        // update stops here instead.
        tracing::debug!(%pos, old_state = id, new_state, "redstone write refused");
        return None;
    }
    let change = PropagatedChange {
        pos,
        cause: UpdateKind::NeighborChanged,
        old_state: id,
        new_state,
        emitted,
    };
    changes.push(change);
    Some(change)
}

/// Queue a re-check one tick from `now` for every **live** wire in `positions`.
///
/// **approximation, and the reason the scheduled-tick path exists at all.** Vanilla
/// re-evaluates redstone dust on the game tick after it changes, which is what makes a dust
/// loop a clock. This function reproduces the "ask again next tick" behaviour for powered
/// wires: every wire it is asked about that currently carries a signal is scheduled one tick
/// ahead, whether or not this tick changed it. That is what makes the schedule
/// **self-perpetuating** — a live wire that was due this tick is live again next tick, so a
/// live dust line is re-evaluated continuously, which is the shape of a redstone clock.
///
/// It is **not** Vanilla's dust delay, and it does not make a dust loop oscillate: the wire's
/// power is recomputed by [`propagate`], and nothing here changes it. A wire that is dark is
/// not rescheduled, so a settled circuit costs nothing.
///
/// Returns how many positions were newly scheduled.
pub fn schedule_wire_recheck(
    world: &(impl BlockView + ?Sized),
    table: EmitterTable<'_>,
    queue: &mut UpdateQueue,
    now: u64,
    positions: &[BlockPos],
) -> usize {
    let mut scheduled = 0;
    for pos in positions {
        let Some(id) = world.get_state(*pos) else {
            continue;
        };
        if !matches!(table.classify(id), BlockRole::Wire { .. }) {
            continue;
        }
        let live = table
            .wire_power(id)
            .is_some_and(|power| power.get() >= MIN_LIVE_WIRE_POWER);
        if live && queue.schedule(now, *pos, 1).is_ok() {
            scheduled += 1;
        }
    }
    scheduled
}

/// Run one tick's worth of scheduled block ticks, then propagate what they touch.
///
/// This is the body of `TickPhase::ScheduledTicks` for redstone. It:
///
/// 1. drains the scheduled ticks due at `now` ([`UpdateQueue::drain_due_block_ticks`]);
/// 2. queues a self and neighbour recomputation for each;
/// 3. runs [`propagate`] with the neighbour budget;
/// 4. re-schedules every wire that the recomputation *changed*, one tick ahead
///    ([`schedule_wire_recheck`]), which is what keeps a live dust line waking up each tick.
///
/// Step 4 runs on the positions the propagation actually wrote, not on the positions that
/// came due. That distinction matters: a wire that a neighbour powered this tick was not
/// itself due, so scheduling only the due positions would let a line fall asleep after one
/// tick. A wire whose recomputation is a no-op stops being rescheduled, so a settled circuit
/// costs nothing.
///
/// A scheduled tick that reschedules itself every tick is legal and is what a clock is. The
/// bound is the two caps in [`UpdateQueue`] plus the budget here, never an iteration limit.
pub fn run_block_tick<V: BlockView + ?Sized>(
    world: &mut V,
    queue: &mut UpdateQueue,
    table: EmitterTable<'_>,
    now: u64,
    budget: UpdateBudget,
) -> PropagationReport {
    let due = queue.drain_due_block_ticks(now, budget);
    for pos in &due.positions {
        prepare(queue, *pos);
        prepare_self(queue, *pos);
    }
    let report = propagate(world, queue, table, budget);
    let changed: Vec<BlockPos> = report
        .changes
        .iter()
        .map(|change| change.pos)
        .chain(due.positions.iter().copied())
        .collect();
    let _ = schedule_wire_recheck(world, table, queue, now, &changed);
    report
}

/// Build the component state a modelled block id represents, for callers that have a block
/// and want its behaviour.
///
/// # Errors
///
/// `ServerError::CorruptData` when the id is not a modelled component or the registry
/// cannot read it - see [`EmitterTable::component_of`].
pub fn component_of(table: EmitterTable<'_>, id: i32) -> ServerResult<ComponentState> {
    table.component_of(id)
}

impl BlockView for mc_world::World {
    /// Block id at `pos`, or `None` when the chunk is not loaded.
    ///
    /// Uses `get_block_loaded` on purpose: `get_block` answers `0` (air) for an unloaded
    /// chunk, and "air" and "not loaded" must not be the same answer to a propagation loop:
    /// one allows reasoning about the position, the other forbids it. Neither *loads* a
    /// chunk, so a redstone update cannot make the server generate terrain
    /// (AGENTS.md section 10).
    fn get_state(&self, pos: BlockPos) -> Option<i32> {
        self.get_block_loaded(pos.x, pos.y, pos.z)
    }

    /// Write a block id, recording the change for broadcast and saving.
    ///
    /// Returns `false` when the world refused the write (an out-of-range `y`) or when the
    /// write was a no-op, which the propagation loop treats as "not updatable".
    fn set_state(&mut self, pos: BlockPos, state: i32) -> bool {
        self.set_block(pos.x, pos.y, pos.z, state)
            .ok()
            .flatten()
            .is_some()
    }
}

/// A flat, finite block array that implements [`BlockView`], for tests and for callers that
/// model a circuit without a loaded world.
///
/// Writes are allowed anywhere inside the box and never create anything. Reads outside it
/// return `None`, exactly like an unloaded chunk in `mc_world::World`, so boundary behaviour
/// is testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatWorld {
    min: BlockPos,
    size: (u32, u32, u32),
    states: Vec<i32>,
}

impl FlatWorld {
    /// An all-air box of `size` blocks with its lowest corner at `min`.
    ///
    /// A zero extent on any axis yields an empty view (every read is `None`) rather than an
    /// error, so a degenerate region cannot panic.
    #[must_use]
    pub fn new(min: BlockPos, size: (u32, u32, u32)) -> Self {
        let cells = (size.0 as usize)
            .saturating_mul(size.1 as usize)
            .saturating_mul(size.2 as usize);
        Self {
            min,
            size,
            states: vec![0; cells],
        }
    }

    /// A box of `size` blocks whose lowest corner is the origin.
    #[must_use]
    pub fn boxed(size: (u32, u32, u32)) -> Self {
        Self::new(BlockPos::new(0, 0, 0), size)
    }

    /// The box's extent.
    #[must_use]
    pub const fn size(&self) -> (u32, u32, u32) {
        self.size
    }

    /// The index of `pos`, or `None` when it is outside the box.
    fn index(&self, pos: BlockPos) -> Option<usize> {
        let x = u32::try_from(pos.x.checked_sub(self.min.x)?).ok()?;
        let y = u32::try_from(pos.y.checked_sub(self.min.y)?).ok()?;
        let z = u32::try_from(pos.z.checked_sub(self.min.z)?).ok()?;
        if x >= self.size.0 || y >= self.size.1 || z >= self.size.2 {
            return None;
        }
        let row = self.size.0 as usize;
        Some(
            (x as usize)
                .saturating_add((z as usize).saturating_mul(row))
                .saturating_add(
                    (y as usize)
                        .saturating_mul(row)
                        .saturating_mul(self.size.2 as usize),
                ),
        )
    }

    /// The id at `pos`, or `None` outside the box.
    #[must_use]
    pub fn get(&self, pos: BlockPos) -> Option<i32> {
        self.index(pos)
            .and_then(|index| self.states.get(index))
            .copied()
    }

    /// Force an id at `pos`. Returns whether the write happened.
    ///
    /// This is a *test and modelling* helper, not part of the engine loop: it exists so a
    /// scenario can be described in one line. Engine writes go through [`propagate`], which
    /// uses [`BlockView::set_state`].
    pub fn set(&mut self, pos: BlockPos, id: i32) -> bool {
        match self.index(pos) {
            Some(index) => {
                self.states[index] = id;
                true
            }
            None => false,
        }
    }
}

impl BlockView for FlatWorld {
    fn get_state(&self, pos: BlockPos) -> Option<i32> {
        self.get(pos)
    }

    fn set_state(&mut self, pos: BlockPos, state: i32) -> bool {
        match self.index(pos) {
            Some(index) => {
                let slot = &mut self.states[index];
                if *slot == state {
                    return false;
                }
                *slot = state;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockInputs, BlockRole, BlockView, EmitterOutput, EmitterTable, FlatWorld,
        MIN_LIVE_WIRE_POWER, WIRE_ATTENUATION_PER_BLOCK, WIRE_LIVE_BLOCKS, prepare, prepare_self,
        propagate,
    };
    use crate::blocks::REDSTONE_WIRE;
    use crate::components::{
        Comparator, ComparatorMode, ComponentState, Lever, RedstoneTorch, Repeater,
    };
    use crate::power::{MAX_POWER, PowerLevel, PowerSource, PowerState, SignalKind};
    use crate::update::{BlockPos, Inserted, UpdateBudget, UpdateQueue};
    use mc_registry::{BlockRegistry, Registries};

    fn registry() -> BlockRegistry {
        Registries::vanilla().expect("registry").blocks
    }

    fn flat() -> FlatWorld {
        FlatWorld::boxed((32, 4, 8))
    }

    fn level(value: u8) -> PowerLevel {
        PowerLevel::new(value).expect("0..=15 is valid")
    }

    #[test]
    fn flat_world_reads_and_writes_inside_its_box_only() {
        let mut world = FlatWorld::new(BlockPos::new(-2, 60, 3), (4, 2, 4));
        assert_eq!(world.size(), (4, 2, 4));
        assert_eq!(world.get(BlockPos::new(-2, 60, 3)), Some(0), "all air");
        assert!(world.set(BlockPos::new(-2, 60, 3), 42));
        assert_eq!(world.get(BlockPos::new(-2, 60, 3)), Some(42));
        // Outside the box reads as `None`, like an unloaded chunk.
        for outside in [
            BlockPos::new(-3, 60, 3),
            BlockPos::new(-2, 59, 3),
            BlockPos::new(-2, 60, 2),
            BlockPos::new(2, 60, 3),
            BlockPos::new(i32::MAX, i32::MIN, i32::MAX),
        ] {
            assert_eq!(world.get(outside), None, "{outside}");
            assert!(!world.set(outside, 1), "{outside} must refuse the write");
            assert!(!BlockView::set_state(&mut world, outside, 1));
        }
        // A no-op write reports `false`: nothing changed.
        assert!(!BlockView::set_state(
            &mut world,
            BlockPos::new(-2, 60, 3),
            42
        ));
        assert!(BlockView::set_state(
            &mut world,
            BlockPos::new(-2, 60, 3),
            7
        ));
        // A degenerate box is empty, not a panic.
        let empty = FlatWorld::boxed((0, 4, 4));
        assert_eq!(empty.get(BlockPos::new(0, 0, 0)), None);
        assert_eq!(
            FlatWorld::boxed((1, 1, 1)).get(BlockPos::new(0, 0, 0)),
            Some(0)
        );
    }

    #[test]
    fn the_wire_rule_is_one_strength_per_block() {
        assert_eq!(WIRE_ATTENUATION_PER_BLOCK, 1);
        assert_eq!(WIRE_LIVE_BLOCKS, 14);
        assert_eq!(MIN_LIVE_WIRE_POWER, 1);
        // `WIRE_LIVE_BLOCKS` is a consequence of the other two, not a third number to keep
        // in sync. It is one less than `MAX_POWER / attenuation` because the first wire takes
        // one attenuation step off the source; see the constant's docs.
        assert_eq!(
            WIRE_LIVE_BLOCKS,
            u32::from(MAX_POWER) / u32::from(WIRE_ATTENUATION_PER_BLOCK) - 1
        );
    }

    #[test]
    fn emitted_kind_follows_the_strong_field() {
        assert!(!EmitterOutput::of(PowerState::weak_only(level(9))).is_strong());
        assert!(EmitterOutput::of(PowerState::strong_at(level(9))).is_strong());
        assert!(!EmitterOutput::OFF.is_strong());
        assert_eq!(EmitterOutput::OFF.effective(), PowerLevel::ZERO);
        assert_eq!(
            EmitterOutput::of(PowerState::new(level(2), level(9))).effective(),
            level(9)
        );
        assert_eq!(
            EmitterOutput::of(PowerState::strong_at(level(1))).kind,
            SignalKind::Strong
        );
    }

    #[test]
    fn inputs_aggregate_as_a_maximum_and_are_independent_of_the_order_they_arrived() {
        let mut one = BlockInputs::NONE;
        let mut other = BlockInputs::NONE;
        one.faces[3] = EmitterOutput::of(PowerState::strong_at(level(12)));
        one.faces[5] = EmitterOutput::of(PowerState::weak_only(level(4)));
        other.faces[5] = EmitterOutput::of(PowerState::weak_only(level(4)));
        other.faces[3] = EmitterOutput::of(PowerState::strong_at(level(12)));
        assert_eq!(one.strongest().effective(), level(12));
        assert_eq!(one.strongest_state().effective(), level(12));
        assert_eq!(one.live_faces(), 2);
        assert!(one.any_live());
        assert_eq!(one, other, "aggregation must not depend on fill order");
        assert!(!BlockInputs::NONE.any_live());
        assert_eq!(BlockInputs::NONE.live_faces(), 0);
        assert_eq!(BlockInputs::NONE.strongest().effective(), PowerLevel::ZERO);
        // A tie keeps the earlier face, which is a fixed rule rather than a coin flip.
        let mut tied = BlockInputs::NONE;
        tied.faces[0] = EmitterOutput::of(PowerState::weak_only(level(5)));
        tied.faces[1] = EmitterOutput::of(PowerState::weak_only(level(5)));
        assert_eq!(tied.strongest().effective(), level(5));
    }

    #[test]
    fn a_wire_takes_one_step_off_its_strongest_neighbour() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        for source in [
            PowerSource::RedstoneBlock,
            PowerSource::Torch,
            PowerSource::Lever,
            PowerSource::Repeater,
        ] {
            let mut inputs = BlockInputs::NONE;
            inputs.faces[0] = EmitterOutput::of(source.active_state());
            let role = BlockRole::Wire {
                stored: PowerLevel::ZERO,
            };
            assert_eq!(
                table.emitted(role, &inputs).effective(),
                level(MAX_POWER - WIRE_ATTENUATION_PER_BLOCK),
                "{source} next to a wire must give the wire 14"
            );
            assert!(
                !table.emitted(role, &inputs).is_strong(),
                "a wire is never a strong source"
            );
        }
        // A wire with nothing live next to it is off.
        assert_eq!(
            table
                .emitted(BlockRole::Wire { stored: level(9) }, &BlockInputs::NONE)
                .effective(),
            PowerLevel::ZERO
        );
    }

    #[test]
    fn a_passive_block_emits_nothing_and_is_never_powered() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let stone = registry.default_state("minecraft:stone").expect("stone");
        let lamp = registry
            .default_state("minecraft:redstone_lamp")
            .expect("lamp");
        assert_eq!(table.classify(stone), BlockRole::Passive);
        assert_eq!(table.classify(registry.air_id()), BlockRole::Passive);
        assert_eq!(
            table.classify(123_456_789),
            BlockRole::Passive,
            "unknown id"
        );
        assert_eq!(table.classify(-1), BlockRole::Passive, "negative id");
        assert_eq!(
            table.classify(lamp),
            BlockRole::Mechanism,
            "the lamp is the one driven mechanism (P13-03)"
        );
        let mut inputs = BlockInputs::NONE;
        inputs.faces[0] = EmitterOutput::of(PowerState::strong_at(PowerLevel::MAX));
        assert_eq!(
            table.emitted(BlockRole::Mechanism, &inputs),
            EmitterOutput::OFF,
            "a mechanism emits nothing itself; its reaction is the state write"
        );
        assert_eq!(
            table.new_state(BlockRole::Mechanism, &inputs, lamp),
            table
                .registry()
                .state_id(
                    "minecraft:redstone_lamp",
                    &[("lit".to_owned(), "true".to_owned())]
                )
                .ok(),
            "a powered lamp lights"
        );
        assert_eq!(
            table.emitted(BlockRole::Passive, &inputs),
            EmitterOutput::OFF,
            "conductivity is not modelled, so a passive block never becomes a source"
        );
        assert_eq!(
            table.new_state(BlockRole::Passive, &inputs, stone),
            Some(stone),
            "a passive block's state is left alone"
        );
    }

    #[test]
    fn classification_reads_names_not_hard_coded_ids() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let wire_default = registry.default_state(REDSTONE_WIRE).expect("wire");
        assert!(matches!(
            table.classify(wire_default),
            BlockRole::Wire { .. }
        ));
        assert_eq!(table.wire_power(wire_default), Some(PowerLevel::ZERO));
        // A wire at power 15 resolves to a different state id and classifies the same.
        let wire_full = table.wire_state(PowerLevel::MAX).expect("wire at 15");
        assert_ne!(wire_full, wire_default);
        assert!(matches!(table.classify(wire_full), BlockRole::Wire { .. }));
        assert_eq!(table.wire_power(wire_full), Some(PowerLevel::MAX));
        assert_eq!(
            table.name_of(wire_full),
            Some(REDSTONE_WIRE),
            "the name must come from the registry, not from a numeric assumption"
        );
        assert_eq!(table.name_of(-5), None);

        // Every 0..=15 wire state round-trips through `wire_state`/`wire_power`.
        for value in 0..=MAX_POWER {
            let id = table.wire_state(level(value)).expect("wire state");
            assert_eq!(table.wire_power(id), Some(level(value)), "power {value}");
            assert!(matches!(table.classify(id), BlockRole::Wire { .. }));
        }
    }

    #[test]
    fn every_source_block_classifies_as_its_source() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        for (name, source) in crate::blocks::SOURCE_BLOCKS {
            let id = registry.default_state(name).expect("block");
            assert_eq!(
                table.classify(id),
                BlockRole::Emitter { source: *source },
                "{name}"
            );
        }
    }

    #[test]
    fn an_unloaded_position_makes_its_update_a_no_op() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let mut world = FlatWorld::boxed((4, 2, 4));
        let mut queue = UpdateQueue::new();
        let outside = BlockPos::new(100, 0, 100);
        queue.push_neighbour(outside);
        prepare(&mut queue, BlockPos::new(0, 0, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        assert_eq!(report.blocks_changed, 0);
        assert!(!report.budget_exhausted);
    }

    #[test]
    fn prepare_queues_the_six_neighbours_and_prepare_self_queues_the_block() {
        let mut queue = UpdateQueue::new();
        let pos = BlockPos::new(5, 0, 5);
        assert_eq!(prepare(&mut queue, pos), 6);
        assert_eq!(queue.neighbour_len(), 6);
        assert_eq!(queue.push_neighbour(pos), Inserted::Inserted);
        assert!(!prepare_self(&mut queue, BlockPos::new(6, 0, 5)).was_inserted());
    }

    #[test]
    fn an_empty_queue_propagates_nothing_and_is_not_exhausted() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let mut world = flat();
        let mut queue = UpdateQueue::new();
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        assert_eq!(report.updates_processed, 0);
        assert_eq!(report.blocks_changed, 0);
        assert!(!report.budget_exhausted);
        assert!(!report.did_work());
        assert_eq!(report, super::PropagationReport::empty());
    }

    #[test]
    fn a_refused_write_stops_propagation_from_that_block() {
        // A sink that accepts nothing, like a world that refuses every write.
        struct FrozenWorld {
            states: Vec<(BlockPos, i32)>,
        }
        impl BlockView for FrozenWorld {
            fn get_state(&self, pos: BlockPos) -> Option<i32> {
                self.states
                    .iter()
                    .find(|(at, _)| *at == pos)
                    .map(|(_, id)| *id)
            }
            fn set_state(&mut self, _pos: BlockPos, _state: i32) -> bool {
                false
            }
        }
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let wire = table.wire_state(PowerLevel::MAX).expect("wire");
        let mut world = FrozenWorld {
            states: vec![(BlockPos::new(0, 0, 0), wire)],
        };
        let mut queue = UpdateQueue::new();
        prepare_self(&mut queue, BlockPos::new(0, 0, 0));
        let report = propagate(&mut world, &mut queue, table, UpdateBudget::nominal());
        // The write was refused, so nothing is reported as changed and the model did not
        // pretend the world moved.
        assert_eq!(report.blocks_changed, 0);
        assert_eq!(report.changes, Vec::new());
    }

    #[test]
    fn component_states_round_trip_through_the_registry_for_every_modelled_component() {
        let registry = registry();
        let table = EmitterTable::new(&registry);
        let components = [
            ComponentState::Lever(Lever::new(true)),
            ComponentState::Lever(Lever::OFF),
            ComponentState::Torch(RedstoneTorch::LIT),
            ComponentState::Torch(RedstoneTorch::new(false)),
            ComponentState::Repeater(Repeater {
                delay: 3,
                powered: true,
                locked: false,
            }),
            ComponentState::Repeater(Repeater {
                delay: 1,
                powered: false,
                locked: true,
            }),
            ComponentState::Comparator(Comparator {
                mode: ComparatorMode::Subtract,
                powered: true,
            }),
            ComponentState::Comparator(Comparator {
                mode: ComparatorMode::Compare,
                powered: false,
            }),
        ];
        for component in components {
            let id = table.component_state(component).expect("state resolves");
            // A modelled component's state must classify as its own source.
            assert_eq!(
                table.classify(id),
                BlockRole::Emitter {
                    source: component.source()
                },
                "{component:?}"
            );
            // And reading it back out of the registry must reproduce the same state.
            assert_eq!(
                table.component_of(id).expect("component reads back"),
                component,
                "{component:?} must round-trip through the registry"
            );
        }
        // A non-component is refused rather than approximated.
        let stone = registry.default_state("minecraft:stone").expect("stone");
        assert!(table.component_of(stone).is_err());
        assert!(table.component_of(registry.air_id()).is_err());
        // A button is recognised as a source but has no component type, so it is reported
        // as an error instead of being approximated by a nearby component.
        let button = registry
            .default_state("minecraft:stone_button")
            .expect("button");
        assert_eq!(
            table.classify(button),
            BlockRole::Emitter {
                source: PowerSource::Button
            }
        );
        assert!(table.component_of(button).is_err());
        assert_eq!(
            table.emitted(BlockRole::Passive, &BlockInputs::NONE),
            EmitterOutput::OFF
        );
    }
}
