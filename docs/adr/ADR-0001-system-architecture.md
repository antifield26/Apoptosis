# ADR-0001 — System Architecture & Risk Register (P00-10)

Date: 2026-09-10. Status: **Accepted** (Phase 00 exit basis).
Context: from-scratch pure-Rust MC Java 26.1.2 dedicated server; 10-player Vanilla Survival;
Pi 5 8GB / Debian 13 / aarch64 first-class; Tokio; fixed 20 TPS; offline default.
Evidence: `docs/research/{reference-repos,protocol-baseline,architecture-comparison,parity-and-testing-strategy}.md`,
`docs/legal/third-party.md`, `docs/research/provenance.md`.

## 1. Decisions

### D-01 Crate boundaries (logical, created only with real behavior — AGENTS.md §7)

```text
crates/
  core/          # ids, math, time/tick types, error taxonomy (P01-05/09)
  nbt/           # NBT model/reader/writer shared by disk + network (P03-01..04, ADR-0002)
  protocol/      # 26.1.2 codec + packet types + version gate 775 (P02)
  network/       # Tokio accept/framing/lifecycle, validated-event handoff (P02)
  registry/      # ids/tags/data loading (seed P04-01, full P07-03)
  persistence/   # region/anvil/level/chunk-serde/dirty/autosave/save barrier (P03, ADR-0002)
  world/         # dimension/chunk/block-state runtime (P04-02)
  entity/        # entity lifecycle/state (P04-04/P05-03)
  simulation/    # 20 TPS scheduler + system ordering (P01-09 clock, P05-01/02)
  command/       # tree/dispatcher (P07-01/02)
  server/        # config/logging/lifecycle orchestration/world wiring (P01-06/07/08, P03)
  test-support/  # fixtures, mock client, trace compare (P01-10; never a prod dep)
apps/
  server/        # binary entrypoint
```

Phase 03 refinement (ADR-0002): the NBT implementation is shared by `protocol`
and `persistence` instead of living in either, and `persistence` owns the disk
schema (`ChunkData`) that `world` converts to and from — gameplay types never
reach the file format directly.

Rejected: one-big-crate (ownership unclear under concurrency contract) and ECS-full-adopt (Valence/Bevy migrates the whole App; cost unjustified — `architecture-comparison.md` §1).

### D-02 Threading / tick model (Pumpkin-shape, Minestom discipline, Valence boundary)

- One tick thread @50ms (`simulation`), overrun clamp (no catch-up spiral), frozen-state still flushes network + keepalive.
- Tokio ONLY at I/O edges (accept, codec, read/write, RCON/Query); gameplay/simulation sync; bridges via bounded channels + `block_in_place`/oneshot for short tasks, worker pool (Rayon-style batch) for chunk gen/compression/persistence with **tick-boundary join** (completion never reorders gameplay).
- Inside tick: phases serial (`net-drain → tasks → worlds → entities → block-entities → packet-flush`), batches parallel (`par_chunks(8..32)` scale TBD by Pi evidence).
- Cross-thread borrows follow `sync/trySync` ownership semantics + affinity assertions (TickThread-style); every boundary documents owner/primitive/ordering/backpressure/failure/shutdown (AGENTS.md §8). TICK_START/TICK_END bracketing + TickMonitor-style observability from day one (P01-08/09).
- Alternatives rejected: per-connection gameplay threads (ordering chaos), Bevy-schedule-everything (framework lock-in), Folia-style region threads (post-release).

### D-03 Networking boundary

TCP/Tokio → decode/validate → bounded queue → tick consumes intents → sim mutates → outbound events → packet scheduler → socket. Auth-critical packets (handshake/status/login/config acks/keepalive) fast-pathed; gameplay packets tick-queued (cf. Minestom `IMMEDIATE_PROCESS_PACKETS`, `PlayerSocketConnection.java:62-74,144-178`). Malformed input never panics; every hostile class gets a regression test (AGENTS.md §10).

### D-04 Persistence strategy

- Format: Vanilla-compatible **Anvil subset** (8KiB header, sector 4KiB, compression ids kept, keep-original-compression, `tmp→rename`, `level.dat_old` backup).
- Layouts: read new `dimensions/<ns>/<value>/region` first, fall back to legacy root `region/` (26.1 break, `AnvilLoader.java:81-96`); write new layout only.
- Versions: reader accepts `[4435, CONFIRMED_26_1]`; writer stamps confirmed 26.1 DataVersion (NOT Pumpkin's 4903/26.2). Exact value resolved per `protocol-baseline.md` §2 before P03-06.
- Memory: hot-chunk struct (palettes + light + heightmaps + tick schedulers + entities + status + AtomicBool dirty); cold/proto staging deferred (no DAG scheduler in P03).
- I/O: per-file lock + watcher refcount + region grouping + snapshot-clear-dirty + save barrier + LRU handle cap; autosave ticks configurable, 0=disable.
- NBT: tolerant read (named/unnamed, palette forms, legacy difficulty/spawn fallbacks) / complete write per Pumpkin field superset, reimplemented clean-room.

### D-05 Protocol policy

**775-only** for 26.1.x (26.1.2 shares 775 — `protocol-baseline.md` §1); Display `"26.1"` on wire, `"26.1.2"` in status/config surfaces only. States: Handshake/Status/Login/Config/Play (+Transfer intent folded into Login). Offline auth default path; online-mode (Mojang session) behind config as a provider boundary (structurally present P02-08, validated P08). Five-state machine with Play↔Config re-entry (StartConfiguration/ConfigurationAck).

### D-06 Determinism

Same seed + ordered inputs + tick count → same normalized state (AGENTS.md §3.6). Worker completion joins at tick boundary; scheduled-tick ring (256 slots + priority, Pumpkin-world shape) snapshots before parallel waves. Deliberate nondeterminism (if any) documented per-site. Regression scenarios from P05-02/17.

### D-07 Plugin readiness = boundary, not abstraction (AGENTS.md §2, MASTER §7)

No plugin API types in core. Conceputal seam only: tick events (start/end), command registration point, datapack-function hook — each introduced only when exercised by real code (P09-12 writes the boundary ADR).

### D-08 Dependencies (initial, P00-09 ratification)

Adopt in P01: `tokio, serde, serde_json, toml, tracing, tracing-subscriber, thiserror, bytes` (all MIT/Apache-2.0; row each in `third-party.md` at adoption). Compression crate chosen in P03 on license+aarch64 evidence. `cargo deny`-equivalent license gate in CI (P01-03). GPL/AGPL vendoring forbidden without owner+legal decision (none made; project itself unlicensed as of this ADR).

## 2. Risk register

| ID | Risk | Likelihood / impact | Mitigation | Owner phase |
|---|---|---|---|---|
| R-01 | No `.git` in reference clones → no SHA traceability; future upstream drift unverifiable | M / M | Cite file:line + version constants; re-verify wire truth against real 26.1.2 in P02; record refreshed SHAs if clones re-fetched | P02-16 |
| R-02 | 26.1.2 wire differs from 26.1 base (packet ids/fields) despite shared proto 775 | M / H | Capture real 26.1.2 session in P02; golden fixtures per packet family; conformance report gate | P02-11/16 |
| R-03 | ~~Exact 26.1 DataVersion/Level-version unconfirmed (only range [4435,4903]/[19132,19133] known)~~ **RESOLVED P03** | H / H → closed | Measured on a vanilla 26.1.2 world generated on this host: DataVersion **4790**, level `version` **19133**; corroborated by `DetectedVersion` bytecode and the jar's `version.json`. Writer stamps 4790, reader accepts [4435,4790] (`protocol-baseline.md` §2) | P03-06 (done) |
| R-04 | GPL contamination (Pumpkin GPL-3.0, Paper GPLv3) via copy/translate/vendor | M / H (license forces) | Clean-room rule + provenance log + no-vendor policy (`third-party.md` §2); review-agent checks each phase | every phase |
| R-05 | Pi 5 20 TPS with 10 players unproven on this codebase (no code yet) | H / H | Pi harness early (P04-18 baseline), no pre-threshold tuning (AGENTS.md §13), profile-gated fixes only (P08-14) | P04-18, P08-09..14 |
| R-06 | Async bleed: gameplay becomes async via Tokio convenience | M / M | D-02 boundary + review checklist (`EXECUTION-LOOP.md` §5); clippy/lint custom pass if drift seen | P01-04, reviews |
| R-07 | Vanilla world corruption (layout break 26.1, DataVersion stamp, sector bugs) | M / H | Dual-layout reader, range-checked versions, tmp→rename + `_old` backup, restart/corruption suites, vanilla-world load test | P03-14/15/16 |
| R-08 | Valence staleness (1.20.1) misleads protocol work | L / M | Valence excluded as protocol oracle (4-state, no Config); used only for codec style/boundary ideas | P02 |
| R-09 | No project license chosen → GPL-reuse automatically forbidden, but also distribution unclear | L / M | Flag for owner; no release artifacts until decided (P09-09 blocked on it) | P09 |
| R-10 | Repo has zero commits (resolved 2026-09-11: initial commit `b7b1c99`); all Phase-00 evidence is docs-only | — / L | Initial commit recommended post-gate (owner approval); docs are the deliverable, no prod claims made | P00 exit |

## 3. Consequences

- Phase 01 builds the boring reproducible workspace (toolchain pin, CI x86_64+aarch64, config/logging/error/clock/lifecycle/test-support) with zero gameplay.
- Phase 02 proves a real 26.1.2 client reaches Play in offline mode + hostile-input suite.
- Phase 03 blocked on R-03 resolution for its writer; reader work may precede with range-acceptance.
- Review/Test agents gate every phase; BLOCKER findings stop phase exit.
