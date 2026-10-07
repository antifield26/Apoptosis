//! Per-connection state machine (P02-01/P02-05/P02-06/P02-07/P02-10).
//!
//! One Tokio task per connection. Flow (offline mode):
//!
//! ```text
//! Handshake ─▶ Status  ─▶ respond + close
//!           └▶ Login   ─▶ LoginStart ─▶ [SetCompression] ─▶ LoginSuccess
//!                       ─▶ LoginAcknowledged ─▶ Configuration
//! Configuration ─▶ known packs → feature flags → registry data → tags
//!               ─▶ FinishConfiguration → ack ─▶ Play
//! Play ─▶ JoinGame → keepalive/intent loop (intents logged, simulated in P04+)
//! ```
//!
//! Every failure closes this connection only; malformed packets, overlong
//! strings and decompression bombs are all plain errors (AGENTS.md section 9).

use crate::auth::{
    GameProfile, OfflineOnlyAuth, OnlineAuthProvider, offline_profile, validate_username,
};
use crate::limits::ConnectionGuard;
use crate::listener::{GameLink, NetworkSettings, NetworkShutdown};
use crate::registry_data;
use mc_core::error::{ServerError, ServerResult};
use mc_protocol::RawPacket;
use mc_protocol::framing::FrameCodec;
use mc_protocol::ids::{ConnectionState, serverbound};
use mc_protocol::packets::Packet;
use mc_protocol::packets::config::{
    ClientInformation, ConfigDisconnect, ConfigKeepAlive, ConfigPong, FinishConfigurationAck,
    SelectKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
use mc_protocol::packets::login::{
    LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess, SetCompression,
};
use mc_protocol::packets::play::{
    ConfigurationAcknowledged, JoinGame, KeepAlive, PlayDisconnect, PlayIntent, PlayPingRequest,
    PlayPong, SetChunkCacheCenter, SetChunkCacheRadius, StartConfiguration,
};
use mc_protocol::packets::status::{StatusPing, StatusPong, StatusRequest, StatusResponse};
use mc_protocol::text::TextComponent;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::Instant;

/// Phase timeout applied to handshake/login/configuration reads.
const PHASE_TIMEOUT: Duration = Duration::from_secs(30);

/// Internal marker for read deadlines (compared by value).
const READ_TIMEOUT: &str = "read timeout";

/// Vanilla's translation key for a refused session (AUDIT-19 C19-L7).
///
/// The jar's `ServerLoginPacketListenerImpl` builds exactly this key
/// (`ldc "multiplayer.disconnect.unverified_username"`), and the English
/// client renders it as "Failed to verify username!" — the sentence this
/// server used to hard-code. The client owns the language file; the server
/// owning the sentence meant a non-English client saw English.
pub const UNVERIFIED_USERNAME_KEY: &str = "multiplayer.disconnect.unverified_username";

/// Vanilla's translation key for an unreachable session server (C19-L7/L8).
///
/// `multiplayer.disconnect.authservers_down`; the English text is the fallback
/// logged here and shown by a client without the key. This is the message a
/// player gets while Mojang is down, instead of being told their session
/// failed (C19-L8).
pub const AUTHSERVERS_DOWN_KEY: &str = "multiplayer.disconnect.authservers_down";

/// What the play loop should do after one packet.
enum PlayAction {
    /// Keep looping.
    Continue,
    /// Client asked to re-enter configuration; caller re-runs the config flow.
    Reconfigure,
}

/// Mutable per-connection session state.
struct Session {
    settings: Arc<NetworkSettings>,
    compression: Option<i32>,
    profile: Option<GameProfile>,
    client_locale: Option<String>,
    client_view_distance: Option<i8>,
    next_keepalive_id: i64,
    pending_keepalive: Option<i64>,
    pending_since: Option<Instant>,
    /// Sender used to report events to the game loop (set on join).
    event_sender: Option<crate::bridge::EventSender>,
    /// Game-loop link, when the server is running a world.
    game: Option<GameLink>,
    /// Remote IP, captured at accept for the join gate (P19-02).
    peer_ip: std::net::IpAddr,
    /// Login stream cipher, installed once the online handshake completes
    /// (P19-05). `None` before that and always in offline mode: plaintext
    /// in, plaintext out.
    cipher: Option<mc_protocol::cipher::PacketCipher>,
    /// Set once the join event has been published.
    joined: bool,
    /// Set once a refusal/disconnect packet has been sent to this client.
    ///
    /// A client that is kicked and immediately closes makes the *next* write
    /// fail with the OS's "connection reset" — which used to be logged as
    /// `socket write failed (os error 10053)` and read exactly like a server
    /// fault (AUDIT-19 G-12). With this flag the close is attributed to the
    /// refusal that caused it.
    refused: bool,
}

impl Session {
    fn new(
        settings: Arc<NetworkSettings>,
        game: Option<GameLink>,
        peer_ip: std::net::IpAddr,
    ) -> Self {
        Self {
            settings,
            compression: None,
            profile: None,
            client_locale: None,
            client_view_distance: None,
            next_keepalive_id: 1,
            pending_keepalive: None,
            pending_since: None,
            event_sender: None,
            game,
            peer_ip,
            cipher: None,
            joined: false,
            refused: false,
        }
    }

    /// Online-mode login: encryption handshake, session check, cipher on.
    ///
    /// Returns the verified profile, or `None` when the client went away
    /// quietly mid-handshake (like every other clean-EOF path, that ends
    /// the login without an error). Refusals kick with Vanilla's messages
    /// and also end quietly: a failed login is normal player input, not a
    /// server failure.
    async fn run_online_login(
        &mut self,
        reader: &mut (impl AsyncReadExt + Unpin),
        writer: &mut (impl AsyncWriteExt + Unpin),
        codec: &mut FrameCodec,
        auth: &dyn OnlineAuthProvider,
        name: &str,
    ) -> ServerResult<Option<GameProfile>> {
        use crate::online::{OnlineIdentity, server_id_hash};
        use mc_protocol::packets::login::{EncryptionRequest, EncryptionResponse};
        let Some(identity) = self.settings.online_identity.clone() else {
            self.kick(
                writer,
                ConnectionState::Login,
                TextComponent::literal("online mode has no login identity"),
            )
            .await;
            return Ok(None);
        };
        let token = OnlineIdentity::verify_token();
        self.send_typed(
            writer,
            &EncryptionRequest {
                server_id: String::new(),
                public_key: identity.public_der.clone(),
                verify_token: token.to_vec(),
                // Jar: `ServerLoginPacketListenerImpl.handleHello` builds
                // `ClientboundHelloPacket("", key, challenge, true)` — the
                // trailing `shouldAuthenticate` is `iconst_1` on the only
                // path that sends this packet (online mode; the offline
                // path never reaches it).
                should_authenticate: true,
            },
        )
        .await?;
        let Some(packet) = read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
        else {
            return Ok(None);
        };
        if packet.id != serverbound::login::KEY {
            return Err(ServerError::Protocol(format!(
                "expected EncryptionResponse, got packet id {}",
                packet.id
            )));
        }
        let response = EncryptionResponse::decode(&packet.payload)?;
        let secret = identity.decrypt(&response.shared_secret)?;
        if secret.len() != crate::online::SHARED_SECRET_LEN {
            return Err(ServerError::Protocol(format!(
                "shared secret is {} bytes, not {}",
                secret.len(),
                crate::online::SHARED_SECRET_LEN
            )));
        }
        let echoed = identity.decrypt(&response.verify_token)?;
        if !crate::online::fixed_time_eq(&echoed, &token) {
            // A bad verify token is a failed *session*, which is the same
            // player-facing answer Vanilla gives for it (C19-L7).
            self.kick(
                writer,
                ConnectionState::Login,
                TextComponent::translatable(UNVERIFIED_USERNAME_KEY, "Failed to verify username!"),
            )
            .await;
            return Ok(None);
        }
        let hash = server_id_hash("", &secret, &identity.public_der);
        let profile = match auth
            .authenticate_from(name, &hash, Some(self.peer_ip))
            .await
        {
            Ok(profile) => profile,
            // Only a genuine refusal lands here — the provider answers
            // `InvalidAction` for "no such login" and `Operational` for the
            // service failing (AUDIT-19 C19-L8). The player is told their
            // session is not valid; the reason is in the log.
            Err(ServerError::InvalidAction(detail)) => {
                tracing::info!(name, %detail, "online login refused by the session server");
                self.kick(
                    writer,
                    ConnectionState::Login,
                    TextComponent::translatable(
                        UNVERIFIED_USERNAME_KEY,
                        "Failed to verify username!",
                    ),
                )
                .await;
                return Ok(None);
            }
            // The session server or the link failed: the player is told the
            // authentication servers are unavailable, not that their
            // credentials are wrong, and the error is surfaced to operators.
            Err(error) => {
                tracing::warn!(name, %error, "session check failed; refusing the login");
                self.kick(
                    writer,
                    ConnectionState::Login,
                    TextComponent::translatable(
                        AUTHSERVERS_DOWN_KEY,
                        "Authentication servers are unavailable",
                    ),
                )
                .await;
                return Err(error);
            }
        };
        let mut secret_array = [0u8; crate::online::SHARED_SECRET_LEN];
        secret_array.copy_from_slice(&secret);
        self.cipher = Some(mc_protocol::cipher::PacketCipher::new(&secret_array));
        Ok(Some(profile))
    }

    async fn send_typed<T: Packet>(
        &mut self,
        writer: &mut (impl AsyncWriteExt + Unpin),
        packet: &T,
    ) -> ServerResult<()> {
        let raw = packet.to_raw()?;
        send_raw(writer, self.compression, self.cipher.as_mut(), &raw).await
    }

    async fn send_raw_packet(
        &mut self,
        writer: &mut (impl AsyncWriteExt + Unpin),
        raw: &RawPacket,
    ) -> ServerResult<()> {
        send_raw(writer, self.compression, self.cipher.as_mut(), raw).await
    }

    /// Send the state-appropriate disconnect packet, then close.
    ///
    /// The reason is a [`TextComponent`], not a string (AUDIT-19 C19-L7): a
    /// player-visible refusal is a Vanilla translation key the client renders
    /// in its own locale, while the log line and the packet shape are still
    /// one implementation. `reason.as_plain()` is what this server logs.
    async fn kick(
        &mut self,
        writer: &mut (impl AsyncWriteExt + Unpin),
        state: ConnectionState,
        reason: TextComponent,
    ) {
        // Owner-session placement disconnects: the client shows only a
        // garbled reason line, so every kick lands here in the log with its
        // reason — a kick and a raw TCP close are otherwise
        // indistinguishable from the game side. `as_plain()` is the English
        // fallback for a translation key, so the log reads as it always did.
        tracing::info!(
            name = %self.profile.clone().map_or("?".to_owned(), |profile| profile.name),
            reason = %reason.as_plain(),
            "kicking player"
        );
        let result = match state {
            ConnectionState::Login => {
                self.send_typed(
                    writer,
                    &LoginDisconnect {
                        json: reason.to_json(),
                    },
                )
                .await
            }
            ConnectionState::Configuration => {
                self.send_typed(writer, &ConfigDisconnect { reason }).await
            }
            ConnectionState::Play => self.send_typed(writer, &PlayDisconnect { reason }).await,
            ConnectionState::Handshake | ConnectionState::Status => Ok(()),
        };
        // From here on a write error is the refused client going away, not the
        // server failing: mark it so `run_connection` says so (AUDIT-19 G-12).
        self.refused = true;
        if let Err(error) = result {
            tracing::debug!(%error, "failed to send kick packet");
        }
        let _ = writer.shutdown().await;
    }
}

/// Handle one accepted connection until it closes or fails.
///
/// The `_guard` holds the admission slot for the connection lifetime; it is
/// intentionally kept even though unused, because dropping it releases the
/// budget tracked by [`crate::limits::ConnectionGate`].
///
/// # Errors
///
/// Returns the terminal connection error (protocol, limit or operational);
/// clean client disconnects return `Ok(())`.
pub async fn run_connection(
    stream: TcpStream,
    settings: Arc<NetworkSettings>,
    shutdown: NetworkShutdown,
    auth: Arc<dyn OnlineAuthProvider>,
    _guard: ConnectionGuard,
    game: Option<GameLink>,
) -> ServerResult<()> {
    let peer = stream
        .peer_addr()
        .map_err(|e| ServerError::Operational(format!("peer addr: {e}")))?;
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "could not set TCP_NODELAY");
    }
    let (mut reader, mut writer) = stream.into_split();
    let mut session = Session::new(Arc::clone(&settings), game, peer.ip());
    let mut codec = FrameCodec::new();

    let outcome = session
        .run(&mut reader, &mut writer, &mut codec, &shutdown, &*auth)
        .await;
    // Tell the game loop the player is gone, so it can unload them and save.
    if session.joined {
        session.report(crate::bridge::ClientEventKind::Left);
    }
    match connection_outcome_log(&outcome, session.refused) {
        OutcomeLog::Quiet(message) => tracing::debug!(%peer, %message, "connection closed"),
        OutcomeLog::ClientLeave(message) => tracing::debug!(
            %peer,
            %message,
            "client ended the connection after being refused"
        ),
        OutcomeLog::ServerFault(message) => {
            tracing::warn!(%peer, %message, "connection failed");
        }
    }
    let _ = writer.shutdown().await;
    outcome
}

/// How one connection's outcome should be logged.
///
/// Split out so the *decision* is testable without capturing a tracing
/// subscriber: [`run_connection`] just applies it. `refused` is the session's
/// flag, set once a kick/disconnect packet has been sent — after that, a write
/// error is the refused client going away rather than a server fault
/// (AUDIT-19 G-12: a client that read its refusal and hung up was logged as
/// `socket write failed (os error 10053)`, which names the wrong cause).
#[derive(Debug, Clone, PartialEq, Eq)]
enum OutcomeLog {
    /// Nothing to report: a clean close, or a client-side protocol error.
    Quiet(String),
    /// The client left after this server refused it.
    ClientLeave(String),
    /// Something this server should look at.
    ServerFault(String),
}

/// Classify one connection outcome for the log.
fn connection_outcome_log(outcome: &ServerResult<()>, refused: bool) -> OutcomeLog {
    match outcome {
        Ok(()) => OutcomeLog::Quiet("connection closed".to_owned()),
        Err(ServerError::Protocol(message)) => {
            OutcomeLog::Quiet(format!("protocol error, connection closed: {message}"))
        }
        Err(ServerError::InvalidAction(message)) => {
            OutcomeLog::Quiet(format!("invalid action, connection closed: {message}"))
        }
        Err(ServerError::Shutdown) => {
            OutcomeLog::Quiet("connection closed for shutdown".to_owned())
        }
        Err(error) if refused => OutcomeLog::ClientLeave(error.to_string()),
        Err(error) => OutcomeLog::ServerFault(error.to_string()),
    }
}

impl Session {
    async fn run(
        &mut self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        codec: &mut FrameCodec,
        shutdown: &NetworkShutdown,
        auth: &dyn OnlineAuthProvider,
    ) -> ServerResult<()> {
        // --- Handshake -----------------------------------------------------
        let Some(packet) = read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
        else {
            return Ok(());
        };
        if packet.id != serverbound::handshake::INTENTION {
            return Err(ServerError::Protocol(format!(
                "expected handshake, got packet id {}",
                packet.id
            )));
        }
        let handshake = Handshake::decode(&packet.payload)?;
        tracing::debug!(
            peer_address = %handshake.server_address,
            protocol = handshake.protocol_version,
            intent = ?handshake.intent,
            "handshake"
        );

        match handshake.intent {
            HandshakeIntent::Status => self.run_status(reader, writer, codec).await,
            HandshakeIntent::Login | HandshakeIntent::Transfer => {
                if handshake.protocol_version != self.settings.protocol_version {
                    let reason = if handshake.protocol_version < self.settings.protocol_version {
                        format!("Outdated client! Please use {}", self.settings.version_name)
                    } else {
                        format!(
                            "Outdated server! I'm still on {}",
                            self.settings.version_name
                        )
                    };
                    self.kick(
                        writer,
                        ConnectionState::Login,
                        TextComponent::literal(reason),
                    )
                    .await;
                    return Ok(());
                }
                self.run_login(reader, writer, codec, shutdown, auth).await
            }
        }
    }

    async fn run_status(
        &mut self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        codec: &mut FrameCodec,
    ) -> ServerResult<()> {
        loop {
            let Some(packet) =
                read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
            else {
                return Ok(());
            };
            match packet.id {
                serverbound::status::STATUS_REQUEST => {
                    StatusRequest::decode(&packet.payload)?;
                    let json = status_json(self.settings.as_ref());
                    self.send_typed(writer, &StatusResponse { json }).await?;
                }
                serverbound::status::PING_REQUEST => {
                    let ping = StatusPing::decode(&packet.payload)?;
                    self.send_typed(
                        writer,
                        &StatusPong {
                            payload: ping.payload,
                        },
                    )
                    .await?;
                    return Ok(()); // vanilla closes after the pong
                }
                other => {
                    tracing::debug!(packet_id = other, "ignoring unknown status packet");
                }
            }
        }
    }

    async fn run_login(
        &mut self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        codec: &mut FrameCodec,
        shutdown: &NetworkShutdown,
        auth: &dyn OnlineAuthProvider,
    ) -> ServerResult<()> {
        let Some(packet) = read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
        else {
            return Ok(());
        };
        if packet.id != serverbound::login::HELLO {
            return Err(ServerError::Protocol(format!(
                "expected LoginStart, got packet id {}",
                packet.id
            )));
        }
        let login_start = LoginStart::decode(&packet.payload)?;
        if let Err(invalid) = validate_username(&login_start.name) {
            // No Vanilla key exists for this shape (Vanilla answers the same
            // disconnect with a literal of its own), so it stays a literal.
            let reason = format!("Invalid username: {invalid}");
            self.kick(
                writer,
                ConnectionState::Login,
                TextComponent::literal(reason),
            )
            .await;
            return Ok(());
        }
        let profile = if self.settings.online_mode {
            let Some(profile) = self
                .run_online_login(reader, writer, codec, auth, &login_start.name)
                .await?
            else {
                return Ok(());
            };
            profile
        } else {
            offline_profile(&login_start.name)
        };
        tracing::info!(name = %profile.name, uuid = %profile.id, "login accepted");

        // Compression negotiation happens before LoginSuccess.
        if self.settings.compression_threshold >= 0 {
            let threshold = self.settings.compression_threshold;
            self.send_typed(writer, &SetCompression { threshold })
                .await?;
            codec.set_compression(threshold);
            self.compression = Some(threshold);
        }

        self.send_typed(
            writer,
            &LoginSuccess {
                uuid: profile.id,
                name: profile.name.clone(),
                properties: profile.properties.clone(),
            },
        )
        .await?;
        self.profile = Some(profile);

        // The client must acknowledge before configuration begins. Vanilla
        // clients may interleave `custom_query_answer` (2) or `cookie_response`
        // (4) here — kicking on anything but the ack would disconnect a real
        // client, so this is an ignore-loop with the same deadline as the other
        // login phases.
        loop {
            let Some(packet) =
                read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
            else {
                return Ok(());
            };
            match packet.id {
                serverbound::login::LOGIN_ACKNOWLEDGED => {
                    LoginAcknowledged::decode(&packet.payload)?;
                    break;
                }
                serverbound::login::CUSTOM_QUERY_ANSWER => {
                    tracing::debug!("ignoring custom_query_answer during login");
                }
                serverbound::login::COOKIE_RESPONSE => {
                    tracing::debug!("ignoring cookie_response during login");
                }
                other => {
                    tracing::debug!(packet_id = other, "ignoring unexpected login packet");
                }
            }
        }

        self.run_configuration(reader, writer, codec, shutdown)
            .await
    }

    async fn run_configuration(
        &mut self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        codec: &mut FrameCodec,
        shutdown: &NetworkShutdown,
    ) -> ServerResult<()> {
        self.send_configuration(writer).await?;
        self.wait_for_configuration_ack(reader, codec, shutdown)
            .await?;
        self.run_play(reader, writer, codec, shutdown).await
    }

    async fn send_configuration(&mut self, writer: &mut OwnedWriteHalf) -> ServerResult<()> {
        for packet in registry_data::configuration_packets(&self.settings.version_name)? {
            self.send_raw_packet(writer, &packet).await?;
        }
        Ok(())
    }

    async fn wait_for_configuration_ack(
        &mut self,
        reader: &mut OwnedReadHalf,
        codec: &mut FrameCodec,
        shutdown: &NetworkShutdown,
    ) -> ServerResult<()> {
        loop {
            if shutdown.is_requested() {
                return Err(ServerError::Shutdown);
            }
            let Some(packet) =
                read_packet(reader, codec, self.cipher.as_mut(), PHASE_TIMEOUT).await?
            else {
                return Err(ServerError::Protocol(
                    "client closed during configuration".to_owned(),
                ));
            };
            match packet.id {
                serverbound::config::SELECT_KNOWN_PACKS => {
                    // Acknowledge-and-ignore: Phase 02 always sends the full
                    // registry payload, so the client's pack list only needs
                    // to decode cleanly.
                    let packs = SelectKnownPacks::decode(&packet.payload)?;
                    tracing::debug!(count = packs.packs.len(), "client known packs");
                }
                serverbound::config::CLIENT_INFORMATION => {
                    let info = ClientInformation::decode(&packet.payload)?;
                    self.client_locale = Some(info.locale);
                    self.client_view_distance = Some(info.view_distance);
                }
                serverbound::config::FINISH_CONFIGURATION => {
                    FinishConfigurationAck::decode(&packet.payload)?;
                    return Ok(());
                }
                serverbound::config::KEEP_ALIVE => {
                    let keepalive = ConfigKeepAlive::decode(&packet.payload)?;
                    tracing::debug!(id = keepalive.id, "configuration keepalive ack");
                }
                serverbound::config::PONG => {
                    let _ = ConfigPong::decode(&packet.payload)?;
                }
                other => {
                    tracing::debug!(packet_id = other, "ignoring unknown configuration packet");
                }
            }
        }
    }

    async fn run_play(
        &mut self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        codec: &mut FrameCodec,
        shutdown: &NetworkShutdown,
    ) -> ServerResult<()> {
        // Join the game loop first when one is running: the game owns the
        // world-side join packets (notably `JoinGame`, which must carry the
        // real player entity id — the network layer cannot know it before the
        // entity store allocates it, and the hardcoded 1 this layer used to
        // send broke every rejoin's self-directed packets, e.g. the effect
        // icons never appeared because `UpdateMobEffect` went to an entity
        // the client does not know as itself). Protocol-only runs (no game
        // loop) keep the local fallback below.
        let mut outbound = self.publish_join();
        if outbound.is_none() {
            self.enter_play(writer).await?;
        } else {
            tracing::info!(
                name = %self.profile.clone().map_or("?".to_owned(), |profile| profile.name),
                "player entered play state (join packets come from the game loop)"
            );
        }
        let mut last_keepalive_sent = Instant::now();
        loop {
            // One timer at a time: when idle it fires to send a keepalive;
            // once one is pending it fires exactly at the response deadline,
            // so a slow reply can never busy-spin the select loop.
            let next_event = self.pending_since.map_or(
                last_keepalive_sent + self.settings.keepalive_interval,
                |since| since + self.settings.keepalive_timeout,
            );
            // `recv` on a channel that will never receive (no game loop) parks
            // forever, which is the correct behaviour: the branch simply never
            // fires and the Phase-02 select shape is preserved.
            tokio::select! {
                biased;
                () = shutdown.notified() => {
                    self.kick(
                        writer,
                        ConnectionState::Play,
                        TextComponent::literal("Server shutting down"),
                    )
                    .await;
                    return Err(ServerError::Shutdown);
                }
                () = tokio::time::sleep_until(next_event) => {
                    if self.pending_keepalive.is_some() {
                        tracing::info!("keepalive timeout, disconnecting player");
                        self.kick(writer, ConnectionState::Play, TextComponent::literal("Timed out"))
                            .await;
                        return Ok(());
                    }
                    let id = self.next_keepalive_id;
                    self.next_keepalive_id = self.next_keepalive_id.wrapping_add(1);
                    self.pending_keepalive = Some(id);
                    self.pending_since = Some(Instant::now());
                    last_keepalive_sent = Instant::now();
                    self.send_typed(writer, &KeepAlive { id }).await?;
                }
                packet = next_outbound(&mut outbound) => {
                    if let Some(packet) = packet {
                        // A write failure here ends the connection the same way a
                        // failed keepalive would; it is not fatal to the process.
                        self.send_raw_packet(writer, &packet).await?;
                    }
                }
                packet = read_packet(reader, codec, self.cipher.as_mut(), self.settings.keepalive_timeout) => {
                    match packet {
                        Ok(Some(packet)) => {
                            match self.handle_play_packet(packet, writer).await? {
                                PlayAction::Continue => {}
                                PlayAction::Reconfigure => {
                                    self.send_typed(writer, &StartConfiguration).await?;
                                    self.send_configuration(writer).await?;
                                    self.wait_for_configuration_ack(reader, codec, shutdown).await?;
                                    self.enter_play(writer).await?;
                                    last_keepalive_sent = Instant::now();
                                    self.pending_keepalive = None;
                                    self.pending_since = None;
                                }
                            }
                        }
                        Ok(None) => return Ok(()),
                        Err(ServerError::Protocol(message)) if message == READ_TIMEOUT => {
                            // A read deadline only means "no bytes in this window".
                            // The keepalive deadline is the authority on liveness:
                            // a client that keeps sending junk but never answers a
                            // keepalive must still be kicked, so fall through to the
                            // select loop and let the keepalive branch decide.
                            tracing::trace!("play read idle window elapsed");
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }

    /// Tell the connection its play-phase preamble when no game loop is attached.
    ///
    /// Protocol-only runs (Phase 02) have no entity store, so there is no real
    /// player entity id to report and 1 is the honest placeholder. Live runs
    /// never call this: the game loop sends `JoinGame` with the allocated id.
    async fn enter_play(&mut self, writer: &mut OwnedWriteHalf) -> ServerResult<()> {
        let profile = self
            .profile
            .clone()
            .ok_or_else(|| ServerError::Invariant("entered play without a profile".to_owned()))?;
        let join = JoinGame {
            entity_id: 1,
            hardcore: false,
            dimension_names: vec![registry_data::OVERWORLD.to_owned()],
            max_players: self.settings.max_players as i32,
            view_distance: self.settings.view_distance,
            simulation_distance: self.settings.view_distance,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            dimension_type_id: 0,
            dimension_name: registry_data::OVERWORLD.to_owned(),
            hashed_seed: 0,
            game_mode: 0,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: false,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            enforce_secure_chat: false,
        };
        self.send_typed(writer, &join).await?;
        self.send_typed(writer, &SetChunkCacheCenter { x: 0, z: 0 })
            .await?;
        self.send_typed(
            writer,
            &SetChunkCacheRadius {
                radius: self.settings.view_distance,
            },
        )
        .await?;
        tracing::info!(name = %profile.name, "player entered play state");
        Ok(())
    }

    /// Handle one play packet.
    async fn handle_play_packet(
        &mut self,
        packet: RawPacket,
        writer: &mut OwnedWriteHalf,
    ) -> ServerResult<PlayAction> {
        match packet.id {
            serverbound::play::KEEP_ALIVE => {
                let ack = KeepAlive::decode(&packet.payload)?;
                if self.pending_keepalive == Some(ack.id) {
                    self.pending_keepalive = None;
                    self.pending_since = None;
                } else {
                    tracing::debug!(id = ack.id, "unexpected keepalive id");
                }
            }
            serverbound::play::PING_REQUEST => {
                let ping = PlayPingRequest::decode(&packet.payload)?;
                self.send_typed(writer, &PlayPong { id: ping.id }).await?;
            }
            serverbound::play::CLIENT_INFORMATION => {
                let info = ClientInformation::decode(&packet.payload)?;
                self.client_locale = Some(info.locale);
                self.client_view_distance = Some(info.view_distance);
                // P14-04: the game owns streaming, so a settings change is an
                // event, not a local field. Unknown sessions are ignored
                // there; an unattached loop drops it here.
                self.report(crate::bridge::ClientEventKind::ViewDistance {
                    distance: info.view_distance,
                });
            }
            serverbound::play::CONFIGURATION_ACKNOWLEDGED => {
                ConfigurationAcknowledged::decode(&packet.payload)?;
                tracing::debug!("client requested reconfiguration");
                return Ok(PlayAction::Reconfigure);
            }
            other => {
                if let Some(intent) = PlayIntent::decode(other, &packet.payload)? {
                    if self.joined {
                        // The game loop owns gameplay now; hand the intent over and
                        // let the tick thread decide what it means.
                        self.report(crate::bridge::ClientEventKind::Intent(intent));
                    } else {
                        tracing::trace!(?intent, "play intent before join; traced only");
                    }
                } else {
                    tracing::debug!(packet_id = other, "ignoring unmodelled play packet");
                    if self.joined {
                        self.report(crate::bridge::ClientEventKind::Unmodelled {
                            packet_id: other,
                        });
                    }
                }
            }
        }
        Ok(PlayAction::Continue)
    }

    /// Tell the game loop this client is in play, and hand it the outbound sender.
    ///
    /// Returns the receiver the play loop reads server→client packets from, or
    /// `None` when no game loop is attached (protocol-only runs, as in Phase 02).
    fn publish_join(&mut self) -> Option<crate::bridge::InboundReceiver> {
        let game = self.game.clone()?;
        let profile = self.profile.clone()?;
        let id = game.next_id();
        let (sender, receiver, outbound) = game.channel_for(id);
        tracing::info!(%id, name = %profile.name, "player entering the world");
        // The address rides ahead of the join on the same FIFO channel, so
        // the game gate sees it before the profile it belongs to (P19-02).
        if !sender.try_send(crate::bridge::ClientEventKind::PeerAddress { ip: self.peer_ip }) {
            tracing::warn!(%id, "game loop queue is full; the player cannot be served");
        }
        if !sender.try_send(crate::bridge::ClientEventKind::Joined { profile, outbound }) {
            tracing::warn!(%id, "game loop queue is full; the player cannot be served");
        }
        self.event_sender = Some(sender);
        // P14-04: the configuration-phase settings arrived before play, so
        // the game never saw them. Forward the view distance now that reports
        // have somewhere to go (this assignment is why the forward sits after
        // it, not before); play-phase changes arrive through the
        // CLIENT_INFORMATION arm above.
        if let Some(distance) = self.client_view_distance {
            self.report(crate::bridge::ClientEventKind::ViewDistance { distance });
        }
        self.joined = true;
        Some(receiver)
    }

    /// Report an event to the game loop, if one is attached.
    fn report(&self, kind: crate::bridge::ClientEventKind) {
        if let Some(sender) = &self.event_sender
            && !sender.try_send(kind)
        {
            tracing::debug!(id = %sender.id, "game loop queue full; event dropped");
        }
    }
}

/// Await the next outbound packet, parking forever when there is no game loop.
///
/// Written as a helper so the play loop can use it as a `select!` branch without
/// special-casing the `None` case inline.
async fn next_outbound(outbound: &mut Option<crate::bridge::InboundReceiver>) -> Option<RawPacket> {
    match outbound {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Read one packet, or `None` on clean EOF. The deadline bounds the whole read.
///
/// When the login cipher is installed, socket bytes are decrypted before
/// they reach the frame codec, and the encoded frame is encrypted before
/// the write — the whole stream past the handshake, length prefixes
/// included, exactly like Vanilla.
async fn read_packet(
    reader: &mut (impl AsyncReadExt + Unpin),
    codec: &mut FrameCodec,
    mut cipher: Option<&mut mc_protocol::cipher::PacketCipher>,
    deadline: Duration,
) -> ServerResult<Option<RawPacket>> {
    let expires = Instant::now() + deadline;
    loop {
        if let Some(packet) = codec.try_next()? {
            return Ok(Some(packet));
        }
        let remaining = expires.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ServerError::Protocol(READ_TIMEOUT.to_owned()));
        }
        let mut buffer = [0u8; 4096];
        match tokio::time::timeout(remaining, reader.read(&mut buffer)).await {
            Err(_) => return Err(ServerError::Protocol(READ_TIMEOUT.to_owned())),
            Ok(Ok(0)) => return Ok(None),
            Ok(Ok(read)) => {
                let bytes = &mut buffer[..read];
                if let Some(cipher) = cipher.as_mut() {
                    cipher.decrypt_bytes(bytes);
                }
                codec.feed(bytes)?;
            }
            Ok(Err(error)) => {
                return Err(ServerError::Operational(format!(
                    "socket read failed: {error}"
                )));
            }
        }
    }
}

/// Encode and write one packet with the negotiated compression.
///
/// A write failure is the *peer's* socket refusing our bytes (the OS's
/// connection-reset/aborted family) or the local stack failing; [`write_error`]
/// names the direction, the packet and the byte count so an operator can tell
/// those apart, and never blames the server for a client that has gone
/// (AUDIT-19 G-12). Whether it is *reported* as a client leave or a server
/// fault is [`run_connection`]'s decision, from the session's `refused` flag.
async fn send_raw(
    writer: &mut (impl AsyncWriteExt + Unpin),
    compression: Option<i32>,
    cipher: Option<&mut mc_protocol::cipher::PacketCipher>,
    packet: &RawPacket,
) -> ServerResult<()> {
    let mut bytes = FrameCodec::encode(packet, compression)?;
    if let Some(cipher) = cipher {
        cipher.encrypt_bytes(&mut bytes);
    }
    writer
        .write_all(&bytes)
        .await
        .map_err(|error| write_error(packet, bytes.len(), &error))
}

/// The error a failed packet write reports, in the shape the fix pins.
///
/// Split from the write so the wording can be tested against a synthetic
/// [`std::io::Error`]: the platform failure it describes (`os error 10053` on
/// Windows, `104`/`Connection reset by peer` elsewhere) cannot be provoked on
/// every host by a loopback socket, and a test that only fires on one platform
/// is not a pin (AUDIT-19 G-12).
fn write_error(packet: &RawPacket, written: usize, error: &std::io::Error) -> ServerError {
    ServerError::Operational(format!(
        "the client's socket refused packet {} ({written} bytes): {error}",
        packet.id
    ))
}

/// Server-list JSON for the status response.
fn status_json(settings: &NetworkSettings) -> String {
    serde_json::json!({
        "version": {
            "name": settings.version_name,
            "protocol": settings.protocol_version,
        },
        "players": {
            "max": settings.max_players,
            "online": 0,
            "sample": [],
        },
        "description": {
            "text": settings.motd,
        },
    })
    .to_string()
}

/// Convenience constructor for the default offline provider.
#[must_use]
pub fn default_auth() -> Arc<dyn OnlineAuthProvider> {
    Arc::new(OfflineOnlyAuth)
}

#[cfg(test)]
mod tests {
    use super::{UNVERIFIED_USERNAME_KEY, status_json};
    use crate::listener::NetworkSettings;

    #[test]
    fn status_json_advertises_protocol_and_motd() {
        let settings = NetworkSettings::default();
        let json = status_json(&settings);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["version"]["protocol"], 775);
        assert_eq!(parsed["version"]["name"], "26.1.2");
        assert_eq!(parsed["players"]["max"], 10);
        assert_eq!(parsed["description"]["text"], settings.motd);
    }

    /// A stub session provider: verifies nobody, returns a fixed profile
    /// with one skin property — or refuses, to pin the kick path.
    struct StubAuth {
        verify: bool,
    }

    impl crate::auth::OnlineAuthProvider for StubAuth {
        fn authenticate<'a>(
            &'a self,
            name: &'a str,
            _server_hash: &'a str,
        ) -> crate::auth::AuthFuture<'a> {
            let verify = self.verify;
            Box::pin(async move {
                if !verify {
                    return Err(mc_core::error::ServerError::InvalidAction(
                        "no such login".to_owned(),
                    ));
                }
                Ok(crate::auth::GameProfile {
                    id: uuid::Uuid::parse_str("069a79f4-44e9-4726-a5be-f4a7b64ac909")
                        .expect("fixture uuid"),
                    name: name.to_owned(),
                    properties: vec![mc_protocol::packets::login::ProfileProperty {
                        name: "textures".to_owned(),
                        value: "abc".to_owned(),
                        signature: Some("sig".to_owned()),
                    }],
                })
            })
        }
    }

    fn framed(id: i32, body: &[u8]) -> Vec<u8> {
        mc_protocol::framing::FrameCodec::encode(
            &mc_protocol::packet::RawPacket::new(id, body.to_vec()),
            None,
        )
        .expect("encodes")
    }

    async fn read_frame(
        reader: &mut (impl tokio::io::AsyncReadExt + Unpin),
        mut cipher: Option<&mut mc_protocol::cipher::PacketCipher>,
    ) -> (i32, Vec<u8>) {
        use mc_protocol::varint::read_varint;
        let mut len_buf = Vec::new();
        loop {
            let mut one = [0u8; 1];
            reader.read_exact(&mut one).await.expect("length byte");
            if let Some(cipher) = cipher.as_deref_mut() {
                cipher.decrypt_bytes(&mut one);
            }
            len_buf.push(one[0]);
            let mut slice: &[u8] = &len_buf;
            if let Ok(len) = read_varint(&mut slice)
                && slice.is_empty()
            {
                let mut body = vec![0u8; len as usize];
                reader.read_exact(&mut body).await.expect("body");
                if let Some(cipher) = cipher.as_deref_mut() {
                    cipher.decrypt_bytes(&mut body);
                }
                let mut slice: &[u8] = &body;
                let id = read_varint(&mut slice).expect("id");
                return (id, slice.to_vec());
            }
        }
    }

    /// Full online login over loopback (P19-05): handshake, `LoginStart`,
    /// `EncryptionRequest`, encrypted `EncryptionResponse`, then
    /// `LoginSuccess` read back *through the cipher* with the verified
    /// profile and its skin property.
    ///
    /// The client half plays a stock client: RSA-encrypt the secret and
    /// token with the server's DER key, enable AES/CFB8, and decode from
    /// there. A token mismatch, a short secret or a refused session would
    /// surface here as a kick or a hang rather than this packet.
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one linear stock-client script; splitting it would scatter the handshake order the test exists to pin"
    )]
    async fn online_login_completes_through_the_cipher() {
        use mc_protocol::cipher::PacketCipher;
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
        use mc_protocol::packets::login::{
            EncryptionRequest, EncryptionResponse, LoginStart, LoginSuccess,
        };
        use tokio::io::AsyncWriteExt as _;

        let identity = crate::online::OnlineIdentity::generate().expect("keygen works");
        let settings = NetworkSettings {
            online_mode: true,
            compression_threshold: -1,
            online_identity: Some(std::sync::Arc::new(identity)),
            ..NetworkSettings::default()
        };
        let auth: std::sync::Arc<dyn crate::auth::OnlineAuthProvider> =
            std::sync::Arc::new(StubAuth { verify: true });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback binds");
        let addr = listener.local_addr().expect("addr");
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("one client");
            let gate = std::sync::Arc::new(crate::limits::ConnectionGate::new(
                8,
                8,
                std::time::Duration::ZERO,
            ));
            let now = std::time::Instant::now();
            let guard = gate
                .try_acquire("127.0.0.1".parse().expect("ip"), now)
                .expect("admitted");
            super::run_connection(
                stream,
                std::sync::Arc::new(settings),
                crate::listener::NetworkShutdown::new(),
                auth,
                guard,
                None,
            )
            .await
        });

        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("client connects");
        // Handshake (next state: login), plaintext.
        let hello = Handshake {
            protocol_version: 775,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            intent: HandshakeIntent::Login,
        };
        let body = hello.encode().expect("encodes");
        client
            .write_all(&framed(
                mc_protocol::ids::serverbound::handshake::INTENTION,
                &body,
            ))
            .await
            .expect("handshake");
        // LoginStart, plaintext.
        let start = LoginStart {
            name: "Miner".to_owned(),
            uuid: uuid::Uuid::nil(),
        };
        let body = start.encode().expect("encodes");
        client
            .write_all(&framed(mc_protocol::ids::serverbound::login::HELLO, &body))
            .await
            .expect("login start");

        // EncryptionRequest arrives plaintext.
        let (id, body) = read_frame(&mut client, None).await;
        assert_eq!(id, mc_protocol::ids::clientbound::login::HELLO);
        let request = EncryptionRequest::decode(&body).expect("decodes");
        assert_eq!(request.server_id, "");
        assert!(!request.public_key.is_empty() && request.verify_token.len() == 4);
        assert!(
            request.should_authenticate,
            "the online path must send shouldAuthenticate=true, as vanilla's handleHello does"
        );

        // Answer like a stock client: RSA-encrypt secret and token.
        let secret = *b"0123456789abcdef";
        let public: rsa::RsaPublicKey =
            rsa::pkcs8::DecodePublicKey::from_public_key_der(&request.public_key)
                .expect("server key parses");
        let mut rng = rand::thread_rng();
        let enc_secret = public
            .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &secret)
            .expect("encrypts");
        let enc_token = public
            .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &request.verify_token)
            .expect("encrypts");
        let answer = EncryptionResponse {
            shared_secret: enc_secret,
            verify_token: enc_token,
        };
        let body = answer.encode().expect("encodes");
        client
            .write_all(&framed(mc_protocol::ids::serverbound::login::KEY, &body))
            .await
            .expect("response");

        // From here the server speaks cipher: enable ours and read
        // LoginSuccess through it.
        let mut cipher = PacketCipher::new(&secret);
        let (id, body) = read_frame(&mut client, Some(&mut cipher)).await;
        assert_eq!(id, mc_protocol::ids::clientbound::login::LOGIN_FINISHED);
        let success = LoginSuccess::decode(&body).expect("decodes");
        assert_eq!(success.name, "Miner");
        assert_eq!(
            success.uuid.to_string(),
            "069a79f4-44e9-4726-a5be-f4a7b64ac909"
        );
        assert_eq!(success.properties.len(), 1);
        assert_eq!(success.properties[0].name, "textures");
        assert_eq!(success.properties[0].signature.as_deref(), Some("sig"));
        drop(client);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_task)
            .await
            .expect("server ends with the client");
    }

    /// Refusals kick with Vanilla's message: a bad verify token and a
    /// refused session both end in `LoginDisconnect`, never in a hang and
    /// never past the gate.
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one linear refusal script; same justification as the happy path above"
    )]
    async fn online_login_refusals_kick() {
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
        use mc_protocol::packets::login::{
            EncryptionRequest, EncryptionResponse, LoginDisconnect, LoginStart,
        };
        use tokio::io::AsyncWriteExt as _;

        /// One login attempt through `EncryptionResponse`; returns the
        /// disconnect reason the client saw.
        async fn attempt(addr: std::net::SocketAddr, name: &str, corrupt_token: bool) -> String {
            let mut client = tokio::net::TcpStream::connect(addr)
                .await
                .expect("client connects");
            let hello = Handshake {
                protocol_version: 775,
                server_address: "localhost".to_owned(),
                server_port: 25565,
                intent: HandshakeIntent::Login,
            };
            let body = hello.encode().expect("encodes");
            client
                .write_all(&framed(
                    mc_protocol::ids::serverbound::handshake::INTENTION,
                    &body,
                ))
                .await
                .expect("handshake");
            let start = LoginStart {
                name: name.to_owned(),
                uuid: uuid::Uuid::nil(),
            };
            let body = start.encode().expect("encodes");
            client
                .write_all(&framed(mc_protocol::ids::serverbound::login::HELLO, &body))
                .await
                .expect("login start");
            let (id, body) = read_frame(&mut client, None).await;
            assert_eq!(id, mc_protocol::ids::clientbound::login::HELLO);
            let request = EncryptionRequest::decode(&body).expect("decodes");
            let secret = *b"0123456789abcdef";
            let public: rsa::RsaPublicKey =
                rsa::pkcs8::DecodePublicKey::from_public_key_der(&request.public_key)
                    .expect("server key parses");
            let mut rng = rand::thread_rng();
            let token = if corrupt_token {
                *b"xxxx"
            } else {
                request.verify_token.clone().try_into().expect("4 bytes")
            };
            let answer = EncryptionResponse {
                shared_secret: public
                    .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &secret)
                    .expect("encrypts"),
                verify_token: public
                    .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &token)
                    .expect("encrypts"),
            };
            let body = answer.encode().expect("encodes");
            client
                .write_all(&framed(mc_protocol::ids::serverbound::login::KEY, &body))
                .await
                .expect("response");
            let (id, body) = read_frame(&mut client, None).await;
            assert_eq!(
                id,
                mc_protocol::ids::clientbound::login::LOGIN_DISCONNECT,
                "a refusal is a kick, not silence"
            );
            LoginDisconnect::decode(&body).expect("decodes").json
        }

        async fn serve_once(verify: bool) -> std::net::SocketAddr {
            let identity = crate::online::OnlineIdentity::generate().expect("keygen works");
            let settings = NetworkSettings {
                online_mode: true,
                compression_threshold: -1,
                online_identity: Some(std::sync::Arc::new(identity)),
                ..NetworkSettings::default()
            };
            let auth: std::sync::Arc<dyn crate::auth::OnlineAuthProvider> =
                std::sync::Arc::new(StubAuth { verify });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("loopback binds");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.expect("one client");
                let gate = std::sync::Arc::new(crate::limits::ConnectionGate::new(
                    8,
                    8,
                    std::time::Duration::ZERO,
                ));
                let guard = gate
                    .try_acquire("127.0.0.1".parse().expect("ip"), std::time::Instant::now())
                    .expect("admitted");
                let _ = super::run_connection(
                    stream,
                    std::sync::Arc::new(settings),
                    crate::listener::NetworkShutdown::new(),
                    auth,
                    guard,
                    None,
                )
                .await;
            });
            addr
        }

        // Wrong token: the handshake's integrity check fires first.
        let addr = serve_once(true).await;
        let reason = attempt(addr, "Miner", true).await;
        assert!(
            reason.contains(UNVERIFIED_USERNAME_KEY),
            "a bad token is refused with Vanilla's translation key, not a sentence: {reason}"
        );
        // Refused session: the provider says no.
        let addr = serve_once(false).await;
        let reason = attempt(addr, "Miner", false).await;
        assert!(
            reason.contains(UNVERIFIED_USERNAME_KEY),
            "a refused session is refused with the same key: {reason}"
        );
    }

    /// A provider that fails the way a *session-server outage* does.
    struct OutageAuth;

    impl crate::auth::OnlineAuthProvider for OutageAuth {
        fn authenticate<'a>(
            &'a self,
            _name: &'a str,
            _server_hash: &'a str,
        ) -> crate::auth::AuthFuture<'a> {
            Box::pin(async move {
                Err(mc_core::error::ServerError::Operational(
                    "session-server transport failure: the session server answered HTTP 503"
                        .to_owned(),
                ))
            })
        }
    }

    /// A session-server outage and a genuine refusal must not look alike
    /// (AUDIT-19 C19-L8), and a refusal must be a translation key (C19-L7).
    ///
    /// The 503 case gets `authservers_down` and the reason the server logged;
    /// the refusal case gets `unverified_username` and ends quietly. The two
    /// kicks are byte-compared through the packet a real client decodes.
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one linear login script per arm; the two arms differ only in the provider"
    )]
    async fn an_auth_outage_kicks_differently_from_a_refusal() {
        use mc_protocol::packets::Packet as _;
        use mc_protocol::packets::handshake::{Handshake, HandshakeIntent};
        use mc_protocol::packets::login::{
            EncryptionRequest, EncryptionResponse, LoginDisconnect, LoginStart,
        };
        use tokio::io::AsyncWriteExt as _;

        /// One login attempt through `EncryptionResponse`; returns the JSON the
        /// client saw on the disconnect.
        async fn attempt(addr: std::net::SocketAddr) -> String {
            let mut client = tokio::net::TcpStream::connect(addr)
                .await
                .expect("client connects");
            let hello = Handshake {
                protocol_version: 775,
                server_address: "localhost".to_owned(),
                server_port: 25565,
                intent: HandshakeIntent::Login,
            };
            let body = hello.encode().expect("encodes");
            client
                .write_all(&framed(
                    mc_protocol::ids::serverbound::handshake::INTENTION,
                    &body,
                ))
                .await
                .expect("handshake");
            let start = LoginStart {
                name: "Miner".to_owned(),
                uuid: uuid::Uuid::nil(),
            };
            let body = start.encode().expect("encodes");
            client
                .write_all(&framed(mc_protocol::ids::serverbound::login::HELLO, &body))
                .await
                .expect("login start");
            let (id, body) = read_frame(&mut client, None).await;
            assert_eq!(id, mc_protocol::ids::clientbound::login::HELLO);
            let request = EncryptionRequest::decode(&body).expect("decodes");
            let secret = *b"0123456789abcdef";
            let public: rsa::RsaPublicKey =
                rsa::pkcs8::DecodePublicKey::from_public_key_der(&request.public_key)
                    .expect("server key parses");
            let mut rng = rand::thread_rng();
            let answer = EncryptionResponse {
                shared_secret: public
                    .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, &secret)
                    .expect("encrypts"),
                verify_token: public
                    .encrypt(
                        &mut rng,
                        rsa::Pkcs1v15Encrypt,
                        request.verify_token.as_slice(),
                    )
                    .expect("encrypts"),
            };
            let body = answer.encode().expect("encodes");
            client
                .write_all(&framed(mc_protocol::ids::serverbound::login::KEY, &body))
                .await
                .expect("response");
            let (id, body) = read_frame(&mut client, None).await;
            assert_eq!(
                id,
                mc_protocol::ids::clientbound::login::LOGIN_DISCONNECT,
                "a refusal is a kick, not silence"
            );
            LoginDisconnect::decode(&body).expect("decodes").json
        }

        async fn serve_once(
            auth: std::sync::Arc<dyn crate::auth::OnlineAuthProvider>,
            expected_key: &str,
            expect_error: bool,
        ) {
            let identity = crate::online::OnlineIdentity::generate().expect("keygen works");
            let settings = NetworkSettings {
                online_mode: true,
                compression_threshold: -1,
                online_identity: Some(std::sync::Arc::new(identity)),
                ..NetworkSettings::default()
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("loopback binds");
            let addr = listener.local_addr().expect("addr");
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.expect("one client");
                let gate = std::sync::Arc::new(crate::limits::ConnectionGate::new(
                    8,
                    8,
                    std::time::Duration::ZERO,
                ));
                let guard = gate
                    .try_acquire("127.0.0.1".parse().expect("ip"), std::time::Instant::now())
                    .expect("admitted");
                super::run_connection(
                    stream,
                    std::sync::Arc::new(settings),
                    crate::listener::NetworkShutdown::new(),
                    auth,
                    guard,
                    None,
                )
                .await
            });

            // The client half first: it must reach the refusal before the server
            // task can finish, and both ends are in this one test.
            let json = attempt(addr).await;
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            assert_eq!(
                parsed["translate"].as_str(),
                Some(expected_key),
                "the key the client resolves, not a sentence: {json}"
            );
            assert_eq!(
                parsed["fallback"].as_str(),
                Some(if expected_key == super::AUTHSERVERS_DOWN_KEY {
                    "Authentication servers are unavailable"
                } else {
                    "Failed to verify username!"
                }),
                "the English fallback is the sentence the old literal sent"
            );
            // The refusal arm ends without an error (the kick is the whole
            // answer); the outage arm keeps its `Operational` for the caller.
            // The *client leave* classification that follows a kick is pinned
            // by `a_client_that_hangs_up_on_a_refusal_is_not_a_write_failure`.
            let outcome = server.await.expect("the server task does not panic");
            assert_eq!(
                outcome.is_err(),
                expect_error,
                "the refusal the caller sees (an outage is an error, a refusal is not): {outcome:?}"
            );
        }

        // A session-server outage: the player is told the service is down.
        serve_once(
            std::sync::Arc::new(OutageAuth),
            super::AUTHSERVERS_DOWN_KEY,
            true,
        )
        .await;
        // A genuine refusal: the player is told their session did not verify.
        serve_once(
            std::sync::Arc::new(StubAuth { verify: false }),
            super::UNVERIFIED_USERNAME_KEY,
            false,
        )
        .await;
    }

    /// A client that hangs up on a refusal is not a server failure
    /// (AUDIT-19 G-12).
    ///
    /// The audit's live probe saw `socket write failed (os error 10053)` for a
    /// client that read its refusal and closed — a normal hang-up logged as a
    /// fault. Both halves are pinned here, deterministically:
    ///
    /// - the error text names the socket, the packet and the byte count, and
    ///   never the old wording ([`super::write_error`] is tested against the
    ///   very OS error the probe saw, because a loopback socket does not fail
    ///   its write on every platform);
    /// - a kick marks the session refused, and the shared classifier turns an
    ///   operational failure into a client leave once it has. That leg rides
    ///   `an_auth_outage_kicks_differently_from_a_refusal`, which already
    ///   drives a full handshake and now asserts the refused session's outcome;
    ///   neutralise `self.refused = true` in `kick` and it goes red.
    #[tokio::test]
    async fn a_client_that_hangs_up_on_a_refusal_is_not_a_write_failure() {
        use mc_protocol::RawPacket;
        use tokio::io::AsyncReadExt as _;

        // The message a failed write reports, through the real formatter with
        // the failure the live probe saw: the socket is named as the refusing
        // side, and the packet and its size are in the report. This is the
        // neutralisation target for the wording (restore
        // `socket write failed: {error}` and it goes red).
        let packet = RawPacket::new(0x2B, vec![0u8; 16]);
        let failure = std::io::Error::from_raw_os_error(10053);
        let message = super::write_error(&packet, 24, &failure).to_string();
        assert!(
            message.contains("the client's socket refused packet 43"),
            "the message names the socket and the packet: {message}"
        );
        assert!(
            message.contains("(24 bytes)") && message.contains("10053"),
            "and the size and the OS error: {message}"
        );
        assert!(
            !message.contains("socket write failed"),
            "the old wording is gone: {message}"
        );

        // The classification the log line is built from, both ways.
        let refused_error = mc_core::error::ServerError::Operational(message.clone());
        match super::connection_outcome_log(&Err(refused_error), true) {
            super::OutcomeLog::ClientLeave(detail) => {
                assert!(detail.contains(&message), "the detail is carried: {detail}");
            }
            other => panic!("a refused connection is a client leave, got {other:?}"),
        }
        let plain = mc_core::error::ServerError::Operational("os error 10053".to_owned());
        match super::connection_outcome_log(&Err(plain), false) {
            super::OutcomeLog::ServerFault(_) => {}
            other => panic!("an unrefused failure is a server fault, got {other:?}"),
        }
        assert_eq!(
            super::connection_outcome_log(&Ok(()), true),
            super::OutcomeLog::Quiet("connection closed".to_owned()),
            "a clean close is quiet whatever else happened"
        );

        // A kick marks the session, and the same failure is then a client
        // leave rather than a server fault. The cheapest real kick is the
        // missing-identity refusal (online mode with no keypair), so no client
        // script is needed: neutralise `self.refused = true` in `kick` and the
        // flag assertion goes red.
        let (client, server) = tokio::io::duplex(4096);
        let (mut client_reader, _client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let settings = std::sync::Arc::new(NetworkSettings {
            online_mode: true,
            online_identity: None,
            ..NetworkSettings::default()
        });
        let mut session =
            super::Session::new(settings, None, "127.0.0.1".parse().expect("loopback"));
        let mut codec = mc_protocol::framing::FrameCodec::new();
        let outcome = session
            .run_online_login(
                &mut server_reader,
                &mut server_writer,
                &mut codec,
                &StubAuth { verify: true },
                "Miner",
            )
            .await;
        assert!(
            matches!(outcome, Ok(None)),
            "the refusal ends the login without an error: {outcome:?}"
        );
        assert!(
            session.refused,
            "a kick must mark the session, or the client leave is logged as a fault"
        );
        // The client receives the refusal the kick sent (an unread writer would
        // not prove `kick` ran).
        let mut buffer = [0u8; 4096];
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client_reader.read(&mut buffer),
        )
        .await
        .expect("the disconnect arrives")
        .expect("readable");
        assert!(read > 0, "the kick wrote a LoginDisconnect");

        // And the failure that follows such a kick is classified as the
        // client's departure.
        let after_kick = mc_core::error::ServerError::Operational(message);
        assert!(
            matches!(
                super::connection_outcome_log(&Err(after_kick), session.refused),
                super::OutcomeLog::ClientLeave(_)
            ),
            "after a kick the same failure is a client leave"
        );
    }
}
