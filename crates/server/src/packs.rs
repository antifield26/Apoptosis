//! Loading the world's data packs, so the enabled list actually gates something (P07-12).
//!
//! ## What was missing
//!
//! `mc-data` could discover packs and honour `level.dat`'s `DataPacks` list, and the server had a
//! `/function` command — but **nothing connected them**: no pack was ever loaded, so the enabled
//! list gated nothing and `/function` could only run functions a test had injected. That is the
//! gap this closes, and it is why the enabled-list work was worth doing at all.
//!
//! ## The two pack roots
//!
//! | Root | Where it comes from | Why |
//! |---|---|---|
//! | the **vanilla** pack | `[datapacks] vanilla_data` in the config | the jar's `data/minecraft`, which is Mojang's and therefore **not committed** — so it is a configured path, and a server without it simply has no vanilla data |
//! | **world** packs | `<world>/datapacks/*` | the format's location, filtered by the world's enabled list |
//!
//! ## What "no vanilla data" means, stated rather than implied
//!
//! A server with no `vanilla_data` configured loads **no** vanilla functions, recipes or tags.
//! That is not a degraded mode to hide: it means `/function` reports unknown names, and the reason
//! is a missing configuration rather than a missing file. [`PackLoadOutcome`] carries it so the
//! lifecycle can log it once at startup instead of a player discovering it.
//!
//! ## Why this does not stop the boot
//!
//! A broken pack must not stop the server (AGENTS.md §9), and neither must a missing one: a world
//! that has been running for months must still start on a machine where nobody copied the jar data.
//! Every problem is recorded and reported.

use std::path::{Path, PathBuf};

use mc_core::error::ServerResult;
use mc_data::Limits;
use mc_data::enabled::EnabledPacks;
use mc_data::pack::{DataPackSet, PackSource};

use crate::functions::load_functions;
use crate::game::Game;

/// The subdirectory a pack keeps its functions in.
pub const FUNCTION_DIRECTORY: &str = "function";

/// What loading packs produced, for one startup log line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackLoadOutcome {
    /// How many packs loaded.
    pub packs_loaded: usize,
    /// Namespaces contributed, deduplicated and sorted.
    pub namespaces: Vec<String>,
    /// Packs deliberately not loaded because the world disables them, with the reason.
    pub skipped: Vec<String>,
    /// Packs that could not be opened, with the reason.
    pub rejected: Vec<String>,
    /// Whether a vanilla data pack was configured and found.
    pub vanilla_data: bool,
    /// How many functions were loaded across every pack.
    pub functions_loaded: usize,
}

impl PackLoadOutcome {
    /// Whether anything went wrong or was left out.
    ///
    /// A world that disables a pack is not a problem, so `skipped` alone does not make this true —
    /// only a rejection, which means a pack the world *wanted* could not be read.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        !self.rejected.is_empty()
    }

    /// A one-line summary for a startup log.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} pack(s), {} function(s), {} namespace(s)",
            self.packs_loaded,
            self.functions_loaded,
            self.namespaces.len()
        );
        if !self.vanilla_data {
            text.push_str("; no vanilla data pack configured");
        }
        if !self.skipped.is_empty() {
            text.push_str(&format!("; {} pack(s) disabled", self.skipped.len()));
        }
        if !self.rejected.is_empty() {
            text.push_str(&format!("; {} pack(s) unreadable", self.rejected.len()));
        }
        text
    }
}

/// The pack roots a server loads from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRoots {
    /// The vanilla data directory — the jar's `data/minecraft` — when configured.
    pub vanilla_data: Option<PathBuf>,
    /// The world directory, whose `datapacks/` subdirectory holds world packs.
    pub world_dir: PathBuf,
}

impl PackRoots {
    /// Roots for a world with no configured vanilla data.
    #[must_use]
    pub fn new(world_dir: impl Into<PathBuf>) -> Self {
        Self {
            vanilla_data: None,
            world_dir: world_dir.into(),
        }
    }

    /// The same roots with a vanilla data directory.
    #[must_use]
    pub fn with_vanilla_data(mut self, path: impl Into<PathBuf>) -> Self {
        self.vanilla_data = Some(path.into());
        self
    }
}

/// Discover and load a world's packs into a game.
///
/// The enabled list comes from the world's `level.dat`; pass [`EnabledPacks::default`] for a world
/// with no list, which enables everything.
///
/// # Errors
///
/// Only for a failure that makes the *game* unusable. A pack problem never propagates: it is
/// recorded in the returned outcome, because a broken pack must not stop the server.
pub fn load_packs(
    game: &mut Game,
    roots: &PackRoots,
    enabled: &EnabledPacks,
) -> ServerResult<PackLoadOutcome> {
    let mut outcome = PackLoadOutcome::default();
    let mut set = DataPackSet::new();

    // The vanilla pack first, so a world pack overrides it — `DataPackSet` keeps load order
    // lowest-priority-first, and the plan is applied in that order.
    if let Some(vanilla) = &roots.vanilla_data {
        // A configured path that does not exist is a **configuration** mistake, and saying so is
        // better than silently loading nothing: the symptom would be "every recipe is missing".
        if vanilla.is_dir() {
            // `BuiltIn`, which is what the jar's `data/minecraft` is: a pack shipped inside the
            // server rather than one a world or an operator supplied.
            set.push(vanilla, PackSource::BuiltIn, Limits::DEFAULT);
            outcome.vanilla_data = true;
        } else {
            outcome.rejected.push(format!(
                "{}: the configured vanilla data directory does not exist",
                vanilla.display()
            ));
        }
    }

    // Then the world's packs, filtered by what the world enables.
    set.discover_world_packs_enabled(&roots.world_dir, enabled, Limits::DEFAULT);

    outcome.packs_loaded = set.len();
    outcome.namespaces = set.namespaces();
    outcome.skipped = set.skipped().to_vec();
    outcome.rejected.extend(set.rejected().iter().cloned());

    // Load functions from **every** pack, in load order, so a world pack's function of the same
    // name overrides the vanilla one it was loaded after.
    let mut functions = mc_data::function::FunctionRegistry::new();
    for (namespace, root, _source) in set.load_plan() {
        // The namespace is used, not discarded. An earlier version had `let _ = namespace;` here
        // and hard-coded `minecraft:` when naming functions, so every function in every pack was
        // named `minecraft:<path>` and a world pack's functions were unreachable.
        let function_root = root.join(FUNCTION_DIRECTORY);
        if !function_root.is_dir() {
            continue;
        }
        let loaded = load_functions(&function_root, &namespace)?;
        // `insert` is last-wins for a duplicate name, which is the override the load order is for.
        for file in loaded.functions() {
            functions.insert(file.clone());
        }
    }
    outcome.functions_loaded = functions.len();
    game.set_functions(functions);

    Ok(outcome)
}

/// Whether a path looks like an extracted vanilla data directory.
///
/// Used by the lifecycle to warn about a `vanilla_data` value that exists but points at the wrong
/// thing — a directory containing `data/` rather than being `data/minecraft` is the easy mistake,
/// and the symptom (no vanilla functions) is identical to having configured nothing.
#[must_use]
pub fn looks_like_vanilla_data(path: &Path) -> bool {
    // Either `data/minecraft` itself (has `tags/` and `recipe/`) or a pack root containing it.
    let direct = path.join("tags").is_dir() && path.join("recipe").is_dir();
    let nested = path.join("data").join("minecraft").join("tags").is_dir();
    direct || nested
}

/// The vanilla data directory inside a pack root, if this is a root rather than the data itself.
///
/// A caller who points `vanilla_data` at a directory *containing* `data/minecraft` should not have
/// to know which level to name, and the two levels are indistinguishable from the outside.
#[must_use]
pub fn resolve_vanilla_data(path: &Path) -> PathBuf {
    let nested = path.join("data").join("minecraft");
    if nested.join("tags").is_dir() {
        nested
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::{PackLoadOutcome, PackRoots, looks_like_vanilla_data, resolve_vanilla_data};
    use mc_test_support::fixtures::TempDir;

    #[test]
    fn pack_roots_default_to_no_vanilla_data() {
        let roots = PackRoots::new("world");
        assert!(roots.vanilla_data.is_none());
        assert_eq!(roots.world_dir, std::path::Path::new("world"));

        let with = roots.with_vanilla_data("/srv/data/minecraft");
        assert_eq!(
            with.vanilla_data.as_deref(),
            Some(std::path::Path::new("/srv/data/minecraft"))
        );
    }

    #[test]
    fn vanilla_data_is_recognised_at_either_level() {
        let dir = TempDir::new("packs-vanilla");
        // The data itself.
        let data = dir.path().join("minecraft");
        std::fs::create_dir_all(data.join("tags")).expect("tags");
        std::fs::create_dir_all(data.join("recipe")).expect("recipe");
        assert!(looks_like_vanilla_data(&data), "data/minecraft itself");

        // A pack root containing it.
        let root = dir.path().join("pack");
        std::fs::create_dir_all(root.join("data").join("minecraft").join("tags")).expect("nested");
        assert!(
            looks_like_vanilla_data(&root),
            "a root containing data/minecraft"
        );

        // Something else entirely.
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).expect("other");
        assert!(!looks_like_vanilla_data(&other));
        assert!(!looks_like_vanilla_data(&dir.path().join("does_not_exist")));
    }

    #[test]
    fn resolve_finds_the_nested_data_directory() {
        // A caller should not have to know which level to name — the two are indistinguishable
        // from outside, and the symptom of getting it wrong is the same as configuring nothing.
        let dir = TempDir::new("packs-resolve");
        let root = dir.path().join("pack");
        let nested = root.join("data").join("minecraft");
        std::fs::create_dir_all(nested.join("tags")).expect("tags");
        assert_eq!(resolve_vanilla_data(&root), nested);

        // Given the data itself, it is returned unchanged.
        let data = dir.path().join("direct");
        std::fs::create_dir_all(data.join("tags")).expect("tags");
        assert_eq!(resolve_vanilla_data(&data), data);
    }

    #[test]
    fn an_outcome_reports_only_rejections_as_problems() {
        // A world that disables a pack is working as intended; a pack that cannot be read is not.
        let mut outcome = PackLoadOutcome::default();
        assert!(!outcome.has_problems());
        assert!(
            outcome
                .summary()
                .contains("no vanilla data pack configured")
        );

        outcome.skipped.push("beta: disabled".to_owned());
        assert!(
            !outcome.has_problems(),
            "a disabled pack is not a problem: {}",
            outcome.summary()
        );
        assert!(outcome.summary().contains("1 pack(s) disabled"));

        outcome.rejected.push("broken: unreadable".to_owned());
        assert!(outcome.has_problems());
        assert!(outcome.summary().contains("1 pack(s) unreadable"));
    }

    #[test]
    fn a_summary_accounts_for_everything_it_loaded() {
        let outcome = PackLoadOutcome {
            packs_loaded: 3,
            namespaces: vec!["minecraft".to_owned(), "test".to_owned()],
            vanilla_data: true,
            functions_loaded: 7,
            ..PackLoadOutcome::default()
        };
        let summary = outcome.summary();
        assert!(summary.contains('3'), "{summary}");
        assert!(summary.contains('7'), "{summary}");
        assert!(summary.contains('2'), "{summary}");
        assert!(
            !summary.contains("no vanilla data"),
            "vanilla data was loaded: {summary}"
        );
    }
}
