//! Shared fixtures for the `mc-redstone` integration tests.
//!
//! Integration tests cannot see `#[cfg(test)]` items, so the harness lives here. It is
//! deliberately small: load the real registry, build a [`FlatWorld`], and offer a few
//! helpers that turn block-state ids into the numbers a hand-written expectation is
//! written in.
//!
//! ## Why a flat world and not `mc_world::World`
//!
//! `BlockView` is implemented for both, and `tests/world_integration.rs` exercises the
//! `mc-world` implementation. The scenarios here are about the *rule*, so they use a flat
//! array where the whole circuit is visible in one screen — which is what makes the golden
//! numbers checkable by hand.

#![allow(dead_code)]

use mc_redstone::propagation::{BlockRole, EmitterTable, FlatWorld};
use mc_redstone::{
    BlockPos, ComponentState, EmitterOutput, MAX_POWER, PowerLevel, PowerState, UpdateBudget,
    UpdateQueue,
};
use mc_registry::{BlockRegistry, Registries};

/// Load the registry table dumped from the official 26.1.2 server jar.
pub fn registry() -> BlockRegistry {
    Registries::vanilla()
        .expect("the registry fixtures load")
        .blocks
}

/// An emitter table over `registry`.
pub fn table(registry: &BlockRegistry) -> EmitterTable<'_> {
    EmitterTable::new(registry)
}

/// A power level in `0..=15`, panicking on anything else (a test bug, not a scenario).
pub fn level(value: u8) -> PowerLevel {
    PowerLevel::new(value).expect("a test power level is in 0..=15")
}

/// The state id of a named block with no properties.
pub fn block(registry: &BlockRegistry, name: &str) -> i32 {
    registry
        .default_state(name)
        .unwrap_or_else(|error| panic!("{name} must be in the registry: {error}"))
}

/// The state id of a modelled component state.
pub fn component(registry: &BlockRegistry, state: ComponentState) -> i32 {
    table(registry)
        .component_state(state)
        .unwrap_or_else(|error| panic!("{state:?} must resolve to a block state: {error}"))
}

/// The state id of a lever with an explicit mount: `face` is
/// `floor`|`wall`|`ceiling`, `facing` is `north`|`south`|`west`|`east`.
///
/// P13-06 measures the mount, not the lever in the abstract — a floor lever
/// powers the block below it, a wall lever the block behind it — so
/// conductivity tests always build the lever this way rather than through
/// [`component`], which keeps the registry's first `face`/`facing`.
pub fn lever(registry: &BlockRegistry, face: &str, facing: &str, powered: bool) -> i32 {
    registry
        .state_id(
            "minecraft:lever",
            &[
                ("face".to_owned(), face.to_owned()),
                ("facing".to_owned(), facing.to_owned()),
                ("powered".to_owned(), powered.to_string()),
            ],
        )
        .unwrap_or_else(|error| panic!("lever {face}/{facing}/{powered} must resolve: {error}"))
}

/// The state id of a lit (or unlit) standing torch.
pub fn torch(registry: &BlockRegistry, lit: bool) -> i32 {
    registry
        .state_id(
            "minecraft:redstone_torch",
            &[("lit".to_owned(), lit.to_string())],
        )
        .unwrap_or_else(|error| panic!("torch lit={lit} must resolve: {error}"))
}

/// The state id of a wall torch facing `facing`, lit or not.
pub fn wall_torch(registry: &BlockRegistry, facing: &str, lit: bool) -> i32 {
    registry
        .state_id(
            "minecraft:redstone_wall_torch",
            &[
                ("facing".to_owned(), facing.to_owned()),
                ("lit".to_owned(), lit.to_string()),
            ],
        )
        .unwrap_or_else(|error| panic!("wall torch {facing}/{lit} must resolve: {error}"))
}

/// The state id of a redstone wire at `power`.
pub fn wire(registry: &BlockRegistry, power: PowerLevel) -> i32 {
    table(registry)
        .wire_state(power)
        .unwrap_or_else(|error| panic!("redstone wire at {power} must resolve: {error}"))
}

/// The power a wire state carries, or `None` when the state is not a wire.
pub fn wire_power_at(world: &FlatWorld, registry: &BlockRegistry, pos: BlockPos) -> Option<u8> {
    let id = world.get(pos)?;
    // The block must actually be wire; a plain "read the power property" would answer for
    // any block that happens to have one.
    match table(registry).classify(id) {
        BlockRole::Wire { stored } => Some(stored.get()),
        BlockRole::Emitter { .. } | BlockRole::Passive | BlockRole::Mechanism => None,
    }
}

/// The emitted output of the block at `pos`, with its neighbours' inputs gathered for real.
///
/// This goes through the library's own `read_block`, so a test reading an emission here is reading
/// the model's answer and not a second implementation of it. Gathering the inputs matters: a
/// comparator's emission depends on what it receives, so reading its emission against an empty input
/// set would report 0 for a comparator that is in fact passing a signal through.
pub fn emitted_at(world: &FlatWorld, registry: &BlockRegistry, pos: BlockPos) -> EmitterOutput {
    match mc_redstone::propagation::read_block(world, table(registry), pos) {
        Some((_, _, output)) => output,
        None => EmitterOutput::OFF,
    }
}

/// Whether the block at `pos` is a redstone torch.
pub fn is_torch(world: &FlatWorld, registry: &BlockRegistry, pos: BlockPos) -> bool {
    world.get(pos).is_some_and(|id| {
        registry.block_name(id).is_ok_and(|name| {
            name.starts_with("minecraft:redstone_torch") || name == "minecraft:redstone_wall_torch"
        })
    })
}

/// A `(32, 4, 8)` box of air at the origin: wide enough for a 15-block wire run plus a
/// source and a sink, four blocks tall for vertical scenarios, eight deep for fans.
pub fn flat() -> FlatWorld {
    FlatWorld::boxed((32, 4, 8))
}

/// A queue and budget at the nominal values.
pub fn fresh_queue() -> (UpdateQueue, UpdateBudget) {
    (UpdateQueue::new(), UpdateBudget::nominal())
}

/// Assert that a wire in `world` carries exactly `expected` at each listed position.
///
/// The message names the position, because a hand-written golden table is only useful if a
/// failure says *which* number was wrong.
pub fn assert_wire_powers(
    world: &FlatWorld,
    registry: &BlockRegistry,
    expected: &[(BlockPos, u8)],
) {
    for (pos, want) in expected {
        let got = wire_power_at(world, registry, *pos);
        assert_eq!(
            got,
            Some(*want),
            "wire at {pos} should carry {want}, got {got:?}"
        );
    }
}

/// The strongest power a wire carries anywhere in `positions`.
pub fn peak_wire_power(
    world: &FlatWorld,
    registry: &BlockRegistry,
    positions: &[BlockPos],
) -> PowerLevel {
    let mut best = PowerLevel::ZERO;
    for pos in positions {
        if let Some(power) = wire_power_at(world, registry, *pos)
            && power > best.get()
        {
            best = level(power);
        }
    }
    best
}

/// A drain/propagation scratch value: the maximum power a wire may carry, re-exported so a
/// test can assert the ceiling without importing `mc_redstone` twice.
pub const CEILING: u8 = MAX_POWER;

/// A `PowerState` that is on weakly, for building inputs in tests.
pub fn on() -> PowerState {
    PowerState::weak_only(PowerLevel::MAX)
}
