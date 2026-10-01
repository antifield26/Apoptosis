#!/usr/bin/env python3
"""Run the real 26.1.2 server on each fluid scenario and dump the resulting chunk cells.

This is the **vanilla side** of the P20-01 differential rig. It does not synthesise,
approximate or hand-write anything: it starts the official server jar, builds each
scenario out of the console commands the frozen spec names, advances it exactly `N`
frozen ticks with `/tick step`, saves, and reads the block states back out of the
region file the server wrote. The dump files under
`crates/test-support/fixtures/fluids/<scenario>/` are that reading, and nothing else.

    python tools/fluid-differential/capture.py                     # all five
    python tools/fluid-differential/capture.py --scenario spring_flow
    python tools/fluid-differential/capture.py --repro             # run each twice

## Why the run is shaped this way

* **`/tick freeze` before the setup, `/tick step N` after it.** A scenario has to start
  at a known tick. With the game frozen the setup commands land on a world that is not
  ticking, and the scheduled fluid ticks they create stay queued until `tick step N`
  consumes exactly `N` of them. Timing the setup from the shell instead would make the
  starting tick a property of how fast this script talks to the console.
* **`/save-all flush` twice, read the region file twice.** The first read is the state
  at tick 0 — what `initial.txt` records, and what the Rust side must build — and the
  second is tick `N`. Both come out of one run, so they cannot come from different
  worlds.
* **A superflat world of one bedrock layer, structures off, `randomTickSpeed 0`.** No
  terrain, no weather, no random ticks: the only thing that changes is the fluid the
  scenario places, so a divergence is a fluid divergence.
* **No player, no client.** Everything is a console command, and `/forceload` keeps the
  one compared chunk loaded and ticking. The `bucket_place` row is the one place this
  costs something: the console cannot deliver a `UseItemOn`, so that row's setup writes
  the block state a bucket leaves behind rather than performing the bucket use. The
  Rust test's module docs name that boundary.
"""

import argparse
import hashlib
import math
import re
import shutil
import subprocess
import sys
import time
import tomllib
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROBE_DIR = ROOT / "tools" / "vanilla-probe"
sys.path.insert(0, str(PROBE_DIR))
from nbt_dump import Reader, read_payload  # noqa: E402  (path set up above)

SPEC_PATH = ROOT / "crates" / "test-support" / "fixtures" / "fluids" / "scenarios.toml"
FIXTURES = ROOT / "crates" / "test-support" / "fixtures" / "fluids"
SCRATCH = ROOT / "target" / "fluid-differential"

SECTOR = 4096
SECTION_CELLS = 4096


# --------------------------------------------------------------------- region file


def region_chunk_bytes(raw, cx, cz):
    """The decompressed NBT of one chunk, or None when its slot is empty."""
    slot = (cx & 31) + (cz & 31) * 32
    offset = int.from_bytes(raw[slot * 4 : slot * 4 + 4], "big")
    if offset == 0:
        return None
    start = (offset >> 8) * SECTOR
    length = int.from_bytes(raw[start : start + 4], "big")
    compression = raw[start + 4]
    payload = raw[start + 5 : start + 4 + length]
    if compression == 1:
        import gzip

        return gzip.decompress(payload)
    if compression == 2:
        import zlib

        return zlib.decompress(payload)
    if compression == 3:
        return payload
    raise SystemExit(f"unsupported region compression id {compression}")


def read_chunk_tree(raw, cx, cz):
    body = region_chunk_bytes(raw, cx, cz)
    if body is None:
        return None
    reader = Reader(body)
    tag = reader.u8()
    reader.string()  # root name
    return read_payload(reader, tag)


def state_string(entry):
    """`minecraft:water[level=3]`, properties sorted — the same form the Rust side prints."""
    if isinstance(entry, str):
        return entry
    name = entry.get("Name", "?")
    properties = entry.get("Properties") or {}
    if not properties:
        return name
    inner = ",".join(f"{key}={properties[key]}" for key in sorted(properties))
    return f"{name}[{inner}]"


def section_cells(section):
    """The 4096 state strings of one section, in vanilla order (y * 256 + z * 16 + x)."""
    block_states = section.get("block_states") or {}
    palette = [state_string(entry) for entry in block_states.get("palette") or []]
    if not palette:
        return ["minecraft:air"] * SECTION_CELLS
    if len(palette) == 1:
        return [palette[0]] * SECTION_CELLS
    data = block_states.get("data") or []
    bits = max(4, math.ceil(math.log2(len(palette))))
    per_long = 64 // bits
    mask = (1 << bits) - 1
    cells = []
    for value in data:
        value &= (1 << 64) - 1
        for index in range(per_long):
            cells.append((value >> (index * bits)) & mask)
    if len(cells) < SECTION_CELLS:
        raise SystemExit(f"section has {len(cells)} cells, expected {SECTION_CELLS}")
    return [palette[index] if index < len(palette) else "?" for index in cells[:SECTION_CELLS]]


def box_states(tree, box):
    """Every cell of `box` (inclusive world coordinates), as `state strings`.

    Row-major over `y`, then `z`, then `x`; the dump file's `cells` lines are one
    `(y, z)` row each, so a reader can reconstruct any coordinate.
    """
    x0, y0, z0, x1, y1, z1 = box
    by_section = {section.get("Y"): section_cells(section) for section in tree.get("sections") or []}
    rows = []
    for y in range(y0, y1 + 1):
        section = by_section.get(y >> 4)
        for z in range(z0, z1 + 1):
            row = []
            for x in range(x0, x1 + 1):
                if section is None:
                    row.append("minecraft:air")
                    continue
                local = ((y & 15) << 8) | ((z & 15) << 4) | (x & 15)
                row.append(section[local])
            rows.append(row)
    return rows


def write_dump(path, scenario_name, header, rows):
    palette = []
    index_of = {}
    indices = []
    for row in rows:
        out = []
        for state in row:
            if state not in index_of:
                index_of[state] = len(palette)
                palette.append(state)
            out.append(index_of[state])
        indices.append(out)
    lines = [
        "# Fluid differential dump (P20-01). Produced by the real 26.1.2 server;",
        "# written by tools/fluid-differential/capture.py. Do not edit by hand.",
        f"# scenario: {scenario_name}",
    ]
    lines.extend(f"# {line}" for line in header)
    lines.append(f"palette {len(palette)}")
    lines.extend(palette)
    lines.append(f"cells {len(indices[0])} {len(indices)}")
    lines.extend(" ".join(str(index) for index in row) for row in indices)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    return len(palette)


# ------------------------------------------------------------------- the server


class Vanilla:
    def __init__(self, java, classpath, main_class, cwd, log_name="console.log"):
        self.cwd = Path(cwd)
        self.log_path = self.cwd / log_name
        self.log = self.log_path.open("w", encoding="utf-8")
        self.proc = subprocess.Popen(
            [java, "-cp", classpath, main_class, "nogui"],
            cwd=str(self.cwd),
            stdin=subprocess.PIPE,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        self.seen = 0
        # Console output read since the last marker or query consumed it. Holding it
        # rather than re-reading the log is what stops a marker split across two reads
        # from being missed.
        self.pending = ""

    def pump(self):
        text = self.log_path.read_text(encoding="utf-8", errors="replace")
        new = text[self.seen :]
        self.seen = len(text)
        self.pending += new
        return new

    def wait_for(self, needle, timeout, what):
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.pump()
            if needle in self.pending:
                self.pending = ""
                return
            if self.proc.poll() is not None:
                raise SystemExit(
                    f"the server exited (code {self.proc.returncode}) while waiting for {what}; "
                    f"see {self.log_path}"
                )
            time.sleep(0.2)
        raise SystemExit(f"timed out after {timeout}s waiting for {what}; see {self.log_path}")

    def send(self, command, marker=None, timeout=180):
        assert self.proc.stdin is not None
        self.proc.stdin.write(command + "\n")
        self.proc.stdin.flush()
        if marker is not None:
            self.wait_for(marker, timeout, f"{marker!r} (after `{command}`)")

    def gametime(self, timeout=120):
        """The overworld game time, which is the tick counter: it does not move while
        the game is frozen, and moves exactly once per stepped tick."""
        self.pending = ""
        self.send("time query gametime")
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.pump()
            match = re.search(r"game time is (\d+)", self.pending)
            if match:
                self.pending = ""
                return int(match.group(1))
            if self.proc.poll() is not None:
                raise SystemExit(
                    f"the server exited (code {self.proc.returncode}) while querying the game time; "
                    f"see {self.log_path}"
                )
            time.sleep(0.05)
        raise SystemExit(f"no game-time answer within {timeout}s; see {self.log_path}")

    def assert_clean(self, commands):
        """Fail when the server rejected a command. A rejected `gamerule` or `fill`
        leaves a world that is not the one the spec describes, and every baseline
        after it would be evidence for the wrong scenario."""
        self.pump()
        bad = [
            line
            for line in self.pending.splitlines()
            if "Incorrect argument for command" in line
            or "Unknown or incomplete command" in line
            or "That position is not loaded" in line
        ]
        self.pending = ""
        if bad:
            raise SystemExit(
                "the server rejected part of the setup, so this run is not the scenario the "
                f"spec describes:\n  " + "\n  ".join(bad) + f"\n  commands sent: {commands}"
            )

    def stop(self):
        if self.proc.poll() is None:
            assert self.proc.stdin is not None
            self.proc.stdin.write("stop\n")
            self.proc.stdin.flush()
            try:
                self.proc.wait(timeout=120)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=30)
        self.log.close()


def sha1(path):
    digest = hashlib.sha1()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def classpath(spec):
    jar = ROOT / spec["vanilla"]["jar"]
    libraries = ROOT / spec["vanilla"]["libraries"]
    entries = [str(jar)]
    entries.extend(str(path) for path in sorted(libraries.rglob("*.jar")))
    return jar, ";".join(entries)


def op_commands(op):
    block = op["block"]
    if "fill" in op:
        x0, y0, z0, x1, y1, z1 = op["fill"]
        return [f"fill {x0} {y0} {z0} {x1} {y1} {z1} {block} replace"]
    x, y, z = op["set"]
    return [f"setblock {x} {y} {z} {block} replace"]


def server_properties(spec, level_name):
    world = spec["world"]
    lines = {
        "level-name": level_name,
        "level-type": world["level_type"].replace(":", "\\:"),
        "generator-settings": world["generator_settings"],
        "level-seed": str(world["seed"]),
        "generate-structures": "false",
        "online-mode": "false",
        "spawn-protection": "0",
        "difficulty": "peaceful",
        "gamemode": "creative",
        "view-distance": "4",
        "simulation-distance": "4",
        "sync-chunk-writes": "true",
        "pause-when-empty-seconds": "0",
        "max-tick-time": "-1",
        "enable-status": "false",
        "enable-jmx-monitoring": "false",
        "enable-query": "false",
        "enable-rcon": "false",
        "enable-code-of-conduct": "false",
        "allow-nether": "false",
    }
    return "".join(f"{key}={value}\n" for key, value in lines.items())


def run_scenario(spec, scenario, java, classpath_str, main_class, home, tag):
    """One server run: setup under freeze, tick `N`, save, dump. Returns the evidence dict."""
    name = scenario["name"]
    ticks = scenario["ticks"]
    box = spec["box"]
    chunk_x, chunk_z = spec["world"]["chunk"]
    level_name = f"world-{name}"
    world_dir = home / level_name

    shutil.rmtree(world_dir, ignore_errors=True)
    home.mkdir(parents=True, exist_ok=True)
    (home / "eula.txt").write_text("eula=true\n", encoding="utf-8")
    (home / "server.properties").write_text(
        server_properties(spec, level_name), encoding="utf-8", newline="\n"
    )

    console = []
    console.append("say RIG-BOOT")
    for rule in spec["world"]["gamerules"]:
        console.append(f"gamerule {rule}")
    console.append("difficulty peaceful")
    fx0, fz0, fx1, fz1 = spec["world"]["forceload"]
    console.append(f"forceload add {fx0} {fz0} {fx1} {fz1}")
    console.append("say RIG-FORCELOADED")
    console.append("tick freeze")
    console.append("say RIG-FROZEN")
    for op in spec["frame"]:
        console.extend(op_commands(op))
    for op in scenario["setup"]:
        console.extend(op_commands(op))
    console.append("say RIG-SETUP-APPLIED")
    console.append("save-all flush")
    console.append("say RIG-INITIAL-SAVED")

    server = Vanilla(java, classpath_str, main_class, home, log_name=f"console-{tag}.log")
    try:
        server.wait_for("Done (", 600, "server startup")
        setup_commands = []
        for command in console:
            setup_commands.append(command)
            marker = command[4:] if command.startswith("say ") else None
            server.send(command, marker=marker)
        # Every command either took effect or this run is not the scenario in the spec.
        server.assert_clean(setup_commands)

        # Tick 0 is this game time: the game is frozen, so it cannot move until the
        # step below, whatever this script does in between.
        tick_zero = server.gametime()

        # The state at tick 0 is on disk now.
        region = (
            world_dir
            / "dimensions"
            / "minecraft"
            / "overworld"
            / "region"
            / f"r.{chunk_x >> 5}.{chunk_z >> 5}.mca"
        )
        if not region.is_file():
            raise SystemExit(f"no region file at {region} after save-all")
        initial_raw = region.read_bytes()
        initial_tree = read_chunk_tree(initial_raw, chunk_x, chunk_z)
        if initial_tree is None:
            raise SystemExit(f"chunk ({chunk_x}, {chunk_z}) is empty in {region} at tick 0")

        # `/tick step N` runs exactly N ticks, but the *console* does not wait for it:
        # each server iteration drains the command queue, so a marker sent after the
        # step is answered after one stepped tick, not N. The game time is the tick
        # counter and stops with the step, so polling it is what says "the N ticks
        # have run" -- and the assertion below is what says they were exactly N.
        server.send(f"tick step {ticks}")
        deadline = time.time() + 600
        while True:
            now = server.gametime()
            if now >= tick_zero + ticks:
                break
            if time.time() > deadline:
                raise SystemExit(
                    f"the step stopped advancing: game time {now} after "
                    f"{time.time() - (deadline - 600):.0f}s, wanted {tick_zero + ticks}"
                )
        server.send("save-all flush")
        server.send("say RIG-FINAL-SAVED", marker="RIG-FINAL-SAVED")
        tick_end = server.gametime()
        if tick_end != tick_zero + ticks:
            raise SystemExit(
                f"the run advanced {tick_end - tick_zero} ticks, not {ticks}: everything the "
                "baseline claims about tick N would be wrong"
            )
        final_raw = region.read_bytes()
        final_tree = read_chunk_tree(final_raw, chunk_x, chunk_z)
        if final_tree is None:
            raise SystemExit(f"chunk ({chunk_x}, {chunk_z}) is empty in {region} after the step")
    finally:
        server.stop()

    header = [
        f"ticks: {ticks}",
        f"game time: {tick_zero} at tick 0, {tick_end} at tick {ticks}",
        f"box: x {box[0]}..{box[3]}, y {box[1]}..{box[4]}, z {box[2]}..{box[5]} (inclusive)",
        f"chunk: ({chunk_x}, {chunk_z})  region: r.0.0.mca",
        f"region sha256 at tick 0: {sha256_bytes(initial_raw)}",
        f"region sha256 at tick {ticks}: {sha256_bytes(final_raw)}",
    ]
    initial_rows = box_states(initial_tree, box)
    final_rows = box_states(final_tree, box)
    out_dir = FIXTURES / name
    out_dir.mkdir(parents=True, exist_ok=True)
    # `run1` is the committed pair; a reproducibility run writes beside it and is
    # deleted once the two have been compared (it is a check, not an artifact).
    suffix = "" if tag == "run1" else f".{tag}"
    write_dump(out_dir / f"initial{suffix}.txt", name, header, initial_rows)
    write_dump(out_dir / f"baseline{suffix}.txt", name, header, final_rows)

    changed = sum(
        1
        for initial_row, final_row in zip(initial_rows, final_rows)
        for a, b in zip(initial_row, final_row)
        if a != b
    )
    return {
        "console": console,
        "header": header,
        "initial_rows": initial_rows,
        "final_rows": final_rows,
        "changed": changed,
        "level_name": level_name,
        "region": region,
        "tick_zero": tick_zero,
        "tick_end": tick_end,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", action="append", default=None)
    parser.add_argument("--java", default="java")
    parser.add_argument("--repro", action="store_true", help="run every selected scenario twice")
    parser.add_argument("--home", default=None)
    args = parser.parse_args()

    with open(SPEC_PATH, "rb") as handle:
        spec = tomllib.load(handle)

    jar, classpath_str = classpath(spec)
    if not jar.is_file():
        raise SystemExit(f"the vanilla server jar is missing: {jar}")
    digest = sha1(jar)
    if digest != spec["vanilla"]["sha1"]:
        raise SystemExit(f"{jar} is sha1 {digest}, the spec pins {spec['vanilla']['sha1']}")

    java_version = subprocess.run(
        [args.java, "-version"], capture_output=True, text=True, encoding="utf-8", errors="replace"
    ).stderr.strip()

    scenarios = spec["scenario"]
    if args.scenario:
        wanted = set(args.scenario)
        scenarios = [s for s in scenarios if s["name"] in wanted]
        missing = wanted - {s["name"] for s in scenarios}
        if missing:
            raise SystemExit(f"no such scenario: {sorted(missing)}")

    home_root = Path(args.home) if args.home else SCRATCH / "server-home"
    manifest = []
    repro = {}
    for scenario in scenarios:
        name = scenario["name"]
        tags = ["run1", "run2"] if args.repro else ["run1"]
        runs = []
        for tag in tags:
            home = home_root / f"{name}-{tag}"
            print(f"== {name} ({tag}): starting the vanilla server in {home}")
            started = time.time()
            result = run_scenario(
                spec, scenario, args.java, classpath_str, spec["vanilla"]["main_class"], home, tag
            )
            print(
                f"   {result['changed']} of "
                f"{len(result['initial_rows'][0]) * len(result['initial_rows'])} box cells changed "
                f"over {scenario['ticks']} ticks ({time.time() - started:.0f}s)"
            )
            runs.append((tag, result))
        manifest.append((scenario, runs))
        if len(runs) == 2:
            differing = sum(
                1
                for a, b in zip(runs[0][1]["final_rows"], runs[1][1]["final_rows"])
                for left, right in zip(a, b)
                if left != right
            )
            repro[name] = differing
            print(f"   reproducibility: {differing} cells differ between two independent runs")

    # The committed dumps are the run1 pair; a second run is only a check.
    for scenario, runs in manifest:
        folder = FIXTURES / scenario["name"]
        for tag, _ in runs[1:]:
            (folder / f"initial.{tag}.txt").unlink(missing_ok=True)
            (folder / f"baseline.{tag}.txt").unlink(missing_ok=True)

    lines = [
        "Fluid differential baselines (P20-01) — provenance",
        "==================================================",
        "",
        "Produced by tools/fluid-differential/capture.py, which starts the official",
        "Minecraft Java 26.1.2 server and drives it entirely through console commands.",
        "Every state in the dumps below was read out of the region file that server wrote.",
        "",
        f"generator      : python tools/fluid-differential/capture.py"
        f"{' --repro' if args.repro else ''}",
        f"captured       : {datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}",
        f"server jar     : {spec['vanilla']['jar']}",
        f"server jar sha1: {digest}",
        f"java           : {java_version.splitlines()[0] if java_version else 'unknown'}",
        f"spec           : crates/test-support/fixtures/fluids/scenarios.toml (version {spec['version']})",
        f"world          : level-type={spec['world']['level_type']} "
        f"generator-settings={spec['world']['generator_settings']} "
        f"seed={spec['world']['seed']}",
        f"gamerules      : {'; '.join(spec['world']['gamerules'])}",
        f"forceload      : {spec['world']['forceload']} (block coords, so chunk "
        f"{spec['world']['chunk']})",
        f"box            : x {spec['box'][0]}..{spec['box'][3]}, y {spec['box'][1]}..{spec['box'][4]}, "
        f"z {spec['box'][2]}..{spec['box'][5]} (inclusive)",
        "",
    ]
    if repro:
        lines.append(
            "reproducibility: every scenario was captured twice, in two freshly generated"
        )
        lines.append(
            "worlds, and the committed dumps are the first run. Cells differing between the"
        )
        lines.append("two runs of the same scenario:")
        for name, differing in repro.items():
            lines.append(f"  {name}: {differing}")
        lines.append("")
    lines += [
        "Per scenario, the console script that produced it. `save-all flush` writes the",
        "region file; `initial.txt` is the dump taken right after the first save and",
        "`baseline.txt` the one taken after `/tick step N`, both from the same run.",
        "",
    ]
    for scenario, runs in manifest:
        tag, result = runs[0]
        lines.append(f"--- {scenario['name']} (ticks {scenario['ticks']}) ---")
        lines.append(f"world dir      : target/fluid-differential/server-home/{scenario['name']}-{tag}/"
                     f"{result['level_name']}")
        for line in result["header"]:
            lines.append(f"  {line}")
        lines.append("console script:")
        lines.extend(f"  {command}" for command in result["console"])
        lines.append(
            f"  tick step {scenario['ticks']}\n  save-all flush\n  say RIG-FINAL-SAVED\n  stop"
        )
        lines.append("")
    lines.append("Cell rows are `y` outer, then `z`; each line lists every `x` in the box as a")
    lines.append("palette index. The Rust differential test parses exactly this format.")
    (FIXTURES / "MANIFEST.txt").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"\nwrote {FIXTURES / 'MANIFEST.txt'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
