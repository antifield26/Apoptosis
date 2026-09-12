# Test Matrix (living document — Phase 09 update)

Conventions: `L1` unit · `L2` property/fuzz · `L3` integration · `L4` golden/fixture ·
`L5` end-to-end · `L6` differential vs vanilla · `L7` regression-per-bug`.
Status: `planned | pass | fail | skipped(reason) | ignored(needs external input)`.
Totals: **1 191 passed, 0 failed, 21 ignored**, re-derived from **three**
`cargo test --workspace --no-fail-fast` runs on 2026-09-12 (74 suites; all
exit 0). The first two ran after the P09 TempDir-tag fix (`target/p09_full_test.log`,
`p09_full_test2.log`); the third after the registry fixture-search fix found by
the Pi acceptance run (`p09_full_test3.log`, +2 `mc-registry` ordering tests).
The 21 ignored = 7 differential suites (15 tests, env-gated) + `pi_profile` 4
+ `tick_baseline` 2 — all run on demand the same day (P09 rows below).

## Phase 07 — Commands, data packs and worldgen

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P07-T01 | Command tree validates its own structure | L1 | **pass** | `dispatch::tests::a_tree_refuses_structures_that_cannot_be_parsed`: duplicate name, empty name, duplicate argument, empty argument name, optional-before-required, greedy-not-last, inverted range, non-finite double range, arity limit — and exactly-at-the-limit accepted, so the boundary is where it claims to be |
| P07-T02 | Tokenizing honours quotes and escapes | L1 | **pass** | `argument::tests`: whitespace runs, quoted groups, `""` as a real empty token, backslash escapes, a trailing backslash kept literally |
| P07-T03 | **Unterminated quotes and empty input are refused** | L1 | **pass** | `unterminated_quotes_and_empty_input_are_refused` — a half-read command is refused, never silently truncated |
| P07-T04 | A command string is bounded before tokenizing | L1+L3 | **pass** | `an_over_long_command_is_refused_before_tokenizing` (at the limit accepted, past it `TooLong`); over a socket, `an_over_long_command_ends_only_that_connection` |
| P07-T05 | Multi-byte characters cannot panic a truncation | L1 | **pass** | `multi_byte_characters_are_counted_as_characters_not_bytes` (3 000 three-byte characters) and `a_long_found_value_is_truncated_on_a_character_boundary` |
| P07-T06 | Numeric arguments are bounded and never coerced | L1 | **pass** | `integers_are_bounded_and_a_fraction_is_refused`, `the_integer_range_is_checked_before_narrowing`, `doubles_refuse_non_finite_values` (`NaN`/`inf` spellings that `f64::parse` accepts) |
| P07-T07 | Block positions accept absolute and relative forms | L1 | **pass** | `block_positions_accept_absolute_and_relative_forms`; a bare `~` is distinct from `~0` |
| P07-T08 | **Greedy and `BlockPos` arguments consume the right number of tokens** | L1 | **pass** | `a_greedy_argument_takes_every_remaining_token`, `a_block_pos_argument_consumes_three_tokens` — the bug these caught made two of seven commands unusable |
| P07-T09 | Permission is checked before the grammar | L1+L3 | **pass** | `permission_is_checked_before_the_arguments_are_parsed` — a denied command must not leak its argument shape |
| P07-T10 | Unknown commands offer near matches | L1+L3 | **pass** | `an_unknown_command_reports_near_matches`, and over a socket `an_unknown_command_is_reported_rather_than_ignored` |
| P07-T11 | Suggestions are roots-only and permission-filtered | L1 | **pass** | `suggestions_are_roots_only_and_respect_permission`; argument completion is explicitly not claimed (P07-06) |
| P07-T12 | **Every declared command is reachable from a real client** | L5 | **pass** | `command_e2e::every_declared_command_is_reachable_from_a_client` — all seven, none disconnecting |
| P07-T13 | Malformed commands never disconnect | L5 | **pass** | `a_malformed_command_is_refused_without_disconnecting` — 11 hostile inputs including `\x00`, a lone quote and a 20-digit number |
| P07-T14 | A player cannot stop the server | L5 | **pass** | the same test asserts `!shutdown_requested()`, which is what a level-0 default is *for* |
| P07-T15 | `/time set` survives the per-second broadcast | L5 | **pass** | `a_time_command_changes_the_broadcast_time` — the offset must persist across 30 further ticks, which is the whole difficulty of the command |
| P07-T16 | `/tp` resolves absolute and relative, and refuses another target | L5 | **pass** | `teleport_moves_the_invoking_player`, `a_relative_teleport_resolves_against_the_player`, `teleporting_someone_else_is_refused_with_a_reason` |
| P07-T17 | Tags resolve transitively with cycle and depth guards | L1 | **pass** | `tag_resolve::tests` (12): transitivity, a cycle reported and terminated, a self-reference, a diamond dependency, depth beyond the limit, merge-vs-replace |
| P07-T18 | A missing tag reference is reported, optional ones distinguishably | L1 | **pass** | `a_missing_tag_reference_is_reported_not_dropped`, `an_optional_missing_tag_is_distinguishable_from_a_real_error` |
| P07-T19 | **The real vanilla data pack loads cleanly** | L6 | **pass (ignored, needs the jar)** | `vanilla_pack::the_real_vanilla_pack_loads_without_a_single_problem`: **758 of 758 tags, 0 problems**, 8 196 ids across 20 registries; 1 421 recipes loaded and 94 counted as unmodelled. This is the test that found the registry-split bug |
| P07-T20 | Pack discovery, ordering and metadata | L1 | **planned** | `pack.rs` is written; its test module is not |
| P07-T21 | Loot tables, advancements, functions, predicates | L1 | **not started** | P07-08/P07-10/P07-11 |
| P07-T22 | Data pack discovery from a world directory | L3 | **not started** | P07-12 |
| P07-T23 | Worldgen: seed pipeline, noise, biomes | L1+L3 | **pass** | mc-worldgen (62 tests) + worldgen_e2e (7): determinism across two games with one seed, different terrain for different seeds, bedrock floor, no all-air column, frequency-period guard |
| P07-T23b | Worldgen: structures | L1+L3 | **not started** | **No structures at all** — nothing from the jar's 1 202 structure/ files. Tree placement exists but is a feature, not a structure. P07-16 |
| P07-T24 | Selectors (`@a`, `@p`, `@s`, `@e`) | L1 | **not started** | P07-06 |
| P07-T25 | `execute` context: modifiers, conditions, nesting | L1+L5 | **pass** | `execute` parser (22 tests) + `execute_e2e` (9). Modifiers parse in written order and are applied in that order; every unsupported one is refused **by name**; nesting terminates at depth 16 with a message |
| P07-T25c | **Scripted multi-step scenario against the real pack** | L6 | **pass** | `scenario_vanilla`: 758 tags → 1 421 recipes → 156 furnace rows → terrain in 256 of 256 columns → a real `igloo/bottom` placed (180 blocks) → a marker surviving save/reopen. Falsification-verified: a no-op save loses the marker, and removing the generation gate breaks the borrowing test |
| P07-T25b | **`execute` tests can actually fail** | L7 | **pass** | Falsification-verified: disabling the `positioned` arm and inverting `unless` each make the corresponding test **FAIL**. The first version of both passed with the mechanism disabled — they asserted the client survived rather than what the reply said, and rewriting them to read the reply immediately exposed three commands missing their `execute` prefix |
| P07-T26 | `/function` execution: order, counts, nesting | L1+L5 | **pass** | `function_e2e` (13): commands run in file order, comments and blanks excluded from the count, a function calls another, and `execute` lines inside a function go through the chain parser |
| P07-T27 | Function recursion and size are bounded | L5 | **pass** | Self-recursion and **mutual** recursion between two files both report the depth bound; falsification-verified — removing the bound makes the test **stack-overflow** (0xC00000FD), so the bound is load-bearing rather than decorative |
| P07-T28 | A function cannot escalate privilege | L5 | **pass** | Falsification-verified: granting every player the console level makes the test **FAIL**. A function containing `stop` and `op` is refused for a level-0 invoker |
| P07-T29 | A malformed function does not remove the others | L1 | **pass** | An over-long file is skipped while the good one still loads and runs |
| P07-T30 | Redstone wired into the tick loop | L3 | **not started** | carried from P06 |

### Bugs found by these tests (L7)

| Bug | Found by | Fix |
|---|---|---|
| **The dispatcher checked a *token* count against the *argument* count**, so `say a b c` and `tp Alex 10 64 -5` were refused as "too many arguments" | `P07-T08` | `Command::token_bounds()` derives the true range: unbounded for a greedy tail, 3 per `BlockPos` |
| `command_tp` took `&self` while calling a mutating teleport | the compiler | `&mut self` — the alternative would have hidden the mutation behind a second path |
| `/time set` would have been overwritten by the next broadcast | `P07-T15` | an offset rather than a value |
| **The tag registry split** — last-separator gave 103 spurious problems, first-separator left 46 | `P07-T19` (real data) | `TAG_REGISTRIES`: 20 measured registry paths, longest match first, unknown prefixes reported |
| The data-pack file counts included directory entries, so `recipe/` was 1 515 not 1 516 | `P07-T19` disagreeing with the document | corrected the measurement, not the code |
| The licence gate rejected `mc-command` | `cargo deny` | it needed `publish = false` like every other member — the gate doing its job on a new crate |

## Phase 06 — Inventory, containers, block entities and redstone

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P06-T01 | **A click cannot create or destroy items** | L2+L3 | **pass** | `menu::tests::a_flood_of_hostile_clicks_cannot_create_or_destroy_items`: 2 000 pseudo-random clicks across every click type, slot and button, asserting conservation, non-negative counts and per-item limits **after every single click**, plus that zero clones happen in survival |
| P06-T02 | Every non-destructive click type conserves items | L1 | **pass** | `transfers_conserve_items`, `a_drag_conserves_items_across_all_three_types` (all three drag types), `pickup_all_gathers_only_matching_items_and_conserves`, `a_swap_conserves_items` |
| P06-T03 | Only throw and creative clone change the total | L1 | **pass** | `only_throw_and_creative_clone_change_the_total`, `creative_clone_is_the_only_way_to_create_items` |
| P06-T04 | A stale state id applies nothing and returns a resync | L1+L3 | **pass** | `a_stale_state_id_triggers_a_resync_and_applies_nothing`, `replaying_a_click_against_the_old_state_is_refused` (the anti-replay property), and over a socket: `a_stale_state_id_gets_a_full_resync_instead_of_moving_items` |
| P06-T05 | A computed slot refuses placement by click, swap and drag | L1 | **pass** | `a_computed_slot_refuses_placement`, `a_swap_cannot_place_into_a_computed_slot`, and the drag arm of the first |
| P06-T06 | **A real socket click moves items and is acknowledged** | L5 | **pass** | `container_e2e::a_container_click_over_the_socket_moves_items_and_is_acknowledged` — hand-encoded packet → decode → validate → apply → `container_set_slot` back |
| P06-T07 | A malformed click is ignored and never kicks | L3 | **pass** | `a_malformed_click_is_ignored_and_never_kicks` (unknown type, absurd slot, negative state id); the connection stays up and nothing moves |
| P06-T08 | A truncated payload ends that connection only | L3 | **pass** | `a_truncated_click_payload_ends_only_that_connection` — every strict prefix is sent, and a **fresh client logs in afterwards**, which is the security property |
| P06-T09 | A throw removes exactly one item and spawns an entity | L3 | **pass** | `a_throw_over_the_socket_removes_exactly_one_item` — the window total drops by one *and* the entity store grows by one, so it is a move rather than a deletion |
| P06-T10 | The player menu matches the jar-verified layout | L1 | **pass** | `the_player_menu_matches_the_jar_verified_layout` (46 slots, 0 result, 1..=4 grid, 5..=8 armour boots-first, 9..=35 main, 36..=44 hotbar, 45 offhand) |
| P06-T11 | A menu built on a bad mapping is refused | L1 | **pass** | `a_menu_rejects_a_mapping_that_points_nowhere` (missing container, slot past the end, impossible ceiling, no slots) |
| P06-T12 | Slot limits and per-item limits are enforced | L1 | **pass** | `a_stack_limit_is_enforced_on_the_cursor_and_in_slots` (asserts the bucket really is a 16-stack first), `placing_more_than_fits_leaves_the_overflow_on_the_cursor`, `a_slot_ceiling_below_the_item_limit_is_respected` |
| P06-T13 | Shaped matching: offset, exact fill, mirroring, shape rejection | L1 | **pass** | crafting tests incl. `a_shaped_recipe_requires_the_grid_to_hold_nothing_else` with its non-vacuous converse |
| P06-T14 | `craft` consumes one of each ingredient and leaves the rest | L1 | **pass** | `crafting_leaves_the_rest_of_the_grid_intact`, `crafted_items_are_consumed_from_a_container_backed_grid` |
| P06-T15 | Furnace: exact burn, exact cook, blocked output wastes no fuel | L1 | **pass** | `a_fuel_item_lasts_exactly_its_documented_number_of_ticks` (measured: 1 stick = 100 lit ticks, 1 coal = 1600), `cooking_takes_exactly_cook_ticks_and_finishes_once`, `running_out_of_fuel_mid_cook_preserves_progress` |
| P06-T16 | Hopper transfer conserves items adversarially | L1+L2 | **pass** | `hopper::tests` incl. a randomised loop asserting the total is invariant, plus computed-slot and pickup-refusal rules |
| P06-T17 | Block entities are typed, ordered and findable when leaked | L1 | **pass** | `block_entity::tests` (10): payload/kind agreement, ascending iteration, displacement reported, `audit_against`/`prune` |
| P06-T18 | **A block change retires its block entity** | L3 | **pass** | `block_entity_e2e::breaking_the_block_retires_its_entity`, `an_entity_whose_block_still_exists_is_not_retired`, `retiring_an_entity_with_contents_is_reported_not_silent` |
| P06-T19 | Redstone power model: bounds, attenuation, weak vs strong | L1 | **pass** | `power_model.rs` (8) incl. the refusal of a level above 15 |
| P06-T20 | **A budgeted run reaches the same state as an unbounded one** | L2 | **pass** | `budget_exhaustion.rs` (7) — the property that caught the dropped-update bug |
| P06-T21 | Propagation is deterministic across the whole change vector | L2 | **pass** | `determinism.rs` (6) compares the full `Vec` of changes between two runs, not just the final state |
| P06-T22 | Golden circuits with hand-written expected power | L4 | **pass** | `golden_circuits.rs` (5) |
| P06-T23 | **A disconnected source zeroes its wire** (and down a chain) | L1+L7 | **pass** | the propagation suite's decrease case — the classic "redstone stays on" defect, caught after the first implementation only propagated increases |
| P06-T24 | Redstone differential test against a real Vanilla server | L6 | **not satisfied** | Needs the P08 harness. Recorded, not claimed |
| P06-T25 | A chest broken with contents drops them | L3 | **not satisfied** | The retirement is reported (report counter + log naming the count) but the items are **lost**. The most serious gap in the phase; recorded in `PHASE-06-REPORT.md` §5.4 |
| P06-T26 | Block entities persist across a restart | L3 | **not satisfied** | No payload↔NBT conversion exists |
| P06-T27 | A hopper moves items on its own schedule | L3 | **not satisfied** | The transfer is pure; nothing ticks it |
| P06-T28 | Redstone is driven by the tick loop | L3 | **not satisfied** | The queue/propagation are not wired into `Game` |

### Bugs found by these tests (L7)

| Bug | Found by | Fix |
|---|---|---|
| `insert` inverted `split`, destroying every item above a slot limit | conservation tests | the excess is `count - limit`, not `limit` |
| The cursor overflow merged against the *cursor's* limit (0 when empty), dropping the remainder | the same | one `return_to_cursor` helper takes the limit from the stack |
| A swap partner was searched in container 0 (the chest in a chest menu) | the swap test | `MenuLayout::player_container` |
| `SLOT_OUTSIDE` (−1) was rejected as out of range | the decoder test | outside-the-window clicks are legal |
| Smelting produced no output: `grow_capped` is a no-op on an empty stack | the furnace tests | construct the stack directly when the slot is empty |
| The shaped matcher matched a pattern inside an occupied grid | the crafting tests | Vanilla's exact-fill rule |
| Fuel was replaced a tick late, costing a dark tick and part-cooked progress | the furnace tests | one `take_fuel` per tick |
| **A budget stop dropped unprocessed updates**, so a long line never lit | budget-exhaustion tests | leave the remainder queued |
| **A disconnected source left its wire powered** | the propagation suite | decreases propagate |
| A dropped chunk packet was a permanent client-side hole (Audit 03) | adversarial audit | `sent_chunks` updated only on a successful queue |
| The overflow disconnect could never fire (Audit 03) | adversarial audit | ids ride on the `TickReport` |
| Damage ignored invulnerability and Resistance (Audit 03) | adversarial audit | both wired into `damage_entity` |
| A borrowed-storage placeholder could overwrite terrain (Audit 03) | adversarial audit | tracked and never saved |
| A failed save was forgotten (Audit 03) | adversarial audit | dirty flags kept on failure |
| Regeneration ran at ~20× Vanilla (Audit 03) | adversarial audit | 80-tick cadence |

## Phase 05 — Simulation, entities, physics and AI

Per-crate after Phase 05: **simulation 27** (new) · entity **127** + 4 doc · world 31 + 4 ·
server 12 + 6 + **9** + 4 + 7 · network 15 + **2** · protocol 101 + 4 + 4 ·
persistence 71 + 9 + 16 + 7 · registry 12 · core 10 · nbt 16 · test-support 4.

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P05-T01 | The six-phase tick order is a compile-time contract and is honoured | L1+L3 | **pass** | `phase::tests`: order equals the documented list, indices match positions, every variant appears once, network first / broadcast last. `entity_lifecycle::the_six_phases_run_and_the_metrics_show_their_cost` shows the phase means are populated after real work |
| P05-T02 | A failing phase aborts the tick but is still measured | L1 | **pass** | `scheduler::tests::a_failing_phase_aborts_the_tick_but_is_still_measured` — later phases do not run, the half-run tick still counts, `last_outcome` reports the failing phase |
| P05-T03 | Tick metrics: percentiles, overruns, bounded window, busiest phase | L1 | **pass** | `metrics::tests` (6 cases) incl. the ring staying bounded over 3×WINDOW ticks and an exactly-at-budget tick not counting as an overrun |
| P05-T04 | **`RandomSource` reproduces `java.util.Random` byte for byte** | L4+L6 | **pass** | `random::tests::matches_the_jdk_for_known_seeds` + `bounded_draws_match_the_jdk_including_the_power_of_two_branch`, vectors measured from **JDK 25** (`target/vanilla-26.1.2/RandomProbe.java`). Caught two real bugs: `nextLong` must sign-extend each half, and the rejection comparison must be signed 32-bit |
| P05-T05 | Same seed replays the same goals and the same tick reports | L2+L3 | **pass** | `mob::the_same_seed_replays_the_same_goals`; `entity_lifecycle::the_same_seed_replays_the_same_tick_reports` compares the whole `TickReport` vector, player position, entity count and RNG state across two runs of the same script |
| P05-T06 | Entity ids are positive, ascending, **never reused**, and capped | L1 | **pass** | `entity::tests::ids_are_positive_ascending_and_never_reused` (remove then spawn yields a higher id), `entity_ids_reject_non_positive_values`, `spawning_respects_the_cap` |
| P05-T07 | Iteration and spatial queries are ascending-id and reject hostile arguments | L1 | **pass** | `entity::tests::iteration_is_ascending_by_id`, `radius_queries_reject_hostile_arguments` (negative, NaN, infinite radius), `kind_filtering_and_lookups` |
| P05-T08 | Entity timers saturate; effects expire; velocity impulses are clamped | L1 | **pass** | `entity::tests::timers_tick_down_and_effects_expire`, `velocity_impulses_are_clamped` (a million-block impulse clamps, and 100 further impulses cannot accumulate past it) |
| P05-T09 | Entity removal is batched, stable and idempotent | L1 | **pass** | `entity::tests::removal_sweep_is_batched_and_stable` |
| P05-T10 | Effect modifiers follow the documented shapes and cannot go absurd | L1 | **pass** | `effect::tests` (6): speed/slowness multipliers, resistance capping at full immunity, poison cadence `25 >> amplifier`, only wither may kill |
| P05-T11 | Item physics settles instead of oscillating; hostile velocity is sanitised | L1 | **pass** | `item_entity::tests`: gravity, drag, a 120-tick settle with no reversal, non-finite and absurd velocity handling |
| P05-T12 | Pickup delay, despawn at the limit, merge remainder, merge refuses bad caps | L1 | **pass** | `item_entity::tests` (13) incl. `a_non_positive_max_stack_is_handled_without_panicking_or_looping` (0, −1, `i32::MIN`, `i32::MAX`) |
| P05-T13 | Projectile trajectory, lifetime and hostile input | L1 | **pass** | `projectile::tests` (10) |
| P05-T14 | Mob table is complete and sane; attack styles are honest | L1 | **pass** | `mob::tests::the_kind_table_is_complete_and_sane`, `hostile_kinds_attack_and_passive_kinds_do_not`, `behaviours_expose_exactly_their_goal_sets`; `MobAttackStyle::is_implemented()` lets a caller refuse an unmodelled attacker |
| P05-T15 | AI goals: aggro radius, retention, attack, flee, walk boundaries | L1 | **pass** | `mob::tests` (14) incl. retention and the walk-boundary case — both of which the author diagnosed as **test** bugs and fixed by asserting the transition rather than loosening the assertion |
| P05-T16 | Pathfinding: steps, falls, walls, bounds, determinism, clearance | L1 | **pass** | `pathfind::tests` (13) incl. `a_goal_ten_thousand_blocks_away_is_refused_without_reading_a_block` (asserts **0** block reads), `the_same_query_returns_the_same_path_twice`, `paths_never_enter_solid_blocks_in_a_maze` |
| P05-T17 | A joined player has an entity that tracks it; leaving reaps it | L3 | **pass** | `entity_lifecycle::a_joined_player_has_an_entity_that_tracks_it`, `leaving_marks_the_entity_removed_and_a_tick_reaps_it` |
| P05-T18 | Dropping the held item spawns a real item entity | L3 | **pass** | `entity_lifecycle::dropping_the_held_item_spawns_an_item_entity`, `spawn_item_creates_an_item_entity_and_refuses_an_empty_stack`. Closes the Phase 04 gap where drops were discarded |
| P05-T19 | An entity falls under gravity and lands on solid ground | L3 | **pass** | `entity_lifecycle::an_entity_falls_under_gravity_and_lands_on_a_solid_floor` |
| P05-T20 | **A stored chunk loads from disk and is never overwritten by a placeholder** | L3+L7 | **pass** | `entity_lifecycle::a_stored_chunk_is_loaded_from_disk_and_never_overwritten_by_a_placeholder` — three phases (write → play+edit+save → reopen from disk), asserting the stored block survives, the real edit persists, and a placeholder-only neighbour is **absent** from disk. **Proven load-bearing**: with the disk read short-circuited the test fails `left: 0, right: 5309` |
| P05-T21 | Chunks outside the view unload and re-stream on return | L3 | **pass** | `entity_lifecycle::chunks_outside_the_view_are_unloaded_and_re_streamed_on_return` |
| P05-T22 | Entity-heavy tick cost is measured and bounded | L4 (perf) | **pass (ignored, on demand)** | `tick_baseline::entity_heavy_ticks_within_the_frame_budget`: 10 players + 600 mobs + 400 items → p50/p95/p99 = 16.75/19.23/21.02 ms, max 25.33 ms, 1 000 entity-ticks/tick |
| P05-T23 | Non-finite movement is refused without moving the box | L1+L7 | **pass** | `world::tests::non_finite_movement_is_refused_without_moving` (NaN/±Inf delta and a NaN box) |
| P05-T24 | Login-phase tolerance and shutdown drain are actually covered | L3 | **pass** | `network/tests/login_tolerance.rs` — Audit 02 found Audit 01 cited tests that never sent these packets; this closes the evidence gap |
| P05-T25 | A movement intent's round trip is falsifiable, not tautological | L3 | **pass** | `network_game_bridge` now asserts the exact requested position **or** an observed correction packet, replacing an assertion that was always true |
| P05-T26 | Mobs spawn and are visible to clients | L3 | **not satisfied** | Nothing spawns mobs (no spawn rule — P05-11 partial) and no entity packet is sent (P05-15). Recorded, not claimed |
| P05-T27 | Entities survive a restart | L3 | **not satisfied** | No entity persistence (P05-16). Dropped items and mobs vanish on restart |
| P05-T28 | Scheduled block/fluid ticks fire | L1+L3 | **not satisfied** | `TickPhase::ScheduledTicks` is a documented no-op (P05-10). Carried to P06 |

### Bugs found by these tests (L7)

| Bug | Found by | Fix |
|---|---|---|
| **Stored terrain silently destroyed** — every streamed chunk became a dirty all-air placeholder that `save_all` wrote over the real file | Audit 02's adversarial review, pinned by P05-T20 | disk→world load path; placeholders are clean, and a failed read never licenses an overwrite |
| **Remote DoS** — one out-of-range block placement returned `Err` from `tick()`, terminating the process | Audit 02 | `in_build_range` guard before every client-path `set_block`; intent paths no longer `?`-propagate |
| `tick_food` could drive saturation negative (reachable every tick) and suppress regeneration | Audit 02 | floor at zero |
| `experience_needed_for_level` overflowed `i32` on a hostile persisted `XpLevel`, panicking debug builds | Audit 02 | saturating arithmetic |
| `move_with_collision` had no finiteness guard; a NaN could make an entity permanently uncollidable | Audit 02 | guard at the solver + P05-T23 |
| Chunks were never unloaded, so a long walk grew memory without bound | Audit 02 | view-distance unloading with a margin, never while dirty |
| `nextLong` treated Java's sign-extended halves as unsigned | P05-T04 (JDK oracle) | sign-extend both halves |
| The RNG rejection comparison was evaluated in `i64`, so it could never be negative and never fired | P05-T04 | signed 32-bit comparison, matching Java |
| Items hovered ~0.97 blocks above the floor (the grounded test was also true inside the air block above it) | P05-T19 | `REST_EPSILON` + `blocked_down` |
| **The author's own** armour-permutation "fix" would have inverted a correct mapping | jar bytecode (`javap -c`), not a test | reverted; `ARMOR_MENU_START`/`OFFHAND_MENU_SLOT` name the two index spaces and the test derives the mapping from them |

## Phase 04 — Survival vertical slice

Per-crate: registry 12 · world 30 unit + 4 vanilla-chunk · entity 59 (+3 doc) ·
server 12 unit + 6 protocol E2E + **7 survival E2E** + **4 network↔game bridge** ·
protocol 101 + 4 fixture + **4 packet-id conformance**.

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P04-T01 | Registry ids match the official jar's own registry | L4+L6 | **pass** | `mc-registry` boots `Block.BLOCK_STATE_REGISTRY` from the 26.1.2 jar: 1 168 blocks / 29 873 states / 1 506 items. `every_state_round_trips_through_the_table` walks all 29 873; sample ids asserted against the verbose dump |
| P04-T02 | Packet ids match the jar (incl. the `chat_command` regression) | L4+L6 | **pass** | `protocol/tests/packet_ids.rs` (4 tests) vs `docs/protocol/packet-ids-775.tsv`, extracted from registration bytecode |
| P04-T03 | Chunk wire format: section blob, palette forms, light, heightmaps | L1+L4 | **pass** | 29 codec tests incl. golden bytes for `block_update`, both palette forms and `chunk_data`; corrections locked by `chunk_data_has_no_section_count_prefix`, `biome_minimum_bit_width_is_two_on_the_network`, `heightmap_ids_match_the_extracted_table` |
| P04-T04 | Real vanilla chunk → runtime → disk is lossless | L4+L6 | **pass** | `world/tests/vanilla_chunk.rs`: 4 tests over the Phase-03 fixture — every column has terrain, bedrock at y=-64, section counts recount, round trip preserves all 4096×24 block ids |
| P04-T05 | Player can stand on and walk across real vanilla terrain | L4 | **pass** | `a_vanilla_chunk_can_be_placed_in_a_world_and_walked_on`: drops a player 20 blocks onto the fixture chunk's surface and lands exactly on it |
| P04-T06 | An unknown block name is refused, never substituted with air | L1 | **pass** | `an_unknown_block_name_is_refused_rather_than_becoming_air` — error names both the block and the slot |
| P04-T07 | Collision: landing, walls, sliding, headroom, no tunnelling | L1 | **pass** | `world::tests` (5 cases) incl. a 10-block fall onto a 1-block gap and a jump-onto-ledge; swept-box resolution |
| P04-T08 | Ray casting for block targeting (faces, distance, misses, hostile input) | L1 | **pass** | `ray::tests` (12 cases): cardinal face ids, first-block-wins, zero-distance start, unloaded-position stop, NaN/zero/infinite rejection |
| P04-T09 | Item stack arithmetic and per-item limits | L1 | **pass** | `mc-entity` stack tests: merge/split/shrink boundaries, 64/16/1 caps, hostile counts rejected |
| P04-T10 | Inventory bounds, hotbar rejection, container permutation | L1+L3 | **pass** | `inventory::tests` incl. `the_container_permutation_is_the_documented_one`, plus `survival_e2e::a_hostile_hotbar_index_is_rejected` (-5 and 9 refused, 4 applied) |
| P04-T11 | Health/food/XP rules incl. hostile NBT values | L1 | **pass** | `player::tests` (28): three XP formula branches, saturation-before-food, starvation at 0, `XpP = 42` repaired |
| P04-T12 | Player NBT round trip + unknown-entry preservation | L1+L4 | **pass** | `player::tests`: full/minimal round trips, unknown top-level entries survive, unknown item name is `CorruptData`, truncated NBT does not panic |
| P04-T13 | Join streams exactly (2r+1)² chunks within the per-tick budget | L3 | **pass** | `survival_e2e::a_player_joins_and_receives_terrain_and_vitals` asserts the total equals the view area and no tick exceeds `CHUNKS_PER_TICK` |
| P04-T14 | Movement is server-authoritative: teleport caps, NaN/Inf, wall stop | L3 | **pass** | `movement_is_clamped_by_collision_and_hostile_values_are_refused`, `a_wall_stops_a_player_and_a_fall_lands_on_the_floor` (500-block teleport refused, wall stops the player, fall lands and reports `on_ground`) |
| P04-T15 | Break/place validation: reach, occupied target, player hitbox, item consumption | L3 | **pass** | `breaking_and_placing_blocks_is_validated_and_broadcast` — in-reach dig applied + broadcast, 40-block dig refused, placement consumes an item **iff** it succeeded, placement into occupied space refused |
| P04-T16 | Death and respawn | L3 | **pass** | `death_and_respawn_restore_the_player`: dead players cannot move, `client_command` respawns, `respawn` packet sent, vitals and position restored |
| P04-T17 | World survives save → close → reopen | L3+L5 | **pass** | `the_world_survives_a_save_and_reload`: blocks placed through gameplay are read back after a restart via the disk→runtime conversion |
| P04-T18 | **A real socket login becomes a game player** | L5 | **pass** | `network_game_bridge::a_real_socket_login_becomes_a_game_player` — full handshake/login/config/play over TCP, then the game loop sees the player and has loaded their chunk |
| P04-T19 | Terrain reaches a socket-connected client | L5 | **pass** | `network_game_bridge::the_client_receives_terrain_over_the_socket` reads `level_chunk_with_light` off the wire |
| P04-T20 | A socket client's raw movement bytes reach the simulation | L5 | **pass** | `a_movement_intent_from_the_socket_is_applied` encodes `move_player_pos` by hand, so the whole decode path is exercised |
| P04-T21 | Disconnect removes the player | L5 | **pass** | `a_client_disconnect_removes_the_player` |
| P04-T22 | Tick cost is measured and bounded | L4 (perf) | **pass (ignored, on demand)** | `tick_baseline`: 10 players @ view 8 → p50/p95/p99 = 0.78/1.42/1.65 ms, max 2.22 ms; asserts a generous ceiling to catch order-of-magnitude regressions |
| P04-T23 | Real 26.1.2 client survival session | L5 | **blocked** | No client in this environment (P02-T12); the exit gate's "real client" clause is reported as qualified, not met |

### Bugs found by these tests (L7)

| Bug | Found by | Fix |
|---|---|---|
| Endpoint-only collision allowed tunnelling through a floor | `world::tests::falling_stops_on_the_floor` | swept-box per-axis resolution |
| `clip_axis` reconstructed its base box wrongly, so every horizontal move passed | the same test | takes the pre-step box; explicit full-step shortcut |
| Heightmap packing shifted 9 bits past a `long` boundary | `world/tests/vanilla_chunk.rs` | reuse the verified shared packer |
| `on_ground` false for a player standing still | `survival_e2e` wall/fall test | grounded = collision **or** solid block under the feet |
| `chat_command` id was 8 (jar: 7) | `protocol/tests/packet_ids.rs` | corrected and locked |
| Respawn was unreachable behind the "dead cannot act" guard | `survival_e2e::death_and_respawn...` | respawn is allowed while dead |
| Reach check accepted a 40-block-away dig | `survival_e2e::breaking_and_placing...` | 3-D distance to the block's nearest point |
| Chunk streaming ignored the per-tick budget on join | `survival_e2e::a_player_joins...` | budget lives on the tick report |

## Audit 01 (2026-09-11) — re-verification round

Full findings: `docs/phases/AUDIT-01-FINDINGS.md`.

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| A01-T01 | Every packet id this server uses matches the official 26.1.2 jar | L4+L6 | **pass** | `crates/protocol/tests/packet_ids.rs` (4 tests) against `docs/protocol/packet-ids-775.tsv`, machine-extracted from the jar's registration bytecode: 256 ids over 5 states; asserts contiguity per direction and every constant used |
| A01-T02 | The `chat_command` id regression stays fixed (was 8, jar says 7) | L7 | **pass** | `packet_ids.rs::the_regression_that_motivated_this_test_stays_fixed`; also pinned in `ids.rs` unit tests |
| A01-T03 | Full signed-chat payload is consumed and validated, not truncated | L1+L2 | **pass** | `play.rs::chat_decodes_the_full_signed_payload`, `chat_rejects_a_hostile_last_seen_count` (absurd count ⇒ error, no loop), `chat_command_allows_vanilla_length_commands` |
| A01-T04 | Interaction intents decode (swing / use_item_on / set_carried_item / container_close) incl. truncation | L1+L2 | **pass** | `play.rs::interaction_intents_decode` |
| A01-T05 | Login phase tolerates `custom_query_answer` / `cookie_response` instead of kicking | L3 | **pass** | `network/tests/login_tolerance.rs`: sends both packets before the ack and asserts the login still completes |
| A01-T06 | Connection gate cannot panic the process (poison recovery) | L1 | **pass** | `limits.rs::lock_state` replaces four `expect`s; covered by the existing gate tests |
| A01-T07 | Shutdown drains live connections before the world is closed | L3 | **pass** | `network/tests/login_tolerance.rs::shutdown_drains_a_live_connection`: a connected, logged-in client is drained rather than abandoned |
| A01-T08 | Per-crate builds do not rely on workspace feature unification | L1 | **pass** | `cargo build -p mc-server` and `-p mc-server-app` succeed standalone (tokio features declared per crate) |
| A01-T09 | Dependency/licence gate exists and is enforceable | — | **pass (config)** | `deny.toml` + `dependency-policy` CI job + `docs/operations/DEPENDENCY-POLICY.md`; written but **never executed**: the repository has no remote, so the workflow has no host (Audit 05) |
| A01-T10 | Reference-repo SHA gap recorded as unmet, not glossed | — | **pass (doc)** | `reference-repos.md` header + ADR R-01; still requires a re-clone to close |

## P02 — Protocol (executed)

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P02-T01 | VarInt/VarLong hostile (non-terminating, overflow, overlong) | L1+L2 | **pass** | `mc-protocol/src/varint.rs` tests: known encodings, 5/10-byte caps, truncated/overlong rejection, 10k seeded random corpus |
| P02-T02 | Frame read/write + 2 MiB cap + truncation | L1+L2 | **pass** | `mc-protocol/src/framing.rs`: split feeds, multi-packet feeds, oversize/negative/zero frames, 5k random corpus |
| P02-T03 | Handshake parse + intent 1/2/3 incl. bad next_state | L1+L4 | **pass** | `packets/handshake.rs` + golden `fixtures/protocol/handshake_login.hex` (protocol 775, "localhost", 25565, intent 2) |
| P02-T04 | Status request/response JSON + ping round-trip + close | L3+L5 | **pass** | `mc-server/tests/e2e_login_play.rs::status_ping_reports_protocol_and_motd` against a live listener |
| P02-T05 | Offline login → LoginSuccess → KnownPacks → RegistryData → FinishConfig → JoinGame (test client) | L3+L5 | **pass** | `offline_login_reaches_play_with_registry_payload` + `offline_uuid_matches_derivation_rule` (vector `b50ad385-…`) |
| P02-T06 | Online-mode provider boundary (fails fast, offline default) | L1+L3 | **pass** | `mc-network/src/auth.rs` tests + `online_mode_refuses_to_start_without_provider` |
| P02-T07 | Compression negotiation + decompression-bomb rejection | L1+L2+L3 | **pass** | `framing.rs`: compressed round trip, below-threshold rejection, bomb rejection; E2E login runs with threshold 256 |
| P02-T08 | Disconnect routing (login vs play kick) + wrong protocol | L3 | **pass** | `wrong_protocol_is_kicked_with_disconnect` (LoginDisconnect JSON); config/play NBT disconnect encoders round-trip; keepalive timeout kick verified in `mc-network/tests/keepalive.rs` |
| P02-T09 | Golden bytes for frame + network NBT | L4 | **pass** | `mc-protocol/tests/fixtures.rs` + `frame_uncompressed.hex`, `nbt_literal_text.hex` |
| P02-T10 | Socket-level hostile fuzz: process survives garbage bursts | L2+L5 | **pass** | `malformed_input_drops_connection_but_not_process` (non-terminating VarInt + 12 seeded random-byte connections, then a clean login) |
| P02-T11 | Connection admission limits (global/per-IP/burst) | L1 | **pass** | `mc-network/src/limits.rs`: global budget, per-IP concurrency, token-bucket burst + refill, drop accounting |
| P02-T12 | Real 26.1.2 client acceptance (login/config/play) | L5 | **blocked** | No real client in this environment (`PHASE-02-REPORT` limitations; registry NBT provisional); protocol test client is the current partner |

## P03 — Persistence (executed)

Per-crate totals: `mc-nbt` 16 unit + 1 doctest · `mc-persistence` 71 unit +
9 fixture + 16 corruption + 7 restart (+2 ignored differential) · `mc-server`
12 unit (3 new world-storage/lifecycle cases).

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P03-T01 | NBT round-trip for all 12 tag types, disk **and** network encodings, incl. modified-UTF-8 limits | L1+L2+L4 | **pass** | `mc-nbt::write` tests (every tag type, both encodings, empty list, `DataOutput.writeUTF` byte vectors, over-long string refusal), `mc-protocol/src/nbt.rs` façade tests |
| P03-T02 | Region header/sector edge cases (short header, zero slot, sector<2, zero count, past-EOF, len>allocation, short final sector) | L1+L2+L3 | **pass** | `mc-persistence/src/region.rs` unit tests + `tests/corruption.rs` (17 cases) |
| P03-T03 | Chunk serde field superset incl. legacy difficulty/spawn fallback and unknown-field preservation | L1+L4 | **pass** | `mc-persistence/src/{chunk,level}.rs` tests; real vanilla chunk with palettes/light/heightmaps/ticks/entities in `tests/anvil_fixture.rs` |
| P03-T04 | Dual-layout load (26.1 `dimensions/<ns>/<v>/region` + legacy root `region/`, `DIM-1`, `DIM1`) | L1+L3 | **pass** | `dimension.rs` layout tests; `WorldStorage::layout_for` prefers modern and warns on legacy |
| P03-T05 | Restart preservation (save→drop→reopen→compare semantic state) | L3 | **pass** | `tests/restart.rs`: 4 chunks across 2 dimensions, 3 region files, level.dat + backup, handle-cache eviction, autosave-driven flush |
| P03-T06 | Corruption safety (bit-flip → error, no panic, no partial publish, healthy neighbour survives) | L2+L3 | **pass** | `tests/corruption.rs` (17 cases) incl. bit flip, truncated payload/header, unknown codec, `.mcc`, corrupt `level.dat`, failing write staying queued |
| P03-T07 | Vanilla 26.1.2 world load **and** write-back accepted by vanilla | L6 | **pass (ignored, run 2026-09-11)** | `tests/vanilla_differential.rs`: 529 vanilla chunks decoded, rewritten, re-read identically; vanilla booted on our rewritten world with our `level.dat`, preserved a `diamond_block` marker we wrote into the spawn chunk, reported no chunk/level error, re-saved 529 chunks that we then re-read |
| P03-T08 | Packing fidelity: unpack vanilla palettes and repack byte-identically | L4 | **pass** | `tests/anvil_fixture.rs::vanilla_palette_packing_round_trips_byte_identically` over 11 packed palettes (up to 18 block states / 5 bits) |
| P03-T09 | Atomic save semantics (tmp→rename, `level.dat_old`, no partial file on failure) | L1+L3 | **pass** | `save.rs` unit tests (`write_atomic`, backup rotation, blocked staging path) + `restart.rs::level_dat_backup_holds_the_previous_version` |
| P03-T10 | Dirty tracking snapshot/restore + deterministic region grouping | L1 | **pass** | `dirty.rs` tests: snapshot clears, restore after failure, mark-order independence |
| P03-T11 | Autosave scheduling determinism (interval, single fire per interval, overrun burst suppression, disable) | L1+L3 | **pass** | `autosave.rs` tests + `restart.rs::queued_chunks_are_written_by_the_autosave_scheduler` (tick 100 fires once; 0 disables) |
| P03-T12 | Byte-level corruption of the NBT payload inside a valid frame is detected | L2 | **pass** | `corruption.rs::a_bit_flip_inside_the_nbt_is_detected_downstream` (zlib or NBT layer must reject; never a half-decoded chunk) |
| P03-T13 | Binary end-to-end: server creates a 26.1.2-shaped `level.dat` at startup and closes the world on shutdown | L3+L5 | **pass** | `mc-server/src/storage.rs` tests + `lifecycle.rs::world_is_created_at_startup_and_saved_at_shutdown`; process smoke run created `level.dat` (DataVersion 4790, `version` 19133) |

### Bugs found by these tests (L7)

| Bug | Found by | Fix / regression |
|---|---|---|
| `PalettedContainer::set` widened the palette without re-packing existing values, mixing two bit widths in one array | `tests/restart.rs` (`paletted container index 25 is outside a palette of 17`) | track the packed width, repack on change; covered by `chunk.rs` unit tests and every restart scenario |
| A failed chunk write was removed from the dirty set, so the next flush never retried it | `tests/corruption.rs::writes_to_a_read_only_directory_fail_cleanly_and_stay_queued` | failed chunks are re-marked dirty and stay queued |
| Region reader rejected a **short final sector**, which a real vanilla world can contain after an interrupted save | `tests/vanilla_differential.rs` (real `r.-1.-1.mca`) | payload bounded by `min(allocated, present)` like vanilla; regression test `a_short_final_sector_is_tolerated_like_vanilla` |
| `cargo build -p mc-server` failed on Windows (`tokio::signal` used without the `signal` feature; workspace feature unification hid it) | process smoke run | feature declared in `crates/server/Cargo.toml` |

## Phase 08 — Pi/Ops (executed, dev host)

Per-suite deltas over the Phase 07 figure: `mc-server` lib 39→44 (+metrics 3,
+shutdown-barrier 1, +flood-adjacent staging 1); `command_e2e` 10→11 (+flood);
`ops_e2e` 6→7 (+full-server bypass); `pi_profile` new file, 4 ignored
on-demand (workload/chunkgen/persistence/profile); `tick_baseline` gains
`print_phase_means` lines inside the existing 2 ignored tests.

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P08-T01 | Config guardrails reject hostile values, accept the documented example | L1 | **pass** | `config.rs` 5 tests: defaults/parse/unknown-fields/hostile-values/compression+motd |
| P08-T02 | Operational snapshot reports scheduler truth without moving the game | L1 | **pass** | `metrics.rs` 3 tests: fresh-zeroes, 5-tick count agreement + p50≤p95≤p99 ordering, read-only probe |
| P08-T03 | Shutdown passes through Stopping with a bounded final save | L1+L3 | **pass** | `lifecycle.rs::shutdown_passes_through_stopping_before_stopped`, `world_is_created_at_startup_and_saved_at_shutdown`; barrier Running→Stopping→drain 5 s→save 30 s→Stopped |
| P08-T04 | systemd unit reviewed line-by-line, never applied here | — | **pass (review)** | `deploy/mc-server.service` + `RUNBOOK.md` §1 table; no Pi in this environment, said out loud |
| P08-T05 | Backup → verify → restore round-trips; overwrite guard holds | L1 | **pass** | `backup.rs` 5 tests: round-trip, overwrite refusal, existing-backup refusal, missing-file report, missing-world error |
| P08-T06 | Full server refuses one more join; bypass operator still joins | L3 | **pass** | `ops_e2e::a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator` probes all three outcomes; gate named `8/4/250 ms` with arithmetic test |
| P08-T07 | Slow drip bounded; registry reservation capped; location word last | L1+L2 | **pass** | `framing.rs::a_slow_drip_never_grows_the_buffer_without_bound`, `config.rs::registry_data_never_reserves_more_than_the_cap` + hostile-count, `region.rs::the_location_word_is_written_last` (closes Audit 05 downgrade) |
| P08-T08 | 300-command flood answered over ticks; sender survives | L5 | **pass** | `command_e2e::a_command_flood_from_one_client_does_not_starve_the_tick` (256/tick drain bound) |
| P08-T09 | 10-player settled workload under the generous ceiling | L4 (perf) | **pass (ignored, on demand)** | `pi_profile::ten_player_survival_workload`: settled p50/p95/p99 0.65/0.80/0.88 ms, max 0.92 ms, 200 ticks, 0.14 s wall (dev host, debug, driven loop — not a 20 TPS claim) |
| P08-T10 | Chunk-generation burst streams fresh chunks | L4 (perf) | **pass (ignored, on demand)** | `pi_profile::chunk_generation_burst_measures_fresh_chunks`: 684 resident, 1 360 streamed, 10.8 s wall |
| P08-T11 | Dirty save timed per chunk; flags cleared | L4 (perf) | **pass (ignored, on demand)** | `pi_profile::persistence_bench_measures_dirty_save`: 81 dirty in 5.2 s (≈64 ms/chunk debug) |
| P08-T12 | Profile run prints the §13 fields this host can supply | L4 (perf) | **pass (ignored, on demand)** | `pi_profile::profile_run_reports_tps_mspt_phases`: wall 2.05 s, mean 10.2 ms, p50/p95/p99 0.66/0.78/370.31 ms, broadcast dominates, 46/240 overruns are the join burst; no CPU/RSS on this host |
| P08-T13 | Differential suites re-run green against the jar | L6 | **pass (ignored, needs the jar)** | 13 passed across 6 suites (`vanilla_pack` 1, `vanilla_data` 1, `vanilla_smelting` 1, `structure_pack` 6, `scenario_vanilla` 2, `structure_wiring` 2), `MC_VANILLA_DATA=$PWD\target\vanilla-26.1.2\extract\data\minecraft` |
| P08-T14 | Flaky metrics TempDir-tag collision observed, deferred | L7 | **pass (observation)** | 1 failure in 4 full runs (`level.dat.tmp` rename race, shared `ops-metrics` tag); recorded in `PHASE-08-REPORT.md` §2.1 for P09 tag-scoping |

### Bugs found by these tests (L7)

| Bug | Found by | Fix / regression |
|---|---|---|
| Persistence bench printed `0.000 s` while passing — it measured nothing (`Game::new` borrows storage, `save_all_owned` silently saves nothing) | P08-T11 first version | Own storage (`with_seed_and_storage`) + `elapsed >= 1 µs` assertion that cannot pass while measuring nothing |
| Workload measured the collision solver (p95 101 ms), not tick overhead | P08-T10 first version | Rotation/swing/hotbar + one periodic real move; both numbers kept in the commit message |
| `bypassesPlayerLimit` comments said "not enforced" after P08-06 enforced it | re-reading every `bypass` mention | Name the enforcement site (`Game::is_full_for`); same doc-rot shape as Phase 07 §2.7/2.8 |
| `PHASE-08-REPORT.md` cited §§2.5/7 that did not exist | P08 verification cluster | Wrote the missing §2.5 (palette no-fix probe) + §2.6 (matrix deferral); fixed the P08-16 row pointer |

## Phase 09 — Conformance, release candidate and plugin readiness

The sweep rows re-derive the phase claims from named suites measured in the
2026-09-12 runs; per-crate lib binaries are in the full-run output, and counts
below are the named integration/fixture/differential suites only.

| ID | Case | Level | Status | Evidence |
|---|---|---|---|---|
| P09-T01 | Full test matrix execution | all | **pass** | three full-workspace runs on 2026-09-12 (1 189 / 0 / 21 twice, then **1 191** / 0 / 21 after the registry fix), 74 suites each; Phase 08 section added in the same pass (commit `bf118c8`) |
| P09-T02 | Protocol conformance sweep | L1+L2+L4+L5 | **pass** | `packet_ids` 4 (jar-extracted ids incl. the `chat_command`=7 regression), `fixtures` 4 (golden bytes), `keepalive` 1, `login_tolerance` 2, `e2e_login_play` 6, plus the hostile VarInt/frame/packet corpus inside the `mc-protocol` lib binaries |
| P09-T03 | Persistence compatibility sweep | L1+L3+L6 | **pass** | `restart` 7, `corruption` 16, `anvil_fixture` 9 (byte-identical palette repack), `vanilla_chunk` 4, and the env-gated `vanilla_differential` 2 run green with `MC_VANILLA_DATA`+`MC_VANILLA_JAR`+`MC_VANILLA_WORLD` — vanilla booted on our rewritten world (2026-09-12) |
| P09-T04 | Survival regression sweep | L3+L4+L5 | **pass** | `survival_e2e` 7 (join/stream/move/break/place/death/save-reload), `network_game_bridge` 4 (real-socket login → play), `vanilla_chunk` 4 (real terrain walkable) |
| P09-T05 | Entity/redstone regression sweep | L1+L2+L3+L5 | **pass** | `entity_lifecycle` 9, redstone suites 47 (`propagation` 17, `world_integration` 4, `budget_exhaustion` 7, `determinism` 6, `golden_circuits` 5, `power_model` 8), inventory/container: `container_e2e` 6, `block_entity_e2e` 6, `inventory_duplication` 5 |
| P09-T06 | Commands/data/worldgen regression sweep | L1+L3+L5+L6 | **pass** | `command_e2e` 11, `execute_e2e` 9, `function_e2e` 14, `pack_discovery` 10, `pack_loading_e2e` 8, `worldgen_e2e` 7, worldgen golden suites 30 (`structure_golden` 9, `seed_derivation` 8, `golden` 6, `determinism` 7), differential: `vanilla_pack` 1, `vanilla_data` 1, `vanilla_smelting` 1, `structure_pack` 6, `scenario_vanilla` 2, `structure_wiring` 2 |
| P09-T07 | Security adversarial sweep | L1+L2+L3+L5 | **pass** | hostile-input classes green in the full run: non-terminating VarInt/frames + random-byte connections (protocol/network libs), slow-drip bound + registry cap + `framing.rs` drip test, `command_e2e` flood test, `ops_e2e` 7 (permission/impersonation), `inventory_duplication` 5 (2 000-click conservation), `corruption` 16 (hostile disk), config guardrails |
| P09-T08 | Pi performance release sweep | L4 (perf) | **pass (ignored, on demand; dev host both profiles + Pi 5 acceptance)** | 2026-09-12: dev-host debug settled p50/p95/p99 0.67/0.74/0.84 ms max 1.02; release figures in `BENCHMARK-BASELINE.md` §P09-08; **the §4 Pi run was then executed on real hardware** — 30-min soak, 10 scripted clients, settled medians 0.21/0.27/0.29 ms, zero settled overruns → 20 TPS accepted for the scripted workload (§P09-Pi; boundaries named there) |
| P09-T09 | Reproducible release build | — | **pass (build) / blocked (publish)** | `cargo build --workspace --release --locked` green + real-socket status smoke; **no artifact published** — at the time, R-09 license decision pending (resolved same day by ADR-0006, MIT; no publication channel exists); recorded in `BENCHMARK-BASELINE.md` §P09-09 |
| P09-T10 | Release documentation | — | **pass** | `docs/release/RELEASE-CANDIDATE.md`: build/run/claims table, every "no" tied to a KD entry |
| P09-T11 | Known divergence catalog | — | **pass** | `docs/vanilla-parity/KNOWN-DIVERGENCES.md` KD-01..38, each sourced to a matrix row or phase report |
| P09-T12 | Rust-native plugin boundary ADR | — | **pass** | `docs/adr/ADR-0005-plugin-boundary.md`: three named seams with code sites and triggers; zero API types (grep-verified) |
| P09-T13 | Independent final review | L7 | **pass** | adversarial review by a fresh agent, findings + dispositions in `docs/phases/AUDIT-06-FINDINGS.md` |
| P09-T14 | Release candidate acceptance report | — | **pass** | `docs/phases/PHASE-09-REPORT.md`, exit-gate verdict table per clause |

### The metrics flaky fixed by this phase (L7)

| Bug | Found by | Fix |
|---|---|---|
| `mc-server` lib tests failed ~1 in 4 full runs: `cannot move .../level.dat.tmp into place (os error 2)` | PHASE-08-REPORT §2.1 observation | Root cause proven with a probe: `TempDir` names are `tag+pid+nanos`; under parallel filesystem I/O the wall clock quantises — duplicate paths recur across identical 160 000-construction probe runs (7 and 17; `target/p09_probe_tempdir.log` retains the 7). All three metrics tests shared the tag `ops-metrics`, so one test's `WorldService::open` raced another's drop-time `remove_dir_all`. Fix: `game(tag)` with a distinct tag per test (the codebase-wide convention); 10 consecutive green runs (`target/p09_metrics_repeat.log`) + two full-workspace green runs after the fix |

## Later phases (tracked, not yet expanded)

P04 survival slice (move/collide/break/place/inventory/death/restart) · P05 determinism scenarios + entity suites ·
P06 inventory-adversarial + redstone golden/differential · P07 command-permission matrix + datapack/worldgen parity ·
Expand at phase entry.
