"""Every numeric registry id this server puts on the wire, and where each one comes from.

## Why this shape

KD-56 was not really about doc comments. It was about **a number sent to a client that was looked up by the
wrong rule**: `default_state` returned a block's lowest id and the client was told to render `axis=x` logs and
waterlogged leaves. The prose was where the mistake was *recorded*, not where it was made.

So the systematic form of that question is: **what else does this server send as a numeric registry id, and
where does each number come from?** A block state id has a jar-derived table that is verified row by row; if an
entity type id, a menu id or a particle id is resolved some other way, it is a candidate for the same defect.

This finds `minecraft:`-namespaced identifier strings in product code, which is how every one of these
lookups is written, and groups them by crate so the id-like ones stand out from the paths and dimension names.
"""

import io
import re
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')

LITERAL = re.compile(r'"(minecraft:[a-z0-9_/]+)"')

by_crate = defaultdict(Counter)
for path in sorted(list(ROOT.glob('crates/**/*.rs')) + list(ROOT.glob('apps/**/*.rs'))):
    if '/tests/' in path.as_posix() or path.name == 'tests.rs':
        continue
    crate = path.relative_to(ROOT).as_posix().split('/')[1]
    for match in LITERAL.finditer(path.read_text(encoding='utf-8', errors='ignore')):
        by_crate[crate][match.group(1)] += 1

out = io.open(ROOT / 'target' / 'registry_ids.txt', 'w', encoding='utf-8', newline='')
total = 0
for crate in sorted(by_crate, key=lambda c: -sum(by_crate[c].values())):
    names = by_crate[crate]
    total += sum(names.values())
    out.write(f'=== {crate} ({len(names)} distinct, {sum(names.values())} uses) ===\n')
    for name, count in names.most_common(40):
        out.write(f'  {count:>4}  {name}\n')
    out.write('\n')
out.write(f'total literal minecraft: identifiers in product code: {total}\n')
out.close()
print(f'written; {total} literal identifiers across {len(by_crate)} crates')
