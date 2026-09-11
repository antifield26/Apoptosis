//! Boundary between the network layer and the game loop (P02-10 → P04-03).
//!
//! ADR-0001 D-03 and the concurrency contract (AGENTS.md section 8) put a
//! **bounded channel** between the Tokio connection tasks and the tick thread:
//!
//! ```text
//! connection task ──ClientEvent──▶ bounded channel ──▶ tick thread (game loop)
//! connection task ◀──RawPacket─── bounded channel ◀── tick thread
//! ```
//!
//! Ownership and guarantees:
//!
//! - **Owner**: the game loop owns the receiving end ([`GameEvents`]) and every
//!   [`OutboundSender`]; the connection task owns its [`InboundReceiver`].
//! - **Primitive**: `tokio::sync::mpsc` with a fixed capacity.
//! - **Ordering**: per connection, events are delivered in the order the socket
//!   produced them; outbound packets in the order the game loop sent them.
//! - **Backpressure**: the connection **never blocks** on the game loop. A full
//!   inbound queue drops the event and counts it ([`InboundReceiver::dropped`]),
//!   so a client that spams packets slows itself down instead of stalling the
//!   tick thread. Outbound uses `try_send` too, and a full outbound queue
//!   disconnects the client rather than growing without bound.
//! - **Failure**: a closed channel means the peer is gone; the connection ends and
//!   the game loop sees [`ClientEvent::Left`].
//! - **Shutdown**: dropped senders make both `recv()` calls return `None`.

use mc_protocol::RawPacket;
use mc_protocol::packets::play::PlayIntent;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// Stable identifier for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "conn#{}", self.0)
    }
}

/// Allocates connection ids.
#[derive(Debug, Default)]
pub struct ConnectionIds {
    next: AtomicU64,
}

impl ConnectionIds {
    /// Create a fresh allocator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    /// Next id.
    pub fn next_id(&self) -> ConnectionId {
        ConnectionId(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

/// Capacity of the inbound (connection → game loop) queue.
///
/// Movement packets arrive at up to 20/s per player; 64 gives several ticks of
/// slack before the drop policy starts shedding.
pub const DEFAULT_INBOUND_CAPACITY: usize = 64;

/// Capacity of the outbound (game loop → connection) queue.
///
/// Chunk bursts are the largest writer: a view-distance-8 join needs ~289 chunk
/// packets. 512 keeps a join from stalling while still bounding memory per player.
pub const DEFAULT_OUTBOUND_CAPACITY: usize = 512;

/// The sending half the game loop keeps for one connection.
#[derive(Debug, Clone)]
pub struct OutboundSender {
    /// Which connection this reaches.
    pub id: ConnectionId,
    sender: mpsc::Sender<RawPacket>,
}

impl OutboundSender {
    /// Create a standalone sender/receiver pair.
    ///
    /// Used by tests and by any in-process transport that wants the game loop's
    /// packet stream without a socket; the receiver is the connection's end.
    #[must_use]
    pub fn pair(id: ConnectionId, capacity: usize) -> (Self, InboundReceiver) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { id, sender }, InboundReceiver { receiver })
    }

    /// Try to queue a packet without waiting.
    ///
    /// # Errors
    ///
    /// Returns the packet back when the queue is full or the connection is gone,
    /// so the caller can decide between dropping the client and dropping the
    /// packet. It never blocks the tick thread.
    pub fn try_send(&self, packet: RawPacket) -> Result<(), RawPacket> {
        self.sender.try_send(packet).map_err(|error| match error {
            mpsc::error::TrySendError::Full(packet) | mpsc::error::TrySendError::Closed(packet) => {
                packet
            }
        })
    }

    /// Whether the connection task is still alive.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.sender.is_closed()
    }

    /// Queue capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.sender.capacity()
    }
}

/// The receiving half a connection task keeps for outbound packets.
#[derive(Debug)]
pub struct InboundReceiver {
    receiver: mpsc::Receiver<RawPacket>,
}

impl InboundReceiver {
    /// Receive the next outbound packet, or `None` once the game loop drops the
    /// sender (connection ended).
    pub async fn recv(&mut self) -> Option<RawPacket> {
        self.receiver.recv().await
    }

    /// Non-blocking receive: `None` when the queue is empty.
    ///
    /// Used by tests and by the connection loop when it wants to drain what is
    /// already queued without awaiting.
    pub fn try_recv(&mut self) -> Option<RawPacket> {
        self.receiver.try_recv().ok()
    }
}

/// The sending half a connection task uses for inbound events.
#[derive(Debug, Clone)]
pub struct EventSender {
    /// Which connection these events belong to.
    pub id: ConnectionId,
    sender: mpsc::Sender<ClientEvent>,
    dropped: Arc<AtomicU64>,
}

impl EventSender {
    /// Queue an event without waiting.
    ///
    /// A full queue drops the event and increments the drop counter rather than
    /// blocking the connection task: a spamming client must not be able to stall
    /// its own socket handling or the tick thread.
    #[must_use]
    pub fn try_send(&self, event: ClientEventKind) -> bool {
        if self
            .sender
            .try_send(ClientEvent {
                id: self.id,
                kind: event,
            })
            .is_ok()
        {
            return true;
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Number of events dropped because the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// One thing a connection tells the game loop.
#[derive(Debug)]
pub struct ClientEvent {
    /// Originating connection.
    pub id: ConnectionId,
    /// What happened.
    pub kind: ClientEventKind,
}

/// Event payloads.
#[derive(Debug)]
pub enum ClientEventKind {
    /// The client finished the configuration handshake and is in play; it can
    /// receive world packets through the attached sender.
    Joined {
        /// Authenticated (or offline-derived) profile.
        profile: crate::auth::GameProfile,
        /// Where the game loop sends packets for this player.
        outbound: OutboundSender,
    },
    /// A decoded gameplay intent.
    Intent(PlayIntent),
    /// A packet the network layer did not model (for observability).
    Unmodelled {
        /// Packet id.
        packet_id: i32,
    },
    /// The connection ended.
    Left,
}

/// The game loop's receiving half.
#[derive(Debug)]
pub struct GameEvents {
    receiver: mpsc::Receiver<ClientEvent>,
}

impl GameEvents {
    /// Receive the next event, or `None` when every connection is gone.
    pub async fn recv(&mut self) -> Option<ClientEvent> {
        self.receiver.recv().await
    }

    /// Non-blocking receive, for draining inside a tick.
    ///
    /// # Errors
    ///
    /// Returns [`tokio::sync::mpsc::error::TryRecvError`] when the queue is empty
    /// (the common case) or every sender is gone.
    pub fn try_recv(&mut self) -> Result<ClientEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Drain everything currently queued, bounded by `limit`.
    ///
    /// The bound matters: one tick must not spend unbounded time on a client that
    /// queued thousands of movement packets.
    pub fn drain(&mut self, limit: usize) -> Vec<ClientEvent> {
        let mut out = Vec::new();
        while out.len() < limit {
            match self.receiver.try_recv() {
                Ok(event) => out.push(event),
                Err(_) => break,
            }
        }
        out
    }
}

/// Create the inbound/outbound halves for one connection.
///
/// Returns the sender the connection reports through, the receiver it reads
/// outbound packets from, and the game loop's view of the pair.
#[must_use]
pub fn channel(
    id: ConnectionId,
    events: mpsc::Sender<ClientEvent>,
    dropped: Arc<AtomicU64>,
    outbound_capacity: usize,
) -> (EventSender, InboundReceiver, OutboundSender) {
    let (tx, rx) = mpsc::channel(outbound_capacity);
    (
        EventSender {
            id,
            sender: events,
            dropped,
        },
        InboundReceiver { receiver: rx },
        OutboundSender { id, sender: tx },
    )
}

/// Create the game loop's end of the event channel.
#[must_use]
pub fn game_channel(capacity: usize) -> (mpsc::Sender<ClientEvent>, GameEvents) {
    let (tx, rx) = mpsc::channel(capacity);
    (tx, GameEvents { receiver: rx })
}
