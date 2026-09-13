"""Is entry 0 of the dimension-type registry really the overworld?

`crates/network/src/registry_data/mod.rs:284` says, as a comment:

> `join_game` sends `dimension_type_id: 0`, so entry 0 of that registry must be the overworld.

and `connection.rs:529` and `game.rs:2857` both send `0`. That is **an assumption about a registry the client
owns**, written down rather than checked — the same shape as KD-56 (a block's default assumed to be its lowest
id) and KD-65 (plains assumed to be biome 0, and id 0 is badlands).

It matters more than either: `join_game`'s dimension type carries the world's height, the sky and ceiling
behaviour, ambient light, and whether the client treats it as the overworld, the Nether or the End. A wrong one
does not error; it renders a different kind of world.

## The check

`crates/network/src/registry_data/config-payload.bin` is the exact byte sequence the client is handed, and the
`minecraft:dimension_type` registry in it is short — four entries rather than sixty-five — so the order is
readable without the prefix trick the biome list needed.
"""

import io
import re
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
payload = (ROOT / 'crates' / 'network' / 'src' / 'registry_data' / 'config-payload.bin').read_bytes()

start = payload.find(b'minecraft:dimension_type')
print(f'"minecraft:dimension_type" at offset {start}')
if start < 0:
    print('  not present: the payload does not carry that registry at all')
else:
    after = start + len(b'minecraft:dimension_type')
    names = []
    for match in re.finditer(rb'minecraft:[a-z0-9_/]{3,40}', payload[after:]):
        name = match.group().decode()
        if name in names:
            continue
        names.append(name)
        if len(names) >= 10:
            break
    print('the identifiers that follow it:')
    for index, name in enumerate(names):
        marker = '   <-- entry 0, which join_game sends' if index == 0 else ''
        print(f'  {index}  {name}{marker}')

print()
print('=== what the code sends ===')
for rel, needle in [
    ('crates/network/src/connection.rs', 'dimension_type_id: 0'),
    ('crates/server/src/game.rs', 'dimension_type_id: 0'),
    ('crates/network/src/registry_data/mod.rs', 'must be the overworld'),
]:
    path = ROOT / rel
    for index, line in enumerate(path.read_text(encoding='utf-8', errors='ignore').splitlines(), start=1):
        if needle in line:
            print(f'  {rel}:{index}  {line.strip()[:100]}')
