//! Shared primitives for the Minecraft server workspace.
//!
//! `core` owns identifiers, math helpers, timing/tick types and the error
//! taxonomy contract (AGENTS.md sections 7-9, ADR-0001 D-01). It must stay free
//! of networking, world state and gameplay semantics so every other crate can
//! depend on it without cycles.

#![forbid(unsafe_code)]

pub mod error;
pub mod ids;
pub mod tick;
