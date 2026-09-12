#!/usr/bin/env python3
"""Compress the vanilla block-state id dump into a compact, self-verifying table.

The full dump (`block_states.tsv`, 2.6 MB) lists every state explicitly. States of
one block are contiguous in id order and follow the cartesian product of the
block's properties, so the whole table fits in ~1/20th of the space as:

    <block name>\t<first state id>\t<state count>\t<prop=values;prop=values...>

Property order matters: it is the order the properties appear in the block's
state definition, which the dump lists in id order. This script derives the axis
order from the full dump and then **verifies** that the mixed-radix expansion
reproduces every single id exactly; it refuses to write the compact file if even
one state disagrees.

Local research tool (not part of the product crates).
"""

import sys
from collections import OrderedDict


def load_full(path):
    """Return [(state id, block name, {property: value})] in file order."""
    rows = []
    for line in open(path, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        ident, name, props = line.rstrip("\n").split("\t")
        parsed = OrderedDict()
        if props != "-":
            for pair in props.split(","):
                key, value = pair.split("=", 1)
                parsed[key] = value
        rows.append((int(ident), name, parsed))
    return rows


def main():
    src, dest = sys.argv[1], sys.argv[2]
    rows = load_full(src)

    blocks = OrderedDict()
    for ident, name, props in rows:
        entry = blocks.setdefault(name, {"first": ident, "states": [], "axes": []})
        entry["states"].append((ident, props))

    out = [
        "# Vanilla 26.1.2 block registry: state id layout.",
        "#",
        "# Generated from the official server jar by booting its own registry",
        "# (target/vanilla-26.1.2/reports/DumpRegistries.java) and compressing the",
        "# full 29873-state dump with target/vanilla-26.1.2/compact_blocks.py, which",
        "# verifies that every id is reproduced exactly before writing this file.",
        "#",
        "# Format: <block name>\\t<first state id>\\t<state count>\\t<axis list>",
        "#   axis list = prop=value|value|value;prop=value|...   ('-' when stateless)",
        "# A state's id is first + sum(index_i * stride_i) over the axes, in listed",
        "# order, where the LAST axis varies fastest (mixed radix).",
    ]

    verified = 0
    for name, entry in blocks.items():
        states = entry["states"]
        first = states[0][0]
        count = len(states)
        # State ids must be contiguous for the mixed-radix form to work.
        assert [s[0] for s in states] == list(range(first, first + count)), name

        if count == 1 and not states[0][1]:
            out.append(f"{name}\t{first}\t1\t-")
            verified += 1
            continue

        # Axis order = property declaration order; value order = order of first
        # appearance along the id sequence.
        axes = []
        for key in states[0][1]:
            values = []
            for _, props in states:
                value = props[key]
                if value not in values:
                    values.append(value)
            axes.append((key, values))

        # Verify mixed radix with the last axis varying fastest.
        for offset, (ident, props) in enumerate(states):
            index = 0
            stride = 1
            for key, values in reversed(axes):
                index += values.index(props[key]) * stride
                stride *= len(values)
            assert index == offset, (name, ident, offset, index)
            verified += 1
        assert stride == count, (name, stride, count)

        axis_text = ";".join(f"{k}=" + "|".join(v) for k, v in axes)
        out.append(f"{name}\t{first}\t{count}\t{axis_text}")

    with open(dest, "w", encoding="ascii", newline="\n") as fh:
        fh.write("\n".join(out) + "\n")
    print(f"{len(blocks)} blocks, {verified} states verified -> {dest}")


if __name__ == "__main__":
    main()
