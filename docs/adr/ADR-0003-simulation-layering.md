# ADR-0003 — Simulation Layering: Enums over Trait Objects, and One Geometry Type

Date: 2026-09-11. Status: **Accepted** (Phase 05, extended in Phase 06).
Context: Phases 05 and 06 added entities, mobs, projectiles, block entities,
containers and redstone. Each of those is a "there are several kinds of thing"
problem, and each could have been modelled with a trait object hierarchy (which is
how Vanilla's class hierarchy and most JVM ports read) or with a closed enum.
Evidence: `crates/entity/src/entity.rs`, `crates/entity/src/mob.rs`,
`crates/container/src/container.rs`, `crates/simulation/src/phase.rs`,
`docs/phases/PHASE-05-REPORT.md`, `docs/phases/AUDIT-02-FINDINGS.md`.

## 1. Decision: closed enums, not `dyn Trait`, for entity and container kinds

`EntityBody` is an enum (`Player`, `Item`, `Mob`, `Projectile`),
`ContainerKind` is an enum (`Player`, `Generic`, `Crafting`, `Furnace`), and
`TickPhase` is an enum. None of them is a trait object.

Three reasons, in the order they mattered:

1. **Determinism (AGENTS.md §3.6).** Gameplay iterates every entity every tick. A
   `BTreeMap<EntityId, Entity>` iterates in ascending id for free; a
   `Vec<Box<dyn Entity>>` invites insertion-order or pointer-order iteration, which
   is exactly the accidental nondeterminism the contract forbids. The same argument
   applies to `Container` (a `Vec` indexed by slot) and to the update queue.
2. **No speculative extensibility (§3.4).** The set of kinds is closed and known:
   the entity kinds, the container kinds and the tick phases we implement. There is
   no plugin that can add one before P09. A trait object would be an abstraction
   justified by a requirement that does not exist yet.
3. **Borrowing.** Per-tick collision reads the world while mutating an entity. A
   concrete enum makes that a plain split borrow; `dyn` needs interior mutability
   or a query system, either of which moves the borrow checker's job into runtime
   invariants.

## 2. The cost, stated plainly

Adding a mob means adding a variant and its arms, not a new `impl`. Adding a
container behaviour means touching `ContainerKind`. **Two deliberate exceptions**
exist where a trait is the right tool, and both are boundaries rather than kinds:

- `mc_entity::mob::Rng` — a *capability* the AI needs, implemented by
  `mc_simulation::RandomSource`. A kind-based enum would have forced `mc-entity` to
  depend on `mc-simulation`, which is the layer above it (a cycle).
- `mc_entity::pathfind::BlockView` and `mc_redstone`'s block view — *world access*
  the algorithm needs, implemented by `mc_world::World`. Here the enum alternative
  does not exist: there is only one world type, and the trait exists so the
  algorithm can be tested against a flat array.

The rule that distinguishes them: **an enum models "which kind of thing is this";
a trait models "what can I do to the thing I am given".** Kinds are closed; the
operations a caller needs happen to be substitutable in tests.

## 3. Decision: one `Aabb`/`Vec3`, owned by `mc-world`

`mc-entity` depends on `mc-world` (one-directional; world never depends on entity).
An entity's hitbox is the same `Aabb` the world resolves movement against.

This was a **reversal** of the crate's first documented boundary, which said
`mc-entity` must not depend on `mc-world` because both define a `Vec3`. The
reversal is recorded because the original reasoning was wrong in a specific way: a
second box type plus a conversion at every call site is more error-prone than one
shared type, and the "duplicated primitive" it avoided was three fields of `f64`.
Audit 02 flagged the duplicate-type risk, and resolving it in favour of one
implementation is what the entity hitbox API now does.

The remaining duplication is `mc_entity::player::Vec3` versus `mc_world::Vec3`.
That one is retained deliberately: `player::Vec3` is persisted and serialized as
part of `playerdata`, so its shape is a *data* decision, while `mc_world::Vec3` is
a geometry decision. Entities convert at the boundary (`Entity::hitbox`) rather
than the two being unified. If that conversion ever spreads beyond one function,
this ADR should be revisited.

## 4. Decision: task-specific views rather than a general ECS

Phase 05/06 code reads the world through narrow traits (`BlockView`) and owns its
state in plain structs, rather than introducing an entity-component-system.

An ECS buys data-oriented iteration for tens of thousands of homogeneous entities.
The Phase 05 workload is 10 players and a few hundred mobs (measured in
`docs/performance/BENCHMARK-BASELINE.md`), and the entity-heavy benchmark spends
its time in the per-entity world query, not in archetype traversal. An ECS now
would be optimisation from intuition, which AGENTS.md §3.2 forbids. The measured
figure exists so this decision can be revisited with evidence rather than taste.

## 5. Consequences

- Every "add a kind" change touches a `match` the compiler checks exhaustively —
  which is the property that made the `TickPhase` order a compile-time contract
  (`PHASE_ORDER` is a `const` array, asserted by tests).
- The dependency order is `core → registry → persistence → world → entity →
  container`, with `simulation` beside `container` and `server` on top. Nothing
  below depends on anything above it.
- A future Rust-native plugin API (P09) that needs third-party *kinds* would have
  to reopen §1. That is the trigger to revisit this ADR, and it is why the trigger
  is written down.
