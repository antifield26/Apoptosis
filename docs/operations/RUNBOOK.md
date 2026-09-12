# Runbook — operating the Rust Minecraft server (P08-15)

Owner: engineering. Scope: the P08 operational slice — install, configure,
start/stop, observe, back up, restore, and recover. Every command below was
run against this tree; anything that was not run says so.

## 0. What this server is and is not

- Pure-Rust Minecraft Java **26.1.2** (protocol 775) dedicated server,
  10-player Vanilla Survival target, Pi 5 8 GB / Debian 13 / aarch64 first-class.
- Offline mode is the default (`online_mode = false`). Setting
  `online_mode = true` **refuses to start** — the Mojang session/encryption
  flow is a structural boundary only (P02-08), and `start_network` returns
  `Operational("online_mode is enabled but the Mojang session/encryption flow
  is not implemented yet")` instead of silently degrading to offline auth.
  There is no Online Mode validation procedure to document beyond that refusal;
  the PHASE-08 prompt's "validate Online Mode path" item is therefore met by
  the fail-fast plus its test
  (`e2e_login_play::online_mode_refuses_to_start_without_provider`), not by a
  handshake this build cannot perform.
- No 20 TPS claim: the P08-13 profile ran on a Windows dev host in a debug
  profile with a driven loop (see `docs/performance/BENCHMARK-BASELINE.md`
  §P08-13). Treat every number here as one order of magnitude.

## 1. Install (Pi, as root)

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin mc-server
mkdir -p /srv/mc-server /var/lib/mc-server /var/backups/mc-server
chown mc-server:mc-server /var/lib/mc-server /var/backups/mc-server
install -m 0755 target/release/mc-server /srv/mc-server/mc-server
install -m 0644 config.example.toml /srv/mc-server/config.toml
# then edit /srv/mc-server/config.toml (bind, world_dir, vanilla_data)
install -m 0644 deploy/mc-server.service /etc/systemd/system/mc-server.service
systemctl daemon-reload
systemctl enable --now mc-server.service
```

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
| `simulation.view_distance` | 2..=32 | `simulation.view_distance out of range (2..=32): ...` |
| `network.compression_threshold` | -1 or 0..=65536 | `network.compression_threshold must be -1 or 0..=65536: ...` |
| `network.motd` | ≤ 128 chars | `network.motd must be at most 128 characters` |
| `storage.world_dir` | non-empty | `storage.world_dir must not be empty` |
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
| `online_mode is enabled but ... not implemented yet` | `online_mode = true` | Set it back to `false` (default). Online auth has no handshake yet by design. |
| `ops.json` ignored / "permissions stopped working" | file beside the wrong dir, bad JSON, or upper-case uuid confusion | `ops.json` lives **beside** the world dir (parent), not inside it (`ops::ops_directory`); a malformed file is an error naming file + problem (startup logs it and runs with no operators rather than refusing boot); uuid matching is case-insensitive. A missing file is normal (empty list). |
| `The server is full` on join | at `max_players` | Wait for a slot, or have a listed operator with `bypassesPlayerLimit: true` join (they bypass the cap by design). |
| `Outbound queue overflow` disconnect | slow client / burst | Per-client only; the server keeps running. Rejoin; check view distance and link. |
| World fails to close cleanly / `shutdown save timed out` | hung or failing save | 30 s bounded save (`SHUTDOWN_SAVE_TIMEOUT`) already fired; dirty chunks keep their flags for retry (Audit 03) — restart and watch the next flush. If it repeats, restore from a verified backup (§4). |
| Connection drain timeout at shutdown | unresponsive peer | Expected path: 5 s drain (`DRAIN_TIMEOUT`), then abort the rest. No action unless every shutdown does this. |
| Reconnect storm / address spray | hostile or flapping client | Per-IP: max 4 concurrent, burst 8 then one token per 250 ms. Legitimate burst logins never notice; a storm is throttled per address. The IP table forgets idle entries past 4096 tracked addresses. |
| Corrupt chunk / region errors | disk or hostile file | Typed `CorruptData`, never a panic; the affected unit is refused, never silently continued. Restore the region from backup if the disk is at fault. |

## 6. Known gaps (not hidden)

- No 20 TPS / production-ready claim: no Pi 5 run exists in this environment.
- Online mode: fail-fast boundary only; enabling it is an error, not auth.
- `.zip` data packs unread; structure subset is single-chunk only; redstone not
  wired into the tick loop; loot/advancements load but never fire; furnace
  smelts from the hand-written baseline (see `PHASE-07-REPORT.md` §4).
- Backup/restore are library calls awaiting a CLI; the unit file was reviewed
  by reading, never applied to a real Pi here.
