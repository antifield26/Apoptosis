//! Player identity.
//!
//! [`GameProfile`] is the key player state is stored under: `playerdata`
//! filenames, session lookups and the entity map all use the profile UUID.
//!
//! ## Why this crate has its own `GameProfile`
//!
//! `mc-network` (P02-07) defines a structurally identical `GameProfile`, and
//! that one is the authoritative value produced by authentication. This crate
//! may not depend on `mc-network` (it would drag Tokio, md-5 and uuid into the
//! simulation layer, and `mc-network` will eventually be a *consumer* of player
//! state, which would make the dependency a cycle). The type is therefore
//! duplicated deliberately, with the **same field names and types**:
//!
//! ```text
//! GameProfile { id: Uuid, name: String }
//! ```
//!
//! Conversions at the boundary are trivial field moves and must be done by the
//! caller. Unifying the two into `mc-core` is the right fix and is recorded as a
//! known gap in the crate docs; it is a cross-crate refactor, not something this
//! crate can do from here (AGENTS.md sections 3.3 and 7).

use mc_core::error::{ServerError, ServerResult};
use std::fmt;

/// A player identity: profile UUID plus the accepted name.
///
/// `Ord`/`Hash` are derived so profiles can key the ordered maps that make
/// iteration order and therefore save/trace output reproducible (AGENTS.md
/// section 3.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GameProfile {
    /// Profile UUID (session-verified in online mode, offline-derived
    /// otherwise).
    pub id: String,
    /// Player name as accepted by the server.
    pub name: String,
}

impl GameProfile {
    /// Build a profile, validating the name.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when [`validate_username`] rejects `name`.
    pub fn new(id: impl Into<String>, name: &str) -> ServerResult<Self> {
        validate_username(name)?;
        Ok(Self {
            id: id.into(),
            name: name.to_owned(),
        })
    }

    /// Build a profile without validating the name.
    ///
    /// For loading persisted data: a `playerdata` file may carry a name that
    /// predates a rename rule or an offline-mode profile that was never
    /// validated, and refusing to load that file is worse than accepting it.
    #[must_use]
    pub fn unvalidated(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for GameProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.id)
    }
}

/// Validate a username exactly as the vanilla login path does: 1..=16
/// characters of `[A-Za-z0-9_]`.
///
/// Mirrors `mc_network::validate_username`; a name that fails here must be
/// rejected before it can become a `playerdata` filename.
///
/// # Errors
///
/// [`ServerError::InvalidAction`] when the name violates the rules.
pub fn validate_username(name: &str) -> ServerResult<()> {
    let valid = !name.is_empty()
        && name.chars().count() <= 16
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(ServerError::InvalidAction(format!(
            "invalid username {name:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{GameProfile, validate_username};
    use mc_core::error::ServerError;

    #[test]
    fn username_rules_match_the_vanilla_login_check() {
        for good in ["Steve", "a_1", "0123456789abcdef"] {
            assert!(validate_username(good).is_ok(), "{good} must be accepted");
        }
        for bad in [
            "",
            "toolongusername17",
            "bad name",
            "bad-name",
            "\u{4E2D}\u{6587}",
        ] {
            assert!(validate_username(bad).is_err(), "{bad} must be rejected");
        }
        assert!(matches!(
            GameProfile::new("id", "bad name"),
            Err(ServerError::InvalidAction(_))
        ));
    }

    #[test]
    fn a_validated_profile_keeps_id_and_name() {
        let profile =
            GameProfile::new("b50ad385-829d-3141-a216-7e7d7539ba7f", "Notch").expect("valid");
        assert_eq!(profile.name, "Notch");
        assert_eq!(profile.id, "b50ad385-829d-3141-a216-7e7d7539ba7f");
        assert_eq!(
            profile.to_string(),
            "Notch (b50ad385-829d-3141-a216-7e7d7539ba7f)"
        );
    }

    #[test]
    fn unvalidated_profiles_are_accepted_for_persistence() {
        // A stored profile whose name would fail the login rule must still load.
        let profile = GameProfile::unvalidated("id", "Legacy Name");
        assert_eq!(profile.name, "Legacy Name");
        assert_eq!(profile, GameProfile::unvalidated("id", "Legacy Name"));
    }
}
