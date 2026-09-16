# AUDIT-12 — P00~P12, weighted by the historical holes

Basis: `c274050` (P12 docs + acceptance record), audited read-only. Fixes for the
High findings landed after the lanes as `74ea53c` (pack join + regression test)
and `caabaf0` (stale-craft guard, break cursor, furnace viewer resync + 3 tests);
doc touch-ups (matrices, README, scanners) ride with this document. No lane
changed the tree. Verdicts: **confirmed** (independent instrument unless labelled
"re-run"), **refuted**, **not checkable** (reason).

## Why these weights

P12 added ~13 commits of never-audited code (wire + menus + tick + persistence +
crafting + pack join), so the container/server lanes carry the most weight. The
historical opens concentrate in tests/docs (AUDIT-09: A-03, B-02, B-04, C-08,
D-07, E-02, E-05; plus M-3 anchor, D-04, fall-damage arithmetic), so one lane
re-derived each open against the current tree. The fifth lane ran the falsification
pass; the sixth re-derived every load-bearing count. The "what nobody looked at"
lane is folded into each lane's NOT-checked list — it produced the highest-value
finding in two of the last three rounds, so every lane names its blind spots
instead of implying coverage.

## The per-claim table (P12 exit gate + totals)

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| Gate 1380/0/34/108 on `c274050` | gate re-run (weak: same instrument) | confirmed | `1380 passed / 0 failed / 34 ignored / 108 suites, every gate passed` |
| Chest/furnace/hopper open on non-zero windows with jar MenuType | read + `survival_e2e::right_clicking_a_chest_opens_a_window` | confirmed | `MENU_* 2/14/16` javap-verified; `open_screen` 59 + contents observed; double-chest-single + barrel-as-chest recorded |
| Chest transactions conserve under flood | read + flood test (20-click, total pinned each round) | confirmed | `menu_total_items==64` across 20 QuickMove rounds |
| Furnaces cook + report `container_set_data` 19 | read + furnace test (progress>0 in 5 ticks, id 19) | confirmed | deltas `[burn,burn_total,cook,cook_total]`; widths VarInt+short+short javap-verified |
| Hoppers pull+push on 8-tick cooldown with resync | read + hopper test (both directions in 20 ticks) | confirmed | push-then-pull single-side divergence recorded; furnace routing skipped |
| Block entities persist + breaks drop + viewers close | read + second-`Game` 17-stone test + break test (≥12 drop) | confirmed | NBT shape self-honest; vanilla resets furnace progress (documented) |
| Crafting recomputes + consumes; tables from pack | read + sticks e2e + `vanilla_crafting` differential (real data, >200 convert) | confirmed, then **refuted as shipped** | hook + conversions verified; **but `load_packs` joined `root/<ns>` onto an already-joined dir, so every boot loaded 0 recipes (A12-16)** — fixed in `74ea53c`, regression test fails without the fix |
| Closes flush + return cursor; cursor rides 96 | read + close test (window 0, cursor empty, 16 back) | confirmed | `set_cursor_item` 96 replaces slot -1 |
| P12-10 + P11-10 screen acceptance | read (no rig trace exists) | confirmed honest (NOT RUN) | CHANGELOG + matrix record the gap; no screen claimed |
| `/tp`-to-air never embeds; fall per-tick ≤5 | read + tp test (feet/head air, `pos.y≈sy`) | confirmed | `find_surface` resolution; 20-block single drop = 17 dmg/3 HP is arithmetic, test kills via `apply_damage` (weak, labelled) |
| Totals 1380 → 1384 after remediation | full gate re-run (see §7) | confirmed | 1380 + 4 (pack recipe, stale craft, break cursor, furnace view); suites 108 unchanged |

## Lane 1 — protocol & wire (P12 packets + historical A-03)

- **A12-01 · Medium · `ContainerSetContent/Slot` window is `i8`, vanilla is VarInt.** Bytes coincide for 0..=127 — everything this server sends (`next_window` wraps 1..=127, player 0) — and diverge at ≥128/negative. Open: re-derive on the day windows exceed 127. (`play.rs:3164,3215` vs jar `CONTAINER_ID`; confirmed by javap.)
- **A12-02 · Medium · serverbound close reads `u8` + no exhaustion check.** Window 128 (`80 01`) decodes as 128 with one byte silently dropped; `PlayIntent::decode` never checks exhaustion (only 3 clientbound moves use `require_exhausted`). Masked today (windows <128, no trailing bytes in practice); changing to hard refusal risks a real client per A-03 reasoning — recorded, not changed.
- **A12-03 · Low · unvalidated widths.** `ItemStack::decode` refuses negative id but not negative/huge count; `OpenScreen` VarInts unvalidated; `text_from_nbt` returns `""` for compound-without-`text` instead of error. Gaps, not live bugs.
- **A12-04 · Low · `packet_ids` test gap.** Joint TSV+`ids.rs` edit passes (verify TSVs never asserted by cargo); single deletion passes the property test alone (soundness-only). The 4 P12 ids are covered both explicitly and by the property test — verified.
- **A-03 (AUDIT-09) · still open.** Serverbound silent-accept / clientbound-test-decoder hard-refuse; neither side implements detect-and-report. No new instance added by P12 (new decoders hard-refuse trailing bytes — correct for clientbound).

## Lane 2 — persistence, world, block-entity NBT (P12-05/06)

- **A12-05 · High · break-before-close lost the cursor — FOUND, FIXED in `caabaf0`.** Open chest → pickup to cursor → break: drops covered entity stacks only; the viewer rebuild used a fresh empty cursor and sent empty `carried`. Fix returns the cursor (inventory, feet on overflow) like `ContainerClose`. Regression: `breaking_a_chest_with_a_full_cursor_keeps_the_cursor` (16 survive).
- **A12-06 · Medium · two viewers, last-writer-wins.** Block half has no version: A writes slot 0 → entity; B with a stale view writes slot 1 → whole-array `write_back_block` overwrites. No state-id on the block half. Open.
- **A12-07 · Low · kind-drift.** Direct `set_block` chest→furnace keeps the 27-slot chest entity (`is_none` false → no replace); furnace open takes 3, write-back `3!=27` no-ops, file persists chest NBT at a furnace pos. Admin-reachable; open.
- **A12-08 · Low · sign asymmetry.** Serializer emits `minecraft:sign`; loader skips non-container/furnace/hopper. Writer implies persistence the loader discards. Open (payload gap already declared).
- **A12-09 · Low · `Count:Int` vs vanilla.** No vanilla chest/furnace fixture decides (`anvil_fixture` asserts empty BE; `vanilla_chunk` never touches BE). Self round-trip honest; vanilla-boot count behavior unknown. Needs a real chest/furnace fixture.
- **A12-10 · Low · dirty gaps.** Hopper cooldown tick-down and direct `block_entities_mut` writes (today only tests) don't mark dirty; broadcast create/retire piggyback the world-edit dirty. Open.
- **B-02 · still open.** Location-word ordering test asserts end-state; passes under reordered write. BE adds no new instrument.
- **B-01 · holds with BE.** Unreadable-chunk guard inserts before any BE load; placeholder air refuses opens. Adjacent leak (not loss): `unload_distant_chunks` never prunes `BlockEntityStore` (stale entries linger; `audit_against`/`prune` uncalled by `Game`). Open.

## Lane 3 — tick, furnace/hopper, entities, phases

- **A12-11 · Medium-High · furnace item changes never reached viewers — FOUND, FIXED in `caabaf0`.** Deltas carried only the 4 data slots; the menu block half diverged (entity smelted, menu showed pre-tick stacks) and the next click flushed stale stacks back over the output. Fix merges furnace item-changed positions into the hopper-style refresh+`ContainerSetContent` resync. Regression: `an_open_furnace_menu_shows_completed_output` (seeded 195/200 → menu shows the ingot).
- **A12-12 · Low · idle hoppers retry every tick.** Cooldown set only on move; empty/blocked attempts leave 0 → neighbour lookups + temp containers every tick (8× vanilla rate, 1-tick latency). Deterministic; docs label the 8-tick figure as busy-only.
- **Stale docs — FOUND, FIXED with this document.** `game.rs:18` BlockEntities no-op table, `block_entity.rs` no-op/ticking bullets, `entity_lifecycle.rs:459` two-no-op-phases comment, `tick_baseline.rs:56-61` spawn/AI staleness, `packs.rs:25-31` "no recipes" header, `smelting_data.rs:5` "six recipes" (seven). All corrected.
- **Determinism — confirmed.** Ascending `BTreeMap` walk, furnace-before-hopper every tick, `hopper_touched` sorted+deduped; vertical chains process bottom-first as an `x`-major side effect (named, not a bug). `PHASE_ORDER` untouched; `ScheduledTicks` still a documented no-op (P13's first task).
- **History — confirmed.** M-3 still 30.41 (pigs/skeletons ~8% slow, recorded); M-4 lookahead unchanged (no ledges, recorded); fall per-tick math matches the P11 test's 5-max claim; zombie `FOLLOW_RANGE` still default 16 (D-04, recorded). P12 touched no entity paths (sole `tick_block_entities` caller is the BlockEntities phase).

## Lane 4 — menus, crafting hook, close/cursor, floods

- **A12-13 · High · stale result-take consumed the grid — FOUND, FIXED in `caabaf0`.** `is_result_take` forced the hook even when `apply_click` returned `full_resync` (nothing applied); `take_craft_result` ate ingredients for no result. Fix gates on `!outcome.full_resync`. Regression: `a_stale_result_take_consumes_nothing` (perturbation-verified: guard removed fails grid 0 vs 2).
- **A12-14 · Medium · `menu_inventory_divergence` hard-coded container 0 — FOUND, FIXED with this document.** Block menus hold the player at 1; the diagnostic compared chest-vs-inventory. One-line fix to `player_container_index`; callers are window-0 tests only.
- **A12-15 · Low · close-rebuild failure double-spends (theoretical).** Cursor copied then `add_stack`ed without clearing; `new_player_menu()->Err` (registry corruption only) logs and keeps the old menu + inventory copy + `open_block`. No hostile path; recorded.
- **F3 · Low · stale-take-no-match resync confirms the duplicated cursor.** Unreachable while recompute/craft share `find_match` in one tick; no revert logic. Fragile, not live.
- **Conversion notes (Info).** Tags fail closed (whole recipe refused, counted); ragged data coerces via `ingredient_at→None` rather than `malformed` (vanilla pack never ragged); `Self::new` duplicate check unreachable via `from_book` (book last-wins first) — doc misleading, behavior correct.
- **Flood gap (Info).** Non-zero-window server path flooded only with QuickMove (20-click); Pickup variants, Throw/outside-`-1`, Swap, Clone, QuickCraft stages, PickupAll, stale/foreign ids, capped overflow covered in unit isolation, not through mirror/write-back/block-flush. No bug found in checked shapes.

## Lane 5 — packs, commands, ops, lifecycle (P12-07/08)

- **A12-16 · High · pack recipes silently loaded zero — FOUND, FIXED in `74ea53c`.** `root.join(namespace)` onto an already-joined `.../data/<ns>` built `.../data/<ns>/<ns>`; every iteration `continue`d with `rejected==[]`, baseline silently retained. Fix uses the plan directory directly + recipe counts in `summary()`. Regression: `a_world_pack_recipe_reaches_the_crafting_table` (perturbation-verified: join restored fails with 0 recipes).
- **A12-17 · Medium · `Ok(empty)` masking (remainder).** `rejected` covers open-failures and `Err` conversions only; empty book + tag-skipped conversions surface solely via the new summary counts (`recipes_loaded/crafting_converted/skipped/smelting_rows`), and `has_problems()` stays false. Partially fixed (counts logged); `has_problems` semantics unchanged by design.
- **A12-18 · Medium · tag/unknown counts dropped at the join.** `SmeltingRegistry` tag/duplicate/empty/unknown buckets and crafting `unknown_results` never reach `PackLoadOutcome` (only rows/converted/skipped-totals). Library counts correctly; join discards. Open.
- **C-08 · still open.** Dispatcher rebuilt per command/inner-execute (9 inserts); no cache. Perf only.
- **/tp + ops — confirmed.** Air resolves to surface via `find_surface` (test matches code); cross-player refusal holds; `/op` still honestly refuses (no write path); baseline-vs-vanilla labelling clean.

## Lane 6 — documents vs tree, counts

Per-claim table (12 load-bearing claims + honesty notes): totals 1380 confirmed pre-remediation; lib spot 116/133 confirmed (other 15 taken from doc — weak); named 346 derived-not-counted (weak, doc admits); `survival_e2e` 19, `block_entity_e2e` 7, `packet_ids` 6/109 confirmed (play=70 of 109, naming caveat noted); CHANGELOG deltas match `f0835f8..c274050`; P12-01..09 behavior confirmed without screen claims; P12-10 + P11 tp correction confirmed honest (17/3 arithmetic weak — no physics end-to-end); parity rows use `partial` with evidence, never `full` for P12; README/CONTRIBUTING totals match after this round's update.
**Refuted (fixed with this document):** README "entities never persist/sync", "only player inventory opens", "no real-client acceptance", "21 ignored", differential "7 suites" — all stale after P10-12.
**New instance fixed:** B-04 scanners now read `//!` in both `scan_doc_claims.py` and `scan_doc_equalities.py` (claim hits 1524 with module docs).
**Unchanged opens:** E-02 env-half, E-05 helper opacity (no new instance), D-07 hetero-list (no new hetero encode; no refusal added).

## Falsification pass (`target/m_probes.py`, guard kept)

13 legacy perturbations (M-1..M-4b, N-1a..e) + 9 new P12 probes (menu-type, set-data widths,
chest slots, no-open, no-furnace-data, no-persist, no-recompute, pack join, stale-guard):
**22/22 failed as they should**, files restored byte-exact under SHA-256
(play `15c94ff1…`, game `bde16d47…`, mob `9d383187…`, menu `a669ce65…`, packs
`86eee035…`; before == after all five, backups cleared). The `target/probe-backup`
startup-restore guard was not removed. Per-test mapping is in the script; the P12-8
(join) and P12-9/F2 (stale guard) probes were additionally verified by hand
(fail-then-restore) during the fix.

## Counts, decomposed

- Workspace gate: **1384 passed / 0 failed / 34 ignored / 108 suites** (1380 + 4
  remediation: pack recipe, stale craft, break cursor, furnace view; suites unchanged).
- Libs re-measured: `mc-protocol` 116, `mc-container` 133, doc-tests 5.
- Named suites: `survival_e2e` 22 (19+3), `block_entity_e2e` 7, `pack_loading_e2e` 9 (8+1),
  `packet_ids` 6; ignored still 34 (differentials incl. `vanilla_crafting` + benchmarks).
- Ids: 109 packet constants in `ids.rs` (play 70); TSV 256 rows; game 69+141.

## What nobody looked at (not covered this round)

`apps/capture-rig` + traces, `tools/pi-bench` + Pi 5 acceptance rerun, network
admission/flood/backpressure (A-01 shared-queue fairness), `execute`/`function`
chains and privilege traversal, redstone model, lighting engine, worldgen
pipelines, `TestClient` hostile-realistic mode (E-03), NBT hetero-list refusal
(D-07), command-suggestion/ping/cookie paths, `ops.json` malformed-policy
beyond startup. Weights went to P12 + historical opens by instruction.

## Previous audits vs reality

- AUDIT-09 opens: A-03 open (unchanged), B-02 open (unchanged), B-04 fixed for
  scanners (new `//!` instance from P12 closed with it), C-08 open (unchanged),
  D-04 stands (16 kept, recorded), D-07 open (unchanged), E-02/E-05 open
  (no new instance). B-01 holds; B-03 contradicted stands.
- AUDIT-10: count-hygiene lesson applied (all counts decomposed, weaks labelled);
  `vanilla_loot` differential still the loot authority.
- AUDIT-11: N-1 refutation stands (no zero-digs in any trace; reach is the jar's
  arithmetic); M-1/M-2 probes re-run green inside the 22; M-3 anchor untouched.

## What this audit changed in the tree

Nothing — lanes were read-only. Dispositions: **fixed** A12-05, A12-11, A12-13,
A12-16 (+ A12-14, stale docs, README, scanners) in `74ea53c`/`caabaf0`/this
commit, each with the instrument that pins it; **open** A12-01, A12-02, A12-06,
A12-07, A12-08, A12-09, A12-10, A12-12, A12-15, A12-17, A12-18, B-02, C-08, D-07,
E-02, E-05 — each with the experiment that would close it (named above).
