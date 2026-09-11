# Parity-critical Domains, Differential Strategy, Dependency Policy (P00-07 / P00-08 / P00-09)

Date: 2026-09-10.

## 1. Behaviorally critical Vanilla parity domains (P00-07)

Ranked by risk = (player-visible wrongness) × (locks architecture early). Each row names the phase that first proves it.

| # | Domain | Why critical | First proof | Evidence form |
|---|---|---|---|---|
| 1 | Protocol wire compat (handshake→login→config→play, framing, compression, keepalive) | Nothing else is testable without a real client joining | P02-14 E2E | Real 26.1.2 client join + malformed-packet suite + golden fixtures |
| 2 | Anvil/region + NBT round-trip incl. 26.1 layout | World loss is unforgivable; layout break at 26.1 is confirmed | P03-14/15 | Restart-preservation + corruption tests + vanilla-world load |
| 3 | Chunk load/stream + view-distance fan-out | Directly gates playability + 20 TPS | P04-03/16 | E2E roam + TPS trace |
| 4 | Movement authority + collision | Cheats/teleport bugs destroy survival trust | P04-06/07 | Server-reconcile tests + adversarial packets |
| 5 | Block break/place validation + block-state store | Dupe/ghost blocks = economy-breaking | P04-08/09/10 | Validation matrix + rollback check |
| 6 | Inventory transactions (server-authoritative) | Dupes live here | P06-01/02/15 | Adversarial click/shift tests |
| 7 | Entity lifecycle + damage/invuln + pickup | Death-item-loss rage + tick-order bugs | P05-03/06/08 | Determinism scenarios |
| 8 | Tick ordering determinism (same seed+inputs+ticks → same state) | Required by AGENTS.md §3.6; chaos otherwise | P05-02/17 | Regression scenarios with normalized-state compare |
| 9 | Redstone update scheduling + power propagation | Ordering-sensitive, combinatorial | P06-09/10/11/16 | Golden + differential circuits |
| 10 | Commands/permissions/selectors | Privilege bypass = security hole | P07-02/04/05 | Permission matrix tests |
| 11 | Registries/tags/data-driven loading | Wrong IDs corrupt everything downstream | P04-01/P07-03 | Registry golden tests vs vanilla datapack |
| 12 | Worldgen seed pipeline (existing-world-first) | Must not corrupt real worlds while parity grows | P07-13/17 | Same-seed chunk compare vs vanilla (bounded area) |
| 13 | Save barrier + graceful shutdown + backup/restore | Crash-loss window defines ops trust | P08-03/05 | Kill-during-save tests |
| 14 | Rate/resource limits + hostile-input rejection | Public-internet survival | P08-06/07 | Fuzz + exhaustion drills |

Deferred deliberately: perfect worldgen parity, Data Pack functions/loot full semantics, resource-pack asset parsing (URL+hash contract only), Bukkit compat (never), plugin API (concept only, P09-12).

## 2. Differential-testing strategy + baseline harness shape (P00-08)

Principles (AGENTS.md §12): compare **semantic state, never incidental bytes**. Every trace records: seed/state, inputs, tick count, outputs, normalized state, first divergence, classification (bug / missing feature / intentional+ADR).

Levels, cheapest first:

1. **Golden fixtures** (checked in): handshake bytes, status JSON, LoginSuccess→FinishConfig→JoinGame sequence skeletons, NBT samples, region header samples. Harness: `test-support` crate (P01-10) with `fixtures/` + byte-compare helpers. First use P02-11.
2. **Property/fuzz**: VarInt/frame/NBT parsers (`proptest`-style + hostile corpus: non-terminating VarInt, oversize, decomp-bomb, bad UTF-8, traversal paths). First use P02-12/P03-04.
3. **Restart/corruption**: save → kill -9 → load → normalized-state equal; bit-flip region/NBT → safe error, no panic. First use P03-14/15.
4. **E2E scripted client**: minimal test client (P02-13) drives join/move/break/place/inventory/death; asserts server state, not packet bytes. First use P02-14, grows through P04-16.
5. **Differential vs trusted 26.1.2 baseline**: same seed + scripted inputs + N ticks → capture normalized state (positions, health, block deltas, inventory) on both sides → first-divergence report. Baseline = vanilla 26.1.2 server (operator-run, offline mode) + our packet captures; reference clones are NOT the oracle (AGENTS.md §4). Harness shape: `tools/difftest` (deferred to P05-17/P07-19; shape defined now: `record/ | replay/ | normalize/ | compare/` subcommands, JSONL traces).
6. **Benchmarks with perf contract fields** (AGENTS.md §13): hw/ram, cpu/os/kernel, toolchain, SHA, profile, workload, warmup, TPS, MSPT p50/p95/p99, CPU, RSS, alloc hotspots, storage/net where relevant. Baselines start P04-18, gate P08-13/16.

## 3. Initial dependency / license policy (P00-09, binding proposal → ratified in ADR-0001 §8)

- Runtime (Phase 01): `tokio` (net/tasks), `serde`+`serde_json`, `toml`, `tracing`+`tracing-subscriber`, `thiserror`, `bytes`. All MIT/Apache-2.0. Pin exact versions in `Cargo.lock` (committed); toolchain pinned post-validation (`rust-toolchain.toml`, P01-02) — current dev toolchain observed: `rustc 1.98.1 / cargo 1.98.1 / stable-x86_64-pc-windows-msvc`.
- Compression (Phase 03 decision point): `flate2` (zlib/gzip, MINIZ) vs `ruzstd`/others — decide by license + aarch64 perf evidence, not preference.
- Dev: fuzz/property + `criterion`-style benches + `cargo deny`-equivalent license check wired in CI (P01-03).
- Forbidden without owner + legal review: GPL/AGPL/SSPL-licensed crates; vendored reference code; reference-extracted Mojang data assets.
- Every adoption adds a row to `docs/legal/third-party.md` (name, version, license, purpose, review date).
