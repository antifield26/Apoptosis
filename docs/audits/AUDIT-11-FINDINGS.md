# AUDIT-11 — the M-1..M-4 landing, audited

Basis: `eaae11d` (the landing) plus its two docs commits, audited from `eaae11d`.
Per the handover: every claim falsified where possible, counts decomposed, weak
evidence labelled. Verdicts: **confirmed** (independent instrument unless labelled
"re-run"), **refuted**, **not checkable** (reason).

## The per-claim table

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| §1 commits/CI (8b43e14, 35bb2fa, eaae11d; runs 35059507773/35059743646/35060848454 all success) | `git log` + `gh run list` | confirmed | all three present; all three `completed success` (13m46s/13m59s/13m47s) — **seen** |
| §1 gate 1358/0/33/106 on the committed tree | gate re-run (weak: same instrument) | confirmed | `1358 passed / 0 failed / 33 ignored / 106 suites, every gate passed` |
| `target/m_probes.py`: 7 perturbations each failing a named test, files restored byte-exact | re-run | confirmed (re-run) | M-1, M-2a/b/c, M-3, M-4a/b all `failed as it should`; SHA-256 before == after for play.rs, game.rs, mob.rs |
| §2 "the rig's `body_bytes` includes the packet-id byte" | independent trace read | **confirmed** | c2s 13 (tick end, empty body) shows `body_bytes` length **1** — the id byte itself; my AUDIT-10 misread it as a payload byte, which is the root of the misdiagnosis |
| §2 "the 95 id-0 packets are accept_teleportation" | independent decode | confirmed | after stripping the id byte, 94 payloads are one 1-byte VarInt and 1 payload is two bytes — teleport ids, zero bytes left over; s2c id 4 (ack) count **0** and s2c id 60 (teleport) count **0** in the same trace |
| §2 "the real digs are id 41, 24 packets, 11-byte bodies" | independent decode | **confirmed with a correction** | 24 packets, 11-byte payloads after the id byte; **correction: all 24 are START (status 0) with BlockPos.ZERO, face DOWN and sequence 0 — there are no FINISH packets, so "12 start/finish pairs" is imprecise** (see N-1) |
| §2 "each dig answered by an s2c id-8 block_update at exactly that position" | independent decode | **confirmed with a correction** | 18/24 digs have a same-position update within 60 seqs — **but every position is BlockPos.ZERO** (see N-1): the server broke deep-underground block (0,0,0) → air twelve times |
| §2 "the client id table agrees with the server table on all 210 ids" | independent extraction (broadened regex) | confirmed | the client `GameProtocols` builder registers **69 serverbound** packets in exactly our TSV order — **but only when the extraction spans all four PacketTypes holders** (Game/Common/Cookie/Ping); a GamePacketTypes-only regex yields 61 refs and a phantom five-position shift — that regex bug was the AUDIT-10 handoff's source, and AUDIT-09's "silent skip" class applied to an extraction regex |
| §2 "26.x clients store, not apply, a block update while a prediction is open; only the ack flushes it" | client-jar javap (builder's read) + the trace's zero acks | confirmed (the trace half independent) | the owner-session trace contains **zero** s2c id-4 acks while 12 block updates shipped — the missing-ack defect is real |
| §1 "the respawn shape is the bytecode's" | client-jar javap re-read (`target/respawn_client.txt`, `target/spawninfo.txt`) | confirmed | CommonPlayerSpawnInfo: holder + ResourceKey + long + 2 bytes + 2 bools + Optional + VarInt + VarInt; Respawn adds a trailing byte — the landing's encoder matches (m_probes M-1 pins the reversion) |

## Lane findings

### The audit's own new findings

- **N-1 · High · the owner-session digs ride BlockPos.ZERO, and the dig path has no
  reach validation.** All 24 dig payloads are `00`×11 (START, BlockPos.ZERO, DOWN,
  sequence 0) and the server dutifully broke deep-underground block (0,0,0) → air
  twelve times. Two consequences: (a) the owner's "mining broken" was the client
  aiming at real blocks while the server dug (0,0,0) — the missing ack is only half
  the story; (b) **the dig path has no reach check**, so a hostile (or desynced)
  client can break any block anywhere on the map — a P04-era named gap that is now
  player-visible. The root cause of the ZERO digs is not settled: candidates are the
  client's prediction state at death/respawn time (the digs cluster after the owner's
  respawn attempts) or our join flow leaving the client's `hitResult` unseeded. Named
  with the experiment: a client-side debug session, or a server log of the dig
  positions versus the client's own F3 coordinates in a controlled round.
- **N-2 · Info · the acceptance round's digs still apply the new ack** — after the
  fix the client applies the server's (0,0,0) updates; the acceptance round must
  confirm a *real* aimed dig breaks *the aimed block* and stays broken (the round
  below did, once).

### The builder's ten self-reports

1. **§4.1 confirmed**: `two_digs_in_one_tick...` sends sequences 11 then 3, so it
   cannot distinguish `max` from `first`; m_probes M-2c only catches "last". The
   product is right (vanilla `Math.max`, the doc comment) — the test is one
   perturbation short. A three-sequence version (3, 11, 7) is the fix.
2. **§4.2 confirmed**: `block_changed_ack_is_a_single_varint` compares our encoder to
   our own `varint::write_varint` — a shape check. The independent authority is the
   client jar's `VarInt.write` (or the TSV); recorded as weak.
3. **§4.3 confirmed**: the rewritten `respawn_rejects_the_old_truncated_spawn_info`
   (eaae11d) builds the old body with a literal writer; the load-bearing assertion
   lives in `respawn_is_exactly_what_the_client_reads`, which m_probes M-1 does catch.
4. **§4.4/§4.5 not re-measured**: M-3's 30.41-vs-33.05 anchor choice is a judgement
   call with a player-visible residual (pigs and skeletons ~8% slow); the capture was
   not built to measure speed, and the creeper/spider rows sit below their
   attributes. The anchor deserves the owner's sign-off; `mob_speed_ceiling.py`'s
   threshold (≥5 ticks at ≥97%) moves the number.
5. **§4.6 confirmed**: M-4's wall half is collision-restating; the new assertions are
   the halt and the fluid refusals; `MOB_LOOKAHEAD_BLOCKS = 1.0` is a choice; no
   ledge/fall handling; the probe uses the mob's centre.
6. **§4.7 recorded**: the landing sends a real `lastDeathLocation`; DeathScreen shows
   no reference (the builder's read) — LocalPlayer/ClientPacketListener/HUD remain
   unchecked. The acceptance round can settle it.
7. **§4.8 checked**: `is_liquid` + `LIQUIDS` carry the registry-existence tests
   (`collision.rs:341-362`) mirroring `NON_SOLID`'s; the three-predicate story for an
   out-of-registry id (solid / solid-or-unknown / liquid-false) is documented at the
   same site.
8. **§4.9 checked**: `note_block_change_sequence` drops negative sequences rather
   than panicking (`game.rs:2670-2676`); the ack-before-validation order is the
   recorded intent (a refused dig whose prediction is never closed would freeze the
   position). The vanilla-throws-on-negative detail matches the builder's read.
9. **§4.10 confirmed, partially re-measured**: the workspace gate re-run for this
   audit measured **1358 / 0 / 33 / 106** (the tree is unchanged since eaae11d, so
   the canonical figure stands); the per-crate lib counts and the "105 constants"
   claim were not independently re-counted this round — recorded, not smoothed.
10. **§2's extraction-regex finding (the auditor's own)**: the AUDIT-10 handoff's
    "the id map must be re-derived" claim rested on a GamePacketTypes-only regex that
    silently dropped the 8 packets registered from Common/Cookie/PingPacketTypes —
    the same silent-skip failure class the project's discipline warns about, found in
    the auditor's own instrument.

## The acceptance round (§3) — partial, recorded honestly

- **M-2 mining + pickup: CONFIRMED LIVE.** In a fresh visual-check session (rebuilt
  binaries) a survival dig broke the aimed block, **stayed broken** (the ack fix) and
  the drop was picked up into the hotbar.
- **Night rendering: confirmed again** (second independent session).
- **M-4 lookahead: live in the server log** — "mob step blocked by a fluid" /
  "mob step blocked; stopping" for skeletons and creepers.
- **M-1 death/respawn: attempted, not completed.** The `/tp` kill route failed —
  every tp position (y=200/250/130 at three columns) embedded the player in terrain
  (the world is taller than the amplitude estimate suggested, and the server does not
  model suffocation), and the zombie route needs the wander to bring a spawner into
  the 16-block aggro radius. The protocol half of M-1 is pinned (m_probes M-1 + the
  bytecode); the live client's death→respawn acceptance is still owed, with the
  experiment named.
- **M-3 subjective speed / pickup-in-view / restart persistence: not run this
  round.** Named experiments: the owner's eye for the chase speed; a pickup watched
  on screen; a server restart with a preserved world directory (run.py deletes it).

## What this audit fixed

Nothing in the tree — the landing survived the audit. The findings (N-1's reach
gap, N-1's ZERO-dig anomaly, §4.1's missing perturbation) are recorded here for the
builder's next pass.
