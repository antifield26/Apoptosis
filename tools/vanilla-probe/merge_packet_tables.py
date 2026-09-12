#!/usr/bin/env python3
"""Merge the per-state extracted tables into the committed evidence file.

ASCII only, so the file survives any Windows/PowerShell code-page round trip.
"""

import os
import sys

STATES = ["handshake", "status", "login", "configuration", "game"]
DIRS = ["serverbound", "clientbound"]

# logical name for the single handshake packet, which has no direction prefix
HANDSHAKE_LOGICAL = {"CLIENT_INTENTION": "intention"}


def main():
    srcdir, dest = sys.argv[1], sys.argv[2]
    out = [
        "# Protocol 775 (Minecraft Java 26.1.x) packet id table - authoritative.",
        "#",
        "# Source: the official 26.1.2 server jar (server.jar sha1",
        "# 97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51). Vanilla registers packets with",
        "# ProtocolInfoBuilder.addPacket from a static initialiser, and each call takes the",
        "# next index, so the order of the `getstatic PacketTypes.<NAME>` instructions in",
        "# that initialiser IS the id table. Extracted from bytecode (not from comment",
        "# text) by target/vanilla-26.1.2/packets_from_jar.py.",
        "#",
        "# Format: <state> <direction> <id> <logical name>",
        "# Consumed by crates/protocol/tests/packet_ids.rs, which fails when a constant in",
        "# crates/protocol/src/ids.rs disagrees with this file.",
    ]
    total = 0
    for state in STATES:
        for direction in DIRS:
            path = os.path.join(srcdir, f"{state}_{direction}.tsv")
            if not os.path.exists(path):
                continue
            for line in open(path, encoding="utf-8"):
                if line.startswith("#") or not line.strip():
                    continue
                index, logical, const = line.rstrip("\n").split("\t")
                if const in HANDSHAKE_LOGICAL:
                    logical = HANDSHAKE_LOGICAL[const]
                out.append(f"{state} {direction} {index} {logical}")
                total += 1
    if not any(line.startswith("handshake ") for line in out):
        out.append("handshake serverbound 0 intention")
        total += 1
    with open(dest, "w", encoding="ascii", newline="\n") as fh:
        fh.write("\n".join(out) + "\n")
    print(f"wrote {total} packet ids to {dest}")


if __name__ == "__main__":
    main()
