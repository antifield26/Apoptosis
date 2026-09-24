# P18-07 — Gate health + AUDIT-17 residuals

Date: 2026-09-23 · baseline `081d98f` + P18-07 work.

## Gate budget (dev host)

| Measure | Value |
|---|---|
| `python tools/gates/run.py --quick` | **every gate passed** |
| Wall time | **585 557 ms ≈ 9.76 min** (target ≤ 10 min) |
| Result | `tests: 1597 passed, 0 failed, 35 ignored, 128 suites` |
| fmt / clippy / docs-audit | all clean |

CI workspace duration on the same tree: record the run id at the next push
(close clause requires it for P18-05).

## TEST-MATRIX named closed set (counted 2026-09-23)

Instrument: walk `crates/**/tests/*.rs` + `apps/**/tests/*.rs`, count
`#[test]` / `#[tokio::test]` excluding `//` lines
(`target/p18_07_census.py`).

| Bucket | Count |
|---|---|
| Named integration files | **92** |
| Named integration tests | **519** |
| Per-crate lib (17) | **1 105** |
| Doc-tests | **5** |
| Arithmetic | 1 105 + 5 + 519 = **1 629** |
| Workspace measured | **1 597 passed + 35 ignored = 1 632** |
| Δ | **3** (labelled: dual `determinism` suites + any `#[test]` in strings; not silently absorbed) |

Full per-file table: run `target/p18_07_census.py`. This closes the "named
arithmetic derived rather than counted" hole for the current tree; refresh
the census whenever a suite file is added.

## B-02 naming — verified closed

`the_location_word_commits_to_a_fitting_payload` is the current name
(`crates/persistence/src/region.rs:888`). Sequence pin remains
`location_word_is_written_after_payload`. **Closed at `06aca2a`; no reopen.**

## Capture corpus c2s 43 / 56 / 19

Scanned `target/**/*trace*.jsonl` and bodies logs for c2s-shaped id fields:

| Packet | id | Hits | Status |
|---|---|---|---|
| `player_input` | 43 | 426 (visual-check-p11 trace) | **present** (older capture; semantic still matches bit5) |
| `set_creative_mode_slot` | 56 | 0 | **carried by name** — need a creative-take capture session |
| `container_close` | 19 | 0 as c2s id field | **carried by name** — need a window-close capture |

Reason: available captures predate P17 creative/close sessions. Not RUN is
not PASS; these two stay on the P18-07 residual list until a capture is
taken (owner or capture-rig).

## Residual IDs (code lane)

| ID | Status |
|---|---|
| A12-06 dual-viewer LWW | **closed** — `write_back_block_slots` + `two_viewers_interleaving_writes_to_different_slots_keep_both` |
| A12-07 kind-drift | **closed** — `changing_a_chest_to_a_furnace_replaces_the_payload_kind` |
| A12-03 width check | **closed** — `item_stack_decode_rejects_hostile_counts`, `open_screen_rejects_out_of_range_varints` |
| A12-10 dirty gap | **closed** — `a_hopper_cooldown_tick_marks_its_chunk_dirty` |
| P15-03 zero-delta | **closed** — `p15_zero_delta::p15_split_observables_replay_identically` |
| c2s 56 / 19 capture | **carried** (NOT RUN) |
