"""Build a replayable fixture from the captured vanilla configuration payload.

## What the capture established

Two facts, both from the real vanilla server, and together they invert the approach:

1. **No registry entry carries element data** — 382 entries across 28 registries, 8 781 bytes, every `has_data`
   byte false. Vanilla sends the registry *shape* (the ids that define the numeric mapping) and nothing else.
2. The client can only be sent that because it **declared the packs it already has**, so the server omits
   content the client can read from its own jar.

So our previous design — converting the jar's data-pack JSON into NBT and shipping full element data — was not
merely unable to encode `enchantment`'s dispatch codecs. It was **sending data the protocol does not ask for**,
which is why the client tried to parse it and refused. This script confirms the client's declaration, then
packs the captured clientbound packets into one replayable blob.

## Fixture format

Deliberately trivial, because every byte in it is already the wire format:

```text
magic  b"MCRP1"
u32    packet_count
per packet:
  u32  packet id
  u32  body length
  ...  body bytes, exactly as captured
```

Replaying these verbatim means **no re-encoding at all**: no JSON, no NBT converter, no shape inference, and
therefore none of the three failure modes that produced `enchantment`, `villager_trade` and the size.
"""

import json
import struct
import sys
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
BODIES = ROOT / 'target' / 'vanilla-capture' / 'bodies'
OUT = ROOT / 'crates' / 'network' / 'src' / 'registry_data' / 'config-payload.bin'


class Reader:
    def __init__(self, data: bytes):
        self.data = data
        self.at = 0

    def varint(self) -> int:
        value = 0
        shift = 0
        while True:
            byte = self.data[self.at]
            self.at += 1
            value |= (byte & 0x7F) << shift
            if not byte & 0x80:
                return value
            shift += 7

    def string(self) -> str:
        length = self.varint()
        text = self.data[self.at:self.at + length].decode('utf-8', errors='replace')
        self.at += length
        return text


def known_packs() -> list:
    """What the client told the server it already has (serverbound config id 7)."""
    path = next(iter(sorted(BODIES.glob('*_c2s_config_7.bin'))), None)
    if path is None:
        raise SystemExit('no captured select_known_packs reply; run p10_capture_registries.py first')
    reader = Reader(path.read_bytes())
    count = reader.varint()
    packs = []
    for _ in range(count):
        packs.append((reader.string(), reader.string(), reader.string()))
    if reader.at != len(reader.data):
        raise SystemExit(f'known-pack reply has {len(reader.data) - reader.at} unread bytes')
    return packs


def main() -> int:
    packs = known_packs()
    print('the client declared these known packs:')
    for namespace, pack_id, version in packs:
        print(f'  {namespace}:{pack_id} = {version}')
    covered = any(f'{namespace}:{pack_id}' == 'minecraft:core' for namespace, pack_id, _ in packs)
    print(f'\nclient knows minecraft:core: {covered}')
    if not covered:
        # Without it, an ids-only payload would leave the client with no element data at all, so the whole
        # premise of this fixture would be wrong rather than merely unsupported.
        raise SystemExit('REFUSING: the client did not declare minecraft:core, so an ids-only registry '
                         'payload would be incorrect for it. The premise of this fixture is that it did.')

    registry_bodies = sorted(BODIES.glob('*_s2c_config_7.bin'))
    tag_bodies = sorted(BODIES.glob('*_s2c_config_13.bin'))
    if not registry_bodies:
        raise SystemExit('no captured registry bodies')

    packets = [(7, path.read_bytes()) for path in registry_bodies]
    packets += [(13, path.read_bytes()) for path in tag_bodies]

    blob = bytearray(b'MCRP1')
    blob += struct.pack('>I', len(packets))
    for packet_id, body in packets:
        blob += struct.pack('>II', packet_id, len(body))
        blob += body

    OUT.write_bytes(bytes(blob))
    print(f'\nwrote {OUT.relative_to(ROOT).as_posix()}')
    print(f'  registry packets : {len(registry_bodies)}')
    print(f'  tags packets     : {len(tag_bodies)}')
    print(f'  blob size        : {len(blob)} bytes')
    print(f'  of which tags    : {sum(len(b) for _, b in packets if _ == 13)} bytes')

    # Record the registry ids in order, because that order IS the wire id mapping and the tags index into it.
    names = []
    for _id, body in packets:
        if _id != 7:
            continue
        reader = Reader(body)
        names.append((reader.string(), reader.varint()))
    print(f'\n  registries in wire order ({len(names)}):')
    for name, count in names:
        print(f'    {name:<34} {count:>4} entries')
    (OUT.parent / 'registries.json').write_text(
        json.dumps({name: count for name, count in names}, indent=2) + '\n',
        encoding='utf-8', newline='\n',
    )
    return 0


if __name__ == '__main__':
    sys.exit(main())
