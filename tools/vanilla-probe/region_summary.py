#!/usr/bin/env python3
"""Evidence tool: summarize every chunk in a region file (fields, statuses, sizes).

Local throwaway research helper (not part of the product crates).
"""

import gzip
import struct
import sys
import zlib
from collections import Counter

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from nbt_dump import Reader, read_payload

SECTOR = 4096


def load_chunk(data, slot):
    o = struct.unpack(">1024I", data[0:SECTOR])[slot]
    if o == 0:
        return None
    start = (o >> 8) * SECTOR
    length = struct.unpack(">I", data[start : start + 4])[0]
    comp = data[start + 4]
    payload = data[start + 5 : start + 4 + length]
    raw = {1: gzip.decompress, 2: zlib.decompress, 3: lambda b: b}[comp](payload)
    r = Reader(raw)
    tag = r.u8()
    r.u16()
    return comp, len(raw), read_payload(r, tag)


def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    offsets = struct.unpack(">1024I", data[0:SECTOR])
    present = [i for i, o in enumerate(offsets) if o != 0]
    keys = Counter()
    statuses = Counter()
    comps = Counter()
    sizes = []
    sections_keys = Counter()
    heightmap_keys = Counter()
    max_sectors = 0
    for slot in present:
        o = offsets[slot]
        max_sectors = max(max_sectors, o & 0xFF)
        out = load_chunk(data, slot)
        if out is None:
            continue
        comp, size, tree = out
        comps[comp] += 1
        sizes.append(size)
        keys.update(tree.keys())
        statuses[tree.get("Status")] += 1
        heightmap_keys.update(tree.get("Heightmaps", {}).keys())
        for s in tree.get("sections", []):
            sections_keys.update(s.keys())
    print(f"# chunks={len(present)} compressions={dict(comps)} max_sectors_per_chunk={max_sectors}")
    print(f"# decompressed sizes: min={min(sizes)} max={max(sizes)} mean={sum(sizes)//len(sizes)}")
    print(f"# statuses: {dict(statuses)}")
    print("# chunk root keys (count/total):")
    for k, v in keys.most_common():
        print(f"    {k}: {v}/{len(present)}")
    print(f"# section keys: {dict(sections_keys)}")
    print(f"# heightmap keys: {dict(heightmap_keys)}")


if __name__ == "__main__":
    main()
