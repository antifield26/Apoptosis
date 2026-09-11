# Test Matrix (living document — Phase 06 update)

Conventions: `L1` unit · `L2` property/fuzz · `L3` integration · `L4` golden/fixture ·
`L5` end-to-end · `L6` differential vs vanilla · `L7` regression-per-bug.
Status: `planned | pass | fail | skipped(reason) | ignored(needs external input)`.
Totals after Phase 06: **745 passed, 0 failed, 4 ignored** (measured; the per-suite
breakdown is in the Phase 06 report §3.1). The figure is re-derived from a run rather
than accumulated, because Audit 03 caught two stale totals in this file and the first
draft of the Phase 06 report got five rows wrong the same way.

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

## Later phases (tracked, not yet expanded)

P04 survival slice (move/collide/break/place/inventory/death/restart) · P05 determinism scenarios + entity suites ·
P06 inventory-adversarial + redstone golden/differential · P07 command-permission matrix + datapack/worldgen parity ·
P08 Pi workload + exhaustion drills + kill-during-save · P09 full-matrix + conformance sweeps. Expand at phase entry.
