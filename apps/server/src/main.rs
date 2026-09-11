//! `mc-server` executable entrypoint (P01-08).
//!
//! Behaviour: read config (file arg or defaults) → init logging → build
//! [`Server`][mc_server] → install signal handlers → run until shutdown.
//! Exit codes: 0 on cooperative shutdown, 1 on any failure.
//!
//! [mc_server]: https://docs.rs/

#![forbid(unsafe_code)]

use mc_core::error::ServerError;
use mc_server::config::ServerConfig;
use mc_server::lifecycle::Server;
use mc_server::logging::init_logging;

#[tokio::main]
async fn main() {
    // `run()` holds the whole `Server` (config, clock, game, network handle)
    // across an await, which trips `clippy::large-futures`. Boxing it keeps the
    // runtime's task stack bounded; the allocation happens once at startup.
    let code = Box::pin(run()).await;
    std::process::exit(code);
}

async fn run() -> i32 {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return 1;
        }
    };
    if let Err(e) = init_logging() {
        eprintln!("logging error: {e}");
        return 1;
    }
    let mut server = Server::new(config);
    server.install_signal_handlers();
    if let Err(e) = server.open_world() {
        tracing::error!(error = %e, "cannot open the world");
        return 1;
    }
    match server.start_network().await {
        Ok(addr) => tracing::info!(%addr, "listening"),
        Err(e) => {
            tracing::error!(error = %e, "cannot start network");
            return 1;
        }
    }
    match server.run().await {
        Err(ServerError::Shutdown) => {
            tracing::info!("shutdown complete");
            0
        }
        Err(e) => {
            tracing::error!(error = %e, "server failed");
            1
        }
        Ok(()) => 0,
    }
}

/// First CLI arg is an optional TOML config path; no arg means defaults.
/// Unknown extra args are a usage error, not silently ignored.
fn load_config() -> mc_core::error::ServerResult<ServerConfig> {
    let mut args = std::env::args().skip(1);
    match (args.next(), args.next()) {
        (None, _) => Ok(ServerConfig::default()),
        (Some(path), None) => ServerConfig::load_from_file(std::path::Path::new(&path)),
        (Some(_), Some(_)) => Err(ServerError::Operational(
            "usage: mc-server [config.toml]".to_owned(),
        )),
    }
}
