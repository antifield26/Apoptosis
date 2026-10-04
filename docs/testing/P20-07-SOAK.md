# P20-07 — Soak / §13 record (Pi 5)

Date: 2026-10-04 (UTC) · tree `4d461eb` (P20 remainder `ce2d3e5` + the
fluid/random metrics-row observability) · Pi over RJ45 `169.254.77.10`.

## §13 identity

| Field | Value |
|---|---|
| Hardware | Raspberry Pi 5 Model B Rev 1.0, 4 × Cortex-A76, 8 GB |
| OS / kernel | Debian GNU/Linux 13 (trixie), aarch64; kernel 6.18.50+rpt-rpi-2712 |
| Toolchain | rustc 1.98.1 / cargo 1.98.1 (rustup on device) |
| Commit | `4d461eb` (soak binary; `ce2d3e5` plus the metrics row) |
| Build profile | `release`, `--locked`, built **on the Pi** (incremental rebuild 2m25s over warm deps; binary SHA-256 prefix `c6ffcca94b94bae7`) |
| Storage | **microSD** `/dev/mmcblk0p2` (58 G, 24 G free) — still not NVMe (P22-04 boundary) |
| Network | loopback clients (127.0.0.2…11) to a dedicated soak instance `127.0.0.1:25566` (production `/srv` unit left untouched) |

## Workload (P20-07 must-exercise)

| Path | How this soak drives it | Result |
|---|---|---|
| Flowing water | lava-cast contact + waterline spread at setup, tank outflow east, dam-breach trickle at t+900 s | conversions fire, flows settle; fluid phase ≈ 0.001 ms throughout |
| Random-tick growth | 144 wheat from age 0, 10 oak saplings, natural canopy repairs, livestock pens, natural spawns to ~95 entities | random phase ~5.4 ms mean sustained (see estimate verdict) |
| Livestock timers/eggs | 12 penned head (4 cows/sheep/pigs/chickens) + natural spawns | chickens lay (timers elapse in-soak); no mass die-off |
| Thunder/lightning | `/weather thunder 18000` second half | storm active full half; ~4 strikes expected at 1/100 000/chunk; none individually logged (no strike log line exists) |
| Saves | 5-minute autosaves + clean SIGTERM final save | 7 autosaves (59→1 regions as dirt settles) + `world saved and closed`, 372 K world |
| 10 clients | `soak_client_v2.py` 127.0.0.1:25566, 1800 s, 20 Hz move + rotating `/list`, view 8, seed 20261004 | 10/10 PLAY, 1194 keepalives, 18000 teleports, 3600 commands, 2890 chunks, 0 client failures |

Farm geometry, intended vs actual: the setup script (`stdin.sh`, kept with the
operator notes, not committed) omitted the reservoir's east wall and two
north-wall cells, so the tank never held water — it flowed east from
placement instead of bursting at the breach. The breach fill opened 6
hillside cells rather than a dam. The lava cast, waterline, wheat field,
pens, saplings and pens all landed as designed (56/56 setup replies green).
The minus-sign suspicion this raised was probed the same day and exonerated:
`tests/fill_negative_coords.rs` pins absolute resolution plus overlap
counting. Consequence for this record: the fluid burst portion is weaker
than designed (trickle, not flood); the sustained-growth portion is as
designed.

## Settled windows (10 players, after join burst)

| Window end (UTC) | mean | p50 | p95 | p99 | overruns (lifetime) | busiest phase |
|---|---|---|---|---|---|---|
| 04:28:06 | 8.95 | 8.48 | 13.45 | 17.73 | 39 | random_ticks |
| 04:33:07 | 8.46 | 8.48 | 9.60 | 12.48 | 41 | random_ticks |
| 04:38:07 | 8.59 | 8.69 | 9.88 | 10.32 | 41 | random_ticks |
| 04:43:07 | 8.56 | 8.69 | 9.62 | 10.13 | 41 | random_ticks |
| 04:48:07 | 8.86 | 8.79 | 9.76 | 10.35 | 41 | random_ticks |
| 04:53:07 | 8.81 | 8.74 | 9.85 | 10.34 | 41 | random_ticks |
| 04:56:07 | 8.87 | 8.76 | 9.89 | 11.08 | 41 | random_ticks |

Tightest settled band: **p50/p95/p99 ≈ 8.8/9.9/10.4 ms**, flat across the
run. Lifetime overruns **41** (38 join burst with 328.9 ms worst, all
broadcast-attributed chunk streaming; +1 rejoin storm, +2 mid-soak — see
below). RSS flat at **164 MB** all run (zero slope); CPU ~18–19 % of one
core, matching the ~18 % tick duty cycle. Entities 89–96; chunks 289.

## Overruns, each accounted

- 38 at join burst (worst 328.9 ms, broadcast chunk streaming) — same class
  as P18's 45.
- +1 at 04:26:06: the rejoin storm below (mass reconnect streaming).
- +2 mid-soak, attributed: tick 10104 (64 ms, worst phase entities 50.7 ms —
  AI burst over the ~90-mob crowd) and tick 10206 (106 ms, worst phase
  players 98 ms — multi-client chunk-crossing send burst). Isolated (100 s
  apart, never repeated in the 25 min after), system recovered instantly,
  p99 band unmoved. No cascade, no pattern.
- +1 in the final row (clients gone, shutdown-adjacent).

## Phase costs vs the ADR-0009 estimate

ADR-0009 §2 estimated, at this exact workload, RandomTicks ≤ 1.5 ms/tick,
FluidTicks ≤ 0.5 ms/tick at the cap, ≤ 2.0 combined on p99. Measured
(means — the scheduler keeps no per-phase distribution, so these read
against the p99 estimate explicitly as means): fluid ≈ **0.001 ms**
(passes with three orders of margin), random **≈ 5.4 ms and slowly rising
(~4.1 → ~5.9 across the run, likely maturing-farm mix dynamics)**.
Combined ≈ 5.5 ms vs 2.0 estimated: **the random estimate is missed by
~3.5×**. Per the ADR's own rule the miss means the early-out is wrong, not
that the estimate was low — the sweep pays a registry name resolve per
sample (~21 k samples/tick here), and raising the estimate needs a new ADR
revision. Filed as follow-up work, not as a P20-07 blocker: the standing
rule (mean ≪ 50 ms, no death spiral) holds with a 5× margin.

## Defects and findings from this soak

1. **Join-storm outbound overflow (robustness finding, filed for P22).**
   At 04:25:46 all 10 clients' per-tick outbound queues filled on the same
   tick and the server dropped every one (`outbound queue full` × 10);
   all 10 rejoined cleanly ~45 s later and the soak continued (clients
   report 0 failures). A restart with 10 players reconnecting at once
   would hit exactly this. Out of P20 scope — chunk streaming and the
   outbound budget predate it — but squarely in P22's reconnect-storm
   brief.
2. **Farm-setup script bug (mine, no server change).** The reservoir walls
   above. No operator impact beyond a weaker fluid burst; the pin above
   keeps the lesson.
3. **No strike log line.** Thunderstorms cannot be audited after the fact
   (this record argues expected-count only). One debug line would close it;
   left for the hardening pass, not smuggled into this tree.

## Verdict (standing rule)

Mean MSPT ≪ 50 ms across 41 k ticks; two attributed mid-soak overruns, no
cascade, flat p99 ≈ 10.4 ms; clean SIGTERM save. **Passed for the
scripted 10-player loopback farm workload on microSD**, with the named
boundaries: scripted clients (not a real-client walk — that is P20-08),
loopback, microSD (not NVMe — P22-04 still open), tank-burst weaker than
designed, lightning by expected-count. The random-tick estimate miss goes
to an ADR-0009 revision as the ADR itself requires. Logs: `~/p20soak/`
(server, world), `~/p20_soak_metrics.csv`, `~/p20_soak_clients.log` on the
Pi; analysis inputs retained by the operator.
