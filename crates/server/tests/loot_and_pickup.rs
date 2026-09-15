//! P11-04/P11-05/P11-06/P11-09 end to end: the loot table is the drop
//! authority, and what it drops can be merged, picked up and killed for.
//!
//! These tests drive the real [`Game`] tick loop through the same channel the
//! network layer uses. The loot tables come from a **fixture pack written to
//! disk and read by the real loader** (`mc_data::loot::load_directory`, the same
//! call `packs.rs` makes), not from a hand-built `LootTables`: a hand-built
//! registry would skip the file-name-to-table-name step that P11-04 depends on
//! (`loot_table/blocks/stone.json` must be reached as `minecraft:blocks/stone`).
//!
//! ## Why the fixture tables name the *wrong* item on purpose
//!
//! The fixture's `blocks/stone` drops **cobblestone** and its `entities/chicken`
//! drops **feathers**. If the drop path were hard-coded — break a block, drop the
//! block; kill a mob, drop its `MobKind` default — these tests would still pass,
//! because stone *does* drop cobblestone in vanilla and the names would be
//! coincidence. They are not coincidence here: the fixture is the only place
//! those names appear, and the assertion is on the fixture's item. A change that
//! replaced the table lookup with a hard-coded mapping fails
//! `a_survival_break_drops_what_the_loot_table_says`.
//!
//! A **differential** counterpart does not exist yet, and its absence is what let a
//! real defect through: these fixtures are hand-written, so they prove the wiring
//! and cannot see how the *shipped* tables interact with this crate's roll rules.
//! They do interact badly — see the measurement and the two candidate fixes in
//! `docs/audits/AUDIT-09-REMEDIATION.md` §"open divergences" and the Phase 11
//! CHANGELOG entry. The counterpart that would have caught it loads the real
//! `data/minecraft` pack and asserts that a bare-handed stone break yields
//! **cobblestone** (`blocks/stone` is an `alternatives` whose first child is gated on
//! a silk-touch `match_tool`), which is the smallest experiment that fails today.
//!
//! ## What these do not prove
//!
//! That a real client renders the drops (P11-10), that the loot *contents* match
//! vanilla (that is the differential test), and that vanilla's exact merge radius
//! is half a block (the simplification is named in `game.rs` at
//! `ITEM_MERGE_RADIUS_SQR`).

// `feet()` floors a finite player position to a block coordinate, which is the
// narrowing that conversion is: the world is far inside `i32` range and `floor`
// has already made the value integral.
#![allow(clippy::cast_possible_truncation)]

use mc_data::Limits;
use mc_data::loot::{LootLoadReport, LootTables, load_directory};
use mc_entity::player::GameMode;
use mc_entity::stack::ItemStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

/// The tables this fixture pack ships, as `(path under the namespace dir, JSON)`.
///
/// Minimal but **vanilla-shaped**: the discriminator keys are the ones the real
/// files use (`type` for a table and an entry, `rolls` as a JSON number), so a
/// loader that only copes with this fixture would still be reading the format.
/// `blocks/stone` -> cobblestone and `entities/chicken` -> 2 feathers are the
/// deliberate mismatches described in the module docs.
const FIXTURE_PACK: &[(&str, &str)] = &[
    (
        "loot_table/blocks/stone.json",
        r#"{
            "type": "minecraft:block",
            "pools": [
                {
                    "rolls": 1.0,
                    "entries": [
                        { "type": "minecraft:item", "name": "minecraft:cobblestone" }
                    ]
                }
            ]
        }"#,
    ),
    (
        "loot_table/entities/chicken.json",
        r#"{
            "type": "minecraft:entity",
            "pools": [
                {
                    "rolls": 1.0,
                    "entries": [
                        {
                            "type": "minecraft:item",
                            "name": "minecraft:feather",
                            "functions": [
                                {
                                    "function": "minecraft:set_count",
                                    "count": 2
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#,
    ),
];

/// Write the fixture pack under `root` and return the namespace directory the
/// loader is pointed at.
fn write_fixture_pack(root: &std::path::Path) -> std::path::PathBuf {
    let namespace = root.join("minecraft");
    for (relative, contents) in FIXTURE_PACK {
        let path = namespace.join(relative);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("the pack directory is writable");
        std::fs::write(&path, contents).expect("the fixture table is written");
    }
    namespace
}

/// Everything a test needs: a live game with the fixture loot pack loaded.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let namespace = write_fixture_pack(&dir.path().join("pack"));
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        // The game must own its storage, or it never generates terrain and every
        // chunk is an all-air placeholder (see natural_spawn.rs).
        let mut game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");

        // The real loader, so the file-name -> table-name step is exercised.
        let mut report = LootLoadReport::default();
        let loaded = load_directory(
            &namespace,
            "minecraft",
            Limits::DEFAULT,
            Some(&game.registries().items),
            Some(&game.registries().blocks),
            &mut report,
        );
        assert!(
            report.skipped.is_empty(),
            "the fixture pack must load cleanly; the loader refused: {:?}",
            report.skipped
        );
        assert_eq!(
            loaded.len(),
            FIXTURE_PACK.len(),
            "every fixture file became a table"
        );
        let mut tables = LootTables::new();
        for table in loaded {
            tables.insert(table);
        }
        game.set_loot(tables);
        assert_eq!(game.loot().len(), FIXTURE_PACK.len());

        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    /// Join a player and run the tick that applies the join.
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

    /// The block-space position of the player's feet.
    fn feet(&self) -> (i32, i32, i32) {
        let position = self.game.player(self.id).expect("player").position;
        (
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        )
    }

    /// Put one `block` at exactly `(x, y, z)`, loading the chunk first and proving
    /// the block reads back through the **same** accessor the dig path uses
    /// (`get_block_loaded`). A placement that silently did not happen would
    /// otherwise look exactly like a break that dropped nothing.
    fn place(&mut self, x: i32, y: i32, z: i32, block: &str) -> i32 {
        assert!(
            self.game.load_chunk(ChunkPos::new(x >> 4, z >> 4)),
            "the chunk at ({x}, {z}) loads"
        );
        let id = self
            .game
            .registries()
            .blocks
            .default_state(block)
            .unwrap_or_else(|_| panic!("{block} is a known block"));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("the block is set");
        assert_eq!(
            self.game.world().get_block_loaded(x, y, z),
            Some(id),
            "the placed block reads back as {block}"
        );
        id
    }

    /// Break the block at `(x, y, z)` the way a client does: `player_action`
    /// status 0 (start digging), which this build treats as an instant break.
    fn dig(&mut self, x: i32, y: i32, z: i32) {
        self.intent(PlayIntent::PlayerAction {
            status: 0,
            position: block_position(x, y, z),
            facing: 1,
            sequence: 0,
        });
    }

    /// The player's inventory item count.
    fn inventory_total(&self) -> i64 {
        self.game
            .session(self.id)
            .expect("a session")
            .inventory_total()
    }

    fn item_id(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }

    fn set_game_mode(&mut self, mode: GameMode) {
        self.game.player_mut(self.id).expect("player").game_mode = mode;
    }

    /// Drain the queued packet ids.
    fn drain_ids(out: &mut InboundReceiver) -> Vec<i32> {
        let mut ids = Vec::new();
        while let Some(raw) = out.try_recv() {
            ids.push(raw.id);
        }
        ids
    }
}

/// The stacks on the ground, as `(item name, count)` is not available here, so
/// this returns the entity stacks for a test to compare ids and counts against.
fn ground(harness: &Harness) -> Vec<ItemStack> {
    harness
        .game
        .dropped_items()
        .into_iter()
        .map(|(stack, _)| stack)
        .collect()
}

#[test]
fn a_survival_break_drops_what_the_loot_table_says() {
    let mut harness = Harness::new("p11-loot-break");
    let mut out = harness.join("Miner");
    // Clear the join burst so the packet assertion below is about the drop.
    let _ = Harness::drain_ids(&mut out);
    assert_eq!(
        harness.game.player(harness.id).expect("player").game_mode,
        GameMode::Survival,
        "a joining player is in survival, or the drop half of this test is not being run"
    );

    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:stone");
    harness.dig(target.0, target.1, target.2);

    assert_eq!(
        harness
            .game
            .world()
            .get_block_loaded(target.0, target.1, target.2),
        Some(harness.game.registries().blocks.air_id()),
        "the dig cleared the block"
    );

    let cobblestone = harness.item_id("minecraft:cobblestone");
    let stone = harness.item_id("minecraft:stone");
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        1,
        "one table roll produced one ground stack; saw {drops:?}"
    );
    assert_eq!(
        drops[0].item_id(),
        Some(cobblestone),
        "the drop is the *table's* item, not the broken block's"
    );
    assert_ne!(
        drops[0].item_id(),
        Some(stone),
        "a block-echoing drop path would name stone here; the fixture table names cobblestone"
    );
    assert_eq!(drops[0].count(), 1);

    // The client half: a drop nobody is told about is a drop that does not exist
    // in play.
    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::ADD_ENTITY),
        "the drop is announced with add_entity; saw {ids:?}"
    );
    assert!(
        ids.contains(&clientbound::play::SET_ENTITY_DATA),
        "and given its stack by set_entity_data, or the client draws a bare entity"
    );
}

#[test]
fn a_creative_break_drops_nothing() {
    let mut harness = Harness::new("p11-loot-creative");
    harness.join("Architect");
    harness.set_game_mode(GameMode::Creative);

    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:stone");
    harness.dig(target.0, target.1, target.2);

    assert_eq!(
        harness
            .game
            .world()
            .get_block_loaded(target.0, target.1, target.2),
        Some(harness.game.registries().blocks.air_id()),
        "the block is still broken in creative"
    );
    assert_eq!(
        ground(&harness).len(),
        0,
        "creative breaking consumes the block without rolling its table"
    );
}

#[test]
fn a_block_with_no_loot_table_drops_nothing() {
    let mut harness = Harness::new("p11-loot-no-table");
    harness.join("Digger");

    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    // Dirt is deliberately absent from the fixture pack: "no table" is a
    // different code path from "a table that rolled nothing", and only a block
    // the pack does not mention can show it.
    harness.place(target.0, target.1, target.2, "minecraft:dirt");
    assert!(
        harness
            .game
            .loot()
            .by_name(&mc_core::ids::ResourceId::parse("minecraft:blocks/dirt").expect("an id"))
            .is_none(),
        "the fixture pack must not ship a dirt table, or this test proves nothing"
    );
    harness.dig(target.0, target.1, target.2);

    assert_eq!(
        harness
            .game
            .world()
            .get_block_loaded(target.0, target.1, target.2),
        Some(harness.game.registries().blocks.air_id()),
        "the block was still broken"
    );
    assert_eq!(
        ground(&harness).len(),
        0,
        "a block with no table drops nothing, which is vanilla's own rule"
    );
}

#[test]
fn a_mob_death_rolls_its_entity_loot_table() {
    let mut harness = Harness::new("p11-loot-mob");
    harness.join("Hunter");

    // Two and a half blocks away, and no further: a swing has to be a swing, and
    // at this distance the drop that lands is outside the one-block pickup
    // radius, so the assertion below cannot be satisfied by an item that was
    // immediately collected.
    let (fx, fy, fz) = harness.feet();
    let at = mc_entity::player::Vec3::new(f64::from(fx) + 2.5, f64::from(fy), f64::from(fz) + 0.5);
    let chicken = harness
        .game
        .spawn_mob(mc_entity::MobKind::Chicken, at)
        .expect("the chicken spawns");

    // A chicken has 4 health and the fist does 1.0, so four swings land it — but
    // a second swing inside the 10-tick hurt window is refused, so the swings
    // have to be spaced. The loop is bounded and reports which half failed.
    let mut swings = 0;
    let mut killed = false;
    for _ in 0..20 {
        harness.intent(PlayIntent::Interact {
            entity: chicken.get(),
            kind: 1,
        });
        swings += 1;
        if harness.game.entity_store().get(chicken).is_none() {
            killed = true;
            break;
        }
        // Outlast the hurt window (and its decrement cadence) before swinging
        // again, so this measures the kill and not the invulnerability.
        harness.run(12);
    }
    assert!(
        killed,
        "the chicken died within 20 spaced swings (used {swings}); a refusal here is the \
         hurt window or the damage amount, not the loot table"
    );

    // Asserted with no further ticks: the drop's own 10-tick pickup delay has not
    // elapsed, so this is the ground state the death produced.
    let feather = harness.item_id("minecraft:feather");
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        1,
        "one entity-table roll, one ground stack; saw {drops:?}"
    );
    assert_eq!(
        drops[0].item_id(),
        Some(feather),
        "the drop comes from minecraft:entities/chicken, not from a hard-coded per-kind table"
    );
    assert_eq!(
        drops[0].count(),
        2,
        "the fixture's set_count function is part of the roll, not decoration"
    );
}

#[test]
fn a_ready_stack_within_reach_is_picked_up_and_the_entity_is_swept() {
    let mut harness = Harness::new("p11-pickup");
    let mut out = harness.join("Collector");
    let _ = Harness::drain_ids(&mut out);

    let before = harness.inventory_total();
    let stone = harness.item_id("minecraft:stone");
    let position = harness.game.player(harness.id).expect("player").position;
    let at = mc_world::Vec3::new(position.x, position.y, position.z);
    let entity = harness
        .game
        .spawn_item(ItemStack::new(stone, 3).expect("stone"), at)
        .expect("the drop spawns");
    assert_eq!(
        ground(&harness).len(),
        1,
        "the drop is on the ground before the delay elapses"
    );

    // The 10-tick pickup delay is part of the rule: a stack dropped this instant
    // must not be collectable yet, or a player could pick up their own drop
    // before the client has even drawn it.
    harness.run(5);
    assert_eq!(
        harness.inventory_total(),
        before,
        "five ticks is inside the 10-tick pickup delay"
    );
    assert!(
        harness.game.entity_store().get(entity).is_some(),
        "and the entity is still there"
    );

    harness.run(10);
    assert_eq!(
        harness.inventory_total(),
        before + 3,
        "the whole stack (all three) moved into the inventory"
    );
    assert!(
        harness.game.entity_store().get(entity).is_none(),
        "the collected entity was removed from the store"
    );
    assert_eq!(
        ground(&harness).len(),
        0,
        "and nothing is left on the ground"
    );

    let ids = Harness::drain_ids(&mut out);
    assert!(
        ids.contains(&clientbound::play::REMOVE_ENTITIES),
        "the client is told to remove the collected entity; saw {ids:?}"
    );
    assert!(
        ids.contains(&clientbound::play::CONTAINER_SET_SLOT)
            || ids.contains(&clientbound::play::CONTAINER_SET_CONTENT),
        "and to put the stack in the slot, or the client shows an empty inventory"
    );
}

#[test]
fn two_nearby_stacks_merge_into_the_older_entity() {
    let mut harness = Harness::new("p11-merge");
    harness.join("Merger");

    // Far from the player on purpose: within a block the pickup pass would take
    // both before the assertion could tell a merge from a collection, and the
    // merge pass runs *before* the pickup pass in the same tick.
    let (fx, fy, fz) = harness.feet();
    let stone = harness.item_id("minecraft:stone");
    let base = mc_world::Vec3::new(
        f64::from(fx) + 30.0,
        f64::from(fy) + 5.0,
        f64::from(fz) + 0.5,
    );
    let older = harness
        .game
        .spawn_item(ItemStack::new(stone, 3).expect("stone"), base)
        .expect("the first drop spawns");
    let younger = harness
        .game
        .spawn_item(
            ItemStack::new(stone, 4).expect("stone"),
            mc_world::Vec3::new(base.x + 0.3, base.y, base.z),
        )
        .expect("the second drop spawns");
    let ids_before: Vec<i32> = harness
        .game
        .dropped_items()
        .iter()
        .map(|(stack, _)| stack.count())
        .collect();
    assert_eq!(ids_before, vec![3, 4], "both stacks start on the ground");

    harness.run(1);
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        1,
        "same item within half a block becomes one entity; saw {drops:?}"
    );
    assert_eq!(drops[0].count(), 7, "the counts are summed, not replaced");
    assert!(
        harness.game.entity_store().get(older).is_some(),
        "the older entity (lower id, equal age) is the one that survives"
    );
    assert!(
        harness.game.entity_store().get(younger).is_none(),
        "and the younger one is the one that was swept"
    );
}

#[test]
fn stacks_beyond_the_merge_radius_and_of_other_items_are_left_alone() {
    let mut harness = Harness::new("p11-merge-negative");
    harness.join("Fence");

    let (fx, fy, fz) = harness.feet();
    let stone = harness.item_id("minecraft:stone");
    let dirt = harness.item_id("minecraft:dirt");
    let base = mc_world::Vec3::new(
        f64::from(fx) + 30.0,
        f64::from(fy) + 5.0,
        f64::from(fz) + 0.5,
    );
    // 0.8 apart: outside the half-block merge radius, so a merge pass with the
    // wrong comparison (`<` vs `>`, or a radius off by a factor) is visible here
    // rather than only as an item that vanished somewhere.
    harness
        .game
        .spawn_item(ItemStack::new(stone, 3).expect("stone"), base)
        .expect("the first drop spawns");
    harness
        .game
        .spawn_item(
            ItemStack::new(stone, 4).expect("stone"),
            mc_world::Vec3::new(base.x + 0.8, base.y, base.z),
        )
        .expect("the second drop spawns");
    // Different item, well inside the radius: merging by distance alone would
    // join these, which would destroy one of them.
    harness
        .game
        .spawn_item(
            ItemStack::new(dirt, 5).expect("dirt"),
            mc_world::Vec3::new(base.x, base.y, base.z),
        )
        .expect("the third drop spawns");

    harness.run(1);
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        3,
        "0.8 apart does not merge, and a different item never does; saw {drops:?}"
    );
    let stone_total: i32 = drops
        .iter()
        .filter(|stack| stack.item_id() == Some(stone))
        .map(ItemStack::count)
        .sum();
    assert_eq!(stone_total, 7, "both stone stacks kept every item");
}

#[test]
fn a_full_inventory_leaves_the_leftover_on_the_ground() {
    let mut harness = Harness::new("p11-pickup-full");
    harness.join("Hoarder");

    // Fill every stored slot with a full stack of stone, then drop more.
    let stone = harness.item_id("minecraft:stone");
    let slots = harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .stored_slots();
    for slot in 0..slots {
        harness
            .game
            .player_mut(harness.id)
            .expect("player")
            .inventory
            .set_slot(slot, ItemStack::new(stone, 64).expect("stone"))
            .expect("a full stack fits the slot");
    }
    let before = harness.inventory_total();
    let slots = i64::try_from(slots).expect("a slot count fits i64");
    assert_eq!(before, 64 * slots, "the inventory starts full");

    let position = harness.game.player(harness.id).expect("player").position;
    harness
        .game
        .spawn_item(
            ItemStack::new(stone, 5).expect("stone"),
            mc_world::Vec3::new(position.x, position.y, position.z),
        )
        .expect("the drop spawns");
    harness.run(15);

    assert_eq!(
        harness.inventory_total(),
        before,
        "a full inventory cannot absorb the stack"
    );
    let drops = ground(&harness);
    assert_eq!(drops.len(), 1, "so it stays on the ground; saw {drops:?}");
    assert_eq!(
        drops[0].count(),
        5,
        "with every item it had: a refusal must not delete the stack"
    );
}
