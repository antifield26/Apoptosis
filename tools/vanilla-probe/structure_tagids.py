"""What NBT tag ids do `size`, `pos` and `state` actually use in the 26.1.2 pack?

The first census parsed tags generically and so could not tell a `TAG_List` from a
`TAG_IntArray`. This prints the exact ids, which is the difference between a
reader that works and one that refuses all 1 202 files.
"""
import gzip, os, struct
from collections import Counter

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                    "extract", "data", "minecraft", "structure")
END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BYTE_ARRAY, STRING, LIST, COMPOUND, INT_ARRAY, LONG_ARRAY = range(13)
NAMES = {END: "END", BYTE: "BYTE", SHORT: "SHORT", INT: "INT", LONG: "LONG", FLOAT: "FLOAT",
         DOUBLE: "DOUBLE", BYTE_ARRAY: "BYTE_ARRAY", STRING: "STRING", LIST: "LIST",
         COMPOUND: "COMPOUND", INT_ARRAY: "INT_ARRAY", LONG_ARRAY: "LONG_ARRAY"}

class R:
    def __init__(self, b): self.b = b; self.p = 0
    def u8(self): v = self.b[self.p]; self.p += 1; return v
    def i16(self): v = struct.unpack_from(">h", self.b, self.p)[0]; self.p += 2; return v
    def i32(self): v = struct.unpack_from(">i", self.b, self.p)[0]; self.p += 4; return v
    def i64(self): v = struct.unpack_from(">q", self.b, self.p)[0]; self.p += 8; return v
    def f32(self): v = struct.unpack_from(">f", self.b, self.p)[0]; self.p += 4; return v
    def f64(self): v = struct.unpack_from(">d", self.b, self.p)[0]; self.p += 8; return v
    def s(self):
        n = struct.unpack_from(">H", self.b, self.p)[0]; self.p += 2
        v = self.b[self.p:self.p+n].decode("utf-8", "replace"); self.p += n; return v
    def payload(self, tid):
        """Returns (value, kind) where kind describes collections exactly."""
        if tid == BYTE: return self.u8(), "byte"
        if tid == SHORT: return self.i16(), "short"
        if tid == INT: return self.i32(), "int"
        if tid == LONG: return self.i64(), "long"
        if tid == FLOAT: return self.f32(), "float"
        if tid == DOUBLE: return self.f64(), "double"
        if tid == BYTE_ARRAY:
            n = self.i32(); v = self.b[self.p:self.p+n]; self.p += n; return v, "byte_array"
        if tid == STRING: return self.s(), "string"
        if tid == LIST:
            et = self.u8(); n = self.i32()
            return [self.payload(et)[0] for _ in range(n)], "LIST<" + NAMES[et] + ">"
        if tid == COMPOUND:
            out = {}
            while True:
                t = self.u8()
                if t == END: return out, "compound"
                name = self.s()
                out[name] = self.payload(t)
        if tid == INT_ARRAY:
            n = self.i32(); return [self.i32() for _ in range(n)], "INT_ARRAY"
        if tid == LONG_ARRAY:
            n = self.i32(); return [self.i64() for _ in range(n)], "LONG_ARRAY"
        raise ValueError(tid)

def load_kinds(path):
    with open(path, "rb") as fh: raw = fh.read()
    body = gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw
    r = R(body); rid = r.u8(); rname = r.s()
    root, _ = r.payload(rid)
    return root

files = []
for dp, _d, fns in os.walk(ROOT):
    for fn in fns: files.append(os.path.join(dp, fn))
files.sort()

size_kinds = Counter()
pos_kinds = Counter()
state_kinds = Counter()
props_kinds = Counter()
name_kinds = Counter()
entities_kinds = Counter()
blocks_kinds = Counter()
palette_kinds = Counter()
palettes_kinds = Counter()
dv_kinds = Counter()
first = None

for path in files:
    root = load_kinds(path)
    # Re-parse just to get kinds: the helper above loses them at top level, so
    # walk the raw body again for the fields we care about.
    with open(path, "rb") as fh: raw = fh.read()
    body = gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw
    r = R(body); rid = r.u8(); r.s()

    # manual walk to capture kinds
    def walk(tid, into):
        if tid == COMPOUND:
            while True:
                t = r.u8()
                if t == END: return
                key = r.s()
                if t in (LIST, INT_ARRAY, LONG_ARRAY, COMPOUND):
                    if t == LIST:
                        et = r.u8(); n = r.i32()
                        into[key] = NAMES[t] + "<" + NAMES[et] + ">"
                        for _ in range(n):
                            walk(et, {})
                    elif t == INT_ARRAY:
                        n = r.i32()
                        into[key] = "INT_ARRAY"
                        for _ in range(n): r.i32()
                    elif t == LONG_ARRAY:
                        n = r.i32()
                        into[key] = "LONG_ARRAY"
                        for _ in range(n): r.i64()
                    else:
                        into[key] = "COMPOUND"
                        walk(COMPOUND, {})
                else:
                    into[key] = NAMES[t]
                    r.payload(t)
        else:
            r.payload(tid)

    top = {}
    walk(rid, top)
    for k, v in top.items():
        if k == "size": size_kinds[v] += 1
        elif k == "blocks": blocks_kinds[v] += 1
        elif k == "entities": entities_kinds[v] += 1
        elif k == "palette": palette_kinds[v] += 1
        elif k == "palettes": palettes_kinds[v] += 1
        elif k == "DataVersion": dv_kinds[v] += 1
    if first is None:
        first = (os.path.relpath(path, ROOT).replace("\\", "/"), top)

print("size      kinds:", dict(size_kinds))
print("blocks    kinds:", dict(blocks_kinds))
print("entities  kinds:", dict(entities_kinds))
print("palette   kinds:", dict(palette_kinds))
print("palettes  kinds:", dict(palettes_kinds))
print("DataVersion kinds:", dict(dv_kinds))
print()
print("first file:", first[0])
for k, v in sorted(first[1].items()):
    print("   %-12s %s" % (k, v))

# --- inner kinds: pos / state / nbt / Properties / Name ------------------
inner = {"pos": Counter(), "state": Counter(), "nbt": Counter(),
         "Properties": Counter(), "Name": Counter(), "Properties.value": Counter()}

def walk_inner(path):
    with open(path, "rb") as fh: raw = fh.read()
    body = gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw
    r = R(body); rid = r.u8(); r.s()

    def skip_value(tid):
        """Consume a value, recording nothing."""
        if tid == COMPOUND:
            while True:
                t = r.u8()
                if t == END: return
                r.s(); skip_value(t)
        elif tid == LIST:
            et = r.u8(); n = r.i32()
            for _ in range(n): skip_value(et)
        elif tid == INT_ARRAY:
            n = r.i32()
            for _ in range(n): r.i32()
        elif tid == LONG_ARRAY:
            n = r.i32()
            for _ in range(n): r.i64()
        else:
            r.payload(tid)

    def kind_of(tid):
        if tid == LIST:
            et = r.u8(); n = r.i32()
            k = "LIST<" + NAMES[et] + ">"
            for _ in range(n): skip_value(et)
            return k
        if tid == INT_ARRAY:
            n = r.i32()
            for _ in range(n): r.i32()
            return "INT_ARRAY"
        if tid == LONG_ARRAY:
            n = r.i32()
            for _ in range(n): r.i64()
            return "LONG_ARRAY"
        if tid == COMPOUND:
            k = "COMPOUND"
            skip_value(COMPOUND)
            return k
        r.payload(tid)
        return NAMES[tid]

    def walk_compound(context):
        while True:
            t = r.u8()
            if t == END: return
            key = r.s()
            if context == "block" and key in ("pos", "state", "nbt"):
                inner[key][kind_of(t)] += 1
            elif context == "palette" and key in ("Properties", "Name"):
                inner[key][kind_of(t)] += 1
            elif context == "properties" and context == "properties":
                inner["Properties.value"][kind_of(t)] += 1
            else:
                skip_value(t)

    # top level
    while True:
        t = r.u8()
        if t == END: return
        key = r.s()
        if t == LIST and key in ("blocks", "palette", "palettes"):
            et = r.u8(); n = r.i32()
            for _ in range(n):
                if et == COMPOUND:
                    walk_compound("block" if key == "blocks" else "palette")
                elif et == LIST:
                    # palettes[i] = LIST<COMPOUND>
                    iet = r.u8(); m = r.i32()
                    for _ in range(m):
                        walk_compound("palette")
                else:
                    skip_value(et)
        elif t == COMPOUND and key == "palette":
            walk_compound("palette")
        else:
            skip_value(t)

# sample broadly: every 17th file is enough for a tag-id census
for path in files[::17]:
    try:
        walk_inner(path)
    except Exception as exc:  # noqa: BLE001 - a census script, not a library
        print("  (census walk stopped early on %s: %s)" % (os.path.basename(path), exc))

print()
print("=== inner tag kinds over %d sampled files ===" % len(files[::17]))
for key, counter in inner.items():
    print("  %-18s %s" % (key, dict(counter)))

