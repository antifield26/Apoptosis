//! Server orchestration: configuration, lifecycle, world storage and
//! startup/shutdown.
//!
//! `mc-server` owns the accept→tick→shutdown flow (ADR-0001 D-02) without
//! owning gameplay: it binds a Tokio runtime, steps the [`core`][mc_core]
//! [`TickClock`], opens the world described by the config, and unwinds
//! cooperatively on Ctrl-C / SIGTERM / `stop()` — flushing the world on the way
//! out ([`storage::WorldService`]).
//!
//! [mc_core]: https://docs.rs/

#![forbid(unsafe_code)]

pub mod commands;
pub mod config;
pub mod game;
pub mod lifecycle;
pub mod logging;
pub mod storage;
