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
    /// Set once the join event has been published.
    joined: bool,
}

impl Session {
    fn new(settings: Arc<NetworkSettings>, game: Option<GameLink>) -> Self {
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
            joined: false,
        }
    }

    async fn resolve_profile(
        &self,
        provider: &dyn OnlineAuthProvider,
        name: &str,
    ) -> ServerResult<GameProfile> {
        if self.settings.online_mode {
            // Online flow (encryption request/response + session check) is
            // deferred; `mc-server` refuses to start with online mode on, so
            // this branch only runs when an operator wired a real provider.
            provider.authenticate(name, "").await
        } else {
            Ok(offline_profile(name))
        }
    }

    async fn send_typed<T: Packet>(
        &self,
        writer: &mut OwnedWriteHalf,
        packet: &T,
    ) -> ServerResult<()> {
        let raw = packet.to_raw()?;
        send_raw(writer, self.compression, &raw).await
    }

    async fn send_raw_packet(
        &self,
        writer: &mut OwnedWriteHalf,
        raw: &RawPacket,
    ) -> ServerResult<()> {
        send_raw(writer, self.compression, raw).await
    }

    /// Send the state-appropriate disconnect packet, then close.
    async fn kick(&self, writer: &mut OwnedWriteHalf, state: ConnectionState, reason: &str) {
        let text = TextComponent::literal(reason);
        let result = match state {
            ConnectionState::Login => {
                self.send_typed(
                    writer,
                    &LoginDisconnect {
                        json: text.to_json(),
                    },
                )
                .await
            }
            ConnectionState::Configuration => {
                self.send_typed(writer, &ConfigDisconnect { reason: text })
                    .await
            }
            ConnectionState::Play => {
                self.send_typed(writer, &PlayDisconnect { reason: text })
                    .await
            }
            ConnectionState::Handshake | ConnectionState::Status => Ok(()),
        };
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
    let mut session = Session::new(Arc::clone(&settings), game);
    let mut codec = FrameCodec::new();

    let outcome = session
        .run(&mut reader, &mut writer, &mut codec, &shutdown, &*auth)
        .await;
    // Tell the game loop the player is gone, so it can unload them and save.
    if session.joined {
        session.report(crate::bridge::ClientEventKind::Left);
    }
    match &outcome {
        Ok(()) => tracing::debug!(%peer, "connection closed"),
        Err(ServerError::Protocol(message)) => {
            tracing::debug!(%peer, %message, "protocol error, connection closed");
        }
        Err(ServerError::InvalidAction(message)) => {
            tracing::debug!(%peer, %message, "invalid action, connection closed");
        }
        Err(ServerError::Shutdown) => tracing::debug!(%peer, "connection closed for shutdown"),
        Err(error) => tracing::info!(%peer, %error, "connection failed"),
    }
    let _ = writer.shutdown().await;
    outcome
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
        let Some(packet) = read_packet(reader, codec, PHASE_TIMEOUT).await? else {
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
                    self.kick(writer, ConnectionState::Login, &reason).await;
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
            let Some(packet) = read_packet(reader, codec, PHASE_TIMEOUT).await? else {
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
        let Some(packet) = read_packet(reader, codec, PHASE_TIMEOUT).await? else {
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
            let reason = format!("Invalid username: {invalid}");
            self.kick(writer, ConnectionState::Login, &reason).await;
            return Ok(());
        }
        let profile = self.resolve_profile(auth, &login_start.name).await?;
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
                properties: Vec::new(),
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
            let Some(packet) = read_packet(reader, codec, PHASE_TIMEOUT).await? else {
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

    async fn send_configuration(&self, writer: &mut OwnedWriteHalf) -> ServerResult<()> {
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
            let Some(packet) = read_packet(reader, codec, PHASE_TIMEOUT).await? else {
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
                    self.kick(writer, ConnectionState::Play, "Server shutting down").await;
                    return Err(ServerError::Shutdown);
                }
                () = tokio::time::sleep_until(next_event) => {
                    if self.pending_keepalive.is_some() {
                        tracing::info!("keepalive timeout, disconnecting player");
                        self.kick(writer, ConnectionState::Play, "Timed out").await;
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
                packet = read_packet(reader, codec, self.settings.keepalive_timeout) => {
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
async fn read_packet(
    reader: &mut OwnedReadHalf,
    codec: &mut FrameCodec,
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
            Ok(Ok(read)) => codec.feed(&buffer[..read])?,
            Ok(Err(error)) => {
                return Err(ServerError::Operational(format!(
                    "socket read failed: {error}"
                )));
            }
        }
    }
}

/// Encode and write one packet with the negotiated compression.
async fn send_raw(
    writer: &mut OwnedWriteHalf,
    compression: Option<i32>,
    packet: &RawPacket,
) -> ServerResult<()> {
    let bytes = FrameCodec::encode(packet, compression)?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|error| ServerError::Operational(format!("socket write failed: {error}")))
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
    use super::status_json;
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
}
