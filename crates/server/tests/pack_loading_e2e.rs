//! Data-pack loading end to end (P07-12).
//!
//! `enabled.rs` proves the matching rules and `pack_discovery.rs` proves the discovery, but both
//! stop at the `DataPackSet`. This proves the whole chain: `level.dat`'s `DataPacks` list decides
//! which packs load, a loaded pack's `.mcfunction` reaches `Game`, and `/function` runs it.
//!
//! That chain is the reason the enabled-list work was worth doing: before it, nothing on the server
//! loaded a pack at all, so the list gated nothing and `/function` could only run functions a test
//! had injected by hand.
//!
//! Every assertion reads the reply text. This project has found **five** assertions that could not
//! fail — the last one in this very area, where a check-ordering test passed with the checks
//! swapped — so "the command was answered" is never treated as evidence.

use mc_data::enabled::EnabledPacks;
use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::game::Game;
use mc_server::packs::{PackRoots, load_packs};
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::time::Duration;

/// Write a pack with one function under `<world>/datapacks/<name>/`.
fn write_pack(world: &std::path::Path, name: &str, function: &str, body: &str) {
    let dir = world
        .join("datapacks")
        .join(name)
        .join("data")
        .join("testns")
        .join("function");
    std::fs::create_dir_all(&dir).expect("function dir");
    std::fs::write(dir.join(format!("{function}.mcfunction")), body).expect("write");
}

/// A world with packs, a server, and a logged-in client.
struct Harness {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    _dir: TempDir,
}

impl Harness {
    async fn start(tag: &str, enabled: &EnabledPacks, packs: &[(&str, &str, &str)]) -> Self {
        let dir = TempDir::new(tag);
        let world = dir.path().join("world");
        std::fs::create_dir_all(&world).expect("world dir");
        for (name, function, body) in packs {
            write_pack(&world, name, function, body);
        }

        let storage = WorldService::open(&mc_server::config::StorageConfig {
            world_dir: world.clone(),
            autosave_ticks: 0,
        })
        .expect("world opens");
        let (event_tx, event_rx) = game_channel(256);
        let mut game = Game::new(&storage, 3, event_rx).expect("game builds");

        // The same call the lifecycle makes.
        let roots = PackRoots::new(&world);
        load_packs(&mut game, &roots, enabled).expect("packs load");

        let settings = mc_network::NetworkSettings {
            bind: "127.0.0.1:0".parse().expect("addr"),
            view_distance: 3,
            ..mc_network::NetworkSettings::default()
        };
        let link = mc_network::listener::GameLink::new(event_tx, 4096);
        let service = mc_network::NetworkService::start_with_game(settings, None, Some(link))
            .await
            .expect("listener starts");
        let addr = service.local_addr();
        let (client, _join) = TestClient::login_join(addr, "Caller")
            .await
            .expect("login completes");
        for _ in 0..40 {
            game.tick().expect("tick");
            if game.player_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Self {
            client,
            game,
            service,
            _dir: dir,
        }
    }

    async fn run(&mut self, text: &str) -> Vec<String> {
        let bytes = text.as_bytes();
        let mut payload = varint(i32::try_from(bytes.len()).expect("fits"));
        payload.extend_from_slice(bytes);
        self.client
            .send_raw_packet(&RawPacket::new(serverbound::play::CHAT_COMMAND, payload))
            .await
            .expect("command sent");

        let mut lines = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(900);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(80), self.client.recv()).await {
                Ok(Ok(raw)) if raw.id == clientbound::play::DISGUISED_CHAT => {
                    match SystemChat::decode(&raw.payload) {
                        Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                        Err(error) => panic!("a system_chat must decode: {error}"),
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    self.game.tick().expect("tick");
                }
            }
        }
        lines
    }
}

/// Minimal `VarInt` encoder for the test's own packets.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn varint(value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut value = value as u32;
    loop {
        if value & !0x7F == 0 {
            out.push(value as u8);
            return out;
        }
        out.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
}

#[tokio::test]
async fn an_enabled_packs_function_reaches_the_command() {
    // The whole chain, end to end: a file on disk, discovered, filtered by the enabled list,
    // loaded into the game, and run by `/function` with its output visible to a client.
    let enabled = EnabledPacks::from_level_dat(&["vanilla".to_owned(), "mypack".to_owned()], &[]);
    let mut harness = Harness::start(
        "packs-enabled",
        &enabled,
        &[("mypack", "greet", "say FROM_THE_PACK\n")],
    )
    .await;

    let lines = harness.run("function testns:greet").await;
    assert!(
        lines.iter().any(|line| line.contains("FROM_THE_PACK")),
        "a function from an enabled pack must run: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Ran 1 command")),
        "and be reported: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_disabled_packs_function_is_not_loadable() {
    // **The property the enabled list exists for.** The file is on disk and perfectly valid; the
    // world does not list its pack, so it must not be runnable. If this passes with the pack
    // enabled, the filter does nothing.
    let enabled = EnabledPacks::from_level_dat(&["vanilla".to_owned()], &[]);
    let mut harness = Harness::start(
        "packs-disabled",
        &enabled,
        &[("mypack", "greet", "say FROM_THE_PACK\n")],
    )
    .await;

    let lines = harness.run("function testns:greet").await;
    assert!(
        !lines.iter().any(|line| line.contains("FROM_THE_PACK")),
        "a function from a pack the world does not enable must NOT run: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Unknown function")),
        "and must be reported as unknown: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_world_with_no_pack_list_runs_its_packs_functions() {
    // **The regression that matters most.** A world whose `level.dat` predates `DataPacks`, or was
    // written by another tool, has no list — and reading that as "nothing enabled" would silently
    // disable every pack in a world that has been running for months.
    let mut harness = Harness::start(
        "packs-nolist",
        &EnabledPacks::default(),
        &[("mypack", "greet", "say FROM_THE_PACK\n")],
    )
    .await;

    let lines = harness.run("function testns:greet").await;
    assert!(
        lines.iter().any(|line| line.contains("FROM_THE_PACK")),
        "with no list, every pack loads: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_explicitly_disabled_pack_does_not_load() {
    let enabled = EnabledPacks::from_level_dat(
        &["vanilla".to_owned(), "mypack".to_owned()],
        &["mypack".to_owned()],
    );
    let mut harness = Harness::start(
        "packs-explicit-off",
        &enabled,
        &[("mypack", "greet", "say FROM_THE_PACK\n")],
    )
    .await;

    let lines = harness.run("function testns:greet").await;
    assert!(
        !lines.iter().any(|line| line.contains("FROM_THE_PACK")),
        "disabling wins over enabling: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn two_packs_both_contribute_functions() {
    let enabled = EnabledPacks::default();
    let mut harness = Harness::start(
        "packs-two",
        &enabled,
        &[
            ("alpha", "a", "say FROM_ALPHA\n"),
            ("beta", "b", "say FROM_BETA\n"),
        ],
    )
    .await;

    let a = harness.run("function testns:a").await;
    assert!(a.iter().any(|line| line.contains("FROM_ALPHA")), "{a:?}");
    let b = harness.run("function testns:b").await;
    assert!(b.iter().any(|line| line.contains("FROM_BETA")), "{b:?}");
    harness.service.shutdown().await;
}

#[test]
fn the_loader_reports_what_it_loaded_and_what_it_left_out() {
    // The outcome is what the lifecycle logs, so it has to be accurate — and it is the only place
    // "my pack is not loading" is answerable.
    let dir = TempDir::new("packs-outcome");
    let world = dir.path().join("world");
    std::fs::create_dir_all(&world).expect("world dir");
    write_pack(&world, "enabled_pack", "one", "say ONE\n");
    write_pack(&world, "disabled_pack", "two", "say TWO\n");
    // A directory that is not a pack at all.
    std::fs::create_dir_all(world.join("datapacks").join("junk")).expect("junk");

    let storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world.clone(),
        autosave_ticks: 0,
    })
    .expect("opens");
    let mut game = Game::new(&storage, 3, game_channel(64).1).expect("game");

    let enabled =
        EnabledPacks::from_level_dat(&["vanilla".to_owned(), "enabled_pack".to_owned()], &[]);
    let outcome = load_packs(&mut game, &PackRoots::new(&world), &enabled).expect("loads");

    assert_eq!(outcome.packs_loaded, 1, "{outcome:?}");
    assert_eq!(outcome.functions_loaded, 1, "{outcome:?}");
    assert!(!outcome.vanilla_data, "no vanilla data was configured");
    assert_eq!(outcome.skipped.len(), 1, "the disabled pack: {outcome:?}");
    assert!(outcome.skipped[0].contains("disabled_pack"));
    assert_eq!(outcome.rejected.len(), 1, "the junk directory: {outcome:?}");
    assert!(
        outcome.rejected[0].contains("no data/ directory"),
        "{outcome:?}"
    );
    assert!(outcome.has_problems(), "a rejection is a problem");
    assert!(
        outcome.summary().contains("unreadable"),
        "{}",
        outcome.summary()
    );
}

#[test]
fn a_world_pack_overrides_a_vanilla_function_of_the_same_name() {
    // Load order exists so a world pack can replace a vanilla one. The mechanism is
    // `FunctionRegistry::insert` being last-wins over the load plan, and this asserts it rather
    // than assuming it.
    let dir = TempDir::new("packs-override");
    let world = dir.path().join("world");
    std::fs::create_dir_all(&world).expect("world dir");

    // A fake "vanilla" pack with the same namespace and function name.
    let vanilla = dir.path().join("vanilla");
    let vanilla_fn = vanilla.join("data").join("testns").join("function");
    std::fs::create_dir_all(&vanilla_fn).expect("vanilla function dir");
    std::fs::write(vanilla_fn.join("shared.mcfunction"), "say FROM_VANILLA\n").expect("write");

    // A world pack that overrides it.
    write_pack(&world, "override", "shared", "say FROM_WORLD\n");

    let storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world.clone(),
        autosave_ticks: 0,
    })
    .expect("opens");
    let mut game = Game::new(&storage, 3, game_channel(64).1).expect("game");

    let roots = PackRoots::new(&world).with_vanilla_data(&vanilla);
    let outcome = load_packs(&mut game, &roots, &EnabledPacks::default()).expect("loads");
    assert_eq!(outcome.packs_loaded, 2, "{outcome:?}");
    assert!(outcome.vanilla_data);

    let name = mc_core::ids::ResourceId::parse("testns:shared").expect("valid");
    let function = game
        .functions()
        .by_name(&name)
        .expect("the function exists");
    let commands = function.command_list().join(" ");
    assert!(
        commands.contains("FROM_WORLD"),
        "the world pack must win: {commands:?}"
    );
    assert!(
        !commands.contains("FROM_VANILLA"),
        "and the vanilla version must be replaced: {commands:?}"
    );
}

#[test]
fn a_configured_vanilla_data_path_that_does_not_exist_is_reported() {
    // The symptom of a wrong path is "every vanilla function is unknown", which is identical to
    // having configured nothing — so the distinction is recorded.
    let dir = TempDir::new("packs-bad-vanilla");
    let world = dir.path().join("world");
    std::fs::create_dir_all(&world).expect("world dir");

    let storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world.clone(),
        autosave_ticks: 0,
    })
    .expect("opens");
    let mut game = Game::new(&storage, 3, game_channel(64).1).expect("game");

    let roots = PackRoots::new(&world).with_vanilla_data(dir.path().join("nowhere"));
    let outcome = load_packs(&mut game, &roots, &EnabledPacks::default()).expect("does not fail");
    assert!(!outcome.vanilla_data);
    assert_eq!(outcome.rejected.len(), 1, "{outcome:?}");
    assert!(
        outcome.rejected[0].contains("does not exist"),
        "{outcome:?}"
    );
}

/// A world pack's `recipe/` files reach the crafting table through `load_packs`.
///
/// Regression for the double-join defect (`root.join(namespace)` built
/// `.../data/<ns>/<ns>`, which never exists, so every boot silently loaded
/// zero recipes and kept the baseline). A shapeless stone→cobblestone recipe
/// must load, convert, and be matchable — if the join regresses,
/// `recipes_loaded` is 0 and the table lacks the test recipe.
#[test]
fn a_world_pack_recipe_reaches_the_crafting_table() {
    let dir = TempDir::new("packs-recipe");
    let world = dir.path().join("world");
    let recipe_dir = world
        .join("datapacks")
        .join("testpack")
        .join("data")
        .join("testns")
        .join("recipe");
    std::fs::create_dir_all(&recipe_dir).expect("recipe dir");
    std::fs::write(
        recipe_dir.join("test_brick.json"),
        r#"{
  "type": "minecraft:crafting_shapeless",
  "category": "misc",
  "ingredients": ["minecraft:stone"],
  "result": {"id": "minecraft:cobblestone"}
}"#,
    )
    .expect("write recipe");

    let storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world.clone(),
        autosave_ticks: 0,
    })
    .expect("opens");
    let mut game = Game::new(&storage, 3, game_channel(64).1).expect("game");
    let enabled = EnabledPacks::from_level_dat(&["vanilla".to_owned(), "testpack".to_owned()], &[]);
    let outcome = load_packs(&mut game, &PackRoots::new(&world), &enabled).expect("loads");
    assert!(
        outcome.recipes_loaded >= 1,
        "the pack recipe must load, saw {outcome:?}"
    );
    assert!(
        game.crafting_registry()
            .by_name("testns:test_brick")
            .is_some(),
        "the converted table must contain the pack recipe"
    );
}

/// Loading packs with no pack configured must not wipe the hand-written loot
/// baseline (P14-09 finding: the live verify server logged "no loot table"
/// for `stone`/`oak_log`/`gravel` and dropped nothing, because `load_packs`
/// rebuilt from empty and `set_loot` replaced the constructor's baseline —
/// while every suite test, which never runs `load_packs`, stayed green).
#[test]
fn loading_with_no_packs_keeps_the_loot_baseline() {
    let dir = TempDir::new("packs-baseline-survives");
    let world = dir.path().join("world");
    std::fs::create_dir_all(&world).expect("world dir");

    let storage = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world.clone(),
        autosave_ticks: 0,
    })
    .expect("opens");
    let mut game = Game::new(&storage, 3, game_channel(64).1).expect("game");
    let stone = mc_core::ids::ResourceId::parse("minecraft:blocks/stone").expect("an id");
    assert!(
        game.loot().by_name(&stone).is_some(),
        "the constructor installs the baseline"
    );

    load_packs(&mut game, &PackRoots::new(&world), &EnabledPacks::default()).expect("loads");
    for table in [
        "minecraft:blocks/stone",
        "minecraft:blocks/cobblestone",
        "minecraft:blocks/oak_log",
    ] {
        let id = mc_core::ids::ResourceId::parse(table).expect("an id");
        assert!(
            game.loot().by_name(&id).is_some(),
            "an empty pack load must keep the baseline table {table}"
        );
    }
}
