//! M-2: the server must acknowledge the client's block predictions.
//!
//! ## The defect these tests pin
//!
//! The owner's P11-10 acceptance round reported "survival mining does not break
//! blocks". The capture was first read as "the dig packets are malformed and every
//! dig is dropped" — that reading counted the packet-id byte as part of the body
//! (`body_bytes` in the rig trace includes it, so a 1-byte
//! `serverbound:accept_teleportation` shows as `3`). Re-deriving from the trace
//! itself (`target/verify/dig_correlation.py`) shows the opposite: 24
//! `player_action` packets in 12 start/finish pairs, and **12 clientbound
//! `block_update` packets at exactly those 12 positions**. The digs were not
//! dropped; the blocks were broken.
//!
//! What the client could not do was *believe* it. Mining opens a prediction
//! (`MultiPlayerGameMode.startDestroyBlock` → `startPrediction`, which raises
//! `BlockStatePredictionHandler.currentSequenceNr` and sends `player_action` with
//! that number). Every server block change then goes through
//! `ClientLevel.setServerVerifiedBlockState`:
//!
//! ```text
//! if (!this.blockStatePredictionHandler.updateKnownServerState(pos, state)) {
//!     super.setBlock(pos, state, flags, 512);
//! }
//! ```
//!
//! `updateKnownServerState` returns **true** while a prediction is pending at that
//! position, so the server's `air` is stored and never applied. The only thing
//! that clears the entry is `endPredictionsUpTo(sequence)`, called from
//! `ClientPacketListener.handleBlockChangedAck` — i.e. from
//! `block_changed_ack`. This server never sent one, so the mined block stayed
//! stone on screen forever, and every later change at that position was swallowed
//! with it.
//!
//! Vanilla's rule (`ServerGamePacketListenerImpl`, bytecode):
//! `ackBlockChangesUpTo(sequence)` keeps `Math.max(sequence, ackBlockChangesUpTo)`
//! — rejecting a negative with `IllegalArgumentException` — and `tick()` sends one
//! `ClientboundBlockChangedAckPacket` per tick while the mark is `> -1`, then
//! resets it. `handlePlayerAction`, `handleUseItemOn` and `handleUseItem` all feed
//! it.
//!
//! ## What these tests do not prove
//!
//! That a real client's prediction actually clears. That is the client's own
//! behaviour, read here from its bytecode; the acceptance round is what confirms
//! it on screen. What is proved here is that the server sends the packet vanilla
//! sends, carrying the sequence the client sent.

// The harness floors a finite player position to a block coordinate, which is what
// `floor` is for; the world is far inside `i32` range.
#![allow(clippy::cast_possible_truncation)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::RawPacket;
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{BlockChangedAck, PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

/// A live game with one joined player, and the receiving end of that player's
/// outbound queue.
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

    /// Queue an intent, then run one tick — the same path the network layer uses.
    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    /// Every `(id, payload)` the client was sent, drained.
    fn drain(out: &mut InboundReceiver) -> Vec<RawPacket> {
        let mut packets = Vec::new();
        while let Some(raw) = out.try_recv() {
            packets.push(raw);
        }
        packets
    }

    /// The sequences of every `block_changed_ack` in a drained batch, in order.
    fn ack_sequences(packets: &[RawPacket]) -> Vec<i32> {
        packets
            .iter()
            .filter(|raw| raw.id == clientbound::play::BLOCK_CHANGED_ACK)
            .map(|raw| {
                BlockChangedAck::decode(&raw.payload)
                    .expect("a block_changed_ack we sent must decode")
                    .sequence
            })
            .collect()
    }

    /// The block-space position of the player's feet.
    fn feet(&self) -> (i32, i32, i32) {
        let position = self.game.player(self.id).expect("player").position;
        (
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        )
    }

    /// Put one `block` at exactly `(x, y, z)`, loading the chunk first.
    fn place(&mut self, x: i32, y: i32, z: i32, block: &str) -> i32 {
        assert!(
            self.game
                .load_chunk(mc_world::ChunkPos::new(x >> 4, z >> 4)),
            "the chunk at ({x}, {z}) loads"
        );
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
        id
    }

    /// A survival dig exactly as a real client sends it: `player_action` status 0
    /// (start digging) with the prediction sequence the client is on.
    fn dig_with_sequence(&mut self, x: i32, y: i32, z: i32, sequence: i32) {
        self.intent(PlayIntent::PlayerAction {
            status: 0,
            position: block_position(x, y, z),
            facing: 1,
            sequence,
        });
    }
}

/// The protocol half: the packet is one `VarInt` and nothing else.
///
/// Independent of the server, and derived from `ClientboundBlockChangedAckPacket`'s
/// own `write` (`FriendlyByteBuf.writeVarInt(sequence)`), so a change that made the
/// body wider would fail here rather than on a client.
#[test]
fn block_changed_ack_is_a_single_varint() {
    for sequence in [0, 1, 2, 127, 128, 300, 16_383, 16_384, i32::MAX] {
        let body = BlockChangedAck { sequence }.encode().expect("encodes");
        // The width is the VarInt's, which is the only field the packet has.
        let mut expected = Vec::new();
        mc_protocol::varint::write_varint(&mut expected, sequence);
        assert_eq!(
            body, expected,
            "sequence {sequence} must encode as one VarInt"
        );
        assert_eq!(
            BlockChangedAck::decode(&body).expect("decodes"),
            BlockChangedAck { sequence }
        );
    }
    // Trailing bytes are refused, like every other packet in this crate.
    assert!(BlockChangedAck::decode(&[0x01, 0x02]).is_err());
    assert_eq!(clientbound::play::BLOCK_CHANGED_ACK, 4);
}

/// **The regression test for M-2.**
///
/// A dig carrying sequence 7 must produce a `block_changed_ack` carrying 7. Delete
/// the send (or drop the sequence on the way in) and this fails.
#[test]
fn a_dig_is_acknowledged_with_the_sequence_the_client_sent() {
    let mut harness = Harness::new("m2-ack");
    let mut out = harness.join("Miner");
    // The join burst is not what this test is about.
    let _ = Harness::drain(&mut out);

    let (fx, fy, fz) = harness.feet();
    let target = (fx, fy - 1, fz);
    harness.place(target.0, target.1, target.2, "minecraft:stone");

    harness.dig_with_sequence(target.0, target.1, target.2, 7);
    let packets = Harness::drain(&mut out);

    // The break still happened: this test must not be satisfied by a server that
    // refuses the dig and acks it anyway.
    assert_eq!(
        harness
            .game
            .world()
            .get_block_loaded(target.0, target.1, target.2),
        Some(harness.game.registries().blocks.air_id()),
        "the dig cleared the block"
    );
    assert_eq!(
        Harness::ack_sequences(&packets),
        vec![7],
        "exactly one ack, carrying the client's own sequence"
    );

    // And the ack comes **after** the block update it is about, in the same
    // per-connection stream. The other order would have the client close its
    // prediction against the pre-dig state and re-apply the old block.
    let update_at = packets
        .iter()
        .position(|raw| raw.id == clientbound::play::BLOCK_UPDATE)
        .expect("the dig queued a block_update");
    let ack_at = packets
        .iter()
        .position(|raw| raw.id == clientbound::play::BLOCK_CHANGED_ACK)
        .expect("the dig queued a block_changed_ack");
    assert!(
        update_at < ack_at,
        "block_update must precede block_changed_ack in the player's stream"
    );
}

/// The high-water mark: one ack per tick, carrying the **maximum** sequence.
///
/// Vanilla keeps `Math.max` and sends one packet per tick, which is what bounds a
/// client that spams `player_action`. Three digs in one tick must produce **one**
/// ack, carrying the largest sequence.
///
/// **The sequences are ordered so that every candidate rule gives a different
/// answer**, which is the whole design of this test:
///
/// | rule | ack it would send |
/// |---|---|
/// | `Math.max` (vanilla, and what this server does) | **11** |
/// | keep the first | 3 |
/// | keep the last | 7 |
///
/// Two earlier versions were weaker and both were caught rather than reasoned
/// about. Sending 3 then 11 made "max" and "last" agree, which `target/m_probes.py`
/// probe M-2c reported as `*** PASSED -- TEST NOT LOAD-BEARING ***`; sending 11 then
/// 3 (the second version) made "max" and "first" agree, which AUDIT-11 §4.1 found by
/// reading. Ascending-then-descending is the shape that separates all three.
#[test]
fn three_digs_in_one_tick_collapse_into_one_ack_carrying_the_highest_sequence() {
    let mut harness = Harness::new("m2-ack-max");
    let mut out = harness.join("Miner");
    let _ = Harness::drain(&mut out);

    let (fx, fy, fz) = harness.feet();
    // Three separate blocks, so each dig is its own action rather than a repeat.
    let first = (fx, fy - 1, fz);
    let highest = (fx, fy - 2, fz);
    let last = (fx, fy - 3, fz);
    for target in [first, highest, last] {
        harness.place(target.0, target.1, target.2, "minecraft:stone");
    }

    // Queued before the tick that applies them (`intent` would tick after each,
    // which would ack them separately): 3, then 11, then 7.
    for (target, sequence) in [(first, 3), (highest, 11), (last, 7)] {
        harness
            .events
            .try_send(ClientEvent {
                id: harness.id,
                kind: ClientEventKind::Intent(PlayIntent::PlayerAction {
                    status: 0,
                    position: block_position(target.0, target.1, target.2),
                    facing: 1,
                    sequence,
                }),
            })
            .expect("intent queued");
    }
    harness.game.tick().expect("tick");
    let packets = Harness::drain(&mut out);

    assert_eq!(
        Harness::ack_sequences(&packets),
        vec![11],
        "one ack per tick at the high-water mark: 11 (the max), not 3 (the first) \
         and not 7 (the last)"
    );

    // A second consecutive ack-less tick must not repeat it: the mark resets.
    harness.game.tick().expect("tick");
    assert!(
        Harness::ack_sequences(&Harness::drain(&mut out)).is_empty(),
        "the mark resets after sending, exactly like vanilla's tick()"
    );
}

/// A refused action is still acknowledged, and a hostile sequence is not.
///
/// Both halves come from the same vanilla method: it acks on the way in (before
/// the action is validated) and throws on a negative. A server that let the
/// negative through would be acknowledging a sequence the client never sent; a
/// server that only acked successful digs would leave a client whose dig was
/// refused frozen on that block forever.
#[test]
fn a_refused_dig_is_acknowledged_but_a_negative_sequence_is_not() {
    let mut harness = Harness::new("m2-ack-refused");
    let mut out = harness.join("Miner");
    let _ = Harness::drain(&mut out);

    // Far outside the interaction range, so the break is refused before it reaches
    // the world.
    let (fx, fy, fz) = harness.feet();
    let far = (fx + 400, fy, fz);
    harness.dig_with_sequence(far.0, far.1, far.2, 5);
    assert_eq!(
        Harness::ack_sequences(&Harness::drain(&mut out)),
        vec![5],
        "a refused dig must still close the client's prediction"
    );

    // A negative sequence is not a sequence. Vanilla throws here; this server must
    // not panic on a hostile client, so it refuses the value instead.
    harness.dig_with_sequence(fx, fy - 1, fz, -1);
    let after = Harness::drain(&mut out);
    assert!(
        Harness::ack_sequences(&after).is_empty(),
        "a negative sequence must not become an ack"
    );
    // And the -1 never lowered the mark that a real sequence had already raised.
    harness.dig_with_sequence(fx, fy - 2, fz, 9);
    assert_eq!(
        Harness::ack_sequences(&Harness::drain(&mut out)),
        vec![9],
        "the high-water mark only ever rises"
    );
}
