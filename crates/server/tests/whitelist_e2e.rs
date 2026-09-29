//! The login whitelist end to end (P19-01).
//!
//! The file shape is proven in `whitelist.rs` unit tests; this proves the
//! **effect** at the join gate plus the command surface: a listed profile
//! joins under enforcement, an unlisted one is refused with Vanilla's
//! message before any state is spent, an operator bypasses without being
//! listed, and nobody is refused while enforcement is off. Commands run
//! through the real dispatcher as an operator and answer over chat.
//!
//! Falsification shape: skip the gate check and the refused joins land;
//! drop the operator exemption and the bypass test goes red with the
//! operator refused; stop persisting and the add/reload tests go red.

use mc_command::PermissionLevel;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet as _;
use mc_protocol::packets::play::{PlayDisconnect, SystemChat};
use mc_server::game::{Game, TickReport};
use mc_server::ops::{OPS_FILE_NAME, OperatorList};
use mc_server::storage::WorldService;
use mc_server::whitelist::{WHITELIST_FILE_NAME, Whitelist};
use mc_test_support::fixtures::TempDir;
use std::path::Path;

fn config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    }
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new(OPS_FILE_NAME)).expect("the fixture parses")
}

fn whitelist_for(name: &str) -> Whitelist {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}"}}]"#);
    Whitelist::from_value(
        &serde_json::from_str(&text).expect("json"),
        Path::new(WHITELIST_FILE_NAME),
    )
    .expect("the fixture parses")
}

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList, whitelist: Whitelist, enforced: bool) -> Self {
        let dir = TempDir::new(tag);
        let service = WorldService::open(&config(&dir)).expect("world opens");
        let (events_tx, events_rx) = mc_network::bridge::game_channel(64);
        let mut game = Game::build_with_operators(None, Some(service), 3, events_rx, 7, operators)
            .expect("game builds");
        game.set_whitelist(whitelist);
        game.set_whitelist_enforced(enforced);
        // Persistence lands beside the world, like `ops.json`, so add/remove
        // exercise the real file path.
        game.set_ops_directory(dir.path().to_path_buf());
        Self {
            game,
            events: events_tx,
            ids: ConnectionIds::new(),
            dir,
        }
    }

    /// Join `name` through the real event channel, returning the connection
    /// and its inbound receiver.
    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        use mc_network::bridge::OutboundSender;
        let id = self.ids.next_id();
        let (outbound, inbound) = OutboundSender::pair(id, 64);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join queued");
        self.game.tick().expect("tick");
        (id, inbound)
    }

    /// Drain a disconnect reason, if the client saw one.
    fn disconnect_reason(inbound: &mut InboundReceiver) -> Option<String> {
        let mut reason = None;
        while let Some(raw) = inbound.try_recv() {
            if let Ok(packet) = PlayDisconnect::decode(&raw.payload) {
                reason = Some(packet.reason.as_plain().to_owned());
            }
        }
        reason
    }

    /// Run `text` as `id`, returning the chat lines the sender saw.
    fn command(
        &mut self,
        id: ConnectionId,
        inbound: &mut InboundReceiver,
        text: &str,
    ) -> Vec<String> {
        let mut report = TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
        let mut lines = Vec::new();
        while let Some(raw) = inbound.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT
                && let Ok(chat) = SystemChat::decode(&raw.payload)
            {
                lines.push(chat.content.as_plain().to_owned());
            }
        }
        lines
    }
}

#[test]
fn enforced_listed_profile_joins() {
    let mut harness = Harness::new(
        "whitelist-join",
        OperatorList::new(),
        whitelist_for("Listed"),
        true,
    );
    let (id, inbound) = harness.join("Listed");
    assert!(harness.game.has_player(id), "a listed profile joins");
    // Note: no disconnect-shape assert here — `PlayDisconnect::decode` is
    // lenient enough to parse join-burst packets, so only a refusal (whose
    // sole packet is the disconnect) can assert on the reason. The refused
    // test below pins the message.
    let _ = inbound;
}

#[test]
fn enforced_unlisted_profile_is_refused_with_vanillas_message() {
    let mut harness = Harness::new(
        "whitelist-refuse",
        OperatorList::new(),
        whitelist_for("Listed"),
        true,
    );
    let (id, mut inbound) = harness.join("Stranger");
    assert!(
        !harness.game.has_player(id),
        "an unlisted profile must not join"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut inbound).as_deref(),
        Some("You are not white-listed on this server!"),
        "the refusal carries Vanilla's message"
    );
    assert_eq!(harness.game.player_count(), 0, "the refusal spends no slot");
}

#[test]
fn enforced_operator_bypasses_without_being_listed() {
    let mut harness = Harness::new(
        "whitelist-bypass",
        ops_for("Chief", 4),
        whitelist_for("Listed"),
        true,
    );
    let (id, _) = harness.join("Chief");
    assert!(
        harness.game.has_player(id),
        "an operator joins under enforcement without being listed"
    );
}

#[test]
fn unenforced_unlisted_profile_joins() {
    let mut harness = Harness::new(
        "whitelist-off",
        OperatorList::new(),
        whitelist_for("Listed"),
        false,
    );
    let (id, _) = harness.join("Stranger");
    assert!(harness.game.has_player(id), "nobody is refused while off");
}

#[test]
fn whitelist_commands_list_add_and_remove() {
    let mut harness = Harness::new(
        "whitelist-commands",
        ops_for("Chief", 4),
        Whitelist::new(),
        false,
    );
    let (chief, mut chief_in) = harness.join("Chief");
    assert_eq!(
        harness.game.player_permission(chief),
        PermissionLevel::Console,
        "the fixture operator holds authority"
    );

    let lines = harness.command(chief, &mut chief_in, "whitelist list");
    assert!(
        lines.iter().any(|line| line.contains("Nobody")),
        "empty list says so: {lines:?}"
    );

    let (_, _) = harness.join("Helper");
    let lines = harness.command(chief, &mut chief_in, "whitelist add Helper");
    assert!(
        lines.iter().any(|line| line.contains("Added Helper")),
        "add confirms: {lines:?}"
    );
    // The listing reaches the file, not just memory: a reboot reads this
    // file, so a memory-only add would silently drop the entry.
    let text = std::fs::read_to_string(harness.dir.path().join(WHITELIST_FILE_NAME))
        .expect("whitelist.json written");
    let helper_uuid = mc_network::auth::offline_profile("Helper").id.to_string();
    assert!(
        text.contains(&helper_uuid),
        "the file carries the addition, saw {text}"
    );
    let lines = harness.command(chief, &mut chief_in, "whitelist list");
    assert!(
        lines.iter().any(|line| line.contains("Helper")),
        "list names the addition: {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_in, "whitelist add Helper");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("already whitelisted")),
        "re-add is idempotent: {lines:?}"
    );

    let lines = harness.command(chief, &mut chief_in, "whitelist remove Helper");
    assert!(
        lines.iter().any(|line| line.contains("Removed Helper")),
        "remove confirms: {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_in, "whitelist list");
    assert!(
        lines.iter().any(|line| line.contains("Nobody")),
        "the list is empty again: {lines:?}"
    );
}

#[test]
fn whitelist_on_off_and_reload_take_effect() {
    let mut harness = Harness::new(
        "whitelist-toggle",
        ops_for("Chief", 4),
        Whitelist::new(),
        false,
    );
    let (chief, mut chief_in) = harness.join("Chief");

    let lines = harness.command(chief, &mut chief_in, "whitelist on");
    assert!(
        lines.iter().any(|line| line.contains("enforced")),
        "on confirms: {lines:?}"
    );
    assert!(harness.game.whitelist_enforced());
    let (stranger, _) = harness.join("Stranger");
    assert!(
        !harness.game.has_player(stranger),
        "toggling on refuses the unlisted"
    );

    let lines = harness.command(chief, &mut chief_in, "whitelist off");
    assert!(
        lines.iter().any(|line| line.contains("no longer enforced")),
        "off confirms: {lines:?}"
    );
    let (stranger, _) = harness.join("Stranger");
    assert!(
        harness.game.has_player(stranger),
        "toggling off re-opens the gate"
    );

    // `reload` replaces the live list from disk: write a file with the
    // stranger listed, reload, enforce, and the stranger joins.
    let dir = harness.dir.path().to_path_buf();
    let stranger_uuid = mc_network::auth::offline_profile("Stranger").id;
    std::fs::write(
        dir.join(WHITELIST_FILE_NAME),
        format!(r#"[{{"uuid": "{stranger_uuid}", "name": "Stranger"}}]"#),
    )
    .expect("whitelist file");
    let lines = harness.command(chief, &mut chief_in, "whitelist reload");
    assert!(
        lines.iter().any(|line| line.contains("1 listed")),
        "reload reports the count: {lines:?}"
    );
    let _ = harness.command(chief, &mut chief_in, "whitelist on");
    let (stranger, _) = harness.join("Stranger");
    assert!(
        harness.game.has_player(stranger),
        "the reloaded entry admits the stranger"
    );
}

#[test]
fn plain_player_cannot_touch_the_whitelist() {
    let mut harness = Harness::new(
        "whitelist-denied",
        OperatorList::new(),
        Whitelist::new(),
        false,
    );
    let (id, mut inbound) = harness.join("Plain");
    let lines = harness.command(id, &mut inbound, "whitelist on");
    assert!(
        lines.iter().any(|line| line.contains("permission")),
        "a level-0 player is refused: {lines:?}"
    );
    assert!(
        !harness.game.whitelist_enforced(),
        "the refusal changes nothing"
    );
}

/// The boot loads `whitelist.json` and enforcement from config (P19-01):
/// a world whose file lists someone and whose config enforces boots
/// enforcing, and a malformed file boots open rather than refusing to
/// start (same policy as `ops.json`).
#[test]
fn boot_loads_the_file_and_the_config_flag() {
    use mc_server::lifecycle::Server;

    let dir = TempDir::new("whitelist-boot");
    let listed = mc_network::auth::offline_profile("Listed").id;
    std::fs::write(
        dir.path().join(WHITELIST_FILE_NAME),
        format!(r#"[{{"uuid": "{listed}", "name": "Listed"}}]"#),
    )
    .expect("whitelist file");
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.access.whitelist_enforced = true;
    let mut server = Server::new(config);
    server.open_world().expect("world opens");
    let game = server.game().expect("game built");
    assert!(
        game.whitelist_enforced(),
        "the config flag reaches the live gate"
    );
    let listed = mc_network::auth::offline_profile("Listed").id.to_string();
    assert!(
        game.whitelist().contains(&listed),
        "the file reaches the live list"
    );
    drop(server);

    // Malformed file: the boot still succeeds, enforcing nothing new.
    std::fs::write(dir.path().join(WHITELIST_FILE_NAME), "{nope").expect("bad file");
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    let mut server = Server::new(config);
    server
        .open_world()
        .expect("a bad file never stops the boot");
    assert!(
        !server.game().expect("game built").whitelist_enforced(),
        "config default is open"
    );
}
