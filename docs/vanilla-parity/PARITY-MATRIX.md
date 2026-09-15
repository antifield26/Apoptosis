# Vanilla Parity Matrix — the single authority

Target: Java 26.1.2 (protocol 775, Display "26.1"). This table is the one
authoritative answer to "where does this server differ from Vanilla?" — the
separate divergence catalog it once coexisted with was merged into it
(governance, 2026-09-12); the stable `KD-nn` identifiers are preserved here so
older documents' citations still resolve.

Update rule: a row may move to `partial`/`full` only with cited evidence
(fixture/differential/E2E); intentional divergence needs a note here
(CONVENTIONS.md §12). "Test client" means our own codec client; it does **not**
prove real-client acceptance. "Vanilla differential" means the real 26.1.2
server jar was run against files our code wrote
(`crates/persistence/tests/vanilla_differential.rs`).

Status vocabulary: **full** (verified, evidence cited) · **partial** (works
within named limits) · **gap** (missing, tracked, never silently substituted) ·
**unverified** (implemented from recall or a product decision) · **intentional
divergence** (different on purpose, reason recorded) · **boundary** (a scope
line the product contract draws).

## 1. Protocol and networking

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| Handshake/intent | proto 775, intents 1/2/3 | **partial** | Golden bytes + round trip (`protocol/tests/fixtures.rs`); real-client not yet verified |
| Status ping | JSON + ping round-trip + close | **partial** | E2E `status_ping_reports_protocol_and_motd`; JSON shape matches plan |
| Offline login → Config → Play | LoginSuccess→KnownPacks→RegistryData→FinishConfig→JoinGame | **verified against a real client** | A vanilla **26.1.2** client (offline profile, through the P10-01 capture rig) now completes handshake, login, configuration and **enters play** — the first Java client to do so in this project. Five successive refusals were each named by the client's own protocol-error report and each fixed, ending with the configuration registry payload being accepted. **The session is now stable and error-free**: no protocol-error report is written at all, and the client is live rather than merely connected — it answered the server's keepalive, sent 410 per-tick reports and accepted 17 `set_time` packets over the observation window. The excluded registries are gone: the payload is replayed verbatim from a captured vanilla server (KD-41), and the play-state divergences named along the way (KD-40, KD-42, KD-43) are all closed. **What is not established is anything about rendering** — see KD-38 |
| `select_known_packs` / registry payload (KD-41) | a client that has the pack is sent registry ids **without element data** | **closed — captured and replayed** | Captured from a real vanilla 26.1.2 server: **no entry carries data** — 382 entries across 28 registries in 8 781 bytes, every `has_data` flag false, plus 32 316 bytes of tags. Vanilla can omit the content because the client declared `minecraft:core = 26.1.2`, so the client reads it from its own jar. Our server converted the jar's data-pack JSON into NBT and sent **full element data**, which the protocol never asks for and the client then tried to parse — the real cause of the `enchantment` dispatch-codec and `villager_trade` mixed-array failures, which were self-inflicted rather than a missing capability. The converter, its JSON fixture and both exclusions are gone; the payload is now the server's own bytes, replayed verbatim from a 41 338-byte committed fixture |
| Play-state `player_position` (KD-42) | `VarInt` teleport id **first**, then 6 `f64`, 2 `f32`, `i32` flags | **closed — fixed from captured bytes** | A real client reported `found 1 bytes extra`, which reads as a width problem. The capture showed the total was right and the **order** was not: vanilla leads with the teleport id and we wrote it last, so the client consumed the top byte of `x` as the id. Fixed and pinned by a golden test holding the 61 captured bytes. Verified: the client's session grew from 119 to 444 packets past it |
| Block light vs a real server (KD-47) | our engine's block light equals vanilla's for vanilla's own blocks | **verified to 99.95%, gap quantified** | A second capture, with eight glowstone blocks placed through the **server console** (`forceload add` first — `setblock` refuses with "That position is not loaded" on a fresh server), so vanilla computes block light and sends the arrays. **40 939 of 40 960 cells match, worst difference 3**, with every disagreement confined to the two chunks adjacent to the source. The cause is `compute_chunk_light`'s **one-block margin**: light enters a chunk across its border but cannot travel several blocks outside it first. Exactness needs a **15-block** margin — 6.5x the work per chunk — so the real fix is to compute light over the **loaded world** rather than per chunk, which is the same work KD-45 already calls for. The test asserts the measured bound with the reason at the assertion |
| Sky light vs a real server (KD-46) | our engine's sky light equals vanilla's for vanilla's own blocks | **verified — 100.0000%, cell for cell** | The captured chunk packets carry **both** halves: the block states of a real vanilla world and the light arrays vanilla computed for them. So our engine is handed vanilla's own blocks and its answer compared against vanilla's — no modelling in between. 65 536 of 65 536 cells match, worst difference **0**, across 8 chunks (2 sky arrays each). The margin is approximated by **replicating** the chunk's edge column rather than treating it as air, which is honest for a superflat world. **Block light is not covered**: vanilla sent no block-light arrays because a superflat world has no light sources, and the test asserts that gap rather than leaving it implicit |
| `light_update` client acceptance (KD-49) | whether a real 26.1.2 client accepts our packet | **closed — a real client accepts it** | Two captures placed four glowstone blocks beside a connected player (`replace` and `destroy` update modes, the second chosen because `javap` on `SetBlockCommand` showed `updateNeighboursOnBlockSet` runs only on that path). **Neither produced an id-48 packet** — zero in 19 000 captured packets, with `level_chunk_with_light` staying at the initial 117. So the guessed reason was wrong and the real trigger is unknown; it is not being invented. Separately, our own `light_update` has been accepted only by our `TestClient`. **That risk is now narrowed rather than merely noted**: a test asserts the packet's light half is **byte-identical** to the light half of `level_chunk_with_light` for the same light — and a real client accepts that half on every chunk it is sent, across 753-packet sessions with no protocol error. Both go through one implementation, `write_light_data`. So the only part of `light_update` no real client has exercised is **two `VarInt` coordinates**, in a position `javap` documents. That is a much smaller claim than "the packet is unverified", and `tools/visual-check/run.py` asks the owner to break one block. **Settled by giving the server a second player**: a real client only receives a `light_update` when the server changes a block while it is connected, and the server changes blocks only when a player breaks one, so the method is two clients — the real one through the rig, and a `TestClient` that logs in separately, reads its position from the `player_position` packet that arrives **after** `join_game`, and breaks the block beneath itself. The update then goes to **every** session holding that chunk. **Result: no protocol-error report, and the client still running**, its only ERROR lines being the offline profile\u2019s expected 401s. Committed as `tools/light-update-trigger/run.py` with the driver at `crates/server/tests/light_update_trigger.rs`. **Vanilla\u2019s own trigger remains unestablished**: two captures with a connected player and console-placed glowstone produced no id-48 packet, and the `javap`-based guess about the block-update flag was wrong — recorded rather than explained |
| `light_update` on change (KD-48) | the light of a changed chunk reaches clients that hold it | **implemented, sent, and tested end to end** | P10-05 named this and it was missing: a placed block changed the block and **not the light**, so a torch did nothing visible until the chunk was re-sent. The packet (clientbound play 48) carries the **same light data** as the tail of `level_chunk_with_light` — four `BitSet` masks and two array lists — but writes its coordinates as **`VarInt`** where the chunk packet uses `i32`, settled with `javap` on the jar and pinned by a test that asserts both encodings side by side. The shared half is one implementation (`write_light_data`/`read_light_data`) because the last time a format was written twice in this phase the two agreed with each other and were both wrong. Work is **queued on block change and spent at four chunks a tick**, so a burst is delayed rather than dropped, and the changed chunk's **neighbours** are queued too since their margin-read light changed. Verified by a test asserting the packet id arrives at a joined client after a block is broken |
| Light on the wire (P10-05) | the four masks and both arrays carry the computed light | **implemented and sent** | Every light section is accounted for: a section that is uniformly the layer's default (15 sky, 0 block) goes in the matching `empty_*` mask, anything else gets an array and a bit in the matching mask. Bit `i` is light section `i`, i.e. world section `i - 1`, so the sections below and above the world are included as full-sky/zero-block. The rule came from the capture: across 117 packets no section is ever in a mask *and* an empty mask, `empty_sky` only ever sets bit 0, and the sky arrays are the open-sky section (uniform 15) and the surface section (mixed). In a real client session the chunk bodies are now 4 626..8 732 bytes, mean 6 108, against vanilla's 7 280 and 9 322 for a comparable world. **Not verified: whether the world looks right** — light is not a field a client validates, so no error would appear either way. |
| Light performance (KD-45) | light computed once per chunk, relit only where a block changed | **partially closed — cached and invalidated; first computation still dominates** | Light is no longer recomputed on **every** chunk send: it is cached per chunk in `World` and dropped when the chunk or a neighbour whose one-block margin reads across the border changes. Measured: the command suite went **11.5 s -> 9.17 s** in debug and runs in **3.46 s in release**, so much of the CI failure was a debug-build cost rather than a production one. Measured end to end on the command suite: **11.5 s (none) -> 9.17 s (cached) -> 6.59 s (cached + cursor)** in debug, and **3.46 s -> 3.20 s** in release. The release figure is the one that matters, and it says the remaining cost is the **inherent array work** — three passes over 124 320 cells — not lookup overhead: the cursor removed a quarter of a million `BTreeMap` lookups per chunk and bought only 7% in release, where the compiler and the cache were already hiding them. **What is still not done:** *incremental relighting* — a torch in a large lit chunk costs a full recompute where vanilla relights only the region the change can reach. **And nothing helps a first join**, which must compute each chunk once whatever the cache does. The next lever is to skip the frontier scan for sections that are uniformly lit, which most sections in an open world are |
| `level_chunk_with_light` light masks (KD-44) | the four masks are `java.util.BitSet`, not `VarInt` | **closed — verified in both directions** | **All 117** captured vanilla chunk packets failed to decode, identically: `light mask has 1 sections set but 0 arrays follow`. Root cause settled with `javap -c` on the jar's `ClientboundLightUpdatePacketData`: the read order is `readBitSet` × 4 then `readList` × 2, and `readBitSet` is a **`VarInt` count of longs followed by that many `i64`s** — we read them as `VarInt`s. **An empty mask is a single `0x00` in both encodings**, which is why this survived: our reading agreed with a real server for an empty mask and disagreed for any other, and Phase 04 only ever sent empty ones. Re-running the layout search under the correct model finds one offset per packet — the same offset across twenty — with `sky bits [1,2]` and two arrays, `block bits []` and none, closing the arithmetic exactly; our header parse was right all along. Fixed by representing the masks as set indices and writing them the way `BitSet.toLongArray` does, trimming trailing zero longs. **Verified both ways: all 117 captured packets now decode, and a real 26.1.2 client still reaches play with no protocol error.** Unblocks P10-05 |
| Play-state `set_time` (KD-43) | `i64` world age + one byte | **closed — a stable session follows** | A real client rejected our 17-byte payload (`i64` + `i64` + `bool`) as `was larger than I expected`. The capture shows **eighteen** packets at this id, all **9 bytes**, with the `i64` incrementing by exactly **20** — one second of ticks, this packet's send rate — and the id is confirmed by the jar-derived `packet-ids-775.tsv` (`game clientbound 113 set_time`). So **26.1.2 removed `time_of_day` from the wire**; the client derives the time of day from the `world_clock` registry. Fixed, pinned by a golden test. **One packet remains unexplained and is recorded as such**: a 31-byte packet at the same id whose `world_age` fits the sequence but whose remaining 23 bytes contain `1.0f` twice, which `set_time` has no field for. Eighteen uniform packets settle the format; that one is an open question about the capture, noted because it would mean an id-table disagreement if genuine |
| Play-state `set_default_spawn_position` (KD-40) | `Identifier` dimension + `BlockPos` + `f32` yaw + `f32` pitch | **closed — fixed from captured bytes** | A real client rejected our 12-byte payload with `readerIndex(10) + length(4) exceeds writerIndex(13)`. Rather than guess the width, the packet was **captured from a real vanilla 26.1.2 server** through the P10-01 rig: 37 body bytes = `13 6d696e6563726166743a6f766572776f726c64` ("minecraft:overworld"), `0000000000000fc4` (i64 4036 = packed `BlockPos(0, -60, 0)`), then two zero floats. 26.1.2 leads with the **dimension** and carries a **pitch**; our encoder had neither. Fixed, verified by the client proceeding past it (the trace grew from 54 to 121 packets), and pinned by a golden test holding the captured bytes. Both readers are big-endian (`to_be_bytes`, matching Netty), so only the field list was wrong |
| Configuration dynamic registries (KD-39) | every synced registry a 26.1.2 client requires, with its tags | **closed — a real client enters play** | Three real-client runs, each naming the next gap. **Run 1** refused on the two the overworld `dimension_type` *references* (`timeline`'s `in_overworld` tag, `world_clock`'s `overworld` value) — fixed, and the client's next report shows `timeline: elements=4 tags=4` and `world_clock: elements=2` with both errors gone. **Run 2** then said `Registry must be non-empty` for thirteen variant registries — fixed, and the payload grew to 17 registry packets with none of those errors. **Run 3** refused on `Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]` — fixed by sending `damage_type` with its 33 tags. **Run 4** refused on every `enchantment` entry with `Failed to parse value`, which is a limit of a shape-based JSON->NBT converter rather than a missing registry (dispatch codecs and float-vs-double are not expressible in shape), so that registry is **excluded with its reason recorded**. **Run 5 reaches play**: the trace's states are `handshake, login, config, play` with `join_game` present, so the configuration payload is accepted. The registry payload has since been replaced by captured bytes (KD-41), removing the excluded registries entirely |
| Online-mode Mojang auth (KD-01) | session verify behind config | **not implemented (fails fast)** | `online_mode_refuses_to_start_without_provider`; boundary trait + call site exist |
| Compression (network) | threshold negotiate, bomb-safe | **partial** | framing unit tests + E2E login at threshold 256; real-client not yet verified |
| Keepalive | timeout kick, ping accounting | **partial** | keepalive deadline is the liveness authority (a read deadline no longer masks it — Audit 01); E2E answers one; timeout path covered by `network/tests/keepalive.rs` |
| Packet ids | 69 serverbound / 141 clientbound play packets, ids by registration order | **full for every id we use** | `docs/protocol/packet-ids-775.tsv` extracted from the official jar's registration bytecode; `protocol/tests/packet_ids.rs` asserts each constant + contiguity. Fixed a real bug: `chat_command` was 8, is **7** |
| Signed chat (1.19+) (KD-02) | `serverbound:chat` + timestamp/salt/signature/last-seen | **decoded, not processed** | Full payload consumed and range-checked (`play.rs`); no chat forwarding/commands yet (P07) |
| Login-phase tolerance | clients may send `custom_query_answer`/`cookie_response` before the ack | **partial** | Audit 01 replaced the hard error with an ignore-loop (`connection.rs`), and `crates/network/tests/login_tolerance.rs` covers it. Partial because the real client behaviour that motivated it is unverified (no 26.1.2 client here) |

## 2. Persistence and world files

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| Anvil region format | 32×32 slots, 4 KiB sectors, 8 KiB header, `(sector<<8)|count`, compression ids 1/2/3/4/127 | **full (default codec)** | Reader+writer, 16 corruption cases, and a **vanilla differential**: vanilla booted on region files we wrote and re-saved them |
| Region compression codecs (KD-04) | gzip(1), deflate(2), none(3), lz4(4), custom(127) | **partial (1/2/3; 4 and 127 refused by name)** | `compression.rs` tests; `region-file-compression=lz4` worlds are rejected with an explicit error rather than misread. Documented gap |
| External `.mcc` chunks (KD-05) | used for chunks ≥ 256 sectors | **not implemented (explicit error)** | `corruption.rs::unsupported_codecs_and_external_chunks_report_what_is_missing` |
| NBT (disk + network) | named/nameless roots, Java modified UTF-8, 12 tag types | **full** | `mc-nbt` tests incl. measured `DataOutput.writeUTF` byte vectors; byte-identical repack of vanilla palettes |
| Chunk NBT fields | `DataVersion/xPos/zPos/yPos/Status/sections/Heightmaps/structures/entities/...` | **full for 26.1.2 output** | Real vanilla chunk decoded field-by-field; unknown fields preserved in `extra`; write path accepted by vanilla |
| Palette packing | `bits = max(min_bits, ceil(log2(palette)))`, LSB-first, no long spanning, 4096/64 entries | **full** | `anvil_fixture.rs` unpacks and repacks vanilla arrays byte-identically (11 packed palettes) |
| Chunk status semantics | `minecraft:structure_starts` … `minecraft:full`, `isLightOn` | **partial (preserved, not produced)** | All five statuses observed in a real world and round-tripped; generation itself is P07 |
| 26.1 dim-path layout | `dimensions/<ns>/<v>/{region,entities,poi,data}` | **full (read+write)** | Real world laid out this way; reader also handles pre-26.1 `region/`, `DIM-1`, `DIM1` with a deprecation warning |
| 26.1 `level.dat` schema | `spawn` compound, `difficulty_settings` string, `version` 19133, `DataVersion` 4790 | **full (write), tolerant (read)** | Vanilla loaded our rewritten `level.dat` without complaint and kept our `Time`; legacy pre-26.1 shapes still read |
| `DataVersion` write stamp | 4790 for 26.1.2 | **full (R-03 resolved)** | Measured in a vanilla `level.dat`/chunks + `DetectedVersion` bytecode + `version.json` |
| DataVersion window (KD-06) | vanilla refuses newer worlds | **intentional divergence (narrower)** | We accept only `[4435, 4790]` (no datafixers). A vanilla 1.21.9..26.1.x world loads; older worlds are refused with a clear error instead of being datafixed |
| Corrupt/partial saves | vanilla reads what is present, warns otherwise | **full for the measured cases** | Short final sector tolerated (regression test); genuinely unreachable/corrupt slots produce typed errors |
| Unknown `level.dat` entries (KD-07) | vanilla's datafixer **drops** them | **intentional divergence (we preserve)** | Probe field disappeared across a vanilla load/save; we keep unknown `Data` entries in `level.extra` so a load/save cycle is lossless for our own tooling |
| Save ordering/atomicity | tmp→rename for `level.dat`, header word last for chunks | **full for the measured halves** | `level.dat` tmp→rename tested (`save.rs`, `restart.rs`); chunk half tested by `region.rs::the_location_word_is_written_last` (P08-07): after two writes the on-disk location word points at a self-consistent payload holding the second write. Shutdown ordering (network drain → bounded final save → close) covered by `lifecycle.rs::shutdown_passes_through_stopping_before_stopped` |
| Autosave cadence (KD-08) | default 6000 ticks | **partial (configurable; the default is a product decision)** | The mechanism is implemented and tested (`autosave.rs`), configurable via `storage.autosave_ticks`, and wired through the server lifecycle. The **default of 6000 is not verified against Vanilla** — Audit 05 found the tests only re-assert the same constant. Recorded as a product decision rather than a parity value |
| `data/minecraft/*.dat` (game rules, weather, clocks, world-gen settings) (KD-09) | separate `{DataVersion, data}` documents | **partial (read/round-trip only)** | Shapes measured; P03 does not yet write gameplay data files (P04/P06/P07 own them) |

## 3. Simulation and gameplay

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| Collision axis order (KD-10) | Vanilla resolves Y, then X, then Z | **intentional divergence** | `World::move_with_collision` resolves X, then Y, then Z. Observable only when a box is obstructed on two axes in the same step, and correcting it would change movement feel; recorded here because Audit 02 promised this row and did not add it |
| Movement/collision (KD-11) | server-authoritative, swept collision, 0.6×1.8×0.6 hitbox | **partial** | `mc-world` swept per-axis resolution + `Game::move_player` caps and corrects; tested for teleports, NaN/Inf, walls, falls. **Gap**: full-cube shapes only (slabs/stairs/fences collide as blocks), no step-up assist, no ladders/vines |
| Break/place validation | reach, occupied-target and hitbox checks, item consumption | **partial** | `survival_e2e` — 40-block dig refused, placement refused inside a player or an occupied block, survival consumes exactly one item on success. **Gap**: no block-specific placement rules, no tool/speed timing, no block-drop items |
| Block state storage | global state ids (29 873), disk names ↔ wire ids | **full for read/write** | `mc-registry` from the jar's own registry; `world/tests/vanilla_chunk.rs` round-trips a real vanilla chunk losslessly |
| Item stacks (KD-24) | id + count + data components | **partial** | `mc-entity::stack` with a 205-entry resolved table. **Gap**: no data components (durability, enchantments, custom names), no item NBT |
| Player inventory | 36 main + 4 armour + 1 offhand + crafting grid | **partial** | Correct window permutation and hostile-index rejection, and since Phase 06 the menu enforces transactions server-side: a stale state id applies nothing, computed slots refuse placement, and a 2 000-click hostile flood neither creates nor destroys an item. **Gap**: no armour auto-equip; the crafting grid exists but nothing fills it from a client |
| Inventory transactions (KD-03) | `container_click` moves items server-side | **partial** | Decoded (`PlayIntent::ContainerClick`, field order verified from the jar), validated against the session's menu, applied, and acknowledged with `container_set_slot`/`container_set_content`; 6 tests over a real socket including the adversarial cases. **Gap**: the packet's two trailing `HashedStack` fields are not decoded (they are the client's desync *prediction*, not the authority mechanism, which is the state id); only the player menu exists; `container_set_data` is not sent |
| Container windows (KD-21) | chests, furnaces, hoppers open as menus | **not implemented** | Only the player inventory (`window 0`) can be open. The container model supports arbitrary menus, but nothing opens one |
| Redstone conductivity (KD-15) | a solid block conducts strong power to adjacent dust | **not implemented (largest redstone gap)** | A solid block is never powered, so "lever on a block, dust on the far side" does not work and the strong/weak distinction is carried but **unobservable**: a strongly-emitting redstone block and a weakly-emitting lever both give adjacent dust 14. A test asserts the gap rather than glossing it |
| Redstone wire length (KD-12) | dust carries 15 blocks from a source | **divergent (unverified which matches 26.1.2)** | `WIRE_LIVE_BLOCKS = 14`, one fewer than the wiki's "up to 15 blocks", because the implemented rule charges the first dust block an attenuation step. The 15th block is dark. The constant documents the discrepancy |
| Redstone update order (KD-13) | per-level, with a block-update/shape-update distinction | **divergent (intentional)** | Positions are processed in the update queue's documented sweep order, so locational circuits (those depending on which of two paths updates first) will differ. Vanilla's block-update/shape-update distinction and its separate comparator-update channel are absent |
| Redstone component direction (KD-14) | torches power their attachment side, comparators read sides | **not implemented** | Components are direction-agnostic: a torch does not power only above/attachment, comparator side inputs are read as zero, and a mechanism block is `Passive` and never reacts — absent, not silently substituted |
| Crafting | the Vanilla recipe set, tags, recipe book | **partial (hand-written subset)** | Shaped/shapeless matching with the exact-fill rule, offset and mirroring, plus the crafting-result slot; 24 tests. **Gap**: the recipes are a **hand-written baseline, not the Vanilla set** (asserted from community knowledge, not a dump); no ingredient tags, no other recipe types, no recipe book, no ingredient remainders. P07-03 |
| Smelting (KD-26) | the `fuelValues`/smelting data tables | **partial (hand-written subset)** | `Furnace::tick` with exact burn/cook accounting and the rule that a blocked output wastes no fuel; 21 tests. **Gap**: fuel and recipe values are from recall and labelled as such; no blasting/smoking/campfire, no fuel tags, no fuel remainder (lava bucket → bucket), no XP orbs |
| Hopper transfer (KD-22) | a scheduled, conservation-safe transfer | **partial** | `Hopper::transfer` with per-container slot roles, conservation by construction, and a randomised adversarial test. **Gap**: **no schedule** — the 8-tick cooldown is a constant the caller must honour and nothing ticks a hopper; one move per call rather than Vanilla's pull-and-push |
| Block entities (KD-20) | typed state that persists and syncs | **partial** | Typed payloads, a deterministic store, and a game-loop lifecycle: a block change retires its entity. **Gap**: **no persistence** (no payload↔NBT conversion), **no client sync** (`block_entity_data` is not sent), **and breaking a container with contents loses the items** — reported in the log and the tick report, but still lost |
| Redstone power model | weak/strong power, attenuation, 15 levels | **partial (model, not wired)** | `mc-redstone`: `PowerLevel`/`PowerState`/`SignalKind`, per-source tables, attenuation 1 per block, 8 power tests. **Gap**: nothing feeds the model from a placed block, and no differential test against a real server exists |
| Redstone update scheduling | neighbour updates, scheduled ticks, budget | **partial (model, not wired)** | A deterministic `(due tick, position)` queue with a bounded neighbour set and a per-tick budget that **leaves work queued** rather than dropping it; 7 budget tests. **Gap**: the server does not drive it; `TickPhase::ScheduledTicks` is still a documented no-op |
| Redstone components | torches, repeaters, comparators, pistons, observers | **partial** | Lever, torch, repeater (documented delay) and comparator (compare/subtract) as pure output functions with golden tests. **Gap**: **no pistons, observers, doors, rails, dispensers or droppers**; the delay constants are a model, and Vanilla's exact update order and block-update/shape-update distinction are not reproduced |
| Player health/hunger/XP | damage, regen/starve thresholds, three-branch XP formula | **partial** | `mc-entity::player` 28 tests. **Gap**: no armour/enchantment/resistance/absorption, no damage typing, no invulnerability frames, no knockback, no effect/attribute system |
| Fall damage | 1 per block beyond 3 | **partial** | Implemented in the movement path and asserted. **Gap**: no feather falling, no water/hay negation, no void damage |
| Death/respawn (KD-27) | death screen, respawn, drop inventory | **partial** | Respawn restores vitals and returns dropped stacks. **Gap**: dropped stacks are discarded (item entities are P05), no `keepInventory` game-rule lookup, no XP orbs, no spawn-point search |
| Chunk streaming | (2r+1)² view, incremental, bounded per tick | **partial** | `Game::stream_for` streams nearest-first in a deterministic order with a per-tick budget, and view-distance unloading exists. The **count** is asserted exactly (`survival_e2e`); the *ordering* is implemented but not asserted by any test. **Gap**: no unload/forget packets, no view-distance changes at runtime |
| Lighting (KD-23) | sky/block light, `light_update` | **full (static model), owner-confirmed** | P10-04/05: per-state emission/dampening/`propagatesSkylightDown` read from the jar's own accessors (`LightProbe.java`), sky flood from the heightmaps, block-light BFS, incremental recompute + `light_update` on block change. The acceptance session found the last wire defect \u2014 fully-lit sky sections were marked empty, which the client reads as dark (dead-black surface patches); fixed to leave fully-lit sections unmentioned (vanilla's captured chunks do the same) and **confirmed by the owner on the live client**. Open: no day/night dimming (client-side), no dynamic opacity beyond block changes |
| Entity identity/lifecycle | monotonic ids, never reused, capped | **full for the modelled kinds** | `mc-entity::entity`: `EntityId` is positive by construction, ids are never recycled (so a stale client reference can never resolve to a new entity). 12 tests. **Not covered**: the `MAX_ENTITIES` refusal is implemented but never exercised — the test spawns 4 entities and asserts `len() < MAX_ENTITIES`, which would pass at any cap (Audit 03 recorded this; it survived into Phase 06, and Audit 05 flagged it again) |
| Entity physics | gravity, swept collision, landing, fall damage | **partial** | Per-tick integration through `World::move_with_collision`; `on_ground` derived from the block beneath as well as a stopped fall; non-finite input refused at the solver. **Gap**: full-cube shapes only, no water/lava/ladder handling, no buoyancy (items do not fall slower in water) |
| Mob AI (KD-16) | goal selection (idle/wander/chase/attack/flee) with world effects | **partial** | `MobAi::decide` is pure and reproducible, with a documented goal set per behaviour. **Gap**: **no mob ever spawns** (no spawn rule, P05-11 partial) and **the decisions have no world-side effect** — `Game::tick_entity_ai` is a documented no-op, so no attack is delivered and no path is followed |
| Pathfinding | bounded A* with step-up and falls | **partial** | 4-way + 1-block step up + falls ≤ 3, bounded at 512 nodes / 64 steps, deterministic tie-break. **Gap**: height-only clearance (a 1.4-wide spider fits a 1-block gap), no jump arcs, no cost model beyond distance, no smoothing, `None` rather than a partial path |
| Mob statistics (KD-17) | health, hitbox, speed, attack per kind | **partial (values largely unverified)** | 8 kinds with a per-value confidence label in the module table. Only the zombie's health/hitbox/attack/speed were checked against the wiki; the rest are task-supplied or this project's own choices. `SPEED_BLOCKS_PER_SECOND_PER_ATTRIBUTE` is an explicit derivation that is probably too generous |
| Item entities | drop, physics, pickup delay, despawn, merge | **partial** | `ItemEntity` with gravity 0.04 / drag 0.98 / friction 0.6, a 10-tick pickup delay and 6000-tick despawn; a drop from the hotbar now spawns a real entity. **Gap**: pickup is only a *decision* (`can_be_picked_up`) — no radius search, no inventory insert, no merge search, and nothing is sent to clients |
| Projectiles | trajectory baseline | **partial (baseline only)** | Arrow and snowball gravity/drag/lifetime with the caller owning collision. **Not modelled**: criticals, enchantments, arrow pickup, tipped/spectral arrows, tridents, fireballs, eggs, fluids, item-based damage scaling, and any hit response (bounce/stick/damage/knockback/owner immunity). `MAX_LIFETIME_SNOWBALL` is an explicit placeholder and `ARROW_BASE_DAMAGE` is a floor, not the charge formula |
| Status effects (KD-19) | container, expiry, numeric modifiers, client sync | **partial** | `ActiveEffect` + 8 effects with numeric helpers that are **unit-tested but were never called from production code** until Phase 06 wired `damage_taken_multiplier` into `Game::damage_entity`; `movement_speed_multiplier`, `damage_over_time` and `can_kill` still have **no caller**. Expiry is ticked per entity. Nothing ever *inserts* an effect either — there is no potion, mob effect or command that grants one. **Gap**: no particles, no HUD icon, **not sent to the client at all** (`update_mob_effect` is not encoded), no instant effects, no milk/beacon removal, and the remaining effects are stored but change nothing |
| Damage/invulnerability | typed sources, i-frames, armour | **partial** | Phase 06 wired both: `Game::damage_entity` now refuses a hit inside the 10-tick invulnerability window and scales by `damage_taken_multiplier` (Resistance). **Gap**: no damage typing, no armour/enchantment/absorption, no knockback, no fire/drowning/void damage, **no AI delivers damage yet** so the path has no production caller, and the untyped window is not refreshed on a *blocked* hit the way Vanilla's `hurtTime` is |
| Scheduled block/entity ticks | fluids, block ticks, redstone | **not implemented** | `TickPhase::ScheduledTicks` and `TickPhase::BlockEntities` are documented no-ops that name P05-05/P05-06/P06. No queue exists |
| Entity persistence (KD-18) | entities survive a restart | **not implemented** | Only chunks and `level.dat` are persisted; dropped items and mobs vanish on restart (P05-16) |
| Entity synchronisation | `add_entity`, `remove_entities`, `set_entity_data`, motion | **not implemented** | Entities are server-side only; a client sees no item and no mob (P05-15). The broadcast phase sweeps removals into `TickReport::removed_ids` and does not encode a packet |
| Deterministic tick order | fixed phase order, seeded randomness | **full for the phase order; the randomness half is unused** | `PHASE_ORDER` is a `const` array asserted by tests; intents apply in arrival order; entity iteration is ascending id. `RandomSource` is verified against JDK 25 byte for byte, but **no gameplay code draws from it yet** (`Game::random` is exposed and unused), so its determinism is proven in isolation rather than exercised. Mob spawning (P05-11) is what will consume it |

## 4. Commands and data

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| Command framework | tree, dispatcher, permission levels | **partial** | `mc-command`: a flat immutable tree with validation, a tokenizer honouring quotes and escapes, six argument kinds, and a dispatcher that checks permission **before** the grammar. 27 unit tests + 11 E2E over a real socket (2026-09-12 run log). **Gap**: no branching grammars beyond `execute`; no argument completion (P07-06); no selectors |
| `execute` (KD-32) | modifier chain ending in `run` | **partial (9 of ~20 modifiers)** | `as`, `at`, `positioned`, `align`, `if`/`unless entity`, `if`/`unless block`, `run`, and nesting bounded at depth 16. **Refused by name**, not ignored: `rotated`, `facing`, `anchored`, `in`, `store`, and the `data`/`score`/`predicate`/`biome`/`loaded`/`blocks`/`function` conditions. **Three divergences**: `as @a` runs once as the first match rather than per entity; only players are command sources; feedback goes to the invoker rather than the executing source |
| Commands (KD-31) | the Vanilla command set | **partial (8 of ~90)** | `help`, `list`, `say`, `time`, `tp`, `execute`, `op`, `stop`, each reachable from a client and each with its limits named (the dispatcher tree has nine nodes; `/function` is counted under functions). **Gap**: `help` does not paginate, `list` does not match Vanilla's exact format, `say` broadcasts to players only, `time` sets `timeOfDay` but not `dayTime` and accepts no named presets, `tp` moves only the invoking player, `op` cannot persist a grant (see the permissions row) |
| Permissions (KD-33) | `ops.json`, the four permission levels | **partial (read yes, write no)** | The four levels exist with Vanilla's meanings, the dispatcher enforces them, and **`ops.json` is read** at startup: a listed uuid's level comes from the file, matched by uuid (so a stale `name` still grants, as in Vanilla) with case-insensitive normalisation, and it lives beside the world rather than inside it. 24 tests including an operator stopping the server and a plain player failing to. **Gap**: `/op` does not **write** the file, so a grant needs a restart and an edit — recorded as the remaining P07-04 work rather than presented as done. `bypassesPlayerLimit` is parsed **and enforced at the join gate** (`Game::is_full_for`, P08-06; proven by `ops_e2e::a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator`) |
| Chat relay | player messages broadcast | **not implemented** | `chat` is logged and answered with a notice rather than relayed. P07 |
| Data packs: tags | the tag format with transitive resolution | **partial** | 758 of 758 tags in the real vanilla pack resolve with **zero problems**; nesting, cycles (reported, not recursed into) and a depth bound are tested. **Gap**: `.zip` packs are not read, the world's enabled-pack list is not read, and registry-relative tag paths need the measured registry table |
| Data packs: recipes (KD-25) | `data/minecraft/recipe/` | **partial (7 of 21 types)** | 1 421 recipes load from the real pack and 94 files across 14 unmodelled types are **counted, not dropped**. **Gap**: `crafting_transmute`, `crafting_dye`, `crafting_imbue` (new in 26.1), smithing, and the `crafting_special_*` behaviours are not modelled. The container layer does not yet consume the loaded recipes — P06's hand-written table is still what `Furnace::tick` sees |
| Data packs: functions (KD-34) | `.mcfunction` files, `/function` | **partial** | Files are discovered by extension, named by path (`function/foo/bar.mcfunction` is `minecraft:foo/bar`), and each line runs as the invoker through the dispatcher, so a function cannot reach a command the invoker could not. Recursion bounded at depth 16; command count bounded at 10 000 across a chain; a malformed file is skipped without removing the others. **Gaps**: function **tags** (`#namespace:tag`) refused by name; **macros** (`$(name)`) refused rather than expanded; `/schedule` not modelled. The jar ships **zero** `.mcfunction` files, so nothing here is verified against vanilla data |
| Data packs: loot/advancements | the remaining data directories | **not implemented (loaded only)** | Loot and advancement *loading* exists (`mc-data::loot`, `mc-data::advancement`) with the real pack's census asserted; neither is wired to gameplay. P07-10/P07-11 |
| Registries/tags/data | data-driven loading | **partial (blocks + items complete)** | `mc-registry` loads all 1 168 blocks / 29 873 states / 1 506 items from the jar-derived table; `Registries::vanilla()` is the loader (data packs load via `mc-data` since P07) |

## 5. World generation

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| Terrain and biomes (KD-28) | vanilla octave/amplitude noise tables, multi-noise biome parameters | **partial (baseline)** | A seed pipeline, documented Perlin noise (**not** Vanilla's octave/amplitude tables), six biomes with biome-driven surface blocks, oak trees, and an existing-world-first provider; stored chunks are read before generation is ever consulted. **Not implemented**: ores, caves, ravines, lakes, and every biome beyond the six |
| Structures (KD-29) | `RandomSpreadStructurePlacement` over per-structure `StructureSet` JSON; all 1 202 templates | **partial (single-chunk subset)** | 1 182 of the jar's 1 202 templates load, and generated chunks are decorated from a deterministic per-chunk selection — 14 of 3 600 sampled chunks, 12 362 blocks, zero refusals. **Only the single-chunk subset (1 028 of 1 182) can generate**, so ancient cities, mansions and bastions never appear; the `SingleChunk` policy refuses them. 20 shipwrecks are refused at load because they use 8 alternative `palettes`, so **no shipwreck generates**. The spacing, separation and chance constants are approximations, not Vanilla values, and structure entity NBT is counted rather than spawned |
| Caves, ores and features (KD-30) | ores, caves, ravines, lakes | **not implemented** | Tracked; the noise is a documented Perlin implementation, not Vanilla's tables |

## 6. Performance and operations

| Domain | Vanilla 26.1.2 behavior | Our status | Evidence |
|---|---|---|---|
| 20 TPS (Pi 5, 10 players) (KD-35) | stable 20 TPS | **met for the scripted workload (2026-09-12)** | Pi 5 Model B, Debian 13 trixie, release profile built on-device: 30-min soak, 10 scripted clients view 8 — settled MSPT p50/p95/p99 medians **0.21/0.27/0.29 ms**, zero overruns in 60 settled windows (join burst: worst tick 105 ms, 5 overruns, never repeated), CPU median 1 % of one core, RSS ≤ 125 MB, autosaves clean on microSD. Boundaries: scripted clients (KD-38), loopback traffic, microSD (not the NVMe target). Full record: [BENCHMARK-BASELINE.md §P09-Pi](../performance/BENCHMARK-BASELINE.md) |
| systemd unit (KD-36) | applied on a real host | **met** | Unit installed per the runbook on the Pi 5, enabled, soak run under it, graceful stop verified on hardware. First application exposed the registry-fixture deployment defect (fixed; see the benchmark record) |
| Backup/restore CLI (KD-37) | operator tooling | **gap** | Library calls with 5 tests (`backup.rs`); no CLI wrapper |
| Real-client acceptance (KD-38) | any Java client 26.1.x | **exercised — rendering unverified** | A vanilla **26.1.2** client now completes handshake, login, configuration and **play** against this server with **no protocol-error report at all**, and is live rather than merely connected: it answered the server's keepalive (serverbound play id 28), sent 410 per-tick reports and accepted 17 `set_time` packets over the observation window. **The client's own log now says it is drawing the world**: `Chunk Sections UBO` resize events show it uploading chunk geometry to the GPU, which it only does for chunks it renders, and `[CHAT] Welcome to the Rust Minecraft server.` shows our message reached its screen. Its only ERROR lines (3) are `InvalidCredentialsException: Status: 401` from the offline profile, which is expected and unrelated. **What that still does not establish is whether the lighting *looks* right** — light is not a field a client validates, so a wrong level produces no error, no warning and no log line. That needs a person looking: `tools/visual-check/run.py` starts the server and client and leaves them running for exactly that. KD-38 stays open until one is |

## P00\u2013P09 review (2026-09-13)

A targeted review of the phases already marked complete, aimed at one failure mode observed four times: **a test
and the code it checks written from the same understanding**, so auditing either only confirms the other.

| Item | Requirement | Status | Evidence |
|---|---|---|---|
| Review method (KD-59) | find tests that pass for a reason other than the property they name | **established** | Reading tests finds only the crude form, which **does not exist here**: a scan of ~1200 tests for bodies naming no assertion, no `expect`, no `unwrap`, no `panic!` and no asserting helper returns nothing vacuous. The form that matters is an assertion **satisfiable for one input**, and it is found by **perturbing an input the suite holds fixed**. Two of the review's findings arrived that way by accident (KD-49 from moving the spawn, KD-52 from correcting a palette); deliberate perturbation found a third on its first attempt (KD-60). Kept at `tools/review/scan_vacuous_tests.py` |
| Fixture provenance (KD-57) | every fixture's source known | **done** | All ten classified. `anvil/*` is a real server with a manifest naming the jar's sha1, the seed and a sha256 per file. Four registry tables are jar-derived; `items.tsv` was the one that only *claimed* a source and is now independently reproduced by `ItemProbe` — **1506 rows, zero differences**. Two protocol fixtures are hand-written: `handshake_login.hex` is verified against a real client's captured handshake, and `nbt_literal_text.hex` was verified against real network NBT from a vanilla server told to `say` (KD-58) |
| Names versus numbers (KD-56, KD-65, KD-69) | a name or doc about a numeric default says what the code does | **was wrong twice, both sent to clients** | `default_state` returned a block's **lowest** state id and its doc called it the default — wrong for **642 of 1168** blocks, so every log lay on its side and every leaf held water. `PLAINS_BIOME_ID = 0` while the client's registry gives id 0 to **`minecraft:badlands`** — every chunk painted red sand and orange terracotta, wherever the player stood, with terrain, blocks and light all correct. And `BlockStateRef::is_default` answered "has no properties", called from nowhere, a trap for the next reader |
| Registry ids on the wire (KD-66) | every numeric id sent to a client is checked against the registry it indexes | **rule: an id with a check is right, an id without one is wrong** | Five ids, and the split is exact. **Wrong:** block state (a jar table whose *rows* were verified and whose *default column did not exist*), biome (a constant named for one biome and valued for another). **Right:** item (independent extraction, row for row), dimension type (`0`, and `registry_data/mod.rs:311` asserts it against the overworld), block entity type (golden bytes captured from a real server). Where no evidence exists, the row belongs here as **unverified** — not as a number somebody believed |
| Biome id (KD-65) | names the biome it claims | **full** | `crates/network/src/registry_data/config-payload.bin` is the exact byte sequence the client receives: the identifier run after `minecraft:worldgen/biome` is **65 names in alphabetical order** ending at `minecraft:chat_type`, and `minecraft:plains` is the 41st. Pinned by `crates/server/tests/registry_ids.rs`, which is **verified to fail** when the constant is set back to 0 |
| Dimension type id | `join_game`'s `0` is the overworld | **full** | Asserted in `registry_data/mod.rs:311` and re-checked from the payload by `registry_ids.rs`; entry 0 is `minecraft:overworld`, the registry is four long, `minecraft:damage_type` begins after it |
| Item ids | match the client's registry | **full** | `items.tsv` against `ItemProbe`'s jar extraction: 1506 of 1506 |
| Per-column biomes | a chunk carries each column's biome | **gap** | Every cell is plains (KD-65). `Biome::index()` is this crate's own six-biome slot, a **different numbering** from the client's registry, so sending it would be a new defect rather than a fix |
| Doc comments versus code (KD-63, KD-70) | prose describes what the code does | **one overstatement, one wrong number** | `ItemStack::is_valid`'s doc claimed the whole of `new`'s contract and checks two of three invariants — benign, because construction covers the third, and now **pinned by a test** so a change to `new` fails there. `default_cooking_time`'s comment says where its numbers came from **and states its own limit**, which is the standard this review argues for; one word of it named 200 for smoking where the constant and the jar say 100 |

**The rule this section exists to carry.** A number sent to a client is **a claim about a registry the client
owns**, and it needs the same evidence as any other compatibility claim: a jar extraction, a capture, or an
assertion that names the registry. Both ids here that had none were wrong, and neither errored anywhere — one
produced a world of sideways waterlogged logs and the other a world of red sand.

## P00\u2013P09 review: what is covered and what is not (2026-09-13)

The findings are in the table above. **This says where the review stopped**, because a review that reports only
what it found reads as complete when it is not — which is the failure this whole exercise has been about.

| clue | searched | read | left |
|---|---|---|---|
| (1) "first" taken for "default" | every `first_state_id` use in product code (12 lines) | all of them, plus `is_default` (KD-69) and the whole `blocks.rs` neighbourhood | **done** |
| (2) fixture and expectation provenance | every file under `crates/test-support/fixtures` (10) | all ten, each classified as jar, capture, or hand-written (KD-57) | **done** |
| (3) tests that cannot fail | every test in the workspace (1257) | the crude form is **absent**; the form that matters needs **perturbation**, and four were run (KD-60, KD-61, KD-62) | crude form done; perturbation is a **method**, not a finite sweep |
| (4) prose that disagrees with code | 1052 doc lines making a falsifiable claim, 60 of them naming a number | the two highest-yield sub-lines are closed: **jar-measurable counts** (4 files: `recipe.rs` wrong, `advancement.rs` 8/8, `loot.rs` 3+3, `tag.rs` 2+2) and **doc tables versus code** (2 tables: fuel wrong, smelting exact) | **all sixty are now read** — the `entity` group, the two closed sub-lines, and the remainder in `data`, `protocol`, `redstone`, `server` and `worldgen`, which holds. The long tail **not** read is the roughly 992 claim lines naming no number, and **those have produced nothing in this review**: every clue-4 finding came from a line checkable against data — the rest are unread |

**One item on this line was chased and left open, on purpose.** `StackSizeTable::len` carries "always 165 for
the 26.1.2 vanilla table", and the module doc says the table comes from "every `stacksTo` call site in the game".
Five rounds produced `tools/vanilla-probe/StacksToProbe.java` — which compiles, runs, and reports **1506 items,
1506 unreadable, with `ERROR:NullPointerException:Components_not_bound_yet` on every row** — and four attempts at
the bootstrap step that would bind item components: `getDefaultMaxStackSize`, `item.components()`,
`new ItemStack(item).getMaxStackSize()`, and `MappedRegistry.freeze()`, the last directed by scanning the jar for
the class carrying the error message.

**None of them bound the components**, and the chase stopped there. **`165` is unverified**, with the attempts on
record rather than a belief in its place, and the probe is committed in the state where it states its own limit
instead of substituting 64 and looking finished. What would settle it is the datapack and registry load a
dedicated server performs, reachable with the launcher already at `target/vanilla-26.1.2/`.

**This is recorded as a stopped side quest rather than an open one**, because it is not one of the four clues: it
is a tool for checking one of sixty doc lines, and keeping it alive because it had been alive for a while is the
shape of the problem this review exists to find.

**What "not read" means here, concretely.** Nothing in the unread remainder is *suspected* — the two sub-lines that
produced every clue-4 finding are the ones that name a value checkable from data, and the KD-73 lesson is that a
claim is only worth checking when there is something independent to check it against. The unread lines are prose
about control flow and invariants, which is the majority and has produced nothing.

**And the honest qualification on clue 3.** Four perturbations were run and two found defects. There is no
measure of how many inputs the suite holds fixed, so "perturbation complete" is not a state this review can
claim; what it can claim is that the method is written down, was productive twice, and returns nothing on a third
input — which is what a method with a hit rate looks like, not what a finished search looks like.

## P00—P09 review: closed

**The four clues have been executed and their productive areas closed.** Clue 1 reviewed every
`first_state_id` use and the whole `blocks.rs` neighbourhood; clue 2 classified all ten fixtures and
independently reproduced the one table that had only claimed a source; clue 3 established that the crude form
of a vacuous test **does not exist here**, and that the form which does **requires perturbation** — which
found two defects and closed both loops; clue 4 read all sixty doc lines naming a number, plus the two
sub-lines where the findings turned out to live.

**What the review produced:** findings with fixes from KD-63 to KD-87, of which three were real product
defects — a block default taken to be a lowest id (**every log on its side and water inside every
leaf**), a biome id naming `badlands` (**every chunk of every world painted red sand**), and a campfire
cooking time of 100 where the jar says 600. **Three regression tests, each verified to fail** when its defect
is put back, and the recording discipline itself: 237 literal escape sequences removed from the documents
this review wrote, and a commit guard that had been checking four gates of six.

**Left open, stated rather than implied:** about 992 doc lines that make a claim and name no number, which
have produced nothing; `StackSizeTable::len`'s 165 and `mob.rs`'s 16.0, both unverified with the attempts and
the route out on record; and the stack-size table's rows, which have still never been compared with the jar.

**The one sentence this review would keep:** a number sent to a client, or written in a comment, is **a claim
about something outside the file it sits in**, and it needs the same evidence as any other compatibility
claim. Every defect found here was a number that had none, and every one of them passed a green test suite.

## Two kinds of registry a client owns (P10-06)

The rule above says a number sent to a client needs a jar extraction, a capture, or an assertion naming the
registry. **Those are not interchangeable, and which one applies depends on who owns the registry:**

| kind | examples | where the client gets it | the instrument that works |
|—-|—-|—-|—-|
| **datapack registry** | `worldgen/biome`, `dimension_type` | **we send it**, verbatim, in `registry_data` | read it back out of the payload we send (`crates/server/tests/registry_ids.rs`) |
| **built-in registry** | block, item, **entity type**, menu | **compiled into the client jar** | jar extraction, with the extraction named in an assertion (`blocks.tsv`, `items.tsv`, `entity_types.tsv`) |

**This was implicit until P10-06 and is written down now** because the two look identical from inside the server
and are not: searching the config payload for `minecraft:entity_type` finds a hit, but it is a **tag** directory in
the `update_tags` packet, and an assertion built on it would have compared our numbers with nothing at all.

## Phase 10 review (2026-09-14)

**Client compatibility and rendering.** The phase's own rule, established three times over and worth stating once
here: **a number sent to a client is a claim about a registry the client owns, and which instrument settles it
depends on who owns the registry.**

| registry | who owns it | the instrument that works | where it is used |
|---|---|---|---|
| **datapack** — biome, dimension type | **we send it**, verbatim, in `registry_data` | read it back out of the payload we send | `crates/server/tests/registry_ids.rs` |
| **built-in** — block, item, entity type, block entity type, menu | **compiled into the client jar** | jar extraction, with the extraction named in an assertion | `blocks.tsv`, `items.tsv`, `entity_types.tsv` |

| Item | Requirement | Status | Evidence |
|---|---|---|---|
| Entity type ids (P10-06) | the id `add_entity` carries is the one the client's registry gives | **full** | `EntityTypeProbe` -> `entity_types.tsv`, **157 rows**, `minecraft:item` = **71**, `player` = **155**, id 0 = `acacia_boat`. **Alphabetical, like the biome registry** \— the assumption "the first entry is the ordinary one" is wrong in both |
| `add_entity` / `remove_entities` / `set_entity_motion` | codecs, checked against real bytes | **full** | **55 / 12 / 2942** captured bodies; every one is a length the field widths imply. `crates/protocol/tests/{add_entity,remove_entities,set_entity_motion}_golden.rs` |
| Entity identity | stable across a replay | **full** | `entity_uuid(seed, id)`, derived rather than random, because a UUID per spawn breaks "same inputs, same state". Five tests, one of which varies an argument alone \— a derivation that ignored the seed passes the other four |
| A drop is visible (P10-06, P10-08) | spawn, contents, removal | **full** | `add_entity` then `set_entity_data` **index 8, serializer 7**; `a_dropped_item_is_announced_with_the_item_type_id` asserts exactly one `ADD_ENTITY`; the item-stack value is **byte-identical** to captured stacks, whose counts and ids match the commands that made them and `items.tsv` besides |
| Relative movement (P10-08) | the three move packets | **wired** | layout measured across **11,713** bodies; `crates/protocol/tests/move_entity_golden.rs`. The call site landed after this row was written (`79bd9b0`): every tick whose 1/4096-scaled delta is non-zero broadcasts `MoveEntityPos` from the Entities phase (game.rs), and `a_drop_that_falls_is_announced_as_a_relative_move` pins it |
| Chat relay (P10-10) | a player's message reaches everyone, attributed | **partial** | `DisguisedChat` byte-identical to a captured payload (three independent sessions), broadcast wired, `a_players_chat_is_relayed_as_disguised_chat`. **Not asserted: that every client received it** \— the harness joins one connection, and the test says so |
| System and feedback routing (P10-10) | system messages to the acting client | **closed** | A capture settled that a 26.1.2 server answers a console `say` with `disguised_chat` and **zero** `system_chat`; all seven send sites now answer that way, each attributed, **zero `SystemChat` sends remain** — proven by the suite, which refused the source-only change with 31 failures until the assertions moved with it |
| `chat_type` (P10-10) | the decoration registry id | **full (datapack instrument)** | `minecraft:chat_type` is a registry this server sends, so the id was resolved **out of our own payload** (the entry named `minecraft:chat`), not out of a jar — `CHAT_TYPE_CHAT` with `the_chat_type_this_server_sends_is_the_one_named_chat` in `registry_ids.rs`. A real capture carrying `5` only proved the number is not free; the payload is what settled it |
| Block entities to the client (P10-09) | when does a real server name a block entity to the client? | **trigger settled by capture** | Three sessions of a silent chest were the wrong experiment; the right one (tools/chat-capture/be_experiment.py) captured **4** real packets: sign placement, sign text edit, campfire item change, spawner with SpawnData \— and **zero** for a chest placed and filled the same way, with `block_event` (7) also silent. The rule: the packet rides the block entity's client-visible NBT \(what the client needs for rendering\), not placement-by-itself and not container contents \(the menu channel's job\). Goldens: block_entity_data_golden.rs over three committed fixtures |
| Per-type metadata tables (P10-07) | the index tables the client uses | **gap, with two analysed paths** | `MetadataProbe` walks the classes and **refuses its own output**: 8 distinct indices across 157 types is one inherited set repeated, because `defineId` runs in the **instance** method `defineSynchedData`, not a static initialiser. Two paths out, both named: (a) a probe that builds each entity to ask it \u2014 which needs a `Level`; (b) a **summon-capture session** \u2014 `execute at @a run summon <type>` for all 157 types through the rig, which yields the table from the server's own sends. Path (b) is preferred and has a decode prerequisite: `MetadataValue::decode` currently refuses the serializer types mob metadata uses (boolean, varlong, string, \u2026), so the table extraction needs those variants first \u2014 "adding one is a new variant plus its `type_id` arm". What is already settled and not to be re-derived: a dropped item's stack is index **8**, serializer **7**, verified three ways (injections, captured bytes, `items.tsv`) |
| Real-client acceptance (P10-11) | a real client in a real world | **owner-verified session** | The owner played on the live client and reported: **colours correct, lighting correct (including the dead-black fix), chat delivered**; survival digging reaches the server and breaks blocks, with the mining-progress animation and drops recorded as P11 scope (no mining-time model; loot not yet wired). The same sessions drove three real fixes: the bare-string component form, the 1-based chat_type wire id, and the outbound-capacity join-burst overflow |
| `client_tick_end` (P10-11) | a client's per-tick packet | **named, not modelled** | A real client sends serverbound play **13** every tick; this server called it `unmodelled` ~**14 times a second**. Found in one session's log and by nothing the suite runs, **because the suite drives our client, which sends what this server expects** |

**What a real client said that no test could.** The `client_tick_end` line is the phase's clearest demonstration of
its own thesis from the other side: every other finding here came from comparing our numbers with the client's
registry, and this one came from **watching what a client actually sends** \— which is a different question from
what it accepts. **The suite cannot ask it**, because its client is ours.

**Left open, stated rather than implied**: the metadata extraction for the types this server does not yet send;
`block_entity_data`'s trigger; the move call site; system-message routing and `chat_type`; and **a real client's
rendering**, which is the one item on this list that no amount of server-side evidence can settle.

## P10 cross-audit -- first pass (2026-09-14)

The five scanners under `tools/review/` were run against everything P10 has delivered. Four found nothing P10
introduced, one produced a candidate, and **the candidate was refuted in the same round** -- which is the outcome
worth writing down, because the refutation is more informative than the suspicion was.

### The candidate, and why it was wrong

`scan_registry_ids` prints what follows each registry key in the payload this server sends:

```text
"minecraft:dimension_type" at offset 4717
  0  minecraft:overworld   <-- entry 0, which join_game sends
  1  minecraft:overworld_caves
  2  minecraft:the_end
```

`sender/game.rs` sends `dimension_type_id: 0`, entry 0 is `minecraft:overworld`, and **a search of
`registry_ids.rs` for `identifiers_after` found only the `chat_type` call** -- so this looked like the next
unchecked number, and exactly the shape of `PLAINS_BIOME_ID = 0`.

**It is checked.** `registry_ids.rs:103` holds `the_dimension_type_id_this_server_sends_is_the_overworld`, and
`e2e_login_play.rs:104` carries the sentence *"Entry 0 is the contract, not the count: `join_game` references
`dimension_type`"* -- two checks, one of them older than this phase.

**And the search that missed them is the lesson.** `identifiers_after` is how the **chat type** check reaches the
payload; the dimension check reaches it another way, so a grep for one call shape reported an absence that was not
there. **That is the fourth time in this phase that a measurement of a measurement has been wrong** -- the metadata
cross-reference that reported "no metadata" for the one serializer it could not read, the probe that reported
`unreadable=0` while reading eight inherited slots, and the coordinate hypothesis that three sessions were spent
eliminating. **The pattern is consistent enough to name: when a tool reports an absence, the first question is what
shape it was looking for.**

### What the scanners found besides

* **`scan_vacuous_tests`: two tests whose bodies name no way to fail** -- `every_tag_type_round_trips_on_disk`
  (`crates/nbt/src/write.rs`) and `the_whole_pipeline_runs_against_the_real_pack` (`scenario_vanilla.rs`). **Both
  predate P10**, and the second is named for the strongest property in the repository, so both are worth a look --
  recorded rather than fixed, because neither is this phase's.
* **`scan_doc_claims`: 1073 falsifiable claims in product-code docs**, 230 using "only", 149 "never". **A
  population, not a defect** -- which is why the audit reads them rather than counting them.
* **`scan_identifier_literals`: 910 literal identifiers across 13 crates.**
* **`scan_doc_equalities`** wrote its claims to `target/equality_claims.txt`.

**The scanner output is not the audit** -- each line is a place to look, and this pass is the argument: the one
line that looked most like a defect was the one that was already covered.


### The cross-audit, second pass — clue four, and the tool that reads it

Clue four is **a doc comment that disagrees with its code**, and the instrument is `tools/review/scan_doc_equalities.py`,
which reports lines in product code that claim two things are equal. It produced **172 lines**, nine of them in files
P10 touched.

**The first one read is not a claim at all:**

`	ext
game.rs:1281
/// them: an empty tick list and a missing scheduler look identical from the
/// outside, so the absence is stated rather than stubbed with a placeholder
`

That is prose using the word identical about two absences, in a function whose doc is *about* being a documented
no-op — **a good comment, matched by a pattern that could not tell it from an equality.**

**This is the fifth time in this phase that an instrument's shape has been the thing under examination**, and the
list is now long enough to be a finding in its own right:

| instrument | what it reported | what was actually true |
|—-|—-|—-|
| the metadata cross-reference | no metadata for four item entities | its width table could not read the one serializer an item must use |
| `MetadataProbe` | `unreadable=0` across 1256 rows | it was reading eight inherited slots, and now refuses its own output |
| the coordinate injections | nothing in view | it was looking at the origin while the client was elsewhere |
| `identifiers_after` in a grep | `dimension_type` unchecked | the check exists, reached by another call shape |
| `scan_doc_equalities` | 172 equality claims | at least the first is a sentence containing the word |

**An audit that trusts its tools reports the tools' shape rather than the code's.** The answer is not to stop using
them — the biome id would have been caught three phases earlier by any of these — but to read what they return
before believing what they say, **which is the same rule this phase applies to the numbers it sends.**


## P10 cross-audit -- third pass (2026-09-14)

Clue one (state-name-to-input: *is the first value correct, or merely first?*)
walked every send site P10 introduced:

* `add_entity`'s type id is **resolved, not typed**: `registries.entities.id(ITEM)`, with the registry table
  carrying the probe's count (157), the alphabetical anchor (id 0 is a boat), a name/id round trip and the
  refusal of gapped tables (`crates/registry/src/entities.rs`).
* `chat_type` is resolved out of the payload (last round); `dimension_type` and biome were settled earlier.
* the item stack in metadata (index 8, serializer 7) was already verified three ways; nothing new superseded it.
* what the walk found instead: **two stale matrix rows and a stale code comment** — the review was written
  before the last two P10 commits landed, so "system routing: gap" and "move: unwired" described a tree that
  had already moved. Both are fixed here; the stale `game.rs` comment claiming `chat_type` was unverified is
  rewritten, which is the same doc-rot class the P00–P09 review caught, this time pointing backwards.

Clue two (fixture provenance) had already been half-done by the committed tests — the slime fixture pins
`type_id == 117` against `entity_types.tsv` — and this round **re-derived it against all 55 captured
`add_entity` bodies**: 52 are type 117 (slime), 2 are type 111 (`minecraft:sheep`), 1 is type 30
(`minecraft:cow`), and **all three resolve to registry rows**. Sheep and cow were *natural spawns* near the
world's spawn, not summoned, which makes the cross-check cover more than the experiment drove.

And the perturbation sweep over the round's own new tests:

* the three `block_entity_data` goldens — a fixture's type byte perturbed `09→08` fails the spawner test;
* the strengthened `every_tag_type_round_trips_on_disk` — its new byte-shape anchor (`0x0A 00 00`) perturbed
  to `0x0A 0x00 0x01` fails it. Both restored and green.

One scanner claim was **refuted this round**: `scan_vacuous_tests` flagged
`the_whole_pipeline_runs_against_the_real_pack` as "naming no way to fail", but the pipeline's
stage helpers carry the assertions (758 tags, 1 202 structures, 256 terrain columns, the gold-block marker
surviving a reopen) — the test's own body delegates to them. The instrument read the test's body and not its
callees. `every_tag_type_round_trips_on_disk` was a real finding, fixed by an independent byte-shape anchor
(root compound id, two-byte name prefix) that the perturbation round then verified.


## Removed rows (governance, 2026-09-12)

Five rows from the pre-governance matrix were deleted rather than updated:
"World generation — not implemented (explicit placeholder)", "Redstone
scheduling/power — unknown (P06)", "Commands/permissions — unknown (P07)",
"Registries/tags/data — partial (no datapack loading yet)" and "Worldgen (seed
pipeline) — unknown (P07)". Each was a stale Phase-07-era placeholder that
contradicted the real rows above, and contradicting rows in one table are how
this project's audits kept finding drift. The full pre-governance matrix is
readable at tag `phase-09-final`.
