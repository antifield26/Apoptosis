# ADR-0007 — Post-P12 Architecture Deltas (amends ADR-0001/0002/0004/0006)

Date: 2026-09-16. Status: **Accepted** (Phase 12 close-out, AUDIT-12).

Context: ADR-0001 sketched 12 crates, a Rayon-style parallel tick
(`net-drain → tasks → worlds → entities → block-entities → packet-flush`), and a
persistence/forum model that P10–P12 outgrew without any amending record: four new
crates plus a second binary, a serial tick with new names, entity/block-entity
persistence, pack-to-table conversions, and a shipped release candidate. The
AUDIT-12 architecture lane found each delta by diffing the ADRs against the tree.
This record amends rather than rewrites: the accepted files stay byte-identical,
and this file carries what changed, why, and where the behaviour lives now.

## 1. Crate boundaries grown (amends ADR-0001 D-01, ADR-0003 dependency order)

The tree holds 16 crates plus two binaries. Beyond ADR-0001's twelve:

| Boundary | Owns | Justification (not scaffolding) |
|---|---|---|
| `mc-container` | menus, click transactions, crafting/furnace/hopper tables, block entities | P06-01..P12-09: the conservation-critical transaction core, tested in isolation (132 lib tests) precisely because nothing here reaches outward |
| `mc-redstone` | power model, budgeted propagation, update queue | P06-09..16 golden/differential model; nothing depends on it yet (P13 wires it) |
| `mc-worldgen` | seeds, noise, biomes, terrain, features, structures, existing-world-first | P07-13..17 pipeline with seed determinism goldens |
| `mc-data` | pack loading (tags, recipes, loot, advancements, functions) | ADR-0004 already justifies the split; P07–P12 made it the crafting/smelting/loot authority |
| `apps/capture-rig` | byte-for-byte client proxy + normalized JSONL traces | P10-01 differential-testing-against-clients instrument; no production dependency |

`mc-container → mc-data` is the one dependency edge ADR-0003's order omits
(`container/Cargo.toml` also uses `mc-entity`, `mc-registry`).

## 2. Tick model is serial with new phase names (amends ADR-0001 D-02)

No Rayon pool, no `par_chunks`, no `block_in_place`/affinity ever landed. The tick
thread owns `Game` outright (`game.rs:39-43`); phases run serially through
`mc-simulation::PHASE_ORDER`: `Network → ScheduledTicks → Entities → Players →
BlockEntities → Broadcast` (`phase.rs:35-42`). `ScheduledTicks` is still a
documented no-op (P13's first task); `BlockEntities` ticks furnaces and hoppers
since P12-03/04. Correctness before optimization (§3.2): no benchmark ever
justified the parallel design, so it stays a sketch.

## 3. Persistence extended to entities (amends ADR-0001 D-04, ADR-0002)

D-04's hot-chunk/region-grouping/snapshot-clear/dual-layout and the `[4435, 4790]`
window still hold, and ADR-0002's `ChunkData` schema boundary still holds. Added:
`Game::serialize/load_chunk_entities` (P11-08) and
`Game::serialize/load_chunk_block_entities` (P12-05) ride `queue_dirty_chunks`,
so dirty chunks save terrain **plus** live entities and block entities. The
P08-03 "dedicated save worker" forecast (`storage.rs:14-17`, ADR-0002) never
landed and stays future work.

## 4. Data loading landed (supersedes ADR-0004 §"NOT loaded")

ADR-0004 recorded loot/advancements/functions/predicates/worldgen/item-modifiers
as not loaded. Now loaded and consumed: loot tables fire as the drop authority
(P11-04), `RecipeBook` converts to the crafting table for item-only
shaped/shapeless (P12-07) and to the furnace table for smelting rows (P12-08),
functions/structures/tags as before. Still not loaded: tag resolution for
gameplay (tag recipe ingredients are counted and skipped), advancements effects,
predicates, item modifiers. Fuel values stay the jar-verified baseline by design.

## 5. Release artifacts exist (addendum to ADR-0006)

ADR-0006's "no tagged binary release yet" is spent: `v0.1.0-rc.1` ships three
assets via `.github/workflows/release.yml`, recorded in
`docs/release/RELEASE-CANDIDATE.md` (frozen at the P09–10 line by design) and
`docs/GOVERNANCE-REPORT.md` (2026-09-12 snapshot; see its header note for what
came after). No license change: MIT throughout.

## 6. What this ADR deliberately does not decide

Redstone wiring (P13), the 0.2.0 milestone (P14), tag resolution, double-chest
merge, and the remaining AUDIT-12 open findings keep their own rows in
`PARITY-MATRIX.md` and their own phase tasks in the agent task index.
