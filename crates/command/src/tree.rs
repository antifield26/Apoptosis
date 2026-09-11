//! The command tree: what names exist and what arguments each accepts (P07-01).
//!
//! A [`CommandTree`] is a flat, immutable description — see the crate docs for why it is
//! not a Brigadier-style mutable graph. The property that buys is
//! [`CommandTree::validate`]: every invariant a graph can violate at runtime is checked
//! once, at construction, and the errors name the offending command.

use crate::argument::{Argument, ArgumentKind};

/// Why a tree is not usable.
///
/// All of these are *construction* errors: the tree is built by the server, so a bad one
/// is a programming mistake rather than hostile input. Reporting them as values rather
/// than panicking means a plugin or a test can assert on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    /// A command's name is empty.
    EmptyName,
    /// Two commands share a root literal.
    DuplicateName(String),
    /// A command has more arguments than [`crate::MAX_ARGUMENTS`].
    TooManyArguments {
        /// The command.
        command: String,
        /// How many it declared.
        count: usize,
        /// The limit.
        limit: usize,
    },
    /// A required argument appears after an optional one.
    OptionalBeforeRequired {
        /// The command.
        command: String,
        /// The argument that is optional but not last.
        optional: &'static str,
        /// The required argument that follows it.
        required: &'static str,
    },
    /// A greedy argument is not the last one.
    ///
    /// A greedy argument consumes the rest of the input, so anything after it could
    /// never be supplied.
    GreedyNotLast {
        /// The command.
        command: String,
        /// The greedy argument.
        argument: &'static str,
    },
    /// An argument has an empty name.
    EmptyArgumentName {
        /// The command.
        command: String,
    },
    /// Two arguments in one command share a name.
    DuplicateArgument {
        /// The command.
        command: String,
        /// The repeated name.
        argument: &'static str,
    },
    /// A numeric range is inverted (`min > max`).
    InvertedRange {
        /// The command.
        command: String,
        /// The argument.
        argument: &'static str,
    },
    /// A double range is inverted or non-finite.
    InvalidDoubleRange {
        /// The command.
        command: String,
        /// The argument.
        argument: &'static str,
    },
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "a command needs a name"),
            Self::DuplicateName(name) => write!(f, "two commands are named {name:?}"),
            Self::TooManyArguments {
                command,
                count,
                limit,
            } => write!(f, "{command} declares {count} arguments, limit is {limit}"),
            Self::OptionalBeforeRequired {
                command,
                optional,
                required,
            } => write!(
                f,
                "{command}: <{optional}> is optional but <{required}> follows it, which \
                 makes the grammar ambiguous"
            ),
            Self::GreedyNotLast { command, argument } => write!(
                f,
                "{command}: <{argument}> consumes the rest of the input, so nothing may \
                 follow it"
            ),
            Self::EmptyArgumentName { command } => {
                write!(f, "{command} has an argument with no name")
            }
            Self::DuplicateArgument { command, argument } => {
                write!(f, "{command} declares <{argument}> twice")
            }
            Self::InvertedRange { command, argument } => {
                write!(f, "{command}: <{argument}> has min > max")
            }
            Self::InvalidDoubleRange { command, argument } => {
                write!(
                    f,
                    "{command}: <{argument}> has a non-finite or inverted range"
                )
            }
        }
    }
}

impl std::error::Error for TreeError {}

/// One command: a name, its arguments, and what it requires to run.
///
/// **Not `Eq`**: an argument may carry a floating-point range, which has no total
/// equality.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// The root literal, without a leading slash.
    pub name: &'static str,
    /// A one-line description, for `/help` and for suggestions.
    pub description: &'static str,
    /// The arguments, in the order they are supplied.
    pub arguments: Vec<Argument>,
    /// Lowest permission level that may run it.
    pub permission: crate::source::PermissionLevel,
}

impl Command {
    /// A command with no arguments.
    #[must_use]
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            arguments: Vec::new(),
            permission: crate::source::PermissionLevel::All,
        }
    }

    /// Add an argument, builder-style.
    #[must_use]
    pub fn with_argument(mut self, argument: Argument) -> Self {
        self.arguments.push(argument);
        self
    }

    /// Set the permission level, builder-style.
    #[must_use]
    pub const fn requiring(mut self, permission: crate::source::PermissionLevel) -> Self {
        self.permission = permission;
        self
    }

    /// The fewest arguments a valid invocation needs: the count up to the first
    /// optional one.
    #[must_use]
    pub fn required_count(&self) -> usize {
        self.arguments
            .iter()
            .take_while(|argument| !argument.optional)
            .count()
    }

    /// Whether the command is usable with `supplied` arguments.
    ///
    /// Counts **arguments**, not tokens. Use [`Command::accepts_token_count`] to check a
    /// token count, which differs whenever an argument consumes more than one token.
    #[must_use]
    pub fn accepts_count(&self, supplied: usize) -> bool {
        supplied >= self.required_count() && supplied <= self.arguments.len()
    }

    /// How many tokens an argument consumes: 3 for a block position, 1 otherwise.
    #[must_use]
    const fn tokens_for(argument: &Argument) -> usize {
        match argument.kind {
            ArgumentKind::BlockPos => 3,
            _ => 1,
        }
    }

    /// The fewest and most tokens a valid invocation may supply.
    ///
    /// `max` is `None` when the last argument is greedy, because such an argument
    /// absorbs every remaining token by definition — so the upper bound is not a
    /// property of the tree at all. `min` counts only the required prefix, with a
    /// `BlockPos` needing three tokens.
    ///
    /// This exists because checking a token count against the argument count is wrong
    /// for two of the argument kinds, which is a bug the dispatcher tests caught: `say a
    /// b c` (one greedy argument, three tokens) and `tp Alex 10 64 -5` (two arguments,
    /// four tokens) were both refused as "too many arguments".
    #[must_use]
    pub fn token_bounds(&self) -> (usize, Option<usize>) {
        let mut min = 0usize;
        let mut max = 0usize;
        for (index, argument) in self.arguments.iter().enumerate() {
            let tokens = Self::tokens_for(argument);
            max += tokens;
            if index < self.required_count() {
                min += tokens;
            }
            if argument.kind.is_greedy() {
                return (min, None);
            }
        }
        (min, Some(max))
    }

    /// Whether the command is usable with `supplied` **tokens**.
    #[must_use]
    pub fn accepts_token_count(&self, supplied: usize) -> bool {
        let (min, max) = self.token_bounds();
        supplied >= min && max.is_none_or(|max| supplied <= max)
    }

    /// The command's usage string, in the `name <arg> [arg]` form Brigadier prints.
    #[must_use]
    pub fn usage(&self) -> String {
        let mut out = String::from("/");
        out.push_str(self.name);
        for argument in &self.arguments {
            out.push(' ');
            if argument.optional {
                out.push('[');
            } else {
                out.push('<');
            }
            out.push_str(argument.name);
            out.push(if argument.optional { ']' } else { '>' });
        }
        out
    }

    /// The indices of arguments that consume the rest of the input, if any.
    #[must_use]
    pub fn greedy_index(&self) -> Option<usize> {
        self.arguments
            .iter()
            .position(|argument| argument.kind.is_greedy())
    }

    /// Validate this command alone; `CommandTree::validate` calls it for every command.
    ///
    /// # Errors
    ///
    /// The first problem found, as a [`TreeError`].
    pub fn validate(&self) -> Result<(), TreeError> {
        if self.name.is_empty() {
            return Err(TreeError::EmptyName);
        }
        if self.arguments.len() > crate::MAX_ARGUMENTS {
            return Err(TreeError::TooManyArguments {
                command: self.name.to_owned(),
                count: self.arguments.len(),
                limit: crate::MAX_ARGUMENTS,
            });
        }
        let mut seen_optional: Option<&'static str> = None;
        let mut seen_names: Vec<&'static str> = Vec::with_capacity(self.arguments.len());
        for (index, argument) in self.arguments.iter().enumerate() {
            if argument.name.is_empty() {
                return Err(TreeError::EmptyArgumentName {
                    command: self.name.to_owned(),
                });
            }
            if seen_names.contains(&argument.name) {
                return Err(TreeError::DuplicateArgument {
                    command: self.name.to_owned(),
                    argument: argument.name,
                });
            }
            seen_names.push(argument.name);

            if let Some(optional) = seen_optional {
                if !argument.optional {
                    return Err(TreeError::OptionalBeforeRequired {
                        command: self.name.to_owned(),
                        optional,
                        required: argument.name,
                    });
                }
            } else if argument.optional {
                seen_optional = Some(argument.name);
            }

            match &argument.kind {
                ArgumentKind::Integer(range) | ArgumentKind::Long(range) => {
                    if !range.is_valid() {
                        return Err(TreeError::InvertedRange {
                            command: self.name.to_owned(),
                            argument: argument.name,
                        });
                    }
                }
                ArgumentKind::Double { min, max }
                    if !min.is_finite() || !max.is_finite() || min > max =>
                {
                    return Err(TreeError::InvalidDoubleRange {
                        command: self.name.to_owned(),
                        argument: argument.name,
                    });
                }
                _ => {}
            }

            if argument.kind.is_greedy() && index + 1 != self.arguments.len() {
                return Err(TreeError::GreedyNotLast {
                    command: self.name.to_owned(),
                    argument: argument.name,
                });
            }
        }
        Ok(())
    }
}

/// Every command the server knows, indexed by root literal.
///
/// Ordered by name so dispatch, `/help` and suggestion output are all reproducible
/// (AGENTS.md §3.6).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandTree {
    commands: Vec<Command>,
}

impl CommandTree {
    /// An empty tree.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Add a command.
    ///
    /// # Errors
    ///
    /// [`TreeError::DuplicateName`] or any error [`Command::validate`] reports. Adding is
    /// fallible rather than panicking so a tree built from configuration can be checked
    /// at startup instead of aborting.
    pub fn insert(&mut self, command: Command) -> Result<(), TreeError> {
        command.validate()?;
        if self.get(command.name).is_some() {
            return Err(TreeError::DuplicateName(command.name.to_owned()));
        }
        // Keep the list sorted by name, so iteration is stable without a map.
        let position = self
            .commands
            .partition_point(|existing| existing.name < command.name);
        self.commands.insert(position, command);
        Ok(())
    }

    /// A command by root literal.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Command> {
        self.commands
            .binary_search_by(|command| command.name.cmp(name))
            .ok()
            .map(|index| &self.commands[index])
    }

    /// Every command, ascending by name.
    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// How many commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Every root literal, ascending.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        self.commands.iter().map(|command| command.name)
    }

    /// Roots that begin with `prefix`, ascending — the basis of suggestions.
    #[must_use]
    pub fn matching(&self, prefix: &str) -> Vec<&Command> {
        self.commands
            .iter()
            .filter(|command| command.name.starts_with(prefix))
            .collect()
    }

    /// Validate every command again.
    ///
    /// Redundant after [`CommandTree::insert`], and kept because a tree assembled some
    /// other way (a future deserialization, or a plugin) can be checked before use.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), TreeError> {
        for command in &self.commands {
            command.validate()?;
        }
        Ok(())
    }
}
