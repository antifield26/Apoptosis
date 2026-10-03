"""The gates, as a file, so the check that was missing cannot be forgotten again.

## Why this exists

Five times this session a commit went out over a gate that had not actually passed, and the fourth's cause is the
reason this file is here: **the gate list lived in a shell command typed fresh each time**, so every run was a
chance to leave something out. That run left out the one condition that mattered --

```text
tests=0 failed=0 suites=0     <- cargo test produced no results at all
```

-- and the guard passed it, because "zero failed suites" is satisfied by *zero suites*. The cause was ordinary: a
running `mc-server.exe` holds the file on Windows, so `cargo test` could not relink and printed an error instead of
a result.

**A check written in a command is a check that is rewritten every time**, which is the same shape as the guards
this phase spent its time finding in people's heads. This one is a file, and its exit code is what a commit waits
on.

## What it refuses

* any gate or audit with a non-zero exit;
* **a test run that produced no results**, which is the hole above;
* a test run with any failing suite.

## The two tiers (TEST-TIME-PLAN §4)

```text
python tools/gates/run.py --quick    # per commit: fmt, clippy, docs audits, nextest over the workspace
python tools/gates/run.py            # full: the above plus aarch64, cargo-deny and the canonical cargo test
```

`--quick` keeps **every test** and buys its wall clock from scheduling rather than from dropping coverage:
nextest interleaves tests across binaries instead of running one binary at a time (measured 155 s against
`cargo test`'s 452 s on the development host, against the plan's < 8 min budget), with the retry policy for
the two suites that carry written flake history in `.config/nextest.toml`. The full tier runs `cargo test`,
which is what `docs/testing/TEST-MATRIX.md` owns and what also runs the doctests nextest skips -- so the
counts a document quotes come from the tier that owns them.

Usage, from the repository root:

```text
python tools/gates/run.py            # every gate (the canonical tier)
python tools/gates/run.py --quick    # the per-commit tier: everything but aarch64/deny, tests via nextest
```
"""

import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QUICK = '--quick' in sys.argv


def run(label: str, command: list[str]) -> tuple[bool, str]:
    print(f'--- {label}')
    # **Encoding named explicitly.** The default on this host is GBK, and a gate that writes a byte it cannot
    # decode makes the reader thread raise -- which turned a passing run into exit code 1 the first time this
    # script was used. The script exists so that a gate cannot be quietly skipped; it cannot do that job if
    # it fails on its own output handling.
    #
    # **stderr is merged into the parsed stream.** Cargo prints each test binary's
    # `Running ... (target/debug/deps/<name>-<hash>.exe)` header to *stderr* while the `test result:` lines go
    # to *stdout*, so parsing stdout alone left every row of the slowest-first table labelled `?` -- a table
    # that times suites without naming them, which is the half of TEST-TIME-PLAN §1 that mattered. Order is
    # what the label state machine needs, so the two are merged at the OS pipe rather than concatenated
    # afterwards. (`capture_output=True` and `stderr=STDOUT` are mutually exclusive, hence the explicit pipes.)
    start = time.monotonic()
    result = subprocess.run(
        command,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding='utf-8',
        errors='replace',
    )
    elapsed = time.monotonic() - start
    print(f'    ({elapsed:.1f}s)')
    if result.returncode != 0:
        tail = (result.stdout or '').strip().splitlines()[-6:]
        for line in tail:
            print(f'      {line[:110]}')
    return result.returncode == 0, result.stdout or ''


def summarize_tests(output: str) -> tuple[int, int, int, int]:
    """Aggregate counts and print a slowest-first table with suite labels.

    Labels come from cargo's own `Running ...` / `Doc-tests ...` headers, so
    a slow suite is named, not just timed (TEST-TIME-PLAN §1). Returns
    (passed, failed, ignored, suites).
    """
    current = '?'
    rows: list[tuple[float, int, str]] = []
    passed = failed = ignored = suites = 0
    for line in output.splitlines():
        stripped = line.strip()
        if stripped.startswith('Running ') or stripped.startswith('Doc-tests'):
            # `Running unittests src/lib.rs (target/.../mc_server-HASH.exe)` or
            # `Running tests/foo.rs (target/.../foo-HASH.exe)`: the exe stem
            # names the target — for lib suites that *is* the crate, for
            # integration suites the file stem (unique in practice).
            # Cargo prints `deps/...` on unix and `deps\...` on Windows.
            exe = re.search(r'deps[/\\]([A-Za-z0-9_]+)-[0-9a-f]+\.exe', stripped)
            if 'unittests' in stripped and exe:
                # The lib target is named after its crate (`mc_server-HASH`).
                current = exe.group(1)
            else:
                current = re.sub(r'\s*\(target[/\\].*$', '', stripped)
                current = re.sub(r'^Running (unittests |tests/)', '', current)
            current = current[:80]
        match = re.match(
            r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored'
            r'.*finished in ([\d.]+)s',
            stripped,
        )
        if match:
            passed += int(match.group(1))
            failed += int(match.group(2))
            ignored += int(match.group(3))
            suites += 1
            rows.append((float(match.group(4)), int(match.group(1)), current))
    rows.sort(reverse=True)
    print('--- slowest suites (seconds, tests, suite):')
    for seconds, count, label in rows[:15]:
        print(f'    {seconds:>9.2f}s  {count:>4} tests  {label}')
    # The plan's acceptance metric (TEST-TIME-PLAN §5: suite-time sum <= ~600 s) is a *sum*, and the table
    # above is the top 15 -- printing only the head made the number this work is measured against unreadable
    # from the gate output that measures it.
    print(f'--- suite-time sum: {sum(row[0] for row in rows):.1f}s across {len(rows)} suites')
    return passed, failed, ignored, suites


#: nextest's per-test line: `        PASS [   0.014s] (   1/2002) mc-capture-rig tests::name`.
NEXTEST_TEST = re.compile(
    r'^\s+(?:PASS|FAIL|FLAKY|SLOW)\s+\[\s*([\d.]+)s\]\s+\(\s*\d+/\d+\)\s+(\S+)\s+'
)
#: nextest's summary: `Summary [ 155.13s] 2002 tests run: 2002 passed (2 slow), 44 skipped`
#: -- the `(N slow)` clause appears once anything passes nextest's slow mark, the `failed` clause only when
#: something failed, and "1 test run" is singular.
NEXTEST_SUMMARY = re.compile(
    r'Summary \[[^\]]*\]\s+(\d+) tests? run:\s+(\d+) passed(?:\s*\(\d+ slow\))?'
    r'(?:,\s*(\d+) failed)?(?:,\s*(\d+) skipped)?'
)


def summarize_nextest(output: str) -> tuple[int, int, int, int]:
    """Aggregate the fast tier's run: nextest does not print cargo's per-suite `test result:` lines.

    Two differences from `summarize_tests` are labelled rather than hidden. The **suite rows are sums of
    their tests' durations**, not a binary's wall clock, because nextest interleaves tests across binaries
    and never reports a per-binary time; and the **totals are nextest's**, which do not include doctests
    (2 002 vs `cargo test`'s 2 006 today), so this tier reports counts but never claims the canonical figure
    that `docs/testing/TEST-MATRIX.md` owns.
    """
    per_binary: dict[str, float] = {}
    for line in output.splitlines():
        match = NEXTEST_TEST.match(line)
        if match:
            per_binary[match.group(2)] = per_binary.get(match.group(2), 0.0) + float(match.group(1))
    rows = sorted(((seconds, label) for label, seconds in per_binary.items()), reverse=True)
    print('--- slowest suites (sum of test seconds, suite):')
    for seconds, label in rows[:15]:
        print(f'    {seconds:>9.2f}s  {label}')
    print(f'--- suite-time sum: {sum(row[0] for row in rows):.1f}s across {len(rows)} suites')
    summary = NEXTEST_SUMMARY.search(output)
    if not summary:
        return 0, 0, 0, 0
    passed = int(summary.group(1))
    failed = int(summary.group(3) or 0)
    skipped = int(summary.group(4) or 0)
    return passed, failed, skipped, len(rows)


failures = []

# The five gates. The test tier is handled separately because its *output* is checked, not only its exit code.
for label, command in [
    ('cargo fmt --all -- --check', ['cargo', 'fmt', '--all', '--', '--check']),
    (
        'cargo clippy -D warnings',
        ['cargo', 'clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'],
    ),
]:
    ok, _ = run(label, command)
    if not ok:
        failures.append(label)

if not QUICK:
    for label, command in [
        (
            'cargo check aarch64',
            ['cargo', 'check', '--target', 'aarch64-unknown-linux-gnu', '--workspace', '--all-targets'],
        ),
        ('cargo deny', ['cargo', 'deny', 'check', 'licenses', 'bans', 'sources']),
    ]:
        ok, _ = run(label, command)
        if not ok:
            failures.append(label)

# The four docs audits.
for script in ('check_encoding', 'check_links', 'check_gate_totals', 'check_line_endings'):
    label = f'docs-audit {script}'
    ok, _ = run(label, [sys.executable, f'tools/docs-audit/{script}.py'])
    if not ok:
        failures.append(label)

# The two tiers (TEST-TIME-PLAN §4). `--quick` is the per-commit gate: the same
# workspace, scheduled across test binaries by nextest instead of one binary at a
# time (measured 344 s against `cargo test`'s 452 s on the development host,
# against the plan's < 8 min budget), with the retry policy in
# `.config/nextest.toml`. The default tier is the canonical one: `cargo test`,
# whose totals TEST-MATRIX owns and which also runs the doctests nextest skips.
TEST_LABEL = 'cargo nextest run --workspace' if QUICK else 'cargo test --workspace'
TEST_COMMAND = (
    ['cargo', 'nextest', 'run', '--workspace', '--no-fail-fast']
    if QUICK
    else ['cargo', 'test', '--workspace', '--no-fail-fast']
)
ok, output = run(TEST_LABEL, TEST_COMMAND)
if QUICK:
    passed, failed, ignored, suites = summarize_nextest(output)
    print(f'--- tests (nextest; doctests are the full tier\'s): {passed} run, '
          f'{failed} failed, {ignored} skipped, {suites} binaries')
else:
    passed, failed, ignored, suites = summarize_tests(output)
    print(f'--- tests: {passed} passed, {failed} failed, {ignored} ignored, {suites} suites')

if not ok:
    failures.append(TEST_LABEL)
if suites == 0:
    # **The hole that let a commit through.** A test binary that cannot link prints an error and no result line,
    # and "no failing suite" is true of no suites at all.
    failures.append(f'{TEST_LABEL} produced no results (a locked binary, or a build failure)')
if failed:
    failures.append(f'{failed} failing tests')

print()
if failures:
    print('REFUSING to pass:')
    for failure in failures:
        print(f'  - {failure}')
    raise SystemExit(1)
print('every gate passed')
