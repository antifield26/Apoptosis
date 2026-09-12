"""Measure the 26.1.2 structure-template pack (P07-16 evidence).

Reads every file under extract/data/minecraft/structure/, parses the
gzip-compressed NBT by hand (no third-party deps), and prints a census.
"""
import gzip
import os
import struct
import sys
from collections import Counter

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                    "extract", "data", "minecraft", "structure")

END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BYTE_ARRAY, STRING, LIST, COMPOUND, INT_ARRAY, LONG_ARRAY = range(13)

class R:
    def __init__(self, b):
        self.b = b
        self.p = 0
    def u8(self):
        v = self.b[self.p]; self.p += 1; return v
    def i16(self):
        v = struct.unpack_from(">h", self.b, self.p)[0]; self.p += 2; return v
    def i32(self):
        v = struct.unpack_from(">i", self.b, self.p)[0]; self.p += 4; return v
    def i64(self):
        v = struct.unpack_from(">q", self.b, self.p)[0]; self.p += 8; return v
    def f32(self):
        v = struct.unpack_from(">f", self.b, self.p)[0]; self.p += 4; return v
    def f64(self):
        v = struct.unpack_from(">d", self.b, self.p)[0]; self.p += 8; return v
    def s(self):
        n = struct.unpack_from(">H", self.b, self.p)[0]; self.p += 2
        v = self.b[self.p:self.p + n].decode("utf-8", "replace"); self.p += n
        return v
    def payload(self, tid):
        if tid == BYTE: return self.u8()
        if tid == SHORT: return self.i16()
        if tid == INT: return self.i32()
        if tid == LONG: return self.i64()
        if tid == FLOAT: return self.f32()
        if tid == DOUBLE: return self.f64()
        if tid == BYTE_ARRAY:
            n = self.i32(); v = self.b[self.p:self.p + n]; self.p += n; return v
        if tid == STRING: return self.s()
        if tid == LIST:
            et = self.u8(); n = self.i32()
            return [self.payload(et) for _ in range(n)]
        if tid == COMPOUND:
            out = {}
            while True:
                t = self.u8()
                if t == END: return out
                name = self.s()
                out[name] = self.payload(t)
        if tid == INT_ARRAY:
            n = self.i32(); return [self.i32() for _ in range(n)]
        if tid == LONG_ARRAY:
            n = self.i32(); return [self.i64() for _ in range(n)]
        raise ValueError("tag %d" % tid)


def parse(path):
    with open(path, "rb") as fh:
        raw = fh.read()
    magic = raw[:2]
    body = gzip.decompress(raw) if magic == b"\x1f\x8b" else raw
    r = R(body)
    root_id = r.u8()
    root_name = r.s()
    root = r.payload(root_id)
    return magic, root_name, root, len(raw), len(body)


def main():
    files = []
    for dirpath, _dirnames, filenames in os.walk(ROOT):
        for fn in filenames:
            files.append(os.path.join(dirpath, fn))
    files.sort()

    dataversions = Counter()
    palette_names = set()
    has_entities = []
    has_properties = []
    sizes = []
    keys = Counter()
    block_keys = Counter()
    palette_keys = Counter()
    bad_magic = []
    empty_size = []
    max_state_index_usage = []
    n_blocks_total = 0
    n_palette_total = 0
    entity_counts = Counter()
    not_gzip = 0
    uncompressed_max = 0
    compressed_max = 0
    name_lengths = []
    palettes_with_air = 0
    sample = None

    for path in files:
        rel = os.path.relpath(path, ROOT).replace("\\", "/")
        magic, root_name, root, clen, ulen = parse(path)
        if magic != b"\x1f\x8b":
            not_gzip += 1
        uncompressed_max = max(uncompressed_max, ulen)
        compressed_max = max(compressed_max, clen)
        name_lengths.append(len(root_name))
        keys.update(root.keys())
        if sample is None:
            sample = (rel, root_name, sorted(root.keys()), clen, ulen)
        dv = root.get("DataVersion")
        dataversions[dv] += 1

        size = root.get("size")
        sizes.append((size[0], size[1], size[2], rel))
        if size == [0, 0, 0]:
            empty_size.append(rel)
        if size[0] <= 0 or size[1] <= 0 or size[2] <= 0:
            bad_magic.append(rel)

        palette = root.get("palette", [])
        n_palette_total += len(palette)
        air = False
        for entry in palette:
            palette_keys.update(entry.keys())
            n = entry.get("Name")
            palette_names.add(n)
            if n == "minecraft:air":
                air = True
            if "Properties" in entry:
                has_properties.append((rel, n))
        if air:
            palettes_with_air += 1

        blocks = root.get("blocks", [])
        n_blocks_total += len(blocks)
        max_state = -1
        for b in blocks:
            block_keys.update(b.keys())
            max_state = max(max_state, b.get("state", -1))
        max_state_index_usage.append(max_state)

        ents = root.get("entities", [])
        if ents:
            has_entities.append((rel, len(ents)))
        entity_counts[len(ents)] += 1

    print("files                       :", len(files))
    print("not gzip                    :", not_gzip)
    print("distinct DataVersion values :", dict(dataversions))
    print("max compressed bytes        :", compressed_max)
    print("max uncompressed bytes      :", uncompressed_max)
    print("distinct palette block names:", len(palette_names))
    print("total palette entries       :", n_palette_total)
    print("total blocks                :", n_blocks_total)
    print("palettes containing air     :", palettes_with_air)
    print("root NBT key frequency      :", dict(keys))
    print("block NBT key frequency     :", dict(block_keys))
    print("palette NBT key frequency   :", dict(palette_keys))
    print("root name lengths           :", sorted(set(name_lengths)))
    print("files with entities         :", len(has_entities), has_entities[:10])
    print("entity-count distribution   :", dict(sorted(entity_counts.items())))
    print("files with Properties       :", len(has_properties), has_properties[:10])
    print("files with a non-positive size:", len(bad_magic), bad_magic[:5])
    print("files with size [0,0,0]     :", len(empty_size), empty_size[:5])

    sizes.sort(key=lambda s: (s[0] * s[1] * s[2], s[0], s[1], s[2]))
    print("smallest 5 by volume        :", sizes[:5])
    print("largest 5 by volume         :", [s for s in sizes[-5:]])
    print("largest by axis x           :", max(s[0] for s in sizes))
    print("largest by axis y           :", max(s[1] for s in sizes))
    print("largest by axis z           :", max(s[2] for s in sizes))
    xs = sorted({s[0] for s in sizes}); ys = sorted({s[1] for s in sizes}); zs = sorted({s[2] for s in sizes})
    print("distinct x sizes            :", len(xs), xs[:8], "...", xs[-8:])
    print("distinct y sizes            :", len(ys), ys[:8], "...", ys[-8:])
    print("distinct z sizes            :", len(zs), zs[:8], "...", zs[-8:])
    print("max state index used        :", max(max_state_index_usage))
    print("sample file                 :", sample)

    # Top-level directories
    dirs = Counter(rel.split("/")[0] for rel in
                   (os.path.relpath(p, ROOT).replace("\\", "/") for p in files))
    print("top-level dirs (%d)         :" % len(dirs), dict(sorted(dirs.items())))

    # Palette sizes
    return 0


if __name__ == "__main__":
    sys.exit(main())
