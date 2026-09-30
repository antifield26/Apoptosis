# P19-07 Adversarial Review (agent half)

Independent of the implementation pass. Every P19 acceptance claim is a row:
what would settle it, the verdict, and the evidence. A refuted claim is a
result — two are recorded below (F1 fixed in this pass, F2 pinned). Counts
are decomposed (N compared / M skipped with a named reason).

Owner real-client access session: NOT RUN (no owner session this pass). The
session items are enumerated at the bottom so the owner pass has a checklist;
automation covers everything except what only a real client can show.

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

## KD-01

Stays partial. Split-a (automated vectors) is green; split-b (real account
join with skin) is NOT RUN and moves only with owner evidence, per the
P19-05 acceptance row.

## Session items for the owner pass (NOT RUN)

1. Unlisted profile sees "You are not white-listed on this server!" on screen.
2. Banned profile sees the ban screen with reason (and removal date for temp bans).
3. `/whitelist add <offline-name>` then that profile joins without a restart.
4. Console `save-all flush` / `save-off` / `save-on` round trip on a live server.
5. Stock RCON client runs `list` from another host (passworded, loopback refused).
6. Online-mode join with skin (P19-05 split-b; closes KD-01 with evidence).
