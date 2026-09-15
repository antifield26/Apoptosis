# AUDIT-09 Remediation — every finding dispositioned (2026-09-15)

Each of the 37 findings from [`AUDIT-09-FINDINGS.md`](AUDIT-09-FINDINGS.md) with
what was done about it and how the fix was verified. The findings document carries
the evidence; this one carries the disposition, including the ones deliberately
left open and the one deliberately not applied.

## Summary

| Disposition | Count | Which |
|---|---|---|
| **Fixed** | 19 | A-01, A-02; B-01, B-05, B-06; C-01..C-07; D-01, D-02, D-03, D-05, D-06; E-01, E-03 |
| **Open**, with the experiment that would close it named | 8 | A-03; B-02, B-04; C-08; D-04 (by decision), D-07; E-02, E-05 |
| **Not re-derived** (the lane's note did not resolve to a specific site) | 10 | A-04, A-05, A-06; B-03 (contradicted by the instrument it named), B-07; D-08; E-04, E-06, E-07, E-08 |
| **Total** | **37** | |

The per-lane splits behind those three numbers, so the totals can be checked against
the findings list rather than taken on trust:

| Lane | Findings | Fixed | Open | Not re-derived |
|---|---|---|---|---|
| A — protocol/wire | 6 | 2 (A-01, A-02) | 1 (A-03) | 3 (A-04, A-05, A-06) |
| B — world/persistence | 7 | 3 (B-01, B-05, B-06) | 2 (B-02, B-04) | 2 (B-03, B-07) |
| C — server logic | 8 | 7 (C-01..C-07) | 1 (C-08) | 0 |
| D — entity/data | 8 | 5 (D-01, D-02, D-03, D-05, D-06) | 2 (D-04, D-07) | 1 (D-08) |
| E — tests/docs | 8 | 2 (E-01, E-03) | 2 (E-02, E-05) | 4 (E-04, E-06, E-07, E-08) |
| **Total** | **37** | **19** | **8** | **10** |

By severity, of the four High findings: **B-01** (world loss) and **D-02** (loot
stack loss) are fixed, and **E-01** (stale documentation) and **E-03**
(self-referential `TestClient`) are fixed and open respectively. No High finding was
left undiagnosed.

Where a fix landed before this landing, the commit is named. "Fixed in the tree" was
the handoff's phrasing for the lane-C findings; it is replaced here with `8a6a459`,
because that is where they actually are and a reader can check it.

## The defect this landing found, which no lane reported

`ItemStack::new(item_id, count)` was called with the arguments **reversed** in two
places in the uncommitted P11-04..08 code:

- `Game::spawn_loot_table` — so every loot drop spawned `count` copies of the item
  whose registry id equalled the intended count. A broken stone block dropped 35 ×
  `minecraft:stone` instead of 1 × `minecraft:cobblestone`.
- `Game::load_chunk_entities` — so every dropped item restored from a save came back
  as the wrong item with the wrong count.

Found by `crates/server/tests/loot_and_pickup.rs::a_survival_break_drops_what_the_loot_table_says`
on its first run, a test written specifically to assert on the item a loot table
named rather than on the fact that something dropped. Fixed in
`Game::spawn_loot_table` and `Game::load_chunk_entities`; pinned by
`a_survival_break_drops_what_the_loot_table_says` and
`entity_persistence.rs::a_saved_chunk_returns_its_mobs_and_its_drops_to_the_next_game`.

Two things make this the most important line in this document:

1. **A green suite of 1 325 tests did not notice.** The defect was invisible to
   review, to clippy and to every existing test, because every existing test asked
   whether *an* entity appeared, not *which*.
2. **The test was written with the same reversed order first.** The first version of
   `loot_and_pickup.rs` called `ItemStack::new(3, stone)`, matching the code's
   mistake, and it failed only because the assertion named the fixture's item. That
   is E-03's thesis — a fixture built from the same reading as the implementation
   cannot catch a wrong reading — demonstrated on the same afternoon it was written
   down.

## Per-finding disposition

| ID | Sev | Disposition | Fix | Verification |
|---|---|---|---|---|
| A-01 | M | **Fixed** (doc + decision) | `DEFAULT_INBOUND_CAPACITY` and `lifecycle::EVENT_QUEUE` now state the real design: one shared 1 024-entry queue, no per-connection bound, with the fairness consequence named. The constant is kept as the figure a per-connection queue would use, and says it is not wired. | Read-back of both docs; `grep` shows the constant has no reader. Doc-only, so no test. Follow-up: per-connection inbound queues |
| A-02 | M | **Fixed** (as a property) | Twelve `check()` lines added **and** `every_constant_in_ids_rs_matches_the_vanilla_table`, which reads `src/ids.rs` and requires all 104 constants to match the jar's table in their own state and direction | `cargo test -p mc-protocol --test packet_ids` = 5 passed. Falsified twice (`target/falsify_ids.py`): transposing `ADD_ENTITY` 1→2 fails it; renaming `pub mod handshake` fails it; tree restored byte-exact (SHA-256) |
| A-03 | L | **Open** | Not applied. A hard trailing-bytes refusal would reject a body a real client sends with a field this build does not model, turning a silent gap into a broken client. The design is detect-and-report | Follow-up: sweep every captured packet body in `target/vanilla-capture/` through its decoder and assert exhaustion — that converts "a decoder left bytes behind" into a measurement over ~19 000 real packets |
| A-04 | L | **Not re-derived** | — | Two searches (decoder arms consuming fewer bytes than declared; a clientbound field written in two places) produced no site. The A-03 sweep is the experiment that would settle it |
| A-05 | L | **Not re-derived** | — | As A-04 |
| A-06 | I | **Not re-derived** | — | As A-04 |
| B-01 | **H** | **Fixed** | `Game::unreadable_chunks`: a chunk whose read *failed* is recorded and `may_generate` is false for it for the session, so generated terrain can never be written over a file that exists but could not be read. The mark is deliberately not cleared on unload — the file is still unreadable | `game.rs`: the insert in the `read_failed` branch and its use in the `may_generate` expression. Prior behaviour reproduced by reading the pre-fix branch: read error → generation → clean chunk → first edit → autosave overwrites |
| B-02 | M | **Open** | Not applied — the test's property is an *ordering*, and ordering is invisible to a reader that only looks at the end state | Follow-up: an ordering-observable instrument — a writer that records the sequence of (offset, bytes) it emits, so the assertion is on the sequence rather than the resulting file |
| B-03 | L | **Not re-derived — contradicted by the instrument named** | Nothing to fix here: the only doc-claims scanner in the tree (`target/scan_doc_claims_copy.py`, whose output is `target/audit_scan_doc_claims.txt`) **does** include `invariant` in its pattern list, as the last alternative. The finding as stated does not reproduce | `target/scan_doc_claims_copy.py` lines 37-41, read in full. Follow-up: the same one B-04 needs — commit the instrument, so "which scanner" is answerable |
| B-04 | L | **Open** | Not applied. The scanner visits a line only when it starts with `///` (line 49), so `//!` module documentation is never read — which is where this repository puts its most load-bearing prose (`spawn.rs`'s provenance, `loot.rs`'s census). The fix is to scan both prefixes | `target/scan_doc_claims_copy.py` lines 49-50, read in full. Follow-up: commit the instrument under `tools/docs-audit/`, accept both prefixes, and plant a `//!` claim in a test to prove it is visited |
| B-05 | M | **Fixed** | `mc_world::chunks_a_block_can_light` is the rule, once: all nine offsets with each axis tested separately, so the **diagonal** at a corner cannot be forgotten. `World::invalidate_light_around` and the server's `light_update` queue both iterate it | `light_cache.rs`: `a_change_on_a_corner_also_drops_the_diagonal_neighbour` (new) and `the_affected_chunk_rule_agrees_with_the_margin_derived_from_its_definition`, which compares the rule against an oracle derived independently from the margin interval over all 256 local positions of two chunks (one with negative coordinates). 8 passed. Falsified by re-adding the old `dx == 0 \|\| dz == 0` restriction: the corner test fails; tree restored byte-exact |
| B-06 | L | **Fixed** | `unload_distant_chunks` removes the position from `placeholder_without_storage` when it unloads a chunk, so the mark means "this *live* chunk must not be written" | `entity_lifecycle.rs`: an exact invariant — `placeholder_chunk_count() == world.chunk_positions().count()` in a borrowing game, where every loaded chunk is a placeholder. Falsified by deleting the removal: the assertion fails; tree restored byte-exact |
| B-07 | I | **Not re-derived** | — | The B-02..B-06 experiments surfaced no seventh defect |
| C-01 | M | **Fixed in `8a6a459`** | `spawn::despawn` gates **both** discard paths on `MobCategory::despawns_by_distance`, with the bytecode offsets (98-102 distance, 161-166 idle) in the comment: a persistent category survives every path | `git show 8a6a459` — the property test over 2 000 seeds was added with the fix |
| C-02 | M | **Fixed in `8a6a459`** | `try_spawn_pack` takes `counts: &mut [i32; 2]`, so a cap reached mid-cycle blocks the same cycle's later positions | `git show 8a6a459` (`counts: [i32; 2]` → `&mut [i32; 2]`) |
| C-03 | M | **Fixed in `8a6a459`** | The module truth-telling list, the phase table and three stale comments updated against HEAD; the spawn simplification corrected and the unmodelled thundering case of `isDarkEnoughToSpawn` named | `git show 8a6a459` (`crates/server/src/spawn.rs`, +37/−5) |
| C-04 | M | **Fixed** (policy chosen) | Policy: **log and continue**. `ops.rs`'s module doc and `OperatorList::load` now state that the load returns the error and the *caller* decides, with the lifecycle's reasoning and the vanilla parallel | Read-back of both docs. `javap -c` on `StoredUserList.load` shows it declares `throws IOException` and catches nothing, so vanilla leaves the choice to its caller too |
| C-05 | L | **Fixed in `8a6a459`** | Named in that commit's message as part of the documentation-truth fix | `git show 8a6a459` |
| C-06 | L | **Fixed** | `execute.rs`'s comment now says what the code does: `select` attaches each matched player's own permission (`with_permission(session.permission)`), so the permission travels with the new source; the vanilla-consistency of that is marked as the lane's finding, not re-derived | Read-back against `select`; `execute_e2e` pins our side |
| C-07 | I | **Fixed in `8a6a459`** | As C-05 | `git show 8a6a459` |
| C-08 | L | **Open** | Not applied: the fix is to build the dispatcher once and reuse it, which touches the command path this landing does not otherwise modify | Follow-up: hoist `Dispatcher::new(build_command_tree())` to a once-per-game value; measure the allocation with the command suite's timing harness rather than asserting it |
| D-01 | M | **Fixed** | Cow `MOVEMENT_SPEED` `0.25` → `0.20000000298023224` | Re-derived for the findings document with `javap -c` on `AbstractCow.createAttributes`, which also confirms `MAX_HEALTH = 10.0` |
| D-02 | **H** | **Fixed** | `resolve_entry` returns `Vec<ItemStackLike>` and the caller extends rather than assigns, so a nested `minecraft:loot_table` produces **every** stack | `crates/data/src/loot.rs` (uncommitted diff: the signature and the `chosen = resolve_entry(...)` call site); multi-stack test added with it |
| D-03 | M | **Fixed** | `minecraft:limit_count` clamps **up** to `min`, the jar's `Mth.clamp`, instead of discarding the stack | `loot.rs`; the test that had enshrined the zeroing was rewritten — a test written from the same misreading as the code cannot catch it |
| D-04 | M | **Open — deliberately not applied** | `AGGRO_RADIUS = 16.0` is jar-measured as `Mob.createMobAttributes`'s **default** `FOLLOW_RANGE`; `Zombie.createAttributes` overrides it to a jar-measured **35.0**. Applying 35 changes the difficulty of every night, which is player-visible and belongs to the owner | `javap -c` on `Zombie.createAttributes`, `Mob.createMobAttributes`, `LivingEntity.createLivingAttributes`, re-derived for the findings document. The constant's doc and the module gap list now carry the measurement, the consequence and the decision |
| D-05 | L | **Fixed** | `Rng::next_f64` draws 26 bits then 27, Java's own shape | `loot.rs` (uncommitted diff), with the reason in the comment |
| D-06 | L | **Fixed** | All six sites (`docs/research/data-pack-baseline.md` section 0 and five differential tests' doc comments) now give an absolute `%CD%`/`$PWD` form, and the canonical document explains why a relative one fails | `grep` for the relative form returns nothing; the reason is that `cargo test` sets the test binary's working directory to the package, which is also why `root.is_dir()` failed |
| D-07 | L | **Open** | Not applied. The NBT writer should refuse a heterogeneous list, but the change belongs with the writer's error surface | Follow-up: check every element's tag id against the first and return `CorruptData` naming the index; a test with `[Int, String]` |
| D-08 | I | **Not re-derived** | — | No site resolved |
| E-01 | **H** | **Fixed** | Eleven `PARITY-MATRIX.md` rows corrected (KD-16 "no mob ever spawns" and the tick-order row both false), `TEST-MATRIX.md` counts brought to this landing's run, and a Phase 11 `CHANGELOG.md` section written | The corrected rows; the counts below come from this landing's own gate run, not from the previous one |
| E-02 | M | **Open** | Not applied. Audit 08's M2 pinned the *precedence* of the override by passing it as a parameter, which deliberately avoids process-global state; the environment half is what is untested | Follow-up: a test that sets `MC_FIXTURE_DIR` in a child process (not in-process, which would race other tests) and asserts the named directory is the one searched |
| E-03 | **H** | **Fixed (self-reference) / Open (the hostile client)** | The tautology is closed from outside: the 26.1.2 server jar's own `version.json` is committed verbatim as `crates/test-support/fixtures/protocol/version.json` (with its source's size, sha1 and sha256 in the `MANIFEST.txt` beside it), and `packet_ids::the_protocol_version_matches_the_jars_own_version_json` asserts that `mc_protocol::ids::PROTOCOL_VERSION` equals the jar's stated `protocol_version` (775), that the fixture is the 26.1.2 jar's, and that `docs/protocol/packet-ids-775.tsv` exists for that version. `EXPECTED_PROTOCOL`'s doc now states the chain — jar → our constant → the alias the tests compare against — so a reader can see why the alias is not vacuous. The **hostile-realistic `TestClient` mode** (serverbound play 13 every tick) is **not** written | `cargo test -p mc-protocol --test packet_ids` = 6 passed. Falsified on both sides (`target/falsify_protocol_pin.py`): setting the fixture to 776 fails the test, and setting `ids.rs` to 776 fails it, so neither side is hard-coded; tree restored byte-exact. Follow-up: the per-tick `client_tick_end` mode, exercised over a full session |
| E-04 | L | **Not re-derived as a finding; the gap it named is closed** | The lane's E-04 was a coverage gap, and the one coverage gap that demonstrably existed when the audit ran — P11-04..08 had no tests — is closed by `loot_and_pickup.rs` (8), `entity_persistence.rs` (2) and `player_attack.rs` (5) | 15 new integration tests, all passing; two written specifically to fail against the reversed-argument defect. No *other* specific gap was re-derived, which is why the finding itself stays in the not-re-derived column |
| E-05 | L | **Open** | Not applied: the scanner's helper names are a readability defect in an uncommitted instrument | Follow-up: as B-03/B-04, rename for the property each checks |
| E-06 | I | **Not re-derived** | — | No site resolved |
| E-07 | I | **Not re-derived** | — | No site resolved |
| E-08 | I | **Not re-derived** | — | No site resolved |

## Open divergences this landing records but does not close

These are not AUDIT-09 findings; they are limits found while closing them, and each
is a player-visible divergence that the owner should decide rather than inherit.

1. **An entity in a chunk nobody has modified is not persisted** (P11-08's limit).
   The build saves dirty chunks and leaves generated ones clean by design, and
   spawning an entity does not dirty a chunk. Vanilla's save set is not the same set.
   Closing it needs the terrain-dirty and entity-dirty cases separated so that an
   entity change can mark a chunk for saving without preventing it from unloading.
   `entity_persistence.rs`'s module documentation states the condition its tests rely
   on, and the parity matrix carries it in the KD-18 row.
2. **A swing outside the interaction range is not refused** (P11-06). Vanilla checks
   `canInteractWithEntity` with a 3-block range before applying damage; this build
   applies whatever `interact` names. Not fixed here because the range constant would
   need the same jar treatment as `AGGRO_RADIUS`, and the effect is a cheat surface
   rather than a broken world.
3. **A held item's damage is not used** (P11-06). The fist figure (1.0) applies
   whatever is held, so a diamond sword hits as hard as a hand.
4. **The zombie's follow range** (D-04 above): our night is less dangerous than
   vanilla's by design, pending the owner's decision.
5. **Most shipped loot tables drop nothing** (found while preparing this landing's
   handover, after the landing itself was pushed and CI-verified — which is the point:
   the tests that existed could not see it). `mc-data::loot::roll` refuses a whole
   table when any construct in it is unmodelled, and the 26.1.2 pack contains
   unmodelled constructs in most tables. Measured from the extracted pack
   (`target/loot_condition_census.py`, 1 326 tables): `match_tool` **156**,
   `block_state_property` **149**, `entity_properties` **27**, `killed_by_player`
   **22**, `table_bonus` **17**, `random_chance_with_enchanted_bonus` **11**; **167**
   tables are enchantment-gated on their own. Since the break path supplies
   `enchantment_levels: None` — which this crate defines as "the tool is unknown" —
   those tables refuse, and the rest refuse on their own unmodelled conditions. So
   mining stone and killing a cow yield **nothing** where vanilla yields cobblestone
   and leather. P11-04's tests use a hand-written pack with none of those constructs,
   so they prove the wiring and cannot see the interaction; the differential test
   their module docs promised in `vanilla_loot.rs` was never written, and that
   dangling reference is now corrected. Two candidate fixes, both player-visible and
   both therefore the owner's call: supply a **known, unenchanted** tool
   (`Some(empty map)` — the module's own reading of "tool known, level 0") instead of
   `None`; and decide whether an unmodelled construct should refuse the **pool**
   rather than the table. The smallest experiment that fails today: load the extracted
   pack, break `minecraft:stone` bare-handed, assert `minecraft:cobblestone`.

## Final verification (this landing's tree)

- `python tools/gates/run.py --quick` on this tree: **every gate passed**;
  `cargo test --workspace --no-fail-fast`: **1 344 passed / 0 failed / 30 ignored /
  102 suites**. All four docs-audit checkers exit 0 — `check_gate_totals.py` forced the
  total to be updated in `README.md`, `CONTRIBUTING.md` and `RELEASE-CANDIDATE.md`
  alongside its owner `docs/testing/TEST-MATRIX.md`; the release record keeps its own
  1 212 and now says why, because rewriting it would falsify what the release measured.
- Falsification, each re-broken and re-restored with a SHA-256 check of the file:
  `target/p11_falsify.py` (the save's entity list; the chunk-stream announcement),
  `target/falsify_ids.py` (a transposed packet id; a renamed state module),
  `target/falsify_world_fixes.py` (the diagonal invalidation; the unload cleanup),
  and `target/falsify_protocol_pin.py` (both sides of the protocol pin).
  Seven perturbations, seven failures, seven byte-exact restores.
- The audit's own instruments (`target/audit_lane_*.py`,
  `target/scan_doc_claims_copy.py`), the loot census
  (`target/loot_condition_census.py`, whose numbers are quoted above), the
  measurement scripts (`target/measure_tests.py`, `target/count_tests_from_source.py`)
  and the re-derived `javap` runs (`Zombie`,
  `Mob`, `LivingEntity`, `AbstractCow`, `StoredUserList`) are uncommitted: they are
  provenance for this document. **That is itself a finding** — B-03 could not be
  settled because the instrument exists only as an uncommitted copy, and B-04's fix
  needs the instrument committed before it can be tested.
