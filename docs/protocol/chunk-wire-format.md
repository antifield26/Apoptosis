# Chunk / light / section wire format — protocol 775 (verification note)

Every claim below was read out of the **official 26.1.2 server jar** (sha1
`97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51`) by disassembling the encoder the
client is actually built against. Class + method are cited for each. This file
exists because a chunk packet built from a second-hand description is the
easiest way to produce a server that logs in and then shows a black void.

Tooling: `tools/vanilla-probe/DumpRegistries.java` for the registry dump, and `javap -p -c -constants` on the
classes named below.

## 1. `level_chunk_with_light` (clientbound 45)

`ClientboundLevelChunkWithLightPacket.write`:

```text
i32  x
i32  z
<ClientboundLevelChunkPacketData.write>
<ClientboundLightUpdatePacketData.write>
```

## 2. Chunk data block

`ClientboundLevelChunkPacketData.write` — **verified order**:

```text
HEIGHTMAPS_STREAM_CODEC      -> see section 3
VarInt  buffer length
byte[]  buffer               -> the section blob, see section 4
BlockEntityInfo.LIST_STREAM_CODEC -> VarInt count, then per entry:
                                     packed u16 (x<<4|z), u16 y, VarInt type, NBT
```

`extractChunkData` writes the section blob and then asserts
`writerIndex == capacity`, i.e. the blob contains **exactly** the sections and
nothing else — no section-count prefix (see section 4).

## 3. Heightmaps

`ClientboundLevelChunkPacketData` static initialiser:

```java
HEIGHTMAPS_STREAM_CODEC = ByteBufCodecs.map(
    IntFunction, Heightmap.Types.STREAM_CODEC, ByteBufCodecs.LONG_ARRAY)
```

- `ByteBufCodecs.map` writes a **VarInt entry count**, then each key/value.
- `Heightmap.Types.STREAM_CODEC` is the enum's own id codec (a VarInt), **not**
  a name string.
- `ByteBufCodecs.LONG_ARRAY` (`ByteBufCodecs$13`) calls
  `FriendlyByteBuf.writeLongArray`, which is
  `VarInt length` followed by `writeFixedSizeLongArray` (raw longs, no padding).
  So: **each heightmap's longs DO carry a VarInt length**, unlike a paletted
  container's storage array.

Because the type is sent as a numeric enum id, the sender must use the ids the
client knows. `docs/protocol/heightmap-types.tsv` records them, extracted from
`Heightmap.Types` in the jar by declaration order (which is what the enum codec
uses).

## 4. Section blob

`ClientboundLevelChunkPacketData.extractChunkData` iterates
`LevelChunk.getSections()` and calls `LevelChunkSection.write` for each.

`LevelChunkSection.write` — **verified order**:

```text
i16  non-empty block count
i16  fluid count          <-- the task brief omitted this; it is required
<PalettedContainer.write> block states
<PalettedContainerRO.write> biomes
```

There is **no section count** in the blob: the count is implied by the reader's
own section layout (`LevelChunkSection[]` length for the dimension height).

## 5. Paletted container

`PalettedContainer.write` → `PalettedContainer$Data.write(buf, globalMap)`:

```text
u8     bits per entry
if bits == 0:  VarInt global palette id (single value for the whole container)
else:
  <Palette.write>:
     VarInt palette length
     per entry: VarInt global palette id
  writeFixedSizeLongArray(storage.getRaw())
     -> raw longs, NO length prefix (the count is derived from bits + entries)
```

`SimpleBitStorage` (the concrete `BitStorage`):

- `valuesPerLong = 64 / bits` (integer division)
- `cellIndex(i) = i / valuesPerLong`
- `bitOffset(i) = (i - cellIndex * valuesPerLong) * bits`
- `get(i) = (data[cellIndex] >>> bitOffset) & ((1 << bits) - 1)`

So values are **LSB-first and never span a long boundary**, which is exactly what
`mc_persistence::packing` already implements (verified byte-identical against a
real vanilla chunk in `crates/persistence/tests/anvil_fixture.rs`).

Bit width and entry counts come from `PalettedContainerFactory.create`:

- block states: `Strategy.createForBlockStates(BLOCK_STATE_REGISTRY)` → min **4**
  bits, `entryCount` 4096 (16³)
- biomes: `Strategy.createForBiomes(...)` → min **2** bits, `entryCount` 64 (4³)

`Strategy.getIndex(x, y, z)` = `((y << bitsPerAxis) | z) << bitsPerAxis | x`, so
the in-section index is `x + z*16 + y*256` for blocks (x fastest) and
`x + z*4 + y*16` for biomes.

## 6. Light data

`ClientboundLightUpdatePacketData` (also used by `light_update`):

```text
VarInt sky light mask
VarInt block light mask
VarInt empty sky light mask
VarInt empty block light mask
VarInt sky light array count,  then per array: VarInt 2048, byte[2048]
VarInt block light array count, then per array: VarInt 2048, byte[2048]
```

Bit `i` of a mask corresponds to section `i - 1` (section 0 maps to bit 1); the
highest bit covers the layer above the world. Phase 04 sent **no** light arrays:
`sky light mask = 0`, `block light mask = 0`, both empty masks = 0. Since P10-05
chunks carry the computed light (sky flood from the heightmaps, block-light BFS,
incremental `light_update` on change — owner-confirmed on a real client; see
`26.1.2-wire-notes.md` §2.3 and the lighting row in `PARITY-MATRIX.md`).

## 7. Follow-ups this creates

- `mc_persistence::packing::{pack, unpack, bits_for, longs_needed,
  values_per_long}` is reusable as-is for the network container; the only
  difference from disk is the `bits == 0` single-value form and the missing
  length prefix.
- `mc-persistence` stores biomes with `BIOME_MIN_BITS = 1`; the network form uses
  **2**. The disk→network conversion must therefore re-derive the width from the
  network strategy, not reuse the disk width.
