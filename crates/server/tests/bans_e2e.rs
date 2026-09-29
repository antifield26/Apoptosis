//! Server bans end to end (P19-02).
//!
//! The file shape, dates and expiry math are proven in `bans.rs` unit
//! tests; this proves the **effect**: a banned profile or address is
//! refused at the join gate before a slot is spent, an expired row admits,
//! `/ban` disconnects the live session with the ban screen and persists,
//! `/pardon` releases, `/ban-ip` sweeps every holder of the address,
//! `/kick` disconnects with a reason and writes nothing.
//!
//! Falsification shape: skip either gate check and the refused joins land;
//! drop the live disconnect and the banned session stays; stop persisting
//! and the file asserts go red.

use mc_command::PermissionLevel;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet as _;
use mc_protocol::packets::play::{PlayDisconnect, SystemChat};
use mc_server::bans::{BANNED_IPS_FILE_NAME, BANNED_PLAYERS_FILE_NAME, BanList};
use mc_server::game::{Game, TickReport};
use mc_server::ops::{OPS_FILE_NAME, OperatorList};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, SystemTime};

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

fn test_ip(last: u8) -> IpAddr {
    format!("192.0.2.{last}").parse().expect("test-net address")
}

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList, bans: BanList) -> Self {
        let dir = TempDir::new(tag);
        let service = WorldService::open(&config(&dir)).expect("world opens");
        let (events_tx, events_rx) = mc_network::bridge::game_channel(64);
        let mut game = Game::build_with_operators(None, Some(service), 3, events_rx, 7, operators)
            .expect("game builds");
        game.set_bans(bans);
        game.set_ops_directory(dir.path().to_path_buf());
        Self {
            game,
            events: events_tx,
            ids: ConnectionIds::new(),
            dir,
        }
    }

    /// Join `name` from `ip` through the real event channel, like the
    /// connection layer does: address first, then the join.
    fn join(&mut self, name: &str, ip: IpAddr) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, inbound) = OutboundSender::pair(id, 64);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::PeerAddress { ip },
            })
            .expect("address queued");
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

    fn disconnect_reason(inbound: &mut InboundReceiver) -> Option<String> {
        let mut reason = None;
        while let Some(raw) = inbound.try_recv() {
            if let Ok(packet) = PlayDisconnect::decode(&raw.payload) {
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
}

fn banned_profile(name: &str) -> BanList {
    let uuid = mc_network::auth::offline_profile(name).id.to_string();
    let mut bans = BanList::new();
    bans.ban_player(mc_server::bans::PlayerBan {
        uuid,
        name: name.to_owned(),
        created: SystemTime::now(),
        source: "Chief".to_owned(),
        expires: None,
        reason: "griefing".to_owned(),
    });
    bans
}

#[test]
fn banned_profile_is_refused_before_a_slot_is_taken() {
    let mut harness = Harness::new(
        "bans-refuse",
        OperatorList::new(),
        banned_profile("Griefer"),
    );
    let (id, mut inbound) = harness.join("Griefer", test_ip(1));
    assert!(
        !harness.game.has_player(id),
        "a banned profile must not join"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut inbound).as_deref(),
        Some("You are banned from this server.\nReason: griefing"),
        "the refusal carries the ban screen"
    );
    assert_eq!(harness.game.player_count(), 0);
}

#[test]
fn banned_address_is_refused() {
    let mut bans = BanList::new();
    bans.ban_ip(mc_server::bans::IpBan {
        ip: test_ip(2),
        created: SystemTime::now(),
        source: "Chief".to_owned(),
        expires: None,
        reason: String::new(),
    });
    let mut harness = Harness::new("bans-ip-refuse", OperatorList::new(), bans);
    let (id, mut inbound) = harness.join("Stranger", test_ip(2));
    assert!(!harness.game.has_player(id));
    assert_eq!(
        Harness::disconnect_reason(&mut inbound).as_deref(),
        Some("You are banned from this server.\nReason: Banned by an operator."),
        "an empty reason falls back to the default"
    );
    // A different address is unaffected.
    let (id, _) = harness.join("Stranger", test_ip(3));
    assert!(harness.game.has_player(id));
}

#[test]
fn expired_row_admits() {
    let past = SystemTime::now() - Duration::from_secs(60);
    let uuid = mc_network::auth::offline_profile("Temp").id.to_string();
    let mut bans = BanList::new();
    bans.ban_player(mc_server::bans::PlayerBan {
        uuid,
        name: "Temp".to_owned(),
        created: past,
        source: "Chief".to_owned(),
        expires: Some(past + Duration::from_secs(30)),
        reason: "test".to_owned(),
    });
    let mut harness = Harness::new("bans-expired", OperatorList::new(), bans);
    let (id, _) = harness.join("Temp", test_ip(4));
    assert!(
        harness.game.has_player(id),
        "an expired row bans nobody (clock-injected past)"
    );
}

#[test]
fn ban_disconnects_the_live_session_and_persists() {
    let mut harness = Harness::new("bans-live", ops_for("Chief", 4), BanList::new());
    let (chief, mut chief_in) = harness.join("Chief", test_ip(5));
    let (victim, mut victim_in) = harness.join("Victim", test_ip(6));
    assert!(harness.game.has_player(victim));

    let lines = harness.command(chief, &mut chief_in, "ban Victim griefing the spawn");
    assert!(
        lines.iter().any(|line| line.contains("Banned Victim")),
        "ban confirms: {lines:?}"
    );
    // The kick queues inside the tick: one more tick drops the session.
    harness.game.tick().expect("tick drops the banned session");
    assert!(
        !harness.game.has_player(victim),
        "banning a live player disconnects them"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut victim_in).as_deref(),
        Some("You are banned from this server.\nReason: griefing the spawn"),
        "the kicked client sees the ban screen"
    );
    // ...and the file carries the row, so a reboot keeps the ban.
    let text = std::fs::read_to_string(harness.dir.path().join(BANNED_PLAYERS_FILE_NAME))
        .expect("player ban file written");
    let victim_uuid = mc_network::auth::offline_profile("Victim").id.to_string();
    assert!(text.contains(&victim_uuid), "the file carries the ban");
    let (again, _) = harness.join("Victim", test_ip(6));
    assert!(
        !harness.game.has_player(again),
        "the recorded ban refuses the rejoin"
    );
}

#[test]
fn pardon_releases_and_banlist_names_rows() {
    let mut harness = Harness::new(
        "bans-pardon",
        ops_for("Chief", 4),
        banned_profile("Griefer"),
    );
    let (chief, mut chief_in) = harness.join("Chief", test_ip(7));

    let lines = harness.command(chief, &mut chief_in, "banlist");
    assert!(
        lines.iter().any(|line| line.contains("Griefer")),
        "banlist names the row: {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_in, "pardon Griefer");
    assert!(
        lines.iter().any(|line| line.contains("Pardoned Griefer")),
        "pardon confirms: {lines:?}"
    );
    let (id, _) = harness.join("Griefer", test_ip(8));
    assert!(harness.game.has_player(id), "the pardoned profile joins");
    let lines = harness.command(chief, &mut chief_in, "banlist");
    assert!(
        lines.iter().any(|line| line.contains("Nobody")),
        "the list is empty again: {lines:?}"
    );
}

#[test]
fn ban_ip_disconnects_every_holder_and_pardon_ip_releases() {
    let mut harness = Harness::new("bans-ip-live", ops_for("Chief", 4), BanList::new());
    let (chief, mut chief_in) = harness.join("Chief", test_ip(9));
    let (first, _) = harness.join("First", test_ip(10));
    let (second, _) = harness.join("Second", test_ip(10));
    assert!(harness.game.has_player(first) && harness.game.has_player(second));

    let lines = harness.command(chief, &mut chief_in, "ban-ip 192.0.2.10 lag machine");
    assert!(
        lines.iter().any(|line| line.contains("Banned 192.0.2.10")),
        "ban-ip confirms: {lines:?}"
    );
    harness.game.tick().expect("tick drops the holders");
    assert!(
        !harness.game.has_player(first) && !harness.game.has_player(second),
        "every holder of the address is disconnected"
    );
    assert!(
        harness.game.has_player(chief),
        "a different address is untouched"
    );
    let text = std::fs::read_to_string(harness.dir.path().join(BANNED_IPS_FILE_NAME))
        .expect("ip ban file written");
    assert!(text.contains("192.0.2.10"));

    let lines = harness.command(chief, &mut chief_in, "banlist ips");
    assert!(
        lines.iter().any(|line| line.contains("192.0.2.10")),
        "banlist ips names the row: {lines:?}"
    );
    let lines = harness.command(chief, &mut chief_in, "pardon-ip 192.0.2.10");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Pardoned 192.0.2.10")),
        "pardon-ip confirms: {lines:?}"
    );
    let (id, _) = harness.join("Third", test_ip(10));
    assert!(harness.game.has_player(id), "the pardoned address joins");
}

#[test]
fn kick_disconnects_with_a_reason_and_writes_nothing() {
    let mut harness = Harness::new("bans-kick", ops_for("Chief", 4), BanList::new());
    let (chief, mut chief_in) = harness.join("Chief", test_ip(11));
    let (victim, mut victim_in) = harness.join("Victim", test_ip(12));

    let lines = harness.command(chief, &mut chief_in, "kick Victim spamming");
    assert!(
        lines.iter().any(|line| line.contains("Kicked Victim")),
        "kick confirms: {lines:?}"
    );
    harness.game.tick().expect("tick drops the kicked session");
    assert!(!harness.game.has_player(victim));
    assert_eq!(
        Harness::disconnect_reason(&mut victim_in).as_deref(),
        Some("spamming"),
        "the kicked client sees exactly the reason"
    );
    assert!(
        !harness.dir.path().join(BANNED_PLAYERS_FILE_NAME).exists()
            && !harness.dir.path().join(BANNED_IPS_FILE_NAME).exists(),
        "a kick is a moment, not a record: no ban file may appear"
    );
    // ...and the kicked profile may rejoin immediately.
    let (again, _) = harness.join("Victim", test_ip(12));
    assert!(harness.game.has_player(again));
}

#[test]
fn plain_player_cannot_ban_or_kick() {
    let mut harness = Harness::new("bans-denied", OperatorList::new(), BanList::new());
    let (id, mut inbound) = harness.join("Plain", test_ip(13));
    for text in ["ban Plain x", "kick Plain", "banlist"] {
        let lines = harness.command(id, &mut inbound, text);
        assert!(
            lines.iter().any(|line| line.contains("permission")),
            "{text:?} must deny level 0: {lines:?}"
        );
    }
    assert_eq!(harness.game.player_permission(id), PermissionLevel::All);
}
