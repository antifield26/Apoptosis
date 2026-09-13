# capture-rig

A TCP proxy that relays a real Minecraft 26.1.2 client to the server **byte for byte** while writing a
normalized JSONL trace of the conversation. It is the client-side application of the differential-testing
contract in [CONVENTIONS.md §12](../../docs/CONVENTIONS.md): a real-client session becomes an artefact you
can diff against the `TestClient` conversation, or against another client's session.

Built for **P10-01**. It exists because "a real client joined and it looked right" is not evidence; a trace
of what the client actually sent and received is.

## Use

```sh
cargo run -p mc-capture-rig --release -- \
  --listen 127.0.0.1:25566 --upstream 127.0.0.1:25565 --out trace.jsonl
```

Point the client's server address at `--listen`. `--once` serves one connection and exits, which is what a
scripted regression session wants. Traces append, so several runs accumulate in one file.

Read it back with any JSONL tool; `jq -c 'select(.kind=="packet") | {seq,dir,state,name}' trace.jsonl` is
enough to see the shape of a session.

## Trace format

One JSON object per line, tagged by `kind`.

| `kind` | When | Fields |
|---|---|---|
| `session_start` | connection accepted | `seq`, `peer`, `upstream` |
| `packet` | one frame relayed | `seq`, `dir`, `state`, `id`, `name`?, `wire_bytes`, `body_bytes`, `compressed`, `digest`, `head` |
| `observer_error` | framing failed for a direction | `seq`, `dir`, `error` |
| `session_end` | session closed | `seq`, `c2s_frames`, `s2c_frames`, `c2s_bytes`, `s2c_bytes`, `duration_ms`, `degraded` |

`dir` is `c2s` or `s2c`. `state` is `handshake`, `status`, `login`, `config` or `play` — a packet id means
different things in each, so an id is not interpretable without it.

### Why those fields, and not others

- **`digest` is taken over the uncompressed body** (id + payload), so two sessions that differ only in
  compression produce identical digests. Compare semantics, not wire incidentals — the same reason §12 gives.
  It is an MD5 fingerprint for comparison, **not** a security primitive.
- **`wire_bytes` is kept separately**, because sometimes the framed size *is* what you are investigating.
- **No timestamps per packet.** A trace is meant to be diffable between runs, and a wall-clock field would
  make every diff non-empty. `session_end` carries one duration.
- **`name` is absent, not `"unknown"`,** when the id is outside the table below. An unknown id is reported as
  unknown rather than guessed.

## Named ids

The table is written as `mc_protocol::ids` constants, so a name cannot drift from the id it labels. It is
**partial on purpose**: the 23 ids covering the join sequence and the packets this project models. Anything
else traces with `name` absent and is still comparable by `id` and `digest`.

## The observer is passive

Forwarding never depends on decoding. If framing cannot be understood for a direction, that direction
**degrades to opaque passthrough**: bytes keep flowing and the trace records `observer_error` plus the
direction in `session_end.degraded`. A capture that ends early is then visibly early rather than looking
like a short session.

A rig that breaks the session it measures is worse than no rig, so this is chosen deliberately and is
covered by a test that feeds an unframeable length prefix and requires the degradation to be traced.

## Limitations, stated rather than implied

- **Partially compressed streams are handled; partial *state* is not.** The rig must see a connection from
  its first byte. Attaching mid-session leaves the state machine in `handshake` and compression unset.
- **No payload decoding beyond framing.** The trace records bodies and digests; it does not interpret
  `level_chunk_with_light`, item stacks or entity metadata. That is deliberate — a second decoder would be a
  second thing to be wrong — but it means a trace shows *that* a packet differed, not *how*.
- **`name` covers 23 ids.** See above.
- **One connection per trace by default** unless `--listen` is left running, in which case sessions append
  with interleaved `session_start`/`session_end` markers and `seq` restarting per session.
