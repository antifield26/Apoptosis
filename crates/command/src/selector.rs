//! Entity selectors: `@a`, `@p`, `@r`, `@s`, `@e` with their argument filters
//! (P07-06).
//!
//! ## What a selector is
//!
//! A selector names a **set** of entities rather than one, which is why it cannot be an
//! ordinary argument: the dispatcher produces values, and the size of this value depends on
//! the world. So parsing is here (a pure function of the text) and *resolution* is the
//! caller's — this crate has no entity model.
//!
//! ```text
//! @e[type=minecraft:zombie,distance=..10,sort=nearest,limit=3]
//! @a[gamemode=creative,team=red]
//! @p[level=5..]
//! @r
//! ```
//!
//! ## The measured rules
//!
//! Vanilla's selector syntax is documented and its behaviour is observable, but its
//! *implementation* is a parser with a specific error vocabulary. What is reproduced here
//! is the **syntax** and the meaning of the options; what is not is Vanilla's exact error
//! message strings, because those are not something this project can verify.
//!
//! ## Options modelled, and the ones deliberately not
//!
//! | Option | Modelled | Note |
//! |---|---|---|
//! | `type` | yes | one or more, `!`-negatable |
//! | `name` | yes | one or more, `!`-negatable |
//! | `distance` | yes | `5`, `..5`, `5..`, `5..10` |
//! | `level` | yes | same range syntax, on experience level |
//! | `gamemode` | yes | one or more, `!`-negatable |
//! | `limit` | yes | positive integer |
//! | `sort` | yes | `nearest`, `furthest`, `random`, `arbitrary` |
//! | `x`, `y`, `z` | yes | the centre a `distance` is measured from |
//! | `dx`, `dy`, `dz` | no | a box volume; refused rather than ignored |
//! | `scores`, `tag`, `team`, `nbt`, `predicate`, `advancements`, `level`-as-object | **no** | refused by name |
//! | `rotated`, `type`-as-tag | **no** | refused by name |
//!
//! The rule for the unmodelled ones is the important part: an *unknown* option is an
//! **error naming it**, never a silently-ignored filter. A selector that quietly drops
//! `tag=foo` would select the wrong entities, which is worse than refusing — the same
//! reasoning as `mc-data`'s unmodelled recipe counting, applied to a parse.

use mc_core::ids::ResourceId;

/// A numeric bound with the range syntax Vanilla uses.
///
/// `5` (exactly), `..5` (at most), `5..` (at least), `5..10` (between), `..` (anything).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
    /// Lowest accepted value, or `None` for unbounded.
    pub min: Option<f64>,
    /// Highest accepted value, or `None` for unbounded.
    pub max: Option<f64>,
}

impl Bound {
    /// Exactly a value.
    #[must_use]
    pub const fn exactly(value: f64) -> Self {
        Self {
            min: Some(value),
            max: Some(value),
        }
    }

    /// Whether a value is inside.
    #[must_use]
    pub fn contains(&self, value: f64) -> bool {
        self.min.is_none_or(|min| value >= min) && self.max.is_none_or(|max| value <= max)
    }

    /// Parse the range syntax.
    ///
    /// # Errors
    ///
    /// A message naming what was wrong, so a player sees which part of the selector failed.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err("a bound cannot be empty".to_owned());
        }
        if text == ".." {
            return Ok(Self {
                min: None,
                max: None,
            });
        }
        let Some((low, high)) = text.split_once("..") else {
            let value = parse_number(text)?;
            return Ok(Self::exactly(value));
        };
        let min = if low.is_empty() {
            None
        } else {
            Some(parse_number(low)?)
        };
        let max = if high.is_empty() {
            None
        } else {
            Some(parse_number(high)?)
        };
        if let (Some(min), Some(max)) = (min, max)
            && min > max
        {
            return Err(format!("{text:?} has its minimum above its maximum"));
        }
        Ok(Self { min, max })
    }
}

/// Parse a number, refusing the spellings `f64::parse` accepts but a coordinate cannot be.
fn parse_number(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{text:?} is not a number"))?;
    if !value.is_finite() {
        return Err(format!("{text:?} is not a finite number"));
    }
    Ok(value)
}

/// How a selection is ordered and truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Nearest to the selector's position first. Vanilla's default.
    #[default]
    Nearest,
    /// Furthest first.
    Furthest,
    /// A random order, which needs the caller's RNG.
    Random,
    /// No defined order — Vanilla's "arbitrary", which its own docs call unpredictable.
    Arbitrary,
}

impl Sort {
    /// Parse a `sort=` value.
    ///
    /// # Errors
    ///
    /// A message naming the unknown order.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "nearest" => Ok(Self::Nearest),
            "furthest" => Ok(Self::Furthest),
            "random" => Ok(Self::Random),
            "arbitrary" => Ok(Self::Arbitrary),
            other => Err(format!("unknown sort order {other:?}")),
        }
    }

    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Furthest => "furthest",
            Self::Random => "random",
            Self::Arbitrary => "arbitrary",
        }
    }

    /// Whether this order needs a random number generator.
    #[must_use]
    pub const fn needs_random(self) -> bool {
        matches!(self, Self::Random)
    }
}

/// A game mode, as `gamemode=` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    /// Survival.
    Survival,
    /// Creative.
    Creative,
    /// Adventure.
    Adventure,
    /// Spectator.
    Spectator,
}

impl GameMode {
    /// Parse a game mode name, accepting Vanilla's short forms.
    ///
    /// # Errors
    ///
    /// A message naming the unknown mode.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "survival" | "s" | "0" => Ok(Self::Survival),
            "creative" | "c" | "1" => Ok(Self::Creative),
            "adventure" | "a" | "2" => Ok(Self::Adventure),
            "spectator" | "sp" | "3" => Ok(Self::Spectator),
            other => Err(format!("unknown game mode {other:?}")),
        }
    }

    /// The long name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

/// Which entities a selector names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    /// `@a` — every player.
    AllPlayers,
    /// `@p` — the nearest player.
    NearestPlayer,
    /// `@r` — a random player.
    RandomPlayer,
    /// `@s` — the executing entity.
    SelfEntity,
    /// `@e` — every entity.
    AllEntities,
    /// `@n` — the nearest entity. Added in 1.21; present in 26.1.
    NearestEntity,
}

impl SelectorKind {
    /// The selector's own name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AllPlayers => "@a",
            Self::NearestPlayer => "@p",
            Self::RandomPlayer => "@r",
            Self::SelfEntity => "@s",
            Self::AllEntities => "@e",
            Self::NearestEntity => "@n",
        }
    }

    /// Whether this selector can name more than one entity.
    ///
    /// Used to refuse a `limit` that cannot mean anything: `@s[limit=2]` is a contradiction,
    /// and Vanilla treats it as an error.
    #[must_use]
    pub const fn is_single(self) -> bool {
        matches!(
            self,
            Self::NearestPlayer | Self::RandomPlayer | Self::SelfEntity | Self::NearestEntity
        )
    }

    /// Whether this selector names entities rather than players by default.
    #[must_use]
    pub const fn defaults_to_entities(self) -> bool {
        matches!(self, Self::AllEntities | Self::NearestEntity)
    }
}

/// One entity type filter, with Vanilla's `!` negation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFilter {
    /// The type id.
    pub id: ResourceId,
    /// Whether it is negated (`!minecraft:zombie`).
    pub negated: bool,
}

/// One name filter, with Vanilla's `!` negation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameFilter {
    /// The name.
    pub name: String,
    /// Whether it is negated.
    pub negated: bool,
}

/// A parsed selector.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// Which entities it names.
    pub kind: SelectorKind,
    /// Entity type filters, in the order written.
    pub types: Vec<TypeFilter>,
    /// Name filters.
    pub names: Vec<NameFilter>,
    /// Distance from the centre, in blocks.
    pub distance: Option<Bound>,
    /// Experience level bound.
    pub level: Option<Bound>,
    /// Game modes, with negation.
    pub game_modes: Vec<(GameMode, bool)>,
    /// Largest number of entities to return.
    pub limit: Option<u32>,
    /// The order to return them in.
    pub sort: Sort,
    /// The centre a `distance` is measured from, if the selector overrode the source's.
    pub position: Option<(f64, f64, f64)>,
}

impl Selector {
    /// A selector with no filters.
    #[must_use]
    pub const fn new(kind: SelectorKind) -> Self {
        Self {
            kind,
            types: Vec::new(),
            names: Vec::new(),
            distance: None,
            level: None,
            game_modes: Vec::new(),
            limit: None,
            sort: Sort::Nearest,
            position: None,
        }
    }

    /// Whether this selector names **only** players.
    ///
    /// `@a`, `@p`, `@r` and `@s` do; `@e` and `@n` span every entity, players included
    /// unless a `type` filter excludes them. This is precisely the rule
    /// [`Selector::matches`] enforces, so the method and the predicate agree by
    /// construction.
    ///
    /// **The predecessor of this method was `includes_players`**, which asked whether a
    /// selector could match a player — a question with only one answer, since `@a` can (it
    /// is player-only) and `@e` can (a player *is* an entity). Its first version was
    /// literally `… || true`, caught by clippy; rewriting it to the inverse was no better,
    /// because a method with one possible answer is not a method. The test written against
    /// it is what exposed that, which is the argument for asserting a helper's two
    /// outcomes rather than one.
    #[must_use]
    pub const fn is_player_only(&self) -> bool {
        !matches!(
            self.kind,
            SelectorKind::AllEntities | SelectorKind::NearestEntity
        )
    }

    /// The maximum number of entities this selector may return, from its kind and `limit`.
    ///
    /// `None` means unbounded. This is what the *caller* needs to bound its own work: a
    /// selector with no limit over a large world can match everything, so resolution has to
    /// be told how many it may return rather than discovering it.
    #[must_use]
    pub fn effective_limit(&self) -> Option<u32> {
        match (self.limit, self.kind) {
            (Some(limit), _) => Some(limit),
            // Every single-entity selector has an implied limit of 1; only `@a` and `@e`
            // are unbounded when the command writer said nothing.
            (
                None,
                SelectorKind::NearestPlayer
                | SelectorKind::NearestEntity
                | SelectorKind::SelfEntity
                | SelectorKind::RandomPlayer,
            ) => Some(1),
            (None, SelectorKind::AllPlayers | SelectorKind::AllEntities) => None,
        }
    }
}

/// Why a selector could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorError {
    /// The text did not begin with `@`.
    NotASelector(String),
    /// The character after `@` was not a known selector letter.
    UnknownSelector(char),
    /// A `[` was not closed.
    UnterminatedOptions,
    /// An option had no `=`.
    MalformedOption(String),
    /// An option this build does not model, named so the caller can see which.
    UnsupportedOption(String),
    /// An option was well formed but its value was wrong.
    BadValue {
        /// The option.
        option: String,
        /// What was wrong with the value.
        reason: String,
    },
    /// The same option was given twice, where Vanilla takes one.
    DuplicateOption(String),
    /// A `limit` on a selector that can only name one entity.
    LimitOnSingleEntity(String),
    /// A `[` with nothing in it, or a trailing comma.
    EmptyOption,
}

impl std::fmt::Display for SelectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotASelector(text) => write!(f, "{text:?} is not a selector"),
            Self::UnknownSelector(c) => write!(f, "@{c} is not a selector"),
            Self::UnterminatedOptions => write!(f, "a selector's [ is never closed"),
            Self::MalformedOption(text) => write!(f, "option {text:?} has no value"),
            Self::UnsupportedOption(name) => {
                write!(f, "{name}= is not supported by this build")
            }
            Self::BadValue { option, reason } => write!(f, "{option}=: {reason}"),
            Self::DuplicateOption(name) => write!(f, "{name}= is given twice"),
            Self::LimitOnSingleEntity(select) => {
                write!(f, "{select} names one entity, so limit= is a contradiction")
            }
            Self::EmptyOption => write!(f, "a selector has an empty option"),
        }
    }
}

impl std::error::Error for SelectorError {}

/// Options this build recognises but does not implement.
///
/// Named rather than merely rejected, because the distinction matters to a player: "this
/// server does not support `tag=`" is actionable, "invalid selector" is not.
pub const UNSUPPORTED_OPTIONS: &[&str] = &[
    "advancements",
    "dx",
    "dy",
    "dz",
    "nbt",
    "predicate",
    "scores",
    "tag",
    "team",
    "x_rotation",
    "y_rotation",
];

/// Parse a selector.
///
/// # Errors
///
/// See [`SelectorError`]. An **unknown** option is `UnsupportedOption` rather than being
/// ignored: a silently-dropped filter selects the wrong entities.
pub fn parse(text: &str) -> Result<Selector, SelectorError> {
    let mut chars = text.chars();
    if chars.next() != Some('@') {
        return Err(SelectorError::NotASelector(text.to_owned()));
    }
    let Some(letter) = chars.next() else {
        return Err(SelectorError::NotASelector(text.to_owned()));
    };
    let kind = match letter {
        'a' => SelectorKind::AllPlayers,
        'p' => SelectorKind::NearestPlayer,
        'r' => SelectorKind::RandomPlayer,
        's' => SelectorKind::SelfEntity,
        'e' => SelectorKind::AllEntities,
        'n' => SelectorKind::NearestEntity,
        other => return Err(SelectorError::UnknownSelector(other)),
    };

    let rest: String = chars.collect();
    let mut selector = Selector::new(kind);
    if rest.is_empty() {
        return Ok(selector);
    }
    let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) else {
        return Err(SelectorError::UnterminatedOptions);
    };
    if inner.trim().is_empty() {
        return Err(SelectorError::EmptyOption);
    }

    // Track which single-valued options were seen, so a repeat is an error rather than a
    // silent last-wins.
    // Owned keys: they are compared against a fixed list but come from the input, so a
    // `&'static str` would not be honest about where they come from.
    let mut seen: Vec<String> = Vec::new();
    for option in split_options(inner) {
        let Some((key, value)) = option.split_once('=') else {
            return Err(SelectorError::MalformedOption(option.clone()));
        };
        let key = key.trim();
        if UNSUPPORTED_OPTIONS.contains(&key) {
            return Err(SelectorError::UnsupportedOption(key.to_owned()));
        }
        if matches!(
            key,
            "distance" | "level" | "limit" | "sort" | "x" | "y" | "z"
        ) {
            if seen.iter().any(|earlier| earlier == key) {
                return Err(SelectorError::DuplicateOption(key.to_owned()));
            }
            seen.push(key.to_owned());
        }
        apply_option(&mut selector, key, value)?;
    }

    if selector.kind.is_single() && selector.limit.is_some_and(|limit| limit != 1) {
        return Err(SelectorError::LimitOnSingleEntity(
            selector.kind.name().to_owned(),
        ));
    }
    Ok(selector)
}

/// Split an option list on commas, keeping `[]` groups together.
///
/// No modelled option takes a nested list, but splitting one naively would mangle a future
/// one, and a `,` inside an unsupported option's value must not become two options — the
/// unsupported one is what the caller needs named.
fn split_options(inner: &str) -> Vec<String> {
    let mut options = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for character in inner.chars() {
        match character {
            '[' | '{' => {
                depth += 1;
                current.push(character);
            }
            ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                options.push(current.trim().to_owned());
                current.clear();
            }
            other => current.push(other),
        }
    }
    options.push(current.trim().to_owned());
    options
}

/// Apply one option to the selector under construction.
fn apply_option(selector: &mut Selector, key: &str, value: &str) -> Result<(), SelectorError> {
    let bad = |reason: String| SelectorError::BadValue {
        option: key.to_owned(),
        reason,
    };
    match key {
        "type" => {
            // Accumulating, because `type=!a,type=b` is legal and Vanilla merges them.
            for entry in value.split(',') {
                let (negated, name) = strip_negation(entry);
                let id =
                    ResourceId::parse(name).map_err(|_| bad(format!("{name:?} is not an id")))?;
                selector.types.push(TypeFilter { id, negated });
            }
        }
        "name" => {
            for entry in value.split(',') {
                let (negated, name) = strip_negation(entry);
                if name.is_empty() {
                    return Err(bad("a name cannot be empty".to_owned()));
                }
                selector.names.push(NameFilter {
                    name: name.to_owned(),
                    negated,
                });
            }
        }
        "distance" => selector.distance = Some(Bound::parse(value).map_err(bad)?),
        "level" => selector.level = Some(Bound::parse(value).map_err(bad)?),
        "gamemode" | "m" => {
            for entry in value.split(',') {
                let (negated, name) = strip_negation(entry);
                let mode = GameMode::parse(name).map_err(bad)?;
                selector.game_modes.push((mode, negated));
            }
        }
        "limit" => {
            let limit: u32 = value
                .parse()
                .map_err(|_| bad(format!("{value:?} is not a count")))?;
            if limit == 0 {
                return Err(bad("a limit of 0 can never be satisfied".to_owned()));
            }
            selector.limit = Some(limit);
        }
        "sort" => selector.sort = Sort::parse(value).map_err(bad)?,
        "x" | "y" | "z" => {
            let number = parse_number(value).map_err(bad)?;
            let (x, y, z) = selector.position.unwrap_or((0.0, 0.0, 0.0));
            selector.position = Some(match key {
                "x" => (number, y, z),
                "y" => (x, number, z),
                _ => (x, y, number),
            });
        }
        other => return Err(SelectorError::UnsupportedOption(other.to_owned())),
    }
    Ok(())
}

/// Strip a leading `!`.
fn strip_negation(text: &str) -> (bool, &str) {
    match text.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

/// Whether an entity passes a selector's filters, given the facts about it.
///
/// The predicate lives here rather than in a caller because it is the *meaning* of the
/// parsed options, and duplicating it in a caller is how two implementations of one rule
/// drift apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityFacts<'a> {
    /// The entity's type id.
    pub type_id: &'a str,
    /// Its name, for a player; `None` for a non-player entity.
    pub name: Option<&'a str>,
    /// Its position.
    pub position: (f64, f64, f64),
    /// Its experience level, if it has one.
    pub level: Option<u32>,
    /// Its game mode, if it has one.
    pub game_mode: Option<GameMode>,
    /// Whether it is a player.
    pub is_player: bool,
}

impl Selector {
    /// How many filters the selector carries, for a caller's cost estimate.
    #[must_use]
    pub fn filter_count(&self) -> usize {
        self.types.len()
            + self.names.len()
            + usize::from(self.distance.is_some())
            + usize::from(self.level.is_some())
            + self.game_modes.len()
    }

    /// Whether an entity passes every filter.
    ///
    /// The `distance` bound is measured from `centre` — which the caller computes, because
    /// "the selector's position" is the source's position unless `x`/`y`/`z` overrode it,
    /// and this crate has no source.
    #[must_use]
    pub fn matches(&self, facts: &EntityFacts<'_>, centre: (f64, f64, f64)) -> bool {
        // A player-only selector never matches a non-player, and vice versa for `@a`.
        match self.kind {
            SelectorKind::AllPlayers
            | SelectorKind::NearestPlayer
            | SelectorKind::RandomPlayer
            | SelectorKind::SelfEntity
                if !facts.is_player =>
            {
                return false;
            }
            _ => {}
        }

        // Type filters: every **positive** filter must match at least one id, and no
        // negated one may match. That is Vanilla's rule and it is not the same as
        // "any filter matches".
        let positives: Vec<&TypeFilter> = self.types.iter().filter(|f| !f.negated).collect();
        if !positives.is_empty()
            && !positives
                .iter()
                .any(|filter| type_matches(&filter.id, facts.type_id))
        {
            return false;
        }
        if self
            .types
            .iter()
            .any(|filter| filter.negated && type_matches(&filter.id, facts.type_id))
        {
            return false;
        }

        if !self.names.is_empty() {
            let name = facts.name.unwrap_or("");
            let positives: Vec<&NameFilter> = self.names.iter().filter(|f| !f.negated).collect();
            if !positives.is_empty() && !positives.iter().any(|filter| filter.name == name) {
                return false;
            }
            if self
                .names
                .iter()
                .any(|filter| filter.negated && filter.name == name)
            {
                return false;
            }
        }

        if let Some(bound) = self.distance {
            let (x, y, z) = facts.position;
            let distance =
                ((x - centre.0).powi(2) + (y - centre.1).powi(2) + (z - centre.2).powi(2)).sqrt();
            if !bound.contains(distance) {
                return false;
            }
        }

        if let Some(bound) = self.level {
            // A non-player has no experience level, so a level filter excludes it. Vanilla
            // does the same: `@e[level=..]` matches only entities that have a level.
            let Some(level) = facts.level else {
                return false;
            };
            if !bound.contains(f64::from(level)) {
                return false;
            }
        }

        if !self.game_modes.is_empty() {
            let positives: Vec<&(GameMode, bool)> =
                self.game_modes.iter().filter(|(_, neg)| !neg).collect();
            match facts.game_mode {
                Some(mode) => {
                    if !positives.is_empty() && !positives.iter().any(|(want, _)| *want == mode) {
                        return false;
                    }
                    if self
                        .game_modes
                        .iter()
                        .any(|(want, neg)| *neg && *want == mode)
                    {
                        return false;
                    }
                }
                // A non-player has no game mode; only an all-negated filter lets it pass.
                None => {
                    if !positives.is_empty() {
                        return false;
                    }
                }
            }
        }

        true
    }
}

/// Whether a filter id matches an entity's type.
///
/// A filter with a path matches that entity type; a filter whose path is a **tag-like**
/// group is not supported, so this compares full ids. Vanilla also accepts `#tag` here,
/// which would need a tag lookup — refused at parse time by `ResourceId::parse` rejecting
/// the leading `#`.
fn type_matches(filter: &ResourceId, type_id: &str) -> bool {
    format!("{}:{}", filter.namespace(), filter.value()) == type_id
}

#[cfg(test)]
mod tests;
