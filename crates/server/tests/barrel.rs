//! Barrel windows and saves end to end (P17-02 Step A, barrel half).
//!
//! Through the real tick loop and the real region files: opening a barrel
//! shows the "Barrel" title on the chest-shape window, and the saved chunk
//! names the entity `minecraft:barrel` (not `minecraft:chest`), so a
//! vanilla server reading our save finds the right block entity. A restart
//! round-trip keeps the contents.
//!
//! Falsification shape: saving under the chest id fails the on-disk id
//! assertion while the in-memory restart still passes — the defect was
//! vanilla-interop, not self-round-trip.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{MENU_GENERIC_9X3, OpenScreen, PlayIntent, block_position};
use mc_protocol::text::TextComponent;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            dir,
        }
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        out
    }

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn give(&mut self, item: &str) {
        let id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(id, 1).expect("stack"))
            .expect("slot 0 takes it");
    }

    fn empty_hand(&mut self) {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::EMPTY)
            .expect("hand emptied");
    }

    fn place(&mut self, item: &str, x: i32, y: i32, z: i32, face: i32) {
        self.give(item);
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }

    fn click(&mut self, x: i32, y: i32, z: i32, face: i32) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }

    fn name_at(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("named")
            .to_owned()
    }
}

fn floor(harness: &mut Harness, sx: i32, sy: i32, sz: i32) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    for x in (sx - 4)..=(sx + 6) {
        for z in (sz - 2)..=(sz + 2) {
            harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
            harness
                .game
                .world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
            for y in [sy, sy + 1, sy + 2] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, air)
                    .expect("cleared");
            }
        }
    }
}

#[test]
fn opening_a_barrel_shows_the_barrel_title() {
    let mut harness = Harness::new("p17-barrel-ui");
    let mut out = harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    let (bx, by, bz) = (sx + 1, sy, sz);
    harness.place("minecraft:barrel", bx, by - 1, bz, 1);
    assert_eq!(harness.name_at(bx, by, bz), "minecraft:barrel");
    harness.empty_hand();
    harness.click(bx, by, bz, 1);
    let mut screens = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::OPEN_SCREEN {
            screens.push(OpenScreen::decode(&raw.payload).expect("decodes"));
        }
    }
    assert_eq!(screens.len(), 1, "one window opens");
    assert_eq!(screens[0].menu_type, MENU_GENERIC_9X3);
    assert_eq!(
        screens[0].title,
        TextComponent::literal("Barrel"),
        "a barrel is not a chest"
    );
}

/// The saved bytes name the barrel, not a chest.
#[test]
fn a_barrel_saves_under_its_own_id() {
    let mut harness = Harness::new("p17-barrel-save");
    harness.join("Hands");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    let (bx, by, bz) = (sx + 1, sy, sz);
    harness.place("minecraft:barrel", bx, by - 1, bz, 1);
    harness.run(2);
    // Fill white-box (the chest tests fill the same way).
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");
    {
        let pos = mc_container::BlockPos::new(bx, by, bz);
        let entity = harness
            .game
            .block_entities_mut()
            .get_mut(pos)
            .expect("barrel entity exists from placement");
        let items = entity.data.items_mut().expect("barrel items");
        items[0] = mc_entity::stack::ItemStack::new(stone, 17).expect("stack");
    }
    harness.game.save_all_owned().expect("saves");
    harness.game.close_storage().expect("closes");

    // Read the saved chunk back through the storage layer (decoded NBT, not
    // the encoder's memory) and find the entity compound at the barrel.
    let mut storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: harness.dir.path().join("world"),
        autosave_ticks: 0,
    })
    .expect("world reopens");
    let chunk_pos = mc_persistence::chunk::ChunkPos::new(bx >> 4, bz >> 4);
    let stored = storage
        .storage_mut()
        .read_chunk(&mc_persistence::Dimension::Overworld, chunk_pos)
        .expect("chunk reads")
        .expect("the barrel chunk saved");
    let mut found: Option<String> = None;
    for tag in &stored.block_entities {
        let mc_nbt::NbtTag::Compound(fields) = tag else {
            continue;
        };
        let get = |name: &str| fields.iter().find(|(key, _)| key == name).map(|(_, v)| v);
        let at = matches!(
            (get("x"), get("y"), get("z")),
            (
                Some(mc_nbt::NbtTag::Int(x)),
                Some(mc_nbt::NbtTag::Int(y)),
                Some(mc_nbt::NbtTag::Int(z)),
            ) if *x == bx && *y == by && *z == bz
        );
        if at && let Some(mc_nbt::NbtTag::String(id)) = get("id") {
            assert!(found.is_none(), "the barrel saved exactly once");
            found = Some(id.clone());
        }
    }
    assert_eq!(
        found.as_deref(),
        Some("minecraft:barrel"),
        "the barrel saves under its own id, saw {found:?}"
    );
}

#[test]
fn a_barrel_survives_a_restart_with_its_contents() {
    use mc_server::config::StorageConfig;
    use mc_server::game::{DEFAULT_RANDOM_SEED, Game};

    let dir = TempDir::new("p17-barrel-restart");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let (chunk, stone_item) = {
        let storage = WorldService::open(&config).expect("world opens");
        let (_tx, rx) = game_channel(64);
        let mut first =
            Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        let (sx, sy, sz) = first.spawn();
        let stone = first
            .registries()
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        let air = first.registries().blocks.air_id();
        for x in (sx - 4)..=(sx + 6) {
            first.load_chunk(ChunkPos::new(x >> 4, sz >> 4));
            first
                .world_mut()
                .set_block(x, sy - 1, sz, stone)
                .expect("floor");
            first.world_mut().set_block(x, sy, sz, air).expect("clear");
        }
        let (bx, by, bz) = (sx + 1, sy, sz);
        let barrel = first
            .registries()
            .blocks
            .default_state("minecraft:barrel")
            .expect("barrel");
        first
            .world_mut()
            .set_block(bx, by, bz, barrel)
            .expect("place barrel");
        first.tick().expect("tick");
        let pos = mc_container::BlockPos::new(bx, by, bz);
        assert!(
            first.block_entities().get(pos).is_some(),
            "the placed barrel must have an entity before filling"
        );
        let stone_id = first
            .registries()
            .items
            .id("minecraft:stone")
            .expect("stone");
        if let Some(items) = first
            .block_entities_mut()
            .get_mut(pos)
            .and_then(|e| e.data.items_mut())
        {
            items[0] = mc_entity::stack::ItemStack::new(stone_id, 17).expect("stack");
        }
        first.save_all_owned().expect("saves");
        first.close_storage().expect("closes");
        (
            mc_persistence::chunk::ChunkPos::new(bx >> 4, bz >> 4),
            stone_id,
        )
    };

    let storage = WorldService::open(&config).expect("world reopens");
    let (_tx, rx) = game_channel(64);
    let mut second =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    assert!(second.load_chunk(chunk), "the saved chunk loads");
    let total: i64 = second.block_entities().total_items();
    assert_eq!(total, 17, "the barrel contents must survive, saw {total}");
    let entity = second
        .block_entities()
        .iter()
        .find(|e| e.kind() == mc_container::BlockEntityKind::Container)
        .expect("a barrel entity");
    let first_stack = entity.data.items().expect("items")[0];
    assert_eq!(first_stack.item_id(), Some(stone_item));
    assert_eq!(first_stack.count(), 17);
}
