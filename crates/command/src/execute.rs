//! `/execute`: a chain of context modifiers ending in `run <command>` (P07-07).
//!
//! ## Why this is not a Brigadier sub-tree
//!
//! Vanilla's `execute` is the one command that genuinely needs a **branching** graph: a node
//! with a dozen children (`as`, `at`, `positioned`, `if`, `unless`, …), each of which has its
//! own children, and any of them can be followed by any other. That is what makes Brigadier's
//! parse state mutable.
//!
//! But the shape it branches into is **regular**: `execute` is a sequence of `keyword
//! arguments` pairs, in any order and any number of times, terminated by `run <command>`. A
//! loop parses that, so the flat tree does not need branches after all — which is a better
//! outcome than adding a graph parser for one command. The crate docs' claim that branching is
//! unsupported stands, and this is why it does not need to change.
//!
//! ## What is modelled, and what is refused by name
//!
//! | Modifier | Modelled | Note |
//! |---|---|---|
//! | `as <selector>` | yes | replaces who the command runs as |
//! | `at <selector>` | yes | replaces position, rotation and dimension |
//! | `positioned <x y z>` | yes | replaces position only |
//! | `align <axes>` | yes | truncates the named axes to whole blocks |
//! | `rotated as <selector>` / `rotated <yaw> <pitch>` | yes | replaces rotation |
//! | `facing <x y z>` / `facing entity <selector> [eyes\|feet]` | yes | sets rotation to look at a point |
//! | `anchored <eyes\|feet>` | yes | sets the anchor `facing`/local coords measure from |
//! | `if entity <selector>` | yes | true when the selector matches anything |
//! | `unless entity <selector>` | yes | the negation |
//! | `if block <pos> <block>` | yes | true when the block matches |
//! | `unless block <pos> <block>` | yes | the negation |
//! | `in`, `on` | **no** | refused by name |
//! | `store …` | **no** | refused by name |
//! | `if`/`unless` with `data`/`score`/`predicate`/`biome`/`loaded`/`blocks`/`function` | **no** | refused by name |
//!
//! The rule for the unmodelled ones is the same one the selector and data loaders use: an
//! unknown or unimplemented construct is an **error naming it**, never silently ignored. A
//! dropped `store` would send output to the chat instead of a block; a dropped `if` would run
//! a command that should not have run.
//!
//! ## Resolution is the caller's
//!
//! A parsed chain says *what* to change, not what the result is: `as @a` needs the entity
//! store, `if block` needs the world. So [`ExecuteChain`] carries the modifiers and the inner
//! command text, and the server resolves and re-dispatches. That keeps this crate testable
//! with no world at all, which is where the parser's hostile-input tests live.

use crate::selector::{self, Selector};
use thiserror::Error;

/// Longest chain of modifiers accepted.
///
/// Vanilla has no explicit bound; a chain longer than this is either a machine-generated
/// mistake or an attempt to make the server do unbounded work per command. 64 is far beyond
/// any hand-written command and keeps the modifier vector's size a property of the grammar.
pub const MAX_MODIFIERS: usize = 64;

/// Three axes, each optionally relative.
///
/// `None` means a bare `~` — "wherever the source is" — which is deliberately different from
/// `~0` once the source moves. Named so a signature says "three coordinates" rather than
/// spelling out three `Option<i32>`s and leaving the reader to count them.
pub type Coordinates = (Option<i32>, Option<i32>, Option<i32>);

/// One context modifier.
#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    /// `as <selector>` — who the command runs as.
    As(Selector),
    /// `at <selector>` — position, rotation and dimension from a matched entity.
    At(Selector),
    /// `positioned <x y z>` — position only.
    Positioned {
        /// X, or `None` for a bare `~`.
        x: Option<i32>,
        /// Y, or `None` for a bare `~`.
        y: Option<i32>,
        /// Z, or `None` for a bare `~`.
        z: Option<i32>,
    },
    /// `align <axes>` — truncate the named axes.
    Align(Axes),
    /// `rotated as <selector>` or `rotated <yaw> <pitch>`.
    Rotated(RotationSource),
    /// `facing <x y z>` or `facing entity <selector> [eyes|feet]`.
    Facing(FacingTarget),
    /// `anchored <eyes|feet>`.
    Anchored(Anchor),
    /// `if <condition>` — run only when it holds.
    If(Condition),
    /// `unless <condition>` — run only when it does not hold.
    Unless(Condition),
}

/// Where `rotated` takes its angles from.
#[derive(Debug, Clone, PartialEq)]
pub enum RotationSource {
    /// `rotated as <selector>` — the first match's rotation.
    As(Box<Selector>),
    /// `rotated <yaw> <pitch>` — absolute degrees, or `None` per axis for a bare `~`.
    Fixed {
        /// Yaw in degrees, or `None` for a bare `~`.
        yaw: Option<f64>,
        /// Pitch in degrees, or `None` for a bare `~`.
        pitch: Option<f64>,
    },
}

/// What `facing` looks at.
#[derive(Debug, Clone, PartialEq)]
pub enum FacingTarget {
    /// `facing <x y z>` — a world point.
    Position {
        /// X, or `None` for a bare `~`.
        x: Option<i32>,
        /// Y, or `None` for a bare `~`.
        y: Option<i32>,
        /// Z, or `None` for a bare `~`.
        z: Option<i32>,
    },
    /// `facing entity <selector> [eyes|feet]`.
    Entity {
        /// Whose position to look at.
        selector: Box<Selector>,
        /// Which point of that entity; defaults to `eyes` like Vanilla.
        anchor: Anchor,
    },
}

/// The point of an entity that `facing entity` / `anchored` measure from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// The feet (entity origin).
    #[default]
    Feet,
    /// The eyes (feet + eye height).
    Eyes,
}

impl Anchor {
    /// Parse `eyes` or `feet`.
    ///
    /// # Errors
    ///
    /// A message naming the unknown word.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "eyes" => Ok(Self::Eyes),
            "feet" => Ok(Self::Feet),
            other => Err(format!("{other:?} is not an anchor (eyes|feet)")),
        }
    }

    /// The wire/keyword name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Eyes => "eyes",
            Self::Feet => "feet",
        }
    }
}

/// Which axes `align` truncates.
///
/// A swizzle: `align xyz` truncates all three, `align xz` leaves y alone. Stored as three flags
/// rather than a string so an invalid swizzle cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Axes {
    /// Truncate x.
    pub x: bool,
    /// Truncate y.
    pub y: bool,
    /// Truncate z.
    pub z: bool,
}

impl Axes {
    /// Parse a swizzle like `xyz`, `xz` or `y`.
    ///
    /// # Errors
    ///
    /// A message when the text is empty, has a character that is not an axis, or repeats one.
    /// A repeated axis is refused rather than tolerated: `align xxy` is a typo, and silently
    /// treating it as `xy` would hide it.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err("align needs at least one axis".to_owned());
        }
        let mut axes = Self::default();
        for character in text.chars() {
            let flag = match character {
                'x' => &mut axes.x,
                'y' => &mut axes.y,
                'z' => &mut axes.z,
                other => return Err(format!("{other:?} is not an axis")),
            };
            if *flag {
                return Err(format!("axis {character:?} is given twice"));
            }
            *flag = true;
        }
        Ok(axes)
    }

    /// How many axes are selected.
    #[must_use]
    pub const fn count(self) -> usize {
        self.x as usize + self.y as usize + self.z as usize
    }

    /// Whether no axis is selected, which cannot happen through [`Axes::parse`].
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.x && !self.y && !self.z
    }
}

/// A test that decides whether the chain continues.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// `entity <selector>` — the selector matches at least one entity.
    Entity(Selector),
    /// `block <pos> <block>` — the block at a position is the named one.
    Block {
        /// X, or `None` for a bare `~`.
        x: Option<i32>,
        /// Y, or `None` for a bare `~`.
        y: Option<i32>,
        /// Z, or `None` for a bare `~`.
        z: Option<i32>,
        /// The block's id, resolved by the caller.
        block: mc_core::ids::ResourceId,
    },
}

/// A parsed `execute` chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecuteChain {
    /// The modifiers, in the order written, so a caller applies them in that order.
    ///
    /// Order matters and is preserved rather than normalised: `as @a at @s` and `at @s as @a`
    /// are different commands in Vanilla.
    pub modifiers: Vec<Modifier>,
    /// The command text after `run`, untouched.
    pub run: String,
}

impl ExecuteChain {
    /// Whether the chain changes who the command runs as.
    #[must_use]
    pub fn has_as(&self) -> bool {
        self.modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifier::As(_)))
    }

    /// Whether the chain changes where the command runs.
    #[must_use]
    pub fn has_position_modifier(&self) -> bool {
        self.modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::At(_)
                    | Modifier::Positioned { .. }
                    | Modifier::Align(_)
                    | Modifier::Facing(_)
            )
        })
    }

    /// Whether the chain changes the executing rotation or anchor.
    #[must_use]
    pub fn has_rotation_modifier(&self) -> bool {
        self.modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifier::Rotated(_) | Modifier::Anchored(_)))
    }

    /// How many conditions the chain carries.
    #[must_use]
    pub fn condition_count(&self) -> usize {
        self.modifiers
            .iter()
            .filter(|modifier| matches!(modifier, Modifier::If(_) | Modifier::Unless(_)))
            .count()
    }
}

/// Why an `execute` chain could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecuteError {
    /// `run` was never reached, so there is no command to execute.
    #[error("an execute chain must end with `run <command>`")]
    MissingRun,
    /// `run` was given with nothing after it.
    #[error("`run` needs a command after it")]
    EmptyRun,
    /// A modifier keyword this build does not model, named so the caller can see which.
    #[error("the `{0}` modifier is not supported by this build")]
    UnsupportedModifier(String),
    /// An `if`/`unless` condition this build does not model.
    #[error("the `{0}` condition is not supported by this build")]
    UnsupportedCondition(String),
    /// A modifier was missing its arguments.
    #[error("`{modifier}` is missing an argument")]
    MissingArgument {
        /// The modifier.
        modifier: String,
    },
    /// A modifier's argument was malformed.
    #[error("`{modifier}`: {reason}")]
    BadValue {
        /// The modifier.
        modifier: String,
        /// What was wrong.
        reason: String,
    },
    /// The chain is longer than [`MAX_MODIFIERS`].
    #[error("an execute chain has at most {limit} modifiers, found {count}")]
    TooLong {
        /// How many modifiers were written.
        count: usize,
        /// The ceiling.
        limit: usize,
    },
}

/// Modifier keywords this build recognises but does not implement.
///
/// Named rather than merely rejected, because the distinction matters to a player: "this
/// server does not support `store`" is actionable, "invalid command" is not.
pub const UNSUPPORTED_MODIFIERS: &[&str] = &["in", "on", "store", "summon"];

/// Condition keywords this build recognises but does not implement.
pub const UNSUPPORTED_CONDITIONS: &[&str] = &[
    "biome",
    "blocks",
    "data",
    "dimension",
    "function",
    "loaded",
    "predicate",
    "score",
];

/// Parse the text after `execute`.
///
/// The tokens are the already-tokenized remainder of the command, which is why this takes a
/// slice rather than a string: `run` must hand the *original* argument text to the inner
/// command, and re-joining tokens would lose a quoted string's grouping.
///
/// # Errors
///
/// See [`ExecuteError`]. An unknown modifier is `UnsupportedModifier` rather than ignored.
pub fn parse(tokens: &[String]) -> Result<ExecuteChain, ExecuteError> {
    let mut modifiers: Vec<Modifier> = Vec::new();
    let mut cursor = 0usize;

    loop {
        let Some(keyword) = tokens.get(cursor) else {
            // Ran out of tokens without `run`.
            return Err(ExecuteError::MissingRun);
        };
        if keyword == "run" {
            if cursor + 1 >= tokens.len() {
                return Err(ExecuteError::EmptyRun);
            }
            // The remainder is rejoined: quote grouping is already lost by tokenizing, and the
            // caller re-parses the inner command through the same tokenizer, so a space-join is
            // the correct round trip for every command this build has.
            return Ok(ExecuteChain {
                modifiers,
                run: tokens[cursor + 1..].join(" "),
            });
        }
        if UNSUPPORTED_MODIFIERS.contains(&keyword.as_str()) {
            return Err(ExecuteError::UnsupportedModifier(keyword.clone()));
        }
        if modifiers.len() >= MAX_MODIFIERS {
            return Err(ExecuteError::TooLong {
                count: modifiers.len() + 1,
                limit: MAX_MODIFIERS,
            });
        }

        let (modifier, consumed) = parse_modifier(keyword, &tokens[cursor..])?;
        modifiers.push(modifier);
        cursor += consumed;
    }
}

/// Parse one modifier, returning it and how many tokens it consumed.
fn parse_modifier(keyword: &str, rest: &[String]) -> Result<(Modifier, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };

    match keyword {
        "as" | "at" => {
            let text = rest.get(1).ok_or_else(missing)?;
            let selector = selector::parse(text).map_err(|error| bad(error.to_string()))?;
            let modifier = if keyword == "as" {
                Modifier::As(selector)
            } else {
                Modifier::At(selector)
            };
            Ok((modifier, 2))
        }
        "positioned" => {
            let ((x, y, z), consumed) = parse_coordinates(keyword, rest)?;
            Ok((Modifier::Positioned { x, y, z }, consumed))
        }
        "align" => {
            let text = rest.get(1).ok_or_else(missing)?;
            let axes = Axes::parse(text).map_err(bad)?;
            Ok((Modifier::Align(axes), 2))
        }
        "rotated" => {
            let (source, consumed) = parse_rotated(keyword, rest)?;
            Ok((Modifier::Rotated(source), consumed))
        }
        "facing" => {
            let (target, consumed) = parse_facing(keyword, rest)?;
            Ok((Modifier::Facing(target), consumed))
        }
        "anchored" => {
            let text = rest.get(1).ok_or_else(missing)?;
            let anchor = Anchor::parse(text).map_err(bad)?;
            Ok((Modifier::Anchored(anchor), 2))
        }
        "if" | "unless" => {
            let (condition, consumed) = parse_condition(keyword, rest)?;
            let modifier = if keyword == "if" {
                Modifier::If(condition)
            } else {
                Modifier::Unless(condition)
            };
            Ok((modifier, consumed + 1))
        }
        other => Err(ExecuteError::UnsupportedModifier(other.to_owned())),
    }
}

/// Parse `rotated as <selector>` or `rotated <yaw> <pitch>`, returning the
/// source and how many tokens it consumed.
fn parse_rotated(keyword: &str, rest: &[String]) -> Result<(RotationSource, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };
    let Some(first) = rest.get(1) else {
        return Err(missing());
    };
    if first == "as" {
        let text = rest.get(2).ok_or_else(missing)?;
        let selector = selector::parse(text).map_err(|error| bad(error.to_string()))?;
        return Ok((RotationSource::As(Box::new(selector)), 3));
    }
    let pitch_text = rest.get(2).ok_or_else(missing)?;
    let yaw = angle(first, &bad)?;
    let pitch = angle(pitch_text, &bad)?;
    Ok((RotationSource::Fixed { yaw, pitch }, 3))
}

/// Parse one rotation angle: a number, `~`, or `~offset`.
fn angle(text: &str, bad: &dyn Fn(String) -> ExecuteError) -> Result<Option<f64>, ExecuteError> {
    let Some(value) = text.strip_prefix('~') else {
        let parsed: f64 = text
            .parse()
            .map_err(|_| bad(format!("{text:?} is not an angle")))?;
        if !parsed.is_finite() {
            return Err(bad(format!("{text:?} is not a finite angle")));
        }
        return Ok(Some(parsed));
    };
    if value.is_empty() {
        return Ok(None);
    }
    let parsed: f64 = value
        .parse()
        .map_err(|_| bad(format!("{text:?} is not an offset")))?;
    if !parsed.is_finite() {
        return Err(bad(format!("{text:?} is not a finite offset")));
    }
    Ok(Some(parsed))
}

/// Parse `facing <x y z>` or `facing entity <selector> [eyes|feet]`.
fn parse_facing(keyword: &str, rest: &[String]) -> Result<(FacingTarget, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };
    let Some(first) = rest.get(1) else {
        return Err(missing());
    };
    if first == "entity" {
        let text = rest.get(2).ok_or_else(missing)?;
        let selector = selector::parse(text).map_err(|error| bad(error.to_string()))?;
        // An optional anchor word, defaulting to Vanilla's `eyes`.
        let mut consumed = 3;
        let mut anchor = Anchor::Eyes;
        if let Some(word) = rest.get(3)
            && (word == "eyes" || word == "feet")
        {
            anchor = Anchor::parse(word).map_err(bad)?;
            consumed = 4;
        }
        return Ok((
            FacingTarget::Entity {
                selector: Box::new(selector),
                anchor,
            },
            consumed,
        ));
    }
    let ((x, y, z), coordinate_tokens) = parse_coordinates_at(keyword, rest, 1)?;
    // `facing` itself is one token on top of the three coordinates.
    Ok((FacingTarget::Position { x, y, z }, 1 + coordinate_tokens))
}

/// Parse one axis: a number, `~`, or `~offset`.
///
/// Shared by `positioned` and `if block`, which is the point: the two had **identical copies** of
/// this closure, and two copies of a parse rule is how two implementations of one rule drift
/// apart.
fn axis(text: &str, bad: &dyn Fn(String) -> ExecuteError) -> Result<Option<i32>, ExecuteError> {
    let Some(value) = text.strip_prefix('~') else {
        return text
            .parse::<i32>()
            .map(Some)
            .map_err(|_| bad(format!("{text:?} is not a coordinate")));
    };
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i32>()
        .map(Some)
        .map_err(|_| bad(format!("{text:?} is not an offset")))
}

/// Parse three coordinates, returning them and how many tokens were consumed.
fn parse_coordinates(keyword: &str, rest: &[String]) -> Result<(Coordinates, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };
    let x = rest.get(1).ok_or_else(missing)?;
    let y = rest.get(2).ok_or_else(missing)?;
    let z = rest.get(3).ok_or_else(missing)?;
    let coordinates: Coordinates = (axis(x, &bad)?, axis(y, &bad)?, axis(z, &bad)?);
    Ok((coordinates, 4))
}

/// Parse an `if`/`unless` condition, returning it and how many tokens **after** the keyword it
/// consumed (the keyword itself is counted by the caller).
fn parse_condition(keyword: &str, rest: &[String]) -> Result<(Condition, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };
    let Some(name) = rest.get(1) else {
        return Err(missing());
    };

    if UNSUPPORTED_CONDITIONS.contains(&name.as_str()) {
        return Err(ExecuteError::UnsupportedCondition(name.clone()));
    }

    match name.as_str() {
        "entity" => {
            let text = rest.get(2).ok_or_else(missing)?;
            let selector = selector::parse(text).map_err(|error| bad(error.to_string()))?;
            Ok((Condition::Entity(selector), 2))
        }
        "block" => {
            // `block <x> <y> <z> <block>`
            let ((x, y, z), coordinate_tokens) = parse_coordinates_at(keyword, rest, 2)?;
            let block_text = rest
                .get(2 + coordinate_tokens)
                .ok_or_else(|| bad("`block` needs a block id".to_owned()))?;
            let block = mc_core::ids::ResourceId::parse(block_text)
                .map_err(|_| bad(format!("{block_text:?} is not a block id")))?;
            Ok((
                Condition::Block { x, y, z, block },
                1 + coordinate_tokens + 1,
            ))
        }
        other => Err(ExecuteError::UnsupportedCondition(other.to_owned())),
    }
}

/// Parse three coordinates starting at `offset`.
fn parse_coordinates_at(
    keyword: &str,
    rest: &[String],
    offset: usize,
) -> Result<(Coordinates, usize), ExecuteError> {
    let missing = || ExecuteError::MissingArgument {
        modifier: keyword.to_owned(),
    };
    let bad = |reason: String| ExecuteError::BadValue {
        modifier: keyword.to_owned(),
        reason,
    };
    let x = rest.get(offset).ok_or_else(missing)?;
    let y = rest.get(offset + 1).ok_or_else(missing)?;
    let z = rest.get(offset + 2).ok_or_else(missing)?;
    let coordinates: Coordinates = (axis(x, &bad)?, axis(y, &bad)?, axis(z, &bad)?);
    Ok((coordinates, 3))
}

/// Resolve three optional coordinates against a source position.
///
/// `None` means a bare `~` — "wherever the source is" — which is deliberately different from
/// `~0` once the source moves. Shared by `positioned` and `if block` so the two cannot drift.
#[must_use]
pub fn resolve_coordinates(
    x: Option<i32>,
    y: Option<i32>,
    z: Option<i32>,
    source: (i32, i32, i32),
) -> (i32, i32, i32) {
    (
        x.unwrap_or(source.0),
        y.unwrap_or(source.1),
        z.unwrap_or(source.2),
    )
}

/// Apply `align` to a position: truncate the named axes toward negative infinity.
///
/// Vanilla aligns by *floor*, not by truncation toward zero, so `align x` on x = -0.5 gives -1
/// and not 0. The distinction only shows at negative coordinates, which is exactly where a
/// silent mistake would live.
#[must_use]
pub fn align(position: (f64, f64, f64), axes: Axes) -> (f64, f64, f64) {
    let align_one = |value: f64, selected: bool| if selected { value.floor() } else { value };
    (
        align_one(position.0, axes.x),
        align_one(position.1, axes.y),
        align_one(position.2, axes.z),
    )
}

#[cfg(test)]
mod tests;
