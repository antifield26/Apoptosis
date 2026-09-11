# Audit 03 — Phases 00–05 re-verification (independent)

Date: 2026-09-11. Method: two **independent adversarial subagents** with read-only
scope, auditing Phase 05's deliverables and all cross-phase claims while the primary
agent worked on Phase 06. Every finding was re-verified by the primary agent against
code before being acted on; the verification result is recorded per item.

This is the third audit pass. `AUDIT-01` covered Phases 00–02, `AUDIT-02` Phases
00–04. The value of a third pass is visible in what it found that two self-reviews and
two prior audits had missed.

## 1. What the audits confirmed

- Test count 504 / 0 / 4 ignored reproduced exactly (at the revision before Phase 06's
  crates landed), and the ignored accounting is right.
- All 17 Phase-05 limitations are real; none is stale.
- Phase-05's documented no-ops really are no-ops.
- Packet ids, palette packing, registry ids, NBT, dimension layout, `level.dat`,
  autosave cadence: each `full` parity row checked out against code and tests.
- The Phase-05 restructure did **not** regress `SetTime`, keepalive, outbound
  draining, chunk sort order, autosave wiring, the shutdown order, or the join
  packet sequence. One audit verified each by reading the code path end to end.
- No `unwrap`/`expect`/`panic!`/`unreachable!` on any non-test path in
  `crates/{simulation,entity}/src` or `game.rs`; no client-controlled number reaches
  an allocation size, index or loop bound unchecked; no I/O in the tick path beyond
  the deliberate budgeted chunk reads.

## 2. False claims (both audits agreed; all corrected)

An audit that finds only code defects is missing half its job. These were **claims
that did not survive contact with the code**:

| Claim | Reality | Correction |
|---|---|---|
| Parity: effects' "numeric behaviour **is applied**" | `movement_speed_multiplier`, `damage_taken_multiplier`, `damage_over_time`, `can_kill` had **zero callers outside their own tests**. Only expiry was applied | Row rewritten; `damage_taken_multiplier` is now genuinely wired (§3.3) |
| Parity: "a 10-tick invulnerability window is ticked **and Resistance reduces incoming damage**" | `invulnerable_ticks` was only decremented; `Game::damage_entity` subtracted raw damage and never read it | Row rewritten; both are now wired (§3.3) |
| Parity: chunk streaming "nearest-first … **asserted exactly**" | The order exists; no test asserts it. Only the **count** is asserted | Rewritten to say which half is tested |
| Parity: "seeded randomness" deterministic | `RandomSource` is verified against the JDK in isolation, but **nothing draws from it** — `Game::random` is exposed and unused | Rewritten: the phase order is tested, the randomness half is unused |
| Phase-05 P05-06 / P05-07 marked **DONE** | Both were **PARTIAL**: the invulnerability field and the modifiers were dead, and nothing ever *inserts* an `ActiveEffect` | Both downgraded with the real gap named |
| Phase-05 §5 quoted the **prompt's** sentence as the exit gate | The real gate (`gates/EXIT-GATES.md`) also requires "scheduled updates" and "basic mobs" | Gate quoted correctly; the verdict changed from "partial (honest)" to **NOT satisfied**, with the two failing clauses named |
| Phase-05 §3 per-crate counts summed to **495**, not the 504 headline | Omitted `mc-network/keepalive`, `mc-nbt`'s doctest, and `mc-server`'s 7 `survival_e2e` tests | Recomputed and reconciled exactly |
| Audit-02: "collision axis order recorded in the parity matrix" | It was not | Row added |
| `MAX_SATURATION` presented as a game fact | It is this project's own simplification, and was **the one constant in `crates/entity` with no label** | Labelled as unverified, with the real Vanilla rule named |
| `DEPENDENCY-POLICY` "90 packages … all permissive" | 91; and it counted our own unlicensed crates as third-party "packages" | Exact count removed (it moves every phase); the licence property the gate checks is stated instead |
| `third-party.md` "`Cargo.lock` is committed" | There were no commits at the time | Corrected; it is genuinely committed now (`b7b1c99`) |
| `provenance.md` cited `crates/entity/src/random` | That path never existed (the module moved to `mc-simulation`) | Corrected |
| Phase-04 P04-17 "place two diamond blocks **through gameplay**" | The test used `world_mut().set_block`, i.e. the storage path | Corrected, with a pointer to the Phase-05 test that does exercise a real restart |

Two of these had been "corrected" by Audit 02 and were still wrong, which is itself
the lesson: a documentation correction needs the same verification as a code change.

## 3. Real defects found and fixed

### 3.1 Chunks recorded as sent before a send that could fail

`send_chunk` inserted the position into `sent_chunks` **before** `try_send`, and the
failure path only logged. `stream_for` skips anything already in `sent_chunks`, so a
dropped chunk packet left a **permanent hole** the client never recovered from.

Fixed: `send_raw` now returns whether the packet was queued, and `sent_chunks` is
updated only on success. A dropped chunk is retried next tick.

### 3.2 The overflow-disconnect contract could never fire

The module docs and `bridge.rs` both promised "a full queue disconnects that player
rather than growing memory without bound". `send_raw` takes `&self` (it is called
while a session is borrowed), so it could not push to `self.overflowed`, and
`Game::request_disconnect` had **no caller anywhere in the workspace**. Packets were
silently dropped and `TickReport::disconnects` counted disconnects that never
happened.

Fixed: the overflowed ids ride on the `TickReport` every send path already carries,
and the Network phase drains them. The contract is now true rather than documented.

### 3.3 Damage ignored the entity model

`Game::damage_entity` subtracted raw damage: no invulnerability window, no Resistance.
It is the *only* damage path, so the parity claims were false and a mob standing in
fire would have taken a hit every tick (20 hits/second) the moment mobs spawn.

Fixed: it now refuses a hit inside `INVULNERABLE_TICKS` (10, Vanilla's window) and
scales by `damage_taken_multiplier`.

### 3.4 A placeholder could still overwrite stored terrain

`read_stored_chunk` returned `Ok(None)` whenever `self.storage` was `None` — which is
every game built from a *borrowed* `WorldService` (`Game::new`, `Game::with_seed`, six
tests). So an edit dirtied the placeholder and `save_all` could write it over the real
chunk. Latent in production (the lifecycle uses the owned constructor) but it
contradicted the absolute claim the previous fix made.

Fixed: the game now distinguishes "no chunk stored" from "I cannot look". A
placeholder created without storage is recorded in `placeholder_without_storage` and
must never be written back.

### 3.5 A failed save was forgotten

`save_all` called `world.clear_dirty()` unconditionally, and `close_storage` discarded
the flush report. A chunk whose write failed would never be retried — the loss would
surface only as missing terrain after a restart. Fixed: the dirty flags are kept when
the flush reports failures, with the failure logged.

### 3.6 Regeneration ran at ~20× Vanilla

`tick_food(0.0)` was called **every tick** while `REGEN_HEALTH_PER_TICK` is 1.0, i.e.
~1 HP/tick against Vanilla's 4-second timer, and exhaustion never accrued so food never
depleted in play. Fixed: the food/regen step now runs on `FOOD_TICK_INTERVAL` (80
ticks). The exhaustion cost table is P05-14's and its absence is stated.

### 3.7 Two smaller ones

- `damage_entity` now refuses non-finite damage rather than propagating a NaN health.
- `MovePlayerRot`/`MovePlayerPosRot` stored yaw/pitch unchecked while x/y/z were
  validated; a non-finite rotation would poison every later look-vector computation.

## 4. Recorded, not fixed

- **`add_experience` can still overflow `i32` on a hostile `XpLevel`.** The cost is
  computed with saturating arithmetic but `level += 1` is not. Unreachable today (no
  production caller) but it is a real latent panic; recorded rather than fixed because
  the correct fix depends on the level cap Vanilla enforces, which is not verified.
- **`Game::random` has no consumer.** Kept because mob spawning (P05-11) is what will
  use it; the parity row now says so instead of claiming exercised determinism.
- **`EntityStore`'s cap test asserts nothing about the cap.** Filling 8 192 entities
  to test it is too slow for the suite; the guard is visible in code and the test was
  left as a weak one rather than deleted.
- **`Strength`/`Weakness`/`Regeneration` have no application function.** Their
  modifiers exist as multipliers but nothing consumes them.
- **The four remaining doc claims** listed in §2 were fixed in Phase 06, not left.

## 5. Incident: `docs/research/provenance.md` was destroyed and reconstructed

While repairing a cosmetic encoding problem in `provenance.md`, a maintenance script
opened the file with `open(path, 'w')`, which **truncates immediately**, and then failed
before writing anything. The file was left at 0 bytes. A follow-up script read the
now-empty file and wrote it back empty.

**Nothing could be restored from history, because the repository had no commits at the
time.** This is the concrete cost of the standing "zero commits" gap that every phase
report recorded as an owner action: a single bad write was unrecoverable. The owner
authorised an initial commit shortly afterwards (`b7b1c99`, 2026-09-11), so the
exposure is closed from that point — the damage this section describes is not.

What was done:

- the document was rebuilt from the rows that are **verifiable verbatim** in the
  tooling that produced them (the in-repo maintenance scripts) and from the documents
  that cite it;
- every row that could not be verified is marked **`[unrecovered]`** rather than
  silently dropped — these are the Phase 00 seed rows for the four reference clones,
  whose substance is preserved in `docs/research/reference-repos.md` (paths, licences,
  version metadata) and `docs/legal/third-party.md` §1;
- the reconstruction is announced **in the document itself**, with the cause, so a
  reader is not misled about its completeness;
- the truncating script pattern is now avoided: all later doc edits in this phase write
  through a helper that builds the whole string first and only then opens the file.

The affected rows are provenance *metadata*, not licence findings or code provenance:
no rule, algorithm or data-asset row was lost, because each of those was written by a
script whose exact text survives in `target/`. The gap is the per-clone inventory
detail, and it is now recorded as a gap rather than presented as complete.

## 6. Method note

The single most useful thing this pass did was **not** find code bugs — it was to
check that documented *evidence* existed. Three parity rows claimed a behaviour was
"applied" or "asserted" when the code only defined it, and the phase report cited a
prompt paraphrase as an acceptance gate. A self-written report is the weakest possible
evidence for its own correctness, and the re-verification of every claim against code
is what caught them.
