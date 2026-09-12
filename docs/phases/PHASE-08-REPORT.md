# Phase 08 Report — Pi Hardening and Operations

Date: 2026-09-12. Scope: `P08-01..P08-16` per `tasks/TASK-INDEX.md`.
HEAD at write time: `d8aae34` plus uncommitted P08-14/15/16 work (this report,
the runbook, the parity-matrix touch-ups); the numbers below are re-derived
from runs in this session, not accumulated (Audits 03 and 05 both found stale
totals in this project's documents).

Exit gate (P08): 10-player workload has baseline and target measurements;
systemd, graceful shutdown, save/recovery and rate limits are operational.
Verdict: **met on a dev host with named gaps; no Pi 5 run exists here, so no
20 TPS verdict exists.** See §5.

## 0. What this phase was for

Phases 04–07 built a server that ticks, simulates, transacts and generates —
but with product-decision constants where operations need measured ones, a
shutdown path with no observable barrier, no backup story, unnamed rate-limit
numbers, and benchmarks that mixed the join burst into every percentile.
Phase 08 makes the server operable: guardrails, metrics, shutdown ordering,
backup/restore, named exhaustion defenses, hostile-packet hardening, an admin
safety review, a benchmark harness with separated burst/settled figures, an
honest no-fix verdict where the evidence did not support one, a runbook, and
this acceptance report.

## 1. Deliverable map

| Task | Status | Evidence |
|---|---|---|
| P08-01 resource/config guardrails | **DONE** | `config.rs::validate` bounds (bind/max_players 1..=100/view 2..=32/compression/motd/world_dir) + `deny_unknown_fields`; 5 tests |
| P08-02 structured operational metrics | **DONE** | `metrics.rs::OperationalSnapshot` (ticks/mean/p50/p95/p99/worst/overruns/players/entities/chunks/dirty_chunks), one `tick metrics` line per 600 ticks; 3 tests incl. falsification-shaped ordering checks. Deliberately no CPU/RSS fields (no platform reader in-tree; adding one would be a fabricated measurement) |
| P08-03 save barrier and shutdown coordinator | **DONE** | `Running → Stopping → drain (5 s) → bounded final save (30 s) → Stopped`; `shutdown_passes_through_stopping_before_stopped`, `world_is_created_at_startup_and_saved_at_shutdown` |
| P08-04 systemd service/unit | **DONE (reviewed, never applied here)** | `deploy/mc-server.service` (TimeoutStopSec 60, MemoryMax 6G, LimitNOFILE 1024, no `Restart=` by decision). No Pi here to install it on — said out loud in the runbook. |
| P08-05 backup/restore helper | **DONE** | `backup.rs`: `backup_world`/`verify_backup`/`restore_world` (offline-only, whole-copy, manifest, overwrite guard); 5 tests |
| P08-06 connection/resource exhaustion defenses | **DONE** | Named gate (`GATE_HEADROOM_CONNECTIONS=8`, `MAX_CONNECTIONS_PER_IP=4`, `RECONNECT_REFILL_INTERVAL=250ms`), game-loop player cap with `bypassesPlayerLimit` bypass (`is_full_for`); `ops_e2e::a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator` probes all three outcomes |
| P08-07 packet/decompression abuse defenses | **DONE** | Slow-drip bound (`a_slow_drip_never_grows_the_buffer_without_bound`), registry reservation cap (`MAX_REGISTRY_ENTRIES=4096` + hostile-count test), region location-word ordering test (`the_location_word_is_written_last`, closes the Audit 05 downgrade) |
| P08-08 command/admin safety review | **DONE** | Flood bound proven (`a_command_flood_from_one_client_does_not_starve_the_tick`: 300 commands answered over ticks, sender stays); stale bypass comments corrected (see §2.4); `/op` cannot write `ops.json` — recorded, not fixed (authority decision, owner call) |
| P08-09 Pi benchmark harness | **DONE** | `pi_profile.rs` (4 ignored on-demand tests, generous 40 ms ceiling) |
| P08-10 10-player workload driver | **DONE** | Settled p50/p95/p99 **0.65/0.80/0.88 ms**, max 0.92 ms, 200 ticks, 0.14 s wall (this session; dev host, debug, driven loop) |
| P08-11 chunk-generation benchmark | **DONE** | 684 resident, 1 360 streamed, 10.8 s wall (this session) |
| P08-12 persistence benchmark | **DONE** | 81 dirty chunks in 5.2 s wall (≈64 ms/chunk debug), flags all cleared (this session) |
| P08-13 CPU/RAM/TPS/MSPT profile run | **DONE (dev host; gaps named)** | `profile_run`: wall 2.05 s, mean 10.2 ms, p50/p95/p99 0.66/0.78/370.31 ms, max 374.47 ms; broadcast lifetime mean dominates; overruns 46/240 are the join burst. Full record: `BENCHMARK-BASELINE.md` §P08-13. No CPU/RSS capture on this host. |
| P08-14 targeted performance fixes | **DONE (no code change; evidence below)** | §2.5: the only evidence-backed target (broadcast palette scan) was probed and does not reproduce on current code — the scan is per-section over ≤ dozens of ids, not per-chunk over 4 096 distinct ones. No benchmark evidence → no change (PHASE-08 prompt rule). |
| P08-15 operational runbook | **DONE** | `docs/operations/RUNBOOK.md`: install/config/observe/backup/restore/failure playbook/known gaps; every bound cites the constant that enforces it |
| P08-16 Pi hardening review and acceptance report | **DONE (this document)** | §5 verdict + §4 limitations; matrices updated (parity rows inline, TEST-MATRIX deferred to P09 — see §2.6) |

## 2. Bugs found and fixed, in the order they were found

### 2.1 The metrics test that collided with itself — P08-02 (flaky, observed)

Full-workspace runs failed `metrics::tests::after_ticks_...` once with
`cannot move .../level.dat.tmp into place ... (os error 2)`, while the same
test passes alone and in `-p mc-server --lib` isolation, and two subsequent
full-workspace runs passed (1 189/0/21). Root cause, stated not proven: every
metrics test builds `TempDir::new("ops-metrics")`, whose uniqueness is
`pid + nanos` — two tests in the same process can share a timestamp tick and
therefore a directory, so one test's `WorldService::open` (which writes
`level.dat` via tmp→rename) races the other's drop-time `remove_dir_all`.
Not fixed in P08: the failure is environmental (parallel tests sharing one
tag), the tag scheme belongs to `test-support` (all 16 crates' tests share
it), and a drive-by fix there would be exactly the unscoped change the
discipline forbids. Recorded so P09 can give each metrics test a distinct tag
or a mutex; the observation (one failure in four full runs, always this test,
never any other) is the evidence.

### 2.2 The persistence bench that measured nothing — P08-12

First version printed `0.000 s`: `Game::new` borrows storage, and
`save_all_owned` on a borrowing game **silently saves nothing** (documented on
the method). The bench now owns storage (`with_seed_and_storage`) and asserts
`elapsed >= 1 µs` — a probe that cannot pass while measuring nothing.

### 2.3 The workload that measured the solver, not the tick — P08-10

Per-tick `MovePlayerPos` for all 10 players gave p95 101 ms: the collision
solver, not tick overhead. The driver now sends rotation/swing/hotbar every
tick with one periodic real move (settled p95 0.76 ms). Both numbers are kept
in the commit message so the wrong one cannot be re-learned later.

### 2.4 Three stale bypass comments — P08-14 (doc-rot, severity: low)

`ops.rs` said in three places that `bypassesPlayerLimit` is "parsed and
reported but not enforced" — true when written (P07-04 §4 item 3), false since
P08-06 wired `is_full_for` through the join gate with an E2E probe. Fixed by
editing the three comments to name the enforcement site. This is the same
failure mode as §2.7/§2.8 of Phase 07 (a doc comment more confident than its
code, in the stale direction). The probe that caught it: re-reading every
`bypass` mention after the enforcement landed, rather than trusting the phase
summary.

### 2.5 The palette scan that is not the bottleneck — P08-14 (no-fix verdict)

The P08-13 baseline names broadcast as the dominant phase (72–140 ms lifetime
mean) and points at `vanilla_chunk_packet`'s per-block palette scan, so that
scan is the only evidence-backed optimisation target this phase owns. Reading
`game.rs:3096-3115` settles it without a benchmark: the loop is per **section**
(4 096 cells), and the linear `palette.iter().position` runs over the ids
seen **in that section** — dozens on real terrain, not 4 096 distinct ones.
Worst case per section is ~4 096 × dozens of integer compares, i.e. well under
a millisecond; the measured join-burst cost is chunk generation + NBT encode +
zlib + 64-chunks-per-tick streaming, not this loop. Replacing it with a
`HashMap` would trade a cache-hot linear scan for hashing on every block with
no measured win — a blanket performance hack, which the PHASE-08 prompt
forbids. Verdict: no code change. The honest follow-up is a Pi-side profile
that attributes broadcast time to generation vs. encode vs. streaming (P09);
until that exists, touching this loop is guessing.

### 2.6 TEST-MATRIX.md is binary-unreadable — P08-16 (deferred, not fixed)

`docs/testing/TEST-MATRIX.md` is 43 226 bytes with one raw NUL at offset 3 432
(inside a P07-T13 cell that inlined a `\x00` byte into the prose). The Read
tool refuses it as binary, so the P08 rows were never appended there. Fixing
it means byte-surgery on a 277-line matrix via a scratch script — exactly the
PowerShell-quoting/UTF-8 shape that destroyed `provenance.md` in Phase 07 —
and any mistake corrupts the whole file with no test to catch it. Deferred to
P09-01 (full test matrix execution), which owns that file and can re-derive
its totals in the same pass. The P08 evidence lives in this report (§1) and
in `BENCHMARK-BASELINE.md` §P08-13 instead; nothing is claimed in the matrix
that is not claimed here.

## 3. What the jar confirms, replacing guesswork

Nothing new in P08 beyond P07's census (758 tags, 1 421 + 94 recipes, 1 326
loot tables, 1 617 advancements, 1 182/1 202 structures). P08's jar contact is
the six differential suites re-run green against
`target/vanilla-26.1.2/extract/data/minecraft` in this session: `vanilla_pack`
1, `vanilla_data` 1, `vanilla_smelting` 1, `structure_pack` 6,
`scenario_vanilla` 2, `structure_wiring` 2 — **13 passed, 0 failed**.

## 4. Honest limitations added or carried by this phase

1. **No 20 TPS verdict.** No Pi 5, no release profile, no real client traffic
   in this environment. The dev-host settled tick (p95 ≈ 0.8 ms) has ~60x
   headroom, but headroom on the wrong hardware is not a verdict.
2. **No CPU/RSS capture.** Deliberately absent from `OperationalSnapshot` (no
   platform reader in-tree); Pi RSS comes from `systemd-cgtop`/`journalctl`
   during a real run.
3. **Online mode is a fail-fast boundary**, not auth. Enabling it is an error
   by design; there is no handshake to validate beyond the refusal.
4. **Backup/restore are library calls** awaiting a CLI; the systemd unit was
   reviewed by reading, never applied to a real Pi here.
5. **Flaky metrics-test collision** (§2.1): one shared `TempDir` tag across
   three parallel tests; P09 should scope the tags.
6. Carried unchanged: furnace smelts from the hand-written baseline; loot /
   advancements load but never fire; redstone not wired into the tick loop;
   single-chunk structure subset; no shipwrecks; `/op` cannot write `ops.json`;
   microSD is not a performance target.

## 5. Exit-gate verdict

| Gate clause | Evidence | Verdict |
|---|---|---|
| 10-player workload has baseline and target measurements | `tick_baseline` (P04/P05) + `pi_profile` 4 tests with settled-vs-burst separated figures; `BENCHMARK-BASELINE.md` §P08-13 | **Met (dev host).** Baselines exist; Pi targets await hardware. |
| systemd operational | Unit reviewed line-by-line in the runbook; shutdown path it relies on tested | **Met by review, not by run.** No Pi here. |
| graceful shutdown operational | `Running → Stopping → drain → bounded save → Stopped`, two lifecycle tests | **Met.** |
| save/recovery operational | tmp→rename + header-word-last both tested; bounded shutdown save; dirty flags kept on failure; backup/verify/restore 5 tests | **Met.** |
| rate limits operational | Named socket gate + game-loop player cap + bypass, each probed; flood/command/registry/drip tests green | **Met.** |

Phase 08 is therefore **DONE within its environment**: every task has an
implementation, a probe that can fail, and a document that says what was not
done. The Pi 5 run is P09's first job, not an asterisk on this phase.

## 6. Method notes worth carrying forward

- **Re-read the code the summary describes.** §2.4's doc-rot survived two
  commit messages that both described the enforcement correctly. Summaries are
  not evidence.
- **A benchmark that cannot fail measures nothing.** The 0.000 s save (§2.2)
  and the 101 ms "tick overhead" (§2.3) both passed while proving the wrong
  thing. Each now carries the assertion that would have caught it.
- **Separate the burst from the settled state.** One percentile over a run
  containing the join burst describes neither. `pi_profile` quotes both,
  always.
- **Flaky is a finding, not noise.** §2.1's single failure names the shared
  tag, the racing syscalls, and the reason it was left for P09 — which is
  more useful than a green re-run with no record.
