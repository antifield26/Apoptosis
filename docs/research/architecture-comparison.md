# Architecture / Threading / World-model Comparison (P00-05 / P00-06)

Date: 2026-09-10. Read-only study; no code copied. Citations are observation points, not derivation sources.

## 1. Tick / threading models (P00-05)

### Pumpkin (Rust, Tokio + Rayon) — PRIMARY MODEL, adopt

- Boot: `#[tokio::main]` (`pumpkin/src/main.rs:48`) + global Rayon pool (`:57-59`, warns never to block-call rayon inside tokio; chunk fetch uses rayon-pool + channel back, `:44-46`).
- Tick thread: dedicated OS thread `Server-Ticker` (`lib.rs:332-344`) running `Ticker::run` (`server/ticker.rs:20-22` enters runtime handle, `:29-50` `manager.tick(); server.tick()` per loop, `:72-78` interval from `nanoseconds_per_tick()` / ZERO when sprinting, `:88-93` `select!{sleep/cancelled}`, `:100-106` clamp `next_tick=now` on overrun — no death spiral, `:32-38,62-68` tick start/end plugin events).
- Tick rate control: `server/tick_rate_manager.rs:11-55` (`AtomicCell<f32>` rate, `AtomicI64` nanos, frozen/frozen_ticks/sprint_ticks; `set_tick_rate(rate.max(1.0))` + `CTickingState` broadcast `:82-90`; freeze/step/sprint `:92-151`) — vanilla `/tick` semantics.
- Normal vs frozen: `server/mod.rs:1083-1129` — frozen still runs `flush_block_updates/flush_synced_block_events + players.par_iter().tick`; normal adds `task_scheduler.tick + scheduled_functions.tick + worlds.par_iter(world.tick) + player_data_storage.tick`.
- Task queues: `server/scheduler.rs:32-120` plugin tasks (min-heap on next_tick, lazy cancel) executed via `server.spawn_task` (tick thread dispatches, Tokio runs, `:200-228`); `:262-366` vanilla schedule/function queue,到期 executes datapack function from console.
- In-chunk delayed ticks: `pumpkin-world/src/tick/mod.rs:11-23` (`MAX_TICK_DELAY=256`, 7-level `TickPriority -3..+3`), `tick/scheduler.rs:11-74` (ring of 256 slots + dedup set, `step_tick` advances offset) — ordering by `(priority, sub_tick_order)` (`mod.rs:101-107`).
- World tick consumption: `pumpkin/src/world/mod.rs:1896-2041` — snapshot tick data then three parallel waves (block/fluid/random, `par_chunks(32)`), spawn list serial then `par_chunks(8)`, inhabited-time fetch-add.
- Tokio boundary: listener accept loop + per-connection tasks (`lib.rs:557,575`), RCON/Query/LAN sockets (`net/rcon/mod.rs`, `net/query.rs`, `net/lan_broadcast.rs`); CPU/crypto offload `rayon::spawn + oneshot` (`server/mod.rs:1044,1068`); sync-world-needs-async uses `block_in_place` (`world/portal/mod.rs:95-96`, `entity/player.rs:575-576`); world tick enters runtime guard so Rayon workers may `block_on` without poisoning Tokio (`world/mod.rs:1487-1590`).
- Ordering invariant: **phases serial, batches parallel** — Ticker定序 → Server::tick → per-world serial phase chain → `par_iter/par_chunks(8/16/32)` inside a phase. `OrderedTick` guarantees logical order after snapshot, not rayon execution order.

### Minestom (Java, single tick-scheduler thread + partitioned dispatcher)

- `ServerFlag.java:15-20`: TPS=20 (`minestom.tps`), catch-up max 5, `DISPATCHER_THREADS` default 1.
- `thread/TickSchedulerThread.java:12-58`: own thread, `TICK_TIME_NANOS`, overrun beyond 5 ticks resets base (drop catch-up, same anti-spiral), Windows-aware sleep.
- `ServerProcessImpl.java:273-320`: `scheduler.processTick → connection.tick → serverTick → clickCallback.tick → processTickEnd → flush → TickMonitor`; `serverTick` = serial `for(instance:tick)` then `dispatcher.updateAndAwait` parallel chunk/entity tick, leftover time `refreshThreads`.
- `thread/ThreadDispatcher(Impl).java`, `TickThread.java`, `Acquirable.java:66-130`: `Partition(Chunk) → Tickable(Entity)` ownership, latch+park workers, `lock/sync/trySync/applySync` cross-thread borrow with same-thread fast path, entity repartition rebinds owner. `ACQUIRABLE_STRICT` assertion flag.
- `timer/Scheduler.java`, `ExecutionType.java`: non-thread-safe, two execution points TICK_START/TICK_END.
- Net: NIO accept + **one virtual-thread pair per connection** (`network/socket/Server.java:75-170`), `flushSync` batched writes, idle park (`PlayerSocketConnection.java:440-456`).
- Borrow: TICK_START/END bracketing + `sync/trySync` ownership semantics + `TickMonitor` observability.

### Valence (Rust, Bevy ECS — contrast, do NOT adopt wholesale)

- `valence_server_common/src/lib.rs:18-87`: `DEFAULT_TPS=20`, `ServerSettings{tick_rate, compression_threshold=256}`, whole Bevy App driven by `ScheduleRunnerPlugin::run_loop(tick_period)`, `Last`-stage tick counter.
- `valence_server/src/event_loop.rs:16-180`: `RunEventLoop` between PreUpdate/Update; `PacketEvent` drained in rounds until empty so all same-tick packets visible; disconnect removes `Client`.
- Ordering declarative: only `PreUpdate/Update/PostUpdate/Last` + sets (`SpawnClientsSet`, `UpdateClientsSet`, `Pre/PostClientSet`); `Layer/Chunk/Entity` are Components/Resources, parallelism at system level.
- Net: self-contained Tokio runtime gateway (`valence_network/src/lib.rs:66-122`), accept/reader/writer all `tokio::spawn`, `flume + Semaphore` backpressure, `PreUpdate` spawns `ClientBundle` — **I/O→channel→tick-spawn decoupling** (adopt this boundary idea, not the ECS).
- Chunk fan-out: `layer/chunk.rs:774-800` `update_pre/post_client` ready/unready double-buffer; `loaded.rs:183-287` viewer-gated deltas (single→BlockUpdate, multi→DeltaUpdate) + `cached_init` packet cache. Adopt for packet scheduling, not persistence.

### Paper (direction only)

Single-tick compat (`TickThread.isTickThreadFor` shims) evolving toward Moonrise parallel chunk system + Folia Global/Async/Entity schedulers (`FoliaGlobalRegionScheduler`, `FoliaAsyncScheduler`, `FoliaEntityScheduler`; `MoonriseCommon` WORKER/IO pools). Lesson: keep `TickThread`-style affinity assertions + WORKER/IO pool separation in mind; region-threading is post-release work, not Phase 01-08.

### Decision for our project

Adopt **Pumpkin's shape**: one tick thread @50ms + Tokio at I/O edges + stage-serial/batch-parallel inside tick + `block_in_place`/oneshot bridges + overrun clamp + frozen-still-flush-network. Add Minestom's TICK_START/END discipline + ownership-checked cross-thread borrow + TickMonitor-style observability, and Valence's I/O→bounded-channel→tick-spawn boundary + viewer-gated chunk packet deltas.

## 2. World / chunk / persistence approaches (P00-06)

### Pumpkin — PRIMARY persistence reference (clean-room)

- Region file (`chunk/format/anvil.rs`): `REGION 32×32 (:27)`, `SECTOR 4096 (:38)`, compress `GZip=1/ZLib=2/None=3/LZ4=4` (`:43-54,121-126`), `WriteAction Pass/All/Parts` (`:79`), in-place header rewrite + sector-sort + seek-only-if-needed (`write_indices:351-434`), full `tmp→rename` (`write_all:441-481`), `update_chunk` keeps original compression, same-sector in-place else tail-64 swap/shift else downgrade All (`:605-772`); `All` noted more power-loss-safe (`:688-691`).
- Read hardening (`read:539-...`): empty→default, <8KiB→InvalidHeader, skip zero offset/count, bounds check, `length-1+type` parse, depad.
- Alt formats same `ChunkSerializer` trait (`format/mod.rs`, `LevelFileIO:442`): Linear v2 (`linear.rs`, zstd buckets + xxhash + footer signature, corrupt bucket skipped not abort) and Pump (`pump.rs`, single-NBT zstd, debug/export use). We implement **Anvil only**; note Linear's checksum idea as future hardening.
- Memory (`chunk/mod.rs:69-156`): `ChunkData{sections(RwLock palettes + random_tick cache), heightmaps(Mutex), light(Mutex+populated), status, blending, inhabited, block/fluid TickScheduler, pending entities, custom_data, dirty:AtomicBool}`; `ChunkEntityData` separate; `LightContainer Empty/Full` lazy-inflate (`format/mod.rs:810-881`).
- NBT mapping (`format/mod.rs:167-612`): tolerant read (named/unnamed, `Name+Properties` or u16 palette, String or u8 biomes, missing light→Empty), full write (`DataVersion 4903`, xPos/zPos/yPos, Status str, 3 heightmaps, per-section Y+block_states+biomes+lights, block/fluid ticks, block_entities, isLightOn, InhabitedTime, custom only-if-nonempty) + `PumpkinCustomData/BukkitValues` passthrough.
- I/O pipeline (`chunk/io/file_manager.rs`): path→`LazyLoader(OnceCell)` + per-file RwLock + watcher refcount + lock order documented (`:34-41`), read-preferring get, per-region grouping + `mpsc(1)` backpressure + rayon parse (`fetch_chunks:257-314`), snapshot-and-clear-dirty under write lock then write-if-watched (`save_chunks:321-403`), `block_await` barrier, 1s-batched unload only-if-dirty.
- Chunk lifecycle (`chunk_system/`): ticket levels (`chunk_loading.rs:91-106`, FULL=43) → level→stage (`chunk_state.rs:106-157`, 12 stages, read/write radii) → DAG (`dag.rs`) + priority heap (`schedule.rs:214-240`) + dependency-chain pinning without leaking target stage (`ensure_dependency_chain:351-463`) + public/downgrade/unpublish rules + IO-vs-generation dispatch capped by `max_in_flight` (`work:1270+`) + relight-downgrade on uniform light (`worker_logic.rs:29-80`).
- level.dat (`world_info/anvil.rs`, `data_files.rs`): gzip, `Data/DataVersion` + `Version/Id` range-checked (`check_data_version:40-57`, `check_level_version:59-75`), seed prefers `world_gen_settings.dat`, `LastPlayed=now`, read-preserves-unknown-fields + `level.dat_new→copy_old→rename` + `data/minecraft/*.dat` (game_rules with `minecraft:` prefix, world_gen with seed+dimensions json↔nbt, clocks, weather, trader, 5 stubs), 26.2 `difficulty_settings` + `spawn{dimension,pos,yaw,pitch}` with legacy fallbacks (`from/to:231-343`).

### Minestom — palette/bit-packing + loader contract reference

- `ChunkLoader.java:16-122`: load/save/instance hooks, nullable load, no-reentrancy rule, `saveChunks` parallel switch, `supportsParallel*` flags, tolerant `unloadChunk`.
- `AnvilLoader.java:58-606`: dim-path + legacy-path (see protocol-baseline §3), region cache + per-region chunk sets + 2 locks, `level.dat` gzip with `level.dat_old` copy, only `empty/full` status parsed (warn-skip others), Y-missing throws, vanilla±1 lighting sections dropped, biome unknown→PLAINS, block `Name+Properties→stateId` with air/default-skipping + `BLOCK_STATE_CACHE`, single-value `fill` (no data array), `bits=max(4,...)` packing, entities resolved via handler-or-dummy, save always ZLIB, new sectors never reuse old slots, region refcount close, parallel flags true.
- `RegionFile.java:25-233`: 1MiB cap refuse, BitSet free map, header-buffer + dirty-gated batch flush.
- `palette/Palette.java, Palettes.java`, `Section.java`, `DynamicChunk.java`, `light/`, `heightmap/`: Single/Indirect(≤16)/Direct states, SWAR count/replace, `Section{block,biome,sky,block}` record, `DynamicChunk` with `entries/tickable/CachedPacket`, incremental heightmap, double-buffered skylight. No dirty bit — persistence fully explicit (`InstanceContainer:316-383`).

### Valence — region-layer + fan-out reference (NOT a persistence loop)

- `valence_anvil/src/lib.rs`: `RegionError` incl. `Trailing/Oversized`, default Zlib, `RegionFolder` LRU-256 + Vacant negative cache + reusable compress buf, `Location{count8+offset24}`, `.mcc` external overflow (≥256 sectors) with `skip→Oversized`, first-fit allocate, header double-write + 4KiB pad. `parsing.rs` drops light/height/status/ticks (parse-only). `bevy.rs` AnvilLevel is **load-only** (no set_chunk call, no dirty, no autosave, no level.dat).
- `valence_server layer`: ChunkLayer/EntityLayer split, `LoadedChunk(viewer+sections+entities+changed+cached_init)` vs `UnloadedChunk` data-only, `PalettedContainer Single/Indirect16/Direct` with shrink, `encode_mc_format` Single=`0+VarInt+0-longs`, viewer-gated updates + init-packet cache, heightmap computed on send (MOTION_BLOCKING only), light masks empty.

### Decision for our project (Phase 03 shape)

1. Disk = Pumpkin-Anvil-compatible subset: 8KiB header, `offset<<8|count`, `len+type+data+pad`, second timestamps, same compression ids, keep-original-compression, tmp→rename, `_old` backup for level.dat. Reader accepts DataVersion ≥ MIN (4435) up to confirmed 26.1 value; writer stamps confirmed value only.
2. NBT field superset = Pumpkin `internal_from/to_bytes` lists (tolerant read, complete write); legacy difficulty/spawn fallbacks required.
3. Memory = hot `ChunkData`-like struct (palette sections + light + heightmap + tick schedulers + entities + status + AtomicBool dirty); cold/proto states deferred to Phase 05/07 (no speculative DAG scheduler in P03 — simple load-on-demand + explicit save).
4. I/O = per-file lock + refcount watch + region grouping + snapshot-clear-dirty + barrier; LRU handle cap + negative cache + reusable compress buffer (Valence) ; single-value fill + `bits=max(4,…)` + unknown-biome→plains + default-skipping (Minestom).
5. Gameplay ordering never depends on I/O completion order (worker results join at tick boundary).
