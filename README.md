# Apoptosis — a pure-Rust Minecraft Java Edition 26.1.2 server

A from-scratch, dependency-light dedicated server for **Minecraft: Java Edition
26.1.2** (protocol **775**), written entirely in safe Rust. First-class target:
Raspberry Pi 5 8 GB (aarch64, Debian 13) serving 10 concurrent Vanilla-Survival
players. It runs offline-mode by default and persists worlds in the vanilla
Anvil format.

| | |
|---|---|
| License | [MIT](LICENSE) (see also [NOTICE](NOTICE)) |
| Target protocol | 775 (Minecraft 26.1.2; wire display string "26.1") |
| Toolchain | Rust **1.98.1**, pinned in [rust-toolchain.toml](rust-toolchain.toml) |
| Platform | x86_64 (dev/CI) + aarch64 (production target) |
| CI | GitHub Actions (`ci` workflow) — fmt, clippy, tests, aarch64 check, cargo-deny |
| Status | **Release candidate `v0.1.0-rc.1`** — binaries on [GitHub Releases](https://github.com/antifield26/Apoptosis/releases); see [docs/release/RELEASE-CANDIDATE.md](docs/release/RELEASE-CANDIDATE.md) |

## What works today

Every claim below is backed by a test or a measurement; the evidence links live
in [docs/vanilla-parity/PARITY-MATRIX.md](docs/vanilla-parity/PARITY-MATRIX.md).

- **Protocol & networking** — handshake/status/login/config/play over Tokio
  TCP, packet ids verified against the official jar's registration bytecode,
  hostile-input hardening (non-terminating varints, oversized frames,
  decompression bombs, slow drips, floods), per-IP and global admission limits.
- **Survival slice** — join/stream, server-authoritative movement with swept
  collision, block break/place validation, inventory transactions
  (server-authoritative, conservation-proven under 2 000-click floods), health/
  hunger/XP, death and respawn, save/reload.
- **Persistence** — NBT + Anvil region read/write with atomic saves, verified
  end-to-end: 529 vanilla chunks decoded, rewritten by this server, then
  **booted on a real vanilla 26.1.2 server**, which preserved our edits and
  re-saved every dimension.
- **Commands & data packs** — 8 commands plus `/function` with permission
  levels (`ops.json` is read), real data-pack loading: 758/758 vanilla tags
  resolve, 1 421 recipes load, functions run under the invoker's permissions.
- **World generation** — seeded Perlin terrain with six biomes, trees, and a
  single-chunk subset of the jar's 1 202 structure templates.
- **Performance** — the documented 20 TPS acceptance procedure was executed on
  a Raspberry Pi 5: a 30-minute soak with 10 players held settled tick
  p50/p95/p99 medians of **0.21/0.27/0.29 ms** with zero overruns outside the
  join burst ([record](docs/performance/BENCHMARK-BASELINE.md)).

## What is explicitly not here

This project records gaps instead of papering over them. The headline items
(full catalog: [docs/vanilla-parity/PARITY-MATRIX.md](docs/vanilla-parity/PARITY-MATRIX.md)):

- **No lighting propagation** — chunks are sent with zero light masks, so
  clients render them dark.
- **Entities are not persisted or synced** — mobs and dropped items vanish on
  restart; clients never see entities.
- **Redstone is a tested model, not wired into the tick loop.**
- **8 of ~90 commands**; only the player inventory can open as a window.
- **No real-client acceptance yet** — the protocol test client and scripted
  soak clients are the partners; a Java client may disagree somewhere the
  fixtures cannot see.
- Offline mode only: enabling `online_mode` refuses to start (no Mojang
  session flow is implemented).

## Build and run

Requirements: Rust **1.98.1** (rustup installs the pinned toolchain
automatically), plus a C toolchain for linking.

```sh
cargo build --workspace --release --locked
cargo run --release -p mc-server-app            # or: target/release/mc-server [config.toml]
```

Copy [config.example.toml](config.example.toml) and edit `bind`, `world_dir`,
`max_players`, `view_distance`. A first start creates a fresh
`level.dat` (DataVersion 4790); a second start reuses it. **The registry tables
are read at runtime from `fixtures/registry/` next to the binary** — for a
deployed layout, install them as shown in
[docs/operations/RUNBOOK.md](docs/operations/RUNBOOK.md), which also covers the
systemd unit ([deploy/mc-server.service](deploy/mc-server.service)), backup and
restore.

## Testing

```sh
cargo test --workspace --no-fail-fast            # 1 358 passed / 0 failed / 33 ignored (106 suites)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target aarch64-unknown-linux-gnu
cargo deny check licenses bans sources
```

The 21 ignored tests are on-demand suites: 7 differential suites that need a
real 26.1.2 server jar (three environment variables — see
[CONTRIBUTING.md](CONTRIBUTING.md)) and the benchmark harness. Details and the
documentation-audit scripts: [CONTRIBUTING.md](CONTRIBUTING.md).

## Project documentation

| Layer | Entry points |
|---|---|
| Current state | [docs/release/RELEASE-CANDIDATE.md](docs/release/RELEASE-CANDIDATE.md) · [docs/architecture/system-overview.md](docs/architecture/system-overview.md) · [docs/vanilla-parity/PARITY-MATRIX.md](docs/vanilla-parity/PARITY-MATRIX.md) · [docs/testing/TEST-MATRIX.md](docs/testing/TEST-MATRIX.md) |
| Reference baselines | [docs/protocol/](docs/protocol/26.1.2-wire-notes.md) · [docs/research/](docs/research/protocol-baseline.md) · [docs/performance/BENCHMARK-BASELINE.md](docs/performance/BENCHMARK-BASELINE.md) |
| Operations | [docs/operations/RUNBOOK.md](docs/operations/RUNBOOK.md) · [docs/operations/DEPENDENCY-POLICY.md](docs/operations/DEPENDENCY-POLICY.md) |
| Decisions (ADR) | [docs/adr/](docs/adr/ADR-0001-system-architecture.md) — accepted with dated status |
| History | [CHANGELOG.md](CHANGELOG.md) — earlier per-phase reports and audits live in git history (pre-governance snapshot: tag `phase-09-final`) |
| Contribute | [CONTRIBUTING.md](CONTRIBUTING.md) · [docs/CONVENTIONS.md](docs/CONVENTIONS.md) |

## License and attribution

This project is [MIT-licensed](LICENSE). It is an independent clean-room
implementation: behaviour and public facts were studied from reference
projects; no source code was copied. Attribution and license notes for the
reference projects (Paper, Pumpkin, Valence, Minestom) and the Mojang data
formats are in [NOTICE](NOTICE).
