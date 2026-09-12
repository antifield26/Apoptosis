# ADR-0006 — Project License: MIT

Date: 2026-09-12. Status: **Accepted** (owner decision, closes ADR-0001 R-09).
Context: the project ran unlicensed through Phases 00–09 with an explicit rule
that this made GPL/AGPL reuse *forbidden* rather than merely discouraged
(`third-party.md` §2, `deny.toml` header) and that no release artifact could be
distributed (R-09). The owner has now decided: **MIT**.
Evidence: `LICENSE`, workspace + member `Cargo.toml` `license` fields,
`docs/legal/third-party.md` §2, `cargo deny` output (the gate now covers the
workspace's own crates).

## 1. Decision

- The project's own code is licensed **MIT**. `LICENSE` carries the full text
  with the copyright line "Copyright (c) 2026 The MinecraftServer Authors"
  (the owner may rename the holder without a new ADR — it is not an
  engineering fact).
- Every workspace manifest inherits `license = "MIT"` from
  `[workspace.package]`, so `cargo deny` no longer needs to skip the project's
  own crates: `private = { ignore = true }` is removed and the licence gate now
  checks all 17 members plus the third-party tree.
- **Distribution is unblocked.** R-09 is closed: release artifacts may be
  published and redistributed under MIT.

## 2. What does NOT change

- **The clean-room rule** (`third-party.md` §2.1): MIT for our code does not
  license copying from GPL projects. Pumpkin (GPL-3.0) and Paper (GPLv3)
  remain copy-forbidden; the `deny.toml` `[bans]` rows stay.
- **The dependency policy**: permissive-only allow-list unchanged; copyleft
  dependencies still require an explicit owner + legal decision before
  adoption.
- **Provenance recording** (CONVENTIONS.md §6): unchanged; MIT is a licence
  decision, not a provenance cleanup.

## 3. Consequences

- ADR-0001 R-09 is resolved; the risk-register row is annotated, not deleted
  (the R-03 convention).
- The release-candidate documentation's "blocked on license" statements are
  updated to cite this ADR; documents written before the decision (phase
  reports, audits) stand as history and gain dated addenda only where they
  make forward-looking claims.
- A distribution channel now exists: the source is published at
  `github.com/antifield26/Apoptosis` (public, MIT, pushed 2026-09-12, merge
  commit `b9f76f4`). Binary release artifacts are still not attached to it —
  "source published" is not "release announced"; a tagged binary release
  remains future work.
