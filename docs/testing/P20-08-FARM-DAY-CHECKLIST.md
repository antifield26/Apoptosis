# P20-08 "Farm Day" — owner real-client checklist (NOT RUN)

Status: **NOT RUN**. This is the scripted single-sitting session the P20-08
task requires (one checklist, one sitting): every item below is checkable on
a real Java 26.1.2 client in one sitting. Nothing here is claimed until the
boxes are ticked with a build id and date.

Setup: release build of this tree, fresh world (`seed` unset), default
config plus `online_mode = false`, one operator (`/op`), survival mode,
daytime, weather clear. Keep the server log: any disconnect or error line is
part of the verdict.

## 1. Pour water that flows and stops

- [ ] NOT RUN — Place a water bucket on flat ground. Water spreads level by
      level and stops. No source blocks appear mid-flow (only the placed
      cell is a source). Expected: matches the `spring_flow` differential.
- [ ] NOT RUN — Dig the poured water's source with a bucket (fill by
      right-clicking the source). The flow drains back to air.

## 2. Bucket lava onto water for obsidian

- [ ] NOT RUN — Pour lava so it meets standing water. The contact cell turns
      to obsidian (source lava) or cobblestone/stone (flowing lava),
      exactly like the `lava_meets_water` differential.
- [ ] NOT RUN — Mine the obsidian with a diamond pickaxe (slow, many hits);
      it drops obsidian.

## 3. Till, plant and harvest grown wheat

- [ ] NOT RUN — Hoe dirt to farmland, plant wheat seeds, watch it grow
      through all stages to golden wheat (in-game time, no commands).
      Growth is slow at `random_tick_speed 3`: this is an afternoon check,
      or `/gamerule random_tick_speed 30` first and note the override.
- [ ] NOT RUN — Harvest by hand: wheat + 0–3 seeds drop and pick up.
- [ ] NOT RUN — Break a wheat block mid-growth with bone meal in hand:
      right-click grows it exactly one stage per meal.

## 4. Grow a tree and watch leaves rot

- [ ] NOT RUN — Plant an oak sapling on dirt in the open. It advances to a
      two-stage sapling, then grows a trunk with a canopy.
- [ ] NOT RUN — Chop the whole trunk. Leaves with no log nearby decay and
      drop saplings/sticks; leaves you placed by hand never decay.

## 5. Breed two cows and milk one

- [ ] NOT RUN — Feed wheat to two adult cows (hearts would show; there is
      no particle channel, so watch for the calf instead). A calf appears
      between them at half size and grows over ~20 minutes.
- [ ] NOT RUN — Right-click an adult cow with a bucket: milk. Drink the
      milk with a status effect active (e.g. poison from `/effect`): the
      effect clears and a bucket stays in hand.
- [ ] NOT RUN — Shear a sheep: 1–3 wool. Wait on grass: the wool grows back
      and a second shearing works.

## 6. Sleep through a rainy night

- [ ] NOT RUN — `/weather thunder`, wait for lightning: strikes flash
      (bolt entity), thunder cracks silently (no sound channel), and anything
      struck takes 5 damage. No fires start (no fire model — confirm none).
- [ ] NOT RUN — At night, right-click a bed with both halves free and no
      monsters within 8 blocks: sleep through to a clear morning. A second
      account (or a second client) staying awake must block the skip at
      default `players_sleeping_percentage 100`.
- [ ] NOT RUN — Die at night far from the bed: respawn lands on the bed
      with the "missing or obstructed" fallback only when the bed is gone.

## 7. Rules round trip

- [ ] NOT RUN — `/gamerule keep_inventory true`, die with items: inventory
      and bar survive, nothing drops. Set back to `false`, die again: items
      scatter.
- [ ] NOT RUN — Stop the server, restart, rejoin: weather, rules, bed
      spawn, calf age and sheep shear state are all as left.

## Verdict section (fill on the run)

- Build commit: —
- Date / operator: —
- Client version: —
- Pass / fail per item above: —
- Log anomalies (disconnects, error lines): —
