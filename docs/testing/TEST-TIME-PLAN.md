# Test Runtime Plan — how the suite gets fast without losing pins

Status: **executed** (2026-10-03). This file is the *method*: what to do about a
slow suite, in what order, and what is off limits. What actually happened — every
measurement, every landing, the lessons and the remaining candidates — is
[TEST-TIME-RESULTS.md](TEST-TIME-RESULTS.md), which owns the runtime figures the
way [TEST-MATRIX.md](TEST-MATRIX.md) owns the pass/fail counts.

Result: suite-time sum **≈1 690 s → 379.5 s**, `cargo test --workspace`
1 307 s → 388 s, per-commit fast tier **135–165 s**.

## 0. Where the time went at the plan's writing (pre-work measurement)

From `cargo test --workspace --no-fail-fast` on the dev host, before any of this
work (161 suites; suite-time sum **≈ 1 690 s**):

| Rank | Suite shape | Example | Cost driver |
|---|---|---|---|
| 1 | one 49-test suite, 168 s | `mc-world` lib | a deep `BlockRegistry` clone per write |
| 2–8 | farm-style integration, 86–123 s each | `growth` (6), `spread` (5), `sleep` (13), `weather` (5), `till` (6) | hundreds of full debug ticks per test |
| 9+ | long tail, 30–55 s each | `lake_stats` smoke, fluid suites | same shape, smaller |

A farm test cost roughly: world build + chunk generation + light settle +
`N × tick`, with debug ticks at ≈ 25–60 ms (8 phases, streaming, light recompute
on every write, broadcast encoding). Five P20 suites alone accounted for
≈ 500 s. Wall clock is the longest pole, not the sum — but every lever below
shortens both.

## 1. Instrument first

1. **Slowest-first report in `tools/gates/run.py`**, with suite names, plus the
   **suite-time sum** — the acceptance metric is a sum, and a table of the top
   15 cannot show it.
2. **`cargo-nextest`** for local and CI runs: per-test timing, work-stealing
   scheduling across binaries, and retries for suites with written flake
   history.
3. **Name the pole** before touching anything: the report exists to say which
   suite is expensive and why.

**Status: done.** Two defects in that report were found by using it (labels
printed `?` because cargo writes its `Running …` headers to stderr; the sum was
not printed at all) — both fixed. The pole was `mc-world` lib, and its cause was
a production inefficiency, not a test one: see RESULTS §1.

## 2. Test at the right layer (the big lever: ~10–50× per mechanism pin)

Almost every P20 mechanism pin ran **hundreds of full ticks** to assert what
**one direct call** proves, and the handlers are already factored for it
(`game::growth`, `game::sleep`, `game::weather` are modules, not tick inline
code). The standing rule:

> **No full-tick test asserts what a direct handler call can assert.**
> Mechanism pins move to in-crate unit tests (`#[cfg(test)]` inside the module —
> the only place that can reach `pub(crate)` handlers; `tests/` cannot); each
> area keeps exactly **one** full-tick wiring test proving phase integration.

Two consequences learned by doing it (RESULTS §2):

- **Run the new pins before deleting the old tick tests.** Four of the `growth`
  draft pins were wrong in ways only a run showed; deleting first would have
  removed the evidence that they were wrong.
- **Keep the arm a direct call cannot reach.** For `till` that was the click
  proving `UseItemOn` still dispatches to `apply_hoe`; without it, deleting the
  dispatch line leaves every direct pin green.

The plan's per-suite estimates (what each suite should become) are recorded in
RESULTS §2 alongside the actual numbers.

## 3. Trim the harnesses (the cheap lever, once §2 has taken the mechanism pins)

1. **View distance 4 → 2** where the suite asserts local behaviour. The
   per-cell random-tick rate is *invariant* to the loaded-chunk count, so
   statistics are identical while the sweep covers 25 chunks instead of 81.
   **Not** where the ring is the thing under test: chunk streaming,
   `survival_e2e`, the light and worldgen/structure suites, `p15_zero_delta`,
   `spawn_on_land`, `fluid_differential`, and `natural_spawn` (its spawn ring is
   a fixed 8 chunks that must meet the 24-block minimum).
2. **Skip the light settle where brightness is not asserted**; where it is,
   compute light directly for the farm chunk instead of settling through ticks.
3. **Revisit tick counts against their floors** — but only where the margin is
   wide, and re-derive the floor from the rate rather than widening it
   (`weather`'s storm pin: ≈88 expected / floor 20 → ≈44 / floor 15).
4. **Share the builders, not the farms:** one harness struct and one world
   builder per file; parallel tests never share a mutable `Game`.

**Status: done** for every suite that was expensive (RESULTS §3): `sleep`
88.5 → 7.8 s, `weather` 59.6 → 12.6 s, and two ring sweeps over 27 suites.

## 4. Tier the gates

1. **Fast gate** (`--quick`): per commit. Target wall clock **< 8 min**.
2. **Full gate**: everything, including `#[ignore]`d acceptance; nightly and at
   phase-close, which records budget vs measured (P18-07 precedent).
3. **CI timeouts**: an explicit `timeout-minutes` on every job, so a hung suite
   fails loudly instead of burning a runner.
4. **Release-profile evaluation (explicitly deferred).** Running heavy suites
   under `cargo test --release` would buy ~5–10× on tick cost but surrenders
   debug overflow checks and splits what "green" means across profiles. Revisit
   only if §§2–4 miss the target; never silently.

**Status: done except §4.4.** The fast tier keeps *every* test and buys its
wall clock from nextest's scheduling rather than from dropping suites — 135–165 s
end to end against the 8 min budget. §4.4 stays closed by §5's own rule: the
sum met the target.

## 5. Rollout order and acceptance

| Step | Work | Verdict | Status |
|---|---|---|---|
| 1 | §1 instrumentation | slowest-first table + sum in the gate output | done |
| 2 | §2 migration, one suite at a time | suite drops ≥ 5×, all pins perturbation-proven | done (`spread`, `growth`, `till`) |
| 3 | §3 harness trims | statistics re-verified, not assumed | done (`sleep`, `weather`, 27 suites) |
| 4 | §4 gate tiers + CI timeouts | `--quick` < 8 min; nightly full green | done except §4.4 |
| 5 | Re-measure; if the sum is not under ~600 s, open the release-profile question with numbers | — | sum **379.5 s**, so §4.4 stays closed |

Overall acceptance: suite-time sum **≤ ~600 s** ✅ (**379.5 s**); zero pins lost
(every neutralisation still turns its named test red — `tools/gates/perturb.py`
carries 19 mechanisms across five groups); zero thresholds widened to fit faster
runs. Any step that cannot meet its verdict is reverted, not explained away.

## 6. Non-goals (recorded so they stay non-goals)

- Deleting or `#[ignore]`-ing slow tests to make the gate fast.
- Lowering statistical floors to allow fewer ticks (floors are evidence).
- A shared mutable `Game` across parallel tests.
- Silent profile splits between local and CI runs.

## 7. Standing rules for the next slow suite

1. **Measure it first** — `python tools/gates/run.py --quick` prints per-suite
   times and the sum; read them with the ±20–30 s noise in mind (RESULTS).
2. **Decide the layer** — mechanism → §2 (direct pins + one wiring test); local
   scene on a wide ring → §3.1; tick count above its floor → §3.3.
3. **Prove the pins still bite** — `python tools/gates/perturb.py <group>`; a
   neutralised mechanism that leaves its pin green means the pin is not carrying
   the mechanism.
4. **Record it** — the landing goes in [TEST-TIME-RESULTS.md](TEST-TIME-RESULTS.md)
   (summary table + the detail), and the count in
   [TEST-MATRIX.md](TEST-MATRIX.md) if it moved.
