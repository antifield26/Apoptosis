//! Client-capture rig (P10-01).
//!
//! A TCP proxy that relays a real client's Minecraft 26.1.2 conversation to the server **byte for byte**
//! while writing a normalized JSONL trace of every packet, both directions. It is the differential-testing
//! contract of CONVENTIONS.md section 12 applied to clients instead of servers: a real-client session
//! becomes a comparable artefact.
//!
//! # The observer is passive
//!
//! Forwarding never depends on decoding. If framing cannot be understood for a direction — an unexpected
//! packet, a compression transition tracked wrongly, a protocol subtlety we got wrong — that direction
//! **degrades to opaque passthrough**: bytes keep flowing, framing is abandoned, and the trace records an
//! `observer_error` plus a `degraded` list on `session_end`. A measurement tool that breaks the session it
//! measures is worse than no tool, so the failure mode is chosen rather than inherited.
//!
//! # What "normalized" means here
//!
//! Each record carries the packet id, the state it was interpreted in, and a digest of its **uncompressed**
//! body. Two sessions differing only in compression produce identical digests, which is the point: compare
//! semantics, not wire incidentals. `wire_bytes` keeps the framed size where the wire form matters.
//!
//! Timestamps are deliberately **absent** from per-packet records. A trace is meant to be diffable between
//! runs and a wall-clock field would make every diff non-empty. Duration is recorded once, on `session_end`.
//!
//! # Names
//!
//! `name` resolves for the ids in [`NAMES`] and is `null` otherwise — partial by design, with an unknown id
//! reported as unknown rather than guessed. Every entry is written as an `mc_protocol::ids` constant rather
//! than a transcribed number, so a name cannot drift from the id it labels.

pub mod relay;

pub use relay::{relay_pair, serve};

use mc_protocol::RawPacket;
use mc_protocol::framing::FrameCodec;
use mc_protocol::ids::{clientbound, serverbound};
use md5::{Digest, Md5};
use serde::Serialize;
use std::io::Write;

/// Which way a frame was travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Direction {
    /// Client to server.
    #[serde(rename = "c2s")]
    ClientToServer,
    /// Server to client.
    #[serde(rename = "s2c")]
    ServerToClient,
}

impl Direction {
    /// Short label used in the trace.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientToServer => "c2s",
            Self::ServerToClient => "s2c",
        }
    }
}

/// Connection state. A packet id means different things in each state, so an id cannot be interpreted
/// without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum State {
    /// Handshake.
    #[serde(rename = "handshake")]
    Handshake,
    /// Server-list ping.
    #[serde(rename = "status")]
    Status,
    /// Login.
    #[serde(rename = "login")]
    Login,
    /// Configuration.
    #[serde(rename = "config")]
    Configuration,
    /// Play.
    #[serde(rename = "play")]
    Play,
}

impl State {
    /// Short label used in the trace.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Handshake => "handshake",
            Self::Status => "status",
            Self::Login => "login",
            Self::Configuration => "config",
            Self::Play => "play",
        }
    }
}

/// The ids this rig can name.
///
/// Written as `mc_protocol::ids` constants, so an entry cannot label the wrong id. **Partial on purpose:**
/// it covers the packets this project models plus the join sequence, not all ~90 of every state.
pub const NAMES: &[(Direction, State, i32, &str)] = &[
    // handshake
    (
        Direction::ClientToServer,
        State::Handshake,
        serverbound::handshake::INTENTION,
        "intention",
    ),
    // status
    (
        Direction::ClientToServer,
        State::Status,
        serverbound::status::STATUS_REQUEST,
        "status_request",
    ),
    (
        Direction::ClientToServer,
        State::Status,
        serverbound::status::PING_REQUEST,
        "ping_request",
    ),
    (
        Direction::ServerToClient,
        State::Status,
        clientbound::status::STATUS_RESPONSE,
        "status_response",
    ),
    (
        Direction::ServerToClient,
        State::Status,
        clientbound::status::PONG_RESPONSE,
        "pong_response",
    ),
    // login
    (
        Direction::ClientToServer,
        State::Login,
        serverbound::login::HELLO,
        "login_start",
    ),
    (
        Direction::ClientToServer,
        State::Login,
        serverbound::login::KEY,
        "encryption_response",
    ),
    (
        Direction::ClientToServer,
        State::Login,
        serverbound::login::LOGIN_ACKNOWLEDGED,
        "login_acknowledged",
    ),
    (
        Direction::ServerToClient,
        State::Login,
        clientbound::login::LOGIN_DISCONNECT,
        "login_disconnect",
    ),
    (
        Direction::ServerToClient,
        State::Login,
        clientbound::login::HELLO,
        "encryption_request",
    ),
    (
        Direction::ServerToClient,
        State::Login,
        clientbound::login::LOGIN_FINISHED,
        "login_finished",
    ),
    (
        Direction::ServerToClient,
        State::Login,
        clientbound::login::LOGIN_COMPRESSION,
        "set_compression",
    ),
    // configuration
    (
        Direction::ClientToServer,
        State::Configuration,
        serverbound::config::CLIENT_INFORMATION,
        "client_information",
    ),
    (
        Direction::ClientToServer,
        State::Configuration,
        serverbound::config::KEEP_ALIVE,
        "config_keep_alive",
    ),
    (
        Direction::ClientToServer,
        State::Configuration,
        serverbound::config::FINISH_CONFIGURATION,
        "finish_configuration",
    ),
    (
        Direction::ServerToClient,
        State::Configuration,
        clientbound::config::DISCONNECT,
        "config_disconnect",
    ),
    (
        Direction::ServerToClient,
        State::Configuration,
        clientbound::config::KEEP_ALIVE,
        "config_keep_alive",
    ),
    (
        Direction::ServerToClient,
        State::Configuration,
        clientbound::config::FINISH_CONFIGURATION,
        "finish_configuration",
    ),
    // play
    (
        Direction::ClientToServer,
        State::Play,
        serverbound::play::KEEP_ALIVE,
        "keep_alive",
    ),
    (
        Direction::ClientToServer,
        State::Play,
        serverbound::play::CONFIGURATION_ACKNOWLEDGED,
        "configuration_acknowledged",
    ),
    (
        Direction::ServerToClient,
        State::Play,
        clientbound::play::LOGIN,
        "join_game",
    ),
    (
        Direction::ServerToClient,
        State::Play,
        clientbound::play::KEEP_ALIVE,
        "keep_alive",
    ),
    (
        Direction::ServerToClient,
        State::Play,
        clientbound::play::DISCONNECT,
        "play_disconnect",
    ),
];

/// Resolve a packet id to a name, or `None` when this rig does not model it.
#[must_use]
pub fn packet_name(direction: Direction, state: State, id: i32) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(dir, st, candidate, _)| *dir == direction && *st == state && *candidate == id)
        .map(|(_, _, _, name)| *name)
}

/// A packet observed on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct PacketRecord {
    /// Monotonic sequence across both directions.
    pub seq: u64,
    /// Which way it travelled.
    pub dir: Direction,
    /// State it was interpreted in.
    pub state: State,
    /// Wire packet id.
    pub id: i32,
    /// Resolved name, absent when this rig does not model the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'static str>,
    /// Bytes this frame occupied on the wire, compression included.
    pub wire_bytes: usize,
    /// Bytes of the uncompressed body (id + payload).
    pub body_bytes: usize,
    /// Whether the frame arrived compressed.
    pub compressed: bool,
    /// MD5 of the uncompressed body. A comparison fingerprint, **not** a security primitive.
    pub digest: String,
    /// Leading body bytes, hex, for eyeballing a trace.
    pub head: String,
}

/// One line of the trace.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEvent {
    /// A connection was accepted.
    SessionStart {
        /// Always 0: the first line of a session.
        seq: u64,
        /// Where the client connected from.
        peer: String,
        /// Where the rig forwards to.
        upstream: String,
    },
    /// A packet was relayed.
    Packet(PacketRecord),
    /// Framing failed for a direction; bytes still flow, interpretation stops.
    ObserverError {
        /// Sequence number.
        seq: u64,
        /// The affected direction.
        dir: Direction,
        /// What went wrong.
        error: String,
    },
    /// The session ended.
    SessionEnd {
        /// Sequence number.
        seq: u64,
        /// Frames relayed client to server.
        c2s_frames: u64,
        /// Frames relayed server to client.
        s2c_frames: u64,
        /// Bytes relayed client to server.
        c2s_bytes: u64,
        /// Bytes relayed server to client.
        s2c_bytes: u64,
        /// Wall-clock duration, recorded once here rather than per packet.
        duration_ms: u64,
        /// Directions that degraded to opaque passthrough.
        degraded: Vec<&'static str>,
    },
}

/// How many body bytes a record keeps as a readable head.
const HEAD_BYTES: usize = 24;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The body of a packet: its id as a `VarInt`, then the payload.
fn body_of(packet: &RawPacket) -> Vec<u8> {
    let mut body = Vec::with_capacity(packet.payload.len() + 5);
    let _ = mc_protocol::varint::write_varint(&mut body, packet.id);
    body.extend_from_slice(&packet.payload);
    body
}

/// One direction's framing state.
#[derive(Debug)]
struct Observer {
    codec: FrameCodec,
    /// Set once framing fails; the direction then forwards opaquely.
    degraded: Option<String>,
    /// Whether the failure has been written to the trace yet.
    degraded_reported: bool,
}

impl Observer {
    fn new() -> Self {
        Self {
            codec: FrameCodec::new(),
            degraded: None,
            degraded_reported: false,
        }
    }

    /// Feed bytes and return each complete frame's `(wire_bytes, packet)`.
    ///
    /// The consumed count comes from `FrameCodec::buffered()` deltas, so a caller can forward exactly the
    /// bytes a frame occupied without re-encoding it.
    fn push(&mut self, chunk: &[u8]) -> Vec<(usize, RawPacket)> {
        if self.degraded.is_some() {
            return Vec::new();
        }
        if let Err(error) = self.codec.feed(chunk) {
            self.degraded = Some(error.to_string());
            return Vec::new();
        }
        let mut frames = Vec::new();
        loop {
            let before = self.codec.buffered();
            match self.codec.try_next() {
                Ok(Some(packet)) => {
                    let consumed = before.saturating_sub(self.codec.buffered());
                    frames.push((consumed, packet));
                }
                Ok(None) => break,
                Err(error) => {
                    self.degraded = Some(error.to_string());
                    break;
                }
            }
        }
        frames
    }
}

/// The trace writer plus the session state machine.
pub struct Session {
    sink: Box<dyn Write + Send>,
    seq: u64,
    state: State,
    compression: Option<i32>,
    c2s: Observer,
    s2c: Observer,
    c2s_frames: u64,
    s2c_frames: u64,
    c2s_bytes: u64,
    s2c_bytes: u64,
    write_error: Option<String>,
}

impl Session {
    /// Create a session writing JSONL to `sink`.
    #[must_use]
    pub fn new(sink: Box<dyn Write + Send>) -> Self {
        Self {
            sink,
            seq: 0,
            state: State::Handshake,
            compression: None,
            c2s: Observer::new(),
            s2c: Observer::new(),
            c2s_frames: 0,
            s2c_frames: 0,
            c2s_bytes: 0,
            s2c_bytes: 0,
            write_error: None,
        }
    }

    /// The state the session is currently in.
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    /// The negotiated compression threshold, once `SetCompression` has been observed.
    #[must_use]
    pub const fn compression(&self) -> Option<i32> {
        self.compression
    }

    /// Record the opening line.
    pub fn start(&mut self, peer: &str, upstream: &str) {
        let seq = self.next();
        self.emit(&TraceEvent::SessionStart {
            seq,
            peer: peer.to_owned(),
            upstream: upstream.to_owned(),
        });
    }

    /// Record the closing line.
    pub fn end(&mut self, duration_ms: u64) {
        let mut degraded = Vec::new();
        if self.c2s.degraded.is_some() {
            degraded.push("c2s");
        }
        if self.s2c.degraded.is_some() {
            degraded.push("s2c");
        }
        let seq = self.next();
        self.emit(&TraceEvent::SessionEnd {
            seq,
            c2s_frames: self.c2s_frames,
            s2c_frames: self.s2c_frames,
            c2s_bytes: self.c2s_bytes,
            s2c_bytes: self.s2c_bytes,
            duration_ms,
            degraded,
        });
    }

    /// Feed relayed bytes for one direction and trace whatever frames they completed.
    ///
    /// The caller forwards the same bytes it read, whether or not this understands them.
    pub fn observe(&mut self, direction: Direction, chunk: &[u8]) {
        match direction {
            Direction::ClientToServer => self.c2s_bytes += chunk.len() as u64,
            Direction::ServerToClient => self.s2c_bytes += chunk.len() as u64,
        }
        let compressed = self.compression.is_some();

        let observer = match direction {
            Direction::ClientToServer => &mut self.c2s,
            Direction::ServerToClient => &mut self.s2c,
        };
        let frames = observer.push(chunk);

        for (wire_bytes, packet) in frames {
            let body = body_of(&packet);
            let record = PacketRecord {
                seq: self.next(),
                dir: direction,
                state: self.state,
                id: packet.id,
                name: packet_name(direction, self.state, packet.id),
                wire_bytes,
                body_bytes: body.len(),
                compressed,
                digest: hex(&Md5::digest(&body)),
                head: hex(&body[..body.len().min(HEAD_BYTES)]),
            };
            match direction {
                Direction::ClientToServer => self.c2s_frames += 1,
                Direction::ServerToClient => self.s2c_frames += 1,
            }
            self.advance(direction, record.id, &packet.payload);
            self.emit(&TraceEvent::Packet(record));
        }

        // Report a degradation once, at the moment it happens.
        let (degraded, reported) = match direction {
            Direction::ClientToServer => (self.c2s.degraded.clone(), self.c2s.degraded_reported),
            Direction::ServerToClient => (self.s2c.degraded.clone(), self.s2c.degraded_reported),
        };
        if let Some(error) = degraded
            && !reported
        {
            match direction {
                Direction::ClientToServer => self.c2s.degraded_reported = true,
                Direction::ServerToClient => self.s2c.degraded_reported = true,
            }
            let seq = self.next();
            self.emit(&TraceEvent::ObserverError {
                seq,
                dir: direction,
                error,
            });
        }
    }

    /// Track the state machine and the compression transition from what was observed.
    ///
    /// Compression is a **both-directions** switch: `SetCompression` is clientbound, but the client
    /// compresses everything after it as well, so both codecs are updated on the same observation.
    fn advance(&mut self, direction: Direction, id: i32, payload: &[u8]) {
        use mc_protocol::ids;

        if direction == Direction::ServerToClient
            && self.state == State::Login
            && id == ids::clientbound::login::LOGIN_COMPRESSION
            && self.compression.is_none()
        {
            // The threshold is the payload's first VarInt. If it will not parse, leave compression off and
            // let framing fail visibly rather than invent a threshold.
            let mut cursor = payload;
            if let Ok(threshold) = mc_protocol::varint::read_varint(&mut cursor) {
                self.compression = Some(threshold);
                self.c2s.codec.set_compression(threshold);
                self.s2c.codec.set_compression(threshold);
            }
            return;
        }

        if direction != Direction::ClientToServer {
            return;
        }
        self.state = match (self.state, id) {
            (State::Handshake, id) if id == ids::serverbound::handshake::INTENTION => {
                // Decode with the typed packet rather than reading a VarInt by hand. The payload is
                // `protocol_version, server_address (length + bytes), server_port (u16), intent`, so the
                // *second* VarInt is the address length, not the intent — reading it positionally was a real
                // bug that left every later packet interpreted in the wrong state.
                use mc_protocol::packets::Packet as _;
                use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
                match Handshake::decode(payload) {
                    Ok(Handshake {
                        intent: HandshakeIntent::Status,
                        ..
                    }) => State::Status,
                    // Transfer is treated as login by the server, so the trace does the same.
                    Ok(Handshake {
                        intent: HandshakeIntent::Login | HandshakeIntent::Transfer,
                        ..
                    }) => State::Login,
                    // Undecodable: leave the state alone rather than guess. The server disconnects, and the
                    // trace should show the packet it could not interpret.
                    Err(_) => State::Handshake,
                }
            }
            (State::Login, id) if id == ids::serverbound::login::LOGIN_ACKNOWLEDGED => {
                State::Configuration
            }
            (State::Configuration, id) if id == ids::serverbound::config::FINISH_CONFIGURATION => {
                State::Play
            }
            (State::Play, id) if id == ids::serverbound::play::CONFIGURATION_ACKNOWLEDGED => {
                State::Configuration
            }
            (state, _) => state,
        };
    }

    fn next(&mut self) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    fn emit(&mut self, event: &TraceEvent) {
        if self.write_error.is_some() {
            return;
        }
        match serde_json::to_string(event) {
            Ok(line) => {
                if let Err(error) = writeln!(self.sink, "{line}") {
                    self.write_error = Some(error.to_string());
                }
            }
            Err(error) => self.write_error = Some(error.to_string()),
        }
    }

    /// Flush the sink.
    ///
    /// # Errors
    ///
    /// Whatever the underlying writer returns.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.sink.flush()
    }

    /// A write failure, if one has occurred. A trace that cannot be written is reported, not ignored.
    #[must_use]
    pub fn write_error(&self) -> Option<&str> {
        self.write_error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A sink that keeps the trace in memory so a test can read it back.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("buffer").clone()).expect("utf-8 trace")
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn new_session() -> (Session, SharedBuf) {
        let buf = SharedBuf::default();
        (Session::new(Box::new(buf.clone())), buf)
    }

    /// Frame a packet the way the wire does, uncompressed.
    fn frame(id: i32, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut body, id);
        body.extend_from_slice(payload);
        let mut out = Vec::new();
        let _ = mc_protocol::varint::write_varint(
            &mut out,
            i32::try_from(body.len()).expect("a test frame fits an i32 length"),
        );
        out.extend_from_slice(&body);
        out
    }

    /// Frame a packet compressed, with `body_len == 0` meaning "sent uncompressed inside a compressed
    /// frame", which is what the protocol allows below the threshold.
    fn frame_compressed(id: i32, payload: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        let mut body = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut body, id);
        body.extend_from_slice(payload);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&body).expect("compress");
        let compressed = encoder.finish().expect("finish");
        let mut inner = Vec::new();
        let _ = mc_protocol::varint::write_varint(
            &mut inner,
            i32::try_from(body.len()).expect("a test frame fits an i32 length"),
        );
        inner.extend_from_slice(&compressed);
        let mut out = Vec::new();
        let _ = mc_protocol::varint::write_varint(
            &mut out,
            i32::try_from(inner.len()).expect("a test frame fits an i32 length"),
        );
        out.extend_from_slice(&inner);
        out
    }

    /// Drive the handshake with intention 2 (login), which is the precondition for the compression
    /// switch: `SetCompression` is a login-state packet, and id 3 means something else in every other
    /// state, so the observer gates on the state rather than on the id alone.
    fn enter_login(session: &mut Session) {
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
        let payload = Handshake {
            protocol_version: 775,
            server_address: "127.0.0.1".to_owned(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        }
        .encode()
        .expect("encode");
        session.observe(Direction::ClientToServer, &frame(0x00, &payload));
        assert_eq!(
            session.state(),
            State::Login,
            "the helper must reach the login state"
        );
    }

    /// The other legal compressed form: `data_len == 0`, meaning the tail is raw.
    ///
    /// A real server uses this for packets **below** the threshold, which is why the threshold-256 test
    /// exercises it: a genuinely compressed 6-byte body would be — correctly — rejected.
    fn frame_raw_in_compressed(id: i32, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut body, id);
        body.extend_from_slice(payload);
        let mut inner = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut inner, 0);
        inner.extend_from_slice(&body);
        let mut out = Vec::new();
        let _ = mc_protocol::varint::write_varint(
            &mut out,
            i32::try_from(inner.len()).expect("a test frame fits an i32 length"),
        );
        out.extend_from_slice(&inner);
        out
    }

    fn packets(trace: &str) -> Vec<serde_json::Value> {
        trace
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["kind"] == "packet")
            .collect()
    }

    #[test]
    fn the_name_table_has_no_duplicate_ids_and_resolves_every_entry() {
        // A duplicate would make `packet_name` silently drop one of the two, which is the kind of
        // silent wrong answer this project keeps finding in evidence files.
        let mut seen = std::collections::BTreeSet::new();
        for (dir, state, id, name) in NAMES {
            assert!(
                seen.insert((*dir, *state, *id)),
                "duplicate entry for {dir:?}/{state:?}/{id} ({name})"
            );
            assert_eq!(packet_name(*dir, *state, *id), Some(*name));
        }
    }

    #[test]
    fn an_unknown_id_resolves_to_none_rather_than_a_guess() {
        // 0x7fff is not a packet id in any state we model.
        assert_eq!(
            packet_name(Direction::ClientToServer, State::Play, 0x7fff),
            None
        );
    }

    #[test]
    fn frames_are_split_incrementally_and_in_order() {
        let (mut session, buf) = new_session();
        let mut stream = frame(0x00, &[0x01]); // handshake intention, next state absent
        stream.extend_from_slice(&frame(0x00, &[0x02]));

        // Feed one byte at a time: a partial frame must produce no record, and the complete one exactly one.
        for byte in &stream {
            session.observe(Direction::ClientToServer, &[*byte]);
        }

        let records = packets(&buf.text());
        assert_eq!(records.len(), 2, "one record per frame, no more");
        // `seq` counts events, and this test emits no `start()` line, so the first packet is 0.
        assert_eq!(records[0]["seq"], 0);
        assert_eq!(records[1]["seq"], 1);
        assert_eq!(records[0]["dir"], "c2s");
    }

    #[test]
    fn the_handshake_intention_selects_the_next_state() {
        // Built with the typed encoder, so the payload is a real handshake — including the address and port
        // that the first version of this test omitted, which is exactly why it agreed with a parser that
        // read the intent from the wrong offset.
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};

        for (intent, expected) in [
            (HandshakeIntent::Login, State::Login),
            (HandshakeIntent::Status, State::Status),
            (HandshakeIntent::Transfer, State::Login),
        ] {
            let payload = Handshake {
                protocol_version: 775,
                server_address: "127.0.0.1".to_owned(),
                server_port: 25565,
                intent,
            }
            .encode()
            .expect("encode");
            let (mut session, _buf) = new_session();
            session.observe(Direction::ClientToServer, &frame(0, &payload));
            assert_eq!(session.state(), expected, "intent {intent:?}");
        }
    }

    #[test]
    fn compression_switches_both_directions_in_both_wire_forms() {
        // A threshold **below** the body size: the sender compresses. A frame in either direction must
        // decode, which is what proves both codecs were switched — switching only the clientbound one
        // would leave the other direction degrading.
        let mut low = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut low, 1);
        let (mut packed, packed_buf) = new_session();
        enter_login(&mut packed);
        packed.observe(
            Direction::ServerToClient,
            &frame(clientbound::login::LOGIN_COMPRESSION, &low),
        );
        assert_eq!(packed.compression(), Some(1));
        packed.observe(
            Direction::ClientToServer,
            &frame_compressed(0x00, &[0x01, 0x02]),
        );
        packed.observe(
            Direction::ServerToClient,
            &frame_compressed(clientbound::login::LOGIN_FINISHED, &[]),
        );

        let packed_text = packed_buf.text();
        assert!(
            !packed_text.contains("observer_error"),
            "both directions must stay framed under a low threshold: {packed_text}"
        );
        assert_eq!(
            packets(&packed_text).len(),
            4,
            "handshake, set_compression and both compressed frames: {packed_text}"
        );

        // A threshold **above** the body size: the sender uses the raw-inside-compressed form instead,
        // because a genuinely compressed small body is rejected by the codec — correctly, per vanilla.
        let mut high = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut high, 256);
        let (mut raw, raw_buf) = new_session();
        enter_login(&mut raw);
        raw.observe(
            Direction::ServerToClient,
            &frame(clientbound::login::LOGIN_COMPRESSION, &high),
        );
        assert_eq!(raw.compression(), Some(256));
        raw.observe(
            Direction::ClientToServer,
            &frame_raw_in_compressed(0x00, &[0x01, 0x02]),
        );
        raw.observe(
            Direction::ServerToClient,
            &frame_raw_in_compressed(clientbound::login::LOGIN_FINISHED, &[]),
        );

        let raw_text = raw_buf.text();
        assert!(
            !raw_text.contains("observer_error"),
            "both directions must stay framed under a high threshold: {raw_text}"
        );
        assert_eq!(
            packets(&raw_text).len(),
            4,
            "the raw-in-compressed frames must be recorded"
        );
    }

    #[test]
    fn the_digest_is_over_the_uncompressed_body_so_compression_does_not_change_it() {
        // This is the normalization claim. The same body sent uncompressed and compressed must produce the
        // same digest, or a trace would differ between two sessions that are semantically identical.
        let payload = vec![0x11, 0x22, 0x33, 0x44];

        let mut threshold = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut threshold, 1);

        // Same packet, same state, once uncompressed and once compressed; only the framing differs.
        let (mut plain, plain_buf) = new_session();
        enter_login(&mut plain);
        plain.observe(Direction::ClientToServer, &frame(0x00, &payload));

        let (mut packed, packed_buf) = new_session();
        enter_login(&mut packed);
        packed.observe(
            Direction::ServerToClient,
            &frame(clientbound::login::LOGIN_COMPRESSION, &threshold),
        );
        packed.observe(Direction::ClientToServer, &frame_compressed(0x00, &payload));

        let plain_records = packets(&plain_buf.text());
        let packed_records = packets(&packed_buf.text());
        // The last record in each trace is the payload packet: nothing follows it.
        let plain_digest = plain_records.last().expect("a record")["digest"].clone();
        let packed_digest = packed_records.last().expect("a record")["digest"].clone();

        assert_eq!(
            plain_digest, packed_digest,
            "the digest must ignore compression"
        );

        // The *body* is the same either way — that is why the digests match — while the *wire* size
        // differs, so the trace still carries what compression did.
        let plain = plain_records.last().expect("a record");
        let packed = packed_records.last().expect("a record");
        assert_eq!(
            plain["body_bytes"], packed["body_bytes"],
            "the uncompressed body length is the same whether or not the frame was compressed"
        );
        assert_ne!(
            plain["wire_bytes"], packed["wire_bytes"],
            "and the wire size is not: compression changed the framed length"
        );
    }

    #[test]
    fn unframeable_input_degrades_one_direction_and_says_so() {
        let (mut session, buf) = new_session();
        // A frame claiming far more bytes than the protocol maximum can never complete.
        let mut absurd = Vec::new();
        let _ = mc_protocol::varint::write_varint(&mut absurd, 0x7fff_ffff);
        absurd.extend_from_slice(&[0u8; 8]);
        session.observe(Direction::ClientToServer, &absurd);

        // Later bytes must not be interpreted, and must not panic.
        session.observe(Direction::ClientToServer, &frame(0x00, &[0x01]));
        session.end(7);

        let text = buf.text();
        assert!(
            text.contains("observer_error"),
            "the degradation must be traced: {text}"
        );
        assert!(
            text.contains("\"degraded\":[\"c2s\"]"),
            "session_end must name the degraded direction: {text}"
        );
        assert!(
            packets(&text).is_empty(),
            "nothing may be recorded from a stream we cannot frame: {text}"
        );
    }

    #[test]
    fn a_write_failure_is_reported_rather_than_swallowed() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut session = Session::new(Box::new(Broken));
        session.start("peer", "upstream");
        assert!(
            session.write_error().is_some(),
            "a lost trace must be visible"
        );
    }
}
