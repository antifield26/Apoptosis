# Release Candidate Documentation (P09-10)

Status: **release candidate, published as source under MIT** (ADR-0006, owner
decision 2026-09-12 — closes ADR-0001 R-09). The full history is on
`github.com/antifield26/Apoptosis` (public) as of 2026-09-12; the binary below
is a local build and no tagged binary release exists yet. This document is what
a builder or operator needs to produce and run the server from source today.

## 1. What ships

A from-scratch, pure-Rust dedicated server for Minecraft: Java Edition
**26.1.2** (protocol **775**, wire display string "26.1"), offline mode,
Vanilla-Survival slice: login → config → play, movement/collision, break/place,
inventory transactions, containers transactable, death/respawn, commands (15 —
help/list/say/time/tp/execute/function/op/deop/stop plus the P14 admin set
gamemode/give/kill/seed/difficulty), data
packs (tags/recipes/functions), seeded terrain with a single-chunk structure
subset, Anvil-compatible persistence verified against real vanilla worlds.
Scope boundaries and every known divergence:
[docs/vanilla-parity/PARITY-MATRIX.md](../vanilla-parity/PARITY-MATRIX.md) (KD-01…KD-39; the
release-affecting highlights: static lighting only (no day/night dimming, no
incremental relight); mobs lack per-kind follow ranges, XP orbs and paths;
redstone is measured and tick-driven but has no pistons/observers and no exact
update order; 15 of ~90 commands).

## 2. Building from source

Requirements: Rust **1.98.1** — pinned, not advisory; `rust-toolchain.toml`
selects it automatically with rustup, and a different toolchain is a build
reproducibility failure, not a configuration.

```text
cargo build --workspace --release --locked     # the server binary
cargo test  --workspace --no-fail-fast         # the full matrix (current figures in docs/testing/TEST-MATRIX.md)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets  # the Pi 5 target compiles (no cross-run on x86_64)
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
| Full regression matrix passes | **Yes** — five gates green on 2026-09-12, the day this candidate was cut: `cargo test --workspace --no-fail-fast` (**1 212** passed / 0 failed / 21 ignored, 78 suites), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --target aarch64-unknown-linux-gnu`, `cargo deny check licenses bans sources` — all exit 0. **That figure is this release's own, and is not restated as current**: the tree has moved on (1 344 / 0 / 30 / 102 when P11-04..09 landed; 1 346 / 0 / 33 / 104 after AUDIT-10; 1 358 after AUDIT-11 remediation; 1 365 at `v0.1.0-rc.1`'s follower; 1 380 at the P12 landing; **1 384** after AUDIT-12 remediation — P13 not started), and `docs/testing/TEST-MATRIX.md` owns the current number. The table records what the release measured, which is why it keeps its own figure rather than tracking the owner. **The run logs live under the git-ignored `target/` on the build machine, not in this repository**, so they are evidence a reader cannot inspect; re-run the five commands in §2 to reproduce them. The P09 logs that were originally cited here record **1 189**, a run from before five tests were added — and this row previously said "the current figure is 1 196", which was true when written and is exactly the drift this note now prevents |
| Conformance report produced | Yes — the Phase 09 acceptance report (git history, tag `phase-09-final`) + the sweep rows in `docs/testing/TEST-MATRIX.md` |
| Known divergences documented | Yes — [PARITY-MATRIX.md](../vanilla-parity/PARITY-MATRIX.md), the single authority (KD-01…KD-39 tagged in its rows) |
| Release artifacts published | **Yes** — tag `v0.1.0-rc.1` on GitHub Releases with the x86_64 (Windows) and aarch64 (Pi) `mc-server` binaries and SHA-256 sums; source is the repository itself (public, MIT) |
| 20 TPS on Pi 5 with 10 players | **Met for the scripted workload** — the `BENCHMARK-BASELINE.md` §4 acceptance run executed on a Pi 5 (Debian 13, release, on-device build): 30-min soak, settled MSPT medians 0.21/0.27/0.29 ms, zero settled overruns (§P09-Pi). Boundaries: scripted clients (KD-38), loopback, microSD |
| Real-client acceptance | **No** — no Java client was driven; the scripted clients speak the exact TestClient conversation the E2E suite proves (KD-38) |

## 5. Document map

- the Phase 09 acceptance report — git history (tag `phase-09-final`), distilled into `CHANGELOG.md`
- `docs/testing/TEST-MATRIX.md` — what every test claims and how it could fail
- [vanilla-parity/PARITY-MATRIX.md](../vanilla-parity/PARITY-MATRIX.md) — per-domain parity with cited evidence
- `docs/vanilla-parity/PARITY-MATRIX.md` — the single parity/divergence authority (KD-01…KD-39)
- `docs/performance/BENCHMARK-BASELINE.md` — every benchmark record + the Pi acceptance procedure
- `docs/adr/ADR-0005-plugin-boundary.md` — plugin readiness: boundary, not API
- `docs/architecture/system-overview.md` — how the crates fit together
