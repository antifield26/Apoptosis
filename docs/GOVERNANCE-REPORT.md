# Governance Report — repository normalisation (2026-09-12)

Scope: engineering governance only. No file under `crates/` or `apps/` was
touched; the five quality gates and both documentation-audit scripts produce
the same results before and after. The owner answered three decision questions
before any file was moved or deleted (the fourth — repository visibility — is
a verifiable fact, not a decision: the repository is **public**).

## Baseline (verified, 2026-09-12)

- Five gates: `cargo test --workspace --no-fail-fast` = **1 194 / 0 /
  21 ignored over 74 suites** at the audit baseline (**1 196 / 0 / 21** after
  the Audit 08 coverage tests); fmt, clippy (`-D warnings`), aarch64
  cross-check and `cargo deny` (licenses/bans/sources) all clean. Identical
  after the governance work (see the final section).
- CI: the `ci` workflow runs on GitHub since the repository gained its remote.
  At the time of writing the two runs then in existence were both green (7m6s,
  6m45s). The full history as of Audit 07 is **six runs: four green, two red**
  (`34683857279` and `34683247774` — the docs-audit job's first execution and
  the archive-tag fetch, both fixed by the two commits that follow them), with
  **HEAD green**. The two failures are part of the record and the sentence above
  was written before them (Audit 07 finding L3).
- **251** tracked files after the governance work (203 under `crates/`, **27**
  under `docs/` = 25 markdown + 2 tsv, 2 under `apps/`, 2 under `tools/`, 15 at
  the repository root plus `.github/`). An earlier version of this line said 256
  with "40 under `docs/`": the 40 was the *pre*-governance docs count, so the
  sub-total and the total were not recomputed after the 15 phase reports and the
  divergence catalog were deleted (40 − 16 + 3 = 27). Corrected by Audit 07
  finding L1. The pre-governance audit's "246" was the encoding script's count
  of tracked *text* files.
- `docs/` was 444.5 KB across 38 markdown files; the retired phases directory alone was
  194.1 KB (43.7%). All figures re-verified with `tools/docs-audit/` scripts.
- 78 references to the git-ignored prompt pack; 4 broken backticked paths;
  0 broken markdown links. All three numbers reproduced from the committed
  audit scripts.

## Owner decisions (2026-09-12)

| Question | Decision |
|---|---|
| Process reports (15 files, 43.7% of docs/) | **③ distill into CHANGELOG, then delete** — explicit approval for the deletions below; the full text stays reachable in git history at tag `phase-09-final` |
| The git-ignored prompt pack and its 78 dangling references | **② distill the rules into an in-repo document and rewrite the references** (`docs/CONVENTIONS.md`); the pack stays untracked |
| Attribution for the studied reference projects and Mojang data | **① a root `NOTICE` file** |

## What changed, and why

1. **Standard facade** (README, CHANGELOG, CONTRIBUTING, SECURITY, NOTICE,
   `.editorconfig`). The public repository had no landing page at all — the
   only README lived in the git-ignored pack. The README's completion claims
   all link to the parity matrix or benchmark records; the "not here yet"
   list is as prominent as the "works" list.
2. **`docs/CONVENTIONS.md`** carries the engineering contract in-repo, with the
   same section numbers the historical documents cite, so the agent contract's "§3.6"
   became "CONVENTIONS.md §3.6" — a faithful mapping, not a renumbering. 37
   anchored replacements across 14 documents; MASTER-PROMPT §7 and
   EXECUTION-LOOP §5 citations folded into their in-repo homes.
3. **One parity authority.** `KNOWN-DIVERGENCES.md` was merged into
   `PARITY-MATRIX.md` (KD-01…KD-38 preserved as row tags; the four-class
   status vocabulary adopted; three operations rows added). Two documents
   answering "how do we differ from Vanilla?" would have drifted apart — the
   audits kept catching exactly that pattern. Five stale rows that
   contradicted real rows are deleted (not updated), and one unverified count
   ("43 unit tests") is corrected to the logged 27 + 11.
4. **TEST-MATRIX.md restructured** from a 255-row accumulator with six
   repeated "Bugs found" tables into: current coverage by area (counts from
   the run log), the differential-suite inventory, one deduplicated 49-entry
   defect history, and the known-false-assertion lessons.
5. **The phases directory retired** (15 files deleted, owner decision ③): the
   content is distilled into `CHANGELOG.md`, the defect history and the parity
   matrix; every reference in the surviving documents was rewritten to the
   git-history form before deletion, and the whole pre-governance tree is the
   tag **`phase-09-final`** (nothing is unreachable). The `AUDIT-04` gap in
   the numbering is explained in [README.md](README.md): the fourth audit was
   folded into the fifth file during Phase 06.
6. **Documentation health made mechanical**: `tools/docs-audit/check_encoding.py`
   and `check_links.py` are committed (repository-relative, exit codes) and run
   as a `docs-audit` CI job. The committed link checker is stricter than the
   pre-governance scratch version: it also treats references to untracked
   files as findings and exempts the declared external-clone paths.
7. **Fixes found on the way**: `ci.yml`'s header claimed the workflow had
   never executed (it has, green); the RUNBOOK install steps named a
   `vanilla_data` config key that does not exist; the RUNBOOK now documents
   the registry-fixture installation; two further unresolvable paths (the
   valence fixture citation and the never-built difftest-harness note) are
   annotated as external/hypothetical.

## Where the audits live, and which ones were retired

Two different things are called "audits" here, and conflating them makes a directory look missing:

- **Retired (pre-governance).** `AUDIT-01`—`AUDIT-06` were finding files under `docs/phases/`, retired to
  git history at tag `phase-09-final` with the phase reports. (`AUDIT-04` never existed as a file — the
  fourth pass was folded into `AUDIT-05`.) The map below covers these.
- **Live (post-governance).** `docs/audits/` holds five files that were created *after* this report and
  are deliberately kept in the working tree: they are the evidence for the current state, so retiring them
  would leave the completion claims in `README.md` and `PARITY-MATRIX.md` pointing at a tag. Indexed in
  [README.md](README.md).

## Old path → new path map

| Old | New |
|---|---|
| The ten phase reports | `CHANGELOG.md` per-phase sections; full text `git show phase-09-final:docs/phases/PHASE-05-REPORT.md` |
| The five audit findings | defect history in `docs/testing/TEST-MATRIX.md`; full text at the tag |
| The retired divergence-catalog file | `vanilla-parity/PARITY-MATRIX.md` (KD tags in rows) |
| (references to) the agent contract's `§N` | `docs/CONVENTIONS.md §N` (same numbering) |
| (references to) `MASTER-PROMPT §7` | `CONVENTIONS.md §3.4` |
| (references to) the execution loop's review checklist | `CONVENTIONS.md §14` |
| (references to) the exit-gates / definition-of-done contract files | `CONVENTIONS.md §15` and the phase-gate summary in git history |
| (references to) the task-index file | `CHANGELOG.md` (phase structure) — no in-repo task index exists |

## Pending items — status after the Audit 08 remediation (2026-09-12)

1. **Code references citing retired reports — CLOSED.** AUDIT-07 measured 13
   references in 12 files (an earlier version said ten); AUDIT-08 re-measured
   12 in 11 files and **all of them were re-pointed** at `CHANGELOG.md` or the
   git-history tag form in the remediation commit. Measured now: **0**.
2. **CI `docs-audit` first run — RESOLVED.** The first run failed and taught
   the checker two things (target/-prefix exemptions, archive-tag fetching);
   the job has since run green.
3. **The prompt pack stays git-ignored** — unchanged by decision.
4. **Tagged binary release — CLOSED.** Tag `v0.1.0-rc.1` with the x86_64 and
   aarch64 `mc-server` binaries (SHA-256 sums attached) on GitHub Releases;
   README/RELEASE-CANDIDATE/CHANGELOG use release language.
5. **KD-38 (real-client acceptance) remains the only unresolved acceptance
   boundary**; the parity matrix tracks it. The parity-matrix roadmap gaps
   (lighting, entity persistence/sync, redstone wiring, container windows, …)
   are development work, not remediation.

## Final verification

Recorded in the governance commits: the five gates match the baseline
exactly (1 194 / 0 / 21, 74 suites; fmt, clippy, aarch64, deny clean), and
both audit scripts report zero findings. Measured by Audit 07 on the same
tree: encoding — **240** text files (the script's suffix filter includes
`.hex`), all valid UTF-8, zero mojibake; links — 0 broken markdown links, 0
broken backticked paths, 0 references to untracked files. An earlier version of
this line said "249 text files", which is the pre-governance figure with `.hex`
included (Audit 07 finding L2). Note the link checker's scope: as of Audit 07
finding H2 it covers **every** tracked markdown file, where the version this
report was written against covered only `docs/**` (25 files).
