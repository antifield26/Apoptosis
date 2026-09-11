# Phase 02 Report — Network and Protocol

Date: 2026-09-10. Preflight: re-read AGENTS.md §2/§9/§10/§14 + PHASE-02 prompt;
`git status` (branch `master`, no commits — still true); toolchain 1.98.1.
Model handover occurred mid-phase: work resumed from the packet-table research
step; no earlier implementation existed to preserve.

## Tasks (P02-01..P02-16)

| Task | Status | Evidence |
|---|---|---|
| P02-01 TCP/Tokio connection lifecycle | DONE | `mc-network/src/{listener.rs,connection.rs}`: bind/accept loop, per-connection task, `NetworkShutdown` notification, admission guard held for connection lifetime |
| P02-02 VarInt/VarLong hostile-input limits | DONE | `mc-protocol/src/varint.rs`: 5/10-byte caps, truncated/overlong rejection, known encodings, 10k seeded corpus |
| P02-03 Frame reader/writer + size limits | DONE | `mc-protocol/src/framing.rs`: incremental `FrameCodec`, 2 MiB cap, buffer cap, split/multi feeds, malformed corpus |
| P02-04 Packet registry/version strategy | DONE | `mc-protocol/src/ids.rs` (775-only table with provenance), dispatch by `ConnectionState` + ids in `connection.rs`; unknown ids ignored with debug trace |
| P02-05 Handshake/status packets | DONE | `packets/handshake.rs`, `packets/status.rs` + golden fixture + E2E status/ping |
| P02-06 Login state machine | DONE | `run_login` → `run_configuration` → `run_play` (+ Play↔Config re-entry via `StartConfiguration`), wrong-protocol kick, dual-state kick routing |
| P02-07 Offline auth/GameProfile | DONE | `auth.rs` offline UUID (MD5/v3/variant) with validated Notch vector; username rules |
| P02-08 Online auth provider boundary | DONE (boundary) | `OnlineAuthProvider` trait + call site; `online_mode=true` fails startup loudly; encryption/session flow explicitly deferred |
| P02-09 Compression negotiation | DONE | threshold send before LoginSuccess, codec switch both directions, vanilla below-threshold rejection, bomb defence |
| P02-10 Play-state packet plumbing | DONE | JoinGame/cache center+radius/keepalive/ping/pong/client info/configuration ack; `PlayIntent` decodes movement/action/chat into typed intents (not simulated); keepalive send/ack/timeout covered by `crates/network/tests/keepalive.rs` |
| P02-11 Packet fixture/golden harness | DONE | `fixtures/protocol/*.hex` + `read_hex_fixture` + `mc-protocol/tests/fixtures.rs` (4 tests) |
| P02-12 Protocol fuzz/property tests | DONE | seeded corpora in varint/framing/nbt + socket-level fuzz in E2E (garbage burst, then clean login), all never-panic |
| P02-13 Minimal test client | DONE | `mc-test-support/src/client.rs`: status, login, configuration, play entry, keepalive replies, raw hostile writes |
| P02-14 Client login/play E2E test | DONE | `mc-server/tests/e2e_login_play.rs` (6 tests, live listener on ephemeral port) |
| P02-15 Connection rate/resource limits | DONE | `limits.rs`: global semaphore, per-IP concurrency cap, token-bucket burst (capacity 8, 1 token/250 ms), map-growth bound; deterministic clock-injected tests |
| P02-16 Protocol review + conformance report | DONE | `docs/protocol/26.1.2-wire-notes.md`, this report, TEST/PARITY matrix updates, provenance + third-party updates |

## Verification (exact commands, this host)

- `cargo test --workspace` → **112 passed, 0 failed** (core 10, network 15 + keepalive 1, protocol 64, protocol fixtures 4, server 8, server E2E 6).
- `cargo fmt --all -- --check` → clean.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` → clean.
- Binary smoke: `mc-server.exe config.example.toml` alive after 2 s and TCP connect to 127.0.0.1:25565 succeeds (no stderr output).

## Exit-gate check (EXIT-GATES.md P02)

- [x] Client can connect through handshake/login/play in offline mode — **protocol test client** completes status, offline login, configuration (known packs, feature flags, two registry payloads, tags, finish), and receives JoinGame; 2 registry packets decoded.
- [x] Malformed packet tests exist — varint/frame/NBT corpora + socket-level hostile burst; process survives and keeps serving.
- [x] Online-mode provider is structurally supported behind configuration — trait + wiring + fail-fast when enabled without a provider.
- ⚠️ **Qualification**: no real 26.1.2 client exists in this environment, so "client" above means our codec test client. Real-client acceptance is listed as a blocking limitation (P02-T12) and must be run before any interoperability claim (AGENTS.md §3.1).

## Known limitations / carry-over

1. **Real-client verification pending (blocker for claims).** Registry element NBT (`dimension_type`, `worldgen/biome`) is authored from the 26.1 datapack schema and marked provisional; a real client may require additional fields. Procedure: run a 26.1.2 client against `config.example.toml`, capture packets, verify/adjust, then flip parity rows from `partial` to verified.
2. **Online mode is a boundary only.** Encryption request/response and Mojang session verification are not implemented; enabling `online_mode` returns a startup error. Tracked for its own task; P08 validates the path.
3. **Keepalive behaviour** is now covered end-to-end with shortened intervals: a responsive client survives and a silent client receives `PlayDisconnect` after the timeout (`crates/network/tests/keepalive.rs`). Production timing constants remain untested at full duration (fast E2E by design); P08-06 will exercise production-scale timing.
4. **Registry optimizations** (known-pack matching to skip data) are deliberately ignored: we always send the full payload. Fine for correctness, worth revisiting in P04/P07.
5. **Unmodelled play packets** are ignored with a debug trace (brand payloads, resource-pack replies); no strict mode yet.
6. Unknown-id packets in login/config are ignored rather than kicked to keep real clients from breaking; revisit with capture evidence.

## Files (production + evidence)

```text
crates/protocol/{Cargo.toml,src/{lib.rs,varint.rs,wire.rs,ids.rs,packet.rs,nbt.rs,framing.rs,text.rs,packets/{mod,handshake,status,login,config,play}.rs},tests/fixtures.rs}
crates/network/{Cargo.toml,src/{lib.rs,auth.rs,connection.rs,limits.rs,listener.rs,registry_data.rs}}
crates/test-support/{Cargo.toml,src/{lib.rs,client.rs,fixtures.rs},fixtures/protocol/{handshake_login,frame_uncompressed,nbt_literal_text}.hex}
crates/server/{Cargo.toml,src/{config.rs,lifecycle.rs},tests/e2e_login_play.rs}
apps/server/src/main.rs, config.example.toml
docs/protocol/26.1.2-wire-notes.md, docs/phases/PHASE-02-REPORT.md
docs/{testing/TEST-MATRIX.md,vanilla-parity/PARITY-MATRIX.md,legal/third-party.md,research/provenance.md}
```

## Gate status

**Phase 02: PASS (qualified)** — all executable exit items pass with the real-client
qualification recorded; no BLOCKER findings inside the code, one external
verification gap (real 26.1.2 client) carried forward to P03/P04 entry. Unblocks
Phase 03 persistence work.
