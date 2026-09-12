# System Overview (Phase 00)

Target: pure-Rust Minecraft Java 26.1.2 dedicated server, 10-player Vanilla Survival,
fixed 20 TPS, Tokio I/O, Pi 5 aarch64 first-class. See ADR-0001 for decisions.

```text
                    +------------------- clients (real 26.1.2) -------------------+
                    | handshake/status/login/config/play, compression, keepalive |
                    +------------------------------+----------------------------+
                                                   | TCP (hostile input)
                                     +-------------v-------------+
                                     | network (Tokio edge)      |  P02
                                     | accept/framing fast-path  |
                                     +-------------+-------------+
                                                   | validated intents (bounded queue)
                                     +-------------v-------------+
                                     | simulation (tick thread)  |  P01-09 clock, P05
                                     | 50ms phases, serial order |
                                     +--+------+------+------+---+
                       +----------------+ +----v---+ +----v---+ +-----------v--...--+
                       | world/dimens.  | | entity | | command| | persistence     |  P03/P04/P05/P07
                       | chunks/blocks  | | AI/phys| |dispatch| | anvil/nbt/level |
                       +----------------+ +--------+ +--------+ +--------+--------+
                                                                    | tmp→rename, barrier
                                                              +-----v-----+
                                                              | disk: Anvil |
                                                              | level.dat   |
                                                              +-------------+
```

Data flow (CONVENTIONS.md §8): `TCP/Tokio → decoded events → tick scheduler → 20 TPS sim → state changes → packet scheduler → socket`.
Workers (chunkgen/compression/persistence) join at tick boundary; gameplay order never depends on I/O completion.

Crate map: `docs/adr/ADR-0001-system-architecture.md` §D-01 (Phase 03 refined it
in `ADR-0002`: `mc-nbt` is shared by `mc-protocol` and `mc-persistence`, and
`ChunkData` is the schema boundary that `world` will convert to/from).
Protocol facts: `docs/research/protocol-baseline.md`. Parity/test strategy:
`docs/research/parity-and-testing-strategy.md`. Risks: ADR-0001 §2.

Phase 03 status (persistence; the Phase 03 report is in git history, tag `phase-09-final`): Anvil region
read/write, `level.dat` (DataVersion 4790 / level version 19133), the 26.1
`dimensions/<ns>/<value>` layout with legacy fallback, chunk serialization,
dirty tracking, autosave and the ordered/atomic save barrier are implemented and
verified — including a differential run where a real vanilla 26.1.2 server booted
on a world this code wrote and re-saved it.

Phase 05 status (simulation; Phase 05 report in git history, tag `phase-09-final`): a new
`mc-simulation` crate owns the **shape of a tick** — a `const` six-phase order
(network → scheduled ticks → entities → players → block entities → broadcast),
per-phase timing, and a seeded `RandomSource` verified byte-for-byte against
`java.util.Random`. `mc-entity` gained the entity store (monotonic ids never
reused), status effects, dropped items, a projectile trajectory baseline, and mobs
with goal-based AI plus bounded A* pathfinding. The game loop now runs through the
scheduler and owns the open world, so a stored chunk is read before a placeholder
can exist (a Phase 04 defect that silently destroyed terrain; see
Audit 02, git history tag `phase-09-final`). Not implemented, and named as such: mob
spawning, entity packets, entity persistence, scheduled block/fluid ticks, and any
world-side effect of an AI decision.

Phase 04 status (survival slice; Phase 04 report in git history, tag `phase-09-final`): `mc-registry`
(29 873 block states / 1 506 items, from the official jar's own registry),
`mc-world` (runtime chunks, swept collision, ray casting) and `mc-entity` (player,
inventory, item stacks, health/food/XP) exist; `mc-network` publishes joins and
intents across a bounded bridge into `mc-server`'s game loop, which streams
terrain, validates movement and block edits, applies fall damage, and saves the
world. Verified end to end over a **real TCP socket** and against a **real vanilla
chunk**. World generation, lighting, entities and commands are explicitly not
implemented.

