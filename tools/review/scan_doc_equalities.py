"""The equality claims and the guarantees, written to a file for a reading pass.

`same as`, `equivalent`, `identical`, `guarantee` and `invariant` together are a few dozen doc lines in product
code, and every one is a **checkable equation or promise**: "X is the same as Y" can be falsified by looking at X
and Y, which is what KD-52, KD-54 and KD-56 each turned out to be.

Written to `target/equality_claims.txt` rather than printed: an em dash in one of the comments kills a GBK
console with `UnicodeEncodeError`, which is a silly way to lose a scan.
"""

import io
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
OUT = ROOT / 'target' / 'equality_claims.txt'

PATTERNS = [
    ('same as / equivalent / identical', ('same as', 'equivalent', 'identical')),
    ('guarantee', ('guarantee',)),
]

with io.open(OUT, 'w', encoding='utf-8', newline='') as out:
    for label, needles in PATTERNS:
        out.write(f'=== {label} ===\n')
        hits = 0
        for path in sorted(list(ROOT.glob('crates/**/*.rs')) + list(ROOT.glob('apps/**/*.rs'))):
            if '/tests/' in path.as_posix() or path.name == 'tests.rs':
                continue
            short = path.relative_to(ROOT).as_posix().replace('crates/', '').replace('/src/', '/')
            lines = path.read_text(encoding='utf-8', errors='ignore').splitlines()
            for index, line in enumerate(lines):
                if not line.strip().startswith('///'):
                    continue
                lowered = line.lower()
                if not any(needle in lowered for needle in needles):
                    continue
                hits += 1
                out.write(f'  {short}:{index + 1}\n')
                for j in range(max(0, index - 1), min(len(lines), index + 2)):
                    text = lines[j].strip().lstrip('/').strip()
                    out.write(f'      {text[:120]}\n')
        out.write(f'  -- {hits} hits\n\n')

print(f'written to {OUT}')
