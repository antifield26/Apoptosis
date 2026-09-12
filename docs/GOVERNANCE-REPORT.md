# Governance Report — repository normalisation (2026-09-12)

Scope: engineering governance only. No file under `crates/` or `apps/` was
touched; the five quality gates and both documentation-audit scripts produce
the same results before and after. The owner answered three decision questions
before any file was moved or deleted (the fourth — repository visibility — is
a verifiable fact, not a decision: the repository is **public**).

## Baseline (verified, 2026-09-12)

- Five gates: `cargo test --workspace --no-fail-fast` = **1 191 passed /
  0 failed / 21 ignored, 74 suites**; fmt, clippy (`-D warnings`), aarch64
  cross-check and `cargo deny` (licenses/bans/sources) all clean. Identical
  after the governance work (see the final section).
- CI: the `ci` workflow has run on GitHub since the repository gained its
  remote — **both runs green** (7m6s, 6m45s). Where the workflow's own header
  previously claimed "configured, never executed" (and cited an audit file
  that no longer exists), it now states the truth.
- 256 tracked files (203 under `crates/`, 40 under `docs/` = 38 markdown +
  2 tsv, 2 under `apps/`, 11 root files). The pre-governance audit's "246" was
  the encoding script's count of tracked *text* files (the two `.tsv` baselines
  and nine other non-text-suffix files are excluded by its suffix filter).
- `docs/` was 444.5 KB across 38 markdown files; `docs/phases/` alone was
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

## Pending items (owner decisions or actions, not done unilaterally)

1. **Ten code comments still cite retired reports** (e.g.
   `crates/container/src/smelting_data.rs` → `PHASE-06-REPORT.md` §5.9;
   `crates/server/src/metrics.rs` → PHASE-08-REPORT §2.1). Left untouched on
   purpose: the task forbids changing `crates/`/`apps/`, and each citation
   resolves in git history (`git show phase-09-final:…`). A future code-touch
   commit can re-point them at the changelog.
2. **CI's `docs-audit` job runs for the first time on the next push.** Both
   scripts pass locally on the post-governance tree (0 findings); if the
   runner disagrees, that is a script portability bug to fix, not a docs one.
3. **The prompt pack stays git-ignored.** If the owner ever wants it public,
   the reference rewrite is superseded by un-ignoring it; the two mechanisms
   should not be mixed.
4. **No tagged binary release exists.** When one is wanted: tag a version,
   build per `docs/release/RELEASE-CANDIDATE.md` §2, attach artifacts, and
   only then may "release" language appear in the README (it currently says
   release candidate).
5. **KD-38 (real-client acceptance) remains the only unresolved acceptance
   boundary**; the parity matrix tracks it.

## Final verification

Recorded in the governance commits: the five gates match the baseline
exactly (1 191 / 0 / 21, 74 suites; fmt, clippy, aarch64, deny clean), and
both audit scripts report zero findings (encoding: 249 text files, all valid
UTF-8, zero mojibake; links: 0 broken markdown links, 0 broken backticked
paths, 0 references to untracked files).
