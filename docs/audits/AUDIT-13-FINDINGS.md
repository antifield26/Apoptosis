# AUDIT-13 — P13 (redstone wiring + conductivity + differential)

Basis: `d592b8f` (P13-07 differential), audited read-only. P13 is seven
commits (`502d10d`..`d592b8f`): queue drain, world feed, lamp drive,
torch/comparator sides, wire length, conductivity matrix, vanilla
differential. No lane changed the tree for a code fix — there is none to
make: the four falsification probes all hit, the gate re-runs green, and
every measured cell the model asserts replays. Doc touch-ups (five stale
post-P13 lines) ride with this document. Verdicts: **confirmed**
(independent instrument unless labelled "re-run"), **refuted**,
**not checkable** (reason).

## Why these weights

P13 added seven commits of never-audited code (tick drain, feed gate,
`new_state` world threading, directional `solid_power`, lamp rule,
fixture + 13 differential tests), so the model-rules lane and the game
wiring lane carry the most weight. The historical opens concentrate
outside redstone (the P06 redstone rows were suite-green/inherited; the
one redstone-adjacent open, P05-10's documented-no-op boundary, is closed
by P13-01 — recorded below), so one lane re-derived the counts and one
swept docs drift instead. The fifth lane ran the falsification pass; the
sixth names blind spots rather than implying coverage.

## The per-claim table (P13 exit gate + totals)

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| Gate 1412/0/34/110 on `d592b8f` | gate re-run (weak: same instrument) | confirmed | `1412 passed / 0 failed / 34 ignored / 110 suites, every gate passed` |
| Queue drains due ticks under budget, defers never drops | read + `scheduled_ticks` tests (re-run) | confirmed | fired/pending on report; budget-exhausted only when work remains |
| Placed/removed/flipped blocks feed the model; dirt does not flood | read (`redstone_feed` gate) + feed/lever e2e (re-run) | confirmed | relevance gate (wire/emitter/mechanism or neighbour); break feeds with the removed id; flood test conserves |
| Lamp lights on pedestal, darkens on flip, broadcasts `block_update` | re-run `a_flipped_lever_powers_dust_and_lights_a_lamp` | confirmed | pedestal geometry (vanilla-faithful per K1); on/off + broadcast asserted |
| Torch follows attachment; comparator reads back/sides | re-run torch e2e + `a_comparator_reads_back_and_sides_by_facing` | confirmed | attach-face flip; compare/subtract modes |
| Wire carries 15 live blocks | re-run golden + fixture row D2 | confirmed | 15..1 hand table; vanilla cell (0,103,0)=15 replays |
| Conductivity matrix (KD-15) per the measured table | re-run 6 matrix tests + 13 differential tests | confirmed | torch-below/lever-mount strong; dust above/beside weak + solid-blind + headroom; block never; lamp above/below |
| Differential replays 92-cell vanilla fixture | re-run `vanilla_conductivity` (13) + fixture read-back | confirmed | 36/36 asserted cells match; H-side cell excluded with its 3:1 history (A13-01) |
| Totals 1399 → 1412 after P13-07 | full gate re-run (see §7) | confirmed | 1399 + 13 (`vanilla_conductivity`); suites 109 → 110; 1031 + 5 + 376 arithmetic holds |
| P05-10 documented-no-op boundary | read (`tick_scheduled` + drain) | confirmed closed | queue wired, drained every `ScheduledTicks` phase, fed by P13-02, reactions in P13-03 |

## Lane 1 — model rules vs the fixture (P13-05/06/07)

- **A13-01 · Low · H-side cell excluded from the replay — CONFIRMED honest, no change.** The fixture reads lit at (1,101,25) (lamp beside a lit torch); three isolated runs read dark (3:1). The model implements the majority. The test documents the split and asserts everything else (36/36 green). A single observation never replicated twice is correctly not modelled.
- **Lamp variance — confirmed, recorded, no change.** ~5 flips in ~45 lamp observations across runs, both directions (G, I, T1×2, H); dust and torches never flipped once in ~55. Consistent with single-shot lamp evaluation (no scheduled re-check) occasionally sticking; dust/torches self-correct through scheduled ticks. The model implements settled majorities, which is the only stable target. NOT-checked: re-running the campaign to shrink the variance (cost without decision value while every majority holds).
- **R10 facing math — confirmed by hand re-derivation.** `facing=west` → `opposite_face(4)=5` → offset (1,0,0): mount east of the lever, matching the vanilla R10 geometry (probe 15) and the wall-torch attach read. `facing_index` unknown-default (north) only affects malformed states.
- **Solid-blind receipt — confirmed load-bearing (probe F3).** Reverting to stored-powers reproduces the exact vanilla Q-a divergence (under-dust 13 vs 0). The rule is not an implementation convenience.
- **Headroom gate — confirmed load-bearing (probe F2).** Removing it lights the z=22 support the vanilla fixture leaves cold.
- **Lamp above/below — confirmed load-bearing (probe F1).** Any-side reading fails the R2/K1/I/L1/L2/G darks while keeping the pedestal golden green (which is why the golden alone cannot pin the rule).
- **Torch-below-only — confirmed load-bearing (probe F4).** Omnidirectional torch power fails R6/Q-b/z14 while keeping M1/T2/R4 green.
- NOT-checked: repeater/comparator facing contributions (directionless by declared gap); ceiling-lever mount (assumed symmetric, no vanilla row); button/plate/rod extrapolation from the block (declared); cover-above-stone variant (only cover-above-dust measured); R1-support predicted cold (follows from solid-blind, no dedicated vanilla row).

## Lane 2 — game wiring (P13-01/02/03)

- **Feed gate soundness — confirmed by read.** `redstone_feed` skips only all-passive neighbourhoods; `solid_power` reads emitters and dust but never solids, so an all-passive neighbourhood cannot change any computed power — the skip is provably complete, not a heuristic. Break feeds with the removed id (torch-on-stone removal reaches the stone); flips feed with the new id; placements feed. No stale-stone path found (the obvious candidate, breaking a lone torch on a lamp-bearing stone, is covered).
- **A13-02 · Low · unloaded neighbours read as absent in the gate.** `get_block_loaded` returns `None` past the border, so a placement beside an unloaded live wire feeds nothing. Covered by the declared chunk-boundary divergence (far side reads absent); no change. NOT-checked: load-time re-feed (no path feeds on chunk load today).
- **Placed-lever initial state — confirmed OFF by instrument.** The fixture lists `powered=true` first, but `a_flipped_lever_powers_dust_and_lights_a_lamp` flips once and reads 15 — a placed-ON lever would read 0. Behaviour verified; the fixture order is not the placement path. No change.
- **Broadcast path — confirmed by read + e2e.** Propagation writes go through `World::set_state` into the change list (`world_integration` pins the recording); the e2e asserts `BLOCK_UPDATE` on lamp changes. No redstone-specific packet path exists or is needed.
- NOT-checked: budget-exhaustion mid-circuit across ticks at game level (model-level covered); hostile-rate feed flooding beyond the queue caps (caps + dedup read, not load-tested here).

## Lane 3 — tests and counts

- **Gate re-run — confirmed.** `1412 passed / 0 failed / 34 ignored / 110 suites, every gate passed` (fmt, clippy `-D warnings`, docs-audit incl. `check_gate_totals`, full workspace).
- **Arithmetic — confirmed.** Lib sum 1 031 (unchanged by P13-07) + 5 doc-tests + 376 named-suite = 1 412. Spot suites: `propagation` 23, `vanilla_conductivity` 13, `golden_circuits` 6, `mc-redstone` lib 66 — all re-run green.
- **Perturbation discipline — confirmed present.** P13-07's matrix/differential tests fail on rule revert (lane 5); the P13-01/02 feed tests carry their perturbation notes in TEST-MATRIX. NOT-checked: re-running the ignored differential suites (jar-gated, unchanged by P13).

## Lane 4 — docs drift (P13 aftermath)

- **FOUND, FIXED with this document (5 lines):** `system-overview.md:45` (`ScheduledTicks` still a no-op), `:109-110` (redstone not wired into the tick loop), `RUNBOOK.md:177` (same), `PARITY-MATRIX.md:91` (torch attachment/comparator sides "unmodelled" after P13-04), `:106` (due ticks "have no reactions" after the wire-recheck scheduling). All verified stale against the tree before editing.
- **Left as dated record, not edited:** `AUDIT-12-FINDINGS.md:62` and `ADR-0007:35` repeat the no-op line — both are frozen snapshots, and editing history to match the present is exactly the drift pattern this lane exists to prevent.
- NOT-checked: a full-matrix prose re-read beyond the redstone rows (the P13 touch surface).

## Lane 5 — falsification probes (all reverted, tree clean)

| Probe | Revert | Expected failure | Observed |
|---|---|---|---|
| F1 lamp reads any side | `lamp_reads_only…` matrix | fails R2/K1/I/L1/L2/G darks | FAILED as expected |
| F2 headroom always open | `dust_powers_stone…` | fails z=22 (support lights) | FAILED as expected |
| F3 cite stored, not solid-blind | `dust_reads_powered…` | fails Q-a (13 vs 0, the vanilla divergence itself) | FAILED as expected |
| F4 torch powers every face | `torch_powers_only…` | fails R6 (stone powers from above) | FAILED as expected |

All four rules are load-bearing: each revert breaks exactly its measured cells while neighbouring tests stay green (F1 keeps the pedestal golden green, which is why goldens alone underdetermine the lamp rule). `git status` clean after revert.

## Lane 6 — historical opens disposition

- **P05-10 (scheduled ticks documented no-op) — CLOSED by P13-01.** Queue owned by `Game`, drained every `ScheduledTicks` phase with fired/pending accounting, fed since P13-02, wire rechecks scheduled. The boundary note in AUDIT-08 stays as history.
- **P06-09..16 redstone rows — no action.** Suite-green/inherited through every audit; P13 replaces their model content with measured rules, and the suites above re-verify them.
- **AUDIT-09/11/12 opens naming redstone (update order, shape-update split, comparator channel) — still open, untouched by P13** except conductivity, which is now closed. No new instance added: `new_state`'s world threading does not smuggle order-dependence (lamp reads computed solid power, deterministically).

## §7 Counts

1412 passed / 0 failed / 34 ignored / 110 suites on `d592b8f` plus this document's five doc lines (no code change; gate re-run after the doc edits to keep `check_gate_totals` honest — re-run figure: 1412 / 0 / 34 / 110, every gate passed).
