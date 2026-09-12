# Release Candidate Documentation (P09-10)

Status: **release candidate, not a release.** No artifact is published and no
version number is claimed: the project has no license decision yet (ADR-0001
R-09), so distribution is blocked by the owner call, not by engineering. This
document is what a builder or operator needs to produce and run the server from
source today.

## 1. What ships

A from-scratch, pure-Rust dedicated server for Minecraft: Java Edition
**26.1.2** (protocol **775**, wire display string "26.1"), offline mode,
Vanilla-Survival slice: login → config → play, movement/collision, break/place,
inventory transactions, containers model, death/respawn, commands (8 — the
dispatcher tree has nine nodes, the ninth being `/function`, which the parity
matrix counts under data packs), data
packs (tags/recipes/functions), seeded terrain with a single-chunk structure
subset, Anvil-compatible persistence verified against real vanilla worlds.
Scope boundaries and every known divergence: `docs/vanilla-parity/KNOWN-DIVERGENCES.md`
(the release-affecting highlights: no lighting propagation yet — clients render
dark; entities are not persisted or synced; redstone is modelled but not
tick-driven; 8 of ~90 commands).

## 2. Building from source

Requirements: Rust **1.98.1** — pinned, not advisory; `rust-toolchain.toml`
selects it automatically with rustup, and a different toolchain is a build
reproducibility failure, not a configuration.

```text
cargo build --workspace --release --locked     # the server binary
cargo test  --workspace --no-fail-fast         # the full matrix (1 189 passed / 0 failed / 21 ignored, 74 suites, 2026-09-12)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target aarch64-unknown-linux-gnu  # the Pi 5 target compiles (no cross-run on x86_64)
cargo deny check licenses bans sources
```

Reproducibility facts recorded at the P09-09 build: `Cargo.lock` is committed;
the release build succeeds from a clean target with `--locked` (no unpinned
resolution); the binary hash and size are recorded in
`docs/performance/BENCHMARK-BASELINE.md` §P09-09. Rust releases are not
bit-identical across hosts/toolchains, so "reproducible" here means: same
commit + same pinned toolchain + `--locked` → a build that passes the same
gates, with the hash recorded for this host's artifact.

## 3. Running

```text
target/release/mc-server [config.toml]     # no argument = built-in defaults
```

Copy `config.example.toml` and edit `bind`, `world_dir`, `max_players`,
`view_distance`. Defaults are offline mode, 10 players, view 8, compression on
at 256 bytes, autosave every 6000 ticks. A first start creates a new
`level.dat` (DataVersion 4790, version 19133 — measured on a vanilla 26.1.2
world); a second start reuses it. `online_mode = true` is a **fail-fast
boundary**: it refuses to start because no Mojang-auth provider exists (KD-01).

Operational material: `docs/operations/RUNBOOK.md` (install, config, observe,
backup/restore, failure playbook), `deploy/mc-server.service` (systemd unit,
reviewed — never applied to a real host), `docs/operations/DEPENDENCY-POLICY.md`.

## 4. What "release candidate" does and does not mean here

| Claim | Status |
|---|---|
| Full regression matrix passes | **Yes** — five gates green on 2026-09-12, each with a retained run: `cargo test --workspace --no-fail-fast` (1 189/0/21, 74 suites; `target/p09_full_test.log` + `target/p09_full_test2.log`), `cargo fmt --check` (`gate_fmt.log`), `cargo clippy --workspace --all-targets -- -D warnings` (`gate_clippy.log`), `cargo check --target aarch64-unknown-linux-gnu` (`gate_aarch64.log`), `cargo deny check licenses bans sources` (`gate_deny.log`) — all exit 0; re-runnable per §2 |
| Conformance report produced | Yes — `docs/phases/PHASE-09-REPORT.md` + sweep rows in `docs/testing/TEST-MATRIX.md` |
| Known divergences documented | Yes — `docs/vanilla-parity/KNOWN-DIVERGENCES.md` (38 entries) |
| Release artifacts published | **No** — blocked on the owner's license decision (R-09); nothing may be distributed until it exists |
| 20 TPS on Pi 5 with 10 players | **No verdict exists** — no hardware in this environment (KD-35); the Pi acceptance procedure to run when hardware exists is in `BENCHMARK-BASELINE.md` §Pi-acceptance |
| Real-client acceptance | **No** — no Java client in this environment (KD-38); the protocol test client is the partner |

## 5. Document map

- `docs/phases/PHASE-09-REPORT.md` — the conformance/acceptance report with evidence
- `docs/testing/TEST-MATRIX.md` — what every test claims and how it could fail
- `docs/vanilla-parity/PARITY-MATRIX.md` — per-domain parity with cited evidence
- `docs/vanilla-parity/KNOWN-DIVERGENCES.md` — the divergence catalog (KD-01…)
- `docs/performance/BENCHMARK-BASELINE.md` — every benchmark record + the Pi acceptance procedure
- `docs/adr/ADR-0005-plugin-boundary.md` — plugin readiness: boundary, not API
- `docs/architecture/system-overview.md` — how the crates fit together
