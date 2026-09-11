//! Arguments: the value model, the kinds, and tokenizing/parsing one argument
//! (P07-01, P07-02).
//!
//! ## Tokenizing
//!
//! A command is split on whitespace, except that a **double quote** groups a run of
//! characters into one token, and a backslash escapes the next character outside quotes.
//! That is Brigadier's `StringReader` rule and the reason `say "hello world"` is three
//! tokens, not four.
//!
//! The tokenizer is the first place hostile input lands, so it is written as an explicit
//! loop over `Vec<char>` with a bounded input rather than with index arithmetic that can
//! walk off the end. An unterminated quote is an error, not a silent truncation — a
//! half-read command is worse than a refused one.

use mc_core::ids::ResourceId;

/// Why an argument could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The input ran out before a required argument was supplied.
    MissingArgument {
        /// The argument's name.
        name: &'static str,
    },
    /// The token was not the kind the argument needed.
    Invalid {
        /// The argument's name.
        name: &'static str,
        /// What the argument expects, e.g. `an integer`.
        expected: &'static str,
        /// The token as supplied, truncated for a log line.
        found: String,
    },
    /// A number was outside the argument's declared range.
    OutOfRange {
        /// The argument's name.
        name: &'static str,
        /// The value supplied.
        value: i64,
        /// The lowest accepted value.
        min: i64,
        /// The highest accepted value.
        max: i64,
    },
    /// The text ended inside a quoted section.
    UnterminatedQuote,
    /// The command string was longer than [`crate::MAX_COMMAND_CHARS`].
    TooLong {
        /// Its length in characters.
        length: usize,
        /// The ceiling.
        limit: usize,
    },
    /// The whole command was empty or whitespace.
    Empty,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingArgument { name } => write!(f, "expected a value for <{name}>"),
            Self::Invalid {
                name,
                expected,
                found,
            } => write!(f, "<{name}> expects {expected}, got {found:?}"),
            Self::OutOfRange {
                name,
                value,
                min,
                max,
            } => write!(f, "<{name}> must be between {min} and {max}, got {value}"),
            Self::UnterminatedQuote => write!(f, "unterminated quoted string"),
            Self::TooLong { length, limit } => {
                write!(f, "command is {length} characters, limit is {limit}")
            }
            Self::Empty => write!(f, "empty command"),
        }
    }
}

impl std::error::Error for ParseError {}

/// An inclusive numeric range an argument accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueRange {
    /// Lowest accepted value.
    pub min: i64,
    /// Highest accepted value.
    pub max: i64,
}

impl ValueRange {
    /// A range.
    #[must_use]
    pub const fn new(min: i64, max: i64) -> Self {
        Self { min, max }
    }

    /// The full `i32` range, which is what an unbounded integer argument accepts.
    #[must_use]
    pub const fn i32_range() -> Self {
        Self {
            min: i32::MIN as i64,
            max: i32::MAX as i64,
        }
    }

    /// Whether a value is inside.
    #[must_use]
    pub const fn contains(&self, value: i64) -> bool {
        value >= self.min && value <= self.max
    }

    /// Whether the range is well formed (`min <= max`).
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.min <= self.max
    }
}

/// What an argument accepts.
///
/// Deliberately a small set covering the commands actually implemented, each with the
/// parse rule in one place. Vanilla has ~30 argument types; the ones not here are listed
/// in the phase report rather than stubbed.
///
/// **Not `Eq`**, because [`ArgumentKind::Double`] holds an `f64` range and a float has
/// no total equality. `PartialEq` is the strongest honest bound.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgumentKind {
    /// A single word: the rest of one token.
    Word,
    /// A quoted-or-bare string that consumes the remainder of the input as one value.
    ///
    /// This is the greedy argument Vanilla writes as `message`; it exists because
    /// `say hello there` must be one argument, not two.
    GreedyString,
    /// A 32-bit integer within a range.
    Integer(ValueRange),
    /// A 64-bit integer within a range.
    Long(ValueRange),
    /// A double within a range.
    Double {
        /// Lowest accepted value.
        min: f64,
        /// Highest accepted value.
        max: f64,
    },
    /// A boolean: `true` or `false`.
    Bool,
    /// A namespaced resource id, e.g. `minecraft:stone` or `stone`.
    Resource,
    /// A block position: three integers, or `~`-relative forms.
    BlockPos,
    /// A player name (a non-empty word with no whitespace).
    PlayerName,
}

impl ArgumentKind {
    /// Whether this kind swallows the rest of the input as one value.
    ///
    /// Used by the parser to decide whether to keep tokenizing, and by
    /// [`crate::tree::CommandTree::validate`] to enforce that such an argument is last.
    #[must_use]
    pub const fn is_greedy(&self) -> bool {
        matches!(self, Self::GreedyString)
    }

    /// What this kind expects, for an error message.
    #[must_use]
    pub const fn expected(&self) -> &'static str {
        match self {
            Self::Word => "a word",
            Self::GreedyString => "text",
            // Both parse as an integer; the difference is the accepted width.
            Self::Integer(_) | Self::Long(_) => "an integer",
            Self::Double { .. } => "a number",
            Self::Bool => "true or false",
            Self::Resource => "a resource id",
            Self::BlockPos => "block coordinates",
            Self::PlayerName => "a player name",
        }
    }
}

/// A parsed argument value.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgumentValue {
    /// A word or greedy string.
    String(String),
    /// An integer or long.
    Integer(i64),
    /// A double.
    Double(f64),
    /// A boolean.
    Bool(bool),
    /// A resource id.
    Resource(ResourceId),
    /// A block position, with `None` for an axis left relative (`~`).
    ///
    /// `~` with no offset means "the source's own coordinate", which is *not* the same
    /// as `~0` once the source moves, so the distinction is carried rather than
    /// resolved at parse time.
    BlockPos {
        /// X, or `None` for a bare `~`.
        x: Option<i32>,
        /// Y, or `None` for a bare `~`.
        y: Option<i32>,
        /// Z, or `None` for a bare `~`.
        z: Option<i32>,
    },
}

impl ArgumentValue {
    /// The value as a string, when it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// The value as an integer, when it is one.
    #[must_use]
    pub const fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a double, when it is one.
    #[must_use]
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Double(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a boolean, when it is one.
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value as a resource id, when it is one.
    #[must_use]
    pub const fn as_resource(&self) -> Option<&ResourceId> {
        match self {
            Self::Resource(id) => Some(id),
            _ => None,
        }
    }

    /// A short type name for error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Integer(_) => "integer",
            Self::Double(_) => "double",
            Self::Bool(_) => "boolean",
            Self::Resource(_) => "resource",
            Self::BlockPos { .. } => "block position",
        }
    }
}

/// One declared argument.
///
/// **Not `Eq`** for the same reason as [`ArgumentKind`]: a `Double` range.
#[derive(Debug, Clone, PartialEq)]
pub struct Argument {
    /// The name shown in usage and suggestions.
    pub name: &'static str,
    /// What it accepts.
    pub kind: ArgumentKind,
    /// Whether the command is valid without it.
    ///
    /// An optional argument must be last, or the grammar is ambiguous — enforced by
    /// [`crate::tree::CommandTree::validate`].
    pub optional: bool,
}

impl Argument {
    /// A required argument.
    #[must_use]
    pub const fn required(name: &'static str, kind: ArgumentKind) -> Self {
        Self {
            name,
            kind,
            optional: false,
        }
    }

    /// An optional argument.
    #[must_use]
    pub const fn optional(name: &'static str, kind: ArgumentKind) -> Self {
        Self {
            name,
            kind,
            optional: true,
        }
    }

    /// A required 32-bit integer.
    #[must_use]
    pub const fn integer(name: &'static str) -> Self {
        Self::required(name, ArgumentKind::Integer(ValueRange::i32_range()))
    }

    /// A required integer inside a range.
    #[must_use]
    pub const fn ranged(name: &'static str, min: i64, max: i64) -> Self {
        Self::required(name, ArgumentKind::Integer(ValueRange::new(min, max)))
    }

    /// A required word.
    #[must_use]
    pub const fn word(name: &'static str) -> Self {
        Self::required(name, ArgumentKind::Word)
    }

    /// A required greedy string.
    #[must_use]
    pub const fn greedy(name: &'static str) -> Self {
        Self::required(name, ArgumentKind::GreedyString)
    }
}

/// Split a command string into tokens, honouring quotes and escapes.
///
/// * whitespace separates tokens;
/// * `"` groups a run of characters, and whitespace inside it does not separate;
/// * `\` escapes the next character, inside or outside quotes.
///
/// # Errors
///
/// [`ParseError::TooLong`], [`ParseError::Empty`] or [`ParseError::UnterminatedQuote`].
pub fn tokenize(input: &str) -> Result<Vec<String>, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() > crate::MAX_COMMAND_CHARS {
        return Err(ParseError::TooLong {
            length: chars.len(),
            limit: crate::MAX_COMMAND_CHARS,
        });
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        match character {
            '\\' if index + 1 < chars.len() => {
                // An escape consumes the next character literally. A trailing
                // backslash is kept as itself rather than dropping it.
                current.push(chars[index + 1]);
                has_token = true;
                index += 2;
            }
            '"' => {
                in_quotes = !in_quotes;
                // `""` is an empty token, which is different from no token at all.
                has_token = true;
                index += 1;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
                index += 1;
            }
            other => {
                current.push(other);
                has_token = true;
                index += 1;
            }
        }
    }

    if in_quotes {
        return Err(ParseError::UnterminatedQuote);
    }
    if has_token {
        tokens.push(current);
    }
    if tokens.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(tokens)
}

/// Parse one token against an argument's kind.
///
/// # Errors
///
/// [`ParseError::Invalid`] or [`ParseError::OutOfRange`] when the token does not fit.
pub fn parse_value(argument: &Argument, token: &str) -> Result<ArgumentValue, ParseError> {
    let expected = argument.kind.expected();
    let invalid = |found: &str| ParseError::Invalid {
        name: argument.name,
        expected,
        found: truncate(found, 32),
    };

    match &argument.kind {
        ArgumentKind::Word | ArgumentKind::GreedyString | ArgumentKind::PlayerName => {
            if token.is_empty() {
                return Err(invalid(token));
            }
            Ok(ArgumentValue::String(token.to_owned()))
        }
        ArgumentKind::Integer(range) => {
            let value: i64 = token.parse().map_err(|_| invalid(token))?;
            if !range.contains(value) {
                return Err(ParseError::OutOfRange {
                    name: argument.name,
                    value,
                    min: range.min,
                    max: range.max,
                });
            }
            // The `i32` range is the only one advertised, so this cannot truncate; the
            // check is here so a future wider range is caught rather than wrapping.
            if value < i64::from(i32::MIN) || value > i64::from(i32::MAX) {
                return Err(ParseError::OutOfRange {
                    name: argument.name,
                    value,
                    min: i64::from(i32::MIN),
                    max: i64::from(i32::MAX),
                });
            }
            Ok(ArgumentValue::Integer(value))
        }
        ArgumentKind::Long(range) => {
            let value: i64 = token.parse().map_err(|_| invalid(token))?;
            if !range.contains(value) {
                return Err(ParseError::OutOfRange {
                    name: argument.name,
                    value,
                    min: range.min,
                    max: range.max,
                });
            }
            Ok(ArgumentValue::Integer(value))
        }
        ArgumentKind::Double { min, max } => {
            let value: f64 = token.parse().map_err(|_| invalid(token))?;
            // A NaN or infinity parses successfully but is never a legal coordinate, so
            // it is refused here rather than reaching the world.
            if !value.is_finite() || value < *min || value > *max {
                return Err(invalid(token));
            }
            Ok(ArgumentValue::Double(value))
        }
        ArgumentKind::Bool => match token {
            "true" => Ok(ArgumentValue::Bool(true)),
            "false" => Ok(ArgumentValue::Bool(false)),
            other => Err(invalid(other)),
        },
        ArgumentKind::Resource => {
            let id = ResourceId::parse(token).map_err(|_| invalid(token))?;
            Ok(ArgumentValue::Resource(id))
        }
        ArgumentKind::BlockPos => parse_block_pos(argument, token),
    }
}

/// Parse one axis of a block position: `12`, `~` or `~5`.
///
/// Returns `Ok(None)` for a bare `~` (meaning "the source's own coordinate") and
/// `Ok(Some(offset))` otherwise. A bare number is an absolute coordinate, which is
/// distinguishable from `~0` once the source moves — which is why the two are not
/// collapsed here.
fn parse_axis(token: &str) -> Result<Option<i32>, ()> {
    let Some(offset) = token.strip_prefix('~') else {
        return token.parse::<i32>().map(Some).map_err(|_| ());
    };
    if offset.is_empty() {
        return Ok(None);
    }
    offset.parse::<i32>().map(Some).map_err(|_| ())
}

/// Parse `x y z`, accepting `~` on any axis.
///
/// The grammar is three separate tokens, so `1 2 3` arrives here as `"1 2 3"` — the
/// *dispatcher* joins the next three tokens before calling this, which is why the
/// splitting happens here rather than in the tokenizer.
fn parse_block_pos(argument: &Argument, token: &str) -> Result<ArgumentValue, ParseError> {
    let parts: Vec<&str> = token.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(ParseError::Invalid {
            name: argument.name,
            expected: "block coordinates",
            found: truncate(token, 32),
        });
    }
    let mut axes = [None; 3];
    for (index, part) in parts.iter().enumerate() {
        axes[index] = parse_axis(part).map_err(|()| ParseError::Invalid {
            name: argument.name,
            expected: "block coordinates",
            found: truncate(token, 32),
        })?;
    }
    Ok(ArgumentValue::BlockPos {
        x: axes[0],
        y: axes[1],
        z: axes[2],
    })
}

/// Truncate for an error message, on a character boundary.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// Split a command's root name from its argument text.
///
/// The leading `/` is optional: a client sends a command *without* it in
/// `chat_command`, but a player typing `/say hi` in chat produces one with it, and both
/// must work.
#[must_use]
pub fn split_root(input: &str) -> (&str, &str) {
    let trimmed = input
        .trim_start()
        .strip_prefix('/')
        .unwrap_or(input.trim_start());
    match trimmed.split_once(char::is_whitespace) {
        Some((root, rest)) => (root, rest.trim_start()),
        None => (trimmed, ""),
    }
}

#[cfg(test)]
mod tests;
