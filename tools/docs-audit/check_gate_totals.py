"""Gate-total consistency: the test total must be stated the same everywhere.

Audit 07's finding M2 was a stale count in `RELEASE-CANDIDATE.md`: six documents each restate the
workspace test total by hand, so updating it takes six edits and any missed one leaves two current
documents contradicting each other. Fixing that instance is not the fix; this is.

`docs/testing/TEST-MATRIX.md` is the **single owner** of the figure. Every other document that states it
must agree, and this fails (exit 1) when one does not.

Two kinds of document are exempt as *history*, and the distinction is the point: a **cumulative record**
states what past rounds measured, so an older figure in it is correct and rewriting it would falsify the
record. Those are `docs/audits/**` (audit reports), `CHANGELOG.md` and `docs/GOVERNANCE-REPORT.md`
(per-round records). Everything else must agree with the owner.

**Limitation, stated rather than left as a trap:** this compares documents **to each other**, not to the
tree. Nothing here can run `cargo test`, so a canonical figure that is stale while every other document
agrees with it passes silently — which is exactly what happened when P10-01 moved the total from 1 196 to
1 206 and this checker still exited 0. Update the owner when the total changes; the checker catches
disagreement, not staleness.

Usage: python tools/docs-audit/check_gate_totals.py
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CANONICAL_DOC = 'docs/testing/TEST-MATRIX.md'

tracked = subprocess.run(
    ['git', 'ls-files'], cwd=ROOT, capture_output=True, text=True, encoding='utf-8'
).stdout.splitlines()
docs = sorted(f for f in tracked if f.endswith('.md'))

canonical_text = (ROOT / CANONICAL_DOC).read_text(encoding='utf-8')
canonical = re.search(r'Totals:\s*\*\*(\d[\d ]*)\s*passed', canonical_text)
if not canonical:
    print(f'{CANONICAL_DOC}: no "Totals: **N passed" line to own the figure')
    sys.exit(1)
TOTAL = canonical.group(1).replace(' ', '')
print(f'canonical total ({CANONICAL_DOC}): {TOTAL} passed\n')

# A stated total looks like "1 194 passed", "1194 passed" or "1,194 passed"
# (comma-grouped, which used to be matched as the bare trailing digits and
# produced a mangled diagnostic — Audit 08, L3).
STATED = re.compile(r'(\d[\d,\u202f ]{2,})\s*passed')
# An unrendered template placeholder (Audit 08, H2): "{{lib_sum}}" style
# fragments left behind by a generation step are exactly as silent as a
# stale number.
PLACEHOLDER = re.compile(r'\{[a-z_]{3,}\}')

#: Documents that record what *past* rounds measured. An older figure in these is correct, and rewriting it
#: would falsify the record; current-state documents must agree with the owner instead.
HISTORY = ('docs/audits/',)
HISTORY_FILES = ('CHANGELOG.md', 'docs/GOVERNANCE-REPORT.md')


def is_history(rel: str) -> bool:
    return rel.startswith(HISTORY) or rel in HISTORY_FILES

findings = []
exempt = []
for rel in docs:
    if rel == CANONICAL_DOC:
        continue
    text = (ROOT / rel).read_text(encoding='utf-8')
    for number, line in enumerate(text.splitlines(), 1):
        if rel != CANONICAL_DOC:
            for match in PLACEHOLDER.finditer(line):
                entry = (rel, number, match.group(0), line.strip()[:100])
                if rel.startswith('docs/audits/'):
                    exempt.append(entry)
                else:
                    findings.append(entry)
        for match in STATED.finditer(line):
            value = match.group(1).replace(' ', '').replace(',', '').replace('\u202f', '')
            if value == TOTAL:
                continue
            entry = (rel, number, value, line.strip()[:100])
            if is_history(rel):
                exempt.append(entry)
            else:
                findings.append(entry)

print(f'exempt (cumulative records and audit reports): {len(exempt)}')
for rel, number, value, line in exempt:
    print(f'    {rel}:{number} states {value} (predates the current tree)')
print()

print(f'MISMATCHED totals: {len(findings)}')
for rel, number, value, line in findings:
    print(f'    {rel}:{number} states {value}, canonical is {TOTAL}')
    print(f'        {line}')
print()

if findings:
    print('Update every document that restates the total, or replace the number with a reference to')
    print(f'{CANONICAL_DOC}.')
sys.exit(1 if findings else 0)
