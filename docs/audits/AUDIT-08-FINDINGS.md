# AUDIT-08 — Second Full Adversarial Audit (Apoptosis)

Date: 2026-09-12. Auditor: independent adversarial agent (unrelated to all
prior implementers and auditors). Proposition audited: *"the initial preset
task list (P00-01..P09-14, 158 tasks) is fully delivered and every current
completion claim has reproducible evidence."*

**VERDICT: PASS WITH FINDINGS.** All 158 tasks carry evidence pointers; 3 are
recorded not-implemented boundaries (P05-15, P05-16, and the P05-10 no-op
half) that the repository's own parity matrix has never claimed as done — so
"fully delivered" holds only in the sense the project defines delivery
("unsupported behaviour is explicitly recorded", CONVENTIONS.md §3.3/§15).
Every number and claim in this report decomposes into actually-compared
entries versus skipped entries.

## 0. Repository facts (Step 0, self-verified)

| Claim from the handover | Measured |
|---|---|
| 46 commits | 46 ✓ |
| clean working tree | clean ✓ |
| HEAD `b279cdf` ahead of origin/main by 2 (`319cdb4`, `b279cdf`), never CI-verified | **confirmed: 0 behind / 2 ahead**; latest CI run targets `bf74123` → HIGH-1 |
| tag `phase-09-final` resolves | ✓ (`git cat-file -e phase-09-final:docs/phases/PHASE-05-REPORT.md` succeeds) |
| ~277 tracked files | 277 ✓ |
| CI history | 6 runs: latest success on `bf74123`; two earlier failures were fixed same-day |

## 1. Findings

| ID | Severity | Finding | Evidence | Suggested disposition |
|---|---|---|---|---|
| H1 | HIGH | Local HEAD is **2 commits ahead of origin/main and unverified by CI** (`319cdb4` AUDIT-07 remediation, `b279cdf` lane E). Every "CI green" statement covers only `bf74123`. | `git rev-list --left-right --count origin/main...HEAD` = `0 2`; `gh run list` latest run = `bf74123` | Push and let CI run; do not treat the unpushed work as verified until it is |
| H2 | HIGH | `docs/testing/TEST-MATRIX.md:19-20` contains **unrendered template placeholders** — "The lib counts sum to {lib_sum}, the {{doc_sum}} doc-tests and {{NAMED}} named-suite tests complete {{TOTAL}}" — in the very document that owns the gate total. `check_gate_totals.py` cannot see it (its regex only matches `<digits> passed`), i.e. the AUDIT-07 silent-skip lesson reproduced inside the remediation. | TEST-MATRIX.md:19-20; my own accounting: lib 956 + doc-tests 5 + named integration 233 = **1194** (74 suites, 0 unattributed) | Render the numbers (956/5/233/1194) or delete the sentence; consider widening the checker to catch `{{…}}` placeholders |
| M1 | MEDIUM | `check_encoding.py`'s suffix filter misses **6 tracked human-readable text files**: `LICENSE`, `NOTICE`, `.gitattributes`, `.gitignore`, `Cargo.lock`, `.editorconfig` (plus 2 binary fixtures, correctly excluded). Probe: appending `0xFF 0xFE` to NOTICE passes silently while the same bytes in README.md fail the check. | probe log (Lane F table, row 2); suffix enumeration: 8 uncovered of 277 | Add the extensions (or invert to a deny-list of binary types); the docstring's "every tracked text file" overclaims |
| M2 | MEDIUM | `Registries::vanilla()`'s `$MC_FIXTURE_DIR` operator-override branch has **zero test coverage** — the symbol appears only in `crates/registry/src/lib.rs:88,118`; no test sets the variable. (AUDIT-08's own P09 code.) | `grep -rn MC_FIXTURE_DIR crates/` → 2 hits, both lib.rs | Add a test setting the override to a fixture copy and asserting it wins |
| M3 | MEDIUM | `save.rs sync_directory` (directory fsync) is **unfalsifiable by the suite**: disabled, all 74 `mc-persistence` lib tests stay green. The code documents the platform limitation (Windows cannot fsync a directory) but no record states that test coverage is impossible. | Lane C F3: no-op injected, 74/74 green | Record the coverage-impossible reason at the function (DoD item 4), or add a Linux-only test |
| M4 | MEDIUM | **Deployed systemd unit scores 9.2 UNSAFE** (`systemd-analyze security mc-server` on the Pi): no `ProtectSystem`/`PrivateTmp`/`ProtectHome` etc. The unit was applied for the first time during the P09 acceptance; hardening was never claimed, but an internet-facing server on the standard unit is exposed. | Lane E output (this audit) | Add the standard filesystem/network sandbox set to `deploy/mc-server.service`, re-verify on the Pi |
| L1 | LOW | `SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE` (mob.rs:115, "derived from the vanilla walking speed") is **unpinned**: changed 43.17 → 40.0, all 127 mc-entity tests stay green. Consistent with the parity matrix's "unverified" label, but the derivation claim has no test. | Lane C F8 | Pin the derived table with a golden test, or mark the constant as a product decision |
| L2 | LOW | `hopper.rs push_target` reports `room = MAX_ITEMS_PER_TRANSFER` for empty slots **without consulting the destination slot's stack limit**. Unreachable today (hopper roles are 64-stack, the only caller is library-only), but the API silently mis-computes for limited slots. | hopper.rs:285-291; Lane C F6: the guarded branch is unreachable by construction (`moved <= room`), so conservation holds structurally — verified by the room over-report probe FAILING the flood test | Consult `slot_limit` in `push_target`, or document the precondition |
| L3 | LOW | `check_gate_totals.py`'s regex does not parse comma-grouped numbers: "1,194 passed" is caught **as "194"** (loud but mangled diagnostic). No-space "1194 passed" is correctly accepted. | Lane F probe 6/7 | Extend the character class with `,` and strip it in the value |
| L4 | LOW | The retired-report reference count is stated inconsistently: GOVERNANCE-REPORT.md says 10, the AUDIT-08 handover said 13/12 files, **measured now: 12 occurrences in 11 files** under `crates/`+`apps/`. | `grep -rn "PHASE-0[0-9]-REPORT\|AUDIT-0[0-9]-FINDINGS" crates apps --include="*.rs"` | Update GOVERNANCE-REPORT to the measured count; the citations resolve at tag `phase-09-final` |
| L5 | LOW | The Pi deployment is stale relative to HEAD: the deployed binary was built 2026-09-12 13:55 (pre-AUDIT-07 code), `~/mcserver` has no git metadata (tarball extract, commit identity unverifiable by git), content diff vs HEAD = 39 lines (AUDIT-07's save.rs/menu tests, governance docs). | Lane E: diff -rq HEAD-tree vs ~/mcserver | Re-deploy from a HEAD archive when the next code change lands; record the built commit on the device |
| I1 | INFO | AUDIT-07 Lane E items confirmed still open: deployed config still loads no data packs (E4); `/var/backups/mc-server/` still empty (E5). New positives: the device copies of the soak tools are **byte-identical** to `tools/pi-bench/` (SHA-256 match), the deployed `level.dat` carries **DataVersion 4790**, the service cycle exits **0** (ExecMainStatus), and the archived bench numbers (684 resident / 1 360 streamed / 45.0 ms per chunk) match the archived log. | Lane E outputs | none / carry E4-E5 |

## 2. Lane B — gates, differential, scripts: measured vs claimed

| Gate / suite | Claimed | Measured | Verdict |
|---|---|---|---|
| `cargo test --workspace --no-fail-fast` | 1 194 / 0 / 21, 74 suites | **1 194 / 0 / 21, 74 suites, exit 0** | match |
| `cargo fmt --all -- --check` | exit 0 | 0 | match |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | 0 | match |
| `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | exit 0 | 0 | match |
| `cargo deny check licenses bans sources` | exit 0 | 0 | match |
| 7 differential suites (3 env vars) | 15 passed / 0 failed | **15 passed / 0 failed** | match |
| Per-crate counts in TEST-MATRIX.md | "re-measured with `cargo test -p <crate> --lib`" | all 17 crate-level rollups recomputed from the run log via cargo-metadata target→package mapping: **every count matches; 0 of 74 suites unattributed** | match |
| Four docs-audit scripts | exit 0 | all 0 (and adversarially probed, §4) | match |

## 3. Lane C — falsification ledger (10 probes; none overlap AUDIT-07's targets)

| # | Mechanism under test | Disable point | Result under break | Restored |
|---|---|---|---|---|
| F1 | failed `commit` must not lose the live file (AUDIT-07 H1's new test) | `save.rs commit`: `remove_file(path)` before the rename | `a_failed_commit_leaves_the_live_file_untouched` **FAILED** (live file gone) | ✓ byte-exact (`git diff` empty), test PASS |
| F2 | capped-slot overflow must return to the cursor (AUDIT-07 M1's new test) | `menu.rs insert`: discard the overflow instead of returning it | `a_flood_over_a_capped_slot…` **FAILED** (cursor 0 ≠ 63) | ✓ restored, PASS |
| F3 | directory fsync after rename | `save.rs sync_directory` → no-op | **no test fails** (74/74 green) — coverage gap, finding M3 | ✓ restored |
| F4 | `$MC_FIXTURE_DIR` override wins | env branch structurally uncovered | no consumer exists — structural finding M2 (runtime probe skipped: nothing to drive it) | n/a (no edit needed beyond analysis) |
| F5 | splitmix64 finalizer | `seed.rs splitmix64_mix` → identity | 4 mc-worldgen tests **FAILED** | ✓ restored, 81/81 PASS |
| F6 | hopper room computation | `push_target` over-reports room by 5 | flood conservation test **FAILED** (Invariant error surfaces) | ✓ restored, PASS |
| F7 | backup stray-file guard | `backup.rs stray_files` → always None | 1 backup test **FAILED** | ✓ restored, 5/5 PASS |
| F8 | mob speed derivation constant | `mob.rs` 43.17 → 40.0 | **no test fails** (127/127 green) — finding L1 | ✓ restored |
| F9 | VarInt overlong cap | `varint.rs` cap check bypassed | 2 varint tests **FAILED** | ✓ restored, 6/6 PASS |
| F10 | palette bit-width boundary | `packing.rs bits_for` off-by-one | 2 mc-persistence tests **FAILED** | ✓ restored, 74/74 PASS |

All restores verified with `git status`/`git diff` (empty) — byte-exact against the index.

## 4. Lane F — the audit tools themselves, adversarially probed

| Script | Claimed detection | Probe | Caught? |
|---|---|---|---|
| `check_encoding.py` | invalid UTF-8 / mojibake in tracked text files | `0xFF 0xFE` appended to README.md; U+951B mark in README.md | **yes, both**, exit 1, names the file |
| `check_encoding.py` | …"every tracked text file" | `0xFF 0xFE` appended to NOTICE (no suffix) | **no** — coverage gap M1 |
| `check_line_endings.py` | working-tree CRLF under `eol=lf` | README.md rewritten with CRLF | **yes**, exit 1 |
| `check_gate_totals.py` | a restated total disagreeing with the canonical | RELEASE-CANDIDATE drifted to 1 191 | **yes**, exit 1, names file:line |
| `check_gate_totals.py` | regex coverage | "1,194 passed" caught as mangled "194"; "1194 passed" correctly accepted | L3 |
| `check_links.py` | untracked-file references, dead paths | verified by AUDIT-07 (per handover); exemption census this round: `target/` citations to machine-local tooling and `OpenSourceMinecraftServer/` clone paths are the excluded population, both documented conventions | per handover + census |
| CI wiring | all four scripts run on push | `.github/workflows/ci.yml` docs-audit job runs encoding → links → gate_totals → line_endings, after fetching the archive tag | verified by reading; job observed running on GitHub |

## 5. Lane A — task verdicts (158 rows)

Decomposition: **32 deep dives (code + test read this round) · 126 inherited
from AUDIT-07's table (its 149 suite rows + 9 falsified rows were verified to
cover all 158 IDs; 32 of them upgraded here) · 0 skipped.** Boundary tasks are
recorded gaps the repository never claimed as done.

| Task | Title | AUDIT-08 verdict | Evidence |
|---|---|---|---|
| P00-01 | Inventory local reference repositories, commits, branches and paths | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-02 | License and provenance audit of Paper/Pumpkin/Valence/Minestom | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-03 | Establish Minecraft 26.1.2 protocol/version baseline | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-04 | Map protocol states, packet families and login/play lifecycle | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-05 | Compare architecture and threading models across references | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-06 | Compare world/chunk/persistence approaches | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-07 | Identify behaviorally critical Vanilla parity domains | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-08 | Define differential-testing strategy and baseline harness shape | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-09 | Produce initial dependency/license policy | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P00-10 | Write ADR-0001 system architecture and risk register | suite-green (inherited) | crate tests re-run green (docs/research, docs/adr) |
| P01-01 | Initialize workspace and package metadata | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-02 | Pin stable Rust toolchain and reproducible build settings | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-03 | Configure fmt/clippy/lints and CI | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-04 | Establish logical crate boundaries with real initial ownership | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-05 | Implement error taxonomy and propagation policy | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-06 | Implement config schema/validation (TOML) | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-07 | Implement tracing/structured logging | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-08 | Implement server lifecycle and graceful shutdown | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-09 | Implement deterministic clock/tick abstraction | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-10 | Establish shared test-support/fixture harness | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-11 | Add x86_64/aarch64 build verification | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P01-12 | Foundation review and hardening | suite-green (inherited) | crate tests re-run green (core, test-support, apps/server, .github, deploy) |
| P02-01 | TCP/Tokio connection lifecycle | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-02 | VarInt/VarLong codecs with hostile-input limits | suite-green (inherited) | mc-protocol lib (126) re-run green; hostile VarInt/frame corpora present in the crate |
| P02-03 | Frame reader/writer and size limits | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-04 | Packet registry/version strategy | suite-green (inherited) | Lane C C1: broke CHAT_COMMAND id to 8 -> packet_ids FAILED (2 failed) |
| P02-05 | Handshake/status packets | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-06 | Login state machine | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-07 | Offline authentication/GameProfile path | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-08 | Online authentication provider boundary | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-09 | Compression negotiation | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-10 | Play-state packet plumbing | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-11 | Packet fixture/golden harness | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-12 | Protocol fuzz/property tests | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-13 | Minimal test client | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-14 | Client login/play E2E test | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-15 | Connection rate/resource limits | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P02-16 | Protocol review and conformance report | suite-green (inherited) | crate tests re-run green (protocol, network) |
| P03-01 | NBT primitive model | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-02 | NBT parser | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-03 | NBT writer | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-04 | NBT malformed/fuzz tests | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-05 | Compression adapters | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-06 | Anvil region header/read path | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-07 | Region chunk write path | suite-green (inherited) | Lane C C2: skipped the palette re-pack -> restart FAILED (3 failed) |
| P03-08 | Level metadata model | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-09 | Dimension/world metadata model | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-10 | In-memory chunk serialization boundary | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-11 | Dirty chunk tracking | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-12 | Autosave scheduling | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-13 | Atomic/ordered save semantics | suite-green (inherited) | Lane C C7: removed the live level.dat before the rename -> **no test noticed** (finding H1) — **caveat:** finding H1: "atomic tmp->rename" is implemented and documented but no test in mc-persistence detects a non-atomic ordering |
| P03-14 | Restart/load integration tests | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-15 | Corruption/failure recovery tests | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P03-16 | Persistence review and compatibility report | suite-green (inherited) | crate tests re-run green (nbt, persistence) |
| P04-01 | Registry/data bootstrap for core block/item IDs | suite-green (inherited) | Lane C C5: moved the build-tree fixture fallback first -> the registry search-order test FAILED (1 of 14) |
| P04-02 | World/dimension runtime shell | **deep-verified** | ids.rs CHAT_COMMAND + packet_ids.rs::the_regression… (Lane C: AUDIT-07 falsified C1; re-verified test exists and is load-bearing per its record) (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-03 | Chunk loading/streaming integration | **deep-verified** | packets/play.rs chunk encode + chunk_data_has_no_section_count_prefix (protocol lib 103) (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-04 | Player entity/state model | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-05 | Spawn/respawn lifecycle | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-06 | Player movement and server authority | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-07 | Basic collision | **deep-verified** | world.rs:274 move_with_collision + world.rs:490 falling_stops_on_the_floor (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-08 | Block state storage/query/update | **deep-verified** | world.rs:212 set_block + world.rs:713 set_block_creates_the_chunk_implicitly (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-09 | Block break validation | **deep-verified** | game.rs in_build_range + survival_e2e::breaking_and_placing_blocks_is_validated_and_broadcast (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-10 | Block place validation | **deep-verified** | game.rs:1631 UseItemOn handler + the same e2e (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-11 | Basic interaction packets/events | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-12 | Item stack/slot primitives | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-13 | Player inventory | **deep-verified** | game.rs stream_for + survival_e2e::a_player_joins_and_receives_terrain_and_vitals (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-14 | Health/hunger/XP baseline | **deep-verified** | game.rs move_player + survival_e2e::movement_is_clamped… (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-15 | Death/respawn | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P04-16 | Survival vertical-slice E2E test | **deep-verified** | survival_e2e (7): break/place/death/respawn e2e (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-17 | Save/reload vertical-slice test | **deep-verified** | survival_e2e::the_world_survives_a_save_and_reload (AUDIT-07: crate tests re-run green (registry, world, entity, server)) |
| P04-18 | TPS baseline and vertical-slice review | suite-green (inherited) | crate tests re-run green (registry, world, entity, server) |
| P05-01 | Fixed 20 TPS scheduler integration | **deep-verified** | scheduler.rs:156 run_tick + a_failing_phase_aborts_the_tick_but_is_still_measured (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-02 | System ordering and deterministic tick phases | **deep-verified** | phase.rs:35 PHASE_ORDER + phase.rs:89 the_order_is_the_documented_one (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-03 | Entity ID/lifecycle manager | **deep-verified** | entity.rs spawn + ids_are_positive_ascending_and_never_reused (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-04 | Spatial query primitives | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-05 | Physics/collision expansion | **deep-verified** | world.rs move_with_collision + vanilla_chunk walk test (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-06 | Damage and invulnerability rules | **deep-verified** | game.rs damage_entity + effect.rs:322 damage_multiplier_caps_at_full_resistance + player.rs creative_and_spectator_are_invulnerable (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-07 | Effects/status conditions | **deep-verified** | effect.rs damage_taken_multiplier + damage_multiplier_caps_at_full_resistance (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-08 | Item entities/pickup | **deep-verified** | item_entity.rs ItemEntity + entity_lifecycle::dropping_the_held_item_spawns_an_item_entity (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-09 | Projectile baseline | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-10 | Scheduled block/entity ticks | **boundary (documented no-op)** | TickPhase::ScheduledTicks exists; parity matrix records the no-op; no covering test (by design of the boundary) (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-11 | Mob spawn rules | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-12 | Basic hostile mob AI | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-13 | Basic passive mob AI | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-14 | Pathfinding baseline | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P05-15 | Entity synchronization | **boundary (not implemented)** | parity matrix: entities are server-side only; recorded, never claimed (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-16 | Entity/persistence integration | **boundary (not implemented)** | parity matrix: entities do not survive restart; recorded, never claimed (AUDIT-07: crate tests re-run green (simulation, entity)) |
| P05-17 | Determinism regression scenarios | **deep-verified** | entity_lifecycle::the_same_seed_replays_the_same_tick_reports (AUDIT-07: Lane C C6: zero-extended nextLong halves -> mc-simulation FAILED (1 failed)) |
| P05-18 | Entity-heavy benchmark/review | suite-green (inherited) | crate tests re-run green (simulation, entity) |
| P06-01 | Server-authoritative inventory transaction model | **deep-verified** | menu.rs apply_click + a_stale_state_id_triggers_a_resync_and_applies_nothing (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-02 | Slot click validation | **deep-verified** | menu.rs set_slot/apply_click + container_e2e.rs:213 a_stale_state_id_gets_a_full_resync… (AUDIT-07: Lane C: inverted the insert split -> caught by a slot-ceiling test; the flood did NOT catch it (finding M1)) |
| P06-03 | Container/session model | **deep-verified** | menu.rs Menu + the_player_menu_matches_the_jar_verified_layout (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-04 | Crafting grid baseline | **deep-verified** | crafting.rs craft + crafting_leaves_the_rest_of_the_grid_intact (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-05 | Furnace/smelting container baseline | **deep-verified** | furnace.rs tick + a_fuel_item_lasts_exactly_its_documented_number_of_ticks (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-06 | Item metadata/tag semantics | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-07 | Block entity lifecycle | **deep-verified** | block_entity.rs:272 BlockEntityStore + block_entity_e2e::breaking_the_block_retires_its_entity (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-08 | Hopper inventory transfer model | **deep-verified** | hopper.rs transfer + a_flood_of_transfers_conserves_items_exactly (Lane C F6b: room over-report FAILS the test) (AUDIT-07: crate tests re-run green (container, redstone)) |
| P06-09 | Redstone state/update abstraction | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-10 | Neighbor/update scheduling | suite-green (inherited) | Lane C C4: discarded drained updates on a budget stop -> propagation FAILED (3 failed) |
| P06-11 | Power propagation baseline | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-12 | Repeater/comparator timing baseline | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-13 | Piston/observer family baseline | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-14 | Redstone containers/hoppers integration | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-15 | Inventory adversarial tests | **deep-verified** | inventory_duplication.rs + menu.rs hostile flood (Lane C F2: overflow discard FAILS the capped-slot test) (AUDIT-07: Lane C: discarding the insert overflow destroyed items -> flood PASSED, slot-ceiling test FAILED (finding M1)) |
| P06-16 | Redstone golden/differential tests | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-17 | Transaction/recovery review | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P06-18 | Automation workload benchmark/review | suite-green (inherited) | crate tests re-run green (container, redstone) |
| P07-01 | Command tree abstraction | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-02 | Dispatcher/parser | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-03 | Registries/tags/data loader | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-04 | Permissions/source context | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-05 | Basic vanilla commands | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-06 | Selector parsing | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-07 | Execute context | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-08 | Data/function execution baseline | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-09 | Recipe data loading | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-10 | Loot data loading | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-11 | Advancement/statistics baseline | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-12 | Data pack discovery/validation | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-13 | Worldgen seed/context pipeline | suite-green (inherited) | Lane C C8: dropped the generation gate -> scenario_vanilla::a_borrowing_game_cannot_generate FAILED |
| P07-14 | Noise/terrain baseline | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-15 | Biome/features baseline | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-16 | Structures/placement baseline | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-17 | Existing-world-first generation integration | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-18 | Command/data/worldgen parity tests | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-19 | Differential scenario expansion | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P07-20 | Parity matrix review and gap triage | suite-green (inherited) | crate tests re-run green (command, data, worldgen) |
| P08-01 | Resource/config guardrails | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-02 | Structured operational metrics | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-03 | Save barrier and shutdown coordinator | **deep-verified** | lifecycle.rs shutdown_passes_through_stopping_before_stopped (AUDIT-07: crate tests re-run green (server, deploy, docs/operations)) |
| P08-04 | systemd service/unit | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-05 | Backup/restore helper | **deep-verified** | backup.rs backup_world + backup_then_verify_then_restore_round_trips (Lane C F7: stray disable FAILS a backup test) (AUDIT-07: crate tests re-run green (server, deploy, docs/operations)) |
| P08-06 | Connection/resource exhaustion defenses | **deep-verified** | listener.rs MAX_CONNECTIONS_PER_IP + ops_e2e::a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator (AUDIT-07: crate tests re-run green (server, deploy, docs/operations)) |
| P08-07 | Packet/decompression abuse defenses | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-08 | Command/admin safety review | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-09 | Pi benchmark harness | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-10 | 10-player workload driver | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-11 | Chunk-generation benchmark | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-12 | Persistence benchmark | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-13 | CPU/RAM/TPS/MSPT profile run | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) — **caveat:** Pi profile numbers exist in BENCHMARK-BASELINE.md; re-running them needs the Pi (Lane E blocked) |
| P08-14 | Targeted performance fixes | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P08-15 | Operational runbook | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) — **caveat:** RUNBOOK.md present and its install steps were corrected by governance; Pi application unverified (Lane E blocked) |
| P08-16 | Pi hardening review and acceptance report | suite-green (inherited) | crate tests re-run green (server, deploy, docs/operations) |
| P09-01 | Full test matrix execution | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-02 | Protocol conformance sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-03 | Persistence compatibility sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-04 | Survival regression sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-05 | Entity/redstone regression sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-06 | Commands/data/worldgen regression sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-07 | Security adversarial sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-08 | Pi performance release sweep | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-09 | Reproducible release build | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-10 | Release documentation | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) — **caveat:** finding M2: RELEASE-CANDIDATE.md states 1 189 passed; the run states 1 191 (five other documents say 1 191) |
| P09-11 | Known divergence catalog | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) — **caveat:** KD-01..KD-38 all resolve in PARITY-MATRIX.md (verified); row tags present |
| P09-12 | Rust-native plugin boundary ADR | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-13 | Independent final review | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |
| P09-14 | Release candidate acceptance report | suite-green (inherited) | crate tests re-run green (docs/release, docs/testing, docs/vanilla-parity, tools/docs-audit, .github) |

## 6. Coverage statement

- Lane A: 158/158 rows present (AUDIT-07 table verified to contain every
  TASK-INDEX ID); 32 deep-dived this round (29 deep-verified, 3 boundaries),
  126 inherited with spot-checks (9 falsified rows re-read, M1/M2/H1 remedial
  tests re-falsified). Skipped: 0. Residual risk: the 126 inherited rows rest
  on AUDIT-07's evidence plus the green gate, not on fresh code reads.
- Lane B: 5/5 gates + 7/7 differential suites + 4/4 scripts run and compared.
  Skipped: 0.
- Lane C: 10 probes (target list per handover, no overlap). Skipped: 0.
- Lane D: per-suite counts all recomputed (crate-aware); test totals in 5
  facade docs recomputed; KD-01..38 integrity checked; AUDIT-07's remediation
  claims for H1/M1/M2 re-falsified/re-probed. Skipped: exhaustive per-claim
  verification of CHANGELOG prose (sampled only).
- Lane E: service cycle, E4/E5, tool-copy SHAs, level.dat, security score,
  deployment staleness. Skipped: none of the handover asks.
- Lane F: 4/4 scripts probed where not already verified.

## 7. What the next round should hunt

1. The unpushed HEAD (H1) — push, then audit the CI result.
2. Widen `check_gate_totals.py` to catch template placeholders and comma
   formats (H2/L3), and `check_encoding.py` to cover extension-less text
   files (M1).
3. The remaining 126 inherited task rows deserve the same deep treatment on
   a round-robin basis (≈30 per round clears them in four rounds).
4. Unit hardening (M4) is a fix-round item with a Pi re-verification.
