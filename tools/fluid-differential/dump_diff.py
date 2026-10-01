#!/usr/bin/env python3
"""Show where two fluid dumps differ, cell by cell.

    python tools/fluid-differential/dump_diff.py <a.txt> <b.txt> [--limit 40]

The dumps are the ones `capture.py` writes (and the Rust differential test parses):
a palette of state strings, then one line per `(y, z)` row of palette indices. This
prints the first differing cells with their world coordinates, the palettes, and a
count -- the same first-divergence report the Rust test makes, so a divergence can be
read without re-running the server.

Useful for the two checks that come up most often:

* `initial.txt` against `baseline.txt` -- what the scenario actually did in vanilla
  (if nothing changed, the scenario is not exercising anything);
* two runs of the same scenario -- whether the vanilla baseline is reproducible at all
  (lava's `getSpreadDelay` has a random branch, so this is worth checking rather than
  assuming).
"""

import argparse
import re
import sys


def load(path):
    lines = open(path, encoding="utf-8").read().splitlines()
    header = [line for line in lines if line.startswith("#")]
    palette_at = next(i for i, line in enumerate(lines) if line.startswith("palette "))
    count = int(lines[palette_at].split()[1])
    palette = lines[palette_at + 1 : palette_at + 1 + count]
    cells_at = next(i for i, line in enumerate(lines) if line.startswith("cells "))
    _, nx, rows = lines[cells_at].split()
    nx, rows = int(nx), int(rows)
    # The dump header carries the box as `x a..b, y c..d, z e..f`; reorder it into
    # (x0, y0, z0, x1, y1, z1) so coordinates can be printed.
    box = None
    for line in header:
        if line.startswith("# box:"):
            numbers = [int(value) for value in re.findall(r"-?\d+", line)]
            if len(numbers) >= 6:
                box = [numbers[0], numbers[2], numbers[4], numbers[1], numbers[3], numbers[5]]
    grid = [[int(value) for value in lines[cells_at + 1 + r].split()] for r in range(rows)]
    for line in lines[cells_at + 1 + rows :]:
        if line.strip():
            raise SystemExit(f"{path}: unexpected content after the cell rows: {line!r}")
    return {"palette": palette, "grid": grid, "nx": nx, "rows": rows, "box": box, "header": header}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("a")
    parser.add_argument("b")
    parser.add_argument("--limit", type=int, default=40)
    args = parser.parse_args()

    a, b = load(args.a), load(args.b)
    if (a["nx"], a["rows"]) != (b["nx"], b["rows"]):
        raise SystemExit(
            f"the two dumps cover different regions: "
            f"{args.a} is {a['nx']}x{a['rows']}, {args.b} is {b['nx']}x{b['rows']}"
        )
    box = b["box"] or a["box"]
    x0, y0, z0 = (box[0], box[1], box[2]) if box else (0, 0, 0)
    nz = (box[5] - box[2] + 1) if box else 1

    changed = 0
    shown = 0
    for r, (row_a, row_b) in enumerate(zip(a["grid"], b["grid"])):
        for c, (ia, ib) in enumerate(zip(row_a, row_b)):
            # **Compare states, not palette indices.** Two dumps build their palettes
            # independently, so the same state can sit at different indices in each and
            # an index comparison reports differences that are not there.
            state_a, state_b = a["palette"][ia], b["palette"][ib]
            if state_a == state_b:
                continue
            changed += 1
            if shown < args.limit:
                shown += 1
                y = y0 + r // nz
                z = z0 + r % nz
                x = x0 + c
                print(f"  ({x}, {y}, {z}): {state_a} -> {state_b}")
    total = a["nx"] * a["rows"]
    print(f"{changed} of {total} cells differ  ({args.a} -> {args.b})")
    if changed and shown < changed:
        print(f"  (showing the first {shown})")
    if not changed:
        print("  identical")
    return 0


if __name__ == "__main__":
    sys.exit(main())
