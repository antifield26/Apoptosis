//! Differential loot test: the **shipped** 26.1.2 pack rolled through the real
//! game path — the test whose absence let the P11-04 loot defect through.
//!
//! `loot_and_pickup.rs` proves the wiring with hand-written fixture tables; this
//! file is the counterpart its module docs once promised as `vanilla_loot.rs`
//! and never delivered. The defect it exists to catch: `mc-data::loot::roll`
//! refuses a whole table when any construct in it is unmodelled, and the real
//! pack contains such constructs in most tables — so a bare-handed stone break
//! produced **nothing** where vanilla produces cobblestone. Measured census
//! (`target/loot_condition_census.py`, 1 326 tables): 167 tables were
//! enchantment-gated on their own (rescued by the owner's fix 1: a bare hand is
//! a *known, unenchanted* tool, `enchantment_levels: Some(empty map)`), and a
//! further set carries constructs this build still refuses outright
//! (`block_state_property` 149, `entity_properties` 27, …), which is the fix-2
//! follow-up (pool-level refusal) the owner also approved.
//!
//! # Scope of this file after fix 1
//!
//! - `blocks/stone` (an `alternatives` whose first child is gated on a
//!   silk-touch `match_tool`) must yield **cobblestone** — the smallest
//!   experiment that failed before the fix.
//! - `blocks/grass_block` must yield **dirt** (a table with no gating at all).
//! - **No assertion pins a defect.** In particular the cow's `entity_properties`
//!   table is *not* asserted to drop nothing: that is the fix-2 work, and a test
//!   asserting today's broken behaviour would make the fix harder.
//!
//! It is `#[ignore]`d because it needs `MC_VANILLA_DATA` — the **pack root**
//! holding `data/minecraft/` (the convention `packs.rs` documents; the untracked
//! extraction lives at `target/vanilla-26.1.2/extract`, see
//! `docs/research/data-pack-baseline.md`). The path must be **absolute**:
//! `cargo test` runs a test binary with its working directory at the *package*
//! (AUDIT-09 D-06). One run:
//!
//! ```text
//! set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract
//! cargo test -p mc-server --test vanilla_loot -- --ignored --nocapture
//! ```

use mc_data::enabled::EnabledPacks;
use mc_entity::player::GameMode;
use mc_entity::stack::ItemStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_persistence::chunk::ChunkPos;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::packs::{PackRoots, load_packs};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// The extracted pack root (`target/vanilla-26.1.2/extract`), or `None` unset.
fn pack_root() -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(
        root.is_dir(),
        "MC_VANILLA_DATA points at {}, which is not a directory; \
         it must be the pack root holding data/minecraft",
        root.display()
    );
    assert!(
        root.join("data").is_dir(),
        "MC_VANILLA_DATA points at {}, which has no data/ directory; point it at the pack \
         root (the directory containing data/minecraft), not at data/minecraft itself",
        root.display()
    );
    Some(root)
}

/// Everything a test needs: a live game with the **real** pack loaded.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let Some(pack) = pack_root() else {
            panic!("this test is #[ignore]d and needs MC_VANILLA_DATA");
        };
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        // Owned storage, or no terrain is ever generated (see natural_spawn.rs).
        let mut game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        // The real loader, through the same `load_packs` the server boots with.
        let roots = PackRoots::new(dir.path().join("world")).with_vanilla_data(&pack);
        let outcome = load_packs(&mut game, &roots, &EnabledPacks::default()).expect("packs load");
        assert!(
            outcome.vanilla_data,
            "the vanilla pack must be found at {}",
            pack.display()
        );
        assert!(
            outcome.loot_tables_loaded > 1_000,
            "the shipped pack loads most of its tables; got {}",
            outcome.loot_tables_loaded
        );
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

    /// The block-space position of the player's feet.
    // `floor` has already made the value integral; the narrowing is the
    // conversion itself.
    #[allow(clippy::cast_possible_truncation)]
    fn feet(&self) -> (i32, i32, i32) {
        let position = self.game.player(self.id).expect("player").position;
        (
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        )
    }

    /// Put one `block` at `(x, y, z)`, loading the chunk first and proving the
    /// placement reads back through the accessor the dig path uses.
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

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn item_id(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }
}

/// Drain the queued packet ids.
fn drain_ids(out: &mut InboundReceiver) -> Vec<i32> {
    let mut ids = Vec::new();
    while let Some(raw) = out.try_recv() {
        ids.push(raw.id);
    }
    ids
}

/// The stacks on the ground.
fn ground(harness: &Harness) -> Vec<ItemStack> {
    harness
        .game
        .dropped_items()
        .into_iter()
        .map(|(stack, _)| stack)
        .collect()
}

/// The one behaviour the landing's missing test would have caught: a bare hand
/// against the **shipped** stone table yields the table's cobblestone child.
#[test]
#[ignore = "needs MC_VANILLA_DATA (absolute pack root); see the module docs"]
fn a_bare_handed_break_of_shipped_stone_yields_shipped_cobblestone() {
    let mut harness = Harness::new("p11-vanilla-loot-stone");
    let mut out = harness.join("Miner");
    let _ = drain_ids(&mut out);
    assert_eq!(
        harness.game.player(harness.id).expect("player").game_mode,
        GameMode::Survival,
        "a joining player is in survival"
    );

    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    let stone = harness.place(target.0, target.1, target.2, "minecraft:stone");
    harness.dig(target.0, target.1, target.2);
    assert_eq!(
        harness
            .game
            .world()
            .get_block_loaded(target.0, target.1, target.2),
        Some(harness.game.registries().blocks.air_id()),
        "the dig cleared the block"
    );
    let _ = stone;

    let cobblestone = harness.item_id("minecraft:cobblestone");
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        1,
        "the shipped stone table rolls exactly one stack; saw {drops:?}"
    );
    assert_eq!(
        drops[0].item_id(),
        Some(cobblestone),
        "a bare hand breaks shipped stone into shipped cobblestone, not nothing"
    );
    assert_eq!(drops[0].count(), 1);
}

/// A shipped table with no gating at all drops through unconditionally.
#[test]
#[ignore = "needs MC_VANILLA_DATA (absolute pack root); see the module docs"]
fn a_shipped_ungated_table_drops_without_a_tool() {
    let mut harness = Harness::new("p11-vanilla-loot-grass");
    harness.join("Miner");
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:dirt");
    harness.dig(target.0, target.1, target.2);

    let dirt = harness.item_id("minecraft:dirt");
    let drops = ground(&harness);
    assert_eq!(
        drops.len(),
        1,
        "the shipped dirt table rolls exactly one stack; saw {drops:?}"
    );
    assert_eq!(
        drops[0].item_id(),
        Some(dirt),
        "the shipped dirt table names dirt and needs no tool"
    );
}

/// The shipped cow table, killed by player swings: the ladder's cow case.
///
/// The shipped table's second entry carries a `furnace_smelt` function whose
/// conditions include an unevaluable `entity_properties` term; under the
/// refusal ladder that **function** is skipped (raw beef, which is correct for
/// a mob that was not on fire) while the leather pool rolls beside it. Before
/// both fixes this table refused whole and a dead cow dropped nothing.
#[test]
#[ignore = "needs MC_VANILLA_DATA (absolute pack root); see the module docs"]
fn a_swing_kills_a_shipped_cow_and_its_table_drops() {
    let mut harness = Harness::new("p11-vanilla-loot-cow");
    harness.join("Hunter");
    let (sx, sy, sz) = harness.game.spawn();
    let at = mc_world::Vec3::new(f64::from(sx) + 2.5, f64::from(sy), f64::from(sz) + 0.5);
    let cow = harness
        .game
        .spawn_mob(mc_entity::mob::MobKind::Cow, at)
        .expect("the cow spawns");

    // The fist deals 1.0 a swing and the mob holds a 10-tick window, so the
    // kill takes 10 swings spaced past the window.
    for _ in 0..12 {
        harness.intent(PlayIntent::Interact {
            entity: cow.get(),
            kind: 1,
        });
        harness.run(11);
    }
    let drops = ground(&harness);
    assert!(
        !drops.is_empty(),
        "a dead shipped cow drops what its table's executable pools say, not nothing"
    );
    let names: Vec<String> = drops
        .iter()
        .map(|stack| {
            let id = stack.item_id().expect("a live stack names an item");
            harness
                .game
                .registries()
                .items
                .name(id)
                .unwrap_or("?")
                .to_owned()
        })
        .collect();
    for name in &names {
        assert!(
            name.contains("leather") || name.contains("beef"),
            "the shipped cow table names leather and beef; saw {names:?}"
        );
    }
}
