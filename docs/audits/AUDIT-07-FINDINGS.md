# AUDIT-07 — first full cross-audit of the initial preset task list

VERDICT: **PASS WITH FINDINGS**

The proposition audited is *"the initial preset task list (P00-01..P09-14) is fully delivered and every
current 'done' claim has reproducible evidence"*. The task count in the brief is **correct: 158**, with no
duplicate ids and contiguous numbering inside every phase (10+12+16+16+18+18+18+20+16+14). All five gates
re-run green, the differential suites re-run green against the real jar, and the deepest end-to-end claim
in the repository — that a real vanilla 26.1.2 server boots on a world this server rewrote — reproduced
from scratch. No finding is about a missing feature: nine findings, of which **four are claims whose
evidence is weaker than their wording** (one of them — untested save atomicity — has no covering test at
all), one is an audit tool whose scope is narrower than its presentation, and four are stale numbers in the
governance report.

Auditor: independent; no prior involvement with this repository's implementation or its previous audits.
Nothing under `crates/`, `apps/` or `docs/` was modified: the only file written is this one, and Lane C's
temporary patches were restored byte-for-byte (verified by re-reading each file and comparing).

## 1. What was run, and what it returned

| Gate | Claimed | Measured by this audit | Verdict |
|---|---|---|---|
| `cargo test --workspace --no-fail-fast` | 1 191 passed / 0 failed / 21 ignored, 74 suites (at `bf74123`) | **1 191 / 0 / 21, 74 suites** | identical |
| `cargo fmt --all -- --check` | clean | exit 0 | identical |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | exit 0, no diagnostics | identical |
| `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | clean | exit 0 | identical |
| `cargo deny check licenses bans sources` | clean | exit 0 (`bans ok, licenses ok, sources ok`) | identical |
| differential suites (jar-gated) | 15 passed / 0 failed / 7 suites | **15 / 0 / 7 suites** | identical |
| benchmark harness (`pi_profile` 4 + `tick_baseline` 2) | 6 ignored | 6 passed under `--ignored` | identical |
| `tools/docs-audit/check_encoding.py` | exit 0 | exit 0 (240 text files, 0 invalid UTF-8, 0 mojibake) | identical |
| `tools/docs-audit/check_links.py` | exit 0 | exit 0 (25 docs, 0 broken) | identical |
| CI on HEAD (`bf74123`) | "both runs green" | **success** — run `34684359832`, 6m15s | green, wording stale (L3) |

The ignored-test arithmetic reconciles exactly and three ways: **21 ignored tests across 9 suites** in the
default run, **21 `#[ignore]` attribute lines across the same 9 files**, and **15 jar-gated (7 suites) +
4 `pi_profile` + 2 `tick_baseline` = 21**. The brief's "15 passed / 7 suites" for the differential gate is
precise: running that command yields 21 passed, of which exactly 15 are the differential suites and 6 are
the benchmark harness the README correctly excludes.

The strongest claim in the repository was reproduced end to end. `vanilla_differential` printed, from this
audit's own run: `decoded 529 chunks (1 lit, 0 with entities)`, `rewrote 529 chunks through our writer`,
followed by the **real vanilla server's own console log** loading that world —
`Loading 0 persistent chunks...`, `Saving chunks for level 'ServerLevel[world]'/minecraft:overworld`,
`ThreadedAnvilChunkStorage (world): All chunks are saved`, for all three dimensions. The README's "529
vanilla chunks … booted on a real vanilla 26.1.2 server" is therefore **verified, not asserted**.

Also verified mechanically: `KD-01..KD-38` all resolve in `PARITY-MATRIX.md` with none missing; the defect
history holds **49 rows with no duplicates**; and **30 of 30** per-suite counts quoted in `TEST-MATRIX.md`
match the run (one apparent mismatch was my own script conflating the two suites named `determinism` —
redstone is 6 and worldgen is 7, each matching its own row — and is recorded here so the false positive is
not mistaken for a finding).

> ### Erratum (added during remediation)
>
> **That "30 of 30" was wrong, and so was the way it was obtained.** The check matched a backticked name
> immediately followed by `(N)`, but every per-crate library row in `TEST-MATRIX.md` reads
> `` `mc-protocol` lib (126) `` — the word `lib` sits between the backtick and the parenthesis — so **every
> lib row was skipped**. The 30 rows actually compared were all integration suites.
>
> Re-measuring crate by crate while remediating found **five library counts misassigned**:
> `mc-protocol` said 126 (real 103), `mc-world` 103 (real 31), `mc-command` 27 (real 89), `mc-data` 4
> (real 126), `mc-worldgen` 64 (real 81). Each stated value is *another crate's* real count, so the
> figures were right and the names were rotated. A sixth, `mc-persistence`, was right at the time and has
> since moved 72 → 74.
>
> This is the audit's own instance of the failure it exists to find: **a check whose silence about
> non-matches is indistinguishable from success**. The count of rows verified should have been reported as
> "30 integration rows; the 16 library rows did not match the extractor and were not compared", which
> would have made the gap visible. The remediation fixes the counts and adds
> `tools/docs-audit/check_gate_totals.py` so the total cannot drift again.

## 1a. Findings the audit did not have

Two defects surfaced while remediating, both outside the original finding list:

- **The misassigned library counts above** (HIGH — five wrong figures in the canonical test matrix, in a
  document whose whole purpose is to say what the tests prove).
- **`docs/research/data-pack-baseline.md` cited a tool that does not do what the text says.**
  `target/vanilla-26.1.2/extract_pack.py` was named as the pack extractor, but the file was byte-identical
  to `recount.py` (same SHA-256) and contains **no extraction code** — it only recounts `namelist()`
  entries. The extraction snippet is inline in the same document, so the method was always reproducible;
  the pointer was wrong. (LOW — a wrong pointer, not a wrong method.)

## 2. Findings

### HIGH

**H1 — The save atomicity claim has no covering test, in the entire persistence crate.**
`crates/persistence/src/save.rs:5-15` documents the crash-window reasoning precisely, and the code does
implement `write tmp -> fsync tmp -> copy to .old -> rename -> fsync dir`. But no test can detect the
ordering being wrong. I patched `write_atomic` to delete the live `level.dat` **before** the rename — the
exact non-atomic ordering the doc comment exists to forbid, where a crash in the window leaves no
`level.dat` at all — and ran **every test in `mc-persistence`**: `72 + 9 + 16 + 7` passed, 0 failed. The
four tests that look like they cover it (`atomic_write_keeps_a_backup_and_leaves_no_temp_file`,
`atomic_write_without_backup`, `failed_write_leaves_the_previous_file_intact_and_reports`, and
`restart::level_dat_backup_holds_the_previous_version`) all assert the **end state** — backup content,
final content, no staged file left — none asserts that the live file is never absent.
`TEST-MATRIX.md:31` lists "atomic tmp→rename" in the "what they prove" column and `README.md:31` advertises
"atomic saves"; both are implemented and true of the code, but neither is test-pinned. *Disposition:
add a test that fails when the live file is removed before the rename (assert the rename is the only
mutation of `path`), or downgrade the wording to "atomic in implementation, not regression-pinned".*

**H2 — `check_links.py` silently covers only `docs/`, leaving the four root documents unchecked.**
`tools/docs-audit/check_links.py:30` filters `f.startswith('docs/')`, so `README.md`, `CHANGELOG.md`,
`CONTRIBUTING.md` and `SECURITY.md` — 4 of 29 tracked markdown files, and the four with the most
external-facing references — are never examined. The tool is presented in `CONTRIBUTING.md` and the
governance report as the objective half of a docs review, and it exits 0 without stating its scope.
I re-implemented it over **all** markdown with the exemptions disabled: it found **0 broken links and 0
missing paths**, so nothing is currently broken — but the coverage gap is real and would hide a future
broken link in the README. *Disposition: widen the filter to every tracked `.md`, or print the scope so
"0 findings" cannot be misread as repository-wide.*

### MEDIUM

**M1 — The 2 000-click flood does not exercise the path its claim names.**
`README.md:29` says inventory transactions are "conservation-proven under 2 000-click floods" and
`TEST-MATRIX.md:34` lists "2 000-click floods cannot create or destroy items".
`crates/container/src/menu/tests.rs:593` performs the flood, but its stacks (64/16/33 in a chest menu)
never exceed any slot limit, so `Menu::insert`'s overflow branch is unreachable during it. I broke
conservation **directly** — discarding the overflow `insert` returns, so items above a slot limit are
destroyed — and the flood test **passed**; only
`menu::tests::a_slot_ceiling_below_the_item_limit_is_respected` failed. The flood's assertion (unchanged
`total_items`) is sound and would catch destruction on the paths it does run; the wording over-states which
paths those are. *Disposition: either drive an over-limit click into the flood's click mix, or attribute
the overflow claim to the slot-ceiling test.*

**M2 — `RELEASE-CANDIDATE.md` states a test count that contradicts five other documents and the run.**
Lines 33 and 69 say **1 189 passed / 0 failed / 21 ignored**; the measured value is **1 191**, and
`README.md:84`, `CHANGELOG.md:107`, `CONTRIBUTING.md:35`, `GOVERNANCE-REPORT.md:11,116` and
`TEST-MATRIX.md:8,24` all say 1 191. The document is not inventing a number: its cited evidence
`target/p09_full_test.log` genuinely contains 1 189, so it faithfully reports a run that **predates two
tests**. The root cause is structural — six documents hand-copy the same figure, so updating it takes six
edits and one was missed. *Disposition: record the gate numbers in one machine-read place and have the
documents reference it, or fix line 33/69 to 1 191 and note the log predates the two added tests.*

**M3 — Every gate "retained run" is an untracked, git-ignored file.**
`RELEASE-CANDIDATE.md:69` claims each gate has "a retained run" and names `target/p09_full_test.log`,
`target/p09_full_test2.log`, and `gate_fmt/clippy/aarch64/deny.log`. All exist on this machine and **none
is tracked** (`git ls-files target` returns 0); `target/` is git-ignored at `.gitignore:2`. The committed
link checker **exempts** this prefix (`check_links.py:84`), so the audit reports 0 findings while a release
document's acceptance evidence remains unreproducible for any reader of the public repository. 23 of the
exempted citations are of this kind, including `docs/research/provenance.md`'s probe sources
(`RandomProbe.java`, `packets_from_jar.py`) and `BENCHMARK-BASELINE.md`'s `target/p09_perf_release.log`.
This is the same class of issue recorded in the pre-governance audit; governance **documented the
exemption rather than restoring reproducibility**. *Disposition: commit the evidence logs and the jar-probe
sources under `docs/` or `tools/`, or mark the citations as "local run, not distributed" so a reader knows
the claim is not independently checkable.*

### LOW

**L1 — `GOVERNANCE-REPORT.md:19-20` file counts are stale.** It states "256 tracked files (203 under
`crates/`, **40 under `docs/`** = 38 markdown + 2 tsv …)". Measured: **251 tracked files**, `crates/` 203
(exact), **`docs/` 27**. The 40 is the *pre-governance* docs count (40 − 16 deleted + 3 added = 27), so the
sub-total and therefore the total were not recomputed after the deletions.

**L2 — the same document's "249 text files" does not match its own tool.** `GOVERNANCE-REPORT.md:117`
reports "encoding: 249 text files"; `check_encoding.py` now prints **240** (the script's filter includes
`.hex`, which is why 240 and not 237). 249 is the pre-governance figure with `.hex` included.

**L3 — the CI claim is stale and understates the history.** `GOVERNANCE-REPORT.md:15-18` says the workflow
has run "**both runs green** (7m6s, 6m45s)". `gh run list` shows **6 runs: 4 success, 2 failure**
(`34683857279`, `34683247774`), both failures being docs-audit/archive-tag issues that the two follow-up
commits fixed. HEAD's run is green, so the outcome is sound; the sentence describes the state at its
writing time and should say so or be updated.

**L4 — `GOVERNANCE-REPORT.md:94` says "Ten code comments still cite retired reports".** Measured: **13
references across 12 files** (`smelting_data.rs`, `recipe.rs`, `registry_data.rs`, `level.rs`, `lib.rs`,
`commands.rs`, `lifecycle.rs`, `metrics.rs`, `vanilla_smelting.rs` ×2, `entity_lifecycle.rs`,
`pi_profile.rs`, `survival_e2e.rs`). These resolve through `git show phase-09-final:…`, so they are
correctly recorded as pending rather than broken; only the count is wrong.

## 3. Task-by-task judgement (158 rows)

**Method, and its limits.** The task index is the authoritative list; it was parsed mechanically (158 rows,
no duplicates, contiguous per phase) and every row below is generated from that parse.

Evidence kinds, strongest first:

- **falsified** — this audit additionally broke the mechanism the row depends on and confirmed the covering
  test fails, then restored the file (Lane C). 9 rows.
- **suite** — the row's crate(s) and their named suites were re-run green **by this audit**, and the
  implementing module was confirmed present in the tracked tree. This is "the artifact exists and its
  crate's tests pass", which is deliberately **weaker** than "I proved this behaviour". 149 rows.

An earlier draft of this table would have marked all 158 as verified; that would have been an over-claim of
the same species as the ones this audit found. What can be said precisely: **no row failed**, every row's
owning crate and suite exist and pass, and 10 rows were adversarially probed. Rows whose wording the audit
found weaker than reality carry a caveat in the last column.

| ID | Task | Judgement | Evidence |
|---|---|---|---|
| P00-01 | Inventory local reference repositories, commits, branches and paths | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-02 | License and provenance audit of Paper/Pumpkin/Valence/Minestom | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-03 | Establish Minecraft 26.1.2 protocol/version baseline | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-04 | Map protocol states, packet families and login/play lifecycle | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-05 | Compare architecture and threading models across references | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-06 | Compare world/chunk/persistence approaches | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-07 | Identify behaviorally critical Vanilla parity domains | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-08 | Define differential-testing strategy and baseline harness shape | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-09 | Produce initial dependency/license policy | suite | crate tests re-run green (docs/research, docs/adr) |
| P00-10 | Write ADR-0001 system architecture and risk register | suite | crate tests re-run green (docs/research, docs/adr) |
| P01-01 | Initialize workspace and package metadata | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-02 | Pin stable Rust toolchain and reproducible build settings | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-03 | Configure fmt/clippy/lints and CI | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-04 | Establish logical crate boundaries with real initial ownership | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-05 | Implement error taxonomy and propagation policy | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-06 | Implement config schema/validation (TOML) | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-07 | Implement tracing/structured logging | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-08 | Implement server lifecycle and graceful shutdown | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-09 | Implement deterministic clock/tick abstraction | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-10 | Establish shared test-support/fixture harness | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-11 | Add x86_64/aarch64 build verification | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-12 | Foundation review and hardening | suite | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P02-01 | TCP/Tokio connection lifecycle | suite | crate tests re-run green (protocol, network) |
| P02-02 | VarInt/VarLong codecs with hostile-input limits | suite | mc-protocol lib (126) re-run green; hostile VarInt/frame corpora present in the crate |
| P02-03 | Frame reader/writer and size limits | suite | crate tests re-run green (protocol, network) |
| P02-04 | Packet registry/version strategy | **falsified** | Lane C C1: broke CHAT_COMMAND id to 8 -> packet_ids FAILED (2 failed) |
| P02-05 | Handshake/status packets | suite | crate tests re-run green (protocol, network) |
| P02-06 | Login state machine | suite | crate tests re-run green (protocol, network) |
| P02-07 | Offline authentication/GameProfile path | suite | crate tests re-run green (protocol, network) |
| P02-08 | Online authentication provider boundary | suite | crate tests re-run green (protocol, network) |
| P02-09 | Compression negotiation | suite | crate tests re-run green (protocol, network) |
| P02-10 | Play-state packet plumbing | suite | crate tests re-run green (protocol, network) |
| P02-11 | Packet fixture/golden harness | suite | crate tests re-run green (protocol, network) |
| P02-12 | Protocol fuzz/property tests | suite | crate tests re-run green (protocol, network) |
| P02-13 | Minimal test client | suite | crate tests re-run green (protocol, network) |
| P02-14 | Client login/play E2E test | suite | crate tests re-run green (protocol, network) |
| P02-15 | Connection rate/resource limits | suite | crate tests re-run green (protocol, network) |
| P02-16 | Protocol review and conformance report | suite | crate tests re-run green (protocol, network) |
| P03-01 | NBT primitive model | suite | crate tests re-run green (nbt, persistence) |
| P03-02 | NBT parser | suite | crate tests re-run green (nbt, persistence) |
| P03-03 | NBT writer | suite | crate tests re-run green (nbt, persistence) |
| P03-04 | NBT malformed/fuzz tests | suite | crate tests re-run green (nbt, persistence) |
| P03-05 | Compression adapters | suite | crate tests re-run green (nbt, persistence) |
| P03-06 | Anvil region header/read path | suite | crate tests re-run green (nbt, persistence) |
| P03-07 | Region chunk write path | **falsified** | Lane C C2: skipped the palette re-pack -> restart FAILED (3 failed) |
| P03-08 | Level metadata model | suite | crate tests re-run green (nbt, persistence) |
| P03-09 | Dimension/world metadata model | suite | crate tests re-run green (nbt, persistence) |
| P03-10 | In-memory chunk serialization boundary | suite | crate tests re-run green (nbt, persistence) |
| P03-11 | Dirty chunk tracking | suite | crate tests re-run green (nbt, persistence) |
| P03-12 | Autosave scheduling | suite | crate tests re-run green (nbt, persistence) |
| P03-13 | Atomic/ordered save semantics | **falsified** | Lane C C7: removed the live level.dat before the rename -> **no test noticed** (finding H1) — **caveat:** finding H1: "atomic tmp->rename" is implemented and documented but no test in mc-persistence detects a non-atomic ordering |
| P03-14 | Restart/load integration tests | suite | crate tests re-run green (nbt, persistence) |
| P03-15 | Corruption/failure recovery tests | suite | crate tests re-run green (nbt, persistence) |
| P03-16 | Persistence review and compatibility report | suite | crate tests re-run green (nbt, persistence) |
| P04-01 | Registry/data bootstrap for core block/item IDs | **falsified** | Lane C C5: moved the build-tree fixture fallback first -> the registry search-order test FAILED (1 of 14) |
| P04-02 | World/dimension runtime shell | suite | crate tests re-run green (registry, world, entity, server) |
| P04-03 | Chunk loading/streaming integration | suite | crate tests re-run green (registry, world, entity, server) |
| P04-04 | Player entity/state model | suite | crate tests re-run green (registry, world, entity, server) |
| P04-05 | Spawn/respawn lifecycle | suite | crate tests re-run green (registry, world, entity, server) |
| P04-06 | Player movement and server authority | suite | crate tests re-run green (registry, world, entity, server) |
| P04-07 | Basic collision | suite | crate tests re-run green (registry, world, entity, server) |
| P04-08 | Block state storage/query/update | suite | crate tests re-run green (registry, world, entity, server) |
| P04-09 | Block break validation | suite | crate tests re-run green (registry, world, entity, server) |
| P04-10 | Block place validation | suite | crate tests re-run green (registry, world, entity, server) |
| P04-11 | Basic interaction packets/events | suite | crate tests re-run green (registry, world, entity, server) |
| P04-12 | Item stack/slot primitives | suite | crate tests re-run green (registry, world, entity, server) |
| P04-13 | Player inventory | suite | crate tests re-run green (registry, world, entity, server) |
| P04-14 | Health/hunger/XP baseline | suite | crate tests re-run green (registry, world, entity, server) |
| P04-15 | Death/respawn | suite | crate tests re-run green (registry, world, entity, server) |
| P04-16 | Survival vertical-slice E2E test | suite | crate tests re-run green (registry, world, entity, server) |
| P04-17 | Save/reload vertical-slice test | suite | crate tests re-run green (registry, world, entity, server) |
| P04-18 | TPS baseline and vertical-slice review | suite | crate tests re-run green (registry, world, entity, server) |
| P05-01 | Fixed 20 TPS scheduler integration | suite | crate tests re-run green (simulation, entity) |
| P05-02 | System ordering and deterministic tick phases | suite | crate tests re-run green (simulation, entity) |
| P05-03 | Entity ID/lifecycle manager | suite | crate tests re-run green (simulation, entity) |
| P05-04 | Spatial query primitives | suite | crate tests re-run green (simulation, entity) |
| P05-05 | Physics/collision expansion | suite | crate tests re-run green (simulation, entity) |
| P05-06 | Damage and invulnerability rules | suite | crate tests re-run green (simulation, entity) |
| P05-07 | Effects/status conditions | suite | crate tests re-run green (simulation, entity) |
| P05-08 | Item entities/pickup | suite | crate tests re-run green (simulation, entity) |
| P05-09 | Projectile baseline | suite | crate tests re-run green (simulation, entity) |
| P05-10 | Scheduled block/entity ticks | suite | crate tests re-run green (simulation, entity) |
| P05-11 | Mob spawn rules | suite | crate tests re-run green (simulation, entity) |
| P05-12 | Basic hostile mob AI | suite | crate tests re-run green (simulation, entity) |
| P05-13 | Basic passive mob AI | suite | crate tests re-run green (simulation, entity) |
| P05-14 | Pathfinding baseline | suite | crate tests re-run green (simulation, entity) |
| P05-15 | Entity synchronization | suite | crate tests re-run green (simulation, entity) |
| P05-16 | Entity/persistence integration | suite | crate tests re-run green (simulation, entity) |
| P05-17 | Determinism regression scenarios | **falsified** | Lane C C6: zero-extended nextLong halves -> mc-simulation FAILED (1 failed) |
| P05-18 | Entity-heavy benchmark/review | suite | crate tests re-run green (simulation, entity) |
| P06-01 | Server-authoritative inventory transaction model | suite | crate tests re-run green (container, redstone) |
| P06-02 | Slot click validation | **falsified** | Lane C: inverted the insert split -> caught by a slot-ceiling test; the flood did NOT catch it (finding M1) |
| P06-03 | Container/session model | suite | crate tests re-run green (container, redstone) |
| P06-04 | Crafting grid baseline | suite | crate tests re-run green (container, redstone) |
| P06-05 | Furnace/smelting container baseline | suite | crate tests re-run green (container, redstone) |
| P06-06 | Item metadata/tag semantics | suite | crate tests re-run green (container, redstone) |
| P06-07 | Block entity lifecycle | suite | crate tests re-run green (container, redstone) |
| P06-08 | Hopper inventory transfer model | suite | crate tests re-run green (container, redstone) |
| P06-09 | Redstone state/update abstraction | suite | crate tests re-run green (container, redstone) |
| P06-10 | Neighbor/update scheduling | **falsified** | Lane C C4: discarded drained updates on a budget stop -> propagation FAILED (3 failed) |
| P06-11 | Power propagation baseline | suite | crate tests re-run green (container, redstone) |
| P06-12 | Repeater/comparator timing baseline | suite | crate tests re-run green (container, redstone) |
| P06-13 | Piston/observer family baseline | suite | crate tests re-run green (container, redstone) |
| P06-14 | Redstone containers/hoppers integration | suite | crate tests re-run green (container, redstone) |
| P06-15 | Inventory adversarial tests | **falsified** | Lane C: discarding the insert overflow destroyed items -> flood PASSED, slot-ceiling test FAILED (finding M1) |
| P06-16 | Redstone golden/differential tests | suite | crate tests re-run green (container, redstone) |
| P06-17 | Transaction/recovery review | suite | crate tests re-run green (container, redstone) |
| P06-18 | Automation workload benchmark/review | suite | crate tests re-run green (container, redstone) |
| P07-01 | Command tree abstraction | suite | crate tests re-run green (command, data, worldgen) |
| P07-02 | Dispatcher/parser | suite | crate tests re-run green (command, data, worldgen) |
| P07-03 | Registries/tags/data loader | suite | crate tests re-run green (command, data, worldgen) |
| P07-04 | Permissions/source context | suite | crate tests re-run green (command, data, worldgen) |
| P07-05 | Basic vanilla commands | suite | crate tests re-run green (command, data, worldgen) |
| P07-06 | Selector parsing | suite | crate tests re-run green (command, data, worldgen) |
| P07-07 | Execute context | suite | crate tests re-run green (command, data, worldgen) |
| P07-08 | Data/function execution baseline | suite | crate tests re-run green (command, data, worldgen) |
| P07-09 | Recipe data loading | suite | crate tests re-run green (command, data, worldgen) |
| P07-10 | Loot data loading | suite | crate tests re-run green (command, data, worldgen) |
| P07-11 | Advancement/statistics baseline | suite | crate tests re-run green (command, data, worldgen) |
| P07-12 | Data pack discovery/validation | suite | crate tests re-run green (command, data, worldgen) |
| P07-13 | Worldgen seed/context pipeline | **falsified** | Lane C C8: dropped the generation gate -> scenario_vanilla::a_borrowing_game_cannot_generate FAILED |
| P07-14 | Noise/terrain baseline | suite | crate tests re-run green (command, data, worldgen) |
| P07-15 | Biome/features baseline | suite | crate tests re-run green (command, data, worldgen) |
| P07-16 | Structures/placement baseline | suite | crate tests re-run green (command, data, worldgen) |
| P07-17 | Existing-world-first generation integration | suite | crate tests re-run green (command, data, worldgen) |
| P07-18 | Command/data/worldgen parity tests | suite | crate tests re-run green (command, data, worldgen) |
| P07-19 | Differential scenario expansion | suite | crate tests re-run green (command, data, worldgen) |
| P07-20 | Parity matrix review and gap triage | suite | crate tests re-run green (command, data, worldgen) |
| P08-01 | Resource/config guardrails | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-02 | Structured operational metrics | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-03 | Save barrier and shutdown coordinator | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-04 | systemd service/unit | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-05 | Backup/restore helper | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-06 | Connection/resource exhaustion defenses | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-07 | Packet/decompression abuse defenses | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-08 | Command/admin safety review | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-09 | Pi benchmark harness | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-10 | 10-player workload driver | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-11 | Chunk-generation benchmark | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-12 | Persistence benchmark | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-13 | CPU/RAM/TPS/MSPT profile run | suite | crate tests re-run green (server, deploy, docs/operations) — **caveat:** Pi profile numbers exist in BENCHMARK-BASELINE.md; re-running them needs the Pi (Lane E blocked) |
| P08-14 | Targeted performance fixes | suite | crate tests re-run green (server, deploy, docs/operations) |
| P08-15 | Operational runbook | suite | crate tests re-run green (server, deploy, docs/operations) — **caveat:** RUNBOOK.md present and its install steps were corrected by governance; Pi application unverified (Lane E blocked) |
| P08-16 | Pi hardening review and acceptance report | suite | crate tests re-run green (server, deploy, docs/operations) |
| P09-01 | Full test matrix execution | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-02 | Protocol conformance sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-03 | Persistence compatibility sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-04 | Survival regression sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-05 | Entity/redstone regression sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-06 | Commands/data/worldgen regression sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-07 | Security adversarial sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-08 | Pi performance release sweep | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-09 | Reproducible release build | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-10 | Release documentation | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) — **caveat:** finding M2: RELEASE-CANDIDATE.md states 1 189 passed; the run states 1 191 (five other documents say 1 191) |
| P09-11 | Known divergence catalog | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) — **caveat:** KD-01..KD-38 all resolve in PARITY-MATRIX.md (verified); row tags present |
| P09-12 | Rust-native plugin boundary ADR | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-13 | Independent final review | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-14 | Release candidate acceptance report | suite | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |

## 4. Falsification experiments (Lane C)

Each experiment: break the mechanism, run the covering test, **require FAIL**, restore, verify the restore
byte-for-byte. A claim whose test survives its mechanism being broken is not evidence.

| # | Claim probed | Break site | First probe | Widened to the whole crate | Restore |
|---|---|---|---|---|---|
| C1 | `chat_command` packet id is 7 | `crates/protocol/src/ids.rs:112` → `= 8` | **FAIL** (2 of 4 failed) | — | exact |
| C2 | palette widened past its bit width is re-packed | `crates/persistence/src/chunk.rs:352` skip re-pack | **FAIL** (3 of 7 failed) | — | exact |
| C3 | 2 000-click flood cannot create/destroy items | `crates/container/src/menu.rs:601` invert the split | **PASS (not caught)** | FAIL via `a_slot_ceiling_below_the_item_limit_is_respected` | exact |
| C4 | a budget stop loses no updates | `crates/redstone/src/propagation.rs:771` discard drained work | **PASS (not caught)** | FAIL via 3 tests in `propagation.rs` | exact |
| C5 | fixture search prefers the executable's side | `crates/registry/src/lib.rs:121` build-tree first | **FAIL** (1 of 14 failed) | — | exact |
| C6 | `nextLong` sign-extends both halves | `crates/simulation/src/random.rs:115` zero-extend | first probe **did not compile** (`u32::cast_unsigned` absent); rerun: **FAIL** (`matches_the_jdk_for_known_seeds`) | — | exact |
| C7 | `level.dat` replaced by rename (crash-safe) | `crates/persistence/src/save.rs:123` delete live file first | **PASS (not caught)** | **STILL PASSES — `mc-persistence` 72+9+16+7 all green** | exact |
| C8 | a game that cannot read storage must not generate | `crates/server/src/game.rs` drop `can_read_stored_chunks` gate | **FAIL** (`a_borrowing_game_cannot_generate`) | — | exact |
| C9 | per-test `TempDir` tag uniqueness (P09 flake fix) | `crates/test-support/src/fixtures.rs:136` | mechanism probe by replica: 2 000 same-tag constructions → **2 000 distinct paths with the nanos component, 1 without** | not deterministically falsifiable: the defect it prevents is a race | n/a |
| C10 | conservation on the insert overflow path | `crates/container/src/menu.rs:604` discard the overflow | **flood PASSED while items were destroyed**; slot-ceiling test FAILED | — | exact |

**Summary: 5 of 8 break-tests failed as required; 3 were caught only after widening the probe from one
named test to the whole crate; 1 (C7) was caught by nothing.** Two of my own probes were defective and are
recorded rather than hidden: C6's first break did not compile, and the C3/C7 "still passed" results were
initially indistinguishable from coverage gaps until the widened runs separated them. One further apparent
mismatch (`determinism` 6 vs 7) was **my script conflating two same-named suites** and is not a finding.

## 5. Lane E — Pi deployment

**Completed** (was `skipped(credential)`; the owner supplied access during remediation). Full record:
[AUDIT-07-LANE-E.md](AUDIT-07-LANE-E.md).

The device is `RPI5` at `169.254.77.10`, aarch64, kernel `6.18.39+rpt-rpi-2712`. Verified: the layout
matches RUNBOOK §1; the unit is **enabled** and its effective systemd settings match the documented table;
**start → Server List Ping → stop** completes with `protocol=775` / `version_name=26.1.2` on 25599 and a
0.2 s graceful stop to `inactive` with `ExecMainStatus=0`; the journal shows the documented
drain/save/close sequence and reproduces DataVersion 4790; and **every P09-Pi soak figure recomputes from
the archived raw data** (62 tick lines, 60 ten-player windows, p50/p95/p99 = 0.206/0.268/0.289 ms against
the documented 0.21/0.27/0.29, overruns 5, RSS 122.2 MB, CPU 1.00 %).

Lane E produced five further findings, recorded there and in the remediation report:

| ID | Severity | One-line |
|---|---|---|
| E1 | LOW | the recorded Pi binary size (4 524 624 B) contradicted its own reproduced SHA-256; measured 4 526 696 B |
| E2 | MEDIUM | three tracked files had CRLF worktree bytes against an LF index, violating the declared `eol=lf` policy; it made byte-identical fixtures compare as different, and caused exactly that false "deployment drifted" conclusion during this audit |
| E3 | LOW | the deployed unit file's comments are an older revision than the committed one (settings identical) |
| E4 | LOW | the deployed server loads no data pack, and RUNBOOK §1 never mentions the option — the acceptance world had bare terrain |
| E5 | LOW | `/var/backups/mc-server/` is empty; no backup was taken during acceptance |

The earlier note stands and is now the only remaining operational gap: the benchmarks run on this host and
pass under `--ignored` (4 + 2 tests), and the documented Pi figures are now **verified to come from a Pi**
rather than merely asserted.

## 6. Checked and clean (coverage statement)

- **Lane A**: task index parsed and counted independently — 158 tasks, 0 duplicate ids, contiguous per
  phase, 10 phases. Every row mapped to an owning crate and to named suites that exist.
- **Lane B**: all five gates re-run on a clean tree at `bf74123`; every claimed figure reproduced exactly;
  differential suites re-run against the real jar (data + jar + world all present); ignored-count
  arithmetic reconciled three independent ways.
- **Lane C**: 8 break-tests + 1 mechanism probe + 1 extra conservation probe; every temporary patch
  restored and byte-compared; no repository file left modified (`git status` clean after the lane).
- **Lane D**: both committed audit scripts run (exit 0) and their **scope read and tested**, not just
  executed; stricter all-markdown link check (0 broken); `KD-01..KD-38` completeness; 49 defect rows with
  no duplicates; 30/30 per-suite counts verified; the "529 chunks + vanilla boots" claim reproduced with
  the vanilla server's own log; CI history read from GitHub; stale-count hunt across six documents.
- **Lane E**: **completed** on 2026-09-12 once access was supplied — layout, unit, start/stop cycle, protocol smoke, journal sequence, archived soak figures recomputed from raw data, fixture and binary hashes, manifest hashes. Five findings; see [AUDIT-07-LANE-E.md](AUDIT-07-LANE-E.md).

Not covered by this audit, stated so the main auditor knows the boundary: behavioural re-derivation of all
149 suite-verified rows; the ~90-vs-8 command-coverage claim against a real client; the
`docs/research/provenance.md` citations against a reference clone (the clones are present but were not
re-read); and anything requiring a real Java client (KD-38, which the repository itself flags as its
unresolved acceptance boundary).

## 7. Disposition summary

| ID | Severity | One-line |
|---|---|---|
| H1 | HIGH | save atomicity implemented and documented, no test anywhere in `mc-persistence` detects a non-atomic ordering |
| H2 | HIGH | `check_links.py` covers `docs/` only; the 4 root documents including README are unchecked and the 0-finding result is silent about scope |
| M1 | MEDIUM | the 2 000-click flood never reaches the overflow path its claim names; a different test covers it |
| M2 | MEDIUM | `RELEASE-CANDIDATE.md` says 1 189 passed; five other documents and the run say 1 191 (its cited log is a pre-two-test run) |
| M3 | MEDIUM | every "retained run" and jar-probe source is untracked under git-ignored `target/`, and the link checker exempts that prefix, so release evidence is unreproducible for readers |
| L1 | LOW | governance report: 256 tracked files / 40 in `docs/` → measured 251 / 27 |
| L2 | LOW | governance report: "249 text files" → tool prints 240 |
| L3 | LOW | governance report: "both runs green" → 6 runs, 4 success / 2 failure (HEAD green) |
| L4 | LOW | governance report: "ten code comments" cite retired reports → 13 references in 12 files |

No BLOCKER was found. Nothing in the audit contradicts a **capability** claim: every feature the README
advertises exists, its crate's tests pass, and the deepest end-to-end claim reproduced from scratch. The
findings are about claims whose *evidence* is weaker than their wording — untested atomicity, an
unstated tool scope, a flood that names paths it does not exercise, and hand-copied numbers that drifted.

Reproduction: every command is quoted inline above. Lane B logs are `target/audit_lane_b_test.txt`,
`target/audit_lane_b_diff.txt`, `target/audit_gate_{fmt,clippy,aarch64,deny}.txt`; Lane C results are
`target/audit_lane_c.json`; Lane A's table is `target/audit_lane_a.json`. Those paths are machine-local by
the same rule M3 describes.
