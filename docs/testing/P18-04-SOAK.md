# P18-04 — Soak / §13 record (Pi 5)

Date: 2026-09-24 (UTC) · tree `195a489` · Pi over RJ45 `169.254.77.10`.

## §13 identity

| Field | Value |
|---|---|
| Hardware | Raspberry Pi 5 Model B Rev 1.0, 4 × Cortex-A76, 8 GB |
| OS / kernel | Debian GNU/Linux 13 (trixie) 13.7, aarch64; kernel 6.18.50+rpt-rpi-2712 |
| Toolchain | rustc 1.98.1 / cargo 1.98.1 (rustup on device) |
| Commit | `195a489` (P18 components/food/commands/ores) |
| Build profile | `release`, `--locked`, built **on the Pi** (49.5 s) |
| Binary | `~/MinecraftServer/target/release/mc-server`; service copy SHA-256 prefix `9cbe19b3ed58b527` |
| Storage | **microSD** `/dev/mmcblk0p2` (58 G, 24 G free) — still not NVMe (P22-04 boundary) |
| Network | loopback clients (127.0.0.2…11) to a dedicated soak instance `127.0.0.1:25566` (production `/srv` unit left untouched; whitelist there refused Soak* names) |

## Workload (P18-04 must-exercise)

| Path | Instrument on this hardware | Result |
|---|---|---|
| Wear on dig | `p18_wear_enchant` (release, on Pi) | **6/6 green** |
| Eating | `p18_hunger` | **8/8 green** |
| Combat (Sharpness/Protection) | `p18_wear_enchant` | **green** |
| Observer clock | `mechanisms` | **4/4 green** |
| Hopper→furnace | `hopper_furnace` | **4/4 green** |
| Doors | `doors` | **6/6 green** |
| New terrain (ores/carvers) | fresh soak world generated **289 chunks** under 10 players; `ore_carver_stats` on Pi | **green** (32×32 `--ignored` subset) |
| Commands | `p18_commands` on dev host (same binary family) | 16/16 |

Live soak (10 scripted clients, 20 Hz move + rotating `/list`, view 8, 1800 s):
server `~/p18_soak_server.log`, metrics `~/p18_soak_metrics.csv`, clients
`~/p18_soak_clients.log`.

## Settled windows (10 players, after join burst)

| Window end (UTC) | mean | p50 | p95 | p99 | overruns (lifetime) | busiest phase |
|---|---|---|---|---|---|---|
| 05:38:11 | 5.58 | 5.52 | 6.31 | 7.82 | 45 | broadcast |
| 05:38:41 | 5.76 | 5.55 | 7.77 | 9.54 | 45 | entities |
| 05:39:11 | 5.59 | 5.49 | 6.76 | 7.72 | 45 | entities |
| 05:39:41 | 5.62 | 5.53 | 6.68 | 7.95 | 45 | entities |
| 05:40:11 | **3.27** | **3.10** | **5.50** | **6.59** | 45 | entities |
| **05:40:41** | **3.10** | **3.09** | **3.24** | **3.34** | **45** | entities |

Notes:
- Windows 05:38–05:39 overlapped an in-process `tick_baseline` run on the same
  Pi (CPU contention). From 05:40:11 onward the figures are clean; the
  tightest settled window (05:40:41) is **p50/p95/p99 ≈ 3.1 / 3.2 / 3.3 ms**,
  matching P14-Pi settled medians.
- **Lifetime overruns 45**, worst tick **549 ms** — join/chunk-stream burst
  (overrun lines name `broadcast`); **zero new overruns in settled windows**
  (count held at 45 across 05:38:11 → 05:40:41).
- Entities ~96–98; chunks 289; RSS **84.9 MB**; CPU **0.2–0.3 %** of one core
  (sampler, 10 s cadence).

## Defect found by this soak

**`tools/pi-bench/soak_client_v2.py` `ClientInformation` body was wrong** for
26.1.2: missing `particle_status` and encoding bools as VarInts. The server
logged `malformed protocol input: truncated VarInt` after `login accepted` and
every client dropped. Fixed in-tree to match `config.rs` `decode_body` (i8
view_distance, bool bytes, u8 skin_parts, trailing particle_status VarInt);
re-run reached 10/10 PLAY. Pinned by this soak's first (failed) vs second
(run) attempt.

## Verdict (standing rule)

Mean MSPT ≪ 50 ms; no settled-window overrun after the join burst. **Passed
for the scripted 10-player loopback workload on microSD**, with the named
boundaries: scripted clients (not a real-client walk), loopback, microSD (not
NVMe — P22-04 still open). Gameplay paths (wear/eat/combat/observer/hopper)
are proven by the on-device release tests above, not by the soak client
(which only moves and `/list`s).
