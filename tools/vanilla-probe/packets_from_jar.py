#!/usr/bin/env python3
"""Extract protocol-775 packet ids from the vanilla jar, from BYTECODE instructions.

Precise method: inside a protocol builder's static initialiser, every packet is
registered by exactly one `getstatic GamePacketTypes.<NAME>` immediately followed
(possibly after `dup`/`ldc`/`invokedynamic` codec materialisation) by
`invokevirtual ProtocolInfoBuilder.addPacket`. So the ordered list of those
`getstatic` INSTRUCTIONS (not comment text) in the lambda body is the id table.

This script prints the raw instruction sequence so the result can be reviewed,
and writes TSVs.

Local research tool (not part of the product crates).
"""

import os
import re
import subprocess
import sys
import zipfile

TARGETS = [
    ("game", "net/minecraft/network/protocol/game/GameProtocols.class"),
    ("configuration", "net/minecraft/network/protocol/configuration/ConfigurationProtocols.class"),
    ("login", "net/minecraft/network/protocol/login/LoginProtocols.class"),
    ("status", "net/minecraft/network/protocol/status/StatusProtocols.class"),
    ("handshake", "net/minecraft/network/protocol/handshake/HandshakeProtocols.class"),
]

INSTR = re.compile(r"^\s*(\d+): (\w+)\s*(.*)$")


def javap(path):
    return subprocess.run(
        ["javap", "-p", "-c", "-constants", path],
        capture_output=True, text=True, check=False,
    ).stdout


def method_bodies(text):
    header = re.compile(r"^  ([^\s].*?\([^)]*\));\s*$", re.M)
    marks = list(header.finditer(text))
    for i, m in enumerate(marks):
        end = marks[i + 1].start() if i + 1 < len(marks) else len(text)
        yield m.group(1).strip(), text[m.end():end]


def main():
    jar, workdir, outdir = sys.argv[1], sys.argv[2], sys.argv[3]
    os.makedirs(workdir, exist_ok=True)
    os.makedirs(outdir, exist_ok=True)

    for state, entry in TARGETS:
        class_file = os.path.join(workdir, entry.replace("/", "_"))
        with zipfile.ZipFile(jar) as zf, zf.open(entry) as src, open(class_file, "wb") as dst:
            dst.write(src.read())
        text = javap(class_file)

        per_direction = {}
        for name, body in method_bodies(text):
            sequence = []
            add_packets = 0
            for line in body.splitlines():
                m = INSTR.match(line)
                if not m:
                    continue
                op, operand = m.group(2), m.group(3)
                if op == "getstatic":
                    ref = re.search(r"(\w+)\.(\w+):", operand)
                    if ref is None:
                        continue
                    const = ref.group(2)
                    if const.startswith(("SERVERBOUND_", "CLIENTBOUND_")) or const.endswith(
                        "CLIENT_INTENTION"
                    ):
                        sequence.append(const)
                elif op == "invokevirtual" and "addPacket" in operand:
                    add_packets += 1
            for direction in ("SERVERBOUND_", "CLIENTBOUND_"):
                names = [n for n in sequence if n.startswith(direction)]
                if len(names) > len(per_direction.get(direction, [])):
                    per_direction[direction] = names
                    per_direction[direction + "_method"] = name
                    per_direction[direction + "_addpacket"] = add_packets
            if state == "handshake" and sequence:
                per_direction["SERVERBOUND_"] = sequence
                per_direction["SERVERBOUND__method"] = name
                per_direction["SERVERBOUND__addpacket"] = add_packets

        for direction in ("SERVERBOUND_", "CLIENTBOUND_"):
            names = per_direction.get(direction)
            if not names:
                continue
            key = direction[:-1].lower()
            print(f"\n### {state} {key}: {len(names)} packets "
                  f"(from {per_direction[direction + '_method']})")
            for i, n in enumerate(names):
                print(f"{i:3d}  {n}")
            lines = [
                f"# {state} {key} — protocol 775 packet ids.",
                "# Source: official 26.1.2 server jar; order of `getstatic PacketTypes.<NAME>`",
                "# instructions in the protocol builder's static initialiser, which is the",
                "# registration order ProtocolInfoBuilder turns into ids. Method + jar hash are",
                "# recorded in docs/research/provenance.md.",
            ]
            lines += [f"{i}\t{n.split('_', 1)[1].lower()}\t{n}" for i, n in enumerate(names)]
            with open(os.path.join(outdir, f"{state}_{key}.tsv"), "w",
                      encoding="utf-8", newline="\n") as fh:
                fh.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
