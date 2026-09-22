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
   appears top-right and `effect clear` removes it (HUD sync is the
   evidence here). Movement speed does **not** change on this build
   (speed/strength application is deferred — do NOT file this as a
   finding). For visible gameplay: `effect give <name> minecraft:poison
   30` drains hearts but floors at half a heart and cannot kill.
5. **Digging + steps while you are there.** Hold left-click on stone:
   cracks grow through 10 stages; by hand it takes ~7.5s, with a pickaxe
   (`give <name> minecraft:stone_pickaxe`) ~0.6s. Walk up a slab or stair
   flight without jumping; confirm a fence still holds you.

## Verdict sheet (fill in, append the answers to this file)

- Commit SHA tested: sessions across `c7ef879`..`873740d` (round-1 base
  through the round-2 fixes; each fix verified on its deploy SHA — see
  CHANGELOG "Owner session findings round 1/2"). Final binary `873740d`,
  gate 1583/0/35/126 at fill time.
- Knockback: zombie recoils on strike — owner-felt horizontal shove on
  fist strikes (server logged damage 1.0 per swing with the knockback
  channel applied; red hurt flash on every landed hit, also owner-seen).
- XP: orbs scattered (cow 3, pig/drops likewise) / bar moved / level 1
  reached and passed; post-magnetism-fix orbs home in and pick up.
  Level-up sound NOT confirmed by owner (no claim).
- Effect: icon appeared for Speed (post raw-id fix; pre-fix the same
  command showed Slowness — see CHANGELOG round 2). Clear removal and
  the poison floor were NOT run on screen (scripted suites only).
  Systematic finding, not a pass: effect modifiers do not apply
  (Slowness II verified: correct icon, normal countdown, zero
  movement/FOV change; effect bytes field-identical to a live 26.1.2
  server) — recorded as a named gap, not a verdict failure of the
  icon sync this sheet asks for.
- Digging: NOT RUN on screen (scripted: crack stages, hand-vs-pick
  timing).
- Steps: NOT RUN on screen (scripted: slab/stair walk, fence hold).
- Anything that looked wrong (positions, sounds, icons, timing):
  everything else is in CHANGELOG "Owner session findings round 1/2"
  (attack decode, knockback channel, hurt flash, orb metadata,
  JoinGame id, effect ids + blend, placement disconnect, UseItemOn
  tail byte, Q-drop arms + throw, hotbar slot translation, revision
  lockstep, gamemode event, orb magnetism, `@s`) — each fixed,
  owner-verified, and gated.

## Known honest divergences (do NOT file these as new findings)

- The *player* is never shoved by hits (mob knockback only; the
  velocity-send path is P18 work). Zombie recoil is the knockback evidence.
- Arrows have no crit particles or pickup; creeper blasts break no blocks.
- Tools take no durability; no Efficiency/Haste/Fatigue; mid-air digs run
  at a fifth; FINISH-trusting instant breaks are refused by design.
- Mobs stop at slab lips when chasing (the one-cell lookahead is still
  name-based); players step them fine.
