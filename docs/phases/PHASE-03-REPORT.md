# Phase 03 Report — NBT, Region, Chunk and Persistence

Date: 2026-09-11. Preflight: re-read `AGENTS.md` §3.1/§3.3/§9/§10/§14/§15,
`MASTER-PROMPT.md`, `prompts/EXECUTION-LOOP.md`, `prompts/PHASE-03.md`,
`tasks/TASK-INDEX.md`, `docs/phases/PHASE-02-REPORT.md`, ADR-0001,
`docs/research/protocol-baseline.md`, `docs/testing/TEST-MATRIX.md`,
`docs/vanilla-parity/PARITY-MATRIX.md`; `git status` (branch `master`, still no
commits — unchanged by instruction); toolchain 1.98.1.

Phase goal: persistence that is Vanilla-compatible for the implemented world
slice and safe across restart and crash paths.

## 0. Headline: risk R-03 closed, and vanilla accepts our bytes

Two results decide this phase:

1. **R-03 resolved with primary evidence.** A vanilla 26.1.2 server (jar sha1
   `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51`, from the official version
   manifest) generated a world on this host. `level.dat` → `Data.DataVersion`
   **4790**, `Data.version` **19133**. Cross-checked against
   `DetectedVersion.createBuiltIn` (`sipush 4790`) and the jar's `version.json`
   (`world_version: 4790`). Backfilled into `protocol-baseline.md` §2.
2. **Differential test: vanilla 26.1.2 boots on a world our code wrote.**
   `crates/persistence/tests/vanilla_differential.rs` decoded all **529** chunks
   of that world, re-encoded and rewrote them through our writer, kept a
   `minecraft:diamond_block` marker we stamped into the spawn chunk, and let
   vanilla start, load and re-save everything. Vanilla logged `Done (0.309s)!`,
   no chunk/level/save error, and preserved the marker through its own
   load→save cycle — which proves it read our palette packing rather than
   regenerating the terrain. Our reader then re-read all 529 chunks vanilla had
   re-saved.

## 1. Tasks (P03-01..P03-16)

| Task | Status | Evidence |
|---|---|---|
| P03-01 NBT primitive model | DONE | `mc-nbt/src/value.rs`: 12 tag types, ordered compounds (last-wins lookup like `CompoundTag.put`), typed accessors with integer widening for tolerant reads, insert/remove with stable key order; shared by disk and network paths (ADR-0002) |
| P03-02 NBT parser | DONE | `mc-nbt/src/read.rs`: `TagReader` with depth/byte/tag budgets, collection lengths bounded by remaining input, `read_named`/`read_unnamed`, exact-consumption reporting |
| P03-03 NBT writer | DONE | `mc-nbt/src/write.rs`: named + nameless encodings, `u16` modified-UTF-8 strings, fallible length handling (no silent truncation), output rolled back on failure |
| P03-04 NBT malformed/fuzz tests | DONE | `mc-nbt` unit tests + `string.rs` measured-Java byte vectors + hostile-length/deep-nesting/unknown-id cases; `mc-protocol` façade keeps the P02 network tests |
| P03-05 Compression adapters | DONE | `mc-persistence/src/compression.rs`: ids 1/2/3 read+write, 4/127 refused by name, bomb-safe `take(limit+1)` decode; **no new dependency** (flate2 already adopted) |
| P03-06 Anvil region header/read path | DONE | `mc-persistence/src/region.rs`: 8 KiB header, `(sector<<8)|count`, sector bitmap, full validation (sector<2, count=0, past-EOF, length>capacity, unknown codec, `.mcc`), short-final-sector tolerance |
| P03-07 Region chunk write path | DONE | `RegionFile::write_chunk`: payload → timestamp → **location word last**, free-run allocation with append fallback, `sync()` barrier, removal frees sectors |
| P03-08 Level metadata model | DONE | `mc-persistence/src/level.rs`: 26.1.2 `level.dat` model (spawn compound, `difficulty_settings` string, `version` 19133), tolerant reader incl. pre-26.1 spawn/difficulty, complete writer, unknown-entry preservation |
| P03-09 Dimension/world metadata model | DONE | `mc-persistence/src/dimension.rs`: `Dimension` over validated `ResourceId`, modern + legacy layouts, `r.X.Z.mca` naming; `WorldStorage::layout_for` prefers modern, warns on legacy |
| P03-10 In-memory chunk serialization boundary | DONE | `mc-persistence/src/chunk.rs`: `ChunkData`/`SectionData`/`PalettedContainer`/`BlockState`, tolerant decode + complete encode, unknown top-level fields preserved in `extra` |
| P03-11 Dirty chunk tracking | DONE | `mc-persistence/src/dirty.rs`: snapshot clears, restore re-dirties (failure path), deterministic grouping by dimension+region |
| P03-12 Autosave scheduling | DONE | `mc-persistence/src/autosave.rs`: interval-driven, one save per interval even after an overrun, 0 disables, `set_interval`/`defer_from` for config reload |
| P03-13 Atomic/ordered save semantics | DONE | `mc-persistence/src/save.rs` + `world.rs`: `tmp → fsync → copy _old → rename → dir fsync` for `level.dat`; chunks before level; per-chunk failures stay queued and dirty |
| P03-14 Restart/load integration tests | DONE | `crates/persistence/tests/restart.rs` (7 cases): 4 chunks across 2 dimensions and 3 region files, level.dat round trip, backup rotation, autosave-driven flush, bounded handle cache, fresh-world creation |
| P03-15 Corruption/failure recovery tests | DONE | `crates/persistence/tests/corruption.rs` (17 cases) + `mc-persistence` region unit tests |
| P03-16 Persistence review + compatibility report | DONE | This report, `ADR-0002`, TEST/PARITY matrices, provenance, third-party, `protocol-baseline.md` §2 |

## 2. Verification (exact commands, this host, 2026-09-11)

| Gate | Command | Result |
|---|---|---|
| Tests | `cargo test --workspace` | **233 passed, 0 failed**, 2 ignored (the differential pair) |
| Format | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Cross-build | `cargo check --target aarch64-unknown-linux-gnu --workspace --all-targets` | clean |
| Differential (manual) | `MC_VANILLA_JAR=… MC_VANILLA_WORLD=… cargo test -p mc-persistence --test vanilla_differential -- --ignored --nocapture --test-threads=1` | 2 passed: 529 chunks decoded; rewritten world accepted by vanilla 26.1.2 |
| Binary smoke | `cargo build -p mc-server-app` then run with a TOML config | starts, creates `world/level.dat` (DataVersion 4790, `version` 19133), listens on the configured port; graceful-close path covered by `lifecycle.rs::world_is_created_at_startup_and_saved_at_shutdown` (the process smoke test force-kills on Windows, which cannot deliver Ctrl-C to a detached console app) |

Per-crate test totals: core 10 · nbt 16 (+1 doctest) · network 15 (+1 keepalive)
· persistence 71 unit + 9 fixture + 16 corruption + 7 restart (+2 ignored) ·
protocol 61 (+4 fixture) · server 12 (+6 E2E) · test-support 4.

## 3. Exit-gate check (`prompts/PHASE-03.md`)

- [x] **create/load/save/restart preserves a verified world slice** —
  `tests/restart.rs` builds a world (2 dimensions, 4 rich chunks, 3 region files,
  level.dat with spawn/difficulty/time), closes it, reopens from scratch and
  compares full semantic content; `tests/anvil_fixture.rs` does the same for a
  real vanilla chunk; the differential test does it for 529 vanilla chunks.
- [x] **malformed/corrupt data fails safely and observably** — 17 corruption
  cases: truncated/short header, sector into the header, zero sector count,
  sector past EOF, length > allocation, zero length, unknown codec, LZ4, `.mcc`,
  truncated payload (both before and after the 5-byte prefix), bit flip, corrupt
  `level.dat` (3 shapes), a bad slot next to a healthy one, and a failing write
  that stays queued. Every case returns a typed error; none panics; a healthy
  chunk in the same file stays readable.
- [x] **unsupported behavior recorded instead of implied away** — LZ4/custom
  codecs, `.mcc` chunks, pre-1.18 chunk layouts and out-of-window DataVersions
  are refused with errors naming what is missing, and are rows in the parity
  matrix.

## 4. Bugs found and fixed during the phase

| Bug | Found by | Fix |
|---|---|---|
| `PalettedContainer::set` grew the palette without re-packing the existing array, silently mixing two bit widths (chunk read back as corrupt) | restart test | width is tracked and the array re-packed on change; covered by unit + restart tests |
| A chunk whose write failed left the dirty set (`snapshot` had cleared it), so it would never be retried — silent data loss | corruption test | failed chunks are re-marked dirty and stay queued |
| The reader rejected a region file with a **short final sector**, which a real vanilla world contains after an interrupted save (measured: 141 sectors + 348 bytes) — we would have refused a world vanilla loads | differential test against a real world | payload bounded by `min(allocated, present)`; partial sectors stay marked as allocated so the writer never reuses them |
| `cargo build -p mc-server` failed (`tokio::signal` without the `signal` feature; workspace feature unification hid it) | binary smoke run | feature declared in `crates/server/Cargo.toml` |

## 5. Evidence artifacts

```text
crates/nbt/{Cargo.toml,src/{lib,value,string,read,write}.rs}
crates/persistence/{Cargo.toml,src/{lib,compression,packing,dimension,chunk,level,region,dirty,autosave,save,world}.rs,
                    tests/{anvil_fixture,restart,corruption,vanilla_differential}.rs}
crates/protocol/src/nbt.rs (façade over mc-nbt)
crates/server/src/storage.rs, crates/server/src/{lib,lifecycle}.rs, apps/server/src/main.rs
crates/test-support/fixtures/anvil/{level_26_1_2.dat,region_26_1_2.mca,MANIFEST.txt}
docs/adr/ADR-0002-nbt-and-persistence-boundary.md
docs/research/protocol-baseline.md §2 (R-03 resolved, measured format facts)
docs/{phases/PHASE-03-REPORT.md,testing/TEST-MATRIX.md,vanilla-parity/PARITY-MATRIX.md,
      research/provenance.md,legal/third-party.md}
```

Fixture regeneration (not committed, kept under `target/`):

```text
target/vanilla-26.1.2/server.jar                 # official 26.1.2 jar (sha1 verified)
target/vanilla-26.1.2/vanilla-world-26.1.2/      # world generated by that server
target/vanilla-26.1.2/{nbt_dump,region_dump,region_summary,make_fixtures}.py
```

## 6. Known limitations / carry-over

1. **LZ4 and custom region compression are not implemented.** A world written
   with `region-file-compression=lz4` is refused with an explicit error. Vanilla
   writes `deflate` by default, so the default path is unaffected. Tracked in the
   parity matrix.
2. **External `.mcc` chunks are not read.** Vanilla only creates them for chunks
   ≥ 256 sectors (≥ 1 MiB), far above anything measured (largest real chunk:
   147 KiB decompressed / 3 sectors). Refused by name.
3. **No datafixers.** Only DataVersions `[4435, 4790]` load; older worlds are
   refused rather than upgraded. Deliberate (ADR-0001 D-04) and recorded as an
   intentional divergence.
4. **Persistence runs on the tick thread today.** `WorldService::on_tick` saves
   inline. P08-03 moves it to a save worker behind the bounded channel described
   in ADR-0002 §3; the single-owner API is already shaped for that move, and the
   Pi-5 TPS impact of the current arrangement is unmeasured (P08-12).
5. **Gameplay data files are read/round-tripped only.** `game_rules.dat`,
   `weather.dat`, `world_clocks.dat` and `world_gen_settings.dat` shapes are
   measured and the envelope `{DataVersion, data}` is understood, but P03 does
   not own writing them (P04/P06/P07 do).
6. **`region-file` header writes are not crash-atomic beyond the ordering
   guarantee.** The location word is written last, and payload/sector reuse
   follows vanilla's allocate-then-free order, so an interrupted save can lose
   the new chunk but not the old one. A torn 4-byte header write remains
   theoretically possible (as in vanilla) and is caught by the reader's
   validation rather than silently accepted.
7. **The differential test needs external inputs** (a vanilla jar + world) and is
   `#[ignore]`d by default; CI cannot run it without those assets. It was run
   manually for this report and the log path is printed by the test.
8. **Real 26.1.2 *client* verification is still outstanding** (carried from
   Phase 02). Nothing in Phase 03 needed it, but the login/config chain remains
   unverified against a real client, and registry element NBT remains
   provisional.

## 7. Gate status

**Phase 03: PASS.** Both exit-gate clauses are satisfied by executable evidence,
the four workspace gates are green, R-03 is closed with primary evidence, and a
real vanilla 26.1.2 server accepted and re-saved a world written entirely by this
code. Known limitations are recorded above and in the parity matrix rather than
implied away. Phase 04 (Survival vertical slice) is unblocked: it has a
`WorldStorage` API, a `ChunkData` schema boundary and a measured vanilla baseline
to compare against.
