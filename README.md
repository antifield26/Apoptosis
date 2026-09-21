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
- **Commands & data packs** — 15 commands with permission levels (`/op` and
  `/deop` persist grants to `ops.json`, which is also read at boot;
  `/gamemode`, `/give`, `/kill`, `/seed`, `/difficulty` are operator-only),
  plus `/function` running under the invoker's permissions; real data-pack
  loading: 758/758 vanilla tags resolve, 1 421 recipes load, functions run.
- **World generation** — seeded Perlin terrain with six biomes, trees, and a
  single-chunk subset of the jar's 1 202 structure templates.
- **Performance** — the documented 20 TPS acceptance procedure was executed on
  a Raspberry Pi 5: a 30-minute soak with 10 players held settled tick
  p50/p95/p99 medians of **0.21/0.27/0.29 ms** with zero overruns outside the
  join burst ([record](docs/performance/BENCHMARK-BASELINE.md)); a later mixed
  real+scripted soak on the current tree held ~3.0/3.2/3.3 ms medians with 58
  lifetime overruns (§P14-Pi in the same record).

## What is explicitly not here

This project records gaps instead of papering over them. The headline items
(full catalog: [docs/vanilla-parity/PARITY-MATRIX.md](docs/vanilla-parity/PARITY-MATRIX.md)):

- **Lighting is a static model, owner-confirmed** — per-state emission and
  dampening read from the jar's own accessors, sky flood from the heightmaps,
  block-light BFS, cached per chunk with invalidation, `light_update` sent on
  block change. Open: no day/night dimming (client-side) and no incremental
  relight (a torch costs a full recompute).
- **Entities persist and sync as drops/mobs** (P11-08, P12-05 for block
  entities); mobs still lack per-kind follow ranges, XP orbs, and paths.
- **Redstone is a measured model, wired into the tick loop** — directional
  conductivity, the 15-block wire and the vanilla differential pin it (P13);
  scheduled ticks propagate and broadcast. Open: pistons/observers/doors,
  exact update order, delay constants.
- **15 of ~90 commands**; chests, furnaces and hoppers open as windows a real
  client can transact with (P12), but no real-client container session has
  been run yet.
- **No full real-client acceptance yet** — a Java 26.1.2 client has joined,
  rendered night, mobs and drops (P10-11/P11 acceptance), and played a
  30-minute mixed 10-client soak (P14-06, pass with one noted idle spike),
  but container, pickup-render, death→respawn and restart screens are still
  unverified on a live client.
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
cargo test --workspace --no-fail-fast            # 1 524 passed / 0 failed / 35 ignored (118 suites)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets
cargo deny check licenses bans sources
```

The 34 ignored tests are on-demand suites: jar-gated differentials, light
suites and terrain distribution (three environment variables — see
[CONTRIBUTING.md](CONTRIBUTING.md)) and the benchmark harness. Full breakdown:
[docs/testing/TEST-MATRIX.md](docs/testing/TEST-MATRIX.md). Details and the
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
