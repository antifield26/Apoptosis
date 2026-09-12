"""Survey vanilla tag semantics from the 26.1.2 jar.

Local research tool (not part of the product crates). Establishes exactly which tag
features the loader must support, rather than guessing from memory.
"""

import collections
import json
import zipfile

JAR = 'server-26.1.2.jar'
PREFIX = 'data/minecraft/tags/'

z = zipfile.ZipFile(JAR)
names = [n for n in z.namelist() if n.startswith(PREFIX) and n.endswith('.json')]
print(f'tag files: {len(names)}')

nested = 0
optional = 0
required_false = 0
replace_count = 0
examples = {}

for n in names:
    d = json.loads(z.read(n))
    if 'replace' in d:
        replace_count += 1
        examples.setdefault('replace', n)
    for v in d.get('values', []):
        if isinstance(v, str) and v.startswith('#'):
            nested += 1
            examples.setdefault('nested', n)
        elif isinstance(v, dict):
            optional += 1
            examples.setdefault('optional', n)
            if v.get('required') is False:
                required_false += 1
                examples.setdefault('required_false', n)

print(f'nested (#tag) references : {nested}')
print(f'dict-form entries        : {optional}')
print(f'  of which required=false: {required_false}')
print(f'tags using "replace"     : {replace_count}')
print()

for key in ('nested', 'optional', 'required_false', 'replace'):
    n = examples.get(key)
    if n:
        print(f'--- {key}: {n}')
        print(z.read(n).decode()[:420])
        print()

# How deep does nesting go? A cycle would be a real hazard.
by_name = {}
for n in names:
    key = n[len(PREFIX):-len('.json')]
    by_name[key] = json.loads(z.read(n))

def depth(key, seen):
    if key in seen:
        return -1  # cycle
    seen = seen | {key}
    best = 0
    for v in by_name.get(key, {}).get('values', []):
        if isinstance(v, str) and v.startswith('#'):
            inner = v[1:].split(':', 1)[-1] if ':' in v else v[1:]
            reg = key.split('/', 1)[0]
            target = f'{reg}/{inner}'
            if target in by_name:
                d = depth(target, seen)
                if d < 0:
                    return -1
                best = max(best, d + 1)
    return best

worst = 0
worst_key = None
cycles = []
for key in by_name:
    d = depth(key, frozenset())
    if d < 0:
        cycles.append(key)
    elif d > worst:
        worst, worst_key = d, key

print(f'deepest nesting: {worst} (e.g. {worst_key})')
print(f'tags in a cycle: {cycles if cycles else "none"}')
print()
print('native entry counts per registry (top 6):')
counts = collections.Counter()
for n in names:
    reg = n[len(PREFIX):].split('/', 1)[0]
    counts[reg] += 1
for reg, c in counts.most_common(6):
    print(f'  {reg:24} {c}')
