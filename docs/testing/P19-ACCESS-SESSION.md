# P19 access session — 2026-09-30 (real client)

The P19-07 owner pass, executed on the dev host against the scratch server in
`target/p19-session/` with the owner's real Minecraft **26.1.2** client. This file
was the run sheet; it now carries the outcome. Verdicts first, then the evidence
each rests on with the exact log lines, then the two defects the session raised.

## Verdicts

| Item | Asked | Verdict |
|---|---|---|
| 1 | Unlisted profile sees the whitelist refusal **on screen** | **PASS** |
| 3 | `/whitelist add <offline-name>` while that profile is offline, then it joins without a restart | **PASS** |
| 2 | Banned profile sees the ban screen **with the reason**; the ban disconnects a live session; pardon restores | **PASS** |
| 4 | Console save round trip: `save-all flush` / `save-off` / `save-on`, and the edit survives a restart | **PASS** |
| 5 | Stock RCON client runs `list` and `stop` | **PASS** |
| 6 | Online-mode join with skin | **WITHDRAWN** (no-online-mode decision, 2026-09-30) |

The session also raised **F3** (a command write into an unloaded chunk destroys the
stored chunk — fixed in P19-08, pinned) and **F4** (`setblock … keep` on an unloaded
chunk, fixed by the same line) — both found by probing past the checklist, neither
an item failure. They are recorded in full in
[P19-REVIEW.md](P19-REVIEW.md) §"Findings from the owner session"; F5 (log
severity) is open.

## Environment

- Server: `target/debug/mc-server.exe target/p19-session/config.toml` — offline mode,
  `127.0.0.1:25565`, RCON on `127.0.0.1:25575` with a session password, whitelist
  off at boot, world `target/p19-session/world` (a scratch world; the real worlds
  were never opened).
- Client: the owner's vanilla **26.1.2** install, launched from its own version
  manifest pointed straight at the server (`--quickPlayMultiplayer`), so every join
  below is a real client's login → config → play, not a scripted test client.
- Console input: the server reads stdin, so commands were appended to a command file
  and forwarded to the process (the P19-03 dispatcher).
- Clock: server log lines are UTC, client log lines are local (UTC+8).

## Item 1 — whitelist refusal is visible on screen

```text
13:47:00  INFO console: Whitelist is now enforced
13:47:16  INFO mc_network::connection: login accepted name=Main uuid=0dd70438-…
13:47:16  WARN mc_server::game::session: refused a join: not on the whitelist id=conn#1 name=Main
```

The client's own log (`[21:47:16]`):

```text
Client disconnected with reason: You are not white-listed on this server!
```

and the screen it put up (captured, then read back) shows the connection-lost panel
with exactly that line and the single "back to the server list" button —
`target/p19-session/item1-whitelist-refusal-client.png`. Refusal happens before a
slot, an entity or a player file is spent: the `13:47:26` metrics row reads
`players=0 entities=0`, and `whitelist list` still shows no entry.

## Item 3 — an offline profile can be listed, and then joins

With the refused client gone (offline), on the console:

```text
13:48:38  INFO console: Added Main to the whitelist
13:48:40  INFO console: Whitelisted (1): Main
```

`target/p19-session/whitelist.json` then holds Vanilla's shape with the
offline-derived uuid:

```json
[ { "name": "Main", "uuid": "0dd70438-3944-307b-9dcc-a7f528c8e300" } ]
```

That uuid is the one the client itself launches with (MD5 of `OfflinePlayer:Main`,
version-3 bits), so the listing admits the later join — no restart:

```text
13:49:05  INFO mc_server::game::session: player joined id=conn#2 name=Main entity_id=1 x=-8 y=66 z=-8
```

`target/p19-session/item3-whitelist-join-client.png` is that client in the world
(survival HUD, ten hearts, ten hunger, hotbar). This is the P19-07 F1 fix under
owner conditions: before it, `add` refused a name that had no live session.

## Item 2 — ban screen with the reason, live disconnect, pardon

Whitelist off, client in the world, then:

```text
13:50:15  INFO console: Banned Main
```

The live session is dropped, and the client shows **both** lines — this is why the
screen is captured and not only the log, whose one-line summary stops at the first:

```text
You are banned from this server.
Reason: griefing the spawn
```

`target/p19-session/item2-ban-live-client.png`. `banned-players.json` carries
Vanilla's six keys:

```json
[ { "created": "2026-09-30 13:50:15 +0000", "expires": "forever", "name": "Main",
    "reason": "griefing the spawn", "source": "Server", "uuid": "0dd70438-…" } ]
```

A rejoin is refused at the gate, and the same two lines come up from the join path
(`refused a join: profile is banned`, `item2-ban-rejoin-client.png`). `pardon Main`
→ `Pardoned Main`, the file becomes `[]`, and the next join is clean
(`player joined id=conn#4`).

## Item 4 — console save round trip, and the edit survives a restart

Two things had to be true: `save-off` holds writes, and `save-all flush` returns
only once the data is on disk. Both are visible in the region files, not inferred.

```text
13:52:35  INFO console: Saved the world                    # flush with nothing dirty
13:53:10  INFO console: Automatic saving is now held
13:53:12  INFO console: Set the block at -8 72 -8          # two markers placed
13:53:12  INFO console: Set the block at -8 65 -8
13:53:23  INFO console: Automatic saving is resumed
13:53:25  INFO console: Saved the world
```

| Moment | `r.-1.-1.mca` |
|---|---|
| before the edits | 0 bytes, mtime 20:41:17 (the earlier session) |
| 8 s after both edits, still held | 0 bytes, mtime 20:41:17 — **unchanged** |
| after `save-on` + `save-all flush` | **12 288 bytes**, mtime 21:53:25 |

Note the first flush at `13:52:35` wrote nothing: freshly generated chunks are not
dirty, so there is nothing to save until something edits them — the edit step is
load-bearing, not ceremony.

`/stop` then shut down gracefully (`shutdown requested by command tick=8300`,
`kicking player name=Main reason=Server shutting down`,
`world saved and closed world=target/p19-session/world chunks=0`,
`shutdown complete`), and after the restart the markers were still there — read
back with a probe that cannot confuse "occupied" with "empty" (see the deviations):

```text
13:57:52  INFO console: No change at -8 72 -8 (mode keep)
13:57:54  INFO console: No change at -14 67 -14 (mode keep)
```

The second of those is a corner of the diamond landmark built around the spawn with
four `fill`s (96 blocks) and flushed before the restart;
`target/p19-session/item4-inworld-landmark-client.png` is the restarted server's
world on the real client, with the landmark rendered — the client sees the persisted
edit, not just the server's memory of it.

## Item 5 — a stock RCON client

Third-party client (`mcrcon` 0.7.0 from PyPI — not this repository's code):

```text
$ mcrcon 127.0.0.1 -p 25575 --password p19-session    # stdin: list
> There are 1 of a maximum of 10 players online: Main

$ mcrcon 127.0.0.1 -p 25575 --password p19-session    # stdin: stop
> Stopping the server…
```

The server side of that `stop`:

```text
14:04:02  INFO mc_server::lifecycle: shutdown requested by command tick=166
14:04:02  INFO mc_server::storage: world saved and closed world=target/p19-session/world chunks=0
14:04:02  INFO mc_server: shutdown complete
```

A wrong password authenticates nothing: the client's login raises, no command runs,
and the server closes the connection. Cross-host refusal was **not** exercised —
the session config binds RCON to loopback, so the structural refusal stands and the
hostile-input suite remains the automated evidence.

## Findings from the owner session

### F3 — a command write into an unloaded chunk destroys the stored chunk (P0, open)

Mechanism, in two files:

- `crates/server/src/commands.rs` `BlockWriteMode::apply` (2091-2105) writes through
  `Game::world_mut().set_block(...)` at whatever coordinates the command names, with
  nothing ensuring that chunk is loaded.
- `crates/world/src/world.rs` `set_block` (318-344) calls `ensure_chunk` (233-239),
  which inserts an **all-air** placeholder when the chunk is absent. The crate says
  so and means it: the placeholder exists "for the paths that need a chunk to write
  into". The placeholder is dirty, so the next save writes it over the stored entry.

Reproduced end to end in this session:

1. `13:57:52` — the landmark is present after a restart with the player online;
   the probes refuse and the screenshot shows it rendered.
2. `13:58:45` — clean stop; `r.-1.-1.mca` = 16 384 bytes, sha256 `bd949a75…`.
3. `13:58:54` — restart with **no player online**. Console:
   `setblock -8 100 -8 minecraft:emerald_block` → `Set the block at -8 100 -8`;
   `setblock -8 72 -8 minecraft:stone keep` → `Set the block at -8 72 -8` — the
   `keep` rule passed although the stored chunk holds a diamond there (F4);
   `save-all flush` → `Saved the world`; and one second later the autosave reports
   `mc_persistence::world: chunk save complete written=1 regions=1`.
4. `13:59:25` restart, `13:59:40` the real client joins. Probes: `-8 90 -8` (a gold
   marker) → `Set the block`; `-14 67 -14` (a landmark corner) → `Set the block`;
   `-8 100 -8` (the emerald) → `No change`. `item4-clobber-verdict-client.png` shows
   the landmark gone and plain terrain in its place.
5. `14:02:34` — `setblock -8 62 -8 minecraft:stone keep` → `Set the block` and
   `setblock -8 0 -8 minecraft:stone keep` → `Set the block`, while
   `setblock -8 30 8 minecraft:stone keep` (the neighbouring chunk) → `No change`.
   So the stored chunk is **all air**, not regenerated terrain — y=0 and y=62 are
   solid in generated terrain. The world now has a 16×16 void column at the spawn.

Reach: any `/setblock` or `/fill` — from the console, from RCON (P19-04, the operator
surface that just landed), or from a future command block — at a coordinate in a
chunk with no player in it. Everything stored in that chunk, terrain and player
edits alike, is replaced by air on the next save. It is the AUDIT-09 B-01 class
("a stored chunk is never overwritten by a placeholder") through a new door: the
command path, which landed in P18-02, does not go through
`Game::load_or_create_chunk`, the path that reads the stored chunk first.

Status: **fixed in P19-08**. `BlockWriteMode::apply` now calls
`Game::load_or_create_chunk` for the target before it reads or writes, which is the
ordering the rest of the server already used; the fix carries two pins
(`setblock_into_an_unloaded_chunk_keeps_the_stored_blocks`,
`keep_reads_the_stored_block_of_an_unloaded_chunk`), each proven red by deleting
that one call and then restored byte-exact, and the live reproduction above was
re-run afterwards and inverted (diamond, the second write and the terrain at y=0
all survive; a control cell that is really air still writes). The perturbation
record and the residual note are in the changelog entry.

### F4 — `setblock … keep` treats an unloaded chunk as air (low, fixed by the same line)

`crates/server/src/commands.rs` tested `current.is_some_and(|id| id != 0)`.
`None` — the chunk is not loaded — is not `Some(non-air)`, so the rule passed and the
write proceeded. Vanilla loads the chunk and then applies the rule. Observed in F3
step 3. Low severity alone; it is the rule that made F3 silent instead of refusing,
and P19-08's load-before-read fixes it with the F3 change.

### F5 — log severity and observability (low)

- A **normal** shutdown logs `WARN mc_network::listener: connection ended with an
  error peer=127.0.0.1:55225 error=shutdown requested` (`13:53:38`): the expected
  close is reported as an error.
- A stock RCON client that disconnects between frames logs
  `DEBUG mc_server::rcon: RCON connection closed … error=malformed protocol input:
  rcon EOF in the length prefix` (`14:04:02`).
- A **failed RCON login logs nothing at all** — the connection opens and closes at
  DEBUG, so a brute-force attempt leaves no line for an operator to see.

### Observations that are not defects

- **A refused join keeps its connection ~45 s.** After the refusal at `13:47:16` the
  server logged `keepalive timeout, disconnecting player` / `kicking player name=Main
  reason=Timed out` at `13:48:01`. No slot or entity was ever spent (`players=0` in
  the `13:47:26` metrics row), so this is a lingering connection task, not a leak.
- **A console line with a byte-order mark is refused, not fatal.** A harness mistake
  sent `\u{feff}whitelist`: `INFO console: Unknown command "\u{feff}whitelist". Try
  /help.`, and the server carried on.

## Deviations from the run sheet (recorded, not silent)

1. **The edit was made by command, not by hand.** The step "build/break a block as
   Main" was performed with console `/setblock` and `/fill`. The harness launches the
   client, reads its log and captures its window, but does not inject keyboard or
   mouse input into the game — so no claim here rests on a player's click.
2. **The restart ran a copy of the config** (`target/p19-session/config-restart.toml`)
   with `autosave_ticks = 200` (10 s) instead of 6000, so the periodic write is
   observable inside a session-length window. The edit, the `save-off` window and the
   flush all ran under the run-sheet config.
3. **Reading a cell used `setblock … keep` as an instrument, with a control.** A cell
   just filled by the same command returned `No change … (mode keep)`, so "the cell is
   air" and "keep always refuses" cannot be confused. The instrument *writes* when the
   cell is air; every cell it wrote is named above. (This is also how F3 was found.)
4. **Client evidence is a log line plus a screenshot.** The client window is raised and
   made topmost for the capture and demoted afterwards; the screenshot is then read
   back, so the claim "the screen shows X" is checked against the image.

## What this session does not establish

- **Cross-host RCON**, for the structural reason above.
- **Online-mode split-b** (item 6): withdrawn by decision, not deferred — no
  Mojang-authenticated join was attempted, and KD-01 therefore stays partial with
  automated pins only.
- **Anything needing a person's judgement of a display.** The screenshots were read
  programmatically; a claim like "the lighting looks right" is not made here.
- **Reproducibility for a reader from the repository alone.** The screenshots and
  server logs are under the git-ignored `target/p19-session/`, so they are artifacts
  of this machine; the server logs' decisive lines are quoted above so the record
  itself carries the evidence. Re-run `target/p19_server.py` + `target/p19_client.py`
  with the session config to regenerate them.

## Artifacts (this machine, git-ignored)

| Artifact | Path |
|---|---|
| Whitelist refusal screen | `target/p19-session/item1-whitelist-refusal-client.png` |
| Ban screen with the reason (live disconnect) | `target/p19-session/item2-ban-live-client.png` |
| Ban screen from the join gate | `target/p19-session/item2-ban-rejoin-client.png` |
| Clean join after the offline listing | `target/p19-session/item3-whitelist-join-client.png` |
| Landmark rendered after a restart | `target/p19-session/item4-inworld-landmark-client.png` |
| Landmark destroyed by the F3 write | `target/p19-session/item4-clobber-verdict-client.png` |
| Server logs (5 runs) | `target/p19-session/session.log`, `session2.log` … `session6.log` |
| Client logs, one per step | `target/p19-session/evidence/*.log` |
| Session config, and its restart copy | `target/p19-session/config.toml`, `config-restart.toml` |
