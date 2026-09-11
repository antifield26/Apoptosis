# Phase 00 Report — Research, Repository, Protocol, License & Architecture Audit

Date: 2026-09-10. Agent operating protocol followed (AGENTS.md §14): read contract prompts,
inspected `git status` (branch `master`, **no commits at Phase 00 (the initial commit landed 2026-09-11 as `b7b1c99`) yet**), searched before creating, smallest coherent change (docs only).

> **Post-hoc status note (added 2026-09-11, audit round):** this report is the
> record of Phase 00 as executed. Two items below have since changed and are
> annotated rather than rewritten, so the original evidence trail stays intact:
>
> - **R-03 (26.1 DataVersion) is CLOSED.** A vanilla 26.1.2 run on this host
>   measured `DataVersion = 4790` and `version = 19133`
>   (`docs/research/protocol-baseline.md` §2; implemented and enforced in
>   `mc-persistence`). Where this file says "OPEN" or "exact TBD", the resolved
>   value supersedes it.
> - **R-01 (reference SHAs) is still open by nature.** The clones have no `.git`,
>   so no SHA exists to record; re-cloning is an owner action.
> - The repository still has **zero commits**. CI (`.github/workflows/ci.yml`) is
>   therefore unexecuted, and reproducibility is not yet demonstrable from VCS.
>   The Phase 01 report's phrase "`Cargo.lock` is committed" meant "intended to be
>   committed"; it is not, yet.
>
> Independent audit results and the fixes they produced are recorded in
> `docs/phases/AUDIT-01-FINDINGS.md`.

## Inputs executed (TASK-INDEX P00-01..P00-10)

| Task | Done | Output |
|---|---|---|
| P00-01 inventory clones/paths/commits | ✅ | `docs/research/reference-repos.md` — 4 clones inventoried; `.git` absent in all → SHA/branch UNKNOWN (recorded, not guessed) |
| P00-02 license/provenance audit | ✅ | `docs/legal/third-party.md` (Pumpkin GPL-3.0 / Paper GPLv3 / Valence MIT / Minestom Apache-2.0 + clean-room policy); `docs/research/provenance.md` seeded |
| P00-03 26.1.2 protocol/version baseline | ✅ | `docs/research/protocol-baseline.md` §1–3: **proto 775, Display "26.1", 26.1.2 shares 775**; 26.2=776/data 4903; 26.1 DataVersion exact TBD (R-03) |
| P00-04 states/packet families/lifecycle | ✅ | `protocol-baseline.md` §4: 5-state + Transfer machine, per-state tables, offline-login order, fast-path rule |
| P00-05 arch/threading compare | ✅ | `docs/research/architecture-comparison.md` §1: adopt Pumpkin-shape tick thread + Tokio edges + stage-serial/batch-parallel |
| P00-06 world/chunk/persistence compare | ✅ | `architecture-comparison.md` §2: Anvil-subset + tolerant NBT + dual-layout reader + per-file-locked I/O |
| P00-07 parity-critical domains | ✅ | `docs/research/parity-and-testing-strategy.md` §1 (14 ranked domains) + `docs/vanilla-parity/PARITY-MATRIX.md` seed (all `unknown`, no claims) |
| P00-08 differential strategy/harness | ✅ | `parity-and-testing-strategy.md` §2 (6 levels, harness shape) + `docs/testing/TEST-MATRIX.md` seed (P02/P03 cases) |
| P00-09 dependency/license policy | ✅ | `parity-and-testing-strategy.md` §3 + `third-party.md` §3 (P01 set + compression decision point + deny-gate) |
| P00-10 ADR-0001 + risks | ✅ | `docs/adr/ADR-0001-system-architecture.md` (8 decisions, 10 risks) + `docs/architecture/system-overview.md` + `docs/performance/BENCHMARK-BASELINE.md` (schema only, no fake numbers) |

Method: 4 parallel read-only subagents (Pumpkin / Valence / Paper / Minestom) + 4 parallel studies
(protocol numbers / state machine / threading / persistence), then direct verification reads of cited
constants (`version.rs:89-143`, `MinecraftConstants.java:11-15`, `valence_protocol/src/lib.rs:79-87`,
`gradle.properties:2`, `Cargo.toml:110-113`, `world_info/mod.rs:14-18`, `anvil.rs:40-41`).

## Exit-gate check (`gates/EXIT-GATES.md` P00)

- [x] Reference repo SHAs/licenses recorded — paths+versions+licenses recorded; SHAs **explicitly unavailable** (no `.git`), documented as limitation + R-01, not faked.
- [x] 26.1.2 protocol/version facts baselined — 775 / "26.1" / state machine / login order; R-02/R-03 capture the two residual unknowns.
- [x] Initial architecture + dependency ADR accepted — ADR-0001 (D-01..D-08).
- [x] Clean-room/provenance policy established — `third-party.md` §2 + `provenance.md`.
- Unresolved risks listed explicitly — ADR-0001 §2 (R-01..R-10). No architecture blocker for Phase 01.

## Key conclusions (behavioral, all reference-observed → must re-verify vs real 26.1.2)

1. 26.1.2 = protocol **775** (no independent proto; zero `26_1_2` hits in code/assets).
2. Five-state machine (Handshake/Status/Login/**Config**/Play) + Transfer intent; Valence's 4-state shape is stale — not an oracle.
3. 26.1 world-layout break confirmed (dim-path vs legacy root `region/`); reader must be dual-layout.
4. 26.1 DataVersion exact value OPEN — writer work blocked until vanilla `level.dat` confirms (reader range `[4435, CONFIRMED]`).
5. Pumpkin GPL-3.0 / Paper GPLv3 → strict clean-room; Valence MIT / Minestom Apache-2.0 still need provenance rows.
6. Architecture: tick thread @50ms + Tokio edges + bounded handoff + stage-serial/batch-parallel + overrun clamp.

## Files added (docs only, zero production code — deliberate per PHASE-01 rule)

```text
docs/research/reference-repos.md
docs/research/protocol-baseline.md
docs/research/architecture-comparison.md
docs/research/parity-and-testing-strategy.md
docs/research/provenance.md
docs/legal/third-party.md
docs/adr/ADR-0001-system-architecture.md
docs/architecture/system-overview.md
docs/testing/TEST-MATRIX.md
docs/vanilla-parity/PARITY-MATRIX.md
docs/performance/BENCHMARK-BASELINE.md
docs/phases/PHASE-00-REPORT.md   (this file)
```

## Gate status

**Phase 00: PASS (conditional)** — all P00 exit items satisfied with two carried risks:
(a) R-01 SHAs unrecoverable from exports (re-fetch or accept file:line traceability);
(b) R-03 26.1 DataVersion confirmation required before P03 writer tasks — **resolved in Phase 03 (4790/19133).**
No code changed, no compatibility claimed, no thresholds invented. **Phase 01 unblocked.**
Recommended next: owner approves ADR-0001, then initial commit (repo currently has zero commits),
then P01-01 workspace init.

> Current gate state (2026-09-11): R-03 closed; R-01 open (owner action); zero
> commits still true, so the CI-based half of the Phase 01 gate remains
> config-only. Everything else in this report was independently verified in the
> audit round — including all cited `file:line` references, each of which
> resolved.
