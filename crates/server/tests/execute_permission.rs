//! `/execute` must never lend the permission of the player a selector matched.
//!
//! AUDIT-19 G-01: the chain handed the *selected* player's level to the inner
//! command, so a level-0 player could run
//! `/execute as @a[name=Admin] run op <self>` and then `/stop`. The pin needs
//! **two** sessions on purpose: the older `execute_e2e` case has one, so `@a`,
//! `@s` and `@p` all resolve to the invoker and the escalation is unobservable —
//! a test that cannot fail.
//!
//! Vanilla's rule is the one pinned here: `/execute` changes who and where a
//! command runs, never what the invoker may do.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::build_with_operators(None, Some(service), 3, rx, 7, operators)
            .expect("game builds");
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, out) = mc_network::bridge::OutboundSender::pair(id, 8192);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        (id, out)
    }

    fn command(&mut self, id: ConnectionId, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let mut report = TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
        let mut lines = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT {
                match SystemChat::decode(&raw.payload) {
                    Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                    Err(error) => panic!("a disguised_chat must decode: {error}"),
                }
            }
        }
        lines
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

fn denied(lines: &[String]) -> bool {
    lines.iter().any(|line| line.contains("permission"))
}

/// The escalation itself, in every selector form that names the operator.
#[test]
fn execute_as_cannot_borrow_a_matched_operators_permission() {
    let mut harness = Harness::new("audit19-exec-escalation", ops_for("Admin", 4));
    let (admin, mut admin_out) = harness.join("Admin");
    let (newbie, mut newbie_out) = harness.join("Newbie");

    // The ladder is real in both directions, so a refusal below is permission and not a typo.
    assert!(
        denied(&harness.command(newbie, &mut newbie_out, "seed")),
        "a level-0 player must not read the seed directly"
    );
    let admin_seed = harness.command(admin, &mut admin_out, "seed");
    assert!(
        admin_seed.iter().any(|line| line.contains("Seed:")),
        "the operator's own /seed must work: {admin_seed:?}"
    );

    // The escalation AUDIT-19 found: the level must not travel with the match.
    for chain in [
        "execute as @a[name=Admin] run seed",
        "execute as @a[name=Admin] run op Newbie",
        "execute as @a[name=Admin] run execute as @s run seed",
        "execute at @a[name=Admin] run seed",
        "execute rotated as @a[name=Admin] run seed",
        "execute anchored feet as @a[name=Admin] run seed",
    ] {
        let lines = harness.command(newbie, &mut newbie_out, chain);
        assert!(
            denied(&lines),
            "{chain:?} must be refused for a level-0 invoker, saw {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("Seed:")),
            "{chain:?} leaked the seed: {lines:?}"
        );
    }
    assert_eq!(
        harness.game.player_permission(newbie),
        mc_command::PermissionLevel::All,
        "no chain may have granted the invoker anything"
    );

    // And the same chain still works for the operator: the level travels with the
    // **invoker**, not with the match — which is what makes this a rule and not a ban.
    let lines = harness.command(admin, &mut admin_out, "execute as @a[name=Newbie] run seed");
    assert!(
        lines.iter().any(|line| line.contains("Seed:")),
        "an operator's /execute as a level-0 player must still run: {lines:?}"
    );
}
