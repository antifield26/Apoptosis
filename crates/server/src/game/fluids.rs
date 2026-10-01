//! The server's answer to `mc_simulation::fluid::FluidWorld` (P20-01).
//!
//! `mc-simulation` owns the flow rules and asks six questions of the world
//! through one trait; this module answers them from `mc_world::World` and
//! `mc_registry::BlockRegistry`, which is where block names and properties
//! actually live.
//!
//! ## The game-rule defaults, quoted from the jar
//!
//! `javap -p -c net.minecraft.world.level.gamerules.GameRules` (2026-10-01)
//! registers both flags through `registerBoolean(String, GameRuleCategory, Z)`:
//!
//! - `lava_source_conversion` — `GameRuleCategory.UPDATES`, `iconst_0`
//!   (**false**);
//! - `water_source_conversion` — `GameRuleCategory.UPDATES`, `iconst_1`
//!   (**true**).
//!
//! P20-01 has no game-rule storage yet (P20-05 owns `/gamerule` and the level
//! data), so [`world_fluid_rules`] returns the vanilla **defaults** and says so.
//! When P20-05 lands, its rule values feed these two arguments and nothing else
//! in the flow engine changes.
//!
//! ## Per-dimension lava
//!
//! `LavaFluid.getTickDelay` branches on `isFastLava(levelReader)`, which is
//! `environmentAttributes().getDimensionValue(EnvironmentAttributes.FAST_LAVA)`
//! — the Nether's lava runs at 10 ticks and the overworld's at 30. This server
//! has one dimension until P21-00a, so `fast = false` here is the overworld's
//! answer, not a hard-coded assumption about lava.

use mc_registry::BlockRegistry;
use mc_simulation::fluid::{
    BlockFacts, FluidKind, FluidRules, FluidState, FluidTickOutcome, FluidWorld, Pos,
};
use mc_world::World;

/// The water rules at the jar's default game rule values.
#[must_use]
pub const fn world_fluid_rules(kind: FluidKind) -> FluidRules {
    match kind {
        FluidKind::Water => FluidRules::water(true),
        // Overworld lava: `FAST_LAVA` is false outside the Nether.
        FluidKind::Lava => FluidRules::lava(false, false),
    }
}

/// Blocks `canHoldAnyFluid` refuses although they do not block motion.
///
/// Quoted from `javap -c FlowingFluid.canHoldAnyFluid`: after the
/// `LiquidBlockContainer` case and the `blocksMotion` refusal, the jar returns
/// false for `DoorBlock`, `BlockTags.SIGNS`, `Blocks.LADDER`,
/// `Blocks.SUGAR_CANE`, `Blocks.BUBBLE_COLUMN`, `Blocks.NETHER_PORTAL`,
/// `Blocks.END_PORTAL`, `Blocks.END_GATEWAY` and `Blocks.STRUCTURE_VOID`. The
/// door and sign families are matched by name suffix, which is how this
/// project's registry names them (`minecraft:oak_door`, `minecraft:oak_sign`,
/// `minecraft:oak_wall_sign`, `minecraft:oak_hanging_sign`).
const NOT_A_FLUID_HOLDER: &[&str] = &[
    "minecraft:ladder",
    "minecraft:sugar_cane",
    "minecraft:bubble_column",
    "minecraft:nether_portal",
    "minecraft:end_portal",
    "minecraft:end_gateway",
    "minecraft:structure_void",
];

/// `canHoldAnyFluid(state)` for a block whose name and cube-ness are known.
fn can_hold_any_fluid(name: &str, full_cube: bool) -> bool {
    if full_cube {
        return false;
    }
    if NOT_A_FLUID_HOLDER.contains(&name) {
        return false;
    }
    !(name.ends_with("_door") || name.ends_with("_sign"))
}

/// `BlockState.getFluidState()` for a block-state id.
///
/// Extracted so the entity-side fluid queries (drowning, lava damage, P20-01)
/// answer "what fluid is here?" through the same rule the flow engine uses,
/// instead of a second, subtly different reading of the `level` property.
#[must_use]
pub(crate) fn fluid_state_of(
    blocks: &BlockRegistry,
    id: i32,
    properties: &[(String, String)],
) -> FluidState {
    let Ok(name) = blocks.block_name(id) else {
        return FluidState::Empty;
    };
    let level = properties
        .iter()
        .find(|(key, _)| key == "level")
        .and_then(|(_, value)| value.parse::<u8>().ok());
    match name {
        "minecraft:water" => FluidState::from_block_level(FluidKind::Water, level.unwrap_or(0)),
        "minecraft:lava" => FluidState::from_block_level(FluidKind::Lava, level.unwrap_or(0)),
        // `SimpleWaterloggedBlock.getFluidState` is a full water source when the
        // property is set.
        _ if properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true") =>
        {
            FluidState::water(0)
        }
        _ => FluidState::Empty,
    }
}

/// The `FluidWorld` the tick loop hands to `mc_simulation::fluid::tick`.
pub(crate) struct WorldFluids<'a> {
    world: &'a mut World,
    blocks: &'a BlockRegistry,
}

impl<'a> WorldFluids<'a> {
    /// Borrow a world and the block registry for one phase body.
    pub(crate) fn new(world: &'a mut World, blocks: &'a BlockRegistry) -> Self {
        Self { world, blocks }
    }

    /// Resolve `name` plus properties to a state id, or `None`.
    fn state_id(&self, name: &str, properties: &[(String, String)]) -> Option<i32> {
        self.blocks.state_id(name, properties).ok()
    }

    /// The full property assignment of a state, or an empty list.
    fn properties(&self, id: i32) -> Vec<(String, String)> {
        self.blocks.properties_of(id).unwrap_or_default()
    }
}

impl FluidWorld for WorldFluids<'_> {
    fn facts(&self, pos: Pos) -> Option<BlockFacts> {
        // `None` is an unloaded chunk, which the engine must never write through.
        let id = self.world.get_block_loaded(pos.x, pos.y, pos.z)?;
        let Ok(name) = self.blocks.block_name(id) else {
            // An id outside the registry fails safe as a full cube: it blocks
            // flow rather than becoming a hole nothing can explain.
            return Some(BlockFacts::solid(id));
        };
        let properties = self.properties(id);
        let has_waterlogged = properties.iter().any(|(key, _)| key == "waterlogged");
        let waterlogged = properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true");
        let fluid = fluid_state_of(self.blocks, id, &properties);
        let full_cube = mc_world::is_full_cube(self.blocks, id);
        Some(BlockFacts {
            id,
            fluid,
            air: self.blocks.is_empty(id),
            full_cube,
            can_hold_any_fluid: has_waterlogged || can_hold_any_fluid(name, full_cube),
            water_container: has_waterlogged,
            waterlogged,
        })
    }

    fn place_fluid(&mut self, pos: Pos, fluid: FluidState) -> bool {
        let Some(id) = self.world.get_block_loaded(pos.x, pos.y, pos.z) else {
            return false;
        };
        let Ok(name) = self.blocks.block_name(id) else {
            return false;
        };
        let name = name.to_owned();
        let properties = self.properties(id);
        let waterlogged = properties.iter().any(|(key, _)| key == "waterlogged");
        let new_id = if fluid.is_empty() {
            if waterlogged {
                // `LiquidBlockContainer` removal clears the property and keeps
                // the block: draining a waterlogged stairs leaves a stairs.
                let mut rewritten = properties.clone();
                if let Some(entry) = rewritten.iter_mut().find(|(key, _)| key == "waterlogged") {
                    "false".clone_into(&mut entry.1);
                }
                match self.state_id(&name, &rewritten) {
                    Some(new_id) => new_id,
                    None => return false,
                }
            } else {
                match self.state_id("minecraft:air", &[]) {
                    Some(air) => air,
                    None => return false,
                }
            }
        } else if matches!(fluid.kind(), Some(FluidKind::Water))
            && waterlogged
            && !properties
                .iter()
                .any(|(key, value)| key == "waterlogged" && value == "true")
        {
            // `LiquidBlockContainer.placeLiquid`: set the property, keep the block.
            let mut rewritten = properties.clone();
            if let Some(entry) = rewritten.iter_mut().find(|(key, _)| key == "waterlogged") {
                "true".clone_into(&mut entry.1);
            }
            match self.state_id(&name, &rewritten) {
                Some(new_id) => new_id,
                None => return false,
            }
        } else {
            let Some(kind) = fluid.kind() else {
                return false;
            };
            // `FluidState.createLegacyBlock`: the fluid block at `getLegacyLevel`.
            let level = vec![("level".to_owned(), fluid.block_level().to_string())];
            match self.state_id(kind.block_name(), &level) {
                Some(new_id) => new_id,
                None => return false,
            }
        };
        if new_id == id {
            return false;
        }
        self.world.set_block(pos.x, pos.y, pos.z, new_id).is_ok()
    }

    fn set_block_named(&mut self, pos: Pos, name: &str) -> bool {
        let Some(new_id) = self.state_id(name, &[]) else {
            return false;
        };
        if self.world.get_block_loaded(pos.x, pos.y, pos.z) == Some(new_id) {
            return false;
        }
        self.world.set_block(pos.x, pos.y, pos.z, new_id).is_ok()
    }
}

/// The jar's `Level.setBlock(pos, state, 3)` post-write work, for a cell a path
/// *outside* the flow engine just wrote (P20-01).
///
/// The engine's own writes already do this inside `mc_simulation::fluid::tick`;
/// a player's bucket, a `/setblock` or a structure goes through
/// `Game::block_feed` instead, and until this was called from there the two paths
/// disagreed: water a player placed beside a lava source did not convert it until
/// the lava's own scheduled tick came due — thirty ticks for overworld lava, by
/// which time the jar has long since written obsidian (`LiquidBlock.onPlace` and
/// the neighbours' `neighborChanged`, both quoted on
/// [`mc_simulation::fluid::notify_neighbors`]).
///
/// The two halves are the engine's, in the jar's order, and neither is
/// re-implemented here:
///
/// 1. [`mc_simulation::fluid::on_place`] on the written cell — `LiquidBlock.onPlace`
///    runs `shouldSpreadLiquid`, so lava *placed* beside water converts itself
///    (obsidian as a source, cobblestone as a flowing cell) and is not scheduled.
/// 2. [`mc_simulation::fluid::notify_neighbors`] — `updateNeighborsAt` over the
///    six faces in `NeighborUpdater.UPDATE_ORDER`, so a lava cell the write
///    reached converts in this tick.
///
/// It runs for every write, not only a fluid one, because that is what
/// `setBlock` does: `updateNeighborsAt` is unconditional, and a neighbour only
/// reacts when it is itself a fluid block. One level deep, as in the engine
/// (module docs on the chain the port deliberately stops at), and an unloaded
/// chunk is never read *or* written through.
pub(crate) fn notify_block_written(world: &mut World, blocks: &BlockRegistry, pos: Pos) {
    let mut outcome = FluidTickOutcome::default();
    let mut adapter = WorldFluids::new(world, blocks);
    mc_simulation::fluid::on_place(&mut adapter, pos, &mut outcome);
    mc_simulation::fluid::notify_neighbors(&mut adapter, pos, &mut outcome);
}

#[cfg(test)]
mod tests {
    use super::can_hold_any_fluid;

    #[test]
    fn a_full_cube_never_holds_a_fluid_and_a_door_never_does_either() {
        // The two refusals before the name list in `canHoldAnyFluid`, plus the
        // name list itself, at the level this helper decides it.
        assert!(!can_hold_any_fluid("minecraft:stone", true));
        // The helper answers the *name* half only; the caller ORs the
        // waterloggable case in before asking it.
        assert!(
            can_hold_any_fluid("minecraft:stone", false),
            "a non-cube stone-named block is not excluded by name"
        );
        assert!(!can_hold_any_fluid("minecraft:oak_door", false));
        assert!(!can_hold_any_fluid("minecraft:oak_sign", false));
        assert!(!can_hold_any_fluid("minecraft:oak_wall_sign", false));
        assert!(!can_hold_any_fluid("minecraft:oak_hanging_sign", false));
        assert!(!can_hold_any_fluid("minecraft:ladder", false));
        assert!(!can_hold_any_fluid("minecraft:bubble_column", false));
        assert!(can_hold_any_fluid("minecraft:air", false));
        assert!(can_hold_any_fluid("minecraft:water", false));
        assert!(can_hold_any_fluid("minecraft:oak_stairs", false));
    }
}
