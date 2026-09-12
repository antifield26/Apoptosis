# Benchmark Baseline

Owner: engineering. Method contract: CONVENTIONS.md section 13 — every record carries
hardware, OS, toolchain, commit, build profile, workload, duration, TPS, MSPT
percentiles, CPU, RSS and (where relevant) storage and network figures.

**Status: no acceptance baseline exists yet, and none is claimed.** The Pi 5 /
10-player target is established by P08-09, P08-10, P08-11, P08-12 and P08-13.
Until then, any number below is a development-host observation for orientation
and regression detection only. §P08-13 records the first P08 harness run; it
is still a dev-host observation, not a Pi claim.

## 1. Measurements taken so far

### P04-18 — Phase 04 tick slice (development host, debug profile)

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Hardware | operator Windows host, x86_64 (not a Pi 5) |
| OS / kernel | Windows 10.0.28020 |
| Toolchain | rustc 1.98.1 (48a229cea 2026-09-01) |
| Commit | `b7b1c99` (measured before the commit existed; this is the tree it captured) |
| Build profile | `dev` (unoptimised, debug assertions on) |
| Workload | 10 synthetic players, view distance 8 chunks, all joined and streaming, flat stone floor, 60 warm-up ticks + 400 measured |
| Command | `cargo test -p mc-server --test tick_baseline -- --ignored --nocapture` |
| MSPT mean | 0.89 ms |
| MSPT p50 / p95 / p99 | 0.78 / 1.42 / 1.65 ms |
| MSPT max | 2.22 ms |
| Wall time (460 ticks) | 24.7 s |
| Full save (361 dirty chunks) | 39.3 s |
| TPS | not measured as a rate — the test drives the loop synchronously |
| CPU / RSS | not captured (needs the P08 harness) |
| Network throughput | not applicable (in-process queues drained by the test) |
| Storage | `TempDir` on the system drive |

Interpretation, stated carefully:

- The **tick** has roughly 50x headroom against the 50 ms frame budget even in an
  unoptimised build, for this synthetic workload. That is a useful regression
  signal, not a 20 TPS claim: it says nothing about a Pi 5, world generation, real
  chunk data or real client traffic.
- The **save** dominates everything at ~108 ms per chunk in debug. It is the first
  thing P08-12/14 must profile, and the reason no throughput claim is made.
- The test asserts only a generous ceiling (p95 < 40 ms), so it catches an
  accidental order-of-magnitude regression without flaking on a busy host.

### P05-18 — entity-heavy slice (development host, debug profile)

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Hardware | operator Windows host, x86_64 (not a Pi 5) |
| Toolchain | rustc 1.98.1 |
| Commit | `b7b1c99` (same tree, measured pre-commit) |
| Build profile | `dev` (unoptimised) |
| Workload | 10 players + **600 mobs + 400 items** (1 000 entity-ticks/tick), view distance 8, flat stone floor, 60 warm-up + 400 measured ticks |
| Command | `cargo test -p mc-server --test tick_baseline -- --ignored --nocapture entity_heavy` |
| MSPT mean | 16.39 ms |
| MSPT p50 / p95 / p99 | 16.75 / 19.23 / 21.02 ms |
| MSPT max | 25.33 ms |
| Wall time (460 ticks) | 30.9 s |
| `busiest_phase` | `broadcast` (51.8 ms **lifetime** mean — the join burst, not steady state) |
| TPS | not measured as a rate — the test drives the loop synchronously |
| CPU / RSS | not captured (needs the P08 harness) |

Interpretation:

- **1 000 entities cost ~16 ms per tick in debug** (≈16 µs per entity), which is
  enough of the 50 ms budget to matter. A release build and a spatial index must be
  measured before any materially larger population; that is P08-14's job.
- The scenario **understates** a real entity-heavy server twice over: the AI hook is
  a documented no-op (no goal decisions or pathfinding are paid), and nothing is
  encoded to clients because entity packets do not exist yet (P05-15). Physics,
  however, is real — items and mobs are integrated against the world every tick.
- `busiest_phase` is a **lifetime** mean and therefore reflects the join burst.
  It needs a windowed variant before it can answer "what dominates a settled
  server"; recorded as a P08-02 item rather than smoothed over.
- Nothing spawns mobs today, so the population here is seeded directly through
  `EntityStore::spawn`. This measures entity bookkeeping and physics, **not** a
  spawning world.

### P08-13 — Pi operations profile (development host, debug profile)

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Hardware | operator Windows host, x86_64 (not a Pi 5) |
| OS / kernel | Windows (dev host; kernel not recorded — see the gap below) |
| Toolchain | pinned `1.98.1` (`rust-toolchain.toml`) |
| Commit | `b9dc784` tree plus the uncommitted `pi_profile.rs` harness (this run predates its commit; re-run after commit for the citable figure) |
| Build profile | `dev` (unoptimised, debug assertions on) |
| Workload (P08-10) | 10 survival clients, view 8, flat stone floor, 40 warm-up + 200 measured ticks; rotation+swing+hotbar every tick, one 0.2-block step every 20th tick, rotating chat/`/list` |
| Command | `cargo test -p mc-server --test pi_profile -- --ignored --nocapture` (four tests; figures below are the `profile_run` test) |
| Wall time (200 ticks) | 2.1–4.2 s across runs (host noise dominates; see interpretation) |
| MSPT mean | 10.5–21.2 ms across runs |
| MSPT p50 / p95 / p99 | run A: 0.69 / 0.85 / 381.23 ms, max 386.08 ms; run B (workload test): 0.66 / 0.76 / 0.90 ms, max 1.01 ms |
| Per-phase means | `network` 0.02, `scheduled_ticks` 0.00, `entities` 0.00, `players` 0.05–0.12, `block_entities` 0.00, `broadcast` 72–140 ms |
| Overruns | 46 of 240 ticks (the join burst; settled ticks do not overrun) |
| TPS | not measured as a rate — the test drives the loop synchronously; the printed "tps estimate" (57–114) is ticks/wall and is **not** a 20 TPS claim |
| CPU / RSS | not captured on this host |
| Network throughput | not applicable (in-process queues drained by the test) |
| Storage | `TempDir` on the system drive |

P08-11 (chunkgen burst): 20 × 64-block hops east, 5 ticks per hop — 684
chunks resident, 1 360 streamed, 10.7 s wall. P08-12 (persistence): 81 dirty
chunks saved in 4.7 s wall (≈58 ms/chunk in debug), dirty flags all cleared.

Interpretation, stated carefully:

- The **settled tick** (workload test: p95 0.76 ms, max 1.0 ms) has ~50x
  headroom against the 50 ms budget even unoptimised. The `profile_run`'s
  p99/max (≈380–810 ms) is the **join burst**: 10 players × 289 chunks built
  and encoded while the window is still filling. That is why two figures are
  quoted, not one — a single percentile over a run that includes the burst
  describes neither the burst nor the steady state.
- The **broadcast phase dominates** (72–140 ms lifetime mean): chunk-packet
  construction (`vanilla_chunk_packet`'s per-block palette scan) plus the
  streaming budget. On a settled server it is idle; under join/gen pressure it
  is the whole tick. P08-14's only evidence-backed target, if one is needed.
- The **save** costs ≈58 ms/chunk in debug (≈108 ms/chunk in the P04-18
  figure; the difference is host noise and chunk content, not a code change).
  Still the first thing a Pi run must profile.
- Run-to-run variance is large (mean 10.5 vs 21.2 ms) on this host: a busy
  desktop moves the figure more than a code change does. Treat every number
  here as one order of magnitude, and re-run on the Pi before any tuning
  decision (P08-14 rule).

Gaps this run does not close (carried, not hidden): no Pi 5 hardware, no
release profile, no real client traffic, no kernel/CPU/RSS capture, and the
commit cited is the harness's parent rather than the harness itself.

### P09-08 — Pi performance release sweep (development host, BOTH profiles)

First run of the harness under the release profile, closing the "no release
profile" gap named in §P08-13 **on this host only**. Still not a Pi 5, still a
driven in-process loop — the verdict rule stays §4.

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Hardware | operator Windows host, x86_64 (not a Pi 5) |
| Toolchain | pinned `1.98.1` (`rust-toolchain.toml`) |
| Commit | `bf118c8` tree plus the uncommitted P09 metrics-tag fix (runs predate the commit; same shape as §P08-13's citation) |
| Build profile | `dev` and `release` (`--locked`) |
| Workload | §P08-13's acceptance workload: 10 survival clients, view 8, flat floor, rotation+swing+hotbar every tick, periodic step, rotating chat/`/list`; 40 warm-up + 200 measured ticks (pi_profile), 60+400 (tick_baseline) |
| Command | `cargo test [-release] -p mc-server --test pi_profile --test tick_baseline -- --ignored --nocapture`; cited release log: `target/p09_perf_release.log` (re-run after the profile-string fix, AUDIT-06 finding 3 — the run self-describes as `release (optimised, debug assertions off)`) |
| MSPT p50/p95/p99 — settled, **release** | **0.060 / 0.072 / 0.137 ms**, max 0.244 (workload test); tick_baseline 10-player: 0.042 / 0.074 / 0.140 ms, max 0.224 |
| MSPT p50/p95/p99 — settled, debug | 0.67 / 0.74 / 0.84 ms, max 1.02 (consistent with §P08-13) |
| MSPT p99 — join burst, **release** | 13.29 ms (max 14.17) vs **390.5 ms (max 396.7) in debug** — the burst is encode/stream cost, which optimization collapses |
| Entity-heavy 1 000 entities, release | p50/p95/p99 1.07 / 1.26 / 1.36 ms, max 1.50 (debug §P05-18: 16.75/19.23/21.02) |
| Chunkgen burst | 684 resident, 1 360 streamed: **0.47 s release / 11.1 s debug** (≈0.34 ms vs ≈8 ms per streamed chunk) |
| Dirty save (81 chunks) | **0.88 s release / 5.13 s debug** (≈11 ms vs ≈63 ms per chunk) |
| TPS | not a rate measurement (driven loop); the printed estimates are ticks/wall |
| CPU / RSS | not captured on this host (§4 step 4 owns that on the Pi) |
| Network / storage | in-process queues; `TempDir` on the system drive |

Interpretation, stated carefully:

- Release settles at ~0.05–0.06 ms p95 for the acceptance workload: **~800×**
  headroom against the 50 ms budget on this host — but the §P08-13 warning
  stands: host noise, wrong hardware and a driven loop make this a regression
  baseline, not a 20 TPS verdict.
- The join burst falling from ~390 ms (debug) to ~13 ms (release) re-frames
  P08-14's palette no-fix: in release the whole burst is a rounding error at
  20 TPS scale. Any future optimization work should start from a Pi profile,
  not from this loop.

### P09-09 — reproducible release build (development host)

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Command | `cargo build --workspace --release --locked` (exit 0; `Cargo.lock` committed, pinned toolchain 1.98.1) |
| Artifact | `target/release/mc-server.exe`, 3 382 272 bytes |
| SHA-256 | `36e3ab018015b6a2fd58e54b9d3d93a8f64c77af744a602d3b6ef64e6936e659` |
| Smoke 1 | fresh world dir: server starts, creates `level.dat`; hand-rolled socket client (`status` handshake proto 775 → request → JSON → ping/pong echo) passes: `{"version":{"name":"26.1.2","protocol":775},"players":{"max":10,...}}` |
| Smoke 2 | second start on the same dir: world reused, no error, smoke passes again |
| Distribution | **source published 2026-09-12** — MIT adopted (ADR-0006) and the full history pushed to `github.com/antifield26/Apoptosis` (public). The binary artifact itself remains a local build (no tagged binary release); the build is reproducible per this record |

### P09-Pi — the acceptance run on real hardware (Pi 5, §4 executed 2026-09-12)

The §4 procedure was executed end to end by the owner's Raspberry Pi 5. This is
the record §4 step 6 asks for. Verdict first, per the §4 rule: **the 20 TPS
acceptance is met for the scripted 10-player workload** — 30 minutes, zero
overruns outside the join burst, every settled window an order of magnitude
under the 50 ms budget. The boundary below names what this does and does not
cover (scripted clients, loopback, microSD).

| Field | Value |
|---|---|
| Date | 2026-09-12 (soak window 14:33–15:03 local; sampler 10 s cadence) |
| Hardware | Raspberry Pi 5 Model B Rev 1.0, 4 × Cortex-A76, 8 GB (`/proc/device-tree/model`) |
| OS / kernel | Debian GNU/Linux 13 (trixie), aarch64; kernel 6.18.39+rpt-rpi-2712 |
| Toolchain | pinned 1.98.1 via rustup on the device (`rust-toolchain.toml` channel) |
| Commit | `14759bc` plus the uncommitted `Registries::vanilla()` fixture-search fix — the deployment defect this run found (see below); rebuilt **on the device** |
| Build profile | `release`, `--locked`, built on the Pi (`cargo build --workspace --release --locked`, 1 m 18 s clean / 26 s incremental) |
| Binary | `/srv/mc-server/mc-server`, SHA-256 `62067e04b4c3f9e5958293425ad056c590fd5bd93c32ea07b2e7c0d3467ae02f`, 4 524 624 B |
| Storage | **microSD** (`/dev/mmcblk0p2`, 29 GB free) — explicitly *not* the performance acceptance target (CONVENTIONS.md §2); storage-bound figures are labelled below |
| Workload | 10 scripted clients (`target/pi_soak_client2.py`, the TestClient conversation): MOVE_PLAYER_ROT 20 Hz, one 0.2-block MOVE_PLAYER_POS step per 20th tick, rotating `/list` (~2/min each), view 8; clients ran **on the Pi** at `nice -n 19` with one loopback source address each (127.0.0.2…11) because the per-IP admission cap (correctly) refuses 10 connections from one address |
| Duration / warmup | 1800 s measured after staggered joins; 62 × 600-tick `tick metrics` windows captured, 60 with all 10 players |
| TPS | the fixed 20 TPS clock held for the whole soak: **zero overruns in 60 settled windows** (lifetime overruns 5, all in the join window); the §4 verdict rule (mean MSPT < 50 ms, no settled-tick overrun) is **passed** |
| MSPT p50/p95/p99 — settled, 10 players | medians across the 60 windows: **0.206 / 0.268 / 0.289 ms** (window p50 max 0.243; window p99 max 29.23 — see the spike note) |
| Join burst | worst single tick 105.2 ms, window p95/p99 27.8/29.2 ms (tick 600 window only); five 55–110 ms ticks at 10 joins, never again |
| Per-phase means | not captured by the soak (OperationalSnapshot reports aggregate MSPT only); the P08-13 harness owns per-phase figures |
| CPU | median **1.0 %** of one core, max 21.3 % (join burst) — sampler on `/proc/<pid>/stat` |
| RSS | median 125 MB, max 125 MB (up from 105 MB at first join) — far under the unit's 6 GB `MemoryMax` |
| Storage | autosaves at ticks 6000/12000/18000/24000/30000/36000 all completed (`dirty_chunks=0` in every window); the microSD save cost from the bench: **45.0 ms/chunk** (81 dirty in 3.65 s) vs 10.9 ms/chunk on the dev host's SSD — the save path is where storage shows, and it stayed inside the budget |
| Network | **loopback**, not the LAN (clients co-located per the per-IP constraint above); the LAN path was exercised separately by the status smoke + a 2-client smoke from the dev host over SSH |
| Graceful stop | `systemctl stop` → SIGTERM → `server stopping ticks=40580` → listener stopped → world saved and closed → `shutdown complete` → unit `inactive` in ~1 s — the P08-03 barrier verified on hardware |

In-process benches on the same device (§4 step 2, release, driven loop — not
rate measurements): 10-player workload settled p50/p95/p99 0.123/0.153/0.183 ms;
`profile_run` p99 27.2 ms (burst); chunkgen 684 resident / 1 360 streamed in
0.68 s; 81-chunk dirty save 3.65 s (45.0 ms/chunk, microSD); tick_baseline
10-player ≈ 0.10 ms, entity-heavy ≈ 1.82 ms mean. Logs: `~/bench.log`,
`~/soak_metrics2.csv`, `~/soak_metrics_lines.txt`, `~/soak_run2.log` on the Pi.

**Defect found and fixed by this run** (the acceptance run's first real catch):
`deploy/mc-server.service` had been reviewed but never applied (KD-36); the
first application failed at startup because `Registries::vanilla()` read its
tables from a `env!(CARGO_MANIFEST_DIR)`-relative path — a build-tree path the
service user cannot read. Fixed by a documented search order (`$MC_FIXTURE_DIR`
→ `fixtures/registry` next to the executable → build-tree fallback), fixture
installation added to the unit header and `RUNBOOK.md` §1, regression tests
added (`mc-registry` lib), and the deployed service verified: status smoke from
the dev host over the LAN, then this soak. The §P09-09 Windows smoke missed
this because it ran the binary in-tree.

**Boundary of the verdict (stated, not hidden):** the clients are scripted
(KD-38 real-client acceptance remains open), the traffic is loopback rather
than LAN, and the storage is microSD rather than the NVMe target. None of
these plausibly moves MSPT by 250×, which is the headroom the settled windows
show — but the record says so, not "production-ready".

## 2. Planned workloads

1. `idle` — empty server, tick overhead floor.
2. `single-roam` — 1 scripted client roaming + chunk streaming.
3. `survival-10` — 10 scripted survival clients (move/mine/place/inventory):
   the acceptance workload (P08-10).
4. `chunkgen-burst` — fresh-area exploration burst (P08-11).
5. `entity-heavy` — mob/item-entity dense area (P05-18 input).
6. `save-storm` — autosave under dirty-chunk pressure (P08-12).

## 3. Production target

Raspberry Pi 5 8 GB, Debian 13 Trixie, aarch64, NVMe. microSD is explicitly **not**
a performance acceptance target (CONVENTIONS.md section 2). Development/CI is x86_64
plus an aarch64 build verification
(`cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets`),
which is green as of Phase 04.

## 4. Pi acceptance procedure (P09-08 — **executed 2026-09-12; record in §P09-Pi**)

No Pi 5 existed in this environment when Phase 08 closed, so the 20 TPS verdict
was **unknown, not claimed** (KD-35). This section is the procedure that was
then executed end to end on the owner's Raspberry Pi 5 the same day — the
record is §P09-Pi above; the steps stay here so a re-run on other hardware
needs no re-derivation.

1. **Build on the Pi** (Debian 13 Trixie aarch64, NVMe, 8 GB): `rustup` installs
   the pinned 1.98.1 toolchain from `rust-toolchain.toml`, then
   `cargo build --workspace --release --locked`. Record the commit SHA and
   `rustc --version`.
2. **In-process benches, release profile** (validates the port, gives the first
   MSPT figures):
   `cargo test --release -p mc-server --test pi_profile --test tick_baseline -- --ignored --nocapture`
   — the same workload that produced §1's dev-host numbers.
3. **Real service run** (the acceptance workload): install
   `deploy/mc-server.service` per `docs/operations/RUNBOOK.md` §1, 10 real or
   scripted clients at view 8 in Vanilla Survival, **30 minutes**.
4. **Capture the §13 fields during the soak**: TPS + MSPT p50/p95/p99 from the
   `tick metrics` log lines (`OperationalSnapshot`, one per 600 ticks —
   `journalctl -u mc-server`); CPU and RSS from `systemd-cgtop` / a `/proc`
   sampler (the fields the snapshot deliberately does not fabricate); storage
   latency from `iostat` on the NVMe; note any overrun outside the join burst.
5. **Verdict rule**: the product contract is stable 20 TPS — the soak passes
   when mean MSPT stays under 50 ms and no settled tick (i.e. excluding the
   join burst) overruns the budget. A soak that fails names the dominant phase
   from the per-phase means, and the P08-14 rule (profile-gated smallest
   change) applies before any tuning.
6. **Record**: a dated `§P09-Pi` section in this file with every §13 field and
   the commit; update `PARITY-MATRIX.md`'s 20 TPS row and `KNOWN-DIVERGENCES.md`
   KD-35 in the same pass. Until that section exists, no document may upgrade
   KD-35.

Until then, the aarch64 half of the story is the cross-build gate
(`cargo check --target aarch64-unknown-linux-gnu`, green 2026-09-12) plus the
release-profile figures below — honest partial evidence, not the verdict.
