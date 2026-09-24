//! Command tree, argument model and dispatcher (P07-01, P07-02).
//!
//! ## What this crate owns, and why it is separate
//!
//! `AGENTS.md` §7 lists `command/ # command parsing/execution` as its own boundary, and
//! that is the right shape here: a command framework is a **parser** (text → typed
//! arguments), a **tree** (what names and argument lists are valid), a **permission
//! check**, and an **execution context** — none of which needs the world, the network
//! or the simulation. Keeping it separate is what makes the dispatcher testable against
//! a tree with no server running, which is where the hostile-input tests live.
//!
//! ## The one design decision worth explaining
//!
//! Vanilla's Brigadier has a *mutable* tree: `CommandDispatcher` owns a graph of
//! `CommandNode`s with parent pointers and a `Literals`/`Argument` split, and parsing is
//! a recursive walk that mutates a `ParseState` as it backtracks.
//!
//! This is a **flat, immutable description** instead: a [`CommandTree`] holds a
//! [`Command`] per root literal, and each command's [`Argument`]s are an ordered list
//! parsed left to right. Why:
//!
//! 1. **No backtracking is needed for the grammar we accept.** A command's arguments
//!    are positional and greedy-typed; there are no alternative branches at the same
//!    position. Vanilla *does* branch (a literal argument can have several children),
//!    which is exactly the complexity that makes Brigadier's parse state mutable — and
//!    implementing that machinery for a grammar with no branches would be scaffolding
//!    (AGENTS.md §3.4).
//! 2. **The tree is data, so it can be checked.** [`CommandTree::validate`] asserts every
//!    literal is unique, every command has a non-empty name, and no required argument
//!    follows an optional one — invariants a mutable graph can violate at runtime and a
//!    flat list cannot.
//! 3. **Brigadier compatibility is not a goal.** The client's command *suggestions* come
//!    from the `command_tree` packet, which needs Brigadier's node shape; that is a
//!    serialization concern for the packet layer, and the flat description can emit it
//!    without the parser having to be a graph.
//!
//! The cost is stated: **branching grammars are not supported.** `execute` with its
//! sub-commands is the obvious case (P07-07) and the module says so rather than
//! pretending otherwise.
//!
//! ## Hostile input (AGENTS.md §10)
//!
//! A command string is client-supplied and reaches a `Vec<char>` and several loops. So:
//! the input length is bounded, the argument count is bounded, recursion is avoided
//! entirely (parsing is a loop over a positional list), numeric parsing goes through
//! checked conversions, and no input can panic.

#![forbid(unsafe_code)]

pub mod argument;
pub mod dispatch;
pub mod execute;
pub mod selector;
pub mod source;
pub mod tree;

pub use argument::Coordinate;
pub use argument::{Argument, ArgumentKind, ArgumentValue, ParseError, ValueRange};
pub use dispatch::{CommandOutcome, Dispatcher, Suggestion};
pub use execute::{Anchor, ExecuteChain, ExecuteError, FacingTarget, Modifier, RotationSource};
pub use selector::{Selector, SelectorError, SelectorKind, sort_and_limit};
pub use source::{CommandSource, PermissionLevel, SourceKind};
pub use tree::{Command, CommandTree, TreeError};

/// Longest command string accepted, in characters.
///
/// Vanilla's client caps a chat command at 32 500 characters (`CommandMaxLength`) and
/// the server never sees more than that, but a hostile client can send any length up to
/// the packet limit. 32 500 is the client's own bound, so accepting it means a legitimate
/// paste is never refused while the parser still has a fixed ceiling.
pub const MAX_COMMAND_CHARS: usize = 32_500;

/// Largest number of arguments a single command may accept.
///
/// A grammar bound, not an input bound: exceeding it is a *tree* error (a command
/// declared with 300 arguments is a programming mistake), and it keeps the parsed
/// argument vector's size a property of the tree rather than of the input.
pub const MAX_ARGUMENTS: usize = 64;
