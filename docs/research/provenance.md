# Provenance Log (P00-02 policy §2)

Owner: engineering. Binding rule (`docs/legal/third-party.md` §2 rule 2): **any
nontrivial algorithm, table, fixture or design directly derived from a reference gets
a row here** — what, from where (with `file:line` or the emitted class/method), its
licence, and how it was reimplemented.

The project is **clean-room** (`third-party.md` §2 rule 1): no source from any
reference clone enters this repository — not copied, not translated, not vendored.
Behaviour, facts and tests may inform us; the code is independently written. Pumpkin
(GPL-3.0) and Paper (GPLv3) are read-only references, and patch *logic* is treated as
tainted in the same way as code.

**No project licence decision had been made** (ADR-0001 R-09) when this policy was
written, so GPL/AGPL-licensed reuse was *forbidden*, not merely discouraged. The
owner adopted **MIT** on 2026-09-12 (ADR-0006); the clean-room rule and the GPL
reuse ban for third-party material are unchanged by it.

> **Reconstruction note.** This document was accidentally truncated on 2026-09-11 by a
> maintenance script whose `open(path, 'w')` truncated the file before a failing write
> (the repository has no commits, so there was no version to restore from). It has been
> rebuilt from the rows that are verifiable verbatim in the tooling that produced them
> and from the citing documents. **Rows that could not be verified are marked
> `[unrecovered]`** rather than silently dropped: the Phase 00 seed rows for the four
> reference clones are the ones affected, and `docs/research/reference-repos.md` holds
> the same inventory. This incident is why the zero-commit standing gap matters, and it
> is recorded in Audit 03 §6 (git history, tag `phase-09-final`).

## 1. Reference-derived items

| Date | Item | Source | Licence | What was taken | How it was reimplemented / verified |
|---|---|---|---|---|---|
| 2026-09-10 | `docs/research/architecture-comparison.md`, ADR-0001 D-01..D-08 | Pumpkin (GPL-3.0), Paper (GPLv3), Valence (MIT), Minestom (Apache-2.0) | n/a for the comparison text itself | **Design observations only** (tick-thread + Tokio edges; stage-serial/batch-parallel; per-file-locked I/O) | The design is stated as a problem and a fit, never as "Paper does it this way" (CONVENTIONS.md §4). No code or table was taken. `[unrecovered]` — the original per-clone rows were lost with the truncation; the inventory they referenced is `reference-repos.md` |
| 2026-09-10 | Anvil-subset, tolerant NBT, dual-layout world reader | Minestom (Apache-2.0) named as the permissive source for loader/palette ideas; Pumpkin/Paper for behaviour | Apache-2.0 (idea only) | **Idea**: a tolerant NBT reader and a dual-layout chunk-directory reader | Independently written in `mc-nbt` and `mc-persistence`; the layouts themselves are Mojang's file format, and the reader's actual behaviour was verified against a **real 26.1.2 world** (Phase 03 differential). `[unrecovered]` as a per-source row |
| 2026-09-11 | `docs/research/protocol-baseline.md` §2 — `DataVersion = 4790`, level version `19133` | A **vanilla 26.1.2 `level.dat`** the operator produced, read with our own NBT reader | Mojang data (facts) | The two integers and the `Version.Name`/`Series` strings | Measured, not recalled. This closed risk R-03; the method and corroboration are in §2 of that document |
| 2026-09-11 | Bug fix: short final region sector | Found by our own differential test against a world vanilla had written after an interrupted save | n/a | — | The reader now bounds the payload by `min(allocated, present)` like vanilla; regression test `a_short_final_sector_is_tolerated_like_vanilla` |
| 2026-09-11 | `docs/protocol/packet-ids-775.tsv` (256 ids) + `mc-protocol/src/ids.rs` | Official 26.1.2 server jar: the order of `getstatic PacketTypes.<NAME>` instructions in `ProtocolInfoBuilder`'s static initialiser, which is the registration order that becomes the id. Extracted by `tools/vanilla-probe/packets_from_jar.py` | Mojang jar (facts, not expression) | Packet id **numbers** for protocol 775 | Numbers transcribed as data and asserted by `crates/protocol/tests/packet_ids.rs` against every constant the server uses. This replaced the Phase 02 transcription from a reference snapshot, which had `chat_command` wrong (8 vs the jar's 7) |
| 2026-09-11 | `crates/test-support/fixtures/registry/{blocks.tsv,items.tsv}` | Official 26.1.2 jar, booted through its own registry: `SharedConstants.tryDetectVersion()` + `Bootstrap.bootStrap()` then walking `Block.BLOCK_STATE_REGISTRY` / `BuiltInRegistries.{BLOCK,ITEM}` (`tools/vanilla-probe/DumpRegistries.java`); compressed by `tools/vanilla-probe/compact_blocks.py`, which proves all 29 873 ids round-trip | Mojang jar (facts) | Block-state id table (1 168 blocks / 29 873 states) and item ids (1 506) | Generated data, not code: a mixed-radix encoding of the registry the client itself is built against. The encoding is re-derived and verified at load time in `mc_registry::BlockRegistry::parse` |
| 2026-09-11 | `docs/protocol/{chunk-wire-format.md,heightmap-types.tsv}` | Official 26.1.2 jar bytecode: `ClientboundLevelChunkPacketData`, `LevelChunkSection`, `PalettedContainer$Data`, `SimpleBitStorage`, `ByteBufCodecs$13`, `Heightmap$Types` | Mojang jar (facts) | Chunk/section/paletted-container/light/heightmap wire shapes and the six `Heightmap.Types` ids | Written up as measured facts with class+method citations. Two of the primary agent's own assumptions were **disproved** by this evidence (a section-count prefix that does not exist, and a `LIGHT_BLOCKING` heightmap type that does not exist) and corrected before any code shipped |
| 2026-09-11 | `mc-world/src/{chunk,collision,ray}.rs` | No reference implementation consulted for the algorithms: swept-AABB resolution and Amanatides–Woo voxel traversal are public techniques, implemented from their descriptions | n/a | Algorithms (public, non-copyrightable) | Written from scratch with the block registry supplying solidity; validated against a real vanilla chunk and by collision/ray tests |
| 2026-09-11 | Bug fix: collision tunnelling and standing-`on_ground` | Found by this project's own collision tests | n/a | — | Swept-box per-axis resolution; grounded derived from the block under the feet as well as from a stopped fall |
| 2026-09-11 | Bug fix: reach accepted out-of-range blocks | Found by the survival E2E suite | n/a | — | Single 3-D distance from the eye to the block's nearest point |
| 2026-09-11 | `mc-simulation/src/random.rs` golden vectors | The **real JDK**: `java.util.Random` on JDK 25 via `tools/vanilla-probe/RandomProbe.java`, the same runtime the 26.1.2 server ships against. Printed `nextInt`/`nextLong`/`nextDouble`/`nextFloat`/`nextBoolean` and bounded `nextInt(n)` for seeds 42/0/12345/7/… | Oracle JDK (facts about a documented algorithm) | The exact draw sequence, used as golden test vectors | The algorithm is Java's documented LCG, reimplemented from its specification. Two of the author's own bugs were caught by the vectors: `nextLong` must sign-extend each `next(32)` half, and the rejection comparison must be signed 32-bit |
| 2026-09-11 | Armour menu layout: `ARMOR_MENU_START`/`OFFHAND_MENU_SLOT` + the payload permutation | Official 26.1.2 jar bytecode: `javap -c net.minecraft.world.inventory.InventoryMenu` (static initializer and constructor) and its `SLOT_*` constants | Mojang jar (facts) | The mapping menu index ↔ equipment slot | `SLOT_IDS = [FEET, LEGS, CHEST, HEAD]`, loop adds `ArmorSlot(.., 39 - i, ..)` at menu index `5 + i`, so menu 5 = FEET and menu 8 = HEAD. This **disproved an audit finding** that claimed our (correct) forward mapping was inverted |
| 2026-09-11 | Mob statistics in `mc-entity::mob` | minecraft.wiki for the zombie (health 20, hitbox 0.6×1.95, attack Normal 3, speed attribute 0.23) and community sources for the spider | Wiki/community sources, **not** a 26.1.2 measurement | Per-kind health, hitbox, attack and speed | Every value carries a confidence label in the module table. Six other healths, six other hitboxes and seven other speeds could **not** be verified and are marked `task-supplied`; `SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE = 43.17` is this project's own derivation and is labelled as probably too generous. No unverified value is presented as a vanilla fact |
| 2026-09-11 | `mc-container/src/{click.rs,menu.rs}` container layouts and click semantics | The `container_click` field order from `javap` on `ServerboundContainerClickPacket` (26.1 adds two trailing `HashedStack` fields the Phase 06 model does not decode); `ContainerInput` ordinals; `InventoryMenu`'s slot constants | Mojang jar (facts) | The wire field order and the menu slot layout | The transaction **rules** are this project's own design, not derived: conservation, boundedness and validation are properties the tests assert. The click-type ids and menu indices are data taken from the jar |
| 2026-09-11 | `mc-container/src/{crafting,furnace,hopper}.rs` recipe and fuel values | **Community knowledge / recall.** No jar dump and no experiment in this session | Wiki/community (CONVENTIONS.md §4 level 5–6) | Recipe shapes, counts and results; fuel burn times | Labelled in-code per value as `verified`/`derived`/`approximation`/`product decision`, and **nothing here may be called Vanilla-verified** — until P12-07/08. Update 2026-09-16: crafting item-only shaped/shapeless and smelting rows now convert from the loaded pack (`from_book`/`from_recipes`; tag ingredients counted); fuel stays the jar-verified baseline rows. |
| 2026-09-11 | `mc-redstone` power and timing rules | **minecraft.wiki** (fetched 2026-09-11), cited inline at each constant; block and property **names** from the jar-derived registry fixture | Wiki (community source, level 5) for the rules; Mojang jar for the names | The 0..=15 range, "decreases by 1 per block of dust", repeater output 15, strong powers adjacent dust and weak does not, comparator compare/subtract, torch = NOT, and what strongly powers a block | **What is verified is the rule, not the number.** Every `PowerSource::base_power()` is assumed 15 and labelled an approximation. All timing (torch delay, burn-out, dust re-evaluation, repeater delay being waited on) is unverified. Update 2026-09-17 (P13-05/06): the wire length (15 live blocks) and the conductivity matrix are now **measured on a real 26.1.2 server**, overriding the wiki paragraph in three places — a torch does not power its attachment, dust beside a lit torch reads 15, and a redstone block never powers an adjacent solid (torch powers only the stone above it; a lever only its mount; dust feeds solids with its solid-blind receipt; weak stone conducts at −1). `WIRE_LIVE_BLOCKS = 15` is measured. |
| 2026-09-11 | Redstone divergences recorded rather than fixed | This project's own analysis, prompted by the implementing agent's own report | n/a | — | Three deliberate divergences: **no conductivity** (a solid block is never powered, so the strong/weak distinction is currently unobservable and "lever on a block, dust on the far side" does not work); no Vanilla **update order** (positions are processed in the queue's documented sweep order, so locational circuits differ); and a wire reads a neighbouring wire's *stored* level rather than evaluating depth-first. Vanilla's block-update/shape-update distinction and comparator-update channel are absent. Update 2026-09-17 (P13-06): the conductivity divergence is **closed** — solids conduct directionally per the measured matrix; update order and depth-first evaluation remain divergent |
| 2026-09-11 | Bug fix: a budget stop dropped updates, and a once-per-tick fairness rule made falling edges travel one hop per tick | The implementing agent's own budget tests, then the reviewer rejecting the agent's first "fix" (renaming the failing test and documenting the decay as a divergence) | n/a | — | The queue now leaves unprocessed work queued, and fairness comes from a sweep cursor instead of a once-per-tick cap. A cascade reaches its fixed point within a tick in both directions. Recorded because the near-miss matters: a failing test was almost converted into a documented "divergence" instead of being diagnosed |
| 2026-09-11 | Audit 02 (git history, tag `phase-09-final`) | Two independent read-only subagents, plus direct `javap`/JDK probes by the primary agent for every contested claim | n/a | Findings and their corrections | Every audit finding was treated as a hypothesis and re-verified against primary evidence before being acted on; one (§3.1) was wrong and was rejected with the bytecode that disproves it |
| 2026-09-11 | Audit 03 (git history, tag `phase-09-final`) | Two further independent read-only subagents auditing Phase 05 and all cross-phase claims | n/a | Findings and their corrections | Found eight defects (two in already-"completed" work) and twelve documentation claims that did not survive contact with the code, including two a previous audit had already "corrected" and left wrong |
| 2026-09-16 | P10–12 wire + table facts: `MenuType` 0..24 order, `open_screen`/`container_set_data`/`container_close`/`set_cursor_item` shapes, `CONTAINER_ID` VarInt, furnace `FURNACE` data slots 0..3, zombie-speed capture ceilings, reach arithmetic | Official 26.1.2 server + client jars via `javap -c -p` (`MenuType`, `ClientboundOpenScreenPacket`, `ClientboundContainerSetDataPacket`, `ClientboundContainerClosePacket`, `ClientboundSetCursorItemPacket`, `FriendlyByteBuf.read/writeContainerId`, `CommonPlayerSpawnInfo`, `ServerGamePacketListenerImpl`, `Player.isWithinBlockInteractionRange`) + the vanilla-server entity capture (`target/entity-capture/`) | Mojang jars (facts, not expression) | Registry order, packet field widths/order, interaction-range arithmetic | Shapes pinned by round-trip tests in `crates/protocol`; ids asserted by `packet_ids.rs` against the extracted table; MenuType order additionally asserted by `open_screen` golden bytes |

## 2. Data assets committed

| Fixture | Size | Contents | Justification |
|---|---|---|---|
| `crates/test-support/fixtures/anvil/level_26_1_2.dat` | 393 B | verbatim gzip `level.dat` from our own vanilla run | Proves the `level.dat` reader against real vanilla output; no user data present |
| `crates/test-support/fixtures/anvil/region_26_1_2.mca` | 16 KiB | vanilla 8 KiB header + one chunk's verbatim sectors | Golden input for region/dechunk decoding and the packing-fidelity check |
| `crates/test-support/fixtures/registry/blocks.tsv` | 89 KB | the block-state id layout of all 1 168 Vanilla blocks (mixed-radix encoding) | The client identifies blocks by numeric state id on the wire, so chunk streaming is impossible without this table |
| `crates/test-support/fixtures/registry/items.tsv` | 76 KB | the 1 506 item ids and which block each places | Needed to validate placement and to persist inventories by name |
| `crates/test-support/fixtures/registry/block_light.tsv` | — | per-state emission/dampening/`propagatesSkylightDown` from the jar's accessors (P10-04) | Drives the static light engine; verified cell-for-cell against a vanilla capture for sky |
| `crates/test-support/fixtures/registry/entity_types.tsv` | — | entity-type registry ids from the jar (P10-06) | The `add_entity` type id is a claim about the client's own registry |
| `crates/test-support/fixtures/registry/block_defaults.tsv` | — | default block-state properties | Startup fallback when a palette block is missing |
| `crates/test-support/fixtures/registry/biome_spawners.tsv` | — | per-biome spawn lists compiled into `spawn.rs` (P11-01) | Natural-spawn rules with per-constant provenance in the module |
| `crates/test-support/fixtures/registry/entity_metadata.tsv` | — | per-type entity-metadata index tables (P10-07) | Extends the `set_entity_data` codec |

All four are **generated by this project** from a jar the operator supplies, are
numeric and name tables (facts, not creative expression), contain no Mojang code, and
are regenerable with the tools under `target/vanilla-26.1.2/`. Hashes are in
`crates/test-support/fixtures/anvil/MANIFEST.txt`. No jar, class file, asset or
datapack is committed.

## 3. Standing gaps

1. **`[unrecovered]` rows.** The Phase 00 seed rows for the four reference clones were
   lost with the truncation described above. Their substance — path, licence, version
   metadata, relevant modules — is in `docs/research/reference-repos.md`, and the
   licence findings are in `docs/legal/third-party.md` §1. The clones have no `.git`
   directory, so pinned commit SHAs were never recordable (ADR-0001 R-02) and that
   remains the reason this log cannot cite them.
2. **History started late.** For the first six phases the repository had **no
   commits**, which is why a single bad script destroyed a document: there was no
   history to restore from. The owner authorised an initial commit on 2026-09-11
   (`b7b1c99`), so that specific exposure is closed from that point on — but the
   earlier work has no per-change history, and the `[unrecovered]` rows above are
   still unrecoverable because of it.
