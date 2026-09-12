//! Menu tests, weighted towards the adversarial cases (P06-15).
//!
//! The properties under test are the ones that decide whether a hostile client can
//! create or destroy items:
//!
//! * conservation — a transfer moves items, it never changes the total;
//! * boundedness — no stack and no cursor ever exceeds its limit;
//! * validation — a stale state id, a foreign window and an out-of-range slot are
//!   all refused before anything mutates;
//! * role enforcement — a client cannot place into a computed slot.

use mc_entity::stack::{ItemStack, StackSizeTable};
use mc_registry::ItemRegistry;
use std::path::Path;

use super::{ClickOutcome, Menu, MenuLayout, SlotMapping, SlotRange};
use crate::click::{Click, ClickType, DragType};
use crate::container::{Container, ContainerKind, SlotRole};

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the registry fixture loads")
}

fn sizes() -> StackSizeTable {
    StackSizeTable::resolve(&items()).expect("the stack-size table resolves")
}

/// An item id resolved by name.
///
/// Numeric ids are **not** hard-coded in these tests: guessing them produced two
/// wrong constants (id 3 is polished granite, not a bucket) and one test that
/// passed for the wrong reason. A name lookup cannot drift from the registry.
fn item(name: &str) -> i32 {
    items()
        .id(name)
        .unwrap_or_else(|_| panic!("{name} must exist in the item registry"))
}

/// A 64-stack block: the ordinary case.
fn stone() -> i32 {
    item("minecraft:stone")
}

/// A different 64-stack item, for "different items do not merge".
fn granite() -> i32 {
    item("minecraft:granite")
}

/// A 16-stack item, for the per-item limit rules.
fn bucket() -> i32 {
    item("minecraft:bucket")
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count).expect("a valid stack")
}

/// A menu with a 9-slot container (0) plus a player inventory (1), laid out the
/// way a chest is: container slots, then the player's 36.
fn chest_menu() -> Menu {
    let chest = Container::new(ContainerKind::Generic, 9).expect("chest");
    let player = Container::new(ContainerKind::Player, 41).expect("player");
    let mut slots = Vec::new();
    for slot in 0..9u16 {
        slots.push(SlotMapping::storage(0, slot));
    }
    // Player main (storage 9..=35) then hotbar (storage 0..=8), as Vanilla.
    for slot in 9..36u16 {
        slots.push(SlotMapping::storage(1, slot));
    }
    for slot in 0..9u16 {
        slots.push(SlotMapping::storage(1, slot));
    }
    Menu::new(
        1,
        vec![chest, player],
        slots,
        MenuLayout::container_and_player(9),
        sizes(),
    )
    .expect("the chest menu builds")
}

fn player_menu() -> Menu {
    let player = Container::new(ContainerKind::Player, 41).expect("player");
    Menu::player(0, player, sizes()).expect("the player menu builds")
}

fn click(slot: i16, button: i8, kind: ClickType) -> Click {
    Click::new(0, 0, slot, button, kind.id()).expect("a structurally legal click")
}

/// Apply a click at the menu's current state id.
fn apply(menu: &mut Menu, slot: i16, button: i8, kind: ClickType) -> ClickOutcome {
    let mut c = click(slot, button, kind);
    c.window_id = menu.window_id();
    c.state_id = menu.state_id();
    menu.apply_click(&c).expect("the click applies")
}

// ------------------------------------------------------------------ layout

#[test]
fn the_player_menu_matches_the_jar_verified_layout() {
    let menu = player_menu();
    assert_eq!(menu.slot_count(), 46);
    assert_eq!(menu.mapping(0).expect("0").role, SlotRole::CraftingResult);
    for slot in 1..=4 {
        assert_eq!(
            menu.mapping(slot).expect("grid").role,
            SlotRole::CraftingInput,
            "slot {slot}"
        );
    }
    for slot in 5..=8 {
        assert_eq!(menu.mapping(slot).expect("armour").role, SlotRole::Armor);
    }
    for slot in 9..=44 {
        assert_eq!(menu.mapping(slot).expect("storage").role, SlotRole::Storage);
    }
    assert_eq!(menu.mapping(45).expect("offhand").role, SlotRole::Offhand);

    // The armour permutation is forward because the jar's SLOT_IDS starts at FEET:
    // menu 5 is boots, which our player container stores at slot 36.
    assert_eq!(menu.mapping(5).expect("boots").container, 0);
    assert_eq!(menu.mapping(5).expect("boots").slot, 36);
    assert_eq!(menu.mapping(8).expect("helmet").slot, 39);
    // The hotbar maps to storage 0..=8.
    assert_eq!(menu.mapping(36).expect("hotbar 0").slot, 0);
    assert_eq!(menu.mapping(44).expect("hotbar 8").slot, 8);
    // Main inventory is an identity mapping.
    assert_eq!(menu.mapping(9).expect("main 0").slot, 9);
    assert_eq!(menu.mapping(35).expect("main 26").slot, 35);
}

#[test]
fn a_menu_rejects_a_mapping_that_points_nowhere() {
    let chest = Container::new(ContainerKind::Generic, 9).expect("chest");
    // Points at container 5, which does not exist.
    assert!(
        Menu::new(
            1,
            vec![chest.clone()],
            vec![SlotMapping::storage(5, 0)],
            MenuLayout::none(),
            sizes()
        )
        .is_err()
    );
    // Points past the end of container 0.
    assert!(
        Menu::new(
            1,
            vec![chest.clone()],
            vec![SlotMapping::storage(0, 9)],
            MenuLayout::none(),
            sizes()
        )
        .is_err()
    );
    // An impossible slot ceiling.
    assert!(
        Menu::new(
            1,
            vec![chest.clone()],
            vec![SlotMapping::with_limit(0, 0, SlotRole::Storage, 0)],
            MenuLayout::none(),
            sizes()
        )
        .is_err()
    );
    // No slots at all.
    assert!(Menu::new(1, vec![chest], Vec::new(), MenuLayout::none(), sizes()).is_err());
}

#[test]
fn transfer_order_never_includes_the_source_group_first() {
    let layout = MenuLayout::container_and_player(9);
    // From the container: player main, then hotbar.
    let order = layout.transfer_order(0);
    assert_eq!(order, vec![SlotRange::new(9, 36), SlotRange::new(36, 45)]);
    // From the hotbar: wraps back to the container first.
    let from_hotbar = layout.transfer_order(40);
    assert_eq!(
        from_hotbar,
        vec![SlotRange::new(0, 9), SlotRange::new(9, 36)]
    );
    // A slot in no group tries every group in order.
    assert_eq!(layout.transfer_order(999).len(), 3);
    assert!(MenuLayout::none().transfer_order(0).is_empty());
    assert_eq!(layout.group_of(0), Some(0));
    assert_eq!(layout.group_of(50), None);
    assert!(SlotRange::new(0, 9).contains(8));
    assert!(!SlotRange::new(0, 9).contains(9));
    assert_eq!(SlotRange::new(0, 9).len(), 9);
    assert!(SlotRange::new(5, 5).is_empty());
    assert_eq!(SlotRange::new(9, 5).len(), 0);
}

// ------------------------------------------------------------ validation

#[test]
fn a_foreign_window_is_refused_and_changes_nothing() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 10)).expect("set");
    let before = menu.total_items();
    let mut c = click(0, 0, ClickType::Pickup);
    c.window_id = menu.window_id().wrapping_add(1);
    assert!(menu.apply_click(&c).is_err());
    assert_eq!(menu.total_items(), before);
    assert!(menu.cursor().is_empty());
}

#[test]
fn an_out_of_range_slot_is_refused() {
    let mut menu = chest_menu();
    let mut c = click(0, 0, ClickType::Pickup);
    c.window_id = menu.window_id();
    c.slot = 999;
    assert!(menu.apply_click(&c).is_err());
}

#[test]
fn a_stale_state_id_triggers_a_resync_and_applies_nothing() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 10)).expect("set");
    let before = menu.total_items();

    let mut c = click(0, 0, ClickType::Pickup);
    c.window_id = menu.window_id();
    c.state_id = menu.state_id() + 1; // the client is one revision behind
    let outcome = menu.apply_click(&c).expect("a resync, not an error");
    assert!(outcome.full_resync);
    assert!(outcome.changed_slots.is_empty());
    assert!(!outcome.cursor_changed);
    assert_eq!(
        menu.total_items(),
        before,
        "a stale click must not move items"
    );
    assert!(menu.cursor().is_empty());
}

#[test]
fn the_state_id_advances_on_every_accepted_click() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 10)).expect("set");
    let start = menu.state_id();
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert!(
        menu.state_id() > start,
        "an accepted click bumps the revision"
    );

    // A click that changes nothing must not bump the revision. Picking up from an
    // empty slot with an empty cursor is the honest no-op; note that the *same*
    // click with a full cursor would legitimately place the stack, so the cursor
    // has to be empty for this to test what it claims.
    let mut empty = chest_menu();
    let before = empty.state_id();
    apply(&mut empty, 5, 0, ClickType::Pickup);
    assert!(
        empty.cursor().is_empty() && empty.display_stack(5).is_empty(),
        "the premise: nothing moved"
    );
    assert_eq!(
        empty.state_id(),
        before,
        "a no-op must not bump the revision"
    );
}

#[test]
fn replaying_a_click_against_the_old_state_is_refused() {
    // The anti-replay property: a client that sends the same click twice is
    // applying the second one to a state it has already moved past.
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    let mut c = click(0, 0, ClickType::Pickup);
    c.window_id = menu.window_id();
    c.state_id = menu.state_id();

    let first = menu.apply_click(&c).expect("first click applies");
    assert!(!first.full_resync);
    assert_eq!(menu.cursor().count(), 64);

    // Replaying the identical packet now carries a stale state id.
    let replay = menu.apply_click(&c).expect("replay is a resync");
    assert!(replay.full_resync, "the replay must be refused");
    assert_eq!(
        menu.cursor().count(),
        64,
        "and must not have moved anything"
    );
    assert_eq!(menu.total_items(), 64);
}

// --------------------------------------------------------- conservation

/// Every click type that must not change the total, exercised against a menu with
/// a full slot and an empty one.
#[test]
fn transfers_conserve_items() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 20)).expect("set");
    menu.set_slot(1, stack(stone(), 5)).expect("set");
    let before = menu.total_items();
    assert_eq!(before, 25);

    // Pick up the whole stack.
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert_eq!(menu.cursor().count(), 20);
    assert_eq!(menu.total_items(), before);

    // Put it down in an empty slot.
    apply(&mut menu, 3, 0, ClickType::Pickup);
    assert_eq!(menu.display_stack(3).count(), 20);
    assert!(menu.cursor().is_empty());
    assert_eq!(menu.total_items(), before);

    // Pick up half.
    apply(&mut menu, 3, 1, ClickType::Pickup);
    assert_eq!(menu.cursor().count(), 10);
    assert_eq!(menu.display_stack(3).count(), 10);
    assert_eq!(menu.total_items(), before);

    // Merge into the matching stack.
    apply(&mut menu, 1, 0, ClickType::Pickup);
    assert_eq!(menu.display_stack(1).count(), 15);
    assert!(menu.cursor().is_empty());
    assert_eq!(menu.total_items(), before);

    // Shift-click it to the player inventory.
    apply(&mut menu, 1, 0, ClickType::QuickMove);
    assert_eq!(menu.total_items(), before);
    assert!(menu.display_stack(1).is_empty(), "the source must empty");
}

#[test]
fn a_drag_conserves_items_across_all_three_types() {
    for drag_type in [DragType::One, DragType::Even, DragType::Full] {
        let mut menu = chest_menu();
        menu.set_slot(0, stack(stone(), 64)).expect("set");
        let before = menu.total_items();
        // Pick the stack up.
        apply(&mut menu, 0, 0, ClickType::Pickup);
        // Drag across four empty slots. The button packs type and stage.
        let start = (drag_type.packed() << 2) as i8;
        let add = (drag_type.packed() << 2) as i8 | 1;
        let end = (drag_type.packed() << 2) as i8 | 2;
        apply(&mut menu, 1, start, ClickType::QuickCraft);
        for slot in 1..=4i16 {
            apply(&mut menu, slot, add, ClickType::QuickCraft);
        }
        apply(&mut menu, 1, end, ClickType::QuickCraft);
        assert_eq!(
            menu.total_items(),
            before,
            "{drag_type:?} drag must conserve items"
        );
    }
}

#[test]
fn pickup_all_gathers_only_matching_items_and_conserves() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 10)).expect("set");
    menu.set_slot(1, stack(stone(), 10)).expect("set");
    menu.set_slot(2, stack(granite(), 10)).expect("set");
    let before = menu.total_items();
    apply(&mut menu, 0, 0, ClickType::PickupAll);
    assert_eq!(menu.cursor().count(), 20, "both stone stacks");
    assert_eq!(menu.display_stack(1), ItemStack::EMPTY);
    assert_eq!(menu.display_stack(2).count(), 10, "dirt is untouched");
    assert_eq!(menu.total_items(), before);
}

#[test]
fn a_swap_conserves_items() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 7)).expect("set");
    // Hotbar slot 0 is menu slot 36 in the chest layout (9 + 27).
    menu.set_slot(36, stack(granite(), 3)).expect("set");
    let before = menu.total_items();
    apply(&mut menu, 0, 0, ClickType::Swap);
    assert_eq!(menu.display_stack(0), stack(granite(), 3));
    assert_eq!(menu.display_stack(36), stack(stone(), 7));
    assert_eq!(menu.total_items(), before);
}

#[test]
fn only_throw_and_creative_clone_change_the_total() {
    // Throw removes exactly what it hands to the caller.
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    let before = menu.total_items();
    let outcome = apply(&mut menu, 0, 0, ClickType::Throw);
    assert_eq!(outcome.dropped.len(), 1);
    assert_eq!(outcome.dropped[0].count(), 1);
    assert_eq!(menu.total_items(), before - 1, "exactly one item left");

    let outcome = apply(&mut menu, 0, 1, ClickType::Throw);
    assert_eq!(outcome.dropped[0].count(), 63, "the rest of the stack");
    assert_eq!(menu.total_items(), 0);

    // Clone in survival is refused outright.
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    let before = menu.total_items();
    let outcome = apply(&mut menu, 0, 0, ClickType::Clone);
    assert!(outcome.is_noop(), "survival cannot clone");
    assert_eq!(menu.total_items(), before);
}

#[test]
fn creative_clone_is_the_only_way_to_create_items() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 16)).expect("set");
    menu.set_creative(true);
    let before = menu.total_items();
    apply(&mut menu, 0, 0, ClickType::Clone);
    assert_eq!(menu.cursor().count(), 16);
    assert_eq!(
        menu.total_items(),
        before + 16,
        "creative clone is documented to duplicate"
    );
    // Button 2 clones a full stack.
    apply(&mut menu, 3, 0, ClickType::Pickup); // put it away again
    menu.set_slot(3, stack(stone(), 1)).expect("set");
    apply(&mut menu, 3, 2, ClickType::Clone);
    assert_eq!(menu.cursor().count(), 64, "clone-to-max");
}

// -------------------------------------------------------------- limits

#[test]
fn a_stack_limit_is_enforced_on_the_cursor_and_in_slots() {
    let mut menu = chest_menu();
    // A bucket stacks to 16. Assert the premise first: if the resolved limit were
    // 64 this test would pass while proving nothing.
    assert_eq!(
        menu.item_limit(stack(bucket(), 1)),
        16,
        "minecraft:bucket must resolve to a 16-stack for this test to mean anything"
    );
    menu.set_slot(0, stack(bucket(), 16)).expect("set");
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert_eq!(
        menu.cursor().count(),
        16,
        "the cursor honours the item limit"
    );

    // A hostile stack of 64 buckets cannot be conjured. Put 64-but-legal-for-the-
    // hard-ceiling buckets on the cursor and check the clamp brings it to 16.
    menu.set_creative(true);
    menu.set_cursor(stack(bucket(), 64));
    assert_eq!(
        menu.cursor().count(),
        16,
        "the cursor clamps to the item's own limit, not the hard ceiling"
    );
}

#[test]
fn placing_more_than_fits_leaves_the_overflow_on_the_cursor() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    menu.set_slot(1, stack(stone(), 60)).expect("set");
    apply(&mut menu, 0, 0, ClickType::Pickup);
    // Slot 1 has room for 4.
    apply(&mut menu, 1, 0, ClickType::Pickup);
    assert_eq!(menu.display_stack(1).count(), 64);
    assert_eq!(
        menu.cursor().count(),
        60,
        "the overflow stayed on the cursor"
    );
    assert_eq!(menu.total_items(), 124);
}

#[test]
fn a_slot_ceiling_below_the_item_limit_is_respected() {
    // A furnace output holds a smelt result; model a slot capped at 1.
    let container = Container::new(ContainerKind::Generic, 2).expect("container");
    let mut slots = vec![
        SlotMapping::with_limit(0, 0, SlotRole::Storage, 1),
        SlotMapping::storage(0, 1),
    ];
    slots.truncate(2);
    let mut menu = Menu::new(1, vec![container], slots, MenuLayout::none(), sizes())
        .expect("the capped menu builds");
    menu.set_slot(1, stack(stone(), 64)).expect("set");
    let outcome = apply(&mut menu, 1, 0, ClickType::Pickup);
    assert_eq!(menu.cursor().count(), 64);
    assert!(outcome.cursor_changed);
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert_eq!(
        menu.display_stack(0).count(),
        1,
        "capped at the slot ceiling"
    );
    assert_eq!(menu.cursor().count(), 63, "the rest came back");
    assert_eq!(menu.total_items(), 64);
}

// ---------------------------------------------------------------- roles

#[test]
fn a_computed_slot_refuses_placement() {
    let mut menu = player_menu();
    // Menu slot 0 is the crafting result: take-only.
    assert!(!menu.mapping(0).expect("result").may_place());
    menu.set_cursor(stack(stone(), 5));
    let outcome = apply(&mut menu, 0, 0, ClickType::Pickup);
    assert!(
        outcome.is_noop(),
        "placing into the result slot must do nothing"
    );
    assert!(menu.display_stack(0).is_empty());
    assert_eq!(menu.cursor().count(), 5);

    // A drag cannot route around the rule either.
    let mut menu = player_menu();
    let before = menu.total_items();
    menu.set_cursor(stack(stone(), 5));
    apply(
        &mut menu,
        0,
        (DragType::One.packed() << 2) as i8,
        ClickType::QuickCraft,
    );
    apply(
        &mut menu,
        0,
        (DragType::One.packed() << 2) as i8 | 1,
        ClickType::QuickCraft,
    );
    apply(
        &mut menu,
        0,
        (DragType::One.packed() << 2) as i8 | 2,
        ClickType::QuickCraft,
    );
    assert!(
        menu.display_stack(0).is_empty(),
        "the result slot stays empty"
    );
    assert_eq!(
        menu.total_items(),
        before + 5,
        "the cursor still holds them"
    );
}

#[test]
fn a_swap_cannot_place_into_a_computed_slot() {
    let mut menu = player_menu();
    menu.set_slot(36, stack(stone(), 5)).expect("set hotbar 0");
    // Swap the crafting result (menu 0) with hotbar 0: the result slot cannot
    // receive, so nothing may move.
    let outcome = apply(&mut menu, 0, 0, ClickType::Swap);
    let _ = outcome;
    assert!(menu.display_stack(0).is_empty());
    assert_eq!(menu.display_stack(36).count(), 5, "the hotbar is untouched");
}

#[test]
fn shift_click_out_of_a_computed_slot_moves_the_result_out() {
    let mut menu = player_menu();
    // Put something in the result slot the way the server would (crafting).
    menu.set_slot(0, stack(stone(), 4))
        .expect("seed the result");
    let before = menu.total_items();
    apply(&mut menu, 0, 0, ClickType::QuickMove);
    assert!(menu.display_stack(0).is_empty());
    assert_eq!(menu.total_items(), before, "shift-click conserves");
    // It landed somewhere in the player's storage.
    let moved = (9..45).any(|slot| menu.display_stack(slot) == stack(stone(), 4));
    assert!(
        moved,
        "the result must have moved into the player inventory"
    );
}

// ------------------------------------------------------------- hostile

/// What a hostile flood did, so the caller can assert conservation against it.
struct Flood {
    /// Items the click generator threw on the floor (the only legal decrease).
    thrown: i64,
    /// Items created — must be zero in survival.
    created: i64,
    /// Structurally illegal clicks the decoder refused.
    refused_as_malformed: usize,
    /// Clicks the menu accepted.
    accepted: usize,
}

/// Drive `rounds` deterministic pseudo-random hostile clicks into `menu`.
///
/// Extracted so one generator can drive several menus. A single menu is not enough: a chest's slots all
/// accept 64, so its flood never reaches `Menu::insert`'s over-limit overflow branch, and an audit showed
/// that discarding the overflow entirely left the chest flood green (Audit 07, finding M1). The caller
/// that needs the overflow path passes a menu with a capped slot.
///
/// The only invariants asserted per click are conservation and boundedness, because a hostile client's
/// goal is to break exactly those.
fn hostile_flood(menu: &mut Menu, rounds: usize) -> Flood {
    let mut seed = 0x1234_5678u64;
    let mut next = |bound: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % bound
    };

    let mut flood = Flood {
        thrown: 0,
        created: 0,
        refused_as_malformed: 0,
        accepted: 0,
    };
    for _ in 0..rounds {
        let slot = next(46) as i16;
        // Any button, including ones no click type defines: the decoder must
        // refuse those rather than reinterpret them.
        let button = next(200) as i8;
        let kind = ClickType::ALL[next(7) as usize];
        let Ok(mut c) = Click::new(0, 0, slot, button, kind.id()) else {
            flood.refused_as_malformed += 1;
            continue;
        };
        c.window_id = menu.window_id();
        c.state_id = menu.state_id();
        // A hostile client also lies about the state id sometimes.
        if next(8) == 0 {
            c.state_id = c.state_id.wrapping_add(next(3) as i32);
        }
        let Ok(outcome) = menu.apply_click(&c) else {
            continue;
        };
        flood.accepted += 1;
        for stack in &outcome.dropped {
            flood.thrown += i64::from(stack.count());
        }
        if kind == ClickType::Clone && menu.is_creative() {
            flood.created += 1;
        }
        // Boundedness, checked after every single click.
        assert!(
            menu.cursor().is_empty() || menu.cursor().count() <= menu.item_limit(menu.cursor()),
            "the cursor exceeded its limit: {:?}",
            menu.cursor()
        );
        for index in 0..menu.slot_count() {
            let stack = menu.display_stack(index);
            assert!(
                stack.is_empty() || stack.count() <= menu.item_limit(stack),
                "slot {index} exceeded its item limit: {stack:?}"
            );
            assert!(stack.count() >= 0, "slot {index} went negative");
        }
        assert!(menu.total_items() >= 0, "the total went negative");
    }
    flood
}

/// A menu whose slot 0 is capped at one item — the furnace-output shape.
///
/// Placing a 64-stack into it takes `Menu::insert`'s overflow branch, which is the path that decides
/// whether the excess comes back to the cursor or is destroyed.
fn capped_slot_menu() -> Menu {
    let container = Container::new(ContainerKind::Generic, 2).expect("container");
    let mut slots = vec![
        SlotMapping::with_limit(0, 0, SlotRole::Storage, 1),
        SlotMapping::storage(0, 1),
    ];
    slots.truncate(2);
    Menu::new(1, vec![container], slots, MenuLayout::none(), sizes())
        .expect("the capped menu builds")
}

#[test]
fn a_flood_of_hostile_clicks_cannot_create_or_destroy_items() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    menu.set_slot(1, stack(bucket(), 16)).expect("set");
    menu.set_slot(9, stack(granite(), 33)).expect("set");
    let before = menu.total_items();

    let flood = hostile_flood(&mut menu, 2_000);
    let thrown = flood.thrown;

    assert_eq!(flood.created, 0, "survival must never create items");
    assert!(
        flood.refused_as_malformed > 0,
        "the generator must have produced some structurally illegal clicks for the \
         decoder to refuse; otherwise this test is not exercising that path"
    );
    assert_eq!(
        menu.total_items(),
        before - thrown,
        "conservation: the only change is what was thrown"
    );
}

#[test]
fn a_flood_over_a_capped_slot_cannot_create_or_destroy_items() {
    // The audit's M1: the chest flood above never reaches `Menu::insert`'s over-limit branch, because
    // every chest slot accepts 64 and its stacks are 64/16/33. So the claim "floods cannot destroy
    // items" was not exercised on the one path that can. This runs the **same** generator against a
    // menu whose slot 0 holds a single item at a time, so each attempt to place a 64-stack there takes
    // the overflow branch.
    let mut menu = capped_slot_menu();
    // Slot 0 is **empty** on purpose. The audit's break lives in `Menu::insert`'s
    // `existing.is_empty()` branch, so a seeded slot would take the merge branch and prove nothing —
    // which is exactly what the first version of this test did.
    // `stone` stacks to 64; `bucket` would stack to 16 and this test needs a full 64 on the cursor.
    menu.set_slot(1, stack(stone(), 64)).expect("set");
    let before = menu.total_items();

    // Deterministic coverage of the over-limit placement: 64 items onto an empty slot that holds one
    // at a time. `placed.count() > limit` is true here, so the overflow branch runs on this click.
    apply(&mut menu, 1, 0, ClickType::Pickup);
    assert_eq!(
        menu.cursor().count(),
        64,
        "the whole stack is on the cursor"
    );
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert_eq!(
        menu.display_stack(0).count(),
        1,
        "the capped slot accepted exactly its ceiling"
    );
    assert_eq!(
        menu.cursor().count(),
        63,
        "and the 63 that did not fit came back to the cursor instead of being destroyed"
    );
    assert_eq!(
        menu.total_items(),
        before,
        "placing into a capped slot must conserve every item"
    );

    // Then the hostile-random part, over the same menu.
    let flood = hostile_flood(&mut menu, 2_000);

    assert_eq!(flood.created, 0, "survival must never create items");
    assert_eq!(
        menu.total_items(),
        before - flood.thrown,
        "conservation over a capped slot: the overflow must return to the cursor, never vanish"
    );
    assert!(
        menu.display_stack(0).count() <= 1,
        "the capped slot must never hold more than its ceiling"
    );
}

#[test]
fn a_drag_that_is_never_ended_does_not_leak_state() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    apply(&mut menu, 0, 0, ClickType::Pickup);
    // Start a drag, add slots, then start another drag of a different type
    // without ending the first.
    apply(
        &mut menu,
        1,
        (DragType::Even.packed() << 2) as i8,
        ClickType::QuickCraft,
    );
    apply(
        &mut menu,
        2,
        (DragType::Even.packed() << 2) as i8 | 1,
        ClickType::QuickCraft,
    );
    apply(
        &mut menu,
        3,
        (DragType::Full.packed() << 2) as i8,
        ClickType::QuickCraft,
    );
    // Ending the *first* type now must not distribute anything.
    let before = menu.total_items();
    apply(
        &mut menu,
        1,
        (DragType::Even.packed() << 2) as i8 | 2,
        ClickType::QuickCraft,
    );
    assert_eq!(menu.total_items(), before);
    assert_eq!(menu.cursor().count(), 64, "the cursor is untouched");
}

#[test]
fn a_close_click_does_not_destroy_the_cursor() {
    // Closing a window with items on the cursor is a classic item-loss bug. The
    // menu does not implement close, but the invariant it must preserve is that the
    // cursor is readable so the caller can put the items back.
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    apply(&mut menu, 0, 0, ClickType::Pickup);
    assert_eq!(menu.cursor().count(), 64);
    assert_eq!(
        menu.total_items(),
        64,
        "the cursor's contents are part of the total, so a caller can recover them"
    );
}

// --------------------------------------------------------- change tracking

#[test]
fn only_touched_slots_are_reported_as_changed() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 64)).expect("set");
    menu.set_slot(20, stack(granite(), 1))
        .expect("set in the player inventory");
    menu.clear_changed();
    apply(&mut menu, 0, 0, ClickType::Pickup);
    let changed = menu.changed_menu_slots();
    assert_eq!(changed.iter().copied().collect::<Vec<_>>(), vec![0]);
    menu.clear_changed();
    assert!(menu.changed_menu_slots().is_empty());
}

#[test]
fn a_full_resync_payload_has_one_entry_per_slot() {
    let menu = player_menu();
    let contents = menu.full_contents();
    assert_eq!(contents.len(), menu.slot_count());
    assert!(contents.iter().all(ItemStack::is_empty));
}

#[test]
fn drain_changed_returns_slots_and_clears_the_flags() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 1)).expect("set");
    menu.set_slot(1, stack(granite(), 1)).expect("set");
    let drained = menu.drain_changed();
    assert_eq!(drained.iter().copied().collect::<Vec<_>>(), vec![0, 1]);
    assert!(menu.drain_changed().is_empty(), "the second drain is empty");
}

#[test]
fn setting_the_same_slot_twice_reports_one_change() {
    let mut menu = chest_menu();
    menu.set_slot(0, stack(stone(), 1)).expect("set");
    menu.set_slot(0, stack(stone(), 1)).expect("set again");
    assert_eq!(menu.changed_menu_slots().len(), 1);
}

#[test]
fn an_out_of_range_write_is_refused() {
    let mut menu = chest_menu();
    assert!(menu.set_slot(999, stack(stone(), 1)).is_err());
    assert!(menu.set_slot(menu.slot_count(), stack(stone(), 1)).is_err());
    // The cursor setter clamps rather than erroring. A bucket's limit is 16, so
    // 32 is legal for `ItemStack::new` (the hard ceiling is 64) but over this
    // item's own limit — which is the case the clamp exists for.
    menu.set_cursor(stack(bucket(), 32));
    assert_eq!(menu.cursor().count(), 16);
    menu.set_cursor(ItemStack::EMPTY);
    assert!(menu.cursor().is_empty());
}
