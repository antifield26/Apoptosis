# P18-04 — Soak / §13 record (dev host stand-in + Pi NOT RUN)

Date: 2026-09-23 · tree after P18-01a/01b/02/03/06/07 lanes.

## Standing rule

Pi 5 is the production target (AGENTS §2). **This machine is not a Pi 5.**
Numbers below are a **dev-host stand-in** that proves the *workload* runs the
new per-tick paths. They are **not** the §13 production verdict. The Pi soak
is **NOT RUN** and remains required before an unconditional v0.3.0 claim.

## Workload (must-exercise list from TASK-INDEX P18-04)

| Path | Instrument | Result |
|---|---|---|
| Wear on dig | `p18_wear_enchant::wear_matrix_dig_n_times_adds_n_damage` | green |
| Eating | `p18_hunger::eating_bread_applies_nutrition_and_saturation_after_consume_seconds` | green |
| Combat (Sharpness/Protection) | `p18_wear_enchant::{sharpness_raises_the_swing, protection_reduces_damage}` | green |
| Observer clock | `mechanisms::an_observer_pulses_dust_behind_it_when_its_watched_block_changes` | green |
| Hopper→furnace | `hopper_furnace::a_fed_furnace_cooks_and_the_hopper_below_collects` | green |
| New terrain (ores/carvers) | `ore_carver_stats` 32×32 (release, `--ignored`) | green; coal 638 555, carved-air 0.00324 |
| Commands / fill | `p18_commands` 16 | green |

## Dev-host measures (stand-in)

| Measure | Value |
|---|---|
| Hardware | Windows dev host (not Pi 5) — **boundary** |
| `run.py --quick` before P18 lanes | 585 s, 1597/0/35, 128 suites |
| After P18 lanes | workspace green on named P18 suites (1+16+8+6 + survival 31 + BE 8); full `run.py` re-timed at close |
| Ore/carver 32×32 | ~50 s release; 33.8 ms/chunk generation hook (dev CPU) |
| Worst-phase attribution | P15-01 `tick metrics` worst-phase field present (AUDIT-16 P0 closed) |

## Pi soak (P18-04 acceptance) — **NOT RUN**

Required on a Pi 5 with 10 mixed real+scripted players for the §13 record:
hardware model/RAM, CPU/OS/kernel, toolchain, git SHA, profile, duration,
TPS, MSPT p50/p95/p99, CPU, RSS, storage. Until that runs, v0.3.0 may be
tagged only with this **named NOT RUN** listed (close clause), not as a
silent pass.

## Named NOT RUN list for the P18-05 verdict

1. **P18-04 Pi soak** (this file).
2. **c2s capture corpus 56 / 19** (`P18-07-GATE-HEALTH.md`).
3. **Real-client walk screens** (owner): pick wear/break, enchant glint+tooltip,
   hunger from sprint and bread, ore vein in a cave.
4. Selector sort/limit **vanilla differential** (`p18_commands` ignored test).
