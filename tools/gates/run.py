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
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
QUICK = '--quick' in sys.argv


def run(label: str, command: list[str]) -> tuple[bool, str]:
    print(f'--- {label}')
    # **Encoding named explicitly.** The default on this host is GBK, and a gate that writes a byte it cannot
    # decode makes the reader thread raise -- which turned a passing run into exit code 1 the first time this
    # script was used. The script exists so that a gate cannot be quietly skipped; it cannot do that job if
    # it fails on its own output handling.
    result = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, encoding='utf-8', errors='replace'
    )
    if result.returncode != 0:
        tail = (result.stderr or result.stdout or '').strip().splitlines()[-6:]
        for line in tail:
            print(f'      {line[:110]}')
    return result.returncode == 0, result.stdout or ''


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
passed = failed = ignored = suites = 0
for match in re.finditer(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored', output):
    passed += int(match.group(1))
    failed += int(match.group(2))
    ignored += int(match.group(3))
    suites += 1
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
