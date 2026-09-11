//! Data pack discovery, ordering and metadata (P07-03, P07-12).
//!
//! ## The pack format, from the jar
//!
//! A pack is a directory with `pack.mcmeta` and a `data/<namespace>/` tree. Vanilla's
//! built-in pack ships as `data/minecraft/` inside the server jar with **8 775 files**
//! across 39 top-level directories, and `data/minecraft/datapacks/` holds 132 further
//! entries — the built-in packs a world gets by default.
//!
//! ## Ordering is the whole point
//!
//! Packs apply in order and a later pack overrides an earlier one **per file**. That
//! is the only mechanism by which a data pack changes vanilla behaviour, so getting
//! the order wrong silently produces the wrong game. The order here is
//! lowest-priority first, so applying in sequence gives "last write wins", and
//! [`DataPackSet::load_order`] is the single place that decides it.
//!
//! ## What is not implemented
//!
//! - **No `.zip` packs.** Vanilla accepts a zip; this reads directories only. A zip
//!   needs an archive reader (a new dependency and a licence review) and no world on
//!   this machine uses one, so it is recorded rather than half-done.
//! - **No `pack_format` enforcement.** The version is read and reported, but a pack
//!   declaring an incompatible format is warned about rather than refused, because
//!   the accepted range for 26.1.2 is not verified.
//! - **No `filter` or `overlay` sections.** Both exist in the format; neither is
//!   modelled, and neither appears in the vanilla pack.
//! - **No pack selection UI or `enabled` list.** A world's
//!   `level.dat → DataPacks` list is not read, so every discovered pack is loaded.

use std::path::{Path, PathBuf};

use crate::json::{JsonError, Limits, optional_i64, optional_str, read_json_object};

/// Where a pack came from, for diagnostics and for the load report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackSource {
    /// Shipped inside the server jar as `data/<namespace>/`.
    BuiltIn,
    /// A directory under the world's `datapacks/`.
    World,
    /// A directory handed to the loader directly (a test or a tool).
    Explicit,
}

impl PackSource {
    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BuiltIn => "builtin",
            Self::World => "world",
            Self::Explicit => "explicit",
        }
    }
}

/// A pack's `pack.mcmeta` contents, as far as this build models them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMetadata {
    /// The `pack_format` the pack declares.
    pub pack_format: Option<i64>,
    /// The human-readable description, if it is a plain string.
    ///
    /// A translatable component (`{"translate": …}`) is legal in the format and
    /// common in real packs; it is reported as `None` rather than flattened to a
    /// wrong string.
    pub description: Option<String>,
}

impl PackMetadata {
    /// Metadata for a pack with no `pack.mcmeta`, which the format allows.
    #[must_use]
    pub const fn absent() -> Self {
        Self {
            pack_format: None,
            description: None,
        }
    }
}

/// Why a pack could not be used.
#[derive(Debug)]
pub enum PackError {
    /// The directory does not exist or is not readable.
    Missing {
        /// The path.
        path: PathBuf,
    },
    /// `pack.mcmeta` exists but is malformed.
    BadMetadata(JsonError),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "{}: not a readable directory", path.display()),
            Self::BadMetadata(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PackError {}

/// One data pack root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPack {
    /// The pack's root directory (the one containing `data/`).
    pub root: PathBuf,
    /// Where it came from.
    pub source: PackSource,
    /// Its metadata, or [`PackMetadata::absent`].
    pub metadata: PackMetadata,
}

impl DataPack {
    /// Open a pack directory, reading `pack.mcmeta` when present.
    ///
    /// A missing `pack.mcmeta` is **not** an error: the format allows an implicit
    /// pack, and the built-in `data/minecraft` directory has no `mcmeta` of its own.
    ///
    /// # Errors
    ///
    /// [`PackError::Missing`] when the directory is not readable, or
    /// [`PackError::BadMetadata`] when `pack.mcmeta` exists but is malformed — a
    /// present-but-broken file is a pack bug worth reporting, unlike an absent one.
    pub fn open(root: &Path, source: PackSource, limits: Limits) -> Result<Self, PackError> {
        if !root.is_dir() {
            return Err(PackError::Missing {
                path: root.to_path_buf(),
            });
        }
        let mcmeta = root.join("pack.mcmeta");
        let metadata = if mcmeta.is_file() {
            read_metadata(&mcmeta, limits).map_err(PackError::BadMetadata)?
        } else {
            PackMetadata::absent()
        };
        Ok(Self {
            root: root.to_path_buf(),
            source,
            metadata,
        })
    }

    /// The `data/` directory inside the pack.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// The namespaces this pack contributes, ascending.
    ///
    /// A namespace is a directory under `data/`. Nameless directories are ignored
    /// rather than reported: `data/` may hold a `.DS_Store` or a stray file, and that
    /// is not a pack error.
    #[must_use]
    pub fn namespaces(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.data_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    /// A namespace's directory inside the pack.
    #[must_use]
    pub fn namespace_dir(&self, namespace: &str) -> PathBuf {
        self.data_dir().join(namespace)
    }
}

fn read_metadata(path: &Path, limits: Limits) -> Result<PackMetadata, JsonError> {
    let map = read_json_object(path, limits)?;
    let pack = match map.get("pack") {
        Some(serde_json::Value::Object(pack)) => pack,
        // `pack.mcmeta` without a `pack` object is malformed; say so rather than
        // silently producing empty metadata.
        Some(other) => {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!(
                    "\"pack\" must be an object, found {}",
                    crate::json::type_name(other)
                ),
            });
        }
        None => {
            return Err(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: "missing the required \"pack\" object".to_owned(),
            });
        }
    };
    Ok(PackMetadata {
        pack_format: optional_i64(pack, "pack_format", path)?,
        description: optional_str(pack, "description", path)?.map(str::to_owned),
    })
}

/// An ordered list of packs, lowest priority first.
#[derive(Debug, Clone, Default)]
pub struct DataPackSet {
    packs: Vec<DataPack>,
    /// Packs that could not be opened, with the reason. Kept so a caller can report
    /// them instead of wondering why a pack had no effect.
    rejected: Vec<String>,
}

impl DataPackSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pack, ignoring one that cannot be opened but recording why.
    ///
    /// A broken pack must not stop the server from starting (AGENTS.md §9): the
    /// remaining packs still load and the failure is visible in
    /// [`DataPackSet::rejected`].
    pub fn push(&mut self, root: &Path, source: PackSource, limits: Limits) {
        match DataPack::open(root, source, limits) {
            Ok(pack) => self.packs.push(pack),
            Err(error) => self.rejected.push(error.to_string()),
        }
    }

    /// Discover the packs under a world's `datapacks/` directory.
    ///
    /// Only *directories* are considered, and they are sorted by name so the load
    /// order is reproducible. Each must contain a `data/` directory to count, which
    /// is what distinguishes a pack from an unrelated folder a player dropped in.
    pub fn discover_world_packs(&mut self, world_root: &Path, limits: Limits) {
        let dir = world_root.join("datapacks");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut candidates: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        candidates.sort();
        for candidate in candidates {
            if candidate.join("data").is_dir() {
                self.push(&candidate, PackSource::World, limits);
            } else {
                self.rejected.push(format!(
                    "{}: no data/ directory, so it is not a data pack",
                    candidate.display()
                ));
            }
        }
    }

    /// The packs, in load order (lowest priority first).
    #[must_use]
    pub fn load_order(&self) -> &[DataPack] {
        &self.packs
    }

    /// Packs that were refused, with the reason.
    #[must_use]
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    /// How many packs loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.packs.len()
    }

    /// Whether no packs loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }

    /// Every namespace any pack contributes, ascending and deduplicated.
    #[must_use]
    pub fn namespaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self.packs.iter().flat_map(DataPack::namespaces).collect();
        names.sort();
        names.dedup();
        names
    }

    /// Every `(namespace, directory)` pair to load, in the order they must be applied.
    ///
    /// This is the function the loader walks: for each pack in order, for each
    /// namespace it contributes. The order it returns *is* the override semantics.
    #[must_use]
    pub fn load_plan(&self) -> Vec<(String, PathBuf, PackSource)> {
        let mut plan = Vec::new();
        for pack in &self.packs {
            for namespace in pack.namespaces() {
                plan.push((
                    namespace.clone(),
                    pack.namespace_dir(&namespace),
                    pack.source,
                ));
            }
        }
        plan
    }
}
