# Audit 01 — Phases 00-03 Re-verification (2026-09-11)

Triggered by the owner's instruction to "review that all of the first three phases
are actually done" before starting Phase 04. Three independent read-only audits
ran in parallel (Phases 00, 01, 02), plus direct verification by the primary
agent for the items an audit cannot settle by reading (packet ids against the
official jar, gate commands).

Everything below is either **verified**, **fixed in this round**, or **open with
an owner**. Nothing was re-labelled "done" without evidence.

## 1. Verified as real (no action)

| Area | Verification |
|---|---|
| P00-01..P00-10 | All ten deliverables exist with substance. ~25 cited `file:line` references spot-checked by the auditor: **every one resolved exactly**. Licences on disk match `third-party.md` (Pumpkin GPL-3.0, Paper GPLv3, Valence MIT, Minestom Apache-2.0). |
| Version facts | Independently reproduced: jar `version.json` → 26.1.2 / protocol 775 / `world_version` 4790; vanilla `level.dat` → `DataVersion` 4790, `version` 19133. |
| P01-01/02/04/06/07/09/10 | Workspace, toolchain pin (1.98.1 installed and pinned by `rust-toolchain.toml`), crate boundaries with real behaviour and no empty shells, config schema + `deny_unknown_fields` + validation, logging initialised by the binary, deterministic 20 TPS clock with overrun clamp, test-support consistently a **dev-only** dependency. |
| Error taxonomy | All five AGENTS.md §9 classes present. Non-test panic sites in the whole workspace were enumerated: 6, of which 4 were on the network admission path and are fixed below; `wire.rs` / `nbt` hostile paths are `ensure()`/`get()`-guarded with no indexing panic. |
| P02-01..P02-09, P02-11, P02-13, P02-15, P02-16 | Genuine implementations. Offline UUID derivation verified by recomputing the MD5 by hand: `b50ad385-829d-3141-a216-7e7d7539ba7f` matches. Decompression-bomb defence limits the *decompressed* size. Rate limiter tests inject the clock. |
| P03 (previous round) | 233 tests green; vanilla 26.1.2 accepted a world our writer produced. |

## 2. Defects found and fixed in this round

### 2.1 `CHAT_COMMAND` had the wrong packet id (real wire bug)

`crates/protocol/src/ids.rs` said `serverbound::play::CHAT_COMMAND = 8`. The
official 26.1.2 jar registers it at **7** (`chat_command_signed` is 8). The
Phase 02 table was transcribed from a reference snapshot whose ordering is wrong
for protocol 775, so a real client sending `/command` would have had it decoded
as a chat message.

Fix: every id was re-extracted from the jar by parsing the **bytecode** order of
`getstatic PacketTypes.<NAME>` inside `ProtocolInfoBuilder`'s registration
lambdas (`target/vanilla-26.1.2/packets_from_jar.py`), committed as
`docs/protocol/packet-ids-775.tsv` (256 packets across 5 states), and locked by
`crates/protocol/tests/packet_ids.rs`, which asserts every constant this server
sends or decodes — plus contiguity of each direction's id space.

### 2.2 Login phase kicked real clients

`crates/network/src/connection.rs` treated any login packet other than
`login_acknowledged` as a protocol error. Vanilla clients may interleave
`custom_query_answer` (2) or `cookie_response` (4) first, so a real client could
be disconnected. Fixed with the same ignore-loop the configuration phase already
used.

### 2.3 Keepalive liveness could be dodged

`run_play` re-armed its read deadline on every packet, so a client that kept
sending anything every <30 s never hit the deadline; and a read timeout returned
`Ok(())` without the disconnect packet the keepalive test claims. Now the read
deadline is not a liveness verdict: the keepalive deadline decides, and a
timeout kicks.

### 2.4 Chat/command payloads were truncated

`PlayIntent::decode` read a 256-char string and ignored the rest of the payload,
so signed chat's trailing fields (timestamp, salt, signature, last-seen) were
never validated, and commands were capped at 256 chars against Vanilla's ~32 500.
Now the full shape is consumed and range-checked (`CHAT_MAX_CHARS = 256` matches
the jar's `writeUtf(message, 256)`).

### 2.5 Four mutex `expect`s on the admission path

`crates/network/src/limits.rs` panicked process-wide if the connection-gate mutex
was ever poisoned. Replaced with poison recovery + an error log: the guarded
state is a counter map, so a recovered lock can still make a correct admission
decision.

### 2.6 Shutdown did not drain connections

`listener.rs` dropped per-connection `JoinHandle`s, so `NetworkService::shutdown`
returned while connections were still live — while `lifecycle.rs` justified
closing the world with "no new player actions can arrive". Connections are now
tracked in a `JoinSet` and drained (5 s cap, then abort).

### 2.7 Signal-handler `expect` in a spawned task

`lifecycle.rs` panicked inside a detached task if SIGTERM registration failed.
Now it logs and falls back to Ctrl-C, so shutdown stays possible.

### 2.8 Missing dependency/licence gate

ADR-0001 D-08 and `third-party.md` promised a `cargo deny`-equivalent gate in
P01-03; none existed. Added `deny.toml` (permissive-only allow-list, crates.io
only, wildcards denied, reference implementations banned by name), a
`dependency-policy` CI job, and `docs/operations/DEPENDENCY-POLICY.md`.

### 2.9 CI hygiene and manifest drift

`dtolnay/rust-toolchain@stable` with an explicit `toolchain:` input (moving ref)
→ `@master`, plus `components: rustfmt, clippy` on both jobs. `apps/server` and
`crates/server` dev-deps pinned tokio outside `[workspace.dependencies]`;
aligned.

### 2.10 Documentation drift

- `docs/protocol/26.1.2-wire-notes.md` still pointed at `mc-protocol::nbt` after
  the P03 move to `mc-nbt`, and its id table now cites the jar-derived evidence.
- Phase 00 report and `protocol-baseline.md` §1 said the DataVersion was open
  (resolved in P03); annotated rather than rewritten so the original trail stays
  intact.
- `third-party.md` audit trail referenced a `src/` directory that never existed
  in this layout.
- `reference-repos.md` now states plainly that the SHA/branch requirement is
  **unmet** rather than merely "unavailable".
- `crates/network/src/lib.rs` had a broken intra-doc link (`ShutdownHandle`) and
  a claim that intents are "dropped".

## 3. Open items (owner action or later phase)

| # | Item | Why it is not fixed here |
|---|---|---|
| 1 | **Repository has zero commits** — *resolved 2026-09-11 by the owner-authorised initial commit `b7b1c99`* | The brief explicitly says not to commit without instruction. CI therefore has never run; `Cargo.lock` is untracked and reproducibility is not demonstrable from VCS. **Highest-value owner action.** |
| 2 | **No reference-repo SHAs** | The clones contain no `.git`; only a re-clone with history can close R-01. |
| 3 | **Real 26.1.2 client verification** | No client exists in this environment (carried from P02). Registry element NBT stays `provisional`; the login/config chain is unverified against a real client. Drives P02-T12 and the P04 exit gate ("real client survival session"). |
| 4 | **`docs/operations/` was empty** | Now holds `DEPENDENCY-POLICY.md`; the runbook itself is P08-15. |
| 5 | **Advisory triage process** | `cargo deny check advisories` runs but does not fail the build; no owner/response window exists yet. Recorded in the policy doc. |
| 6 | **`bytes` advertised but unused** | ADR D-08 lists it as a P01 adoption; it is currently only transitive. Either adopt it deliberately or drop the claim (cosmetic). |
| 7 | Cosmetic | `wire.rs` char-vs-byte string cap is 4× loose (framing still bounds it); `crates/test-support/src/fixtures.rs` layout docs omit `.hex/.dat/.mca`; `handshake_login.hex` is handshake-only. |

## 4. Gate status after the round

| Gate | Result |
|---|---|
| `cargo test --workspace` | green (see `PHASE-03-REPORT.md` for the count, re-run after these fixes) |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | clean |
| Phase 00 exit gate | PASS (R-01 open, R-03 closed) |
| Phase 01 exit gate | PASS on substance; CI evidence untested until the repo is committed |
| Phase 02 exit gate | PASS with the qualification the report already carried (no real client); two interop defects fixed above |
| Phase 03 exit gate | PASS (unchanged) |

Conclusion: Phases 00-03 are **substantively complete**, with one real wire bug
and several robustness/interop defects found and fixed, and four honest open
items (commits, reference SHAs, real-client verification, advisory triage).
Phase 04 may proceed; its exit gate depends on open item 3, which is called out
in the Phase 04 report rather than assumed away.
