# Changelog

All notable changes to this project are documented here. The project keeps a
linear history on `main`; this file distills it per phase. The complete
per-phase reports and five adversarial audits that this file condenses live in
git history — the pre-governance snapshot (which still contains them as files)
is the tag **`phase-09-final`** (`git show phase-09-final:docs/phases/…`).

Format follows [Keep a Changelog](https://keepachangelog.com/) in spirit. The first
entry is the release candidate matching the workspace version (`0.1.0` in
[Cargo.toml](Cargo.toml)); it is **published** as tag `v0.1.0-rc.1` with built
artifacts, and no later version has been released.

## Unreleased — Phase 10 (client compatibility and rendering)

### P10-01 — client-capture rig

A TCP proxy that relays a real Minecraft 26.1.2 client to the server **byte for byte** while writing a
normalized JSONL trace of the conversation, in both directions. It is CONVENTIONS.md §12's differential-testing
contract applied to clients instead of servers, and it exists because "a real client joined and it looked
right" is not evidence.

- **New crate** `apps/capture-rig` (`mc-capture-rig`), with the binary `capture-rig`. No new third-party
  dependencies: `mc-protocol`, `serde_json`, `md-5`, `tokio` were all workspace deps already.
- Frames are split by the project's own `FrameCodec`, with consumed-byte counts taken from `buffered()`
  deltas, so the bytes forwarded are the bytes read.
- **The observer is passive.** Forwarding never depends on decoding: if framing fails for a direction, that
  direction degrades to opaque passthrough and the trace records `observer_error` plus the direction in
  `session_end.degraded`. A capture that ends early is visibly early rather than looking like a short session.
- **Normalized for comparison:** the digest is over the *uncompressed* body, so two sessions differing only in
  compression produce identical digests; `wire_bytes` keeps the framed size separately; no per-packet
  timestamps, because a trace is meant to be diffable between runs.
- A session ends when **either** direction does, with a bounded 3 s drain, so a peer that never closes cannot
  leave a capture without an end marker.
- 10 tests: 8 unit (framing, state machine, compression in both wire forms, digest invariance, degradation,
  write-failure reporting) and 2 integration that drive the `TestClient` through the rig to a **real server**
  and require the trace to show handshake → login → config → play with no degradation.

**Two real defects the integration tests found**, both invisible to the unit tests:

1. The handshake intent was read as the payload's *second* VarInt, which is the **address length**. The state
   machine therefore never left `handshake`, and every later packet was interpreted against the wrong id
   table. The unit test passed because it built a payload matching the same wrong assumption; it now builds a
   real handshake with the typed encoder. *A unit test that constructs its input from the same mental model as
   the implementation cannot catch a wrong mental model.*
2. `relay_pair` waited for **both** directions, so a server holding its half open after the client left meant
   `session_end` was never written and the capture had no end marker.

Six falsification probes confirm the load-bearing mechanisms: framing, digest-over-uncompressed-body, typed
handshake decoding, the bounded drain, the per-connection sink, and degradation reporting. Each was disabled,
the covering test confirmed to fail, and the file restored byte-exact.

**Still blocked:** P10-02 and every acceptance task in this phase need an owner-provided runnable Java 26.1.2
client. The `TestClient` is not a substitute and has not been used as one.

## [0.1.0-rc.1] — 2026-09-12 (release candidate)

**Released.** Tag [`v0.1.0-rc.1`] with a GitHub Release carrying three assets:

| Asset | Size | SHA-256 |
|---|---|---|
| `mc-server-aarch64` | 4 526 384 B | `0a9575c7499c03573f4b83e0b4b762c60daff55ba49e0d87b2997d845baea3e3` |
| `mc-server-x86_64-windows.exe` | 3 385 344 B | `a11d6f06dd7269b9b3ecc68ff8735db4f502ae60bc66bf768e14f910adfd0b45` |
| `SHA256SUMS` | 185 B | `02c0a325ced2ea2eda5c444848d6fd09dcc8a2915b76ae58bc2dbab9d14e56b8` |

Both binaries are built from this tag; the aarch64 one was built **on the device** and is
byte-identical to the binary the Pi acceptance host runs (verified by SHA-256 during the
governance round, 2026-09-12). Build and release procedure:
[CONTRIBUTING.md](CONTRIBUTING.md).


### Phase 00 — Research (2026-09-10)

- Inventoried the four local reference clones (Pumpkin, Paper, Valence,
  Minestom) with licence and module citations; established the 26.1.2 protocol
  baseline (protocol 775, DataVersion 4790 — later measured, not guessed).
- Architecture decision: crate-per-boundary workspace, one tick thread with
  Tokio only at I/O edges, vanilla-compatible Anvil subset, 775-only protocol.
  Recorded as [ADR-0001](docs/adr/ADR-0001-system-architecture.md) with a risk
  register whose items (R-01…R-10) were tracked to closure across the project.
- Clean-room policy set: behaviour may be studied from references, source may
  never be copied ([NOTICE](NOTICE), [docs/legal/third-party.md](docs/legal/third-party.md)).

### Phase 01 — Foundation (2026-09-10)

- Virtual-manifest workspace (16 crates + server binary), pinned toolchain
  1.98.1, fmt/clippy/pedantic lint contract, CI workflow, TOML config with
  validation, structured logging, deterministic tick clock, lifecycle with
  graceful shutdown, shared test-support crate.

### Phase 02 — Network & protocol (2026-09-10)

- Tokio connection lifecycle, VarInt/VarLong and frame codecs with
  hostile-input caps (5/10-byte, 2 MiB), handshake/status/login/config state
  machines, offline authentication, compression negotiation with bomb
  rejection, connection admission limits, packet-fixture and fuzz harnesses,
  and an end-to-end test client reaching Play.

### Phase 03 — Persistence (2026-09-11)

- NBT (disk + network encodings), Anvil region reader/writer with atomic
  tmp→rename saves, chunk serde that preserves unknown fields, dirty tracking
  with retry-on-failure, autosave scheduling, corruption and restart suites.
- Closed risk R-03 by measurement: DataVersion 4790, `version` 19133; the
  writer stamps what vanilla 26.1.2 writes.
- The differential proof of the phase: a vanilla-world round trip where our
  rewritten regions and `level.dat` were accepted by a real vanilla server.

### Phase 04 — Survival vertical slice (2026-09-11)

- Block/item registries loaded from the jar's own registry dump (1 168 blocks,
  29 873 states, 1 506 items); world/chunk runtime with swept collision and
  ray casting; player state (health/hunger/XP with hostile-NBT hardening);
  break/place validation; death and respawn; join streaming with per-tick
  budgets; real-socket E2E tests; the first tick-cost baseline.

### Phase 05 — Simulation, entities, physics, AI (2026-09-11)

- Six-phase tick order as a compile-time contract, tick metrics, a
  byte-exact `java.util.Random` reimplementation (verified against JDK 25
  vectors — it caught two real bugs), entity lifecycle/ids that are never
  reused, item entities, projectiles, effect containers, mob tables and
  goal-based AI, bounded deterministic pathfinding, entity-heavy tick baseline.
- Opened with the second adversarial audit (data loss: streamed chunks could
  overwrite stored terrain; a placement DoS; RNG sign-extension bugs).

### Phase 06 — Inventory, containers, block entities, redstone (2026-09-11)

- Server-authoritative inventory transactions: stale-state-id resync,
  computed slots, conservation proven under 2 000-click adversarial floods;
  shaped/shapeless crafting; furnace with exact burn/cook accounting;
  hopper transfer model; block-entity lifecycle.
- Redstone: power model, budgeted propagation that never drops updates,
  golden circuit tests, determinism proofs — model-complete and explicitly
  not yet wired into the tick loop.
- Third adversarial audit; the first project commit (`b7b1c99`) landed here
  with owner approval.

### Phase 07 — Commands, data packs, worldgen (2026-09-11)

- Command tree/dispatcher with permission-before-grammar checking; eight
  commands reachable from a real client; `execute` modifier chains; `/function`
  with recursion and privilege bounds.
- Real data-pack loading: 758/758 vanilla tags resolve cleanly, 1 421 recipes
  load (94 counted as unmodelled), registry split fix, `ops.json` read at
  startup, pack discovery from world directories.
- Worldgen: seeded Perlin terrain, six biomes, trees, structure loading
  (1 182 of 1 202 templates) with a single-chunk placement policy, wired into
  generation with golden tests; existing-world-first generation.

### Phase 08 — Pi hardening & operations (2026-09-12)

- Operational guardrails: config bounds, structured 30-second metrics lines,
  shutdown barrier (drain 5 s → bounded save 30 s), systemd unit, offline
  whole-copy backup/restore with manifest and overwrite guard, named
  connection limits with a full-server bypass path, slow-drip and registry
  reservation caps, command-flood proof, admin-safety review.
- The benchmark harness (10-player workload driver, chunkgen burst, dirty-save
  timing, profile run) with the burst/settled separation rule; honest no-fix
  verdict where evidence did not support a change; operational runbook.

### Phase 09 — Conformance & release candidate (2026-09-12)

- Full-matrix sweeps across every domain (the sweep measured **1 194 / 0 /
  21 over 74 suites**; the remediation and Audit 08 coverage tests brought the
  tree to **1 196 / 0 / 21**); all seven differential suites green
  including the vanilla round trip; both build profiles measured; release
  build reproducible with a real-socket smoke.
- Known-divergence catalog, release-candidate documentation, plugin-boundary
  ADR (three named seams, zero API types), independent adversarial review
  (10 findings, all dispositioned), acceptance report.
- Fixed the one known flaky: three metrics tests shared a `TempDir` tag whose
  uniqueness collapsed under parallel I/O (probe-proven: duplicate paths in
  160 000 same-tag constructions); per-test tags now follow the codebase
  convention.

### Platform work (2026-09-12, post-phase)

- **Raspberry Pi 5 acceptance executed**: on-device release build with the
  pinned toolchain; 30-minute soak under the installed systemd unit with 10
  scripted clients — settled MSPT p50/p95/p99 medians 0.21/0.27/0.29 ms, zero
  overruns outside the join burst, clean autosaves, graceful stop verified.
  Full record: [docs/performance/BENCHMARK-BASELINE.md](docs/performance/BENCHMARK-BASELINE.md).
- **Deployment defect found and fixed by that run**: the registry tables were
  read from a build-tree path baked in at compile time, so the first real
  systemd application could not start. `Registries::vanilla()` now searches
  `$MC_FIXTURE_DIR`, then `fixtures/registry/` next to the executable, then
  the build tree, with ordering regression tests.

### Governance (2026-09-12)

- **MIT license adopted** (owner decision; [ADR-0006](docs/adr/ADR-0006-licensing.md),
  closing risk R-09) — `LICENSE`, license fields on all 17 manifests, the
  cargo-deny licence gate now covers the workspace's own crates.
- Source published at `github.com/antifield26/Apoptosis`.
- Repository governance: standard project facade (README, CHANGELOG,
  CONTRIBUTING, SECURITY, NOTICE, `.editorconfig`), the engineering
  conventions carried in-repo ([docs/CONVENTIONS.md](docs/CONVENTIONS.md)),
  divergence catalogs merged into a single parity matrix, the test matrix
  restructured to a current view plus a deduplicated defect history,
  per-phase reports distilled into this file and retired to git history, and
  the documentation-audit scripts committed to `tools/docs-audit/`.

[`v0.1.0-rc.1`]: https://github.com/antifield26/Apoptosis/releases/tag/v0.1.0-rc.1
