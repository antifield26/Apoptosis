//! Phase wiring for slice 2b: the `RandomTicks` sweep reaches the stalk and
//! spread handlers (P20-02).
//!
//! Exactly one full-tick test lives here: cane growing through the phase
//! proves dispatch → handler integration. Every mechanism (grow/climb/cap
//! for cane and cactus, starve/spread/refusal for grass, mycelium spread)
//! is pinned by direct calls in `game::growth::tests` — same handlers, no
//! sweep, ~0.3 s for all ten. See TEST-TIME-PLAN §2 for why the split
//! exists; the unit pins carry the perturbation proofs, this one carries
//! the phase proof.

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    /// View distance 2 (TEST-TIME-PLAN §3): the per-cell hit rate is
    /// invariant to the loaded-chunk count, so statistics are identical
    /// while the sweep covers 25 chunks instead of 81.
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
            Game::with_seed_and_storage(storage, 2, rx, DEFAULT_RANDOM_SEED).expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
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
        out
    }

    /// Stand the player in the block at `(x, y, z)`.
    fn stand(&mut self, x: i32, y: i32, z: i32) {
        let player = self.game.player_mut(self.id).expect("player");
        player.position = mc_world::Vec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
    }
}

/// Cane grows through the phase (the wiring proof; mechanisms are unit-pinned).
#[test]
fn cane_grows_through_the_phase() {
    let mut harness = Harness::new("spread-wiring");
    harness.join("Watcher");
    assert!(harness.game.load_chunk(ChunkPos::new(0, 0)), "field loads");
    assert!(harness.game.load_chunk(ChunkPos::new(1, 0)), "field loads");
    let blocks = harness.game.registries().blocks.clone();
    let sand = blocks.default_state("minecraft:sand").expect("sand");
    for i in 0..40 {
        let x = (i % 20) - 2;
        let z = (i / 20) - 2;
        harness
            .game
            .world_mut()
            .set_block(x, 120, z, sand)
            .expect("sand placed");
        let stalk = blocks
            .state_id(
                "minecraft:sugar_cane",
                &[("age".to_owned(), "15".to_owned())],
            )
            .expect("max-age cane");
        harness
            .game
            .world_mut()
            .set_block(x, 121, z, stalk)
            .expect("cane placed");
    }
    harness.stand(8, 121, -2);
    for _ in 0..200 {
        harness.game.tick().expect("tick");
    }
    let mut tall = 0_usize;
    for i in 0..40 {
        let x = (i % 20) - 2;
        let z = (i / 20) - 2;
        let mut height = 0_usize;
        for dy in 0..4 {
            let Some(id) = harness.game.world().get_block_loaded(x, 121 + dy, z) else {
                break;
            };
            if blocks.block_name(id).unwrap_or("") != "minecraft:sugar_cane" {
                break;
            }
            height += 1;
        }
        if height >= 2 {
            tall += 1;
        }
    }
    assert!(
        tall >= 2,
        "the phase must deliver cane ticks (≈6 first-hits over the run), only {tall}/40 grew"
    );
}
