//! `JoinGame` names the joining player's own entity id.
//!
//! The network layer used to send a hardcoded 1 before the entity store had
//! allocated anything, so every rejoin told the client it was entity 1 while
//! the server simulated it as 18+, and every self-directed packet (effect
//! icons, …) went to an entity the client does not know as itself. The game
//! loop now sends `JoinGame` first in the join burst with the allocated id.
//! Reverting either half fails this: no `JoinGame` at all (network skips it,
//! game does not send it), or 1 twice (old network behaviour).

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::JoinGame;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
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
        let game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn drain(out: &mut InboundReceiver) -> Vec<mc_protocol::RawPacket> {
        let mut packets = Vec::new();
        while let Some(raw) = out.try_recv() {
            packets.push(raw);
        }
        packets
    }

    /// Join and return the `JoinGame` entity id, asserting it leads the burst
    /// and matches the server-side player.
    fn join_burst(&mut self, name: &str) -> i32 {
        let (outbound, mut out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        let packets = Self::drain(&mut out);
        assert!(
            !packets.is_empty(),
            "join burst is never empty (a revert that drops JoinGame fails here)"
        );
        assert_eq!(
            packets[0].id,
            clientbound::play::LOGIN,
            "JoinGame leads the burst — the client keys its self id off the first play packet"
        );
        let join = JoinGame::decode(&packets[0].payload).expect("JoinGame decodes");
        let owned = self.game.player(self.id).expect("player").entity_id;
        assert_eq!(
            join.entity_id, owned,
            "JoinGame names the allocated entity, not a constant"
        );
        join.entity_id
    }

    fn leave(&mut self) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Left,
            })
            .expect("leave event queued");
        self.game.tick().expect("tick");
    }
}

#[test]
fn a_rejoin_gets_its_own_entity_id_in_joingame() {
    let mut harness = Harness::new("joingame-rejoin");
    let first = harness.join_burst("First");
    harness.leave();
    let second = harness.join_burst("Second");
    assert_ne!(
        first, second,
        "entity ids are never reused, so a repeated id is the hardcoded-1 bug"
    );
}
