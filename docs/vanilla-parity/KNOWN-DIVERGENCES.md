# Known Divergence Catalog (P09-11)

Purpose: one place that answers "where does this server not behave like Vanilla
26.1.2, and is that a bug, a gap, or a decision?" Every entry is sourced from the
parity matrix or a phase report; nothing here is new information, and any row
that loses its source row becomes stale. Classification:

- **intentional divergence** — we do it differently on purpose, with a recorded
  reason (AGENTS.md §12 requires this class to carry an ADR/parity note);
- **gap** — vanilla behavior missing, tracked for a later phase, never silently
  substituted;
- **unverified** — implemented, but the constant/value came from recall or a
  product decision rather than a 26.1.2 measurement;
- **boundary** — a scope line the product contract draws (not a missing feature).

Divergence IDs are stable (`KD-01`…) so reports and reviews can cite them.

## Networking / protocol

| ID | Domain | Vanilla 26.1.2 | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-01 | Online mode | Mojang session server handshake | **fails fast at startup by design**; offline default. No auth provider is bundled | boundary | `PARITY-MATRIX` online-mode row; P02-08 |
| KD-02 | Signed chat | player→server signed chat participates in the signature chain | full payload is decoded and range-checked; nothing is forwarded or signature-processed | gap | `PARITY-MATRIX` signed-chat row |
| KD-03 | `container_click` trailing fields | client sends two `HashedStack` prediction fields | not decoded; the server-authoritative state id is the mechanism that matters | gap (recorded) | `PARITY-MATRIX` inventory-transactions row |
| KD-04 | Region codecs | gzip/deflate/none/lz4/custom(127) | 1/2/3 read+written; **lz4 and custom refused by name**, never misread | intentional divergence (narrower reader) | `PARITY-MATRIX` region-codecs row |
| KD-05 | External `.mcc` chunks | oversized chunks spill to `.mcc` sidecar files | explicit typed error, not a misread | intentional divergence (narrower reader) | `PARITY-MATRIX` `.mcc` row |

## Persistence / world files

| ID | Domain | Vanilla 26.1.2 | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-06 | DataVersion window | refuses newer worlds, runs datafixers for older ones | accepts `[4435, 4790]` only; older worlds refused with a clear error — **no datafixers** | intentional divergence | `PARITY-MATRIX` DataVersion-window row; ADR-0001 D-04 |
| KD-07 | Unknown `level.dat` entries | the datafixer **drops** them across load/save | preserved in `level.extra`, so our own load/save cycles are lossless | intentional divergence | `PARITY-MATRIX` unknown-level.dat row |
| KD-08 | Autosave cadence | 6000-tick default | mechanism implemented and tested; **the 6000 default is a product decision**, not a measured vanilla value | unverified | `PARITY-MATRIX` autosave row; Audit 05 |
| KD-09 | Gameplay `.dat` files (game rules, weather, world-gen settings) | written and maintained per dimension | read/round-tripped only; not authored by the server | gap | `PARITY-MATRIX` data/*.dat row |

## Simulation / gameplay

| ID | Domain | Vanilla 26.1.2 | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-10 | Collision axis order | resolves Y, then X, then Z | resolves X, then Y, then Z — observable only when one step is blocked on two axes | intentional divergence | `PARITY-MATRIX` collision row |
| KD-11 | Movement shapes | slabs/stairs/fences have partial hitboxes | full cubes only; no step-up assist, ladders or vines | gap | `PARITY-MATRIX` movement row |
| KD-12 | Redstone wire length | dust carries 15 blocks | `WIRE_LIVE_BLOCKS = 14` (first block charged an attenuation step); the 15th is dark | unverified divergence | `PARITY-MATRIX` redstone-wire row |
| KD-13 | Redstone update order | block-update vs shape-update distinction; comparator channel | one queue, documented sweep order; locational circuits may settle differently | intentional divergence | `PARITY-MATRIX` redstone-order row |
| KD-14 | Redstone component direction | torches power attachment side; comparators read side inputs | components direction-agnostic; comparators read side inputs as zero | gap | `PARITY-MATRIX` redstone-components row |
| KD-15 | Redstone integration | drives the world tick | model complete and tested, **not wired into the tick loop** | gap | `PARITY-MATRIX` redstone rows; P06-T28 |
| KD-16 | Mob spawning / AI effects | spawn rules; AI moves and attacks | no mob ever spawns; AI decisions are pure functions with no world-side effect | gap | `PARITY-MATRIX` mob rows |
| KD-17 | Mob statistics | per-kind constants | only the zombie's values wiki-checked; the rest carry per-value confidence labels | unverified | `PARITY-MATRIX` mob-statistics row |
| KD-18 | Entity persistence & sync | mobs/items survive restart; clients see entities | neither — entities are server-side only and vanish on restart | gap | `PARITY-MATRIX` entity rows |
| KD-19 | Status effects on the client | `update_mob_effect` and HUD | effects exist server-side; never inserted by gameplay and not encoded to the client | gap | `PARITY-MATRIX` status-effects row |
| KD-20 | Block entities | persist and sync to the client | no payload↔NBT conversion, no `block_entity_data`; **breaking a container with contents loses the items** (logged, reported) | gap (most serious) | `PARITY-MATRIX` block-entities row; P06-T25 |
| KD-21 | Containers | chests/furnaces/hoppers open as windows | only the player menu (window 0) can open | gap | `PARITY-MATRIX` container row |
| KD-22 | Hopper cadence | scheduled 8-gametick transfers | pure `transfer` with the cooldown left to the caller; nothing ticks a hopper | gap | `PARITY-MATRIX` hopper row |
| KD-23 | Lighting | sky/block light propagation | chunks sent with zero light masks — **clients render them dark** | gap | `PARITY-MATRIX` lighting row; `chunk-wire-format.md` §6 |
| KD-24 | Item data components | durability, enchantments, names | id + count only | gap | `PARITY-MATRIX` item-stacks row |
| KD-25 | Recipes | the full vanilla recipe set from data | 7 of 21 types modelled from the real pack; 14 types counted, not dropped; the container layer still uses the P06 hand-written table | gap | `PARITY-MATRIX` recipes row; ADR-0004 §2 |
| KD-26 | Smelting values | `fuelValues`/cooking data tables | exact burn/cook accounting, but **the values are hand-written** and labelled as recall | unverified | `PARITY-MATRIX` smelting row |
| KD-27 | Death drops | dropped stacks become item entities | stacks return on respawn; dropped items are discarded (item entities exist but nothing drops on death) | gap | `PARITY-MATRIX` death row |

## World generation

| ID | Domain | Vanilla 26.1.2 | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-28 | Noise/terrain | vanilla octave/amplitude tables, multi-noise biomes | documented Perlin implementation + six hand-labelled biomes | intentional divergence (baseline) | `PARITY-MATRIX` worldgen row |
| KD-29 | Structures | `RandomSpreadStructurePlacement` over `StructureSet` JSON; all 1 202 templates | 1 182 load; only the single-chunk subset (1 028) can generate; placement constants are approximations; **no shipwrecks** (8 alternative palettes); entity NBT counted, not spawned | gap (partial) | `PARITY-MATRIX` worldgen row |
| KD-30 | Caves/ores/features | ores, caves, ravines, lakes | none | gap | `PARITY-MATRIX` worldgen row |

## Commands

| ID | Domain | Vanilla 26.1.2 | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-31 | Command set | ~90 commands | 8 (`help list say time tp execute op stop`), each with named limits | gap | `PARITY-MATRIX` commands row |
| KD-32 | `execute` modifiers | ~20 modifiers + all conditions | 9 modifiers; unsupported ones **refused by name**; three divergences: `as @a` runs once, players-only sources, feedback to the invoker | intentional divergence within a gap | `PARITY-MATRIX` execute row |
| KD-33 | `/op` persistence | writes `ops.json` | reads the file; **cannot write a grant** — operator edits the file and restarts | gap (authority decision pending) | `PARITY-MATRIX` permissions row; P08-08 |
| KD-34 | Function features | macros `$(…)`, function tags `#ns:tag`, `/schedule` | refused by name; the jar ships zero `.mcfunction` files, so nothing here is vanilla-verified | gap | `PARITY-MATRIX` functions row |

## Operations

| ID | Domain | Production | Ours | Class | Source |
|---|---|---|---|---|---|
| KD-35 | 20 TPS verdict | Pi 5 8GB, 10 players | **met for the scripted workload** — the §4 acceptance run executed on a Pi 5 (Debian 13, release, on-device build): 30-min soak, 10 players, settled MSPT p50/p95/p99 medians 0.21/0.27/0.29 ms, zero overruns outside the join burst. Boundaries: scripted clients (KD-38), loopback traffic, microSD storage | resolved (with named boundaries) | `BENCHMARK-BASELINE.md` §P09-Pi |
| KD-36 | systemd unit | applied on a real host | **applied on the Pi 5**: unit installed per `RUNBOOK.md` §1, enabled, soak run under it, graceful stop verified on hardware. First application exposed the registry-fixture deployment defect (fixed; see §P09-Pi) | resolved | `BENCHMARK-BASELINE.md` §P09-Pi |
| KD-37 | Backup/restore CLI | operator tooling | library calls with 5 tests; no CLI wrapper | gap | P08-16 |
| KD-38 | Real-client acceptance | any Java client 26.1.x | protocol test client only; no real client in this environment | boundary (no client) | `PARITY-MATRIX` handshake row; P04-T23, P02-T12 |

## Non-divergences worth remembering

- Packet ids, region format, palette packing, NBT encodings and the 26.1
  dimension layout are **not** divergences — each is verified against the jar or
  a real vanilla world (`PARITY-MATRIX` "full" rows).
- The known-flaky list is empty as of P09: the metrics TempDir collision
  (PHASE-08-REPORT §2.1) was root-caused with a probe and fixed by per-test
  tags (PHASE-09-REPORT §2).
