//! TCP/Tokio connection lifecycle for the Minecraft 26.1.2 protocol (P02-01).
//!
//! Boundaries (ADR-0001 D-01/D-03, ARCHITECTURE-CONTRACT):
//! - this crate owns sockets, per-connection state and the login/config/play
//!   handshake; it does **not** own world state or gameplay semantics;
//! - gameplay packets are decoded into typed intents
//!   ([`mc_protocol::packets::play::PlayIntent`]); Phase 04 attaches the
//!   simulation handler, so the decode path is already complete;
//! - every parse error closes one connection only; malformed input never
//!   panics or aborts the process (AGENTS.md section 9).
//!
//! Threading: each connection is one Tokio task, tracked in a `JoinSet` by the
//! accept loop. Both the loop and every connection observe the same cooperative
//! [`listener::NetworkShutdown`], so Ctrl-C and systemd stops drain live
//! connections before the server closes the world.

#![forbid(unsafe_code)]
// Network code converts bounded protocol values to host types constantly;
// every site validates bounds first. See `mc-protocol` for the same rationale.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

pub mod auth;
pub mod bridge;
pub mod connection;
pub mod limits;
pub mod listener;
pub mod registry_data;

pub use listener::{NetworkService, NetworkSettings};
