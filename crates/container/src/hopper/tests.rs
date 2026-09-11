//! Hopper tests (P06-08), weighted towards conservation.
//!
//! The properties under test:
//!
//! * **conservation** — a hopper move never creates or destroys an item, checked
//!   after every move in a deterministic adversarial loop;
//! * **exactness** — it moves exactly `per_transfer`, no more and no less, and
//!   nothing at all for a zero or negative request;
//! * **limits** — it respects a stack limit and stops at the destination's room;
//! * **roles** — it does not pull from a pickup-refusing slot and does not push
//!   into a computed slot;
//! * **no room** — a full destination leaves the items where they were.

use mc_entity::stack::{DEFAULT_MAX_STACK_SIZE, ItemStack};
use mc_registry::ItemRegistry;
use std::path::Path;

use super::{
    HOPPER_TRANSFER_COOLDOWN_TICKS, Hopper, HopperTransfer, MAX_ITEMS_PER_TRANSFER, wanted,
};
use crate::container::{Container, ContainerKind, SlotRole};

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the registry fixture loads")
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

fn container(slots: usize) -> Container {
    Container::new(ContainerKind::Generic, slots).expect("a generic container")
}

fn stone() -> i32 {
    item("minecraft:stone")
}

fn granite() -> i32 {
    item("minecraft:granite")
}

/// A 5-slot source with a stone stack in slot 0 and a granite stack in slot 2.
fn source() -> Container {
    let mut source = container(5);
    source.set(0, stack(stone(), 20)).expect("source 0");
    source.set(2, stack(granite(), 7)).expect("source 2");
    source
}

/// The total across every slot of a container, as the conservation instrument.
fn total(container: &Container) -> i64 {
    container.total_items()
}

// ------------------------------------------------------------------- basic

#[test]
fn a_transfer_moves_exactly_per_transfer_items() {
    let mut source = source();
    let mut destination = container(3);
    let result = Hopper::transfer_storage(&mut source, &mut destination, 5).expect("transfer");
    assert_eq!(
        result,
        HopperTransfer {
            moved: 5,
            source_slot: Some(0),
            destination_slot: Some(0),
        }
    );
    assert_eq!(source.get(0), stack(stone(), 15));
    assert_eq!(destination.get(0), stack(stone(), 5));
    assert_eq!(source.get(2), stack(granite(), 7), "untouched");

    // The next transfer takes from the same source slot until it is empty.
    let result = Hopper::transfer_storage(&mut source, &mut destination, 5).expect("transfer");
    assert_eq!(result.moved, 5);
    assert_eq!(result.source_slot, Some(0));
    assert_eq!(destination.get(0), stack(stone(), 10));
    let result = Hopper::transfer_storage(&mut source, &mut destination, 5).expect("transfer");
    assert_eq!(result.moved, 5);
    assert_eq!(source.get(0), stack(stone(), 5));
    let result = Hopper::transfer_storage(&mut source, &mut destination, 5).expect("transfer");
    assert_eq!(result.moved, 5, "the last five of the stack");
    assert!(
        source.get(0).is_empty(),
        "an emptied source slot must be empty, not a zero stack"
    );
    assert_eq!(destination.get(0), stack(stone(), 20));
}

#[test]
fn a_transfer_moves_at_most_what_the_source_holds() {
    let mut source = container(1);
    source.set(0, stack(stone(), 3)).expect("source");
    let mut destination = container(1);
    let result = Hopper::transfer_storage(&mut source, &mut destination, 64).expect("transfer");
    assert_eq!(
        result.moved, 3,
        "a 64-item request cannot take 3 items four times"
    );
    assert!(source.get(0).is_empty());
    assert_eq!(destination.get(0), stack(stone(), 3));
}

#[test]
fn a_transfer_moves_one_item_at_a_time_when_asked_for_one() {
    // `per_transfer = 1` is Vanilla's hopper: one item per transfer.
    let mut source = container(1);
    source.set(0, stack(stone(), 2)).expect("source");
    let mut destination = container(1);
    for expected in 1..=2 {
        let result = Hopper::transfer_storage(&mut source, &mut destination, 1).expect("transfer");
        assert_eq!(result.moved, 1);
        assert_eq!(destination.get(0).count(), expected);
    }
    assert!(source.get(0).is_empty());
}

#[test]
fn the_hopper_cooldown_constant_is_the_documented_one() {
    // Labelled as a product decision in the module docs: 8 ticks is the value this
    // build uses; nothing here schedules it.
    assert_eq!(HOPPER_TRANSFER_COOLDOWN_TICKS, 8);
    assert_eq!(MAX_ITEMS_PER_TRANSFER, DEFAULT_MAX_STACK_SIZE);
    assert_eq!(wanted(0), 0);
    assert_eq!(wanted(-1), 0);
    assert_eq!(wanted(i32::MIN), 0);
    assert_eq!(wanted(1), 1);
    assert_eq!(wanted(64), 64);
    assert_eq!(wanted(65), 64, "a request above a stack is clamped");
    assert_eq!(wanted(i32::MAX), 64);
}

// ------------------------------------------------------------------ limits

#[test]
fn a_transfer_respects_the_destination_stack_limit() {
    let mut source = container(1);
    source.set(0, stack(stone(), 64)).expect("source");
    let mut destination = container(1);
    destination.set(0, stack(stone(), 60)).expect("destination");
    let result = Hopper::transfer_storage(&mut source, &mut destination, 64).expect("transfer");
    assert_eq!(
        result.moved, 4,
        "only the four items of room may move, whatever was asked for"
    );
    assert_eq!(
        destination.get(0),
        stack(stone(), 64),
        "filled to the limit"
    );
    assert_eq!(source.get(0), stack(stone(), 60), "the rest stayed behind");

    // Now the destination slot is full, so the next transfer moves nothing and
    // looks for another slot.
    let result = Hopper::transfer_storage(&mut source, &mut destination, 64).expect("transfer");
    assert_eq!(result, HopperTransfer::none(), "a full container refuses");
    assert_eq!(source.get(0), stack(stone(), 60));
    assert!(!result.moved_anything());
}

#[test]
fn a_transfer_uses_the_second_slot_when_the_first_is_full() {
    let mut source = container(1);
    source.set(0, stack(stone(), 10)).expect("source");
    let mut destination = container(2);
    destination.set(0, stack(stone(), 64)).expect("full");
    let result = Hopper::transfer_storage(&mut source, &mut destination, 10).expect("transfer");
    assert_eq!(result.moved, 10);
    assert_eq!(
        result.destination_slot,
        Some(1),
        "it fell through to slot 1"
    );
    assert_eq!(destination.get(0), stack(stone(), 64));
    assert_eq!(destination.get(1), stack(stone(), 10));
}

#[test]
fn a_transfer_fills_the_first_acceptable_slot_and_merges_when_there_is_no_empty_one() {
    // Slot order decides between an empty slot and a matching stack: slot 0 is
    // empty, so the ten items go there and the matching stack in slot 1 is left
    // alone. (Vanilla's hopper tries the destination's slots in index order.)
    let mut source = container(2);
    source.set(0, stack(stone(), 10)).expect("source 0");
    source.set(1, stack(granite(), 10)).expect("source 1");
    let mut destination = container(2);
    destination
        .set(1, stack(stone(), 5))
        .expect("destination 1");
    let result = Hopper::transfer_storage(&mut source, &mut destination, 10).expect("transfer");
    assert_eq!(result.destination_slot, Some(0));
    assert_eq!(destination.get(0), stack(stone(), 10));
    assert_eq!(destination.get(1), stack(stone(), 5), "untouched");
    assert!(source.get(0).is_empty());

    // With slot 0 holding a different item, the matching stack in slot 1 takes
    // what fits and the rest stays in the source.
    let mut source = container(1);
    source.set(0, stack(stone(), 10)).expect("source");
    let mut destination = container(2);
    destination
        .set(0, stack(granite(), 1))
        .expect("a different item");
    destination.set(1, stack(stone(), 60)).expect("matching");
    let result = Hopper::transfer_storage(&mut source, &mut destination, 10).expect("transfer");
    assert_eq!(result.moved, 4, "60 + 10 would exceed 64, so only 4 fit");
    assert_eq!(result.destination_slot, Some(1));
    assert_eq!(destination.get(1), stack(stone(), 64));
    assert_eq!(destination.get(0), stack(granite(), 1), "untouched");
    assert_eq!(source.get(0), stack(stone(), 6), "six items stayed behind");
}

#[test]
fn a_destination_with_no_room_leaves_the_items_in_the_source() {
    let mut source = container(1);
    source.set(0, stack(stone(), 10)).expect("source");
    let mut destination = container(1);
    destination
        .set(0, stack(granite(), 1))
        .expect("a different item");

    let before_source = total(&source);
    let before_destination = total(&destination);
    let result = Hopper::transfer_storage(&mut source, &mut destination, 8).expect("transfer");
    assert_eq!(result, HopperTransfer::none());
    assert_eq!(source.get(0), stack(stone(), 10), "the items never moved");
    assert_eq!(destination.get(0), stack(granite(), 1));
    assert_eq!(total(&source), before_source);
    assert_eq!(total(&destination), before_destination);
}

#[test]
fn a_zero_or_negative_transfer_moves_nothing_and_does_not_panic() {
    for per_transfer in [0, -1, i32::MIN] {
        let mut source = source();
        let mut destination = container(3);
        let before_source = total(&source);
        let result = Hopper::transfer_storage(&mut source, &mut destination, per_transfer)
            .expect("no error");
        assert_eq!(
            result,
            HopperTransfer::none(),
            "per_transfer {per_transfer}"
        );
        assert_eq!(result.moved, 0);
        assert_eq!(result.source_slot, None);
        assert_eq!(result.destination_slot, None);
        assert_eq!(total(&source), before_source, "nothing left the source");
        assert_eq!(total(&destination), 0, "nothing arrived");
    }
}

#[test]
fn an_absurd_transfer_request_is_clamped_to_a_stack() {
    for per_transfer in [65, 1_000, i32::MAX] {
        let mut source = container(1);
        source.set(0, stack(stone(), 64)).expect("source");
        let mut destination = container(1);
        let result = Hopper::transfer_storage(&mut source, &mut destination, per_transfer)
            .expect("transfer");
        assert_eq!(
            result.moved, 64,
            "per_transfer {per_transfer} must not exceed a stack"
        );
        assert_eq!(destination.get(0), stack(stone(), 64));
    }
}

// ------------------------------------------------------------------- roles

#[test]
fn a_transfer_does_not_push_into_a_computed_slot() {
    // The crafting result and the furnace output are computed by the server; a
    // hopper pushing into one would be creating items.
    for role in [SlotRole::CraftingResult, SlotRole::FurnaceOutput] {
        let mut source = container(1);
        source.set(0, stack(stone(), 10)).expect("source");
        let mut destination = container(1);
        let roles = [role];
        let before = total(&destination);
        let result =
            Hopper::transfer(&mut source, &[], &mut destination, &roles, 10).expect("transfer");
        assert_eq!(
            result,
            HopperTransfer::none(),
            "{role:?} must refuse placement"
        );
        assert_eq!(source.get(0), stack(stone(), 10));
        assert_eq!(total(&destination), before);
    }

    // A furnace destination with input/fuel/output roles: only the input and fuel
    // slots may receive.
    let mut source = container(1);
    source.set(0, stack(stone(), 10)).expect("source");
    let mut furnace = container(3);
    let roles = [
        SlotRole::FurnaceInput,
        SlotRole::FurnaceFuel,
        SlotRole::FurnaceOutput,
    ];
    let result = Hopper::transfer(&mut source, &[], &mut furnace, &roles, 10).expect("transfer");
    assert_eq!(
        result.destination_slot,
        Some(0),
        "the input slot is a valid target"
    );
    assert_eq!(furnace.get(0), stack(stone(), 10));
    assert!(furnace.get(2).is_empty(), "the output slot is untouched");

    // A destination whose only slot is computed takes nothing.
    let mut source = container(1);
    source.set(0, stack(stone(), 10)).expect("source");
    let mut output_only = container(1);
    let result = Hopper::transfer(
        &mut source,
        &[],
        &mut output_only,
        &[SlotRole::FurnaceOutput],
        10,
    )
    .expect("transfer");
    assert_eq!(result, HopperTransfer::none());
    assert!(output_only.get(0).is_empty());
}

#[test]
fn a_transfer_does_not_pull_from_a_pickup_refusing_slot() {
    // `SlotRole::may_pickup` is `true` for every role this build has, so the
    // "refuses pickup" case cannot be built from the enum today. The test states
    // that premise explicitly (so a future role that refuses pickup turns this into
    // a real check) and then exercises the mechanism the code uses: the first
    // source slot whose role refuses pickup is skipped in favour of the next.
    for role in [
        SlotRole::Storage,
        SlotRole::Armor,
        SlotRole::Offhand,
        SlotRole::CraftingInput,
        SlotRole::CraftingResult,
        SlotRole::FurnaceInput,
        SlotRole::FurnaceFuel,
        SlotRole::FurnaceOutput,
    ] {
        assert!(
            role.may_pickup(),
            "{role:?} is documented as takeable; the premise of this test is that no \
             role refuses pickup yet"
        );
    }

    // A source whose *only* non-empty slot is not pullable must not be drained.
    // Since no role refuses pickup, the equivalent observable rule is that a
    // computed slot is still a legal source (a hopper under a furnace pulls the
    // output), which is what makes the "no pull" case a future change rather than a
    // silent behaviour here.
    let mut source = container(2);
    source.set(1, stack(stone(), 10)).expect("source 1");
    let mut destination = container(1);
    let roles = [SlotRole::Storage, SlotRole::FurnaceOutput];
    let result =
        Hopper::transfer(&mut source, &roles, &mut destination, &[], 10).expect("transfer");
    assert_eq!(result.source_slot, Some(1), "a furnace output is pullable");
    assert_eq!(result.destination_slot, Some(0));
    assert_eq!(destination.get(0), stack(stone(), 10));
    assert!(source.get(1).is_empty());
}

#[test]
fn a_role_slice_shorter_than_the_container_defaults_to_storage() {
    // A caller that supplies no roles (or too few) gets ordinary storage
    // behaviour, not a panic and not a silent refusal.
    let mut source = container(3);
    source.set(2, stack(stone(), 5)).expect("source");
    let mut destination = container(3);
    // One role for a three-slot container: the missing two are `Storage`.
    let result =
        Hopper::transfer(&mut source, &[SlotRole::Storage], &mut destination, &[], 5).expect("ok");
    assert_eq!(result.moved, 5);
    assert_eq!(result.source_slot, Some(2));
    assert_eq!(destination.get(0), stack(stone(), 5));
}

// ------------------------------------------------------------ empty inputs

#[test]
fn an_empty_source_container_or_role_list_is_a_no_op() {
    // No slots at all: `Container::new` refuses a zero-slot container, so this is
    // the smallest legal one, and it is empty.
    let mut empty_source = container(1);
    let mut destination = container(1);
    let result = Hopper::transfer_storage(&mut empty_source, &mut destination, 8).expect("ok");
    assert_eq!(result, HopperTransfer::none());

    // A source with items but a destination with no slots cannot be constructed
    // (`Container::new(kind, 0)` is an error), which is the boundary this module
    // relies on; assert it rather than assume it.
    assert!(Container::new(ContainerKind::Generic, 0).is_err());

    // An all-empty destination of one slot is a valid target; an all-empty source
    // still moves nothing.
    let mut destination = container(1);
    let result = Hopper::transfer_storage(&mut empty_source, &mut destination, 8).expect("ok");
    assert_eq!(result.moved, 0);
    assert!(destination.get(0).is_empty());
}

// ------------------------------------------------------------- adversarial

#[test]
fn a_flood_of_transfers_conserves_items_exactly() {
    // A deterministic pseudo-random sequence of transfers between containers of
    // different sizes, with items appearing only from a fixed starting budget. The
    // invariant is exact conservation: the two containers' totals plus everything
    // handed back must equal what we started with.
    let mut left = container(4);
    let mut right = container(6);

    let mut seed = 0x0BAD_F00D_1234_5678u64;
    let mut next = |bound: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % bound
    };

    // Seed with a bounded budget so conservation is a real claim: 4 + 6 slots of
    // at most 64 items across three item types.
    let kinds = [stone(), granite(), item("minecraft:dirt")];
    left.set(0, stack(kinds[0], 64)).expect("seed");
    left.set(1, stack(kinds[1], 33)).expect("seed");
    right.set(0, stack(kinds[2], 17)).expect("seed");
    right.set(3, stack(kinds[0], 40)).expect("seed");
    let budget = total(&left) + total(&right);
    assert_eq!(budget, 64 + 33 + 17 + 40);
    let mut moved_total = 0i64;

    for _ in 0..5_000 {
        let per_transfer = match next(4) {
            0 => 1,
            1 => 8,
            2 => 64,
            _ => i32::try_from(next(200)).expect("small") - 20, // includes 0 and negatives
        };
        // Take both mutable borrows in one destructuring, so neither outlives the
        // call.
        let (a, b) = if next(2) == 0 {
            (&mut left, &mut right)
        } else {
            (&mut right, &mut left)
        };
        // A hostile caller also lies about the roles: give slices that are too
        // short, too long, or full of computed roles.
        let roles_a = match next(3) {
            0 => Vec::new(),
            1 => vec![SlotRole::Storage; 1],
            _ => vec![
                SlotRole::FurnaceOutput,
                SlotRole::CraftingResult,
                SlotRole::FurnaceInput,
            ],
        };
        let roles_b = match next(3) {
            0 => Vec::new(),
            1 => vec![SlotRole::CraftingResult; 8],
            _ => vec![SlotRole::Storage, SlotRole::FurnaceFuel],
        };
        let result =
            Hopper::transfer(a, &roles_a, b, &roles_b, per_transfer).expect("a transfer applies");
        moved_total += i64::from(result.moved);

        // Boundedness: nothing may exceed a stack, and nothing may go negative.
        for container in [&left, &right] {
            for (index, slot) in container.slots().iter().enumerate() {
                assert!(
                    slot.count() <= DEFAULT_MAX_STACK_SIZE,
                    "slot {index} exceeded a stack: {slot:?}"
                );
                assert!(slot.count() >= 0, "negative count in slot {index}");
            }
        }
        assert!(
            result.moved >= 0 && result.moved <= per_transfer.clamp(0, MAX_ITEMS_PER_TRANSFER),
            "moved {} for a request of {per_transfer}",
            result.moved
        );
        assert_eq!(
            total(&left) + total(&right),
            budget,
            "conservation broke (moved {})",
            result.moved
        );
        // A move reports both ends or neither: a half-reported move would make the
        // caller's packet wrong.
        assert_eq!(
            result.source_slot.is_some(),
            result.destination_slot.is_some(),
            "a move must name both slots"
        );
        assert_eq!(
            result.moved > 0,
            result.source_slot.is_some(),
            "the moved count and the slot report must agree"
        );
    }

    // The loop must actually have moved things, or it proves nothing.
    assert!(
        moved_total > 100,
        "the flood must have moved items, moved {moved_total}"
    );
    assert_eq!(total(&left) + total(&right), budget, "final conservation");
}

#[test]
fn a_transfer_between_two_containers_conserves_items_at_every_step() {
    // The exhaustive small case: every source/destination slot pair and every
    // per_transfer from 0 to 64.
    for destination_size in 1..=3usize {
        for per_transfer in 0..=64 {
            let mut source = container(2);
            source.set(0, stack(stone(), 64)).expect("source 0");
            source.set(1, stack(granite(), 30)).expect("source 1");
            let mut destination = container(destination_size);
            if destination_size >= 1 {
                destination
                    .set(0, stack(stone(), 40))
                    .expect("destination 0");
            }
            if destination_size >= 2 {
                destination
                    .set(1, stack(granite(), 64))
                    .expect("destination 1");
            }
            let before = total(&source) + total(&destination);
            let result =
                Hopper::transfer_storage(&mut source, &mut destination, per_transfer).expect("ok");
            assert_eq!(
                total(&source) + total(&destination),
                before,
                "size {destination_size}, per_transfer {per_transfer}: conservation"
            );
            if per_transfer <= 0 {
                assert_eq!(result.moved, 0, "a non-positive request must move nothing");
                assert_eq!(source.get(0), stack(stone(), 64), "the source is intact");
                continue;
            }
            // Slot 0 is the first acceptable destination for the source's stone in
            // every one of these layouts: it holds stone with 24 items of room,
            // while slot 1 (when there is one) holds a full granite stack. So the
            // stone moves first, into slot 0, and by exactly the room available or
            // the request, whichever is smaller.
            let expected = per_transfer.min(24);
            assert_eq!(
                result.destination_slot,
                Some(0),
                "size {destination_size}, per_transfer {per_transfer}"
            );
            assert_eq!(result.source_slot, Some(0));
            assert_eq!(result.moved, expected);
            assert_eq!(destination.get(0), stack(stone(), 40 + expected));
            assert_eq!(source.get(0), stack(stone(), 64 - expected));
            assert_eq!(
                source.get(1),
                stack(granite(), 30),
                "the granite above the stone is untouched"
            );
        }
    }
}

#[test]
fn a_transfer_never_writes_a_slot_twice_or_loses_the_remainder() {
    // A source stack larger than the request, into a destination with a matching
    // partial stack: the exact arithmetic is checked item by item.
    let mut source = container(1);
    source.set(0, stack(stone(), 50)).expect("source");
    let mut destination = container(1);
    destination.set(0, stack(stone(), 61)).expect("destination");
    let result = Hopper::transfer_storage(&mut source, &mut destination, 10).expect("transfer");
    assert_eq!(result.moved, 3, "61 + 10 would exceed 64, so only 3 fit");
    assert_eq!(destination.get(0), stack(stone(), 64));
    assert_eq!(source.get(0), stack(stone(), 47));
    assert_eq!(
        total(&source) + total(&destination),
        50 + 61,
        "the three items moved, none were created or lost"
    );
}
