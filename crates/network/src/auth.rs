//! Authentication boundary (P02-07/P02-08).
//!
//! Offline mode is the product default and is fully implemented: the profile
//! UUID is derived exactly like vanilla's `UUID.nameUUIDFromBytes` over
//! `OfflinePlayer:<name>` (MD5, version 3, IETF variant).
//!
//! Online mode is a **structural boundary** in Phase 02: the trait and call
//! site exist, and enabling it without a configured provider fails startup
//! loudly instead of silently degrading to offline auth. The actual Mojang
//! session handshake (encryption request/response, RSA/AES, HTTP session
//! verification) is deferred to its own task and tracked in the phase report.

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

/// Placeholder provider installed when `online_mode = true` but no real
/// provider has been wired yet. Startup refuses to run with this provider so
/// operators get a clear error instead of broken logins.
#[derive(Debug, Default)]
pub struct UnconfiguredOnlineAuth;

impl UnconfiguredOnlineAuth {
    /// Human-readable reason surfaced at startup and on kick.
    #[must_use]
    pub fn reason() -> &'static str {
        "online mode is enabled but no authentication provider is configured"
    }
}

impl OnlineAuthProvider for UnconfiguredOnlineAuth {
    fn authenticate<'a>(&'a self, _name: &'a str, _server_hash: &'a str) -> AuthFuture<'a> {
        Box::pin(async { Err(ServerError::Operational(Self::reason().to_owned())) })
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
    use super::{GameProfile, UnconfiguredOnlineAuth, offline_profile, validate_username};
    use mc_core::error::ServerError;

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

    #[tokio::test]
    async fn unconfigured_online_auth_fails_clearly() {
        let provider = UnconfiguredOnlineAuth;
        let result = super::OnlineAuthProvider::authenticate(&provider, "Steve", "hash").await;
        assert!(matches!(result, Err(ServerError::Operational(_))));
        let _ = GameProfile {
            id: uuid::Uuid::nil(),
            name: "x".to_owned(),
        };
    }
}
