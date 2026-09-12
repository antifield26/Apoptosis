# Security Policy

This repository contains a network-facing Minecraft server. Assume clients are
hostile: the codebase is built and tested against that assumption, and reports
that expand the hostile-input surface are the most valuable kind.

## Supported version

`main` (HEAD). There are no tagged releases yet, so no older lines are
supported.

## How to report a vulnerability

Use **GitHub's private vulnerability reporting** for this repository
(Security → Report a vulnerability). Please do not open a public issue for
anything that is exploitable by a network peer before a fix exists.

Include: the affected commit, the packet/code path, a minimal reproduction
(script, packet trace or test), and your assessment of impact. A regression
test is the ideal report — this project turns every fixed vulnerability into
one.

## Scope — classes this server explicitly defends against

The threat model assumes hostile clients and covers, at minimum:

- malformed or non-terminating VarInt/VarLong, oversized frames and
  decompression bombs, allocation amplification;
- slow-drip connections, connection/resource exhaustion, reconnect storms;
- invalid UTF-8, invalid coordinates and quantities, inventory spoofing,
  reach/interaction abuse;
- command permission bypass, file/path traversal, malformed NBT.

Anything that lets a peer panic the process, exhaust memory/disk, corrupt a
world or escalate permissions is a vulnerability by definition. A malformed
packet must never crash the process — that is an error contract, not a
coincidence.

## Out of scope

- DDoS volumetric attacks (not a capability of this project; deploy behind
  standard network protections).
- The online-mode boundary: enabling `online_mode = true` refuses to start by
  design (no Mojang session flow is implemented), so "online mode is
  unsupported" is a documented product boundary, not a flaw — but a *crash* or
  auth bypass through that path would absolutely be in scope.
- Vanilla-parity gaps (missing features are tracked publicly in the
  [parity matrix](docs/vanilla-parity/PARITY-MATRIX.md), not as security
  issues, unless they are exploitable).

## Disclosure

Fixes land on `main` with a regression test and are noted in the changelog;
reports are credited in the fix commit unless the reporter prefers otherwise.
