# ADR-0009 — World ticking model (fluids, random ticks, radius, budgets)

Date: 2026-10-01. Status: **Accepted** (P20-00).

Context: the overworld is currently inert. `TickPhase::ScheduledTicks` drains the
block scheduled-tick queue that redstone feeds (P13-01) with **no fluid
producers**, nothing samples chunk sections for growth, world time advances but
the weather does not, and `game/mod.rs` says so in as many words ("fluid
scheduled ticks … not simulated"). P20 makes the world tick. This ADR fixes
where that work sits in `PHASE_ORDER`, what it may cost, and over what radius,
**before any of it is written** — the phase's own rule, and the reason it is
first.

Clean-room (binding, `third-party.md` §2): the wire and simulation facts below
come from the 26.1.2 server jar's bytecode (`javap`), not from any reference
clone's source. The design is ours; the facts are the jar's.

## 1. What the jar actually does (verified, 2026-10-01)

`javap -p -c -classpath target/vanilla-26.1.2/server-26.1.2.jar
net.minecraft.server.level.ServerLevel`:

- `ServerLevel` owns **two independent queues**: `LevelTicks<Block> blockTicks`
  and `LevelTicks<Fluid> fluidTicks`. They are separate objects with separate
  bookkeeping — not one queue with two kinds of entry.
- `tickChunk(LevelChunk, int)` — the random-tick sweep is a per-chunk method
  taking the random-tick speed (the `randomTickSpeed` game rule), not a queue.
- Inside `ServerLevel.tick(BooleanSupplier)` (241-line body) the call order is,
  by bytecode offset:
  `tickTime()` (89) → `blockTicks.tick(…)` (110) → `fluidTicks.tick(…)` (120) →
  `ServerChunkCache.tick(…)` (139, which drives `tickChunk`) →
  `entityTickList.forEach(…)` (189/194) → `tickBlockEntities()` (199).

Three consequences are load-bearing for the design below: **fluids are scheduled
ticks, not a sweep**; **random ticks are a sweep, not a queue**; and the world's
time/weather advance happens **before** either of them.

## 2. Decision

**2.1 Two phases are added, and `PHASE_ORDER` becomes eight entries.**

| # | Phase | Work |
|---|---|---|
| 0 | `Network` | drain the inbound channel, queue intents (unchanged) |
| 1 | `ScheduledTicks` | advance world time + weather timers, then drain due **block** scheduled ticks (redstone, as today) |
| 2 | **`FluidTicks`** *(new)* | drain due **fluid** scheduled ticks |
| 3 | **`RandomTicks`** *(new)* | per-section random-tick sweep of the ticking radius |
| 4 | `Entities` | natural spawn, AI, despawn, physics (unchanged) |
| 5 | `Players` | apply queued intents, player timers/physics (unchanged) |
| 6 | `BlockEntities` | furnaces, hoppers (unchanged) |
| 7 | `Broadcast` | block changes, streaming, world time packet, removals (unchanged) |

This mirrors the jar's verified order (time → block → fluid → random → entities →
block entities) while keeping our three pipeline-specific phases (`Network`,
`Players`, `Broadcast`) where they are. It also gives the P20 exit gate what it
asks for literally — "fluid and random-tick phase costs reported separately":
each is now its own timed phase in `TickMetrics`, not a label inside another one.

**2.2 Time and weather advance in `ScheduledTicks`.** The jar puts `tickTime()`
before both queues. Ours moves world time and the weather timers at the head of
`ScheduledTicks`, which is the same slot relative to the queues. A separate
`WorldTime` phase was rejected: it would be a third new phase for an O(1)
advance, and the phase's cost would be uninteresting. `ScheduledTicks` therefore
means "the world's own clock, then the block queue"; the name survives, the doc
table gains the sentence.

**2.3 Budgets.** Three numbers, each enforced where its cost is, with **spill in
queue order rather than drops** — a queue that overflows must lose nothing, only
lateness:

- **Fluid ticks: `MAX_FLUID_TICKS_PER_TICK = 4_096`.** Due fluid updates beyond
  the cap stay queued (counted as `fluid_ticks_pending`), exactly like the
  existing redstone neighbour budget (1 024/tick, `mod.rs`), which is the
  precedent shape.
- **Random ticks: bounded by construction, not by a counter.** The sweep cost is
  `ticking_sections × randomTickSpeed`; the radius (2.4) *is* the budget, and
  `randomTickSpeed = 0` disables the work (game rule, P20-05). A counter would
  have to cut a sweep mid-section, which is not a thing the jar does.
- **Block scheduled ticks keep the existing queue budget** (1 024 neighbours /
  the redstone queue's own caps). P20 changes nothing here.

**2.4 Ticking radius.** The radius is the P19-06 `simulation_distance`
(default 8), which exists precisely to separate *ticking* from *sending* (view
distance, also default 8). Beyond it, chunks are simulated only as much as they
already were. **Named gap:** our loader loads `view_distance`, so the effective
ticking radius is `min(simulation_distance, view_distance)`; a chunk we have not
loaded cannot be ticked, and the case `simulation_distance > view_distance`
ticking only loaded chunks is a boundary this phase records rather than closes.

**2.5 Estimate (the number P20-07 checks).** At the P20-07 workload — 10 players
inside the radius, ~400 ticking chunks (radius 8 clustered; 289 chunks for one
player), 24 sections per chunk, `randomTickSpeed = 3`:

- `RandomTicks`: ~29 000 samples/tick → **≤ 1.5 ms/tick** (≈50 ns/sample, i.e. an
  early-out that rejects non-random-tickable states without touching a
  neighbour).
- `FluidTicks`: at the 4 096 cap → **≤ 0.5 ms/tick** (≈120 ns/update).
- Combined **≤ 2.0 ms/tick = 4 % of the 50 ms budget**, and **p99** — not the
  mean — is what P20-07 compares. If the sweep costs more than 1.5 ms the
  early-out is at fault and the phase must be fixed, not the estimate raised;
  the estimate is a commitment, and raising it is a new ADR revision.

**2.6 Frozen differential set (P20-01).** The five scenario names are frozen
here, before the fixture is first run, per the phase prompt: `spring_flow`,
`falling_column`, `lava_meets_water`, `waterlogged_stairs`, `bucket_place`, with
**N = 40 ticks** unless a scenario needs a jar-stated spread delay, which is
recorded per row. Compared 5 / skipped 0. Renaming a scenario later would make
"compared 5" unfalsifiable, which is why the names live in the ADR.

## 3. Interface note for P21-00a (multi-dimension runtime)

P21-00a does **not** depend on this ADR; it must *align* with it:

- **Queues are per dimension.** The jar's `ServerLevel` owns its own
  `blockTicks`/`fluidTicks`; P21 must not share one pair across dimensions, and
  the per-dimension caps are the numbers in 2.3.
- **Phase slots are shared.** One `FluidTicks` and one `RandomTicks` phase per
  tick, iterating dimensions in a fixed order (Overworld → Nether → End), so
  per-phase cost stays a single number per phase and the tick stays
  deterministic (the engineering contract §3.6).
- **Radius is per dimension.** `simulation_distance` is global config today;
  P21 decides whether it becomes per-dimension, and inherits the
  `min(simulation, view)` boundary of 2.4 for each.
- **Cost model.** P21-00a's Pi measurement should be comparable to 2.5: same
  workload shape, same p99 basis, so the two ADRs' numbers can be read together.

## 4. Pins this ADR commits the phase to

- P20-01: the five frozen scenarios, cell-by-cell against the vanilla server,
  compared 5/skipped 0; the fluid spread delays jar-sourced per row.
- P20-02: the sampling rate (`randomTickSpeed` per section) pinned to the jar's
  value; growth probability tested statistically against a **seeded** source, so
  zeroing the rate is a named red rather than a flake.
- P20-03: weather durations jar-sourced; the rain/thunder level packets observed;
  weather survives restart.
- P20-05: every game rule either wired with a behaviour test or listed inert.
- P20-07: `FluidTicks` and `RandomTicks` reported **separately** and compared to
  2.5's numbers, with the §13 record.

## 5. Consequences

- `TickPhase` grows 6 → 8: `PHASE_ORDER`, `TickPhase::index()`, `name()`, the
  metrics arrays and any test asserting the phase count or an index must be
  updated **in the same commit as P20-01's first code**, not left for later.
- `TickReport` gains fluid and random-tick counters (`fluid_ticks_fired`,
  `fluid_ticks_pending`, `random_tick_samples`, `random_ticks_applied`), in the
  style of the existing `scheduled_ticks_fired`/`_pending`.
- `game/mod.rs`'s phase table gains two rows and loses its "fluid scheduled
  ticks … not simulated" disclaimer; the "Not simulated" list shrinks to what
  remains true after P20 (fire spread, for instance).
- The tick has 8 timed buckets instead of 6; existing per-phase numbers in any
  performance record are not comparable across this change, and records made
  before it must not be compared with records made after.
- No new dependency.
