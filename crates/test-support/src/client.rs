//! Minimal protocol test client (P02-13).
//!
//! Speaks the same frames and packets as a 26.1.2 client for the Phase-02
//! slice: status ping, offline login, configuration exchange and `JoinGame`.
//! It is intentionally strict (unknown *required* packets fail the test) and
//! tiny — it exists to give the server an automated conversation partner, not
//! to emulate a full client.

use mc_core::error::{ServerError, ServerResult};
use mc_protocol::RawPacket;
use mc_protocol::framing::FrameCodec;
use mc_protocol::ids::{self, clientbound};
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    ClientInformation, FinishConfigurationAck, KnownPack, RegistryData, SelectKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
use mc_protocol::packets::login::{LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::{JoinGame, KeepAlive, PlayPong};
use mc_protocol::packets::status::{StatusPing, StatusPong, StatusRequest, StatusResponse};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Default per-step timeout for test conversations.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed server-list response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusInfo {
    /// Advertised protocol number.
    pub protocol: i32,
    /// Advertised version name.
    pub version_name: String,
    /// Advertised player cap.
    pub max_players: i64,
    /// MOTD text.
    pub motd: String,
}

/// Everything observed during a successful login/configuration/play entry.
#[derive(Debug, Clone)]
pub struct JoinResult {
    /// `LoginSuccess` payload.
    pub login: LoginSuccess,
    /// Registry payloads received during configuration.
    pub registries: Vec<RegistryData>,
    /// `JoinGame` payload that opened the play state.
    pub join: JoinGame,
}

/// A live test client conversation.
pub struct TestClient {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    codec: FrameCodec,
    compression: Option<i32>,
}

impl TestClient {
    /// Connect to `addr`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the socket cannot be established.
    pub async fn connect(addr: SocketAddr) -> ServerResult<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ServerError::Operational(format!("test client connect failed: {e}")))?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader,
            writer,
            codec: FrameCodec::new(),
            compression: None,
        })
    }

    /// Send a typed packet.
    ///
    /// # Errors
    ///
    /// [`ServerError`] on encoding/socket failure.
    pub async fn send<T: Packet>(&mut self, packet: &T) -> ServerResult<()> {
        self.send_raw_packet(&packet.to_raw()?).await
    }

    /// Send a raw packet.
    ///
    /// # Errors
    ///
    /// [`ServerError`] on encoding/socket failure.
    pub async fn send_raw_packet(&mut self, packet: &RawPacket) -> ServerResult<()> {
        let bytes = FrameCodec::encode(packet, self.compression)?;
        self.writer
            .write_all(&bytes)
            .await
            .map_err(|e| ServerError::Operational(format!("test client write failed: {e}")))
    }

    /// Send raw bytes exactly as given (hostile-input tests).
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] on socket failure.
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> ServerResult<()> {
        self.writer
            .write_all(bytes)
            .await
            .map_err(|e| ServerError::Operational(format!("test client write failed: {e}")))
    }

    /// Receive the next packet, honouring compression negotiated so far.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] on timeout or malformed frames;
    /// [`ServerError::Operational`] when the server closes the connection.
    pub async fn recv(&mut self) -> ServerResult<RawPacket> {
        let expires = tokio::time::Instant::now() + STEP_TIMEOUT;
        loop {
            if let Some(packet) = self.codec.try_next()? {
                return Ok(packet);
            }
            let remaining = expires.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ServerError::Protocol("test client recv timeout".to_owned()));
            }
            let mut buffer = [0u8; 4096];
            let read = tokio::time::timeout(remaining, self.reader.read(&mut buffer))
                .await
                .map_err(|_| ServerError::Protocol("test client recv timeout".to_owned()))?
                .map_err(|e| ServerError::Operational(format!("test client read failed: {e}")))?;
            if read == 0 {
                return Err(ServerError::Operational(
                    "server closed the connection".to_owned(),
                ));
            }
            self.codec.feed(&buffer[..read])?;
        }
    }

    /// Perform the status ping flow and return the parsed response.
    ///
    /// # Errors
    ///
    /// [`ServerError`] when the exchange fails.
    pub async fn status(addr: SocketAddr) -> ServerResult<StatusInfo> {
        let mut client = Self::connect(addr).await?;
        client
            .send(&Handshake {
                protocol_version: ids::PROTOCOL_VERSION,
                server_address: addr.ip().to_string(),
                server_port: addr.port(),
                intent: HandshakeIntent::Status,
            })
            .await?;
        client.send(&StatusRequest).await?;
        let response = client.recv().await?;
        if response.id != clientbound::status::STATUS_RESPONSE {
            return Err(ServerError::Protocol(format!(
                "expected StatusResponse, got packet {}",
                response.id
            )));
        }
        let json = StatusResponse::decode(&response.payload)?.json;
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| ServerError::Protocol(format!("status JSON invalid: {e}")))?;
        let info = StatusInfo {
            protocol: i32::try_from(parsed["version"]["protocol"].as_i64().unwrap_or(-1))
                .unwrap_or(-1),
            version_name: parsed["version"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            max_players: parsed["players"]["max"].as_i64().unwrap_or(-1),
            motd: parsed["description"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };

        // Ping round trip: payload must be echoed exactly.
        let payload = 0x0123_4567_89AB_CDEF_i64;
        client.send(&StatusPing { payload }).await?;
        let pong = client.recv().await?;
        if pong.id != clientbound::status::PONG_RESPONSE {
            return Err(ServerError::Protocol(format!(
                "expected PongResponse, got packet {}",
                pong.id
            )));
        }
        if StatusPong::decode(&pong.payload)?.payload != payload {
            return Err(ServerError::Protocol("ping payload not echoed".to_owned()));
        }
        Ok(info)
    }

    /// Run the full offline login → configuration → play entry.
    ///
    /// Returns the live client (for further conversation such as keepalive
    /// handling) together with everything observed.
    ///
    /// # Errors
    ///
    /// [`ServerError`] when any stage fails or the server kicks the client.
    pub async fn login_join(addr: SocketAddr, name: &str) -> ServerResult<(Self, JoinResult)> {
        let mut client = Self::connect(addr).await?;
        client
            .send(&Handshake {
                protocol_version: ids::PROTOCOL_VERSION,
                server_address: addr.ip().to_string(),
                server_port: addr.port(),
                intent: HandshakeIntent::Login,
            })
            .await?;
        client
            .send(&LoginStart {
                name: name.to_owned(),
                uuid: uuid::Uuid::nil(),
            })
            .await?;

        let login = client.complete_login().await?;
        let registries = client.complete_configuration().await?;
        let join = client.enter_play().await?;
        Ok((
            client,
            JoinResult {
                login,
                registries,
                join,
            },
        ))
    }

    /// [`login_join`](Self::login_join) while driving an external ticker.
    ///
    /// `JoinGame` now comes from the game loop, so a harness that ticks its
    /// game manually would deadlock awaiting it: the login only completes
    /// once the game processes the join, and the game is only ticked after
    /// the login returns. This polls the login and runs `tick` whenever it
    /// would otherwise stall, which is what the live lifecycle does
    /// continuously. Harnesses on the full lifecycle keep using
    /// [`login_join`](Self::login_join).
    ///
    /// # Errors
    ///
    /// [`ServerError`] when any stage fails or the server kicks the client.
    pub async fn login_join_tick(
        addr: SocketAddr,
        name: &str,
        mut tick: impl FnMut(),
    ) -> ServerResult<(Self, JoinResult)> {
        let login = Self::login_join(addr, name);
        tokio::pin!(login);
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                biased;
                result = &mut login => return result,
                _ = interval.tick() => tick(),
            }
        }
    }

    /// Login stage: optional `SetCompression`, then `LoginSuccess`, then the
    /// `LoginAcknowledged` reply.
    async fn complete_login(&mut self) -> ServerResult<LoginSuccess> {
        for _ in 0..4 {
            let packet = self.recv().await?;
            match packet.id {
                clientbound::login::LOGIN_COMPRESSION => {
                    let set = SetCompression::decode(&packet.payload)?;
                    self.codec.set_compression(set.threshold);
                    self.compression = Some(set.threshold);
                }
                clientbound::login::LOGIN_FINISHED => {
                    let login = LoginSuccess::decode(&packet.payload)?;
                    self.send(&mc_protocol::packets::login::LoginAcknowledged)
                        .await?;
                    return Ok(login);
                }
                clientbound::login::LOGIN_DISCONNECT => {
                    return Err(ServerError::InvalidAction(
                        "server sent LoginDisconnect".to_owned(),
                    ));
                }
                other => {
                    return Err(ServerError::Protocol(format!(
                        "unexpected login packet {other}"
                    )));
                }
            }
        }
        Err(ServerError::Protocol("no LoginSuccess".to_owned()))
    }

    /// Configuration stage: send client information, answer known packs,
    /// collect registry payloads and acknowledge `FinishConfiguration`.
    async fn complete_configuration(&mut self) -> ServerResult<Vec<RegistryData>> {
        self.send(&ClientInformation {
            locale: "en_us".to_owned(),
            view_distance: 8,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7F,
            main_hand: 1,
            text_filtering: false,
            server_listing: true,
        })
        .await?;
        let mut registries = Vec::new();
        loop {
            let packet = self.recv().await?;
            match packet.id {
                clientbound::config::SELECT_KNOWN_PACKS => {
                    let _ = SelectKnownPacks::decode(&packet.payload)?;
                    self.send(&SelectKnownPacks {
                        packs: vec![KnownPack {
                            namespace: "minecraft".to_owned(),
                            id: "core".to_owned(),
                            version: ids::VERSION_NAME.to_owned(),
                        }],
                    })
                    .await?;
                }
                clientbound::config::REGISTRY_DATA => {
                    registries.push(RegistryData::decode(&packet.payload)?);
                }
                clientbound::config::UPDATE_ENABLED_FEATURES
                | clientbound::config::UPDATE_TAGS
                | clientbound::config::CUSTOM_PAYLOAD
                | clientbound::config::KEEP_ALIVE
                | clientbound::config::PING => {}
                clientbound::config::FINISH_CONFIGURATION => {
                    self.send(&FinishConfigurationAck).await?;
                    return Ok(registries);
                }
                clientbound::config::DISCONNECT => {
                    return Err(ServerError::Protocol(
                        "server disconnected during configuration".to_owned(),
                    ));
                }
                other => {
                    return Err(ServerError::Protocol(format!(
                        "unexpected configuration packet {other}"
                    )));
                }
            }
        }
    }

    /// Play stage: wait for `JoinGame`, answering keepalives along the way.
    async fn enter_play(&mut self) -> ServerResult<JoinGame> {
        loop {
            let packet = self.recv().await?;
            match packet.id {
                clientbound::play::LOGIN => return JoinGame::decode(&packet.payload),
                clientbound::play::KEEP_ALIVE => {
                    let keepalive = KeepAlive::decode(&packet.payload)?;
                    self.send(&KeepAlive { id: keepalive.id }).await?;
                }
                clientbound::play::DISCONNECT => {
                    return Err(ServerError::Protocol(
                        "server disconnected in play".to_owned(),
                    ));
                }
                clientbound::play::SET_CHUNK_CACHE_CENTER
                | clientbound::play::SET_CHUNK_CACHE_RADIUS
                | clientbound::play::GAME_EVENT => {}
                other => {
                    return Err(ServerError::Protocol(format!(
                        "unexpected play packet {other}"
                    )));
                }
            }
        }
    }

    /// Reply to a keepalive and echo pings until the server disconnects or
    /// `duration` elapses; returns how many keepalives were answered.
    ///
    /// # Errors
    ///
    /// [`ServerError`] on malformed traffic.
    pub async fn stay_alive(&mut self, duration: Duration) -> ServerResult<u32> {
        let mut answered = 0;
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Ok(answered);
            }
            match self.recv().await {
                Ok(packet) => match packet.id {
                    clientbound::play::KEEP_ALIVE => {
                        let keepalive = KeepAlive::decode(&packet.payload)?;
                        self.send(&KeepAlive { id: keepalive.id }).await?;
                        answered += 1;
                    }
                    clientbound::play::PING => {
                        if let Ok(ping) =
                            mc_protocol::packets::play::PlayPingRequest::decode(&packet.payload)
                        {
                            self.send(&PlayPong { id: ping.id }).await?;
                        }
                    }
                    clientbound::play::DISCONNECT => return Ok(answered),
                    _ => {}
                },
                Err(ServerError::Operational(_)) => return Ok(answered),
                Err(error) => return Err(error),
            }
        }
    }
}

/// The protocol version this test client expects, for assertions.
///
/// **This is deliberately the server's own constant**, and on its own that would
/// make every assertion built on it vacuous — a client and server that agreed on a
/// wrong version would pass (AUDIT-09 E-03). What makes the chain meaningful is that
/// the constant is pinned from outside it:
/// `mc_protocol::ids::PROTOCOL_VERSION` is asserted against the 26.1.2 jar's own
/// `version.json`, committed at
/// `crates/test-support/fixtures/protocol/version.json`, by
/// `packet_ids::the_protocol_version_matches_the_jars_own_version_json`. So the
/// chain is: the jar states 775 → our constant must equal it → this value (aliasing
/// that constant) is what the status response must report. A test using this alias is
/// only as strong as that pin, which is why the pin exists and names the artifact.
pub const EXPECTED_PROTOCOL: i32 = ids::PROTOCOL_VERSION;
