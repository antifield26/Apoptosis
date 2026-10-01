//! AUDIT-19 Lane E: three gate behaviours that had **no** pin at all.
//!
//! Lane E judged 80 pin rows and found four behaviours unrepresented. Three are
//! pinned here (the fourth, `/fill`'s chunk door, belongs to the A-09 budget
//! test):
//!
//! 1. **Bans exempt no operator.** `bans.rs` states the policy ("No operator
//!    exemption ... exempting ops would make `/ban` unable to stop an abusive
//!    operator") and the join gate repeats it ("with no operator exemption"),
//!    but no test said so either way. The pin runs both directions: a level-4
//!    operator and a level-0 player are kicked, banned and refused on return,
//!    and even a listing at level 0 — which *does* exempt from the whitelist —
//!    exempts from nothing here. If the ban path ever grows the whitelist's
//!    exemption, this file goes red; the whitelist's own half of "who is
//!    exempt" is pinned in `whitelist_revocation.rs` and `whitelist_e2e.rs`.
//! 2. **The join gate's order: whitelist first, bans second.** A profile that
//!    is both banned and unlisted meets the whitelist refusal, so the ban
//!    screen is only reachable for a profile the whitelist admits. The two
//!    refusals are different player-visible messages, and which one arrives is
//!    the entire observable content of the order.
//! 3. **The exposure warning's call site.** `exposure_warning_needed` has unit
//!    tests, but a function whose only caller is a test would pass them while
//!    warning nobody. This boots the real `Server::start_network` and asserts
//!    the warning reaches the log — through the boot path, not the predicate.
//!
//! Falsification shape, one perturbation per pin: add an operator check to the
//! ban gate and (1) fails; swap the whitelist and ban blocks and (2) fails;
//! delete the warning call and (3) fails.

use mc_command::PermissionLevel;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet as _;
use mc_protocol::packets::play::{PlayDisconnect, SystemChat};
use mc_server::bans::{BANNED_PLAYERS_FILE_NAME, BanList, PlayerBan};
use mc_server::config::{ServerConfig, StorageConfig};
use mc_server::game::{Game, TickReport};
use mc_server::lifecycle::Server;
use mc_server::ops::{OPS_FILE_NAME, OperatorList};
use mc_server::storage::WorldService;
use mc_server::whitelist::{WHITELIST_FILE_NAME, Whitelist};
use mc_test_support::fixtures::TempDir;
use std::path::Path;
use std::time::SystemTime;

/// Vanilla's white-list kick, word for word the join gate's refusal.
const UNLISTED_KICK: &str = "You are not white-listed on this server!";

/// Vanilla's ban screen for a permanent ban, as `ban_message` formats it.
fn ban_screen(reason: &str) -> String {
    format!("You are banned from this server.\nReason: {reason}")
}

/// The uuid the join gate compares for an offline-mode profile.
fn uuid_of(name: &str) -> String {
    mc_network::auth::offline_profile(name).id.to_string()
}

/// An operator list with one entry per `(name, level)` pair.
fn operators_for(entries: &[(&str, u8)]) -> OperatorList {
    let rows: Vec<String> = entries
        .iter()
        .map(|(name, level)| {
            format!(
                r#"{{"uuid": "{}", "name": "{name}", "level": {level}}}"#,
                uuid_of(name)
            )
        })
        .collect();
    OperatorList::parse(&format!("[{}]", rows.join(",")), Path::new(OPS_FILE_NAME))
        .expect("the operator fixture parses")
}

/// A whitelist listing exactly `names` (by their offline uuid).
fn whitelist_for(names: &[&str]) -> Whitelist {
    let rows: Vec<String> = names
        .iter()
        .map(|name| format!(r#"{{"uuid": "{}", "name": "{name}"}}"#, uuid_of(name)))
        .collect();
    Whitelist::from_value(
        &serde_json::from_str(&format!("[{}]", rows.join(","))).expect("json"),
        Path::new(WHITELIST_FILE_NAME),
    )
    .expect("the whitelist fixture parses")
}

/// A permanent player ban row.
fn ban_row(name: &str, reason: &str) -> PlayerBan {
    PlayerBan {
        uuid: uuid_of(name),
        name: name.to_owned(),
        created: SystemTime::now(),
        source: "Warden".to_owned(),
        expires: None,
        reason: reason.to_owned(),
    }
}

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let service = WorldService::open(&config).expect("world opens");
        let (events, rx) = game_channel(64);
        let mut game = Game::build_with_operators(None, Some(service), 3, rx, 7, operators)
            .expect("game builds");
        // Persistence lands beside the world, like `ops.json`: `/ban` must write
        // a real file, or "the row is what refuses the return" would be a claim
        // about memory only.
        game.set_ops_directory(dir.path().to_path_buf());
        Self {
            game,
            events,
            ids: ConnectionIds::new(),
            dir,
        }
    }

    /// Join `name` through the real event channel, like the connection layer.
    ///
    /// The queue is the 8192 slots `execute_permission.rs` uses, not the 64 of
    /// the two-player harnesses: with four clients and nobody draining, a join
    /// burst overflows 64, and `enforce_overflow` then drops that session in
    /// the same tick — which would look like a missing player much later.
    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, inbound) = OutboundSender::pair(id, 8192);
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

    /// The play-phase disconnect reason a client saw, if any. Filtered by
    /// packet id: `PlayDisconnect::decode` is lenient enough to parse a
    /// join-burst packet, so an unfiltered drain can report a phantom kick.
    fn disconnect_reason(inbound: &mut InboundReceiver) -> Option<String> {
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

/// The ban path checks no operator exemption: whoever is named goes, at level
/// 4, at level 0, listed or not — and `/kick` is no different.
#[allow(clippy::too_many_lines)]
#[test]
fn bans_and_kicks_exempt_no_operator_however_listed() {
    let mut harness = Harness::new(
        "audit19-ban-exemption",
        operators_for(&[("Chief", 4), ("Boss", 4), ("Rookie", 0)]),
    );
    let (chief, mut chief_in) = harness.join("Chief");
    let (boss, mut boss_in) = harness.join("Boss");
    let (rookie, rookie_in) = harness.join("Rookie");
    // "Newbie" is in no list at all: the plain level-0 player.
    let (newbie, newbie_in) = harness.join("Newbie");
    // The ladder is real before any ban, so a refusal below is the ban and not
    // a typo that quietly demoted the operator.
    assert_eq!(
        harness.game.player_permission(chief),
        PermissionLevel::Console,
        "the fixture operator holds authority"
    );
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "Rookie is listed at level 0: a listing, and no authority"
    );
    assert!(
        harness.game.has_player(chief)
            && harness.game.has_player(boss)
            && harness.game.has_player(rookie)
            && harness.game.has_player(newbie),
        "all four clients must hold a session before any ban; a missing one means a join \
         was refused or the outbound queue dropped it, not that a ban exempted anybody"
    );

    // `/kick` names a listed level-4 operator. The kick path has no exemption
    // either — and it writes nothing, so the same operator rejoins.
    let lines = harness.command(chief, &mut chief_in, "kick Boss rudeness");
    assert!(
        lines.iter().any(|line| line.contains("Kicked Boss")),
        "kick confirms: {lines:?}"
    );
    harness.game.tick().expect("tick drops the kicked session");
    assert!(
        !harness.game.has_player(boss),
        "a listed level-4 operator is kickable"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut boss_in).as_deref(),
        Some("rudeness"),
        "the kicked operator sees the operator's reason"
    );
    let (boss, mut boss_in) = harness.join("Boss");
    assert!(
        harness.game.has_player(boss),
        "a kick is a moment, not a record: the operator rejoins"
    );

    // `/ban` on the same listed operator: banned, kicked, filed, refused on
    // return. This is the direction the audit could not find a test for.
    let lines = harness.command(chief, &mut chief_in, "ban Boss cheating");
    assert!(
        lines.iter().any(|line| line.contains("Banned Boss")),
        "ban confirms: {lines:?}"
    );
    harness.game.tick().expect("tick drops the banned session");
    assert!(
        !harness.game.has_player(boss),
        "a listed level-4 operator is not exempt from /ban"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut boss_in).as_deref(),
        Some(ban_screen("cheating").as_str()),
        "the banned operator is dropped with the ban screen"
    );
    let filed = std::fs::read_to_string(harness.dir.path().join(BANNED_PLAYERS_FILE_NAME))
        .expect("banned-players.json written");
    assert!(
        filed.contains(&uuid_of("Boss")),
        "the row is what refuses the return, so it must reach the file: {filed}"
    );
    let (again, mut again_in) = harness.join("Boss");
    assert!(
        !harness.game.has_player(again),
        "the join gate refuses a banned operator: no exemption exists"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut again_in).as_deref(),
        Some(ban_screen("cheating").as_str()),
        "the refusal is the ban screen, not a permission or whitelist message"
    );

    // The other directions: a listing at level 0 (which does exempt from the
    // whitelist) and a plain level-0 player are banned exactly the same way, so
    // the exemption is not merely level-gated — it is absent.
    let mut others = vec![("Rookie", rookie, rookie_in), ("Newbie", newbie, newbie_in)];
    for (name, id, inbound) in &mut others {
        let name = *name;
        // A reason is spelled out because the tree's `reason` is a required
        // greedy argument: bare `/ban <player>` is a parse error, so the
        // handler's default-reason path is not reachable from chat.
        let reason = format!("{name} cheated");
        let lines = harness.command(chief, &mut chief_in, &format!("ban {name} {reason}"));
        assert!(
            lines.iter().any(|line| line.contains("Banned")),
            "ban confirms for {name}: {lines:?}"
        );
        harness.game.tick().expect("tick drops the banned session");
        assert!(
            !harness.game.has_player(*id),
            "{name} must be disconnected: the ban gate exempts nobody"
        );
        let expected = ban_screen(&reason);
        assert_eq!(
            Harness::disconnect_reason(inbound).as_deref(),
            Some(expected.as_str()),
            "the kicked {name} sees the ban screen naming the reason"
        );
        let (returning, mut returning_in) = harness.join(name);
        assert!(
            !harness.game.has_player(returning),
            "{name} must be refused on return"
        );
        assert_eq!(
            Harness::disconnect_reason(&mut returning_in).as_deref(),
            Some(expected.as_str()),
            "{name}'s refusal is the ban screen"
        );
    }
}

/// The join gate checks the whitelist **before** the ban list, and the order is
/// player-visible: a banned profile that is not white-listed is told only that
/// it is not white-listed.
#[test]
fn the_whitelist_refusal_wins_over_the_ban_refusal() {
    let mut bans = BanList::new();
    for name in ["Banned", "Both", "Chief"] {
        bans.ban_player(ban_row(name, "griefing"));
    }
    let mut harness = Harness::new("audit19-gate-order", operators_for(&[("Chief", 4)]));
    // "Both" and "Chief" are listed; "Banned" and "Chief" are not, and "Chief"
    // is the operator whose listing buys the whitelist bypass.
    harness
        .game
        .set_whitelist(whitelist_for(&["Listed", "Both"]));
    harness.game.set_whitelist_enforced(true);
    harness.game.set_bans(bans);

    // Banned and unlisted: the whitelist refuses first, so the ban screen is
    // never reached and the player cannot tell a ban from a missing listing.
    let (banned, mut banned_in) = harness.join("Banned");
    assert!(
        !harness.game.has_player(banned),
        "a banned, unlisted profile must not join"
    );
    assert_eq!(
        Harness::disconnect_reason(&mut banned_in).as_deref(),
        Some(UNLISTED_KICK),
        "the whitelist refusal wins for a profile that is both banned and unlisted"
    );

    // Banned and listed: the whitelist admits, so the ban gate is reached and
    // its message is what arrives. Same list, same enforcement, different
    // listing — which is the only reason the order is observable.
    let (both, mut both_in) = harness.join("Both");
    assert!(!harness.game.has_player(both));
    assert_eq!(
        Harness::disconnect_reason(&mut both_in).as_deref(),
        Some(ban_screen("griefing").as_str()),
        "a listed banned profile meets the ban gate, so the ban screen is reachable"
    );

    // The operator case joins the two halves: the whitelist exempts the listing
    // (so the whitelist refusal does not win), and the ban gate then refuses.
    // An operator's listing buys a bypass of the whitelist, never of a ban.
    let (chief, mut chief_in) = harness.join("Chief");
    assert!(!harness.game.has_player(chief));
    assert_eq!(
        Harness::disconnect_reason(&mut chief_in).as_deref(),
        Some(ban_screen("griefing").as_str()),
        "the whitelist bypasses an operator; the ban gate does not"
    );

    // Controls, so neither message above is an artifact of refusing everyone:
    // an unbanned unlisted profile sees the whitelist refusal too, and the one
    // profile both lists admit actually joins.
    let (stranger, mut stranger_in) = harness.join("Stranger");
    assert!(!harness.game.has_player(stranger));
    assert_eq!(
        Harness::disconnect_reason(&mut stranger_in).as_deref(),
        Some(UNLISTED_KICK),
        "an unbanned unlisted profile sees the same whitelist refusal"
    );
    let (listed, _listed_in) = harness.join("Listed");
    assert!(
        harness.game.has_player(listed),
        "an unbanned listed profile joins"
    );
    assert_eq!(
        harness.game.player_count(),
        1,
        "exactly the profile both gates admit is in"
    );
}

/// A `MakeWriter` that keeps what the boot logs in memory, so the test needs no
/// process-wide subscriber (which the other tests in this binary would race).
#[derive(Clone, Default)]
struct Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Logs {
    fn text(&self) -> String {
        let bytes = self.0.lock().expect("the log buffer is not poisoned");
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

struct LogSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("the log buffer is not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        LogSink(std::sync::Arc::clone(&self.0))
    }
}

/// Boot the **real** network phase on `bind` and return what the boot logged.
///
/// `Server::start_network` is the boot path; nothing here calls
/// `exposure_warning_needed` directly, so a warning in the returned text can
/// only have come from the call site inside that function.
async fn boot_logs(bind: &str, whitelist_enforced: bool) -> String {
    let mut config = ServerConfig::default();
    bind.clone_into(&mut config.network.bind);
    config.access.whitelist_enforced = whitelist_enforced;
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let mut server = Server::new(config);
    server
        .start_network()
        .await
        .expect("the boot path binds the listener");
    drop(guard);
    logs.text()
}

/// The boot path calls the exposure predicate — the predicate's own unit tests
/// cannot tell a wired call site from a dead function.
#[tokio::test]
async fn the_boot_path_emits_the_exposure_warning() {
    const WARNING: &str = "anyone on the network can join as anyone";

    // Off loopback, offline auth, whitelist off: the exposure the warning names.
    let exposed = boot_logs("0.0.0.0:0", false).await;
    assert!(
        exposed.contains(WARNING),
        "a non-loopback bind with offline auth and no whitelist must warn from the boot \
         path itself, saw: {exposed:?}"
    );

    // The controls prove the call site consults its arguments rather than
    // logging unconditionally: the same bind with the whitelist enforced is
    // silent, and a loopback bind was never exposed in the first place.
    let enforced = boot_logs("0.0.0.0:0", true).await;
    assert!(
        !enforced.contains(WARNING),
        "an enforced whitelist removes the exposure the warning describes, saw: {enforced:?}"
    );
    let loopback = boot_logs("127.0.0.1:0", false).await;
    assert!(
        !loopback.contains(WARNING),
        "a loopback bind is not reachable off the machine, saw: {loopback:?}"
    );
}
