# AUDIT-14 — P14 (admin commands, streaming, loot baseline, Pi soak)

Basis: `f83689b` (loot baseline) plus this audit's own changes (A14-01
trailing-check fix, three new tests, soak record + drift touch-ups), audited
read-only except the fixes below. P14 is eight landings (`7d4fb1f`..`1f57a4c`)
plus two soak follow-ups (`8fa8521`, `f83689b`): admin command set, op/deop
persistence + ladder, Y-then-longer axis order, forget + view distance,
reconnect sweep, changelog verdict, cache-center fix, loot baseline. Verdicts:
**confirmed** (independent instrument unless labelled "re-run"),
**refuted**, **not checkable** (reason).

## Why these weights

P14 writes new authority state (ops file, level.dat difficulty) and changes
the collision ladder — a wrong write there corrupts a world or opens a
privilege hole, so the authority lane carries the most weight. Streaming
packets and the loot baseline come next (new wire bytes, new default drops).
Movement, reconnect, counts and docs carry the remainder. The historical
opens concentrate in protocol strictness (A-03), persistence ordering (B-02),
command-tree cost (C-08) and fixtures (E-02) — one lane rechecks exactly the
opens P14 touches. The falsification lane perturbs six P14 rules; the soak
lane re-derives the Pi verdict from the archived logs instead of the chat.

## The per-claim table (P14 exit gate + soak)

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| Admin set: gamemode/give/kill/seed/difficulty, Operator-only, self-only | re-run `admin_commands` (13) | confirmed | modes flip, counts + overflow drops, creative kill, seed echo, difficulty persist + peaceful gate, locked refusal |
| `/op` grants Console, persists, reloads; `/deop` revokes | re-run op/deop tests | confirmed | live + file + reload agree; unknown player and level-2 grant refused |
| Failed save rolls back the grant | NEW `op_rolls_back_when_the_file_write_fails` | confirmed | file-as-directory: refusal message, live level unchanged, nothing lingers in memory |
| Locked difficulty refuses the set | NEW `locked_difficulty_refuses_the_set` | confirmed | refusal names the lock, difficulty unchanged |
| Axis order is Y, then the longer of X/Z | re-run `mc-world` collision pair | confirmed | longer-axis-first slip-past; inverted order fails (probe P5) |
| Teleport-away forgets departed chunks by name | re-run `chunk_streaming` teleport | confirmed | spawn chunk named in `forget_level_chunk` 37 |
| View distance clamped, confirmed, honoured per session | re-run `chunk_streaming` clamp | confirmed | 100 → server max, `set_chunk_cache_radius` confirm, radius-2 receives nothing beyond 2 |
| Config-phase setting reaches the game | NEW `client_information_view_distance_reaches_the_game` | confirmed | radius event applied to the session, not just stored |
| Center re-sent on every chunk crossing | re-run `chunk_streaming` center | confirmed | crossing emits `set_chunk_cache_center` (the soak defect) |
| Leave sweeps entities; restart rejoin works | re-run `reconnect` pair | confirmed | `remove_entities` broadcast; post-restart rejoin |
| Eight-table loot baseline covers packless breaks | re-run `mc-data` pair + `loot_and_pickup` baseline | confirmed | stone→cobble etc.; silk branches; bare hand is known-unenchanted |
| Mixed 10-client Pi soak passes with one noted spike | re-derived from archived logs (see Soak lane) | confirmed with correction | load medians 3.06/3.20/3.31 ms; 58 lifetime overruns (timeline below); clients 9/9 zero failures |
| Gate 1440/0/34/113 | full gate re-run (see §7) | confirmed | fmt, clippy `-D warnings`, docs-audit incl. `check_gate_totals`, full workspace |

## Lane 1 — authority writes (ops file, level.dat, ladder)

- **Ladder soundness — confirmed by read + probe P1.** `/op` and `/deop`
  require Administrator; the five admin commands require Operator; `stop`
  requires Console; `function` requires nothing itself because every command
  inside re-checks (the comment at `commands.rs:143-145` states the reason:
  a permissioned wrapper would be a privilege-escalation route). Lowering
  `/op` to Operator fails `level_two_holder_cannot_op_after_the_raise`.
- **Grant-before-save ordering — confirmed by read.** `grant_operator`
  applies live, saves, and on save failure revokes live + reports "nothing
  was changed". The NEW rollback test pins all three halves (message, live
  level, memory). Without the test the ordering would be a two-line comment
  away from silent divergence.
- **Difficulty lock — confirmed by read + NEW test.** The lock lives in
  `level.dat` (`difficulty_locked`), survives reload, gates peaceful
  spawning, and now refuses `/difficulty` with a reason instead of silently
  overwriting. Behaviour verified, not just the flag round trip.
- **A14-02 · Info · the admin rejoin anomaly is closed as a test artifact.**
  Once, a hand-rolled `admin_commands` harness saw `PermissionDenied` for an
  op grant after rejoin where the `ops_e2e` path succeeds. Never reproduced;
  the product path (join → op → rejoin → command) is green in `ops_e2e` and
  in the NEW rollback test's harness. No product change; recorded so the next
  person who sees it knows it was seen before.
- NOT-checked: concurrent op/deop from two sessions racing the same file
  (single tick thread serialises commands today — read, not load-tested);
  ops file hand-edited while running (load-on-boot only, declared).

## Lane 2 — streaming packets (forget, center, radius)

- **Forget shape — confirmed by jar table + round trip.** Id 37, one packed
  long, x low / z high per `ChunkPos.pack` bytecode; negatives round-trip.
- **A14-01 · Low · the new clientbound decoder skipped the trailing check —
  FOUND, FIXED with this document.** `ForgetLevelChunk::decode` read its
  long and ignored the rest, while every other fixed-shape clientbound
  decode refuses trailing bytes (the AUDIT-12 convention: clientbound shapes
  are fully modelled because our encoder writes them; the deliberate
  silent-accept design covers serverbound only, A-03). Fix: the standard
  `has N trailing bytes` refusal + a padded-input assertion in the existing
  test. Falsification by construction — the pre-fix body IS the perturbation
  (it returns `Ok` on the padded input the test now feeds it).
- **Center on every crossing — confirmed by read + re-run.** The soak defect
  (center once at enter-play) is fixed at the crossing detector, not papered
  at the caller: any path that changes chunk coordinates re-emits before the
  stream pass reads the session.
- **Radius clamp + confirm — confirmed by re-run + probe P3.** Raw value
  fails the cap assertion; the confirm packet is what the client renders to,
  and both passes read the session radius (no second source of truth).
- NOT-checked: config-phase tree path (declared gap — the setting arrives
  through the join-time forward); chunk-ordering assertions (declared gap).

## Lane 3 — loot baseline

- **Baseline content — confirmed by re-run + probe P4.** Eight tables,
  silk branches for stone and grass, bare hand known-unenchanted; bare
  `LootTables::new()` fails `breaking_common_blocks_without_a_pack_drops_the_baseline`.
  The pack path is untouched: `load_packs` still replaces the baseline, and
  `vanilla_loot` still proves shipped stone yields shipped cobblestone.
- **Pool-not-table refusal and the empty-map tool — confirmed inherited.**
  Both owner decisions from the landing thread are in the code with their
  reasons; the tests pin the behaviours (single-branch table still drops,
  unenchanted hand evaluates enchantment-gated pools at level 0).
- NOT-checked: a full-pack-vs-baseline diff (the census suite owns the pack
  side; the baseline is eight hand-written tables, reviewed line by line).

## Lane 4 — movement, reconnect, counts

- **Axis order — confirmed by bytecode + re-run + probe P5.** Y first, then
  the longer horizontal axis; zero axes skipped in-loop as before. Inverting
  the comparison fails `the_longer_horizontal_axis_resolves_first`. Step-up
  stays deferred (full-cube worlds step integer heights by construction —
  declared, not smuggled).
- **Reconnect — confirmed by re-run.** Leave removes the session, the sweep
  tells watchers, restart rejoin works. Death/respawn on screen stays on
  KD-38 (needs a keyboard).
- **Gate re-run — confirmed.** `1440 passed / 0 failed / 34 ignored / 114
  suites, every gate passed` (fmt, clippy `-D warnings`, docs-audit incl.
  `check_gate_totals`, full workspace).
- **Arithmetic — confirmed.** Lib sum 1 039 (unchanged — the trailing
  assertion extends an existing test) + 5 doc-tests + 396 named-suite
  (393 + 2 `admin_commands` + 1 `command_e2e`) = 1 440. Suites stay 113:
  `command_e2e` 11 → 12 is a count corrected inside an existing suite, not a
  new suite.
- NOT-checked: re-running the ignored differential suites (jar-gated,
  untouched by P14); a full named-suite recount (the P10-03 limitation in
  TEST-MATRIX stands).

## Lane 5 — falsification probes (all reverted, tree clean)

| Probe | Revert | Expected failure | Observed |
|---|---|---|---|
| P1 `/op` requires Operator, not Administrator | `level_two_holder_cannot_op_after_the_raise` | level-2 grant succeeds | FAILED as expected |
| P2 unload skips the forget send | `teleporting_away_forgets_the_departed_chunks_by_name` | no forget packet | FAILED as expected |
| P3 view distance unclamped | `view_distance_is_clamped_confirmed_and_honoured` | 100 honoured | FAILED as expected (`left == right`, caps the request) |
| P4 bare tables, no baseline | `breaking_common_blocks_without_a_pack_drops_the_baseline` | nothing drops | FAILED as expected |
| P5 shorter axis resolves first | `the_longer_horizontal_axis_resolves_first` | wrong slip side | FAILED as expected |
| P6 `/kill` via ordinary damage | `kill_runs_the_death_path_even_in_creative` | creative survives | FAILED as expected |

Every P14 rule is load-bearing. `git status` after revert showed only the
three intended audit test additions (two files); the probes left no trace.

## Lane 6 — historical opens disposition

- **A-03 (trailing bytes) — design holds, one new instance caught (A14-01
  above).** Serverbound silent-accept stays deliberate (refusing an
  unmodelled field would break a real client); clientbound hard-refusal is
  the convention and P14-04's new decoder is now back inside it. The
  capture-sweep experiment stays open.
- **B-02 (region write order) — untouched by P14**, still open.
- **C-08 (command tree rebuilt per dispatch) — noted, no change.** The tree
  grows with the six P14 commands; a dispatch builds ~13 nodes to answer one
  command. Trivial at command rates (the 300-command flood in `command_e2e`
  covers the rate side); rebuilding keeps handler registration branchless.
- **E-02 / E-05 / D-04 / D-07 / M-3 — untouched by P14**, still open.
- **P05-10 — stays closed** (drain re-verified in passing; nothing in P14
  touches the queue).

## Soak lane — the verdict, recomputed from the archived logs

The chat verdict ("settled mean ~3 ms, p95 ~3.2, zero new overruns after
14:35") was read off a live tail. Recomputed from `~/soak_server.log`
(174 `tick metrics` windows), `~/soak_metrics.csv` (190 sampler rows) and
`~/soak_clients.log`:

- **Load medians confirmed:** 14:28–14:55, 10 players, n=54 — window-mean
  median 3.06 ms, p50/p95/p99 medians 3.04/3.20/3.31 ms, worst 301 ms,
  chunks ≤ 649. The chat's 3.0/3.2 figures were right for the load window.
- **A14-03 · Medium · the tail claim is REFUTED — corrected in
  `BENCHMARK-BASELINE.md` §P14-Pi and the CHANGELOG.** Lifetime overruns
  total 58, not "none after 14:35": +2 at 14:36:03 (total 52), thirty-one
  quiet minutes, then **+6 at 15:07:33** with 1 idle player (total 58),
  then zero for the final 44 minutes. Lesson, recorded: verdicts recompute
  from archived logs; a live tail is not the log.
- **The 15:07:33 spike is not checkable with current instrumentation.**
  Window mean 2.27 ms / p95 0.90 ms with 6 ticks over budget (worst 301 ms);
  1 player, 85 entities, 406 chunks, 0 dirty; no join/leave/command/save
  line anywhere near it at INFO or DEBUG. The server recovered on its own
  (44 clean minutes after). Follow-up, backlog: log the worst phase on
  overrun windows — the snapshot reports aggregate MSPT only. Not blocking:
  no crash, no disconnect, owner stayed connected.
- **Sampler confirmed:** CPU p50 6.9 / p95 8.5 / max 35.2 (% of one core);
  RSS 14 MB → 337 MB, flat at the end (load-up, no leak signal in-window).
- **Clients confirmed from the file:** 9/9 reached PLAY, 1802.1 s, 1076
  keepalives, 16200 teleports acked, 3240 commands, 2601 chunks,
  `failures: []`. Plus the owner: 10 concurrent.
- **A14-04 · Low · the soak ran but was never recorded.** The repo still
  said P14-06 NOT RUN after the verdict was announced in chat. Fixed with
  this document (§P14-Pi + CHANGELOG P14-06/07/08). Ops note: the server
  logged at DEBUG (32 MB); future soaks log at INFO.

## Lane 7 — docs drift (P14 aftermath)

- **FOUND, FIXED with this document:** `BENCHMARK-BASELINE.md` (no §P14-Pi
  — added with the full re-derivation); `CHANGELOG.md:78-83` (P14-06 NOT
  RUN after the soak ran — rewritten as RAN with corrected figures);
  `:114-118` (`admin_commands` "11 tests" after two audit tests made 13;
  soak clause NOT RUN — both updated); `:85-90` + `:122-123` (verdict now
  records the soak as pass-with-note; tag still not cut — the screen halves
  and the 25567 verification walk stay open); `RUNBOOK.md:174-175` (re-soak
  "owed" after it ran — pointed at §P14-Pi);
  `TEST-MATRIX.md:8,12,72,111,117,120` (totals 1437→1440,
  `admin_commands` 11→13, `command_e2e` 11→12 inside the existing 113 suites,
  audit-tests paragraph, A-03 line precision) plus the two total
  restatements the gate guards (`README.md:86`, `CONTRIBUTING.md:35`:
  1437→1440).
- **Left as dated record:** the P09-Pi figures and the KD-35 parity row
  (that soak's workload and tree, frozen); the chat's tail claim (superseded
  by §P14-Pi, not edited anywhere because it was never written down — which
  was A14-04).
- NOT-checked: prose beyond the P14 touch surface.

## §8 Counts

1440 passed / 0 failed / 34 ignored / 113 suites on `f83689b` plus this
document's changes (one trailing-check fix, three new tests, soak record +
drift lines; gate re-run after all edits — re-run figure: 1440 / 0 / 34 /
113, every gate passed).
