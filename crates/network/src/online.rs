//! Online-mode login cryptography and session-server checks (P19-05, ADR-0008).
//!
//! The vanilla handshake, in order: the server sends `EncryptionRequest`
//! (empty server id, RSA public key, random verify token, and the trailing
//! `shouldAuthenticate=true` the jar's `ClientboundHelloPacket` writes last);
//! the client
//! answers `EncryptionResponse` (shared secret + token, RSA-encrypted);
//! the server decrypts, checks the token, derives the session hash, asks
//! Mojang's session server (`hasJoined`), and — on success — enables
//! AES-128/CFB8 both ways with the secret and finishes login with the
//! verified profile *including its properties* (skins).
//!
//! Protocol facts (not code) behind the shapes: the digest is SHA-1 over
//! the server-id ASCII bytes, the secret and the DER public key, rendered
//! as a signed hex integer with no leading zeros (vanilla
//! `MojangCrypt.digestData` + `new BigInteger(digest).toString(16)`); the
//! key is RSA-1024 with X.509 DER encoding; the cipher is AES/CFB8 with
//! the secret as key and IV alike (see `mc_protocol::cipher`).
//!
//! Timeouts and refusals are first-class: `hasJoined` runs with a bounded
//! timeout, and every failure mode (timeout, transport error, unknown
//! user, malformed body) maps to a typed refusal the login flow kicks
//! with — never a hang, never a default-accept.

use mc_core::error::{ServerError, ServerResult};
use mc_protocol::packets::login::ProfileProperty;
use std::net::IpAddr;
use uuid::Uuid;

/// RSA key size in bits (vanilla: 1024).
pub const RSA_KEY_BITS: usize = 1024;

/// Verify-token length in bytes (vanilla: 4).
pub const VERIFY_TOKEN_LEN: usize = 4;

/// Shared-secret length in bytes (AES-128).
pub const SHARED_SECRET_LEN: usize = 16;

/// `hasJoined` request timeout.
pub const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The server's login identity: RSA keypair plus its DER public key.
///
/// `RsaPrivateKey` carries secret material: `Debug` is hand-written and
/// redacted, so logs and dumps never contain key bytes.
pub struct OnlineIdentity {
    private_key: rsa::RsaPrivateKey,
    /// X.509 DER (`SubjectPublicKeyInfo`) as sent in `EncryptionRequest`.
    pub public_der: Vec<u8>,
}

impl std::fmt::Debug for OnlineIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnlineIdentity(..)")
    }
}

impl OnlineIdentity {
    /// Generate a fresh RSA-1024 keypair.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the CSPRNG or the key build fails,
    /// or the public key does not DER-encode (a programmer error, surfaced
    /// loudly rather than booting keyless).
    pub fn generate() -> ServerResult<Self> {
        let mut rng = rand::thread_rng();
        let private_key = rsa::RsaPrivateKey::new(&mut rng, RSA_KEY_BITS)
            .map_err(|e| ServerError::Operational(format!("RSA keypair generation failed: {e}")))?;
        let public_der =
            rsa::pkcs8::EncodePublicKey::to_public_key_der(&rsa::RsaPublicKey::from(&private_key))
                .map_err(|e| ServerError::Operational(format!("public key DER failed: {e}")))?
                .to_vec();
        Ok(Self {
            private_key,
            public_der,
        })
    }

    /// Random verify token from the CSPRNG.
    #[must_use]
    pub fn verify_token() -> [u8; VERIFY_TOKEN_LEN] {
        use rand::RngCore as _;
        let mut token = [0u8; VERIFY_TOKEN_LEN];
        rand::thread_rng().fill_bytes(&mut token);
        token
    }

    /// The public half, for encrypting (tests) and requesting (login flow).
    #[must_use]
    pub fn public_key(&self) -> rsa::RsaPublicKey {
        rsa::RsaPublicKey::from(&self.private_key)
    }

    /// RSA-decrypt a PKCS#1 v1.5 block from the client.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when the block is not ours (wrong key,
    /// wrong padding, hostile bytes): a login failure, not a boot failure.
    pub fn decrypt(&self, ciphertext: &[u8]) -> ServerResult<Vec<u8>> {
        use rsa::Pkcs1v15Encrypt;
        self.private_key
            .decrypt(Pkcs1v15Encrypt, ciphertext)
            .map_err(|_| ServerError::Protocol("login RSA block does not decrypt".to_owned()))
    }
}

/// The Minecraft session hash: SHA-1 of server-id ASCII + secret + key DER,
/// rendered as a signed hex integer with no leading zeros (vanilla
/// `new BigInteger(digest).toString(16)` semantics).
#[must_use]
pub fn server_id_hash(server_id: &str, secret: &[u8], public_der: &[u8]) -> String {
    use sha1::Digest as _;
    let mut hasher = sha1::Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(secret);
    hasher.update(public_der);
    mc_hex(&hasher.finalize())
}

/// Render a SHA-1 digest the way Java's `BigInteger.toString(16)` does:
/// two's complement sign, magnitude hex, leading zeros stripped (`"0"`
/// for zero).
fn mc_hex(digest: &[u8]) -> String {
    let negative = digest.first().is_some_and(|b| b & 0x80 != 0);
    // Magnitude: strip leading zeros, or two's-complement them first.
    let mut magnitude: Vec<u8> = if negative {
        let mut inverted: Vec<u8> = digest.iter().map(|b| !b).collect();
        let mut carry = 1u16;
        for byte in inverted.iter_mut().rev() {
            let sum = u16::from(*byte) + carry;
            *byte = sum as u8;
            carry = sum >> 8;
        }
        inverted
    } else {
        digest.to_vec()
    };
    while magnitude.len() > 1 && magnitude[0] == 0 {
        magnitude.remove(0);
    }
    let mut hex = String::with_capacity(magnitude.len() * 2 + 1);
    if negative {
        hex.push('-');
    }
    for byte in &magnitude {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    // Strip the hex-level leading zero the byte strip cannot see
    // (`0x0abc…` renders `abc…`, like BigInteger).
    let stripped = if negative {
        let (sign, digits) = hex.split_at(1);
        format!("{sign}{}", digits.trim_start_matches('0'))
    } else {
        hex.trim_start_matches('0').to_owned()
    };
    if stripped.is_empty() || stripped == "-" {
        "0".to_owned()
    } else {
        stripped
    }
}

/// A verified session profile: Mojang's uuid, name and properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProfile {
    /// Mojang uuid (undashed hex in the response).
    pub id: Uuid,
    /// Verified username.
    pub name: String,
    /// Skin/cape properties with Mojang signatures.
    pub properties: Vec<ProfileProperty>,
}

/// Why `hasJoined` refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRefusal {
    /// The session server has no such login (unknown user or wrong hash).
    Unknown,
    /// The check did not answer in time.
    Timeout,
    /// Transport or status failure carrying the detail.
    Transport(String),
    /// A 200 body that is not a session profile.
    Malformed(String),
}

/// Mojang session-server client. The base URL is a field (not a constant)
/// so tests point it at a local stub: timeout/refusal vectors pin offline.
#[derive(Debug, Clone)]
pub struct MojangClient {
    base_url: String,
}
impl MojangClient {
    /// The production session server.
    #[must_use]
    pub fn mojang() -> Self {
        Self {
            base_url: "https://sessionserver.mojang.com".to_owned(),
        }
    }

    /// A client against another base (tests).
    #[must_use]
    pub fn at_base(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// Ask whether `username` joined with `server_hash` from `ip`.
    ///
    /// Vanilla (and authlib) always send the joining address as a trailing
    /// `ip=` parameter, which is what binds the session to the address the
    /// player is joining from — AUDIT-19 C19-M3: without it a stolen session
    /// answer is replayable from anywhere. `None` omits the parameter (the
    /// pre-fix shape) and exists only for callers that genuinely have no
    /// address to bind.
    ///
    /// Blocking HTTPS: the caller runs it off the async runtime
    /// (`spawn_blocking`). Every failure maps to [`SessionRefusal`] — a
    /// refused login, never a hang (bounded by [`SESSION_TIMEOUT`]) and
    /// never a default-accept.
    ///
    /// # Errors
    ///
    /// All `ureq` failures map to [`SessionRefusal`]: timeouts, status
    /// refusals (404 unknown user included), transport errors and
    /// malformed bodies. Nothing here hangs or default-accepts.
    pub fn has_joined(
        &self,
        username: &str,
        server_hash: &str,
        ip: Option<IpAddr>,
    ) -> Result<SessionProfile, SessionRefusal> {
        // Usernames are `[A-Za-z0-9_]` (validated at login) and hashes are
        // hex-or-minus, so neither needs percent-encoding in the query. The
        // address is a bare host address, exactly what vanilla appends
        // (`InetAddress.getHostAddress()`; IPv6 stays unbracketed).
        let ip = ip.map_or_else(String::new, |address| format!("&ip={address}"));
        let url = format!(
            "{}/session/minecraft/hasJoined?username={username}&serverId={server_hash}{ip}",
            self.base_url
        );
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(SESSION_TIMEOUT))
                .build(),
        );
        let mut response = agent.get(&url).call().map_err(|e| match e {
            ureq::Error::Timeout(_) => SessionRefusal::Timeout,
            // Any status refusal (404 unknown user included) ends the
            // login; the distinction between "unknown" and "broken" is
            // a log line, not a second code path.
            ureq::Error::StatusCode(_) => SessionRefusal::Unknown,
            other => SessionRefusal::Transport(format!("{other}")),
        })?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| SessionRefusal::Malformed(format!("unreadable body: {e}")))?;
        parse_session_profile(&body).ok_or(SessionRefusal::Malformed(body))
    }
}

/// Session-server authentication for online mode (P19-05).
///
/// Implements [`crate::auth::OnlineAuthProvider`] over [`MojangClient`]:
/// the blocking HTTPS call runs off the async runtime, and every refusal
/// maps to a login refusal — unknown users as `InvalidAction` (a client
/// problem, answered with a kick), transport/timeout/malformed as
/// `Operational` (our problem, logged loudly).
pub struct MojangSessionAuth {
    client: MojangClient,
}

impl MojangSessionAuth {
    /// Against the production session server.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: MojangClient::mojang(),
        }
    }

    /// Against another base (tests point it at a local stub).
    #[must_use]
    pub fn at_base(base_url: &str) -> Self {
        Self {
            client: MojangClient::at_base(base_url),
        }
    }
}

impl Default for MojangSessionAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::auth::OnlineAuthProvider for MojangSessionAuth {
    fn authenticate<'a>(
        &'a self,
        name: &'a str,
        server_hash: &'a str,
    ) -> crate::auth::AuthFuture<'a> {
        // No address to bind: the caller did not have one (AUDIT-19 C19-M3).
        self.authenticate_from(name, server_hash, None)
    }

    fn authenticate_from<'a>(
        &'a self,
        name: &'a str,
        server_hash: &'a str,
        peer: Option<std::net::IpAddr>,
    ) -> crate::auth::AuthFuture<'a> {
        Box::pin(async move {
            let client = self.client.clone();
            let name = name.to_owned();
            let server_hash = server_hash.to_owned();
            // Blocking HTTPS must not stall the connection task: run it on
            // the blocking pool. A panicking or cancelled blocker is our
            // failure, never a default-accept.
            // The join address rides `authenticate_from` and becomes Vanilla's
            // `&ip=` on the request, so the session is bound to where it came
            // from (AUDIT-19 C19-M3; wired at `connection.rs`'s login path).
            let checked =
                tokio::task::spawn_blocking(move || client.has_joined(&name, &server_hash, peer))
                    .await
                    .map_err(|e| ServerError::Operational(format!("session check failed: {e}")))?;
            match checked {
                Ok(profile) => Ok(crate::auth::GameProfile {
                    id: profile.id,
                    name: profile.name,
                    properties: profile.properties,
                }),
                Err(SessionRefusal::Unknown) => Err(ServerError::InvalidAction(
                    "the session server has no such login".to_owned(),
                )),
                Err(SessionRefusal::Timeout) => Err(ServerError::Operational(
                    "the session server did not answer in time".to_owned(),
                )),
                Err(SessionRefusal::Transport(detail)) => Err(ServerError::Operational(format!(
                    "session-server transport failure: {detail}"
                ))),
                Err(SessionRefusal::Malformed(body)) => Err(ServerError::Operational(format!(
                    "session-server body is not a profile: {body}"
                ))),
            }
        })
    }
}

/// Parse a `hasJoined` 200 body into a session profile (`None` when the
/// shape is wrong — a refusal, not a default profile).
fn parse_session_profile(body: &str) -> Option<SessionProfile> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let id = value.get("id")?.as_str()?;
    let name = value.get("name")?.as_str()?;
    let id = parse_undashed_uuid(id)?;
    let mut properties = Vec::new();
    if let Some(list) = value
        .get("properties")
        .and_then(serde_json::Value::as_array)
    {
        for entry in list {
            properties.push(ProfileProperty {
                name: entry.get("name")?.as_str()?.to_owned(),
                value: entry.get("value")?.as_str()?.to_owned(),
                signature: entry
                    .get("signature")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    Some(SessionProfile {
        id,
        name: name.to_owned(),
        properties,
    })
}

/// Fixed-time byte equality (verify-token check).
///
/// No early-out on the first mismatch, so a timing probe learns nothing
/// per byte. Different lengths differ (unavoidably — the length is one
/// comparison), content never short-circuits.
#[must_use]
pub fn fixed_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse Mojang's undashed uuid hex (`069a79f4…`).
fn parse_undashed_uuid(hex: &str) -> Option<Uuid> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let dashed = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    Uuid::parse_str(&dashed).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        MojangClient, OnlineIdentity, SessionRefusal, mc_hex, parse_undashed_uuid, server_id_hash,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn keypair_generates_rsa_1024_with_der() {
        let identity = OnlineIdentity::generate().expect("keygen works");
        // X.509 SubjectPublicKeyInfo opens with a SEQUENCE tag; a 1024-bit
        // RSA key encodes to ~162 bytes.
        assert_eq!(identity.public_der.first(), Some(&0x30));
        assert!(
            (150..=180).contains(&identity.public_der.len()),
            "DER length {}, want an RSA-1024 envelope",
            identity.public_der.len()
        );
        // The token is fresh randomness per call (4 bytes, CSPRNG).
        assert_ne!(
            OnlineIdentity::verify_token(),
            OnlineIdentity::verify_token(),
            "two tokens must differ (flaky only at 1 in 4 billion)"
        );
    }

    #[test]
    fn rsa_round_trips_pkcs1_v15() {
        use rsa::Pkcs1v15Encrypt;
        let identity = OnlineIdentity::generate().expect("keygen works");
        let secret = *b"0123456789abcdef";
        let mut rng = rand::thread_rng();
        let ciphertext = identity
            .public_key()
            .encrypt(&mut rng, Pkcs1v15Encrypt, &secret)
            .expect("encrypts");
        assert_eq!(ciphertext.len(), 128, "RSA-1024 output is 128 bytes");
        assert_eq!(identity.decrypt(&ciphertext).expect("decrypts"), secret);
    }

    #[test]
    fn server_hash_matches_independent_vectors() {
        // Recipe vectors from an independent SHA-1 (Python hashlib) plus
        // Java `BigInteger.toString(16)` formatting, computed offline and
        // pasted: order is server-id bytes, secret, key DER. Swapping any
        // two inputs changes every digest below.
        let zeros = "00".repeat(162);
        for (server_id, secret_hex, key_hex, expected) in [
            (
                "",
                "30313233343536373839616263646566",
                "746573742d7075626c69632d6b65792d646572",
                "-491ecc87b0f1884efdd6d7a3937b6346804bd2de",
            ),
            (
                "localhost:25565",
                "000102030405060708090a0b0c0d0e0f",
                "6465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b",
                "-3faf5b520bc7d004623e585b77e5495e6d2ab2b6",
            ),
            (
                "",
                "00000000000000000000000000000000",
                zeros.as_str(),
                "6b08b43754ce487c2a13ddd95a52653f49dacc03",
            ),
        ] {
            assert_eq!(
                server_id_hash(server_id, &unhex(secret_hex), &unhex(key_hex)),
                expected,
                "recipe vector for {server_id:?}"
            );
        }
    }

    /// Decode even-length hex (test helper).
    fn unhex(hex: &str) -> Vec<u8> {
        assert!(hex.len().is_multiple_of(2));
        hex.as_bytes()
            .chunks(2)
            .map(|pair| {
                u8::try_from(
                    (pair[0] as char).to_digit(16).expect("hex") * 16
                        + (pair[1] as char).to_digit(16).expect("hex"),
                )
                .expect("two nibbles")
            })
            .collect()
    }

    #[test]
    fn server_hash_has_biginteger_shape() {
        // Deterministic and hex-shaped, no leading zeros unless zero.
        let a = server_id_hash("", b"secret", b"key");
        assert_eq!(a, server_id_hash("", b"secret", b"key"));
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "hex or a leading minus: {a}"
        );
        // Synthetic digests pin the sign/magnitude rules directly.
        assert_eq!(mc_hex(&[0u8; 20]), "0");
        assert_eq!(
            mc_hex(&[0x0f; 20]),
            "f".to_owned() + &"0f".repeat(19),
            "leading zero nibble strips like BigInteger"
        );
        assert!(
            mc_hex(&[0x80; 20]).starts_with('-'),
            "high bit set renders negative"
        );
        assert_eq!(mc_hex(&[0x80; 20]).len(), 1 + 40);
        // Different inputs diverge (avalanche spot-check, not a proof).
        assert_ne!(a, server_id_hash("", b"other", b"key"));
    }

    #[test]
    fn undashed_uuid_parses_and_rejects() {
        let id = parse_undashed_uuid("069a79f444e94726a5bef4a7b64ac909").expect("parses");
        assert_eq!(id.to_string(), "069a79f4-44e9-4726-a5be-f4a7b64ac909");
        assert!(parse_undashed_uuid("069a79f4-44e9-4726-a5be-f4a7b64ac909").is_none());
        assert!(parse_undashed_uuid("xyz").is_none());
        assert!(parse_undashed_uuid("").is_none());
    }

    /// Serve one canned HTTP response on loopback, returning the base URL
    /// and the request path the stub saw. Bodies are owned up front: the
    /// serving thread must own everything it touches.
    fn stub_once(status: &str, body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
        let status = status.to_owned();
        let body = body.to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("stub binds");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("one connection");
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).expect("read");
            let head = String::from_utf8_lossy(&request[..read]);
            let path = head
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_owned();
            let _ = tx.send(path);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (base, rx)
    }

    #[test]
    fn has_joined_parses_profile_and_properties() {
        let body = r#"{"id":"069a79f444e94726a5bef4a7b64ac909","name":"Notch","properties":[{"name":"textures","value":"abc","signature":"sig"}]}"#;
        let (base, path_rx) = stub_once("200 OK", body);
        let profile = MojangClient::at_base(&base)
            .has_joined("Notch", "hash", None)
            .expect("200 parses");
        assert_eq!(profile.name, "Notch");
        assert_eq!(
            profile.id.to_string(),
            "069a79f4-44e9-4726-a5be-f4a7b64ac909"
        );
        assert_eq!(profile.properties.len(), 1);
        assert_eq!(profile.properties[0].signature.as_deref(), Some("sig"));
        let path = path_rx.recv_timeout(Duration::from_secs(5)).expect("path");
        assert!(
            path.contains("username=Notch") && path.contains("serverId=hash"),
            "query carries both values: {path}"
        );
        assert!(
            !path.contains("ip="),
            "no address available means no ip= parameter: {path}"
        );
    }

    #[test]
    fn has_joined_binds_the_join_address() {
        // AUDIT-19 C19-M3: vanilla and authlib always append the joining
        // address, so a caller that knows it must put it on the wire.
        let body = r#"{"id":"069a79f444e94726a5bef4a7b64ac909","name":"Notch"}"#;
        let (base, path_rx) = stub_once("200 OK", body);
        MojangClient::at_base(&base)
            .has_joined(
                "Notch",
                "hash",
                Some("203.0.113.7".parse().expect("test address")),
            )
            .expect("200 parses");
        let path = path_rx.recv_timeout(Duration::from_secs(5)).expect("path");
        assert!(
            path.contains("username=Notch") && path.contains("serverId=hash"),
            "the address is an addition, not a replacement: {path}"
        );
        assert!(
            path.ends_with("&ip=203.0.113.7"),
            "the query ends with the joining address, like vanilla: {path}"
        );
    }

    #[test]
    fn has_joined_refusal_vectors() {
        // Unknown user (vanilla answers 404 with an error body).
        let (base, _) = stub_once("404 Not Found", r#"{"error":"Not found"}"#);
        assert_eq!(
            MojangClient::at_base(&base).has_joined("Nobody", "hash", None),
            Err(SessionRefusal::Unknown)
        );
        // Malformed 200 body is a refusal, not a default profile.
        let (base, _) = stub_once("200 OK", "not json");
        assert!(matches!(
            MojangClient::at_base(&base).has_joined("Notch", "hash", None),
            Err(SessionRefusal::Malformed(_))
        ));
        // Unreachable host is transport, never accept.
        assert!(matches!(
            MojangClient::at_base("http://127.0.0.1:1").has_joined("Notch", "hash", None),
            Err(SessionRefusal::Transport(_))
        ));
    }
}
