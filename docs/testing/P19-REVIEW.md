# P19-07 Adversarial Review (agent half + owner session)

Independent of the implementation pass. Every P19 acceptance claim is a row:
what would settle it, the verdict, and the evidence. A refuted claim is a
result — two are recorded below (F1 fixed in this pass, F2 pinned). Counts
are decomposed (N compared / M skipped with a named reason).

Owner real-client access session: **RUN 2026-09-30** with the owner's real
Minecraft 26.1.2 client. Items 1–5 pass; the run and its evidence are in
[P19-ACCESS-SESSION.md](P19-ACCESS-SESSION.md). Item 6 stays withdrawn by the
no-online-mode decision. The session raised three further findings — F3 (a
command write into an unloaded chunk destroys the stored chunk, **fixed in
P19-08 with two pins**), F4 (`setblock … keep` on an unloaded chunk, fixed by the
same line) and F5 (log severity, open) — below.

## Instrument

- Code read for every gate/semantic claim (`path:line` where it matters).
- Automated: `cargo test -p mc-server --test whitelist_e2e` (11),
  `--test bans_e2e` (9), `--test gameplay_config_e2e` (3),
  `--lib` config pins + exposure matrix, the P19-04 RCON hostile suite,
  the P19-05 online/handshake vectors. Full headline re-derived from
  `python tools/gates/run.py --quick` (see TEST-MATRIX).
- Probe discipline: each new pin proven red by neutralising its mechanism,
  then restored and re-run green.

## Claim table

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| Non-listed client refused at login with Vanilla's message (P19-01) | `game/session.rs:473-485` + `whitelist_e2e` | confirmed | `refuse_join(&outbound, "You are not white-listed on this server!")`; gate runs before slot/entity/state spend |
| Operator exemption per the jar (P19-01) | `game/session.rs:478-481` + `whitelist_e2e` | confirmed | `!is_operator` bypasses; operator-joins-unlisted test green |
| Banned player/IP refused before a slot is taken (P19-02) | `game/session.rs:486-504` + `bans_e2e` | confirmed | player-ban then ip-ban checks precede `prepare_join`; rejoin-refused tests green |
| Banning a live player disconnects them (P19-02) | `bans_e2e` | confirmed | ban queues disconnect, next tick drops session; client sees the ban screen text |
| Expiry honoured (P19-02) | `bans_e2e` clock-injected | confirmed | expired row admits, live row refuses |
| No op exemption for bans (P19-02) | `game/session.rs:486-496` | confirmed | ban gate has no operator check, unlike the whitelist gate above it |
| Console commands run at console level; EOF never stops the server (P19-03) | `apps/server/src/console.rs` + lib EOF pin | confirmed | stdin dispatch at console level; EOF test green |
| `save-off` holds all region writes; `save-all flush` returns after data on disk (P19-03) | `game` save-flag tests | confirmed | shared `autosave_due` gate; flush path pinned |
| Stock RCON client runs `list`/`stop`; hostile-input suite green; refuses empty password (P19-04) | RCON codec/config/tick-loop tests | confirmed | LE framing + fixed-time auth + throttle budget + chunked replies; config gate refuses passwordless enable |
| Split-a: encryption handshake + `hasJoined` timeout/refusal vectors (P19-05) | `mc-network` online/handshake tests | confirmed | RSA-1024 handshake, fixed-time token check, SHA-1 server-hash vectors, blocking `hasJoined` vectors, AES-128/CFB8 transport |
| Split-b: a real account joins with its skin (P19-05) | owner-run | NOT RUN | carried as the named gap; KD-01 stays partial (see below) |
| Each gameplay key reads/writes/defaults through `Game`; enforcement P20-owned (P19-06) | `gameplay_config_e2e` (3) + config lib pins | confirmed | defaults/boot-install/read-back green; every key's doc names its P20 owner |
| Unknown keys still refused (P19-06) | config lib pin | confirmed | `deny_unknown_fields` refusal test green |
| Exposure warning once on non-loopback + offline + no whitelist (P19-06) | lib exposure matrix | confirmed | pure predicate + single call site at end of `start_network` |
| Gate order whitelist-then-bans is deterministic (adversarial) | `game/session.rs:473-504` | confirmed | full → whitelist → player-ban → ip-ban; a profile both unlisted and banned sees the whitelist message, always |
| F1: `/whitelist add|remove` resolves offline names (adversarial) | `whitelist_e2e` new pins | refuted, then fixed | add/remove required a live session (`session_id_by_name` → refuse). Now both resolve via `ban_uuid_for` (single derivation site, `commands.rs:1015-1016`, shared with bans); offline add admits the later join, offline remove unlists without a join. Both pins proven red by restoring the refuse path |
| F2: `whitelist reload` on a malformed file keeps the live list (adversarial) | `whitelist_e2e` new pin | confirmed + pinned | `reload_whitelist` loads before replacing (`game/session.rs:1655-1663`), so a bad file errors with the list untouched. Pin proven red by clearing first (live list lost, test red), restored green |

## Counts

- Compared: 16 claim rows settled (13 confirmed, 1 confirmed+pinned, 1 refuted-then-fixed, 1 NOT RUN).
- Skipped with a named reason: 1 (split-b needs a real account + owner client; automation has no Mojang credentials).
- New pins this pass: 3 (`whitelist_add_lists_an_offline_profile`, `whitelist_remove_unlists_an_offline_profile`, `whitelist_reload_malformed_keeps_the_live_list`) + 1 (`ban_files_an_offline_profile`, documents the pre-existing offline-ban path the whitelist now mirrors).
- Owner session (2026-09-30): **4 of 4 runnable items pass**, item 6 withdrawn, and 3 further findings raised — F3 (P0) and F4 fixed in P19-08 with two pins each proven red, F5 (low) open. None of the three is a P19 claim failure.

## KD-01

Stays partial. Split-a (automated vectors) is green. Split-b (real account
join with skin) is DROPPED by operator decision (2026-09-30): online mode
is explicitly not enabled on this server, so no Mojang-authenticated join
will ever be exercised here — the online path stays code-complete and
automated-pinned, deployment stays offline. Session item 6 below is
withdrawn, not pending.

## Findings from the owner session (2026-09-30)

None of these is a claim-table row above; they were found by probing past the
checklist, and each is recorded in full with its reproduction in
[P19-ACCESS-SESSION.md](P19-ACCESS-SESSION.md) §Findings.

| ID | Instrument | Verdict | Evidence |
|---|---|---|---|
| F3: a `/setblock` or `/fill` into a chunk that is not loaded replaces the stored chunk with the all-air placeholder, and the next save writes it over the region entry — terrain and every player edit in that chunk are lost | end-to-end on the scratch world: landmark persisted and rendered → clean stop → restart with no player → two console writes + flush → restart + real-client join → probes and screenshot | **confirmed → FIXED (P19-08)**, pinned | `commands.rs` `BlockWriteMode::apply` wrote through `World::set_block` without loading; `world.rs:318-344` → `ensure_chunk` (233-239) inserts the placeholder. Observed: markers `No change (mode keep)` before, `Set the block` after; `-8 62 -8` and `-8 0 -8` air while the neighbouring chunk refuses; `chunk save complete written=1 regions=1` one second after the unloaded write. Pin: `setblock_into_an_unloaded_chunk_keeps_the_stored_blocks` (red without the fix: the stored diamond reads back as block 0) |
| F4: `setblock … keep` passes when the chunk is not loaded (`is_some_and` on `None`), where Vanilla loads the chunk first | same run, step 3 | **confirmed → FIXED (P19-08)**, same line | `commands.rs` `BlockWriteMode::apply`; `Set the block at -8 72 -8` with `keep` against a stored diamond block. Pin: `keep_reads_the_stored_block_of_an_unloaded_chunk` (red without the fix: `Set the block at -4000 64 -4000` instead of `No change … (mode keep)`) |
| F5: a normal shutdown and a normal RCON client close are logged as errors, and a failed RCON login logs nothing | server logs, both paths | **confirmed (low)** | `connection ended with an error … error=shutdown requested`; `RCON connection closed … error=malformed protocol input: rcon EOF in the length prefix`; no line for a bad password |

F3 is the AUDIT-09 B-01 class ("a stored chunk is never overwritten by a
placeholder") reached through a door that landed later: the P18-02 command path
did not go through `Game::load_or_create_chunk`. It is not a P19 defect, and it is
**fixed in P19-08** — the command path now takes that ordering, with the two pins
named above; the live reproduction was re-run and inverted. F5 is still open and
has no owner: it is a log-severity tidy (a normal shutdown and a normal RCON close
are logged as errors; a failed RCON password logs nothing), not a behaviour
defect, so it is recorded rather than patched here.

## Session items for the owner pass

Run 2026-09-30; verdicts below, evidence per item in
[P19-ACCESS-SESSION.md](P19-ACCESS-SESSION.md).

1. Unlisted profile sees "You are not white-listed on this server!" on screen — **PASS** (screen captured; client log agrees).
2. Banned profile sees the ban screen with reason (and removal date for temp bans) — **PASS** (live disconnect and rejoin both show the reason); pardon restores the join.
3. `/whitelist add <offline-name>` then that profile joins without a restart — **PASS** (added while offline, joined next launch).
4. Console `save-all flush` / `save-off` / `save-on` round trip on a live server — **PASS** (region file held at 0 bytes while held, 12 288 bytes after the flush; the edit survives a restart).
5. Stock RCON client runs `list` from another host (passworded, loopback refused) — **PASS for `list` and `stop`** with a third-party client on the same host; cross-host deferred (the bind is loopback, so the refusal is structural).
6. ~~Online-mode join with skin~~ — WITHDRAWN by the no-online-mode decision.

## Phase verdict

**P19 is closed on `2d1d66b`** (`gates/EXIT-GATES.md` §P19, universal
phase-close clause). The phase's own evidence: this review's 16 claim rows, the
owner session above, and P19-08's two pins. The gate evidence: `run.py --quick`
every gate passed (1 788 / 0 / 41 / 141) and CI run `36803950975` green on the
same commit. Named open at closure: **F5** (log severity, no owner) and the
online-mode real-account join (withdrawn by decision, so KD-01 stays partial and
the v0.4.0 verdict at P22-08 must record it).
