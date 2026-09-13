//! The server's command set (P07-05).
//!
//! Seven commands, chosen because each exercises a different part of the framework and
//! each is *usefully* implementable today:
//!
//! | Command | Argument shape it exercises | What it does |
//! |---|---|---|
//! | `help` | none | lists what the source may use |
//! | `list` | none | names the online players |
//! | `say` | greedy string | broadcasts a message |
//! | `time` | optional ranged integer | queries or sets the world time |
//! | `tp` | word + block position | moves the source (or reports where it would) |
//! | `op` | word, operator-only | reports that permission grants are not persisted |
//! | `stop` | none, console-only | asks the server to shut down |
//!
//! ## What each command deliberately does *not* do
//!
//! Vanilla's versions are larger, and the difference is stated rather than glossed:
//!
//! - **`help`** takes no page argument and does not paginate. Vanilla's does both.
//! - **`list`** does not report the maximum player count in Vanilla's exact format.
//! - **`say`** broadcasts to players only. The server console sees it in the log.
//! - **`time`** sets `timeOfDay` but not `dayTime`, so it does not advance the day
//!   counter the way Vanilla's `time set` does. `time set day` and the named presets
//!   (`day`/`noon`/`night`/`midnight`) are not accepted — only an integer.
//! - **`tp`** moves the *invoking* player, not a named target, because the server has no
//!   cross-player teleport authority model yet; the `target` argument is validated and
//!   must name the source itself. That is a real limitation, not a stub.
//! - **`op`** cannot persist a grant: `ops.json` is read at startup but never
//!   written (P07-04's remaining half), so it reports that and changes nothing.
//! - **`stop`** sets the shutdown flag; it does not save first, because the lifecycle's
//!   shutdown path already saves after the network drains (ADR-0001 D-04).
//!
//! Every one of these is a line in the parity matrix (the Phase 07 report is in git history, tag `phase-09-final`).

use mc_command::source::{CommandSource, SourceKind};
use mc_core::error::ServerResult;
use mc_protocol::packets::play::{SetTime, SystemChat};
use mc_protocol::text::TextComponent;
use tracing::{debug, info, warn};

use crate::game::{Game, TickReport};

/// The result of running a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// The command ran; the message goes to the source (or to everyone, for `say`).
    Ok {
        /// What to tell the source, if anything.
        feedback: Option<String>,
    },
    /// The command ran and wants the server to stop.
    Stop,
}

impl CommandResult {
    /// A command that reports something.
    #[must_use]
    pub fn message(text: impl Into<String>) -> Self {
        Self::Ok {
            feedback: Some(text.into()),
        }
    }

    /// A command that reports nothing.
    #[must_use]
    pub const fn silent() -> Self {
        Self::Ok { feedback: None }
    }
}

impl Game {
    /// Build the command tree this server supports.
    ///
    /// A free method rather than a `const`, because `CommandTree::insert` is fallible and
    /// a failure here should be impossible — the tests assert the tree builds.
    ///
    /// # Panics
    ///
    /// Never in practice: every command below is valid by construction, and
    /// `command_tree_is_valid` asserts it. Building it at startup rather than lazily
    /// means a malformed tree would stop the server instead of a player's command.
    #[must_use]
    pub fn build_command_tree() -> mc_command::CommandTree {
        use mc_command::{Argument, ArgumentKind, Command, PermissionLevel, ValueRange};

        let mut tree = mc_command::CommandTree::new();
        // The `expect` is a startup invariant with a test behind it, which is the one
        // place this project allows one outside a test: a server that cannot describe
        // its own commands is not fit to run.
        let mut add = |command: Command| {
            tree.insert(command)
                .expect("the built-in command tree is valid; see command_tree_is_valid");
        };

        add(Command::new("help", "List the commands you can use"));
        add(Command::new("list", "List the players online"));
        add(Command::new("say", "Broadcast a message").with_argument(Argument::greedy("message")));
        add(
            Command::new("time", "Query or set the world time").with_argument(Argument::optional(
                "value",
                ArgumentKind::Integer(ValueRange::new(0, 24_000)),
            )),
        );
        add(Command::new("tp", "Teleport to coordinates")
            .with_argument(Argument::word("target"))
            .with_argument(Argument::required("pos", ArgumentKind::BlockPos)));
        // The whole chain is one greedy argument: `execute` is parsed by its own regular
        // grammar (see `mc_command::execute`), not by the tree, because its modifiers may
        // appear in any order and any number of times.
        add(
            Command::new("execute", "Run a command in a modified context")
                .with_argument(Argument::greedy("command")),
        );
        // A function runs commands as the invoker, so it needs no permission of its own: each
        // command inside it passes its own check. Vanilla agrees, and it is what stops a function
        // from being a privilege-escalation route.
        add(Command::new("function", "Run a data function").with_argument(Argument::word("name")));
        add(Command::new("op", "Grant operator status").requiring(PermissionLevel::Operator));
        add(Command::new("stop", "Stop the server").requiring(PermissionLevel::Console));
        tree
    }

    /// Dispatch a command string from a connection.
    ///
    /// This is the whole of P07-05's integration: parse, then run. The permission check
    /// happened during parsing, so by the time a handler runs the source is known to be
    /// allowed.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`](mc_core::error::ServerError::Invariant) when a reply
    /// cannot be encoded, which is a server-side bug rather than client input. A command
    /// that does not parse, is not permitted or does not fit its grammar is **not** an
    /// error: it is answered with a message, because that is normal player input.
    pub fn dispatch_command(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        command: &str,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let Some(source) = self.command_source(id) else {
            return Ok(());
        };
        let dispatcher = mc_command::Dispatcher::new(Self::build_command_tree());
        let outcome = dispatcher.parse(command, &source);

        let feedback = match outcome {
            mc_command::CommandOutcome::Parsed(parsed) => {
                match self.run_command(id, &parsed, report)? {
                    CommandResult::Ok { feedback } => feedback,
                    CommandResult::Stop => {
                        info!(id = %id, "stop requested via command");
                        self.request_shutdown();
                        Some("Stopping the server…".to_owned())
                    }
                }
            }
            mc_command::CommandOutcome::UnknownCommand { name, suggestions } => {
                if suggestions.is_empty() {
                    Some(format!("Unknown command {name:?}. Try /help."))
                } else {
                    Some(format!(
                        "Unknown command {name:?}. Did you mean {}?",
                        suggestions.join(", ")
                    ))
                }
            }
            mc_command::CommandOutcome::PermissionDenied { name, required, .. } => {
                // The level is logged, not sent: a player does not need to know the
                // numeric ladder to be told no.
                debug!(id = %id, command = name, %required, "command denied by permission");
                Some(format!("You do not have permission to use /{name}."))
            }
            mc_command::CommandOutcome::BadArguments(error) => Some(error.to_string()),
        };

        if let Some(text) = feedback {
            self.send(
                id,
                &SystemChat {
                    content: TextComponent::literal(text),
                    overlay: false,
                },
                report,
            )?;
        }
        Ok(())
    }

    /// The command source for a connection, or `None` when it has no session.
    ///
    /// The level comes from the session, which the join path set from
    /// `ops.json`: listed uuids hold their file level, everyone else is level
    /// 0 (`All`). A stale comment here once claimed no storage was read; the
    /// `ops_e2e` suite proves otherwise (a level-4 operator stops the server).
    #[must_use]
    pub fn command_source(&self, id: mc_network::bridge::ConnectionId) -> Option<CommandSource> {
        let session = self.sessions.get(&id)?;
        let position = session.player.position;
        Some(
            CommandSource::player(
                session.player.profile.name.clone(),
                mc_command::source::SourcePosition::new(position.x, position.y, position.z),
            )
            .with_permission(self.player_permission(id)),
        )
    }

    /// The permission level a connection holds.
    ///
    /// Level 0 unless the join path granted more from `ops.json` (P07-04,
    /// proven by `ops_e2e`). A command that needs more than the source holds
    /// is answered with a refusal, never run (P08-08).
    #[must_use]
    pub fn player_permission(
        &self,
        id: mc_network::bridge::ConnectionId,
    ) -> mc_command::PermissionLevel {
        self.sessions
            .get(&id)
            .map_or(mc_command::PermissionLevel::All, |session| {
                session.permission
            })
    }

    /// Run a parsed command.
    ///
    /// `pub(crate)` so `/execute` runs its inner command through this same match rather than
    /// duplicating it: two dispatch paths would be two places for a new command to be forgotten.
    pub(crate) fn run_command(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        match parsed.name {
            "help" => Ok(Self::command_help(&parsed.source)),
            "list" => Ok(self.command_list()),
            "say" => Ok(self.command_say(parsed, report)?),
            "time" => Ok(self.command_time(parsed, report)?),
            "tp" => Ok(self.command_tp(id, parsed)),
            "execute" => {
                // The chain parser owns the grammar; the token list is the raw argument text so
                // `run` can hand the inner command its original words.
                let tokens: Vec<String> = parsed
                    .raw_arguments
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                self.dispatch_execute(id, &tokens, report, 0)?;
                Ok(CommandResult::silent())
            }
            "function" => self.command_function(id, parsed, report),
            "op" => Ok(Self::command_op(parsed)),
            "stop" => Ok(CommandResult::Stop),
            // Unreachable: the tree only contains the names above, and `parse` resolved
            // this one through it. Returning a refusal rather than panicking keeps a
            // future tree/`match` divergence from taking the server down.
            other => {
                warn!(command = other, "a parsed command has no handler");
                Ok(CommandResult::message(format!(
                    "/{other} is declared but not implemented."
                )))
            }
        }
    }

    /// `/help`
    ///
    /// Associated rather than a method: nothing about the server's state affects the
    /// answer, only the source's permissions.
    fn command_help(source: &CommandSource) -> CommandResult {
        let tree = Self::build_command_tree();
        let mut available: Vec<String> = tree
            .commands()
            .iter()
            .filter(|command| source.may_use(command.permission))
            .map(|command| format!("{} — {}", command.usage(), command.description))
            .collect();
        available.sort();
        let count = available.len();
        // No pagination: Vanilla's `/help` takes a page number, and this build does not
        // have enough commands for one to matter. Stated in the module docs.
        CommandResult::message(format!(
            "{count} commands available:\n{}",
            available.join("\n")
        ))
    }

    /// `/list`
    fn command_list(&self) -> CommandResult {
        let names = self.player_names();
        CommandResult::message(format!(
            "There are {} of a maximum of {} players online: {}",
            names.len(),
            self.max_players(),
            names.join(", ")
        ))
    }

    /// `/say <message>`
    fn command_say(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let Some(message) = parsed.string(0) else {
            // Unreachable: `say` declares a required greedy argument, so parsing refuses
            // an invocation without one.
            return Ok(CommandResult::message("Usage: /say <message>"));
        };
        let line = format!("[{}] {message}", parsed.source.name);
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            self.send(
                target,
                &SystemChat {
                    content: TextComponent::literal(line.clone()),
                    overlay: false,
                },
                report,
            )?;
        }
        // The console sees it through the log rather than a packet.
        info!(from = %parsed.source.name, %message, "say");
        Ok(CommandResult::silent())
    }

    /// `/time [value]`
    fn command_time(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let Some(value) = parsed.integer(0) else {
            // Query: there is no per-world time field yet, so the answer comes from the
            // tick counter the world-time broadcast already uses.
            let time = (self.tick_count() % 24_000).cast_signed();
            return Ok(CommandResult::message(format!(
                "The time is {time} (daytime)"
            )));
        };
        // Setting overwrites `time_of_day` for this tick and every later one, since the
        // broadcast recomputes it from the tick counter each second. So the offset is
        // recorded rather than the value, which is what makes `/time` stick.
        // The day-time arithmetic is signed because the offset can be negative; the
        // tick counter is not, and the modulo keeps the value small either way.
        self.set_time_offset(value - (self.tick_count() % 24_000).cast_signed());
        let time = (self.tick_count() % 24_000).cast_signed() + self.time_offset();
        // The offset is still recorded, because it is what the query above and any future clock sync read.
        // It can no longer reach the client through `set_time`: 26.1.2 removed `time_of_day` from that packet
        // (P10-03, KD-43). The offset is applied to `time` for the reply below.
        let packet = SetTime {
            world_age: self.tick_count().cast_signed(),
            flag: 0,
        };
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            self.send(target, &packet, report)?;
        }
        Ok(CommandResult::message(format!(
            "Set the time to {}",
            time.rem_euclid(24_000)
        )))
    }

    /// `/tp <target> <pos>`
    ///
    /// The target must name the invoking player: there is no cross-player teleport
    /// authority model yet, so teleporting someone else is refused with that reason
    /// rather than silently doing nothing.
    fn command_tp(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(target) = parsed.string(0) else {
            return CommandResult::message("Usage: /tp <target> <pos>");
        };
        if !target.eq_ignore_ascii_case(&parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot teleport {target:?}: this build only teleports the invoking \
                 player ({})",
                parsed.source.name
            ));
        }
        let Some(mc_command::ArgumentValue::BlockPos { x, y, z }) = parsed.argument(1) else {
            return CommandResult::message("Usage: /tp <target> <pos>");
        };
        let base = parsed.source.block_position().unwrap_or((0, 0, 0));
        // `~` takes the source's own coordinate; a bare number is absolute.
        let resolve = |offset: Option<i32>, base: i32| offset.unwrap_or(base);
        let (tx, ty, tz) = (
            resolve(*x, base.0),
            resolve(*y, base.1),
            resolve(*z, base.2),
        );
        match self.teleport_source(&parsed.source, tx, ty, tz) {
            Ok(()) => CommandResult::message(format!("Teleported to {tx} {ty} {tz}")),
            Err(reason) => CommandResult::message(reason),
        }
    }

    /// `/op`
    ///
    /// Associated rather than a method: it changes nothing and reads nothing, which is
    /// precisely the point it reports.
    fn command_op(parsed: &mc_command::dispatch::ParsedCommand) -> CommandResult {
        // Vanilla's `/op` takes a player and writes `ops.json`. This build *reads*
        // the file at startup (P07-04) but never writes it, so granting here would
        // not survive a restart — and writing an operator file is an authority
        // decision this phase deliberately does not make silently. Saying exactly
        // that is better than a fake success (P08-08 review).
        CommandResult::message(format!(
            "/op is not implemented: permission grants are not persisted (ops.json \
              is read at startup, never written), so {} is not changed.",
            parsed.source.name
        ))
    }
}

/// Whether a source kind may run commands at all.
///
/// A command block is a source kind the enum carries but nothing constructs, so this
/// exists to make the intent explicit rather than leaving `SourceKind` unread.
#[must_use]
pub const fn may_run_commands(kind: SourceKind) -> bool {
    match kind {
        SourceKind::Player | SourceKind::Console => true,
        // Recorded as false rather than true: a command block needs the block-entity
        // machinery that P06-07 does not yet run on a schedule.
        SourceKind::CommandBlock => false,
    }
}
