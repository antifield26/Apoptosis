# AUDIT-07 remediation

Date: 2026-09-12. Scope: fix the nine findings in [AUDIT-07-FINDINGS.md](AUDIT-07-FINDINGS.md) using the
dispositions that report recommended, plus two defects that surfaced while doing so.

Every fix below carries the check that shows it works. Where a finding was "this claim has no test that
could fail", a new test was written **and then the audit's own break re-applied** to confirm the test
fails: a test that only passes proves nothing, which is the lesson the audit itself was about.

## 1. Disposition

| ID | Action | Verification |
|---|---|---|
| **H1** save atomicity untested | Split `write_atomic` into `stage` + `commit` (`crates/persistence/src/save.rs`), the latter documented as "exactly one `rename`, never touches `path`", and added `a_failed_commit_leaves_the_live_file_untouched` and `a_staged_write_leaves_the_live_file_untouched` | Re-applied the audit's break (delete the live file before the rename): the new test **FAILED**; without it, passes. `mc-persistence` 74 pass |
| **H2** link checker covered `docs/` only | `tools/docs-audit/check_links.py` now scans **every** tracked markdown file and prints its scope, listing the root documents explicitly | Scope went 25 → 32 files. Broke a link in `README.md`: the checker now **exits 1 and names it**; before, it never read the file |
| **M1** flood did not reach the overflow path | Extracted the click loop into `hostile_flood` and added `a_flood_over_a_capped_slot_cannot_create_or_destroy_items`, which drives the same 2 000 hostile clicks over a menu whose slot 0 is capped at 1 — and asserts the over-limit placement deterministically, so coverage does not depend on the generator | Re-applied the break (discard the overflow): the capped-slot flood **FAILED**, the chest flood still passes — which is the finding, reproduced exactly |
| **M2** `RELEASE-CANDIDATE.md` said 1 189 | Corrected to the measured figure, with a note that the cited P09 log predates the added tests. Root cause addressed: **all** documents now state 1 194, and `check_gate_totals.py` fails when one drifts | Drifted `README.md` to 1 191: checker **exits 1** and names the file |
| **M3** evidence in git-ignored `target/` | Committed 20 reproduction sources under `tools/vanilla-probe/` and `tools/pi-bench/` with READMEs; every citation re-pointed; log citations now say "machine-local, git-ignored, not distributed" with the command that regenerates them | `check_links.py` reports 0 references to untracked files; no `target/` citation remains except the two labelled as local |
| **L1** 256 tracked files / 40 in `docs/` | Corrected to 251 / 27, with the arithmetic that explains the stale 40 | Re-counted from `git ls-files` |
| **L2** "249 text files" | Corrected to 240, and noted the figure moved to 265 once the probe sources were committed and the filter widened | `check_encoding.py` prints the count it checks |
| **L3** "both runs green" | Corrected to the full history: six runs, four green, two red, HEAD green, with the failing run ids and what fixed them | `gh run list` |
| **L4** "ten code comments" | Corrected to 13 references in 12 files | Grep over `crates/` |

## 2. Defects found while remediating

Both are recorded in the audit report as §1a, because an audit that only reports what it set out to look
for is not much of an audit.

### 2.1 Five library test counts were assigned to the wrong crates (HIGH)

`docs/testing/TEST-MATRIX.md` quoted `mc-protocol` lib (126), `mc-world` (103), `mc-command` (27),
`mc-data` (4), `mc-worldgen` (64). Re-measured per crate:

| crate | stated | measured |
|---|---|---|
| `mc-protocol` | 126 | **103** |
| `mc-world` | 103 | **31** |
| `mc-command` | 27 | **89** |
| `mc-data` | 4 | **126** |
| `mc-worldgen` | 64 | **81** |

Each stated value is another crate's real count (126 is `mc-data`'s, 103 is `mc-protocol`'s, 27 is
`mc-simulation`'s, 64 is `mc-redstone`'s, 4 is `mc-test-support`'s). The figures were right; the names
were rotated. All are corrected from fresh `cargo test -p <crate> --lib` runs, the document now carries
per-crate counts for every crate, and the sum is asserted against the workspace total.

**The audit missed this**, and the erratum in the audit report says how: its count check silently skipped
every library row because its regex did not match the `lib` word between the backtick and the number. The
number of verified rows was reported as 30 of 30 when 16 rows had never been compared.

### 2.2 A cited tool did not do what the document said (LOW)

`docs/research/data-pack-baseline.md` named `target/vanilla-26.1.2/extract_pack.py` as the pack extractor.
That file was byte-identical to `recount.py` (same SHA-256 `88dba705…`) and contains **no extraction
code** — only `namelist()` counting. The extraction snippet is inline in the same document, so the method
was always reproducible; the pointer was simply wrong. Committed once as `recount.py`; the document now
points at its own snippet and says why.

## 3. Gates after remediation

| Gate | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **1 194 passed / 0 failed / 21 ignored, 74 suites** (1 191 before; +2 H1 tests, +1 M1 test) |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | exit 0 |
| `cargo deny check licenses bans sources` | exit 0 (`bans ok, licenses ok, sources ok`) |
| `cargo test -p mc-data -p mc-container -p mc-persistence -p mc-server -p mc-worldgen -- --ignored` | 21 passed (15 differential in 7 suites + 6 benchmark), 0 failed |
| `tools/docs-audit/check_encoding.py` | exit 0 — **265** tracked text files, 0 invalid UTF-8, 0 mojibake (was 240: the filter now also covers `.py`, `.java` and `.service`, so the newly committed sources are inside the check rather than beside it) |
| `tools/docs-audit/check_links.py` | exit 0 — **32** markdown files incl. the 4 root documents, 0 broken |
| `tools/docs-audit/check_gate_totals.py` | exit 0 — new; 0 mismatched totals, 7 exempt historical records |

## 4. What changed

- `crates/persistence/src/save.rs` — `stage`/`commit` split, invariant documented, two tests
- `crates/container/src/menu/tests.rs` — click loop extracted, capped-slot flood added
- `tools/docs-audit/check_links.py` — scope widened to all tracked markdown, scope printed
- `tools/docs-audit/check_encoding.py` — filter extended with `.py`, `.java`, `.service`
- `tools/docs-audit/check_gate_totals.py` — **new**, single-owner total enforcement
- `tools/vanilla-probe/` — 17 reproduction sources + README
- `tools/pi-bench/` — 3 benchmark clients + README
- `docs/testing/TEST-MATRIX.md` — 7 counts corrected, total 1 194, H1/M1 attributions
- `docs/release/RELEASE-CANDIDATE.md`, `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`,
  `docs/GOVERNANCE-REPORT.md` — totals and the four L-findings
- `docs/research/data-pack-baseline.md`, `docs/research/provenance.md`, `docs/legal/third-party.md`,
  `docs/protocol/chunk-wire-format.md`, `docs/performance/BENCHMARK-BASELINE.md`,
  `docs/adr/ADR-0004-data-loading-and-registry-split.md` — citations re-pointed
- `.github/workflows/ci.yml` — runs the third audit script; job comment describes the real scope
- `docs/audits/AUDIT-07-FINDINGS.md` — erratum on its own over-claim, plus §1a

## 5. Still open

- **Lane E (Pi) remains unverified.** The host answers ICMP but its SSH channel is a password login and no
  credential is available to an automated agent, so the P08/P09 operational claims that live only on the
  device are still `unverified`, not clean. This is a credential boundary, not a defect.
- **`RELEASE-CANDIDATE.md`'s cited run logs still live only on the build machine.** The reproduction
  *sources* are now committed and the citations say so; committing multi-hundred-kilobyte logs was judged
  not worth the repository weight when the five commands in §2 regenerate them.
- **`mc-persistence`'s atomicity invariant is pinned on the failure path.** A successful `rename` cannot
  be distinguished from delete-then-rename by any outcome a test can observe, so the guarantee rests on
  `commit` being a single `rename` plus the failure test. That is stated in the function's doc comment
  rather than left implied.
