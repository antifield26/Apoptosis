# Fluid differential tooling (P20-01)

The vanilla side of the fluid acceptance rig. Two scripts, no library:

| Tool | What it does |
|---|---|
| `capture.py` | Runs the **official 26.1.2 server jar** on each frozen scenario and dumps the resulting chunk cells. The baselines under `crates/test-support/fixtures/fluids/` are its output and nothing else. |
| `dump_diff.py` | Prints where two dumps differ, cell by cell — used to check what a scenario actually did in vanilla, and whether two capture runs agree. |

The Rust half of the rig is `crates/server/tests/fluid_differential.rs`, which builds the
same world from the same spec, ticks it the same number of ticks, and compares cell by cell.

## Running a capture

```powershell
python tools/fluid-differential/capture.py                 # all five scenarios
python tools/fluid-differential/capture.py --scenario spring_flow
python tools/fluid-differential/capture.py --repro         # capture each one twice
```

It needs the real server jar at `target/vanilla-26.1.2/server-26.1.2.jar` (the operator
supplies it; it is not committed) plus its libraries, which the bundler extracted to
`target/vanilla-26.1.2/vanilla-world-26.1.2/libraries/`, and a `java` on `PATH`. The jar's
sha1 is pinned in the spec, and the capture refuses to run against a different jar.

`--repro` captures every scenario a second time, in a freshly generated world, and reports
how many cells differ; the committed dumps stay the first run. Every row of the committed
set has been checked this way (0 cells differ), which matters because lava's
`getSpreadDelay` has a random branch.

## What it does per scenario

1. Generates a superflat world (one bedrock layer, no structures) in
   `target/fluid-differential/server-home/<scenario>-<run>/`, force-loads chunk (0, 0),
   freezes the game with `/tick freeze` and applies the setup — frame first, then the
   scenario's own operations — from `scenarios.toml`. **A rejected command fails the run**,
   so a stale game-rule name or a typo cannot leave a default in place and produce a
   baseline for a world the spec does not describe. (It caught exactly that: 26.1.2 renamed
   the rules to `random_tick_speed`, `spawn_mobs`, `advance_time`, `advance_weather`.)
2. Saves and dumps the box — this is `initial.txt`, the state at tick 0.
3. Runs `/tick step N` and **polls `/time query gametime` until the game time has moved by
   exactly N**. The console does not wait for a step: each server iteration drains the
   command queue, so a marker sent after `/tick step` is answered after one stepped tick,
   not N. The game time is the tick counter and stops when the step ends, and the run fails
   if it moved by anything other than N.
4. Saves again and dumps the box — this is `baseline.txt`.

`MANIFEST.txt` records the jar hash, the Java version, the rule set, the box, the exact
console script, the region-file hashes and the game time at both ends.

## What the dumps are

A palette of block states, then one line per `(y, z)` row of palette indices. States are
written `name[prop=value,...]` with the properties sorted, which is the form the Rust side
prints too, so the two can be compared as text. Rows are `y` outer, then `z`; each line
lists every `x` in the box.

They are small on purpose — 864 cells per scenario, about 2.4 KB — because the box is a
sealed bedrock shell: fluid cannot leave it, so comparing the box is comparing everything
the scenario can touch.
