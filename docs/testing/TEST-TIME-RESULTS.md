# Test Runtime — executed record

Owner of the **runtime figures** for the workspace test suite, the way
[TEST-MATRIX.md](TEST-MATRIX.md) owns the pass/fail counts. The method, the rules
and the non-goals live in [TEST-TIME-PLAN.md](TEST-TIME-PLAN.md); this file is
what actually happened, in order, with the measurement behind each claim.

Read the numbers with the caveats at the end: per-suite times on this host move
by up to ±60 % between runs, so the suite-time sum is ±20–30 s.

## Summary

| # | Landing | Before → after | Evidence |
|---|---|---|---|
| 1 | `mc-world` lib: `World` registry clone per write (`790441d`) | **169.89 s → 0.55 s** (309×) | attribution: floor build 58.8 s + 4 356 `BlockRegistry` clones 59.8 s ≈ 100 % of the suite; floor 58.8 s → 12 ms |
| 2 | `spread` pins → direct handler calls (`bb36924`) | 121 s → ~15 s | 10 in-crate pins + 1 phase-wiring test; 5 perturbations red |
| 3 | `growth` pins → direct handler calls (`3215159`) | **90.2 s wall / 268.5 s sum → 19.3 s** | 8 pins (1.6 s together) + 1 wiring test carrying the rate pin; 7 perturbations red |
| 4 | `till` pins → direct `apply_hoe` calls (`f77971c`) | **47.7 s → 3.9 s** | 3 pins + a click pin (dispatch) + a threshold pin; 7 perturbations red |
| 5 | `sleep` harness (§3) (`b8c51e0`) | **88.5 s → 7.8 s** | ring 81 → 25 chunks; the damage wake waits 80 ticks instead of 300; 3 perturbations red |
| 6 | `weather` harness (§3) (`0ff43cd`) | **59.6 s → 12.6 s** | ring; storm pin 300 → 150 ticks with the floor re-derived (≈44 expected, floor 15); 1 perturbation red |
| 7 | Ring sweep, first pass (`5650776`) | 10 suites **≈286 s → 46 s** | per-suite table below; verified one suite at a time |
| 8 | Gate tiers + CI timeouts (§4) (`2bdb527`) | fast tier **135–165 s** end to end (two measurements) | nextest 155 s against `cargo test`'s 452 s; retries + per-job `timeout-minutes` |
| 9 | Ring sweep, second pass (`4c42edb`) | gate sum **431.3 s → 379.5 s** | 19 harnesses in 17 suites; whole workspace verified green |
| 10 | Remaining candidates recorded (`c299ee9`) | — | measured suite clocks for what is left |

**Totals now:** suite-time sum **379.5 s** (1 690 s at the plan's writing, −78 %),
`cargo test --workspace` **388 s** (1 307 s), fast tier **135–165 s**, counts
**2 054 passed / 0 failed / 44 ignored / 165 suites** (the +48/+4 over the
runtime work's close are the P20 remainder: `gamerules` 11, `saplings` 4,
`lightning` 4, `animals` 5, plus 24 lib/unit pins and the `gamerule` root —
see TEST-MATRIX.md, which owns the counts).

## §1 Instrumentation

- `tools/gates/run.py` gained per-gate wall clock and a slowest-first table.
- **Two defects in that report were found by using it**, both fixed:
  - every row printed `?` — cargo writes the `Running … (deps/<name>-<hash>.exe)`
    header to *stderr* and the results to *stdout*, and the report parsed stdout
    only. The streams are now merged at the pipe (verified: `mc_core`,
    `Doc-tests mc_core`).
  - the acceptance metric was unreadable: the table printed the top 15 and not
    the **sum**, so the gate could not show the number it exists to move. It now
    prints `suite-time sum`.
- **nextest adopted** (v0.9.146): per-test timing, work-stealing scheduling
  across binaries, and the retry policy below.
- **The pole was named and fixed, not just named:** the 168 s suite was
  `mc-world` lib. Attribution by measurement: registry load 138 ms, `World::new`
  0 ms, the 66×66 floor build 58.8 s — and 4 356 bare `BlockRegistry` clones at
  59.8 s, i.e. the per-write deep clone in `World::set_block` was ~100 % of it
  (~13.7 ms/write in debug). The field became `Arc<BlockRegistry>`: suite
  **169.89 s → 0.55 s**, floor build 58.8 s → 12 ms, and every production edit
  path got the same win. Zero behavior change.

## §2 Migrations (mechanism pins → direct handler calls)

Rule applied throughout: *no full-tick test asserts what a direct handler call
can assert*; each area keeps exactly **one** full-tick wiring test for phase
integration; every migrated pin is perturbation-proven.

### `spread` (slice 2b)

Five tick tests became ten direct-call pins in `game::growth::tests` plus one
phase-wiring test (cane through the sweep, view distance 2). 121 s → ~15 s. All
five perturbations re-proven red and restored byte-exact.

### `growth` (slice 1)

Four tick farms (300 ticks each) became eight direct-call pins; `tests/growth.rs`
keeps one wiring test that carries the sweep-rate pin *and* the deleted dry
farm's phase claim (60 dry cells lose moisture through the same sweep), beside
the fixture age/moisture bands. Suite 90.2 s wall / 268.5 s test-time → 19.3 s;
the eighteen crop/farmland/stalk/spread pins run in 1.6 s.

**Four draft-pin defects, all found by running the pins *before* deleting the
tick tests** — which is the argument for that order:

1. the plant helper built a lone soil column, so `getGrowthSpeed` scored 4.0
   where the rate arithmetic assumed the isolated-patch 10.0 (three pins red);
2. the draw loops cached a pre-growth block id, which `random_tick_block`
   trusts (the sweep always hands it a fresh read), so the crop re-wrote the
   same age step and the "grows" pin sat at age 1;
3. the max-age pin could not see a deleted guard — `write_property` refuses the
   out-of-band `wheat[age=8]` state either way — so it now also asserts the
   guard returns **before the roll** (the seeded source is untouched);
4. the beetroot pin compared final ages, which saturate on both sides, and
   stayed green with the pre-gate deleted; it now compares hit counts over two
   equal fields.

Kept: `crops_do_not_grow_in_the_dark`'s moisture half ("dark soil is still
watered") as `dark_farmland_still_wets_on_direct_call`, with the box's darkness
(brightness 0) itself asserted. Seven perturbations red, restored byte-exact.

### `till` (slice 2a)

The three hoe tests — the `TILLABLES` table, rooted dirt's drop, the three
refusals — became three direct `apply_hoe` pins in `game::session::tests`. The
file keeps what a direct call cannot show: the 5-block landing, the shaft-guided
mob fall, the idle control (200 → 10 ticks), and **one click proving `UseItemOn`
still dispatches to `apply_hoe`** — without it, deleting the dispatch line would
leave every direct pin green. All at view distance 2. Suite 47.7 s → 3.9 s (the
ring alone took the mob fall from 23.9 s to 2.8 s).

New coverage, not migrated: `fallOn`'s threshold is now a unit pin
(`short_falls_do_not_trample`). The idle control could never reach it — an idle
player sends no movement intent, so it never produces a landing at all — which is
why its 200 ticks were 47 s of runtime for no additional red-capability.

Seven perturbations red, including the dispatch line itself.

### The §2 P20 set closed on numbers

`spread`, `growth` and `till` were migrated (the three still expensive);
`sleep` and `weather` were resolved by §3 at 7.8 s and 12.6 s, where §2's ~10×
would buy single-digit seconds. §2 applies again when a suite is both expensive
*and* mechanism-heavy.

## §3 Harness trims

### `sleep` — solved by the harness, not by §2

All 13 tick tests built at view distance 4 to read a bed and a clock in chunk
(0, 0), so the 81-chunk ring was pure cost. At view distance 2: **88.5 s →
7.8 s** (test-time sum ≈250 s → 49 s), pins unchanged. The pole was
`damage_wakes_the_sleeper` — 88.5 s waiting for a zombie spawned two blocks away
to land a hit — now 3.8 s with the zombie adjacent and 80 ticks instead of 300
("a hit wakes the sleeper" is this pin; that a zombie can walk is
`mob_pathing`'s). Three mechanisms re-proven red: `after_damage`'s wake call,
the 100 % skip threshold, the 100-tick deep counter.

### `weather`

Both harnesses at view distance 4, storm pin at 300 ticks × 400 cells. At view
distance 2 the cycle/packet/command pins are 1.4–1.8 s and the restart
round-trip 4.9 s; the wetting pin runs 150 ticks with the floor re-derived from
the rate (≈44 expected, floor 15). **59.6 s → 12.6 s**, pins unchanged. One
mechanism re-proven red: the rain gate feeding farmland wetting.

### Ring sweep, first pass — the ten largest suites

| Suite | Before | After | | Suite | Before | After |
|---|---|---|---|---|---|---|
| `reach_validation` | 64.8 s | 18.6 s | | `loot_and_pickup` | 15.4 s | 2.3 s |
| `xp_orbs` | 54.7 s | 2.7 s | | `mechanisms` | 14.8 s | 1.7 s |
| `ai_wiring` | 34.3 s | 2.5 s | | `ranged_explosive` | 12.7 s | 1.7 s |
| `p18_hunger` | 33.3 s | 10.6 s | | `status_effects` | 12.3 s | 1.8 s |
| `mob_pathing` | 32.1 s | 1.8 s | | `player_attack` | 12.4 s | 2.8 s |

**≈286 s → 46 s.** Checked before changing them, not after: none of the ten
asserts the ring (their "radius" references are gameplay radii — ignite, merge,
wander), and `mob_pathing`'s walk, which treats an unloaded cell as solid, still
passes, so the mob never leaves the ring.

### Ring sweep, second pass — the remaining local scenes

19 constructors in 17 suites (`barrel` ×3, `reconnect` ×2, `block_change_ack`,
`console_save_e2e`, `crafting_table`, `dig_progress`, `doors`, `double_chest`,
`entity_lifecycle`, `entity_persistence`, `fluid_core`, `hopper_furnace`,
`join_entity_id`, `p17_owner_pins`, `p18_wear_enchant`, `p19_write_pins`,
`pvp_authority`). Gate sum **431.3 s → 379.5 s**. No per-suite saving is claimed:
the before/after nextest logs are not comparable (231 s against 155 s wall, with
summed test time 45 % higher under load), so the evidence is the reasoning
(81 chunks of ring down to 25) plus the gate's sum.

**Left at 4 on purpose:** `natural_spawn` (its spawn ring is a fixed 8 chunks
that must meet the 24-block minimum, so the view distance decides how much of
the annulus is loaded — a real semantic, not cost), plus `chunk_streaming`,
`survival_e2e`, the light suites, the worldgen/structure suites,
`p15_zero_delta`, `fluid_differential` and `spawn_on_land`.

## §4 Gate tiers and CI

- `run.py --quick` is the per-commit tier: fmt, clippy, the four docs audits,
  and **nextest over the whole workspace** — every test kept, wall clock bought
  from scheduling rather than from dropping suites (**155 s** against
  `cargo test`'s 452 s; end to end **135–165 s** across two measurements, budget < 8 min). That is a
  deliberate deviation from §4.1's letter: with nextest the whole workspace
  already fits the budget, so there is nothing worth dropping.
- The default tier stays canonical: aarch64 + cargo-deny + `cargo test
  --workspace --no-fail-fast`, which owns the TEST-MATRIX totals and runs the
  doctests nextest skips (2 006 against nextest's 2 002). `run.py` parses both
  formats and labels the fast tier's counts as nextest's.
- `.config/nextest.toml`: two retries for `network_game_bridge` and
  `whitelist_revocation` — the two suites with written flake history — none for
  anything else; a retried pass is reported **FLAKY**.
- `ci.yml`: explicit `timeout-minutes` on every job (GitHub's default is 360, so
  a hang used to burn six runner-hours silently), the per-push x86_64 job on the
  nextest tier, and a `schedule`-gated `nightly-full` job running the canonical
  `cargo test` plus the `#[ignore]`d acceptance suites.
- **Not done, and named:** §4.4 (release profile) stays deferred; the CI jobs
  have not been observed on a runner from here — they run on push.

## §5 Acceptance

| Target | Status |
|---|---|
| suite-time sum ≤ ~600 s | **379.5 s** (1 690 s at the plan's writing) — met with ~37 % margin |
| zero pins lost | every migration carries perturbation proofs (`tools/gates/perturb.py`: `growth` 7, `till` 7, `sleep` 3, `weather` 1, `reach` 1) |
| zero thresholds widened to fit faster runs | one floor was **re-derived** from the rate when ticks were halved (`weather`: ≈88/20 → ≈44/15); none widened |
| fast gate < 8 min | **135–165 s** |
| nightly full green | the job exists; unverified until a push |

## Remaining candidates, measured

Suite wall clocks taken one suite at a time on an otherwise idle host
(2026-10-03). The distribution is flat; what is left is small.

| Item | Current | Expected value | Cost / risk | Recommendation |
|---|---|---|---|---|
| Per-suite **budget check** in `run.py` (§1) | table + sum only; nothing fails when a suite creeps 5 s → 50 s | no speed — the regression guard this work lacks (P18-07 precedent) | low | **do** — the only item whose value is not speed |
| View-distance-4 stragglers (§3) | `gameplay_config_e2e` 7.4 s (3 harnesses), `player_info_tab_list` 4.9 s | ~5–8 s | very low (same mechanical change) | optional |
| The view-distance-3 set (§3) | 26 suites, ≈72 s measured (`execute_e2e` 13.7 s largest) | ~15–25 s | medium: six are socket e2e suites that set `NetworkSettings { view_distance: 3 }` as well, so both numbers must move together | optional |
| `natural_spawn` (§3) | 39.6 s, the largest single suite | potentially ~20 s | medium: semantic (spawn annulus). Experiment first — *load* the outer chunks explicitly without paying a larger ring's tick cost, if the spawn cycle scans loaded chunks | experiment before claiming |
| §2 wiring tests' tick counts (§3) | `growth` 18.5 s, `reach_validation` 18.7 s, `weather` 12.4 s, `spread` 9.0 s | small | high: the tick counts *are* the mechanism (a stone dig needs its 150 ticks) | **don't** |
| `mc-container` lib attribution (§7) | 8–20 s (load-dependent) against the 31 s that queued it | low | medium | close as no longer warranted at this size |
| §4.4 release profile | not done | 5–10× tick cost, i.e. potentially 379.5 s → 100–150 s | **policy**: surrenders debug overflow checks and splits what "green" means across profiles | closed by the plan's own rule (the sum is under the ~600 s trigger); re-open only with a new reason |

## Lessons recorded (they cost real time)

- **A perturbation harness must not preserve mtimes.** Restoring the perturbed
  file with a copy that kept the original timestamp left cargo treating the
  *perturbed* test binary as current, and the next full run reported two harness
  artifacts as test failures (the neutralised dry arm's "dirting applies"; the
  rate pin against `RANDOM_TICK_SPEED = 2`). Reproduced deliberately (identical
  bytes restored → still red; mtime touched → green), then fixed: the committed
  `tools/gates/perturb.py` rewrites the bytes and hash-checks every restore.
- **Load noise is large.** Per-suite times move up to ±60 % between runs (the
  same `mc_container` lib read 20.5 s, 18.4 s and 8.4 s; two nextest logs of the
  same tree differ by 45 % in summed test time). Read the sum as ±20–30 s, and
  prefer a like-for-like comparison or an isolated re-run before calling a small
  delta a win.
- **Bulk edits must write bytes.** A sweep script using `Path.write_text`
  translated newlines on Windows and silently turned 17 LF files into CRLF;
  `check_line_endings` caught it and its own message prescribes the fix
  (`read_bytes().replace(b'\r\n', b'\n')` + `write_bytes`).
- **Documentation audits earn their keep.** In this work `check_gate_totals`
  caught a pre-existing drift on `main` (README/CONTRIBUTING stating 1 903 while
  TEST-MATRIX owned 1 994) *and* an in-flight mistake of this session's own (a
  nextest count restated as if it were canonical).
