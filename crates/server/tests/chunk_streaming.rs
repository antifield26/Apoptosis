//! Chunk unload/forget and runtime view distance end to end (P14-04).
//!
//! Two claims, both observable on the wire. First: when the server unloads a
//! chunk a client was shown, the client gets `forget_level_chunk` naming it
//! (before this phase the server dropped it from `sent_chunks` silently and
//! the client kept a ghost). Second: the client's `client_information` view
//! distance reaches the game loop as an event — clamped to the server
//! maximum, confirmed with `set_chunk_cache_radius`, and honoured by later
//! streaming (every chunk a radius-2 client receives sits within 2 of it).

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{ForgetLevelChunk, LevelChunkWithLight, SetChunkCacheRadius};
use mc_server::game::{Game, TickReport};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// Everything a test needs: a live game on real terrain and the client's end
/// of the channel.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: mc_network::bridge::ConnectionIds,
    _storage: WorldService,
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
        let game = Game::new(&storage, 4, rx).expect("game builds");
        Self {
            game,
            events: tx,
            ids: mc_network::bridge::ConnectionIds::new(),
            _storage: storage,
            _dir: dir,
        }
    }

    /// Join `name` and return its connection plus the packet channel.
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

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn command(&mut self, id: ConnectionId, text: &str) {
        let mut report = TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
    }

    /// The player's chunk. `floor` runs first, so each float is already
    /// integral: the narrowing cast below cannot truncate a fraction.
    #[allow(clippy::cast_possible_truncation)]
    fn player_chunk(&self, id: ConnectionId) -> (i32, i32) {
        let position = self.game.player(id).expect("player").position;
        (
            position.x.floor() as i32 >> 4,
            position.z.floor() as i32 >> 4,
        )
    }
}

#[test]
fn teleporting_away_forgets_the_departed_chunks_by_name() {
    let mut harness = Harness::new("p14-forget");
    let (id, mut out) = harness.join("Walker");
    harness.run(10);
    let (home_x, home_z) = harness.player_chunk(id);
    // Drain the join burst: only forgets from here on are evidence.
    while out.try_recv().is_some() {}

    harness.command(id, "tp Walker 600 80 0");
    harness.run(5);

    let mut forgot = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::FORGET_LEVEL_CHUNK {
            let packet = ForgetLevelChunk::decode(&raw.payload).expect("a forget decodes");
            forgot.push((packet.x, packet.z));
        }
    }
    assert!(
        !forgot.is_empty(),
        "leaving the area must forget chunks, saw nothing"
    );
    assert!(
        forgot.contains(&(home_x, home_z)),
        "the spawn chunk {home_x},{home_z} must be forgotten, saw {forgot:?}"
    );
}

#[test]
fn view_distance_is_clamped_confirmed_and_honoured() {
    let mut harness = Harness::new("p14-view-distance");
    let (id, mut out) = harness.join("Viewer");

    let send_view_distance = |harness: &mut Harness, distance: i8| {
        harness
            .events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::ViewDistance { distance },
            })
            .expect("event queued");
        harness.run(2);
    };
    let drain_radius = |out: &mut InboundReceiver| {
        let mut radii = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::SET_CHUNK_CACHE_RADIUS {
                radii.push(
                    SetChunkCacheRadius::decode(&raw.payload)
                        .expect("a radius decodes")
                        .radius,
                );
            }
        }
        radii
    };

    send_view_distance(&mut harness, 2);
    assert_eq!(
        drain_radius(&mut out),
        vec![2],
        "a lower setting is confirmed back"
    );

    // Above the server maximum (4 here) clamps rather than applying.
    send_view_distance(&mut harness, 100);
    assert_eq!(
        drain_radius(&mut out),
        vec![4],
        "the server maximum caps the request"
    );

    // Unchanged settings stay silent; unknown sessions are ignored.
    send_view_distance(&mut harness, 100);
    assert!(drain_radius(&mut out).is_empty(), "no change, no confirm");
    harness
        .events
        .try_send(ClientEvent {
            id: ConnectionId(999),
            kind: ClientEventKind::ViewDistance { distance: 2 },
        })
        .expect("event queued");
    harness.run(2);
    assert!(
        drain_radius(&mut out).is_empty(),
        "an unknown session must not conjure a confirm"
    );

    // Behaviour, not just packets: back to radius 2, teleported to fresh
    // land, every chunk the client receives sits within 2 of it.
    send_view_distance(&mut harness, 2);
    assert_eq!(
        drain_radius(&mut out),
        vec![2],
        "back to 2 for the behavioural half"
    );
    while out.try_recv().is_some() {}
    harness.command(id, "tp Viewer 600 80 0");
    harness.run(3);
    let (cx, cz) = harness.player_chunk(id);
    let mut beyond = 0;
    let mut total = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::LEVEL_CHUNK_WITH_LIGHT {
            let packet = LevelChunkWithLight::decode(&raw.payload).expect("a chunk decodes");
            total += 1;
            if (packet.chunk_x - cx).abs() > 2 || (packet.chunk_z - cz).abs() > 2 {
                beyond += 1;
            }
        }
    }
    assert!(total > 0, "fresh land must stream");
    assert_eq!(
        beyond, 0,
        "{beyond} of {total} chunks arrived from outside radius 2"
    );
}
