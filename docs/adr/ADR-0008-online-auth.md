# ADR-0008 — Online-mode authentication (closes KD-01)

Date: 2026-09-30. Status: **Accepted** (P19-05).

Context: `online_mode` refused to start since Phase 02 (`start_network`
returned `Operational`: "the Mojang session/encryption flow is not
implemented yet"). The boundary trait (`OnlineAuthProvider`) and call site
existed; the handshake, cipher, session check and profile properties did
not. P19-05 closes KD-01 per its split acceptance: (a) encryption
handshake + `hasJoined` timeout/refusal vectors pinned against the jar's
cipher — automated, required; (b) a real account joins with its skin —
owner-run, may be NOT RUN without blocking P19-07.

Clean-room (binding, `third-party.md` §2): no source from any reference
clone enters this tree. Behaviour facts only — Minestom `MojangCrypt`
(digest order, `BigInteger.toString(16)` shape) and `LoginListener`
(handshake order), Pumpkin `authentication.rs` (`hasJoined` URL shape);
all code independently written. No GPL copy; new dependencies are all
`MIT OR Apache-2.0` (see §4).

## 1. Decision

- **Handshake**: `EncryptionRequest` (empty server id, RSA-1024 DER public
  key, 4-byte CSPRNG token) → `EncryptionResponse` (secret + token,
  PKCS#1 v1.5) → decrypt → fixed-time token compare → session hash →
  `hasJoined` → enable AES-128/CFB8 both ways → `LoginSuccess` with the
  verified profile **including properties**. Refusals kick with Vanilla's
  *translation keys* — `multiplayer.disconnect.unverified_username` and
  `multiplayer.disconnect.authservers_down` (`TextComponent::translatable`,
  AUDIT-19 C19-L7), with the English sentence as the component's fallback, so
  the client renders the message in the player's own language while the log
  line still reads as before. Only a **404** from `hasJoined` is a refused
  session; every other status (5xx/429) is a service failure and gets the
  `authservers_down` key plus a WARN in the log (AUDIT-19 C19-L8). Clean EOF
  mid-handshake ends quietly.
- **RSA**: 1024-bit, X.509 DER, generated once per boot (not per
  connection). PKCS#1 v1.5 decrypt failures are login failures, never boot
  failures.
- **Session hash**: SHA-1 over server-id ASCII + secret + key DER, rendered
  as signed hex with no leading zeros (Java `BigInteger.toString(16)`
  semantics, reimplemented). Pinned against independent recipe vectors
  (Python hashlib + pasted formatting).
- **Session check**: blocking HTTPS (`ureq`) with a 10 s global timeout,
  run off the async runtime (`spawn_blocking`). Every failure maps to a
  typed refusal (`Unknown` / `Timeout` / `Transport` / `Malformed`) —
  never a hang, never a default-accept. `Unknown` is **404 only**: a 5xx/429
  is `Transport`, so the player is told the authentication servers are down
  rather than that their session failed (AUDIT-19 C19-L8). Base URL is a
  field so tests pin offline against a loopback stub.
- **Cipher**: AES-128/CFB8, key and IV both the secret (Vanilla). The mode
  is hand-rolled over audited `aes` block encryption (16-byte shift
  register + XOR) because the `cfb8` crate's streaming API is one-shot by
  construction; `cfb8` stays as a dev-dependency known-answer oracle only.
  State lives outside `FrameCodec` (pure bytes in/out); the connection
  decrypts before `feed` and encrypts before the write.
- **Identity plumbing**: `NetworkSettings.online_identity` (shared `Arc`,
  `Debug`-redacted), `GameProfile.properties` (empty offline; verified
  online), `LoginSuccess.properties` actually sent.

## 2. Cost (measured, dev host; Pi estimate is proportional)

Scratch probe (deleted after recording), RSA-1024 keygen **320 ms once per
boot** (logged at startup), RSA decrypt **5 ms per login**, AES-CFB8 over a
2 KiB play burst **819 µs**. Keygen dominates and happens once; per-login
and per-packet costs are sub-tick. No Pi re-measure required for P19-05
(the Pi gate owns P20+ tick budgets); the numbers above are the estimate
P20-00 checks against.

## 3. Pins (all automated; (b) is NOT RUN by design)

- `cipher::wiring_matches_cfb8` (KAT via `cfb8` one-shot) +
  `oracle_vector_replays` (published `33b356ce…` bytes) +
  `chunked_stream_survives_segmentation` (byte-at-a-time continuity);
  feedback-swap probe goes red.
- `server_hash_matches_independent_vectors` (3 recipe vectors); order-swap
  probe goes red.
- `online_login_completes_through_the_cipher` (stock-client RSA + cipher
  + verified profile with skin property over loopback) +
  `online_login_refusals_kick` (bad token + refused session kick with
  Vanilla's translation key); token-skip probe goes red.
- `has_joined_parses_profile_and_properties` + `has_joined_refusal_vectors`
  (200/refusal/malformed/unreachable via loopback stub, clock-free) +
  `a_session_server_outage_is_not_an_unknown_user` (5xx/429 are transport,
  404 is the refusal) +
  `provider_maps_an_outage_to_operational_and_a_refusal_to_invalid_action`
  (the typed refusal the login flow keys its kick message on);
  flipping the 404 arm to catch-all goes red (AUDIT-19 C19-L8).
- `an_auth_outage_kicks_differently_from_a_refusal` (the two keys a client
  decodes over the wire) + `a_client_that_hangs_up_on_a_refusal_is_not_a_write_failure`
  (a hang-up after a kick is a client leave, not a write fault; AUDIT-19
  C19-L7/G-12).
- (b) A real Mojang account joining with its skin is owner-run and named
  NOT RUN in the row; it does not block P19-07 when (a) is green.

## 4. Dependencies (all `MIT OR Apache-2.0`, `Cargo.lock` pinned)

| Crate (locked) | License | Purpose | First use |
|---|---|---|---|
| rsa 0.9.10 | MIT OR Apache-2.0 | RSA-1024 keygen + PKCS#1 v1.5 decrypt | P19-05 (`mc-network::online`) |
| aes 0.8.4 | MIT OR Apache-2.0 | AES-128 block primitive for CFB8 | P19-05 (`mc-protocol::cipher`) |
| sha1 0.10.7 | MIT OR Apache-2.0 | session-hash digest | P19-05 (`mc-network::online`) |
| rand 0.8.8 | MIT OR Apache-2.0 | CSPRNG keygen/token | P19-05 (`mc-network::online`) |
| ureq 3.4.2 | MIT OR Apache-2.0 | blocking HTTPS `hasJoined` | P19-05 (`mc-network::online`) |
| cfb8 0.8.1 (dev-only) | MIT OR Apache-2.0 | known-answer oracle for the wiring test | P19-05 (`mc-protocol` dev-deps) |

Transitives (`pkcs8`, `cipher`, `rand_core`, `zeroize`, `subtle`, `rustls`,
etc.) are permissive MIT/Apache-2.0; no copyleft entered the tree
(`cargo deny` gate unchanged). `cfb8 0.8.1` (not 0.9: pairs with `cipher`
0.4 like `aes` 0.8; the wire mode is identical).

## 5. Consequences

- `online_mode = true` now boots (keygen + Mojang provider wired in
  `start_network`); default stays offline, and offline logins are
  byte-identical to before.
- KD-01 moves from "not implemented (fails fast)" to the split above.
- `UnconfiguredOnlineAuth` is deleted; the placeholder path no longer
  exists.
- RCON/console/command surfaces unchanged (RCON is console-level by
  definition and needs no auth rework).
