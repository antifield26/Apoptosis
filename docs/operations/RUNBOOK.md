# Runbook — operating the Rust Minecraft server (P08-15)

Owner: engineering. Scope: the P08 operational slice — install, configure,
start/stop, observe, back up, restore, and recover. Every command below was
run against this tree; anything that was not run says so.

## 0. What this server is and is not

- Pure-Rust Minecraft Java **26.1.2** (protocol 775) dedicated server,
  10-player Vanilla Survival target, Pi 5 8 GB / Debian 13 / aarch64 first-class.
- Offline mode is the default (`online_mode = false`). Setting
  `online_mode = true` runs the Mojang handshake (P19-05, ADR-0008):
  RSA-1024 `EncryptionRequest`, session-hash `hasJoined` with timeout /
  refusal vectors, AES-128/CFB8 from there, verified profile with
  properties into `LoginSuccess`. A real-account join with skin is
  owner-run NOT RUN; the automated handshake pins are green.
- No 20 TPS claim: the P08-13 profile ran on a Windows dev host in a debug
  profile with a driven loop (see `docs/performance/BENCHMARK-BASELINE.md`
  §P08-13). Treat every number here as one order of magnitude.

## 1. Install (Pi, as root)

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin mc-server
mkdir -p /srv/mc-server /var/lib/mc-server /var/backups/mc-server
chown mc-server:mc-server /var/lib/mc-server /var/backups/mc-server
install -m 0755 target/release/mc-server /srv/mc-server/mc-server
# The registry tables are read at runtime from next to the binary (P09 finding:
# the binary does not carry them compiled-in, and the service user cannot read
# the build tree). All ten are installed: the first four are required (the boot
# fails without them), the other six are optional and each one missing costs a
# documented behaviour with a WARN (AUDIT-19 G-04: installing only the first
# four left 5 WARNs, wrong default block states and flat dig rates):
mkdir -p /srv/mc-server/fixtures/registry
install -m 0644 crates/test-support/fixtures/registry/blocks.tsv \
                 crates/test-support/fixtures/registry/items.tsv \
                 crates/test-support/fixtures/registry/block_light.tsv \
                 crates/test-support/fixtures/registry/entity_types.tsv \
                 crates/test-support/fixtures/registry/block_defaults.tsv \
                 crates/test-support/fixtures/registry/block_hardness.tsv \
                 crates/test-support/fixtures/registry/block_shapes.tsv \
                 crates/test-support/fixtures/registry/block_mineable.tsv \
                 crates/test-support/fixtures/registry/block_tool_tiers.tsv \
                 crates/test-support/fixtures/registry/tool_rules.tsv \
                 /srv/mc-server/fixtures/registry/
install -m 0644 docs/operations/RUNBOOK.md /srv/mc-server/RUNBOOK.md
install -m 0644 config.example.toml /srv/mc-server/config.toml
# then edit /srv/mc-server/config.toml (bind, world_dir; see section 2 for
#      the optional [datapacks] vanilla_data path, without which the server has
#      no vanilla functions, recipes or tags)
install -m 0644 deploy/mc-server.service /etc/systemd/system/mc-server.service
systemctl daemon-reload
systemctl enable --now mc-server.service
```

The ten tables, and what each one costs when it is missing (`Registries::load`,
`mc-registry`):

| Table | Missing it means |
|---|---|
| `blocks.tsv` | **Boot fails**: no block states, so no world opens. |
| `items.tsv` | **Boot fails**: no items. |
| `block_light.tsv` | **Boot fails**: no per-state light properties. |
| `entity_types.tsv` | **Boot fails**: `add_entity` has no registry id. |
| `block_defaults.tsv` | WARN; 642 of 1 168 blocks fall back to their lowest state id — logs on their side, water inside every leaf. |
| `block_hardness.tsv` | WARN; digs fall back to a flat rate. The other mining tables are **independent** (AUDIT-19 G-05: a missing hardness table used to skip `block_shapes.tsv` as well, so collision silently changed with it). |
| `block_shapes.tsv` | WARN; partial blocks collide as full cubes. |
| `block_mineable.tsv` | WARN; the mineable tag lists are empty, so tool judgments lose their membership data. |
| `block_tool_tiers.tsv` | WARN; tier refusals (a stone pick on obsidian) are not enforced. |
| `tool_rules.tsv` | WARN; every held item digs as an empty hand. |

`biome_spawners.tsv` and `entity_metadata.tsv` sit in the same directory but
are compiled into the binary (`include_str!`), so they are not installed.

The unit file is `deploy/mc-server.service` in this repo. Its load-bearing
lines and why they are what they are:

| Line | Value | Reason |
|---|---|---|
| `KillSignal=SIGTERM` + `TimeoutStopSec=60` | 60 s | Shutdown is Running → Stopping → drain (5 s) → bounded final save (30 s) → Stopped (`lifecycle.rs`, P08-03). 60 s covers drain + save with margin; systemd's SIGKILL is the backstop. |
| `MemoryMax=6G` | 6 GB of 8 | Lets a 10-player world breathe while the OOM killer still has room before the box wedges. A number, not a measurement — no Pi RSS capture exists yet. |
| `LimitNOFILE=1024` | 1024 fds | 64 region handles + one socket per connection + stdio, with headroom. |
| No `Restart=` | deliberate | A corrupt world fails boot the same way every time; always-restart turns that into a restart loop that also defeats the save barrier. Add `Restart=on-failure` explicitly if wanted. |
| `ReadWritePaths=` | world + backups | The world, `ops.json` and backups live outside the binary dir so a binary swap cannot take them with it. `world_dir` must point at `/var/lib/mc-server/world`. |

`systemctl stop mc-server` exits 0 via the cooperative `Shutdown` path; any
other exit is a failure worth investigating, not masking (`SuccessExitStatus=0`
is therefore just the default, stated so a non-zero stop is read correctly).

## 2. Configure

Copy `config.example.toml`. Every value shown is also the built-in default;
deleting a line keeps working. Validation (`ServerConfig::validate`) rejects
bad values with `Operational` **before any socket is bound**:

| Key | Bounds | Violation symptom |
|---|---|---|
| `network.bind` | parseable `SocketAddr` | `network.bind is not a socket address: ...` |
| `network.max_players` | 1..=100 | `network.max_players out of range (1..=100): ...` |
| `network.online_mode` | `true`/`false` | none (bool): `true` needs outbound HTTPS to the session server, and a login it cannot verify is refused (§0) |
| `simulation.view_distance` | 2..=32 | `simulation.view_distance out of range (2..=32): ...` |
| `network.compression_threshold` | -1 or 0..=65536 | `network.compression_threshold must be -1 or 0..=65536: ...` |
| `network.motd` | ≤ 128 chars | `network.motd must be at most 128 characters` |
| `storage.world_dir` | non-empty | `storage.world_dir must not be empty` |
| `storage.seed` | any `i64`; unset means "no opinion" | none: a seed stored in `level.dat` always wins, so naming one cannot fork an existing world (a fresh world uses it, else 0) |
| `access.whitelist_enforced` | `true`/`false` | none (bool): read at boot by the join gate, `/whitelist on\|off` overrides it live and a restart restores this value |
| `gameplay.spawn_protection` | 0..=256 | `gameplay.spawn_protection out of range (0..=256): ...` |
| `gameplay.pvp` | `true`/`false` | none (bool); the damage gate is P20-owned |
| `gameplay.idle_timeout_minutes` | 0..=1440 | `gameplay.idle_timeout_minutes out of range (0..=1440): ...` |
| `gameplay.simulation_distance` | 2..=32 | `gameplay.simulation_distance out of range (2..=32): ...` |
| `gameplay.default_gamemode` | `"survival"`, `"creative"`, `"adventure"`, `"spectator"` (lower-case) | `invalid config TOML: ...` — another spelling fails the enum |
| `gameplay.hide_online_players` | `true`/`false` | none (bool); the status-sample wiring is P20-owned |
| `rcon.enabled` | `true`/`false` | `rcon.enabled refuses an empty rcon.password: set one or disable RCON` |
| `rcon.password` | non-empty whenever `rcon.enabled` | the message above; it crosses the wire in clear, so RCON stays on loopback unless moved deliberately |
| `rcon.bind` | parseable `SocketAddr`, checked only when `rcon.enabled` | `rcon.bind is not a socket address: ...` |
| unknown fields | — | rejected (`deny_unknown_fields`): `invalid config TOML: ...` |

Operational notes:

- `max_players` caps **players**, not sockets: the connection gate admits
  `max_players + 8` sockets (`GATE_HEADROOM_CONNECTIONS`) so status pings and
  in-flight logins keep working on a full server. The 11th concurrent login on
  a 10-slot server therefore gets past the socket and is refused **in the game
  loop** with a `play_disconnect` ("The server is full") — visible in
  `ops_e2e::a_full_server_refuses_one_more_join_but_keeps_a_bypass_operator`.
- `view_distance` default is **8**, not Vanilla's 10, until a Pi benchmark says
  otherwise. The game clamps at build time; a stored chunk is still read before
  generation is ever consulted.
- `autosave_ticks = 0` disables the timer (explicit saves only). Default 6000
  (5 min at 20 TPS) is a **product decision, not a Vanilla-verified value**
  (Audit 05).
- **`[datapacks] vanilla_data` is optional, and leaving it unset has
  consequences worth knowing before they are discovered.** Point it at the
  26.1.2 jar's extracted `data/minecraft` (either that directory or a pack root
  containing it — both resolve) to load vanilla functions, recipes, tags and
  structures. With it unset, **no** vanilla function, recipe or tag loads:
  `/function` reports unknown names and generated terrain carries no
  structures, while world packs under `<world_dir>/datapacks/` still load.
  The data is Mojang's and is not committed, so a fresh checkout is in exactly
  that state. Either accepted form works: the extracted `data/minecraft`
  directory itself, or a pack root **containing** `data/minecraft`. The Pi
  acceptance host now uses the second — verified on the device,
  `vanilla_data = "/srv/mc-server/vanilla-data"` with the pack at
  `/srv/mc-server/vanilla-data/data/minecraft` (758 tags, 1 515 recipes, 1 617
  advancements), and the journal reports `vanilla_data=true` with
  `namespaces=1`. The P09 acceptance run itself did **not** configure it — its
  world was bare generated terrain, which is fine for a performance soak and is
  recorded here so the omission is not mistaken for a regression (AUDIT-07
  finding E4).

## 3. Observe (logs and metrics)

Structured logs via `tracing`; level from `MC_LOG`, falling back to
`RUST_LOG`, default `info,mc_server=debug,mc_core=debug` (`logging.rs`, unit
has no log-output test — it asserts filter resolution only; journal output is
covered by the unit's `Documentation=` pointer, not by a test).

- Every 30 s (600 ticks) the lifecycle logs one `tick metrics` line with the
  same fields in the same order (`metrics::OperationalSnapshot::log`):
  `tick, ticks, mean_ms, p50_ms, p95_ms, p99_ms, worst_ms, overruns, players,
  entities, chunks, dirty_chunks`.
- What is **not** in that line, deliberately: CPU/RSS. Both need a platform
  API behind a new dependency or `cfg`-split import, and the 20 TPS gate
  checks wall time, TPS and MSPT without them. On the Pi, read RSS from
  `systemd-cgtop` / `journalctl` during the profile run instead.

## 4. Back up, verify, restore (P08-05)

The API is `mc-server::backup` (`backup_world`, `verify_backup`,
`restore_world`); five unit tests cover round-trip, overwrite refusal, backup
collision, missing-file report, and missing-world error. There is **no CLI
subcommand yet** — these are library calls an operator script or a future
`mc-admin` binary drives. The procedure below is therefore exact about the
semantics and honest that the last mile is unwired.

1. **Stop the server first**: `systemctl stop mc-server`. Backups are offline
   only — nothing locks the world, and a copy taken mid-flush can catch a
   region file half-written.
2. **Back up**: copy the whole world directory, then verify. `backup_world`
   refuses a destination that already holds a manifest (never merges into an
   older backup) and writes `mc-backup.json` naming every file, the source
   dir, the wall time and the server version.
3. **Verify**: `verify_backup` re-reads the manifest, checks every listed file
   exists, and decodes `level.dat` with a supported `DataVersion`. A backup
   that verifies is one the server can open.
4. **Restore**: `restore_world` refuses to overwrite a world holding files the
   backup does not know about unless `overwrite = true` — restoring over a
   newer world without saying so is exactly the data loss a backup exists to
   prevent. The manifest never leaks into the live world.

`level.dat_old` beside `level.dat` is crash recovery for one file, not a
backup: it covers neither region files nor operator error.

## 5. Failure playbook

| Symptom | Likely cause | Action |
|---|---|---|
| Refuses to start, `invalid config TOML` / `out of range` | bad `config.toml` | Fix the named key (§2); unknown fields are rejected, not ignored. |
| `Failed to verify username!` on join | bad token or refused session | Normal login refusal (P19-05): a bad verify token or a session server with no such login kicks with Vanilla's key `multiplayer.disconnect.unverified_username`, which an English client renders exactly as this line says (AUDIT-19 C19-L7). Any other language shows that language. Check the name; check `hasJoined` reachability. The server log line for the same event is `online login refused by the session server` with the reason. |
| `Authentication servers are unavailable` on join | session-server transport/timeout, or any non-404 status | Mojang unreachable, slow (10 s timeout), or answering 5xx/429 — key `multiplayer.disconnect.authservers_down` (AUDIT-19 C19-L8). Only a **404** is treated as "your session did not verify"; every other status is reported here, and the server logs `session check failed; refusing the login` with the detail. Retry; offline mode needs no session server. |
| `ops.json` ignored / "permissions stopped working" | file beside the wrong dir, bad JSON, or upper-case uuid confusion | `ops.json` lives **beside** the world dir (parent), not inside it (`ops::ops_directory`); a malformed file is an error naming file + problem (startup logs it and runs with no operators rather than refusing boot); uuid matching is case-insensitive. A missing file is normal (empty list). |
| `The server is full` on join | at `max_players` | Wait for a slot, or have a listed operator with `bypassesPlayerLimit: true` join (they bypass the cap by design). |
| `Outbound queue overflow` disconnect | slow client / burst | Per-client only; the server keeps running. Rejoin; check view distance and link. |
| World fails to close cleanly / `shutdown save timed out` | hung or failing save | 30 s bounded save (`SHUTDOWN_SAVE_TIMEOUT`) already fired; dirty chunks keep their flags for retry (Audit 03) — restart and watch the next flush. If it repeats, restore from a verified backup (§4). |
| Connection drain timeout at shutdown | unresponsive peer | Expected path: 5 s drain (`DRAIN_TIMEOUT`), then abort the rest. No action unless every shutdown does this. |
| Reconnect storm / address spray | hostile or flapping client | Per-IP: max 4 concurrent, burst 8 then one token per 250 ms. Legitimate burst logins never notice; a storm is throttled per address. The IP table forgets idle entries past 4096 tracked addresses. |
| Bad RCON password / RCON brute force | hostile client on the admin port | The budget is **per source address**, not per connection (AUDIT-19 G-08): five wrong passwords close one socket (backoff 0.2/0.4/0.8/1.6/3.2 s, ~3.0 s for the run), and 20 failures inside 300 s block the address at `accept` — reconnecting does not reset either. The log lines are `RCON login failed` (WARN, one per failure, with the address) and `RCON connection refused: too many bad logins from this address`. RCON's socket gate is the game listener's, but its reconnect refill is deliberately slower: 1 socket/s per address after the 8-token burst, against the game port's 4/s (`LISTENER_RECONNECT_REFILL_INTERVAL` vs `RECONNECT_REFILL_INTERVAL`). |
| `/tp`, `/teleport`, `/time set` refused for a normal player | these are level-2 commands | Deliberate and Vanilla's own rule (AUDIT-19 G-09): the tree grants them `PermissionLevel::Operator` (level 2), so a non-operator gets `You do not have permission …`. Give the player op (`/op <name>`, level 4) or run the command from the console/RCON. `/say`, `/list`, `/help`, `/msg`, `/me` stay at level 0 and are unaffected. |
| Corrupt chunk / region errors | disk or hostile file | Typed `CorruptData`, never a panic; the affected unit is refused, never silently continued. Restore the region from backup if the disk is at fault. |
| `whitelist.json` / `ops.json` / `banned-*.json` named in an `ERROR`, that table empty after the boot or a `reload` | the file is damaged: not the JSON array it must be (hand edit, disk corruption, or a file a pre-atomic build left truncated mid-write) | The whole file is refused, and the impact differs by table: the **whitelist** fails closed (nobody is listed — only operators get in), `ops.json` boots with no operators, and the **bans fail open for that boot** ("banning nobody"). Restore the file from a backup (§4) before opening the server, or fix the JSON and restart. One damaged **row** is *not* this case: it is skipped with a `WARN` naming the file and the row, its siblings load, and the server starts normally — but the skipped row is **not enforced**, so a skipped ban row is a ban that is not in force until the row is fixed. That includes a row whose `uuid` is not a uuid (a typo, a name pasted in, 31 hex digits — AUDIT-19 G-10): it is skipped rather than loaded as an entry that could never match, so an operator who typo'd a uuid sees `… is not a uuid — a row like this would load and then never match a player; skipped` and must fix the row. Grep the journal for `skipping a damaged` after any hand edit. |
| `/whitelist add` / `/ban` / `/op` answers with an error and the access file is unchanged | the access file, or its directory, is not writable (ownership, `ReadWritePaths=` in the unit, read-only filesystem, full disk) | The write is atomic (temp file → fsync → rename) and the in-memory change is rolled back, so the file and the list still agree and nothing is half-applied; the error names the file. Fix the permission or the space and repeat the command — no restart is needed, and a crash inside the write can no longer truncate the file. **Bans fail closed from this release on**: a write that fails leaves the previous ban file live (before it, a truncated `banned-players.json` read as "banning nobody" — the fail-open AUDIT-19 G-02/G-03 reproduced). |

## 6. Known gaps (not hidden)

- Pi 5 soak: two 30-minute 10-player runs on record
  (`docs/performance/BENCHMARK-BASELINE.md` §§P09-Pi, P14-Pi) — the second is
  the re-soak after P12 tick work, mixed real+scripted, pass with one noted
  idle spike. Soak at INFO level (the P14 run logged DEBUG: 32 MB).
- Online mode: implemented (P19-05, [ADR-0008](../adr/ADR-0008-online-auth.md)) —
  the handshake and session verify are pinned by automated vectors, the default
  stays offline, and a real Mojang-account join has not been exercised here
  (KD-01 partial). Enabling it needs outbound HTTPS to the session server.
- `.zip` data packs unread; structure subset is single-chunk only; redstone timing
  (torch delay/burn-out, update order) unmodelled; advancements load but never fire; loot fires as the
  block/mob drop authority (P11-04); furnace recipes come from the loaded pack
  once `vanilla_data` is set, else the hand-written baseline (P12-08; see the
  parity matrix's smelting row).
- Backup/restore are library calls awaiting a CLI (KD-37); the systemd unit has
  been applied on the Pi 5 — installed per §1, enabled, the soak run under it and
  the graceful stop verified on hardware (KD-36), where the first application
  exposed the registry-fixture deployment defect, since fixed.
- Access-file durability (AUDIT-19 G-02/G-03): **fixed in this release, with two
  boundaries stated rather than hidden.** `whitelist.json`, `ops.json` and both
  ban files are now written the way `level.dat` is — sibling temp file, fsync,
  one rename (`mc_persistence::save::write_atomic`) — so a crash or a full disk
  inside a write leaves the previous file live and **bans fail closed from this
  change on** (before it, a truncated `banned-players.json` silently read as
  "banning nobody" and a permanently banned player joined). The boundaries: a
  damaged *row* is skipped with a `WARN` rather than enforced (§5 says what to do
  about it), and a file damaged outside the write path — bit rot, a hand edit —
  is still reported and boots that table empty, so restore it from a backup (§4).
  A file that yields *no* readable row is an error, never an empty table.
