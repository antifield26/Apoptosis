# ADR-0005 — Rust-Native Plugin Boundary: Three Named Seams, Zero API Types

Date: 2026-09-12. Status: **Accepted** (Phase 09, P09-12).
Context: `AGENTS.md` §2 reserves a "Rust-native plugin API" for a later phase;
ADR-0001 D-07 fixed the shape in advance — *boundary, not abstraction*: no
plugin API types in core, a conceptual seam only, each part introduced "only
when exercised by real code". MASTER-PROMPT §7 forbids interfaces that exist
solely to reserve space. This ADR is the promised P09-12 record: it names the
seams, pins them to the code that would grow them, and states what would have
to be true before any of them becomes an API.
Evidence: `crates/simulation/src/phase.rs`, `crates/server/src/commands.rs`,
`crates/server/src/functions.rs`; a workspace grep for `plugin` finds five
doc-comment mentions and **no type, trait or module** named for plugins.

## 1. Decision: the boundary is three named seams, not an API

A future plugin would need exactly three things the server already has one
in-tree caller for. Each is listed with its current code site and the trigger
condition that must be met before it becomes a public extension point.

| Seam | Where it lives today | Who exercises it today | Trigger for an API |
|---|---|---|---|
| **Tick events** (work at phase boundaries) | `TickPhase`/`PHASE_ORDER`, a `const` array asserted by tests (`phase.rs`) | the six phases themselves; the scheduler runs the array | a second in-tree consumer that must run at a phase boundary without belonging to a phase (first candidate: P06's scheduled-tick/redstone wiring if it lands as work *between* phases rather than inside one) |
| **Command registration** (adding nodes to the dispatch tree) | `build_command_tree()`, one function that returns the whole tree (`server/src/commands.rs`) | nine nodes — the eight Vanilla commands of KD-31 plus `/function`, which the matrices count under the datapack seam — all from one place | a second tree contributor — e.g. commands behind a feature flag or a chunk of the Vanilla set grown by data rather than code |
| **Datapack-function hook** (a `.mcfunction` line reaching something that is not a dispatcher command) | `run_function()` (`server/src/functions.rs`), which routes every line through `dispatch_command` under the invoker's permission level | `/function` and `execute (if function)` via the dispatcher | a first non-command function target, which must pass through the same permission check — a function must never reach more than its invoker could |

Until each trigger fires, the seams stay what they are: a `const` phase array,
one tree-building function, one function-runner loop. None gains a trait, a
callback parameter or a registry "so plugins could hook in later".

## 2. Decision: what is rejected, and why

Rejected now, and re-rejectable only with a named second consumer:

- **A `Plugin` trait / handler registry / event-bus type in core.** Every one of
  these has one hypothetical consumer and zero real ones — the exact
  "speculative abstraction" AGENTS.md §3.4 and MASTER-PROMPT §7 forbid. The
  three seams are call sites; a call site costs nothing to leave alone.
- **`dyn`-anything at the tick boundary.** The tick is the determinism
  contract (ADR-0001 D-02/D-06): a fixed phase order, ascending entity ids,
  arrival-ordered intents. A dynamic handler list would make tick cost and
  ordering depend on registration order — a correctness surface before it is a
  feature.
- **Async plugin tasks.** Tokio stays at the I/O edges (D-02). A plugin API
  that let gameplay work escape the tick would reintroduce the concurrency
  hazards the architecture exists to prevent.
- **A `mc-plugin-api` crate now.** §7 of AGENTS.md: a crate exists when it owns
  real behaviour. When the first trigger fires, the API types should live in
  such a crate — until then the boundary is this document.

## 3. Constraints any future API inherits (non-negotiable, already enforced in-tree)

1. **The error contract applies.** A misbehaving extension is treated like a
   hostile client: it may fail its own work, it may not panic the tick or the
   process (AGENTS.md §9).
2. **Permissions are enforced at dispatch, not at trust.** Whatever a plugin
   registers joins the tree below `PermissionLevel` checks (P07-04); a function
   hook inherits the invoker's level by construction (`run_function`).
3. **Deterministic ordering.** Registration order is the execution order, and
   it is stable and testable — the same property `PHASE_ORDER` and the
   ascending-id entity iteration already guarantee.
4. **No `unsafe`.** `#![forbid(unsafe_code)]` holds at the app boundary; an
   API that required `unsafe` would need a new ADR first.
5. **Configuration stays TOML, data stays data packs.** A plugin is not a new
   configuration format; datapack loading (`mc-data`) is the sanctioned
   data-driven surface and data packs are already "user code" in the sense the
   plugin API would formalize.

## 4. Consequences

- Nothing to maintain: no code changed to write this ADR, and the grep above is
  the recurring check — if a plugin type appears in core before a trigger in
  §1 fires, the ADR and the code disagree, which is a review finding.
- The seams are cheap to grow precisely because they are ordinary code: when
  the second command contributor appears, `build_command_tree` grows a
  parameter or a caller-composed tree with no deprecation of anything.
- `docs/architecture/system-overview.md` and the parity matrix point here for
  plugin-readiness claims; no release document may say "plugin API planned"
  more strongly than "boundary documented (ADR-0005), API unbuilt".
