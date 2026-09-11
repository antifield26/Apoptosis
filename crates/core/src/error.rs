//! Error taxonomy and propagation policy (P01-05, AGENTS.md section 9).
//!
//! Rules enforced here:
//! - `panic!` is never routine control flow; every fallible boundary returns
//!   [`ServerError`].
//! - The five failure classes below are distinct variants so callers handle
//!   them differently (kick player vs. retry op vs. abort load vs. bug report).
//! - A malformed client packet must map to [`ServerError::Protocol`] or
//!   [`ServerError::InvalidAction`], never to a process abort.

use thiserror::Error;

/// The five failure classes of AGENTS.md section 9, plus a shutdown signal.
///
/// The shutdown variant exists so the server lifecycle (P01-08) can unwind
/// blocking waits without inventing a sentinel string.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServerError {
    /// Bytes on the wire that cannot be decoded or violate framing limits.
    /// Response: drop/kick the connection, keep the process alive.
    #[error("malformed protocol input: {0}")]
    Protocol(String),

    /// A well-formed request the player is not allowed to make
    /// (bad coordinates, spoofed inventory, permission bypass...).
    /// Response: reject the action, keep the connection.
    #[error("invalid player action: {0}")]
    InvalidAction(String),

    /// A recoverable operational failure (bind conflict, transient I/O...).
    /// Response: retry or degrade; surfaced to operators, not players.
    #[error("operational failure: {0}")]
    Operational(String),

    /// Persistent data that fails validation or checksums.
    /// Response: refuse to load the affected unit, never silently continue.
    #[error("persistent data corruption: {0}")]
    CorruptData(String),

    /// A programmer invariant was violated. This is the only variant that may
    /// justify aborting a task; it must still unwind to a controlled shutdown,
    /// never an unhandled panic across a thread boundary.
    #[error("internal invariant violated: {0}")]
    Invariant(String),

    /// Cooperative shutdown was requested (Ctrl-C / SIGTERM / operator stop).
    #[error("shutdown requested")]
    Shutdown,
}

/// Fallible result alias used across all workspace crates.
pub type ServerResult<T> = Result<T, ServerError>;
