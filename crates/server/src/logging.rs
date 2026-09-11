//! Structured logging bootstrap (P01-07, AGENTS.md section 2).
//!
//! One global `tracing` subscriber, installed exactly once. Format is
//! human-readable by default; `RUST_LOG` / `MC_LOG` overrides the level
//! without recompiling. JSON output is intentionally deferred until an
//! operator asks for it (no speculative abstraction).

use mc_core::error::{ServerError, ServerResult};
use tracing_subscriber::{EnvFilter, fmt};

/// Environment variable selecting the log filter. Falls back to `RUST_LOG`,
/// then to a compiled-in default.
pub const LOG_ENV_VAR: &str = "MC_LOG";

/// Default filter when neither `MC_LOG` nor `RUST_LOG` is set.
pub const DEFAULT_FILTER: &str = "info,mc_server=debug,mc_core=debug";

/// Install the global tracing subscriber. Safe to call once; a second call
/// returns [`ServerError::Operational`] instead of panicking.
///
/// # Errors
///
/// Returns [`ServerError::Operational`] when a global subscriber is already
/// installed or the filter directive cannot be parsed.
pub fn init_logging() -> ServerResult<()> {
    let filter = EnvFilter::try_new(
        std::env::var(LOG_ENV_VAR)
            .or_else(|_| std::env::var("RUST_LOG"))
            .unwrap_or_else(|_| DEFAULT_FILTER.to_owned()),
    )
    .map_err(|e| ServerError::Operational(format!("invalid log filter: {e}")))?;

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init()
        .map_err(|e| ServerError::Operational(format!("logging already initialised: {e}")))?;
    Ok(())
}
