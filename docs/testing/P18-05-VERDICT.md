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
| Soak | `P18-04-SOAK.md` | **Pi NOT RUN** |

## P14-07 publication rules — checklist

| Clause | Status |
|---|---|
| Real-client walk + screens | **NOT RUN** — owner session required |
| Tag commit + artifacts + green CI | pending walk; CI run id must be recorded at tag |
| CHANGELOG / PARITY / TEST-MATRIX agree | updated in this closeout |
| No tag without walk and screens | **honoured: tag withheld** |

## Named NOT RUN (blocks an unconditional verdict)

1. Real-client walk screens (wear/break, enchant glint+tooltip, hunger, ore in cave).
2. P18-04 Pi soak (§13).
3. c2s capture corpus 56/19.
4. Selector sort/limit vanilla differential.

## Plan snapshot

`docs/planning/TASK-INDEX-v0.3.0-pre.md` (R8 interim rule).

## Owner checklist (one sitting)

1. Join 26.1.2 client to the dev server.
2. Dig with a pick until it breaks — wear and break visible.
3. Hold an enchanted item — glint + tooltip.
4. Sprint until hunger drops; eat bread; bar refills.
5. `/tp` into a carved cave; find an ore vein.
6. Record pass/fail per line in `docs/testing/P17-BUILD-SESSION.md` style.
7. If all four screens pass and the NOT RUN list is accepted, tag `v0.3.0`
   under P14-07 with the CI run id.
