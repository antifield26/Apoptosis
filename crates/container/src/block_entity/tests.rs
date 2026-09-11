//! Block-entity model tests (P06-07).
//!
//! The properties worth testing here are the ones that make the store safe to keep
//! in sync with the world: a payload always matches its kind, iteration is
//! deterministic, replacing an entity hands back what it displaced (so contents
//! cannot vanish silently), and a leaked entry is *findable* rather than invisible.

use mc_entity::stack::ItemStack;

use super::{
    BlockEntity, BlockEntityData, BlockEntityKind, BlockEntityStore, BlockPos, inventory_of,
};

const STONE: i32 = 1;

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count).expect("a valid stack")
}

#[test]
fn a_new_entity_has_an_empty_payload_of_its_own_kind() {
    for kind in [
        BlockEntityKind::Container,
        BlockEntityKind::Furnace,
        BlockEntityKind::Hopper,
        BlockEntityKind::Sign,
    ] {
        let entity = BlockEntity::new(BlockPos::new(1, 2, 3), kind);
        assert_eq!(entity.kind(), kind, "{kind} kind");
        assert!(entity.data.is_well_formed(), "{kind} payload shape");
        assert_eq!(entity.data.kind(), kind);
        assert!(!kind.name().is_empty());
        if kind.has_inventory() {
            let items = entity.data.items().expect("an inventory");
            assert_eq!(items.len(), kind.slot_count(), "{kind} slot count");
            assert!(items.iter().all(ItemStack::is_empty));
            assert_eq!(entity.data.total_items(), 0);
        } else {
            assert!(entity.data.items().is_none());
        }
    }
    // Slot counts are the documented ones.
    assert_eq!(BlockEntityKind::Container.slot_count(), 27);
    assert_eq!(BlockEntityKind::Furnace.slot_count(), 3);
    assert_eq!(BlockEntityKind::Hopper.slot_count(), 5);
    assert_eq!(BlockEntityKind::Sign.slot_count(), 0);
    assert!(!BlockEntityKind::Sign.has_inventory());
}

#[test]
fn a_sign_holds_four_lines_and_no_items() {
    let mut entity = BlockEntity::new(BlockPos::new(0, 64, 0), BlockEntityKind::Sign);
    match &mut entity.data {
        BlockEntityData::Sign { lines } => {
            assert_eq!(lines.len(), 4);
            lines[0] = "hello".to_owned();
        }
        other => panic!("expected a sign payload, got {other:?}"),
    }
    assert_eq!(entity.data.total_items(), 0);
    assert!(entity.data.items_mut().is_none(), "a sign has no slot list");
    assert!(entity.data.is_well_formed());
    // A sign with no inventory cannot be opened as one.
    assert!(inventory_of(&entity).is_err());
}

#[test]
fn the_store_keeps_positions_unique_and_reports_what_it_displaced() {
    let mut store = BlockEntityStore::new();
    let pos = BlockPos::new(4, 64, 8);
    assert!(
        store
            .insert(BlockEntity::new(pos, BlockEntityKind::Container))
            .is_none()
    );
    // Give the chest contents.
    if let Some(items) = store.get_mut(pos).expect("the chest").data.items_mut() {
        items[0] = stack(STONE, 32);
    }
    assert_eq!(store.len(), 1);
    assert_eq!(store.total_items(), 32);

    // Replacing it must hand the old one back: silently dropping it would destroy
    // 32 items with nothing to notice.
    let displaced = store
        .insert(BlockEntity::new(pos, BlockEntityKind::Furnace))
        .expect("the chest it replaced");
    assert_eq!(displaced.kind(), BlockEntityKind::Container);
    assert_eq!(
        displaced.data.total_items(),
        32,
        "the contents are recoverable"
    );
    assert_eq!(store.len(), 1, "one entity per position");
    assert_eq!(store.total_items(), 0, "the furnace is empty");
}

#[test]
fn iteration_is_ascending_by_position() {
    let mut store = BlockEntityStore::new();
    for (x, z) in [(5, 0), (0, 0), (0, 5), (5, 5), (-1, 0)] {
        store.insert(BlockEntity::new(
            BlockPos::new(x, 64, z),
            BlockEntityKind::Container,
        ));
    }
    let positions: Vec<BlockPos> = store.positions().collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted, "iteration order must be deterministic");
    // And filtering keeps the order.
    let containers = store.of_kind(BlockEntityKind::Container);
    assert_eq!(containers, positions);
    assert!(store.of_kind(BlockEntityKind::Furnace).is_empty());
}

#[test]
fn removal_returns_the_entity_and_is_idempotent() {
    let mut store = BlockEntityStore::new();
    let pos = BlockPos::new(1, 2, 3);
    store.insert(BlockEntity::new(pos, BlockEntityKind::Hopper));
    assert!(store.get(pos).is_some());
    let removed = store.remove(pos).expect("the hopper");
    assert_eq!(removed.pos, pos);
    assert!(store.remove(pos).is_none());
    assert!(store.is_empty());
}

#[test]
fn the_audit_finds_leaked_entities_and_prune_clears_them() {
    // Build a store where three chests exist but only two are still chests in the
    // world: the third's block was broken and nobody removed the entity.
    let mut store = BlockEntityStore::new();
    let kept = [BlockPos::new(0, 64, 0), BlockPos::new(16, 64, 0)];
    let leaked = BlockPos::new(32, 64, 0);
    for pos in kept.iter().chain(std::iter::once(&leaked)) {
        store.insert(BlockEntity::new(*pos, BlockEntityKind::Container));
    }
    assert_eq!(store.len(), 3);

    let still_a_chest =
        |pos: BlockPos, kind: BlockEntityKind| kind == BlockEntityKind::Container && pos != leaked;
    let stale = store.audit_against(still_a_chest);
    assert_eq!(stale, vec![leaked], "the audit must find exactly the leak");
    assert_eq!(store.len(), 3, "auditing does not mutate");

    let pruned = store.prune(still_a_chest);
    assert_eq!(pruned, vec![leaked]);
    assert_eq!(store.len(), 2);
    assert!(
        store.prune(still_a_chest).is_empty(),
        "pruning is idempotent"
    );
}

#[test]
fn a_hand_built_payload_with_the_wrong_slot_count_is_refused() {
    // A store built through `BlockEntity::new` cannot produce this, so the check
    // exists for a payload assembled by hand (a future NBT load).
    let mut store = BlockEntityStore::new();
    let pos = BlockPos::new(0, 64, 0);
    store.insert(BlockEntity {
        pos,
        data: BlockEntityData::Items(vec![ItemStack::EMPTY; 3]),
    });
    assert!(!store.is_well_formed());
    let entity = store.get(pos).expect("the entity");
    let error = inventory_of(entity).expect_err("a 3-slot container must be refused");
    assert!(
        format!("{error}").contains("3 slots"),
        "the error must name the mismatch: {error}"
    );
    // The store still reports its totals rather than panicking.
    assert_eq!(store.total_items(), 0);
}

#[test]
fn totals_sum_across_every_inventory() {
    let mut store = BlockEntityStore::new();
    for (index, kind) in [
        BlockEntityKind::Container,
        BlockEntityKind::Furnace,
        BlockEntityKind::Hopper,
        BlockEntityKind::Sign,
    ]
    .into_iter()
    .enumerate()
    {
        let pos = BlockPos::new(index as i32 * 16, 64, 0);
        store.insert(BlockEntity::new(pos, kind));
        if let Some(items) = store.get_mut(pos).expect("the entity").data.items_mut() {
            items[0] = stack(STONE, 10);
        }
    }
    // Three inventories of 10; the sign contributes nothing.
    assert_eq!(store.total_items(), 30);
    assert!(store.is_well_formed());
}

#[test]
fn chunk_columns_are_computed_by_arithmetic_shift() {
    // Negative coordinates must floor, not truncate toward zero.
    assert_eq!(BlockPos::new(0, 0, 0).chunk(), (0, 0));
    assert_eq!(BlockPos::new(15, 0, 15).chunk(), (0, 0));
    assert_eq!(BlockPos::new(16, 0, -1).chunk(), (1, -1));
    assert_eq!(BlockPos::new(-1, 0, -16).chunk(), (-1, -1));
    assert_eq!(BlockPos::new(-17, 0, 0).chunk(), (-2, 0));
    assert_eq!(BlockPos::new(3, 64, 5).to_string(), "(3, 64, 5)");
}

#[test]
fn clearing_empties_the_store() {
    let mut store = BlockEntityStore::new();
    store.insert(BlockEntity::new(
        BlockPos::new(0, 64, 0),
        BlockEntityKind::Container,
    ));
    store.clear();
    assert!(store.is_empty());
    assert_eq!(store.total_items(), 0);
    assert!(store.is_well_formed(), "an empty store is well formed");
}
