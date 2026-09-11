# Benchmark Baseline

Owner: engineering. Method contract: AGENTS.md section 13 — every record carries
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
