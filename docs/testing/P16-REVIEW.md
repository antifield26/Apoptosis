# P16 review — Combat & the Survival Loop (agent half, 2026-09-21)

Scope per the task index (P16-01..P16-07; TASK-INDEX lives in the
operator's prompt pack, which is gitignored and therefore uncitable here).
P16-01..P16-06 are landed below; P16-07's automated half is this file and
the runbook; its real-client half is pending the owner (see Verdict below).

## Landed work and where it is proved

- **P16-01 damage model** — `mc-entity::combat` (sources with armour-bypass
  and knockback flags, armour absorb, weapon/armour tables, reach hook),
  threaded through swings, mob melee, falls and starvation. Probes:
  armour-bypass matrix, knockback facing/away unit tests, sword/shove/
  iron-suit integrations.
- **P16-02 XP orbs** — `EntityBody::Orb`, split bands, 6000-tick life,
  player-kill scatter, capped player-death scatter, contact pickup with the
  2-tick throttle into the three-branch level map, `SetExperience` on
  pickup, merge, despawn, `AddEntity` + value slot, restart persistence.
- **P16-03 status effects** — poison/wither/regen ticking on players and
  mobs with floor/bypass rules, `/effect give|clear` (operator, self-only,
  modelled names), `update_mob_effect` (132) / `remove_mob_effect` (78) on
  give/join-sync/expiry, `playerdata` persistence.
- **P16-04 mob AI** — per-kind follow ranges (zombie 35.0, closing AUDIT-09
  D-04), `World::has_line_of_sight`, A* chase with direct fallback,
  skeleton bow shots (15 blocks, LOS-gated, difficulty-scaled) through
  `DamageSource::Arrow`, creeper 30-tick fuse with flash metadata through
  `DamageSource::Explosion` (no block damage, suicide drops nothing).
- **P16-05 digging** — four extracted fixtures (hardness, tool
  requirement, mineable/efficiency tags, incorrect-tier sets, per-item
  Tool rules), `speed / hardness / divisor` evaluation with the mid-air
  fifth, START/ABORT/FINISH state machine with per-tick accumulation,
  crack overlays, harvest-gated drops, creative instant intact.
- **P16-06 shapes + step-up** — `block_shapes.tsv` (asset state i == our
  state first+i, verified 1168/1168 three ways), per-state collision in
  the mover, 0.6 auto-step for living movers with wall/ceiling/headroom
  rejection, non-living movers unchanged.

## Falsification inventory (each fails on revert, verified)

Sword scatter vs fall-kill silence; effect give/refresh rules; zombie
35.0 vs 16.0 radii; frozen dig progress; no-op abort; neutered step-up;
no-step `move_player`; removed LOS gate (fires through the wall);
zeroed arrow damage; neutered creeper detonation.

## Found during P16 (fixed, with the mechanism)

- Creeper fuse packets spammed every tick for unlit creepers (announce on
  transitions only) — caught by `natural_spawn` counting 652 datas.
- Two bow-wall tests passed vacuously (entombed skeleton, then a
  walk-around): plant entombment fixed *by* shapes; the test rewritten
  hermetic (floor + corridor) with a point-blank positive control.
- Hand-mined stone dropped cobblestone (harvest gate); bedrock refused by
  name only (now by value); projectile announce typed arrows.

## Deliberately deferred (named, not hidden)

Enchantment combat math, absorption, fire/drowning/void/magic sources,
player knockback velocity-send, per-item reach, arrow criticals/pickup,
charged creepers, blast block damage and exposure fractions, Efficiency/
Haste/Fatigue, water mining penalty, tool durability, block-specific
placement rules, mob-lookahead shape awareness, ladders/vines, natural
effect sources, milk, speed/strength application, mob-effect chunk
persistence, potions, shields/parry, difficulty-scaled melee damage.

## Gate and verdict

Automated gate at review time: **1537 passed, 0 failed, 35 ignored, 119
suites** (`python tools/gates/run.py --quick`), fmt + clippy `-D warnings`
+ docs audit clean. Real-client night fight (runbook:
`docs/testing/P16-NIGHT-FIGHT.md`): **NOT RUN — owner verdict pending**.
P17 work may proceed on the automated evidence; the P16 exit gate closes
when the verdict sheet in the runbook is filled.
