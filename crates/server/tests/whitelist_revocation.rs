//! Whitelist revocation takes effect on the live session (AUDIT-19 A-12, A-06).
//!
//! The join gate only runs at login, so before this `/whitelist on` and
//! `/whitelist remove` left a player who had just lost access holding a live
//! session until they happened to reconnect. `/ban` already disconnected.
//!
//! The second half is A-06: a whitelist row filed while the player was
//! **offline** carries the offline-derived uuid, while the same person online
//! carries the Mojang uuid from `hasJoined`. `remove` must unlist whichever
//! row exists, or the operator's "remove this name" reports "not whitelisted"
//! and changes nothing.
//!
//! Falsification shape: drop the kick from `on`/`remove` and the "who just
//! lost access" tests go red while the file assertions stay green; drop the
//! offline-row fallback in `remove` and the online-remove test goes red; kick
//! without checking enforcement and the open-gate negative control goes red.
//!
//! This file is separate from `whitelist_e2e.rs` (which owns the join gate's
//! own shape) so the two can be edited without fighting over one hunk.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
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

/// Vanilla's whitelist refusal, word for word the join gate's message.
const UNLISTED: &str = "You are not white-listed on this server!";

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

/// A whitelist listing the **offline derivation** of `name` — what a row
/// written before the player ever joined looks like.
fn offline_listing(name: &str) -> Whitelist {
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
    fn new(tag: &str, whitelist: Whitelist, enforced: bool) -> Self {
        let dir = TempDir::new(tag);
        let service = WorldService::open(&config(&dir)).expect("world opens");
        let (events_tx, events_rx) = game_channel(64);
        let mut game =
            Game::build_with_operators(None, Some(service), 3, events_rx, 7, ops_for("Chief", 4))
                .expect("game builds");
        game.set_whitelist(whitelist);
        game.set_whitelist_enforced(enforced);
        game.set_ops_directory(dir.path().to_path_buf());
        Self {
            game,
            events: events_tx,
            ids: ConnectionIds::new(),
            dir,
        }
    }

    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        self.join_profile(mc_network::auth::offline_profile(name))
    }

    /// Join an explicit profile — the shape online-mode auth produces, where
    /// the uuid comes from `hasJoined`, not from the name (A-06).
    fn join_profile(
        &mut self,
        profile: mc_network::auth::GameProfile,
    ) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, inbound) = OutboundSender::pair(id, 64);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined { profile, outbound },
            })
            .expect("join queued");
        self.game.tick().expect("tick");
        (id, inbound)
    }

    /// The play-phase disconnect reason a client saw, if any. Filtered by
    /// packet id: `PlayDisconnect::decode` is lenient enough to parse a
    /// join-burst packet, so an unfiltered drain can report a phantom kick.
    fn kick_reason(inbound: &mut InboundReceiver) -> Option<String> {
        let mut reason = None;
        while let Some(raw) = inbound.try_recv() {
            if raw.id == clientbound::play::DISCONNECT
                && let Ok(packet) = PlayDisconnect::decode(&raw.payload)
            {
                reason = Some(packet.reason.as_plain().to_owned());
            }
        }
        reason
    }

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

    fn whitelist_file(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(WHITELIST_FILE_NAME))
            .expect("whitelist.json written")
    }
}

#[test]
fn whitelist_on_disconnects_an_online_player_who_lost_access() {
    let mut harness = Harness::new("whitelist-revoke-on", Whitelist::new(), false);
    let (chief, mut chief_in) = harness.join("Chief");
    let (stranger, mut stranger_in) = harness.join("Stranger");
    assert!(
        harness.game.has_player(stranger),
        "the gate is open, so the unlisted player is in"
    );

    let lines = harness.command(chief, &mut chief_in, "whitelist on");
    assert!(
        lines.iter().any(|line| line.contains("enforced")),
        "on confirms: {lines:?}"
    );
    // The kick is queued during the command; the network phase applies it.
    harness
        .game
        .tick()
        .expect("tick applies the queued disconnect");
    assert!(
        !harness.game.has_player(stranger),
        "the player who just lost access is disconnected now, not at their next login"
    );
    assert_eq!(
        Harness::kick_reason(&mut stranger_in).as_deref(),
        Some(UNLISTED),
        "and the kick carries the join gate's own message"
    );
    assert!(
        harness.game.has_player(chief),
        "an operator keeps the bypass, exactly as at the gate"
    );
}

#[test]
fn whitelist_remove_disconnects_the_online_player_it_unlists() {
    let mut harness = Harness::new("whitelist-revoke-remove", offline_listing("Helper"), true);
    let (chief, mut chief_in) = harness.join("Chief");
    let (helper, mut helper_in) = harness.join("Helper");
    assert!(harness.game.has_player(helper), "the listed player joins");

    let lines = harness.command(chief, &mut chief_in, "whitelist remove Helper");
    assert!(
        lines.iter().any(|line| line.contains("Removed Helper")),
        "remove confirms: {lines:?}"
    );
    harness
        .game
        .tick()
        .expect("tick applies the queued disconnect");
    assert!(
        !harness.game.has_player(helper),
        "the unlisted player is disconnected with the removal"
    );
    assert_eq!(
        Harness::kick_reason(&mut helper_in).as_deref(),
        Some(UNLISTED),
        "the removal kick carries the gate's message"
    );

    // And the revocation is on the file, so the rejoin is refused by the gate.
    let (again, mut again_in) = harness.join("Helper");
    assert!(
        !harness.game.has_player(again),
        "the gate refuses the now-unlisted rejoin"
    );
    assert_eq!(
        Harness::kick_reason(&mut again_in).as_deref(),
        Some(UNLISTED),
        "the refusal is the whitelist's, not something else"
    );
}

/// The negative control: with enforcement **off** nobody is refused, so
/// removing a name must not disconnect a player who still has access. Without
/// this the two tests above would pass for a `remove`/`on` path that kicks
/// unconditionally, which would be a new way to disconnect players.
#[test]
fn removal_does_not_kick_while_the_whitelist_is_off() {
    let mut harness = Harness::new("whitelist-revoke-off", offline_listing("Helper"), false);
    let (chief, mut chief_in) = harness.join("Chief");
    let (helper, mut helper_in) = harness.join("Helper");
    assert!(harness.game.has_player(helper));

    let lines = harness.command(chief, &mut chief_in, "whitelist remove Helper");
    assert!(
        lines.iter().any(|line| line.contains("Removed Helper")),
        "remove still confirms: {lines:?}"
    );
    harness.game.tick().expect("tick");
    assert!(
        harness.game.has_player(helper),
        "nobody lost access while the gate is open, so nobody is kicked"
    );
    assert_eq!(
        Harness::kick_reason(&mut helper_in),
        None,
        "and no disconnect packet was sent"
    );
}

#[test]
fn whitelist_remove_reaches_a_row_filed_before_an_online_join() {
    // A-06, whitelist half: the row carries the offline derivation, the live
    // session carries the Mojang uuid. Removing by name must find the row that
    // exists instead of reporting "not whitelisted" and leaving it behind.
    let mut harness = Harness::new("whitelist-online-remove", Whitelist::new(), false);
    let (chief, mut chief_in) = harness.join("Chief");
    // Filed offline: no session holds "FarAway" yet.
    let lines = harness.command(chief, &mut chief_in, "whitelist add FarAway");
    assert!(
        lines.iter().any(|line| line.contains("Added FarAway")),
        "the offline listing lands: {lines:?}"
    );
    let derived = mc_network::auth::offline_profile("FarAway").id.to_string();
    assert!(harness.whitelist_file().contains(&derived));

    // The same person now joins with an online (Mojang) uuid.
    let online = mc_network::auth::GameProfile {
        id: uuid::Uuid::parse_str("3f2504e0-4f89-41d3-9a0c-0305e82c3301").expect("uuid"),
        name: "FarAway".to_owned(),
        properties: Vec::new(),
    };
    let (id, _) = harness.join_profile(online);
    assert!(harness.game.has_player(id), "the online profile joins");

    let lines = harness.command(chief, &mut chief_in, "whitelist remove FarAway");
    assert!(
        lines.iter().any(|line| line.contains("Removed FarAway")),
        "the row filed offline is the one removed: {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_in, "whitelist list");
    assert!(
        lines.iter().any(|line| line.contains("Nobody")),
        "the list is empty again: {lines:?}"
    );
    assert!(
        !harness.whitelist_file().contains(&derived),
        "and the file no longer carries the stale row"
    );
}
