"""Decode a captured vanilla `level_chunk_with_light` to learn the light conventions.

This is P10-05's stated requirement ("golden bytes from a vanilla capture") and, more usefully, it settles the
questions that a light engine has to answer before it can produce anything:

* **how the four masks are used.** Our packet supports them but ships all four as zero, which is why a real
  client renders a dark world. The capture shows what a real server actually sets.
* **what the masks index.** Bit `i` marks light section `i - 1`, so the lowest section (below the world) is bit
  0 and the first real section is bit 1. Getting that off by one produces light in the wrong place, silently.
* **how many arrays follow.** The counts must agree with the mask population counts exactly — "the arrays
  carry no section index of their own, so a mismatch is unrecoverable rather than correctable", as our own
  packet doc already says.

Wire shape:

```text
i32 chunk_x, i32 chunk_z
heightmaps:  VarInt count, then per entry: VarInt kind, VarInt long_count, longs
data:        VarInt byte_count, bytes          (section payloads)
block entities: VarInt count, then per entry: packed xz, i16 y, VarInt type, NBT
sky_light_mask     VarInt
block_light_mask   VarInt
empty_sky_mask     VarInt
empty_block_mask   VarInt
sky light:   VarInt array_count, then per array: VarInt byte_count, bytes
block light: VarInt array_count, then per array: VarInt byte_count, bytes
```
"""

import sys
from collections import Counter
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
BODIES = ROOT / 'target' / 'vanilla-capture' / 'bodies'


class Reader:
    def __init__(self, data: bytes, at: int = 0):
        self.data = data
        self.at = at

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

    def i32(self) -> int:
        value = int.from_bytes(self.data[self.at:self.at + 4], 'big', signed=True)
        self.at += 4
        return value

    def i16(self) -> int:
        value = int.from_bytes(self.data[self.at:self.at + 2], 'big', signed=True)
        self.at += 2
        return value

    def take(self, count: int) -> bytes:
        chunk = self.data[self.at:self.at + count]
        self.at += count
        return chunk

    def remaining(self) -> int:
        return len(self.data) - self.at


def skip_nbt(reader: Reader, depth: int = 0) -> None:
    """Walk one network-NBT tag. Only enough to step over it."""
    if depth > 64:
        raise SystemExit('NBT nesting too deep')
    tag = reader.take(1)[0]
    if tag == 0:
        return
    if tag == 1:
        reader.take(1)
    elif tag == 2:
        reader.take(2)
    elif tag in (3, 5):
        reader.take(4)
    elif tag in (4, 6):
        reader.take(8)
    elif tag == 7:
        reader.take(reader.i32())
    elif tag == 8:
        length = reader.data[reader.at] << 8 | reader.data[reader.at + 1]
        reader.take(2 + length)
    elif tag == 9:
        element = reader.take(1)[0]
        count = reader.i32()
        for _ in range(count):
            if element == 0:
                break
            # Re-read the element by faking its type byte.
            saved = reader.data
            reader.data = bytes([element]) + saved[reader.at:]
            reader.at = 0
            skip_nbt(reader, depth + 1)
            consumed = reader.at - 1
            reader.data = saved
            reader.at += consumed
    elif tag == 10:
        while True:
            if reader.data[reader.at] == 0:
                reader.take(1)
                break
            skip_nbt(reader, depth + 1)
    elif tag == 11:
        reader.take(4 * reader.i32())
    elif tag == 12:
        reader.take(8 * reader.i32())
    else:
        raise SystemExit(f'unknown NBT tag {tag}')


def popcount(value: int) -> int:
    count = 0
    while value:
        count += value & 1
        value >>= 1
    return count


def decode(data: bytes) -> dict:
    reader = Reader(data)
    chunk_x = reader.i32()
    chunk_z = reader.i32()

    heightmaps = []
    for _ in range(reader.varint()):
        kind = reader.varint()
        longs = reader.varint()
        heightmaps.append((kind, longs, reader.take(8 * longs)))

    section_bytes = reader.take(reader.varint())

    block_entities = 0
    file_count = reader.varint()
    for _ in range(file_count):
        reader.take(8)   # packed xz
        reader.i16()     # y
        reader.varint()  # type
        skip_nbt(reader)
        block_entities += 1

    sky_mask = reader.varint()
    block_mask = reader.varint()
    empty_sky = reader.varint()
    empty_block = reader.varint()

    sky_arrays = [len(reader.take(reader.varint())) for _ in range(reader.varint())]
    block_arrays = [len(reader.take(reader.varint())) for _ in range(reader.varint())]

    return {
        'chunk': (chunk_x, chunk_z),
        'heightmaps': heightmaps,
        'section_bytes': len(section_bytes),
        'block_entities': block_entities,
        'sky_mask': sky_mask,
        'block_mask': block_mask,
        'empty_sky': empty_sky,
        'empty_block': empty_block,
        'sky_arrays': sky_arrays,
        'block_arrays': block_arrays,
        'trailing': reader.remaining(),
    }


def main() -> int:
    files = sorted(BODIES.glob('*_s2c_play_45.bin'))
    if not files:
        print('no captured level_chunk_with_light bodies')
        return 1
    print(f'captured chunk packets: {len(files)}')
    print(f'sizes: {dict(Counter(path.stat().st_size for path in files).most_common(6))}')

    # Decode the largest, which is the one most likely to carry real light.
    target = max(files, key=lambda path: path.stat().st_size)
    print(f'\n=== decoding {target.name} ({target.stat().st_size} bytes) ===')
    try:
        result = decode(target.read_bytes())
    except Exception as error:  # noqa: BLE001 - a decode failure is a result here
        print(f'  decode failed: {type(error).__name__}: {error}')
        return 1

    print(f"  chunk           : {result['chunk']}")
    print(f"  heightmaps      : {[(kind, longs) for kind, longs, _ in result['heightmaps']]}")
    print(f"  section bytes   : {result['section_bytes']}")
    print(f"  block entities  : {result['block_entities']}")
    print(f"  sky mask        : 0x{result['sky_mask']:x}  ({popcount(result['sky_mask'])} bits)")
    print(f"  block mask      : 0x{result['block_mask']:x}  ({popcount(result['block_mask'])} bits)")
    print(f"  empty sky mask  : 0x{result['empty_sky']:x}  ({popcount(result['empty_sky'])} bits)")
    print(f"  empty block mask: 0x{result['empty_block']:x}  ({popcount(result['empty_block'])} bits)")
    print(f"  sky arrays      : {len(result['sky_arrays'])} -> {result['sky_arrays'][:6]}")
    print(f"  block arrays    : {len(result['block_arrays'])} -> {result['block_arrays'][:6]}")
    print(f"  trailing bytes  : {result['trailing']}")

    print('\n=== consistency ===')
    print(f"  sky arrays == popcount(sky mask)          : "
          f"{len(result['sky_arrays'])} == {popcount(result['sky_mask'])} "
          f"-> {len(result['sky_arrays']) == popcount(result['sky_mask'])}")
    print(f"  block arrays == popcount(block mask)      : "
          f"{len(result['block_arrays'])} == {popcount(result['block_mask'])} "
          f"-> {len(result['block_arrays']) == popcount(result['block_mask'])}")
    print(f"  empty sky is a subset of sky mask         : "
          f"{(result['empty_sky'] & ~result['sky_mask']) == 0}")
    print(f"  empty block is a subset of block mask     : "
          f"{(result['empty_block'] & ~result['block_mask']) == 0}")

    if result['sky_arrays']:
        non_zero = sum(1 for size in result['sky_arrays'] if size)
        print(f"  sky arrays with a non-zero byte count     : {non_zero} of {len(result['sky_arrays'])}")
    return 0


if __name__ == '__main__':
    sys.exit(main())
