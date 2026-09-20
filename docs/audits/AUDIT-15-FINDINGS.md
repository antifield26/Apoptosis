# AUDIT-15 — P00–P15落实复核 (basis `11c4f2e` + remediation `afb08b8`)

Five-lane read-only cluster (A–E, weights below) plus a synthesizer-run
falsification pass. Verdicts: **confirmed** (fresh instrument, re-run by the
lane or the synthesizer unless labelled otherwise), **refuted**, **not
checkable** (reason). P00–P15 are all落实; the carried opens are pre-existing
gaps, none a P15 regression.

## Why these weights

Confirmed-finding density across AUDIT-07..14 concentrates in protocol
strictness (A-01/A-02/A-03, the four AUDIT-10 wire defects) and persistence
ordering/loss (B-01 data-loss grade, B-02), and P15 touched protocol hardest
(split, intents, LpVec3, metadata, config types). Hence protocol 30,
world/persistence/simulation 25, server/gameplay/commands 20,
entity/data/NBT/worldgen 15, tests/docs/gates 10 (plus the P00–P15 spine).
Lanes were read-only; all tree mutations below are the synthesizer's, each
with its pinning instrument.

## Per-claim table (P00–P15 exit gates)

| Phase | Verdict | Evidence |
|---|---|---|
| P00 | confirmed | Lane E: third-party SHAs honestly UNKNOWN (recorded, not hidden), 26.1.2 facts, 7 ADRs, clean-room policy |
| P01 | confirmed | Lane E: toolchain 1.98.1 pinned ×3 jobs, x86_64+aarch64 CI, infra files present |
| P02 | confirmed | Lane A: offline login→play over TCP re-read, hostile-packet tests cited, online-ON fail-fast path read (Operational, pre-bind) |
| P03 | confirmed | Lane B: anvil_fixture 9/9, reconnect 5/5, B-01 guard present, corruption suite 16/16 |
| P04 | confirmed | Lane C (slice exists) + synthesizer P15-08 re-run survival_e2e 27/27 |
| P05 | confirmed | Lane B: simulation 18/18, phase order, determinism replays; worst-phase field present |
| P06 | confirmed | Lane C: container_e2e 6/6, inventory_duplication 5/5 re-run |
| P07 | confirmed | Lane D: mc-data 135 green, command framework 93 green |
| P08 | confirmed | Lane E (light): Pi §§P09/P14/P15 records, service file, shutdown/save/rate tests cited |
| P09 | confirmed | Lane E: RELEASE-CANDIDATE, P09-09 SHAs, tag v0.2.0 present |
| P10 | confirmed | Lane A: parity-matrix acceptance current; KD-38 three opens disclosed, not hidden |
| P11 | confirmed | Lane D: loot 49/49, baseline present; entity lifecycle green (P15-08) |
| P12 | confirmed | Lane C: container suites; A12-01/02 closed (see opens); carried container opens below are pre-existing |
| P13 | confirmed | Lane B: redstone differential 13/13 + propagation/power/model suites green |
| P14 | confirmed | Lane C: admin 13/13 incl. rollback + lock re-runs; tag + soak record (E) |
| P15 | confirmed | P15-08 gate + this audit's re-runs: worst-phase test, packing/random units, edge greps, 14 message pins, container/entity suites, region/nbt/fixture/sweep/cow pins, six e2e falsification suites |

## Lane verdicts (full reports in session record)

- **A (30%)**: 17 claim rows confirmed, 1 refuted (corpus 6273→6258, fixed). New: High corpus figure (fixed), Medium packet_ids explicit line (fixed), Low close-u8→VarInt (fixed), Low OpenScreen widths (carried), Info lp_vec3 pins (fixed).
- **B (25%)**: all rows confirmed. New: 2 Info (scanner path note, rename note). Opens re-verified: B-01..B-06 closed (B-07 absence-claim not-checkable by construction); A12-06/07/08/09/10/12 + BE-unload leak still open (carried).
- **C (20%)**: 16 rows confirmed. No new defects; 2 Info (remaining expects sampled benign, Unmodelled handler noise reduction intended). C-08 still open perf-only (carried).
- **D (15%)**: all unit rows confirmed, 1 refuted (loot.rs:246 stale line, fixed). New: Low D-01 exact pin (fixed), Info variant-breadth judgment (kept, per-id capture desired for P18), Info E-02 robustness (no action).
- **E (10%+spine)**: docs/counts rows confirmed save F1–F3 (fixed/carried); 16-row spine assembled, defers resolved by lanes above. Opens: E-01 process-closed, E-02 re-verified green, E-03 half-open, E-05 open-candidate-moot.

## Falsification pass (synthesizer, all reverted, tree clean)

- B-02 order swap → new test red, end-state test green (third demonstration including P15-07 and P15-08).
- PlayIntent enforcement disabled → tick-end padded test red (pins the refusal; move/click paddings are Packet-level checks, unaffected).
- LpVec3 old `(magnitude & 3) != 0` restored → sweep AddEntity mismatches return, sweep red.
- No perturbation left in the tree (`grep FALSIFICATION` empty; `git status` clean before doc commits).

## Historical opens disposition

Closed-verified this round: A-01, A-02, A-03, A12-01, A12-02, A12-04, B-01..B-06 (minus B-07 absence), C-01, C-02, C-04, C-06, D-01..D-07 (minus D-08 no-site), E-01 (process), E-02, AUDIT-14 op-rollback + difficulty-lock.
Still open (carried, none P15 regressions): C-08 (perf-only); E-03 second half (hostile tick-13 mode); E-05 (candidate moot); A12-06/07/08/09/10/12 + block-entity-unload leak (container/BE); D-04 (owner decision); A12-03 remainder (OpenScreen widths, text leniency); A12-15/17/18 (pack observability); B-07 (absence claim); F2 matrix-interior derivation frozen at 1452 (self-disclosed; refresh on next count move).

## Counts

Gate: **1477 passed, 0 failed, 34 ignored, 115 suites** (fresh full run at sign-off).
Walk 1452 → 1468 (+16 stability) → 1475 (+7 gap closures) → 1477 (+container-close
128 case, +LpVec3 canonical 3-case; the cow exact pin adds asserts to an existing
test). Additions only; no test removed or weakened.

## What this audit changed in the tree (`afb08b8`)

Serverbound close window id u8→VarInt (+128 unit pin + e2e `i32::from` adaptation); sweep corpus figure 6273→6258 (comment + CHANGELOG P15-07 line); explicit `CHUNK_BATCH_RECEIVED` packet-ids check; LpVec3 canonical-form 3-case test; cow speed exact pin; loot.rs ownership prose. Falsification left no trace.

## Not covered

Live real-client acceptance (no 25565 bind per constraints); Pi/SSH soak re-derivation (P15 has no soak clause); aarch64 on-device run; `bodies-destroy/lit/update` beyond existence; D-08 (no site); per-id variant captures for the 12 unobserved family members (P18); full jar re-derivation of the closed-18 s2c list (TSV spot-checks only).
