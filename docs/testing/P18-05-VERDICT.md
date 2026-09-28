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

## Walk round 2 (2026-09-28, Antifield, fixed binary `b3a871c`)

1. Rejoin — **PASS** (patch fix verified: the damaged shovel syncs, no disconnect).
2. Wear/break — **PASS** (wooden shovel → grass, broke after 59 digs = max_damage).
3. Enchant — **PASS** (diamond sword → Sharpness I, glint + tooltip).
4. Hunger — **FAIL, fixed pending re-walk**: three defects, not one. (a) The
   food spend never sent vitals, so the bar sat frozen until a damage sync
   surfaced the banked spend at once ("9 on attack"). (b) `/give` bread had
   no food components, so eating silently never started. Fixed: vitals-dirty
   `SetHealth` after the session loop + in `finish_eat`; derived
   `food_defaults_of` (39 Eat foods) attached in `give_player_item`. (c) Not
   a defect: saturation absorbs first and spends in 4 s batches — short
   sprints correctly move no visible bar. See CHANGELOG round-2 entry.
5. Cave ore — **PARTIAL**: veins generate as scouted, but cave lighting looks
   wrong (details + screenshot pending from owner).
6. Extra owner findings (carried with IDs, not fixed in this pass):
   - P18-05-C1 cave lighting looks wrong (veins generate correctly).
     Needed: screenshot + too-dark vs light-leak + which cave. Candidates:
     static-model limits (KD-23) vs carve-time staleness from the new live
     wiring. Not reproduced locally yet.
   - P18-05-C2 block breaking probabilistically fails (shovel → grass).
     Needed: exact symptom (block reappears? no drops? how often?) + a
     minimal repro. Untouched P16-05 path so far.
   - P18-05-C3 spider behavior + post-kill model persists (XP orb drops).
     Read-only triage: mob death sets `removed = true` and scatters XP
     (`damage_entity`), so the kill path runs — the remove broadcast is
     suspect, unverified. `CaveSpider` is not a modeled kind (only
     `Spider`); if the sighted spider never moved at all, that needs its
     own repro. Both carried.

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
