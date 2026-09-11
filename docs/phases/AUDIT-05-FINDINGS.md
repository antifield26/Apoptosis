# Audit 04 & 05 — Phase 06 deliverables, cross-phase claims, and CI

Date: 2026-09-11. Method: two **independent adversarial subagents** with read-only
scope, run in parallel. Audit 04 audited Phase 06's code and its report; Audit 05
audited all cross-phase claims *and* the CI configuration, which was newly reviewable
because the repository had just gained commits. Every finding was re-verified by the
primary agent before being acted on.

These are the fourth and fifth passes. `AUDIT-01` covered Phases 00–02, `AUDIT-02`
Phases 00–04, `AUDIT-03` Phases 00–05.

## 1. The headline finding: client-reachable item duplication

**Audit 04, severity high, reproduced and fixed in `dd03231`.**

The server kept the player's items in **two** places and synced them one-way at one
moment:

- `PlayerInventory` — authoritative, mutated directly by the Q-drop, the offhand swap
  and survival block placement;
- `Session::menu`'s player container — mirrored **only at join**, and written back
  **unconditionally** on every `container_click`.

Pressing Q removed the stack from the inventory *and* spawned an item entity; the next
click — even one that changed nothing — flushed the stale menu copy back over the
inventory. The stack then existed twice. Conservation held inside `Menu`, which is why
every `Menu`-level test passed; it failed for the server's authoritative state, which
is what a player actually owns.

Fixed by making the direction explicit: mirror the inventory **into** the menu before
applying a click, and re-mirror plus notify after any action that mutates the
inventory directly. Both fixes were **proven load-bearing** by disabling each in turn
and watching the corresponding assertion fail — and that exercise found something
worth recording: the mirror-in protects the duplication and the sync protects the
client's view. They are two properties, and the first version of the regression test
could not tell them apart because it failed on the view assertion before reaching the
duplication one. The test now asserts them separately.

## 2. Four more defects the fix exposed

| Defect | Found by | Fix |
|---|---|---|
| **The reach check applied to every `player_action`** — drop, swap and release carry no block position (Vanilla sends `BlockPos.ZERO`), so Q-drop was refused for any player not standing near the origin | the new duplication test | scoped to the three block-targeting statuses by an explicit list |
| **Cursor items were destroyed on disconnect** — `leave` dropped the session without returning the cursor, and `write_back_inventory` never covered it | Audit 04 A2 | returned to the inventory, with the shortfall reported if it does not fit |
| **`grant_item` could silently truncate** — a 64-count grant of a stack-1 item lost 63 to `Menu::set_slot`'s clamp | Audit 04 B4 | refused as a caller bug |
| **A hard-coded `state_id: 0`** on the placement path's slot update, handing the client a revision the server did not have | Audit 04 A6 | the bespoke send is gone; placement uses the same sync path as drop and swap |

## 3. Claim-versus-reality (all corrected)

| Claim | Reality | Correction |
|---|---|---|
| `AUDIT-02` "CI has now run for the first time on that commit" | **There is no git remote.** The workflow has never executed anywhere | claim removed; the CI file now states it is configured-but-never-executed |
| `PHASE-06` §3.1 `mc-server` row said **38** | its own listed suites sum to **50**; every per-suite count and the 745 total were right | row corrected |
| `PHASE-06` §2/§3.2 "2 000 pseudo-random clicks over every click type" | replaying the PRNG: only **46 of 2 000** clicks survive `Click::new`; no drag ever reaches the end stage, so the distribution arithmetic is never exercised | claim narrowed, and the test strengthened (§4) |
| `PHASE-06` P06-15 marked **DONE** | partial: the flood is 93% decoder refusals | downgraded |
| `PHASE-06` P06-17 marked **DONE** | partial: the close-with-cursor case was never tested, and disconnects lost the cursor | downgraded; the case is now tested |
| `PHASE-06` §0/§2 imply crafting, smelting and hopper transfer are delivered capabilities | all three are **library-only with zero server call sites** | stated in §5; the marks were accurate but understated it |
| `furnace.rs` labels 6 of 8 fuel values `Evidence::Verified` | the report said "community knowledge"; the **code** was the over-claim | labels aligned with the report |
| `game.rs` comment said retired block-entity items "are dropped into the world" | contradicted by its own log on the next line: they are not | comment corrected |
| `PHASE-00` post-hoc note still said "still has zero commits … CI therefore unexecuted … not committed, yet" | inside a block added *after* the commit | rewritten as history, with the CI part kept — it is still true, for a different reason |
| `third-party.md` "no commits yet", `TEST-MATRIX` "not yet executed because the repo has no commits" | stale, and in one case the *reason* was wrong | both corrected |
| Benchmark records "Commit | none" | AGENTS.md §13 requires a SHA in every record; `147fffb`'s message claimed it closed this gap but only annotated it | records now cite `b7b1c99`, the tree measured |

Three of these — the CI claim, the `PHASE-00` note and the benchmark rows — were
**introduced or left by the very commit that was meant to fix stale claims**. A
correction pass needs the same verification as the original claim; this is the second
time in this project that a documentation fix was itself wrong.

## 4. Parity-matrix rows corrected (Audit 05)

| Row | Was | Now |
|---|---|---|
| Save ordering: "header word last for chunks", citing "region.rs ordering tests" | **no such test exists**; only the `level.dat` half is covered | downgraded to partial with the covered half named |
| Autosave "default 6000 ticks" | the test re-asserts the same constant; no jar or measurement source | downgraded: the constant is a product decision, not a verified vanilla value |
| Entity store "refuses spawns past `MAX_ENTITIES`" | the test spawns 4 and asserts `len() < MAX_ENTITIES`; it never approaches the cap | downgraded; Audit 03 had already recorded this and the row survived |
| "17 corruption cases" | 16, measured | corrected |

## 5. CI findings

The workflow is now reviewable, and reviewing it found real problems. All are fixed in
`.github/workflows/ci.yml`.

**The `dependency-policy` job would have failed on its first run.** Two config errors,
found by installing `cargo-deny` 0.20.2 and running the exact command:

1. `wildcards = "deny"` treats a **path-only** dependency as a wildcard. Every
   internal crate is referenced that way, so the gate failed on 40+ dependencies that
   are vendored in this repository. Fixed with `allow-wildcard-paths = true`.
2. That alone was not enough: `allow-wildcard-paths` does not apply to *publishable*
   crates, because crates.io forbids a versionless path dependency in a published
   crate. The workspace is a **server binary, not a library**, so every member now
   declares `publish = false` — and the gate then correctly flagged that our own
   crates are **unlicensed**, which is ADR-0001 **R-09**, an owner decision. Fixed
   with `[licenses] private.ignore = true`, which makes the gate check the
   third-party tree, which is what the policy is about. **R-09 is not resolved by
   this** and is still recorded as an owner action.

The gate now passes: `bans ok, licenses ok, sources ok`, exit 0.

**The `check-aarch64` job named a runner that may not exist.** `ubuntu-24.04-arm` is a
real label, but GitHub's *hosted* arm64 runners are available for **public**
repositories only; a private repository needs paid larger runners. In a repository
whose visibility cannot be determined from here — there is no remote at all — naming
that runner means the gate is either fictional or silently absent.

Replaced with a **cross-`check` on `ubuntu-24.04`**, which always runs, and the
limitation is stated in the job rather than hidden: a cross-check does not link.
Running natively on arm64 belongs to the P08 Pi harness, where the hardware is real.
The old job's comment also claimed it ran "typecheck + clippy" while the step actually
ran the whole test suite; the comment and the step now agree. A new `check-linux` job
covers the *host* difference, which matters because development is on Windows and
production is Linux.

**The four gates are covered, and one gap closed.** `fmt`, `clippy`, `test` and the
aarch64 check all run; the local `cargo check --target aarch64-unknown-linux-gnu`
equivalent is now in CI, which it was not before.

## 6. Reproducibility gap quantified (Audit 05)

Fourteen documentation sites and eight committed source comments justify a fixture or
table by pointing at tools and data under `target/vanilla-26.1.2/` — which is
**git-ignored and uncommitted**. The claim "regenerable with the tools under
`target/`" is therefore **circular**: the tools are as absent as the data.

What mitigates it: the *outputs* of those tools that the project depends on are
committed (`crates/test-support/fixtures/**`, `docs/protocol/packet-ids-775.tsv`,
`docs/protocol/heightmap-types.tsv`), and `docs/research/protocol-baseline.md` §1
documents how to obtain the jar. What does not: the extraction scripts themselves.

Recorded as a P08 task rather than fixed here — committing twelve research scripts is a
packaging decision, not a Phase 07 one.

## 7. What the audits confirmed

Worth stating, because an audit that only lists problems reads as though nothing is
sound:

- **Test arithmetic is exactly right.** Audit 05 reproduced 745 passed / 0 failed / 4
  ignored and verified every per-suite row.
- **13 of Phase 06's 14 limitations are accurate**, traced to file:line.
- **`deny.toml`'s schema is valid** — `version = 2` is accepted, the licence tables
  are correctly placed, and `{ name, reason }` uses a still-accepted key. Only six
  non-fatal unused-allowance warnings, which are honest (we allow licences we do not
  currently use).
- **Linux portability is clean**: no hardcoded paths, ephemeral ports via `:0`, no
  case-sensitive fixture collisions, and the single `#[cfg(unix)]` block is correct.
- **The ignored tests are correctly skipped rather than failing**, though they would
  panic under `--ignored` without the environment variables — which is the right
  behaviour for a manual differential run.
- **No reachable panic on client input** in `mc-container` or `mc-redstone`: every
  `Invariant` is construction-time and reached only from server-built values.
- **No `HashMap` iteration on a tick-walked path** anywhere checked.
