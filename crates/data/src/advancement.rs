//! Advancements: the data model, the tree and the loader (P07-11).
//!
//! ## Nothing here grants an advancement, and that is deliberate
//!
//! An advancement's `criteria` are evaluated by the whole game: `minecraft:inventory_changed`
//! needs the player's inventory, `minecraft:player_killed_entity` needs a damage source,
//! `minecraft:location` needs a position in a biome. `mc-data` has none of that, and a
//! criteria engine that guessed would mark advancements complete that vanilla does not.
//!
//! So this module **represents** criteria and does not evaluate them. There is no
//! `is_complete`, no `check`, no `grant` — not because they were forgotten, but because a
//! stub there would be a lie (AGENTS.md §3.3). What it does own is everything that is a
//! property of the *data*: the tree, the links, the triggers named, the rewards, and the
//! three hazards a real pack can contain (a missing parent, a cycle, a duplicate id).
//!
//! ## The format, surveyed from the 26.1.2 jar
//!
//! `data/minecraft/advancement/` holds **1 617 files**, and every one was read while this
//! module was written. Survey: `target/vanilla-26.1.2/survey_loot_adv.py`.
//!
//! | Fact | Count |
//! |---|---|
//! | files | 1 617 |
//! | with `parent` | 1 611 |
//! | roots (no `parent`) | **6** |
//! | with `display` | 125 |
//! | with `rewards` | 1 514 |
//! | with `sends_telemetry_event` | 125 |
//! | distinct `trigger` strings | **54** |
//! | criteria entries | 3 546 |
//! | criteria carrying `conditions` | 3 531 |
//!
//! ### Hazards, checked against vanilla
//!
//! | Hazard | Vanilla 26.1.2 |
//! |---|---|
//! | missing parent | **0** |
//! | cycle | **0** |
//! | duplicate id | **0** (impossible in one namespace anyway: a duplicate path is one file) |
//!
//! All three are therefore exercised only by this crate's tests — which is exactly why
//! [`AdvancementLoadReport::check`] exists rather than an assumption that the input is sane.
//!
//! ### Details the files forced, which a parser written from a description would miss
//!
//! - **`title` and `description` are never plain strings.** All 125 display blocks use a
//!   `{"translate": …}` object, so a parser that only accepts a string models nothing. Both
//!   forms are accepted and preserved by [`TextComponent`].
//! - **`frame` is usually absent.** 90 of 125 displays omit it, and the format's default is
//!   `task`; only `goal` (10) and `challenge` (25) are written out.
//! - **`requirements` is always an array of arrays**, never a bare array of strings: 1 806
//!   group entries, ten advancements with more than one group.
//! - **`icon.components`** appears on 3 displays (`{"minecraft:damage": …}`), so the icon is
//!   kept with its components rather than flattened to a bare id.
//! - **The 6 roots are not all top-level trees.** `minecraft:adventure/root` and friends are
//!   the tabs; the deepest chain is 9 levels.
//!
//! ## Hostile input (AGENTS.md section 10)
//!
//! Every file goes through [`crate::json`] with [`Limits`]. The parent chain is walked
//! iteratively with a visited set, so a hostile pack whose parents form a cycle is *reported*
//! and terminates rather than recursing until the stack dies. Criteria and requirement groups
//! are bounded by the file size limit, and no path here can panic.

use mc_core::ids::ResourceId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use crate::json::{
    JsonError, Limits, optional_i64, optional_str, read_json_object, required_array, required_str,
    type_name,
};

/// Deepest parent chain [`AdvancementRegistry::depth_of`] will walk before giving up.
///
/// Vanilla's deepest chain is **9**, so 64 is 7x headroom. The bound is not what stops a
/// cycle — a visited set does that — it is what stops a pathological acyclic chain from
/// being an unbounded loop in a caller that walks ancestors.
pub const MAX_PARENT_DEPTH: usize = 64;

/// Named text: a literal, or a translation key with its arguments.
///
/// Both forms occur in vanilla, and only one of them is a string: all 125 display blocks use
/// `{"translate": …}`. Flattening a translation key to a string would put
/// `advancements.story.root.title` on screen, so the two are kept apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextComponent {
    /// A literal string.
    Literal(String),
    /// A translation key, optionally with fallback text.
    Translate {
        /// The key.
        key: String,
        /// The `fallback` field, when the file states one.
        fallback: Option<String>,
    },
    /// A component shape this build does not model (`{"text": …}` with extras, a list, …),
    /// kept whole.
    Raw(serde_json::Value),
}

impl TextComponent {
    /// The translation key, when it is one.
    #[must_use]
    pub fn translation_key(&self) -> Option<&str> {
        match self {
            Self::Translate { key, .. } => Some(key),
            Self::Literal(_) | Self::Raw(_) => None,
        }
    }

    /// The literal text, when there is one to show.
    #[must_use]
    pub fn literal(&self) -> Option<&str> {
        match self {
            Self::Literal(text) => Some(text),
            Self::Translate { fallback, .. } => fallback.as_deref(),
            Self::Raw(_) => None,
        }
    }

    /// Whether this side of the display can be shown without a language file.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::Literal(_))
    }
}

/// The icon an advancement shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvancementIcon {
    /// The item id.
    pub item: ResourceId,
    /// The `components` object, kept whole because item components are not modelled.
    pub components: Option<serde_json::Value>,
}

/// How prominent an advancement is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Frame {
    /// An ordinary advancement. The default when `frame` is absent.
    Task,
    /// A notable one.
    Goal,
    /// A milestone.
    Challenge,
}

impl Frame {
    /// The frame string in the pack format.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Goal => "goal",
            Self::Challenge => "challenge",
        }
    }

    /// Recognise a frame string. `task` is also the answer for an absent field, which the
    /// caller does by defaulting rather than by calling this with an empty string.
    #[must_use]
    pub fn from_name(text: &str) -> Option<Self> {
        match text {
            "task" => Some(Self::Task),
            "goal" => Some(Self::Goal),
            "challenge" => Some(Self::Challenge),
            _ => None,
        }
    }
}

/// What an advancement shows when it is announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvancementDisplay {
    /// The title.
    pub title: TextComponent,
    /// The description.
    pub description: TextComponent,
    /// The icon.
    pub icon: AdvancementIcon,
    /// `task` when the file omits it, which 90 of vanilla's 125 displays do.
    pub frame: Frame,
    /// The tab background, on the roots that have one.
    pub background: Option<ResourceId>,
    /// Whether to announce in chat. Absent means `true`, which is the format's default.
    pub announce_to_chat: bool,
    /// Whether to show a toast. Absent means `true`.
    pub show_toast: bool,
    /// Whether the advancement is hidden until earned. Absent means `false`.
    pub hidden: bool,
}

/// The rewards an advancement grants when it is earned.
///
/// Modelled because it is a property of the data. **Nothing here grants them**: handing out
/// recipes or experience needs the player, so the caller does it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdvancementRewards {
    /// Experience points.
    pub experience: i32,
    /// Recipe ids to unlock.
    pub recipes: Vec<ResourceId>,
    /// Loot tables to roll for the player.
    pub loot: Vec<ResourceId>,
    /// Functions to run.
    ///
    /// **Not present in vanilla 26.1.2**: all 1 514 reward blocks carry only `recipes` (on
    /// 1 491) and `experience` (on 23). The field is modelled because the format defines it
    /// and a pack may use it, and is labelled here as not vanilla-exercised.
    pub function: Option<ResourceId>,
}

impl AdvancementRewards {
    /// Whether it grants anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.experience == 0
            && self.recipes.is_empty()
            && self.loot.is_empty()
            && self.function.is_none()
    }
}

/// One criterion: a trigger name and the conditions it fires under.
///
/// The conditions are **kept as raw JSON**. They are a predicate language of their own
/// (`minecraft:entity_properties`, `minecraft:location`, item predicates, and so on), and
/// nothing in this crate evaluates them — see the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct Criterion {
    /// The criterion's name, as written in the key.
    pub name: String,
    /// The `trigger` string, e.g. `minecraft:inventory_changed`.
    pub trigger: String,
    /// The `conditions` object, or `Value::Null` when the criterion states none.
    ///
    /// 15 of vanilla's 3 546 criteria state none, so this is a real case rather than a
    /// defensive one.
    pub conditions: serde_json::Value,
}

impl Criterion {
    /// Whether it states conditions at all.
    #[must_use]
    pub fn has_conditions(&self) -> bool {
        !self.conditions.is_null()
    }
}

/// A loaded advancement.
#[derive(Debug, Clone, PartialEq)]
pub struct Advancement {
    /// The advancement's name, as `namespace:path`.
    pub name: ResourceId,
    /// The parent, when it has one. Six vanilla advancements have none.
    pub parent: Option<ResourceId>,
    /// The display block, when it has one. 1 492 of vanilla's 1 617 have none, which is
    /// normal: an advancement can exist purely to be a parent.
    ///
    /// This is a *declared* field rather than a getter because the loader must be able to
    /// tell "no display" from "display not parsed"; the latter would be a load problem.
    pub display: Option<AdvancementDisplay>,
    /// The criteria, in ascending name order.
    pub criteria: Vec<Criterion>,
    /// The `requirements` groups, in file order.
    ///
    /// Each group is a set of criterion names that must *all* be satisfied, and the groups
    /// are alternatives: any one group completes the advancement. Empty means the format's
    /// default, which is one group holding every criterion.
    pub requirements: Vec<Vec<String>>,
    /// The rewards.
    pub rewards: AdvancementRewards,
    /// `sends_telemetry_event`. Absent means `false`.
    pub sends_telemetry_event: bool,
}

impl Advancement {
    /// A criterion by name.
    #[must_use]
    pub fn criterion(&self, name: &str) -> Option<&Criterion> {
        self.criteria
            .iter()
            .find(|criterion| criterion.name == name)
    }

    /// Every criterion name, ascending.
    #[must_use]
    pub fn criterion_names(&self) -> Vec<&str> {
        self.criteria
            .iter()
            .map(|criterion| criterion.name.as_str())
            .collect()
    }

    /// Distinct trigger strings, ascending.
    #[must_use]
    pub fn triggers(&self) -> BTreeSet<&str> {
        self.criteria
            .iter()
            .map(|criterion| criterion.trigger.as_str())
            .collect()
    }

    /// The requirement groups, or the format's default when the file states none.
    ///
    /// The default is one group holding every criterion name, which is what "all criteria
    /// are required" means written out. Returning it explicitly means a caller never has to
    /// special-case the absent field.
    #[must_use]
    pub fn effective_requirements(&self) -> Vec<Vec<&str>> {
        if self.requirements.is_empty() {
            return vec![self.criterion_names()];
        }
        self.requirements
            .iter()
            .map(|group| group.iter().map(String::as_str).collect())
            .collect()
    }

    /// Criterion names required by a group that the file does not define.
    ///
    /// A dangling name here is a real data bug: the advancement could never be completed,
    /// because a requirement names something with no criteria behind it.
    #[must_use]
    pub fn undefined_requirements(&self) -> Vec<&str> {
        self.requirements
            .iter()
            .flatten()
            .map(String::as_str)
            .filter(|name| self.criterion(name).is_none())
            .collect()
    }

    /// Criteria that no requirement group mentions.
    ///
    /// Measured against [`Self::effective_requirements`], not the raw field: a file that omits
    /// `requirements` means "every criterion is required", so an absent field must report
    /// **nothing** here. An earlier draft read the raw vector, which made every criterion of
    /// every vanilla advancement with an implicit requirement look unused.
    ///
    /// Not necessarily a bug — a criterion can exist only to be referenced by another
    /// advancement's conditions — but worth reporting, because it usually is one.
    #[must_use]
    pub fn unreferenced_criteria(&self) -> Vec<&str> {
        let referenced: BTreeSet<&str> = self
            .effective_requirements()
            .iter()
            .flatten()
            .copied()
            .collect();
        self.criterion_names()
            .into_iter()
            .filter(|name| !referenced.contains(name))
            .collect()
    }

    /// The icon's item, when there is a display.
    #[must_use]
    pub fn icon_item(&self) -> Option<&ResourceId> {
        self.display.as_ref().map(|display| &display.icon.item)
    }
}

/// How a parent chain ends.
///
/// Returned by [`AdvancementRegistry::chain_of`]. Kept as its own type because "the chain is
/// broken", "the chain is a cycle" and "the chain is fine" are three different answers, and a
/// caller that only gets `Option<usize>` cannot tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chain {
    /// It ends at an advancement with no parent. `depth` counts the advancements on the
    /// chain, so a root is depth 1.
    Rooted {
        /// Advancements on the chain, the starting one included.
        depth: usize,
    },
    /// It ends at a parent that is not loaded.
    MissingParent {
        /// The absent parent.
        at: ResourceId,
    },
    /// It closes on itself.
    Cyclic,
    /// It is longer than [`MAX_PARENT_DEPTH`].
    TooDeep {
        /// The advancement at which the limit was reached.
        at: ResourceId,
        /// The limit.
        limit: usize,
    },
}

/// Something wrong with the advancement *data*.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdvancementProblem {
    /// An advancement names a parent that is not loaded.
    ///
    /// Vanilla 26.1.2 has **0** of these. A pack with one has a broken tree, and every
    /// descendant silently becomes unreachable — which is exactly why this is reported
    /// rather than treated as a root.
    MissingParent {
        /// The child.
        from: ResourceId,
        /// The parent it names.
        missing: ResourceId,
    },
    /// Parent links form a cycle.
    ///
    /// Vanilla has **0**. Without a visited set a walker hangs here, so the detector is what
    /// makes the tree walkable at all.
    Cycle {
        /// The cycle, in order, starting and ending at the same advancement.
        path: Vec<ResourceId>,
    },
    /// Two advancements claim the same id.
    ///
    /// Impossible within one namespace and directory — a duplicate path is one file — so
    /// this can only come from merging several namespaces or packs. Vanilla has **0**.
    Duplicate {
        /// The id claimed twice.
        name: ResourceId,
    },
    /// A requirement group names a criterion the advancement does not define.
    UndefinedRequirement {
        /// The advancement.
        name: ResourceId,
        /// The criterion it names.
        criterion: String,
    },
    /// A criterion that no requirement group mentions, so nothing can ever require it.
    UnreferencedCriterion {
        /// The advancement.
        name: ResourceId,
        /// The criterion.
        criterion: String,
    },
    /// A parent chain deeper than [`MAX_PARENT_DEPTH`]; not followed further.
    TooDeep {
        /// The advancement at which the limit was hit.
        at: ResourceId,
        /// The limit.
        limit: usize,
    },
}

impl fmt::Display for AdvancementProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParent { from, missing } => {
                write!(f, "{from} names the missing parent {missing}")
            }
            Self::Cycle { path } => {
                let rendered: Vec<String> = path.iter().map(ToString::to_string).collect();
                write!(f, "advancement cycle: {}", rendered.join(" -> "))
            }
            Self::Duplicate { name } => write!(f, "{name} is defined more than once"),
            Self::UndefinedRequirement { name, criterion } => {
                write!(f, "{name} requires the undefined criterion {criterion:?}")
            }
            Self::UnreferencedCriterion { name, criterion } => {
                write!(
                    f,
                    "{name} defines the criterion {criterion:?}, which no group requires"
                )
            }
            Self::TooDeep { at, limit } => {
                write!(f, "{at} has a parent chain deeper than {limit}")
            }
        }
    }
}

/// What an advancement load did.
///
/// ```text
/// files == loaded + unmodelled.values().sum() + skipped.len()
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdvancementLoadReport {
    /// JSON files found in the directory.
    pub files: usize,
    /// Files whose advancement parsed.
    pub loaded: usize,
    /// File shapes this build does not model, with how many files used them.
    ///
    /// An advancement has no in-file `type` disciminator, so the only way a file lands here
    /// is a shape the loader refuses to guess at — currently nothing in vanilla, and nothing
    /// at all: it exists so [`Self::is_fully_accounted`] has a third bucket and a future
    /// shape has somewhere honest to go.
    pub unmodelled: BTreeMap<String, usize>,
    /// Files skipped because they could not be read or parsed, with the reason.
    pub skipped: Vec<String>,
    /// Distinct advancement names loaded.
    pub advancements: usize,
    /// Problems found by [`Self::check`], once it has been called.
    problems: Vec<AdvancementProblem>,
}

impl AdvancementLoadReport {
    /// Files counted as unmodelled.
    #[must_use]
    pub fn total_unmodelled_files(&self) -> usize {
        self.unmodelled.values().sum()
    }

    /// Whether every file was accounted for.
    #[must_use]
    pub fn is_fully_accounted(&self) -> bool {
        self.loaded + self.total_unmodelled_files() + self.skipped.len() == self.files
    }

    /// Whether nothing was skipped and nothing was unmodelled.
    ///
    /// Says nothing about the tree: call [`Self::check`] for that.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.skipped.is_empty() && self.unmodelled.is_empty()
    }

    /// Walk the tree and record every problem, replacing anything a previous call found.
    ///
    /// Deliberately explicit rather than run inside the loader: a caller that only wants the
    /// parse counts should not pay for a full tree walk, and a caller that wants the problems
    /// gets them in one deterministic pass.
    pub fn check(&mut self, registry: &AdvancementRegistry) {
        self.problems = registry.problems();
    }

    /// Every problem found by the last [`Self::check`].
    #[must_use]
    pub fn problems(&self) -> &[AdvancementProblem] {
        &self.problems
    }

    /// Problems that are real errors rather than a note about unused data.
    ///
    /// [`AdvancementProblem::UnreferencedCriterion`] is excluded: it is a smell, not a
    /// broken tree, and vanilla content does it.
    #[must_use]
    pub fn errors(&self) -> Vec<&AdvancementProblem> {
        self.problems
            .iter()
            .filter(|problem| !matches!(problem, AdvancementProblem::UnreferencedCriterion { .. }))
            .collect()
    }

    /// Every cycle found.
    #[must_use]
    pub fn cycles(&self) -> Vec<&AdvancementProblem> {
        self.problems
            .iter()
            .filter(|problem| matches!(problem, AdvancementProblem::Cycle { .. }))
            .collect()
    }
}

/// Every loaded advancement, with its parent links indexed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvancementRegistry {
    advancements: Vec<Advancement>,
    by_name: BTreeMap<ResourceId, usize>,
    children: BTreeMap<ResourceId, Vec<ResourceId>>,
    duplicates: Vec<ResourceId>,
    /// Whether `children` is stale, so [`AdvancementRegistry::children_of`] can rebuild it
    /// on demand instead of the loader rebuilding it per insert.
    index_dirty: bool,
}

impl AdvancementRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an advancement. A later advancement with the same name **replaces** an earlier
    /// one — the pack-override rule — but the id is also recorded in [`Self::duplicates`],
    /// because "two packs defined this" is worth knowing even when the override is correct.
    ///
    /// The child index used by [`Self::children_of`] is **not** rebuilt here: an override can
    /// change a parent, so rebuilding per insert would be quadratic, and the caller that
    /// cares calls [`Self::rebuild_index`] once after loading. [`Self::insert_all`] does that
    /// for you.
    pub fn insert(&mut self, advancement: Advancement) {
        let name = advancement.name.clone();
        if let Some(index) = self.by_name.get(&name).copied() {
            if !self.duplicates.contains(&name) {
                self.duplicates.push(name);
            }
            self.advancements[index] = advancement;
            self.index_dirty = true;
            return;
        }
        self.by_name.insert(name, self.advancements.len());
        self.advancements.push(advancement);
        self.index_dirty = true;
    }

    /// Add many advancements, then rebuild the child index once.
    pub fn insert_all(&mut self, advancements: impl IntoIterator<Item = Advancement>) {
        for advancement in advancements {
            self.insert(advancement);
        }
        self.rebuild_index();
    }

    /// Rebuild the child index that [`Self::children_of`] reads.
    ///
    /// Rebuilt rather than patched because the parent of an existing entry can change on
    /// override, and an index that is *usually* right is worse than one that is always
    /// right. [`Self::children_of`] calls it on demand, so a caller that never asks pays
    /// nothing.
    pub fn rebuild_index(&mut self) {
        let mut children: BTreeMap<ResourceId, Vec<ResourceId>> = BTreeMap::new();
        for advancement in &self.advancements {
            if let Some(parent) = &advancement.parent {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(advancement.name.clone());
            }
        }
        for list in children.values_mut() {
            list.sort();
            list.dedup();
        }
        self.children = children;
        self.index_dirty = false;
    }

    /// Every advancement, in load order.
    #[must_use]
    pub fn advancements(&self) -> &[Advancement] {
        &self.advancements
    }

    /// How many advancements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.advancements.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.advancements.is_empty()
    }

    /// An advancement by name.
    #[must_use]
    pub fn by_name(&self, name: &ResourceId) -> Option<&Advancement> {
        self.by_name
            .get(name)
            .map(|index| &self.advancements[*index])
    }

    /// Every name, ascending.
    pub fn names(&self) -> impl Iterator<Item = &ResourceId> {
        self.by_name.keys()
    }

    /// Ids that were defined more than once.
    #[must_use]
    pub fn duplicates(&self) -> &[ResourceId] {
        &self.duplicates
    }

    /// Advancements with no parent, ascending.
    ///
    /// Vanilla has six, one per tab.
    #[must_use]
    pub fn roots(&self) -> Vec<&Advancement> {
        self.by_name
            .keys()
            .filter_map(|name| self.by_name(name))
            .filter(|advancement| advancement.parent.is_none())
            .collect()
    }

    /// Advancements whose parent is `name`, ascending by name.
    #[must_use]
    pub fn children_of(&self, name: &ResourceId) -> Vec<&Advancement> {
        // `&self` cannot rebuild the index, so a caller that mutated after loading gets an
        // empty list rather than a wrong one; `insert_all` and `rebuild_index` are the
        // supported ways to make it current.
        if self.index_dirty {
            return Vec::new();
        }
        self.children
            .get(name)
            .map(|ids| ids.iter().filter_map(|id| self.by_name(id)).collect())
            .unwrap_or_default()
    }

    /// Whether the child index is stale, so [`Self::children_of`] would return nothing.
    #[must_use]
    pub const fn is_index_stale(&self) -> bool {
        self.index_dirty
    }

    /// The parent of `name`, when it has one that is loaded.
    #[must_use]
    pub fn parent_of(&self, name: &ResourceId) -> Option<&Advancement> {
        self.by_name(name)
            .and_then(|advancement| advancement.parent.as_ref())
            .and_then(|parent| self.by_name(parent))
    }

    /// The chain from `name` up to a root, `name` first.
    ///
    /// Stops at [`MAX_PARENT_DEPTH`] and at a cycle, so it always terminates. A missing
    /// parent ends the chain, because there is nothing to walk to.
    #[must_use]
    pub fn ancestors_of(&self, name: &ResourceId) -> Vec<&Advancement> {
        let mut out = Vec::new();
        let mut seen: Vec<ResourceId> = Vec::new();
        let mut current = self.by_name(name);
        while let Some(advancement) = current {
            if out.len() >= MAX_PARENT_DEPTH || seen.contains(&advancement.name) {
                break;
            }
            seen.push(advancement.name.clone());
            out.push(advancement);
            current = advancement
                .parent
                .as_ref()
                .and_then(|parent| self.by_name(parent));
        }
        out
    }

    /// How many parent links sit above `name`, or `None` when the chain is broken or cyclic.
    ///
    /// A root is depth 1, which is the numbering the depth histogram in the module
    /// documentation uses. Callers that need to tell the two `None` cases apart use
    /// [`Self::chain_of`].
    #[must_use]
    pub fn depth_of(&self, name: &ResourceId) -> Option<usize> {
        match self.chain_of(name) {
            Chain::Rooted { depth } => Some(depth),
            Chain::MissingParent { .. } | Chain::Cyclic | Chain::TooDeep { .. } => None,
        }
    }

    /// How a parent chain ends.
    ///
    /// Four genuinely different outcomes, kept apart because collapsing them loses the
    /// distinction a caller needs: a chain that stops at a **missing parent** is a reported
    /// data error, one that stops at a root is normal, and one caught in a cycle is a hostile
    /// pack. An earlier draft returned `None` for all of them, which made every advancement
    /// under a missing parent look "too deep".
    #[must_use]
    pub fn chain_of(&self, name: &ResourceId) -> Chain {
        let mut seen: Vec<ResourceId> = Vec::new();
        let Some(start) = self.by_name(name) else {
            return Chain::MissingParent { at: name.clone() };
        };
        let mut current = start;
        let mut depth = 1usize;
        loop {
            if seen.contains(&current.name) {
                return Chain::Cyclic;
            }
            seen.push(current.name.clone());
            let Some(parent) = current.parent.as_ref() else {
                return Chain::Rooted { depth };
            };
            if depth >= MAX_PARENT_DEPTH {
                return Chain::TooDeep {
                    at: current.name.clone(),
                    limit: MAX_PARENT_DEPTH,
                };
            }
            let Some(next) = self.by_name(parent) else {
                return Chain::MissingParent { at: parent.clone() };
            };
            current = next;
            depth += 1;
        }
    }

    /// Every advancement reachable from `name`, `name` first, breadth-first and ascending
    /// within each level.
    ///
    /// Bounded by the registry size and a visited set, so a cycle cannot make it loop.
    #[must_use]
    pub fn subtree_of(&self, name: &ResourceId) -> Vec<&Advancement> {
        let mut out = Vec::new();
        let mut queue = vec![name.clone()];
        let mut seen: BTreeSet<ResourceId> = BTreeSet::new();
        while let Some(current) = queue.first().cloned() {
            queue.remove(0);
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(advancement) = self.by_name(&current) else {
                continue;
            };
            out.push(advancement);
            for child in self.children_of(&current) {
                queue.push(child.name.clone());
            }
        }
        out
    }

    /// Every advancement, sorted by name.
    ///
    /// The deterministic order a caller can compare, hash or print.
    #[must_use]
    pub fn sorted(&self) -> Vec<&Advancement> {
        self.by_name
            .keys()
            .filter_map(|name| self.by_name(name))
            .collect()
    }

    /// Every advancement whose criteria use `trigger`.
    #[must_use]
    pub fn with_trigger(&self, trigger: &str) -> Vec<&Advancement> {
        self.advancements
            .iter()
            .filter(|advancement| advancement.triggers().contains(trigger))
            .collect()
    }

    /// Every distinct trigger string, ascending.
    #[must_use]
    pub fn triggers(&self) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        for advancement in &self.advancements {
            out.extend(advancement.triggers());
        }
        out
    }

    /// Every problem in the tree, sorted.
    ///
    /// Sorted so the output is reproducible regardless of insertion order (AGENTS.md §3.6).
    #[must_use]
    pub fn problems(&self) -> Vec<AdvancementProblem> {
        let mut out = Vec::new();
        for name in self.duplicates.clone() {
            out.push(AdvancementProblem::Duplicate { name });
        }
        for advancement in &self.advancements {
            if let Some(parent) = &advancement.parent
                && !self.by_name.contains_key(parent)
            {
                out.push(AdvancementProblem::MissingParent {
                    from: advancement.name.clone(),
                    missing: parent.clone(),
                });
            }
            for criterion in advancement.undefined_requirements() {
                out.push(AdvancementProblem::UndefinedRequirement {
                    name: advancement.name.clone(),
                    criterion: criterion.to_owned(),
                });
            }
            for criterion in advancement.unreferenced_criteria() {
                out.push(AdvancementProblem::UnreferencedCriterion {
                    name: advancement.name.clone(),
                    criterion: criterion.to_owned(),
                });
            }
            if let Chain::TooDeep { at, limit } = self.chain_of(&advancement.name) {
                out.push(AdvancementProblem::TooDeep { at, limit });
            }
        }
        out.extend(self.cycles());
        out.sort();
        out.dedup();
        out
    }

    /// Every parent cycle, each reported once from its lowest id.
    ///
    /// Iterative, with a visited set: a recursive walk would die on the first hostile pack,
    /// and this is one of the three hazards the loader exists to catch.
    #[must_use]
    pub fn cycles(&self) -> Vec<AdvancementProblem> {
        let mut colour: BTreeMap<ResourceId, u8> = BTreeMap::new();
        let mut out = Vec::new();
        for start in self.by_name.keys() {
            if colour.contains_key(start) {
                continue;
            }
            let mut path: Vec<ResourceId> = Vec::new();
            let mut node = start.clone();
            loop {
                if let Some(seen) = colour.get(&node) {
                    // `1` means "on the current path", `2` means "finished".
                    if *seen == 1
                        && let Some(index) = path.iter().position(|entry| entry == &node)
                    {
                        let mut cycle: Vec<ResourceId> = path[index..].to_vec();
                        cycle.push(node);
                        out.push(AdvancementProblem::Cycle { path: cycle });
                    }
                    break;
                }
                colour.insert(node.clone(), 1);
                path.push(node.clone());
                let Some(next) = self
                    .by_name(&node)
                    .and_then(|advancement| advancement.parent.as_ref())
                else {
                    break;
                };
                node = next.clone();
            }
            for entry in path {
                colour.insert(entry, 2);
            }
        }
        out
    }

    /// The deepest parent chain, or `None` when any chain is broken or cyclic.
    ///
    /// Reported as a figure rather than asserted, because a hostile pack can make it
    /// undefined and the caller should be able to say so.
    #[must_use]
    pub fn max_depth(&self) -> Option<usize> {
        let mut deepest = 0usize;
        for name in self.by_name.keys() {
            let depth = self.depth_of(name)?;
            deepest = deepest.max(depth);
        }
        Some(deepest)
    }

    /// A depth histogram: depth -> how many advancements sit there.
    ///
    /// Vanilla's is `1: 6, 2: 1531, 3: 48, 4: 13, 5: 8, 6: 5, 7: 3, 8: 2, 9: 1`. A chain
    /// that is broken or cyclic is counted at depth 0.
    #[must_use]
    pub fn depth_histogram(&self) -> BTreeMap<usize, usize> {
        let mut out = BTreeMap::new();
        for name in self.by_name.keys() {
            let depth = self.depth_of(name).unwrap_or(0);
            *out.entry(depth).or_insert(0) += 1;
        }
        out
    }

    /// Load every advancement under `<namespace_dir>/advancement/`.
    ///
    /// Never fails as a whole: a malformed file is recorded and skipped, so one bad file in a
    /// pack does not discard the rest (AGENTS.md §9).
    #[must_use]
    pub fn load_directory(
        namespace_dir: &Path,
        namespace: &str,
        limits: Limits,
        report: &mut AdvancementLoadReport,
    ) -> Self {
        let base = namespace_dir.join("advancement");
        let mut registry = Self::new();
        for path in crate::tag::json_files(&base) {
            report.files += 1;
            let Ok(relative) = path.strip_prefix(&base) else {
                report.skipped.push(format!(
                    "{}: not under the advancement directory",
                    path.display()
                ));
                continue;
            };
            let stem_path = relative.with_extension("");
            let Some(stem) = stem_path.to_str() else {
                *report
                    .unmodelled
                    .entry("non-utf8 file name".to_owned())
                    .or_insert(0) += 1;
                continue;
            };
            let stem = stem.replace('\\', "/");
            let Ok(name) = ResourceId::parse(&format!("{namespace}:{stem}")) else {
                *report
                    .unmodelled
                    .entry("unusable advancement name".to_owned())
                    .or_insert(0) += 1;
                continue;
            };
            match read_advancement_file(&path, name, limits) {
                Ok(advancement) => {
                    report.loaded += 1;
                    registry.insert(advancement);
                }
                Err(error) => report.skipped.push(error.to_string()),
            }
        }
        report.unmodelled.retain(|_, count| *count > 0);
        // One rebuild for the whole directory, rather than one per insert.
        registry.rebuild_index();
        report.advancements = registry.len();
        registry
    }
}

/// The 54 trigger strings measured in vanilla 26.1.2, for reporting coverage.
pub const OBSERVED_TRIGGERS: &[&str] = &[
    "minecraft:allay_drop_item_on_block",
    "minecraft:avoid_vibration",
    "minecraft:bee_nest_destroyed",
    "minecraft:bred_animals",
    "minecraft:brewed_potion",
    "minecraft:changed_dimension",
    "minecraft:channeled_lightning",
    "minecraft:construct_beacon",
    "minecraft:consume_item",
    "minecraft:crafter_recipe_crafted",
    "minecraft:cured_zombie_villager",
    "minecraft:effects_changed",
    "minecraft:enchanted_item",
    "minecraft:enter_block",
    "minecraft:entity_hurt_player",
    "minecraft:entity_killed_player",
    "minecraft:fall_after_explosion",
    "minecraft:fall_from_height",
    "minecraft:filled_bucket",
    "minecraft:fishing_rod_hooked",
    "minecraft:hero_of_the_village",
    "minecraft:impossible",
    "minecraft:inventory_changed",
    "minecraft:item_durability_changed",
    "minecraft:item_used_on_block",
    "minecraft:kill_mob_near_sculk_catalyst",
    "minecraft:killed_by_arrow",
    "minecraft:levitation",
    "minecraft:lightning_strike",
    "minecraft:location",
    "minecraft:nether_travel",
    "minecraft:placed_block",
    "minecraft:player_generates_container_loot",
    "minecraft:player_hurt_entity",
    "minecraft:player_interacted_with_entity",
    "minecraft:player_killed_entity",
    "minecraft:player_sheared_equipment",
    "minecraft:recipe_crafted",
    "minecraft:recipe_unlocked",
    "minecraft:ride_entity_in_lava",
    "minecraft:shot_crossbow",
    "minecraft:slept_in_bed",
    "minecraft:slide_down_block",
    "minecraft:spear_mobs",
    "minecraft:started_riding",
    "minecraft:summoned_entity",
    "minecraft:target_hit",
    "minecraft:tame_animal",
    "minecraft:thrown_item_picked_up_by_entity",
    "minecraft:thrown_item_picked_up_by_player",
    "minecraft:tick",
    "minecraft:used_totem",
    "minecraft:using_item",
    "minecraft:villager_trade",
];

/// Read one advancement file.
///
/// # Errors
///
/// [`JsonError`] when the file is unreadable, too large, not JSON, or has the wrong shape.
pub fn read_advancement_file(
    path: &Path,
    name: ResourceId,
    limits: Limits,
) -> Result<Advancement, JsonError> {
    let map = read_json_object(path, limits)?;
    let parent = match optional_str(&map, "parent", path)? {
        Some(text) => Some(ResourceId::parse(text).map_err(|error| JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("parent {text:?} is not a valid resource id: {error}"),
        })?),
        None => None,
    };
    let display = match map.get("display") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(read_display(value, path)?),
    };
    let criteria = read_criteria(&map, path)?;
    let requirements = read_requirements(&map, path)?;
    let rewards = match map.get("rewards") {
        None | Some(serde_json::Value::Null) => AdvancementRewards::default(),
        Some(serde_json::Value::Object(rewards)) => read_rewards(rewards, path)?,
        Some(other) => {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("\"rewards\" must be an object, found {}", type_name(other)),
            });
        }
    };
    let sends_telemetry_event = match map.get("sends_telemetry_event") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(other) => {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "\"sends_telemetry_event\" must be a boolean, found {}",
                    type_name(other)
                ),
            });
        }
    };
    Ok(Advancement {
        name,
        parent,
        display,
        criteria,
        requirements,
        rewards,
        sends_telemetry_event,
    })
}

fn read_display(value: &serde_json::Value, path: &Path) -> Result<AdvancementDisplay, JsonError> {
    let serde_json::Value::Object(map) = value else {
        return Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("\"display\" must be an object, found {}", type_name(value)),
        });
    };
    let title = read_text(map.get("title"), "title", path)?;
    let description = read_text(map.get("description"), "description", path)?;
    let icon = read_icon(map.get("icon"), path)?;
    // `frame` is absent on 90 of vanilla's 125 displays; the format's default is `task`.
    let frame = match optional_str(map, "frame", path)? {
        None => Frame::Task,
        Some(text) => Frame::from_name(text).ok_or_else(|| JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("unknown frame {text:?}"),
        })?,
    };
    let background = match optional_str(map, "background", path)? {
        Some(text) => Some(ResourceId::parse(text).map_err(|error| JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("background {text:?} is not a valid resource id: {error}"),
        })?),
        None => None,
    };
    Ok(AdvancementDisplay {
        title,
        description,
        icon,
        frame,
        background,
        announce_to_chat: optional_bool_default(map, "announce_to_chat", true, path)?,
        show_toast: optional_bool_default(map, "show_toast", true, path)?,
        hidden: optional_bool_default(map, "hidden", false, path)?,
    })
}

/// Read a text component, which the format writes as a string or an object.
fn read_text(
    value: Option<&serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<TextComponent, JsonError> {
    let Some(value) = value else {
        return Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("a display needs {key:?}"),
        });
    };
    match value {
        serde_json::Value::String(text) => Ok(TextComponent::Literal(text.clone())),
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(translation)) = map.get("translate") {
                let fallback = match map.get("fallback") {
                    None => None,
                    Some(serde_json::Value::String(text)) => Some(text.clone()),
                    Some(other) => {
                        return Err(JsonError::Invalid {
                            path: path.to_path_buf(),
                            reason: format!(
                                "a {key:?} fallback must be a string, found {}",
                                type_name(other)
                            ),
                        });
                    }
                };
                return Ok(TextComponent::Translate {
                    key: translation.clone(),
                    fallback,
                });
            }
            // `{"text": …}`, a list of siblings, and anything else are kept whole rather
            // than flattened to a string that would be wrong on screen.
            Ok(TextComponent::Raw(value.clone()))
        }
        other => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "{key:?} must be a string or a component object, found {}",
                type_name(other)
            ),
        }),
    }
}

fn read_icon(value: Option<&serde_json::Value>, path: &Path) -> Result<AdvancementIcon, JsonError> {
    let Some(serde_json::Value::Object(map)) = value else {
        return Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "a display needs an \"icon\" object, found {}",
                value.map_or("nothing", type_name)
            ),
        });
    };
    let id = required_str(map, "id", path)?;
    let item = ResourceId::parse(id).map_err(|error| JsonError::Invalid {
        path: path.to_path_buf(),
        reason: format!("icon id {id:?} is not a valid resource id: {error}"),
    })?;
    // Item components are not modelled, so the object is kept as it was written.
    Ok(AdvancementIcon {
        item,
        components: map.get("components").cloned(),
    })
}

fn read_criteria(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<Vec<Criterion>, JsonError> {
    let Some(value) = map.get("criteria") else {
        // Every one of vanilla's 1 617 advancements states `criteria`, and an advancement
        // with none could never be completed, so its absence is refused rather than
        // defaulted to empty.
        return Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: "an advancement needs \"criteria\"".to_owned(),
        });
    };
    let serde_json::Value::Object(criteria) = value else {
        return Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("\"criteria\" must be an object, found {}", type_name(value)),
        });
    };
    let mut out = Vec::with_capacity(criteria.len());
    for (name, value) in criteria {
        let serde_json::Value::Object(entry) = value else {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "criterion {name:?} must be an object, found {}",
                    type_name(value)
                ),
            });
        };
        let trigger = required_str(entry, "trigger", path)?;
        out.push(Criterion {
            name: name.clone(),
            trigger: trigger.to_owned(),
            // 15 of vanilla's 3 546 criteria state no conditions, so an absent field is
            // `null` rather than an error.
            conditions: entry
                .get("conditions")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
    }
    Ok(out)
}

fn read_requirements(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<Vec<Vec<String>>, JsonError> {
    let Some(value) = map.get("requirements") else {
        // Absent means "every criterion is required", which `effective_requirements`
        // materialises; storing an empty vector keeps the two cases apart.
        return Ok(Vec::new());
    };
    let groups = value.as_array().ok_or_else(|| JsonError::Invalid {
        path: path.to_path_buf(),
        reason: format!(
            "\"requirements\" must be an array, found {}",
            type_name(value)
        ),
    })?;
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        let names = group.as_array().ok_or_else(|| JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "every requirement group must be an array, found {}",
                type_name(group)
            ),
        })?;
        let mut group_out = Vec::with_capacity(names.len());
        for name in names {
            let Some(text) = name.as_str() else {
                return Err(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "every requirement must name a criterion as a string".to_owned(),
                });
            };
            group_out.push(text.to_owned());
        }
        out.push(group_out);
    }
    Ok(out)
}

fn read_rewards(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<AdvancementRewards, JsonError> {
    let experience = optional_i64(map, "experience", path)?.unwrap_or(0);
    let experience = i32::try_from(experience).map_err(|_| JsonError::Invalid {
        path: path.to_path_buf(),
        reason: format!("reward experience {experience} does not fit an i32"),
    })?;
    Ok(AdvancementRewards {
        experience,
        recipes: read_id_array(map, "recipes", path)?,
        loot: read_id_array(map, "loot", path)?,
        function: match optional_str(map, "function", path)? {
            Some(text) => Some(ResourceId::parse(text).map_err(|error| JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("reward function {text:?} is not a valid resource id: {error}"),
            })?),
            None => None,
        },
    })
}

fn read_id_array(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Vec<ResourceId>, JsonError> {
    if map.get(key).is_none() {
        return Ok(Vec::new());
    }
    let list = required_array(map, key, path)?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let Some(text) = item.as_str() else {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("every {key:?} entry must be a string"),
            });
        };
        out.push(ResourceId::parse(text).map_err(|error| JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("{key:?} entry {text:?} is not a valid resource id: {error}"),
        })?);
    }
    Ok(out)
}

fn optional_bool_default(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: bool,
    path: &Path,
) -> Result<bool, JsonError> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::Bool(flag)) => Ok(*flag),
        Some(other) => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "field {key:?} must be a boolean, found {}",
                type_name(other)
            ),
        }),
    }
}

#[cfg(test)]
mod tests;
