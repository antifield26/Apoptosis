//! Shared test harness: fixtures, temp dirs and byte-compare helpers (P01-10).
//!
//! `test-support` is a **dev-only** crate: it must never appear in a
//! production `[dependencies]` section (ADR-0001 D-01, ARCHITECTURE-CONTRACT).
//! Phase 01 ships the scaffolding (fixture layout + helpers); protocol and
//! persistence fixtures land with their phases (P02-11, P03).

#![forbid(unsafe_code)]

pub mod client;
pub mod fixtures;
