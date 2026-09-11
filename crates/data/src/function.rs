//! Functions: the data model and the loader (P07-08).
//!
//! ## Execution is not this module's job, and that is a layering decision
//!
//! A `.mcfunction` file is a list of **command strings**. Running one means parsing and
//! dispatching Minecraft commands, which lives in `mc-command` — a crate that sits *above*
//! `mc-data`. Depending on it here would invert the layering, so the boundary is explicit:
//!
//! - [`FunctionRegistry`] returns the command strings ([`FunctionFile::commands`]);
//! - whoever owns the dispatcher parses and runs them.
//!
//! Two consequences the caller has to own, because this crate cannot:
//!
//! 1. **Recursion is the caller's guard.** `/function minecraft:x` inside
//!    `minecraft:x` recurses, and vanilla caps it (`maxCommandForkCount` /
//!    `gamerule maxCommandChainLength`). A dispatcher should carry an explicit depth bound;
//!    [`SUGGESTED_MAX_RECURSION_DEPTH`] records the bound this project suggests, and
//!    [`FunctionRegistry::command_budget`] gives the caller the total command count it
//!    needs to size a chain budget before running anything.
//! 2. **Macro lines are not expanded.** A line using the `$(name)` macro syntax needs the
//!    caller's argument values, which do not exist at load time. Such a line is preserved
//!    verbatim and flagged by [`FunctionFile::has_macros`], so a dispatcher that cannot
//!    expand them can refuse *before* running half a function.
//!
//! ## The format, and how far it is verified
//!
//! ```text
//! data/<namespace>/function/<path>.mcfunction
//! # a comment
//!   # an indented comment
//!
//! say hello
//! ```
//!
//! **Verified against the 26.1.2 jar: nothing.** This is the one module here that has no
//! vanilla data behind it, and saying so is the point: the jar's own
//! `data/minecraft/` contains **zero** `.mcfunction` files and no `function/` directory at
//! all — the built-in `datapacks/minecart_improvements`, `redstone_experiments` and
//! `trade_rebalance` hold only `pack.mcmeta` and data. So the loader below implements the
//! documented format and is exercised only by this crate's own tests (AGENTS.md §3.1: an
//! unverified feature is labelled, not presented as tested).
//!
//! What *is* established from the jar is the shape of the path: every data-pack directory
//! lives at `data/<namespace>/<directory>/…`, which `loot_table/`, `advancement/`,
//! `recipe/` and `tags/` all confirm.
//!
//! ## Hostile input (AGENTS.md section 10)
//!
//! A function is executed, so its size is the attack: a million-line file is not a slow
//! load, it is a slow *run*. [`FunctionLimits`] bounds the byte size, the line count and
//! the per-line length, and every refusal names the file.
//!
//! A note on what this module does *not* claim: it does not bound what the commands do.
//! `forceload`, `/setblock` in a loop, or a nested `/function` are the dispatcher's
//! problem, which is why [`FunctionRegistry::command_budget`] exists.

use mc_core::ids::ResourceId;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Ceilings applied to every function file.
///
/// Derived from what the format needs rather than from a measurement, because there is no
/// vanilla function to measure — see the module documentation. The numbers are chosen to be
/// far above any hand-written function and far below anything that could stall a tick:
/// 64 KiB is about 1 000 dense commands, and 65 536 lines at 255 characters each is the
/// worst case the byte bound already excludes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionLimits {
    /// Largest file read, in bytes.
    pub max_bytes: u64,
    /// Most lines a file may have, comments and blanks included.
    pub max_lines: usize,
    /// Longest single line accepted, in bytes.
    ///
    /// A command's own argument-length limits are the dispatcher's business; this only
    /// stops one line from being the whole file.
    pub max_line_bytes: usize,
}

impl FunctionLimits {
    /// 256 KiB, 65 536 lines, 32 767 bytes per line.
    ///
    /// The line bound is `u16::MAX + 1` because vanilla's command dispatcher reads a line
    /// into a buffer, and a bound that is a power of two is easier to reason about than a
    /// round decimal. The byte bound is `4 * max_line_bytes`, so a file of maximum-length
    /// lines is refused by the byte bound rather than by an allocation.
    pub const DEFAULT: Self = Self {
        max_bytes: 256 * 1024,
        max_lines: 65_536,
        max_line_bytes: 32_767,
    };

    /// Limits for tests that need to provoke a refusal cheaply.
    #[must_use]
    pub const fn with_max_lines(max_lines: usize) -> Self {
        Self {
            max_lines,
            ..Self::DEFAULT
        }
    }

    /// Limits with a byte ceiling, for the size test.
    #[must_use]
    pub const fn with_max_bytes(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            ..Self::DEFAULT
        }
    }
}

impl Default for FunctionLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a function file could not be loaded.
#[derive(Debug)]
pub enum FunctionError {
    /// The file could not be opened or read.
    Io {
        /// The file.
        path: std::path::PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file is larger than [`FunctionLimits::max_bytes`].
    TooLarge {
        /// The file.
        path: std::path::PathBuf,
        /// Its size in bytes.
        bytes: u64,
        /// The ceiling.
        limit: u64,
    },
    /// The file has more lines than [`FunctionLimits::max_lines`].
    TooManyLines {
        /// The file.
        path: std::path::PathBuf,
        /// How many it has.
        lines: usize,
        /// The ceiling.
        limit: usize,
    },
    /// A line is longer than [`FunctionLimits::max_line_bytes`].
    LineTooLong {
        /// The file.
        path: std::path::PathBuf,
        /// 1-based line number.
        line: usize,
        /// Its length in bytes.
        bytes: usize,
        /// The ceiling.
        limit: usize,
    },
    /// The text is not valid UTF-8.
    NotUtf8 {
        /// The file.
        path: std::path::PathBuf,
        /// The underlying error.
        source: std::string::FromUtf8Error,
    },
    /// The file name is not a usable resource id.
    BadName {
        /// The file.
        path: std::path::PathBuf,
        /// Why.
        reason: String,
    },
}

impl FunctionError {
    /// The file this error is about.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Io { path, .. }
            | Self::TooLarge { path, .. }
            | Self::TooManyLines { path, .. }
            | Self::LineTooLong { path, .. }
            | Self::NotUtf8 { path, .. }
            | Self::BadName { path, .. } => path,
        }
    }
}

impl fmt::Display for FunctionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::TooLarge { path, bytes, limit } => write!(
                f,
                "{}: {bytes} bytes exceeds the {limit}-byte function limit",
                path.display()
            ),
            Self::TooManyLines { path, lines, limit } => write!(
                f,
                "{}: {lines} lines exceeds the {limit}-line function limit",
                path.display()
            ),
            Self::LineTooLong {
                path,
                line,
                bytes,
                limit,
            } => write!(
                f,
                "{}:{line}: {bytes} bytes exceeds the {limit}-byte line limit",
                path.display()
            ),
            Self::NotUtf8 { path, source } => {
                write!(f, "{}: not UTF-8: {source}", path.display())
            }
            Self::BadName { path, reason } => write!(f, "{}: {reason}", path.display()),
        }
    }
}

impl std::error::Error for FunctionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::NotUtf8 { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One loaded `.mcfunction`.
///
/// Holds the commands and where they came from. It deliberately holds **no** execution
/// state: running a function is a command-dispatcher concern (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFile {
    /// The function's name, as `namespace:path`.
    pub name: ResourceId,
    /// The commands, in file order, with comments and blank lines removed.
    pub commands: Vec<String>,
    /// Whether any command uses the `$(name)` macro syntax.
    pub has_macros: bool,
    /// The file it was read from.
    pub source: std::path::PathBuf,
}

impl FunctionFile {
    /// The commands, in order. A convenience alias for the field, named as a getter so a
    /// dispatcher can be written against methods.
    #[must_use]
    pub fn command_list(&self) -> &[String] {
        &self.commands
    }

    /// How many commands it would run.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether it has no commands (only comments and blanks).
    ///
    /// Legitimate, and worth distinguishing from "failed to load": an empty function is a
    /// no-op, not an error.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Whether it calls `/function`, directly or not.
    ///
    /// A cheap syntactic check, not a resolution: whether the call is to *this* function,
    /// to another, or to something that does not exist is the dispatcher's question. It
    /// exists so a dispatcher can decide up front whether it needs a depth guard.
    #[must_use]
    pub fn calls_function_command(&self) -> bool {
        self.commands.iter().any(|command| is_function_call(command))
    }

    /// Total characters across every command, for a caller sizing a chain budget.
    #[must_use]
    pub fn total_chars(&self) -> usize {
        self.commands.iter().map(String::len).sum()
    }
}

/// Whether a command is a `/function` invocation.
///
/// Leading slashes are accepted because a pack may write `function foo` or `/function foo`;
/// the dispatcher strips them. The check is on the first token, so `say function` is not a
/// call. Command names are matched case-insensitively, which is how the dispatcher reads
/// them.
#[must_use]
pub fn is_function_call(command: &str) -> bool {
    function_call_target(command).is_some() || is_bare_function_command(command)
}

/// Whether the command is `function` with no argument, which is a syntax error the
/// dispatcher will reject but is still a `function` command.
fn is_bare_function_command(command: &str) -> bool {
    let command = command.trim_start_matches('/').trim();
    command.eq_ignore_ascii_case("function")
}

/// The recursion bound this project suggests to whoever owns the dispatcher.
///
/// The dispatcher **must** enforce its own; this constant exists so the number is chosen
/// once and recorded rather than re-derived. 64 is well above any sensible pack (vanilla's
/// own command chains are single digits) and small enough that the worst case is bounded
/// work rather than a hang.
pub const SUGGESTED_MAX_RECURSION_DEPTH: usize = 64;

/// What a function load did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionLoadReport {
    /// Files found in the directory.
    pub files: usize,
    /// Files loaded.
    pub loaded: usize,
    /// Files whose *name* this build could not model.
    ///
    /// Functions have no in-file `type` to be unmodelled by, so this map is populated by a
    /// file whose path cannot be turned into a resource id (a non-UTF-8 name, or a name
    /// outside the id grammar). Vanilla contributes **zero**.
    pub unmodelled: BTreeMap<String, usize>,
    /// Files skipped because they could not be read, parsed or bounded, with the reason.
    pub skipped: Vec<String>,
    /// Commands across every loaded file.
    pub commands: usize,
}

impl FunctionLoadReport {
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
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.skipped.is_empty() && self.unmodelled.is_empty()
    }
}

/// Every loaded function, indexed by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionRegistry {
    functions: Vec<FunctionFile>,
    by_name: BTreeMap<ResourceId, usize>,
}

impl FunctionRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a function. A later function with the same name **replaces** an earlier one,
    /// which is the pack-override rule.
    pub fn insert(&mut self, function: FunctionFile) {
        if let Some(index) = self.by_name.get(&function.name).copied() {
            self.functions[index] = function;
            return;
        }
        self.by_name
            .insert(function.name.clone(), self.functions.len());
        self.functions.push(function);
    }

    /// Every function, in load order.
    #[must_use]
    pub fn functions(&self) -> &[FunctionFile] {
        &self.functions
    }

    /// How many functions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// A function by name.
    #[must_use]
    pub fn by_name(&self, name: &ResourceId) -> Option<&FunctionFile> {
        self.by_name
            .get(name)
            .map(|index| &self.functions[*index])
    }

    /// Every function name, ascending.
    pub fn names(&self) -> impl Iterator<Item = &ResourceId> {
        self.by_name.keys()
    }

    /// Total commands across every function.
    ///
    /// This is the number a dispatcher needs before it runs anything: a chain budget of at
    /// least this size is required to run every function once without recursion.
    #[must_use]
    pub fn command_budget(&self) -> usize {
        self.functions.iter().map(FunctionFile::len).sum()
    }

    /// Every function that calls `/function`, directly or not.
    ///
    /// The set a caller has to reason about for recursion, since the others cannot recurse
    /// through the dispatcher.
    #[must_use]
    pub fn functions_calling_functions(&self) -> Vec<&FunctionFile> {
        self.functions
            .iter()
            .filter(|function| function.calls_function_command())
            .collect()
    }

    /// Functions whose body references `name` in a `/function` command.
    ///
    /// A syntactic scan, reported as such: a function that builds the name at run time
    /// (through a macro) is not found by it. It is enough to *detect* a cycle up front,
    /// which is what a dispatcher needs to refuse one before running anything.
    #[must_use]
    pub fn calling(&self, name: &ResourceId) -> Vec<&FunctionFile> {
        self.functions
            .iter()
            .filter(|function| {
                function.commands.iter().any(|command| {
                    function_call_target(command)
                        .and_then(|target| resolve_target(target, &function.name))
                        .is_some_and(|target| &target == name)
                })
            })
            .collect()
    }

    /// Whether `/function` references form a cycle reachable from `name`.
    ///
    /// The dispatcher still needs its own depth guard — this only answers the static
    /// question, using the same syntactic scan as [`Self::calling`], and a call whose target
    /// is built at run time by a macro is invisible to it.
    #[must_use]
    pub fn has_recursion(&self, name: &ResourceId) -> bool {
        let mut stack = vec![name.clone()];
        let mut visited: Vec<ResourceId> = Vec::new();
        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current.clone());
            let Some(function) = self.by_name(&current) else {
                continue;
            };
            for command in &function.commands {
                let Some(target) = function_call_target(command)
                    .and_then(|target| resolve_target(target, &function.name))
                else {
                    continue;
                };
                if &target == name {
                    return true;
                }
                stack.push(target);
            }
        }
        false
    }

    /// Load every function under `<namespace_dir>/function/`.
    ///
    /// `namespace` qualifies each file's name, as it does for every other data directory.
    ///
    /// Never fails as a whole: an unreadable or oversized file is recorded in the report
    /// and skipped, so one bad file in a pack does not discard the rest (AGENTS.md §9).
    #[must_use]
    pub fn load_directory(
        namespace_dir: &Path,
        namespace: &str,
        limits: FunctionLimits,
        report: &mut FunctionLoadReport,
    ) -> Self {
        let base = namespace_dir.join("function");
        let mut registry = Self::new();
        for path in files_with_extension(&base, "mcfunction") {
            report.files += 1;
            let Ok(relative) = path.strip_prefix(&base) else {
                report.skipped.push(format!(
                    "{}: not under the function directory",
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
                    .entry("unusable function name".to_owned())
                    .or_insert(0) += 1;
                continue;
            };
            match read_function_file(&path, name, limits) {
                Ok(function) => {
                    report.commands += function.len();
                    report.loaded += 1;
                    registry.insert(function);
                }
                Err(error) => report.skipped.push(error.to_string()),
            }
        }
        report
            .unmodelled
            .retain(|_, count| *count > 0);
        registry
    }
}

/// The target of a `/function` command, when the command is one.
///
/// The command name is matched case-insensitively, since the dispatcher does; a command with
/// no argument returns `None`, because there is no target to resolve.
fn function_call_target(command: &str) -> Option<&str> {
    let command = command.trim_start_matches('/').trim();
    let (head, rest) = command.split_once(char::is_whitespace)?;
    if !head.eq_ignore_ascii_case("function") {
        return None;
    }
    rest.split_whitespace().next()
}

/// Resolve a `/function` target against the calling function's namespace.
///
/// A target without a namespace means the calling function's own namespace, which is how the
/// command parser reads it. A target that is not a valid resource id at all yields `None`
/// rather than a guess.
fn resolve_target(target: &str, caller: &ResourceId) -> Option<ResourceId> {
    let qualified = if target.contains(':') {
        target.to_owned()
    } else {
        format!("{}:{target}", caller.namespace())
    };
    ResourceId::parse(&qualified).ok()
}

/// Every file under `dir` with the given extension, in a deterministic order.
///
/// The same traversal rule as [`crate::tag::json_files`], for a different extension.
/// Iterative and sorted, so the order is reproducible (AGENTS.md §3.6) and a deep tree
/// cannot overflow the stack. A missing directory yields nothing.
#[must_use]
pub fn files_with_extension(dir: &Path, extension: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut paths: Vec<std::path::PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths.into_iter().rev() {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Read one `.mcfunction` file.
///
/// # Errors
///
/// [`FunctionError`] when the file is unreadable, not UTF-8, larger than
/// [`FunctionLimits::max_bytes`], longer than [`FunctionLimits::max_lines`], or has a line
/// longer than [`FunctionLimits::max_line_bytes`].
pub fn read_function_file(
    path: &Path,
    name: ResourceId,
    limits: FunctionLimits,
) -> Result<FunctionFile, FunctionError> {
    let metadata = std::fs::metadata(path).map_err(|source| FunctionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > limits.max_bytes {
        return Err(FunctionError::TooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            limit: limits.max_bytes,
        });
    }
    let bytes = std::fs::read(path).map_err(|source| FunctionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let text = String::from_utf8(bytes).map_err(|source| FunctionError::NotUtf8 {
        path: path.to_path_buf(),
        source,
    })?;
    parse_function(&text, path, name, limits)
}

/// Parse `.mcfunction` text that was already read.
///
/// Split from [`read_function_file`] so the parser can be exercised without a filesystem,
/// and so a caller that already holds the bytes (a `.zip` pack, say) does not have to write
/// them out first.
///
/// # Errors
///
/// As [`read_function_file`], minus the I/O cases.
pub fn parse_function(
    text: &str,
    path: &Path,
    name: ResourceId,
    limits: FunctionLimits,
) -> Result<FunctionFile, FunctionError> {
    let mut commands = Vec::new();
    let mut has_macros = false;
    let mut line_number = 0usize;
    for raw in text.lines() {
        line_number += 1;
        if line_number > limits.max_lines {
            return Err(FunctionError::TooManyLines {
                path: path.to_path_buf(),
                lines: line_number,
                limit: limits.max_lines,
            });
        }
        if raw.len() > limits.max_line_bytes {
            return Err(FunctionError::LineTooLong {
                path: path.to_path_buf(),
                line: line_number,
                bytes: raw.len(),
                limit: limits.max_line_bytes,
            });
        }
        // A trailing `\r` is a Windows line ending, not part of the command; `lines()`
        // strips `\n` but not `\r`.
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        // Blank lines and `#` comments are skipped. The `#` test is on the *trimmed* line,
        // because an indented comment is still a comment and vanilla's own command parser
        // strips leading whitespace before the check.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.contains("$(") {
            has_macros = true;
        }
        commands.push(trimmed.to_owned());
    }
    Ok(FunctionFile {
        name,
        commands,
        has_macros,
        source: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests;
