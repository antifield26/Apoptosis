//! The dispatcher: text in, a validated invocation out (P07-02).
//!
//! [`Dispatcher::parse`] turns a command string into a [`ParsedCommand`] — the command,
//! its typed arguments, and the source — after checking the **permission** and the
//! argument list. It runs nothing: execution needs the world, so it belongs to the
//! caller (P07-05/P07-07). Keeping the split means every validation rule is testable
//! without a server, and a command handler cannot accidentally be reached with
//! unvalidated arguments.
//!
//! ## The resolution order, and why it matters
//!
//! 1. **Tokenize** — the whole string, bounded by [`crate::MAX_COMMAND_CHARS`].
//! 2. **Root lookup** — an unknown name stops here, and the suggestion list is built
//!    from the prefix so the message can be useful.
//! 3. **Permission** — checked *before* arguments are parsed, so a lower-privileged
//!    source cannot probe a command's argument grammar by reading its error messages.
//!    Vanilla does the same, and the ordering is deliberate rather than incidental.
//! 4. **Arguments** — parsed left to right against the declared list.
//!
//! ## Greedy arguments and `BlockPos`
//!
//! Two argument kinds do not map to exactly one token:
//!
//! - a **greedy** argument takes every remaining token and joins them with a single
//!   space, which is how `say hello there` becomes one message;
//! - a **`BlockPos`** takes the next **three** tokens, which is why `parse_value`
//!   receives them joined.
//!
//! Both are handled here because the token count is a dispatch concern: the argument
//! parser sees one string either way.

use crate::argument::{ArgumentValue, ParseError, parse_value, split_root, tokenize};
use crate::source::CommandSource;
use crate::tree::{Command, CommandTree};

/// A command that parsed, validated and passed its permission check.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCommand {
    /// The command's name.
    pub name: &'static str,
    /// One value per declared argument that was supplied.
    pub arguments: Vec<ArgumentValue>,
    /// The source, for a handler that needs the position or the name.
    pub source: CommandSource,
    /// The raw text after the root, unmodified.
    ///
    /// Kept for the commands that pass it through (`say`), and because a handler that
    /// wants something the parser does not model should not have to re-slice the input.
    pub raw_arguments: String,
}

impl ParsedCommand {
    /// The argument at `index`, when it was supplied.
    #[must_use]
    pub fn argument(&self, index: usize) -> Option<&ArgumentValue> {
        self.arguments.get(index)
    }

    /// The argument at `index` as a string.
    #[must_use]
    pub fn string(&self, index: usize) -> Option<&str> {
        self.argument(index).and_then(ArgumentValue::as_str)
    }

    /// The argument at `index` as an integer.
    #[must_use]
    pub fn integer(&self, index: usize) -> Option<i64> {
        self.argument(index).and_then(ArgumentValue::as_i64)
    }

    /// The argument at `index` as a double.
    #[must_use]
    pub fn double(&self, index: usize) -> Option<f64> {
        self.argument(index).and_then(ArgumentValue::as_f64)
    }

    /// The argument at `index` as a boolean.
    #[must_use]
    pub fn boolean(&self, index: usize) -> Option<bool> {
        self.argument(index).and_then(ArgumentValue::as_bool)
    }
}

/// What parsing produced.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// A parsed, permitted command, ready to run.
    Parsed(Box<ParsedCommand>),
    /// The root was not a known command.
    UnknownCommand {
        /// The name that was supplied.
        name: String,
        /// Known names beginning with the same prefix, for the message.
        suggestions: Vec<&'static str>,
    },
    /// The source is not allowed to use it.
    ///
    /// Carries the required level so the caller can log it, and **not** the argument
    /// grammar — see the resolution order in the module docs.
    PermissionDenied {
        /// The command.
        name: &'static str,
        /// What it requires.
        required: crate::source::PermissionLevel,
        /// What the source has.
        actual: crate::source::PermissionLevel,
    },
    /// The arguments did not fit the command's grammar.
    BadArguments(ParseError),
}

impl CommandOutcome {
    /// Whether parsing succeeded.
    #[must_use]
    pub const fn is_parsed(&self) -> bool {
        matches!(self, Self::Parsed(_))
    }

    /// The parsed command, when there is one.
    #[must_use]
    pub fn parsed(&self) -> Option<&ParsedCommand> {
        match self {
            Self::Parsed(command) => Some(command),
            _ => None,
        }
    }
}

/// A completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The text to offer.
    pub text: String,
    /// Whether offering it should append a space (a finished argument).
    pub complete: bool,
}

/// Parses commands against a tree.
#[derive(Debug, Clone)]
pub struct Dispatcher {
    tree: CommandTree,
}

impl Dispatcher {
    /// A dispatcher over a tree.
    #[must_use]
    pub const fn new(tree: CommandTree) -> Self {
        Self { tree }
    }

    /// The tree.
    #[must_use]
    pub const fn tree(&self) -> &CommandTree {
        &self.tree
    }

    /// Parse and validate a command string.
    ///
    /// Never fails: every failure is a [`CommandOutcome`] variant, because a bad command
    /// is normal input from a player rather than an error condition (AGENTS.md §9).
    #[must_use]
    pub fn parse(&self, input: &str, source: &CommandSource) -> CommandOutcome {
        let Ok(tokens) = tokenize(input) else {
            // A tokenizer failure is an argument-level error; the caller's message is
            // the same as for any malformed input.
            return CommandOutcome::BadArguments(
                tokenize(input).err().unwrap_or(ParseError::Empty),
            );
        };
        let Some((root, rest)) = tokens.split_first() else {
            return CommandOutcome::BadArguments(ParseError::Empty);
        };

        let Some(command) = self.tree.get(root) else {
            // A leading slash is already handled by `tokenize` only when the caller
            // passed it, so try the stripped form before reporting an unknown command.
            let (bare, _) = split_root(root);
            let found = self.tree.get(bare);
            let Some(command) = found else {
                return CommandOutcome::UnknownCommand {
                    name: root.clone(),
                    suggestions: self
                        .tree
                        .matching(root)
                        .iter()
                        .map(|command| command.name)
                        .collect(),
                };
            };
            return Self::finish(command, rest, tokens.len() - 1, source, input);
        };

        Self::finish(command, rest, tokens.len() - 1, source, input)
    }

    /// Permission check, then argument parsing.
    ///
    /// An associated function rather than a method: it needs nothing from the
    /// dispatcher, because the tree lookup has already happened and the remaining work
    /// is a pure function of the command, the tokens and the source.
    fn finish(
        command: &Command,
        rest: &[String],
        supplied: usize,
        source: &CommandSource,
        input: &str,
    ) -> CommandOutcome {
        // Permission before grammar, so error messages do not leak a command's shape.
        if !source.may_use(command.permission) {
            return CommandOutcome::PermissionDenied {
                name: command.name,
                required: command.permission,
                actual: source.permission,
            };
        }
        // A **token** count, so the tree's token bounds are what apply: an argument
        // may consume three tokens (`BlockPos`) or all of them (greedy).
        if !command.accepts_token_count(supplied) {
            let (minimum, _) = command.token_bounds();
            return CommandOutcome::BadArguments(if supplied < minimum {
                ParseError::MissingArgument {
                    name: command
                        .arguments
                        .get(supplied)
                        .map_or("argument", |argument| argument.name),
                }
            } else {
                ParseError::Invalid {
                    name: command.arguments.last().map_or("argument", |a| a.name),
                    expected: "no further arguments",
                    found: truncate_for_message(&rest.join(" ")),
                }
            });
        }

        let mut values: Vec<ArgumentValue> = Vec::with_capacity(command.arguments.len());
        let mut cursor = 0usize;
        for argument in &command.arguments {
            if cursor >= rest.len() {
                // Only reachable for an optional argument; a required one was caught by
                // the count check above.
                break;
            }
            if argument.kind.is_greedy() {
                let joined = rest[cursor..].join(" ");
                match parse_value(argument, &joined) {
                    Ok(value) => values.push(value),
                    Err(error) => return CommandOutcome::BadArguments(error),
                }
                cursor = rest.len();
                continue;
            }
            if matches!(argument.kind, crate::argument::ArgumentKind::BlockPos) {
                // Three tokens, joined for the parser.
                if cursor + 3 > rest.len() {
                    return CommandOutcome::BadArguments(ParseError::MissingArgument {
                        name: argument.name,
                    });
                }
                let joined = rest[cursor..cursor + 3].join(" ");
                match parse_value(argument, &joined) {
                    Ok(value) => values.push(value),
                    Err(error) => return CommandOutcome::BadArguments(error),
                }
                cursor += 3;
                continue;
            }
            match parse_value(argument, &rest[cursor]) {
                Ok(value) => values.push(value),
                Err(error) => return CommandOutcome::BadArguments(error),
            }
            cursor += 1;
        }

        let raw_arguments = split_root(input).1.to_owned();
        CommandOutcome::Parsed(Box::new(ParsedCommand {
            name: command.name,
            arguments: values,
            source: source.clone(),
            raw_arguments,
        }))
    }

    /// Completion candidates for a partially typed command.
    ///
    /// **Roots only.** Completing arguments needs each argument's own candidate source —
    /// a player list, a block-state list, a resource registry — which is the caller's
    /// knowledge, not the tree's. Offering nothing for an argument is honest; offering
    /// a wrong list is not. The argument half is P07-06's, with the selector work.
    #[must_use]
    pub fn suggest(&self, input: &str, source: &CommandSource) -> Vec<Suggestion> {
        let trimmed = input.strip_prefix('/').unwrap_or(input);
        // Only complete while the first word is being typed.
        if trimmed.contains(char::is_whitespace) {
            return Vec::new();
        }
        self.tree
            .matching(trimmed)
            .into_iter()
            .filter(|command| source.may_use(command.permission))
            .map(|command| Suggestion {
                text: command.name.to_owned(),
                // A command with no arguments is complete at its name; one with
                // arguments expects a space next.
                complete: command.arguments.is_empty(),
            })
            .collect()
    }
}

/// Truncate a found value for a message, on a character boundary.
fn truncate_for_message(text: &str) -> String {
    const LIMIT: usize = 32;
    if text.chars().count() <= LIMIT {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(LIMIT).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests;
