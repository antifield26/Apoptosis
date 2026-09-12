#!/usr/bin/env python3
"""Evidence tool: dump an Anvil region file header and decode one chunk.

Local throwaway research helper (not part of the product crates). Independently
written from the Anvil format description.
"""

import struct
import sys
import zlib
import gzip

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from nbt_dump import Reader, read_payload, print_tree, TAG_NAMES

SECTOR = 4096


def header(data):
    if len(data) < 2 * SECTOR:
        print(f"!! file shorter than 8KiB header: {len(data)} bytes")
    offsets = struct.unpack(">1024I", data[0:SECTOR])
    stamps = struct.unpack(">1024i", data[SECTOR : 2 * SECTOR])
    present = [(i, o) for i, o in enumerate(offsets) if o != 0]
    print(f"# region file {len(data)} bytes = {len(data) / SECTOR:.2f} sectors")
    print(f"# present chunk slots: {len(present)}")
    for i, o in present:
        sector = o >> 8
        count = o & 0xFF
        print(
            f"  slot {i:4d} (x={i % 32:2d}, z={i // 32:2d}) sector={sector} count={count} "
            f"byte_offset={sector * SECTOR} mtime={stamps[i]}"
        )
    return offsets


def decode_chunk(data, slot):
    offsets = struct.unpack(">1024I", data[0:SECTOR])
    o = offsets[slot]
    if o == 0:
        raise SystemExit(f"slot {slot} empty")
    sector = o >> 8
    count = o & 0xFF
    start = sector * SECTOR
    length = struct.unpack(">I", data[start : start + 4])[0]
    comp = data[start + 4]
    print(f"# slot {slot}: sector={sector} sectors={count} length_field={length} compression_id={comp}")
    payload = data[start + 5 : start + 4 + length]
    print(f"# payload bytes present: {len(payload)}")
    if comp == 1:
        raw = gzip.decompress(payload)
    elif comp == 2:
        raw = zlib.decompress(payload)
    elif comp == 3:
        raw = payload
    else:
        raise SystemExit(f"unsupported compression {comp}")
    print(f"# decompressed {len(raw)} bytes")
    return raw


def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    offsets = header(data)
    slots = [int(a) for a in sys.argv[2:]] or [
        i for i, o in enumerate(offsets) if o != 0
    ]
    for slot in slots[:2]:
        raw = decode_chunk(data, slot)
        r = Reader(raw)
        root_tag = r.u8()
        root_name = r.u16()
        r.pos -= 2
        root_name = r.string()
        print(f"# root tag={TAG_NAMES[root_tag]} name={root_name!r}")
        tree = read_payload(r, root_tag)
        if isinstance(tree, dict) and "sections" in tree:
            sections = tree["sections"]
            tree = dict(tree)
            tree["sections"] = f"<{len(sections)} sections>"
            for s in sections:
                y = s.get("Y")
                bs = s.get("block_states", {})
                pal = bs.get("palette", [])
                print(
                    f"  section Y={y} keys={sorted(s.keys())} "
                    f"block_palette={len(pal)} data_len={len(bs.get('data', []))}"
                )
                print(f"    palette[0..3]={pal[:3]}")
            if sections:
                print("  --- full first section ---")
                print_tree("section0", sections[0], 1)
        print_tree("root", tree)


if __name__ == "__main__":
    main()
