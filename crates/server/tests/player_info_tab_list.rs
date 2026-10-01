//! The player list (`player_info_update` / `player_info_remove`) reaches the
//! right clients (AUDIT-19 A-04).
//!
//! The defect these tests were written against: the server sent **no** tab-list
//! packet at all. `login_finished` carries the joining player's own profile
//! properties to *that* client, so the tree could claim skins travelled — but
//! nothing ever told anyone about anyone else, which left every client's tab list
//! empty and every other player texture-less.
//!
//! Falsification shape, one per mechanism:
//!
//! * drop the join announcement and no `player_info_update` reaches the other
//!   client (`the_others_receive_a_joining_players_entry`);
//! * drop the newcomer's copy of the existing entries and the joining client
//!   learns about nobody (`a_joining_player_receives_the_entries_already_listed`);
//! * drop the leave announcement and a departed player's entry is never removed
//!   (`a_leaving_player_is_removed_from_the_others_tab_list`);
//! * drop the properties from the entry and the `textures` blob never leaves the
//!   server (`the_entry_carries_the_profile_properties_the_skin_comes_from`).
//!
//! What these tests do **not** establish: that a real 26.1.2 client renders the
//! result. No client is available here; the byte shape is pinned in
//! `crates/protocol/tests/player_info_wire_shape.rs` against the jar and against
//! real captured server bodies, and client acceptance stays on the walk list.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::login::ProfileProperty;
use mc_protocol::packets::play::{
    PlayerInfoEntry, PlayerInfoField, PlayerInfoRemove, PlayerInfoUpdate,
};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    _dir: TempDir,
}

impl Harness {
    /// A harness whose game owns its storage handle, so joins go through the
    /// production shape (the borrowed constructor never generates terrain, and a
    /// joining player would stand in an all-air placeholder chunk).
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        self.join_profile(mc_network::auth::offline_profile(name))
    }

    /// A join carrying a specific profile — the only way to give a session the
    /// `textures` property an online-mode login would have verified.
    fn join_profile(
        &mut self,
        profile: mc_network::auth::GameProfile,
    ) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, out) = OutboundSender::pair(id, 8192);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined { profile, outbound },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        (id, out)
    }

    fn leave(&mut self, id: ConnectionId) {
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Left,
            })
            .expect("leave event queued");
        self.game.tick().expect("tick");
    }
}

/// Drain every `player_info_update` a client received, flattened to entries.
fn updates(out: &mut InboundReceiver) -> Vec<PlayerInfoEntry> {
    let mut seen = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::PLAYER_INFO_UPDATE {
            let packet =
                PlayerInfoUpdate::decode(&raw.payload).expect("the tab-list update decodes");
            assert!(
                !packet.actions.is_empty(),
                "an update with no actions tells a client nothing"
            );
            seen.extend(packet.entries);
        }
    }
    seen
}

/// Drain every `player_info_remove` a client received, flattened to uuids.
fn removals(out: &mut InboundReceiver) -> Vec<uuid::Uuid> {
    let mut seen = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::PLAYER_INFO_REMOVE {
            let packet =
                PlayerInfoRemove::decode(&raw.payload).expect("the tab-list removal decodes");
            seen.extend(packet.profile_ids);
        }
    }
    seen
}

/// The profile name an entry carries, or a panic naming what was there.
fn name_of(entry: &PlayerInfoEntry) -> &str {
    match &entry.profile {
        PlayerInfoField::Value(profile) => &profile.name,
        other => panic!("the entry must carry ADD_PLAYER, found {other:?}"),
    }
}

/// The named pin for the send site: a joining player's entry reaches the client
/// that was already there, carrying the fields a tab list needs.
///
/// Neutralised by removing the `pending_player_info.push(Join)` in
/// `session.rs::join` (or the `broadcast_player_info` call), which is what makes
/// this red rather than vacuous.
#[test]
fn the_others_receive_a_joining_players_entry() {
    let mut harness = Harness::new("pi-join");
    let (_alpha, mut alpha_out) = harness.join("Alpha");
    // Alpha's own join announcement is the precondition, not the measurement:
    // draining it before Beta joins means what follows is exactly Beta's arrival.
    assert!(
        updates(&mut alpha_out)
            .iter()
            .any(|entry| name_of(entry) == "Alpha"),
        "Alpha was listed to itself when it joined"
    );

    let (beta, _beta_out) = harness.join("Beta");
    let _ = harness.game.player(beta).expect("Beta is in the game");
    let beta_profile = mc_network::auth::offline_profile("Beta");
    let entries = updates(&mut alpha_out);
    assert_eq!(
        entries.len(),
        1,
        "Beta's join announces Beta and nobody else, saw {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(entry.uuid, beta_profile.id);
    assert_eq!(
        name_of(entry),
        "Beta",
        "ADD_PLAYER carries the profile name"
    );
    assert_eq!(
        entry.listed,
        PlayerInfoField::Value(true),
        "UPDATE_LISTED must be set, or the client creates the entry unlisted and \
         draws no tab-list row"
    );
    assert_eq!(
        entry.game_mode,
        PlayerInfoField::Value(0),
        "UPDATE_GAME_MODE carries the session's mode (0 = survival)"
    );
    assert_eq!(
        entry.show_hat,
        PlayerInfoField::Value(true),
        "UPDATE_HAT must be set: the client's default is false, which hides the hat layer"
    );
    assert!(
        !entries.iter().any(|entry| name_of(entry) == "Alpha"),
        "the packet Alpha receives for Beta must not also re-announce Alpha"
    );
}
/// The joining client's half of the same moment: it is handed the entries of the
/// players already listed, so its tab list is not limited to itself.
#[test]
fn a_joining_player_receives_the_entries_already_listed() {
    let mut harness = Harness::new("pi-newcomer");
    let (alpha, _alpha_out) = harness.join("Alpha");
    let (_beta, mut beta_out) = harness.join("Beta");
    let _ = harness.game.player(alpha).expect("Alpha is in the game");

    let entries = updates(&mut beta_out);
    let alpha_profile = mc_network::auth::offline_profile("Alpha");
    let alpha_entry = entries
        .iter()
        .find(|entry| entry.uuid == alpha_profile.id)
        .unwrap_or_else(|| panic!("Beta must learn about Alpha, saw {entries:?}"));
    assert_eq!(name_of(alpha_entry), "Alpha");
    // Vanilla sends the player their own entry too (`createPlayerInitializing`
    // covers the whole list and the broadcast follows), so a client is not
    // missing its own name from the tab list.
    let beta_profile = mc_network::auth::offline_profile("Beta");
    assert!(
        entries.iter().any(|entry| entry.uuid == beta_profile.id),
        "Beta receives its own entry as well; saw {entries:?}"
    );
}

/// The departure half: the other client is told to drop the entry.
#[test]
fn a_leaving_player_is_removed_from_the_others_tab_list() {
    let mut harness = Harness::new("pi-leave");
    let (_alpha, mut alpha_out) = harness.join("Alpha");
    let (beta, _beta_out) = harness.join("Beta");
    // The announcement is the precondition: a removal for an entry that was
    // never sent would be invisible either way.
    let beta_profile = mc_network::auth::offline_profile("Beta");
    assert!(
        updates(&mut alpha_out)
            .iter()
            .any(|entry| entry.uuid == beta_profile.id),
        "Beta was listed before leaving"
    );

    harness.leave(beta);

    let gone = removals(&mut alpha_out);
    assert_eq!(
        gone,
        vec![beta_profile.id],
        "Alpha's client must be told Beta's profile id is gone from the tab list"
    );
    assert_eq!(harness.game.player_count(), 1, "and the session is gone");
}

/// The skin: the profile properties from the authenticated login are the only
/// copy of the `textures` blob outside the joining client, and they must ride the
/// tab-list entry to everyone else.
#[test]
fn the_entry_carries_the_profile_properties_the_skin_comes_from() {
    let mut harness = Harness::new("pi-skin");
    let (_alpha, mut alpha_out) = harness.join("Alpha");

    let mut profile = mc_network::auth::offline_profile("Skinner");
    let textures = ProfileProperty {
        name: "textures".to_owned(),
        value: "c2tpbg==".to_owned(),
        signature: Some("sig".to_owned()),
    };
    profile.properties.push(textures.clone());
    let (_skinner, _skinner_out) = harness.join_profile(profile);

    let entries = updates(&mut alpha_out);
    let entry = entries
        .iter()
        .find(|entry| name_of(entry) == "Skinner")
        .unwrap_or_else(|| panic!("Skinner must be listed for Alpha, saw {entries:?}"));
    let PlayerInfoField::Value(announced) = &entry.profile else {
        panic!("the entry must carry ADD_PLAYER");
    };
    assert_eq!(
        announced.properties,
        vec![textures],
        "the textures property is what a client resolves into the skin"
    );
}

/// A server with nobody else in it still lists the joining player to themselves:
/// the alternative (skip the owner, as the player-*entity* announcement does)
/// would leave every player with an empty tab list on a one-player server.
#[test]
fn a_player_joining_an_empty_server_is_listed_to_themselves() {
    let mut harness = Harness::new("pi-alone");
    let (_alpha, mut alpha_out) = harness.join("Alpha");
    let profile = mc_network::auth::offline_profile("Alpha");
    assert!(
        updates(&mut alpha_out)
            .iter()
            .any(|entry| entry.uuid == profile.id),
        "the tab list is built from these entries alone, so a solo player needs their own"
    );
}
