//! Authentication boundary (P02-07/P02-08).
//!
//! Offline mode is the product default and is fully implemented: the profile
//! UUID is derived exactly like vanilla's `UUID.nameUUIDFromBytes` over
//! `OfflinePlayer:<name>` (MD5, version 3, IETF variant).
//!
//! Online mode is live (P19-05, ADR-0008). The boundary trait and call site
//! are still what `start_network` consults, but the flow behind them is
//! implemented: `EncryptionRequest`/`EncryptionResponse` over a per-boot
//! RSA-1024 key, fixed-time token compare, the SHA-1 session hash, a blocking
//! `hasJoined` (timeout and refusals typed, never a default-accept) and
//! AES-128/CFB8 both ways — see `mc_network::online`. Split (b) of KD-01, a
//! real Mojang account joining with its skin, is dropped by the 2026-09-30
//! no-online-mode deployment decision; the path stays code-complete and
//! automated-pinned. `online_mode = true` selects the Mojang provider at
//! startup (keypair generated once per boot, `crates/server/src/lifecycle.rs`);
//! the default stays offline through [`OfflineOnlyAuth`].

use mc_core::error::{ServerError, ServerResult};
use md5::{Digest, Md5};
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// A resolved player profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    /// Profile UUID (offline-derived or session-verified).
    pub id: Uuid,
    /// Player name as accepted by the server.
    pub name: String,
    /// Verified skin/cape properties (P19-05); empty unless a session
    /// server supplied them — offline profiles never carry any.
    pub properties: Vec<mc_protocol::packets::login::ProfileProperty>,
}

/// Derive the vanilla offline-mode profile for `name`.
#[must_use]
pub fn offline_profile(name: &str) -> GameProfile {
    let digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes: [u8; 16] = digest.into();
    // MD5-based version 3 UUID: set version nibble and IETF variant bits,
    // matching Java's UUID.nameUUIDFromBytes.
    bytes[6] = (bytes[6] & 0x0F) | 0x30;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    GameProfile {
        id: Uuid::from_bytes(bytes),
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

/// Boxed authentication future (keeps the trait dyn-compatible without an
/// async-trait dependency).
pub type AuthFuture<'a> = Pin<Box<dyn Future<Output = ServerResult<GameProfile>> + Send + 'a>>;

/// Online-mode authentication provider boundary.
///
/// `server_hash` is the Minecraft-style hex digest the client would have
/// signed; an implementation performs the session-server check with it.
pub trait OnlineAuthProvider: Send + Sync {
    /// Authenticate `name` for `server_hash`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when no provider is configured,
    /// [`ServerError::InvalidAction`] when the session server rejects the
    /// player.
    fn authenticate<'a>(&'a self, name: &'a str, server_hash: &'a str) -> AuthFuture<'a>;

    /// Authenticate `name` for `server_hash`, carrying the address the login
    /// arrived from.
    ///
    /// Vanilla's session check binds the request to that address (`&ip=`), so a
    /// provider that can use it should. Defaulted to the address-less call so
    /// providers without one keep working: AUDIT-19 C19-M3 found the address was
    /// built into the URL but never threaded here, which left every online-mode
    /// session unbound to its join address.
    fn authenticate_from<'a>(
        &'a self,
        name: &'a str,
        server_hash: &'a str,
        peer: Option<std::net::IpAddr>,
    ) -> AuthFuture<'a> {
        let _ = peer;
        self.authenticate(name, server_hash)
    }
}

/// Provider used when `online_mode = false` (default). Should never be called;
/// if it is, the caller misrouted the login flow.
#[derive(Debug, Default)]
pub struct OfflineOnlyAuth;

impl OnlineAuthProvider for OfflineOnlyAuth {
    fn authenticate<'a>(&'a self, _name: &'a str, _server_hash: &'a str) -> AuthFuture<'a> {
        Box::pin(async {
            Err(ServerError::Invariant(
                "online authentication requested in offline mode".to_owned(),
            ))
        })
    }
}

/// Validate a requested username: 1..=16 chars of `[A-Za-z0-9_]`.
///
/// # Errors
///
/// [`ServerError::InvalidAction`] when the name violates vanilla rules.
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
    use super::{offline_profile, validate_username};

    /// AUDIT-19 C19-L5: the module header claimed online mode was a
    /// "structural boundary" whose Mojang handshake was deferred to its own
    /// task, long after P19-05 implemented it. The doc is the deliverable, so
    /// the pin is a regression guard on the *false* claim, written with the
    /// words split so that this guard cannot satisfy itself. The behaviour the
    /// corrected header describes is pinned separately by
    /// `connection::tests::online_login_completes_through_the_cipher` and
    /// `online::tests::provider_maps_an_outage_to_operational_and_a_refusal_to_invalid_action`.
    #[test]
    fn the_module_docs_do_not_call_the_handshake_deferred() {
        let docs = include_str!("auth.rs");
        // Split so the needle is not a substring of this test's own source.
        for stale in ["deferred to its own", "structural boundary in"] {
            let needle = format!("{stale} task");
            assert!(
                !docs.contains(&needle),
                "the online handshake is implemented (P19-05); see `crate::online`"
            );
        }
        for expected in [
            "Online mode is live",
            "mc_network::online",
            "`hasJoined` (timeout and refusals typed",
        ] {
            assert!(
                docs.contains(expected),
                "the header must keep naming what exists: {expected:?}"
            );
        }
    }

    #[test]
    fn offline_profile_matches_vanilla_vector() {
        // Vector computed with Java's UUID.nameUUIDFromBytes over
        // "OfflinePlayer:Notch" using the documented MD5/v3/variant rules.
        let profile = offline_profile("Notch");
        assert_eq!(
            profile.id.to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(profile.name, "Notch");
    }

    #[test]
    fn offline_profile_is_deterministic() {
        assert_eq!(offline_profile("Steve"), offline_profile("Steve"));
        assert_ne!(offline_profile("Steve"), offline_profile("Alex"));
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("Steve").is_ok());
        assert!(validate_username("a_1").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("toolongusername17").is_err());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("bad-name").is_err());
        assert!(validate_username("\u{4E2D}\u{6587}").is_err());
    }
}
