# AUDIT-10 — the P11-04..09 landing, audited

Basis: `e52ab82` (the landing) plus the two docs commits that follow it
(`bde0413`, `9ec110d`), audited from `9ec110d`. The landing's own account
(`CHANGELOG.md`, "Unreleased — Phase 11") names two defects found in this area
after the fact; per the handover prompt, every claim below was treated as
something to falsify. Verdicts: **confirmed** (by an independent instrument
unless labelled "re-run"), **refuted**, or **not checkable** (reason given).

Instrument legend: `read` = code read at `path:line`; `rerun` = the author's own
command re-run (weak); `probe` = perturbation that must fail a named test;
`test` = a committed test whose failure mode is known; `census` =
`target/loot_condition_census.py` re-run.

## The per-claim table

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| §1 commits/CI (e52ab82, bde0413, 9ec110d; runs 34955006168/73209/7237862 success) | `git log` + `gh run list` | confirmed | all three present; all three CI runs `completed success` (13m48s/13m06s/13m25s) — **seen**, not assumed |
| §1 gate 1344/0/30/102 on the landing tree | gate re-run **before** any change | confirmed (weak: same instrument) | `1344 passed / 0 failed / 30 ignored / 102 suites, every gate passed`; after AUDIT-10's additions the tree is 1346/0/32/104 |
| §0 the reversed-`ItemStack::new` defect existed and is fixed | `read` all call sites | confirmed | signature is `(item_id, count)` (`stack.rs:111`); every game.rs site is in that order — grant `game.rs:3436`, load `4599`, loot `4651` — with the why-comments at each |
| §2 census: 1 326 tables; match_tool 156, block_state_property 149, entity_properties 27, killed_by_player 22, table_bonus 17, random_chance_with_enchanted_bonus 11; (A) 167 drop-nothing from unknown-tool, disjoint from (B) | `census` re-run | confirmed | `tables scanned: 1326`; the six counts reproduced; `tables in both (A) and (B): 0`, (A) 167 |
| §2 `blocks/stone` is silk-gated `alternatives`; `entities/cow` carries `entity_properties` | `read` of the shipped JSON + census listing | confirmed | cow appears in the census's `entity_properties` list; stone's alternatives child carries `match_tool` (read from the extracted pack) |
| §1 "19 new tests" | count over the landing's changed test files | **refuted as stated** | the landing's test files contain 22 `#[test]` fns (loot_and_pickup 6, player_attack 5, ai_wiring 5→6 after AUDIT-10, entity_persistence 3, unreadable_chunk 0 at landing); some of the 5 ai_wiring files predate `e52ab82` (P11-01/02 round), so "19 new in this landing" cannot be reproduced exactly from the tree alone — the honest number is "22 `#[test]` in the five suites the landing owns, of which 19 postdate `8a6a459`" |
| §1 AI/melee/merge/persistence behaviour claims (§1 of the landing) | `read` (Lane A below) | confirmed | see Lane A |
| §2 "the differential test that was promised and never written" | `read` + `test` | confirmed, then closed | `loot_and_pickup.rs`'s module doc admitted the dangling promise; `vanilla_loot.rs` now exists (fix 1, `6a0325c`) and **probe A fails it by reverting fix 1** — the test is load-bearing |
| §4 "an entity in an unmodified chunk is not persisted" | `read` save path | confirmed | `queue_dirty_chunks` saves only `world.dirty_chunks()`; `serialize_chunk_entities` runs inside that loop, so a clean chunk's entities are never serialised (the divergence is recorded, not fixed) |
| §4 "a swing outside the interaction range is not refused"; "a held item's damage is not used" | `read` | confirmed | `resolve_mob_melee` re-checks range for the *mob* side only; the player's `Interact` arm applies `FIST_ATTACK_DAMAGE` unconditionally (named gaps, correctly recorded) |

## Lane A — the code, against the landing's behavioural claims

Independent instruments: code reads and jar fact cross-checks, **not** the tests.

- **A-01 · confirmed · the five named claims hold.**
  `ItemStack::new` order (three sites, above); the hurt window wiring
  (`Session::hurt_invuln_ticks` declared `game.rs:645`, checked `2246`, set
  `2251`, decremented `4087`); the merge rule (`merge_ground_stacks`: same item,
  ≤ 0.5 blocks, the *older* entity keeps, capped by the inventory's own
  `stack_sizes.max_stack_size`, remainder written back, empty younger removed);
  the diagonal light rule (`mc_world::chunks_a_block_can_light` enumerates the
  9-offset product, and **both** the world's invalidation and the server's
  `light_update` queue call that one helper — `game.rs:2707`); the
  unreadable-chunk guard (`read_failed` → `unreadable_chunks.insert` `4252`,
  checked in `may_generate` `4266`).
- **A-02 · Low, confirmed · the merge cap falls back to 64 with no players.**
  `max_stack` comes from `self.sessions.values().next()`; when no session
  exists the fallback is `DEFAULT_MAX_STACK_SIZE`, so a 16-stack item could
  merge past its limit in a playerless world before anyone could pick it up.
  Cosmetic (an empty world has no drops worth the name), but it is a
  divergence between "the inventory governs" and the fallback. Follow-up: key
  the fallback off the item registry rather than the sessions.
- **A-03 · not checkable (environment) · P11-10** — the landing's
  decomposition of what is unverified stands; nothing in the tree can confirm
  or refute what a real client renders.

## Lane B — the tests

Four existing probes **re-run clean**, each perturbation failing its named test
with the file restored byte-exact under SHA-256: `p11_falsify.py` (the save's
entity list; the join announcement), `falsify_ids.py` (add_entity id transposed;
a state module renamed), `falsify_world_fixes.py` (B-05 corner; B-06 unload
mark), `falsify_protocol_pin.py` (both sides of the 775 pin).

Three new probes (`target/audit10_probes.py`, first run here — the loot, merge
and hurt-window families had never been probed):

- **B-04 · confirmed load-bearing ·** reverting fix 1 (tool unknown again)
  fails `vanilla_loot::a_bare_handed_break_of_shipped_stone_yields_shipped_cobblestone`
  while the hand-written fixture suite still passes — which is exactly the gap
  the differential test exists to close, now demonstrated both ways.
- **B-05 · confirmed load-bearing ·** inflating `ITEM_MERGE_RADIUS_SQR` to 25
  fails the negative case `stacks_beyond_the_merge_radius_and_of_other_items_are_left_alone`.
- **B-06 · confirmed load-bearing ·** unsatisfying the mob's own hurt window
  fails `a_second_swing_inside_the_hurt_window_is_refused`.

**B-07 · Medium · confirmed — `Session::hurt_invuln_ticks` had no test that
could fail if the window were deleted.** Every existing scenario has a single
attacker whose own 20-tick cooldown already exceeds the 10-tick player window,
so the per-attacker windows shadow the session one; the CHANGELOG's "a mob
cannot take a player from full to dead in one window" claim was pinned by
tests of the *player→mob* window only (an earlier probe targeting the session
window passed, which is how the gap was found). **Fixed in this audit**:
`ai_wiring.rs::two_simultaneous_attackers_land_one_hit_per_window` puts two
zombies in range on the same tick and pins the collapse (17.0 after the first
window, and a floor of 2.0 against six uncollapsed hits), and the window's
deletion was falsified against it.

**B-08 · the probe method's own detector had a bug worth naming**: the first
version of the new probe read `"test result: ok" in output` as "the test
passed", but every *other* test binary in the workspace prints
`test result: ok. 0 passed` for a filter that matched nothing — the probe
reported "not load-bearing" for a test the manual run showed failing. The
fixed detector requires `test result: FAILED` in the output. Any future probe
script inherits this trap: a workspace-wide `cargo test` is not a boolean.

**B-09 · Low, confirmed · B-01's fix had no regression test.** The High
unreadable-chunk guard was verified by reading, not by a failing test. **Fixed
in this audit**: `unreadable_chunk.rs` plants a chunk whose `DataVersion` is
outside the readable window, asserts the owning game loads a placeholder (no
generated terrain in the column), then dirties the chunk, saves, and asserts a
third boot still cannot read the file — i.e. the save did not replace the
planted data (the data-loss path the guard closes).

## Lane C — the documents

- **C-01 · Low, confirmed and fixed · a stale cross-reference my own fix
  created.** `loot_and_pickup.rs`'s module doc still said the differential
  counterpart "does not exist yet" after `vanilla_loot.rs` landed; corrected
  in-tree to name it and to state what it now deliberately does *not* assert
  (the cow, until fix 2).
- **C-02 · confirmed, already corrected ·** the handoff's two dangling
  references in Rust comments (the promised `vanilla_loot.rs`, and the
  `docs/testing/…` mis-path of the parity matrix) are absent from the pushed
  tree; the *lesson* stands — `check_links.py` reads markdown links only, so
  Rust comments need their own grep, and this audit greps them.
- **C-03 · the totals drift by construction.** After AUDIT-10's two additions
  (the two-attacker test; the B-01 test; the two `#[ignore]`d differential
  tests) the tree measures **1346 passed / 0 failed / 32 ignored / 104
  suites**; every document that restates the total (TEST-MATRIX, README,
  CONTRIBUTING, RELEASE-CANDIDATE) must be re-reconciled before the next push,
  which the final commit does. The release record keeps its own historical
  figure on purpose.

## Lane D — the audit of AUDIT-09

Sampled the 19 "fixed" dispositions along the two axes the handoff named:

| Disposition | Support in tree |
|---|---|
| Code + test that fails on revert (strongest) | D-01 (speed pin), D-02 (nested multi-stack test), D-03 (rewritten clamp test), D-05 (draw-order), C-01 (creature-despawn property test over 2000 seeds), B-01 (the new `unreadable_chunk.rs`) |
| Code-backed, falsified this round | B-05 (light corner), B-06 (unload mark), A-02 (the ids property test), the loot fix 1 context (`audit10_probes.py` probe A) |
| Documentation-only, verified by reading the new text (weak, and labelled so) | A-01, C-04, C-06, D-06 |
| Second-hand (commit message + stat) | C-05, C-07 |
| Deliberately not applied | D-04 (the owner's decision stands; the 16.0/35.0 measurements are recorded) |

**D-01 · confirmed · the dispositions table is honest about this split**, and
the two weakest rows (C-05/C-07) now also carry the shared-helper code evidence
(`mc_world::chunks_a_block_can_light`), which upgrades them one column.

## Lane E — what nobody looked at

- **E-01 · Info · `serialize_chunk_entities` writes `Count` with a capital C**
  where modern vanilla writes `count` (lowercase) in the item's component map.
  The matrix already records that a vanilla server reading our file sees a
  default-valued entity rather than a corrupt one; the field-name gap is now
  named beside it rather than inside the general sentence. Not fixed here:
  which spelling the real 26.1.2 writer uses needs the jar's
  `ItemStack.save`-side read, and it changes nothing a client sees.
- **E-02 · Info · the AI's draw stream is now shared by three consumers**
  (spawning, goals, loot rolls). Determinism holds (one stream, fixed order),
  but a future differential replay must account for the merged stream — noted
  on `AiRng`/`LootRng`'s docs.
- **E-03 · Info · `ground()`-style accessors make loot tests read the store
  directly** while the packet assertions read the wire; both sides exist, so
  the two views can disagree silently. The landing's tests assert both halves;
  nothing new.

## What this audit fixed (in-tree)

1. Fix 1 of the loot decision (owner-approved): bare hand = known unenchanted
   tool at both call sites, with `vanilla_loot.rs` — the differential test the
   landing promised — passing against the extracted pack.
2. `two_simultaneous_attackers_land_one_hit_per_window` — the session hurt
   window's distinguishing test (B-07).
3. `unreadable_chunk.rs` — B-01's regression test (B-08).
4. `loot_and_pickup.rs`'s stale "does not exist yet" paragraph (C-01).

Fix 2 (pool-level refusal, owner-approved) is a separate change to `mc-data`'s
refusal architecture and lands after this document.

## The owner's P11-10 acceptance round (2026-09-15 21:44 session) — four defects

The owner played on the fixed build (LpVec3, entities=17 live). Positive: pigs, cows,
creepers, spiders and zombies **spawned and rendered on a real client**; the client also
**rendered nightfall** (26.1's clock works farther than the P10-03 record claims).
Four defects, each with the instrument run so far:

1. **M-1 · confirmed · respawn packet is undecodable by the real client.** Bytecode-read
   (`CommonPlayerSpawnInfo` read ctor + `ClientboundRespawnPacket.write`, client jar):
   26.1.2's Respawn = dimension-type **registry-friendly holder**, dimension ResourceKey
   (VarInt), hashed seed (long), game mode (byte), previous game mode (byte), debug
   (bool), flat (bool), **Optional<GlobalPos> death location**, **portal cooldown
   VarInt**, **one more VarInt**, plus a **trailing byte** after the spawn info. Our
   Respawn ends at `is_flat` — the client reads past our body and refuses. Fix: extend
   the encoder (death location `None` = absent, cooldown 0, trailing byte).
2. **M-2 · confirmed · survival mining does not break blocks.** The rig trace holds the
   owner's session: **95 `player_action` packets (c2s id 0) with 2-3-byte bodies**
   (`00 00` / `00 00 00`) against only 12 block updates. Our decode expects
   status+position+face+sequence (11 bytes minimum), so every dig is dropped. The
   client's own `ServerboundPlayerActionPacket.write` bytecode still shows
   enum+BlockPos+direction+sequence — which cannot produce a 2-byte body — so the id-0
   packets on the wire are either a different packet or 26.1.2 reshaped the action;
   **the decisive instrument is a client-jar read of the real play-protocol id map**
   (`GameProtocols` builder order does not match the wire: extraction says move packets
   ride 26-28, the trace says 30-31). The trace's frequencies (30/31 = the move flood,
   0 = the dig attempts) agree with OUR table for 13/30/31/63, so the first fix to try
   is decoding id 0's body as two VarInts and treating it as a dig at the client's aimed
   block — which the protocol no longer carries, meaning 26.1.2 mining needs the
   **`ServerboundAttackPacket` (new in 26.x, id 1, 4 seen)** and the swing/interact path
   re-examined against the client's own `MultiPlayerGameMode`.
3. **M-3 · confirmed · mobs move too fast.** `SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE`
   (43.17) is the player-walk derivation applied to mobs whose attribute is not the
   player's 0.1: the zombie (0.23) walks at 9.9 blocks/second — over twice the player.
   The constant was marked unverified in `mob.rs` and used anyway in P11-02; the owner
   saw the result. Fix: measure the real conversion from the captured move deltas
   (entity-capture bodies carry 1/4096-scaled per-tick movement for vanilla mobs), or
   pin the vanilla formula.
4. **M-4 · Low · pathing is direct steering** (recorded): mobs walk into water and
   walls. A one-cell passability lookahead (stop or re-target when the next cell is not
   passable) is the cheap first mitigation; real pathfinding stays a named gap.

The environment was left running for a follow-up round; the four fixes above are the
next session's opening work, before any P12 start.
