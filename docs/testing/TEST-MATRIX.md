# Test Matrix — current view

Conventions: the test-level vocabulary `L1` unit · `L2` property/fuzz ·
`L3` integration · `L4` golden/fixture · `L5` end-to-end · `L6` differential vs
vanilla · `L7` regression-per-bug is defined in
[CONVENTIONS.md §11](../CONVENTIONS.md).

Totals: **1 500 passed, 0 failed, 34 ignored** across **116 suites**, re-derived from
`cargo test --workspace --no-fail-fast` on the current tree (`python
tools/gates/run.py --quick`: every gate passed). The count has
moved 1 194 -> 1 196 -> 1 206 -> 1 207 -> 1 212 -> 1 325 -> 1 344 -> 1 346 -> 1 358 ->
1 365 -> 1 380 -> 1 384 -> 1 386 -> 1 388 -> 1 389 -> 1 392 -> 1 399 -> 1 412 -> 1 426 -> 1 428 -> 1 431 -> 1 433 -> 1 437 -> 1 440 -> 1 441 -> 1 445 -> 1 451 -> 1 452 -> 1 468 -> 1 475 -> 1 477 -> 1 491 -> **1 500**: three from the Audit 07 remediation, two Audit 08 coverage tests, ten from the
`mc-capture-rig` crate (P10-01), one regression test for the
compression-transition defect (P10-02), the synced-registry work (P10-03), then
P10-04..11 and P11-01..03, nineteen from the P11-04..09 landing, two from
AUDIT-10 itself, twelve from the M-1..M-4 fixes (three more `respawn` wire-shape
tests in the `mc-protocol` lib — one round trip became four tests, one of them a
`#[should_panic]` that drives the client's reader off the end of the old body — the
liquid predicate's own test in `mc-world`, and the server suites `block_change_ack`
(4) and `mob_pathing` (4)), seven from the AUDIT-11 remediation (the
`reach_validation` suite), fifteen from P12 (three `mc-protocol` lib
round-trips (`open_screen`, `container_set_data`, `container_close`+`set_cursor_item`),
three `mc-container` lib (`block_menus` slot counts, chest-click conservation,
pack-book conversion), eight `survival_e2e` (`/tp`-to-air death→respawn, chest
open, chest-transaction flood, furnace progress, hopper pull+push, chest-break
drops, close cursor return, crafting recompute+take), and one `block_entity_e2e`
(chest restart)), and **four from the AUDIT-12 remediation**: the pack
double-join regression (`pack_loading_e2e`, perturbation-verified), the stale
craft-take guard (`survival_e2e`, perturbation-verified), the break-cursor
return, the furnace viewer-slot resync, and **two from P13-01**: the
scheduled-tick due-once timing and the budget-defers-burst cases
(`scheduled_ticks`, perturbation-verified no-drain), and **two from P13-02**:
the place/break feed with the dirt relevance gate and the lever flip
round-trip (`survival_e2e`, perturbation-verified neighbour arm), and **one
from P13-03**: lever—wire—wire—lamp on/off end to end with `block_update`s
(`survival_e2e`), and **three from P13-04**: torch attachment flip plus
comparator back/side separation (`mc-redstone` lib) and the torch-attachment
end-to-end round-trip (`survival_e2e`), and **seven from P13-06**: six net-new
conductivity-matrix tests in `propagation` (17 -> 23: torch-only-above,
lever-mount-only, block-never, dust-beneath/beside/headroom,
dust-reads-beside/below, lamp-above/below; the two weak/strong-through-stone
placeholders are replaced by the measured cells) and one net-new
`golden_circuits` test (5 -> 6: the lever-mount-strong golden); the lamp
golden and the lamp end-to-end moved onto the vanilla-faithful pedestal
geometry in the same pass, and **thirteen from P13-07**: the `vanilla_conductivity`
differential, which replays the machine-read fixture (92 cells, one vanilla
world) through the model row by row, and **fourteen from P14-01/02**: eleven
`admin_commands` end-to-end (gamemode/give/kill/seed/difficulty + peaceful-night
 + op/deop grant, revoke, persist, reload, ladder, no-dir), two `ops` unit
(insert/grant/revoke/round-trip/save shape), and one `mc-entity` unit
(`kill` bypasses invulnerability but not death), and **two from P14-03**: the
Y-first landing and the longer-axis-first slip-past (`mc-world` collision,
both verified to fail on the old X-first order), and **three from P14-04**: the
`forget_level_chunk` packed-long round trip (`mc-protocol`) plus teleport-away
forget-by-name and view-distance clamp/confirm/honour (`chunk_streaming` e2e),
and **two from P14-05**: leave-then-rejoin with the entity sweep, and rejoin
after a full restart (`reconnect` e2e; death/respawn with a real client stays
on KD-38's open list), and **four from the soak follow-ups**: the cache-center
update on chunk crossing (`chunk_streaming`), two loot-baseline roll tests
(`mc-data`), and breaking common blocks with no pack loaded
(`loot_and_pickup`), and **three from AUDIT-14**: the locked-difficulty
refusal and the op-save-failure rollback (`admin_commands` 11 -> 13), and the
config-phase view-distance event reaching the game (`command_e2e` 11 -> 12),
and **one from the P14-09 walk**: the empty pack load keeps the loot baseline
(`pack_loading_e2e` 9 -> 10; fails on revert to the baseline-wiping load), and
**one from the P14-09 walk round 2**: the overworld-clock `set_time` golden
(`mc-protocol` lib 117 -> 118), `time set` + presets reaching the client
(`command_e2e` 12 -> 13), the respawn level-load event (folded into the
`death_and_respawn` test), in-place held-slot consume (`survival_e2e`
26 -> 27), and rejoin restoring the leave state (`reconnect` 2 -> 3), and
**six from the P14-10 walk**: four `playerdata` unit (missing/path/round-trip/
garbage, `mc-server` lib 63 -> 67) plus restart-restore and corrupt-fresh
(`reconnect` 3 -> 5; both fail with the write or the read removed), and
**one from P15-01**: the overrun worst-phase synthetic test (`mc-simulation`
27 -> 28; fails when the tie-break flips), and **sixteen from P15-06**: four
error-message stability tests (`mc-command`: parse/execute/selector/tree),
six (`mc-data`: function/json/loot/pack messages plus the function/json
`source()`-chain pins), one (`mc-network` limit errors), five
(`mc-worldgen`: provider/generation plus three structure groups), and **seven
from P15-07**: the region commit-order test (fails when the location word
moves before the payload), the NBT hetero-list refusal, three fixture-env
subprocess tests, the capture sweep, and the cow sound-variant golden, and
**two from AUDIT-15 remediation**: the serverbound-close 128 case and the
LpVec3 canonical 3-case (the cow exact pin adds asserts to an existing test),
and **fourteen from P16-01**: seven combat numbers (sources, absorb shape,
weapon/armour tables, reach hook, held damage, worn stats), three knockback
(away, halve, facing fallback), one armour-bypass (melee vs falls vs
starvation), one sword integration, one shove integration and one iron-suit
integration, and **nine from P16-02**: three orb numbers (splits,
age/despawn, merge) and six orb end-to-end (sword scatter, fall-kill
silence, pickup levels, throttle, merge, capped player death).
The 34 ignored = the 8 differential
suites in the last section (16 tests) + `vanilla_loot` 3 + `vanilla_chunk_light` 2
+ `vanilla_light_differential` 1 + `light_update_trigger` 1 + `sky_light_surface` 2
+ `terrain_distribution` 3 + `pi_profile` 4 + `tick_baseline` 2 (16+3+2+1+1+2+3+4+2
= 34) — all run on demand.

**Every per-crate count below was re-measured with `cargo test -p <crate> --lib`**
while updating this total. The 17 lib counts sum to **1 045** (`mc-protocol` 113 ->
**118**, `mc-container` 130 -> **133**, `mc-redstone` 64 -> **66**, `mc-entity` 135 ->
**136**, `mc-server` 61 -> **67**, `mc-simulation` 27 -> **28**, `mc-world` 42 -> **44**, `mc-data` 127 ->
**129**, rest unchanged), the 5 doc-tests and
the **402** named-suite tests complete the 1 452 (1 045 + 5 + 402 = 1 452, the third
figure derived from the run total and the other two rather than counted
independently). Both Audit 07's method and its lesson still apply: the figures must
be re-measured per crate, because five of them once turned out to be **another
crate's** count with the names rotated (`docs/audits/AUDIT-07-REMEDIATION.md`
§"what the audit missed").
**Limitation, stated rather than left as a trap.** The per-*area* lib counts below
are measured and current. The named-suite counts in the middle column were measured
in the P10-03 round; the suites the later landings touched are updated
(`packet_ids` 4 -> 6, `light_cache` 8, `loot_and_pickup` 8, `entity_persistence` 2,
`player_attack` 5, `entity_lifecycle` 9), and the P10/P11 suites the old table never
listed (`natural_spawn` 4, `ai_wiring` 6, `block_change_ack` 4, `mob_pathing` 4,
`reach_validation` 7, and
the server suites added since) are added. A re-measure of **every** named suite is
outstanding, and the two obvious instruments both fail: `target/debug/deps`
accumulates binaries from every past session (listing them summed to 6 385 against a
real total of 1 344), and counting `#[test]` in the sources over-counts (348 against
the derived 350, because some test files contain `#[test]` sequences inside template
strings). The authoritative figures are the totals above and the per-crate lib
counts; a per-suite figure without a note is from the P10-03 round.

This file replaced an accumulator that had grown one per-phase section per
phase (255 rows, six duplicated "Bugs found" tables). The per-phase historical
matrices are preserved in git history at tag **`phase-09-final`**; what stays
here is the matrix a contributor can actually use: what is covered now, what
each suite proves, and the deduplicated defect history.

## Current coverage by area

Counts are from the P11-04..09 run, except the named-suite figures noted above as
from the P10-03 round. Named integration suites are counted explicitly; the
remaining per-crate lib binaries are itemised above and complete the 1 389 total.

| Area | Named suites (lib count) | What they prove |
|---|---|---|
| Protocol | `packet_ids` (6), `fixtures` (4), `mc-protocol` lib (**118**) | every packet id matches the jar's registration bytecode (incl. the `chat_command`=7 regression, L7); **every constant in `ids.rs` is compared against the jar-extracted table, in its own state and direction** (110 of them: 105 + P12 `open_screen`/`container_set_data`/`container_close`/`set_cursor_item` + P14 `forget_level_chunk`); **the protocol version is compared against the jar's own `version.json`** (AUDIT-09 E-03); golden wire bytes for frames/handshake/NBT; hostile VarInt/frame corpora; compression bomb rejection; **`respawn` is decoded the way the client decodes it** — a reader transcribed from `CommonPlayerSpawnInfo`'s bytecode, not a round trip through our own encoder, with the old truncated shape pinned as an out-of-bytes read (M-1); **P12 window packets round-trip with jar-verified shapes** (`open_screen` VarInt+MENU+Component, `container_set_data` VarInt+short+short, `container_close` VarInt, `set_cursor_item` single stack) |
| Network | `mc-network` lib (18), `keepalive` (1), `login_tolerance` (2), `e2e_login_play` (6) | connection lifecycle, admission limits, keepalive timeout kick, malformed input drops only that connection, login-phase tolerance, full offline login over a real socket |
| Persistence | `mc-persistence` lib (74), `anvil_fixture` (9), `corruption` (16), `restart` (7) | region/NBT codec edges, byte-identical palette repack, bit-flip → typed error with no partial publish, save→close→reopen semantics, atomic tmp→rename pinned by `a_failed_commit_leaves_the_live_file_untouched` (Audit 07 finding H1), dirty-flag retry on failure. **Gap, AUDIT-09 B-02**: `the_location_word_is_written_last` asserts the end state, so it passes under a reordered write — the ordering needs an instrument that observes the sequence |
| Survival & world | `mc-world` lib (44), `light_cache` (8), `vanilla_chunk` (4), `survival_e2e` (**27**: 11 + P11 `/tp`-to-air + P12 open/tx/furnace/hopper/break/close/crafting + AUDIT-12 stale-take/break-cursor/furnace-view + P13-02 feed/lever-flip + P13-03 lamp circuit + P13-04 torch attachment + P14-09 held-slot consume), `network_game_bridge` (4), **`block_change_ack` (4)**, **`reach_validation` (7)**, **`chunk_streaming` (3)**, **`admin_commands` (13)**, **`reconnect` (5)** | collision/ray/hostile movement guards; a real vanilla chunk walks and round-trips losslessly; join/stream budgets (unload forgets by name, per-session view distance, cache centre on every crossing), break/place validation, death/respawn, save-reload over real sockets, admin commands, reconnect sweep; **light-cache invalidation drops the diagonal chunk at a corner**, compared against an oracle derived independently from the margin interval (AUDIT-09 B-05); **a fluid is non-solid *and* liquid, and an unknown id is neither** — the predicate M-4's lookahead reads; **a dig is acknowledged with the client's own sequence, once per tick at the high-water mark, after the block update and even when the dig was refused** (M-2); **the reach rule is the jar's arithmetic** — the survival buffer case (4.5 < d < 5.5 accepted, which the pre-AUDIT-11 constant refused), the creative `+0.5`, the strict `<` boundary, the 3-D eye measurement, and the entity gate that stops a swing landing from anywhere (AUDIT-11 N-1) |
| Entities & simulation | `mc-entity` lib (136 + 4 doc), `entity_lifecycle` (9), `entity_persistence` (2), `player_attack` (5), `natural_spawn` (4), `ai_wiring` (6), **`mob_pathing` (4)** | ids never reused, timers/effects/projectiles/pathfinding invariants, JDK-25-verified RNG, phase ordering, determinism replays, spawn/despawn/chunk-unload lifecycle; **a second `Game` on the same `world_dir` gets the saved mobs and drops back**, and a joining player is told about a resident entity on the join tick (P11-08); **one swing takes exactly the fist damage, a second inside the 10-tick window is refused** (P11-06); day/night spawn rules and the 24-block minimum (P11-01); **the walk-speed constant is the measured zombie ceiling, not the player extrapolation, and a zombie is pinned below 7.5 blocks/s** (M-3); **a mob walks up to water and stops at its edge, and a clear course still lets it reach the player** — the negative control that keeps the two refusal tests from being satisfied by a lookahead that refuses everything (M-4) |
| Inventory & containers | `mc-container` lib (**133**), `container_e2e` (6), `block_entity_e2e` (**7**), `inventory_duplication` (5), `loot_and_pickup` (9) | click/swap/drag conservation, stale-state resync, computed slots, retirement reporting; real-socket click round trips; 2 000-click floods cannot create or destroy items — the flood over a **capped slot** reaches the over-limit path the chest flood cannot, so a discarded overflow is caught (Audit 07 finding M1); **the loot table is the drop authority** — a survival break drops the table's item (the fixture names cobblestone for stone, so a block-echoing path fails), creative and table-less blocks drop nothing, a mob death rolls `entities/<kind>`, a ready stack is collected, nearby stacks merge into the older one, and a full inventory leaves the leftover on the ground (P11-04/05/09); since the P14
soak an eight-table loot baseline covers common breaks with no pack loaded
(stone drops cobblestone); **P12**: chest/furnace/hopper menus open on non-zero windows and transact with conservation (20-click flood), furnaces cook with `container_set_data` progress, hoppers transfer on the 8-tick cooldown, block entities persist across restart, breaks drop contents, crafting recomputes from the table, closes return the cursor |
| Redstone | `propagation` (23), `budget_exhaustion` (7), `determinism` (6), `golden_circuits` (6), `power_model` (8), `world_integration` (4), `vanilla_conductivity` (13) | budgeted propagation reaches unbounded-run state, full change-vector determinism, golden circuits (lever—wire—lamp on a dust-powered pedestal; comparator faces east at its wire), power bounds; P13-01 wires the scheduled-tick queue into the tick (see Server foundations); P13-03 drives the lamp; P13-04 reads torch attachments and comparator back/sides by facing; P13-05 measures 15 live blocks on a real 26.1.2 server (world + scripts under `target/p13-wire/`); P13-06 measures the conductivity matrix on the same rig (torch-below/lever-mount strong, dust above/beside weak and solid-blind, block never; lamp reads above/below) |
| Commands & data | `mc-command` lib (89), `command_e2e` (13), `execute_e2e` (9), `function_e2e` (14), `mc-data` lib (129), `pack_discovery` (10), `pack_loading_e2e` (**10**: 9 + P14-09 baseline-survives-load) | permission-before-grammar, every declared command reachable, malformed commands never disconnect, execute modifier chains, function recursion/privilege bounds, pack discovery and world-pack loading (P12-07/08 recipe load + conversions) |
| World generation | `mc-worldgen` lib (81), `worldgen_e2e` (7), `structure_golden` (9), `seed_derivation` (8), `golden` (6), `determinism` (7) | seed determinism, terrain invariants, structure placement goldens, existing-world-first generation |
| Server foundations | `mc-server` lib (63), `scheduled_ticks` (**2**), `mc-core` (10), `mc-nbt` (16 + 1 doc), `mc-registry` (21), `mc-simulation` (28), `mc-redstone` (**66**), `mc-test-support` (4), `mc-capture-rig` (11) | config guardrails, backup/verify/restore, shutdown barrier, operational metrics snapshot, error taxonomy, NBT vectors, the registry table itself (including the `MC_FIXTURE_DIR` precedence half), fixture helpers, the tick scheduler, the redstone model's own invariants, the client-capture rig; P13-01 scheduled-tick drain timing + budget (perturbation-verified) |
| Security (cross-cutting) | classes live inside the suites above | hostile VarInt/frames + random-byte connections (protocol/network libs), slow-drip bound, registry reservation cap, 300-command flood (`command_e2e`), hostile op paths (`ops_e2e`, 7), 2 000-click conservation (`inventory_duplication`), hostile disk state (`corruption`, 16), config bounds (`mc-server` lib), **a decoded packet body with trailing bytes is accepted** (AUDIT-09 A-03, open — serverbound silent-accept is deliberate; AUDIT-14 A14-01 found P14's new clientbound `forget_level_chunk` decoder skipping the hard-refusal convention and fixed it with a padded-input test) |

## Differential suites (jar-gated, `--ignored`)

Need the three environment variables below; they skip cleanly without them.
Evidence as of 2026-09-12: **15 passed / 0 failed across the 7 suites**, plus
P12's `vanilla_crafting` (verified 2026-09-16 with real data: hundreds of
item-only recipes convert, sticks present).

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
| `vanilla_crafting` (mc-container) | 1 | crafting table conversion against the real pack (item-only converts, tags counted) |
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
| 11 | **The real client could not decode `respawn`** — our body ended at `is_flat`, then wrote the data-retention byte and the sea level, so the client read `Optional<GlobalPos>` + `portalCooldown` + `seaLevel` + a trailing byte past the end of it and refused. The owner died and could not respawn | the owner's acceptance round; bytecode-read from the client jar's `CommonPlayerSpawnInfo` and `ClientboundRespawnPacket` | the encoder writes the whole spawn info in the client's own order and the retention byte **after** it; `respawn_is_exactly_what_the_client_reads` decodes our bytes with a reader transcribed from that bytecode, and `respawn_rejects_the_old_truncated_spawn_info` pins the old shape as unreadable (M-1) |
| 11 | **Mining did not appear to break blocks** — the server broke them and told the client, and the client could not apply it. A 26.x client routes a server block change through `ClientLevel.setServerVerifiedBlockState`, which stores it instead of applying it while a prediction is open at that position, and only `block_changed_ack` closes the prediction. We never sent one, so the mined block stayed stone and every later change there was swallowed | the owner's acceptance round; the trace's 12 digs each matched by a `block_update`, then the client jar's `BlockStatePredictionHandler`/`MultiplayerGameMode` bytecode | `block_changed_ack` (clientbound play 4) added, fed by a per-session high-water mark of the sequence in `player_action`/`use_item_on`/`use_item` and sent once per tick after the block changes; `block_change_ack.rs` pins the value, the one-per-tick collapse and the refused-dig case (M-2). **The first version of the collapse test passed under "keep the last sequence" too**, which `target/m_probes.py` probe M-2c caught by perturbation — the test now sends the higher sequence first |
| 11 | **Mobs walked at 9.9 blocks/s** — `SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE` was the walking *player* figure (4.317 ÷ 0.1 = 43.17) extrapolated to every mob, a value its own module doc called unverified and "probably too generous". A zombie's attribute is 0.23 | the owner's acceptance round, then measured from the vanilla capture: the zombie ceiling is 0.3497 blocks/tick over 21 entities | the constant is now the measurement (30.41), the living-world capture is the instrument and `mob_speed_ceiling.py` reruns it; the test pins both the value and the "a zombie is under 7.5 blocks/s" guard (M-3) |
| 11 | **Mobs walked into water and walls** — direct steering wrote a velocity at the target with no look at what was in the way. For water this was not even a collision failure: water is non-solid, so the mob had simply decided to swim | the owner's acceptance round | the AI checks the next cell (feet and head) before steering: solid or fluid refuses the step, a blocked wander abandons its destination and re-rolls. `mob_pathing.rs` pins the water and lava refusals **and** the clear-course arrival that stops them being vacuous (M-4) |
| 11 | **The reach check was stricter than the jar's, in the one band a client actually uses** — it was the bare `block_interaction_range` attribute (4.5) for every game mode, so a survival dig between 4.5 and 5.5 blocks from the eye was refused although vanilla accepts it | AUDIT-11 N-1 (whose stated evidence and conclusion were both refuted; the real defect was underneath them) | the rule is now vanilla's own, bytecode-read: `AABB(pos).distanceToSqr(eye) < (blockInteractionRange() + 1.0)^2` — 5.5 survival, 6.0 creative, strict `<` (`reach_validation.rs`, 7 tests, each perturbation-verified) |
| 11 | **A swing damaged an entity from any distance at all** — `PlayIntent::Interact` never checked reach, a recorded open divergence since AUDIT-09 | AUDIT-11 N-1's *concern*, verified against the jar | `Game::within_entity_reach` applies `isWithinEntityInteractionRange(aabb, 3.0)`, the gate vanilla takes in `handleInteract` before branching on the action: effective 6.0 from the eye. Two suites that had been measuring *wandering* rather than damage were corrected to keep their target in reach (`player_attack`, `loot_and_pickup`) |

## Known-false-assertion lessons

Three of the defects above passed their first version while proving the wrong
thing (the `0.000 s` bench, the 101 ms "tick overhead", the tautological
movement assertion of the bridge test) — each now carries the assertion that
would have caught it. Two more were documents more confident than their code
(the bypass comments, the cited-but-absent report sections). The rule these
taught, now in [CONVENTIONS.md §3.1](../CONVENTIONS.md): a check that cannot
fail is not evidence.

**Two more from the M-1..M-4 landing, both caught by probes rather than by
reading** — recorded here because the same shape will recur:

- **A test that cannot tell two rules apart.** `block_change_ack.rs`'s
  high-water-mark test first queued its two sequences *ascending* (3 then 11), so
  "keep the maximum" and "keep the last" produced the same ack and the test passed
  under either. `target/m_probes.py` probe M-2c perturbs the rule to "last" and
  reported `*** PASSED -- TEST NOT LOAD-BEARING ***`. The order is now 11 then 3,
  and the test says in its own doc why.
- **A test whose name promises more than its body asserts.** The respawn
  falsification anchor was called `respawn_rejects_the_old_truncated_spawn_info`
  while its body asserted only that the client's reader *misaligns* on the old
  shape — it consumed all 35 bytes and the test passed. It is now two tests: a
  `#[should_panic(expected = "out of range")]` named for the rejection it really
  performs, and a separate `the_old_respawn_shape_misaligns_the_clients_fields`
  that names which field lands on which.

**Three more from the AUDIT-11 remediation, and one of them is a probe bug** —
recorded together because they are the same failure mode at three removes:

- **A test that computed its own geometry wrongly, twice.** `reach_validation.rs`
  placed the target at the player's **feet** level and called the horizontal offset
  "the distance", but the eye sits 0.62 above such a block's top face — so the
  boundary case sat at 5.53 rather than 5.5 and the eye-versus-feet case was out of
  range under *both* readings. Both tests passed under the perturbations that were
  supposed to break them (probes N-1c, N-1e: `NOT LOAD-BEARING`). The file now
  computes every distance through the same public primitive the server calls
  (`Aabb::distance_to_sqr`), and the two cases place the target at the eye's level
  so the vertical term is exactly zero by construction.
- **A probe that could not tell it had already corrupted the tree.** `m_probes.py`
  restored each file in a `finally`, which covers an exception but not a `SIGKILL`
  or a timeout. A foreground run was killed with the N-1e perturbation still
  applied; the next run then snapshotted the corrupted file as its baseline,
  restored *to* the corruption, and reported the tree clean. The script now writes
  the originals to `target/probe-backup/` before the first perturbation and
  restores from them on startup, so a killed run is self-healing and visible.
  **The general lesson: a probe's restore step is itself an instrument, and it had
  never been tested by killing it.**
- **Two suites were measuring the wrong thing and passing.** `player_attack`'s
  kill test and `loot_and_pickup`'s mob-death test stood still and swung at a
  chicken across a 10-tick window; once reach was enforced they failed
  (`the chicken is gone after 20 spaced swings`). The *product* was right and the
  tests were wrong — they had been measuring whether a wandering chicken stays
  put. Both now walk the player into range before swinging, with the reason in the
  comment, because the alternative (relaxing the reach check) would have been
  fixing the test by breaking the feature.
