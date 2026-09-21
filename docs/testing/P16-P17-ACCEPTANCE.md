# P16–P17 Automated Acceptance (agent half)

Scope: the scripted evidence for P16-01..P16-06, P17-01 and P17-02 Step A,
re-run on the current tree. Real-client halves are out of scope here —
P16-07 night fight, the P17-02 container session, P17-04 differentials and
the P17-05 build session all need the owner at a keyboard.

## Method

One falsification probe per task, each applied, run, observed red, then
reverted; the tree was verified byte-clean afterwards with git status
(the only remaining diff is the xp_orbs.rs rewrite from §F-1). Full quick
gate afterwards: fmt plus clippy minus-D-warnings plus docs audit clean,
tests 1558/0/35 across 123 suites.

| Task | Probe edit (reverted) | Test | Result |
|---|---|---|---|
| P16-02 attribution | attacker gate opened to any death | xp_orbs blast-kill silence | red, orb (1, 3) on the ground |
| P16-03 poison floor | floor clamp disabled in player effect ticks | entity poison-floor pin | red, health 0.0 vs 1.0 |
| P16-04 follow range | zombie range 35.0 set to 16.0 | zombie far-notice pin | red, 16.0 vs 35.0 |
| P16-05 dig progress | per-tick accumulation removed | dig_progress dirt timing | red, dig never completes |
| P16-06 step-up | step height 0.6 set to 0.0 | world slab step-up pin | red, lip not climbed |
| P17-01 door toggle | block write skipped in the toggle arm | doors both-halves toggle | red, stays closed |
| P17-02A furnace pull | output-only roles replaced by all-Storage | hopper output-only pull | red, input stolen (3 and 9 collected) |

## Findings

### F-1: the fall-kill silence test was vacuous — rewritten as blast-kill

Re-running the P16-02 probe (open the attacker gate, expect the fall-kill
test to fail) stayed green. A temporary instrumented run showed why: the
cow teleported 40 blocks up landed after about 60 ticks at exactly 10.0
HP — full health. Per-tick fall segments floor at the 3-block threshold,
so terminal-velocity ticks deal zero each (the P05 gap the parity matrix
already records for multi-tick falls). No death can occur in that
geometry, so the test passed with or without the gate — the bow-wall
mistake class, caught by this acceptance instead of a player.

Fix, same file: `a_fall_kill_scatters_nothing` is now
`an_explosion_kill_scatters_nothing`. A creeper penned next to a penned
cow detonates through the real 30-tick fuse; the cow dies with attacker
None; the assertion is orbs (0, 0). The rewritten test was itself
probed: with the gate open it fails showing exactly (1, 3) — the cow's
reward — then the probe was reverted. The player-side wall in the rig
keeps the witness out of the blast, and a level-0 death scatters nothing,
so the ground reading is uncontaminated.

Not expanded: mob fall damage from movement stays as-is (per-tick
segments, long falls deal nothing). That is the recorded physics gap,
not new scope.

## Verdict

Automated evidence for P16-01..P16-06, P17-01 and P17-02 Step A holds on
this tree: every probe fails with the feature neutered and the gate is
green with it present. The one vacuous test found is fixed and itself
probed. Owner halves outstanding as listed above.
