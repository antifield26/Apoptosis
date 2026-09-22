//! Listener and service lifecycle (P02-01).
//!
//! [`NetworkService::start`] binds the TCP socket, spawns the accept loop and
//! returns a handle exposing the resolved local address (important for tests
//! binding port 0). [`NetworkService::shutdown`] stops accepting and signals
//! every live connection through [`NetworkShutdown`]; connection tasks then
//! send their state-appropriate disconnect and close.

use crate::auth::OnlineAuthProvider;
use crate::connection::{default_auth, run_connection};
use crate::limits::ConnectionGate;
use mc_core::error::{ServerError, ServerResult};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};

/// Everything the connection layer needs to know about the server.
#[derive(Debug, Clone)]
pub struct NetworkSettings {
    /// Socket to bind.
    pub bind: SocketAddr,
    /// Advertised player slots (also the budget headroom base).
    pub max_players: u32,
    /// Online mode toggle (offline is the product default).
    pub online_mode: bool,
    /// Compression threshold; negative disables compression.
    pub compression_threshold: i32,
    /// Chunk view distance sent in `JoinGame`.
    pub view_distance: i32,
    /// Server-list description.
    pub motd: String,
    /// Version string surfaced in status/known-packs.
    pub version_name: String,
    /// Protocol number surfaced in status and enforced at handshake.
    pub protocol_version: i32,
    /// Keepalive cadence in the play state.
    pub keepalive_interval: Duration,
    /// Time allowed for a keepalive response before disconnecting.
    pub keepalive_timeout: Duration,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:25565".parse().expect("valid default bind"),
            max_players: 10,
            online_mode: false,
            compression_threshold: 256,
            view_distance: 8,
            motd: "A Rust Minecraft Server".to_owned(),
            version_name: mc_protocol::ids::VERSION_NAME.to_owned(),
            protocol_version: mc_protocol::ids::PROTOCOL_VERSION,
            keepalive_interval: Duration::from_secs(15),
            keepalive_timeout: Duration::from_secs(30),
        }
    }
}

/// Cooperative network shutdown: cloneable, idempotent, wakes waiters.
#[derive(Debug, Clone, Default)]
pub struct NetworkShutdown {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl NetworkShutdown {
    /// Create a fresh signal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request shutdown.
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether shutdown was requested.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Wait until shutdown is requested.
    pub async fn notified(&self) {
        while !self.is_requested() {
            self.notify.notified().await;
        }
    }
}

/// Everything a connection needs to talk to the game loop.
///
/// Passed to [`NetworkService::start_with_game`]; absent in Phase-02 style tests
/// that only exercise the protocol handshake, in which case play-state intents are
/// traced instead of delivered (which is what the Phase 02 report describes).
#[derive(Debug, Clone)]
pub struct GameLink {
    events: tokio::sync::mpsc::Sender<crate::bridge::ClientEvent>,
    ids: Arc<crate::bridge::ConnectionIds>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    outbound_capacity: usize,
}

impl GameLink {
    /// Build a link from the game loop's receiving half.
    #[must_use]
    pub fn new(
        events: tokio::sync::mpsc::Sender<crate::bridge::ClientEvent>,
        outbound_capacity: usize,
    ) -> Self {
        Self {
            events,
            ids: Arc::new(crate::bridge::ConnectionIds::new()),
            dropped: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            outbound_capacity,
        }
    }

    /// Total events dropped because the game loop's queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Allocate the next connection id.
    #[must_use]
    pub fn next_id(&self) -> crate::bridge::ConnectionId {
        self.ids.next_id()
    }

    /// Outbound queue capacity for a new connection.
    #[must_use]
    pub const fn outbound_capacity(&self) -> usize {
        self.outbound_capacity
    }

    /// Create the bridge halves for `id`.
    #[must_use]
    pub fn channel_for(
        &self,
        id: crate::bridge::ConnectionId,
    ) -> (
        crate::bridge::EventSender,
        crate::bridge::InboundReceiver,
        crate::bridge::OutboundSender,
    ) {
        crate::bridge::channel(
            id,
            self.events.clone(),
            Arc::clone(&self.dropped),
            self.outbound_capacity,
        )
    }
}

/// Running network service.
pub struct NetworkService {
    local_addr: SocketAddr,
    shutdown: NetworkShutdown,
    accept_task: JoinHandle<JoinSet<()>>,
    gate: Arc<ConnectionGate>,
}

impl NetworkService {
    /// Bind and start serving.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the socket cannot be bound.
    pub async fn start(
        settings: NetworkSettings,
        auth: Option<Arc<dyn OnlineAuthProvider>>,
    ) -> ServerResult<Self> {
        Self::start_with_game(settings, auth, None).await
    }

    /// Bind and start serving, delivering play-state intents to `game`.
    ///
    /// With `game` set, a connection that reaches the play state publishes a
    /// [`crate::bridge::ClientEventKind::Joined`] event carrying an
    /// [`crate::bridge::OutboundSender`]; the game loop then drives the world side.
    /// With `game` unset the connection behaves as in Phase 02: intents are decoded
    /// and traced, and nothing is simulated.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the socket cannot be bound.
    pub async fn start_with_game(
        settings: NetworkSettings,
        auth: Option<Arc<dyn OnlineAuthProvider>>,
        game: Option<GameLink>,
    ) -> ServerResult<Self> {
        let listener = TcpListener::bind(settings.bind).await.map_err(|error| {
            ServerError::Operational(format!("cannot bind {}: {error}", settings.bind))
        })?;
        let local_addr = listener.local_addr().map_err(|error| {
            ServerError::Operational(format!("cannot read local address: {error}"))
        })?;
        let shutdown = NetworkShutdown::new();
        // Headroom covers status pings and simultaneous login handshakes. The
        // three numbers are named at the gate constructor (P08-06): the global
        // budget is players plus headroom, the per-IP cap stops one address
        // owning the server, and the refill interval blunts reconnect storms.
        let connection_gate = Arc::new(ConnectionGate::new(
            settings
                .max_players
                .saturating_add(GATE_HEADROOM_CONNECTIONS),
            MAX_CONNECTIONS_PER_IP,
            RECONNECT_REFILL_INTERVAL,
        ));
        let auth = auth.unwrap_or_else(default_auth);
        let task_shutdown = shutdown.clone();
        let task_settings = Arc::new(settings);
        let task_gate = Arc::clone(&connection_gate);
        let accept_task = tokio::spawn(async move {
            accept_loop(
                listener,
                task_settings,
                task_shutdown,
                task_gate,
                auth,
                game,
            )
            .await
        });
        tracing::info!(%local_addr, "network listener started");
        Ok(Self {
            local_addr,
            shutdown,
            accept_task,
            gate: connection_gate,
        })
    }

    /// The actually-bound address (resolves `:0` to the OS-assigned port).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Current tracked connection-budget usage (diagnostics/tests).
    #[must_use]
    pub fn gate(&self) -> &Arc<ConnectionGate> {
        &self.gate
    }

    /// Stop accepting, signal live connections, and wait for them to finish.
    ///
    /// The per-connection tasks are tracked in a [`JoinSet`], so shutdown drains
    /// them instead of returning while they may still be reading. Callers rely on
    /// this ordering: the server closes the world *after* the network is down, so
    /// no player action can be applied to a world that is already being saved
    /// (`mc_server::lifecycle::Server::run`).
    ///
    /// A connection that ignores the shutdown signal is bounded by the drain
    /// timeout rather than blocking shutdown forever.
    pub async fn shutdown(self) {
        self.shutdown.request();
        // The accept loop observes the signal via `select!`; if it is blocked
        // in `accept`, the pending future is dropped on wake.
        let mut connections = match self.accept_task.await {
            Ok(connections) => connections,
            Err(_) => JoinSet::new(),
        };
        let live = connections.len();
        if live > 0 {
            tracing::info!(connections = live, "waiting for connections to finish");
        }
        let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
            while connections.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                remaining = connections.len(),
                "connection drain timed out; aborting the rest"
            );
            connections.abort_all();
        }
        tracing::info!("network listener stopped");
    }
}

/// How long shutdown waits for live connections before aborting them.
///
/// Long enough for a connection to notice the shutdown signal and send its
/// disconnect packet, short enough that an unresponsive peer cannot delay a
/// restart by more than this.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Extra sockets beyond `max_players` the gate admits (P08-06).
///
/// Status pings and in-flight login handshakes are connections too: refusing
/// them the moment the player count hits the cap would make the server list
/// lie and logins flake under exactly the load the cap is meant to serve.
/// Eight covers a full server plus a burst of pings.
pub const GATE_HEADROOM_CONNECTIONS: u32 = 8;

/// Most concurrent connections one IP address may hold (P08-06).
///
/// Four lets a normal household (a few players plus status pings behind one
/// NAT address) play while stopping one address from owning the whole global
/// budget. The game loop's own player cap is the second line behind this one.
pub const MAX_CONNECTIONS_PER_IP: u32 = 4;

/// Time to regain one reconnect token after the burst is spent (P08-06).
///
/// Eight tokens burst, then one per 250 ms: a normal login (a handful of
/// connections) never notices, while a reconnect storm is throttled to four
/// connections per second per address.
pub const RECONNECT_REFILL_INTERVAL: Duration = Duration::from_millis(250);

async fn accept_loop(
    listener: TcpListener,
    settings: Arc<NetworkSettings>,
    shutdown: NetworkShutdown,
    gate: Arc<ConnectionGate>,
    auth: Arc<dyn OnlineAuthProvider>,
    game_link: Option<GameLink>,
) -> JoinSet<()> {
    let mut connections: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = shutdown.notified() => break,
            // Reap finished connections so the set does not grow without bound.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        match gate.try_acquire(peer.ip(), Instant::now()) {
                            Ok(guard) => {
                                let settings = Arc::clone(&settings);
                                let shutdown = shutdown.clone();
                                let auth = Arc::clone(&auth);
                                let link = game_link.clone();
                                connections.spawn(async move {
                                    if let Err(error) = run_connection(
                                        stream, settings, shutdown, auth, guard, link,
                                    )
                                    .await
                                    {
                                        // Owner placement disconnects: this
                                        // result used to be discarded, so a
                                        // connection killed by a decode
                                        // failure looked exactly like a clean
                                        // client leave. Clean disconnects are
                                        // `Ok` and stay silent.
                                        tracing::warn!(%peer, %error, "connection ended with an error");
                                    }
                                });
                            }
                            Err(reason) => {
                                tracing::debug!(%peer, ?reason, "connection refused by limits");
                                drop(stream);
                            }
                        }
                    }
                    Err(error) => {
                        // Accept errors on one connection must not kill the loop.
                        tracing::warn!(%error, "accept failed");
                    }
                }
            }
        }
    }
    connections
}

#[cfg(test)]
mod tests {
    use super::{
        GATE_HEADROOM_CONNECTIONS, MAX_CONNECTIONS_PER_IP, NetworkService, NetworkSettings,
        NetworkShutdown, RECONNECT_REFILL_INTERVAL,
    };
    use std::time::Duration;

    #[test]
    fn the_gate_budget_is_players_plus_named_headroom() {
        // P08-06: the numbers a reviewer asks about live here, not inline.
        // A 10-player server admits 18 sockets; one address may hold 4 of them.
        assert_eq!(GATE_HEADROOM_CONNECTIONS, 8);
        assert_eq!(MAX_CONNECTIONS_PER_IP, 4);
        assert_eq!(RECONNECT_REFILL_INTERVAL, Duration::from_millis(250));
        let settings = NetworkSettings {
            max_players: 10,
            ..NetworkSettings::default()
        };
        assert_eq!(
            settings
                .max_players
                .saturating_add(GATE_HEADROOM_CONNECTIONS),
            18
        );
    }

    #[tokio::test]
    async fn service_binds_ephemeral_port_and_stops() {
        let settings = NetworkSettings {
            bind: "127.0.0.1:0".parse().expect("addr"),
            ..NetworkSettings::default()
        };
        let service = NetworkService::start(settings, None).await.expect("starts");
        assert_ne!(service.local_addr().port(), 0);
        service.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_signal_is_idempotent_and_visible() {
        let shutdown = NetworkShutdown::new();
        assert!(!shutdown.is_requested());
        shutdown.request();
        shutdown.request();
        assert!(shutdown.is_requested());
        tokio::time::timeout(Duration::from_millis(50), shutdown.notified())
            .await
            .expect("returns immediately");
    }
}
