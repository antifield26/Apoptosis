//! Tags: the data model and file loading (P07-03).
//!
//! Resolution lives in [`crate::tag_resolve`] so the algorithm and its guards can be
//! tested without a filesystem.
//!
//! ## The format, established from the 26.1.2 jar
//!
//! ```text
//! data/<namespace>/tags/<registry>/<path>.json
//! { "values": [ "minecraft:oak_planks", "#minecraft:wooden_slabs", { "id": "…", "required": false } ] }
//! ```
//!
//! `survey_tags.py` measured what vanilla actually exercises:
//!
//! | Feature | Vanilla usage | Consequence |
//! |---|---|---|
//! | 758 tag files across 17 registries | — | the loader is registry-agnostic |
//! | nested `#other` references | **384** | transitivity is the common case |
//! | deepest nesting | **4** | a depth bound of 64 is 16x headroom |
//! | cycles | **none** | the cycle detector is only exercised by our tests |
//! | `"replace": true` | **0** | implemented because the format defines it; **unverified against vanilla** |
//! | `{ "id": …, "required": false }` | **0** | same: implemented, unverified |

use mc_core::ids::ResourceId;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::json::{JsonError, Limits, read_json_object};

/// A tag's identity: its registry directory and its namespaced name.
///
/// `registry` is a plain string (`"item"`, `"block"`, `"worldgen/biome"`) because it
/// is a filesystem directory, not a closed set — vanilla alone has 17 and a pack may
/// add more. An enum here would make the loader reject a pack for using a registry we
/// had not enumerated, which is exactly what data packs exist to allow.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TagKey {
    /// The registry directory, e.g. `"item"` or `"worldgen/biome"`.
    pub registry: String,
    /// The tag's namespaced name.
    pub name: ResourceId,
}

impl TagKey {
    /// A tag key.
    #[must_use]
    pub fn new(registry: impl Into<String>, name: ResourceId) -> Self {
        Self {
            registry: registry.into(),
            name,
        }
    }

    /// Parse `registry/namespace:path`.
    ///
    /// # Errors
    ///
    /// [`JsonError::Invalid`] when there is no `/` or the name is not a valid
    /// resource id. Used by tests and diagnostics rather than by the loader, which
    /// builds keys from the file path.
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        let fail = |reason: String| JsonError::Invalid {
            path: PathBuf::from(text),
            reason,
        };
        let Some((registry, name)) = text.split_once('/') else {
            return Err(fail(
                "a tag key needs the form registry/namespace:path".to_owned(),
            ));
        };
        let name = ResourceId::parse(name).map_err(|error| fail(format!("{error}")))?;
        Ok(Self::new(registry, name))
    }

    /// Where this tag lives inside a pack, relative to the pack root.
    #[must_use]
    pub fn relative_path(&self) -> String {
        format!(
            "tags/{}/{}/{}.json",
            self.registry,
            self.name.namespace(),
            self.name.value()
        )
    }
}

impl std::fmt::Display for TagKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.registry, self.name)
    }
}

/// One entry inside a tag's `values` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagEntry {
    /// A concrete registry entry.
    Id(ResourceId),
    /// A reference to another tag, resolved transitively.
    Tag(TagKey),
    /// A concrete entry the pack marks as possibly absent.
    OptionalId(ResourceId),
    /// A tag reference the pack marks as possibly absent.
    OptionalTag(TagKey),
}

impl TagEntry {
    /// Whether the pack said this entry may be absent (`required: false`).
    #[must_use]
    pub const fn is_optional(&self) -> bool {
        matches!(self, Self::OptionalId(_) | Self::OptionalTag(_))
    }

    /// The tag it references, if it is a reference.
    #[must_use]
    pub const fn as_tag(&self) -> Option<&TagKey> {
        match self {
            Self::Tag(key) | Self::OptionalTag(key) => Some(key),
            Self::Id(_) | Self::OptionalId(_) => None,
        }
    }

    /// The concrete id, if it is one.
    #[must_use]
    pub const fn as_id(&self) -> Option<&ResourceId> {
        match self {
            Self::Id(id) | Self::OptionalId(id) => Some(id),
            Self::Tag(_) | Self::OptionalTag(_) => None,
        }
    }
}

/// A tag's contents from one file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagValue {
    /// `"replace": true` discards everything earlier packs contributed.
    pub replace: bool,
    /// The entries, in file order.
    pub entries: Vec<TagEntry>,
}

/// A tag value together with where it came from, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFile {
    /// Which tag.
    pub key: TagKey,
    /// The file's contents.
    pub value: TagValue,
    /// The file it was read from.
    pub source: PathBuf,
}

/// Something wrong with a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagProblem {
    /// A `#reference` to a tag that does not exist and was not marked optional.
    MissingTag {
        /// The tag containing the reference.
        from: TagKey,
        /// The tag it references.
        missing: TagKey,
    },
    /// A `#reference` marked `required: false` whose target does not exist.
    OptionalMissingTag {
        /// The tag containing the reference.
        from: TagKey,
        /// The tag it references.
        missing: TagKey,
    },
    /// An id that does not exist in a registry we *do* know.
    MissingId {
        /// The tag containing the reference.
        from: TagKey,
        /// The absent id.
        missing: ResourceId,
        /// The registry it should have been in.
        registry: String,
    },
    /// An id marked `required: false` that does not exist in a known registry.
    OptionalMissingId {
        /// The tag containing the reference.
        from: TagKey,
        /// The absent id.
        missing: ResourceId,
        /// The registry it should have been in.
        registry: String,
    },
    /// The tag is part of a reference cycle; the path closes on itself.
    Cycle {
        /// The cycle in order, starting and ending at the same tag.
        path: Vec<TagKey>,
    },
    /// Nesting deeper than [`crate::tag_resolve::MAX_TAG_DEPTH`]; not followed.
    TooDeep {
        /// The tag at which the limit was hit.
        at: TagKey,
        /// The limit.
        limit: usize,
    },
}

impl std::fmt::Display for TagProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTag { from, missing } => {
                write!(f, "{from} references the missing tag {missing}")
            }
            Self::OptionalMissingTag { from, missing } => {
                write!(f, "{from} optionally references the absent tag {missing}")
            }
            Self::MissingId {
                from,
                missing,
                registry,
            } => write!(
                f,
                "{from} references the missing {registry} entry {missing}"
            ),
            Self::OptionalMissingId {
                from,
                missing,
                registry,
            } => write!(
                f,
                "{from} optionally references the absent {registry} entry {missing}"
            ),
            Self::Cycle { path } => {
                let rendered: Vec<String> = path.iter().map(ToString::to_string).collect();
                write!(f, "tag cycle: {}", rendered.join(" -> "))
            }
            Self::TooDeep { at, limit } => {
                write!(f, "{at} nests deeper than the {limit}-level limit")
            }
        }
    }
}

/// What a tag load did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagLoadReport {
    /// Files merged.
    pub files: usize,
    /// Files skipped because they could not be read or parsed, with the reason.
    pub skipped: Vec<String>,
    /// Problems found while resolving.
    pub problems: Vec<TagProblem>,
}

impl TagLoadReport {
    /// Whether anything was skipped or reported.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.skipped.is_empty() && self.problems.is_empty()
    }

    /// Every cycle found.
    #[must_use]
    pub fn cycles(&self) -> Vec<&TagProblem> {
        self.problems
            .iter()
            .filter(|problem| matches!(problem, TagProblem::Cycle { .. }))
            .collect()
    }

    /// Problems that are real errors rather than a pack marking something optional.
    #[must_use]
    pub fn errors(&self) -> Vec<&TagProblem> {
        self.problems
            .iter()
            .filter(|problem| {
                !matches!(
                    problem,
                    TagProblem::OptionalMissingTag { .. } | TagProblem::OptionalMissingId { .. }
                )
            })
            .collect()
    }
}

/// Every tag, resolved to its transitive id set.
///
/// Materialised rather than resolved lazily on lookup. Vanilla has 758 tags at depth
/// 4, so the closure is small, and materialising it makes a lookup an infallible map
/// hit — which is what gameplay call sites want.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSet {
    entries: BTreeMap<TagKey, BTreeSet<ResourceId>>,
}

impl TagSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every id in a tag; `None` when the tag is not declared.
    ///
    /// A declared-but-empty tag returns `Some(empty)`, so a caller can tell the two
    /// apart.
    #[must_use]
    pub fn get(&self, key: &TagKey) -> Option<&BTreeSet<ResourceId>> {
        self.entries.get(key)
    }

    /// Whether a tag is declared.
    #[must_use]
    pub fn contains_tag(&self, key: &TagKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Whether an id is in a tag; `false` for an undeclared tag.
    #[must_use]
    pub fn contains(&self, key: &TagKey, id: &ResourceId) -> bool {
        self.entries.get(key).is_some_and(|set| set.contains(id))
    }

    /// How many tags are declared.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no tags are declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every tag key, ascending.
    pub fn keys(&self) -> impl Iterator<Item = &TagKey> {
        self.entries.keys()
    }

    /// Every tag in one registry, ascending.
    #[must_use]
    pub fn in_registry(&self, registry: &str) -> Vec<&TagKey> {
        self.entries
            .keys()
            .filter(|key| key.registry == registry)
            .collect()
    }

    /// Every registry that has at least one tag, ascending.
    #[must_use]
    pub fn registries(&self) -> Vec<&str> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for key in self.entries.keys() {
            seen.insert(&key.registry);
        }
        seen.into_iter().collect()
    }

    /// Total ids across every tag, counting an id once per tag containing it.
    #[must_use]
    pub fn total_entries(&self) -> usize {
        self.entries.values().map(BTreeSet::len).sum()
    }

    /// Insert an already-resolved tag (programmatic packs and tests).
    pub fn insert(&mut self, key: TagKey, ids: BTreeSet<ResourceId>) {
        self.entries.insert(key, ids);
    }

    /// Build from a resolved map.
    #[must_use]
    pub fn from_map(entries: BTreeMap<TagKey, BTreeSet<ResourceId>>) -> Self {
        Self { entries }
    }
}

/// The ids a registry currently holds, used to validate tag references.
///
/// A plain map rather than a trait: the loader has one real caller (the server, which
/// has the block and item registries), so a trait would be scaffolding for a
/// substitution that does not exist (AGENTS.md §3.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryContents {
    by_registry: BTreeMap<String, BTreeSet<ResourceId>>,
}

impl RegistryContents {
    /// Empty contents.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a registry's known ids.
    pub fn insert(&mut self, registry: impl Into<String>, ids: BTreeSet<ResourceId>) {
        self.by_registry.insert(registry.into(), ids);
    }

    /// Whether a registry is modelled at all.
    #[must_use]
    pub fn has_registry(&self, registry: &str) -> bool {
        self.by_registry.contains_key(registry)
    }

    /// Whether an id exists.
    ///
    /// `None` means "this registry is not modelled", which the resolver must not
    /// treat as "the id is absent" — claiming an id is missing when we simply do not
    /// know the registry would make the loader report false errors for every registry
    /// we have not implemented yet.
    #[must_use]
    pub fn contains(&self, registry: &str, id: &ResourceId) -> Option<bool> {
        self.by_registry.get(registry).map(|ids| ids.contains(id))
    }

    /// How many registries are modelled.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_registry.len()
    }

    /// Whether none are.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_registry.is_empty()
    }
}

/// Read every tag file under `<root>/tags/`.
///
/// An unreadable or malformed file is recorded in the report and skipped rather than
/// failing the load, so one bad file in a pack does not discard the rest
/// (AGENTS.md §9).
#[must_use]
pub fn load_directory(
    root: &Path,
    namespace: &str,
    limits: Limits,
    report: &mut TagLoadReport,
) -> Vec<(TagKey, TagValue)> {
    let base = root.join("tags");
    let mut out = Vec::new();
    for path in json_files(&base) {
        let Ok(relative) = path.strip_prefix(&base) else {
            continue;
        };
        // `with_extension` returns an owned PathBuf, so bind it before borrowing.
        let stem_path = relative.with_extension("");
        let Some(stem) = stem_path.to_str() else {
            continue;
        };
        // The registry may itself contain a slash (`worldgen/biome`), so split at the
        // last separator: everything before it is the registry.
        let normalised = stem.replace('\\', "/");
        let Some((registry, name)) = normalised.rsplit_once('/') else {
            report.skipped.push(format!(
                "{}: a tag file must sit under a registry directory",
                path.display()
            ));
            continue;
        };
        let Ok(name) = ResourceId::parse(&format!("{namespace}:{name}")) else {
            report
                .skipped
                .push(format!("{}: unusable tag name", path.display()));
            continue;
        };
        let key = TagKey::new(registry, name);
        match read_tag_file(&path, &key, limits) {
            Ok(value) => out.push((key, value)),
            Err(error) => report.skipped.push(error.to_string()),
        }
    }
    out
}

/// Read one tag file.
///
/// # Errors
///
/// [`JsonError`] when the file is unreadable, too large, not JSON, or has the wrong
/// shape.
pub fn read_tag_file(path: &Path, key: &TagKey, limits: Limits) -> Result<TagValue, JsonError> {
    let map = read_json_object(path, limits)?;
    // `replace` is only meaningful as `true`; a non-boolean is a pack error rather
    // than something to coerce.
    let replace = match map.get("replace") {
        None => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(other) => {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "tag \"replace\" must be a boolean, found {}",
                    crate::json::type_name(other)
                ),
            });
        }
    };
    let values = crate::json::required_array(&map, "values", path)?;
    let mut entries = Vec::with_capacity(values.len());
    for value in values {
        entries.push(parse_value(value, key, path)?);
    }
    Ok(TagValue { replace, entries })
}

/// Parse one element of a `values` array.
fn parse_value(
    value: &serde_json::Value,
    owner: &TagKey,
    path: &Path,
) -> Result<TagEntry, JsonError> {
    match value {
        serde_json::Value::String(text) => parse_named(text, false, owner, path),
        serde_json::Value::Object(entry) => {
            let id = crate::json::required_str(entry, "id", path)?;
            // `required` defaults to true, so an entry is optional only when the pack
            // says so explicitly.
            let required = match entry.get("required") {
                None => true,
                Some(serde_json::Value::Bool(flag)) => *flag,
                Some(other) => {
                    return Err(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: format!(
                            "tag entry \"required\" must be a boolean, found {}",
                            crate::json::type_name(other)
                        ),
                    });
                }
            };
            parse_named(id, !required, owner, path)
        }
        other => Err(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "a tag value must be a string or an object, found {}",
                crate::json::type_name(other)
            ),
        }),
    }
}

/// Parse a tag value or reference, resolving a missing namespace to the owner's.
///
/// A reference is always within the **same registry** as the tag containing it: the
/// format has no way to name another registry, which is why the registry is inherited
/// rather than parsed from the string.
fn parse_named(
    text: &str,
    optional: bool,
    owner: &TagKey,
    path: &Path,
) -> Result<TagEntry, JsonError> {
    let invalid = |reason: String| JsonError::Invalid {
        path: path.to_path_buf(),
        reason,
    };
    let (is_reference, rest) = match text.strip_prefix('#') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    // Vanilla always namespaces its values, but the format allows omitting it.
    let qualified = if rest.contains(':') {
        rest.to_owned()
    } else {
        format!("{}:{rest}", owner.name.namespace())
    };
    let id = ResourceId::parse(&qualified)
        .map_err(|error| invalid(format!("tag value {text:?} is not a valid name: {error}")))?;
    Ok(match (is_reference, optional) {
        (true, false) => TagEntry::Tag(TagKey::new(owner.registry.clone(), id)),
        (true, true) => TagEntry::OptionalTag(TagKey::new(owner.registry.clone(), id)),
        (false, false) => TagEntry::Id(id),
        (false, true) => TagEntry::OptionalId(id),
    })
}

/// Every `.json` file under `dir`, in a deterministic order.
///
/// Iterative and sorted, so the traversal is reproducible (AGENTS.md §3.6) and a
/// deeply nested tree cannot overflow the stack. A missing directory yields nothing,
/// because most packs do not have every directory.
#[must_use]
pub fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths.into_iter().rev() {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
