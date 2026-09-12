#!/usr/bin/env python3
"""Ground-truth NBT reader for vanilla artifact inspection (evidence tool).

Local, throwaway research helper: reads a gzipped or raw NBT file and prints a
tag tree. Written independently from the NBT specification; not part of the
product crates.
"""

import gzip
import struct
import sys
import zlib

TAG_NAMES = {
    0: "End",
    1: "Byte",
    2: "Short",
    3: "Int",
    4: "Long",
    5: "Float",
    6: "Double",
    7: "ByteArray",
    8: "String",
    9: "List",
    10: "Compound",
    11: "IntArray",
    12: "LongArray",
}


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def take(self, n):
        if self.pos + n > len(self.data):
            raise EOFError(f"need {n} bytes at {self.pos}, have {len(self.data) - self.pos}")
        out = self.data[self.pos : self.pos + n]
        self.pos += n
        return out

    def u8(self):
        return self.take(1)[0]

    def i16(self):
        return struct.unpack(">h", self.take(2))[0]

    def u16(self):
        return struct.unpack(">H", self.take(2))[0]

    def i32(self):
        return struct.unpack(">i", self.take(4))[0]

    def i64(self):
        return struct.unpack(">q", self.take(8))[0]

    def f32(self):
        return struct.unpack(">f", self.take(4))[0]

    def f64(self):
        return struct.unpack(">d", self.take(8))[0]

    def string(self):
        return self.take(self.u16()).decode("utf-8", "replace")


def read_payload(r, tag):
    if tag == 1:
        return struct.unpack(">b", r.take(1))[0]
    if tag == 2:
        return r.i16()
    if tag == 3:
        return r.i32()
    if tag == 4:
        return r.i64()
    if tag == 5:
        return r.f32()
    if tag == 6:
        return r.f64()
    if tag == 7:
        return read_byte_array(r)
    if tag == 8:
        return r.string()
    if tag == 9:
        elem = r.u8()
        count = r.i32()
        return [read_payload(r, elem) for _ in range(count)]
    if tag == 10:
        out = {}
        while True:
            t = r.u8()
            if t == 0:
                return out
            name = r.string()
            out[name] = read_payload(r, t)
    if tag == 11:
        return [r.i32() for _ in range(r.i32())]
    if tag == 12:
        return [r.i64() for _ in range(r.i32())]
    raise ValueError(f"unknown tag {tag}")


def read_byte_array(r):
    n = r.i32()
    return list(r.take(n))


def load(path):
    raw = open(path, "rb").read()
    for name, fn in (
        ("gzip", lambda b: gzip.decompress(b)),
        ("zlib", lambda b: zlib.decompress(b)),
        ("raw", lambda b: b),
    ):
        try:
            return name, fn(raw)
        except Exception:
            continue
    raise SystemExit("could not decompress")


def main():
    path = sys.argv[1]
    how, data = load(path)
    r = Reader(data)
    root_tag = r.u8()
    root_name = r.string()
    print(f"# {path} ({how}, {len(data)} bytes) root={TAG_NAMES[root_tag]} name={root_name!r}")
    print_tree("", read_root(r, root_tag))


def read_root(r, tag):
    if tag == 7:
        return read_byte_array(r)
    return read_payload(r, tag)


def print_tree(name, value, indent=0):
    pad = "  " * indent
    label = f"{pad}{name}: " if name else pad
    if isinstance(value, dict):
        print(f"{label}Compound({len(value)})")
        for k, v in value.items():
            print_tree(k, v, indent + 1)
    elif isinstance(value, list):
        if value and not isinstance(value[0], (dict, list)):
            preview = value if len(value) <= 12 else value[:12] + ["..."]
            print(f"{label}List({len(value)}) {preview}")
        else:
            print(f"{label}List({len(value)})")
            for i, v in enumerate(value[:8]):
                print_tree(f"[{i}]", v, indent + 1)
    elif isinstance(value, str):
        print(f"{label}{value!r}")
    else:
        print(f"{label}{value!r}")


if __name__ == "__main__":
    main()
