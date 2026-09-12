//! Server lifecycle and graceful shutdown (P01-08, ADR-0001 D-02, P08-03/04).
//!
//! Ownership model:
//! - [`Server`] owns config + tick clock + shutdown coordination, the network
//!   listener (attached via [`Server::start_network`]) and the open world
//!   (attached via [`Server::open_world`], owned by the simulation).
//! - Shutdown is cooperative: [`Server::shutdown_handle`] clones a
//!   [`ShutdownHandle`]; any thread/task may call [`ShutdownHandle::request`].
//!   [`Server::run`] polls the flag between ticks and returns
//!   [`ServerError::Shutdown`] instead of hanging.
//! - OS signals (Ctrl-C, SIGTERM on unix) feed the same flag, so systemd
//!   stops (P08-04's unit file) and operator Ctrl-C share one code path.
//!
//! Shutdown order (the P08-03 save barrier, ADR-0001 D-04):
//!
//! ```text
//! Running → Stopping → drain the network → bounded final save → Stopped
//! ```
//!
//! The save runs after the network is down so no new player action can land
//! mid-flush, and it is bounded by [`SHUTDOWN_SAVE_TIMEOUT`] so a hung save
//! becomes a loud error instead of a wedged shutdown.
//!
//! Tick discipline: each loop iteration polls the [`TickClock`] for due ticks
//! and counts them. Overdue sleeps use [`TickClock::nanos_until_next`] — no
//! busy-wait. Simulated work per tick is a hook ([`TickHook`]) so Phase 05 can
//! mount the real scheduler without rewriting the loop.

use mc_core::error::{ServerError, ServerResult};
use mc_core::tick::{Tick, TickClock, WallClock};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// Work executed once per tick. Phase 01 ships a no-op; Phase 05 mounts the
/// world/entity scheduler behind this trait without touching the loop.
pub trait TickHook: Send + Sync {
    /// Run one tick. Returning [`ServerError::Shutdown`] stops the server.
    ///
    /// # Errors
    ///
    /// Any error stops the loop and propagates to [`Server::run`] callers.
    fn on_tick(&self, tick: Tick) -> ServerResult<()>;
}

/// No-op hook: proves the loop advances without inventing gameplay.
#[derive(Debug, Default)]
pub struct NoopHook;

impl TickHook for NoopHook {
    fn on_tick(&self, _tick: Tick) -> ServerResult<()> {
        Ok(())
    }
}

/// Capacity of the network → game event queue.
///
/// Sized so a full server (10 players) plus status pings can burst without
/// dropping gameplay events, while still bounding memory if the tick loop stalls.
pub const EVENT_QUEUE: usize = 1024;

/// Longest a shutdown waits for the final world save before giving up.
///
/// The save runs on the tick thread and is normally milliseconds; 30 s is the
/// backstop that turns a hung save into a loud error instead of a wedged
/// shutdown (P08-03). Named rather than inline so the test and the path agree.
pub const SHUTDOWN_SAVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Cooperative shutdown flag. Cheap to clone, safe to share across the tick
/// thread, Tokio tasks and signal handlers.
#[derive(Debug, Clone)]
pub struct ShutdownHandle {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Request shutdown. Idempotent; wakes a sleeping [`Server::run`].
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether shutdown was requested.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn notified(&self) {
        self.notify.notified().await;
    }
}

/// Lifecycle states, for logs and the future systemd notifier (P08-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// Built, not yet running.
    Starting,
    /// Tick loop active.
    Running,
    /// Shutdown requested, unwinding (save barrier lands here in P08-03).
    Stopping,
    /// Loop exited.
    Stopped,
}

/// The server. Owns config, clock, shutdown coordination, the network listener
/// (attached via [`Server::start_network`]) and the open world (attached via
/// [`Server::open_world`]).
pub struct Server<H: TickHook = NoopHook> {
    config: crate::config::ServerConfig,
    clock: TickClock,
    wall: WallClock,
    shutdown: ShutdownHandle,
    state: LifecycleState,
    hook: H,
    network: Option<mc_network::NetworkService>,
    /// Simulation, present once a world and a game channel exist. The simulation
    /// owns the open world; [`Server::world`] reads it through here.
    game: Option<crate::game::Game>,
    /// Sending end of the network → game channel, kept so `start_network` can
    /// hand a [`mc_network::listener::GameLink`] to the listener.
    game_link: Option<mc_network::listener::GameLink>,
}

impl Server<NoopHook> {
    /// Build a server from validated config with the Phase-01 no-op hook.
    #[must_use]
    pub fn new(config: crate::config::ServerConfig) -> Self {
        Self::with_hook(config, NoopHook)
    }
}

impl<H: TickHook> Server<H> {
    /// Build a server with an explicit tick hook (tests, future scheduler).
    #[must_use]
    pub fn with_hook(config: crate::config::ServerConfig, hook: H) -> Self {
        Self {
            config,
            clock: TickClock::new(),
            wall: WallClock::real(),
            shutdown: ShutdownHandle::new(),
            state: LifecycleState::Starting,
            hook,
            network: None,
            game: None,
            game_link: None,
        }
    }

    /// Open (or create) the world described by `config.storage`, then build the
    /// simulation and the channel the network layer feeds.
    ///
    /// Kept separate from [`Server::start_network`] so tests and tooling can run
    /// without touching the filesystem.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the world directory cannot be created or
    /// written, or the registry tables cannot be read;
    /// [`ServerError::CorruptData`] when the existing `level.dat` cannot be
    /// interpreted.
    pub fn open_world(&mut self) -> ServerResult<()> {
        let world = crate::storage::WorldService::open(&self.config.storage)?;
        let (events_tx, events_rx) = mc_network::bridge::game_channel(EVENT_QUEUE);
        // The game takes ownership of the open world so its Broadcast phase can
        // load a chunk from disk before creating an all-air placeholder for it;
        // `Server` therefore drives saving through the game and never closes the
        // handle itself (see `Game`'s threading notes).
        // `ops.json` lives beside `server.properties`, which is the directory *containing*
        // the world. A malformed file is logged and treated as empty rather than stopping the
        // boot: refusing to start would take a working world offline over an operator file,
        // and the operator can see the message and fix it.
        let operators = match crate::ops::OperatorList::load(&crate::ops::ops_directory(
            &self.config.storage.world_dir,
        )) {
            Ok(list) => {
                if !list.is_empty() {
                    tracing::info!(
                        operators = list.len(),
                        bypasses = list.bypass_count(),
                        "loaded the operator list"
                    );
                }
                list
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "ops.json could not be read; the server will run with no operators"
                );
                crate::ops::OperatorList::new()
            }
        };
        let mut game = crate::game::Game::build_with_operators(
            None,
            Some(world),
            i32::try_from(self.config.simulation.view_distance).unwrap_or(8),
            events_rx,
            crate::game::DEFAULT_RANDOM_SEED,
            operators,
        )?;
        // Data packs. The world's `DataPacks` list is read from the `level.dat` of the world just
        // opened, which is why this happens here and not at config-validation time: the list is
        // world data, not configuration.
        //
        // The enabled list decides which world packs load. A world with **no** list enables
        // everything, which is the case that matters: reading an absent list as "nothing enabled"
        // would silently disable every pack in a world that has been running for months.
        // `storage().level()` is the same path the operator list uses: the world's parsed
        // `level.dat`, which is where `DataPacks` lives.
        let enabled = self
            .world()
            .and_then(|service| service.storage().level())
            .map_or_else(mc_data::enabled::EnabledPacks::default, |level| {
                mc_data::enabled::EnabledPacks::from_level_dat(
                    &level.enabled_packs,
                    &level.disabled_packs,
                )
            });
        let mut roots = crate::packs::PackRoots::new(&self.config.storage.world_dir);
        if let Some(configured) = &self.config.datapacks.vanilla_data {
            // `with_vanilla_data` resolves the level the operator named, so nothing here has to.
            roots = roots.with_vanilla_data(configured);
        }
        match crate::packs::load_packs(&mut game, &roots, &enabled) {
            Ok(outcome) => {
                if outcome.has_problems() {
                    tracing::warn!(
                        packs = outcome.packs_loaded,
                        functions = outcome.functions_loaded,
                        rejected = ?outcome.rejected,
                        "some data packs could not be read"
                    );
                } else {
                    tracing::info!(
                        packs = outcome.packs_loaded,
                        functions = outcome.functions_loaded,
                        namespaces = outcome.namespaces.len(),
                        skipped = outcome.skipped.len(),
                        vanilla_data = outcome.vanilla_data,
                        "data packs loaded"
                    );
                }
            }
            // A pack problem must never stop the boot: a world that has run for months has to start
            // on a machine where nobody copied the jar data (AGENTS.md §9).
            Err(error) => {
                tracing::error!(%error, "loading data packs failed; the server continues without them");
            }
        }

        // The lifecycle is the authority for the player cap, so it tells the game
        // rather than the game reading the config itself (`/list` is the only consumer).
        game.set_max_players(self.config.network.max_players);
        self.game_link = Some(mc_network::listener::GameLink::new(
            events_tx,
            mc_network::bridge::DEFAULT_OUTBOUND_CAPACITY,
        ));
        self.game = Some(game);
        Ok(())
    }

    /// The open world, if any.
    ///
    /// The *simulation* owns the handle — [`Server::open_world`] hands it to
    /// [`crate::game::Game`] so its Broadcast phase can read a chunk from disk
    /// before creating an all-air placeholder for it — so this accessor reports the
    /// world through the game rather than through a second field. One owner, one
    /// path: a `Server`-held clone would be a second handle to the same directory,
    /// and the two would disagree about which chunks are loaded.
    #[must_use]
    pub fn world(&self) -> Option<&crate::storage::WorldService> {
        self.game.as_ref().and_then(crate::game::Game::storage)
    }

    /// Mutable access to the open world.
    pub fn world_mut(&mut self) -> Option<&mut crate::storage::WorldService> {
        self.game.as_mut().and_then(crate::game::Game::storage_mut)
    }

    /// The running simulation, if any.
    #[must_use]
    pub const fn game(&self) -> Option<&crate::game::Game> {
        self.game.as_ref()
    }

    /// Mutable access to the simulation (tests and admin tooling).
    pub fn game_mut(&mut self) -> Option<&mut crate::game::Game> {
        self.game.as_mut()
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// Current tick count.
    #[must_use]
    pub fn tick(&self) -> Tick {
        self.clock.tick()
    }

    /// Validated config this server was built from.
    #[must_use]
    pub fn config(&self) -> &crate::config::ServerConfig {
        &self.config
    }

    /// Cloneable handle that requests shutdown from anywhere.
    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.clone()
    }

    /// Bind the TCP listener and start serving the protocol slice.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the bind fails or online mode is
    /// enabled (the encryption/session flow is not implemented yet; see
    /// the Phase 02 report, git history tag `phase-09-final`).
    pub async fn start_network(&mut self) -> ServerResult<std::net::SocketAddr> {
        if self.config.network.online_mode {
            return Err(ServerError::Operational(
                "online_mode is enabled but the Mojang session/encryption flow is not implemented yet".to_owned(),
            ));
        }
        let bind: std::net::SocketAddr = self
            .config
            .network
            .bind
            .parse()
            .map_err(|e| ServerError::Operational(format!("invalid bind address: {e}")))?;
        let settings = mc_network::NetworkSettings {
            bind,
            max_players: self.config.network.max_players,
            online_mode: self.config.network.online_mode,
            compression_threshold: self.config.network.compression_threshold,
            view_distance: i32::try_from(self.config.simulation.view_distance).unwrap_or(8),
            motd: self.config.network.motd.clone(),
            ..mc_network::NetworkSettings::default()
        };
        let service =
            mc_network::NetworkService::start_with_game(settings, None, self.game_link.clone())
                .await?;
        let addr = service.local_addr();
        self.network = Some(service);
        Ok(addr)
    }

    /// Address the network listener is bound to, if started.
    #[must_use]
    pub fn local_addr(&self) -> Option<std::net::SocketAddr> {
        self.network
            .as_ref()
            .map(mc_network::NetworkService::local_addr)
    }

    /// Install Ctrl-C + (unix) SIGTERM handlers feeding the shutdown flag.
    ///
    /// Call once per process. Extra calls are harmless (both waiters call
    /// [`ShutdownHandle::request`], which is idempotent) but spawn a redundant
    /// task each time, so callers should not rely on that.
    pub fn install_signal_handlers(&self) {
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
                let Ok(mut term) = term else {
                    // A missing SIGTERM handler must not abort the process: fall
                    // back to Ctrl-C so shutdown is still possible.
                    tracing::error!("cannot install the SIGTERM handler; Ctrl-C still works");
                    let _ = tokio::signal::ctrl_c().await;
                    tracing::info!("shutdown signal received");
                    shutdown.request();
                    return;
                };
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = term.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            tracing::info!("shutdown signal received");
            shutdown.request();
        });
    }

    /// Run the fixed-step loop until shutdown is requested or the hook fails.
    ///
    /// Returns [`ServerError::Shutdown`] on cooperative exit so callers can
    /// distinguish it from real failures.
    ///
    /// # Errors
    ///
    /// Propagates hook failures; returns [`ServerError::Shutdown`] when the
    /// shutdown flag is set.
    // The body is one linear narrative — start the network, loop the fixed step,
    // then stop in a defined order. Splitting it would put the shutdown ordering
    // (which ADR-0001 D-04 depends on) behind a function call for no benefit.
    #[allow(clippy::too_many_lines)]
    pub async fn run(&mut self) -> ServerResult<()> {
        self.state = LifecycleState::Running;
        tracing::info!(
            version = crate::config::VERSION_NAME,
            protocol = crate::config::PROTOCOL_VERSION,
            bind = %self.config.network.bind,
            max_players = self.config.network.max_players,
            online_mode = self.config.network.online_mode,
            "server running"
        );
        loop {
            if self.shutdown.is_requested() {
                break;
            }
            let now = self.wall.now_nanos();
            let due = self.clock.poll(now);
            for _ in 0..due {
                let tick = self.clock.tick();
                self.hook.on_tick(tick)?;
                // Simulation runs on the tick thread; the network layer only
                // decodes and hands over intents (ADR-0001 D-02/D-03). `tick()`
                // runs the six scheduled phases and returns this tick's counters.
                if let Some(game) = self.game.as_mut() {
                    let report = game.tick()?;
                    // `/stop` sets this flag; draining it here is how a command reaches
                    // the lifecycle without the game holding a shutdown handle. The
                    // direction stays one-way: the server reads the game.
                    if game.shutdown_requested() {
                        tracing::info!(tick, "shutdown requested by command");
                        self.shutdown.request();
                    }
                    if report.events > 0 || report.block_changes > 0 {
                        tracing::trace!(
                            tick,
                            events = report.events,
                            blocks = report.block_changes,
                            chunks = report.chunks_sent,
                            "tick"
                        );
                    }
                }
                // Autosave runs on the tick thread until P08-03 hands storage to
                // a save worker (ADR-0001 D-02, `storage.rs` threading note). The
                // simulation owns the handle, so it asks its own autosave clock and
                // does the save itself: `save_all_owned` pushes the dirty chunks and
                // flushes, which is exactly what a shutdown save does, so a periodic
                // save and a final save persist the same data.
                if let Some(game) = self.game.as_mut() {
                    let due = match game.storage_mut() {
                        Some(storage) => storage.storage_mut().autosave_mut().on_tick(tick),
                        None => false,
                    };
                    if due {
                        game.save_all_owned()?;
                    }
                }
                // One summary line every 30 s (P05-18 input, P08-02 fields): enough
                // to see a trend, cheap enough not to matter inside a 50 ms budget.
                // Field order follows `OperationalSnapshot::log` so the two lines
                // stay greppable as one shape.
                if tick.is_multiple_of(crate::game::METRICS_LOG_INTERVAL_TICKS)
                    && let Some(game) = self.game.as_ref()
                {
                    crate::metrics::OperationalSnapshot::of(game).log(tick);
                }
            }
            if self.shutdown.is_requested() {
                break;
            }
            let sleep_nanos = self.clock.nanos_until_next(self.wall.now_nanos());
            if sleep_nanos > 0 {
                let sleep = tokio::time::sleep(std::time::Duration::from_nanos(sleep_nanos));
                tokio::pin!(sleep);
                tokio::select! {
                    () = &mut sleep => {},
                    () = self.shutdown.notified() => {},
                }
            } else {
                // Overdue: yield so signal/notify tasks get a chance to run
                // instead of starving them in a tight catch-up loop.
                tokio::task::yield_now().await;
            }
        }
        // The save barrier (P08-03): Stopping is entered *before* the network
        // drains, so every phase of the shutdown is observable in order —
        // Running → Stopping → (drain, then save) → Stopped — rather than
        // inferred from a log line after the fact.
        self.state = LifecycleState::Stopping;
        tracing::info!(ticks = self.clock.tick(), "server stopping");
        if let Some(network) = self.network.take() {
            network.shutdown().await;
        }
        // Save after the network is down: no new player actions can arrive, so
        // the flushed world matches the last tick that ran (ADR-0001 D-04). The
        // simulation's dirty chunks go out first, then the world closes.
        //
        // The save is **bounded**: a save that hangs must not wedge the shutdown
        // forever (P08-03). `close_storage` runs on the tick thread and finishes
        // in practice in milliseconds; the timeout is the backstop that turns a
        // hung save into a loud error instead of a silent hang.
        if let Some(mut game) = self.game.take() {
            match tokio::time::timeout(SHUTDOWN_SAVE_TIMEOUT, async { game.close_storage() }).await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "world failed to close cleanly");
                    self.state = LifecycleState::Stopped;
                    return Err(e);
                }
                Err(_) => {
                    tracing::error!("shutdown save timed out; the world may be incomplete");
                    self.state = LifecycleState::Stopped;
                    return Err(ServerError::Operational(
                        "shutdown save timed out".to_owned(),
                    ));
                }
            }
        }
        self.state = LifecycleState::Stopped;
        Err(ServerError::Shutdown)
    }
}

#[cfg(test)]
mod tests {
    use super::{LifecycleState, NoopHook, Server, ShutdownHandle, TickHook};
    use mc_core::error::{ServerError, ServerResult};
    use mc_core::tick::Tick;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    struct CountingHook {
        ticks: Arc<AtomicU64>,
    }

    impl TickHook for CountingHook {
        fn on_tick(&self, _tick: Tick) -> ServerResult<()> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_exits_cooperatively_on_shutdown_request() {
        let mut server = Server::new(crate::config::ServerConfig::default());
        assert_eq!(server.state(), LifecycleState::Starting);
        let handle: ShutdownHandle = server.shutdown_handle();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            handle.request();
        });
        let err = server
            .run()
            .await
            .expect_err("run returns Shutdown, not Ok");
        assert!(matches!(err, ServerError::Shutdown), "wrong error: {err:?}");
        assert_eq!(server.state(), LifecycleState::Stopped);
        assert!(server.tick() > 0, "loop must have ticked before shutdown");
    }

    #[tokio::test]
    async fn tick_hook_observes_monotonic_ticks() {
        let seen = Arc::new(AtomicU64::new(0));
        let hook = CountingHook {
            ticks: seen.clone(),
        };
        let mut server = Server::with_hook(crate::config::ServerConfig::default(), hook);
        let handle = server.shutdown_handle();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(160)).await;
            handle.request();
        });
        let _ = server.run().await;
        let ticks = seen.load(Ordering::SeqCst);
        assert!(
            ticks >= 2,
            "expected several 50ms ticks in 160ms, got {ticks}"
        );
    }

    #[tokio::test]
    async fn hook_failure_propagates_and_stops() {
        struct FailingHook;
        impl TickHook for FailingHook {
            fn on_tick(&self, _tick: Tick) -> ServerResult<()> {
                Err(ServerError::Invariant("boom".to_owned()))
            }
        }
        let mut server = Server::with_hook(crate::config::ServerConfig::default(), FailingHook);
        // Pre-request shutdown is NOT set; the hook error must surface instead.
        // Give the clock a chance to release a tick by pre-anchoring via a short run.
        tokio::select! {
            r = server.run() => {
                assert!(matches!(r, Err(ServerError::Invariant(_))), "wrong result: {r:?}");
            }
            () = tokio::time::sleep(Duration::from_millis(500)) => {
                panic!("hook failure did not stop the loop");
            }
        }
        let _ = NoopHook;
    }

    #[tokio::test]
    async fn shutdown_passes_through_stopping_before_stopped() {
        // P08-03: the save barrier must be observable as a state, not inferred.
        // No network and no world here, so the run drains immediately; the
        // assertion is that Stopping was entered on the way out.
        let mut server = Server::new(crate::config::ServerConfig::default());
        assert_eq!(server.state(), LifecycleState::Starting);
        let handle = server.shutdown_handle();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            handle.request();
        });
        let err = server.run().await.expect_err("shutdown");
        assert!(matches!(err, ServerError::Shutdown), "{err:?}");
        assert_eq!(server.state(), LifecycleState::Stopped);
    }

    #[tokio::test]
    async fn world_is_created_at_startup_and_saved_at_shutdown() {
        let dir = mc_test_support::fixtures::TempDir::new("lifecycle-world");
        let mut config = crate::config::ServerConfig::default();
        config.storage.world_dir = dir.path().join("world");
        config.storage.autosave_ticks = 0; // explicit saves only
        let mut server = Server::new(config.clone());
        assert!(server.world().is_none(), "no world until opened");
        server.open_world().expect("opens the world");
        // The simulation owns the handle (so its Broadcast phase can read chunks
        // from disk); `Server::world` reports it through the game rather than
        // keeping a second handle to the same directory.
        assert_eq!(
            server.world().expect("the game owns the open world").root(),
            config.storage.world_dir.as_path()
        );
        assert!(server.game().is_some(), "the simulation is built");

        let handle = server.shutdown_handle();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            handle.request();
        });
        let err = server.run().await.expect_err("shutdown");
        assert!(matches!(err, ServerError::Shutdown), "{err:?}");
        assert!(server.world().is_none(), "the world is closed by run()");
        // The shutdown path left a complete, readable level.dat behind.
        let level = mc_persistence::level::LevelDat::from_bytes(
            &std::fs::read(config.storage.world_dir.join("level.dat")).expect("level.dat"),
        )
        .expect("decodes");
        assert_eq!(
            level.data_version,
            mc_persistence::level::DATA_VERSION_26_1_2
        );
        assert_eq!(level.level_name, "world");
    }
}
