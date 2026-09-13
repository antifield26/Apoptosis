"""Find tests that cannot fail — the filter, with a working matcher this time.

## What was wrong

`\\bassert\\b` does not match `assert_bytes_eq`, because `_` is a word character. `\\bpanic!\\b` does not match
`panic!("...")`, because `!` is not one. So the first version of this filter reported sixteen tests as
assertion-free, and all four of the ones I read asserted — two through a helper named `assert_*`, two through
`panic!`. **It produced a tidy list, and a tidy list is not a correct one**, which is the failure this whole
review is about, arriving this time in the tool doing the reviewing.

## What it looks for now

Substring matches on the words that make a test able to fail, plus the naming conventions this codebase uses for
helpers that assert (`check_`, `verify_`, `ensure_`). It is still a **filter, not a verdict**: a test whose
purpose is "this does not panic" has no assertion by design and is a real test, because a panic fails it.
"""

import re
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')

TEST_ATTR = re.compile(r'#\[(tokio::)?test\]')
FN_START = re.compile(r'\s*(?:async\s+)?fn\s+(\w+)')
CAN_FAIL = re.compile(r'assert|expect|unwrap|panic|unreachable|todo|check_|verify_|ensure_')

results = []
for path in sorted(list(ROOT.glob('crates/**/*.rs')) + list(ROOT.glob('apps/**/*.rs'))):
    text = path.read_text(encoding='utf-8', errors='ignore')
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if not TEST_ATTR.search(line):
            continue
        cursor = index + 1
        while cursor < len(lines) and not FN_START.match(lines[cursor]):
            cursor += 1
        if cursor >= len(lines):
            continue
        name = FN_START.match(lines[cursor]).group(1)
        depth = 0
        body = []
        started = False
        for line in lines[cursor:]:
            body.append(line)
            depth += line.count('{') - line.count('}')
            if '{' in line:
                started = True
            if started and depth <= 0:
                break
        blob = '\n'.join(body)
        if not CAN_FAIL.search(blob):
            results.append((path.relative_to(ROOT).as_posix(), cursor + 1, name, len(body)))

print(f'tests whose body names no way to fail: {len(results)}')
for path, line, name, size in results:
    # A name that promises a property, on a body that asserts nothing, is the interesting one.
    promises = any(word in name for word in ('match', 'refus', 'report', 'pass', 'reject', 'equal', 'correct'))
    mark = '  <-- promises a property' if promises else ''
    print(f'  {path}:{line}  {name}  ({size} lines){mark}')
