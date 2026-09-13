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

### P10-03 (continued) \u2014 a real 26.1.2 client reaches play

Four further real-client runs, each naming the next gap. **The fifth ends with the client in the play state**,
which is the first time a Java client has done so in this project \u2014 KD-38 has read "boundary (not yet
exercised)" since Phase 09.

| Run | Client said | Outcome |
|---|---|---|
| 3 | `Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]` | fixed \u2014 `damage_type` sent with its 33 tags |
| 4 | `enchantment: Failed to parse value` for every entry | **excluded, with its reason recorded** |
| 5 | `Failed to decode clientbound/minecraft:set_default_spawn_position` | **play reached** |

**Run 4 is the important negative result.** Every enchantment failed to parse, and the cause is a limit of the
approach rather than a missing registry: those fields use *dispatch* codecs (a bare number or an object with a
`type`), NBT lists are homogeneous, and a float is a different tag from a double. A converter that infers
everything from JSON **shape** cannot express any of the three. Excluding the registry was the honest move and
it is also what unblocked the phase: sending a payload the client rejects is a hard failure, while omitting it
leaves a gap the client names precisely \u2014 and it named none.

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
real payloads from a vanilla 26.1.2 server \u2014 the jar is already in this workspace and the P10-01 rig is the
tool for exactly this \u2014 and replay them, as this project already does for packet ids, block states and the
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

### P10-03 (continued) \u2014 replaying the server's own bytes, and two more play packets fixed

The capture remedy, applied to the registries. **The finding inverted the approach.**

**Vanilla sends registry ids and no element data.** 382 entries across 28 registries in 8 781 bytes, with every
`has_data` flag false, followed by 32 316 bytes of tags. It can, because the client declared
`minecraft:core = 26.1.2` under `select_known_packs`, so the server sends the registry *shape* and the client
reads the content from its own jar.

So our previous design was wrong twice over, and only the first was visible. It could not encode
`enchantment`'s dispatch codecs or `villager_trade`'s mixed-type arrays \u2014 the failures I spent two rounds
excluding registries over \u2014 and more fundamentally it was **sending data the protocol does not ask for**.
The client parsed it because it was there, and refused when it did not match. Both \"problems\" were
self-inflicted. (`villager_trade` is not even a synced registry: vanilla does not send it at all.)

The converter, its JSON fixture and both exclusions are gone. The payload is now the vanilla server's own
bytes, replayed verbatim from a committed 41 338-byte fixture built by
`tools/vanilla-probe/build_config_payload.py`, with the rig's new `--bodies` mode as the capture path.

**`player_position` (KD-42) fixed from captured bytes.** The client reported `found 1 bytes extra`, which reads
as a width problem. The capture showed the total was right and the **order** was not: 26.1.2 leads with the
`VarInt` teleport id and we wrote it last, so the client consumed the top byte of `x` as the id. Fixed, pinned
by a golden test holding the 61 captured bytes, and verified \u2014 the client's session grew from **119 to 444
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
starting the server, then \u2014 after that was fixed \u2014 probed the rig's port by **connecting** to it, which
consumed the single connection the rig exists to serve. A liveness probe must not change what it observes.

### P10-03 (continued) \u2014 a real 26.1.2 client plays with no protocol errors

**KD-43 closed, and the session is stable.** A real vanilla 26.1.2 client \u2014 the owner's installation,
through the P10-01 rig \u2014 now runs a complete session against this server with **no protocol-error report
written at all**, and is *live* rather than merely connected:

| Signal | Reading |
|---|---|
| client \u2192 server, play id 28 | `keep_alive` \u2014 it **answered** the server's keepalive |
| client \u2192 server, play id 13 | **410 per-tick reports** (\u224820/s) \u2014 the game loop is running |
| server \u2192 client, play id 113 | 17 `set_time` packets, accepted |
| server \u2192 client, play id 45 | 289 chunk packets |
| rig | `degraded=[]` \u2014 everything observed cleanly |

**What `set_time` actually is.** Captured: **eighteen** packets at id 113, all **9 bytes**, with the leading
`i64` incrementing by exactly **20** \u2014 one second of ticks, which is this packet's send rate. The id is
confirmed independently by the jar-derived `packet-ids-775.tsv` (`game clientbound 113 set_time`) and by the
client's own error text. So **26.1.2 removed `time_of_day` from the wire**: the client derives the time of day
from the `world_clock` registry, which is precisely why this phase had to make that registry work before play
was reachable at all. We sent `i64` + `i64` + `bool`, 8 bytes too many. Fixed, pinned by a golden test.

**One packet is left unexplained, deliberately.** Of nineteen packets at id 113, eighteen are 9 bytes and
**one is 31**: its `world_age` of 7405 fits the sequence immediately before the first 9-byte packet, but the
remaining 23 bytes contain `3f800000` (`1.0f`) twice, and `set_time` has no float field at all. Eighteen
uniform packets settle the format, so the fix does not depend on it \u2014 but if that packet is genuinely
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
"a client plays, and **nothing about what it renders has been verified**" \u2014 the client could be staring at
an unlit void and nothing here would know. That is what P10-04 onward exists to settle.

### P10-04 / P10-05 \u2014 reconnaissance: `level_chunk_with_light` cannot be parsed at all

Starting the light engine turned up a defect that comes **before** it, and which changes what P10-05 has to do.

**All 117 captured vanilla chunk packets fail to decode with our implementation**, every one identically:

```text
light mask has 1 sections set but 0 arrays follow
```

**Two hypotheses, one excluded.** The first was that our decoder is stricter than the protocol \u2014 that a real
server sends a mask bit with no array. That is now ruled out by arithmetic that uses **neither** decoder: a
light array is fixed-width, so for a packet of known size only one small (sky, block) pairing can consume the
remainder. **None does \u2014 they are all 24 bytes short.** So the bytes being read as masks cannot be masks.
They look plausible because light data is mostly zero, which is exactly why a shared misreading survived two
implementations.

**What the real bytes do confirm.** Scanning for the array signature \u2014 a `80 10` VarInt (2048) followed by a
mostly-`0xFF` span \u2014 finds it at offset 5229 of a 7280-byte packet, ending at 7279. So `LIGHT_ARRAY_BYTES`
and the length-prefixed array convention are **right**, and **one byte remains after the array**, which is
itself unexplained.

**Not established: where the 24 bytes are.** The candidates \u2014 an extra heightmap long per entry, a misread
`data` size, or a field between them \u2014 are not distinguished yet, and guessing is what this phase keeps
paying for. The next step is to settle it against the bytes rather than to start filling masks.

**A note on method.** A diagnostic test is committed (`vanilla_chunk_light.rs`, ignored by default because the
capture lives under `target/`). Its first version asserted the packet ends with `[0x80, 0x10]`; the slice came
back `[0x10, 0xff]`, one byte out. That assertion was **removed rather than adjusted**, because a claim the
evidence does not support is the failure mode this whole phase has been unlearning. What it asserts instead is
the part that is solid: every captured packet fails identically, which is what makes this a layout defect
rather than a quirk of one packet.

**Consequence for the plan.** P10-04 (the light engine) is not blocked \u2014 it computes light and does not care
about the packet. **P10-05 is blocked**, because filling four masks is meaningless while the field order around
them is wrong.

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
