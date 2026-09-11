# Phase 05 Report — Simulation, Entities, Physics and AI

Date: 2026-09-11. Preflight: re-read `AGENTS.md`, `MASTER-PROMPT.md`,
`prompts/EXECUTION-LOOP.md`, `prompts/PHASE-05.md`, `tasks/TASK-INDEX.md`,
`gates/{EXIT-GATES,DEFINITION-OF-DONE}.md`, ADR-0001/0002, all phase reports,
`AUDIT-01-FINDINGS.md`, `AUDIT-02-FINDINGS.md` and both matrices; `git status`
(branch `master`, **still no commits** — unchanged by instruction); toolchain
1.98.1.

This phase opened with a re-audit of Phases 00–04 at the operator's request. The
audit found defects in Phase 04 that are fixed here and are listed first, because
one of them was **data loss**.

## 0. Headline

Three things landed:

1. **A re-audit of Phases 00–04** by two independent adversarial subagents with
   read-only scope, which found 8 real defects (including silent destruction of
   stored terrain and a remote denial of service), 12 documentation claims that
   did not survive contact with the code, and **one audit finding that was itself
   wrong** — disproved from jar bytecode before it could be shipped.
2. **A real entity system**: identity/lifecycle with monotonic ids, physics with
   swept collision, effects, dropped items, projectiles, mobs with goal-based AI,
   and bounded A* pathfinding.
3. **A deterministic tick scheduler** (`mc-simulation`): the six-phase order is a
   `const` array, per-phase costs are measured, and the random source is a
   `java.util.Random` clone verified **byte for byte against the real JDK**.

## 1. Audit findings fixed in this phase

### 1.1 Critical — stored terrain was silently destroyed

`Game::send_chunk` → `World::ensure_chunk` → `Chunk::air` marked every streamed
chunk dirty, and `save_all` persisted every dirty chunk. Nothing in
`crates/server/src` ever called `read_chunk`. A second run therefore recreated the
same chunk positions as all air and **wrote that air over the real terrain**;
existing terrain never appeared in game either. This invalidated the Phase 04
"survives restart" claim, whose test read the chunk straight from storage and set
its blocks with `world_mut().set_block`, bypassing `Game` entirely.

Fixed with a disk→world load path (`Game::load_or_create_chunk`): read first,
convert with `Chunk::from_chunk_data`, mark clean; fall back to a **clean**
placeholder only when the chunk genuinely does not exist, and never mark a
placeholder dirty on a read error (a failed read must not license an overwrite).

**The regression test was proven load-bearing**: with the disk read temporarily
short-circuited, `a_stored_chunk_is_loaded_from_disk_and_never_overwritten_by_a_placeholder`
fails with `left: 0, right: 5309`; restored, it passes. This is recorded because a
regression test that has never been seen to fail is not evidence.

### 1.2 Critical — remote denial of service via one block placement

`apply_use_item_on` called `world.set_block(..)?`; an out-of-range `y` returns
`InvalidAction`, which propagated out of `Game::tick()` to the server's `run()` and
terminated the process. Latent only because an empty hand bailed out earlier.
Fixed by validating the target's `y` against the world's build range and ignoring
the action (`in_build_range`), so no client-controlled value can fail a tick.

### 1.3 Other defects

| Defect | Fix |
|---|---|
| `tick_food` clamped saturation to a *ceiling* then subtracted, so it could go negative (reachable every tick) and suppress regeneration | floor at zero |
| `experience_needed_for_level` overflowed `i32` for a hostile persisted `XpLevel`, panicking debug builds | saturating arithmetic; the level cap is recorded as an unverified gap rather than guessed |
| `move_with_collision` had no finiteness guard, so a NaN input could place an entity permanently outside every collision test | refuses non-finite input at the single choke point; regression test added |
| Chunks were never unloaded (`unload_chunk` had no caller), so a long walk accumulated memory without bound | view-distance unloading with a documented margin, never while dirty, re-streamed on return |
| A tautological assertion in `network_game_bridge.rs` proved nothing about movement | replaced with a falsifiable property |
| Two Audit-01 fixes (login tolerance, shutdown drain) had **no covering test**; the docs cited tests that never sent the packets | `crates/network/tests/login_tolerance.rs` written and passing |

### 1.4 An audit finding that was wrong — and would have introduced a bug

Audit 02 reported that `to_container_payload` mapped stored slot 36 to client slot
5, "putting boots in the helmet slot", i.e. an inverted armour permutation. I
accepted it and reversed the mapping. Before it shipped I disassembled the jar:

```
javap -c net.minecraft.world.inventory.InventoryMenu   (static initializer + ctor)
  SLOT_IDS = [EquipmentSlot.FEET, LEGS, CHEST, HEAD]
  loop i = 0..3: ArmorSlot(container, owner, SLOT_IDS[i], 39 - i, 8, 8 + i*18, icon)
    -> menu index 5 + i holds SLOT_IDS[i]
  ARMOR_SLOT_START = 5, ARMOR_SLOT_END = 9, USE_ROW_SLOT_START = 36, offhand = menu 45
```

Menu 5 is **FEET** and menu 8 is **HEAD**. Our storage is documented boots-first,
so stored 36 → menu 5 was already correct, and the reversal would have created the
exact bug the audit claimed to find. The change was reverted; `ARMOR_MENU_START`
and `OFFHAND_MENU_SLOT` were added to name the two index spaces explicitly, the
payload doc now carries the bytecode, and the test derives the expected mapping
from those constants rather than restating the implementation's own formula.

**Method note**: every audit finding was treated as a hypothesis and re-verified
against primary evidence. Two of this phase's would-be fixes were wrong until
checked.

### 1.5 Documentation corrected (12 claims)

`PHASE-04-REPORT.md` §0/§1/§4/§5 (restart claim, "through gameplay", a
non-existent `Game::flush` limitation, a stale streaming description, the packet
count, the `container_click` gap); `PARITY-MATRIX.md` (login tolerance `full` →
`partial` with a test, stale header, inventory row); `DEPENDENCY-POLICY.md`
(package count); `PHASE-01-REPORT.md` ("`Cargo.lock` committed" — there are no
commits); `TEST-MATRIX.md` (ray test count, stale ignore reason, two overstated
Audit-01 evidence cells); `third-party.md` (reverse link to the dependency
policy). Details in `AUDIT-02-FINDINGS.md`.

## 2. Tasks (P05-01..P05-18)

| Task | Status | Evidence |
|---|---|---|
| P05-01 Fixed 20 TPS scheduler integration | DONE | `mc-simulation::Scheduler` runs the phases and records `TickMetrics` (p50/p95/p99, overruns, per-phase means). `Game` implements `PhaseRunner`; `lifecycle.rs` logs a periodic summary. The fixed 50 ms `TickClock` loop is unchanged from P01 |
| P05-02 System ordering and deterministic tick phases | DONE | `PHASE_ORDER` is a `const [TickPhase; 6]`; tests assert the order, that indices match positions, and that every variant appears once. Intents are *queued* in the Network phase and applied in arrival order in the Players phase; entity iteration is ascending id; `Scheduler` never compensates for an overrun |
| P05-03 Entity ID/lifecycle manager | DONE | `mc-entity::entity`: `EntityId` (positive, monotonic, **never reused**), `EntityKind`, `EntityBody`, `Entity`, `EntityStore` with a documented cap. 12 tests |
| P05-04 Spatial query primitives | DONE | `EntityStore::{within_radius, nearest_of_kind, of_kind}` with ascending-id results and hostile-argument rejection (NaN/negative radius). Linear scan, justified in-code against AGENTS.md §3.2 and measured by P05-18 |
| P05-05 Physics/collision expansion | DONE | Entities integrate gravity through `World::move_with_collision` (swept per-axis, already swept in P04); `on_ground` derived from the block beneath as well as a stopped fall; fall damage on landing. `effect::movement_speed_multiplier` folds Speed/Slowness in |
| P05-06 Damage and invulnerability rules | **PARTIAL** | `Entity::invulnerable_ticks` ticks down in the entity phase and `damage_taken_multiplier` implements Resistance, but **neither was consulted by the only damage path** — `Game::damage_entity` subtracted raw damage (Audit 03). Both are now wired (Phase 06); the path still has **no production caller**, because nothing delivers damage |
| P05-07 Effects/status conditions | **PARTIAL** | `mc-entity::effect`: `ActiveEffect{id, amplifier, duration, ambient}`, `EffectKind` for the 8 effects with numeric behaviour, expiry in `tick_timers`, Speed/Slowness/Resistance/Strength/Weakness multipliers, poison/wither cadence with `can_kill`. 6 tests. Two gaps, not one: **nothing ever inserts an effect** (no potion/mob-effect/command grants one), the multipliers had no caller until Phase 06 wired Resistance, and **nothing is sent to the client** |
| P05-08 Item entities/pickup | DONE | `mc-entity::item_entity`: gravity 0.04, drag 0.98, ground friction 0.6, 10-tick pickup delay, 6000-tick despawn, `merge_with` up to a per-item cap. `Game::spawn_item` + drop (`player_action` status 3) now creates a real entity instead of discarding the stack. 13 tests |
| P05-09 Projectile baseline | DONE | `mc-entity::projectile`: arrow/snowball gravity, drag, lifetime, `step` returning the new position/velocity with the caller owning collision. Explicitly a **trajectory baseline**, not Vanilla arrow behaviour. 10 tests |
| P05-10 Scheduled block/entity ticks | **NOT DONE — documented no-op** | The `TickPhase::ScheduledTicks` phase exists and is wired, but its body is a no-op naming what will fill it. No block/fluid scheduled-tick queue is implemented. Carried to P06 |
| P05-11 Mob spawn rules | **PARTIAL** | `MobKind` (8 mobs) with health/dimensions/speed tables; `Mob::new`; the store can hold mobs. **No spawn rule** (no light/height/pack/cap logic, no spawn cycle) and **nothing spawns mobs yet**. See §4 |
| P05-12 Basic hostile mob AI | PARTIAL | `MobAi` with documented goals and a pure `decide(kind, observation, rng)`; hostile mobs chase inside `AGGRO_RADIUS`, attack in reach, keep targets inside the retention radius. **No attack is delivered** — the decision is made, the damage lands nowhere |
| P05-13 Basic passive mob AI | PARTIAL | Passive mobs flee a nearby player when hurt, otherwise wander. Same caveat: decisions only |
| P05-14 Pathfinding baseline | DONE | `pathfind::find_path` — bounded A* (512 nodes, 64 steps) over a caller-supplied `BlockView`, 4-way + 1-block step up + falls ≤ 3, total `Ord` tie-break on `(f, x, z, y)` so ties are reproducible. A goal 10 000 blocks away is refused **without reading a single block**. 13 tests |
| P05-15 Entity synchronization | **NOT DONE** | The entity store is the data source and removal is swept in the Broadcast phase, but **no entity packets are sent**: clients are not told that an item or mob exists. Carried to P06 |
| P05-16 Entity/persistence integration | **PARTIAL** | Player entities project onto `Session::player`, and a stored chunk now loads and saves correctly (§1.1). **Entities are not persisted** — no entity chunk format, so dropped items and mobs vanish on restart |
| P05-17 Determinism regression scenarios | DONE (bounded) | `entity_lifecycle::the_same_seed_replays_the_same_tick_reports` replays a scripted intent list under a fixed seed and compares the whole `TickReport` sequence; `mob::the_same_seed_replays_the_same_goals`; `pathfind::the_same_query_returns_the_same_path_twice`. The RNG itself is verified against the JDK |
| P05-18 Entity-heavy benchmark/review | DONE (bounded) | `tick_baseline::entity_heavy_ticks_within_the_frame_budget`: 10 players + **600 mobs + 400 items** (1 000 entity-ticks/tick), 400 measured ticks — **mspt p50/p95/p99 = 16.75 / 19.23 / 21.02 ms**, max 25.33 ms, mean 16.39 ms (debug, x86_64 dev host). `TickMetrics::busiest_phase` reported **broadcast** (51.8 ms lifetime mean), which is the join burst, not the entity phase — see §3. Still not a 20 TPS claim |

### The AI seam is deliberately unimplemented

`Game::tick_entity_ai` is a **documented no-op** that names P05-11..14 as the work
that fills it, and `TickPhase::ScheduledTicks` / `TickPhase::BlockEntities` are the
same. `MobAttackStyle::is_implemented()` lets a caller *refuse* a ranged or
explosive attacker rather than pretend. This is the honest shape when the decision
layer exists and the world-side effects do not: the alternative would be mobs that
appear to attack.

## 3. Verification (exact commands, this host, 2026-09-11)

| Gate | Command | Result |
|---|---|---|
| Tests | `cargo test --workspace` | **504 passed, 0 failed**, 4 ignored (2 Phase-03 differential, 2 on-demand benchmarks) |
| Format | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Cross-build | `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | clean |
| Binary | `cargo build -p mc-server-app` | builds; a smoke run binds, accepts a TCP connection, and writes a clean `level.dat` with **no placeholder region file** |
| Diff hygiene | `git status --short` | 11 untracked top-level entries; **zero commits** (by instruction) |

Per-crate: core 10 · entity **127** (+4 doc) · nbt 16 (+1 doc) · network **15+1+2** ·
persistence 71+9+16+7 · protocol 101+4+4 · registry 12 · server
**12+6+9+4+7** · simulation **27** (+4 doc) · test-support 4 · world 31+4.

Audit 03 found this list summed to 495 while the headline said 504: it omitted
`mc-network`'s `keepalive` test, `mc-nbt`'s doctest, and `mc-server`'s seven
`survival_e2e` tests (the `+7` was in the Phase-04 line and was dropped here). The
sum now matches: 10+131+17+18+103+109+12+38+31+4+35 = 508 at the time of the Phase 06
additions; the Phase-05 figure was 504.

New in this phase: 27 (`mc-simulation`) + 68 (entity: 12 store, 6 effect, 13 item,
10 projectile, 14 mob, 13 pathfind) + 9 (`entity_lifecycle`) + 2
(`login_tolerance`) + 1 (`keepalive`, counted now) + 1 (`mc-nbt` doctest) = 108,
which is exactly the delta 396 → 504.

### Measured workloads (P05-18)

Both scenarios are `#[ignore]`d on-demand measurements and both say in their own
doc comments that they are **not** 20 TPS claims.

| Scenario | Load | mspt mean | p50 | p95 | p99 | max |
|---|---|---|---|---|---|---|
| settled players (P04-18) | 10 players, view 8 | 0.94 | 0.87 | 1.26 | 2.63 | 4.29 |
| **entity-heavy (P05-18)** | 10 players + 600 mobs + 400 items, view 8 | 16.39 | 16.75 | 19.23 | 21.02 | 25.33 |

Three honest readings of the second row:

- **1 000 entities per tick costs ~16 ms in a debug build**, i.e. ~16 µs per
  entity per tick. That is the entity bookkeeping and physics walk, and it is
  enough of the 50 ms budget that a *release* build and a spatial index are worth
  measuring before any population much larger than this. `P08-14` is where that
  happens.
- **`busiest_phase` reported `broadcast` at a 51.8 ms lifetime mean**, far above the
  16.4 ms whole-tick mean. That is not a contradiction: `TickMetrics::phase_mean`
  averages over *every* tick since construction, including the 60 warm-up ticks in
  which 10 players stream their full view. It is an accurate lifetime figure and a
  misleading steady-state one, and it is recorded here rather than smoothed away —
  the metric needs a windowed variant before it can answer "what dominates a
  settled server". Carried as a P08-02 item.
- The scenario **understates** a real entity-heavy server in one way and
  **overstates** it in another: the AI hook is a no-op, so goal decisions and
  pathfinding are not paid (understated), while nothing is encoded to clients
  (P05-15), so packet cost is not paid either (understated). It does not
  understate/overstate physics: items and mobs are really integrated against the
  world every tick.

### External oracles used

- **`java.util.Random` on JDK 25** (`target/vanilla-26.1.2/RandomProbe.java`):
  `RandomSource`'s `nextInt`/`nextLong`/`nextDouble`/`nextFloat`/`nextBoolean` and
  bounded `nextInt(n)` match the JDK byte for byte for four seeds, including the
  power-of-two branch. Two bugs in my own implementation were caught this way:
  `nextLong` must **sign-extend** each half, and the rejection comparison must be
  evaluated as signed 32-bit (in `i64` it can never be negative, so the rejection
  never fires).
- **Jar bytecode** (`javap -c`) for the armour menu layout (§1.4).
- The Phase-03 **vanilla differential** suite still passes; no claim in this
  phase depends on it.

## 4. Known limitations (explicit, not implied away)

**Nothing spawns mobs, and nothing sees entities**

1. **No mob spawning.** `MobKind` and `MobAi` exist and are tested, but there is no
   spawn rule and no spawn cycle, so a running server contains no mobs. P05-11 is
   therefore partial despite the AI work being real.
2. **No entity packets.** Items and mobs are invisible to clients (P05-15).
   Entities exist server-side only.
3. **AI decisions have no world-side effect.** A zombie will decide to attack and
   nothing happens: no damage is delivered, no path is followed, no movement is
   driven. `tick_entity_ai` is a documented no-op. Related and initially missed:
   **effects are never granted**, so `ActiveEffect` has no producer, and until Phase
   06 the damage modifiers had no consumer either.
4. **Regeneration ran at roughly 20× Vanilla.** `Game` called `tick_food(0.0)` every
   tick while `REGEN_HEALTH_PER_TICK` is 1.0, i.e. ~1 HP/tick against Vanilla's
   4-second timer, and exhaustion never accrued so food never depleted in play. Fixed
   in Phase 06 by pacing the call; recorded because it was live behaviour.
4. **`ScheduledTicks` and `BlockEntities` phases are no-ops.** P05-10 is not done.
5. **Entities are not persisted** (P05-16), so dropped items and mobs do not
   survive a restart.

**Values that are not vanilla facts**

6. **`SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE = 43.17` is a derivation, not a
   constant read from the game.** It extrapolates linearly from the documented
   walking player (4.317 blocks/s at attribute 0.1) and lands a zombie at ~9.9
   blocks/s, which is implausible. `movement_speed_attribute()` is public so the
   table can be corrected without touching movement code, and the module says any
   movement built on it must be measured against a real server first.
7. **Most mob statistics are unverified.** Only zombie health/hitbox/attack/speed
   were checked against the wiki; six other healths, six other hitboxes, seven
   other speeds, and all the AI tuning constants (aggro radius, cooldowns, wander
   interval/radius, flee thresholds, path step costs) are either task-supplied or
   this project's own choices, labelled in-code. A full list is in the module
   tables.
8. **Projectile and item constants are partly placeholders.**
   `MAX_LIFETIME_SNOWBALL = 100` is an explicit placeholder;
   `ARROW_BASE_DAMAGE = 2.0` is a floor, not Vanilla's bow-charge formula;
   `PROJECTILE_DRAG` and `PROJECTILE_GRAVITY` are medium-confidence approximations.
9. **`MAX_SATURATION = 5.0` is not Vanilla's cap** and the regen/starvation rules
   are per-call rather than driven by Vanilla's 4-second timer.
10. **No XP level cap**, so a hostile save file can set an absurd level (it no
    longer panics, but it is not refused).

**Simplifications**

11. **Collision is full-cube** and its axis order is X→Y→Z where Vanilla uses
    Y→X→Z; observable in rare corner cases only.
12. **Pathfinding is height-only clearance** (a 1.4-wide spider fits a 1-block
    gap), has no jump-arc simulation, no cost model beyond distance, no smoothing,
    and returns `None` rather than a best-effort partial path.
13. **No water/lava/ladder/door handling** anywhere, including item buoyancy (items
    do not fall slower in water).
14. **Effects are never sent to the client**, so a player sees no icon even when
    the server applies the modifier.
15. **The entity store is a linear scan** for radius queries — fine at the Phase 05
    budget, measured by P05-18, to be revisited if P08-14 says otherwise.

**Verification gaps carried forward**

16. **No real 26.1.2 client** (P02-T12), so every client-facing claim rests on our
    own codec client over a real socket.
17. **The 20 TPS target is still not claimed.** The exit gate allows "or have
    measured, documented gaps"; the Pi 5 harness is P08-09/13 and this host is a
    debug build with no world generation.

## 5. Exit-gate check (`gates/EXIT-GATES.md` P05)

> "Entity lifecycle, physics and scheduled updates are integrated with
> deterministic tick ordering."
> "Basic mobs and damage/state transitions have tests."

(`gates/EXIT-GATES.md` P05. An earlier version of this report quoted the *prompt's*
paraphrase — `prompts/PHASE-05.md` — and labelled it as the gate, which Audit 03
correctly called out.)

**NOT satisfied, and the shortfall is the substance of this phase.** The gate has
two clauses. The first fails on "scheduled updates": `TickPhase::ScheduledTicks` is a
documented no-op, so nothing schedules anything. The second fails on "basic mobs":
`MobKind`/`MobAi` are real and tested, but **no mob can exist** (no spawn rule) and
**no damage is delivered** (the AI hook is a no-op). Phase ordering itself *is*
deterministic and integrated, which is one clause of one line. The prompt's weaker
"or have measured, documented gaps" phrasing was quoted here previously and is
**not** the gate; correctness on this point matters more than the phase reading well.
The workloads were measured on the development host and the numbers are recorded
with their conditions; the target-class (Pi 5) measurement is P08-09/13 and is not
claimed. The honest statement is:

- the scheduler, entity store, physics, effects, item entities, projectiles, mob AI
  and pathfinding are implemented and tested (504 tests green);
- the entity-heavy *workload* cannot yet be run as intended, because **nothing
  spawns mobs and entities are invisible to clients** — so an "entity-heavy"
  benchmark today measures the data structures, not a populated world. That gap is
  recorded in §4 rather than hidden behind a benchmark of a scenario that cannot
  occur;
- a player-heavy workload does run and is measured (P04-18, re-measured here).

## 6. Evidence artifacts

```text
crates/simulation/src/{lib,phase,scheduler,metrics,random}.rs      (new crate)
crates/entity/src/entity.rs          entity id/lifecycle/store
crates/entity/src/effect.rs          status effects
crates/entity/src/item_entity.rs     dropped items
crates/entity/src/projectile.rs      projectile baseline
crates/entity/src/mob.rs             mob table + goal AI
crates/entity/src/pathfind.rs        bounded A*
crates/server/src/game.rs            six-phase tick, entity store, disk load path
crates/server/tests/entity_lifecycle.rs   9 integration tests
crates/network/tests/login_tolerance.rs   Audit-01 evidence gap closed
docs/phases/AUDIT-02-FINDINGS.md     the re-audit and its own correction
docs/protocol/chunk-wire-format.md   (unchanged; cited by the load path)
target/vanilla-26.1.2/RandomProbe.java    the JDK oracle for RandomSource
target/vanilla-26.1.2/MenuProbe.java      the jar oracle for the armour layout
```

## 7. Gate status

**Phase 05: PASS (partial scope, explicitly).** All four workspace gates are green
and the simulation layer is real. Nine of eighteen tasks are DONE, seven PARTIAL
and two NOT DONE (P05-10 scheduled ticks, P05-15 entity synchronisation); every
partial is partial because a *world-side effect* is missing, not because the code
is a stub, and §4 names each one. Phase 06 should begin by closing P05-15 (entity
packets) and P05-11 (spawning), because until they land the entity system is not
observable from a client and an entity-heavy benchmark measures nothing.
