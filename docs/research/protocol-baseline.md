# Protocol / Version Baseline — Minecraft Java 26.1.2 (P00-03 / P00-04)

Date: 2026-09-10. Status: baseline for implementation. All values observed in local clones, not guessed.
Target product: **Java Edition 26.1.2, offline mode default, online-mode provider behind config**.

## 1. Version / protocol numbers (verified)

| Version | Java proto | World DataVersion | Level `version` | Source |
|---|---|---|---|---|
| 1.20.1 | 763 | — (not needed) | — | `valence-main/crates/valence_protocol/src/lib.rs:83,87` |
| 1.21.9 | 773 | 4435 (MIN) | 19132 (MIN) | `Pumpkin-master/crates/pumpkin-util/src/version.rs:138-139`; `pumpkin-world/src/world_info/mod.rs:14,17` |
| 1.21.11 | 774 | in (4435, 4903), exact unconfirmed | — | `version.rs:139`; `assets/viabackwards/data/mappings-26.1to1.21.11.nbt` exists |
| **26.1 (Tiny Takeover) = our baseline** | **775** | **4790** (resolved, section 2) | **19133** (resolved, section 2) | `version.rs:77-78,140,201,278`; jar `version.json` + vanilla `level.dat` (section 2) |
| 26.2 | 776 | 4903 (MAX) | 19133 (new) / 19132 compat | `version.rs:141,202,279`; `world_info/mod.rs:15`; `chunk/format/anvil.rs:40-41`; `Minestom.../MinecraftConstants.java:11-15` |
| Bedrock 1.26.45 (out of scope, noted) | 2169 | — | — | `pumpkin-util/src/version.rs:298-309,324-331` |

### 26.1.2 has NO independent protocol number

Evidence (negative, checked 2026-09-10):

- `Grep "26_1_2|V_26_1_2|26\.1\.2" in Pumpkin-master/crates` → no hits.
- `assets/` has `26_1_packets.json`, `26_2_packets.json`, `meta_data_type/26_1+26_2`, `datapacks/26_1,26_2` — no `26_1_2` asset.
- `version.rs` models one variant per protocol bump (`V_26_1=>775`, `V_26_2=>776`).

Conclusion: **26.1.2 reuses protocol 775 and Display `"26.1"`**, consistent with Mojang patch-without-proto-bump practice.
Implementation rule: gate on `protocol_version == 775` for 26.1.x; surface version string `"26.1.2"` only in status/MOTD/config, never as a separate protocol branch.
`LOWEST_SUPPORTED` for *network* (Pumpkin accepts back to 1.7.2/4, `packet.rs:5`) is NOT our policy: we implement **775-only** in Phase 02 (kick `outdated_client`/`outdated_server` otherwise, cf. `Pumpkin.../net/java/handshake.rs:20-36`).

## 2. RESOLVED (P03): 26.1 DataVersion = 4790, level `version` = 19133

Risk R-03 is closed with **primary evidence**: a world generated and saved by the
vanilla 26.1.2 dedicated server on this host.

Method (reproducible, 2026-09-11):

1. `server.jar` for 26.1.2 from `piston-data.mojang.com`, sha1
   `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51` (matches the version manifest).
2. Booted once with `level-seed=MC26RUSTPERSIST`, then `stop`.
3. Read the resulting files with an independent NBT/region dumper
   (`target/vanilla-26.1.2/{nbt_dump,region_dump}.py`).

| File | Field | Value |
|---|---|---|
| `world/level.dat` | `Data.DataVersion` | **4790** |
| `world/level.dat` | `Data.Version.{Id,Name,Series,Snapshot}` | 4790 / `26.1.2` / `main` / 0 |
| `world/level.dat` | `Data.version` (storage generation) | **19133** |
| every chunk | `DataVersion` | 4790 |
| `data/minecraft/*.dat` | `DataVersion` | 4790 |

Corroboration independent of the file: `DetectedVersion.createBuiltIn` in the
26.1.2 jar builds `new DataVersion(4790, "main")` (bytecode constant `sipush
4790`), and the jar's own `version.json` reports `"world_version": 4790`.

Implementation rule (per ADR-0001 D-04): the **writer stamps 4790**; the reader
accepts `[4435, 4790]` and refuses anything newer (26.2 carries 4903, which this
build must not reinterpret). Enforced by
`mc_persistence::LevelDat::is_supported_data_version` and
`WorldStorage::open`, with tests in `crates/persistence/tests/`.

Also measured in the same run and used by P03-06/07/08/10:

- region geometry: 32×32 slots, 4096-byte sectors, 8 KiB header, default
  compression id **2 (deflate)**; a real chunk measured 2.9 KiB..147 KiB
  decompressed (320 chunks in one region file, 1.36 MB);
- the chunk root compound name is `""` (empty but present), sections are ordered
  by `Y`, `yPos` is the lowest section (-4), and the section field set is
  `{Y, block_states{palette,data?}, biomes{palette,data?}, BlockLight?, SkyLight?}`;
- `level.dat` 26.1 shape: spawn moved from `SpawnX/Y/Z` + `SpawnAngle` into a
  `spawn` compound; difficulty moved from a `Difficulty` byte into
  `difficulty_settings` with a **string** value; world-gen settings, game rules,
  weather and day time moved out into
  `data/minecraft/{world_gen_settings,game_rules,weather,world_clocks}.dat`, each
  wrapped in `{DataVersion, data}`;
- a region file can end in a **partial sector** (measured: 141 sectors plus 348
  bytes) when a save is interrupted before `RegionFile.close`; vanilla reads such
  a chunk as long as the declared payload is present, and so do we (regression
  test `a_short_final_sector_is_tolerated_like_vanilla`);
- vanilla's datafixer **drops unknown entries** from `level.dat`'s `Data`
  compound (probe field `RustProbeMarker` disappeared across a vanilla
  load/save), so no compatibility claim may rest on custom level fields.

Evidence artifacts: `crates/test-support/fixtures/anvil/` (a vanilla `level.dat`
plus a derived one-chunk region file, with MANIFEST and hashes), the ignored
differential test `crates/persistence/tests/vanilla_differential.rs`, and
`docs/phases/PHASE-03-REPORT.md`.

## 3. World-layout break at 26.1 (confirmed)

Minestom `instance/anvil/AnvilLoader.java:81-96`:

- New (≥26.1): `<world>/dimensions/<namespace>/<value>/region/` (+ `level.dat` at root).
- Old (<26.1): `<world>/region/` — constructors kept `@Deprecated(forRemoval=true)`, javadoc "worlds created before 26.1".

Implementation rule: Phase 03 reader must support **both** layouts (new first, old fallback with deprecation log); writer uses new layout only.

## 4. Protocol states and lifecycle (P00-04)

Authoritative shape = Pumpkin + Minestom (five states + Transfer, post-1.20.2 with Configuration). Valence is stale (four states, no Configuration — do NOT copy its state enum).

### 4.1 States

`Pumpkin.../pumpkin-protocol/src/lib.rs:54-62`:
`HandShake, Status, Login, Transfer, Config, Play`.
`Minestom.../network/ConnectionState.java:6-27`: `HANDSHAKE, STATUS, LOGIN, CONFIGURATION, PLAY` (CONFIGURATION re-enterable from PLAY).
`Minestom.../network/packet/client/handshake/ClientHandshakePacket.java:26-43` Intent: `STATUS=1, LOGIN=2, TRANSFER=3`.
`Minestom.../network/player/PlayerConnection.java:47,60-64,213-225`: **dual client/server state** (server may be CONFIGURATION while client still PLAY during StartConfiguration window); kick packet depends on server state (`:142-149`).

### 4.2 Per-state packet families (union of `PacketVanilla.java:287-570` + Pumpkin `java/{server,client}/`)

| State | C2S | S2C | Transition out |
|---|---|---|---|
| Handshake | Handshake (`protocol,addr,port,intent`) | — | intent → Status/Login(/Transfer→Login); version gate for non-Status (Pumpkin `handshake.rs:20-36`) |
| Status | StatusRequest, PingRequest(PingResponse payload i64) | StatusResponse(JSON), PingResponse; then close | Ping → close (`Pumpkin.../status.rs:59-64`; `Minestom StatusListener.java:25-42`) |
| Login | LoginStart/Hello, EncryptionResponse/Key, PluginResponse, CookieResponse, LoginAcknowledged | EncryptionRequest/Hello, SetCompression, LoginSuccess, PluginRequest, CookieRequest, Disconnect | LoginSuccess → Config (`ConnectionManager.java:232-234`); LoginAcknowledged → Config (`LoginListener.java:214-226`); pre-1.20.2 path Login→Play direct (Pumpkin `encryption_response.rs:147-189`) — we implement Config path, keep direct-path knowledge for compat notes |
| Configuration | ClientInformation/Settings, PluginMessage, FinishConfiguration/Ack, SelectKnownPacks/KnownPacksResp, KeepAlive, Pong, ResourcePackResponse, CookieResponse, CustomClick, AcceptCodeOfConduct | FinishConfiguration, RegistryData, SelectKnownPacks/KnownPacks, FeatureFlags(`minecraft:vanilla`), PluginMessage, KeepAlive, Ping, Disconnect, ResourcePack, Transfer | server: brand → SelectKnownPacks(`minecraft:core`) → AsyncPlayerConfigurationEvent → UpdateEnabledFeatures → registryData → FinishConfiguration (`ConnectionManager.java:246-296`); Pumpkin mirror `config/known_packs.rs:10-12,19-102,110`. Client FinishConfiguration → Play (`LoginListener.java:228-238`) |
| Play | KeepAlive, Position/Rotation/Ground, Chat/Command, Attack/Interact, TeleportConfirm, ConfigurationAck (→Config), + ~70 others (`PacketVanilla.java:317-387`) | JoinGame/PlayLogin, ChunkData, KeepAlive, StartConfiguration (→Config), Disconnect, + entity/world sync | ConfigurationAck / StartConfiguration = Play↔Config re-entry (Pumpkin `play/configuration_acknowledged.rs:6-12`, `config_acknowledged.rs:5-19`; Minestom `nextClientState/nextServerState`, `PacketVanilla.java:261-284`) |
| Transfer | (intent 3, handled as Login; `pending.rs:253-255`) | TransferPacket | Minestom `HandshakeListener.java:50-84` (TRANSFER + ACCEPT_TRANSFERS check, fallthrough); `PlayerConnection:296-299 transfer()` |

### 4.3 Login ordering (offline default)

`Minestom LoginListener.java:55-104` + Pumpkin `login/login_start.rs:86-108`, `login/encryption_response.rs:30-92,147-189`:

1. LoginStart (username validation).
2. If online-mode: send EncryptionRequest → verify nonce → decrypt shared secret → enable encryption → Mojang session check → continue. If offline/Bungee: skip to enter-config.
3. Compression: `SetCompressionPacket(threshold)` / `LoginCompressionS2c` when threshold ≥ 0 (`PlayerSocketConnection.java:211-217`; Valence `connect.rs:287-294`).
4. `LoginSuccessPacket` → state CONFIGURATION (never straight to PLAY on our path).
5. LoginAcknowledged → `createPlayer` → `executeConfig(first=true)` (`LoginListener.java:214-226`); `transitionLoginToConfig` runs compression → AsyncPlayerPreLoginEvent → drain login plugin messages → send LoginSuccess (`ConnectionManager.java:207-234`).

### 4.4 Framing / hardening inputs (feeds P02-02/03/12/15)

- `MAX_PACKET_SIZE = 2097152` (`valence_protocol/src/lib.rs:79-80`).
- Decode order `decrypt → decompress → RawPacket{id,payload}` (`pumpkin-protocol/.../packet_decoder.rs:76-201`); encode `raw → compress → encrypt` (`packet_encoder.rs:85-128,238-337`).
- `SHandShake.next_state: TryFrom<VarInt>` (`lib.rs:65-77`, `handshake/mod.rs:36-39`) — hostile VarInt path.
- `PlayerSocketConnection.java:62-74` IMMEDIATE_PROCESS_PACKETS (Handshake/Cookie/Status/Ping/KeepAlive/Login*/SelectKnownPacks/LoginAck/FinishConfig) bypass tick queue; `:144-178` others queued to tick; `:160-178` immediate processed inline. We adopt the same split (auth-critical fast path, gameplay via tick queue).

## 5. What Phase 02 must fixture first

- Handshake parse + intent mapping (1/2/3) incl. malformed VarInt.
- Status request/response JSON shape (version/players/description) + ping round-trip + close.
- Offline login → LoginSuccess → KnownPacks → RegistryData → FinishConfiguration → PlayLogin(JoinGame) happy path against real 26.1.2 client.
- Compression negotiation boundary + oversize/decompression-bomb rejection (limits TBD in P02, MAX_PACKET_SIZE adopted).
- Dual-state kick routing (LOGIN→LoginDisconnect vs PLAY→Disconnect).

All above is observation of references; 26.1.2 wire truth must be re-verified with packet capture against a real 26.1.2 client/server in Phase 02 (differential harness P00-08).
