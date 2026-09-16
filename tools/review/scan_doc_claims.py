"""Find doc comments that make a claim strong enough to be wrong.

## Why this and not a reading of every comment

Every confirmed failure of this review that came from prose came from a comment stating a **semantics** the code
did not implement:

* `first_state_id`'s doc said "the id of this block's first (**default**) state" — the whole of KD-56, and the
  parenthesis is where the two ideas were conflated;
* `parse_axis`'s doc said a bare number "is distinguishable from `~0` once the source moves", which was true of
  the intent and false of the code — KD-54;
* `PalettedContainer`'s `values` were documented as indices and filled with ids — KD-52.

Three for three, the wrong thing was in the comment **before** it was in the code's behaviour, and in each case
the comment named both the right idea and the wrong one side by side.

## What it looks for

Words that make a claim falsifiable by reading the code beneath them:

* `default` — the KD-56 word;
* `always`, `never`, `only`, `exactly` — absolutes, which are cheap to disprove;
* `same as`, `equivalent`, `identical` — an equation between two things;
* `must`, `cannot` — an invariant;
* `distinguishable`, `not the same` — a distinction the code may not draw.

It is a **filter**: most hits are correct and careful. The point is that the list is short enough to read, and
that each hit is a place where the code can be asked a question with a definite answer.
"""

import re
from collections import Counter
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')

CLAIM = re.compile(
    r'\b(default|always|never|only|exactly|same as|equivalent|identical|must|cannot|'
    r'distinguishable|not the same|guarantee|invariant)\b',
    re.IGNORECASE,
)

hits = []
for path in sorted(list(ROOT.glob('crates/**/*.rs')) + list(ROOT.glob('apps/**/*.rs'))):
    if '/tests/' in path.as_posix() or path.name == 'tests.rs':
        continue  # test prose is not a contract the product must keep
    for index, line in enumerate(path.read_text(encoding='utf-8', errors='ignore').splitlines(), start=1):
        stripped = line.strip()
        # Module docs (`//!`) carry this repo's most load-bearing prose
        # (AUDIT-09 B-04, AUDIT-12 lane 6); skipping them blinds the scanner.
        if not (stripped.startswith('///') or stripped.startswith('//!')):
            continue
        match = CLAIM.search(stripped)
        if match:
            hits.append((path.relative_to(ROOT).as_posix(), index, match.group(1).lower(), stripped))

print(f'doc lines in product code making a falsifiable claim: {len(hits)}')
print()
print('by word:')
for word, count in Counter(hit[2] for hit in hits).most_common():
    print(f'  {word:<16} {count}')
print()
print('the first forty, which is what a reading pass would start with:')
for path, line, word, text in hits[:40]:
    short = path.replace('crates/', '').replace('/src/', '/')
    print(f'  {short}:{line}  [{word}]  {text[3:].strip()[:96]}')
