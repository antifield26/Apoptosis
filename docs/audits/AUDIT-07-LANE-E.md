# AUDIT-07 lane E — Raspberry Pi 5 deployment

Date: 2026-09-12. Host: `RPI5` at `169.254.77.10` (the RJ45 link), aarch64, kernel
`6.18.39+rpt-rpi-2712`. Access: `ssh antifield@…`, password auth, passwordless `sudo`.

**Lane E was recorded as `skipped(credential)` in the findings report. It is now complete**, and the
deployment claims it was blocking are verified rather than unverified. The lane did no deletions; the only
mutation was `systemctl start` / `systemctl stop`, which the brief allows.

## 1. Layout against RUNBOOK section 1 — matches

| Runbook says | Device has | Verdict |
|---|---|---|
| system user `mc-server`, nologin | `mc-server:x:999:984::/home/mc-server:/usr/sbin/nologin` | matches |
| `/srv/mc-server` | present, `root:root`, containing `mc-server` (0755), `config.toml`, `fixtures/registry/`, `logs/` | matches |
| binary `install -m 0755` | `-rwxr-xr-x`, ELF 64-bit LSB pie, ARM aarch64 | matches |
| fixtures `install -m 0644` next to the binary | `fixtures/registry/{blocks,items}.tsv`, both `0644` | matches |
| `/var/lib/mc-server` owned by `mc-server` | `mc-server:mc-server` | matches |
| `/var/backups/mc-server` owned by `mc-server` | `mc-server:mc-server` | matches |

`blocks.tsv` is **byte-identical** to the committed fixture (`5d9fb9c4…`). `items.tsv` is byte-identical
too (`b5dfeddb…`) — but reaching that conclusion required fixing a line-ending defect first; see finding
E2, which this lane is what exposed.

The service user **can** read `/srv/mc-server/fixtures/registry/blocks.tsv`, so the P09 defect the test
matrix records — the compiled-in path pointing back into an unreadable build tree — is genuinely fixed.

## 2. The unit file

`systemctl is-enabled` = **enabled**; state before the smoke = **inactive (dead)**, last exit
`status=0/SUCCESS`.

Effective settings, read back from systemd rather than from the file, all match RUNBOOK section 1's table:

| Setting | Value | Runbook |
|---|---|---|
| `KillSignal` | 15 (SIGTERM) | SIGTERM |
| `TimeoutStopUSec` | 1min | 60 s |
| `MemoryMax` | 6442450944 (6 GiB) | 6 GB of 8 |
| `LimitNOFILE` | 1024 | 1024 |
| `Restart` | no | deliberate |
| `User` / `Group` | `mc-server` | mc-server |
| `ReadWritePaths` | `/var/lib/mc-server /var/backups/mc-server` | world + backups |
| `SuccessExitStatus` | 0 | default, unmasked |

Finding E3: the file's **comments** are an older revision than the repository's.

## 3. Start → status smoke → graceful stop

| Stage | Evidence |
|---|---|
| start | `systemctl start` rc=0, `is-active` = **active**, `MainPID` 7594 |
| listener | `LISTEN 0 128 0.0.0.0:25599 users:(("mc-server",pid=7594,fd=9))` |
| **protocol** | a real Server List Ping: `protocol=775`, `version_name=26.1.2`, `max_players=10`, `motd="P09 Pi 5 soak"` |
| stop | `systemctl stop` rc=0 in **0.2 s**, `is-active` = **inactive**, port closed, `ExecMainStatus=0` |

The ping matters more than the open port: an open socket only proves a listener, whereas the status
response proves the server speaks **775** and reports the version the documentation claims.

The journal for the cycle is the exact sequence RUNBOOK section 1 describes:

```text
opened existing world world=/var/lib/mc-server/world data_version=4790
world storage ready world=/var/lib/mc-server/world level_name="world" autosave_ticks=6000
data packs loaded packs=0 functions=0 namespaces=0 skipped=0 vanilla_data=false
network listener started local_addr=0.0.0.0:25599
server running version="26.1.2" protocol=775 bind=0.0.0.0:25599 max_players=10 online_mode=false
shutdown signal received
server stopping ticks=79
network listener stopped
world saved and closed world=/var/lib/mc-server/world chunks=0
shutdown complete
```

`data_version=4790` independently reproduces the documented DataVersion from the deployed world.

## 4. The archived acceptance run — every figure reproduces

`/srv/mc-server/logs/acceptance-2026-09-12/` holds the primary record. Rather than trust the summary, the
soak figures were **recomputed from the raw numbers** in `logs/soak_metrics_lines.txt` (the 62 extracted
`tick metrics` journal lines) and `logs/soak_metrics2.csv`.

| Claim | Documented | Recomputed from the archive |
|---|---|---|
| tick-metrics lines | 62 | **62** |
| settled windows with 10 players | 60 | **60** (2 further lines carry `players=0`) |
| MSPT p50 median | 0.21 ms | **0.206 ms** |
| MSPT p95 median | 0.27 ms | **0.268 ms** |
| MSPT p99 median | 0.29 ms | **0.289 ms** |
| overruns | lifetime 5, all in the join burst | **5, constant across every window** |
| window p99 max | 29.23 ms | **29.231 ms** |
| RSS | ≤ 125 MB | **122.2 MB max** (201 samples) |
| CPU | 1.0 % of one core | **1.00 % median** |
| clients | 10/10 reached play, 0 failures | `reached_play: 10`, `failures: []` |
| keepalives / `/list` / chunks | 1 200 / 3 600 / 2 890 | **1 200 / 3 600 / 2 890** |
| duration | 1 800 s | `elapsed_s: 1802.4`; 1 770 s of 10-player windows |

Every figure matches. The archive README's "build tree was cleaned after this run" is also true:
`~/mcserver` is 4.9 MB of source with **no `target/`**.

`dirty_chunks=0` in every window, and the shutdown line reports `chunks=0` with 0-byte region files. That is
the documented design rather than a defect: generated chunks are deterministic and deliberately left
**clean**, so a world that was never built on is not written back.

## 5. Findings from this lane

**E1 — the recorded binary size contradicts its own hash (LOW, fixed).**
`BENCHMARK-BASELINE.md` recorded the Pi binary as `SHA-256 62067e04…, 4 524 624 B`. The device reports the
**same SHA-256** and **4 526 696 B**. A hash fixes the content and therefore the size, so the size was
wrong; corrected to the measured value with the reason recorded.

**E2 — three tracked files violated the declared `eol=lf` policy, and it produced a false drift report
(MEDIUM, fixed).** `.gitattributes` normalises every textual file to LF in the repository *and* the working
tree, and states why: the target is Linux while development is on Windows, so otherwise "the same commit
produces different bytes per platform — which would make `cargo fmt --check`, fixture hashes and any
byte-exact wire test platform-dependent". Three files had CRLF in the working tree against an LF index:

| File | Worktree | After fix |
|---|---|---|
| `crates/test-support/fixtures/registry/items.tsv` | 75 615 B CRLF | 74 107 B LF |
| `crates/test-support/fixtures/anvil/MANIFEST.txt` | 714 B CRLF | 704 B LF |
| `tools/docs-audit/check_encoding.py` | 2 126 B CRLF | 2 072 B LF |

`git status` was **clean** before the fix, because git normalises on comparison — so nothing looked wrong.
The cost was concrete and landed on this audit: comparing the deployed `items.tsv` with the repository copy
by SHA-256 reported a drift that did not exist. After normalisation the two hashes are **identical**
(`b5dfeddb…`), which is what proved the deployment had been correct all along.

`tools/docs-audit/check_line_endings.py` now detects the class. Its docstring states its own scope
honestly: on the Linux runner `eol=lf` checkout already yields LF, so it can only fire on a Windows
worktree — it is a local guard, not CI coverage. Verified by injecting CRLF (exit 1, names the file) and
restoring (exit 0).

**E3 — the deployed unit file is an older revision than the committed one (LOW).**
`/etc/systemd/system/mc-server.service` (`4cb47416…`) differs from `deploy/mc-server.service`
(`e95ef4bf…`) **only in comment lines**: the repository's now documents installing the registry fixtures
and mentions `vanilla_data` when editing the config, where the deployed copy does not. Every setting is
identical and the effective values match section 1, so the device is running what the repository
describes; only the on-device instructions are stale.

**E4 — the deployed server loads no data pack (LOW, documentation gap).**
The journal reports `vanilla_data=false`, and `/srv/mc-server/config.toml` has no `datapacks` key. So the
acceptance world ran with **no tags, recipes, structures or functions** — bare generated terrain. That is
acceptable for a performance soak, which is what the run was for, but RUNBOOK section 1's install
procedure never mentions the option, so an operator following it gets a server that cannot run
`/function` or place structures and has no way to know why. (The repository's newer unit-file comment does
mention `vanilla_data`; the runbook's install steps should too.)

**E5 — no backup exists on the device (LOW, observation).**
`/var/backups/mc-server/` is empty. The runbook documents a backup/restore helper, and section 1 creates
the directory, but the acceptance run took no backup. Recorded so a reader does not assume one is there.

**Corroboration, not new findings.** The journal still contains the two deployment defects the test matrix
already records: `cannot open the world … Permission denied … fixtures/registry/blocks.tsv` (the build-tree
path) and `cannot start network … Address already in use`, plus the `Failed with result 'exit-code'`
starts they caused. Their presence in the log is evidence that the recorded history is real, and both are
fixed in the running configuration.

## 6. Audit trail

Read-only except the service cycle. Commands run: `getent`, `ls -ld`, `find`, `stat`, `sha256sum`, `cat`,
`systemctl is-enabled|is-active|status|show`, `ss -ltnp`, `journalctl`, `du`, `sudo -u mc-server test -r`,
`python3 -` with a Server List Ping client on stdin, and `systemctl start|stop`. Nothing was modified on
the device: no file written, no package installed, no backup removed. The credential was used only through
a git-ignored `target/askpass.cmd` and is not in the repository.
