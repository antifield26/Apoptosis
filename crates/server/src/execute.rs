//! `/execute` resolution and dispatch (P07-07).
//!
//! `mc_command::execute` parses a modifier chain; this applies it. The split is where the
//! knowledge lives: the parser knows the grammar, and only the server knows the entity store and
//! the world that `as`, `at` and `if block` need.
//!
//! ## The resolution model
//!
//! A command source is *replaced* as modifiers are applied, **in written order** — which is why
//! the chain keeps its order rather than normalising it:
//!
//! | Modifier | What it replaces |
//! |---|---|
//! | `as <selector>` | the name, and therefore who a later `@s` resolves to |
//! | `at <selector>` | position and dimension, from the **first** matched entity |
//! | `positioned <x y z>` | position |
//! | `align <axes>` | position, floored on the named axes |
//! | `if`/`unless` | nothing — it decides whether to stop |
//!
//! ## Three things this deliberately does not do
//!
//! **1. `as @a` runs once, not once per entity.** Vanilla runs the command for each match. This
//! build runs it **once, as the first match** (ordered by name, so it is reproducible). The
//! reason is not laziness: a multi-target run multiplies a command that may not be idempotent,
//! and `/execute as @a run say hi` announcing itself once rather than N times is the behaviour
//! that cannot surprise an operator. Recorded in the parity matrix as a divergence rather than
//! presented as Vanilla's semantics.
//!
//! **2. Only players are command sources.** A non-player entity has no inventory, no permission
//! level and no reply channel, so there is nothing for a command to run against. `@e` therefore
//! resolves to the players among the entities, and a selector matching only mobs selects nothing
//! — reported as "no entity matched" rather than silently running as the invoker.
//!
//! **3. `store` is not supported**, and neither are `rotated`, `facing`, `anchored`, `in` nor the
//! `data`/`score`/`predicate`/`biome`/`loaded`/`blocks`/`function` conditions. Each is **refused
//! by name** by the parser.
//!
//! ## Recursion
//!
//! `/execute run execute run …` terminates only by a bound. [`MAX_DEPTH`] is that bound, and
//! exceeding it is a message naming the limit rather than a stack overflow — the one place in
//! this crate where hostile input could otherwise take the process down.

use mc_command::execute::{Condition, ExecuteChain, Modifier, align, resolve_coordinates};
use mc_command::selector::{EntityFacts, Selector};
use mc_command::source::{CommandSource, SourcePosition};
use mc_core::error::ServerResult;
use mc_protocol::text::TextComponent;

use crate::commands::CommandResult;
use crate::game::{Game, TickReport};

/// Deepest `/execute` nesting accepted.
///
/// A chain cannot nest without `run execute`, so this bounds the *recursion*, while
/// `mc_command::execute::MAX_MODIFIERS` bounds the chain length separately. 16 is far beyond any
/// real command and shallow enough that the stack is never in question.
pub const MAX_DEPTH: u32 = 16;

/// What resolving a chain produced.
enum Resolution {
    /// Run `command` as this source.
    Run {
        /// The source after every modifier.
        source: CommandSource,
        /// The inner command text.
        command: String,
    },
    /// A condition did not hold, so nothing runs.
    ConditionFailed {
        /// A phrase naming the condition.
        which: String,
    },
}

impl Game {
    /// Run an `/execute` chain for a connection.
    ///
    /// # Errors
    ///
    /// As for [`Game::dispatch_command`](crate::commands::Game::dispatch_command).
    pub fn dispatch_execute(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        tokens: &[String],
        report: &mut TickReport,
        depth: u32,
    ) -> ServerResult<()> {
        let Some(source) = self.command_source(id) else {
            return Ok(());
        };
        if depth >= MAX_DEPTH {
            return self.reply(
                id,
                format!("execute may nest at most {MAX_DEPTH} deep."),
                report,
            );
        }
        let chain = match mc_command::execute::parse(tokens) {
            Ok(chain) => chain,
            Err(error) => return self.reply(id, error.to_string(), report),
        };

        let resolved = match self.resolve_chain(&chain, &source) {
            Ok(resolved) => resolved,
            Err(message) => return self.reply(id, message, report),
        };
        let Resolution::Run {
            source: inner_source,
            command,
        } = resolved
        else {
            let Resolution::ConditionFailed { which } = resolved else {
                unreachable!()
            };
            return self.reply(id, format!("Test failed: {which}"), report);
        };

        // The inner command goes back through the dispatcher, so its **own** permission check
        // applies -- and the source it is checked against is the one the chain produced.
        // `select` attaches each matched session's **own** permission level
        // (`with_permission(session.permission)`), so `/execute as @a run <op command>`
        // succeeds only for the operators in `@a`: the permission travels with the new
        // source rather than staying with the invoker. An earlier version of this comment
        // said the opposite ("`as` changes who the command runs as, not what they are
        // allowed to do"), which is the reverse of what the code does and was read that way
        // (AUDIT-09 C-06). That the transfer is also Vanilla's behaviour is AUDIT-09 lane C's
        // finding; it is not re-derived here, and `execute_e2e` pins our side.
        let dispatcher = mc_command::Dispatcher::new(Self::build_command_tree());
        match dispatcher.parse(&command, &inner_source) {
            mc_command::CommandOutcome::Parsed(parsed) => {
                if parsed.name == "execute" {
                    // Recurse: `execute … run execute …`. The inner text is handed back to the
                    // chain parser unchanged, so nesting parses exactly as the top level does.
                    let inner: Vec<String> = parsed
                        .raw_arguments
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    return self.dispatch_execute(id, &inner, report, depth + 1);
                }
                // The **invoking** connection replies, even when `as` changed the source: a
                // non-player source has no reply channel, and a player who invoked the command
                // needs to see the outcome. Vanilla sends feedback to the executing source; this
                // build sends it to the invoker and says so here.
                match self.run_command(id, &parsed, report)? {
                    CommandResult::Ok { feedback } => {
                        if let Some(text) = feedback {
                            self.reply(id, text, report)?;
                        }
                        Ok(())
                    }
                    CommandResult::Stop => {
                        self.request_shutdown();
                        self.reply(id, "Stopping the server…".to_owned(), report)
                    }
                }
            }
            other => {
                let message = describe_outcome(&other);
                self.reply(id, message, report)
            }
        }
    }

    /// Apply every modifier, in order, to produce the source and command to run.
    fn resolve_chain(
        &self,
        chain: &ExecuteChain,
        source: &CommandSource,
    ) -> Result<Resolution, String> {
        let mut current = source.clone();
        for modifier in &chain.modifiers {
            let centre = (current.position.x, current.position.y, current.position.z);
            match modifier {
                Modifier::As(selector) => {
                    let mut matches = self.select(selector, centre);
                    if matches.is_empty() {
                        return Err(format!("No entity matched {}", selector.kind.name()));
                    }
                    current = matches.remove(0);
                }
                Modifier::At(selector) => {
                    let matches = self.select(selector, centre);
                    let Some(first) = matches.into_iter().next() else {
                        return Err(format!("No entity matched {}", selector.kind.name()));
                    };
                    current.position = first.position;
                    current.dimension = first.dimension;
                }
                Modifier::Positioned { x, y, z } => {
                    let base = block_position(&current)?;
                    let (bx, by, bz) = resolve_coordinates(*x, *y, *z, base);
                    current.position =
                        SourcePosition::new(f64::from(bx), f64::from(by), f64::from(bz));
                }
                Modifier::Align(axes) => {
                    let (px, py, pz) = align(
                        (current.position.x, current.position.y, current.position.z),
                        *axes,
                    );
                    current.position = SourcePosition::new(px, py, pz);
                }
                Modifier::If(condition) => {
                    if let Some(which) = self.condition_fails(condition, &current, centre)? {
                        return Ok(Resolution::ConditionFailed { which });
                    }
                }
                Modifier::Unless(condition) => {
                    if self.condition_fails(condition, &current, centre)?.is_none() {
                        return Ok(Resolution::ConditionFailed {
                            which: describe_condition(condition),
                        });
                    }
                }
            }
        }
        Ok(Resolution::Run {
            source: current,
            command: chain.run.clone(),
        })
    }

    /// Every command source a selector matches.
    ///
    /// Only **players** become command sources, and the sort by name is what makes `as @a` pick
    /// the same "first" entity on every run (AGENTS.md §3.6). `limit` applies after the sort,
    /// which is what makes it meaningful rather than arbitrary.
    fn select(&self, selector: &Selector, centre: (f64, f64, f64)) -> Vec<CommandSource> {
        let mut matched: Vec<CommandSource> = self
            .sessions
            .values()
            .filter(|session| {
                let position = session.player.position;
                let facts = EntityFacts {
                    type_id: "minecraft:player",
                    name: Some(session.player.profile.name.as_str()),
                    position: (position.x, position.y, position.z),
                    // Experience level is not tracked per session yet, so a `level=` filter
                    // cannot match. Stated rather than faked with a zero.
                    level: None,
                    game_mode: Some(selector_game_mode(session.player.game_mode)),
                    is_player: true,
                };
                selector.matches(&facts, centre)
            })
            .map(|session| {
                let position = session.player.position;
                CommandSource::player(
                    session.player.profile.name.clone(),
                    SourcePosition::new(position.x, position.y, position.z),
                )
                .with_permission(session.permission)
            })
            .collect();
        matched.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(limit) = selector.effective_limit() {
            matched.truncate(limit as usize);
        }
        matched
    }

    /// Whether a condition fails, and how to describe it. `None` means it holds.
    fn condition_fails(
        &self,
        condition: &Condition,
        source: &CommandSource,
        centre: (f64, f64, f64),
    ) -> Result<Option<String>, String> {
        match condition {
            Condition::Entity(selector) => {
                if self.select(selector, centre).is_empty() {
                    Ok(Some(describe_condition(condition)))
                } else {
                    Ok(None)
                }
            }
            Condition::Block { x, y, z, block } => {
                let base = block_position(source)?;
                let (bx, by, bz) = resolve_coordinates(*x, *y, *z, base);
                let name = format!("{}:{}", block.namespace(), block.value());
                // An unknown block is a *command* error, not a false condition: reporting "test
                // failed" for a typo would send an operator looking for the wrong problem.
                let expected = self
                    .registries()
                    .blocks
                    .default_state(&name)
                    .map_err(|_| format!("Unknown block {name}"))?;
                if self.world().get_block(bx, by, bz) == expected {
                    Ok(None)
                } else {
                    Ok(Some(format!(
                        "{} at {bx} {by} {bz}",
                        describe_condition(condition)
                    )))
                }
            }
        }
    }

    /// Send one line of feedback to a connection.
    fn reply(
        &self,
        id: mc_network::bridge::ConnectionId,
        text: String,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        self.send(
            id,
            &mc_protocol::packets::play::DisguisedChat {
                message: TextComponent::literal(text),
                chat_type: crate::game::CHAT_TYPE_CHAT,
                sender_name: TextComponent::literal("Server"),
                target_name: None,
            },
            report,
        )
        .map(|_sent| ())
    }
}

/// Convert the simulation's game mode into the selector's.
///
/// The two enums are **deliberately separate**: `mc-command` must not depend on `mc-entity`
/// (a command framework has no business knowing what a mob is), so the selector defines its own
/// four variants. That makes this conversion the honest join between them, and the alternative —
/// sharing one type — would invert the dependency to save eight lines.
fn selector_game_mode(mode: mc_entity::GameMode) -> mc_command::selector::GameMode {
    match mode {
        mc_entity::GameMode::Survival => mc_command::selector::GameMode::Survival,
        mc_entity::GameMode::Creative => mc_command::selector::GameMode::Creative,
        mc_entity::GameMode::Adventure => mc_command::selector::GameMode::Adventure,
        mc_entity::GameMode::Spectator => mc_command::selector::GameMode::Spectator,
    }
}

/// The block position a `~` resolves against.
fn block_position(source: &CommandSource) -> Result<(i32, i32, i32), String> {
    source
        .block_position()
        .ok_or_else(|| "The executing position is not finite.".to_owned())
}

/// A phrase naming a condition, for the "test failed" message.
fn describe_condition(condition: &Condition) -> String {
    match condition {
        Condition::Entity(selector) => format!("entity {}", selector.kind.name()),
        Condition::Block { x, y, z, block } => {
            let axis = |value: Option<i32>| match value {
                Some(number) => number.to_string(),
                None => "~".to_owned(),
            };
            format!("block {} {} {} {block}", axis(*x), axis(*y), axis(*z))
        }
    }
}

/// The message for a failed inner dispatch.
fn describe_outcome(outcome: &mc_command::CommandOutcome) -> String {
    match outcome {
        mc_command::CommandOutcome::UnknownCommand { name, .. } => {
            format!("Unknown command {name:?}.")
        }
        mc_command::CommandOutcome::PermissionDenied { name, .. } => {
            format!("You do not have permission to use /{name}.")
        }
        mc_command::CommandOutcome::BadArguments(error) => error.to_string(),
        // Unreachable: this is called only for the non-`Parsed` arms.
        mc_command::CommandOutcome::Parsed(_) => "Internal error.".to_owned(),
    }
}
