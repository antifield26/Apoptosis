# Phase 09 Report — Conformance, Release Candidate and Plugin Readiness

Date: 2026-09-12. Scope: `P09-01..P09-14` per `tasks/TASK-INDEX.md`.
HEAD at write time: `bf118c8` plus the uncommitted P09 work (this report,
AUDIT-06, the three new documents, the matrix updates, and two code fixes —
`metrics.rs` and `pi_profile.rs`, both verified by the full runs cited below;
the numbers are from this session's retained logs, not accumulated — the same
re-derivation rule as Phase 08, and AUDIT-06 confirmed it was applied).

Exit gate (P09): full regression suite passes; conformance report produced;
release artifacts reproducible; known divergences explicitly documented.
Verdict: **met, with the release-publish clause split honestly** — the build is
reproducible and recorded, but no artifact is published because the project has
no license decision (ADR-0001 R-09), and the 20 TPS verdict does not exist
because no Pi 5 exists in this environment. Both are owner-side gaps, recorded
rather than papered over. See §5.

## 0. What this phase was for

Phases 00–08 built and hardened the server; nothing had yet *swept the whole
board in one pass*. Phase 09 re-derives every claim against the current tree
(five gates, seven differential suites, both build profiles), produces the
conformance and divergence documents a release consumer actually reads, fixes
the one known flaky (P08 §2.1), builds the release binary reproducibly and
smokes it over a real socket, writes the promised plugin-boundary ADR
(ADR-0001 D-07), and hands the owner a release decision with the honest parts
named. It deliberately does **not** publish a release, claim 20 TPS, or pretend
a systemd unit was installed: those are decisions and hardware this environment
does not have.

## 1. Deliverable map

| Task | Status | Evidence |
|---|---|---|
| P09-01 full test matrix execution | **DONE** (previous commit `bf118c8`) | TEST-MATRIX NUL repair + Phase 08 section; totals re-derived again this session (see P09-T01 row) |
| P09-02 protocol conformance sweep | **DONE** | `packet_ids` 4 (jar-extracted), `fixtures` 4, `keepalive` 1, `login_tolerance` 2, `e2e_login_play` 6, hostile VarInt/frame corpora inside the protocol/network libs — all green 2026-09-12 |
| P09-03 persistence compatibility sweep | **DONE** | `restart` 7, `corruption` 16, `anvil_fixture` 9, `vanilla_chunk` 4, and `vanilla_differential` 2 green **with the jar and world set**: 529 vanilla chunks decoded from the real 26.1.2 world, rewritten through our writer, and the real vanilla server booted on the rewritten world, preserved our diamond-block marker and re-saved every dimension (`p09_vanilla_diff.log`) |
| P09-04 survival regression sweep | **DONE** | `survival_e2e` 7, `network_game_bridge` 4, `vanilla_chunk` 4 — join/stream/move/break/place/death/save-reload over real sockets |
| P09-05 entity/redstone regression sweep | **DONE** | `entity_lifecycle` 9, redstone suites 47, container/block-entity/duplication 17 |
| P09-06 commands/data/worldgen regression sweep | **DONE** | command/execute/function/pack suites 52, worldgen + golden suites 37, six differential suites 13 |
| P09-07 security adversarial sweep | **DONE** | hostile-input classes green in the full run: non-terminating VarInt/frames, random-byte connections, slow-drip bound, registry reservation cap, command flood, `ops_e2e` 7, `inventory_duplication` 5 (2 000-click conservation), `corruption` 16, config guardrails |
| P09-08 Pi performance release sweep | **DONE (dev host, both profiles; no Pi)** | §P09-08 in `BENCHMARK-BASELINE.md`: release settled p50/p95/p99 **0.060/0.072/0.137 ms**, join-burst p99 13.3 ms (vs 390.5 debug), chunkgen 0.47 s, 81-chunk save 0.88 s. **The no-hardware substitute is written down**: `BENCHMARK-BASELINE.md` §4 is the exact Pi acceptance run (build on Pi → in-process benches → 30-min systemd soak → §13 capture → verdict rule) for whoever has the hardware. No 20 TPS claim exists (KD-35) |
| P09-09 reproducible release build | **DONE (build) / BLOCKED (publish)** | `cargo build --workspace --release --locked` green; binary 3 382 272 B, SHA-256 `36e3ab01…e659` recorded in §P09-09; **real-socket smoke ×2** (fresh world creates `level.dat`, status JSON `protocol:775`, pong echo; second start reuses the world) — transcript `p09_smoke.log`. **Nothing is published**: R-09 makes distribution an owner decision; the build is reproducible, the release is not claimed |
| P09-10 release documentation | **DONE** | `docs/release/RELEASE-CANDIDATE.md`: what ships, pinned-toolchain build recipe, run instructions, claims table where every "No" cites a KD entry |
| P09-11 known divergence catalog | **DONE** | `docs/vanilla-parity/KNOWN-DIVERGENCES.md`: KD-01..KD-38 across four classes (intentional divergence / gap / unverified / boundary), every row sourced to the parity matrix or a phase report; AUDIT-06 content-matched ~23 of them against their sources |
| P09-12 plugin boundary ADR | **DONE** | `docs/adr/ADR-0005-plugin-boundary.md`: three named seams (tick events, command registration, datapack-function hook) pinned to code sites with explicit triggers; zero API types (grep-verified); the rejected alternatives named with reasons |
| P09-13 independent final review | **DONE** | `docs/phases/AUDIT-06-FINDINGS.md`: read-only adversarial subagent re-derived the claims itself; **PASS WITH FINDINGS** — 0 BLOCKER, 4 MEDIUM, 6 LOW, all dispositioned in the same commit |
| P09-14 release candidate acceptance report | **DONE (this document)** | §5 gate table |

## 2. Bugs found and fixed, in the order they were found

### 2.1 The flaky that was not noise — P08 §2.1 closed with a probe

The one-in-four `mc-server` lib failure (`cannot move .../level.dat.tmp into
place (os error 2)`) was root-caused before fixing, not after: a throwaway
probe replicated the `TempDir` name format (`tag+pid+nanos`) and hammered it —
**duplicate paths occurred in concurrent same-tag constructions (7 and 17
across two identical 160 000-construction runs; the retained log
`p09_probe_tempdir.log` records the 7)**, because the wall clock
quantises under parallel filesystem I/O. All three metrics tests shared the tag
`ops-metrics`, so one test's `WorldService::open` raced another test's
drop-time `remove_dir_all` at the *same* directory. Fix: `game(tag)` with a
distinct tag per test — the convention every other test in the tree already
follows. Verified: 10 consecutive green metric-suite runs
(`p09_metrics_repeat.log`) and two post-fix full-workspace runs, both exit 0.
The fix is deliberately in `metrics.rs` and not in `test-support`: the shared
helper's uniqueness contract is fine for distinct tags, and widening the change
would have been the unscoped edit P08 declined.

### 2.2 The harness that mislabeled its own profile

`pi_profile.rs` printed `build profile: dev` as a hardcoded string — harmless
when only debug runs existed, wrong the moment P09-08 ran it in release (the
log contradicted the document citing it; AUDIT-06 finding 3). Fixed: the line
is now derived from `cfg!(debug_assertions)`, and the release suite was re-run
after the fix so the cited log describes itself correctly.

### 2.3 The differential count that was smaller than the suite

The P08 evidence line said "13/13 across 6 suites" — but the persistence
differential is a **7th suite of 2 tests** that needs `MC_VANILLA_JAR` and
`MC_VANILLA_WORLD` besides `MC_VANILLA_DATA`; without them it fails by design
(a named panic, not a hang). Running it with the assets gives **15/15 across 7
suites**, including the strongest single claim in the repo: the real 26.1.2
vanilla server booted on a world our writer produced and re-saved every
dimension of it. Recorded so the sweep count and the suite list agree.

### 2.4 Numeric rot in a freshly written document

AUDIT-06 finding 1: the plugin ADR said "seven Vanilla commands" — the stale
P07-05 count — while the code registers nine tree nodes. Caught by the
independent review, fixed before commit. Lesson repeated: counts rot *while
being written*, not just while aging.

## 3. What the jar confirms

Same-day, all seven differential suites green against the extracted 26.1.2
data: 758/758 tags, 1 421 recipes (94 counted as unmodelled), 156 furnace rows,
real structure templates placed, the five-stage pipeline scenario, and — new
this phase — the full round trip in `vanilla_differential`: **529 vanilla
chunks decoded → rewritten by our writer → vanilla booted on the result → our
diamond-block marker preserved → all dimensions re-saved by vanilla**. That is
the persistence compatibility claim, now re-proven end to end in Phase 09.

## 4. Honest limitations added or carried by this phase

1. **No 20 TPS verdict** (KD-35): no Pi 5, no real client traffic. The release
   profile's settled p95 (0.072 ms) is a regression baseline on the wrong
   hardware, not the acceptance number. §4 of `BENCHMARK-BASELINE.md` is the
   prepared run for whoever has the hardware.
2. **No release artifacts** (R-09): the build is reproducible and hashed;
   publishing waits on the owner's license decision. `cargo deny` is green, so
   the *dependency* license posture is known even though the *project* license
   is not.
3. **No real-client acceptance** (KD-38): the protocol test client and the
   hand-rolled socket smoke are the partners; a Java client may still disagree
   with us somewhere the fixtures cannot see.
4. **Carried unchanged from P08**: backup/restore are library calls without a
   CLI (KD-37); the systemd unit is reviewed, never applied (KD-36); entity
   persistence/sync, lighting, and the container window family remain the
   largest gameplay gaps (KD-18/23/21).
5. **Known flaky list is now empty** — the P08 §2.1 item was the only one, and
   §2.1 of this report closes it with a probe plus retained logs.

## 5. Exit-gate verdict

| Gate clause | Evidence | Verdict |
|---|---|---|
| Full regression suite passes | 1 189 / 0 / 21 over 74 suites, two post-fix full runs exit 0; fmt/clippy/aarch64/deny all exit 0 same day, logs retained; differential 15/15 | **Met.** |
| Conformance report produced | This report + the P09 sweep rows in TEST-MATRIX + per-domain evidence in PARITY-MATRIX | **Met.** |
| Release artifacts reproducible | `--locked` + pinned 1.98.1 + committed lockfile; binary hash recorded; smoke ×2 over a real socket. **Publishing blocked by R-09 (owner license decision)** — nothing distributed | **Met for the build; distribution withheld by owner decision, stated in every document that mentions release.** |
| Known divergences explicitly documented | `KNOWN-DIVERGENCES.md` KD-01..38, sourced and review-audited; parity matrix unchanged where it is the source | **Met.** |

Phase 09 is therefore **DONE within its environment**, with two owner-side
decisions outstanding and named: the project license (R-09) and the Pi 5
hardware (KD-35). Both have prepared, written-down next steps that require no
re-derivation.

## 6. Method notes worth carrying forward

- **Reproduce the flaky before fixing it.** A one-in-four race fixed on
  speculation would have been a guess with a green run attached. The probe made
  the mechanism certain (17 collisions in 160 000), and the fix is one line of
  convention compliance.
- **A log that contradicts its own citation is a bug.** The release run
  printing "dev" (§2.2) survived P08 because the harness had never been run in
  release; first-run-in-a-new-configuration is when self-descriptions get
  audited.
- **Counts need sources even on day zero.** AUDIT-06's cheapest catch (§2.4)
  was a number written from memory into a brand-new ADR.
- **Retain what you cite.** Seven of the ten review findings were "the number
  is true but the artifact is not retained" — the evidence chain script now
  produces every cited log in one pass, so the next phase cannot repeat the
  class.

## 7. Post-report updates (2026-09-12, same day)

- **R-09 resolved**: the owner adopted **MIT** (ADR-0006; `LICENSE`, license
  fields in all 17 manifests, `cargo deny` no longer skips the workspace's own
  crates). The §1/§5 statements "publishing blocked on R-09" were true at
  write time and stand as history; `ADR-0001`'s R-09 row and the release
  documents now record the resolution. Publishing remains unstarted only
  because no distribution channel (git remote, registry) exists.
- **Pi 5 hardware became available** the same day (`antifield@10.130.136.226`,
  Raspberry Pi 5 Model B, Debian 13 trixie aarch64, 8 GB, microSD): the KD-35
  acceptance run of `BENCHMARK-BASELINE.md` §4 is being executed; results land
  as §P09-Pi in that file and in the KD-35 / parity-matrix updates.
