# Reference Repositories Inventory (P00-01)

Date: 2026-09-10. Method: read-only (`Read`/`Glob`/`Grep`, subagents). No source copied.
Workspace root: `C:\Users\25371\projects\MinecraftServer`
Local clones root: `OpenSourceMinecraftServer/`

> Common limitation: all four clones were exported **without `.git/`**.
> `Test-Path .git` = False in each. Therefore **commit SHA / branch cannot be obtained**.
> Version identity below comes from in-tree metadata + code constants (file:line cited).
>
> **This is an unmet CONVENTIONS.md §5 requirement, not a satisfied one** (audit
> 2026-09-11). §5 asks for filesystem path, commit SHA, branch/tag, version,
> licence, modules and tests. Five of the seven are recorded below; SHA and
> branch/tag are not obtainable from these exports. Traceability rests on
> `path:line` citations plus the version constants those lines declare — which the
> audit re-verified, but which cannot detect silent upstream drift. Tracked as
> ADR-0001 R-01; closing it needs a re-clone with history (owner action).

## 1. Pumpkin-master

- Path: `OpenSourceMinecraftServer/Pumpkin-master`
- .git: absent → SHA/branch/tag: UNKNOWN (unavailable, not guessed)
- Workspace version: `Cargo.toml:110` `version = "0.1.0+26.2-26.45"` (Java 26.2 + Bedrock 26.45)
- Toolchain: `rust-toolchain.toml:2-4` `channel="stable"` (+rust-analyzer, rust-src); `rustfmt.toml:1` edition 2024; `Cargo.toml:111-112` edition 2024, rust-version 1.96
- License: root `LICENSE:1-2` **GPL-3.0** (`Cargo.toml:113` agrees); exceptions: `crates/pumpkin-plugin-api/Cargo.toml:5` and `crates/pumpkin-plugin-utils/Cargo.toml:5` are `MIT OR Apache-2.0`; `assets/bedrock/LICENSE-GEYSER:1-3` MIT (GeyserMC); `assets/NOTICE.md:1-39` records server-GPLv3 / plugin-API-MIT-Apache / Mojang-data-EULA-only split
- Target version: Java **26.2 / protocol 776**; Bedrock 1.26.45 / 2169; lowest net-compat 1.7.2/4.
  - `crates/pumpkin-util/src/version.rs:76-79` enum tail `V_26_1, V_26_2`; `:140-141` `V_26_1=>775, V_26_2=>776`; `:201-202` reverse map; `:278-279` Display `"26.1"/"26.2"`; `:298-309,324-331` Bedrock 2169
  - `crates/pumpkin-data/src/generated/packet.rs:3-5` `CURRENT=V_26_2, LOWEST=V_1_7_2`
  - `crates/pumpkin-world/src/lib.rs:22-24` `CURRENT_MC_VERSION="26.2"`, bedrock consts
  - `crates/pumpkin-world/src/world_info/mod.rs:14-15` DataVersion range **4435 (=1.21.9) .. 4903 (=26.2)**
  - `crates/pumpkin-world/src/chunk/format/anvil.rs:40-41` `WORLD_DATA_VERSION=4903`
- Workspace members (`Cargo.toml:1-23`): pumpkin, pumpkin-api-macros, pumpkin-auth, pumpkin-codecs, pumpkin-command, pumpkin-config, pumpkin-data, pumpkin-gametest, pumpkin-inventory, pumpkin-macros, pumpkin-nbt, pumpkin-plugin-api, pumpkin-plugin-runtime, pumpkin-plugin-utils, pumpkin-protocol, pumpkin-util, pumpkin-world, tools/pumpkin-codegen, tools/pumpkin-fuzzer (+ empty submodule dir `OpenSourceMinecraftServer/Pumpkin-master/crates/pumpkin-plugin-wit/` (external clone), see `.gitmodules:1-3`, not checked out, not in workspace)
- Relevant modules:
  - protocol/net: `crates/pumpkin-protocol/src/{java/{client,server,packet_encoder.rs,packet_decoder.rs},bedrock/,ser/,codec/,packet.rs}` + `crates/pumpkin/src/net/{java/{handshake.rs,pending.rs,status.rs,login/,play/},bedrock/,query.rs,rcon/,chunk_sender.rs,packet_limiter.rs}`
  - persistence/world/chunk: `crates/pumpkin-world/src/{world_info/{mod.rs,anvil.rs,data_files.rs},chunk/{mod.rs,palette.rs,format/{anvil.rs,linear.rs,pump.rs,mod.rs},io/{mod.rs,file_manager.rs}},chunk_system/{dag.rs,schedule.rs,chunk_holder.rs,chunk_loading.rs,worker_logic.rs},tick/,generation/,lighting/}` + runtime `OpenSourceMinecraftServer/Pumpkin-master/crates/pumpkin/src/world/` (external clone)
  - entity/simulation: `crates/pumpkin/src/{entity/{player.rs,living.rs,mob/,ai/},server/{ticker.rs,tick_rate_manager.rs,scheduler.rs}}`
- Tests/fixtures: inline unit tests (`world_info/anvil.rs:549-981`, `chunk_system/tests.rs`, `version.rs:334-345`); benches (`pumpkin-world/benches/{chunk,chunk_io,chunk_gen,chunk_gen_concurrent,noise_router}.rs`, `pumpkin-nbt/benches/nbt.rs`); fuzz targets (`pumpkin-protocol/fuzz/fuzz_targets/`, `pumpkin-nbt/fuzz/fuzz_targets/`); `assets/packet/26_1_packets.json` + `26_2_packets.json` (+1.7.2..1.21.11 series); `assets/meta_data_type/26_1+26_2`, `assets/tracked_data/26_2_tracked_data.json`, `assets/datapacks/{...,26_1,26_2}/`, `assets/tests/*.chunk|*.json`, `assets/viabackwards/data/mappings-26.2to26.1.nbt`, `mappings-26.1to1.21.11.nbt`

## 2. Paper-main

- Path: `OpenSourceMinecraftServer/Paper-main`
- .git: absent → SHA/branch: UNKNOWN. Note `settings.gradle.kts:12-30` errors without `.git`; this is an export snapshot, **not buildable as-is**
- Version: `gradle.properties:2` **`mcVersion=26.2`**, `:4-5` apiVersion 26.2, `:6` channel STABLE; consumers: `settings.gradle.kts:61-68`, `paper-server/build.gradle.kts:17` (`mache 26.2+build.1`), `:21-22`, `:164-171` (Manifest needs git hash → unavailable)
- License: `LICENSE.md:1-15` **GPLv3** inherited from Spigot/Bukkit/CraftBukkit; contributor code optionally MIT (`licenses/GPL.md`, `licenses/MIT.md`); `paper-api/LICENCE.txt:1-2`, `paper-server/LICENCE.txt:1-2` GPL-3.0 headers
- Modules: `settings.gradle.kts:32-42` → `paper-api/` (Bukkit/Paper API), `paper-server/` (runnable server: CraftBukkit impl + `patches/sources|features` + Moonrise), `paper-generator/` (codegen), `build-data/` (AT + mapping patches)
- Relevant paths:
  - protocol/net (Paper-owned, readable): `paper-api/src/main/java/io/papermc/paper/connection/`, `paper-api/src/main/java/com/destroystokyo/paper/network/`, `paper-server/src/main/java/io/papermc/paper/{connection,network}/`, `paper-server/src/main/java/com/destroystokyo/paper/network/`; vanilla `net.minecraft.network` only as `paper-server/patches/sources/net/minecraft/network/*.patch` (+ features `0004,0007,0016,0028`)
  - persistence (vanilla only as patches): `paper-server/patches/sources/net/minecraft/{nbt/,world/level/chunk/storage/,world/level/storage/}` (RegionFile, NBT, LevelStorageSource); Paper-side wrappers: `paper-server/src/main/java/org/bukkit/craftbukkit/persistence/`, `io/papermc/paper/persistence/`, `io/papermc/paper/util/MCUtil.java:298-309`, `io/papermc/paper/world/migration/WorldFolderMigration.java`
  - threading direction: `ca.spottedleaf.moonrise.common.util.TickThread.java`, `MoonriseCommon.java` (WORKER/IO pools), `FoliaGlobalRegionScheduler/FoliaAsyncScheduler/FoliaEntityScheduler` — region-parallel direction, single-tick compat shims at present
- Tests/fixtures: not inventoried in depth (Gradle test trees under paper-server/paper-api); vanilla behavior evidence lives upstream, not in this export

## 3. valence-main

- Path: `OpenSourceMinecraftServer/valence-main`
- .git: absent → SHA/branch: UNKNOWN
- Version: `Cargo.toml:105` `0.2.0-alpha.1+mc.1.20.1`; `crates/valence_protocol/src/lib.rs:83` **PROTOCOL_VERSION=763**, `:87` **MINECRAFT_VERSION="1.20.1"**; `:79-80` MAX_PACKET_SIZE 2097152. **Stale for our target** (1.20.1, no 26.x). Extractor mod already wired to 1.21.1 (`extractor/gradle.properties:6`) but protocol constants not moved — upgrade-in-progress trace
- License: root `LICENSE.txt:1` **MIT** (Copyright 2022 Ryan Johnson); `Cargo.toml:109` agrees
- Crates (`Cargo.toml:99-102` members `crates/*, tools/*`; 28 crates): protocol=`valence_protocol` (+macros, +generated), net=`valence_network`, persistence=`valence_anvil` (README: read-oriented), world=`valence_server::layer` (ChunkLayer/EntityLayer) + `valence_spatial`, entity=`valence_entity`, registry=`valence_registry`, nbt=`valence_nbt` 0.8.0, plus math/ident/text/command/inventory/advancement/boss_bar/etc.; tools: stresser, packet_inspector, playground, dump_schedule
- Relevant paths:
  - protocol: `crates/valence_protocol/src/{lib.rs:70-189,decode.rs,packets.rs,packets/{handshaking,login,status,play}/,var_int.rs,var_long.rs,velocity.rs}`
  - network: `crates/valence_network/src/{lib.rs,connect.rs:129-414,packet_io.rs,legacy_ping.rs}` (Tokio gateway + flume into ECS)
  - world/persistence: `crates/valence_anvil/src/{lib.rs,parsing.rs,bevy.rs}`, `crates/valence_server/src/{lib.rs,layer.rs,layer/chunk.rs,event_loop.rs,keepalive.rs}`, `OpenSourceMinecraftServer/valence-main/crates/valence_spatial/src/` (external clone)
  - tick: `crates/valence_server_common/src/lib.rs:18-21` DEFAULT_TPS=20, `ScheduleRunnerPlugin::run_loop(tick_period)`
- Tests/fixtures: `src/tests/*.rs` (12 files: client, layer, inventory, ...), `src/testing.rs` harness (MockClient), `benches/*.rs` (packet, anvil, var_int/long, many_players; anvil bench downloads external `sp_world_1.19.2.zip`, not vendored), `examples/` 28 files, extracted fixtures `crates/valence_{generated,entity,lang,registry}/extracted/`, `OpenSourceMinecraftServer/valence-main/tools/packet_inspector/extracted/packets.json` (valence paths are relative to its clone root), `extractor/` Fabric data-extraction mod + `extractor/README.md:23-32` upgrade procedure

## 4. Minestom-master

- Path: `OpenSourceMinecraftServer/Minestom-master`
- .git: absent → SHA/branch: UNKNOWN (build stamps `{{COMMIT}}/{{BRANCH}}` from `GITHUB_SHA/GITHUB_REF` default LOCAL, see `src/main/java-templates/.../Git.java.peb:4-16`, `build.gradle.kts:16-23`)
- Version: `gradle/libs.versions.toml:6` data `26.2-rv3` → `build.gradle.kts:28-35` derives **26.2**; generated `src/autogenerated/java/net/minestom/server/MinecraftConstants.java:11-19`: **VERSION 26.2 / PROTOCOL 776 / DATA 4903** / resource-pack 88.0 / data-pack 107.1. `gradle.properties:1-7` has no MC version (Gradle flags only)
- License: root `LICENSE:1-3` **Apache-2.0**
- Source root: `src/main/java/net/minestom/server/` (~49 subpackages) + `src/autogenerated/` + `testing/` module + `code-generators/`, `demo/`, `jmh-benchmarks/`, `jcstress-tests/`
- Relevant paths:
  - network/protocol: `network/{NetworkBuffer.java,NetworkBufferTypeImpl.java,packet/{Packet,Parser,Registry,Vanilla}.java,packet/client/{handshake,login,status,configuration,play}/,packet/server/.../,player/{PlayerConnection,PlayerSocketConnection}.java,socket/Server.java,ConnectionState.java,ConnectionManager.java}` + `listener/{preplay/{Handshake,Status,Login}Listener.java,manager/}`
  - world/chunk: `instance/{Instance,InstanceContainer,InstanceManager,Chunk,DynamicChunk,Section,EntityTracker}.java`, `instance/{palette/,generator/,batch/,light/,heightmap/,block/,fluid/,gamerule/}`
  - persistence: `instance/ChunkLoader.java:16` (load/save contract) + `instance/anvil/{AnvilLoader.java,RegionFile.java}` + `instance/NoopChunkLoaderImpl.java`; **26.1 world-layout break**: new `dimensions/<ns>/<value>/region` (`AnvilLoader.java:81-85`), old root `region/` kept deprecated (`:92-105`, `@Deprecated(forRemoval=true)`, "worlds created before 26.1")
  - entity/registry/tick: `entity/{Entity,LivingEntity,Player,metadata/,attribute/,ai/,pathfinding/}`, `registry/{Registries,StaticRegistry,DynamicRegistry,VanillaRegistries}`, `thread/{TickSchedulerThread,ThreadDispatcher(Impl),TickThread,Acquirable}.java`, `timer/{Scheduler,ExecutionType}`, `event/GlobalEventHandler.java`; `ServerFlag.java:15-16` TPS=20, catch-up max 5
- Tests: `testing/src/main/java/net/minestom/testing/{Env,TestConnection,Collector}.java` reusable harness (`EnvTest` JUnit ext); `src/test/java/net/minestom/server/` — network (SocketRead/Write, PacketWriteRead, NetworkBuffer*, ProxyProtocolDecoder), world (InstanceBlock, Generator(Fork), ChunkViewer/Heightmap/FluidCount, EntityTracker, collision, snapshot), registry (VanillaRegistries, Material/Item)

## 5. Cross-reference summary

| Clone | Version / proto | License | Use in this project |
|---|---|---|---|
| Pumpkin | 26.2 / 776 (26.1 / 775 present) | GPL-3.0 | Primary structural reference: protocol states, Anvil/region impl, tick model, NBT field set. **Clean-room only, no copying** |
| Paper | 26.2 | GPLv3 (+MIT opt) | Vanilla-behavior oracle direction (patch paths), Moonrise/Folia threading direction. **Clean-room only** |
| Valence | 1.20.1 / 763 (stale) | MIT | Protocol codec style, Tokio-gateway/channel boundary, ECS-vs-tick contrast, test-harness ideas. Permissive but still record provenance |
| Minestom | 26.2 / 776 / data 4903 | Apache-2.0 | Packet registry + dual client/server connection state, ChunkLoader contract, palette/bit-packing, 26.1 layout break evidence. Permissive but still record provenance |

No repository files were copied into this project during Phase 00. All facts above are observations with file:line citations, not derivations.
