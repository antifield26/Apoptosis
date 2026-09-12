# ADR-0004 — Data Loading: a Separate Crate, and the Formats Measured From the Jar

Date: 2026-09-11. Status: **Accepted** (Phase 07, P07-03).
Context: Phase 07 had to load data packs — 8 775 files under `data/minecraft/` in the
26.1.2 jar, plus whatever a world or an operator adds. `CONVENTIONS.md` §7 lists
"registry/ # Minecraft registry/data loading and lookup" as one boundary, which raised
the question of whether this belongs in `mc-registry`.
Evidence: `crates/data/`, `docs/research/data-pack-baseline.md`,
`tools/vanilla-probe/survey_tags.py` (the measurement tool),
the Phase 07 report (git history, tag `phase-09-final`).

## 1. Decision: `mc-data` is a separate crate from `mc-registry`

`mc-registry` stays what it is: the **id tables**, generated from the jar into a
compact fixture that it parses itself, with no filesystem access, no `serde_json`, and
no I/O. `mc-data` loads the pack format.

The two have almost nothing in common mechanically:

| | `mc-registry` | `mc-data` |
|---|---|---|
| input | one generated fixture per registry | a directory tree of JSON |
| dependencies | none beyond `mc-core` | `serde_json`, the filesystem |
| lookup | `state_id("minecraft:stone")` — an index | `tag("minecraft:planks")` — a set |
| failure mode | corrupt fixture = startup error | a bad pack file = skip and report |
| trust | built by this project | operator-supplied, possibly downloaded |

Putting them together would have forced every `state_id` caller to link `serde_json`
and the filesystem, and would have merged two different failure policies (a corrupt id
table must stop startup; a malformed pack file must not).

**This refines §7 rather than departing from it.** §7's own preface says the list is
"logical boundaries, not permission to create empty crates" and that "a crate should
exist when it owns real behavior". Both own real behaviour and neither is empty; the
grouping was a sketch, and this is the split reality produced. Recorded here because a
future reader comparing the tree to §7 would otherwise see a discrepancy.

## 2. Decision: the formats are measured from the jar, not recalled

Every format decision in `mc-data` is backed by a measurement, and the measurement is
reproducible:

```text
data/minecraft/              8 775 files across 39 directories
  tags/                        758 files, 17 registries
    nested #references         384        (so transitivity is the common case)
    deepest nesting              4        (so a bound of 64 is 16x headroom)
    cycles                       0        (so the detector is test-only)
    "replace": true              0        (implemented, UNVERIFIED against vanilla)
    {"required": false}          0        (implemented, UNVERIFIED against vanilla)
  recipe/                    1 516 files, 21 types
    modelled                   7 types   (shaped, shapeless, stonecutting, 4 cooking)
    skipped, counted          14 types
```

Why this matters: Phase 06 shipped hand-written recipe and fuel tables from community
knowledge, and the Phase 06 report §5.9 (git history, tag `phase-09-final`) had to record that they were unverified. The
jar confirms the values that were guessed (`iron_ingot_from_smelting_raw_iron` is
`cookingtime: 200, experience: 0.7`). Loading the real data is what retires that
caveat, and it is why the loader exists before the world generator does.

Consequences recorded rather than hidden:

- the two format features vanilla does not exercise are implemented and labelled
  **unverified**; a test covers them, and the test says so;
- **14 of 21 recipe types are skipped and counted**, never silently dropped, because a
  pack whose recipe did nothing is otherwise indistinguishable from one that failed to
  load;
- `.zip` packs are **not** supported (they need an archive reader: a new dependency and
  a licence review). Directory packs only, recorded in `pack.rs`.

## 3. Decision: one JSON boundary, with the limits stated

All pack reading goes through `mc-data::json`, so the hostile-input policy is in one
place: a size ceiling checked from metadata **before** parsing, `serde_json`'s
recursion limit for depth, a typed error carrying the path (with 758 tag files, "invalid
JSON" without a filename is not a diagnosis), and no `unwrap` anywhere.

A pack is operator-supplied but not trusted. The failure policy is uniform: **skip the
file, record why, keep loading.** One malformed loot table must not stop a server.

## 4. Decision: resolution is depth-bounded recursion, not an explicit stack machine

An earlier draft of tag resolution used a hand-rolled explicit-stack DFS with per-frame
accumulators. It was harder to read *and* it was wrong (frames popped out of step with
the stack). The property that has to hold is "resolution terminates on any input", and
there are two honest ways to get it. Vanilla's deepest nesting is 4, so a checked depth
bound of 64 makes the recursion depth a *validated input* rather than an assumption
about the input — the same guarantee with far less mechanism.

A cycle is detected by a marker set on the resolution path and reported as a problem
that contributes nothing, so `#a → #b → #a` terminates and says why.

## 5. Consequences

- The dependency order becomes `core → registry → {persistence, world, data} → …` with
  `mc-data` depending only on `mc-core` and `mc-registry`.
- A pack's override semantics live in exactly one function
  (`DataPackSet::load_plan`, `tag_resolve::merge`), so "why did my pack not take
  effect" has one place to look.
- When real recipe/fuel tables replace the Phase 06 hand-written ones, the "unverified
  values" caveat recorded during Phase 06 shrinks to nothing — that is the intended
  payoff, and P07-09/P07-10 are where it lands.
- **Loot tables, advancements, functions, predicates, worldgen data and item modifiers
  are NOT loaded** by this crate yet. They are listed in `DATA_DIRECTORIES` as intent,
  and their absence is in the parity matrix.
