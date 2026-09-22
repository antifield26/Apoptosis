//! `/execute` end to end (P07-07, P07-18).
//!
//! `mc_command::execute`'s tests prove the chain grammar, including that every unsupported
//! modifier is refused by name. This proves the **effect**, over a real socket, by reading what
//! the server actually replies.
//!
//! # Why the replies are read, not just counted
//!
//! The first version of this file asserted only that the client survived and the command was
//! answered — which is true whether or not the mechanism works. Verified by disabling
//! `positioned` and inverting `unless`: **both tests still passed.** So every test here that
//! makes a claim about behaviour decodes the `disguised_chat` reply and asserts on its text. That is
//! the difference between "something happened" and "the right thing happened", and it is the
//! fourth time in this project that distinction has had to be forced.

use mc_network::bridge::game_channel;
use mc_protocol::RawPacket;
use mc_protocol::ids::{clientbound, serverbound};
// `Packet` must be in scope for `SystemChat::decode`: it is a trait method, and the error
// without the import ("no associated function named `decode`") does not say so.
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::client::TestClient;
use mc_test_support::fixtures::TempDir;
use std::time::Duration;

/// A server with a real socket, a logged-in client, and the game loop.
struct Harness {
    client: TestClient,
    game: Game,
    service: mc_network::NetworkService,
    _dir: TempDir,
}

impl Harness {
    async fn start(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let storage = WorldService::open(&mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        })
        .expect("world opens");
        let (event_tx, event_rx) = game_channel(256);
        let mut game = Game::new(&storage, 3, event_rx).expect("game builds");

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
        let (client, _join) = TestClient::login_join_tick(addr, "Executor", || {
            game.tick().expect("tick");
        })
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

    /// Queue a `chat_command`.
    async fn send_command(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut payload = varint(i32::try_from(bytes.len()).expect("a test string fits"));
        payload.extend_from_slice(bytes);
        self.client
            .send_raw_packet(&RawPacket::new(serverbound::play::CHAT_COMMAND, payload))
            .await
            .expect("command sent");
    }

    /// Run a command and return every `disguised_chat` line it produced.
    ///
    /// This is the whole point of the harness: the *text* is the evidence.
    async fn run(&mut self, text: &str) -> Vec<String> {
        self.send_command(text).await;
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
                // Another packet: not evidence, keep waiting.
                Ok(Ok(_)) => {}
                // The connection ended without a reply.
                Ok(Err(_)) => break,
                // Nothing yet: tick the game and keep waiting, so a slow reply is not read as
                // no reply.
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
async fn execute_run_runs_the_inner_command() {
    let mut harness = Harness::start("exec-basic").await;
    // `say` broadcasts to players, so the invoker receives their own message. That text is the
    // evidence that the inner command ran.
    let lines = harness.run("execute run say hello from execute").await;
    assert!(
        lines.iter().any(|line| line.contains("hello from execute")),
        "the inner command must run: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn positioned_really_moves_the_executing_position() {
    // **Probed and fixed.** The first version asserted only that the client survived, which is
    // true whether or not `positioned` does anything. This asserts the reply text, and the
    // negative case is what makes it non-vacuous: the same `if block` on a block that is *not*
    // there must report a failed test.
    let mut harness = Harness::start("exec-positioned").await;
    let gold = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:gold_block")
        .expect("gold_block resolves");
    harness
        .game
        .world_mut()
        .set_block(100, 70, 100, gold)
        .expect("place");

    let hit = harness
        .run("execute positioned 100 70 100 if block ~ ~ ~ minecraft:gold_block run say FOUND")
        .await;
    assert!(
        hit.iter().any(|line| line.contains("FOUND")),
        "`positioned` must move the position so the block condition holds: {hit:?}"
    );
    assert!(
        !hit.iter().any(|line| line.contains("Test failed")),
        "and must not report a failure: {hit:?}"
    );

    let miss = harness
        .run("execute positioned 100 70 100 if block ~ ~ ~ minecraft:diamond_block run say FOUND")
        .await;
    assert!(
        !miss.iter().any(|line| line.contains("FOUND")),
        "a condition on an absent block must not run the command: {miss:?}"
    );
    assert!(
        miss.iter().any(|line| line.contains("Test failed")),
        "and must say the test failed: {miss:?}"
    );

    // And the **control**: without `positioned`, the same condition at the player's own feet
    // must fail, so the success above cannot be an accident of the player already standing on
    // gold.
    let control = harness
        .run("execute if block ~ ~ ~ minecraft:gold_block run say FOUND")
        .await;
    assert!(
        !control.iter().any(|line| line.contains("FOUND")),
        "the player must not be standing on gold, or the test above proves nothing: {control:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn unless_really_inverts_if() {
    // **Probed and fixed.** Verified by inverting the `Unless` arm: the first version still
    // passed. This one cannot: it asserts that `unless` on a *present* block blocks the command
    // and on an *absent* block lets it through, and those two are opposites.
    let mut harness = Harness::start("exec-unless").await;
    let gold = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:gold_block")
        .expect("gold_block resolves");
    harness
        .game
        .world_mut()
        .set_block(50, 70, 50, gold)
        .expect("place");

    // `if` on a present block: runs.
    let if_present = harness
        .run("execute positioned 50 70 50 if block ~ ~ ~ minecraft:gold_block run say RAN")
        .await;
    assert!(
        if_present.iter().any(|line| line.contains("RAN")),
        "`if` on a present block must run: {if_present:?}"
    );

    // `unless` on the same present block: must NOT run.
    let unless_present = harness
        .run("execute positioned 50 70 50 unless block ~ ~ ~ minecraft:gold_block run say RAN")
        .await;
    assert!(
        !unless_present.iter().any(|line| line.contains("RAN")),
        "`unless` on a present block must not run: {unless_present:?}"
    );
    assert!(
        unless_present
            .iter()
            .any(|line| line.contains("Test failed")),
        "and must report the failure: {unless_present:?}"
    );

    // `unless` on an absent block: must run. This is the arm that an `unless`-behaves-like-`if`
    // bug gets backwards.
    let unless_absent = harness
        .run("execute positioned 50 70 50 unless block ~ ~ ~ minecraft:diamond_block run say RAN")
        .await;
    assert!(
        unless_absent.iter().any(|line| line.contains("RAN")),
        "`unless` on an absent block must run: {unless_absent:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn as_does_not_grant_permission() {
    // `as` changes who the command runs as, not what they may do: the inner command goes back
    // through the dispatcher's own permission check. A level-0 player must not reach a
    // console-only command this way.
    let mut harness = Harness::start("exec-perm").await;
    for command in [
        "execute as @a run stop",
        "execute as @s run stop",
        "execute as @a run execute as @s run stop",
        "execute as @p run op",
    ] {
        let lines = harness.run(command).await;
        assert!(
            !harness.game.shutdown_requested(),
            "{command:?} must not let a level-0 player stop the server"
        );
        // And the refusal is explained rather than silent, which rules out "the command never
        // reached the dispatcher".
        assert!(
            lines.iter().any(|line| line.contains("permission")),
            "{command:?} must be refused with a message: {lines:?}"
        );
    }
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_unsupported_modifier_is_named_in_the_reply() {
    // The property that matters most: a silently-dropped `store` would send output to chat
    // instead of a block, and a silently-dropped `if` would run a command that should not run.
    let mut harness = Harness::start("exec-unsupported").await;
    for (command, expected) in [
        ("execute store result score x run say hi", "store"),
        ("execute rotated 0 0 run say hi", "rotated"),
        ("execute facing 0 0 0 run say hi", "facing"),
        ("execute anchored eyes run say hi", "anchored"),
        ("execute in minecraft:overworld run say hi", "in"),
        ("execute if score x run say hi", "score"),
        ("execute if data entity @s run say hi", "data"),
        ("execute if predicate minecraft:x run say hi", "predicate"),
    ] {
        let lines = harness.run(command).await;
        assert!(
            lines.iter().any(|line| line.contains(expected)),
            "{command:?} must name {expected:?} in its reply: {lines:?}"
        );
    }
    harness.service.shutdown().await;
}

#[tokio::test]
async fn a_nested_chain_terminates_with_a_message_rather_than_overflowing() {
    let mut harness = Harness::start("exec-depth").await;
    let nested = format!("{}say DEEP", "execute run ".repeat(40));
    let lines = harness.run(&nested).await;
    assert!(
        lines.iter().any(|line| line.contains("nest")),
        "the depth bound must be reported: {lines:?}"
    );
    assert_eq!(harness.game.player_count(), 1, "and the client survives");

    // A shallow chain still works, so the bound is not simply refusing everything.
    let shallow = format!("{}say SHALLOW", "execute run ".repeat(2));
    let lines = harness.run(&shallow).await;
    assert!(
        lines.iter().any(|line| line.contains("SHALLOW")),
        "a chain inside the bound must run: {lines:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn malformed_chains_are_refused_without_disconnecting() {
    let mut harness = Harness::start("exec-malformed").await;
    for command in [
        "execute",
        "execute run",
        "execute as",
        "execute as @z run say hi",
        "execute positioned 1 2 run say hi",
        "execute align q run say hi",
        "execute align xx run say hi",
        "execute if run say hi",
        "execute if entity run say hi",
        "execute if block ~ ~ ~ run say hi",
        "execute positioned x y z run say hi",
        &format!("execute {}say hi", "as @s ".repeat(100)),
    ] {
        let lines = harness.run(command).await;
        assert_eq!(
            harness.game.player_count(),
            1,
            "{command:?} must not disconnect the client"
        );
        assert!(
            !lines.is_empty(),
            "{command:?} must be answered rather than ignored"
        );
    }
    harness.service.shutdown().await;
}

#[tokio::test]
async fn execute_is_no_more_privileged_than_the_command_it_runs() {
    let mut harness = Harness::start("exec-privilege").await;
    let unknown = harness.run("execute run definitely_not_a_command").await;
    assert!(
        unknown.iter().any(|line| line.contains("Unknown command")),
        "an unknown inner command stays unknown: {unknown:?}"
    );

    let denied = harness.run("execute run op").await;
    assert!(
        denied.iter().any(|line| line.contains("permission")),
        "an operator-only inner command stays denied: {denied:?}"
    );
    harness.service.shutdown().await;
}

#[tokio::test]
async fn an_entity_condition_reflects_who_is_online() {
    let mut harness = Harness::start("exec-entity").await;
    // `@a` matches the single online player, so the command runs.
    let present = harness.run("execute if entity @a run say SOMEONE").await;
    assert!(
        present.iter().any(|line| line.contains("SOMEONE")),
        "`if entity @a` must hold with a player online: {present:?}"
    );

    // No zombie exists, so the same shape must fail.
    let absent = harness
        .run("execute if entity @e[type=minecraft:zombie] run say ZOMBIE")
        .await;
    assert!(
        !absent.iter().any(|line| line.contains("ZOMBIE")),
        "`if entity @e[type=zombie]` must not hold: {absent:?}"
    );
    assert!(
        absent.iter().any(|line| line.contains("Test failed")),
        "and must say so: {absent:?}"
    );
    harness.service.shutdown().await;
}
