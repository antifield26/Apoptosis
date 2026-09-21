# P17 build session — owner runbook (real Java 26.1.2 client)

Closes the P17-02 container debt (P12-10) and the P17-05 build gate with
eyes-on evidence no suite can produce: a double chest transacted, a
hopper feeding a furnace, a crafting table crafting, an observer clock
ticking, and a chest rendering correctly after a restart.

## Setup (5 minutes)

1. Build and start the server from this tree (`cargo run --release -p
   mc-server-app`), note the commit SHA below.
2. **Port:** set `bind` to `127.0.0.1:25567` (the verification-walk
   port). Never use 25565 (production Java) or the Pi's 25599.
3. Connect a vanilla 26.1.2 client in Survival, offline mode.
4. From the server console: `op <name>`, then in game `gamemode
   survival`, `time set day`, `difficulty normal`.
5. Clear a flat workspace (creative-style staging is fine — switch back
   to survival for every check below).

## Session A — mechanisms (P17-01, 10 minutes)

1. **Doors.** Place two adjacent oak doors, right-click one: both halves
   swing. Iron door: right-click does nothing (redstone-only). Trapdoor
   on the floor edge, fence gate facing you — they toggle by hand.
2. **Observer clock.** Put an observer facing a lever (or a door), dust
   off its back. Flip the lever: the dust flashes for ~2 ticks then goes
   dark. Flip twice more: one pulse per flip, never stuck on.
3. **Dispenser.** Open a dispenser (9 slots), load cobblestone, power it
   with a lever: exactly one block pops out per flip-off-flip-on. Held
   power fires once, not a stream.

## Session B — containers (P17-02, 15 minutes)

4. **Double chest.** Place two adjacent chests: one 54-slot "Large Chest"
   window opens ("Large Copper Chest" for copper). Shift-click a stack
   in: it lands in the top half. Close, reopen from the other half:
   everything is where you left it, in both halves.
5. **Hopper feeds furnace.** Hopper above a furnace loaded with cobble, a
   side hopper loaded with coal, an empty hopper below. Within ~20
   seconds the furnace lights, smelts, and stone starts collecting below
   — hands off after loading.
6. **Crafting table.** Right-click opens the 3×3 ("Crafting Table").
   Two planks in a column: 4 sticks appear; take them. Three planks in
   the top row: 6 slabs (impossible 2-wide — proves the big grid).
   Leave planks in the grid and close: they come back to your inventory.
7. **Barrel.** Opens with the "Barrel" title (not "Chest").

## Session C — chest post-restart render (5 minutes + a restart)

8. Put a distinctive item (e.g. dirt) in a single chest, note its exact
   position. Stop the server cleanly (wait for the save flush), start it
   again, rejoin.
9. Walk to the chest. Classify what you see:
   - **clean**: renders exactly like before, opens with its items; or
   - **broken**: invisible/collision-only/wrong texture/position jitter —
     describe precisely which, and whether opening still works.
10. No server errors are expected in either case; copy any log lines that
    mention the chunk or block entity into the verdict.

## Verdict sheet (fill in, append the answers to this file)

- Commit SHA tested:
- Doors: both halves swing / iron refuses / trapdoor+gate toggle —
- Observer: one pulse per flip / never stuck —
- Dispenser: one item per rising edge / no stream on held power —
- Double chest: 54 slots transact / both halves persist across close+reopen —
- Hopper→furnace: lit without touching after loading / stone collected —
- Crafting: sticks + slabs craft / take consumes / leftovers return —
- Barrel title: "Barrel" —
- Chest restart render: clean / broken (describe) —
- Server log anomalies (paste):
- Anything else that looked wrong:

## Known honest divergences (do NOT file these as new findings)

- No pistons, no rails, no note blocks, no TNT (P17-01 deferred with reasons).
- Chests always join adjacently (no sneak-to-place-single); all placed
  chests face north (placement orientation is first-state).
- Copper joins same-variant copper only; trapped chests show "Chest".
- Hoppers never lock under power (`enabled` unmodelled); no furnace XP on
  hopper extract; hopper placement faces down.
- Shift-click on the crafting result crafts once per click (same totals,
  more clicks); drag-crafting across the 3×3 is untested on screen.
- Dispensers drop every item as a ground entity (no arrow shots, no fluid
  placement); observer facing/output-side question rides the P17-04
  differential, not this session.
