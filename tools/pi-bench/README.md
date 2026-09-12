# Pi benchmark harness

The producers of the numbers in `docs/performance/BENCHMARK-BASELINE.md` §P09-Pi (the Raspberry Pi 5
acceptance run). Committed for the same reason as `tools/vanilla-probe/`: the document cited them under
git-ignored `target/`, so the measurement could not be inspected by a reader (Audit 07, finding M3).

| File | Role |
|---|---|
| `soak_client_v1.py` | the first soak client. Its failure is why v2 exists: the per-IP admission cap (`MAX_CONNECTIONS_PER_IP = 4`) correctly refused 10 clients from one source address, and a 20 ms socket timeout could interrupt `sendall` mid-packet |
| `soak_client_v2.py` | the client the acceptance run used — one client per loopback address `127.0.0.2…`, with a blocking reader thread (keepalive/ping/chunk drain) separate from the writer (20 Hz movement), so a busy receive path cannot interrupt a send |
| `soak_sampler.py` | RSS and CPU% for the `mc-server` service every 10 s, as CSV. CPU% is not divided by the core count (100% = one core), matching `systemd-cgtop` |

The server-side half of the same measurement is the `pi_profile` and `tick_baseline` test suites in
`crates/server/tests/`, which run on the host and are `#[ignore]`d by default:

```sh
cargo test -p mc-server --test pi_profile --test tick_baseline -- --ignored --nocapture
```

The Pi's own SSH channel is a password login; the harness assumes the operator has already placed the
client and the built binary on the device.
