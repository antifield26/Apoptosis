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

Usage, from the repository root:

```text
python tools/gates/run.py            # every gate
python tools/gates/run.py --quick    # skip the aarch64 and deny passes
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


failures = []

# The five gates. `cargo test` is handled separately because its *output* is checked, not only its exit code.
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

# The tests, and **the count is the check**, not only the exit code.
ok, output = run('cargo test --workspace', ['cargo', 'test', '--workspace', '--no-fail-fast'])
passed, failed, ignored, suites = summarize_tests(output)
print(f'--- tests: {passed} passed, {failed} failed, {ignored} ignored, {suites} suites')

if not ok:
    failures.append('cargo test')
if suites == 0:
    # **The hole that let a commit through.** A test binary that cannot link prints an error and no result line,
    # and "no failing suite" is true of no suites at all.
    failures.append('cargo test produced no results (a locked binary, or a build failure)')
if failed:
    failures.append(f'{failed} failing tests')

print()
if failures:
    print('REFUSING to pass:')
    for failure in failures:
        print(f'  - {failure}')
    raise SystemExit(1)
print('every gate passed')
