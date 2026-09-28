# Atomic Task DAG

**Current phase: P18** (v0.3.0 round closer). This header is the single owner
of "which phase is current" — README, MASTER-PROMPT and EXECUTION-LOOP point
here instead of restating it. Update it at every phase transition.

Current baseline (2026-09-23, `main @ 081d98f`, post-AUDIT-17 P0/P1
remediation): P00-P14 completed and archived in `../prompts/legacy/`; P15-P17
completed (outcomes in the tables below); P18 re-scoped and current; P19-P22
planned as v0.4.0. Rationale for the re-scope and the v0.4.0 shape:
`../PLANNING-REVIEW.md`. The tables are a task DAG; live outcomes live in the
Status column and in CHANGELOG / linked matrices / reviews. Task IDs and
recorded outcomes are preserved; missing evidence is not invented.

Open-task rows carry an **Acceptance** column: the externally checkable
outcome and the pin it needs (DoD item 13). The phase prompt holds scope; the
row holds the bar. A row whose acceptance cannot be stated is not ready to
start — write it first.

Scheduling: phase numbers group delivery goals, not a strict execution barrier.
A task may run ahead when its dependencies are satisfied. Phase completion
requires all applicable task evidence and the exit gate.

Legend: `Pxx-NN` = phase/task. A task is independently shippable only when its dependencies and universal DoD are satisfied.

## Completed phases (P00–P14) — archived at the v0.2.0 closeout

P00–P14 are DONE. Their detail tables lived here until the closeout; the
frozen per-phase prompts now live in `../prompts/legacy/PHASE-00.md` …
`../prompts/legacy/PHASE-14.md`. Status per phase (evidence pointers):

| Phase | Status | Evidence |
|---|---|---|
| P00 Research | DONE | docs/research/*, ADR-0001, docs/legal/third-party.md |
| P01 Foundation | DONE | workspace, toolchain 1.98.1 pin, CI, quality gates |
| P02 Protocol | DONE | packet-ids-775.tsv, hostile-input tests, login/play E2E |
| P03 Persistence | DONE | NBT, Anvil region, atomic save, corruption recovery |
| P04 Survival Vertical Slice | DONE | survival_e2e, swept collision, death/respawn |
| P05 Simulation/Entities | DONE | 20 TPS scheduler, PHASE_ORDER, spawn, AI wiring |
| P06 Inventory/Redstone (early) | DONE (gaps declared) | container conservation, redstone baselines |
| P07 Commands/Data/Worldgen | DONE (gaps declared) | 15 commands, 758 tags, 1421 recipes, worldgen |
| P08 Pi/Ops | DONE | systemd unit, RUNBOOK, pi-bench, P09-Pi soak |
| P09 Release/Conformance | DONE | release.yml, tag v0.1.0-rc.1, PARITY-MATRIX KD-01..KD-39 |
| P10 Client Compatibility & Rendering | DONE (re-acceptance via P14-10) | capture rig, light engine, entity wire |
| P11 Living World | DONE (re-acceptance via P14-10) | spawn, AI, loot authority, entity persistence |
| P12 Containers & the Survival Loop | DONE (re-acceptance via P14-10) | chest/furnace/hopper, crafting, BE persist |
| P13 World Systems & Redstone | DONE | tick-driven model, 15-block wire, conductivity 36/36 |
| P14 Usability & the 0.2.0 Milestone | DONE (tag v0.2.0) | CHANGELOG [0.2.0], BENCHMARK P14-Pi, 11 walk fixes |

Carry-forward gaps stay owned by the current phase row that names them
(P18-07 residuals; P16/P17 screen-only items). Historical P04–P14 items
(creeper fuse, dig progress, crafting-table window, chest post-restart
render, pistons, terrain richness, NVMe soak) were re-accepted or moved —
do not re-open them here without a named current-phase ID.

Historical scheduling note: phase numbers grouped delivery goals, not a strict
execution barrier (P07-03 once ran ahead of P06-06). Both done; the rule below
applies to P15+ only.

Do not replay archived phases. New work starts at **P18**; re-acceptance of
old behavior goes through current-phase tasks only.

## Phase 15 — Observability + Core Hardening (v0.3.0 round opener) — **DONE**
| ID | Task | Depends | Status |
|---|---|---|---|
| P15-01 | Worst-phase logging on overrun windows (warn + `tick metrics` row) + synthetic-window unit test | — | DONE |
| P15-02 | INFO-level 30-minute re-soak of the P14-Pi workload; spike-reproduction verdict | P15-01 | DONE |
| P15-03 | Split `game.rs` → session/tick/persist; zero behavior change; module probes re-run | P14-07 | DONE |
| P15-04 | Split `play.rs` → chunk/light/inventory/position modules; zero behavior change | P15-03 | DONE |
| P15-05 | Sink `packing` into mc-core/nbt (delete protocol→persistence edge); move `RandomSource` into mc-core | P15-03 | DONE |
| P15-06 | Hot-path `expect` → typed errors; `thiserror` unification; single `Vec3` | P15-03 | DONE |
| P15-07 | B-02 write-sequence assertion + D-07 hetero-list refusal + A-03 capture-sweep harness + E-02 fixture-env subprocess test | P15-05 | DONE |
| P15-08 | Hardening review: per-step gates, probe re-runs, zero-observable-delta sign-off | P15-04, P15-06, P15-07 | DONE |

Evidence: CHANGELOG P15-01..P15-08 (`11c4f2e` hardening sign-off). Exit gate `gates/EXIT-GATES.md` §P15.

## Phase 16 — Combat & the Survival Loop (v0.3.0 gameplay I) — **DONE**
| ID | Task | Depends | Status |
|---|---|---|---|
| P16-01 | Damage model: held-item damage, armor, damage types, knockback, `AttackRange` (jar-measured) | P15-08 | DONE |
| P16-02 | XP orbs: new entity kind, wire spawn/pickup, levels, death scatter | P16-01 | DONE |
| P16-03 | Status effects with sources + `update_mob_effect` encoding to the client | P16-01 | DONE |
| P16-04 | Mob-AI wiring: A* chase hookup, line of sight, per-kind follow ranges, skeleton bows, creeper fuse/explosions | P15-08 | DONE |
| P16-05 | Digging progress: hardness, tool-speed multipliers, per-tick accumulation, abort on stop | P15-08 | DONE |
| P16-06 | Step-up + non-full-cube collision for stairs/slabs/fences (KD-11) | P15-08 | DONE |
| P16-07 | Real-client night fight (knockback felt, XP gained, effect on screen) + review | P16-01..P16-06 | DONE (named gaps: on-screen dig cracks / stair-slab-fence steps scripted-only — effect modifiers closed by AUDIT-17 P0) |

Evidence: CHANGELOG P16-01..P16-07; `docs/testing/P16-REVIEW.md`; night-fight verdict (`208ae71`). Exit gate §P16 closes on the three named criteria.

## Phase 17 — World Interaction (v0.3.0 gameplay II) — **DONE**
| ID | Task | Depends | Status |
|---|---|---|---|
| P17-01 | Doors/trapdoors, observers, dispensers/droppers as reactive components (pistons deferred with written reason) | P16-07 | DONE |
| P17-02 | Containers round-up: double chests, barrel fix, hopper→furnace routing; crafting-table 3×3 window; inventory-grid sync robustness; chest post-restart render; real-client session retires the P12-10 debt | P16-07 | DONE |
| P17-03 | Crafting tag-ingredient resolution + next recipe kinds | P16-07 | DONE (simple transmute; complex transmute / dye / imbue / smithing named gaps) |
| P17-04 | Observer/dispenser differentials against the vanilla server | P17-01 | DONE |
| P17-05 | Real-client build session (timed mining, stairs, observer clock, double chest) + review | P17-01..P17-04 | DONE after AUDIT-17 P0+P1 (named gaps: chest `block_event` animation; remaining weak owner pins) |

Evidence: CHANGELOG P17-01..P17-05; `docs/testing/P17-REVIEW.md`; `docs/audits/AUDIT-17.md` + P0 remediation (`fc0ab52`) + P1/P2 pins (`06aca2a`, `abba3f4`, `081d98f`); `docs/testing/P17-BUILD-SESSION.md` (owner verdict 2026-09-23). Exit gate §P17 satisfied with named opens. P12-10 retired. Pins: `doors`, `hopper_furnace`, `crafting_table`, `p17_owner_pins`.

Status history (kept, not rewritten): P17 was first signed while AUDIT-17
measured fmt/clippy red and `doors`/`hopper_furnace` failing on the signed
commit; the DONE above is the re-sign after the P0 queue landed. Residual
AUDIT-17 IDs are owned by P18-07. P17 named opens at re-sign: chest
`block_event` animation only — packed_xz one-byte is pinned
(`chunk_block_entity_packed_xz_is_one_byte`); weak owner pins live in
P18-07's residual list, not as a second open set.

## Phase 18 — Items, Food, Commands & Terrain (v0.3.0 round closer) — **CURRENT**

Re-scoped 2026-09-23 (`../PLANNING-REVIEW.md` R1–R3): food moved up from
v0.4.0 because hunger is inert; lakes moved to P20 (need fluids);
multi-chunk structures moved to P21-00 (need the terrain-model ADR); gate
health and AUDIT-17 residuals added as the entry task.

| ID | Task | Depends | Acceptance |
|---|---|---|---|
| P18-07 | Gate health + AUDIT-17 residuals: quick gate inside a recorded budget **and** the matching CI workspace-test duration on the same tree; A12-06/07, A12-03, P15-03 zero-delta, c2s 43/56/19 capture corpus, TEST-MATRIX named closed set; **verify** B-02 naming already closed (`commits_to_a_fitting_payload`) | P17-05 | `run.py --quick` completes and passes inside the budget on the dev host (time recorded) **and** the commit's CI run is green (run id in the status line); every listed ID closed with a pin **or** carried forward by name with an owner-visible reason; B-02 naming either already closed (cite the commit) or reopened with the missing pin |
| P18-01a | Item components — wire/disk: `damage`/`max_damage`, `enchantments` + `stored_enchantments`, `custom_name`, `repair_cost`, `attack_range`, `food`/`consumable`; unknown-component preservation | P18-07 | Components round-trip wire (golden bytes from a vanilla capture) and disk (playerdata, chunk entity, BE) byte-stable; a stack holding an unknown component survives save→load byte-identical (named red on drop); **vanilla-boot interop is NOT RUN until instrumented** — when run, a vanilla server boots our saved items and keeps them (never substitute self round-trip) |
| P18-01b | Item components — wear and four enchantment effects (Efficiency, Sharpness, Protection, Unbreaking) | P18-01a | Wear per action jar-sourced (dig/attack/hit) with break at max + break event; each of the four effects changes a named behaviour test; every inert enchantment named in PARITY; perturbation: zeroing the wear step turns a named wear-matrix test red |
| P18-06 | Food & hunger: jar exhaustion table, `UseItem` eat cycle from `food`/`consumable`, saturation fast-regen, difficulty starvation floors | P18-01a | Sprinting N blocks drains the jar-computed exhaustion (test through `Game`, not the helper); eating bread restores 5/6.0 after `consume_seconds`; releasing early restores nothing **and** that cancel is its own named red; the saturation fast-regen branch has its own named red; perturbation: passing `0.0` exhaustion again turns a named test red |
| P18-02 | Commands (closed list in PHASE-18) + selectors `@a/@p/@s/@e/@r` with 8 argument families + `execute rotated/facing/anchored` | P18-07, P18-01a | Each command has a permission test and an argument-refusal test; `enchant` waits for P18-01b effects (store-only is enough to land the command); selector sort/limit semantics diff-checked against the vanilla server via `execute` output (fixed seed + normalized ordering; compared N / skipped M); `fill` volume cap enforced with a named red; KD-31/KD-32 counts updated from the dispatcher, not by hand |
| P18-03 | Terrain: pack-driven overworld ore features + cave/canyon carvers (no water fill) | P18-07 | Per-Y-band ore counts and carved-air fraction over a 32×32-chunk region within a stated tolerance of a vanilla region of the same seed (tolerance number written before the run); generation cost per chunk measured on the Pi; KD-30 row updated |
| P18-04 | Pi soak on the gameplay-heavy tree; §13 record | P18-07, P18-01b, P18-02, P18-03, P18-06 | Workload asserts it ran wear, eating, combat, the observer clock, hopper→furnace and new-terrain generation; verdict per the standing rule; worst-phase attribution for every overrun window |
| P18-05 | v0.3.0 verdict + tag under the P14-07 rules | P18-04 | Real-client walk evidences the PHASE-18 exit items (wear/break, enchant glint+tooltip, hunger from sprint and bread, ore vein in a cave); tag commit, artifacts, green CI (run id) verified; CHANGELOG / PARITY / TEST-MATRIX agree |

## v0.4.0 "operable survival" — P19–P22 (planned)

Theme: a small community can run a persistent survival world on a Pi 5 for
weeks, beyond the owner's desk, and the operator can admin, back up and
upgrade it without a developer. Scope/rationale per phase in
`../prompts/PHASE-19.md` … `PHASE-22.md`; the production-gap analysis in
`../PLANNING-REVIEW.md` §2. Rows may be refined after P18 tags; IDs are stable.

### Phase 19 — Access Control & the Operator Surface
| ID | Task | Depends | Acceptance |
|---|---|---|---|
| P19-01 | `whitelist.json` + `/whitelist` + enforce semantics | P18-05 | Non-listed client refused at login with vanilla's key; a vanilla server boots on our file and keeps entries; operator exemption per the jar |
| P19-02 | Bans (`banned-players/ips.json`), `/ban`, `/ban-ip`, `/pardon(-ip)`, `/banlist`, `/kick` | P18-05 | Banned player/IP refused before a slot is taken; banning a live player disconnects them; expiry honoured (clock-injected test); file differential as P19-01 |
| P19-03 | Console stdin dispatcher; `save-all [flush]`, `save-off`, `save-on` | P18-05 | Console commands run at console level; stdin EOF does not stop the server; `save-off` holds all region writes (asserted), `save-all flush` returns only after data is on disk |
| P19-04 | RCON (off by default, loopback default, password required) | P19-03 | Stock RCON client runs `list`/`stop`; hostile-input suite (length, flood, bad auth throttle) green; refuses to enable without a password |
| P19-05 | Online-mode auth: ADR + deps, encryption, `hasJoined`, profile properties (closes KD-01) | P18-05 | **Split pins:** (a) encryption handshake + `hasJoined` timeout/refusal vectors pinned against the jar's cipher — automated, required to close the row; (b) a real account joins with its skin — owner-run, may be named NOT RUN on the row without blocking P19-07 if (a) is green; ADR accepted with license rows and Pi CPU cost |
| P19-06 | Properties: spawn protection, pvp, idle timeout, `simulation_distance` (**config + persistence only**; ticking honours it in P20), default gamemode, hide-online-players; exposure warning | P18-05 | Each key has a read/write + default test through `Game` config; behaviour that needs world ticking is named as P20-owned, not stubbed here; unknown keys still refused; non-loopback + offline + no whitelist logs the warning once |
| P19-07 | Adversarial review + real-client access session | P19-01..P19-06 | PHASE-19 session items evidenced; review table per REVIEW-AGENT; KD-01 moves only with evidence |

### Phase 20 — The Living World
| ID | Task | Depends | Acceptance |
|---|---|---|---|
| P20-00 | ADR: fluid/random-tick placement in `PHASE_ORDER`, budgets, simulation radius, Pi cost estimate | P19-06 | ADR accepted; estimate stated as a number P20-07 checks; **interface note for P21-00** (budgets/radius) recorded — P21-00 does not depend on this ADR |
| P20-01 | Fluids core: flow/levels/falling, delays, conversions, waterlogging, buckets, drowning, lava damage | P20-00 | The **frozen** five scenarios in PHASE-20 (spring, falling column, lava+water, waterlogged stairs, bucket placement) equal cell-by-cell after the frozen N ticks; compared 5 / skipped 0 |
| P20-01b | Lakes + water-filled carvers in generated terrain | P20-01, P18-03 | Lakes appear in generated terrain (statistical presence check vs vanilla same seed); a dry carver stays dry without P20-01b |
| P20-02 | Random ticks + crops, farmland, saplings, grass/mycelium, leaf decay, cane/cactus, bone meal | P20-00 | Sampling rate per section jar-sourced and pinned; growth probability tested statistically with a seeded source (named red if rate is zeroed); leaf decay distance differential |
| P20-03 | Weather cycle, packets, `/weather`, persistence, lightning | P20-00 | Durations jar-sourced; rain level packets observed by a real client; weather survives restart |
| P20-04 | Beds, sleep rules, skip-night, respawn point, `/spawnpoint` | P20-03 | Refusal reasons each tested; skip-night respects the percentage rule with 10 sessions; obstructed bed → world spawn + message |
| P20-05 | Game rules: storage (jar-read names/location), `/gamerule`, wiring of every mechanised rule | P20-02, P20-04 | Rules round-trip with a vanilla server; each wired rule has a behaviour test; inert rules listed in PARITY |
| P20-06 | Animals: breeding, babies, tempt, shearing/regrowth, milk, eggs | P18-01a, P20-00 | Breed → baby → adult cycle through `Game`; per-kind food tag read from the pack; milk clears P16-03 effects |
| P20-07 | Pi soak with farm + fluid workload | P20-01, P20-01b, P20-02..P20-06 | §13 record; fluid and random-tick phase costs reported separately vs the P20-00 estimate; verdict per the standing rule |
| P20-08 | Real-client "farm day" + review | P20-07 | PHASE-20 session items evidenced; review table |

### Phase 21 — Dimensions & Progression
| ID | Task | Depends | Acceptance |
|---|---|---|---|
| P21-00a | ADR: multi-dimension runtime (ownership, tick order, streaming, respawn sequence) | P18-05 | ADR accepted with a Pi cost measurement; aligns with P20-00 budgets/radius by **interface**, not by blocking on P20 |
| P21-00b | ADR: terrain model (Perlin vs pack density functions) + multi-chunk structures sequencing | P18-03 | ADR accepted with Pi cost + parity estimate per terrain option; structures sequenced after the model choice |
| P21-01 | Multi-dimension runtime, dimension-change sequence, persisted dimension, `execute in` | P21-00a | Change sequence golden from a vanilla capture; a player saved in the Nether reloads there; entities stay in their dimension across restart |
| P21-02 | Nether generation per P21-00b (terrain, roof/floor, lava sea, biomes, ores) | P21-01, P21-00b | Distribution check vs vanilla of the same seed (as P18-03); chunk cost on the Pi |
| P21-03 | Portals: frames, ignition, collapse, delay/cooldown, 8:1, search/creation, POI | P21-01 | Seeded link cases equal to the vanilla server's destinations; POI records readable by vanilla |
| P21-04 | Zombified piglin + ghast with spawn rules | P21-02 | Group-anger and fireball behaviour tests; spawn rules from the pack |
| P21-05 | Enchanting table + anvil + 7 more enchantment effects | P18-01b, P21-00a | Offers equal the jar's algorithm for fixed seeds/bookshelf counts (differential); anvil costs and "too expensive" pinned; may start after P18-01b without waiting for Nether |
| P21-06 | Smoker, blast furnace, stonecutter, smithing table | P18-01a | Each window transacts with conservation tests; recipe kinds loaded from the pack; KD-25 counts updated |
| P21-07 | Real-client "into the Nether" + review | P21-01..P21-06 | PHASE-21 session items evidenced; review table |

### Phase 22 — Production Hardening & v0.4.0
| ID | Task | Depends | Acceptance |
|---|---|---|---|
| P22-01 | `backup/restore/verify/--check-config` CLI; online backup; release format stamp + upgrade rule | P19-03 | Round-trip backup→restore→boot test; restoring onto a running world refused; a world from v0.3.0 opens under the documented rule (closes KD-37) |
| P22-02 | CPU/RSS telemetry, tick watchdog, optional metrics endpoint (ADR) | P18-05 | A synthetic stall trips the watchdog with the phase named; CPU/RSS fields match `/proc` in a Linux test |
| P22-03 | Fuzz/property harness for every hostile parser, CI budget | P19-01, P19-02, P19-04, P19-05 | All targets listed in PHASE-22 exist, run in CI within budget; every crash found becomes a regression test |
| P22-04 | 24-hour NVMe Pi soak + `kill -9` crash tests | P20-07, P21-07 | §13 record with RSS slope and save latency; world + playerdata verified after each crash point; NVMe boundary in AGENTS.md §2 closed; **no NVMe Pi → NOT RUN and P22-08 cannot sign unconditionally** |
| P22-05 | Threat-model update, `cargo deny advisories`, SECURITY process check | P19-07 | Threat model covers online mode/RCON/whitelist; advisories gate green in CI |
| P22-06 | RUNBOOK for Internet deploy, vanilla-world migration, upgrade notes | P22-01, P22-05 | A second person follows the runbook on a clean Pi without developer help (recorded) |
| P22-07 | AUDIT-22 (P18–P22), P0 queue fixed and pinned | P22-01..P22-06 | Audit published in AUDIT-17 format; every P0 closed with a pin before P22-08 |
| P22-08 | v0.4.0 walk (≥ 2 humans, full survival loop), verdict, tag | P22-07 | PHASE-22 walk evidenced clause by clause; P14-07 publication rules; any NOT RUN (e.g. P22-04 NVMe, P19-05 owner online-join) listed and blocks an unconditional verdict |

## Beyond v0.4.0 — queued, not planned (IDs reserved)

IDs are reserved so audits stop filing these as untracked gaps. No task
detail is written until v0.4.0 tags; the list is unordered.

| ID | Item |
|---|---|
| P23-01 | Pistons + quasi-connectivity |
| P23-02 | Redstone update order and delays (KD-13) |
| P23-03 | Comparator/repeater output facing + locking |
| P23-04 | Torch delay / burn-out |
| P23-05 | The End and strongholds |
| P23-06 | Fortresses / bastions |
| P23-07 | Villagers and trading |
| P23-08 | Brewing |
| P23-09 | Advancements and statistics firing |
| P23-10 | Signed chat (KD-02) |
| P23-11 | Boats and minecarts |
| P23-12 | Fire spread |
| P23-13 | Proxy forwarding |
| P23-14 | Rust-native plugin API |
| P23-15 | Fence-gate swing condition + button wall facing (Pumpkin/jar oracle) |
| P23-16 | Chest `block_event` open/close animation |
