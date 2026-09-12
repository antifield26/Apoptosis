# Test Matrix — current view

Conventions: the test-level vocabulary `L1` unit · `L2` property/fuzz ·
`L3` integration · `L4` golden/fixture · `L5` end-to-end · `L6` differential vs
vanilla · `L7` regression-per-bug is defined in
[CONVENTIONS.md §11](../CONVENTIONS.md).

Totals: **1 194 passed, 0 failed, 21 ignored** across **74 suites**, re-derived
from `cargo test --workspace --no-fail-fast` on the remediated tree (Audit 07;
1 191 before its two atomicity tests and one conservation test were added). The
21 ignored = 7 differential suites (15 tests, jar-gated) + `pi_profile` 4 +
`tick_baseline` 2 — all run on demand (see the last section).

**Every per-crate count below was re-measured with `cargo test -p <crate> --lib`** while updating this
total, and five were wrong: `mc-protocol` said 126, `mc-world` 103, `mc-command` 27, `mc-data` 4 and
`mc-worldgen` 64, where the real figures are 103, 31, 89, 126 and 81. Each stated value turned out to be
**another crate's** count, so the figures were right and the names were rotated (Audit 07 remediation;
the audit itself missed it — see `docs/audits/AUDIT-07-REMEDIATION.md` §"what the audit missed"). The lib
counts sum to {lib_sum}, the {doc_sum} doc-tests and {NAMED} named-suite tests
complete {TOTAL}.

This file replaced an accumulator that had grown one per-phase section per
phase (255 rows, six duplicated "Bugs found" tables). The per-phase historical
matrices are preserved in git history at tag **`phase-09-final`**; what stays
here is the matrix a contributor can actually use: what is covered now, what
each suite proves, and the deduplicated defect history.

## Current coverage by area

Counts are from the 2026-09-12 run log. Named integration suites are counted
explicitly; the remaining per-crate lib binaries (core, nbt, registry,
simulation, redstone, server, test-support and the rest) complete the 1 194
total and are itemised in the run log rather than here.

| Area | Named suites (count) | What they prove |
|---|---|---|
| Protocol | `packet_ids` (4), `fixtures` (4), `mc-protocol` lib (103) | every packet id matches the jar's registration bytecode (incl. the `chat_command`=7 regression, L7); golden wire bytes for frames/handshake/NBT; hostile VarInt/frame corpora; compression bomb rejection |
| Network | `mc-network` lib (16), `keepalive` (1), `login_tolerance` (2), `e2e_login_play` (6) | connection lifecycle, admission limits, keepalive timeout kick, malformed input drops only that connection, login-phase tolerance, full offline login over a real socket |
| Persistence | `mc-persistence` lib (74), `anvil_fixture` (9), `corruption` (16), `restart` (7) | region/NBT codec edges, byte-identical palette repack, bit-flip → typed error with no partial publish, save→close→reopen semantics, atomic tmp→rename pinned by `a_failed_commit_leaves_the_live_file_untouched` (Audit 07 finding H1), dirty-flag retry on failure |
| Survival & world | `mc-world` lib (31), `vanilla_chunk` (4), `survival_e2e` (7), `network_game_bridge` (4) | collision/ray/hostile movement guards; a real vanilla chunk walks and round-trips losslessly; join/stream budgets, break/place validation, death/respawn, save-reload over real sockets |
| Entities & simulation | `mc-entity` lib (127 + 4 doc), `entity_lifecycle` (9) | ids never reused, timers/effects/projectiles/pathfinding invariants, JDK-25-verified RNG, phase ordering, determinism replays, spawn/despawn/chunk-unload lifecycle |
| Inventory & containers | `mc-container` lib (130), `container_e2e` (6), `block_entity_e2e` (6), `inventory_duplication` (5) | click/swap/drag conservation, stale-state resync, computed slots, retirement reporting; real-socket click round trips; 2 000-click floods cannot create or destroy items — the flood over a **capped slot** reaches the over-limit path the chest flood cannot, so a discarded overflow is caught (Audit 07 finding M1) |
| Redstone | `propagation` (17), `budget_exhaustion` (7), `determinism` (6), `golden_circuits` (5), `power_model` (8), `world_integration` (4) | budgeted propagation reaches unbounded-run state, full change-vector determinism, golden circuits, power bounds; the model is complete and (deliberately) not tick-wired |
| Commands & data | `mc-command` lib (89), `command_e2e` (11), `execute_e2e` (9), `function_e2e` (14), `mc-data` lib (126), `pack_discovery` (10), `pack_loading_e2e` (8) | permission-before-grammar, every declared command reachable, malformed commands never disconnect, execute modifier chains, function recursion/privilege bounds, pack discovery and world-pack loading |
| World generation | `mc-worldgen` lib (81), `worldgen_e2e` (7), `structure_golden` (9), `seed_derivation` (8), `golden` (6), `determinism` (7) | seed determinism, terrain invariants, structure placement goldens, existing-world-first generation |
| Server foundations | `mc-server` lib (44), `mc-core` (10), `mc-nbt` (16 + 1 doc), `mc-registry` (14), `mc-simulation` (27), `mc-redstone` (64), `mc-test-support` (4) | config guardrails, backup/verify/restore, shutdown barrier, operational metrics snapshot, error taxonomy, NBT vectors, the registry table itself, fixture helpers, the tick scheduler, the redstone model's own invariants |
| Security (cross-cutting) | classes live inside the suites above | hostile VarInt/frames + random-byte connections (protocol/network libs), slow-drip bound, registry reservation cap, 300-command flood (`command_e2e`), hostile op paths (`ops_e2e`, 7), 2 000-click conservation (`inventory_duplication`), hostile disk state (`corruption`, 16), config bounds (`mc-server` lib) |

## Differential suites (jar-gated, `--ignored`)

Need the three environment variables below; they skip cleanly without them.
Evidence as of 2026-09-12: **15 passed / 0 failed across the 7 suites**.

```sh
export MC_VANILLA_DATA="$PWD/target/vanilla-26.1.2/extract/data/minecraft"
export MC_VANILLA_JAR="$PWD/target/vanilla-26.1.2/server.jar"
export MC_VANILLA_WORLD="$PWD/target/vanilla-26.1.2/vanilla-world-26.1.2/world"
```

| Suite | Tests | Proves |
|---|---|---|
| `vanilla_pack` (mc-data) | 1 | 758/758 vanilla tags resolve with zero problems; 1 421 recipes load, 94 counted as unmodelled |
| `vanilla_data` (mc-data) | 1 | the real data pack loads end to end |
| `vanilla_smelting` (mc-container) | 1 | furnace table consistency against the jar's own values |
| `structure_pack` (mc-worldgen) | 6 | real structure templates place exactly where they declare; every palette block resolves |
| `scenario_vanilla` (mc-server) | 2 | the five-stage pipeline (tags → recipes → furnace → terrain → structure) runs against the real pack; borrow-safety |
| `structure_wiring` (mc-server) | 2 | generated chunks are decorated from the real templates consistently |
| `vanilla_differential` (mc-persistence) | 2 | 529 vanilla chunks decode; our rewritten world boots on the real vanilla 26.1.2 server, keeps our marker and re-saves every dimension |

## Performance suites (on demand)

`pi_profile` (4) and `tick_baseline` (2), `--ignored --nocapture` — the
workload driver, chunkgen burst, dirty-save timing and profile run, with
settled-vs-burst separation. Records and the Pi 5 acceptance verdict:
[BENCHMARK-BASELINE.md](../performance/BENCHMARK-BASELINE.md).

## Defect history (deduplicated, L7)

Every bug below was found by the tests (or the adversarial audits the tests
pin), fixed, and covered by a regression. Merged from the six per-phase tables
of the pre-governance matrix — 49 entries, none duplicated.

| Phase | Defect | Found by | Fix |
|---|---|---|---|
| 03 | `PalettedContainer::set` widened the palette without re-packing, mixing two bit widths in one array | `tests/restart.rs` | track the packed width, repack on change |
| 03 | A failed chunk write was removed from the dirty set, so the next flush never retried it | `corruption.rs` (read-only directory) | failed chunks re-marked dirty and stay queued |
| 03 | Region reader rejected a short final sector, which a real vanilla world can contain | `vanilla_differential.rs` (real `r.-1.-1.mca`) | payload bounded by `min(allocated, present)`; regression test |
| 03 | `cargo build -p mc-server` failed on Windows (tokio `signal` feature hidden by feature unification) | process smoke run | feature declared in the crate manifest |
| 04 | Endpoint-only collision allowed tunnelling through a floor | `falling_stops_on_the_floor` | swept-box per-axis resolution |
| 04 | `clip_axis` reconstructed its base box wrongly, so every horizontal move passed | the same test | take the pre-step box; explicit full-step shortcut |
| 04 | Heightmap packing shifted 9 bits past a `long` boundary | `vanilla_chunk.rs` | reuse the verified shared packer |
| 04 | `on_ground` false for a player standing still | `survival_e2e` | grounded = collision **or** solid block under the feet |
| 04 | `chat_command` id was 8 (jar: 7) | `packet_ids.rs` | corrected and locked |
| 04 | Respawn was unreachable behind the "dead cannot act" guard | `survival_e2e::death_and_respawn…` | respawn allowed while dead |
| 04 | Reach check accepted a 40-block-away dig | `survival_e2e::breaking_and_placing…` | 3-D distance to the block's nearest point |
| 04 | Chunk streaming ignored the per-tick budget on join | `survival_e2e::a_player_joins…` | budget lives on the tick report |
| 05 | **Stored terrain silently destroyed** — every streamed chunk became a dirty all-air placeholder that `save_all` wrote over the real file | Audit 02, pinned by `entity_lifecycle` | disk→world load path; placeholders are clean; a failed read never licenses an overwrite |
| 05 | **Remote DoS** — one out-of-range block placement returned `Err` from `tick()`, terminating the process | Audit 02 | `in_build_range` guard before every client-path `set_block` |
| 05 | `tick_food` could drive saturation negative and suppress regeneration | Audit 02 | floor at zero |
| 05 | `experience_needed_for_level` overflowed `i32` on a hostile persisted value | Audit 02 | saturating arithmetic |
| 05 | `move_with_collision` had no finiteness guard; a NaN made an entity permanently uncollidable | Audit 02 | guard at the solver + `non_finite_movement_is_refused_without_moving` |
| 05 | Chunks were never unloaded, so a long walk grew memory without bound | Audit 02 | view-distance unloading with a margin, never while dirty |
| 05 | `nextLong` treated Java's sign-extended halves as unsigned | JDK-25 oracle vectors | sign-extend both halves |
| 05 | The RNG rejection comparison was evaluated in `i64`, so it could never fire | the same | signed 32-bit comparison, matching Java |
| 05 | Items hovered ~0.97 blocks above the floor | `entity_lifecycle` landing test | `REST_EPSILON` + `blocked_down` |
| 05 | The author's own armour-permutation "fix" would have inverted a correct mapping | jar bytecode (`javap -c`) | reverted; the two index spaces are named constants |
| 06 | `insert` inverted `split`, destroying every item above a slot limit | conservation tests | the excess is `count - limit` |
| 06 | Cursor overflow merged against the cursor's limit (0 when empty), dropping the remainder | the same | one `return_to_cursor` helper takes the limit from the stack |
| 06 | A swap partner was searched in container 0 (the chest in a chest menu) | the swap test | `MenuLayout::player_container` |
| 06 | `SLOT_OUTSIDE` (−1) was rejected as out of range | the decoder test | outside-the-window clicks are legal |
| 06 | Smelting produced no output: `grow_capped` is a no-op on an empty stack | the furnace tests | construct the stack directly when the slot is empty |
| 06 | The shaped matcher matched a pattern inside an occupied grid | the crafting tests | Vanilla's exact-fill rule |
| 06 | Fuel was replaced a tick late, costing a dark tick and part-cooked progress | the furnace tests | one `take_fuel` per tick |
| 06 | **A budget stop dropped unprocessed updates**, so a long line never lit | budget-exhaustion tests | leave the remainder queued |
| 06 | **A disconnected source left its wire powered** | the propagation suite | decreases propagate |
| 06 | A dropped chunk packet was a permanent client-side hole | Audit 03 | `sent_chunks` updated only on a successful queue |
| 06 | The overflow disconnect could never fire | Audit 03 | ids ride on the `TickReport` |
| 06 | Damage ignored invulnerability and Resistance | Audit 03 | both wired into `damage_entity` |
| 06 | A borrowed-storage placeholder could overwrite terrain | Audit 03 | tracked and never saved |
| 06 | A failed save was forgotten | Audit 03 | dirty flags kept on failure |
| 06 | Regeneration ran at ~20× Vanilla | Audit 03 | 80-tick cadence |
| 07 | **The dispatcher checked a *token* count against the *argument* count**, refusing `say a b c` | `a_greedy_argument_takes_every_remaining_token` | `Command::token_bounds()` derives the true range |
| 07 | `command_tp` took `&self` while calling a mutating teleport | the compiler | `&mut self` |
| 07 | `/time set` would have been overwritten by the next broadcast | `a_time_command_changes_the_broadcast_time` | an offset rather than a value |
| 07 | **The tag registry split** — last-separator gave 103 spurious problems, first-separator left 46 | `vanilla_pack` (real data) | `TAG_REGISTRIES`: 20 measured registry paths, longest match first |
| 07 | The data-pack file counts included directory entries, so `recipe/` was 1 515 not 1 516 | `vanilla_pack` disagreeing with the document | corrected the measurement, not the code |
| 07 | The licence gate rejected `mc-command` | `cargo deny` | `publish = false` like every other member |
| 08 | Persistence bench printed `0.000 s` while passing — it measured nothing (`save_all_owned` on a borrowed game saves nothing) | P08-T11 first version | own the storage; assert `elapsed >= 1 µs` |
| 08 | The workload measured the collision solver (p95 101 ms), not tick overhead | P08-T10 first version | rotation/swing/hotbar driver + one periodic real move; both numbers kept |
| 08 | `bypassesPlayerLimit` comments said "not enforced" after P08-06 enforced it | re-reading every `bypass` mention | name the enforcement site (`Game::is_full_for`) |
| 08 | `PHASE-08-REPORT.md` cited sections that did not exist | the P08 verification cluster | wrote the missing sections; fixed the pointer |
| 09 | **Metrics-test flake (1 in 4 full runs)**: three tests shared a `TempDir` tag whose `pid+nanos` uniqueness collapsed under parallel I/O (probe: duplicate paths in 160 000 same-tag constructions), so one test's `level.dat` rename raced another's `remove_dir_all` | PHASE-08-REPORT §2.1 observation, root-caused in P09 with a probe | distinct tag per test; 10 green repeats + three full-workspace runs after the fix |
| 09 | `pi_profile` printed `build profile: dev` from a hardcoded string, so the first release run's log contradicted the document citing it | the P09 audit | derive from `cfg!(debug_assertions)`; release suite re-run after the fix |

## Known-false-assertion lessons

Three of the defects above passed their first version while proving the wrong
thing (the `0.000 s` bench, the 101 ms "tick overhead", the tautological
movement assertion of the bridge test) — each now carries the assertion that
would have caught it. Two more were documents more confident than their code
(the bypass comments, the cited-but-absent report sections). The rule these
taught, now in [CONVENTIONS.md §3.1](../CONVENTIONS.md): a check that cannot
fail is not evidence.
