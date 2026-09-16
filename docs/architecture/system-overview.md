# System Overview

Target: a from-scratch, pure-Rust Minecraft Java **26.1.2** dedicated server (protocol **775**), serving
10 concurrent Vanilla-Survival players at a fixed 20 TPS, with Tokio only at the I/O edges and a
Raspberry Pi 5 (aarch64, Debian 13) as the first-class deployment target. Architecture decisions and the
risk register: [ADR-0001](../adr/ADR-0001-system-architecture.md).

**This document describes what the system is now.** For how it came to be, see
[CHANGELOG.md](../../CHANGELOG.md); the per-phase reports it distils are in git history at tag
`phase-09-final`.

```text
                    +------------------- clients (real 26.1.2) -------------------+
                    | handshake/status/login/config/play, compression, keepalive |
                    +------------------------------+----------------------------+
                                                   | TCP (hostile input)
                                     +-------------v-------------+
                                     | network (Tokio edge)      |  mc-network
                                     | accept/framing fast-path  |
                                     +-------------+-------------+
                                                   | validated intents (bounded queue)
                                     +-------------v-------------+
                                     | simulation (tick thread)  |  mc-simulation
                                     | 50ms phases, serial order |
                                     +--+------+------+------+---+
                       +----------------+ +----v---+ +----v---+ +-----------v--...--+
                       | world/dimens.  | | entity | | command| | persistence     |  mc-world
                       | chunks/blocks  | | AI/phys| |dispatch| | anvil/nbt/level |  mc-entity
                       +----------------+ +--------+ +--------+ +--------+--------+  mc-command
                                                                    | tmp→rename,   mc-persistence
                                                                    | barrier
                                                              +-----v-----+
                                                              | disk: Anvil |
                                                              | level.dat   |
                                                              +-------------+
```

Data flow ([CONVENTIONS.md](../CONVENTIONS.md) §8):
`TCP/Tokio → decoded events → tick scheduler → 20 TPS sim → state changes → packet scheduler → socket`.
Workers (chunkgen, compression, persistence) join at a tick boundary; gameplay order never depends on I/O
completion. Connection tasks handle keepalive/ping/client-information inline; only play intents are
tick-queued, and every connection shares one bounded inbound queue (`lifecycle::EVENT_QUEUE`, 1024).

A tick runs six phases in serial order (`mc-simulation::PHASE_ORDER`): `Network → ScheduledTicks →
Entities → Players → BlockEntities → Broadcast`. `ScheduledTicks` is still a documented no-op (P13's
first task); `BlockEntities` ticks furnaces and hoppers since P12-03/04. Each player holds one open
window: `window 0` is the player inventory, `1..=127` a chest/furnace/hopper menu whose block half
flushes into the chunk's block entity; the cursor rides `set_cursor_item` and closes restore the
player menu. Chunk saves carry both `entities` and `block_entities` NBT (P11-08/P12-05).

## Crate map

Sixteen crates plus two binaries. Each boundary exists for a stated reason; the ones that were contested are
recorded as decisions rather than left implicit.

| Crate | Owns |
|---|---|
| `mc-core` | Error taxonomy, resource ids, the deterministic tick clock |
| `mc-nbt` | The NBT model and both encodings (big-endian network, gzip'd on disk) |
| `mc-protocol` | The 775 wire codec: VarInt/VarLong, framing, and every packet the server sends or reads |
| `mc-network` | Tokio listener and connection lifecycle; the offline/online authentication boundary; admission limits |
| `mc-registry` | Block-state and item tables read from the jar-derived fixtures, with the documented search order |
| `mc-world` | Runtime chunks, swept collision, ray casting |
| `mc-persistence` | Anvil regions, `level.dat`, palette packing, dirty tracking, autosave and the atomic save barrier |
| `mc-simulation` | The shape of a tick: a `const` six-phase order, per-phase timing, and a seeded `RandomSource` |
| `mc-entity` | Entity store, players, inventories and item stacks, mobs with goal-based AI, bounded A*, effects, projectiles |
| `mc-container` | Menus and server-authoritative click transactions, crafting, furnace, hopper, block entities |
| `mc-redstone` | The power model, budgeted propagation, the update queue |
| `mc-worldgen` | Seeds, noise, biomes, terrain, features, structures, and existing-world-first generation |
| `mc-data` | Tags, recipes, loot tables, advancements, functions, pack discovery — the loader, separate from the registry ([ADR-0004](../adr/ADR-0004-data-loading-and-registry-split.md)) |
| `mc-command` | The command tree, dispatcher, argument types, selectors and the `execute` context |
| `mc-server` | Lifecycle and the game loop, config, logging, operational metrics, storage, data packs, ops, backup |
| `mc-test-support` | Fixtures, temp directories, the protocol `TestClient` |
| `apps/server` | The `mc-server` binary: read config, init logging, run until shutdown |
| `apps/capture-rig` | The `capture-rig` proxy: relay a real client byte-for-byte while writing normalized JSONL traces |

Two boundaries are worth naming because they are easy to get wrong and are recorded as decisions:
`mc-nbt` is shared by `mc-protocol` and `mc-persistence`, with `ChunkData` as the schema boundary
([ADR-0002](../adr/ADR-0002-nbt-and-persistence-boundary.md)); and simulation uses enums and one geometry
type rather than trait objects ([ADR-0003](../adr/ADR-0003-simulation-layering.md)). Plugin readiness is a
boundary with three named seams and **zero API types** — deliberately not an API
([ADR-0005](../adr/ADR-0005-plugin-boundary.md)). The project is MIT
([ADR-0006](../adr/ADR-0006-licensing.md)).

## What runs today

Every claim below has a test or a measurement behind it; the per-domain evidence and every known
divergence live in [PARITY-MATRIX.md](../vanilla-parity/PARITY-MATRIX.md), which is the single authority.
`README.md` carries the same list for a reader who has not opened `docs/`.

| Domain | State |
|---|---|
| Protocol & networking | handshake → status → login → config → play over Tokio TCP; packet ids verified against the jar's registration bytecode; hostile-input hardening (non-terminating varints, oversized frames, decompression bombs, slow drips, floods); per-IP and global admission limits |
| Survival slice | join/stream, server-authoritative movement with swept collision, break/place validation, inventory transactions (player + chest/furnace/hopper windows), player crafting from the pack table, health/hunger/XP, death and respawn, save/reload |
| Persistence | NBT + Anvil read/write with atomic saves, carrying live entities and block entities (P11-08/P12-05); verified end to end — a world this code rewrote was **booted on a real vanilla 26.1.2 server**, which preserved the edits and re-saved every dimension |
| Commands & data | 8 commands plus `/function`, permission levels from `ops.json`; real data-pack loading from the configured vanilla pack and from world packs; the deployed vanilla pack resolves 758 tags and loads 1 421 recipes; loot tables fire as the drop authority and furnace/crafting tables convert from the pack (fuel stays the jar-verified baseline) |
| World generation | seeded terrain with six biomes, trees, and a single-chunk subset of the jar's 1 202 structure templates |
| Performance | the documented 20 TPS acceptance procedure ran on a Raspberry Pi 5: a 30-minute soak with 10 scripted clients held settled tick p50/p95/p99 medians of 0.21/0.27/0.29 ms with zero settled overruns ([record](../performance/BENCHMARK-BASELINE.md)); that soak predates the P12 furnace/hopper tick work, so re-soak is owed before the verdict covers it |

## What is deliberately absent

Recorded rather than papered over; the full catalogue with per-row evidence is the parity matrix.

- **Static lighting only.** Sky/block light is computed from jar-measured tables and owner-confirmed on a
  real client; there is no day/night dimming and no incremental relight beyond block changes.
- **Entities sync and persist, within limits.** Mobs spawn, walk, hit back, drop loot, and ride the chunk
  save with drops (P11); viewers see moves, hurt and death. Gaps: per-kind follow ranges, XP orbs,
  pathfinding, and entities in otherwise-clean chunks.
- **Redstone is a tested model, not wired into the tick loop.** Propagation, budgets and determinism have
  tests; `TickPhase::ScheduledTicks` is still a no-op (P13's first task).
- **8 of roughly 90 Vanilla commands.** Chests, furnaces and hoppers open as windows a client can
  transact with; double chests open single, tag recipes never convert, hopper↔furnace routing is skipped.
- **Partial real-client acceptance.** A Java 26.1.2 client has joined, entered play, rendered night/mobs/
  drops and chatted (P10-11/P11); container, death→respawn and restart screens are still unverified.
- **Offline mode only.** `online_mode = true` refuses to start rather than degrading silently.

## Invariants a change must not break

- **A stored chunk is read before a placeholder can exist.** Generation is gated on being able to read
  storage, so a game that cannot look cannot generate over a saved world.
- **Generated chunks stay clean.** They are reproducible from the seed, so marking them dirty broke
  unloading and let placeholders be written back over real terrain.
- **The world is only replaced by `rename`.** `level.dat` must never be absent, so the save barrier writes a
  staged file, fsyncs, and renames — the replace is one atomic step.
- **Gameplay order never depends on I/O completion.** Workers join at a tick boundary.
- **Collections that a tick walks are ordered**, and all randomness comes from `mc-simulation`'s seeded
  `RandomSource`, so a replay is reproducible.
