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
//! A server with no `vanilla_data` configured loads **no** vanilla functions and **no** structures.
//! That is not a degraded mode to hide: it means `/function` reports unknown names and every generated
//! chunk is bare terrain, and the reason is a missing configuration rather than a missing file.
//! [`PackLoadOutcome`] carries it so the lifecycle can log it once at startup instead of a player
//! discovering it.
//!
//! ## What this does **not** load, stated because the earlier wording claimed otherwise
//!
//! **Recipes and tags.** `mc-data` parses both and the differential suites assert their census against
//! the real pack, but nothing here installs them: a server loads no recipes whether or not `vanilla_data`
//! is set, and no tag lookup is reachable from gameplay. The consequence is concrete — **a furnace
//! smelts from the hand-written Phase 06 baseline, not from the pack**, which is why P07-09's status is
//! "the table is built and tested" rather than "the server smelts from data".
//!
//! An earlier version of this comment said the loader handled "functions, recipes or tags". It handled
//! functions. That is the same failure this project keeps finding — a description of an intention, three
//! lines above code that does something narrower — and it is why the sentence now names what is absent.
//!
//! ## Why this does not stop the boot
//!
//! A broken pack must not stop the server (AGENTS.md §9), and neither must a missing one: a world
//! that has been running for months must still start on a machine where nobody copied the jar data.
//! Every problem is recorded and reported.

use std::fmt::Write as _;
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
    /// How many structure files the packs hold.
    pub structures_seen: usize,
    /// How many structure templates loaded.
    pub structures_loaded: usize,
    /// Structure files that were refused, with the reason.
    pub structures_refused: Vec<String>,
    /// How many loot tables loaded across every pack (P11-04).
    pub loot_tables_loaded: usize,
    /// Loot table files seen, loaded or not (P11-04).
    pub loot_files_seen: usize,
    /// `(unmodelled kind, count)` pairs from the loot load, so what the drop
    /// authority does not cover is on the record (P11-04).
    pub loot_unmodelled: Vec<(String, usize)>,
    /// Loot table files that were refused, with the reason (P11-04).
    pub loot_refused: Vec<String>,
    /// How many recipes loaded across every pack (P12-07/08).
    pub recipes_loaded: usize,
    /// Crafting recipes converted to the table (P12-07).
    pub crafting_converted: usize,
    /// Crafting recipes skipped for tags/unknowns (P12-07).
    pub crafting_skipped: usize,
    /// Smelting rows converted for the furnace table (P12-08).
    pub smelting_rows: usize,
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
        // `write!` rather than `push_str(&format!(...))`: one allocation instead of two.
        if !self.skipped.is_empty() {
            let _ = write!(text, "; {} pack(s) disabled", self.skipped.len());
        }
        if !self.rejected.is_empty() {
            let _ = write!(text, "; {} pack(s) unreadable", self.rejected.len());
        }
        if self.structures_seen > 0 {
            let _ = write!(
                text,
                "; {} of {} structure templates ({} refused)",
                self.structures_loaded,
                self.structures_seen,
                self.structures_refused.len()
            );
        }
        if self.recipes_loaded > 0 {
            let _ = write!(
                text,
                "; {} recipe(s), {} crafting converted ({} skipped), {} smelting rows",
                self.recipes_loaded,
                self.crafting_converted,
                self.crafting_skipped,
                self.smelting_rows
            );
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
    ///
    /// The path is **resolved here**, so a caller may name the pack root, the `data` directory or the
    /// `minecraft` namespace directory and get the same result. Resolving it in the callers instead
    /// meant one of them forgot — my own wiring test — and the symptom was a pack that loaded zero
    /// namespaces: silent, and indistinguishable from a pack with no data.
    #[must_use]
    pub fn with_vanilla_data(mut self, path: impl Into<PathBuf>) -> Self {
        self.vanilla_data = Some(resolve_vanilla_data(&path.into()));
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
#[allow(clippy::too_many_lines)]
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
        // A root with no `data/` is a pack that loads and contributes nothing, which is the
        // silent-nothing failure this module exists to prevent — so it is rejected with a reason
        // rather than pushed. The world-pack path already checks this; the vanilla path did not.
        if vanilla.join("data").is_dir() {
            // `BuiltIn`, which is what the jar's `data/minecraft` is: a pack shipped inside the
            // server rather than one a world or an operator supplied.
            set.push(vanilla, PackSource::BuiltIn, Limits::DEFAULT);
            outcome.vanilla_data = true;
        } else if vanilla.is_dir() {
            outcome.rejected.push(format!(
                "{}: has no data/ directory, so it is not a pack root; point vanilla_data at the \
                 directory containing data/minecraft",
                vanilla.display()
            ));
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

    // Structures, from every pack's `structure/` directory in load order.
    let mut structures = mc_worldgen::structures::StructureRegistry::new();
    for (_namespace, root, _source) in set.load_plan() {
        let structure_root = root.join("structure");
        if !structure_root.is_dir() {
            continue;
        }
        let (loaded, report) = mc_worldgen::structures::load_structures(
            &structure_root,
            mc_worldgen::StructureLimits::PACK,
        );
        outcome.structures_seen += report.files;
        outcome.structures_loaded += report.loaded;
        for (path, reason) in &report.skipped {
            outcome
                .structures_refused
                .push(format!("{}: {reason}", path.display()));
        }
        for name in loaded.names() {
            if let Some(template) = loaded.by_name(&name) {
                structures.insert(name, template.clone());
            }
        }
    }
    game.set_structures(structures);

    // Loot, from every pack's `loot_table/` directory in load order (P11-04).
    // The registries are read once here; `load_directory` only consults them
    // for validation reports. Last-wins insert keeps the override semantics.
    let mut loot = mc_data::loot::LootTables::new();
    let mut loot_report = mc_data::loot::LootLoadReport::default();
    let blocks = game.registries().blocks.clone();
    let items = game.registries().items.clone();
    for (namespace, root, _source) in set.load_plan() {
        if !root.join("loot_table").is_dir() {
            continue;
        }
        let loaded = mc_data::loot::load_directory(
            &root,
            &namespace,
            mc_data::Limits::DEFAULT,
            Some(&items),
            Some(&blocks),
            &mut loot_report,
        );
        for table in loaded {
            loot.insert(table);
        }
    }
    outcome.loot_tables_loaded = loot.len();
    outcome.loot_files_seen = loot_report.files;
    outcome.loot_unmodelled = loot_report
        .unmodelled
        .iter()
        .map(|(kind, count)| (kind.clone(), *count))
        .collect();
    outcome.loot_refused.clone_from(&loot_report.skipped);
    game.set_loot(loot);

    // Recipes, from every pack's `recipe/` directory in load order (P12-07/08).
    // Last-wins insert keeps pack overrides; the conversions below count what
    // they could not represent rather than dropping it silently.
    let mut book = mc_data::RecipeBook::new();
    let mut recipe_report = mc_data::RecipeLoadReport::default();
    // `load_plan` yields `(namespace, namespace_dir)` where the directory is
    // already `.../data/<ns>` — the loot/function/structure loops use it
    // directly, and recipes must too (`root.join(namespace)` would be
    // `.../data/<ns>/<ns>`, which never exists and silently loads zero).
    for (namespace, root, _source) in set.load_plan() {
        // `load_directory` expects the namespace directory (`.../data/<ns>`).
        let namespace_dir = root;
        if !namespace_dir.join("recipe").is_dir() {
            continue;
        }
        for recipe in mc_data::recipe::load_directory(
            &namespace_dir,
            &namespace,
            mc_data::Limits::DEFAULT,
            &mut recipe_report,
        ) {
            book.insert(recipe);
        }
    }
    outcome.recipes_loaded = book.len();
    // Crafting table: item-only conversion (tags counted, not guessed).
    match mc_container::RecipeRegistry::from_book(&book, &items) {
        Ok((table, report)) => {
            outcome.crafting_converted = report.converted;
            outcome.crafting_skipped =
                report.tag_or_unknown + report.malformed.len() + report.other_kinds;
            // An empty pack (no packs configured) keeps the baseline: replacing
            // a working table with nothing would uncraft sticks.
            if !table.is_empty() {
                game.set_crafting_registry(table);
            }
        }
        Err(error) => {
            outcome
                .rejected
                .push(format!("crafting table conversion refused: {error}"));
        }
    }
    // Furnace table: smelting-kind rows from the same book (P12-08). Fuel
    // values stay the jar-verified baseline; only recipes come from data.
    match mc_container::SmeltingRegistry::from_recipes(
        &book,
        mc_data::SmeltingKind::Smelting,
        &items,
        None,
    ) {
        Ok((table, report)) => {
            outcome.smelting_rows = report.rows;
            if !table.is_empty() {
                game.set_smelting_furnace(table);
            }
        }
        Err(error) => {
            outcome
                .rejected
                .push(format!("smelting table conversion refused: {error}"));
        }
    }

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

/// The pack root that holds the vanilla data, from any of the three forms a caller might name.
///
/// A `DataPack` is a directory **containing** `data/<namespace>/`, so the root is what the loader needs
/// — but there are three natural ways to point at Mojang's data, and a caller should not have to know
/// which one the pack model wants:
///
/// | Caller names | Root |
/// |---|---|
/// | `…/extract` (containing `data/minecraft`) | `…/extract` |
/// | `…/extract/data` | `…/extract` |
/// | `…/extract/data/minecraft` (the namespace directory itself) | `…/extract` |
///
/// The third is the one `MC_VANILLA_DATA` uses in every differential test, and getting it wrong is
/// **silent**: the pack loads and contributes zero namespaces. So the rule is explicit here and
/// `looks_like_vanilla_data` reports a path that matches none of the three.
#[must_use]
pub fn resolve_vanilla_data(path: &Path) -> PathBuf {
    // The namespace directory itself: `…/data/minecraft` means the root is `…`.
    let is_namespace_dir = path.file_name().is_some_and(|name| name == "minecraft")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "data");
    if is_namespace_dir {
        return path
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf);
    }
    // The `data` directory itself.
    if path.file_name().is_some_and(|name| name == "data") {
        return path
            .parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf);
    }
    // A root containing `data/minecraft`, or something unrecognised — returned unchanged so the
    // caller's own check can report it.
    path.to_path_buf()
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

        // The setter **resolves**, so a caller may name any of the three forms. `/srv/data/minecraft`
        // is the namespace directory, whose pack root is `/srv`.
        let with = roots.with_vanilla_data("/srv/data/minecraft");
        assert_eq!(
            with.vanilla_data.as_deref(),
            Some(std::path::Path::new("/srv")),
            "the namespace-directory form resolves to its pack root"
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
    fn every_accepted_form_resolves_to_the_same_pack_root() {
        // **The property that matters**, and the one the previous version of this test did not check:
        // a caller may name any of three forms and must get the same root.
        //
        // The third form is the one `MC_VANILLA_DATA` uses — the namespace directory itself — and it was
        // the one the old contract rejected. Rejecting it was **silent**: the pack loaded and contributed
        // zero namespaces, so a fully configured server saw no structures and no functions.
        let dir = TempDir::new("packs-resolve");
        let root = dir.path().join("pack");
        let data = root.join("data");
        let namespace = data.join("minecraft");
        std::fs::create_dir_all(namespace.join("tags")).expect("tags");
        std::fs::create_dir_all(namespace.join("structure")).expect("structure");

        for (form, named) in [
            ("the pack root", root.clone()),
            ("the data directory", data.clone()),
            ("the namespace directory", namespace.clone()),
        ] {
            assert_eq!(
                resolve_vanilla_data(&named),
                root,
                "naming {form} must resolve to the pack root"
            );
        }

        // An unrecognised path is returned unchanged, so the caller's own check reports it rather than
        // this function silently inventing a root.
        let stray = dir.path().join("stray");
        std::fs::create_dir_all(&stray).expect("stray");
        assert_eq!(resolve_vanilla_data(&stray), stray);
    }

    #[test]
    fn a_namespace_directory_is_recognised_only_under_data() {
        // The rule is "a path named `minecraft` whose parent is named `data`". A directory named
        // `minecraft` anywhere else must not be mistaken for one, or a stray path would resolve to a
        // root that does not exist and the pack would load nothing — silently, again.
        let dir = TempDir::new("packs-namespace-rule");
        let elsewhere = dir.path().join("minecraft");
        std::fs::create_dir_all(&elsewhere).expect("dir");
        assert_eq!(
            resolve_vanilla_data(&elsewhere),
            elsewhere,
            "`minecraft` outside a `data` directory is not a namespace directory"
        );
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
