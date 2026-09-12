# License & Provenance Audit (P00-02 / P00-09 input)

Date: 2026-09-10. Scope: the four local clones + our project policy. No code copied in Phase 00.

## 1. Findings per clone

| Clone | License (evidence) | Risk for us |
|---|---|---|
| Pumpkin-master | **GPL-3.0** — `LICENSE:1-2`, `Cargo.toml:113`; plugin-api/utils `MIT OR Apache-2.0` (`crates/pumpkin-plugin-*/Cargo.toml:5`); Bedrock Geyser data MIT (`assets/bedrock/LICENSE-GEYSER:1-3`); data-split notice `assets/NOTICE.md:1-39` | **HIGHEST RISK.** Any copy of GPL-3.0 code (or derivative) into our tree would force GPL-3.0 on the combined work (CONVENTIONS.md §6: no GPL copy without explicit license decision + legal review — none made). Structural learning + behavior observation only. Even "MIT" plugin-api crates live in a GPL workspace — treat as tainted until proven otherwise; do not vendor |
| Paper-main | **GPLv3** (from Spigot/Bukkit/CraftBukkit) + optional MIT for contributor code — `LICENSE.md:1-15`, `paper-{api,server}/LICENCE.txt:1-2` | **HIGH RISK**, same rule as Pumpkin. Patch files describe vanilla code without containing full files — still treat patch *logic* as GPL-tainted: reimplement from behavior, never translate patch hunks |
| valence-main | **MIT** — `LICENSE.txt:1` (© 2022 Ryan Johnson), `Cargo.toml:109` | Low copyleft risk, but CONVENTIONS.md §6 still requires provenance records for nontrivial derivations + dependency license review. Stale version (1.20.1) limits what we can derive anyway |
| Minestom-master | **Apache-2.0** — `LICENSE:1-3` | Low copyleft risk, same provenance duty as MIT. Most useful permissive source for loader/palette ideas — still **reimplement**, record in `provenance.md` when a design is directly inspired |

## 2. Project policy (binding until explicitly changed)

1. **Clean-room**: no source from any reference clone enters this repo — no copy, no translate, no vendor. Behavior/facts/tests may inform us; code must be independently written. (CONVENTIONS.md §6)
2. **Provenance log**: any nontrivial algorithm/table/design directly derived from a reference gets a row in `docs/research/provenance.md` (what / from where incl. file:line / license / how reimplemented).
3. **Dependency gate** (feeds ADR-0001 § dependencies): every new third-party crate needs license check (prefer MIT/Apache-2.0/BSD/ISC/Zlib; anything GPL/AGPL/SSPL → requires explicit owner decision + legal review BEFORE use). Record in `docs/legal/third-party.md` with version + license + purpose.
4. **Data assets**: Mojang game data (packets.json-style tables, datapacks, mappings NBT, structure NBT) are Mojang-EULA-bound (cf. Pumpkin `assets/NOTICE.md`). We do not vendor reference-extracted data; where runtime data is needed (Phase 04/07), prefer generating/loading from a user-supplied vanilla client jar or server operator's files, and document the source. No extracted asset is committed in Phase 00.
5. **Project license: MIT** (owner decision 2026-09-12, ADR-0006 — resolves ADR-0001 R-09; `LICENSE` + `license = "MIT"` in every manifest). Rules 1–4 above are unchanged by it: clean-room stays binding, and GPL/AGPL *source* reuse stays **forbidden** — MIT covers our own code, not theirs.

## 3. Dependency pre-policy (P00-09 output, to be ratified in ADR-0001)

- Allowed without further review: current std + (Phase 01 proposes) `tokio` (MIT), `serde`+`serde_json` (MIT/Apache), `toml` (MIT/Apache), `tracing`+`tracing-subscriber` (MIT), `thiserror` (MIT/Apache), `bytes` (MIT/Apache) — each still gets a `third-party.md` row at adoption time with exact version.
- Needs case-by-case: `flate2`/`ruzstd`/`snap` (compression choice, P03), `proptest`/`criterion` (dev-deps ok, MIT/Apache), anything with `GPL*`/`AGPL*` in its tree (`cargo deny`-style check to be added in Phase 01 CI, P01-03).
- Forbidden for now: any `-sys` crate pulling copyleft build scripts; GPL-licensed reference crates (`pumpkin-*`, Paper artifacts) as dependencies.

## 4. Audit trail

- Method: read-only inspection 2026-09-10 (file headers + workspace manifests). Full per-repo details: `docs/research/reference-repos.md` §1-4.
- `git status` **at audit time** (2026-09-10): repo had **no commits yet**; the initial commit `b7b1c99` landed 2026-09-11 (branch `master`; the tree holds the workspace manifests and `docs/`). Audit artifacts are new files under `docs/`; nothing was copied from the clones. (An earlier revision of this line listed a top-level `src/` that never existed in this layout — corrected 2026-09-11.)

## 5. Adopted dependencies (Phase 01, P01-01/P00-09)

Resolved 2026-09-10 via `cargo build --workspace` (toolchain 1.98.1). All MIT/Apache-2.0.
`Cargo.lock` pins the versions below and is committed (initial commit `b7b1c99`); before that it was untracked, which Audit 02/03 recorded.

| Crate (locked) | License | Purpose | First use |
|---|---|---|---|
| tokio 1.53.1 | MIT | async runtime: listener/accept, tick-loop sleep/select, signal handlers | P01-08 (`apps/server`, `mc-server::lifecycle`) |
| serde 1.0.229 (+ serde_core) | MIT OR Apache-2.0 | config (de)serialization | P01-06 |
| serde_json 1.0.151 | MIT OR Apache-2.0 | status-response JSON, log/config surfaces | P02-05 (`mc-network`) |
| toml 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | TOML config file parsing | P01-06 |
| tracing 0.1.44 | MIT | structured logging API | P01-07 |
| tracing-subscriber 0.3.23 | MIT | fmt subscriber + env-filter | P01-07 |
| thiserror 2.0.20 | MIT OR Apache-2.0 | error taxonomy derive | P01-05 (`mc-core::error`) |
| bytes 1.12.1 (transitive via tokio) | MIT | byte buffers (not yet used directly) | — |
| flate2 1.1.10 (miniz_oxide 0.9.1 backend) | MIT OR Apache-2.0 | zlib frame compression/decompression (pure-Rust backend for aarch64) | P02-09 (`mc-protocol::framing`) |
| uuid 1.26.1 | MIT OR Apache-2.0 | profile UUID type and offline derivation result | P02-07 (`mc-network::auth`, packets) |
| md-5 0.10.6 | MIT OR Apache-2.0 | MD5 for vanilla offline UUID derivation (RustCrypto) | P02-07 (`mc-network::auth`) |

Transitive crates resolved by `Cargo.lock` (miniz_oxide, adler2, crc32fast,
digest, generic-array, etc.) are permissive MIT/Apache-2.0; no copyleft
dependency entered the tree (P00-09 gate). The promised gate now exists:
`deny.toml` + the `dependency-policy` CI job, with its operating rules in
[`docs/operations/DEPENDENCY-POLICY.md`](../operations/DEPENDENCY-POLICY.md).

## 6. Phase 03 (P03-05): no new external dependencies

Phase 03 added two **internal** crates (`mc-nbt`, `mc-persistence`) and no new
third-party crate:

| Crate | License | Purpose | First use |
|---|---|---|---|
| mc-nbt (internal) | project (MIT) | NBT model/reader/writer shared by protocol + persistence | P03-01..04 |
| mc-persistence (internal) | project (MIT) | Anvil region IO, `level.dat`, chunk schema, dirty/autosave/save barrier | P03-05..15 |

Compression reuses the already-adopted `flate2 1.1.10` (MIT OR Apache-2.0) for
zlib **and** gzip; no LZ4 crate was added, because LZ4 region compression
(`region-file-compression=lz4`) is refused with an explicit error instead of
being implemented (see `docs/vanilla-parity/PARITY-MATRIX.md`).

### Data assets (P00-02 policy §4, revisited)

Phase 03 commits two small test fixtures under
`crates/test-support/fixtures/anvil/`, both generated **by this project** from a
vanilla 26.1.2 server run (not extracted from any reference repository):

| Fixture | Size | Contents | Justification |
|---|---|---|---|
| `level_26_1_2.dat` | 393 B | verbatim gzip `level.dat` from our own vanilla run | Needed to prove the `level.dat` reader against real vanilla output; no user data present |
| `region_26_1_2.mca` | 16 KiB | vanilla 8 KiB header + one chunk's verbatim sectors | Needed as golden input for region/dechunk decoding and as the packing-fidelity check |

They are world data generated by a tool the operator runs (the same data any
server deployment stores), carry no reference-repo code, are used read-only as
test inputs, and are documented in `docs/research/provenance.md` with hashes in
`MANIFEST.txt`. The regeneration procedure is in
`docs/phases/PHASE-03-REPORT.md`. No Mojang binary, jar, asset or datapack is
committed.

## 7. Phase 04 (P04-01): no new external dependencies

Phase 04 added three **internal** crates and no third-party crate:

| Crate | License | Purpose | First use |
|---|---|---|---|
| mc-registry (internal) | project (MIT) | block-state and item id tables, loaded from a generated fixture | P04-01 |
| mc-world (internal) | project (MIT) | runtime chunks, block storage, collision, ray casting | P04-02/07/08 |
| mc-entity (internal) | project (MIT) | player state, inventory, item stacks, health/food/XP | P04-04/12/13/14 |

`mc-server` gained normal (non-dev) dependencies on `mc-entity`, `mc-world`,
`mc-registry` and `mc-protocol`; all internal, so `deny.toml` is unaffected.

### Data assets added

| Fixture | Size | Contents | Justification |
|---|---|---|---|
| `crates/test-support/fixtures/registry/blocks.tsv` | 89 KB | the block-state id layout of all 1 168 Vanilla blocks (mixed-radix encoding) | The client identifies blocks by numeric state id on the wire, so chunk streaming is impossible without this table |
| `crates/test-support/fixtures/registry/items.tsv` | 76 KB | the 1 506 item ids and which block each places | Needed to validate placement and to persist inventories by name |

Both are **generated by this project** from a jar the operator supplies
(`target/vanilla-26.1.2/reports/DumpRegistries.java` boots the jar's own registry;
`compact_blocks.py` compresses and verifies it). They are numeric and name
tables — facts, not creative expression — and contain no Mojang code. No jar,
class file, asset or datapack is committed.
