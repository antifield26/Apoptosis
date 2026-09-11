//! `/function` execution end to end (P07-08, P07-18).
//!
//! The loader's own tests prove that a `.mcfunction` parses. This proves the **execution** half,
//! which is what P07-08 actually names: a real file on disk, discovered by extension, named by its
//! path, and run command by command as the invoker.
//!
//! Every assertion reads the reply text rather than counting packets. This project has been bitten
//! four times by assertions that could not fail — most recently the first version of the
//! `/execute` tests, which passed with the mechanism disabled — so "the command was answered" is
//! never treated as evidence that the right thing happened.

use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::path::Path;
use std::time::Duration;

/// A pack directory holding functions, and a server reading it.
struct Harness {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    functions_dir: std::path::PathBuf,
    _dir: TempDir,
}

impl Harness {
    /// Write a function file at `relative` under the function directory.
    fn write_function(&self, relative: &str, body: &str) {
        let path = self.functions_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("function subdir");
        }
        std::fs::write(&path, body).expect("write the function");
    }

    async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let functions_dir = dir.path().join("pack").join("function");
        std::fs::create_dir_all(&functions_dir).expect("function dir");

        let storage = WorldService::open(&mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        })
        .expect("world opens");
        let (event_tx, event_rx) = game_channel(256);
        let mut game = Game::new(&storage, 3, event_rx).expect("game builds");
        game.load_functions_from(&functions_dir, "minecraft")
            .expect("functions load");

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
            functions_dir,
            _dir: dir,
        }
    }

    /// Run a command and return every `system_chat` line it produced.
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
                Ok(Ok(raw)) if raw.id == clientbound::play::SYSTEM_CHAT => {
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

#[test]
fn a_function_is_named_by_its_path_relative_to_the_function_directory() {
    // The format's rule, and the one thing a caller has to get right because the loader takes the
    // name rather than deriving it.
    let root = Path::new("/pack/function");
    let cases = [
        ("/pack/function/simple.mcfunction", "minecraft:simple"),
        ("/pack/function/foo/bar.mcfunction", "minecraft:foo/bar"),
        (
            "/pack/function/deep/nested/name.mcfunction",
            "minecraft:deep/nested/name",
        ),
        // A dot in the file name other than the extension: only the extension is stripped.
        ("/pack/function/v1.2.mcfunction", "minecraft:v1.2"),
    ];
    for (path, expected) in cases {
        let name = mc_server::functions::function_name(root, Path::new(path), "minecraft")
            .unwrap_or_else(|| panic!("{path} must yield a name"));
        assert_eq!(name.to_string(), expected, "{path}");
    }

    // A path outside the function root has no name, which means the caller discovered it wrongly.
    assert!(
        mc_server::functions::function_name(
            root,
            Path::new("/elsewhere/x.mcfunction"),
            "minecraft"
        )
        .is_none()
    );
}

#[test]
fn the_namespace_comes_from_the_caller_not_a_constant() {
    // **The bug this parameter exists to prevent.** The first version hard-coded `minecraft:`, so a
    // world pack's `data/testns/function/greet.mcfunction` was named `minecraft:greet` and
    // `/function testns:greet` reported an unknown function — with the file present, loaded and
    // counted. The same path under two namespaces must give two different names.
    let root = Path::new("/pack/function");
    let path = Path::new("/pack/function/greet.mcfunction");

    let vanilla = mc_server::functions::function_name(root, path, "minecraft").expect("a name");
    let pack = mc_server::functions::function_name(root, path, "testns").expect("a name");
    assert_eq!(vanilla.to_string(), "minecraft:greet");
    assert_eq!(pack.to_string(), "testns:greet");
    assert_ne!(vanilla, pack, "the namespace must change the name");

    // A nested path keeps its subdirectory under either namespace.
    let nested = mc_server::functions::function_name(
        root,
        Path::new("/pack/function/foo/bar.mcfunction"),
        "testns",
    )
    .expect("a name");
    assert_eq!(nested.to_string(), "testns:foo/bar");
}

#[tokio::test]
async fn a_function_runs_its_commands_in_order() {
    let mut harness = Harness::start("fn-order").await;
    harness.write_function("ordered.mcfunction", "say FIRST\nsay SECOND\nsay THIRD\n");

    // Reload so the new file is seen. A server loads functions at startup in production; here the
    // reload stands in for that, and the alternative (writing the files before `start`) cannot
    // work because the harness creates the directory it writes into.
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:ordered").await;
    let said: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("[Caller]"))
        .collect();
    assert_eq!(said.len(), 3, "three commands must run: {lines:?}");
    assert!(said[0].contains("FIRST"), "in file order: {said:?}");
    assert!(said[1].contains("SECOND"), "in file order: {said:?}");
    assert!(said[2].contains("THIRD"), "in file order: {said:?}");

    assert!(
        lines.iter().any(|line| line.contains("Ran 3 command")),
        "and the count is reported: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn comments_and_blank_lines_are_not_commands() {
    let mut harness = Harness::start("fn-comments").await;
    harness.write_function(
        "sparse.mcfunction",
        "# a comment\n\nsay ONLY\n\n# another\n",
    );
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:sparse").await;
    let said: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("[Caller]"))
        .collect();
    assert_eq!(said.len(), 1, "one real command: {lines:?}");
    assert!(said[0].contains("ONLY"));
    assert!(
        lines.iter().any(|line| line.contains("Ran 1 command")),
        "the count excludes comments and blanks: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_unknown_function_is_reported_rather_than_silently_doing_nothing() {
    let mut harness = Harness::start("fn-unknown").await;
    let lines = harness.run("function minecraft:does_not_exist").await;
    assert!(
        lines.iter().any(|line| line.contains("Unknown function")),
        "an unknown function must be named: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_cannot_exceed_the_recursion_bound() {
    // A self-calling function. The static cycle detector in the loader could catch a literal one,
    // but the run-time bound is the real guarantee, so this asserts the bound fires rather than
    // assuming the detector did.
    let mut harness = Harness::start("fn-recursion").await;
    harness.write_function("loop.mcfunction", "function minecraft:loop\n");
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:loop").await;
    assert!(
        lines.iter().any(|line| line.contains("recursion")),
        "the recursion bound must be reported: {lines:?}"
    );
    assert_eq!(
        harness.game.player_count(),
        1,
        "and the server survives, which is the property that matters"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn mutual_recursion_between_two_functions_is_also_bounded() {
    // A cycle through *two* files, which a single-file detector cannot see and the run-time bound
    // must catch.
    let mut harness = Harness::start("fn-mutual").await;
    harness.write_function("ping.mcfunction", "function minecraft:pong\n");
    harness.write_function("pong.mcfunction", "function minecraft:ping\n");
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:ping").await;
    assert!(
        lines.iter().any(|line| line.contains("recursion")),
        "mutual recursion must hit the same bound: {lines:?}"
    );
    assert_eq!(harness.game.player_count(), 1);
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_does_not_grant_permission() {
    // **The safety property.** A function's commands each pass their own permission check as the
    // invoker, so a level-0 player cannot use one to reach an operator command. Without this,
    // `/function` would be a privilege-escalation route for anyone who can author a pack.
    let mut harness = Harness::start("fn-permission").await;
    harness.write_function("sneaky.mcfunction", "stop\nop\n");
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:sneaky").await;
    assert!(
        !harness.game.shutdown_requested(),
        "a function must not let a level-0 player stop the server"
    );
    assert!(
        lines.iter().any(|line| line.contains("permission")),
        "and the refusals are reported: {lines:?}"
    );
    assert_eq!(harness.game.player_count(), 1);
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_containing_macros_is_refused_with_that_reason() {
    // `$(name)` needs the caller's arguments, which this build does not substitute. Passing the
    // text through would dispatch a command that does not exist and report a confusing error, so
    // the file is refused and the reason names macros.
    let mut harness = Harness::start("fn-macro").await;
    harness.write_function("macro.mcfunction", "say $(greeting) world\n");
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:macro").await;
    assert!(
        lines.iter().any(|line| line.contains("macro")),
        "the refusal must name macros: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_can_call_another_function() {
    // Nesting that is *not* recursive must work, so the bound does not simply refuse every call.
    let mut harness = Harness::start("fn-nested").await;
    harness.write_function("inner.mcfunction", "say INNER\n");
    harness.write_function("outer.mcfunction", "say OUTER\nfunction minecraft:inner\n");
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:outer").await;
    assert!(
        lines.iter().any(|line| line.contains("OUTER")),
        "the outer command runs: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("INNER")),
        "and the nested one does too: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("recursion")),
        "a non-recursive nest must not hit the bound: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_tag_is_refused_by_name() {
    // Vanilla's `/function #namespace:tag` runs every function in a tag. Not supported here, and
    // saying so is better than reporting an unknown function.
    let mut harness = Harness::start("fn-tag").await;
    let lines = harness.run("function #minecraft:tick").await;
    assert!(
        lines.iter().any(|line| line.contains("tags")),
        "a tag must be refused by name: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_runs_execute_chains_and_reports_them() {
    // The two features composing: a function line that is an `execute` chain goes through the
    // chain parser rather than being dispatched as a root command named `execute`.
    let mut harness = Harness::start("fn-execute").await;
    harness.write_function(
        "chained.mcfunction",
        "execute if entity @a run say CHAINED\nsay AFTER\n",
    );
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:chained").await;
    assert!(
        lines.iter().any(|line| line.contains("CHAINED")),
        "the execute line runs and its condition holds: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("AFTER")),
        "and the next line still runs: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Ran 2 command")),
        "both lines are counted: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_function_with_an_unknown_command_reports_it_and_keeps_going() {
    let mut harness = Harness::start("fn-bad-line").await;
    harness.write_function(
        "mixed.mcfunction",
        "say BEFORE\ndefinitely_not_a_command\nsay AFTER\n",
    );
    harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("reload");

    let lines = harness.run("function minecraft:mixed").await;
    assert!(
        lines.iter().any(|line| line.contains("BEFORE")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Unknown command")),
        "the bad line is reported: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("AFTER")),
        "and the function continues rather than aborting: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_malformed_function_file_does_not_remove_the_others() {
    // AGENTS.md §9: one bad file must not discard the rest. The loader counts it and continues.
    let mut harness = Harness::start("fn-malformed").await;
    harness.write_function("good.mcfunction", "say GOOD\n");
    // An over-long line, which the loader's per-file limits must refuse without taking the
    // registry down.
    let huge = format!("say {}\n", "x".repeat(300_000));
    harness.write_function("huge.mcfunction", &huge);
    let loaded = harness
        .game
        .load_functions_from(&harness.functions_dir, "minecraft")
        .expect("a malformed file must not fail the load");
    assert!(loaded >= 1, "the good function is still loaded: {loaded}");

    let lines = harness.run("function minecraft:good").await;
    assert!(
        lines.iter().any(|line| line.contains("GOOD")),
        "and it still runs: {lines:?}"
    );
    harness.service.shutdown().await;
}
