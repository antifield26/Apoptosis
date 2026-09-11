//! Running `/function`: issuing a loaded data function's commands (P07-08).
//!
//! ## What was missing
//!
//! P07-08's loader half has existed since the `mc-data` work: `FunctionRegistry` returns a
//! function's command lines. What was missing is the *execution* half, and it needed the
//! dispatcher — which is why this lands after `/execute` rather than before it. A function is
//! just a list of command strings run in order, as the invoking source.
//!
//! ## The four hazards, and how each is handled
//!
//! **1. Recursion.** A function may call itself, directly or through a cycle. The loader can
//! *detect* a static cycle among literal `/function` lines, but a name built by a macro is
//! invisible to that — so the run-time depth bound is the real guarantee. [`MAX_DEPTH`] mirrors
//! `mc_command::execute`'s bound, and exceeding it stops the *run* with a message rather than
//! unwinding the stack.
//!
//! **2. Command count.** A function with a thousand lines is a thousand dispatches per `/function`,
//! and a chain of functions multiplies. [`MAX_COMMANDS`] bounds the total across one top-level
//! invocation, so a hostile or careless pack cannot make one command do unbounded work in a tick.
//!
//! **3. Macros.** `$(name)` needs the caller's arguments, which is a substitution language this
//! build does not implement. The loader flags such a file; here a function containing macro lines
//! is **refused with that reason** rather than run with the `$(…)` text passed through as a
//! literal — which would dispatch a command that does not exist and report a confusing error.
//!
//! **4. Output.** Vanilla's `/function` reports how many commands ran. Each command's own feedback
//! is sent as it happens, so a function that says something is visible; the final line reports the
//! count. Both, rather than one or the other.
//!
//! ## What is deliberately not done
//!
//! - **`#tag` function tags** are not run. Vanilla's `/function #namespace:tag` runs every function
//!   in a tag; that needs the tag set threaded to the dispatcher, and it is recorded as a gap.
//! - **`/schedule` and `function` permissions** are not modelled: a function's commands each pass
//!   the ordinary permission check *as the invoking source*, so a level-0 player cannot use a
//!   function to run an operator command. That is the same guarantee `/execute` has, and it is
//!   tested for the same reason.

use mc_core::error::ServerResult;
use mc_core::ids::ResourceId;
use mc_data::function::FunctionRegistry;

use std::path::Path;

use tracing::warn;

use crate::commands::CommandResult;
use crate::game::{Game, TickReport};

/// Deepest `/function` nesting accepted.
///
/// Deliberately far below `mc_data::function::SUGGESTED_MAX_RECURSION_DEPTH` (64): that constant is
/// the loader's suggestion for a *cycle detector*, and a run-time bound has to be shallow enough
/// that the stack is never in question while staying deeper than any hand-written function chain.
pub const MAX_DEPTH: u32 = 16;

/// Most commands one top-level `/function` may run, across every nested call.
///
/// A function is a few lines in practice. 10 000 bounds the work one command can cause inside a
/// tick while leaving room for a genuinely large generated function.
pub const MAX_COMMANDS: usize = 10_000;

/// What a function run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRun {
    /// How many commands were dispatched.
    pub commands_run: usize,
    /// The line that stopped the run, if one did.
    pub stopped: Option<String>,
    /// How many functions were entered.
    pub depth_reached: u32,
}

/// Load every function under a pack's `function/` directory.
///
/// A separate entry point from the server's construction because a data pack is optional: a world
/// with no pack runs no functions, and that is not a misconfiguration.
///
/// # Errors
///
/// [`mc_core::error::ServerError`] when the directory exists and cannot be read.
/// The subdirectory a pack keeps functions in.
pub const FUNCTION_DIRECTORY: &str = "function";

/// Derive a function's name from its path, per the format's rule.
///
/// `data/<namespace>/function/foo/bar.mcfunction` is `<namespace>:foo/bar`: the path relative to
/// the function directory, minus the extension, under the **pack's own namespace**.
///
/// The namespace is a parameter rather than the vanilla constant, and that is a correction: the
/// first version hard-coded `minecraft:`, so a world pack's `data/testns/function/greet.mcfunction`
/// was named `minecraft:greet` and `/function testns:greet` reported an unknown function — with the
/// file present, loaded and counted. A test found it.
///
/// Returns `None` when the path is not under the function directory, which means the caller
/// discovered it wrongly rather than that the file is bad.
#[must_use]
pub fn function_name(function_root: &Path, path: &Path, namespace: &str) -> Option<ResourceId> {
    let relative = path.strip_prefix(function_root).ok()?;
    // On Windows a path uses `\`, and the format uses `/`, so the separator is normalised rather
    // than left to the platform — a name with a backslash in it would never match a `/function`
    // argument.
    let without_extension = relative.with_extension("");
    let text = without_extension.to_string_lossy().replace('\\', "/");
    ResourceId::parse(&format!("{namespace}:{text}")).ok()
}

/// Load every function from one namespace's function directory.
///
/// `function_root` is the directory holding `.mcfunction` files —
/// `data/<namespace>/function` in a pack — because that is what the naming rule is relative to, and
/// `namespace` is the `<namespace>` segment, which becomes the name's prefix.
///
/// # Errors
///
/// Never for a malformed file: each is skipped with a warning and counted, so one bad function
/// does not remove every other one (AGENTS.md §9). The `Result` is kept for the caller's symmetry
/// with the other loaders and for a future variant that can fail to read a directory.
pub fn load_functions(function_root: &Path, namespace: &str) -> ServerResult<FunctionRegistry> {
    // The loader discovers by **extension**, not by directory walk: `.mcfunction` is the format's
    // marker, and `files_with_extension` is where that rule lives. Following it exactly rather than
    // adapting another loader's shape is the point — the extension is what decides whether a file
    // is a function at all.
    let mut registry = FunctionRegistry::new();
    for path in mc_data::function::files_with_extension(function_root, "mcfunction") {
        let Some(name) = function_name(function_root, &path, namespace) else {
            warn!(path = %path.display(), "a discovered function is not under the function root");
            continue;
        };
        match mc_data::function::read_function_file(
            &path,
            name,
            mc_data::function::FunctionLimits::default(),
        ) {
            Ok(file) => registry.insert(file),
            Err(error) => {
                warn!(path = %path.display(), %error, "a function file was skipped");
            }
        }
    }
    Ok(registry)
}

impl Game {
    /// The functions this game has loaded.
    #[must_use]
    pub const fn functions(&self) -> &FunctionRegistry {
        &self.functions
    }

    /// Replace this game's function registry.
    ///
    /// What the pack loader calls: it loads every pack's functions in load order and installs the
    /// merged result, so a world pack's function of the same name overrides the vanilla one it was
    /// loaded after.
    pub fn set_functions(&mut self, functions: FunctionRegistry) {
        self.functions = functions;
    }

    /// Load functions from one namespace's function directory into this game.
    ///
    /// Replaces the whole registry rather than merging: this is the "load a pack" entry point, and
    /// a caller wanting several packs merged should use [`crate::packs::load_packs`], which applies
    /// the load order.
    ///
    /// `namespace` is a parameter rather than defaulting to `minecraft`, because defaulting would
    /// reintroduce the bug the pack loader just had — every function named `minecraft:<path>` — in
    /// the method a caller is most likely to reach for.
    ///
    /// # Errors
    ///
    /// As for [`load_functions`].
    pub fn load_functions_from(
        &mut self,
        function_root: &Path,
        namespace: &str,
    ) -> ServerResult<usize> {
        self.functions = load_functions(function_root, namespace)?;
        Ok(self.functions.len())
    }

    /// Run a function by name as the given connection.
    ///
    /// # Errors
    ///
    /// As for [`Game::dispatch_command`](crate::commands::Game::dispatch_command).
    pub fn run_function(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        name: &ResourceId,
        report: &mut TickReport,
    ) -> ServerResult<FunctionRun> {
        let mut run = FunctionRun {
            commands_run: 0,
            stopped: None,
            depth_reached: 0,
        };
        self.run_function_inner(id, name, report, 0, &mut run)?;
        Ok(run)
    }

    /// The recursive worker.
    fn run_function_inner(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        name: &ResourceId,
        report: &mut TickReport,
        depth: u32,
        run: &mut FunctionRun,
    ) -> ServerResult<()> {
        if depth >= MAX_DEPTH {
            run.stopped = Some(format!(
                "Function recursion is limited to {MAX_DEPTH} deep; {name} was not run."
            ));
            return Ok(());
        }
        run.depth_reached = run.depth_reached.max(depth + 1);

        let Some(function) = self.functions.by_name(name).cloned() else {
            // Unknown is a *command* error, not a silent no-op: a typo in a function name would
            // otherwise look like a function that does nothing.
            run.stopped = Some(format!("Unknown function {name}."));
            return Ok(());
        };

        if function.has_macros {
            run.stopped = Some(format!(
                "Function {name} uses $(…) macros, which this build does not expand."
            ));
            return Ok(());
        }

        // The command list is cloned because the loop needs `&mut self` to dispatch. A function is
        // a handful of lines, so the clone is cheaper than the borrow gymnastics an iterator would
        // need.
        let commands: Vec<String> = function.command_list().to_vec();
        for line in commands {
            if run.commands_run >= MAX_COMMANDS {
                run.stopped = Some(format!(
                    "Stopped after {MAX_COMMANDS} commands; the function chain is too large."
                ));
                return Ok(());
            }
            let tokens: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
            let Some(root) = tokens.first() else {
                continue;
            };
            if root == "function" {
                // Recurse. The name is the next token; a `#tag` is not supported, and saying so is
                // better than reporting an unknown function.
                let Some(target) = tokens.get(1) else {
                    run.stopped = Some("`function` needs a name.".to_owned());
                    return Ok(());
                };
                if target.starts_with('#') {
                    run.stopped = Some(format!(
                        "Function tags ({target}) are not supported by this build."
                    ));
                    return Ok(());
                }
                let Ok(target_id) = ResourceId::parse(target) else {
                    run.stopped = Some(format!("{target:?} is not a function name."));
                    return Ok(());
                };
                self.run_function_inner(id, &target_id, report, depth + 1, run)?;
                if run.stopped.is_some() {
                    return Ok(());
                }
                continue;
            }

            run.commands_run += 1;
            // Through the ordinary dispatcher, as the invoking source: a function's commands each
            // pass their own permission check, so a level-0 player cannot use one to reach an
            // operator command.
            if root == "execute" {
                self.dispatch_execute(id, &tokens[1..], report, 0)?;
            } else {
                self.dispatch_command(id, &line, report)?;
            }
        }
        Ok(())
    }

    /// `/function <name>`
    ///
    /// # Errors
    ///
    /// As for [`Game::dispatch_command`](crate::commands::Game::dispatch_command).
    pub(crate) fn command_function(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let Some(text) = parsed.string(0) else {
            return Ok(CommandResult::message("Usage: /function <name>"));
        };
        if text.starts_with('#') {
            return Ok(CommandResult::message(format!(
                "Function tags ({text}) are not supported by this build."
            )));
        }
        let Ok(name) = ResourceId::parse(text) else {
            return Ok(CommandResult::message(format!(
                "{text:?} is not a function name."
            )));
        };
        let run = self.run_function(id, &name, report)?;
        // `write!` rather than `push_str(&format!(…))`: one allocation instead of two, and clippy
        // is right that the nested format is noise.
        let mut message = format!("Ran {} command(s) in {name}", run.commands_run);
        if let Some(stopped) = run.stopped {
            use std::fmt::Write as _;
            let _ = write!(message, "\n{stopped}");
        }
        Ok(CommandResult::message(message))
    }
}
