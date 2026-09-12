# Vanilla probe tooling

These are the research tools behind the committed fixtures and the measured figures in `docs/`. They read
the **official Minecraft 26.1.2 server jar** (`server-26.1.2.jar`, DataVersion 4790), which the operator
supplies: no jar is committed, because it is Mojang's and this repository is MIT.

They lived under `target/` until Audit 07 (finding M3) pointed out that documents cited them as the
evidence for their numbers while `target/` is git-ignored scratch space — so no reader of the public
repository could inspect the tool, let alone re-run it. They are committed here, unchanged.

## Layout they expect

Run them from a directory containing the jar, with the extraction beside it:

```text
<workdir>/
  server-26.1.2.jar
  extract/data/minecraft/        # produced by the snippet in docs/research/data-pack-baseline.md §0
  fixtures-out/                  # produced by make_fixtures.py
  reports/                       # produced by DumpRegistries.java
```

Then point the differential suites at the extraction:

```powershell
$env:MC_VANILLA_DATA = "<workdir>\extract\data\minecraft"
$env:MC_VANILLA_JAR  = "<workdir>\server-26.1.2.jar"
$env:MC_VANILLA_WORLD = "<workdir>\vanilla-world-26.1.2\world"
```

## What each tool produced

| Tool | Output it is cited for |
|---|---|
| `survey_tags.py` | tag semantics and the 758 tag files (`docs/research/data-pack-baseline.md`, ADR-0004) |
| `recount.py` | the corrected data-pack counts — directory entries excluded, which is why `recipe/` is 1 515 not 1 516 |
| `packets_from_jar.py` | the 256 protocol-775 packet ids, read from `getstatic` **bytecode** order, emitted as `docs/protocol/packet-ids-775.tsv` |
| `merge_packet_tables.py` | joins the per-state packet tables into that TSV |
| `DumpRegistries.java` | boots the jar's own registry and dumps block-state/item ids; the source of `crates/test-support/fixtures/registry/*.tsv` |
| `compact_blocks.py` | compresses that dump and **verifies** every one of the 29 873 state ids round-trips |
| `census_loot_adv.py` | loot-table / advancement / recipe census figures |
| `structure_census.py`, `structure_tagids.py` | the 1 202 structure templates: load/refuse split and palette ids |
| `make_fixtures.py` | the committed P03 fixtures (`level_26_1_2.dat`, `region_26_1_2.mca`, …) plus their sha256 manifest |
| `nbt_dump.py`, `region_dump.py`, `region_summary.py` | ad-hoc NBT/region inspection used while writing `mc-nbt` and `mc-persistence` |
| `RandomProbe.java` | JDK `java.util.Random` vectors, the golden data for `mc-simulation/src/random.rs` |
| `MenuProbe.java` | `InventoryMenu` slot layout via `javap` |
| `UtfProbe.java` | the UTF-8 question behind `mc-nbt`'s string handling |

## What is deliberately **not** here

Several hundred one-shot patch scripts (`fix_*.py`, `wire_*.py`, `write_*.py`) remain under `target/`.
They are not tooling: they are transcripts of edits already applied, aimed at tree states that no longer
exist, and committing them would bury the files above. That is a decision, not an omission.
