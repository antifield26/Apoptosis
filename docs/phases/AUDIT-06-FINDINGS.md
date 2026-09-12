# Audit 06 — Phase 09 final review (P09-13)

Date: 2026-09-12. Method: one **independent adversarial subagent** with read-only
scope (the REVIEW-AGENT role, `mc-rust-agent-prompts/agents/REVIEW-AGENT.md`),
run against the full Phase 09 changeset with the contract documents, the phase
reports and the retained run logs under `target/` in scope. It re-derived
numbers itself rather than trusting the documents under review. Verdict:
**PASS WITH FINDINGS** — no BLOCKER, four MEDIUM and six LOW findings, every one
dispositioned below before the phase's commit. The primary agent re-verified
each finding before acting.

Scope reviewed: `crates/server/src/metrics.rs` (the TempDir-tag fix),
`crates/server/tests/pi_profile.rs` (post-fix), and the new/changed documents
`docs/adr/ADR-0005-plugin-boundary.md`, `docs/vanilla-parity/KNOWN-DIVERGENCES.md`,
`docs/release/RELEASE-CANDIDATE.md`, `docs/testing/TEST-MATRIX.md` (Phase 09
section), `docs/performance/BENCHMARK-BASELINE.md` (§P09-08/§P09-09/§4),
`docs/vanilla-parity/PARITY-MATRIX.md` (20 TPS row).

## 1. What the review verified clean (its own coverage list, condensed)

- **Claims vs evidence.** Headline totals 1 189 / 0 / 21 over 74 suites
  re-aggregated from the full-run logs; every per-suite count cited in the
  TEST-MATRIX P09 rows (32 named suites) matched digit-for-digit; all §P09-08
  benchmark figures matched the logs; the release binary SHA-256 and size
  matched; the differential total (15/15 over 7 suites, one env-gated failure
  superseded by the re-run with the jar and world set) held; **no** "20 TPS" or
  "production-ready" claim anywhere; **no** claim that release artifacts are
  published (R-09 respected in all four documents); no invented version numbers.
- **Internal consistency.** ~23 KD rows content-matched against their source
  rows (1 182/1 202 structures, 1 028/1 182 generable, 8 shipwreck palettes,
  `WIRE_LIVE_BLOCKS = 14`, DataVersion window `[4435, 4790]`, zero light masks,
  7 of 21 recipe types — all exact); ADR-0005's code-site citations verified in
  source (`PHASE_ORDER` const + ordering tests, `build_command_tree`,
  `run_function` → `dispatch_command`); its "five doc-comment mentions, zero
  plugin types" grep claim exactly reproduced; every cross-reference in the new
  documents resolves (the two declared-in-flight files being the phase's own
  remaining deliverables).
- **The metrics fix.** The diff does exactly what the L7 row and the doc
  comment claim; no leftover shared tag anywhere in the crate; the per-test-tag
  convention claim verified across all 16 other call sites; the probe's design
  matches the claimed mechanism.
- **Scope/DoD.** Exactly one code file changed beyond tests; zero TODO/stub
  leakage in new docs; TEST-MATRIX byte-clean (NUL 0, valid UTF-8, no
  duplicated sections, well-formed tables).

## 2. Findings and dispositions

| # | Severity | Finding | Disposition |
|---|---|---|---|
| 1 | MEDIUM | `ADR-0005` §1 said "the **seven** Vanilla commands" — the stale P07-05 count; `build_command_tree` registers nine nodes (eight KD-31 commands + `/function`), and "seven" matches no source | **Fixed**: the seam row now says "nine nodes — the eight Vanilla commands of KD-31 plus `/function`, counted under the datapack seam". Numeric-rot caught in an Accepted ADR — exactly what this phase's own review criterion exists for |
| 2 | MEDIUM | TEST-MATRIX P09-T01 said "two consecutive post-fix full runs" but only one retained post-fix log existed (`p09_full_test.log`); the other same-day log was the pre-fix P09-01 gate run | **Fixed**: a second post-fix full run was executed after the final code change and retained (`target/p09_full_test2.log`); the matrix cites both logs and their dates |
| 3 | MEDIUM | §P09-08 cited a release-profile log whose self-description printed `build profile: dev` — a hardcoded string in `pi_profile.rs` (a P08-13 harness defect that only became observable once release was first run) | **Fixed in code**: the string is now derived from `cfg!(debug_assertions)`; the release perf suite was re-run after the fix and §P09-08 quotes the new log. The mislabel is recorded here as a named harness defect rather than silently patched |
| 4 | MEDIUM | RELEASE-CANDIDATE claimed "five gates green, 2026-09-12" without enumerating them, and three retained gate logs were the previous day's | **Fixed**: the claims table now enumerates all five gates with commands and retained logs, and all five were re-run on 2026-09-12 post-fix (`gate_fmt/clippy/aarch64/deny.log`, `p09_full_test2.log`) |
| 5 | LOW | `BENCHMARK-BASELINE` §4 said aarch64 "green 2026-09-12" citing only a 2026-09-11 artifact | **Fixed** by the same-day re-run in the chain (`gate_aarch64.log`) |
| 6 | LOW | The flaky-fix L7 row claimed "10 consecutive green metric runs" with no retained artifact | **Fixed**: the repetition run is retained (`target/p09_metrics_repeat.log`) |
| 7 | LOW | The §P09-09 smoke had tooling but no retained transcript | **Fixed**: transcript retained (`target/p09_smoke.log`), covering both starts |
| 8 | LOW | The probe's "17 duplicates in 160 000" headline had no retained stdout (source and binary retained, mechanism sound) | **Fixed**: probe re-run with stdout retained (`target/p09_probe_tempdir.log`); it produced 7 duplicates, the earlier run 17 — the documents now cite the recurring result with the retained log named, so the number and the artifact agree |
| 9 | LOW | TEST-MATRIX P09-T13 declared "pass" before `AUDIT-06-FINDINGS.md` existed | **Resolved**: this document is that deliverable and lands in the same commit, so the row is true at commit time |
| 10 | LOW | RELEASE-CANDIDATE "commands (8)" undercounts the dispatcher tree by `/function` without stating the convention | **Fixed**: the release doc now states the convention out loud; `PARITY-MATRIX`/KD-31 stay as the source rows |

## 3. What the review changed about the phase

Two of the four MEDIUM findings were real evidence-integrity defects in
documents whose entire purpose is defensible evidence: an over-broad "two
consecutive runs" provenance claim (2) and an under-specified gates claim (4).
The other two were numeric rot (1) and a measurement-harness defect that had
survived P08 because nothing had ever run the harness in release (3). No
finding touched code correctness, the divergence catalog's content, or the
honesty boundaries — the release-blocking facts (R-09, no Pi, no real client)
are intact everywhere they appear.
