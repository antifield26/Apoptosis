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


## Documentation and release consolidation (2026-09-12, second governance round)

Scope: governance and documentation only. **No file under `crates/` or `apps/` was touched**, and no
document was deleted or moved. The five gates and the four audit scripts produce the same results before
and after.

Baseline this round, all self-verified rather than taken from the handover:

| Claim in the handover | Measured |
|---|---|
| HEAD `6d778ff`, `origin/main` in sync | confirmed (→ 0 behind / 0 ahead) |
| clean working tree | content-clean; git reported one phantom modification, see G1 |
| tags `phase-09-final` and `v0.1.0-rc.1` | both present; the archive tag resolves (29 documents cite it) |
| CI green on `6d778ff` for **main and tag** | confirmed: runs `34699067993` and `34699069558`, both success |
| `v0.1.0-rc.1` has three assets | confirmed: `mc-server-aarch64`, `mc-server-x86_64-windows.exe`, `SHA256SUMS` |
| `cargo test --workspace` = 1 196 / 0 / 21, 74 suites | **identical** |
| four docs-audit scripts exit 0 | **identical** |

### What changed

**W1/W6 — `docs/audits/` was in no index.** Five audit documents existed and no index mentioned them;
worse, the layer table's History row said "the per-phase reports and five adversarial audits were retired
to git history", which is true of the *pre*-governance `AUDIT-01..06` files but reads as "there are no
audits here". `docs/README.md` gains a **Governance audits** layer naming all five files with a summary and
status, and its History row now says which audits were retired; `GOVERNANCE-REPORT.md` gains a paragraph
separating retired from live audits, since a map of retired paths cannot honestly list a directory that was
never retired. *Evidence:* all 11 directories under `docs/` are reachable from the layer table (11/11) and
all five audit files are named.

**W2 — the CHANGELOG did not describe the release, and claimed there was none.** The `[0.1.0-rc.1]` entry
listed no artifacts, so the CHANGELOG and the Release page disagreed about what the release is; and the
preamble still said "the project has shipped no tagged release yet", false since the tag existed. The entry
now carries a table of all three assets with sizes and SHA-256 values read from `gh release view`, and the
link definition points at the release page. *Evidence:* the asset hashes; the aarch64 hash was confirmed
against the binary running on the Pi, so the published artifact and the deployed server are the same build.

**W3 — the `[datapacks]` option was undocumented (AUDIT-07 finding E4's tail).** E4 recorded that the
deployed server loaded no data pack and that `RUNBOOK.md` never mentioned the option, leaving an operator
unable to explain missing `/function` names and absent structures. The device has since been reconfigured,
but neither document was updated — the gap outlived its cause. `config.example.toml` gains a
`[datapacks]` section (the key **commented out**, because that file promises every uncommented value is the
built-in default), and `RUNBOOK.md` §1 and §2 document it with the consequence wording aligned to
`DataPackConfig::vanilla_data` and the `packs.rs` module head. *Evidence:* `config.example.toml` is parsed
by no test, so it was verified against the real parser instead — the built binary was run on two
variants, the example verbatim and the example with `vanilla_data` uncommented; both reached
`server running`, and because `ServerConfig` uses `deny_unknown_fields` a misspelled key would have printed
`config error`. The second variant is what proves the key name.

**W4 — `docs/architecture/system-overview.md` was a per-phase document in the current-state layer.** It
was titled "(Phase 00)" and organised as phase-03/05/04 status blocks; two statements had become false,
including "World generation, lighting, entities and commands are explicitly not implemented" (worldgen and
commands are implemented, entities are modelled; only lighting is absent), and the crate map never
mentioned the 16-crate reality or ADR-0004/0005/0006. Rewritten around the present: the diagram and the
data-flow invariant are kept, the crate map is stated from the code with each boundary's reason, the
current-state and deliberately-absent lists align with `README.md` and `PARITY-MATRIX.md`, and the phase
blocks become one pointer to `CHANGELOG.md` and the tag. An "Invariants a change must not break" section
replaces the history that had been doing that job implicitly. *Evidence:* the file now names ADR-0005 and
ADR-0006 and the 16 crates, and no longer contains the false sentence.

**W5 — the release path was manual.** `v0.1.0-rc.1`'s artifacts were built and uploaded by hand, so
nothing recorded how. `.github/workflows/release.yml` triggers on a `v*` tag, builds x86_64 with `--locked`,
writes `SHA256SUMS`, creates the Release when the tag has none, and attaches both files with `--clobber` so
a re-run is idempotent; `CONTRIBUTING.md` gains a Releases section with the six-step checklist. Two
decisions are recorded in the workflow: **only x86_64 is built there** (the aarch64 binary is built on the
Pi, because a cross-build cannot be *run* from CI and therefore cannot be verified — the same limitation
`ci.yml` already documents), and the version guard is a **prefix** match, because the tag is `v0.1.0-rc.1`
while the manifest says `0.1.0` and cargo does not model the suffix. *Evidence:* YAML parses with the
intended shape; the guard was run against the real manifest and four tags that must fail; all three
embedded PowerShell blocks parse. The workflow writes the x86_64-only limitation into the release body, so
the page is honest before the manual step lands.

**G1 — a phantom modification, and two of my own defects.** Reported for the record rather than hidden:

- The handover said the tree was clean; `git status` said one file modified while `git diff` was empty. The
  index's cached *stat size* still held a CRLF-era value (37 655) for a blob that is 37 375 bytes and LF.
  `git update-index --really-refresh` cleared it and the blob hash is unchanged. This is the third
  appearance of the line-ending/stat-cache class in this project's history (AUDIT-07 E2 was the second),
  and it is worth knowing that a contributor can see a modification that does not exist.
- While writing W1 I copied anchor strings out of PowerShell's console rendering, where a UTF-8 em dash
  appears as a GBK-looking sequence. The README anchor consequently missed, and the `GOVERNANCE-REPORT`
  insertion wrote the rendering **literally**, turning a display artifact into real mojibake.
  `check_encoding.py` caught it on the next run. The check worked; the author was the defect. Both files
  were repaired with explicit unicode escapes and re-scanned at byte level.

### Device state in this round

Read-only apart from nothing at all — the service was not started or stopped. Verified because W3 writes
these facts down: service `active` and `enabled`; `[datapacks] vanilla_data = "/srv/mc-server/vanilla-data"`
using the **pack-root** form, with `data/minecraft` beneath it holding 758 tags, 1 515 recipes, 1 617
advancements, 1 326 loot tables and 1 202 structures; the journal reports `vanilla_data=true` with
`namespaces=1`; `/srv/mc-server/BUILD-INFO` records `commit=v0.1.0-rc.1`, `built=2026-09-12T22:23:05+08:00`,
`profile=release (on-device, --locked)`; the deployed binary's SHA-256 is
`0a9575c7499c...`, **identical to the published `mc-server-aarch64` asset**; the E5 backup exists at
`/var/backups/mc-server/world-v0.1.0-rc.1.tar.gz`; and the AUDIT-08 M4 hardening is live
(`ProtectSystem=strict`, `PrivateTmp`, `ProtectHome`, `NoNewPrivileges`, `MemoryDenyWriteExecute`,
`RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`).

**Correction to the record:** both the handover and `AUDIT-08-REMEDIATION.md` state the configured path as
`/srv/mc-server/vanilla-data/minecraft`. That path does not exist; the config names the pack root
`/srv/mc-server/vanilla-data`, which is an accepted form and works. The configuration was always correct
— the written path was off by one level in two documents. It is recorded here rather than edited into
the audit record, which is history.

### Acceptance evidence

| Gate | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **1 196 passed / 0 failed / 21 ignored, 74 suites** — identical to the handover baseline |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | exit 0 |
| `cargo deny check licenses bans sources` | exit 0 |
| `check_encoding.py` | exit 0 |
| `check_links.py` | exit 0 — 36 markdown files including the 4 root documents |
| `check_gate_totals.py` | exit 0 — 0 mismatched totals; canonical 1 196 |
| `check_line_endings.py` | exit 0 — 0 violations |

### Handed over

1. **W7 was not done** — deepening the ~126 task rows that still carry only suite-level evidence. It is
   explicitly optional in the brief, and it is the right kind of work to hand over rather than rush: it
   needs per-row reading of implementation *and* tests, and the previous two rounds both showed that a
   hurried coverage claim is worse than an acknowledged gap (AUDIT-07 reported "30 of 30 verified" while 16
   rows were never compared). The decomposition to start from is AUDIT-08-FINDINGS §5: 32 rows deep-dived,
   126 inherited.
2. **CI has not seen this round's commits at the time of writing.** They are local. The next push runs the
   four audit scripts and the five gates on them; nothing here should be treated as CI-verified until then.
3. **`release.yml` has never executed.** It cannot be validated end-to-end without publishing a tag, which
   this round deliberately did not do. Its YAML, its version guard and its embedded PowerShell were checked
   locally; the first real tag is its first real test.
4. **KD-38 (real-client acceptance) remains the only unresolved acceptance boundary**, unchanged.
5. **The `check_line_endings.py` guard can only fire on Windows** — a Linux checkout already yields LF,
   as its docstring says. It runs in CI for completeness, not because CI is where it earns its keep.

## Final verification (first governance round)

The numbers below are that round's, kept as measured: **1 194** is the total *before* AUDIT-08 added two
coverage tests, and "both audit scripts" is correct for a tree that had two of them. Current values are in
the section above (*Documentation and release consolidation*, second round: 1 196 and four scripts).

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
