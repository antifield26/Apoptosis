# P16-07 night fight — owner runbook (real Java 26.1.2 client)

Exit-gate evidence for P16 that no automated suite can produce: knockback
*felt*, XP *gained*, an effect *on screen*.

## Setup (5 minutes)

1. Build and start the server from this tree (`cargo run --release -p
   mc-server-app` or the Pi deployment), note the commit SHA below.
2. Connect a vanilla 26.1.2 client in Survival, offline mode.
3. Become operator from the server console: `op <name>` (effect/give need
   it; the game refuses cross-player targeting, so run everything as
   yourself).
4. `gamemode survival` (if not already), `time set night`, `difficulty
   normal`.

## The fight (10–15 minutes)

1. **Wait for the dark to fill.** Hostiles spawn on light-0 ground at
   least 24 blocks out and walk in (zombies notice at 35). Give it up to
   two minutes; if nothing comes, `time set midnight` (deepest dark) and
   walk away from torches and lit ground.
2. **Knockback.** Give yourself an iron sword (`give <name>
   minecraft:iron_sword`), let a zombie close, and strike it. Look for
   the recoil: the mob visibly shoves backwards ~0.4 on every landed hit
   (the 10-tick hurt window paces multi-hit shove chains).
3. **XP.** Kill the zombie with the sword. Green orbs scatter; walk over
   them. The XP bar fills and the level counter clicks up with the
   level-up sound (7 points bank a full level-1 from empty).
4. **Effect.** `effect give <name> minecraft:speed 60` — the swirl icon
   appears top-right and movement is visibly faster. Then `effect clear`
   and watch the icon leave. (Poison also shows hearts if you prefer
   drama; it floors at half a heart and cannot kill.)
5. **Digging + steps while you are there.** Hold left-click on stone:
   cracks grow through 10 stages; by hand it takes ~7.5s, with a pickaxe
   (`give <name> minecraft:stone_pickaxe`) ~0.6s. Walk up a slab or stair
   flight without jumping; confirm a fence still holds you.

## Verdict sheet (fill in, append the answers to this file)

- Commit SHA tested:
- Knockback: zombie recoils on strike / hardly noticeable / absent —
- XP: orbs scattered (count?) / bar moved / level+sound on ___ points —
- Effect: icon appeared for ___ / movement change felt / clear removed it —
- Digging: crack stages seen 0-9 / break time felt right for hand vs pick —
- Steps: slab/stair flight walked without jumping / fence held —
- Anything that looked wrong (positions, sounds, icons, timing):

## Known honest divergences (do NOT file these as new findings)

- The *player* is never shoved by hits (mob knockback only; the
  velocity-send path is P18 work). Zombie recoil is the knockback evidence.
- Arrows have no crit particles or pickup; creeper blasts break no blocks.
- Tools take no durability; no Efficiency/Haste/Fatigue; mid-air digs run
  at a fifth; FINISH-trusting instant breaks are refused by design.
- Mobs stop at slab lips when chasing (the one-cell lookahead is still
  name-based); players step them fine.
