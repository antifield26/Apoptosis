# ADR-0002 — NBT and Persistence Boundaries (P03-01, P03-10)

Date: 2026-09-11. Status: **Accepted** (Phase 03).
Context: Phase 03 had to add disk NBT, the Anvil format, `level.dat` and chunk
serialization. Phase 02 already owned a network-NBT implementation inside
`mc-protocol`. Two crates of persistence code could have grown three or four
private NBT models.
Evidence: `crates/nbt/`, `crates/persistence/`, `docs/research/protocol-baseline.md`
§2, the Phase 03 report (git history, tag `phase-09-final`), `docs/research/provenance.md`.

## 1. Decision: one NBT implementation, two encodings

`mc-nbt` owns the tag model, the bounded reader and the writer. The disk and
network encodings differ **only** in whether the root tag carries a name:

| Encoding | Root name | Framing | Entry points |
|---|---|---|---|
| disk (`level.dat`, chunk NBT) | present | gzip (level.dat) / zlib (region payload) | `read_named`, `write_named` |
| network (packet payloads) | omitted | inside the packet frame | `NbtTag::read_network`, `NbtTag::write_network` |

`crates/protocol/src/nbt.rs` is now a façade that re-exports
`mc_nbt::NbtTag as Nbt`, so Phase 02 call sites and fixtures were unaffected.

Why: the two formats share tag ids, payload layout **and** Java modified UTF-8
strings (`net.minecraft.nbt.StringTag.write` → `DataOutput.writeUTF`, measured on
the 26.1.2 runtime). A second implementation would have doubled the
hostile-input surface and given the two paths a chance to drift.

Consequences:

- one hostile-input test suite (`Limits`: depth, input bytes, tag budget) covers
  both paths;
- the writer became fallible (`ServerError::Operational` for a string whose
  modified-UTF-8 form exceeds the `u16` prefix) instead of silently truncating a
  length field; the five Phase-02 call sites propagate it with `?`;
- `ResourceId` gained `Ord` so ids can key ordered maps (deterministic save
  order, CONVENTIONS.md §3.6).

## 2. Decision: `ChunkData` is a schema boundary, not the runtime chunk

`mc-persistence::chunk::ChunkData` models the file, not gameplay:

- it keeps `extra` for every top-level entry the model does not interpret
  (`carving_mask`, future fields), so loading and re-saving a vanilla chunk
  cannot silently drop data;
- it is `PartialEq` and JSON-free, so the restart tests can compare semantic
  content instead of bytes;
- Phase 04's `mc-world` chunk converts to/from it. Gameplay refactors therefore
  cannot change the file format by accident, and a format change cannot ripple
  into gameplay.

Rejected alternatives: serializing the runtime chunk directly (couples the
format to gameplay structures — the option `prompts/PHASE-03.md` explicitly
rules out), and a trait-based `ChunkSerializer` abstraction (no second
implementation exists, so it would be speculative — CONVENTIONS.md §3.4).

## 3. Decision: single-owner storage, one writer

`WorldStorage` is `Send` but not `Sync`; every mutation takes `&mut self`. Chunk
writes go through a `queue → flush` pair with a snapshot/restore protocol:

1. `queue_chunk_save` encodes the chunk now (an encoding failure surfaces to the
   caller immediately) and marks it dirty;
2. `flush` takes a dirty snapshot (clearing the tracker), writes chunks grouped
   by region file, syncs each touched region, then writes `level.dat` **last**;
3. anything that failed stays queued *and* is re-marked dirty, so the next flush
   retries it.

The tick thread currently drives this (`mc_server::storage::WorldService`).
P08-03 moves it to a dedicated save worker behind a bounded channel; the
ownership rules above are what make that move mechanical.

## 4. Consequences

- `mc-persistence` depends on `mc-core`, `mc-nbt`, `flate2`, `tracing` only.
- The `world`/`persistence` split in ADR-0001 D-01 is now real: `world` owns
  runtime chunks (P04), `persistence` owns the disk schema (P03).
- Unsupported formats are refused by name, never guessed at: LZ4/custom region
  codecs, external `.mcc` chunks, pre-1.18 chunk layouts, `DataVersion` outside
  `[4435, 4790]`.
