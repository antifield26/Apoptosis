//! Honouring a world's enabled-pack list (P07-12).
//!
//! ## What this fixes
//!
//! `pack.rs` discovered every pack under `world/datapacks/` and loaded all of them. A real world
//! does not work that way: `level.dat`'s `DataPacks.Enabled` list says which packs are on, and a
//! pack the player disabled must **not** take effect. Loading a disabled pack is the most confusing
//! possible failure — the world behaves as if a pack the player turned off were still active, and
//! nothing in the logs explains it.
//!
//! ## The naming rule, measured rather than assumed
//!
//! Vanilla identifies a pack in `Enabled` by its **directory name** for a filesystem pack, and by
//! a `file/<name>.zip` (or plain `<name>.zip`) form for an archive. The built-ins are `vanilla`
//! and the `fabric`-style loaders' additions. So matching is by name, and the three spellings for
//! one pack have to be normalised to a single identity or a legitimately enabled pack looks absent.
//!
//! ## The default, which is the part that bites
//!
//! A world created **before** `DataPacks` existed, or one whose `level.dat` has no `DataPacks`
//! section, has an empty enabled list. Treating that as "nothing is enabled" would disable every
//! pack in a world that has been running for months — so an absent list means **everything is
//! enabled**, which is what Vanilla does for a world with no explicit list. The distinction
//! between "no list" and "an empty list" is therefore carried rather than collapsed, and that is
//! why [`EnabledPacks`] is a three-state type rather than a `Vec`.

use std::collections::BTreeSet;

/// Which packs a world enables.
///
/// Three states, because "no list" and "an empty list" mean opposite things and a `Vec` cannot
/// tell them apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EnabledPacks {
    /// `level.dat` has no `DataPacks` section: **everything is enabled**.
    ///
    /// The default, and the safe one: a world that predates the field, or whose file was written
    /// by another tool, keeps working with all its packs.
    #[default]
    Unspecified,
    /// `level.dat` states a list. Only packs whose name appears in `enabled` load, and any pack
    /// named in `disabled` is refused even if it also appears in `enabled`.
    Listed {
        /// Names the world enables, normalised.
        enabled: BTreeSet<String>,
        /// Names the world disables, normalised.
        disabled: BTreeSet<String>,
    },
}

impl EnabledPacks {
    /// Build from a world's two lists, normalising the names.
    ///
    /// `enabled` is Vanilla's `DataPacks.Enabled`. A list that is present but empty is a real
    /// state — a player who disabled everything — so it is `Listed` with nothing enabled, not
    /// `Unspecified`.
    #[must_use]
    pub fn from_level_dat(enabled: &[String], disabled: &[String]) -> Self {
        if enabled.is_empty() && disabled.is_empty() {
            // Both empty is indistinguishable from the section being absent, and vanilla writes
            // `Enabled: ["vanilla"]` for a normal world — so an entirely empty pair means the
            // world has no list at all.
            return Self::Unspecified;
        }
        Self::Listed {
            enabled: enabled.iter().map(|name| normalise(name)).collect(),
            disabled: disabled.iter().map(|name| normalise(name)).collect(),
        }
    }

    /// Whether a pack with this name should load.
    ///
    /// `directory_name` is the pack's directory (or, for an archive, its file stem) — the
    /// identity Vanilla's list uses.
    #[must_use]
    pub fn allows(&self, directory_name: &str) -> bool {
        let Self::Listed { enabled, disabled } = self else {
            // No list: everything loads. See the type docs for why this is the safe default.
            return true;
        };
        let normalised = normalise(directory_name);
        // A name in both lists is disabled: disabling is the more specific intent, and Vanilla
        // writes a pack there precisely to override an inherited enabled state.
        if disabled.contains(&normalised) {
            return false;
        }
        enabled.contains(&normalised)
    }

    /// Whether this is the "no list" state.
    #[must_use]
    pub const fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }

    /// How many packs are named enabled.
    #[must_use]
    pub fn enabled_count(&self) -> usize {
        match self {
            Self::Unspecified => 0,
            Self::Listed { enabled, .. } => enabled.len(),
        }
    }
}

/// A pack name as the world's list spells it, reduced to one identity.
///
/// Vanilla's `Enabled` entries appear in three forms for one pack — a bare directory name, a
/// `file/<name>` prefix for a world-folder pack, and `<name>.zip` for an archive — so a
/// legitimately enabled pack would look absent under a naive comparison. Folding all three to the
/// bare name is what makes the check match Vanilla's.
///
/// Lower-cased because pack directory names are case-insensitive on Windows and a `MyPack` folder
/// with `mypack` in `level.dat` is a real mismatch that would otherwise silently disable a pack.
#[must_use]
pub fn normalise(name: &str) -> String {
    let trimmed = name.trim().trim_start_matches("file/");
    let without_extension = trimmed
        .strip_suffix(".zip")
        .or_else(|| trimmed.strip_suffix(".ZIP"))
        .unwrap_or(trimmed);
    without_extension.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{EnabledPacks, normalise};

    #[test]
    fn no_list_enables_everything() {
        // The default that matters: a world whose `level.dat` predates `DataPacks`, or was written
        // by another tool, must keep working with all its packs.
        let packs = EnabledPacks::default();
        assert!(packs.is_unspecified());
        assert!(packs.allows("anything"));
        assert!(packs.allows(""));
        assert_eq!(packs.enabled_count(), 0);

        let from_empty = EnabledPacks::from_level_dat(&[], &[]);
        assert!(from_empty.is_unspecified(), "no names at all means no list");
        assert!(from_empty.allows("anything"));
    }

    #[test]
    fn an_explicit_list_gates_packs() {
        let packs = EnabledPacks::from_level_dat(
            &["vanilla".to_owned(), "my_pack".to_owned()],
            &["other".to_owned()],
        );
        assert!(!packs.is_unspecified());
        assert!(packs.allows("vanilla"));
        assert!(packs.allows("my_pack"));
        assert!(!packs.allows("other"), "explicitly disabled");
        assert!(
            !packs.allows("never_heard_of_it"),
            "a pack the world does not name must not load"
        );
        assert_eq!(packs.enabled_count(), 2);
    }

    #[test]
    fn a_name_in_both_lists_is_disabled() {
        // Disabling is the more specific intent, and Vanilla writes a pack there precisely to
        // override an inherited enabled state.
        let packs = EnabledPacks::from_level_dat(&["both".to_owned()], &["both".to_owned()]);
        assert!(!packs.allows("both"));
    }

    #[test]
    fn an_empty_enabled_list_with_a_disabled_entry_is_a_real_state() {
        // A player who disabled everything. This must NOT fall back to "everything enabled".
        let packs = EnabledPacks::from_level_dat(&[], &["something".to_owned()]);
        assert!(!packs.is_unspecified());
        assert!(!packs.allows("something"));
        assert!(
            !packs.allows("anything_else"),
            "an explicit list with nothing enabled enables nothing"
        );
    }

    #[test]
    fn the_three_spellings_of_one_pack_are_one_identity() {
        // Vanilla writes a world-folder pack as `file/<name>` and an archive as `<name>.zip`. A
        // naive comparison would make a legitimately enabled pack look absent.
        for spelling in ["my_pack", "file/my_pack", "my_pack.zip", "file/my_pack.zip"] {
            assert_eq!(normalise(spelling), "my_pack", "{spelling}");
        }
        let packs = EnabledPacks::from_level_dat(&["file/My_Pack.zip".to_owned()], &[]);
        for asked in ["my_pack", "My_Pack", "file/my_pack.zip", "my_pack.zip"] {
            assert!(packs.allows(asked), "{asked} must match");
        }
    }

    #[test]
    fn names_are_matched_case_insensitively_and_trimmed() {
        // Pack directories are case-insensitive on Windows, so `MyPack` on disk with `mypack` in
        // `level.dat` is a real mismatch that would otherwise silently disable a pack.
        let packs = EnabledPacks::from_level_dat(&["  MY_PACK  ".to_owned()], &[]);
        assert!(packs.allows("my_pack"));
        assert!(packs.allows("My_Pack"));
        assert!(packs.allows("my_pack.zip"));
        assert!(!packs.allows("my_pack2"), "a prefix is not a match");
    }

    #[test]
    fn a_zip_suffix_only_is_stripped() {
        // `name.zipx` is not an archive name, so stripping naively would conflate two packs.
        assert_eq!(normalise("a.zipx"), "a.zipx");
        assert_eq!(normalise("a.zip"), "a");
        assert_eq!(normalise("archive.tar.gz"), "archive.tar.gz");
        assert_eq!(normalise("file/x"), "x", "the prefix is stripped");
        // But only the prefix, not an occurrence in the middle.
        assert_eq!(normalise("my/file/x"), "my/file/x");
    }
}
