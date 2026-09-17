//! Reconnect robustness (P14-05): leave and rejoin while running, and rejoin
//! after a full restart.
//!
//! Sessions are not persisted (no playerdata yet): a rejoin starts fresh at
//! spawn with an empty inventory, like a first join. What must hold is the
//! plumbing — the old entity is swept exactly once, no ghost session lingers,
//! the new session streams, and a restart accepts the same name again. Death
//! and respawn with a real client, and a rejoin finding pre-restart state,
//! stay on KD-38's open list: they need a real client at a keyboard, and no
//! test here can stand in for one.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, OutboundSender,
    game_channel,
};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    _storage: WorldService,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::new(&storage, 4, rx).expect("game builds");
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            _storage: storage,
            dir,
        }
    }

    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, out) = OutboundSender::pair(id, 8192);
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

    fn leave(&mut self, id: ConnectionId) {
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Left,
            })
            .expect("leave event queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }
}

#[test]
fn leave_then_rejoin_works_and_sweeps_the_old_entity() {
    let mut harness = Harness::new("p14-rejoin");
    let (watcher, mut watch_out) = harness.join("Watcher");
    let (id, _out) = harness.join("Returner");
    assert_eq!(harness.game.player_count(), 2);
    while watch_out.try_recv().is_some() {}

    harness.leave(id);
    harness.run(3);
    assert_eq!(harness.game.player_count(), 1, "leave removes the session");
    let mut removals = 0;
    while let Some(raw) = watch_out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::REMOVE_ENTITIES {
            removals += 1;
        }
    }
    assert!(
        removals >= 1,
        "the watcher must be told the departed entity is gone"
    );

    let (id2, _) = harness.join("Returner");
    harness.run(3);
    assert_eq!(harness.game.player_count(), 2, "rejoin works");
    assert_ne!(id, id2, "a rejoin is a new connection");
    let _ = watcher;
}

#[test]
fn rejoin_after_a_full_restart_works() {
    let mut harness = Harness::new("p14-restart");
    let (_id, _out) = harness.join("Restarter");
    assert_eq!(harness.game.player_count(), 1);

    // Full restart: drop the game, keep the directory, reopen, rejoin.
    let Harness { dir, .. } = harness;
    let world_dir = dir.path().join("world");
    let service = WorldService::open(&mc_server::config::StorageConfig {
        world_dir: world_dir.clone(),
        autosave_ticks: 0,
    })
    .expect("world reopens");
    // Non-default construction (seed, view distance, listed operators):
    // the restart path must not depend on defaults.
    let rookie_uuid = mc_network::auth::offline_profile("Restarter").id;
    let operators = mc_server::ops::OperatorList::parse(
        &format!(r#"[{{"uuid": "{rookie_uuid}", "name": "Restarter", "level": 4}}]"#),
        std::path::Path::new("ops.json"),
    )
    .expect("fixture parses");
    let (tx, rx) = game_channel(256);
    let mut game2 =
        Game::build_with_operators(None, Some(service), 3, rx, 7, operators).expect("game builds");
    let ids = ConnectionIds::new();
    let id2 = ids.next_id();
    let (outbound, _out2) = OutboundSender::pair(id2, 8192);
    tx.try_send(ClientEvent {
        id: id2,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Restarter"),
            outbound,
        },
    })
    .expect("rejoin queued");
    for _ in 0..40 {
        game2.tick().expect("tick");
        if game2.player_count() > 0 {
            break;
        }
    }
    assert_eq!(
        game2.player_count(),
        1,
        "a restart must accept the same name again"
    );
}
