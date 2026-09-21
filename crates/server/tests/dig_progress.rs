//! Digging progress end to end (P16-05): hardness, tool speeds, per-tick
//! accumulation, abort.
//!
//! Through the real tick loop: START names a block, survival ticks
//! accumulate `speed / hardness / divisor` per tick, the block breaks at
//! 1.0, and ABORT (or a held swap, target loss, walking away) cancels.
//! Stages ride `block_destruction` (id 5); -1 clears.
//!
//! Falsification shape: instant-break the START arm again and the
//! still-intact assertions fail; never clear on abort and the overlay
//! assertions fail.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{BlockDestruction, PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

const START: i32 = 0;
const ABORT: i32 = 1;
const FINISH: i32 = 2;

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

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn feet(&self) -> (i32, i32, i32) {
        let p = self.game.player(self.id).expect("player").position;
        (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32)
    }

    fn place(&mut self, x: i32, y: i32, z: i32, block: &str) {
        let id = self
            .game
            .registries()
            .blocks
            .default_state(block)
            .unwrap_or_else(|_| panic!("{block} is a known block"));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("the block is set");
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> i32 {
        self.game.world().get_block_loaded(x, y, z).expect("loaded")
    }

    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.block_at(x, y, z) == self.game.registries().blocks.air_id()
    }

    /// White-box: the harness never moves, so the tracked ground flag stays
    /// false and every dig would run at the mid-air fifth. Standing digs
    /// need the flag a real client reports 20 times a second.
    fn stand(&mut self) {
        self.game.player_mut(self.id).expect("player").on_ground = true;
    }

    fn give(&mut self, item: &str) {
        let id = self
            .game
            .registries()
            .items
            .id(item)
            .unwrap_or_else(|_| panic!("{item} is a known item"));
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, mc_entity::stack::ItemStack::new(id, 1).expect("stack"))
            .expect("slot 0 takes the tool");
    }

    fn action(&mut self, status: i32, x: i32, y: i32, z: i32) {
        self.intent(PlayIntent::PlayerAction {
            status,
            position: block_position(x, y, z),
            facing: 1,
            sequence: 0,
        });
    }

    /// Every crack stage announced since the drain, as raw stages.
    fn stages(out: &mut InboundReceiver) -> Vec<i8> {
        let mut stages = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::BLOCK_DESTRUCTION {
                stages.push(
                    BlockDestruction::decode(&raw.payload)
                        .expect("decodes")
                        .stage,
                );
            }
        }
        stages
    }

    fn ground_items(&self) -> usize {
        self.game.dropped_items().len()
    }
}

#[test]
fn dirt_breaks_on_the_fifteenth_tick_not_the_first() {
    // Dirt (0.5, no tool needed) by hand: 1/0.5/30 per tick, so tick 15.
    let mut harness = Harness::new("p16-dirt");
    let mut out = harness.join("Miner");
    let _ = Harness::stages(&mut out);
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:dirt");
    harness.stand();
    harness.action(START, target.0, target.1, target.2);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "one START no longer breaks: progress just started"
    );
    harness.run(4);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "5 ticks of 15 keep the block"
    );
    harness.run(12);
    assert!(
        harness.is_air(target.0, target.1, target.2),
        "17 ticks break a 15-tick dig"
    );
    let stages = Harness::stages(&mut out);
    assert!(
        stages.iter().any(|stage| (0..=9).contains(stage)),
        "crack stages were announced; saw {stages:?}"
    );
}

#[test]
fn abort_cancels_progress_and_clears_the_overlay() {
    let mut harness = Harness::new("p16-abort");
    let mut out = harness.join("Miner");
    let _ = Harness::stages(&mut out);
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:dirt");
    harness.stand();
    harness.action(START, target.0, target.1, target.2);
    harness.run(10);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "10 of 15 ticks keep the block"
    );
    harness.action(ABORT, target.0, target.1, target.2);
    harness.run(30);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "an aborted dig never completes on its own"
    );
    let stages = Harness::stages(&mut out);
    assert!(
        stages.contains(&-1),
        "abort clears the overlay; saw {stages:?}"
    );
    // And the abort reset rather than paused: a fresh dig needs the full 15.
    harness.action(START, target.0, target.1, target.2);
    harness.run(5);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "a restarted dig runs the full 15 ticks, not the remaining 5"
    );
}

#[test]
fn a_diamond_pick_breaks_stone_in_six_ticks_a_hand_needs_150() {
    let mut harness = Harness::new("p16-tools");
    let mut out = harness.join("Miner");
    let _ = Harness::stages(&mut out);
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:stone");
    harness.stand();
    harness.give("minecraft:diamond_pickaxe");
    harness.action(START, target.0, target.1, target.2);
    harness.run(7);
    assert!(
        harness.is_air(target.0, target.1, target.2),
        "8/1.5/30 breaks stone on the 6th tick"
    );
    let dropped_with_pick = harness.ground_items();
    assert_eq!(
        dropped_with_pick, 1,
        "the harvested pick break drops its cobblestone"
    );
    // Bare hand on the restored stone: 1/1.5/100 per tick.
    harness.place(target.0, target.1, target.2, "minecraft:stone");
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(0, mc_entity::stack::ItemStack::EMPTY)
        .expect("hand emptied");
    // Stand clear of pickup (4 east: out of the 1.0 pickup reach, inside
    // the 5.5 dig reach) so the no-drop assertion below reads the ground.
    let p = harness.game.player(harness.id).expect("player").position;
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .position = mc_world::Vec3::new(p.x + 4.0, p.y, p.z);
    harness.stand();
    harness.action(START, target.0, target.1, target.2);
    harness.run(7);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "7 ticks barely scratch a 150-tick hand dig"
    );
    harness.run(150);
    assert!(
        harness.is_air(target.0, target.1, target.2),
        "157 ticks finish the hand dig"
    );
    assert_eq!(
        harness.ground_items(),
        dropped_with_pick,
        "stone broken by hand drops nothing: the harvest judgment failed"
    );
}

#[test]
fn bedrock_refuses_in_survival() {
    let mut harness = Harness::new("p16-bedrock");
    let mut out = harness.join("Miner");
    let _ = Harness::stages(&mut out);
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:bedrock");
    harness.stand();
    harness.give("minecraft:netherite_pickaxe");
    harness.action(START, target.0, target.1, target.2);
    harness.run(60);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "bedrock (-1.0) never accumulates, whatever the tool"
    );
    assert!(
        Harness::stages(&mut out).is_empty(),
        "a refused dig announces no overlay at all"
    );
}

#[test]
fn a_finish_before_progress_completes_breaks_nothing() {
    // A too-early FINISH is a prediction the ack already closed: it must
    // not spend the dig (that would be START+FINISH instant-break spam).
    let mut harness = Harness::new("p16-finish");
    harness.join("Miner");
    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:dirt");
    harness.stand();
    harness.action(START, target.0, target.1, target.2);
    harness.action(FINISH, target.0, target.1, target.2);
    assert!(
        !harness.is_air(target.0, target.1, target.2),
        "FINISH at ~2/15 progress breaks nothing"
    );
    // The early FINISH kept the dig rather than cancelling it (the client
    // runs a tick ahead of the server by construction), so the remaining
    // ticks still complete it.
    harness.run(13);
    assert!(
        harness.is_air(target.0, target.1, target.2),
        "the kept dig completes on the server's own accumulation"
    );
}
