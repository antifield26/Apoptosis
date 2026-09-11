# Benchmark Baseline

Owner: engineering. Method contract: AGENTS.md section 13 — every record carries
hardware, OS, toolchain, commit, build profile, workload, duration, TPS, MSPT
percentiles, CPU, RSS and (where relevant) storage and network figures.

**Status: no acceptance baseline exists yet, and none is claimed.** The Pi 5 /
10-player target is established by P08-09, P08-10, P08-11, P08-12 and P08-13.
Until then, any number below is a development-host observation for orientation
and regression detection only.

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
a performance acceptance target (AGENTS.md section 2). Development/CI is x86_64
plus an aarch64 build verification
(`cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets`),
which is green as of Phase 04.
