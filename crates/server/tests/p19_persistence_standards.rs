//! AUDIT-19 persistence standards at the boundary an operator actually sees: a
//! stored chunk whose component patch is awkward, and the log line a save emits
//! when it leaves chunks out.
//!
//! The unit pins live next to the readers (`mc-entity::components`): the read
//! rule for B19-4 (an absent field an older writer could omit reads the
//! pre-strict default; a field that is present and unreadable is refused with a
//! message naming the component, the field and the shape found) and the one
//! policy for B19-5 (keep the stack, report the reason). What those cannot
//! prove is that the *chunk* reader — `load_chunk_block_entities` and
//! `load_chunk_entities` in `crate::game::persist` — uses them, and that the
//! reasons reach the log rather than being dropped at the call site.
//!
//! Each test writes a real chunk through `WorldService`, rewrites one item's
//! `components` compound the way a foreign or hand-edited file can, and boots a
//! game over it. The warnings are read out of a `tracing` subscriber installed
//! for the test thread, which is where the game's own tick and load run.
//!
//! Falsification shape: make `saturation` required again and
//! `a_pre_strict_food_patch_loads_with_its_historical_saturation` fails; make
//! `read_inventory` propagate the decoder error again and the playerdata test in
//! `mc-entity` fails; drop the reason at either call site — or put the retired
//! skip sentence back — and the captured-log assertions below fail.

// Exact `f32` component fields are compared by value here — the saturation a
// pre-strict food patch reads back — as `dig_progress.rs` does.
#![allow(clippy::float_cmp)]

use mc_container::BlockPos;
use mc_entity::components::{DataComponent, ItemComponents};
use mc_entity::stack::ItemStack;
use mc_nbt::NbtTag;
use mc_network::bridge::game_channel;
use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::dimension::Dimension;
use mc_server::config::StorageConfig;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// A world directory that outlives every game opened on it.
struct World {
    _dir: TempDir,
    config: StorageConfig,
}

impl World {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        Self { _dir: dir, config }
    }

    /// A game that owns its storage, so it can read and write real chunks.
    fn game(&self) -> Game {
        let storage = WorldService::open(&self.config).expect("world opens");
        let (_tx, rx) = game_channel(64);
        Game::with_seed_and_storage(storage, 3, rx, DEFAULT_RANDOM_SEED).expect("game builds")
    }

    /// Rewrite the stored chunk with `edit` applied to its NBT.
    fn rewrite_chunk(&self, pos: ChunkPos, edit: impl FnOnce(&mut ChunkData)) {
        let mut storage = WorldService::open(&self.config).expect("world reopens");
        let mut data = storage
            .storage_mut()
            .read_chunk(&Dimension::Overworld, pos)
            .expect("the chunk reads")
            .expect("the chunk is stored");
        edit(&mut data);
        storage
            .storage_mut()
            .queue_chunk_save(&Dimension::Overworld, &data)
            .expect("the rewritten chunk queues");
        storage.storage_mut().flush().expect("it is on disk");
        storage.close().expect("the handle closes");
    }
}

/// A `tracing` sink the test thread owns.
///
/// `tracing::subscriber::with_default` is thread-local and the game runs on this
/// thread, so a warning the load emits can be asserted on without touching the
/// process-global subscriber (which parallel tests share).
#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

impl Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("the log lock is not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogSink {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("the log lock is not poisoned")).into_owned()
    }
}

/// Run `body` with every `tracing` event raised on this thread captured.
fn capture_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
    let sink = LogSink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let value = tracing::subscriber::with_default(subscriber, body);
    (value, sink.text())
}

/// The patch a foreign writer (or a hand edit) can produce: a modelled
/// component whose field is present and is not the shape it must be.
fn unreadable_patch() -> NbtTag {
    NbtTag::compound([(
        "minecraft:food".to_owned(),
        NbtTag::compound([
            ("nutrition".to_owned(), NbtTag::Int(4)),
            ("saturation".to_owned(), NbtTag::String("lots".to_owned())),
        ]),
    )])
}

/// The patch a **pre-strict** save can carry: `minecraft:food` with the field
/// the P18 round made required omitted, which the reader of the day defaulted.
fn pre_strict_food_patch() -> NbtTag {
    NbtTag::compound([(
        "minecraft:food".to_owned(),
        NbtTag::compound([("nutrition".to_owned(), NbtTag::Int(5))]),
    )])
}

/// First block entity with an `Items` list gets its first item's patch replaced.
fn plant_block_item_patch(entities: &mut [NbtTag], patch: NbtTag) -> bool {
    for tag in entities.iter_mut() {
        let Some(items) = tag.get_list("Items") else {
            continue;
        };
        let mut items = items.to_vec();
        let Some(first) = items.first_mut() else {
            continue;
        };
        first.insert("components", patch);
        tag.insert("Items", NbtTag::List(items));
        return true;
    }
    false
}

/// First item entity's `Item` compound gets its patch replaced.
fn plant_drop_patch(entities: &mut [NbtTag], patch: NbtTag) -> bool {
    for tag in entities.iter_mut() {
        let Some(item) = tag.get("Item") else {
            continue;
        };
        let mut item = item.clone();
        item.insert("components", patch);
        tag.insert("Item", item);
        return true;
    }
    false
}

/// Boot a world, place a chest next to the spawn point, put `stack` in slot 0,
/// save and close. Returns the chest position and the chunk holding it.
fn world_with_a_stored_chest(world: &World, stack: ItemStack) -> (BlockPos, ChunkPos) {
    let mut game = world.game();
    let (bx, by, bz) = game.spawn();
    let chest = game
        .registries()
        .blocks
        .default_state("minecraft:chest")
        .expect("chest");
    game.world_mut()
        .set_block(bx + 1, by, bz, chest)
        .expect("the chest is placed");
    // The broadcast phase turns the placed block into a block entity.
    game.tick().expect("tick");
    let pos = BlockPos::new(bx + 1, by, bz);
    let items = game
        .block_entities_mut()
        .get_mut(pos)
        .and_then(|entity| entity.data.items_mut())
        .expect("the placed chest has 27 slots");
    items[0] = stack;
    game.save_all_owned().expect("the world saves");
    game.close_storage().expect("the storage handle closes");
    (pos, ChunkPos::new((bx + 1) >> 4, bz >> 4))
}

/// An all-air placeholder's sibling: a chunk whose `DataVersion` is outside the
/// readable window, so the loader fails to read it without storage being at
/// fault (the audit's E-5 shape).
fn unreadable_chunk(pos: ChunkPos) -> ChunkData {
    ChunkData {
        pos,
        data_version: 5000, // above `DATA_VERSION_26_1_2` (4790)
        status: "minecraft:full".to_owned(),
        min_section_y: -4,
        last_update: 0,
        inhabited_time: 0,
        light_correct: false,
        sections: vec![mc_persistence::chunk::SectionData::filled(
            4,
            mc_persistence::chunk::BlockState::new("minecraft:stone"),
            "minecraft:plains",
        )],
        heightmaps: Vec::new(),
        block_entities: Vec::new(),
        entities: Vec::new(),
        block_ticks: Vec::new(),
        fluid_ticks: Vec::new(),
        post_processing: Vec::new(),
        structures: None,
        extra: Vec::new(),
    }
}

#[test]
fn a_stored_block_item_keeps_its_slot_when_its_patch_cannot_be_read() {
    // AUDIT-19 B19-5, block-entity half: the outcome **and** the message. This
    // arm warned and kept a component-free stack while the playerdata arm
    // refused the whole file; both use `attach_saved_components` now, so the
    // stack, its count and the slot survive a patch this build cannot read, and
    // the warning names the component rather than the stack.
    let world = World::new("p19-b19-5-chest");
    let stone = {
        let game = world.game();
        game.registries()
            .items
            .id("minecraft:stone")
            .expect("stone")
    };
    let mut components = ItemComponents::new();
    components.set(DataComponent::Damage(5));
    let damaged = ItemStack::with_components(stone, 17, components).expect("a damaged stack");
    let (pos, chunk) = world_with_a_stored_chest(&world, damaged);

    world.rewrite_chunk(chunk, |data| {
        assert!(
            plant_block_item_patch(&mut data.block_entities, unreadable_patch()),
            "the stored chest had no item to rewrite, so this test would prove nothing"
        );
    });

    let (game, logs) = capture_logs(|| {
        let mut game = world.game();
        assert!(game.load_chunk(chunk), "the chunk loads");
        game
    });
    let items = game
        .block_entities()
        .get(pos)
        .expect("the chest came back")
        .data
        .items()
        .expect("27 slots");
    assert_eq!(
        items[0].item_id(),
        Some(stone),
        "one unreadable patch must not empty the slot"
    );
    assert_eq!(items[0].count(), 17, "nor lose the count");
    assert!(
        items[0].components().is_empty(),
        "the unreadable patch is dropped whole rather than half-applied"
    );
    assert!(
        logs.contains("a saved block item's component patch is unreadable"),
        "the loss is reported at the call site: {logs}"
    );
    assert!(
        logs.contains("minecraft:food.saturation"),
        "and the report names the component and the field: {logs}"
    );
}

#[test]
fn a_stored_drop_keeps_its_stack_when_its_patch_cannot_be_read() {
    // The same policy on the entity arm: the drop still spawns. Refusing here
    // (the playerdata arm's old answer) would delete a dropped stack from the
    // world over one component.
    let world = World::new("p19-b19-5-drop");
    let diamond = {
        let game = world.game();
        game.registries()
            .items
            .id("minecraft:diamond")
            .expect("diamond")
    };
    let mut game = world.game();
    let (bx, by, bz) = game.spawn();
    let at = mc_world::Vec3::new(
        f64::from(bx) + 5.5,
        f64::from(by) + 4.0,
        f64::from(bz) + 0.5,
    );
    game.spawn_item(ItemStack::new(diamond, 7).expect("a stack"), at)
        .expect("the drop spawns");
    // A block change in the same chunk makes it dirty, and only a dirty chunk
    // is saved (`load_or_create_chunk`'s clean-marking).
    let stone = game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    game.world_mut()
        .set_block(bx, by + 4, bz, stone)
        .expect("the marker block is set");
    game.save_all_owned().expect("the world saves");
    game.close_storage().expect("the storage handle closes");
    drop(game);

    let chunk = ChunkPos::new(bx >> 4, bz >> 4);
    world.rewrite_chunk(chunk, |data| {
        assert!(
            plant_drop_patch(&mut data.entities, unreadable_patch()),
            "the stored chunk had no item entity to rewrite"
        );
    });

    let (second, logs) = capture_logs(|| {
        let mut second = world.game();
        assert!(second.load_chunk(chunk), "the saved chunk loads");
        second
    });
    let drops = second.dropped_items();
    assert_eq!(
        drops.len(),
        1,
        "the drop survived its patch; saw {}",
        drops.len()
    );
    assert_eq!(drops[0].0.item_id(), Some(diamond));
    assert_eq!(drops[0].0.count(), 7, "with its count intact");
    assert!(
        drops[0].0.components().is_empty(),
        "and without the patch it could not read"
    );
    assert!(
        logs.contains("a saved item's component patch is unreadable"),
        "the entity arm reports it too: {logs}"
    );
    assert!(
        logs.contains("minecraft:food.saturation"),
        "with the component named: {logs}"
    );
}

#[test]
fn a_pre_strict_food_patch_loads_with_its_historical_saturation() {
    // AUDIT-19 B19-4, end to end. A save written before the strictness round
    // can carry `minecraft:food` without `saturation`; requiring it — which is
    // what the P18 round did — refuses the server's own history. It reads the
    // value that round used (0.0) instead, and the item stays food.
    let world = World::new("p19-b19-4-food");
    let bread = {
        let game = world.game();
        game.registries()
            .items
            .id("minecraft:bread")
            .expect("bread")
    };
    let (pos, chunk) = world_with_a_stored_chest(&world, ItemStack::new(bread, 3).expect("bread"));

    world.rewrite_chunk(chunk, |data| {
        assert!(
            plant_block_item_patch(&mut data.block_entities, pre_strict_food_patch()),
            "the stored chest had no item to rewrite"
        );
    });

    let mut game = world.game();
    assert!(game.load_chunk(chunk), "the chunk loads");
    let items = game
        .block_entities()
        .get(pos)
        .expect("the chest came back")
        .data
        .items()
        .expect("27 slots");
    assert_eq!(items[0].item_id(), Some(bread));
    assert_eq!(items[0].count(), 3);
    let food = items[0]
        .food()
        .expect("a pre-strict food patch still reads as food, not as an unknown");
    assert_eq!(food.nutrition, 5);
    assert_eq!(
        food.saturation, 0.0,
        "the saturation this crate read before it was made required"
    );
}

#[test]
fn the_skip_warning_names_both_reasons_a_placeholder_is_not_saved() {
    // AUDIT-19 D-19-L2. The skip warning is the operator's only statement of why
    // a save left chunks out, and it used to assert one cause ("this game cannot
    // read storage") for a set that a readable-storage game also joins whenever
    // a stored chunk fails to read — the audit's E-5 probe produced its
    // `skipped=1` exactly that way, and this test reproduces that shape: the
    // planted chunk's `DataVersion` is outside the readable window, storage is
    // fine, and the save skips the placeholder.
    let dir = TempDir::new("p19-d19-l2");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let (bx, by, bz) = {
        let storage = WorldService::open(&config).expect("world opens");
        let probe = Game::new(&storage, 2, game_channel(4).1).expect("probe builds");
        probe.spawn()
    };
    let pos = ChunkPos::new(bx >> 4, bz >> 4);
    {
        let mut storage = WorldService::open(&config).expect("world opens");
        storage
            .storage_mut()
            .queue_chunk_save(&Dimension::Overworld, &unreadable_chunk(pos))
            .expect("the planted chunk queues");
        storage.storage_mut().flush().expect("it is on disk");
        storage.close().expect("the handle closes");
    }

    let storage = WorldService::open(&config).expect("world reopens");
    let (_tx, rx) = game_channel(64);
    let (game, logs) = capture_logs(move || {
        let mut game =
            Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        assert!(
            game.load_chunk(pos),
            "the chunk loads as a placeholder, as it does for a failed read"
        );
        let stone = game
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        game.world_mut()
            .set_block(bx, by, bz, stone)
            .expect("the edit makes the placeholder dirty");
        game.save_all_owned().expect("the save runs");
        game.close_storage().expect("the storage handle closes");
        game
    });
    drop(game);

    assert!(
        logs.contains("placeholder chunks were not saved"),
        "the skip is reported: {logs}"
    );
    assert!(
        logs.contains("no storage handle"),
        "and the first producer is named: {logs}"
    );
    assert!(
        logs.contains("failed to load"),
        "and the second producer — the one this scenario is: {logs}"
    );
    assert!(
        !logs.contains("cannot read storage"),
        "the retired attribution (false whenever storage is readable) is gone: {logs}"
    );
}
