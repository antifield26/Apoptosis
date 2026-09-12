"""Recount the data pack precisely and correct the baseline.

Every count in the baseline that came from counting `zipfile` namelist entries included
**directory entries**, which are separate names ending in `/`. That inflated each
figure by one per directory:

    data/minecraft/recipe/        1516 entries, 1515 .json files
    data/minecraft/               8775 entries, 8282 files

The recipe discrepancy is what exposed it: the loader reported 1421 + 94 = 1515, the
test expected 1516, and the loader was right. Correcting the *measurement* rather than
the code is the point — and it is the third time in this project that a number written
from a hasty count had to be corrected against a precise one.
"""

import collections
import io
import json
import zipfile

JAR = 'C:/Users/25371/projects/MinecraftServer/target/vanilla-26.1.2/server-26.1.2.jar'
z = zipfile.ZipFile(JAR)

names = z.namelist()
files = [n for n in names if not n.endswith('/')]
prefix_files = [n for n in files if n.startswith('data/minecraft/')]
print(f'data/minecraft: {len([n for n in names if n.startswith("data/minecraft/")])} entries, '
      f'{len(prefix_files)} files')

tags = [n for n in prefix_files if n.startswith('data/minecraft/tags/')]
tags_json = [n for n in tags if n.endswith('.json')]
print(f'tags: {len(tags)} files, {len(tags_json)} json')

recipes = [n for n in prefix_files if n.startswith('data/minecraft/recipe/')]
recipes_json = [n for n in recipes if n.endswith('.json')]
print(f'recipes: {len(recipes)} files, {len(recipes_json)} json')

# Per-directory table for the baseline.
kinds = collections.Counter()
for n in prefix_files:
    rest = n[len('data/minecraft/'):]
    if '/' in rest:
        kinds[rest.split('/')[0]] += 1
print('\nper-directory file counts:')
for kind, count in kinds.most_common(12):
    print(f'  {kind:24} {count}')

# Tag registry table, derived from the .json files only.
regs = collections.Counter()
for n in tags_json:
    rest = n[len('data/minecraft/tags/'):]
    first = rest.split('/')[0]
    regs[first] += 1
print(f'\ntag first-segments: {len(regs)}')
for reg, count in regs.most_common():
    print(f'  {reg:24} {count}')
