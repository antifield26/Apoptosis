"""Line-ending policy: no tracked text file may violate its `eol=lf` attribute.

`.gitattributes` normalises every textual file to LF in the repository **and** the working tree, and gives
the reason: the target is Linux (Pi 5, Debian 13) and CI runs there while development is on Windows, so
without the policy the same commit produces different bytes per platform — which would make `cargo fmt
--check`, fixture hashes and byte-exact wire tests platform-dependent.

That is not hypothetical. During Audit 07 lane E, three tracked files had CRLF in the working tree while
the index held LF. `git status` was clean (git normalises on comparison), so nothing looked wrong, but
comparing the *deployed* `items.tsv` on the Pi against the repository copy by SHA-256 reported a drift that
did not exist: 75 615 bytes of CRLF against 74 107 bytes of LF, one byte per line, identical content. After
normalisation the two hashes match exactly, which is what proved the deployment was correct.

## Scope, stated so the result is not over-read

This can only fire where a working tree was written by something that ignores the attributes — in practice
a Windows checkout or a script using `open(..., "w")` without `newline=""`. On the Linux runner `eol=lf`
checkout already yields LF, so the check passes there trivially. It is a **local** guard; CI runs it for
completeness, not because CI is where it earns its keep.

Usage: python tools/docs-audit/check_line_endings.py
"""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

result = subprocess.run(
    ['git', 'ls-files', '--eol'], cwd=ROOT, capture_output=True, text=True, encoding='utf-8',
    errors='replace',
)
if result.returncode != 0:
    print('git ls-files --eol failed; is this a git working tree?')
    sys.exit(1)

checked = 0
violations = []
for line in result.stdout.splitlines():
    parts = line.split('\t')
    if len(parts) != 2:
        continue
    fields = parts[0].split()
    if len(fields) < 3:
        continue
    checked += 1
    index_eol = fields[0].removeprefix('i/')
    work_eol = fields[1].removeprefix('w/')
    attrs = ' '.join(fields[2:])
    if 'eol=lf' in attrs and work_eol not in ('lf', 'none'):
        violations.append((parts[1], index_eol, work_eol, attrs))

print(f'tracked files with an eol= attribute: {checked}')
print(f'working-tree files violating eol=lf : {len(violations)}')
for path, index_eol, work_eol, attrs in violations:
    print(f'    {path}  (index {index_eol}, worktree {work_eol})')

if violations:
    print()
    print('Normalise them with a byte rewrite, e.g.:')
    print("    raw = path.read_bytes().replace(b'\\r\\n', b'\\n'); path.write_bytes(raw)")
    print('The index already holds LF, so the committed content is correct; only the working tree differs.')
    print('Nothing here is deleted, and `git status` may stay clean either way, which is why this check')
    print('exists rather than relying on git to notice.')

sys.exit(1 if violations else 0)
