//! Block-entity lifecycle tests through the game loop (P06-07).
//!
//! `mc-container`'s unit tests prove the store's invariants against a bare map. This
//! file proves the *integration*: that a block changing on the server retires the
//! entity attached to it, and that the store the game owns is the one the tick path
//! touches. Without this, a chest's contents would outlive its block and reappear
//! when the same block was placed again.

use mc_container::{BlockEntityKind, BlockPos};
use mc_network::bridge::game_channel;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

fn game(tag: &str) -> (Game, WorldService, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = game_channel(64);
    let game = Game::new(&storage, 3, rx).expect("game builds");
    (game, storage, dir)
}

#[test]
fn a_placed_block_entity_is_retrievable_and_typed() {
    let (mut game, _storage, _dir) = game("p06-be-place");
    let pos = BlockPos::new(4, 64, 6);
    assert!(
        game.place_block_entity(pos, BlockEntityKind::Container)
            .is_none()
    );
    let store = game.block_entities();
    assert_eq!(store.len(), 1);
    let entity = store.get(pos).expect("the chest");
    assert_eq!(entity.kind(), BlockEntityKind::Container);
    assert_eq!(entity.data.items().expect("an inventory").len(), 27);
    assert!(store.is_well_formed());
}

#[test]
fn replacing_a_block_entity_reports_what_it_displaced() {
    let (mut game, _storage, _dir) = game("p06-be-replace");
    let pos = BlockPos::new(0, 64, 0);
    game.place_block_entity(pos, BlockEntityKind::Container);
    // Fill it, so losing it would lose items.
    if let Some(items) = game
        .block_entities_mut()
        .get_mut(pos)
        .expect("the chest")
        .data
        .items_mut()
    {
        items[0] = mc_entity::stack::ItemStack::new(1, 32).expect("stack");
    }
    assert_eq!(game.block_entities().total_items(), 32);
    let displaced = game
        .place_block_entity(pos, BlockEntityKind::Furnace)
        .expect("the chest it replaced");
    assert_eq!(displaced.kind(), BlockEntityKind::Container);
    assert_eq!(
        displaced.data.total_items(),
        32,
        "the displaced entity's contents must be recoverable, not silently dropped"
    );
    assert_eq!(game.block_entities().len(), 1, "one entity per position");
}

/// A12-07 kind-drift: `set_block` from chest to furnace must replace the
/// block-entity payload, not leave a 27-slot Container under a furnace.
#[test]
fn changing_a_chest_to_a_furnace_replaces_the_payload_kind() {
    let (mut game, _storage, _dir) = game("a12-07-kind-drift");
    let pos = BlockPos::new(4, 64, 4);
    let chest = game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest");
    let furnace = game
        .registries()
        .blocks
        .default_state("minecraft:furnace")
        .expect("furnace");
    let stone = game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");

    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, chest)
        .expect("place chest");
    game.tick().expect("tick creates the chest entity");
    let entity = game.block_entities().get(pos).expect("chest entity");
    assert_eq!(entity.kind(), BlockEntityKind::Container);
    if let Some(items) = game
        .block_entities_mut()
        .get_mut(pos)
        .expect("chest entity")
        .data
        .items_mut()
    {
        items[0] = mc_entity::stack::ItemStack::new(stone, 5).expect("stack");
    }

    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, furnace)
        .expect("replace with furnace");
    game.tick().expect("tick applies the kind change");

    let entity = game
        .block_entities()
        .get(pos)
        .expect("furnace entity after the swap");
    assert_eq!(
        entity.kind(),
        BlockEntityKind::Furnace,
        "a chest→furnace swap must replace the payload kind, not leave a Container"
    );
    assert!(
        matches!(entity.data, mc_container::BlockEntityData::Furnace { .. }),
        "the payload must be furnace-shaped, not a leftover slot list"
    );
    let items = entity.data.items().expect("furnace slots");
    assert_eq!(
        items.len(),
        3,
        "a furnace holds 3 slots, not the chest's 27"
    );
    assert!(
        items.iter().all(mc_entity::stack::ItemStack::is_empty),
        "the replaced chest's contents are dropped, not silently kept in the wrong shape"
    );
}

#[test]
fn breaking_the_block_retires_its_entity() {
    let (mut game, _storage, _dir) = game("p06-be-break");
    let pos = BlockPos::new(8, 64, 8);

    // Put a stone block in the world and a chest entity on it.
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = game.registries().blocks.air_id();
    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, stone)
        .expect("place the block");
    game.place_block_entity(pos, BlockEntityKind::Container);
    assert_eq!(game.block_entities().len(), 1);

    // Break the block through the world API, which is what the gameplay path does,
    // then tick so the broadcast phase sees the change.
    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, air)
        .expect("break the block");
    let report = game.tick().expect("tick");

    assert!(
        game.block_entities().get(pos).is_none(),
        "the block entity must not outlive its block"
    );
    assert_eq!(
        report.block_entities_changed, 1,
        "the report must say a block entity was retired"
    );
}

#[test]
fn an_entity_whose_block_still_exists_is_not_retired() {
    let (mut game, _storage, _dir) = game("p06-be-kept");
    let kept = BlockPos::new(2, 64, 2);
    let changed = BlockPos::new(3, 64, 2);
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let dirt = game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt");

    for pos in [kept, changed] {
        game.world_mut()
            .set_block(pos.x, pos.y, pos.z, stone)
            .expect("place");
        game.place_block_entity(pos, BlockEntityKind::Container);
    }

    // Drain the *placement* changes first. Placing a block is itself a change, so
    // without this the tick under test would legitimately see two changed positions
    // and retire both entities — which is correct behaviour, not the case this test
    // is about.
    let placed = game.tick().expect("tick");
    assert_eq!(
        placed.block_entities_changed, 2,
        "both placements are changes, so both entities are retired on the first tick"
    );
    // Re-place them, now that the world state is established.
    for pos in [kept, changed] {
        game.place_block_entity(pos, BlockEntityKind::Container);
    }

    // Change only one of the two blocks.
    game.world_mut()
        .set_block(changed.x, changed.y, changed.z, dirt)
        .expect("replace");
    let report = game.tick().expect("tick");

    assert!(
        game.block_entities().get(kept).is_some(),
        "an untouched block keeps its entity"
    );
    assert!(
        game.block_entities().get(changed).is_none(),
        "only the changed block's entity is retired"
    );
    assert_eq!(report.block_entities_changed, 1);
}

#[test]
fn retiring_an_entity_with_contents_is_reported_not_silent() {
    // The items are not yet dropped as entities (that needs per-item positions and is
    // P06-08's work). What this asserts is that the loss is *visible*: the report
    // counts the retirement, so it cannot happen with no trace at all.
    let (mut game, _storage, _dir) = game("p06-be-contents");
    let pos = BlockPos::new(1, 64, 1);
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = game.registries().blocks.air_id();
    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, stone)
        .expect("place");
    game.place_block_entity(pos, BlockEntityKind::Furnace);
    if let Some(items) = game
        .block_entities_mut()
        .get_mut(pos)
        .expect("the furnace")
        .data
        .items_mut()
    {
        items[0] = mc_entity::stack::ItemStack::new(1, 5).expect("stack");
    }
    assert_eq!(game.block_entities().total_items(), 5);

    game.world_mut()
        .set_block(pos.x, pos.y, pos.z, air)
        .expect("break");
    let report = game.tick().expect("tick");
    assert_eq!(report.block_entities_changed, 1);
    assert_eq!(
        game.block_entities().total_items(),
        0,
        "the store no longer counts the retired items"
    );
}

#[test]
fn a_sign_entity_round_trips_through_the_store() {
    let (mut game, _storage, _dir) = game("p06-be-sign");
    let pos = BlockPos::new(0, 70, 0);
    game.place_block_entity(pos, BlockEntityKind::Sign);
    if let Some(mc_container::BlockEntityData::Sign { lines }) =
        game.block_entities_mut().get_mut(pos).map(|e| &mut e.data)
    {
        lines[0] = "phase six".to_owned();
    }
    let entity = game.block_entities().get(pos).expect("the sign");
    match &entity.data {
        mc_container::BlockEntityData::Sign { lines } => assert_eq!(lines[0], "phase six"),
        other => panic!("expected a sign, got {other:?}"),
    }
    assert_eq!(entity.data.total_items(), 0);
}

/// P12-05: a chest's contents survive a restart through the chunk save.
///
/// Builds two games on the same directory (the only honest restart): the
/// first places a chest block, fills it, saves; the second loads the chunk
/// and must find the same items. Proves the payload↔NBT path, not the encoder
/// agreeing with itself.
#[test]
fn a_chest_survives_a_restart_with_its_contents() {
    use mc_network::bridge::game_channel;
    use mc_server::config::StorageConfig;
    use mc_server::game::{DEFAULT_RANDOM_SEED, Game};

    let dir = TempDir::new("p12-be-restart");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let stone_item = {
        let storage = WorldService::open(&config).expect("world opens");
        let (_tx, rx) = game_channel(64);
        let mut first =
            Game::with_seed_and_storage(storage, 3, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        let (bx, by, bz) = first.spawn();
        let chest = first
            .registries()
            .blocks
            .default_state("minecraft:chest")
            .expect("chest");
        first
            .world_mut()
            .set_block(bx + 1, by, bz, chest)
            .expect("place chest");
        // Run a tick so the broadcast creates the block entity for the placed
        // chest (placing queues, broadcast retires-into-existence).
        first.tick().expect("tick");
        let pos = BlockPos::new(bx + 1, by, bz);
        assert!(
            first.block_entities().get(pos).is_some(),
            "the placed chest must have an entity before filling"
        );
        let stone = first
            .registries()
            .items
            .id("minecraft:stone")
            .expect("stone");
        if let Some(items) = first
            .block_entities_mut()
            .get_mut(pos)
            .and_then(|e| e.data.items_mut())
        {
            items[0] = mc_entity::stack::ItemStack::new(stone, 17).expect("stack");
        }
        // Mark dirty is automatic on set_block; the entity write-back also
        // marks, so the save must include this chunk.
        first.save_all_owned().expect("saves");
        first.close_storage().expect("closes");
        (
            stone,
            mc_persistence::chunk::ChunkPos::new((bx + 1) >> 4, bz >> 4),
        )
    };

    let (stone, chunk) = stone_item;
    let storage = WorldService::open(&config).expect("world reopens");
    let (_tx, rx) = game_channel(64);
    let mut second =
        Game::with_seed_and_storage(storage, 3, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    assert!(second.load_chunk(chunk), "the saved chunk loads");
    let total: i64 = second.block_entities().total_items();
    assert_eq!(total, 17, "the chest contents must survive, saw {total}");
    let entity = second
        .block_entities()
        .iter()
        .find(|e| e.kind() == BlockEntityKind::Container)
        .expect("a chest entity");
    let first_stack = entity.data.items().expect("items")[0].clone();
    assert_eq!(first_stack.item_id(), Some(stone));
    assert_eq!(first_stack.count(), 17);
}
