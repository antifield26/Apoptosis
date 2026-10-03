# Test Runtime Plan — from ~30 min toward ~10 min without losing pins

Status: **plan** (2026-10-03). Owner: engineering. This file is the single
answer to "why is the suite slow and what do we do about it, in what order".

## 0. Where the time goes (measured, not guessed)

From `cargo test --workspace --no-fail-fast` on the dev host (161 suites; the
pass/fail totals belong to [TEST-MATRIX.md](TEST-MATRIX.md), and §7 records
what each step moved — the suite-time sum at the plan's writing was
**≈ 1 690 s**):

| Rank | Suite shape | Example | Cost driver |
|---|---|---|---|
| 1 | one 49-test suite, 168 s | (identify in §1) | mixed |
| 2–8 | farm-style integration, 86–123 s each | `growth` (6), `spread` (5), `sleep` (13), `weather` (5), `till` (6) | hundreds of full debug ticks per test |
| 9+ | long tail, 30–55 s each | `lake_stats` smoke, fluid suites | same shape, smaller |

A farm test costs roughly: world build + chunk generation + light settle +
`N × tick`, with debug ticks at **≈ 25–60 ms** (8 phases, streaming,
light recompute on every write, broadcast encoding). Five P20 suites alone
account for **≈ 500 s** of the sum. Wall clock is the longest pole, not the
sum — but every lever below shortens both.

What this plan does **not** propose: weakening thresholds, deleting
coverage, moving everything to release profile, or redefining green. Every
lever keeps all pins red-capable (DoD item 13) or says so explicitly.

## 1. Instrument first (half a day, unlocks everything else)

1. **Slowest-first report in `tools/gates/run.py`.** It already prints per-suite
   durations; aggregate the top 20 with suite names into the gate output and
   fail loudly if any suite exceeds its recorded budget (P18-07 precedent:
   budget vs measured, recorded per phase-close).
2. **Adopt `cargo-nextest`** for local and CI runs: per-test (not just
   per-suite) timing, work-stealing scheduling across binaries, and
   automatic retries for the known timing-sensitive suites
   (`network_game_bridge`, `whitelist_revocation` — both have written
   flake history). Nextest does not change what is asserted.
3. **Name the 168 s pole.** The current data strips suite labels; the first
   run under nextest identifies it, and it gets §2 or §3 treatment first.

Acceptance: `run.py` prints a slowest-first table; CI uses nextest; the pole
has an owner and a number.

## 2. Test at the right layer (the big lever: ~10–50× per mechanism pin)

Almost every P20 mechanism pin runs **hundreds of full ticks** to assert what
**one direct call** proves. The handlers are already factored for it
(`game::growth`, `game::sleep`, `game::weather` are modules, not tick
inline code). Rule going forward:

> **No full-tick test asserts what a direct handler call can assert.**
> Mechanism pins move to in-crate unit tests (`#[cfg(test)]` inside the
> module — the only place that can reach `pub(crate)` handlers; `tests/`
> cannot); each area keeps exactly **one** full-tick wiring test proving
> phase integration.

Concretely, per suite:

| Suite | Today | After |
|---|---|---|
| `growth` (6, ~126 s) | 300 ticks × 6 through players+streaming | 1 wiring test (sweep rate + one growth) + unit pins calling `tick_crop`/`tick_farmland` on hand-built cells |
| `spread` (5, ~121 s) | 400 ticks × 5 | 1 wiring test + unit pins calling `tick_cane`/`tick_cactus`/`tick_spread` directly (≈ seconds) |
| `till` (6, ~55 s) | intent + fall + mob-fall through ticks | hoe table/refusals as unit pins via `apply_hoe`; keep one fall + one mob-fall tick test (physics needs the loop) |
| `sleep` (13, ~107 s) | 150–300 ticks × several | drive `tick_sleep` directly for quorum/deep-counter pins; keep one 10-player skip + one wake-path tick test |
| `weather` (5, ~90 s) | scheduled flips over ticks | drive `tick_weather` directly for cycle/packet pins; keep command + restart round-trip |

Perturbation value is preserved: neutralising a handler turns the unit pin
red exactly as it turns the tick test red today (same mechanism, fewer
ticks around it). The migration itself is verified by keeping the old
full-tick test green side-by-side for one commit, then deleting it.

Estimated effect: the five P20 suites drop from ≈ 500 s to **≈ 60–80 s**
of suite time. Nothing about thresholds, seeds, or statistics changes.

## 3. Shrink the remaining full-tick tests (~2–4× each)

For the wiring tests that must keep the loop:

1. **`view_distance` 4 → 2 in farm harnesses.** The per-cell hit rate is
   invariant to the loaded-chunk count (samples and cells scale together —
   stated in `growth.rs`/`spread.rs` headers), so statistics are identical
   while sweep samples drop 5 832 → 1 800/tick and streaming, light and
   broadcast volume shrink with the ring. (~3× on tick cost.)
2. **Skip the light settle where brightness is unasserted.** Cane, cactus,
   hoe, trample and dark-farm tests never read light; `settle_after_streaming`
   (5 ticks + full-ring recompute) is pure overhead there. Keep it only
   where a brightness gate is pinned. (Saves seconds per test, not minutes
   — do it while touching the file anyway.)
3. **Revisit tick counts against calibrated floors.** Floors were set below
   deterministic actuals; where margin is wide (e.g. actual 25 vs floor 12),
   halving ticks is safe *if* the floor still holds — re-run, don't assume.
4. **Share farm builders, not farms.** `growth`/`spread`/`till` each rebuild
   near-identical scaffolding; factor one builder (code reuse, no shared
   mutable state — parallel tests must never share a `Game`).

## 4. Tier the gates (stop running everything everywhere)

Today `run.py --quick` still runs **all** tests; CI runs the full workspace
on two OSes with no tiers and no timeouts.

1. **Fast gate** (`--quick` redefined): lib + unit tests + integration
   suites under a per-suite budget (target: wall clock **< 8 min** on the
   dev host). Runs per commit and in CI per push.
2. **Full gate**: everything, including `#[ignore]`d acceptance
   (`lake_stats` 32×32, ore/carver stats, Pi hooks). Runs nightly and at
   phase-close; phase-close records budget vs measured (P18-07 rule).
3. **CI timeouts.** Add explicit `timeout-minutes` to every CI job so a
   hung suite fails loudly instead of burning the runner silently.
4. **Release-profile evaluation (explicitly deferred).** Running heavy
   suites under `cargo test --release` would buy ~5–10× on tick cost but
   surrenders debug overflow checks and splits what "green" means across
   profiles. Revisit only if §§2–4 miss the target; never silently.

## 5. Rollout order and acceptance

| Step | Work | Verdict |
|---|---|---|
| 1 | §1 instrumentation (report + nextest + name the pole) | slowest-first table in gate output |
| 2 | §2 migration, one suite at a time (`till` first: smallest) | suite time drops ≥ 5×, all pins still perturbation-proven |
| 3 | §3 harness trims on the remaining tick tests | view_distance change with statistics re-verified, not assumed |
| 4 | §4 gate tiers + CI timeouts | `--quick` wall clock < 8 min; nightly full green |
| 5 | Re-measure; if the sum is not under ~600 s, open the release-profile question with numbers |

Overall acceptance: suite-time sum **≤ ~600 s** (≈ 10 min single-threaded,
well under that wall-clock with parallelism), zero pins lost (every
neutralisation still turns its named test red), zero thresholds widened to
fit faster runs. Any step that cannot meet its verdict is reverted, not
explained away.

## 6. Non-goals (recorded so they stay non-goals)

- Deleting or `#[ignore]`-ing slow tests to make the gate fast.
- Lowering statistical floors to allow fewer ticks (floors are evidence).
- A shared mutable `Game` across parallel tests.
- Silent profile splits between local and CI runs.

## 7. Executed: Step 1 results (2026-10-03)

- **Gate instrumentation landed** (`tools/gates/run.py`): per-gate wall
  clock, slowest-first table with suite labels (crate from the exe stem
  for lib suites, file stem for integration suites), same refusal
  semantics as before.
- **Nextest installed** (v0.9.146): trial on `mc-core` green with per-test
  timing; CI integration and retry policy still open.
- **The 168 s pole is fixed, not just named: `mc-world` lib, 49 tests.**
  Attribution by measurement, not guessing: registry load 138 ms,
  `World::new` 0 ms, the 66×66 floor build **58.8 s** — and 4 356 bare
  `BlockRegistry` clones at **59.8 s**. The per-write deep clone in
  `World::set_block` was 100% of it (~13.7 ms/write in debug). Fix: the
  `World` registry field is now `Arc<BlockRegistry>` (constructors still
  take the table by value; `set_block` bumps the refcount). Measured
  after: floor **12 ms**, suite **169.89 s → 0.55 s** (309×). Zero
  behavior change (same writes, same order), `clippy -D warnings` and
  `fmt --check` clean, `mc-server` lib still green. This also speeds
  every production edit path and every farm test that places blocks.
- **Next suspect queued, not started:** `mc-container` lib (136 tests,
  31 s) — same attribution method before any change.
- **§2 step 2, second suite: `growth` migrated (2026-10-03).** The four
  slice-1 tick farms in `tests/growth.rs` (300 ticks each) became eight
  direct-call pins in `game::growth::tests`; the file keeps **one**
  phase-wiring test — sweep rate plus wheat through the sweep at
  view_distance 2 — beside the fixture age/moisture bands; that test also
  carries the deleted dry farm's phase claim (60 dry cells lose moisture
  through the same sweep). Suite **90.2 s wall / 268.5 s test-time →
  19.3 s**; the eighteen crop/farmland/stalk/spread pins together run in
  **1.6 s**. Seven perturbations re-proven red and restored byte-exact
  (light gate, max age, beetroot pre-gate, growth-speed moisture term,
  farmland wet arm, farmland dry arm, `random_tick_speed`).
  - **Four draft-pin defects, all found by running the pins *before*
    deleting the tick tests** — the argument for that order:
    1. the plant helper built a lone soil column, so `getGrowthSpeed`
       scored 4.0 where the rate arithmetic assumed the isolated-patch
       10.0 (three pins red);
    2. the draw loops cached a pre-growth block id, which
       `random_tick_block` trusts (the sweep always hands it a fresh
       read), so the crop re-wrote the same age step and the "grows" pin
       sat at age 1;
    3. the max-age pin could not see a deleted guard — `write_property`
       refuses the out-of-band `wheat[age=8]` state either way — so it now
       also asserts the guard returns **before the roll** (the seeded
       source is untouched);
    4. the beetroot pin compared final ages, which saturate on both sides,
       and stayed green with the pre-gate deleted; it now compares hit
       counts over two equal fields.
  - **Kept:** `crops_do_not_grow_in_the_dark`'s moisture half ("dark soil
    is still watered") as `dark_farmland_still_wets_on_direct_call`, with
    the box's darkness (brightness 0) itself asserted.
- **§2 step 2, third suite: `till` migrated (2026-10-03).** The three hoe
  tests in `tests/till.rs` — the `TILLABLES` table, rooted dirt's drop, the
  three refusals — became three direct `apply_hoe` pins in
  `game::session::tests`. The file keeps what a direct call cannot show: the
  5-block landing, the shaft-guided mob fall, the idle control (200 ticks →
  10), and **one click proving `UseItemOn` still dispatches to `apply_hoe`**
  (without it, deleting the dispatch would leave every direct pin green).
  All at view distance 2. Suite **47.7 s wall → 3.9 s** (the ring alone took
  the mob fall from 23.9 s to 2.8 s). Seven perturbations re-proven red and
  restored byte-exact, including the dispatch line itself.
  - **New coverage, not migrated:** `fallOn`'s threshold is now a unit pin
    (`short_falls_do_not_trample`). The idle control could never reach it —
    an idle player sends no movement intent, so it never produces a landing
    at all — which is why its 200 ticks were 47 s of runtime for no
    additional red-capability.
- **§3 step, first suite: `sleep` (2026-10-03).** Solved by the *harness*, not
  by §2. All 13 tick tests built at view distance 4 to read a bed and a clock
  in chunk (0, 0), so the 81-chunk ring was pure cost: at view distance 2 the
  suite is **88.5 s wall → 7.8 s** (sum ≈ 250 s → 49 s), pins unchanged. The
  pole was `damage_wakes_the_sleeper` — 88.5 s waiting for a zombie two blocks
  away to land a hit — now 3.8 s with the zombie adjacent and 80 ticks instead
  of 300 ("a hit wakes the sleeper" is this pin; that a zombie can walk is
  `mob_pathing`'s). Three mechanisms re-proven red: `after_damage`'s wake call,
  the 100 % skip threshold, the 100-tick deep counter.
  - **§2's sleep rows were not taken**, as a decision with a number: at 7.8 s
    the suite is no longer a pole, so §2's ~10× would buy seconds while the
    remaining poles (`xp_orbs` 94 s, `reach_validation` 87 s, `weather` 84 s,
    `natural_spawn` 51 s) are tens of seconds each. §3 first; §2 where a suite
    is still expensive.
- **§3 step, second suite: `weather` (2026-10-03).** Same shape as `sleep`:
  both harnesses at view distance 4, and the storm-wetting pin at 300 ticks ×
  400 cells. View distance 2 takes the cycle/packet/command pins to 1.4–1.8 s
  and the restart round-trip to 4.9 s; the wetting pin runs 150 ticks with the
  floor re-derived from the rate (≈44 expected, floor 15). Suite **59.6 s wall
  → 12.6 s**, pins unchanged. One mechanism re-proven red: the rain gate
  feeding farmland wetting. §2's weather rows (direct `tick_weather` pins) were
  not taken, for the same measured reason as sleep: at 12.6 s the suite is no
  longer a pole.
  - **The §2 P20 set is closed on the numbers:** `spread` and `growth` and
    `till` were migrated (the three that were still expensive), and `sleep` and
    `weather` were resolved by §3 at 7.8 s and 12.6 s, where §2's ~10× would buy
    single-digit seconds. §2 applies again when a suite is both expensive and
    mechanism-heavy.
- **§3 ring sweep, ten suites (2026-10-03).** The same harness pattern — view
  distance 4 on tests that assert local behaviour — ran in ten more
  integration suites. At view distance 2 they go **≈286 s → 46 s** of suite
  time: `reach_validation` 64.8 → 18.6, `xp_orbs` 54.7 → 2.7, `ai_wiring`
  34.3 → 2.5, `p18_hunger` 33.3 → 10.6, `mob_pathing` 32.1 → 1.8,
  `loot_and_pickup` 15.4 → 2.3, `mechanisms` 14.8 → 1.7, `ranged_explosive`
  12.7 → 1.7, `status_effects` 12.3 → 1.8, `player_attack` 12.4 → 2.8. No pin
  changed, and each suite was run green on its own before the landing's gate.
  - **Checked, not assumed:** none of the ten asserts the ring — their
    "radius" references are gameplay radii (ignite, merge, wander) — and
    `mob_pathing`'s walk, which treats an unloaded cell as solid, still passes
    (so the mob never leaves the ring).
  - **Left at 4 on purpose:** `chunk_streaming`, `survival_e2e`, the light,
    worldgen and structure suites, and `natural_spawn` (its spawn ring meets a
    24-block minimum).
- **Still queued:** the remaining ~30 harnesses that still build at view
  distance 4 — mechanical, one suite at a time (verify green, then trim; the
  ring-owning suites above stay); `mc-container` lib attribution (§7); gate
  tiers and CI timeouts (§4) untouched.
- **Plan §5 acceptance:** the gate's suite-time sum reads **≈450 s** after the
  ring sweep, against the ≤ ~600 s target the plan set (≈1 690 s at the plan's
  writing).
- **Proofs are a tool now, not a command:** `python tools/gates/perturb.py
  growth` neutralises each mechanism the group names, requires that
  mechanism's pin to go red, and restores byte-exact (hash-checked). It
  exists because the hand-rolled version restored the perturbed file with a
  copy that preserved its original mtime, so cargo reused the *perturbed*
  test binary in the next full run and reported two harness artifacts as
  test failures (see CHANGELOG). The §2 migrations still to come add their
  cases to the same file.
