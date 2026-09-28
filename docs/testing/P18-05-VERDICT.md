# P18-05 — v0.3.0 verdict (materials; tag withheld)

Date: 2026-09-23 · pre-tag closeout.

## What is evidenced (automated)

| Exit item (PHASE-18 walk) | Automated stand-in | Status |
|---|---|---|
| Pickaxe wearing and breaking | `p18_wear_enchant::{wear_matrix…, break_at_max…}` | **scripted green** |
| Enchanted item glint + tooltip | components round-trip + enchantments stored (glint is client-side) | **scripted / partial** |
| Hunger drops while sprinting, refills from bread | `p18_hunger::{sprint…, eating_bread…}` | **scripted green** |
| Ore vein inside a cave | `ore_carver_stats` (ores + carvers together) | **scripted green** |
| Gate + residuals | `P18-07-GATE-HEALTH.md` | **green with named NOT RUN** |
| Soak | `P18-04-SOAK.md` | **Pi DONE 2026-09-24** (`195a489`: settled 3.09/3.24/3.34 ms, 45 lifetime overruns all join-burst, zero new settled; scripted/loopback/microSD boundaries) |

## P14-07 publication rules — checklist

| Clause | Status |
|---|---|
| Real-client walk + screens | **NOT RUN** — owner session required |
| Tag commit + artifacts + green CI | pending walk; CI run id must be recorded at tag |
| CHANGELOG / PARITY / TEST-MATRIX agree | updated in this closeout |
| No tag without walk and screens | **honoured: tag withheld** |

## Named NOT RUN (blocks an unconditional verdict)

1. Real-client walk screens (wear/break, enchant glint+tooltip, hunger, ore in cave).
2. ~~P18-04 Pi soak~~ — **DONE 2026-09-24** on Pi 5 (`P18-04-SOAK.md`).
3. c2s capture corpus 56/19.
4. Selector sort/limit vanilla differential.

## Plan snapshot

`docs/planning/TASK-INDEX-v0.3.0-pre.md` (R8 interim rule).

## Walk round 1 (2026-09-28, Antifield, 26.1.2 client, `47eb8f7` + walk config)

1. Join — **PASS** (spawn on land, no disconnect).
2. Dig with a wooden shovel — **FAIL**: first block break disconnected the
   client on a `container_set_content` patch decode failure; the rejoin died
   the same way in the join burst (server log: outbound queue flood, then
   `player left`). Root cause: the patch wrote the removed count after the
   entries; vanilla reads both counts up front (fixed, proven against the
   26.1.2 client jar's own decoder — see CHANGELOG P18-05 walk finding).
   **Re-walk pending on the fixed binary.**
3. Enchant glint + tooltip — NOT RUN (blocked behind item 2).
4. Hunger + bread — NOT RUN (blocked behind item 2).
5. Cave ore vein — NOT RUN (blocked behind item 2; scouted targets on seed 0:
   iron `/tp Antifield 8 -22 -70`, gold `/tp Antifield -51 -58 -53`, coal
   `/tp Antifield -4 11 -45`, copper `/tp Antifield -8 59 -23`).

## Owner checklist (one sitting, round 2 = re-walk after the patch fix)

1. Join 26.1.2 client to the dev server.
2. Dig with a pick until it breaks — wear and break visible.
3. Hold an enchanted item — glint + tooltip.
4. Sprint until hunger drops; eat bread; bar refills.
5. `/tp` into a carved cave; find an ore vein.
6. Record pass/fail per line in `docs/testing/P17-BUILD-SESSION.md` style.
7. If all four screens pass and the NOT RUN list is accepted, tag `v0.3.0`
   under P14-07 with the CI run id.
