# Changelog

All notable changes to this project are documented here. The project keeps a
linear history on `main`; this file distills it per phase. The complete
per-phase reports and five adversarial audits that this file condenses live in
git history — the pre-governance snapshot (which still contains them as files)
is the tag **`phase-09-final`** (`git show phase-09-final:docs/phases/…`).

Format follows [Keep a Changelog](https://keepachangelog.com/) in spirit. The first
entry is the release candidate matching the workspace version (`0.1.0` in
[Cargo.toml](Cargo.toml)); it is **published** as tag `v0.1.0-rc.1` with built
artifacts, and no later version has been released.

## Unreleased — Phase 10 (client compatibility and rendering)

### P10-01 — client-capture rig

A TCP proxy that relays a real Minecraft 26.1.2 client to the server **byte for byte** while writing a
normalized JSONL trace of the conversation, in both directions. It is CONVENTIONS.md §12's differential-testing
contract applied to clients instead of servers, and it exists because "a real client joined and it looked
right" is not evidence.

- **New crate** `apps/capture-rig` (`mc-capture-rig`), with the binary `capture-rig`. No new third-party
  dependencies: `mc-protocol`, `serde_json`, `md-5`, `tokio` were all workspace deps already.
- Frames are split by the project's own `FrameCodec`, with consumed-byte counts taken from `buffered()`
  deltas, so the bytes forwarded are the bytes read.
- **The observer is passive.** Forwarding never depends on decoding: if framing fails for a direction, that
  direction degrades to opaque passthrough and the trace records `observer_error` plus the direction in
  `session_end.degraded`. A capture that ends early is visibly early rather than looking like a short session.
- **Normalized for comparison:** the digest is over the *uncompressed* body, so two sessions differing only in
  compression produce identical digests; `wire_bytes` keeps the framed size separately; no per-packet
  timestamps, because a trace is meant to be diffable between runs.
- A session ends when **either** direction does, with a bounded 3 s drain, so a peer that never closes cannot
  leave a capture without an end marker.
- 11 tests: 9 unit (framing, state machine, compression in both wire forms, the compression-transition
  regression from P10-02, digest invariance, degradation, write-failure reporting) and 2 integration that
  drive the `TestClient` through the rig to a **real server**
  and require the trace to show handshake → login → config → play with no degradation.

**Two real defects the integration tests found**, both invisible to the unit tests:

1. The handshake intent was read as the payload's *second* VarInt, which is the **address length**. The state
   machine therefore never left `handshake`, and every later packet was interpreted against the wrong id
   table. The unit test passed because it built a payload matching the same wrong assumption; it now builds a
   real handshake with the typed encoder. *A unit test that constructs its input from the same mental model as
   the implementation cannot catch a wrong mental model.*
2. `relay_pair` waited for **both** directions, so a server holding its half open after the client left meant
   `session_end` was never written and the capture had no end marker.

Six falsification probes confirm the load-bearing mechanisms: framing, digest-over-uncompressed-body, typed
handshake decoding, the bounded drain, the per-connection sink, and degradation reporting. Each was disabled,
the covering test confirmed to fail, and the file restored byte-exact.

**Still blocked:** P10-02 and every acceptance task in this phase need an owner-provided runnable Java 26.1.2
client. The `TestClient` is not a substitute and has not been used as one.

### P10-02 — real-client first contact

The owner supplied the client (`D:\HMCL`, HMCL 3.16.3 with a vanilla `26.1.2` instance). A minimal launcher
was built from the version manifest — libraries selected by the manifest's own platform rules, placeholders
substituted, offline UUID derived the same way the server does — and the client was pointed at the rig with
`--quickPlayMultiplayer`. Evidence is the JSONL trace plus the protocol-error report the vanilla client writes
to `debug/`; the game window is not evidence of anything.

**Result: the client joins, completes login and configuration, and is then refused by its own registry
loader.** Its report names both missing registries:

```text
Description: Registry Loading
Errors:
  minecraft:root/minecraft:timeline:    Unbound tags   ...: [minecraft:in_overworld]
  minecraft:root/minecraft:world_clock: Unbound values ...: [minecraft:overworld]
Dynamic Registries:
  minecraft:dimension_type: elements=1 tags=0
  minecraft:worldgen/biome: elements=1 tags=0
```

The server sends two dynamic registries; a 26.1.2 client needs those two plus `timeline` and `world_clock`.
Recorded as **KD-39**, and the login row in the parity matrix now says "fails with a real client" instead of
"partial (test client)". The fix belongs to P10-03.

**A defect in the P10-01 rig, found by this run.** The trace recorded `login id=0 login_disconnect` — a packet
the server never sent — immediately after `set_compression`. The cause: `observe` extracted **every** frame in
a chunk with the codec's current compression setting and applied the transition only afterwards, so a
`SetCompression` and the following packet arriving in one TCP chunk left the second frame parsed with
compression still off. Below the threshold that packet is `[len][data_len = 0][raw id + payload]`, so the `0`
marker was read as the packet id; framing stayed aligned because the outer length prefix is the same in both
forms, and nothing else looked wrong.

It was caught because the client's own report says it reached `finish_configuration`, which is impossible if
the login state had ended in a disconnect — **the two artefacts disagreed, and the disagreement was the
finding.** Fixed by extracting one frame at a time with transitions applied between frames, plus a regression
test that delivers the two frames in a single chunk. A second instance of the same mistake sat one line above:
`compressed` was sampled once per chunk, so the frame after the transition was labelled uncompressed; also
fixed and covered by the same test.

Re-running first contact after the fix produces `login_finished` where the phantom disconnect was, which
verifies the repair against a real client rather than against a fixture.

**Limitations, stated rather than implied.** First contact was run against a **vanilla** client only, not the
`26.1.2_Fabric` instance also present. The launcher is a test harness in `target/`, not a supported tool. The
session ends at the registry refusal, so nothing after `finish_configuration` — lighting, entities, chat —
has been reached by a real client yet, and those remain exactly as unverified as KD-38 says.

### P10-03 — first-contact remediation: the synced registries (KD-39)

P10-02 measured the refusal; this fixes the first two causes of it and records the third. Three real-client
runs, each naming the next gap in its own protocol-error report — which is the loop working, and the reason a
`TestClient` cannot stand in for a client.

| Run | The client said | Outcome |
|---|---|---|
| 1 (P10-02) | `timeline: Unbound tags [in_overworld]`, `world_clock: Unbound values [overworld]` | **fixed** |
| 2 | `Registry must be non-empty` for 13 variant registries | **fixed** |
| 3 | `Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]` | **not fixed** |

**What was actually wrong, run 1.** The overworld `dimension_type` declares `timelines: "#minecraft:in_overworld"`
and `default_clock: "minecraft:overworld"` — two **references** — and nothing verified that the names exist in
anything the server sends. Neither the `timeline` nor the `world_clock` registry was sent, and `UpdateTags` was
a unit struct that always encoded **zero registries** and whose decoder *rejected* anything else, so tags could
not be sent at all. Both were fixed, and the client's next report shows `timeline: elements=4 tags=4` and
`world_clock: elements=2` with both errors gone.

**Run 2** then reported thirteen variant registries as `Registry must be non-empty`, so a 26.1.2 client requires
every synced registry the pack defines. They total 33 KB, so the probe now extracts the whole set and the server
sends **whatever the fixture holds** rather than a list of its own — the same information in two places would go
stale the first time one grew.

**Run 3** still refuses, on a missing `minecraft:damage_type / minecraft:is_fire` tag. The pattern is now
unambiguous and the remedy is mechanical: add the remaining pack registries (`damage_type`, `enchantment`,
`jukebox_song`, `instrument`, `banner_pattern`, `chat_type`, `trim_material`, `trim_pattern`, `dialog`,
`trade_set`, `villager_trade`, `trial_spawner`, `enchantment_provider`, `test_environment`, `test_instance`) to
the probe's list. That is a one-line change plus a re-run. It is deliberately **not** claimed here: it needs the
same real-client verification, and asserting it without that run is exactly what KD-38 exists to prevent.

**Pipeline.** `tools/vanilla-probe/extract_synced_registries.py` extracts the registries and their tags from the
jar's own pack into a committed fixture, on the same footing as `blocks.tsv`/`items.tsv`. It resolves
`#minecraft:universal` inside `in_overworld` at extraction time, because the wire carries a tag's members as
numeric registry ids and a nested tag is not representable. The server maps the resolved names back to ids from
its own entry order, so the id assignment lives in exactly one place.

**A derived rule, labelled.** JSON cannot distinguish a float from a double and Minecraft's codec does. Every
non-integer in the vanilla timelines (50 of them: track `value`s and `cubic_bezier` coefficients) is a
float-typed field, so the converter maps float → `Nbt::Float`. That is a **derivation from the data, not a
verified fact**, and the real client is what adjudicates it — so far without complaint.

**The test that would have caught run 1 without a client.**
`every_reference_the_overworld_declares_is_actually_sent` walks the dimension element's `#tag` and value
references and requires each to resolve in the payload. KD-39 was never a wrong value; it was a reference with
no referent, and nothing checked. Two other tests were replaced rather than edited: the packet sequence now
asserts its **ordering rule** instead of an exact id list, and `e2e_login_play` asserts that no registry is sent
empty instead of pinning a count of two — a count that was itself the limitation, which is why the test agreed
with the bug for as long as it did.

**Still unverified, and therefore still claimed by nobody:** nothing after `finish_configuration` has been
reached by a real client. Lighting, entities and chat remain exactly as unmeasured as KD-38 says.

### P10-03 (continued) — a real 26.1.2 client reaches play

Four further real-client runs, each naming the next gap. **The fifth ends with the client in the play state**,
which is the first time a Java client has done so in this project — KD-38 has read "boundary (not yet
exercised)" since Phase 09.

| Run | Client said | Outcome |
|---|---|---|
| 3 | `Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]` | fixed — `damage_type` sent with its 33 tags |
| 4 | `enchantment: Failed to parse value` for every entry | **excluded, with its reason recorded** |
| 5 | `Failed to decode clientbound/minecraft:set_default_spawn_position` | **play reached** |

**Run 4 is the important negative result.** Every enchantment failed to parse, and the cause is a limit of the
approach rather than a missing registry: those fields use *dispatch* codecs (a bare number or an object with a
`type`), NBT lists are homogeneous, and a float is a different tag from a double. A converter that infers
everything from JSON **shape** cannot express any of the three. Excluding the registry was the honest move and
it is also what unblocked the phase: sending a payload the client rejects is a hard failure, while omitting it
leaves a gap the client names precisely — and it named none.

**A silent-corruption bug in that converter was found by the test suite, not by the client.** Adding
`villager_trade` failed 44 tests with `unknown NBT tag id 64 in compound`: NBT lists are homogeneous, so seven
mixed-type arrays (`number_of_dyes.summands` is `[{...}, 1]`) made the writer declare one element type and
write another. The converter now refuses a mixed array **by name**, and the probe refuses to emit such a
registry at extraction time, where a human is reading.

**Run 5's divergence is the same lesson.** `set_default_spawn_position` decodes as
`readerIndex(10) + length(4) exceeds writerIndex(13)`: our encoder writes `BlockPos: i64` + `angle: f32` = 12
payload bytes, and 26.1.2 wants more. Recorded as **KD-40**. Like `enchantment`, it needs the packet's
**schema**, not a guess at its width.

**The remedy for both, and the recommended next step: stop re-implementing codecs from shape.** Capture the
real payloads from a vanilla 26.1.2 server — the jar is already in this workspace and the P10-01 rig is the
tool for exactly this — and replay them, as this project already does for packet ids, block states and the
data pack.

`villager_trade` and `enchantment` are excluded from the synced-registry fixture, each with its reason in the
probe. The fixture stands at 128 436 bytes over 28 registries; `tools/vanilla-probe/extract_synced_registries.py`
is the committed extractor.

### P10-03 (continued) — capturing from a real server instead of inferring from shape

The recommended remedy, applied at the smallest useful scale. A vanilla **26.1.2 server** was stood up
locally (`target/vanilla-capture`, flat world, loopback, JDK 25) and the real client was pointed through the
P10-01 rig at **it** instead of at our server. The resulting trace is a reference conversation: 9 271 packets
of what a 26.1.2 client and a 26.1.2 server actually say to each other.

**KD-40 closed from captured bytes.** Our `set_default_spawn_position` sent 13 body bytes and a real client
rejected it with `readerIndex(10) + length(4) exceeds writerIndex(13)`. The vanilla server's body is **37
bytes**, and it decodes cleanly:

```text
61                                             id 97
13 6d696e6563726166743a6f766572776f726c64     "minecraft:overworld"
0000000000000fc4                               i64 4036 = packed BlockPos(0, -60, 0)
00000000 00000000                              f32 yaw, f32 pitch
```

So 26.1.2 leads with the **dimension** and carries a **pitch**; our encoder had neither. Both readers are
big-endian (`to_be_bytes`, matching Netty), so the field list was the only thing wrong — which is exactly the
class of error inference produces and capturing settles. Verified by the client proceeding past it (the trace
grew from 54 to 121 packets), and **pinned by a golden test holding the captured bytes**, so a revert fails a
test rather than waiting for a client to object.

**The next divergence, again one byte out.** The client now rejects
`clientbound/minecraft:player_position` as "**1 bytes extra**". Same class, same remedy: the vanilla trace has
that packet too, and it is already captured.

**Two defects of my own, found on the way.**

* `mc-capture-rig` used `tokio::time::timeout` without declaring tokio's `time` feature. It built inside the
  workspace because feature unification let other members supply it, so `cargo build --workspace` succeeded
  while `cargo build -p mc-capture-rig` alone failed. A crate must declare what it uses.
* `HEAD_BYTES` was raised from 24 to 64, because that is what makes the rig usable as a *reference-capture*
  tool: the small play-state packets a client rejects are under that size, and the first 24 bytes were not
  enough to determine this packet's field order. Raising it removed the need for a separate dump mode.

`enchantment` and `villager_trade` remain excluded from the synced-registry fixture, each with its reason in
the probe. The same capture now offers a way to close them too: the vanilla server's `registry_data` packets
can be replayed rather than re-encoded from JSON shape.

### P10-03 (continued) — replaying the server's own bytes, and two more play packets fixed

The capture remedy, applied to the registries. **The finding inverted the approach.**

**Vanilla sends registry ids and no element data.** 382 entries across 28 registries in 8 781 bytes, with every
`has_data` flag false, followed by 32 316 bytes of tags. It can, because the client declared
`minecraft:core = 26.1.2` under `select_known_packs`, so the server sends the registry *shape* and the client
reads the content from its own jar.

So our previous design was wrong twice over, and only the first was visible. It could not encode
`enchantment`'s dispatch codecs or `villager_trade`'s mixed-type arrays — the failures I spent two rounds
excluding registries over — and more fundamentally it was **sending data the protocol does not ask for**.
The client parsed it because it was there, and refused when it did not match. Both \"problems\" were
self-inflicted. (`villager_trade` is not even a synced registry: vanilla does not send it at all.)

The converter, its JSON fixture and both exclusions are gone. The payload is now the vanilla server's own
bytes, replayed verbatim from a committed 41 338-byte fixture built by
`tools/vanilla-probe/build_config_payload.py`, with the rig's new `--bodies` mode as the capture path.

**`player_position` (KD-42) fixed from captured bytes.** The client reported `found 1 bytes extra`, which reads
as a width problem. The capture showed the total was right and the **order** was not: 26.1.2 leads with the
`VarInt` teleport id and we wrote it last, so the client consumed the top byte of `x` as the id. Fixed, pinned
by a golden test holding the 61 captured bytes, and verified — the client's session grew from **119 to 444
packets** past it.

**`set_time` is recorded, not guessed.** The client now rejects it as `was larger than I expected`. Our payload
is 17 bytes; the capture shows 9-byte packets at that id repeating periodically, consistent with a `set_time`
reduced to `i64` + `bool`, but **also a 31-byte packet at the same id that fits neither shape**. Until those
reconcile, our id 113 may not be vanilla's `set_time`, and encoding from an unverified reference is precisely
the mistake this method exists to avoid. Recorded as KD-43.

**A trap worth remembering.** `SelectKnownPacks`'s `Packet::ID` is the **serverbound** id, because the type
models the reply a client sends. Rewriting our send path as `to_raw()` therefore emitted a clientbound packet
carrying id 7. Where two directions share a packet name the id must be chosen explicitly; a test now asserts it.

**Two more of my own defects.** The capture script's readiness check trusted a **stale log** and skipped
starting the server, then — after that was fixed — probed the rig's port by **connecting** to it, which
consumed the single connection the rig exists to serve. A liveness probe must not change what it observes.

### P10-03 (continued) — a real 26.1.2 client plays with no protocol errors

**KD-43 closed, and the session is stable.** A real vanilla 26.1.2 client — the owner's installation,
through the P10-01 rig — now runs a complete session against this server with **no protocol-error report
written at all**, and is *live* rather than merely connected:

| Signal | Reading |
|---|---|
| client \u2192 server, play id 28 | `keep_alive` — it **answered** the server's keepalive |
| client \u2192 server, play id 13 | **410 per-tick reports** (\u224820/s) — the game loop is running |
| server \u2192 client, play id 113 | 17 `set_time` packets, accepted |
| server \u2192 client, play id 45 | 289 chunk packets |
| rig | `degraded=[]` — everything observed cleanly |

**What `set_time` actually is.** Captured: **eighteen** packets at id 113, all **9 bytes**, with the leading
`i64` incrementing by exactly **20** — one second of ticks, which is this packet's send rate. The id is
confirmed independently by the jar-derived `packet-ids-775.tsv` (`game clientbound 113 set_time`) and by the
client's own error text. So **26.1.2 removed `time_of_day` from the wire**: the client derives the time of day
from the `world_clock` registry, which is precisely why this phase had to make that registry work before play
was reachable at all. We sent `i64` + `i64` + `bool`, 8 bytes too many. Fixed, pinned by a golden test.

**One packet is left unexplained, deliberately.** Of nineteen packets at id 113, eighteen are 9 bytes and
**one is 31**: its `world_age` of 7405 fits the sequence immediately before the first 9-byte packet, but the
remaining 23 bytes contain `3f800000` (`1.0f`) twice, and `set_time` has no float field at all. Eighteen
uniform packets settle the format, so the fix does not depend on it — but if that packet is genuinely
something else then our id table and vanilla's disagree somewhere, and other id-labelled conclusions would need
re-checking. Recorded as an open question rather than smoothed over.

**A capability this removes, stated rather than hidden.** With `time_of_day` gone from the wire, the server can
no longer tell the client what time of day it is, so `/time set` no longer moves a real client's sky. That is
not a regression from this change: the previous encoding carried `time_of_day` and a real client **rejected the
whole packet**, so `/time set` never reached a real client either way. Restoring it needs 26.1's clock
mechanism, which belongs with the light engine work.

**Two naming gaps the trace exposes**, recorded as format issues rather than bugs: serverbound play id 13
(the per-tick report, 410 occurrences) and clientbound play id 45 (chunk data, 289) are both unnamed in the
rig's table, so the trace shows bare ids for the two most frequent packets of a live session.

**KD-38 moves, and not as far as it looks.** Its boundary was "no Java client has been driven". It is now
"a client plays, and **nothing about what it renders has been verified**" — the client could be staring at
an unlit void and nothing here would know. That is what P10-04 onward exists to settle.

### P10-04 / P10-05 — reconnaissance: `level_chunk_with_light` cannot be parsed at all

Starting the light engine turned up a defect that comes **before** it, and which changes what P10-05 has to do.

**All 117 captured vanilla chunk packets fail to decode with our implementation**, every one identically:

```text
light mask has 1 sections set but 0 arrays follow
```

**Two hypotheses, one excluded.** The first was that our decoder is stricter than the protocol — that a real
server sends a mask bit with no array. That is now ruled out by arithmetic that uses **neither** decoder: a
light array is fixed-width, so for a packet of known size only one small (sky, block) pairing can consume the
remainder. **None does — they are all 24 bytes short.** So the bytes being read as masks cannot be masks.
They look plausible because light data is mostly zero, which is exactly why a shared misreading survived two
implementations.

**What the real bytes do confirm.** Scanning for the array signature — a `80 10` VarInt (2048) followed by a
mostly-`0xFF` span — finds it at offset 5229 of a 7280-byte packet, ending at 7279. So `LIGHT_ARRAY_BYTES`
and the length-prefixed array convention are **right**, and **one byte remains after the array**, which is
itself unexplained.

**Not established: where the 24 bytes are.** The candidates — an extra heightmap long per entry, a misread
`data` size, or a field between them — are not distinguished yet, and guessing is what this phase keeps
paying for. The next step is to settle it against the bytes rather than to start filling masks.

**A note on method.** A diagnostic test is committed (`vanilla_chunk_light.rs`, ignored by default because the
capture lives under `target/`). Its first version asserted the packet ends with `[0x80, 0x10]`; the slice came
back `[0x10, 0xff]`, one byte out. That assertion was **removed rather than adjusted**, because a claim the
evidence does not support is the failure mode this whole phase has been unlearning. What it asserts instead is
the part that is solid: every captured packet fails identically, which is what makes this a layout defect
rather than a quirk of one packet.

**Consequence for the plan.** P10-04 (the light engine) is not blocked — it computes light and does not care
about the packet. **P10-05 is blocked**, because filling four masks is meaningless while the field order around
them is wrong.

### KD-44 closed — the light masks are `BitSet`s, and the fix is verified in both directions

The reconnaissance finding from this round is now fixed, and the root cause came from the jar rather than from
more reasoning.

**`javap -c` on `ClientboundLightUpdatePacketData`** gives the authoritative read order:

```text
readBitSet() -> skyYMask          readBitSet() -> blockYMask
readBitSet() -> emptySkyYMask     readBitSet() -> emptyBlockYMask
readList(DATA_LAYER_STREAM_CODEC) -> skyUpdates
readList(DATA_LAYER_STREAM_CODEC) -> blockUpdates
```

`readBitSet` is a **`VarInt` count of longs followed by that many `i64`s**. We read the four masks as
`VarInt`s.

**Why that survived.** An empty mask is a single `0x00` in **both** encodings. Our reading therefore agreed
with a real server for every mask Phase 04 ever sent — all of them empty — and disagreed the moment one
had content. A single captured packet with light in it exposed it.

**Re-running the layout search under the correct model** finds exactly one offset per packet, the same offset
across twenty captured packets, and the arithmetic closes exactly:

```text
offset 3150 (where our header parse already ended)
  sky bits [1, 2]  -> 2 arrays      block bits []  -> 0 arrays
  sky array lengths [2048, 2048]   block array lengths []
```

So **our header parse was right all along**; only the mask encoding was wrong. The "24 missing bytes" from the
previous round were an artifact of the wrong model, not a second defect.

**The fix.** The masks are now `Vec<u32>` of set section indices rather than an integer bit pattern — what a
`BitSet` means, which makes the "arrays follow the mask bits" check the array count itself, and which encodes
without the trailing-zero hazard: vanilla's `BitSet.toLongArray()` trims, so a fixed-width integer would emit
bytes no real server produces.

**Verified in both directions.**

* **Decode:** all **117** captured vanilla chunk packets now parse, where before **zero** did.
* **Encode:** a real 26.1.2 client still reaches play through the rig with **no protocol-error report**, on a
  783-packet session.

**Consequence.** P10-05 is unblocked: the field order around the masks is now known to be right, so filling
them is meaningful. P10-04 (the light engine itself) is still to build — that is what actually puts light in
those arrays.

### P10-04 (part 1) — the light engine, and the per-state table it runs on

**The table comes from the jar's own accessors, not from documentation.** `tools/vanilla-probe/LightProbe.java`
boots the registry the dedicated server boots and calls the three methods `LevelLightEngine` itself reads:
`getLightEmission()`, `getLightDampening()` and `propagatesSkylightDown()`. It writes `block_light.tsv`
(738 block rows plus 20 883 state rows, for the blocks whose states differ), which now loads beside `blocks.tsv`
and `items.tsv` as `Registries::light`.

The values were spot-checked against known blocks before anything was built on them: air `0/0/1`, stone
`0/15/0`, torch emission 14, glowstone `15/15/0`, water `0/1/0`, magma block 3, and `redstone_lamp` and
`light` correctly falling to per-state rows because their light varies by state.

**The engine** (`mc_world::light`) computes both layers:

* **Sky light** falls straight down at 15 while every block it passes propagates sky light, then spreads
  sideways losing at least one level per block.
* **Block light** seeds from each state's emission and spreads the same way, losing `max(1, dampening)`.

The one rule that matters most is `max(1, dampening)`: it is why a shadow never brightens as it spreads, and it
is what makes an opaque block opaque — a block with dampening 15 absorbs the whole level however bright its
neighbour is. Two of the three test failures while writing this were my own expectations contradicting that
rule, not the engine: I had light passing *through stone*, and I had a cell with open sky above it decaying
because I had mis-drawn a pillar.

**The approximation is stated, not implied.** A chunk is computed with a **one-block margin** in x and z, read
through a caller-supplied accessor, so light crosses chunk borders. Where the accessor reports an unloaded
chunk the cell is treated as air — the same assumption the client makes about ungenerated space. The symptom is
that a chunk at the edge of the loaded area can be brighter at its border than it will be once its neighbour
exists; the alternative, treating an unloaded neighbour as opaque, would make every frontier chunk visibly
dark, which is worse and less true.

Verified by 11 unit tests over synthetic three-state tables, including the cases that catch a
plausible-but-wrong engine: a sideways decay of exactly one per block under a roof (which a "spread without
decrement" bug would leave at 15 everywhere), stone absorbing all light, an emitter decaying outward, and a
torch **outside the chunk** lighting cells inside it — the last being the only test that would fail if the
margin were removed.

**Not yet done, and therefore not claimed:** the masks and arrays are still empty on the wire, so the client
still renders a dark world. `Game` does not yet call the engine. That is the rest of P10-05.

### P10-05 — light on the wire, and a performance bug of my own

The masks and arrays are no longer empty: a real client now receives computed light instead of an unlit world.

**The mask rule came from the capture, not from assumption.** Across all 117 packets: no section is ever in a
mask *and* an empty mask; `empty_sky` only ever sets bit **0** (the section below the world); the sky arrays are
exactly two per chunk — the open-sky section (uniformly 15) and the surface section (mixed). That matches
vanilla's `DataLayer`, whose default is 15 for sky and 0 for block, and it fixes the encoding:

* a `*_mask` bit means an array follows for that section;
* an `empty_sky` bit means uniformly **15**, an `empty_block` bit uniformly **0**;
* bit `i` is light section `i`, which is world section `i - 1`.

Every light section is accounted for, including the two outside the world — a section no mask mentions is one
whose value we did not choose.

**Evidence it is really there.** In a real 26.1.2 session the chunk bodies are now **4 626..8 732 bytes, mean
6 108**, where the empty-mask version was about 3 KB and vanilla's comparable chunks were 7 280 and 9 322. The
session has **no protocol error**, and the client is live: 447 client-to-server packets including per-tick
reports. The client's own decoder reads four `BitSet`s and two lists without complaint, which is itself a
structural check.

**A performance bug I introduced, and how it surfaced.** Seeding queued **every** sky-lit cell — 124 000 per
chunk for an open column — which made chunk sends slow enough that an unrelated test,
`an_over_long_command_ends_only_that_connection`, began timing out: its five-second deadline expired before a
command reply was generated. The fix is exact rather than a tuning: a cell can only raise a neighbour if some
neighbour is **strictly darker**, because `candidate = level - max(1, dampening)` can never exceed `level`. So
queueing only those cells is the precise precondition, and the scan that finds them uses flat-array strides
(`1`, `width`, `width * width`) instead of recomputing coordinates per neighbour. The suite went from 39.9 s to
11.5 s and the test passes; the 11 unit tests were unchanged throughout, which is what says the optimisation
did not alter the result.

**What is not verified, and cannot be from here: whether the world *looks* right.** Light is not a field a
client validates, so a wrong light level produces no error and no log — the same silence that hid the
empty-mask version. What is established is that the data is well-formed, is the right order of magnitude, and
is accepted. Confirming it looks correct needs a person looking at the screen.

**Also not done: incremental updates on block change.** Light is recomputed when a chunk is sent, so placing a
torch does not relight the chunk until it is resent. That is the remaining part of P10-04.

### KD-45 — the light engine's cost, measured rather than assumed

Putting light on the wire made a **second** test time out, and this time in CI only: locally
`an_over_long_command_ends_only_that_connection` passes in an 11.5 s suite, on the runner it fails in a 27.7 s
one against the same five-second deadline.

**The cause is exact.** Light is recomputed on **every chunk send**, per recipient, with no cache. Each chunk is
three passes over 124 320 cells — sky seeding, block seeding, and the frontier scan — and the frontier
dominates at six neighbour comparisons per cell per layer. A fresh login is 205 chunks.

**What was done, and what was not.** The deadline was raised to 60 s, with the reasoning written at the line:
its job is to fail when the server is **wedged**, not to measure throughput, so it is set well above the honest
cost rather than tuned to it. What was *not* done is reverting the light, which would have hidden a real cost
behind a feature that does not work. **The gap is recorded as KD-45 rather than absorbed.**

**The fix is known and is the same work P10-04 still owes.** Compute light once per chunk, keep it, and
invalidate only what a block change affects — caching and incremental relighting are one problem, not two.
Until then, placing a torch does not relight a chunk until it is resent, and a joining player waits longer than
they should for their first view.

**A note on how this surfaced.** Both performance problems in this round were found by tests failing for a
reason that looked unrelated: a command test that has nothing to do with lighting, timing out because logins got
slow. The first was mine to fix outright (queueing every lit cell, 124 000 per chunk); the second is a genuine
limitation that needs the caching work.

### KD-46 — differential verification, and the bug it found immediately

The goal's strongest verification: **run our engine on vanilla's own blocks and compare with vanilla's own
light.** The captured packets carry both halves — a real world's block states and the light arrays vanilla
computed for them — so no modelling sits between the two.

**It found a bug on the first run.** Agreement was **87.5%, exactly 7/8**: fourteen of sixteen arrays matched
cell for cell and the terrain section was uniformly 15 where vanilla's was mixed. Our engine was lighting the
world **straight through its terrain**.

The cause was in the light table's parser. `block` rows — the 738 blocks whose states share one triple, which
covers **stone, dirt and every common terrain block** — were matched by a pattern arm that pushed them into a
vector **nothing ever read**. Every state they covered kept the `UNKNOWN` default of `(0, 0, true)`:
transparent air.

**Why nothing else caught it.** The file was right. The parser ran. No error was raised. The payload was
well-formed and a client renders it without complaint, because light levels are not something a client
validates. The unit tests used synthetic tables where every state had an explicit row. Only running against a
real server's data could see it — which is precisely why the goal asked for this.

**The fix, and the format change that prevents a repeat.** `block` rows carried only the block's *name*, so a
parser had no way to know which state ids they covered — the information was missing, not merely ignored. The
probe now emits the range it already knew: `block <name> <first state id> <count> <emission> <dampening>
<propagates>`. A row that cannot be applied is now impossible to write.

**After the fix the agreement is 100.0000%** — 65 536 of 65 536 cells, worst difference **0**, across eight
chunks. The test asserts exact equality rather than a threshold, because that is what the evidence shows.

**What it does not cover, asserted rather than implied.** Vanilla sent **no block-light arrays at all** for this
capture, because a superflat world has no light sources, so block light is **not verified against a real
server** — only against synthetic unit tests, which is the kind of evidence that just missed this bug. The
test asserts that gap explicitly so a reader cannot take 100% as covering both layers. Verifying block light
needs a capture of a world with light sources in it.

### KD-47 — block light verified against a real server, and the margin quantified

The first capture could not test block light at all: a superflat world has no light sources, so vanilla sent no
block-light arrays and the test asserted `block_total == 0` to keep that gap visible. A **second capture** fixes
that, and the method is worth recording because it needs no GUI: the dedicated server reads commands from
**stdin**, so eight glowstone blocks were placed through the server console — after `forceload add`, because
`setblock` on a fresh server answers **"That position is not loaded"** and no player has been near spawn to load
it.

**Block light agrees on 40 939 of 40 960 cells** — 99.95%, worst difference 3, with every disagreement
confined to the two chunks adjacent to the glowstone. The chunk containing it matches exactly.

**The cause is a design approximation, not a bug**, and the test now names it at the assertion.
`compute_chunk_light` reads a **one-block margin** in x and z, which lets light enter a chunk across its border
but not travel several blocks outside it first. Being exact would need a margin of **15** — light loses at
least one level per block, so nothing further can matter — which enlarges the work region from 18x18x384 to
46x46x384, **6.5x the work per chunk**. Against an engine that already recomputes everything on every send
(KD-45), that is the wrong trade for 0.05% of cells.

**The real fix is the one vanilla uses and the one KD-45 already points at**: compute light over the **loaded
world** rather than per chunk, so a border is answered by a neighbour's already-computed light instead of by a
margin. Caching, incremental relighting and this all become one piece of work.

**A second false negative, also mine, also found by the comparison.** The first run with light sources showed
block light at 3455/4096 for a chunk *next to* the glowstone while ours read 0. The engine was right and the
**test** was wrong: it approximated the margin by replicating the chunk's own edge column, which is exactly
right for uniform terrain and exactly wrong for a source in the next chunk along — replicating the edge
replicates the absence of the source. The capture holds 117 chunks, so the margin no longer has to be
approximated at all: they are stitched into one world and the answer comes from real neighbouring blocks. That
change also made the **sky** verification stronger — 262 144 of 262 144 cells, worst difference 0, across 32
chunks with light crossing borders through real blocks rather than through an assumption.

### KD-45 (part 1) — chunk light is cached and invalidated on change

Light was recomputed on **every** `level_chunk_with_light` the server built — every chunk, for every recipient,
on every send. That cost is what started timing out an unrelated command test in CI.

It is now computed once per chunk and kept in `World`, keyed by the same `ChunkPos` as the chunk itself. The
cache lives in `World` rather than in `Chunk` because `Chunk` is built in persistence, worldgen and many tests,
so a field there means touching every struct literal, while `World` already owns the chunks and has one
constructor.

**Invalidation drops the changed chunk and the neighbours whose margin reads it** — the chunks across
whichever border the block sits within one block of, since the margin is one block. Six tests pin this,
including the case that catches an over-eager implementation: an **interior** change must leave the neighbours
alone, or the invalidation is simply "drop everything" wearing a condition.

**What it is not, said plainly.** This is **invalidation, not incremental relighting**. Vanilla relights only
the region a change can reach; this drops the whole chunk and recomputes it when next needed. It is correct and
it is cheaper than what it replaced by the ratio of how often a chunk is *sent* to how often it *changes* — but
a torch placed in a large lit chunk still costs a full recompute.

**It also does not help a first join**, which must compute each chunk once whatever the cache does. The honest
numbers: the command suite went **11.5 s \u2192 9.17 s** in debug, and runs in **3.46 s in release**. So a
substantial part of the CI failure was a **debug-build cost rather than a production one** — which is worth
knowing before treating it as an alarm, and is not a reason to leave it.

The remaining cost is the first computation: three passes over 124 320 cells, with the frontier scan dominating
at six neighbour comparisons per cell per layer. **The next step is to skip that scan for sections that are
uniformly lit** — most sections in an open world are, and for those the scan reads 4 096 cells to conclude
nothing can spread, where a section-level check would settle it far more cheaply.

**One design detail worth recording.** `vanilla_chunk_packet` takes `&self`, so it **cannot fill** the cache; the
pre-warm happens in `send_chunk`, which has `&mut self` and runs before the borrow that builds the packet. The
builder reads the cache and falls back to computing without keeping the result, so a caller that forgets to
pre-warm gets a **correct packet at the old cost rather than a wrong one**.

### KD-45 (part 2) — the cost was where I was not looking

The remaining cost of computing light per chunk looked like the frontier scan, which does six neighbour
comparisons per cell per layer. It was not. **`World::get_block_loaded` builds a `ChunkPos` and walks a
`BTreeMap` on every call**, and the light engine calls it **per cell** — 124 320 of them, twice, once for the
sky seeding pass and once for the block pass. That is about a **quarter of a million map lookups per chunk**,
against array arithmetic worth a few milliseconds.

**`BlockCursor` fixes it**: the engine walks a column at a time, so consecutive questions are almost always
about the same chunk, and one remembered chunk removes essentially all of those lookups. A missing chunk is
memoised too, since an unloaded neighbour along an edge is asked about as often as a loaded one.

**The measurements, which say more than the optimisation does:**

| Build | none | cached | cached + cursor |
|---|---|---|---|
| debug | 11.5 s | 9.17 s | **6.59 s** |
| release | — | 3.46 s | **3.20 s** |

**The release column is the honest one, and it undercuts the story I was telling.** The cursor removed a
quarter of a million lookups per chunk and bought **7% in release**, where the compiler and the cache were
already hiding them. So the remaining cost is the **inherent array work** — three passes over 124 320 cells —
and KD-45's severity in production is much lower than the CI failure suggested. It was a debug-build cost that
happened to break a deadline.

**What is still not done, and is a different thing from what was done:**

* **Incremental relighting.** The cache invalidates; it does not relight a region. A torch in a large lit chunk
  costs a full recompute where vanilla touches only what the change can reach.
* **Anything helping a first join**, which must compute each chunk once whatever the cache does.
* **The section-uniformity shortcut**, which is now the clearest remaining lever: most sections in an open
  world are uniformly lit, and for those the frontier scan reads 4 096 cells to conclude that nothing can
  spread.

**A note on the shape of this.** Two rounds of performance work have both been corrected by measurement — the
first by a test failing for an unrelated reason, this one by the release column disagreeing with the debug
column. The instinct to optimise the thing that looks expensive has been wrong twice; the release number is
what a player experiences.

### Real-client evidence: the client is drawing the world

An evidence source I had been overlooking: **the client writes its own log**, and it says things the protocol
trace cannot.

* **`Resizing Chunk Sections UBO, capacity limit of 2 reached … New capacity will be 128`** — the client is
  uploading chunk geometry to the GPU, which it only does for chunks it is actually drawing. It is not sat
  behind a loading screen.
* **`[System] [CHAT] Welcome to the Rust Minecraft server.`** — our chat message reached its screen.
* **Three ERROR lines in the whole session**, all `InvalidCredentialsException: Status: 401` from the offline
  profile fetching user properties. Expected, and unrelated to the server.

**What this does not establish, and nothing in a log could: whether the lighting *looks* right.** Light is not
a field a client validates — it is baked into the chunk mesh — so a wrong level produces no error, no warning
and no log line. It is the one claim in this phase that more tests cannot close.

So it is handed over rather than asserted: **`tools/visual-check/run.py`** starts the server, the rig and the
client and then **leaves them running** instead of tearing everything down like every other script here. It
prints what the client's log says about rendering, and what to look for: a bright sky, shaded ground, shadows
under overhangs — and the known behaviour that a hand-placed torch will *not* relight its chunk until that
chunk is re-sent (KD-45).

The differential results stand behind it: sky light matches a real server on every cell, block light on 99.95%.
If the world looks wrong anyway, the fault is somewhere the comparison does not reach — which is worth knowing
either way.

### KD-48 — `light_update`, the packet P10-05 named and did not have

A placed block changed the block and **not the light**. The client kept rendering the old light until that
chunk happened to be re-sent, so a torch did nothing visible — the limitation recorded under KD-45, now
closed.

P10-05 lists "encode `light_update` for changes" and it was the one named item with no implementation, only a
doc comment referring to it.

**The coordinate encoding is the trap, and `javap` settled it.** The packet carries the **same light data** as
the tail of `level_chunk_with_light` — the same four `BitSet` masks, the same two array lists, written by the
same `ClientboundLightUpdatePacketData`. It is natural to assume the packets are shaped alike. They are not:
`light_update` writes its two chunk coordinates as **`VarInt`**, the chunk packet as **`i32`**. Reading them the
other way consumes two extra bytes each and misparses everything after. A test asserts both encodings side by
side so the difference lives in the test rather than only in a comment.

**The shared half is shared.** `write_light_data` and `read_light_data` are one implementation used by both
packets. The last time a format in this phase was implemented twice — once in Rust, once in a script — the
two agreed with each other and were both wrong. The coordinate encoding is deliberately **not** shared: it is
the one thing that differs, and a helper parameterised by "which packet is this" is how that gets lost.

**The wiring is bounded on purpose.** Recomputing one chunk is three passes over 124 320 cells and the packet
is kilobytes, so work is **queued on block change and spent at four chunks a tick**. Nothing is dropped — a
chunk stays queued until sent — so a burst is delayed rather than lost; a dropped update would leave the
client showing stale light until that chunk was re-sent, which is the silent-wrong this phase keeps finding.
The changed chunk's **neighbours** are queued too, since the light they were read for has changed as well.

**Verified by an end-to-end test**, not a counter: breaking a block must make `light_update` arrive at a joined
client. A counter would say the sender loop ran; the packet id says the client was told. A `light_updates`
counter was added to the tick report anyway, because a light update that stops being sent is invisible.

Five gates green: 1234 passed / 0 failed / 24 ignored across 81 suites, fmt, clippy -D warnings, aarch64 and
cargo deny clean.

### KD-49 — the `light_update` trigger is not what I guessed, and that tempers the last round

Two captures placed four glowstone blocks beside a **connected** player and looked for `light_update`. Neither
produced one: **zero id-48 packets in 19 000 captured packets**, with `level_chunk_with_light` staying at
exactly the initial 117.

**The guess, and why it was wrong.** `javap -c` on `SetBlockCommand` showed `replace` mode passing
`iconst_2` — `UPDATE_CLIENTS` alone, without `UPDATE_NEIGHBORS` — and `updateNeighboursOnBlockSet` being
called **only on the `DESTROY` path**. Neighbour notification is what tells the light engine a block appeared,
so that looked like the answer. Re-running with `destroy` produced **no packet either**. The trigger for this
packet is therefore **not established**, and it is not being invented.

**What this does to the previous round's claim.** I wrote that wiring `light_update` closed the "a torch does
nothing" limitation. What is actually true is narrower: the packet is **sent** and **well-formed**, and it has
been accepted only by our own `TestClient` — because nothing the server does autonomously changes a block
while a real client is connected. **Test-client acceptance is precisely the evidence that failed in KD-44**:
our implementation agreeing with itself.

So the claim is "sent, self-consistent, and encoded from the jar", not "verified against a client". The packet
is still the right one to send — it exists for exactly this, and its field order and `BitSet` form come from
`javap` — but the difference between those two sentences is the whole point of this phase.

**The stale guidance, corrected.** `tools/visual-check/run.py` still told the reader that a hand-placed torch
would *not* relight its chunk, which was true when it was written. It now asks for the opposite observation:
**break a block and watch the light follow it**. That is the one remaining way to learn whether a real client
accepts the packet — and if it disconnects instead, that is a real finding rather than a surprise.

### KD-49 (continued) — the unverified part is two `VarInt`s, not a packet

The last round left KD-49 as a flat "no real client has received our `light_update`". That is true and it
understated how much of the packet is already covered, so the risk is now **narrowed by evidence** instead.

* the light half is written by **one implementation**, `write_light_data`, shared with
  `level_chunk_with_light`;
* a real client accepts that half on **every chunk it is sent** — 753-packet sessions with no protocol error;
* a test now asserts the two are **byte-identical** for equal light, so the sharing is pinned rather than
  asserted in prose;
* the only difference between the packets is the two coordinates, `VarInt` here and `i32` there, and that is
  `javap`-confirmed and pinned by a test that asserts both encodings side by side.

**What remains unexercised is therefore two `VarInt`s, not a packet.** That is a claim a person can settle by
breaking one block with `tools/visual-check/run.py` running — which is what it now asks for.

**A note on the shape of this.** Three times in this phase a finding has been narrowed by comparing against
something real rather than by more tests: KD-44 (the jar's bytecode, after two implementations agreed with each
other), KD-46 (vanilla's own light, after the unit tests passed), and now KD-49. The pattern is not that tests
are weak; it is that tests written by the same author as the code share its assumptions.

### KD-49 closed — a real 26.1.2 client accepts our `light_update`

The gap was that no real client had ever received one, and that self-testing cannot close it: our `TestClient`
agreeing with our encoder is the evidence that failed in KD-44.

**A real client only receives a `light_update` when the server changes a block while it is connected**, and the
server changes blocks only when a player breaks or places one. So the method is **two clients**: the real one
through the rig, and a `TestClient` that logs in separately, reads its own position, and breaks the block
beneath itself. The resulting update goes to **every** session holding that chunk.

Two details the driver had to get right, both from the wire rather than from assumption:

* the position arrives in `player_position`, which is sent **after** `join_game` — the packet `login_join` stops
  at — so it is read afterwards. Without it there is nothing to reach, since the server refuses a break beyond
  4.5 blocks.
* the break is a **raw** packet: there is no serverbound `PlayerAction` struct in this crate, because the server
  decodes raw packets into `PlayIntent`.

**Result:**

```text
logged in as Trigger
breaking the block at (0, 63, 0) under the player at (0.5, 64, 0.5)
test result: ok. 1 passed

client still running after the light update: True
new protocol-error reports: none
```

The client's only ERROR lines are the offline profile's expected 401s. The check is a **new protocol-error
report** rather than "did it stay connected", because a wedged client is silent; the directory is snapshotted
first, and twelve reports were already sitting there from the owner's own sessions.

**What remains open.** Vanilla's own trigger for this packet is still not established: two captures with a
connected player and console-placed glowstone produced no id-48 packet, and the `javap`-based guess about the
block-update flag was wrong. Our use of the packet is now verified against a client; **its faithful use
relative to vanilla is not**, and that is recorded rather than papered over.

The method is committed: `tools/light-update-trigger/run.py`, with the driver as
`crates/server/tests/light_update_trigger.rs`.

### KD-50 — the client never left "Loading terrain"; my diagnosis of it was wrong twice over

**The owner looked at the screen and reported the client stuck on "加载地形中".** That is real, it corrects a
claim I made twice, and the explanation I then produced was **wrong**.

**First, the claim it corrects.** I said a real client was "live and **rendering**". All three of my reasons hold
on the loading screen: the client answers keepalives while it waits, it sends its per-tick packets, and
`Chunk Sections UBO` grows as chunk geometry arrives. None requires being *in* the world. I read them as proof
of something they do not establish, and only a person looking settled it. The claim is withdrawn.

**Second, the diagnosis I then offered was wrong in two ways.**

* **I labelled the packets from memory instead of looking them up.** I wrote "(id 12)" and "(id 11)" and then
  measured the trace for those ids, found zero, and called it the cause. The table says
  `11 -> chunk_batch_finished` and `12 -> chunk_batch_start`; the chunk-cache packets are **94** and **95**, and
  our own `ids.rs` has always said so. **The measurement was against two constants that have nothing to do with
  the packets.** \"Sent 0 times\" was an artefact of my own labelling.
* **They were already being sent**, by the network layer at `crates/network/src/connection.rs:542`. So the
  \"fix\" sent correct packets **twice** — visible in the next trace as `94, 95, 95, 94` where the original
  was `94, 95`. It is reverted.

**What the episode is actually worth.** A wrong number that looks measured is worse than no number: I reported
\"sent 0 times\" as evidence, built a fix on it, and only the trace from the *next* run contradicted it. The
lesson is the one this phase keeps teaching from the other direction — the earlier corrections came from
comparing against something real, and this error came from not doing that: I never looked up the ids in the
table that exists for exactly that purpose.

**What is still true and still unexplained.** The client does sit on the loading screen. It receives
`join_game`, the chunk-cache centre and radius, its position, and 289 chunks including the one it stands in. So
the cause is something else, and it is **not known**.

**One latent bug found on the way.** `connection.rs:542` sends `SetChunkCacheCenter { x: 0, z: 0 }`
**hard-coded**. The test world's spawn happens to be chunk (0, 0), so it is not this symptom, but a player
spawning anywhere else would be given the wrong centre. Recorded rather than fixed here, since the join path is
not something to change again without knowing what is actually wrong.

### KD-50 (continued) — two hypotheses tested and both disproved

The client still sits on the loading screen. Two explanations were tested against the jar and **neither holds**,
which is worth as much as a cause would be: it removes them from the search and it records that the obvious
answers are wrong.

**Hypothesis 1, disproved: the chunk-cache packets.** I claimed `set_chunk_cache_center` and
`set_chunk_cache_radius` were never sent. They were, all along, by the network layer at
`connection.rs:542` — and I had measured the trace for ids **11** and **12**, which are
`chunk_batch_finished` and `chunk_batch_start`. The real ids are **94** and **95**, and our own `ids.rs` has
always said so. My "sent 0 times" was an artefact of labelling the packets from memory. The duplicate sends
this produced are reverted.

**Hypothesis 2, disproved: the chunk-batch protocol.** 26.x has `chunk_batch_start` (12) and
`chunk_batch_finished` (11), which we never send, and the loading screen is driven by a `LevelLoadTracker`
whose `loadingPacketsReceived()` looked like the gate. `javap` on
`ClientPacketListener.handleChunkBatchFinished` shows it calling only `ChunkBatchSizeCalculator.onBatchFinished`
and replying with `ServerboundChunkBatchReceivedPacket`: **the batch is for pacing, and it does not touch the
load tracker.** So it is not the gate either.

**What the client's own classes say.** `LevelLoadingScreen` dismisses on `LevelLoadTracker.isLevelReady()`, and
the tracker holds a `ChunkLoadStatusView` the server can push, plus a `CLIENT_WAIT_TIMEOUT_MS` and a
`LEVEL_LOAD_CLOSE_DELAY_MS`. There is also a `ServerboundPlayerLoadedPacket` (serverbound 44), a handshake our
server does not model — it logs it as an unmodelled packet at most.

**What is established, and what is not.** The client receives `join_game`, the cache centre and radius, its
position, and 289 chunks including the one it stands in, and it reports no protocol error. **Why
`isLevelReady()` stays false is not known.** The next step is concrete and small: find what calls
`LevelLoadTracker.loadingPacketsReceived()` in `ClientPacketListener` — it is at bytecode offset 658 and
is neither chunk-batch handler — and read `isLevelReady()`'s actual condition rather than inferring it from
method names.

**One latent bug found on the way**, recorded but not fixed: `connection.rs:542` sends
`SetChunkCacheCenter { x: 0, z: 0 }` **hard-coded**. The test world's spawn happens to be chunk (0, 0), so it is
not this symptom, but a player spawning elsewhere would be handed the wrong centre.

### KD-50 closed — the missing `game_event(LEVEL_CHUNKS_LOAD_START)`, and the client is in the world

**The owner confirmed the client enters the world** after this fix. Before it, the client sat on "Loading
terrain" indefinitely with every packet well-formed and no error anywhere.

**The chain, every link measured rather than inferred:**

1. `LevelLoadingScreen` dismisses on `LevelLoadTracker.isLevelReady()` (`javap` on the **client** jar).
2. `isLevelReady()` is true only once `clientState` has become `ClientLevelReady`, and `startClientLoad` puts it
   in **`WaitingForServer`** (`javap`).
3. The **only** caller of `LevelLoadTracker.loadingPacketsReceived()` — the thing that moves it out of that
   state — is `ClientPacketListener.handleGameEvent` (`javap`).
4. `ClientboundGameEventPacket` has an event type **`LEVEL_CHUNKS_LOAD_START`** (`javap`).
5. A real vanilla server sends it on join, and the capture gives the wire values with nothing inferred:
   `game_event` (clientbound play 38), body 6 bytes, **`26 0d 00000000`** — id 38, event **13**, value
   `0.0`.
6. Our join sequence contained **no `game_event` at all**: `49, 94, 95, 95, 94, 72, 97, 104, 103, 121`.

Our server now sends it, and the packet is **byte-for-byte identical to the real server's** — `26 0d
00000000`, at the same point in the join sequence, between the chunk-cache packets and the teleport.

**Two wrong turns are recorded because they cost real time and both had the same shape.** First I claimed the
chunk-cache packets were never sent, having labelled their ids from memory as 11 and 12 — which are
`chunk_batch_finished` and `chunk_batch_start`; the real ids are 94 and 95 and they were always sent, so the
"fix" duplicated them and had to be reverted. Then I hypothesised the chunk-batch protocol was the gate, and
`javap` on `handleChunkBatchFinished` disproved it: it only paces the calculator and never touches the load
tracker. **The successful conclusion came from reading the client's own bytecode and then taking the number
from a real server's wire — not from reasoning about names.** That is the same lesson as KD-44, KD-46 and
KD-49, and this time it was learned from the other side.

**One latent bug found on the way**, recorded but not fixed: `connection.rs:542` sends
`SetChunkCacheCenter { x: 0, z: 0 }` **hard-coded**. The test world's spawn happens to be chunk (0, 0), so it is
not this symptom, but a player spawning elsewhere would be handed the wrong centre.

**What this changes for the phase.** The claim that a real client is "live and rendering" was withdrawn as
unproven; it is now **established**, by the owner seeing the world.

### KD-51 — the black surface blocks are not our light data, established by eliminating six of my own errors

The owner is in the world and reports **a small number of surface blocks dead black**. The engine and the wire
were both checked against the invariant that decides it — **a cell with only air above it is open to the sky,
and open to the sky means 15** — and **both pass**:

* the **engine**, over a 9x9 of chunks of generated terrain, so chunk borders are included;
* the **wire**, over 53 chunks a real session captured, reconstructing the light the way a client does: a set
  bit takes the next array in order, a bit in `empty_*` is that layer's default, and neither mask means zero.

So the black blocks do **not** come from the light we send. `light_update` is not a separate suspect either: it
and `level_chunk_with_light` are written by the same `light_fields` and `write_light_data`, so its light values
are the same code that the verified chunk packets use.

**Six errors, all mine, all in the measuring rather than the measured.** They are listed because the shape
repeats and each one cost real time:

1. **a packet id labelled from memory** — I wrote "(id 12)" for `set_chunk_cache_center` and measured the trace
   for it; 12 is `chunk_batch_start`. The real id is 94, the packets were always sent, and the "fix" this
   produced sent them twice and had to be reverted (KD-50).
2. **a body read from byte 0** — the rig's `head` includes the packet id, so `level_chunk_with_light`'s chunk x
   decoded as 754 974 720, which is `0x2D` (its own id) followed by three zeros.
3. **a mask read as bitmask words** — the masks are lists of set section **indices**, so nine indices looked
   like one set bit against nine arrays and produced a confident mismatch report.
4. **a light section indexed without its one-offset** — light section `i` holds world section `i - 1`, so
   reading `offset / 16` looked up the section *below*: underground, where sky light genuinely is 0.
5. **a block column read a section low** — the same offset applied to blocks, which printed a coherent-looking
   tree sixteen levels below the one beside it.
6. **a "uniformly lit" array filled with `0x0F` instead of `0xFF`** — this is the one that mattered. Every byte
   had a low nibble of 15 and a **high nibble of 0**, so every cell at an odd index read as dark. It produced a
   finding of **"2176 of 13568 surface cells dark"** that was entirely fictitious, and the tell was in the data
   all along: exactly **half** of every affected chunk, always at odd `x`.

**The pattern.** Errors 2\u20136 were all caught by printing the actual bytes and none by re-reasoning about them,
and 1 was caught only because a later trace contradicted it. That is the same lesson as KD-44, KD-46, KD-49 and
KD-50, now from the side of the measurer rather than the measured.

**Two real defects were found on the way**, both recorded rather than folded in:

* `connection.rs:542` sends `SetChunkCacheCenter { x: 0, z: 0 }` **hard-coded**; the test world's spawn happens
  to be chunk (0, 0), so a player spawning elsewhere would be handed the wrong centre;
* a capture session logged **`outbound queue full; the player will be disconnected`** repeatedly, and a
  **tick of 3618 ms against a 50 ms budget** — the light work of KD-45 landing on one tick.

**Where the black blocks must come from instead.** With the light values excluded, the remaining suspects are
outside them: the **block-state ids** the chunk palette carries, which a client resolves against the registry we
sent it and would render as the wrong block if the two disagreed; or client-side rendering of the sections
themselves. Both are testable the same way — against what a real client does with what we send.

### KD-52 — a real decoder bug, found at the end of eight errors of my own

**`PalettedContainer::decode` put the block-state id where the palette index belongs.** For the single-value
form (`bits = 0`) it returned

```rust
palette: vec![value],
values: vec![value; entries],   // the id in every slot
```

and `values` is documented as **indices into `palette`**, so every slot must be `0`. The result is that
`palette[values[i]]` is an out-of-range read for every container whose single value is not zero.

**It hid because the single-value form is overwhelmingly used for air, whose state id is `0`** — so the
wrong index and the right one are the same number, and every round-trip test agreed with itself. It showed only
for a uniform section of something else: a chunk section that is **entirely leaves**, `palette=[86]`,
`values=[86, 86, ...]`.

**The test fixture had the same misunderstanding.** `uniform_section`, which every chunk test is built from,
constructed `values: vec![block_state; BLOCKS_PER_SECTION]` — the decoder's bug written a second time, in
the fixture, so six tests failed the moment the decoder was corrected. **This is KD-44's shape exactly**: an
implementation and its tests sharing an assumption, green together and wrong together.

**What it cost.** With the block read of a leaf section coming back as air, a correctly-shaded canopy looked like
a cell open to the sky reading `14`, and the search went after the light engine. It ended when the engine was
asked directly: the world has leaves through `y = 48..63`, the packet's own section 7 is `palette=[86]`, and the
light is right.

### KD-49 corrected — the acceptance test rested on a packet that was never sent

The `light_update` capture contained **zero** id-48 packets. The driver announced "breaking the block at ..."
before sending anything, and the server **refuses a break in a chunk it has not loaded** — which is exactly
the state a client is in right after `join_game`. So the run that concluded "a real client accepts our
`light_update`" had no `light_update` in it, and the client accepted nothing.

The driver now **waits for the chunks** rather than a fixed sleep, and the same run produced **three**
`light_update`s. A test then checks their content: every light section appears in exactly one mask, the array
count matches the set bits, and the surface invariant holds after the update. **All three pass.**

### What is now established about the light

Three independent checks, all passing:

* the **engine**, over a 9x9 of chunks of generated terrain, at the **production seed** (`DEFAULT_RANDOM_SEED`
  is `0`; the test had been choosing its own, so it was examining a different world from the capture);
* the **wire**, over **81 chunks** a real session captured, reconstructed the way a client reads it;
* the **`light_update`** that a block change produces.

**Eight measurement errors of mine were eliminated on the way**, each found by printing bytes rather than
re-reasoning: a packet id labelled from memory, a body read from byte 0, a mask read as bitmask words, a light
section indexed without its one-offset, a block column read a section low, a "uniformly lit" array filled with
`0x0F` instead of `0xFF`, a capture compared against a different run's trace, and a `MIN_SECTION_Y` assumed
rather than looked up. The white whale was a real bug, but seven of the eight were noise, and every one of them
was a measurement rather than a subject.

### Tooling corrections

* `tools/surface-capture/run.py` now removes the **world** and the **trace** as well as the bodies: an existing
  world is never regenerated, so a reused one examines terrain from a different seed, and an appended trace has
  `seq` numbers that no longer match the per-run body file names.
* the capture driver waits for chunks before digging, which is what made the `light_update` capture empty.

### KD-53 — the world was an ocean, and the spawn was in it

**The owner reported that terrain and biomes were wrong**: no trees, no structures, only "dirt variants and
stone". The blocks confirmed it — the chunks the server had sent use **exactly three states**, `stone`,
`water` and `sand` — and none of that is a generation defect. Those three states are what an **ocean** is made
of.

Sampling the height field over a 1024-block square says the generator is healthy:

```text
height: min 24, max 112, mean 66      sea level 63
below sea level: 39.2%
biomes: ocean 39.2% \u00b7 plains 26.6% \u00b7 forest 25.3% \u00b7 desert 4.6% \u00b7 taiga 3.9% \u00b7 mountains 0.5%
```

**Six biomes, sensible heights, and two columns in five are ocean.** The defect was where the player starts: a
fresh world's `level.dat` names `(0, 64, 0)`, and `(0, 0)` is water. With a view distance of four to eight
chunks, everything visible was sea bed — no grass, no trees, no biome variety, and an ocean floor that is
**correctly** dark because water attenuates sky light. Every part of the report follows from one hard-coded
spawn.

**Fixed by searching for land**, as vanilla does, and only when the stored spawn is **in water**: a stored
world's own choice is left alone. `crates/server/tests/spawn_on_land.rs` asserts it, and asserts that the search
*moved* the spawn rather than the origin having been dry by luck.

### KD-54 — `~12` and `12` were the same value, so `/tp ~` ignored the player

Fixing the spawn broke `command_e2e`'s relative-teleport test, with `-7.5 -> 1.5`. The reference was right
(`base=(-8, 66, -8)`) and the resolution was wrong:

```rust
let resolve = |offset: Option<i32>, base: i32| offset.unwrap_or(base);
```

`~1` arrived as `Some(1)`, so `unwrap_or` returned **`1`** — the offset was used as an absolute coordinate and
the source's position was discarded.

**The argument type could not have done better.** `parse_axis` produced `Some(12)` for both `12` and `~12`, so no
consumer could tell them apart; its own doc comment claimed the two were "distinguishable once the source
moves", which was true of the intent and false of the code.

**And the test was vacuous.** It teleported the player to the spawn and then stepped with `~1` — while the
spawn *was* the origin, so `0 + 1` and the right answer were the same number. It passed without the property it
named. It now acknowledges the teleport (the server holds a teleport pending until the client confirms, which is
vanilla's `awaitingPositionFromClient` and correct), and the parser test asserts the thing the type exists for:
**`12` and `~12` must differ, and must resolve to different places from a source away from the origin.**

`Coordinate { value, relative }` replaces `Option<i32>`, with `resolve(base)` as the one place the two are
combined. Bare `~` and `~0` collapse to the same value, which is right — both mean "the source's own
coordinate" — and the old comment's insistence that they differ was part of the same confusion. **This affects
every command that takes coordinates, not just `/tp`.**

### KD-55 — no world this server generated ever contained a tree

**`TerrainGenerator::generate_chunk` produces terrain only.** Trees are `TerrainGenerator::decorate`, a separate
pass — "terrain and decoration are two passes in Vanilla too", as its own doc says — and **the server never
called it**. `decorate_with_structures` runs structures and nothing else, and returns early when no structure
templates are loaded, which they are not.

**The block census settled it.** Over a 5x5 of chunks:

```text
stone 256758 \u00b7 water 10754 \u00b7 sand 9114 \u00b7 dirt 4724 \u00b7 grass_block 2362 \u00b7 podzol 2000 \u00b7 coarse_dirt 1000
```

Grass over dirt over stone, podzol and coarse dirt for taiga, sand and water for ocean — **the biome surface
rule is working perfectly** — and not one `oak_log` or `oak_leaves` anywhere.

**That is why the world read as broken terrain rather than as an unlit one.** Every block was the right block for
its biome; the features that make a biome recognisable were simply absent, and nothing anywhere said so: the
chunks were well-formed, the light was right, the biomes were right, and no unit test of terrain, biomes, blocks
or light can see a missing feature pass because none of them is wrong.

**Fixed** by calling `decorate` after structures, with a running `TreeStats` on the game so that "no trees" is
something a counter reports rather than something a player has to notice. `crates/server/tests/world_features.rs`
asserts both that the pass ran and that logs and leaves are **in the loaded chunks**, not merely counted.

**The order is terrain, structures, trees.** Structures already ran after terrain and that is unchanged; trees
go last so one cannot be planted through a structure placed a line earlier.

### KD-56 — `default_state` returned the lowest state id, and 642 of 1168 blocks disagree with it

**The owner looked at a tree and said what was wrong:** "the leaves contain water, the logs are lying on their
side". Both are one mistake.

`BlockRegistry::default_state(name)` returned `first_state_id`, and its doc comment said *"the id of this
block's first (default) state"* — **the assumption written into the comment**. Vanilla chooses the default
explicitly with `registerDefaultState`, and it is not in general the lowest id:

```text
minecraft:oak_log     137   (lowest 136 -> axis=x;  137 is axis=y)
minecraft:oak_leaves  279   (lowest 252 -> distance 1, persistent, waterlogged=true)
minecraft:grass_block   9   (lowest   8 -> snowy=true)
```

A probe of the jar says **642 of 1168 blocks** differ, **55%**. So every log a world generator placed lay on its
side, every leaf held water, and every grass block was snowy — and none of it had an error anywhere: the
blocks were all real, the light was plausible, and the chunks were well-formed.

**Extracted, not guessed.** `tools/vanilla-probe/DefaultStateProbe.java` boots the server's own registry and
reads `Block.defaultBlockState()`, the same method `LightProbe` uses, and writes
`crates/test-support/fixtures/registry/block_defaults.tsv`. The registry reads it beside `blocks.tsv`; a missing
table warns rather than failing, because the difference between two deployments must not be silent.

**Three call sites conflated the two**, and all three were wrong: `default_state`, `state_id`'s empty-property
path, and the absence of any table to consult.

### The tests that agreed with it

Two asserted the bug as an expectation, and both said so in their own words:

* `structure.rs` compared a resolved `axis=y` log against `default_state("minecraft:oak_log")` and required them
  to **differ**, with the comment *"a different id from the property-less **first** state"* — naming the
  thing it was really comparing against. It now asserts that resolving `axis=y` names the default, and that
  `axis=x` is what differs, which is the property it existed for.
* The doc comment on `first_state_id` read *"the id of this block's first (default) state"*.

**This is the third time this round that a test encoded the implementation's mistake** — after KD-52's
palette fixture and KD-49's vacuous teleport — and the pattern is worth naming: the suites pass because the
code and the tests were written from one understanding, so an audit of either confirms the other.

### KD-57 — the review's second clue: where every fixture came from, and the two that cannot say

**Every fixture in `crates/test-support/fixtures/` now has a known source**, which is the property that decides
whether a test can disagree with the wire at all:

| fixture | source |
|---|---|
| `anvil/level_26_1_2.dat`, `anvil/region_26_1_2.mca` | **a real server** — the manifest records the jar's sha1, the seed, and a sha256 per file |
| `registry/blocks.tsv` | jar (`DumpRegistries` + `compact_blocks.py`, with ids verified before writing) |
| `registry/block_light.tsv` | jar (`LightProbe.java`) |
| `registry/block_defaults.tsv` | jar (`DefaultStateProbe.java`, KD-56) |
| `registry/items.tsv` | jar — **verified this round**, see below |
| `protocol/handshake_login.hex` | hand-assembled — **verified against a real client**, see below |
| `protocol/frame_uncompressed.hex` | hand-written, no source stated |
| `protocol/nbt_literal_text.hex` | hand-written, **never compared to anything real** |

**`items.tsv` was the one table that only claimed a source.** Its header says "Vanilla 26.1.2 item registry
order" and nothing in the repository would have failed had it been wrong — a transcription error in 1 506 ids
would leave every lookup succeeding and naming the wrong item. `tools/vanilla-probe/ItemProbe.java` now extracts
the same three columns from the jar, and the two agree **row for row, 1506 of 1506, zero differences**.

**`handshake_login.hex` was the one golden byte string written from a reading of the spec** rather than
captured — the shape every confirmed failure of this review has had. A real 26.1.2 handshake, captured
through the rig, is `00 87 06 09 <"127.0.0.1"> 63 eb 02` against the fixture's `87 06 09 <"localhost"> 63 dd 02`:
**identical in every field the test exercises**, and it verifies.

**Two fixtures still cannot say where they came from**, and one of them has never been checked against anything:

* `frame_uncompressed.hex` is four bytes and is self-consistent by inspection — `03` is the length of
  `2A 01 02` — which is why it has not mattered;
* **`nbt_literal_text.hex` claims to be "network NBT for the text component `{"text":"bye"}`" and has never been
  compared with network NBT from a real server.** The vanilla captures contain no `system_chat` at all, so
  nothing in the repository can contradict it, and comparing it with our own server's chat would be the
  round-trip trap this review exists to find. The next capture must have the vanilla server say something.

**What this round did not do:** clues 3 and 4 — tests that cannot fail, and doc comments that describe a
semantics the code does not implement — are untouched. Both have already produced confirmed findings
(KD-49, KD-54, KD-56), and both are still open.

### KD-58 — the last hand-written fixture is checked, and `say` does not use `system_chat`

**`nbt_literal_text.hex` had never been compared with anything real.** No vanilla capture contained a
`system_chat`, so nothing in the repository could contradict it, and comparing it with our own chat output would
have been the round-trip trap this review exists to find. The fix was to make a real server say something: a
vanilla 26.1.2 server, a connected client, and `say bye` on the console **after** the join.

**The encoding verifies.** The real packet carries

```text
08 00 03 62 79 65  05 08 00 06 53 65 72 76 65 72  00
^TAG_String ^len3 "bye"
```

— tag type `08`, a two-byte name length, UTF-8 payload, exactly the form the fixture uses for its string
entry. Vanilla wrote the **bare-string** form of the component there and the fixture writes the **compound**
form; both are valid, and the encoding the test exercises is the one they share.

**And the capture turned up a parity difference.** The console `say` is carried by **`disguised_chat`
(clientbound play 33)**, not `system_chat` — two commands, two packets, and `system_chat` (121) appears zero
times. Our server uses `system_chat` for its own welcome message, which is a legitimate use of that packet, but
**a `/say` implemented with it would be wrong**, and nothing in the repository says which of the two a given
message belongs in.

**Two of my own errors on the way**, both of the kind this review keeps finding:

* I filtered the capture for **id 119** and then for `system_chat`, and reported "no chat was sent" twice. The
  table said **121** all along — the same mislabelled-id mistake as KD-50, made again after recording it as a
  lesson. The packets were in the capture the first time.
* The first two capture attempts failed on `server.properties`: the vanilla server defaults to port **25565**,
  which this project must not bind because it belongs to the owner's own server. Both the port and offline mode
  are now set before boot, and `eula.txt` with it, which a fresh scratch directory does not have.

### KD-59 — the review's third clue: tests that cannot fail, and why the interesting half resists a search

**The crude form does not exist here.** A scan of every test in the workspace — around twelve hundred — for
bodies that name no `assert`, no `expect`, no `unwrap`, no `panic!`, and no helper called `check_*`, `verify_*`
or `ensure_*`, returns **nothing that is actually vacuous**. The four candidates it did surface were each
verified by reading them: `byte_compare_passes_on_equal_input` and `fractal_noise_matches_five_frozen_values`
assert through helpers named `assert_bytes_eq` and `assert_bits`; `every_tag_type_round_trips_on_disk` calls a
`round_trip_disk` that carries three assertions; and `the_whole_pipeline_runs_against_the_real_pack` drives four
`stage_*` helpers carrying three, seven, nine and six.

**Two versions of the filter were wrong before that answer was trustworthy**, and both failed the same way — by
producing a tidy list:

* `\bassert\b` does not match `assert_bytes_eq`, because `_` is a word character, and `\bpanic!\b` does not
  match `panic!(..)`, because `!` is not one. It reported **sixteen** tests.
* The naming conventions it then looked for were incomplete, so it reported **two**.

**A tidy list is not a correct one** — which is the failure this review exists to find, arriving this time in
the tool doing the reviewing.

### And the interesting half cannot be found this way at all

KD-49's relative-teleport test had a real assertion, and it was structurally satisfiable: it teleported the
player to the spawn and stepped with `~1` **while the spawn was the origin**, so `0 + 1` and the correct answer
were the same number. No scan of test bodies can see that. It took **moving the spawn** — an unrelated change
— for the assertion to become capable of failing, and it failed immediately.

KD-52's palette fixture has the same shape: a test helper that encoded the decoder's own misunderstanding, found
only when the decoder was corrected for an unrelated reason.

**So clue 3's method is not a search, it is a perturbation**: change an input the tests hold fixed — a spawn
point, a seed, a coordinate, a default — and see which assertions stop holding. Both of this review's
clue-3 findings arrived that way by accident, from changes made for other reasons. Making it deliberate is the
remaining work.

**What this round did change.** Nothing in the product. The filter is committed as
`tools/review/scan_vacuous_tests.py`: its answer is negative **today**, and a check whose answer is none is
worth re-running after the next round of changes rather than rewriting from memory — which is exactly how the
two broken versions of it happened.

### KD-60 — the perturbation method works, and it found KD-49's shape in my own test on the first try

Clue 3's crude half — a test that cannot fail — is absent from this codebase (KD-59). The half that matters
cannot be found by reading tests at all, because the assertion is real and merely **structurally satisfiable for
one input**. So the method is to **perturb an input the tests hold fixed** and see which assertions stop holding.

**The first perturbation was the world seed**, from 0 to 12345, and it failed **exactly one test in the
workspace**:

```text
---- a_fresh_world_spawns_the_player_on_land stdout ----
```

That test is mine, written last round, and its assertion said the thing out loud:

```rust
assert_ne!((sx, sz), (0, 0),
    "the default spawn at the origin is ocean at this seed, so a spawn still there means no search ran");
```

**"at this seed"** — in a test that uses whatever the production seed is. At seed 0 the origin is ocean and the
assertion holds; at any other seed the origin may be dry, the search correctly does nothing, and the test fails
**having found no defect**. That is KD-49's shape exactly: satisfiable for one input, unsatisfiable for another,
with nothing in the test saying which it needs.

**The fix is a split**, and it is what the perturbation taught:

* the **property** stays with the production seed — a fresh world spawns the player on land, true whatever the
  seed;
* the **evidence that the search runs** moves to its own test which **names the seed it needs**, because it is
  *about* that precondition: `WATER_AT_ORIGIN_SEED = 0`, and it asserts the precondition (the origin is under
  water) before asserting the consequence.

**And the perturbation was re-run to close the loop: 1240 passed, 0 failed, 86 suites**, with the seed restored
and `git diff` clean.

**What this suggests for the rest of the review.** Two of this review's findings arrived by accident from
changes made for other reasons — KD-49 from moving the spawn, KD-52 from correcting the palette. Deliberate
perturbation found a third **on its first attempt**. The inputs worth perturbing next are the ones the suite
holds fixed and the code assumes: coordinates (many tests use the origin or `(8, 8)`), the view distance, chunk
section counts, and the tick counts a test waits for.

### KD-61 — the second perturbation: a tick count standing in for a property

`CHUNKS_PER_TICK` from 64 to 8 failed one test, and again **having found no defect**:

```text
assertion `left == right` failed: the view distance must be exactly (2r+1)^2 chunks
left: 72
right: 81
```

```rust
let expected = ((2 * view_distance + 1).pow(2)) as usize;   // 81, a property of the view distance
for _ in 0..8 {                                             // 8,  a property of CHUNKS_PER_TICK
    tick();
    total += chunk packets;
}
assert_eq!(total, expected, "the view distance must be exactly (2r+1)^2 chunks");
```

**The server streamed 72 of 81 chunks in the eight ticks the test allowed, which is correct behaviour** — a
view is streamed over as many ticks as the budget needs. The test had baked the budget it happened to run with
into an assertion about the view distance.

**Same shape as the seed perturbation an hour earlier** (KD-60) and the same shape as KD-49: **an assertion
structurally satisfiable for one value of an input it never names.** The fix is to **wait for the count** with a
deadline generous enough for any budget the server ships, which is what the test meant in the first place.

**And the loop closed**: with the fix in, the same perturbation now passes — **1240 passed, 0 failed, 86
suites** — with the budget restored and `git diff` clean.

**Two perturbations, two findings, both closed loops.** Every one is a test that was green for a reason other
than the property it names, and none of them could have been found by reading the tests: in both cases the
assertion is real, and the input that makes it unable to fail is one the suite holds fixed.

**A process note, recorded because it is now twice.** I committed with a failing `cargo clippy` in the previous
commit (KD-60's doc comment needed fencing). It was caught and fixed immediately, and it is the second time this
review has committed over a failing gate — the first being `check_line_endings` in KD-57. **The gates are run
before the commit in both cases; what fails is reading their output as a formality once the interesting work is
done.**

### KD-62 — two more perturbations, and the one test in the tree that guards against this review's subject

**`LIGHT_UPDATES_PER_TICK` from 4 to 1: nothing failed.** 1240 passed, 0 failed. The light-update tests wait for
what they assert rather than assuming a budget, so lowering it changed nothing. A perturbation that finds
nothing is worth recording as such — otherwise the method reads as though every input hides a defect.

**The view-distance clamp from `(2, 16)` to `(2, 2)`: one failure, and it is the harness working correctly.**

```rust
// The replay is not accidentally empty: the game did real work in both runs.
assert!(
    first_reports.iter().any(|report| report.chunks_sent > 0),
    "the script streamed chunks"
);
```

At a view distance of two the join sends the whole 5x5 view itself, so no *subsequent* tick has a chunk in it,
and this guard fires. **It is a sentinel against the determinism test becoming vacuous**, and at that input the
test genuinely is vacuous — so failing is the correct behaviour, not a defect.

**It is the only assertion of its kind in the codebase**, and it anticipates exactly what this review has spent
four rounds finding: a test that passes for a reason other than the property it names. `the same seed replays
the same tick reports` compares two runs, and two empty runs compare equal; the guard is what stops that from
counting as a pass. Every other test examined here would have been improved by one.

**That is the positive result of the perturbation work.** Three perturbations found two defects (KD-60, KD-61)
and one deliberate guard; the guard is the pattern worth copying, and the four rounds of this review are the
argument for it.

### KD-63 — clue 4 opens with a fourth instance of the same sentence

Three of this review's confirmed failures came from **prose that names two ideas side by side and conflates
them**: `first_state_id`'s "first (**default**) state" (KD-56), `parse_axis`'s "distinguishable from `~0`"
(KD-54), and `values` documented as indices (KD-52). The first file clue 4 opened has a fourth:

```rust
/// Whether this stack is within the limits [`ItemStack::new`] enforces.
pub fn is_valid(&self) -> bool {
    self.item_id >= AIR_ITEM_ID && self.count >= 0 && self.count <= HARD_MAX_STACK_SIZE
}
```

`ItemStack` promises **three** things at line 66 — `item_id >= 0`, `0 <= count <= 64`, and **`item_id == 0`
implies `count == 0`** — and `new` enforces all three by returning `EMPTY` whenever the id is air. `is_valid`
checks two, so its doc names a superset of what it does.

**It is benign, and why it is benign is the part worth writing down.** The third invariant cannot be violated
through the public API: every path into an `ItemStack`, **including the decode path in `player.rs` that
inventory spoofing would use**, goes through `new`. So the omission is covered **by construction rather than by
this function** — and nothing in the code said which.

**So the doc moves and the code does not.** Adding the check would add a branch that can never be taken: dead
code dressed as a defence, which is worse than the sentence it replaces. The doc now says exactly what is
checked and where the rest is kept, and
`a_valid_stack_covers_the_whole_guarantee` **pins the relationship** — it asserts that everything `new`
accepts satisfies the third invariant, so a later change that let `new` build `{item_id: 0, count: 5}` fails
there rather than producing a stack this function waves through.

**Clue 4's method, stated.** `tools/review/` now carries a scan for prose that makes a falsifiable claim: the
words `default`, `always`, `never`, `only`, `exactly`, `same as`, `equivalent`, `identical`, `must`, `cannot`,
`distinguishable`, `guarantee`, `invariant`. There are **1044 such lines** in product code, which is too many to
read, and the counts are what make it usable: `distinguishable` appears three times and `(default)` in
parentheses also three, and those are the two signatures of the defects already confirmed. This finding came
from six of those lines.

### KD-64 — clue 4's equality claims are clean, and that says where the defects live

The signatures that **equate two things** — `same as`, `equivalent`, `identical` — are about twenty-five doc
lines in product code, and every one is checkable by looking at both sides. Four were read in full:

* `ClientInformation::encode_body`'s `# Errors` says "Same as `Packet::encode`" — looser than the others, since
  the two write different bodies, but both write the same fields and the error conditions coincide;
* `Packet::to_raw`'s says "Same as `Packet::encode`" and its body is `Ok(RawPacket::new(Self::ID, self.encode()?))`
  — **the only error source is that call**, so it is exact;
* `PacketWriter::write_identifier`'s says "Same as `write_string`" and its body is
  `self.write_string(&value.to_string())` — likewise exact;
* `mc_entity::Vec3`'s says the parallel `mc_world::Vec3` is "structurally identical and conversion is a field
  move" — and both are exactly `{ x: f64, y: f64, z: f64 }`. **Correct.**

**None is a defect.** That is worth recording rather than passing over, because it locates the problem: every
prose defect this review has found — KD-52, KD-54, KD-56 and KD-63 — is in prose that **defines a term**
(`default`, `distinguishable`, `indices`), not in prose that **equates two things**.

The difference is not stylistic. "A is the same as B" is checkable in one reading, and the author writing it has
both sides in front of them. "The default state" is a term the author believes they know, and the belief is what
turns out to be wrong — 642 times, in KD-56's case.

**So clue 4's remaining work is the defining prose**: `(default)` in parentheses (three lines, one read, one
verified correct), `must` and `cannot` (358 lines), and `invariant` (48). The counts are what make 1044 lines
readable, and they now have a direction.

### KD-65 — every chunk said `badlands`, which is what "the terrain and the biomes do not generate correctly" was

```rust
// Biome ids are not modelled in P04: one plains biome fills every cell.
const PLAINS_BIOME_ID: u32 = 0;
```

**A constant named for one biome and valued for another.** The client resolves a chunk's biome ids against the
registry this server hands it — a verbatim replay of vanilla's — and in that registry **id 0 is
`minecraft:badlands`**. So every column of every chunk was painted as badlands: **red sand and orange terracotta
under a hazy sky**, wherever the player stood, with the terrain, the blocks and the light all correct.

**That is the owner's report, precisely.** A biome decides the colour of grass, leaves and water, the sky and the
fog — so a world painted one wrong biome looks broken everywhere and nothing errors. It was reported as a
terrain and generation problem, and three rounds of this review went after light, palettes and features before
this.

**Measured, not guessed.** `crates/network/src/registry_data/config-payload.bin` is the exact byte sequence the
client receives. The identifier run after `minecraft:worldgen/biome` is **65 names in alphabetical order**,
ending at `minecraft:chat_type` — the next registry, which is where the run stops being alphabetical:

```text
0 badlands \u00b7 21 forest \u00b7 35 ocean \u00b7 40 plains \u00b7 64 wooded_badlands
```

**`minecraft:plains` is id 40.** The constant now says 40.

**Three attempts to read that list.** A 20 000-byte window collected 384 "biomes" including `minecraft:11`,
`minecraft:moon` and `minecraft:villager_schedule` from later registries, and printed `plains at 40` **by
coincidence**. A "stop at the first name containing a slash" rule failed the same way. The third bounded the
section by the **longest strictly-increasing prefix**, which is self-validating: the next registry breaks the
alphabetical order, so the prefix ends exactly where the biomes do, and the 65-name count matches the biomes
vanilla ships. **Two of the three produced a confident number that meant nothing**, and only the third's
boundary can be checked from the data.

### The class, and why it keeps appearing

KD-56 was a block's default state assumed to be its lowest id. KD-65 is a biome assumed to be id 0. **Both are a
number sent to a client, resolved by a rule that was assumed rather than looked up**, and neither had anything in
the repository that could contradict it — the prose said what the number was for, and the number was never
compared with the registry it indexes.

Per-column biomes are still not modelled: `Biome::index()` is this crate's own six-biome slot, a **different
numbering** from the client's registry, so sending it would be a new defect rather than a fix. That is recorded
rather than half-done.

### KD-66 — the sweep is done, and the rule is: an id with an assertion is right, an id without one is wrong

Every numeric registry id this server puts on the wire, and where each comes from:

| id | source | verdict |
|---|---|---|
| block state | `blocks.tsv`, jar-derived, ids verified before writing | **was wrong** — KD-56, `default_state` returned the lowest id, wrong for 642 of 1168 blocks |
| biome | the registry the client is sent, read out of `config-payload.bin` | **was wrong** — KD-65, every chunk said `badlands` |
| item | `items.tsv`, jar-derived | **correct** — verified row for row by `ItemProbe`, 1506 of 1506 |
| dimension type | `0`, hard-coded | **correct, and asserted** |
| block entity type | `4` for a chest, in a golden test | **correct**, and the golden bytes came from a vanilla capture |

**The dimension type is the one that shows what was missing elsewhere.** `connection.rs:529` sends
`dimension_type_id: 0` and `registry_data/mod.rs:284` writes the assumption down:

> `join_game` sends `dimension_type_id: 0`, so entry 0 of that registry must be the overworld.

**and then line 311 asserts it**: "entry 0 must be the overworld, because join_game references dimension_type id
0". Reading the registry out of the payload we send confirms it — entry 0 is `minecraft:overworld`, the
registry is four long, and `minecraft:damage_type` begins right after it.

**So the pattern is not "these ids are hard".** It is that **the two ids with nothing checking them were both
wrong, and the three with something checking them are all right**:

* the block-state id had a jar-derived table whose *rows* were verified and whose *default column did not exist*
  — the check was one column narrow, and the missing column was the one that mattered;
* the biome id was a constant named for one biome and valued for another, with a comment admitting the ids were
  not modelled;
* the item id had an independent extraction and is exact;
* the dimension type id has an assertion naming the client's registry as the reason;
* the block entity type id is pinned by golden bytes captured from a real server.

**What follows for the rest of the project**, and it is the single most useful thing this review has produced:
**a number sent to a client is a claim about a registry the client owns, and it needs the same evidence as any
other compatibility claim** — a jar extraction, a capture, or an assertion that names the registry. The two
that had none were both wrong, in ways that produced a world of sideways waterlogged logs and then a world of red
sand, neither of which errored anywhere.

### KD-67 — the regression test KD-65 was fixed without, and proof that it has teeth

**KD-65 was fixed with no test.** `PLAINS_BIOME_ID` went from 0 to 40 and nothing stopped it, or the next
constant like it, from going back. `crates/server/tests/registry_ids.rs` now reads the registry blob the client
is sent and holds the constant against it:

* the biome registry's identifiers come out **alphabetical**, ending where the next registry breaks the order, so
  the **longest strictly-increasing prefix** is the registry itself. That is self-validating: if the payload ever
  stops carrying them that way the prefix collapses and the test says which of the two happened rather than
  asserting against a number it invented.
* `PLAINS_BIOME_ID`'s index into that prefix must name `minecraft:plains`.
* and `dimension_type_id 0` must name `minecraft:overworld`, which the code already asserted in
  `registry_data/mod.rs:311` — the one id in the sweep that had a check and was right.

**Verified by perturbation, because a test that has never failed is not a test.** Setting the constant back to
the value KD-65 shipped fails it with

```text
PLAINS_BIOME_ID is 0, and the registry the client is sent gives that id to "minecraft:badlands".
Every chunk would be painted as badlands
```

— which is the defect and the symptom in one sentence, and the constant is back at 40.

**Two things this round got wrong, both worth the line.** The test first went through `captured_payload()`, whose
payloads concatenate to 41 097 bytes containing `minecraft:` and **not** the biome registry's key — so it is not
the uncompressed registry bytes, whatever the reason; the test now reads the committed blob directly through
`CARGO_MANIFEST_DIR` and **says so**, rather than quietly reading a file and letting a reader assume it went
through the server. And the identifier scan found **nothing at all** in a file the same test had just located a
key in, which is impossible for a correct scanner; a byte-index version was replaced by `str::find` and worked
first time. **A test that does not work is worse than no test**, which is why the second failure was diagnosed
rather than committed.

### KD-68 — my commit guard checked four gates out of six, and I committed over a failing clippy a third time

The loop that runs the gates before committing recorded a boolean for the **four docs-audit scripts** and
nothing else, so if ( -eq 0 -and ) was true while cargo clippy -D warnings had failed on a binding
name. The commit went through, as it had twice before for different reasons.

**The guard was wrong in exactly the way the review keeps finding**: it checked a subset and reported a whole. It
now records **fmt, clippy, the test count and all four audits**, and the boolean is only true if every one of
them is zero.

The failure itself was trivial — 
amed too similar to another binding — and that is the point: a guard that
lets a trivial failure through will let a real one through, and three commits in this session are evidence.

### KD-69 — `is_default()` answered a different question from the one it was named for

```rust
/// Whether this state has no properties.
pub fn is_default(&self) -> bool {
    self.properties.is_empty()
}
```

The name says **default**; the body answers **has no properties**; the doc matches the body. For `oak_log` the
default is `axis=y`, which has a property, so this returns `false` for the real default and `true` for any
stateless block.

**It is called from nowhere** — not in product code, not in a test. That makes it a **trap rather than a
defect**: a future caller reads `is_default()`, believes it, and rebuilds exactly the assumption behind KD-56,
where a block's default was taken to be its lowest state id and every log lay on its side with water inside every
leaf.

**`BlockStateRef` cannot answer the question its name asks.** It holds a name, its properties and an id, and no
registry to compare against. The fix is therefore a name for what it can determine, which is what its own doc
already said.

**Verified by the compiler**: renaming a `pub` method breaks every caller, and the workspace builds. There were
none.

**KD-56 and this are mirror images, which is what makes the pair worth stating.** There the doc claimed more than
the code did — "the id of this block's first (**default**) state" — and the name was merely ambiguous. Here the
doc is exact and the **name** claims more. Both were read as the same wrong thing, *this number is the default*,
and one of the two ended up sending a client sideways logs with water inside them.

**And a smaller repeat**: the commit that carried this fix has no CHANGELOG entry, because the inline script
writing it died on a quoting error and **the commit guard only reads the gates**. A guard that checks what it can
and reports a whole — the same shape as KD-68 one round earlier, and not fixable by a boolean: a shell heredoc is
not a place to write a document.

### KD-70 — the standard this review has been arguing for, already in use, with one word wrong

Searching for the KD-69 class — a name or doc about a numeric default — turns up eight functions. Two of them
make numeric claims about vanilla, and one of those is the best-written comment this review has read:

```rust
/// **From the jar's own data, verified by counting**: every `blasting` recipe carries `cookingtime: 100` ...
/// These defaults matter only for a pack that omits the field, which vanilla never does — so they are
/// recorded as *not exercised by vanilla* rather than presented as verified.
```

**It says where the numbers came from, and it states its own limit.** That is precisely the evidence discipline
KD-56 and KD-65 were missing, already in use here — which is worth recording, because four rounds of this review
have been arguing for a standard the codebase already meets in places.

**And one word of it is wrong.** The sentence read "every `smoking` and `campfire_cooking` recipe carries
`200`/`100` respectively", which names **200 for smoking** where the constant and the jar both say **100**.
Vanilla cooks blasting, smoking and campfire cooking in half the time it cooks smelting; the code is right and the
sentence was not.

**A number in prose that nothing compares with the code beside it** — KD-56's shape in one word, inside a comment
that gets the hard part right. The fix is the sentence.

**The other numeric claim checks out.** `ContainerKind::default_slots` gives 41 for a player (36 + 4 armour + 1
offhand), 10 for crafting (a table's nine plus its result) and 3 for a furnace (input, fuel, output) — all three
correct. Its doc does **not** say where they came from, which is the weaker standard next to the cooking-time
comment, and it is a gap rather than a defect: the numbers are right and nothing depends on the reader trusting
them.

### KD-73 — a constant, its comment and my own correction, all wrong together, against the jar

`CookingRecipeKind::default_cooking_time` returned **100 for `campfire_cooking`**. The jar says **600**, and every
one of its nine campfire recipes says so:

```text
minecraft:smelting:          cookingtime=200  x73
minecraft:blasting:          cookingtime=100  x25
minecraft:smoking:           cookingtime=100  x9
minecraft:campfire_cooking:  cookingtime=600  x9
```

**A campfire is the slow method, not a fast one.** It cooks four items at once and takes thirty seconds over them,
which is why 600 rather than 100 — and grouping it with blasting and smoking is the kind of plausible mistake
that survives every test in the suite, because no test in the suite ever read a recipe.

### Three artifacts, written from one understanding

| | what it said | |
|---|---|---|
| `data/recipe.rs`'s constant | 100 | wrong |
| `data/recipe.rs`'s comment | ambiguous enough to read as 100 | unhelpful |
| **my KD-70 "fix" of that comment** | **"100 for all three of blasting, smoking and campfire cooking"** | **confidently wrong** |
| `container/furnace.rs:82-84` | "100 for `blasting` and `smoking`, and **600 for `campfire_cooking`**" | **right all along** |

**KD-70 read the constant, decided the ambiguous sentence was the error, and rewrote the sentence to match the
constant.** That turned a sentence that could be read either way into one that states the wrong value outright \
the worst of the three states that comment has been in, and the clearest demonstration yet of why **a comment is
not evidence about the code beside it**.

**And this is the review's subject in its purest form.** Two artifacts written from one understanding — a
constant and its comment — agreeing with each other, while a third document **in the same workspace** and the
jar's own data both said otherwise. Nothing compared them. The full suite is green either way: `cargo test` does
not read `data/minecraft/recipe/*.json`.

### The fix, and where the regression test belongs

The constant returns 600 for `CampfireCooking`, the comment names the measurement instead of a recollection, and
both now carry the four counts above.

**A regression test asserting the four values would not have caught this**, and that is worth stating rather than
papering over: a test written from the same belief asserts the same wrong number. The check that works is
**differential** — read the jar's recipes and compare — which is what `crates/data/tests/vanilla_data.rs`
already does for the pack, gated on `MC_VANILLA_DATA`. Adding the cooking times to it is the remaining work, and it
is recorded here rather than left implied.

### KD-74 — the same kind of claim, measured, is right eight times out of eight

KD-73 was a number in prose that nothing had compared with its source. `data/advancement.rs` makes **eight of the
same kind of claim**, and every one matches the jar exactly:

| claim | measured from the jar |
|---|---|
| "all **125** display blocks" | 125 |
| "all **1 514** reward blocks" | 1 514 |
| "`recipes` (on **1 491**)" | 1 491 |
| "**1 492** of vanilla's **1 617** have none" | 1 617 files, 125 with a display, so 1 492 |
| "**15** of vanilla's **3 546** criteria state none" | 3 546 criteria, 15 with no `conditions` |
| "The **54** trigger strings" | 54 distinct |
| "Vanilla's deepest chain is **9**" | the histogram below |
| "`1: 6, 2: 1531, 3: 48, 4: 13, 5: 8, 6: 5, 7: 3, 8: 2, 9: 1`" | identical, all nine buckets |

**Eight for eight, and the histogram is the strongest of them**: it is not a field counted but a **parent chain
walked** for every one of 1 617 advancements, and all nine buckets agree to the digit.

**That is what makes KD-73 a finding rather than a genre.** The difference between the two files is **not the kind
of claim** — both state counts about vanilla in a comment — it is **whether the number was measured**.
`advancement.rs`'s were. `recipe.rs`'s constant was 100 for a 600-tick recipe, and its comment had been rewritten
to agree with it.

**Two of my own readings were wrong before the check was right**, and both were caught by looking rather than by
believing:

* "15 of vanilla's 3 546 criteria state none" reads naturally as "no trigger", and the jar has **zero** criteria
  without one — the count is of criteria with no **`conditions`**, which is exactly 15;
* the first depth measurement gave `1: 6, 2: 1611` because I keyed advancements by file path (`adventure/kill_a_mob`)
  while their `parent` fields are namespaced (`minecraft:adventure/root`), so no parent ever resolved. **The claim
  was right and my measurement was wrong twice**, which is the mistake this review has made more often than any
  other.

### KD-75 — `loot.rs`'s counts hold at the scope they name, and one of them corrects a mistake of mine

Continuing the line that produced KD-73: **a comment stating a count about vanilla, measured against the jar.**

| claim | measured | |
|---|---|---|
| "The **11** table `type`s" | **11** | verified — the eleven named types, no more and no fewer |
| "The **19** `function` types" | **19** distinct | verified |
| "all **1 326** tables" | **1 326** | verified — see below |
| "**1 383** literals and **76** `uniform` providers" | 1 399 and 85 | **not verified** — see below |
| "**164** of vanilla's **1 392**" | 1 389 | **not verified** — see below |

**The 1 326 is the interesting one, because it caught me.** My first count was **1 331**, and the difference is
five files under `data/minecraft/datapacks/trade_rebalance/` — **a built-in data pack that overrides five chest
loot tables**. The comment counts the main pack, which is the right scope for a statement about vanilla's data,
and my count included the override copies. **The claim was right and my measurement was wrong**, which is the
twentieth time in this review and the fourth in the last three rounds.

**And that is why three of the numbers are recorded as unverified rather than wrong.** 1 399 against 1 383 and
85 against 76 are **close**, and close is the signature of a scope difference rather than a wrong figure: my
counting is textual, over `"rolls": <number>` and `"function"` occurrences at any depth, while the file's numbers
were presumably taken from a structural walk of the main pack alone. **A number that is nearly right is not
evidence of a defect, and reporting it as one would be the same error as KD-70** — where I read a constant,
decided the prose was wrong, and made it confidently wrong.

**So the row above says what was verified and what was not**, rather than collapsing the two: two counts and the
table total hold exactly at the scope the comment names, and three need a structural parse of the main pack before
anything can be said about them.

### KD-76 — the fuel table's best-designed column, and the three ways a doc and its code come apart

`container/furnace.rs` is the best-designed artifact this review has read. Its fuel table carries **the evidence
label inside the row**, and it says why:

> The label is part of the **row** rather than a parallel table, so reordering or adding a row cannot silently
> attach the wrong evidence to a fuel — the hazard a parallel `&[(&str, Evidence)]` would carry.

That is the discipline this review has spent twenty rounds arguing for, already in use — and the thing that is
wrong in it is a label:

```text
| `minecraft:coal_block` | 16000 | 800 | derived (9 x coal) |
```

**Nine coals are 14400. A coal block is 16000**, because it smelts 80 items where nine separate coals smelt 72,
and the eleven-item gap is real Vanilla behaviour. The value is right and **the label claims a multiplication that
produces a different number** — which matters more here than anywhere, because `Derived` is defined as a Vanilla
figure this build is willing to assert, so a wrong label mis-states how much confidence the number is entitled to.
The row now says `verified`, and the code's own `Evidence::Derived` was corrected with it.

**`Derived`'s definition used the same wrong instance** — "a coal block is nine coal" was the reader's example of
what the category means. An example that is wrong teaches the category wrongly, and this one would have been
labelled `verified` by anyone who checked it. With the row corrected, **no row uses `Derived` at all**, which the
definition now says rather than leaving a variant that exists for symmetry.

### The three ways a doc and a doc's code come apart, all inside four rounds

| | what was changed | what was left | result |
|---|---|---|---|
| **KD-70** | the prose, to match a constant | the constant | **confidently wrong prose** |
| **KD-71** | the body (KD-56) | the doc describing the old body | a reader told to work around a fixed defect |
| **KD-76** | the doc table | the struct literal under it | the two disagreeing |

**And the third was mine, inside the fix for the first.** I changed the table's `derived (9 x coal)` to `verified`
and left `evidence: Evidence::Derived` in the row it describes — a fresh doc-versus-code disagreement created by
the commit that was correcting one.

**The common shape is that the two are edited as though they were one artifact and treated as though they were
two.** Every gate was green each time, because no gate reads a doc table and compares it with the literal beneath
it — which is the same reason the air-with-a-count invariant needed a test rather than a comment (KD-63), and why
this review's two real fixes came with `registry_ids.rs` and a pinning test rather than a sentence.

### KD-77 — a test that compares the prose with the code, verified by putting the defect back

KD-76 changed a doc table and left the struct literal beneath it, and **every gate stayed green**, because no
gate reads a doc and compares it with the code. `crates/container/tests/fuel_table_consistency.rs` is that check:

* it parses the **Markdown table out of the doc comment** with `include_str!` and the **rows out of the code**, and
  asserts the fuels and their evidence labels agree in order;
* and it asserts no **table row** calls the coal block a derivation of nine coal.

**Verified by perturbation**, which is the only way to know: setting `evidence: Evidence::Derived` back on the
coal block row fails it with

```text
minecraft:coal_block: the doc says "verified" and the row says "derived".
This is KD-76: the two are edited as one artifact and treated as two.
```

**A test written from the same belief would not have caught this**, which is why this one reads the source rather
than holding a list — the same reason `registry_ids.rs` reads the registry the client is sent instead of a copy.
Both are the shape of check this review concluded it needed: **the two artifacts compared with each other, rather
than each compared with a belief.**

### Three faults in the test before it worked, all of them mine

| fault | what it looked like | |
|---|---|---|
| the parse ran into the second table | "the doc table has 15 rows and the code has 8" — `furnace.rs` documents a smelting table too, whose fourth column is an experience value | |
| the forbidden phrase appears in the sentence forbidding it | the comment explaining KD-76 says the block "is not `derived (9 x coal)`", so a file-wide `contains` check fails **on the corrected code** — a self-inflicted false positive | |
| the item name kept its closing backtick | `"minecraft:coal\`"` against `"minecraft:coal"`, from stripping the opening backtick and not the closing one | |

**Two of the three are the same mistake in different clothes**: a check that looks right and tests the wrong
thing. The first asserted a property of the whole file where the property belonged to a table; the second
asserted a property of the whole file where it belonged to a row. **A check is only as good as its scope**, and
this review has now made that mistake in the filter that found nothing (KD-59), in the guard that checked four
gates of six (KD-68), in the sweep that reported done while leaving seven (KD-72), and twice here.

### KD-78 — `tag.rs` holds where I could measure it, and my instrument was wrong where I could not

| claim | my measurement | |
|---|---|---|
| "Vanilla has **758** tags" | **758** | exact — a plain file count |
| "vanilla alone has **17**" tag directories | 16 in the main pack | **unverified** |
| "Vanilla's deepest is **4**" (`block/supports_crimson_fungus`) | 2, by my walk | **unverified** |
| "**103** spurious ... splitting at the first left **46**" | 163 and 384 | **unverified** |

**The one exact match is the one that needs no interpretation**: counting `data/minecraft/tags/**/*.json`. The
three I could not reproduce all need a model of the format, and mine is wrong in a way I can name.

`minecraft:block/supports_crimson_fungus` has one value, `#supports_warped_fungus`, and **a `#` reference is
relative to the same registry** — the key is `minecraft:block/supports_warped_fungus`. I resolved it as
`minecraft:supports_warped_fungus`, which does not exist, so **every relative reference in the pack counted as
missing** and my depth walk never followed one. That is the shape of the 384-against-46 gap exactly.

And 16 directories against a claimed 17 is **the same scope question the loot tables raised**:
`data/minecraft/datapacks/trade_rebalance/` carries a second copy of some registries, and "vanilla" may or may not
include it. KD-75 measured 1 331 where the comment's 1 326 was right, for that reason.

### The rule this adds

**A count reproducible without understanding the format is evidence. A count that needs a model of the format is
evidence only once the model is right.** The four files on this line have now produced:

* `recipe.rs` — one claim, **wrong**, and the measurement needed no model (the jar states `cookingtime` as a field);
* `advancement.rs` — eight claims, **all exact**, the hardest needing a parent-chain walk that was right on the third attempt;
* `loot.rs` — three exact, three unverified, separated by a built-in data pack;
* `tag.rs` — one exact, three unverified, separated by a format rule I had not modelled.

**Three of the four hold up, and the one that does not is the one where the number was never measured at all.**
That is the distinction this line exists to draw, and it is worth more than another sweep: **"unverified by me"
and "wrong" are different findings**, and collapsing them is the error that produced KD-70.

### KD-79 — with the right model, `tag.rs`'s `46` comes out exactly, and the other two are conventions

KD-78 recorded three of `tag.rs`'s numbers as unverified because my instrument was wrong: I resolved
`#supports_warped_fungus` as `minecraft:supports_warped_fungus` when **a `#` reference is relative to the same
registry**, so every relative reference in the pack counted as missing.

With that one rule applied:

| claim | measured | |
|---|---|---|
| "Vanilla has **758** tags" | **758** | exact |
| "splitting at the first left **46**" | **46 unresolved** | **exact** |
| "Vanilla's deepest is **4**" | 5 by my numbering | a **convention**, not a defect |
| "vanilla alone has **17**" directories | 16 | a **scope**, not a defect |

**The 46 is the one that matters.** It is not a count of anything visible in a file listing: it is the number of
tag references that do not resolve **once the registry-relative rule is applied**, and the comment states it
alongside the 103 that the naive separator-split produces. I measured 384 with the rule missing and **46** with it
— so the file's number is the correct-model answer and my first one was the broken-model answer, in exactly the
shape the comment describes.

**The other two differ by one each, and both differences are conventions rather than errors:**

* **depth**: I count a tag with no tag-references as depth 1; the file's numbering makes that 0. Under its
  convention the deepest is 4 and under mine it is 5 — and **both name `block/supports_crimson_fungus`**, which
  is the tag the comment calls the deepest. A self-consistent claim with a different origin is not a defect, and
  calling it one would be KD-70 all over again.
* **directories**: 16 under `data/minecraft/tags/`, against a claimed 17. `worldgen` holds sub-registries
  (`worldgen/biome`, `worldgen/structure`, ...) that a directory count can reasonably split, which is the same
  class of question as the built-in `trade_rebalance` pack in KD-75.

### What this line has now established

Four files, and the distinction that took three rounds to draw cleanly:

| file | outcome |
|---|---|
| `recipe.rs` | **one claim, wrong** — and reproducible without any model of a format |
| `advancement.rs` | **eight claims, all exact** — the hardest needed a parent-chain walk, right on the third attempt |
| `loot.rs` | three exact, three separated by a built-in data pack |
| `tag.rs` | two exact once the rule was right, two separated by conventions of numbering and scope |

**Three of the four hold up, and the fourth failed where the number had never been measured at all.** The line's
real product is therefore not a defect count but a way of telling three things apart: **measured and right**,
**measured and wrong**, and **not measured by me** — where the third has repeatedly meant *my instrument*, and
collapsing it into the second is what produced KD-70 and then KD-73.

### KD-80 — the second table in `furnace.rs` agrees with its code in every cell

KD-76 was the fuel table's coal-block row, where the doc and the code disagreed and a test now compares them.
`furnace.rs` documents **a second table** — seven smelting recipes — and that one is exact:

| doc column | code | |
|---|---|---|
| 7 rows | `[(&str, &str, f32); 7]` and a `Vec::with_capacity(7)` | agrees, and the capacity says so too |
| `iron_ore`, `deepslate_iron_ore`, `raw_iron` to `iron_ingot` | the same three, in the same order | agrees |
| `sand` to `glass`, `cobblestone` to `stone` | same | agrees |
| `porkchop`, `potato` | same | agrees |
| `1` output each | `SmeltingRecipe::one` takes no count | agrees |
| `200` ticks on every row | `SMELTING_COOK_TICKS = 200`, applied to every push | agrees |
| `0.7` on the three iron rows | `EXPERIENCE_PER_IRON_SMELT = 0.7` | agrees |
| `0.1`, `0.1`, `0.35`, `0.35` | the same four literals | agrees |
| "verified shape, **approximate xp**" | the module doc calls the experience values "an **approximation** ... rather than the exact per-recipe figure" | agrees, and in the same words |

**Nine cells, no disagreement**, in the file where the other table's label was wrong. That is worth recording
rather than passing over: **the same author, the same file, the same convention, and one table is exact while the
other carried a label claiming arithmetic that produced a different number** — which is what makes the fuel
table's defect a mistake rather than a habit, and what makes the new consistency test worth having rather than
worth distrusting.

**And the two tables needed different checks.** The fuel table's fourth column is an evidence label, so the test
compares label against `evidence:`. This one's fourth column is an experience value, so the comparison is against
`EXPERIENCE_PER_IRON_SMELT` and four literals — a different assertion over a different shape. That is why the
existing test stops at the fuel table rather than running on: **a comparison is only meaningful where the two
sides are the same kind of thing**, which is the scope lesson from KD-77 in a different dress.

### KD-81 — the sixty, read: eleven hold, two name a measurement nobody has taken

Thirteen of the sixty doc lines that name a number are in `mc-entity`, the largest unread group. All thirteen
were read, and **eleven hold by inspection** — each is arithmetic or a Vanilla fact checkable without a tool:

| line | claim | check |
|---|---|---|
| `player.rs:994` | a merge "must not cost the player the other **40**" | the player inventory is 41 slots, so 40 remain — and the constant in the same crate says 41 |
| `profile.rs:75` | "exactly as the vanilla login path does: **1..=16**" | Vanilla's username limit is 16 |
| `stack.rs:239,297` | "the vanilla default limit of **64**" for `grow` and `merge` | `DEFAULT_MAX_STACK_SIZE` is 64 and both call through to it |
| `stack.rs:602,845` | an empty table gives "the vanilla default of **64**" | `max_stack_size_for_name` falls back to that constant |
| `item_entity.rs:278` | "a ceiling of 64 cannot create a stack larger than 64" | it clamps to `HARD_MAX_STACK_SIZE`, which is that ceiling |
| `player.rs:1091` | "a count above **127**, which `ItemStack` cannot produce" | `new` refuses above 64, so 127 is unreachable and the bound says why |
| `mob.rs:122` | "Vanilla's default `FOLLOW_RANGE` attribute (**16.0**)" | Vanilla's default follow range is 16.0 |
| `stack.rs:78,591` | a doctest and a comment | both are arithmetic on 64 that comes out as written |

**Two name a measurement rather than illustrate one**, and those are the next targets on this line:

* **`stack.rs:645` — "Number of exception entries (always 165 for the 26.1.2 vanilla table)"**. The same shape as
  KD-73: a count about Vanilla, in a comment beside a constant that encodes it, naming the exact version.
  **Whether it was measured is a question with an answer, and it has not been asked.**
* **`mob.rs:122` — the `16.0` follow range.** Stated as Vanilla's default without saying where from, which is the
  weaker standard KD-70 recorded beside the cooking-time comment that *did* say where its numbers came from.

**No defect in the thirteen**, which is the expected shape by now: a line that pairs a claim with a number it can
be checked against is right far more often than not, and the finding on this line came from the one where the
number had never been measured at all.

**And a smaller repeat worth one line**: this entry's script was the second in two rounds to write a document by
reading another document it had forgotten to create. Both failed loudly on their own anchor assertion rather than
quietly writing nothing, which is the property that made them cheap to catch.

### KD-82 — the `165` is recorded as unverified, because both of my instruments were broken

`StackSizeTable::len` carries this doc:

```rust
/// Number of exception entries (always 165 for the 26.1.2 vanilla table).
```

and the module doc one level up states where the table comes from: **"every `stacksTo` call site in the game"**.

**Two attempts to count it, both with a broken instrument.** A regular expression over the two constant arrays
returned "not found" for both names; a PowerShell range extraction printed an empty start line and then counted
**167 twice**, the same number for two different arrays, which is the signature of reading one range both times.

**Neither number is evidence, and publishing either would be the KD-70 mistake** — reading a source, deciding
the prose is wrong, and stating the result confidently.

### The method that would settle it

The module doc makes a **bytecode-level** claim, which is the kind `DefaultStateProbe` settled for
`defaultBlockState` and `ItemProbe` for the item table. A probe that walks `Item.Properties` construction and
counts `stacksTo(n)` calls with `n != 64` would give **the exception count** to compare against 165, and **the
per-item limits** to compare against this table row for row.

That second comparison is the one that matters, and it is the check this table has never had: `items.tsv` is
trustworthy because `ItemProbe` reproduced all 1506 of its rows; the stack-size table is trusted because its doc
says it came from the jar. **A doc saying where a number came from is not the same as the number having been
checked**, which is the whole of what this review found on the jar-count line — and this is the last of the
tables in this area without an independent source.

### KD-83 — the stack-size probe compiles and stops one bootstrap short

`tools/vanilla-probe/StacksToProbe.java` is written and compiles against the 26.1.2 jar. It fails at runtime:

```text
java.lang.NullPointerException: Components not bound yet
```

**In 26.x the maximum stack size is a data component**, not a constant on the item. `getDefaultMaxStackSize` reads
it from the bound `DataComponents`, and `Bootstrap.bootStrap()` alone does not bind them — that needs the datapack
load a dedicated server performs. **So the probe is one bootstrap step away rather than one idea away.**

**That is a better state than KD-82 left it in.** The instrument that can settle the 165 now exists and is known
to need one specific thing, which is the distinction this review has spent six rounds drawing between *not
measured*, *measured wrong*, and *measured right* — and "written, compiles, needs a datapack load" is the first of
those three with a route out of it.

**What it will produce when it runs:**

* **the exception count**, against the doc's 165;
* **the per-item limits**, against `STACK_SIZE_1` and `STACK_SIZE_16` row for row — **the check this table has
  never had**, and the one that matters more than the count, since `items.tsv` is trustworthy because `ItemProbe`
  reproduced all 1506 of its rows while this table is trusted only because its doc says it came from the jar.

### KD-84 — the probe runs and reports 1506 unreadable, which is the better failure

```text
items=1506
exceptions=0
unreadable=1506
```

**Every item's `MAX_STACK_SIZE` component came back null.** The items are registered — 1506 of them, the count
`ItemProbe` established — but **their default components are not built by `Bootstrap.bootStrap()`**. The first
version died on that as `NullPointerException: Components not bound yet`; this one reports it as 1506 unreadable.

**A probe that substituted 64 for a value it could not read would have printed `exceptions=0` and looked
finished** — which is the shape of every defect this review found on the jar-count line: a number that agreed with
the belief beside it because nothing had measured it. **1506 unreadable is a fact about the instrument a reader can
act on**; `exceptions=0` from the same run would have been a lie the summary told.

**The route out, narrowed to one step.** The components are built by the **datapack and registry load** a dedicated
server performs, which is more than `SharedConstants.tryDetectVersion()` plus `Bootstrap.bootStrap()`. Two ways to
reach it, both recorded rather than guessed:

* run the vanilla server far enough to initialise its registries and read them through `RegistryAccess`, using the
  launcher this workspace already has at `target/vanilla-26.1.2/`;
* or find the initialiser that populates item components and call it after bootstrapping — which is what
  `DataComponentInitializers` looked like on inspection, and was not confirmed.

**The probe is committed as it stands**, because a tool that reports "1506 unreadable" is already more than this
table has ever had: its rows have never been compared with anything, and there is now something that says so in a
run rather than in a comment.

### KD-85 — the probe's boundary is now in the data, per row, and names the missing step

```text
0 minecraft:air     ERROR:NullPointerException:Components_not_bound_yet
1 minecraft:stone   ERROR:NullPointerException:Components_not_bound_yet
2 minecraft:granite ERROR:NullPointerException:Components_not_bound_yet
```

**The same condition as the first version's crash, now written per row.** My first `catch` kept only the
exception's **class name**, which said `NullPointerException` and nothing a reader could act on; the second prints
the **message**, and the message names the cause.

**That is the difference between reporting a boundary and reporting a symptom** — the distinction this review
keeps drawing. `unreadable=1506` was true and nearly useless;
`ERROR:NullPointerException:Components_not_bound_yet` on every row says **which** bootstrap step is missing, and it
lives in the artifact rather than beside it.

### The runtime accessor does not help, which narrows the route to one

`new ItemStack(item).getMaxStackSize()` fails the same way as `item.components().get(MAX_STACK_SIZE)`. So the
route is **not** "use the API the game uses" — `getMaxStackSize` reads the bound components too. **The datapack and
registry load is the step**: KD-84 recorded that, and this round turns it from a reading of `javap` output into a
demonstration.

### Why this is a commit rather than a shrug

The probe compiles, runs in one pass, and produces a 1506-row file whose every row carries the reason it is
unreadable. **The stack-size table has never been compared with anything**; it now has a tool that was pointed at
it, got an answer, and wrote the answer down **including why the answer is not the one wanted**.

**A tool that fails at a named step is where the next attempt starts.** The alternative — substituting 64 per
item — would have produced a file that agreed with the table and a summary that agreed with the file, which is
precisely the failure mode this review exists to find.

### KD-86 — `freeze()` was the right kind of guess and not the step

Scanning the jar's classes for the message found **exactly one** carrying `Components not bound yet`:
`net/minecraft/core/Holder$Reference`. `MappedRegistry` exposes `freeze()`, `bindTags(...)` and
`bindAllTagsToEmpty()`, so the probe now calls `BuiltInRegistries.ITEM.freeze()` after bootstrapping.

**It still reports 1506 unreadable, with the same message.** `freeze()` is not the step, and the negative result
is recorded as such rather than left as an untried idea.

**Asking the jar which class raises an error is a technique, not a one-off.** It took the condition from a string
in a stack trace to a named class and a named set of candidate calls in one pass, with no guessing about what
"bound" means in 26.x — the same move as `DefaultStateProbe` reading `defaultBlockState` off the bytecode.

**Where the next attempt starts, named rather than gestured at:**

* **`DataComponentInitializers`** — it mentions a binding entry point and has a `BakedEntry` type, which is what a
  component map looks like once built. If its `build(...)` populates item components, calling it after
  bootstrapping is the step.
* **the full server bootstrap** — the datapack and registry load a dedicated server performs, reachable with the
  launcher already in this workspace at `target/vanilla-26.1.2/`.

**The probe stays as it is**, because its current state is the useful one: it runs in one pass and every row of
its output carries the exact reason it is unreadable. **`165` remains unverified, now with three attempts behind it
rather than one belief** — which is what the coverage statement has to say, and now can.

### KD-87 — the remainder of the sixty holds, and I twice nearly reported a sentence I had read in halves

The rest of the doc lines that name a number, in `data`, `protocol`, `redstone`, `server` and `worldgen`. **Every
one holds**, and most need no tool: `f64`'s mantissa is 53 bits; a section is 16^3 = 4096 block states and 4^3 = 64
biomes; Vanilla's default view distance is 10; a redstone level of 15 is the only one in a table with
`powered=true|false`; the 24-bit and 53-bit draws match the RNG's documented widths.

### The near-miss that matters

```text
With the default five-block trunk the tree is 36 blocks: 5 logs, 21 leaves
(5x5 minus four corners), 9 leaves (3x3) and 1 leaf tip.
```

I read "5 logs, 21 leaves" and had 26 against a claimed 36 — **a defect, apparently, in the file that generates the
trees this review has already corrected twice**. The sentence continues past the line I stopped at: 5 + 21 + 9 + 1
= 36, and every part checks — 5x5 minus four corners is 21, 3x3 is 9, the tip is 1, and the trunk column is
skipped where the canopy passes over it so the leaves do not double-count the logs.

**The second was the same mistake one level up**: three of the lines in this group are continuations of sentences
whose first halves are a different grep hit, and **a claim read in halves is read wrong**. Both were caught by
opening the file rather than by reasoning about the line — which is the only thing that has ever caught this
class, in this review or in the tooling it built.

### Why a round with no finding is worth recording

**The rate matters more than the result.** The jar-count line now covers six files: `recipe.rs` wrong,
`advancement.rs` 8 of 8, `loot.rs` 3 plus 3, `tag.rs` 2 plus 2, `furnace.rs` one label wrong, and this remainder
clean. **The findings came from values that had never been measured, not from prose that was hard to read** — so
a remainder of easy prose, checked anyway, is what makes "clean" a result rather than an assumption.

### P10-06 (part 1) — the entity type table, and id 0 is a boat

`add_entity` carries an **entity type id** and the client resolves it against the registry this server sends it.
That is the exact shape of the two defects this review already fixed — a block default taken to be a lowest id
(KD-56), and `PLAINS_BIOME_ID = 0` where id 0 is `minecraft:badlands` (KD-65) — so the table is **extracted
rather than retyped**, in the pipeline `blocks.tsv` and `items.tsv` went through:

```text
tools/vanilla-probe/EntityTypeProbe.java  ->  crates/test-support/fixtures/registry/entity_types.tsv
```

**And the numbers are not the ones anyone would guess:**

```text
0    minecraft:acacia_boat
30   minecraft:cow
71   minecraft:item          <- the drop this phase has to make visible
150  minecraft:zombie
155  minecraft:player
```

**`entity_types=157`, and id 0 is a boat.** The registry is alphabetical, exactly as the biome registry is — which
is why the biome defect was invisible for so long: the assumption "the first entry is the ordinary one" is true
often enough to survive, and wrong in both of these registries.

**The fixture is 159 lines** (two header lines and 157 rows), **LF only**, checked for CRLF because a CRLF table
reached this repository once already (KD-57).

**What this does not yet do.** Nothing sends `add_entity` yet: the server's own comments say the drop is invisible
to clients (`game.rs:1060`, `game.rs:2631`, the latter naming P05-15). The table is the half that has to be right
before the packets can be, and the next step is the registry lookups plus the encoder wiring.

### P10-06 (part 2) — the entity type table, and the two kinds of registry a client owns

The table is loaded and checked: `crates/registry/src/entities.rs` carries `EntityTypeRegistry` with
`load`/`parse`/`id`/`name`/`names`, the parser refuses non-contiguous ids, and five tests pin what the jar says —
`player` at **155**, `item` at **71**, and id 0 at **`minecraft:acacia_boat`**.

**And extending `registry_ids.rs` to the entity types turned out to be the wrong instrument**, which is worth
recording because it sharpens the rule this review produced. Searching the payload for `minecraft:entity_type`
finds a hit followed by `minecraft:axolotl_always_hostiles` and `minecraft:can_equip_harness` — **tag names from
the `update_tags` packet**. The `registry_data` packets carry the **datapack** registries; `entity_type` is a
**built-in** registry, compiled into the client jar.

| kind | examples | where the client gets it | how to check our numbers |
|—-|—-|—-|—-|
| **datapack registry** | biome, dimension type | **we send it** in `registry_data` | read it back out of the payload we send |
| **built-in registry** | block, item, **entity type**, menu | **compiled into the client jar** | extract it from the jar, and name the extraction in an assertion |

**This explains the fixtures and was implicit until now**: `blocks.tsv`, `items.tsv` and `entity_types.tsv` are jar
extractions because the client owns those registries, while the biome ids came from the payload because we hand
that registry over ourselves. **The same rule, applied with the instrument that matches who owns the number.**

### P10-06 (part 3) — the entity packet ids, from the table the jar produced

dd_entity is **1**, 
emove_entities is **77**, set_entity_motion is **101**, and all three come from
docs/protocol/packet-ids-775.tsv — the table machine-extracted from the official 26.1.2 server jar, which
ids.rs already names as the source every constant is checked against.

### And the capture agrees with the table this round extracted

	arget/vanilla-capture/bodies-lit/ holds **55 dd_entity bodies from a real 26.1.2 server**. Read as a VarInt
entity id, a 16-byte UUID and then the entity type id:

`	ext
sample 1:  entity id 78, uuid, type id 117  ->  entity_types.tsv says 117 = minecraft:slime
sample 2:  entity id 59, uuid, type id 117  ->  the same, two different slimes
`

**That is one agreeing reading, not a proof**, and it is worth saying which: the offset is an inference from the
packet's documented shape. The proof is the encoder plus a golden test against these bytes, which is the next step
and the route light_update already took.

### P10-06 (part 4) — add_entity, and a real server's bytes it decodes

AddEntity is in crates/protocol/src/packets/play.rs with encode and decode, and
crates/protocol/tests/add_entity_golden.rs checks it against a body **a real 26.1.2 dedicated server sent**,
committed as crates/test-support/fixtures/protocol/add_entity_slime.hex with its provenance in the header.

### The claim that does not depend on my reading of the format

A golden test through our own decoder proves the layout round-trips, and if the fixture and the decoder came from
one reading then comparing them only confirms that reading. So the first assertion is arithmetic on somebody
else's output:

`	ext
1 (VarInt id) + 16 (UUID) + 1 (VarInt type) + 3*8 (f64) + 3*1 (i8) + 1 (VarInt data) + 3*2 (i16) = 52
`

and the captured body **is** 52 bytes.

### What the decode says

`	ext
entity id 78, type id 117 = minecraft:slime, coordinates inside the world height
`

**The type id agrees with the table P10-06 extracted from the jar**, which is the cross-check that matters here:
a jar extraction, a real capture, and our decoder all naming the same entity. The coordinates are asserted to be
finite and within the world height, because a decoder that read the doubles at the wrong offset would produce a
denormal rather than a mistake anyone would notice.

### And a truncated body is refused rather than padded

Every prefix of the captured body is short of some field, and **none of them may decode** — a lenient decoder
that defaulted the missing fields would send a client an entity at the origin.

### P10-06 (part 5) — where the wiring goes, and the captured evidence for `remove_entities`

**Both halves of the remaining work already have a home**, and the server's own comments say so, which is the
engineering contract's no-fake-completeness rule doing its job:

```text
game.rs:603  /// the batch it needs. **No `remove_entities` packet is encoded yet**: clients
game.rs:1060 /// the `add_entity` packet that would make the drop visible to a client. A
game.rs:2631 // `add_entity` packet that would show it are still P05-15; the
```

`spawn_item_owned` spawns into `self.entities` and sends nothing; `sweep_entity_removals` already runs in the
Broadcast phase and already counts what it collected. So the wiring is **`spawn_item_owned` sends `AddEntity`**
and **`sweep_entity_removals` sends `RemoveEntities`** for the ids it has.

### And `remove_entities` has twelve real bodies to check against

```text
001788_s2c_play_77.bin: 01 0f   -> count 1, entity id 15
002890_s2c_play_77.bin: 01 4d   -> count 1, entity id 77
003812_s2c_play_77.bin: 01 36   -> count 1, entity id 54
```

**Every one is two bytes.** A VarInt count followed by that many VarInt entity ids is `1 + 1 = 2`, and that is
what a real server sent twelve times — the same arithmetic check `add_entity` got, on a shorter packet.
`set_entity_motion` has **2942** bodies in the same capture.

### Left

`RemoveEntities` and `SetEntityMotion` codecs, the two call sites above, and then a real client to confirm the
drop is visible — which is P10-08's acceptance and P10-11's session.

### P10-06 (part 6) — set_entity_motion, and the endianness I got wrong by hand

`SetEntityMotion` is in `play.rs` with `encode`/`decode` plus `velocity()` and `from_velocity`, and
`crates/protocol/tests/set_entity_motion_golden.rs` checks it against bodies a real server sent.

**All 2942 `set_entity_motion` bodies in the capture are exactly seven bytes**, and `1 + 3 * 2 = 7` is what a
`VarInt` id plus three `i16` velocities comes to. `add_entity` had 55 bodies for a 52-byte layout; this has 2942
for a 7-byte one, so "the lengths agree" is not a coincidence that survived a single sample.

### My hand-computed expectations were little-endian

I read `49 f9` as `0xf949` (-1719) and wrote that into the test. **The decoder reads `0x49F9` (18937) and is
right**: this protocol writes multi-byte integers **big-endian**, as every other codec in the repository already
does. The failure was mine, and it is the specific kind this work keeps meeting — a value produced from a
remembered convention rather than from the code beside it.

**And the assertion that caught it is the one worth having.** The test asserts the lengths *and* that the decoded
velocities are plausible: **2.37, 3.99 and -0.64 blocks per tick**, all within what an entity walks at. With only
the length check this would have gone in green carrying a comment claiming numbers the code did not produce,
which is the shape of the cooking-time defect.

### A caller error is refused rather than wrapped

`from_velocity` rejects a component beyond what an `i16` at 1/8000 carries, plus NaN and infinity. A silent wrap
would send a client an entity moving the other way at speed.

### And the codec's doc now states the endianness

Because that is what I got wrong, and a reader should not have to derive it from a sample.

### P10-06 (part 7) — the wiring's prerequisite: the entity model has no UUID

Sending `AddEntity` needs an entity id **and a 16-byte UUID**. The entity store has the first and not the second:
a search for `uuid` in `crates/entity/src/entity.rs` returns nothing, and `spawn` allocates only
`EntityId(self.next_id)`. So this is a **prerequisite rather than a codec gap** — the sort of thing wiring finds
and unit tests do not.

### And it is a design decision, not a line of code

`Uuid::new_v4()` is the obvious implementation and is wrong here for a stated reason: the engineering contract
requires that **the same initial state and the same ordered inputs over the same tick count produce the same
normalized simulation state**, and a random UUID per spawn breaks exactly that. The UUID must be **derived from
stable inputs**, and the two candidates are:

* **from the world seed and the entity id** — deterministic by construction, and entity ids are already allocated
  sequentially;
* **from a counter seeded by the world seed** — the same property stated explicitly, which is what Vanilla's
  offline-mode player UUID does for the case that matters most here.

**Neither is chosen yet**, deliberately. It changes a core type in `mc-entity`, it interacts with
`crates/server/tests/entity_lifecycle.rs`'s determinism test — the one test in the repository with an explicit
anti-vacuity sentinel — and it is the identity a client keeps for an entity across packets. That deserves its own
round and its own evidence rather than an edit appended to a codec.

### Where the wiring stands

Both insertion points exist and the code's own comments name them. `spawn_item_owned` queues an `AddEntity` for
the Broadcast phase, the way `pending_light` already queues work, because it has no `TickReport` and
`broadcast_chunk` needs one; and `sweep_entity_removals` sends `RemoveEntities` for the ids it already gathers.

### P10-06 (part 8) — entity identity, derived rather than random

`crates/entity/src/identity.rs` adds `entity_uuid(seed, id)`, the prerequisite part 7 identified: `AddEntity`
carries a 16-byte UUID and the entity model had none.

**Not `Uuid::new_v4()`**, for a reason the contract states: the same initial state and the same ordered inputs
over the same tick count must produce the same normalized simulation state, and a random UUID per spawn breaks
exactly that — two runs of one script would differ in a field a client sees and a trace records.

**A plain counter would not do either.** "The first spawn is UUID 1" is deterministic but says nothing about
*which run* a UUID belongs to, so two worlds at different seeds would share identities in any trace comparing
them. Mixing the seed in costs one multiply.

The derivation is SplitMix64's finaliser over `(seed, id)`, then version-4 and RFC 4122 variant bits, and five
tests hold it in place:

| test | what it prevents |
|—-|—-|
| same seed and id always give the same UUID | a replay that does not replay |
| ten thousand ids in one world are all distinct | two entities sharing an identity |
| the same id in two worlds gets two UUIDs | a trace agreeing across worlds that means nothing |
| the result is a well-formed v4 UUID | sixteen bytes that merely happen to be the right length |
| neither argument alone determines the result | a derivation that silently drops one input, which would still pass the first two tests |

That last one is the one worth having: **a derivation that ignored the seed would pass both the stability test and
the distinctness test**, and only an assertion that varies one argument at a time catches it.

`uuid` becomes a dependency of `mc-entity` rather than the identity being reduced to sixteen bytes here and
rebuilt in `mc-protocol`: it is the same type on both sides of the boundary, and the crate is already in the
workspace tree and licence-checked through that dependency.

### P10-06 (part 9) — identity on the store, so `Entity` does not change

`EntityStore` gains a `seed`, a `with_seed`, a `seed()` accessor and `uuid(id)`, which derives an entity's
identity from the seed and the id and returns `None` for an id that is not live.

**Not a field on `Entity`.** `Entity` is the simulation body — position, velocity, yaw, pitch, on_ground — and a
`uuid` field would make identity part of that body and touch **every** construction site, including the tests that
build an entity to check collision or physics. Identity is a function of the store's seed and the entity's id, so
it belongs on the store: **nothing that constructs an `Entity` has to know**.

`new()` keeps seed `0`, the same value a world with no configured seed uses, so a store built without one still
produces stable identities rather than inventing entropy. The derivation is total in it.

### And the wrapper has its own tests

`entity_uuid` has five, and **none of them would notice a store that passed the wrong seed through or answered for
an entity that is not live**. So `a_live_entity_has_the_identity_its_store_derives` checks the call site: the
store agrees with the derivation, two stores at one seed agree, two at different seeds do not, and
`an_entity_that_is_not_live_has_no_identity` asks for an id that was never spawned and gets `None`.

**A test of a derivation is not a test of its call site** — the distinction this project has now met from both
directions.

## [0.1.0-rc.1] — 2026-09-12 (release candidate)

**Released.** Tag [`v0.1.0-rc.1`] with a GitHub Release carrying three assets:

| Asset | Size | SHA-256 |
|---|---|---|
| `mc-server-aarch64` | 4 526 384 B | `0a9575c7499c03573f4b83e0b4b762c60daff55ba49e0d87b2997d845baea3e3` |
| `mc-server-x86_64-windows.exe` | 3 385 344 B | `a11d6f06dd7269b9b3ecc68ff8735db4f502ae60bc66bf768e14f910adfd0b45` |
| `SHA256SUMS` | 185 B | `02c0a325ced2ea2eda5c444848d6fd09dcc8a2915b76ae58bc2dbab9d14e56b8` |

Both binaries are built from this tag; the aarch64 one was built **on the device** and is
byte-identical to the binary the Pi acceptance host runs (verified by SHA-256 during the
governance round, 2026-09-12). Build and release procedure:
[CONTRIBUTING.md](CONTRIBUTING.md).


### Phase 00 — Research (2026-09-10)

- Inventoried the four local reference clones (Pumpkin, Paper, Valence,
  Minestom) with licence and module citations; established the 26.1.2 protocol
  baseline (protocol 775, DataVersion 4790 — later measured, not guessed).
- Architecture decision: crate-per-boundary workspace, one tick thread with
  Tokio only at I/O edges, vanilla-compatible Anvil subset, 775-only protocol.
  Recorded as [ADR-0001](docs/adr/ADR-0001-system-architecture.md) with a risk
  register whose items (R-01…R-10) were tracked to closure across the project.
- Clean-room policy set: behaviour may be studied from references, source may
  never be copied ([NOTICE](NOTICE), [docs/legal/third-party.md](docs/legal/third-party.md)).

### Phase 01 — Foundation (2026-09-10)

- Virtual-manifest workspace (16 crates + server binary), pinned toolchain
  1.98.1, fmt/clippy/pedantic lint contract, CI workflow, TOML config with
  validation, structured logging, deterministic tick clock, lifecycle with
  graceful shutdown, shared test-support crate.

### Phase 02 — Network & protocol (2026-09-10)

- Tokio connection lifecycle, VarInt/VarLong and frame codecs with
  hostile-input caps (5/10-byte, 2 MiB), handshake/status/login/config state
  machines, offline authentication, compression negotiation with bomb
  rejection, connection admission limits, packet-fixture and fuzz harnesses,
  and an end-to-end test client reaching Play.

### Phase 03 — Persistence (2026-09-11)

- NBT (disk + network encodings), Anvil region reader/writer with atomic
  tmp→rename saves, chunk serde that preserves unknown fields, dirty tracking
  with retry-on-failure, autosave scheduling, corruption and restart suites.
- Closed risk R-03 by measurement: DataVersion 4790, `version` 19133; the
  writer stamps what vanilla 26.1.2 writes.
- The differential proof of the phase: a vanilla-world round trip where our
  rewritten regions and `level.dat` were accepted by a real vanilla server.

### Phase 04 — Survival vertical slice (2026-09-11)

- Block/item registries loaded from the jar's own registry dump (1 168 blocks,
  29 873 states, 1 506 items); world/chunk runtime with swept collision and
  ray casting; player state (health/hunger/XP with hostile-NBT hardening);
  break/place validation; death and respawn; join streaming with per-tick
  budgets; real-socket E2E tests; the first tick-cost baseline.

### Phase 05 — Simulation, entities, physics, AI (2026-09-11)

- Six-phase tick order as a compile-time contract, tick metrics, a
  byte-exact `java.util.Random` reimplementation (verified against JDK 25
  vectors — it caught two real bugs), entity lifecycle/ids that are never
  reused, item entities, projectiles, effect containers, mob tables and
  goal-based AI, bounded deterministic pathfinding, entity-heavy tick baseline.
- Opened with the second adversarial audit (data loss: streamed chunks could
  overwrite stored terrain; a placement DoS; RNG sign-extension bugs).

### Phase 06 — Inventory, containers, block entities, redstone (2026-09-11)

- Server-authoritative inventory transactions: stale-state-id resync,
  computed slots, conservation proven under 2 000-click adversarial floods;
  shaped/shapeless crafting; furnace with exact burn/cook accounting;
  hopper transfer model; block-entity lifecycle.
- Redstone: power model, budgeted propagation that never drops updates,
  golden circuit tests, determinism proofs — model-complete and explicitly
  not yet wired into the tick loop.
- Third adversarial audit; the first project commit (`b7b1c99`) landed here
  with owner approval.

### Phase 07 — Commands, data packs, worldgen (2026-09-11)

- Command tree/dispatcher with permission-before-grammar checking; eight
  commands reachable from a real client; `execute` modifier chains; `/function`
  with recursion and privilege bounds.
- Real data-pack loading: 758/758 vanilla tags resolve cleanly, 1 421 recipes
  load (94 counted as unmodelled), registry split fix, `ops.json` read at
  startup, pack discovery from world directories.
- Worldgen: seeded Perlin terrain, six biomes, trees, structure loading
  (1 182 of 1 202 templates) with a single-chunk placement policy, wired into
  generation with golden tests; existing-world-first generation.

### Phase 08 — Pi hardening & operations (2026-09-12)

- Operational guardrails: config bounds, structured 30-second metrics lines,
  shutdown barrier (drain 5 s → bounded save 30 s), systemd unit, offline
  whole-copy backup/restore with manifest and overwrite guard, named
  connection limits with a full-server bypass path, slow-drip and registry
  reservation caps, command-flood proof, admin-safety review.
- The benchmark harness (10-player workload driver, chunkgen burst, dirty-save
  timing, profile run) with the burst/settled separation rule; honest no-fix
  verdict where evidence did not support a change; operational runbook.

### Phase 09 — Conformance & release candidate (2026-09-12)

- Full-matrix sweeps across every domain (the sweep measured **1 194 / 0 /
  21 over 74 suites**; the remediation and Audit 08 coverage tests brought the
  tree to **1 196 / 0 / 21**); all seven differential suites green
  including the vanilla round trip; both build profiles measured; release
  build reproducible with a real-socket smoke.
- Known-divergence catalog, release-candidate documentation, plugin-boundary
  ADR (three named seams, zero API types), independent adversarial review
  (10 findings, all dispositioned), acceptance report.
- Fixed the one known flaky: three metrics tests shared a `TempDir` tag whose
  uniqueness collapsed under parallel I/O (probe-proven: duplicate paths in
  160 000 same-tag constructions); per-test tags now follow the codebase
  convention.

### Platform work (2026-09-12, post-phase)

- **Raspberry Pi 5 acceptance executed**: on-device release build with the
  pinned toolchain; 30-minute soak under the installed systemd unit with 10
  scripted clients — settled MSPT p50/p95/p99 medians 0.21/0.27/0.29 ms, zero
  overruns outside the join burst, clean autosaves, graceful stop verified.
  Full record: [docs/performance/BENCHMARK-BASELINE.md](docs/performance/BENCHMARK-BASELINE.md).
- **Deployment defect found and fixed by that run**: the registry tables were
  read from a build-tree path baked in at compile time, so the first real
  systemd application could not start. `Registries::vanilla()` now searches
  `$MC_FIXTURE_DIR`, then `fixtures/registry/` next to the executable, then
  the build tree, with ordering regression tests.

### Governance (2026-09-12)

- **MIT license adopted** (owner decision; [ADR-0006](docs/adr/ADR-0006-licensing.md),
  closing risk R-09) — `LICENSE`, license fields on all 17 manifests, the
  cargo-deny licence gate now covers the workspace's own crates.
- Source published at `github.com/antifield26/Apoptosis`.
- Repository governance: standard project facade (README, CHANGELOG,
  CONTRIBUTING, SECURITY, NOTICE, `.editorconfig`), the engineering
  conventions carried in-repo ([docs/CONVENTIONS.md](docs/CONVENTIONS.md)),
  divergence catalogs merged into a single parity matrix, the test matrix
  restructured to a current view plus a deduplicated defect history,
  per-phase reports distilled into this file and retired to git history, and
  the documentation-audit scripts committed to `tools/docs-audit/`.

[`v0.1.0-rc.1`]: https://github.com/antifield26/Apoptosis/releases/tag/v0.1.0-rc.1
