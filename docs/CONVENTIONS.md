# Engineering Conventions

This document is the repository's engineering contract, in one place and
self-contained. It distills the rules the project was built under — the same
rules its docs cite as "CONVENTIONS.md §N" (historical documents cite the
numbering of the original agent contract file, which used identical section
numbers).

The one-sentence version: **every claim carries evidence, and a test that
cannot fail proves nothing.**

## 1. Mission

Build a from-scratch, pure-Rust Minecraft Java Edition 26.1.2 dedicated server
(protocol 775) targeting a Raspberry Pi 5 8 GB (aarch64, Debian 13 Trixie)
with 10 concurrent Vanilla-Survival players. The product is a server
implementation, not a Bukkit/Paper compatibility layer and not a JVM-hosted
framework.

Priority order:

1. 26.1.2 protocol/client interoperability.
2. Correct Vanilla Survival semantics.
3. Vanilla-compatible world persistence.
4. Stable 20 TPS for the target 10-player workload on Raspberry Pi 5.
5. Strong automated validation and differential testing.
6. Rust-native plugin *readiness* without contaminating the core architecture
   (boundary, not API — [ADR-0005](adr/ADR-0005-plugin-boundary.md)).

## 2. Product contract

Fixed unless the owner changes them explicitly: Minecraft Java 26.1.2;
Vanilla Survival; 10 concurrent players; vanilla-compatible persistence;
redstone support eventually full; no Bukkit/Paper plugins; no embedded JVM; a
Rust-native plugin API reserved for later phases; Tokio for networking/async
I/O only; a fixed 20 TPS simulation clock; staged world generation with
eventual vanilla parity as the target; offline mode default and
online mode configurable (currently a fail-fast boundary); TOML config;
`tracing` structured logs; Rust stable pinned in `rust-toolchain.toml`;
`unsafe` Rust only when narrowly isolated, justified and documented (the
workspace currently forbids it).

## 3. Non-negotiable engineering rules

### 3.1 Evidence before claims

Never claim "Vanilla-compatible", "protocol-compatible", "safe", "20 TPS" or
"production-ready" without evidence. A compatibility claim must have at least
one of: a reproducible experiment, official/verified specification evidence, a
fixture/golden test, a differential test, or an end-to-end test.

### 3.2 Correctness before optimization

Do not optimize from intuition. Establish a reproducible baseline, profile the
workload, identify the bottleneck, make the smallest defensible change, then
re-run the benchmark and the relevant tests.

### 3.3 No fake completeness

Do not hide TODOs, stubs, placeholder return values or unsupported semantics
behind a "completed" API. Unsupported behaviour must be explicit, tracked and
documented — this repository records gaps in the
[parity matrix](vanilla-parity/PARITY-MATRIX.md) rather than papering over
them.

### 3.4 Vertical slices over giant scaffolding

Prefer an end-to-end path that actually works over unused abstraction. A new
abstraction is justified only when it simplifies a current requirement,
enables verified reuse, or establishes a boundary that is immediately
exercised. No interface exists "to reserve space" for a hypothetical plugin —
see [ADR-0005](adr/ADR-0005-plugin-boundary.md).

### 3.5 Async boundary discipline

Tokio belongs at I/O and asynchronous worker boundaries (accept, codec,
socket I/O, RCON). Gameplay and simulation code stays synchronous; bridges
cross via bounded channels and tick-boundary joins.

### 3.6 Deterministic simulation

Where practical, the same initial state + the same ordered inputs + the same
tick count produce the same normalized simulation state. Any deliberate
nondeterminism must be documented at the site.

## 4. Source-of-truth hierarchy

When behaviour is uncertain, prefer, in order:

1. Verified Minecraft 26.1.2 behaviour.
2. Official Mojang material applicable to 26.1.2.
3. Reproducible observation against a 26.1.2 baseline.
4. Local reference repositories and their tests/fixtures.
5. Community documentation.
6. Inference (labelled as such).

Paper, Pumpkin, Valence and Minestom are reference implementations, not
specifications ([NOTICE](../NOTICE)). Never choose an architecture because
"Paper does it this way" — explain what problem the design solves here.

## 5. Reference repositories

Reference clones live outside the repository (git-ignored) and are inventoried
with licence, version and file-level citations in
[research/reference-repos.md](research/reference-repos.md). Provenance of any
nontrivial idea traced to them is recorded in
[research/provenance.md](research/provenance.md). See [NOTICE](../NOTICE) for
the attribution statement.

## 6. Licensing and provenance

The project is MIT ([ADR-0006](adr/ADR-0006-licensing.md)). Rules that survive
every other decision:

- **Clean-room**: no source from any reference clone enters this repo — no
  copy, no translation, no vendoring. Behaviour, facts and tests may inform
  us; code must be independently written.
- Every third-party dependency gets a licence check and a row in
  [legal/third-party.md](legal/third-party.md) at adoption time; the
  `cargo deny` gate enforces the allow-list.
- GPL/AGPL-licensed material is never vendored without an explicit owner and
  legal decision (none exists).

## 7. Repository shape

The crate layout is the logical boundary list in
[ADR-0001](adr/ADR-0001-system-architecture.md) §D-01: `protocol`, `network`,
`core`, `registry`, `nbt`, `persistence`, `world`, `entity`, `simulation`,
`command`, `server`, `test-support` under `crates/`, plus `apps/server` — as
amended by [ADR-0007](adr/ADR-0007-post-p12-deltas.md), which records the four
grown boundaries (`container`, `redstone`, `worldgen`, `data`) and the second
binary (`apps/capture-rig`). These
are boundaries, not permission to create empty crates — a crate exists when it
owns real behaviour. The one deliberate refinement (data loading split from
the registry) is [ADR-0004](adr/ADR-0004-data-loading-and-registry-split.md).

## 8. Concurrency contract

Every cross-thread boundary documents: owner of the data, synchronization
primitive, ordering guarantees, queue/backpressure behaviour, failure
semantics, shutdown behaviour. The default model:

```text
TCP/Tokio -> decoded/validated input events -> tick scheduler
-> 20 TPS simulation -> state changes / outbound events
-> packet scheduler -> Tokio socket
```

One tick thread runs the phases serially (network → scheduled ticks →
entities → players → block entities → broadcast, `PHASE_ORDER` in
`crates/simulation/src/phase.rs`); workers (chunk gen, compression,
persistence) join at tick boundaries so completion never reorders gameplay.

## 9. Error contract

Never use `panic` as routine control flow. Distinguish: malformed protocol
input, player-caused invalid action, recoverable operational failure,
persistent-data corruption, programmer invariant violation. **A malformed
client packet must not crash the process.**

## 10. Security baseline

Assume clients are hostile. Defend against at least: malformed or
non-terminating VarInt/VarLong, oversized packets, decompression bombs,
allocation amplification, invalid UTF-8, invalid coordinates and quantities,
inventory spoofing, reach/interaction abuse, command permission bypass,
file/path traversal, connection/resource exhaustion, malformed NBT, reconnect
storms. Every security bug that reaches a fix becomes a regression test.
Report vulnerabilities per [SECURITY.md](../SECURITY.md) — never in a public
issue.

## 11. Testing contract

A feature is not done without proportionate automated verification. The
preferred evidence stack (the levels used throughout the test matrix):

- **L1** unit tests for local invariants;
- **L2** property/fuzz tests for parsers and hostile input;
- **L3** integration tests for subsystem interaction;
- **L4** golden/fixture tests for stable representations;
- **L5** end-to-end client/server tests;
- **L6** differential tests against a trusted 26.1.2 baseline;
- **L7** (house addition) a regression test for every discovered bug.

Never delete or weaken a failing test merely to get a green build. A
falsification check ("disable the mechanism, watch the test fail") is the
standard of proof for a load-bearing assertion.

## 12. Differential testing contract

Compare semantic state, not incidental implementation details. A differential
trace identifies: initial world seed/state, player/entity inputs, tick count,
captured outputs, normalized state, first divergence, and a classification —
bug / missing feature / intentional divergence. Intentional divergences carry
a note in the [parity matrix](vanilla-parity/PARITY-MATRIX.md). Since P10 the
same contract covers real clients via `apps/capture-rig` (jar-gated
`--ignored` suites), and since P11–12 the convertible-data joins
(`RecipeBook`→crafting/smelting tables, loot refusal ladder) count skipped
constructs instead of dropping them silently.

## 13. Performance contract

The Pi 5 is the production target, not a post-release optimization target.
Every benchmark record includes: hardware model and RAM, CPU/OS/kernel,
toolchain, commit SHA, build profile, workload description, duration and
warmup, TPS, MSPT p50/p95/p99, CPU utilisation, RSS, and storage/network
figures where relevant. Do not hard-code numeric thresholds before a
representative workload establishes them. Records live in
[performance/BENCHMARK-BASELINE.md](performance/BENCHMARK-BASELINE.md); the
20 TPS acceptance procedure and its verdict are there too.

## 14. Workflow and review checklist

Inspect → plan → locate existing code → implement the smallest coherent
change → focused test → broader tests → diff review → docs/ADR → report
evidence. For every bug: reproduce → minimize → classify → fix → regression
test → re-run the baseline. For every performance change: baseline → profile
→ hypothesis → patch → correctness tests → benchmark → compare → retain or
revert.

Before calling work done, review the diff for: accidental API expansion,
TODO/stub leakage, reachable panic paths, concurrency hazards, compatibility
regressions, security issues, unnecessary allocations/copies.

## 15. Definition of done

A task is done only when: implementation exists (no unstated stubs); the
affected target builds; relevant tests pass; new behaviour has regression
coverage or a documented reason why coverage is impossible; no known
panic/`unwrap` path is reachable from hostile network input without an
invariant proof; compatibility claims have evidence; dependency/provenance
implications are recorded; documentation is updated when behaviour or
architecture changed; `cargo fmt --check` and `cargo clippy -D warnings` pass
at the workspace scope; the working tree contains no unrelated changes; and
the report states exact commands and outcomes.

A phase is done only when its exit gate passes and the report cites the
evidence — the phase gates and their verdicts are part of the archived
history (tag `phase-09-final` in git).
