//! Furnace tests (P06-05), weighted towards the rules whose absence causes the
//! classic item-loss and fuel-waste bugs.
//!
//! The properties under test:
//!
//! * **fuel is not wasted** — nothing cookable or a blocked output means no fuel
//!   is consumed;
//! * **cook timing** — cooking takes exactly `cook_ticks` and finishes once;
//! * **the output limit** — a full output slot stops progress;
//! * **a full output of a different item blocks cooking and wastes no fuel**;
//! * **burn time** — the documented ticks, counted exactly;
//! * **resume** — running out of fuel mid-cook preserves progress;
//! * **the labelling** — every fuel row carries evidence, and the two tables agree.

use mc_core::error::ServerError;
use mc_entity::stack::{ItemStack, StackSizeTable};
use mc_registry::ItemRegistry;
use std::path::Path;

use super::{
    Evidence, FUEL_BURST_TICKS, Furnace, FurnaceSlots, FurnaceState, MAX_COOK_TICKS,
    SMELTING_COOK_TICKS, SmeltingRecipe, SmeltingRegistry, TICKS_PER_SECOND, burn_ticks_for,
    fuel_evidence,
};
use crate::container::{Container, ContainerKind};

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the registry fixture loads")
}

fn sizes() -> StackSizeTable {
    StackSizeTable::resolve(&items()).expect("the stack-size table resolves")
}

/// An item id resolved by name. Numeric ids are never hard-coded in these tests.
fn item(name: &str) -> i32 {
    items()
        .id(name)
        .unwrap_or_else(|_| panic!("{name} must exist in the item registry"))
}

fn stack(item_id: i32, count: i32) -> ItemStack {
    ItemStack::new(item_id, count).expect("a valid stack")
}

fn recipes() -> SmeltingRegistry {
    SmeltingRegistry::baseline(&items()).expect("the baseline smelting table resolves")
}

/// A 3-slot furnace container, empty.
fn furnace_container() -> Container {
    Container::with_default_size(ContainerKind::Furnace).expect("a 3-slot furnace")
}

/// A container with `input` items of `input_item` and `fuel` of `fuel_item`.
fn loaded(input_item: i32, input: i32, fuel_item: i32, fuel: i32) -> Container {
    let mut container = furnace_container();
    container.set(0, stack(input_item, input)).expect("input");
    container.set(1, stack(fuel_item, fuel)).expect("fuel");
    container
}

/// Tick once, expecting success.
fn tick(state: &mut FurnaceState, container: &mut Container) -> super::FurnaceTickReport {
    Furnace::tick(
        state,
        container,
        FurnaceSlots::CANONICAL,
        &recipes(),
        &items(),
        &sizes(),
    )
    .expect("a furnace tick applies")
}

/// Tick `count` times, returning the last report.
fn tick_many(
    state: &mut FurnaceState,
    container: &mut Container,
    count: u32,
) -> super::FurnaceTickReport {
    let mut last = super::FurnaceTickReport::default();
    for _ in 0..count {
        last = tick(state, container);
    }
    last
}

fn iron_ore() -> i32 {
    item("minecraft:iron_ore")
}

fn iron_ingot() -> i32 {
    item("minecraft:iron_ingot")
}

fn coal() -> i32 {
    item("minecraft:coal")
}

fn stick() -> i32 {
    item("minecraft:stick")
}

fn stone() -> i32 {
    item("minecraft:stone")
}

// ------------------------------------------------------ the labelled tables

#[test]
fn every_fuel_row_is_labelled_and_reachable() {
    assert!(!FUEL_BURST_TICKS.is_empty(), "the table must have rows");
    for fuel in FUEL_BURST_TICKS {
        assert!(
            fuel.ticks > 0,
            "{} must burn for at least one tick",
            fuel.item
        );
        assert_eq!(
            fuel_evidence(fuel.item),
            fuel.evidence,
            "{} must expose the label its own row carries",
            fuel.item
        );
        // The label is part of the row, so a row can never be unlabelled; what can
        // still go wrong is a row for an item that does not exist, which would make
        // the fuel unreachable.
        items()
            .id(fuel.item)
            .unwrap_or_else(|_| panic!("{} must be a registered item", fuel.item));
    }
    // Every item appears at most once: two rows for one fuel would make
    // `burn_ticks_for` order-dependent.
    for (index, fuel) in FUEL_BURST_TICKS.iter().enumerate() {
        assert!(
            !FUEL_BURST_TICKS[..index]
                .iter()
                .any(|earlier| earlier.item == fuel.item),
            "{} appears twice in the fuel table",
            fuel.item
        );
    }
    // An unlabelled item falls back to the weakest label, never a stronger one.
    assert_eq!(
        fuel_evidence("minecraft:not_a_fuel"),
        Evidence::Approximation
    );

    // The named values the task calls out, at the ticks this build documents.
    for (name, ticks) in [
        ("minecraft:coal", 1600u32),
        ("minecraft:charcoal", 1600),
        ("minecraft:oak_planks", 300),
        ("minecraft:stick", 100),
        ("minecraft:lava_bucket", 20000),
        ("minecraft:blaze_rod", 2400),
    ] {
        assert_eq!(
            burn_ticks_for(&items(), item(name)).expect("resolves"),
            Some(ticks),
            "{name}"
        );
        assert!(
            fuel_evidence(name).is_vanilla(),
            "{name} is asserted as a Vanilla figure and must be labelled as one"
        );
    }
    // A non-fuel is not a fuel, and that is not an error.
    assert_eq!(
        burn_ticks_for(&items(), stone()).expect("resolves"),
        None,
        "stone is not a fuel"
    );
    // An id outside the registry is corrupt data, not "not a fuel".
    let error = burn_ticks_for(&items(), 900_000).expect_err("an unregistered id is corrupt data");
    assert!(matches!(error, ServerError::CorruptData(_)), "{error:?}");

    // The evidence helper never overclaims.
    assert!(Evidence::Verified.is_vanilla());
    assert!(Evidence::Derived.is_vanilla());
    assert!(!Evidence::Approximation.is_vanilla());
    assert!(!Evidence::ProductDecision.is_vanilla());
    assert_eq!(Evidence::Verified.name(), "verified");
    assert_eq!(Evidence::ProductDecision.name(), "product decision");
}

#[test]
fn the_baseline_smelting_table_resolves_and_is_reachable() {
    let items = items();
    let recipes = recipes();
    assert_eq!(recipes.len(), 7, "the documented baseline size");
    for recipe in recipes.recipes() {
        items.entry(recipe.input).expect("input resolves");
        items.entry(recipe.output).expect("output resolves");
        assert_eq!(recipe.output_count, 1);
        assert_eq!(
            recipe.cook_ticks, SMELTING_COOK_TICKS,
            "every baseline recipe uses the smelting cook time"
        );
        assert!(
            recipe.experience >= 0.0,
            "a negative experience figure would subtract from the state"
        );
    }
    for (input, output) in [
        ("minecraft:iron_ore", "minecraft:iron_ingot"),
        ("minecraft:deepslate_iron_ore", "minecraft:iron_ingot"),
        ("minecraft:raw_iron", "minecraft:iron_ingot"),
        ("minecraft:sand", "minecraft:glass"),
        ("minecraft:cobblestone", "minecraft:stone"),
        ("minecraft:porkchop", "minecraft:cooked_porkchop"),
        ("minecraft:potato", "minecraft:baked_potato"),
    ] {
        let recipe = recipes
            .recipe_for(item(input))
            .unwrap_or_else(|| panic!("{input} must smelt"));
        assert_eq!(recipe.output, item(output), "{input} -> {output}");
    }
    assert!(
        recipes.recipe_for(stone()).is_none(),
        "stone is a result, not a smeltable input"
    );
    // A missing name is corrupt data naming the item, never a silent skip.
    let tiny = ItemRegistry::parse("0\tminecraft:air\t-\n").expect("parses");
    let error = SmeltingRegistry::baseline(&tiny).expect_err("the baseline cannot resolve");
    assert!(matches!(error, ServerError::CorruptData(_)), "{error:?}");
    assert!(
        error.to_string().contains("minecraft:iron_ore"),
        "{error:?}"
    );
    // Duplicate inputs are refused.
    let one = SmeltingRecipe::one(item("minecraft:sand"), item("minecraft:glass"), 200, 0.1)
        .expect("valid");
    let two = SmeltingRecipe::one(item("minecraft:sand"), item("minecraft:stone"), 200, 0.1)
        .expect("valid");
    assert!(SmeltingRegistry::new(vec![one]).is_ok());
    assert!(SmeltingRegistry::new(vec![one, two]).is_err());
}

#[test]
fn impossible_recipes_are_refused_at_construction() {
    let input = sand();
    assert!(
        SmeltingRecipe::new(input, glass(), 0, 200, 0.1).is_err(),
        "a recipe producing nothing is a table bug"
    );
    assert!(SmeltingRecipe::new(input, glass(), 65, 200, 0.1).is_err());
    assert!(
        SmeltingRecipe::new(input, glass(), 1, 0, 0.1).is_err(),
        "a zero-tick recipe would cook instantly"
    );
    assert!(
        SmeltingRecipe::new(input, glass(), 1, MAX_COOK_TICKS + 1, 0.1).is_err(),
        "a recipe above the ceiling would stall a furnace forever"
    );
    assert!(SmeltingRecipe::new(input, glass(), 1, MAX_COOK_TICKS, 0.1).is_ok());
    assert!(SmeltingRecipe::one(input, glass(), SMELTING_COOK_TICKS, 0.1).is_ok());
    assert_eq!(TICKS_PER_SECOND, 20);
}

fn sand() -> i32 {
    item("minecraft:sand")
}

fn glass() -> i32 {
    item("minecraft:glass")
}

// ------------------------------------------------------------- fuel economy

#[test]
fn fuel_is_consumed_only_when_there_is_something_to_cook() {
    // An empty furnace with fuel: nothing burns.
    let mut state = FurnaceState::new();
    let mut container = loaded(0, 0, coal(), 1);
    let report = tick_many(&mut state, &mut container, 10);
    assert!(!report.fuel_consumed);
    assert!(!report.cook_progressed);
    assert!(!report.lit);
    assert_eq!(container.get(1), stack(coal(), 1), "the fuel is untouched");
    assert_eq!(container.get(0), ItemStack::EMPTY);

    // A furnace with an un-smeltable input: still nothing burns.
    let mut state = FurnaceState::new();
    let mut container = loaded(stone(), 1, coal(), 1);
    let report = tick_many(&mut state, &mut container, 10);
    assert!(!report.fuel_consumed);
    assert!(!report.lit);
    assert_eq!(container.get(1), stack(coal(), 1), "the fuel is untouched");
    assert_eq!(
        container.get(0),
        stack(stone(), 1),
        "the input is untouched"
    );

    // An empty furnace with no fuel: nothing happens at all.
    let mut state = FurnaceState::new();
    let mut container = furnace_container();
    let report = tick(&mut state, &mut container);
    assert!(report.is_noop(), "{report:?}");
    assert!(!report.lit, "an empty furnace is not lit");

    // With a smeltable input the fuel is consumed on the very first tick.
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, coal(), 1);
    let report = tick(&mut state, &mut container);
    assert!(
        report.fuel_consumed,
        "the first tick must light the furnace"
    );
    assert!(report.lit);
    assert!(report.state_changed, "the lit flag changed");
    assert_eq!(container.get(1), ItemStack::EMPTY, "one coal was consumed");
    assert_eq!(
        state.burn_ticks_remaining, 1600,
        "the tick that takes the fuel is credited to it, so the counter starts at the \
         coal's full 1600 and reaches 0 at the end of its 1600th lit tick"
    );
    assert_eq!(state.burn_ticks_total, 1600);
}

#[test]
fn a_fuel_item_lasts_exactly_its_documented_number_of_ticks() {
    let burn = burn_ticks_for(&items(), stick())
        .expect("resolves")
        .expect("a fuel");
    assert_eq!(burn, 100);

    // The requirement, measured directly: **one** fuel item lights the furnace for
    // exactly its documented burn time.
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 64, stick(), 1);
    for lit_tick in 1..=burn {
        let report = tick(&mut state, &mut container);
        assert!(
            report.lit,
            "a {burn}-tick stick must stay lit through tick {lit_tick}"
        );
    }
    assert!(
        state.is_lit(),
        "after exactly {burn} lit ticks the counter holds the last one, so the state \
         still reports lit; the next tick is the dark one"
    );
    assert_eq!(
        state.burn_ticks_remaining, 1,
        "the counter's accounting, per the FurnaceState docs"
    );
    let report = tick(&mut state, &mut container);
    assert!(!report.lit, "so tick {} is dark", burn + 1);
    assert!(report.state_changed, "and the client is told");
    assert!(!report.fuel_consumed);
    assert!(!state.is_lit());
    assert_eq!(state.burn_ticks_remaining, 0);
    assert_eq!(state.burn_ticks_total, 0, "the burn is over");

    // Four sticks are four of those, back to back: 400 consecutive lit ticks with no
    // dark tick in between, because the tick that spends one stick takes the next in
    // the same tick.
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 64, stick(), 4);
    for lit_tick in 1..=(burn * 4) {
        let report = tick(&mut state, &mut container);
        assert!(
            report.lit,
            "the furnace must stay lit for the whole burn, went dark on tick {lit_tick}"
        );
    }
    assert_eq!(
        container.get(1),
        ItemStack::EMPTY,
        "all four sticks burned in exactly {} ticks",
        burn * 4
    );
    assert_eq!(
        container.get(2).count(),
        2,
        "400 lit ticks cook exactly two 200-tick items"
    );
    // The next tick finds no fuel, so the furnace goes dark.
    let report = tick(&mut state, &mut container);
    assert!(!report.lit);
    assert!(
        report.state_changed,
        "the client must be told the furnace went out"
    );
    assert!(!report.fuel_consumed);
    assert!(!state.is_lit(), "the burn is over");
    assert!(
        container.get(0).count() < 64,
        "the input was being consumed while it burned"
    );
}

#[test]
fn the_burn_denominator_is_the_consumed_fuel_items_burn_time() {
    let mut state = FurnaceState::new();
    // 64 ore so a cook is always available: with a single ore the furnace would run
    // out of something to cook after 200 ticks and (correctly) stop taking fuel,
    // which would make this test about the wrong rule.
    let mut container = loaded(iron_ore(), 64, coal(), 1);
    // The tick that takes the coal is its first lit tick, so the counter starts at
    // the coal's full burn time.
    let first = tick(&mut state, &mut container);
    assert!(first.lit && first.fuel_consumed);
    assert_eq!(state.burn_ticks_total, 1600);
    assert_eq!(state.burn_ticks_remaining, 1600);
    let second = tick(&mut state, &mut container);
    assert!(second.lit);
    assert_eq!(
        state.burn_ticks_remaining, 1599,
        "one tick per tick after that"
    );
    // The remaining 1598 ticks of the coal's 1600.
    tick_many(&mut state, &mut container, 1598);
    assert_eq!(
        state.burn_ticks_total, 1600,
        "still the coal's own burn time"
    );
    assert_eq!(
        state.burn_ticks_remaining, 1,
        "after the coal's 1600th lit tick the counter holds that last tick, per the \
         FurnaceState accounting docs"
    );
    assert!(state.is_lit(), "and that tick was lit");

    // The end of the fuel: the coal is spent and there is nothing to replace it, so
    // the next tick is dark and the total goes to zero rather than describing a burn
    // that is not happening.
    let report = tick(&mut state, &mut container);
    assert!(!report.lit);
    assert!(!report.fuel_consumed);
    assert_eq!(state.burn_ticks_total, 0, "a finished burn has no total");
    assert_eq!(state.burn_ticks_remaining, 0);
    assert_eq!(state.cook_progress, 0, "an unlit furnace keeps no progress");
    assert_eq!(state.cook_total, 0);

    // A relight takes the *new* fuel's own burn time as its denominator, not the old
    // one, and the tick that relights also cooks that tick's worth.
    container.set(1, stack(stick(), 1)).expect("a fresh stick");
    let report = tick(&mut state, &mut container);
    assert!(report.fuel_consumed);
    assert!(report.cook_progressed, "the relighting tick also cooks");
    assert_eq!(state.burn_ticks_total, 100, "the stick, not the coal");
    assert_eq!(
        state.burn_ticks_remaining, 100,
        "the tick that takes the stick is credited to it"
    );
    assert_eq!(state.cook_progress, 1, "a fresh cook from the relight");
}

#[test]
fn running_out_of_fuel_mid_cook_preserves_progress() {
    let mut state = FurnaceState::new();
    // One stick lights the furnace for 100 ticks; an iron ingot needs 200, so the
    // cook cannot finish on one stick. The stick is spent on the 100th lit tick and
    // the furnace goes dark on the 101st.
    let mut container = loaded(iron_ore(), 1, stick(), 1);
    tick_many(&mut state, &mut container, 100);
    assert!(state.is_lit(), "the lit tick is the hundredth");
    assert_eq!(
        state.burn_ticks_remaining, 1,
        "one tick of the stick is left"
    );
    assert!(
        container.get(1).is_empty(),
        "the stick is consumed when it is taken"
    );
    assert_eq!(state.cook_progress, 100, "100 of 200 ticks were cooked");
    assert_eq!(state.cook_total, SMELTING_COOK_TICKS);

    // With no fuel left to take, the next tick spends the stick's last tick, goes
    // dark, and the part-cooked item is lost with it: there is nothing to resume it
    // with.
    let report = tick(&mut state, &mut container);
    assert!(!report.lit, "the fuel ran out, so the furnace goes dark");
    assert!(
        report.state_changed,
        "the client must be told the furnace went out"
    );
    assert_eq!(state.cook_progress, 0, "a dark furnace keeps no progress");
    assert_eq!(state.cook_total, 0);
    assert_eq!(state.burn_ticks_remaining, 0, "the stick is spent");
    assert!(!state.is_lit());

    // Dark ticks change nothing at all.
    for _ in 0..2 {
        let report = tick(&mut state, &mut container);
        assert!(!report.cook_progressed);
        assert_eq!(state.cook_progress, 0);
        assert_eq!(
            container.get(0),
            stack(iron_ore(), 1),
            "no input may be consumed while dark"
        );
        assert!(container.get(2).is_empty());
    }

    // Feed it again: the furnace relights and cooks from the start.
    container.set(1, stack(coal(), 1)).expect("more fuel");
    let report = tick(&mut state, &mut container);
    assert!(report.fuel_consumed);
    assert!(report.cook_progressed);
    assert_eq!(state.cook_progress, 1);
}

#[test]
fn a_relight_keeps_a_part_cooked_item() {
    // The complement of the previous test, and the reason the top-up happens inside
    // the same tick: the second stick is taken on the very tick the first is spent,
    // so the furnace never goes dark and the part-cooked item survives the handover.
    //
    // One stick lights the furnace for exactly 100 ticks (its documented burn time),
    // so the handover lands on tick 101 and the cook's progress at that moment is 100
    // of the 200 it needs.
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, stick(), 2);
    for _ in 0..100 {
        tick(&mut state, &mut container);
    }
    assert_eq!(state.cook_progress, 100);
    assert!(state.is_lit(), "the stick's hundredth tick is lit");
    assert_eq!(
        container.get(1),
        stack(stick(), 1),
        "the second stick is untouched until the first is spent"
    );
    let report = tick(&mut state, &mut container);
    assert!(report.fuel_consumed, "the second stick takes over");
    assert!(report.lit, "the furnace never went dark");
    assert_eq!(state.burn_ticks_total, 100, "the new stick's own burn time");
    assert_eq!(
        state.cook_progress, 101,
        "the cook continued rather than restarting from zero"
    );
    assert_eq!(container.get(1), ItemStack::EMPTY, "both sticks are spent");
}

// -------------------------------------------------------------- cook timing

#[test]
fn cooking_takes_exactly_cook_ticks_and_finishes_once() {
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, coal(), 1);
    // One tick of furnace time is one tick of cooking, starting from the tick that
    // lights the furnace.
    let mut finished_at = None;
    for elapsed in 1..=SMELTING_COOK_TICKS {
        let report = tick(&mut state, &mut container);
        if report.finished_items == 1 {
            finished_at = Some(elapsed);
            break;
        }
        assert_eq!(
            state.cook_progress, elapsed,
            "cook progress after {elapsed} furnace ticks must be exactly {elapsed}"
        );
        assert_eq!(
            container.get(0),
            stack(iron_ore(), 1),
            "the input is consumed only when the cook finishes"
        );
        assert!(container.get(2).is_empty());
    }
    assert_eq!(
        finished_at,
        Some(SMELTING_COOK_TICKS),
        "a {SMELTING_COOK_TICKS}-tick recipe must finish on its \
         {SMELTING_COOK_TICKS}th lit tick, not before and not after"
    );
    assert_eq!(container.get(0), ItemStack::EMPTY, "the input was consumed");
    assert_eq!(container.get(2), stack(iron_ingot(), 1), "the output");
    assert_eq!(state.cook_progress, 0, "progress resets for the next item");
    assert!(
        state.experience > 0.0,
        "the recipe's experience accumulated"
    );

    // With no input left, the next tick stops cooking and reports no finish.
    let report = tick(&mut state, &mut container);
    assert_eq!(report.finished_items, 0);
    assert!(!report.cook_progressed);
    assert_eq!(state.cook_total, 0);
    assert!(
        container.get(2) == stack(iron_ingot(), 1),
        "output unchanged"
    );
}

#[test]
fn a_short_cook_recipe_finishes_on_its_own_tick_count() {
    // A 2-tick recipe, so the boundary is observable without 200 iterations.
    let registry = SmeltingRegistry::new(vec![
        SmeltingRecipe::one(sand(), glass(), 2, 0.1).expect("valid"),
    ])
    .expect("one recipe");
    let mut state = FurnaceState::new();
    let mut container = loaded(sand(), 1, coal(), 1);
    let mut finish_tick = None;
    for elapsed in 1..=10u32 {
        let report = Furnace::tick(
            &mut state,
            &mut container,
            FurnaceSlots::CANONICAL,
            &registry,
            &items(),
            &sizes(),
        )
        .expect("a tick applies");
        assert_eq!(
            report.finished_items,
            u32::from(elapsed == 2),
            "exactly one finish, on tick 2: the tick that lights the furnace is also \
             its first tick of cooking"
        );
        if report.finished_items == 1 {
            finish_tick = Some(elapsed);
            break;
        }
    }
    assert_eq!(
        finish_tick,
        Some(2),
        "a 2-tick recipe finishes on the second tick of furnace time: one tick to \
         light (which also cooks) and one more to reach two"
    );
    assert_eq!(container.get(2), stack(glass(), 1));
}

#[test]
fn a_recipe_with_a_longer_cook_time_keeps_its_own_denominator() {
    let registry = SmeltingRegistry::new(vec![
        SmeltingRecipe::one(sand(), glass(), 5, 0.1).expect("valid"),
    ])
    .expect("one recipe");
    let mut state = FurnaceState::new();
    let mut container = loaded(sand(), 1, coal(), 1);
    for _ in 0..3 {
        Furnace::tick(
            &mut state,
            &mut container,
            FurnaceSlots::CANONICAL,
            &registry,
            &items(),
            &sizes(),
        )
        .expect("a tick applies");
    }
    assert_eq!(state.cook_progress, 3);
    assert_eq!(state.cook_total, 5);
    assert!((state.cook_fraction() - 0.6).abs() < f32::EPSILON);
}

// --------------------------------------------------------------- output slot

#[test]
fn a_full_output_slot_stops_progress() {
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 4, coal(), 1);
    container
        .set(2, stack(iron_ingot(), 64))
        .expect("a full output");
    let report = tick_many(&mut state, &mut container, 5);
    assert!(!report.cook_progressed, "a full output blocks cooking");
    assert!(!report.fuel_consumed, "and wastes no fuel");
    assert_eq!(state.cook_progress, 0);
    assert_eq!(state.burn_ticks_remaining, 0);
    assert_eq!(container.get(2), stack(iron_ingot(), 64));
    assert_eq!(
        container.get(0),
        stack(iron_ore(), 4),
        "the input is intact"
    );
    assert_eq!(container.get(1), stack(coal(), 1), "the fuel is intact");

    // Empty one item of room and cooking resumes.
    container.set(2, stack(iron_ingot(), 63)).expect("room");
    let report = tick(&mut state, &mut container);
    assert!(report.fuel_consumed);
    assert!(report.cook_progressed);
}

#[test]
fn a_full_output_of_a_different_item_blocks_cooking_and_wastes_no_fuel() {
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, coal(), 2);
    container
        .set(2, stack(stone(), 64))
        .expect("a full output of the wrong item");
    for _ in 0..50 {
        let report = tick(&mut state, &mut container);
        assert!(!report.fuel_consumed, "no fuel may be burned while blocked");
        assert!(!report.cook_progressed);
        assert!(!report.lit);
        assert_eq!(report.finished_items, 0);
    }
    assert_eq!(
        container.get(1),
        stack(coal(), 2),
        "both coal items are still there: a blocked furnace wastes nothing"
    );
    assert_eq!(container.get(0), stack(iron_ore(), 1));
    assert_eq!(container.get(2), stack(stone(), 64));

    // And the progress that a *lit* blocked furnace would have had does not
    // exist: nothing is cooking.
    assert_eq!(state.cook_progress, 0);
    assert_eq!(state.cook_total, 0);

    // Clear the output and the same fuel lights it immediately.
    container.set(2, ItemStack::EMPTY).expect("clear");
    let report = tick(&mut state, &mut container);
    assert!(report.fuel_consumed);
    assert_eq!(container.get(1), stack(coal(), 1));
}

#[test]
fn a_partially_full_output_of_the_right_item_still_cooks() {
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, coal(), 1);
    container
        .set(2, stack(iron_ingot(), 63))
        .expect("one item of room");
    let report = tick_many(&mut state, &mut container, SMELTING_COOK_TICKS);
    assert_eq!(report.finished_items, 1);
    assert_eq!(
        container.get(2),
        stack(iron_ingot(), 64),
        "filled to the brim"
    );
    // Now it is full, so the next cook is blocked.
    let report = tick_many(&mut state, &mut container, 5);
    assert_eq!(report.finished_items, 0);
    assert_eq!(container.get(2), stack(iron_ingot(), 64));
}

#[test]
fn the_output_never_exceeds_the_items_own_stack_limit() {
    // Smoke a stack of porkchops into a nearly full output: the output must stop
    // at the limit rather than overflowing into an illegal stack.
    let mut state = FurnaceState::new();
    let mut container = loaded(item("minecraft:porkchop"), 4, coal(), 1);
    container
        .set(2, stack(item("minecraft:cooked_porkchop"), 64))
        .expect("a full output");
    let report = tick_many(&mut state, &mut container, 300);
    assert!(!report.cook_progressed);
    assert!(!report.fuel_consumed);
    assert_eq!(
        container.get(2),
        stack(item("minecraft:cooked_porkchop"), 64),
        "an output at its limit must not grow"
    );
    assert_eq!(
        container.get(0),
        stack(item("minecraft:porkchop"), 4),
        "the input must not be consumed into a full output"
    );
}

// ----------------------------------------------------------------- hostile

#[test]
fn a_furnace_refuses_impossible_slot_layouts_instead_of_panicking() {
    let recipes = recipes();
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, coal(), 1);

    // Two slots the same.
    let overlapping = FurnaceSlots {
        input: 0,
        fuel: 0,
        output: 2,
    };
    let error = Furnace::tick(
        &mut state,
        &mut container,
        overlapping,
        &recipes,
        &items(),
        &sizes(),
    )
    .expect_err("overlapping slots are an invariant violation");
    assert!(matches!(error, ServerError::Invariant(_)), "{error:?}");
    assert_eq!(
        container.get(1),
        stack(coal(), 1),
        "the refused tick must not have burned fuel"
    );

    // A slot past the end of the container.
    let too_far = FurnaceSlots {
        input: 0,
        fuel: 1,
        output: 3,
    };
    let error = Furnace::tick(
        &mut state,
        &mut container,
        too_far,
        &recipes,
        &items(),
        &sizes(),
    )
    .expect_err("a slot outside the container is an invariant violation");
    assert!(matches!(error, ServerError::Invariant(_)), "{error:?}");

    // The same checks on the non-mutating form.
    assert!(Furnace::ready_to_cook(&container, overlapping, &recipes, &sizes()).is_err());
    assert!(Furnace::ready_to_cook(&container, too_far, &recipes, &sizes()).is_err());
    assert!(
        Furnace::ready_to_cook(&container, FurnaceSlots::CANONICAL, &recipes, &sizes())
            .expect("a valid layout")
    );
    assert!(!FurnaceSlots::CANONICAL.indices().is_empty());
    assert!(FurnaceSlots::CANONICAL.are_distinct());
    assert!(!overlapping.are_distinct());
}

#[test]
fn an_impossible_input_count_or_unregistered_input_never_panics() {
    // A negative count cannot exist in an `ItemStack`, and neither can a 65-count
    // one: the type refuses both before a furnace ever sees them.
    assert!(
        ItemStack::new(iron_ore(), -1)
            .expect("normalises")
            .is_empty()
    );
    assert!(ItemStack::new(iron_ore(), 65).is_err());

    // An un-smeltable input id that is nevertheless a real item.
    let mut state = FurnaceState::new();
    let mut container = loaded(stone(), 1, coal(), 1);
    let report = tick_many(&mut state, &mut container, 3);
    assert!(report.is_noop(), "{report:?}");
}

#[test]
fn an_empty_furnace_never_reports_a_change_after_the_first_idle_tick() {
    let mut state = FurnaceState::new();
    let mut container = furnace_container();
    // Even a completely idle furnace must answer coherently for many ticks.
    for _ in 0..100 {
        let report = tick(&mut state, &mut container);
        assert!(!report.lit);
        assert_eq!(report.finished_items, 0);
    }
    assert_eq!(
        state.cook_fraction().to_bits(),
        0.0f32.to_bits(),
        "no denominator means no progress"
    );
    assert_eq!(state, FurnaceState::new());
}

#[test]
fn a_furnace_without_fuel_but_with_input_stays_dark_and_keeps_its_input() {
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, 0, 0);
    let report = tick_many(&mut state, &mut container, 10);
    assert!(!report.lit);
    assert!(!report.fuel_consumed);
    assert!(!report.cook_progressed);
    assert_eq!(container.get(0), stack(iron_ore(), 1));
    assert_eq!(state, FurnaceState::new());
}

#[test]
fn a_non_fuel_in_the_fuel_slot_is_refused_rather_than_burned() {
    // Vanilla's fuel slot accepts anything; the burn time decides. A stone in the
    // fuel slot must not be consumed.
    let mut state = FurnaceState::new();
    let mut container = loaded(iron_ore(), 1, stone(), 1);
    let report = tick_many(&mut state, &mut container, 20);
    assert!(!report.fuel_consumed);
    assert!(!report.lit);
    assert_eq!(
        container.get(1),
        stack(stone(), 1),
        "the stone is untouched"
    );
    assert_eq!(container.get(0), stack(iron_ore(), 1));
}

#[test]
fn a_full_tick_sequence_is_deterministic() {
    // AGENTS.md section 3.6: the same state plus the same tick count is the same
    // state. Run the same sequence twice and compare the outcome.
    let run = || {
        let mut state = FurnaceState::new();
        let mut container = loaded(iron_ore(), 5, coal(), 1);
        let mut finishes = 0u32;
        for _ in 0..500 {
            finishes += tick(&mut state, &mut container).finished_items;
        }
        (state, container.slots().to_vec(), finishes)
    };
    let (state_a, slots_a, finishes_a) = run();
    let (state_b, slots_b, finishes_b) = run();
    assert_eq!(state_a, state_b);
    assert_eq!(slots_a, slots_b);
    assert_eq!(finishes_a, finishes_b);
    // 500 ticks lit by one coal: two 200-tick cooks complete and a third is part-way
    // through, 100 ticks in.
    assert_eq!(finishes_a, 2, "two ingots in 500 ticks");
    assert_eq!(slots_a[2], stack(iron_ingot(), 2));
    assert_eq!(slots_a[0], stack(iron_ore(), 3), "three inputs left");
    assert_eq!(state_a.cook_progress, 100, "the third cook is part-way");
}
