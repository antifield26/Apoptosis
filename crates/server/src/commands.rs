//! The server's command set (P07-05, P14-01).
//!
//! Twelve commands, chosen because each exercises a different part of the framework and
//! each is *usefully* implementable today:
//!
//! | Command | Argument shape it exercises | What it does |
//! |---|---|---|
//! | `help` | none | lists what the source may use |
//! | `list` | none | names the online players |
//! | `say` | greedy string | broadcasts a message |
//! | `time` | optional ranged integer | queries or sets the world time |
//! | `tp` | word + block position | moves the source (or reports where it would) |
//! | `op` | player name, administrator-only | grants operator status at level 4 and persists `ops.json` |
//! | `deop` | player name, administrator-only | revokes operator status and persists `ops.json` |
//! | `stop` | none, console-only | asks the server to shut down |
//! | `gamemode` | word + optional player name, operator-only | sets the invoking player's game mode |
//! | `give` | player name + resource + optional ranged integer, operator-only | gives items, dropping overflow at the player's feet |
//! | `kill` | optional player name, operator-only | kills the invoking player through the damage path, even in creative |
//! | `seed` | none, operator-only | reports the world seed |
//! | `difficulty` | optional word, operator-only | queries or sets the world difficulty |
//!
//! ## What each command deliberately does *not* do
//!
//! Vanilla's versions are larger, and the difference is stated rather than glossed:
//!
//! - **`help`** takes no page argument and does not paginate. Vanilla's does both.
//! - **`list`** does not report the maximum player count in Vanilla's exact format.
//! - **`say`** broadcasts to players only. The server console sees it in the log.
//! - **`time`** sets the overworld clock but not the day counter, so it does
//!   not advance the date the way Vanilla's `time set` does; `add` is not
//!   modelled. `set` takes an integer or a preset
//!   (`day`/`noon`/`night`/`midnight`); a bare integer still sets for
//!   back-compat. Anything else prints usage — it never answers the time,
//!   because a silent query once hid failed sets (P14-09 walk).
//! - **`tp`** moves the *invoking* player, not a named target, because the server has no
//!   cross-player teleport authority model yet; the `target` argument is validated and
//!   must name the source itself. That is a real limitation, not a stub.
//! - **`op`** grants level 4 (Vanilla's default `op-permission-level`) to an
//!   *online* player and writes `ops.json` beside the world, taking effect
//!   immediately and surviving restarts; only online players can be named
//!   (uuids come from sessions, never from name matching). Both `/op` and
//!   `/deop` require level 3 like Vanilla — `/op` used to sit at level 2,
//!   which let a level-2 holder mint level-4 operators.
//! - **`deop`** removes the entry (and demotes a live session to 0);
//!   revoking someone who was never listed changes nothing and says so.
//!   A failed file write rolls the in-memory change back and says that
//!   instead of claiming success.
//! - **`stop`** sets the shutdown flag; it does not save first, because the lifecycle's
//!   shutdown path already saves after the network drains (ADR-0001 D-04).
//! - **`gamemode`** takes the invoking player only (same authority reason as `tp`);
//!   it sets the mode field and the menu's creative flag the way join and respawn do,
//!   but broadcasts no player-info or abilities update, so other clients learn of it
//!   late. Mode words are Vanilla's full names plus the `s`/`c`/`a`/`sp` shortcuts.
//! - **`give`** takes the invoking player only; the count caps at one stack (64) and
//!   the remainder of a full inventory drops at the player's feet, which is Vanilla's
//!   rule. Item names are registry ids (`minecraft:stone`, bare `stone` works).
//! - **`kill`** takes the invoking player only; it bypasses creative invulnerability
//!   the way Vanilla's kill does, and drops run through the normal death path.
//! - **`seed`** reports the simulation seed, which is also the worldgen seed.
//! - **`difficulty`** sets the world's difficulty (persisted to `level.dat` when
//!   storage is present) and gates hostile monster spawns on peaceful; mob damage
//!   numbers stay the hardcoded Normal values, and a locked `level.dat` refuses
//!   the set. Words are Vanilla's four full names, case-sensitive like Vanilla.
//!
//! Every one of these is a line in the parity matrix (the Phase 07 report is in git history, tag `phase-09-final`).

use mc_command::source::{CommandSource, SourceKind};
use mc_core::error::ServerResult;
use mc_protocol::packets::play::SetTime;
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
        add(Command::new("time", "Query or set the world time")
            .with_argument(Argument::optional("action", ArgumentKind::Word))
            .with_argument(Argument::optional("value", ArgumentKind::Word)));
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
        add(Command::new("op", "Grant operator status")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("deop", "Revoke operator status")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("stop", "Stop the server").requiring(PermissionLevel::Console));
        // P14-01: the admin set, all operator-only like Vanilla's level 2.
        add(Command::new("gamemode", "Set your game mode")
            .with_argument(Argument::word("mode"))
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Operator));
        add(Command::new("give", "Give yourself items")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .with_argument(Argument::required("item", ArgumentKind::Resource))
            .with_argument(Argument::optional(
                "count",
                ArgumentKind::Integer(ValueRange::new(1, 64)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("kill", "Kill yourself")
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Operator));
        add(Command::new("effect", "Give or clear a status effect")
            .with_argument(Argument::word("action"))
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .with_argument(Argument::optional("effect", ArgumentKind::Word))
            .with_argument(Argument::optional(
                "seconds",
                ArgumentKind::Integer(ValueRange::new(1, 1_000_000)),
            ))
            .with_argument(Argument::optional(
                "amplifier",
                ArgumentKind::Integer(ValueRange::new(0, 255)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("seed", "Show the world seed").requiring(PermissionLevel::Operator));
        add(Command::new("difficulty", "Query or set the difficulty")
            .with_argument(Argument::optional("difficulty", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
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
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(text),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
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
            "op" => Ok(self.command_op(id, parsed)),
            "deop" => Ok(self.command_deop(id, parsed)),
            "gamemode" => Ok(self.command_gamemode(id, parsed)),
            "give" => Ok(self.command_give(id, parsed)),
            "effect" => Ok(self.command_effect(id, parsed, report)),
            "kill" => Ok(self.command_kill(id, parsed)),
            "seed" => Ok(self.command_seed()),
            "difficulty" => Ok(self.command_difficulty(parsed)),
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
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(line.clone()),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                report,
            )?;
        }
        // The console sees it through the log rather than a packet.
        info!(from = %parsed.source.name, %message, "say");
        Ok(CommandResult::silent())
    }

    /// `/time [query]`, `/time <ticks>`, `/time set <ticks|day|noon|night|midnight>`
    ///
    /// Vanilla's `add` is not modelled (declared): advancing the clock by a
    /// delta is a second verb on the same offset, and this handler covers set
    /// plus query — the two the walk asked for.
    fn command_time(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let action = parsed.string(0);
        let value = parsed.string(1);
        let query = || {
            let time = (self.tick_count() % 24_000).cast_signed() + self.time_offset();
            Ok(CommandResult::message(format!(
                "The time is {} (daytime)",
                time.rem_euclid(24_000)
            )))
        };
        // Bare `/time` and `/time query` read the clock. Anything else must
        // resolve to a set or say usage: the old fallback answered every typo
        // with the time, which made a failed set indistinguishable from a
        // successful one on the live client (P14-09 walk).
        let target: Option<i64> = match (action, value) {
            (None | Some("query"), None) => return query(),
            (Some("set"), Some(word)) => Self::time_value(word),
            (Some(word), None) => word.parse::<i64>().ok().or_else(|| Self::time_preset(word)),
            _ => {
                return Ok(CommandResult::message(
                    "Usage: /time [query | <ticks> | set <ticks|day|noon|night|midnight>]"
                        .to_owned(),
                ));
            }
        };
        let Some(target) = target else {
            return Ok(CommandResult::message(
                "Usage: /time [query | <ticks> | set <ticks|day|noon|night|midnight>]".to_owned(),
            ));
        };
        // Setting records an offset from the tick counter, so the value sticks
        // while the clock keeps advancing — and the broadcast below carries it
        // to every client in the overworld clock entry (P14-09 walk: the old
        // shape decoded as an empty clock map, so `/time` moved the server's
        // mobs but never the client's sky).
        self.set_time_offset(target - (self.tick_count() % 24_000).cast_signed());
        info!(
            from = %parsed.source.name,
            target,
            offset = self.time_offset(),
            "time offset set"
        );
        let packet = SetTime {
            world_age: self.tick_count().cast_signed(),
            clocks: vec![self.overworld_clock_entry(self.tick_count())],
        };
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            self.send(target, &packet, report)?;
        }
        Ok(CommandResult::message(format!(
            "Set the time to {}",
            (self.tick_count().cast_signed() + self.time_offset()).rem_euclid(24_000)
        )))
    }

    /// A `/time` value word: an integer, or one of Vanilla's presets
    /// (`day` 1000, `noon` 6000, `night` 13000, `midnight` 18000).
    fn time_value(word: &str) -> Option<i64> {
        if let Ok(value) = word.parse::<i64>() {
            return Some(value.rem_euclid(24_000));
        }
        Self::time_preset(word)
    }

    /// Vanilla's named times, or `None`.
    fn time_preset(word: &str) -> Option<i64> {
        match word {
            "day" => Some(1_000),
            "noon" => Some(6_000),
            "night" => Some(13_000),
            "midnight" => Some(18_000),
            _ => None,
        }
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
        // `~` takes the source's own coordinate; a bare number is absolute. The two are carried separately by
        // `Coordinate` — an `Option<i32>` could not tell them apart, which is why `~1` used to land at the
        // absolute `1`.
        let (tx, ty, tz) = (x.resolve(base.0), y.resolve(base.1), z.resolve(base.2));
        match self.teleport_source(&parsed.source, tx, ty, tz) {
            Ok(()) => CommandResult::message(format!("Teleported to {tx} {ty} {tz}")),
            Err(reason) => CommandResult::message(reason),
        }
    }

    /// `/op <player>` and `/deop <player>` (P14-02).
    ///
    /// Both require level 3, matching Vanilla (`/op` used to sit at level 2
    /// here, which let a level-2 holder mint level-4 operators — a privilege
    /// escalation Vanilla's ladder does not allow). Only online players can
    /// be named: their uuid comes from the session, and matching by name
    /// would let anyone take an operator's identity. Grants land at level 4,
    /// Vanilla's default `op-permission-level`.
    fn command_op(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /op <player>");
        };
        let Some(target) = self.session_id_by_name(name) else {
            return CommandResult::message(format!(
                "Cannot op {name:?}: only online players can be granted (name matching cannot identify anyone else)"
            ));
        };
        match self.grant_operator(target, mc_command::PermissionLevel::Console) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the grant, and nothing was changed: {error}"
            )),
            Ok(None) => CommandResult::message(
                "/op cannot persist: no ops directory was ever set, so nothing was changed.",
            ),
            Ok(Some((granted, true))) => {
                CommandResult::message(format!("Made {granted} a server operator"))
            }
            Ok(Some((granted, false))) => {
                CommandResult::message(format!("Nothing changed: {granted} is already an operator"))
            }
        }
    }

    /// `/deop <player>` (P14-02): the reverse grant, same ladder, same file.
    fn command_deop(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /deop <player>");
        };
        let Some(target) = self.session_id_by_name(name) else {
            return CommandResult::message(format!(
                "Cannot deop {name:?}: only online players can be revoked"
            ));
        };
        match self.revoke_operator(target) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the revocation, and nothing was changed: {error}"
            )),
            Ok(None) => {
                CommandResult::message(format!("Nothing changed: {name} is not an operator"))
            }
            Ok(Some(revoked)) => {
                CommandResult::message(format!("Made {revoked} no longer a server operator"))
            }
        }
    }

    /// Parse a game-mode word: Vanilla's full names plus the `s`/`c`/`a`/`sp`
    /// shortcuts Vanilla accepts.
    fn parse_game_mode(word: &str) -> Option<mc_entity::GameMode> {
        use mc_entity::GameMode;
        Some(match word {
            "survival" | "s" => GameMode::Survival,
            "creative" | "c" => GameMode::Creative,
            "adventure" | "a" => GameMode::Adventure,
            "spectator" | "sp" => GameMode::Spectator,
            _ => return None,
        })
    }

    /// `/gamemode <mode> [target]`
    fn command_gamemode(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(word) = parsed.string(0) else {
            return CommandResult::message("Usage: /gamemode <mode> [target]");
        };
        let Some(mode) = Self::parse_game_mode(word) else {
            return CommandResult::message(
                "Usage: /gamemode <survival|creative|adventure|spectator> [target]",
            );
        };
        let target = parsed.string(1).unwrap_or(&parsed.source.name);
        if !target.eq_ignore_ascii_case(&parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot change {target:?}'s game mode: this build only changes the invoking player"
            ));
        }
        if !self.set_player_game_mode(id, mode) {
            return CommandResult::message("You are not online.");
        }
        let name = match mode {
            mc_entity::GameMode::Survival => "Survival Mode",
            mc_entity::GameMode::Creative => "Creative Mode",
            mc_entity::GameMode::Adventure => "Adventure Mode",
            mc_entity::GameMode::Spectator => "Spectator Mode",
        };
        CommandResult::message(format!("Set own game mode to {name}"))
    }

    /// `/give <target> <item> [count]`
    fn command_give(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let target = parsed.string(0).unwrap_or("");
        if !target.eq_ignore_ascii_case(&parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot give to {target:?}: this build only gives to the invoking player"
            ));
        }
        let Some(item) = parsed.argument(1).and_then(|value| value.as_resource()) else {
            return CommandResult::message("Usage: /give <target> <item> [count]");
        };
        let name = item.to_string();
        let Ok(item_id) = self.registries().items.id(&name) else {
            return CommandResult::message(format!("Unknown item {name:?}"));
        };
        // The grammar caps the count at one stack (1..=64), so the conversion
        // below cannot truncate; the fallback is unreachable caution, not a
        // second rule.
        let count = parsed
            .integer(2)
            .and_then(|count| i32::try_from(count).ok())
            .unwrap_or(1);
        match self.give_player_item(id, item_id, count) {
            None => CommandResult::message("You are not online."),
            Some((placed, 0)) => {
                CommandResult::message(format!("Gave {placed} [{name}] to {}", parsed.source.name))
            }
            Some((placed, dropped)) => CommandResult::message(format!(
                "Gave {placed} [{name}] to {} ({dropped} dropped: inventory full)",
                parsed.source.name
            )),
        }
    }

    /// `/kill [target]`
    fn command_kill(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let target = parsed.string(0).unwrap_or(&parsed.source.name);
        if !target.eq_ignore_ascii_case(&parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot kill {target:?}: this build only kills the invoking player"
            ));
        }
        match self.kill_player(id) {
            None => CommandResult::message("You are not online."),
            Some(outcome) if outcome.died => {
                CommandResult::message(format!("Killed {}", parsed.source.name))
            }
            Some(_) => CommandResult::message(format!("{} is already dead", parsed.source.name)),
        }
    }

    /// `/effect give <target> <effect> [seconds] [amplifier]` /
    /// `/effect clear [<target> [<effect>]]` (P16-03).
    ///
    /// The operator-visible source for status effects: no natural source
    /// exists in the tree yet (no eating, no witch, no beacon), so a command
    /// is what puts an effect on a player — the same way vanilla's command
    /// does. Like `give`/`kill`, only the invoking player may be targeted.
    /// Unmodelled effect names are refused with the modelled list rather
    /// than granted as dead icons.
    fn command_effect(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let action = parsed.string(0).unwrap_or("");
        let target = parsed.string(1).unwrap_or(&parsed.source.name);
        if !target.eq_ignore_ascii_case(&parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot affect {target:?}: this build only targets the invoking player"
            ));
        }
        match action {
            "give" => self.command_effect_give(id, parsed, report),
            "clear" => self.command_effect_clear(id, parsed, report),
            _ => CommandResult::message(
                "Usage: /effect <give|clear> <target> [<effect> [seconds] [amplifier]]",
            ),
        }
    }

    /// `/effect give` arm: resolve, store, announce, confirm.
    fn command_effect_give(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(name) = parsed.string(2) else {
            return CommandResult::message(
                "Usage: /effect give <target> <effect> [seconds] [amplifier]",
            );
        };
        let Some(kind) = mc_entity::effect::EffectKind::from_name(name) else {
            return CommandResult::message(format!(
                "Unknown effect {name:?}: this build models speed, slowness, strength, weakness, resistance, poison, wither and regeneration"
            ));
        };
        let seconds = parsed.integer(3).unwrap_or(30).clamp(1, 1_000_000);
        let amplifier = i32::try_from(parsed.integer(4).unwrap_or(0).clamp(0, 255)).unwrap_or(0);
        let duration = seconds.saturating_mul(20).min(i64::from(i32::MAX));
        let duration = i32::try_from(duration).unwrap_or(i32::MAX);
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        session.player.give_effect(kind.id(), amplifier, duration);
        let packet = mc_protocol::packets::play::UpdateMobEffect {
            entity_id: session.entity.get(),
            effect_id: kind.id(),
            amplifier,
            duration,
            flags: mc_protocol::packets::play::UpdateMobEffect::flags_for(false),
        };
        let _ = self.send(id, &packet, report);
        CommandResult::message(format!(
            "Gave {} {} ({}s) to {}",
            name, amplifier, seconds, parsed.source.name
        ))
    }

    /// `/effect clear` arm: one id or everything, each with its removal packet.
    fn command_effect_clear(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        let entity_id = session.entity.get();
        let removed: Vec<i32> = if let Some(name) = parsed.string(2) {
            let Some(kind) = mc_entity::effect::EffectKind::from_name(name) else {
                return CommandResult::message(format!("Unknown effect {name:?}"));
            };
            session
                .player
                .clear_effect(kind.id())
                .then_some(kind.id())
                .into_iter()
                .collect()
        } else {
            let ids: Vec<i32> = session.player.effects.keys().copied().collect();
            for effect_id in &ids {
                session.player.clear_effect(*effect_id);
            }
            ids
        };
        for effect_id in &removed {
            let packet = mc_protocol::packets::play::RemoveMobEffect {
                entity_id,
                effect_id: *effect_id,
            };
            let _ = self.send(id, &packet, report);
        }
        CommandResult::message(format!(
            "Cleared {} effect(s) from {}",
            removed.len(),
            parsed.source.name
        ))
    }

    /// `/seed`
    fn command_seed(&self) -> CommandResult {
        CommandResult::message(format!("Seed: {}", self.random_seed()))
    }

    /// `/difficulty [difficulty]`
    fn command_difficulty(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        use mc_persistence::level::Difficulty;
        fn display(difficulty: Difficulty) -> &'static str {
            match difficulty {
                Difficulty::Peaceful => "Peaceful",
                Difficulty::Easy => "Easy",
                Difficulty::Normal => "Normal",
                Difficulty::Hard => "Hard",
            }
        }
        let Some(word) = parsed.string(0) else {
            return CommandResult::message(format!(
                "The difficulty is {}",
                display(self.difficulty())
            ));
        };
        // Vanilla's four full names, case-sensitive like Vanilla's enum parser.
        let difficulty = match word {
            "peaceful" => Difficulty::Peaceful,
            "easy" => Difficulty::Easy,
            "normal" => Difficulty::Normal,
            "hard" => Difficulty::Hard,
            _ => {
                return CommandResult::message("Usage: /difficulty [peaceful|easy|normal|hard]");
            }
        };
        if self.difficulty_locked() {
            return CommandResult::message("The difficulty is locked and cannot be changed.");
        }
        if let Err(error) = self.set_difficulty(difficulty) {
            return CommandResult::message(format!("Could not save level.dat: {error}"));
        }
        CommandResult::message(format!("Set the difficulty to {}", display(difficulty)))
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
