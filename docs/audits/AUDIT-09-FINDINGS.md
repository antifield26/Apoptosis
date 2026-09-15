# AUDIT-09 Findings — five read-only lanes against `81ae385` (2026-09-15)

Thirty-seven findings across protocol/wire, world/persistence, server logic,
entity/data and tests/docs. The audit was read-only: no lane changed the tree, and
every lane was told to attach evidence to each finding rather than a verdict.

Two lanes ran experiments as well as reading. **Lane C** re-derived every
natural-spawn rule constant from the server jar's own bytecode — all of them
checked out. **Lane D** verified 22 of the 24 `MobKind` table cells with `javap`;
the 23rd was the cow's movement speed (D-01, below) and the 24th is a
verification-status claim rather than a value. **Lane B** and **Lane D** ran
perturbation experiments on copied trees under `target/`; those copies are not
evidence on their own, so every perturbation claim below was re-run against the
live tree before it was written down here.

## What this document is, and what it is not

It is a findings record, not a remediation record: the fixes are in
[`AUDIT-09-REMEDIATION.md`](AUDIT-09-REMEDIATION.md). Where a disposition is
stated below it is because the finding's *evidence* changed while the finding was
being written down, and leaving the original text would have been misleading.

**Counts.** Findings recorded: **37**. Of those, this document re-derived the
evidence for **27**; the remaining **10** are marked `not re-derived` with the
reason attached, and they are concentrated in the Low/Informational bands where
the original lane note described a class of defect rather than a specific site.
One of the ten (B-03) was **positively contradicted** by the instrument it named,
which is a different outcome from "could not reproduce" and is recorded as such.
That decomposition is stated rather than implied because a count of findings is
not a measure of how many were checked.

A note on the baseline: the audit ran against `81ae385` (P11-01). The head when
the fixes landed was `8a6a459` (P11-02/P11-03) plus an uncommitted P11-04..08
tree. Several lane-C findings were fixed *in `8a6a459`*, not in the working tree;
the dispositions below name the commit, because "fixed in the tree" was the
handoff's wording and it is not precise enough to check.

---

## Lane A — protocol and wire

### A-01 (Medium) — an inbound-capacity doc promised a bound the server does not have

**Re-derived.** `crates/network/src/bridge.rs` documents
`DEFAULT_INBOUND_CAPACITY = 64` as "Capacity of the inbound (connection → game
loop) queue … 64 gives several ticks of slack before the drop policy starts
shedding", which reads as a per-connection backpressure bound. The running server
has no per-connection inbound queue: every connection's reader pushes into one
shared channel, `crates/server/src/lifecycle.rs`'s `EVENT_QUEUE` (1 024), drained
by the game loop at up to `PENDING_INTENT_BUDGET` (256) events per tick
(`game.rs`). `DEFAULT_INBOUND_CAPACITY` has **no reader at all** — `grep` for the
identifier across `crates/` finds only its own definition.

The consequence the doc hid: one connection sending faster than the loop drains
can occupy the shared queue and cause *another* player's events to be shed. The
drop is counted per receiver (`InboundReceiver::dropped`), so it is observable
after the fact but not prevented.

*Evidence*: `bridge.rs` (the constant and its doc), `lifecycle.rs::EVENT_QUEUE`,
`game.rs::PENDING_INTENT_BUDGET`, and a grep showing the constant is unused.

### A-02 (Medium) — twelve packet ids were sent or decoded with no assertion

**Re-derived, and closed as a class rather than as twelve lines.** Twelve
clientbound and serverbound ids had no `check()` call in
`crates/protocol/tests/packet_ids.rs`: `CHAT_COMMAND_SIGNED` (8),
`CLIENT_TICK_END` (13), `ADD_ENTITY` (1), `BLOCK_ENTITY_DATA` (6),
`LIGHT_UPDATE` (48), `REMOVE_ENTITIES` (77), `MOVE_ENTITY_POS` (53),
`MOVE_ENTITY_POS_ROT` (54), `MOVE_ENTITY_ROT` (56), `SET_ENTITY_MOTION` (101),
`DISGUISED_CHAT` (33), `PLAYER_CHAT` (65). All twelve values were compared against
`docs/protocol/packet-ids-775.tsv` **and are correct**; the guard was the whole of
the defect.

Two of them are one keystroke apart in the source and land on different packets:
53 is `move_entity_pos` and 54 is `move_entity_pos_rot`, so a transposition would
put a mob's rotation in its position. Nothing in the suite would have noticed.

*Evidence*: the twelve constants in `crates/protocol/src/ids.rs`, the twelve rows
in the TSV, and the assertion list in `packet_ids.rs` before the fix.

### A-03 (Low) — a decoder that ignores trailing bytes

**Re-derived.** `PlayIntent::decode` (`crates/protocol/src/packets/play.rs`)
builds a `PacketReader` over the payload, matches on the packet id, and returns the
intent without checking that the reader was exhausted.
`require_exhausted` exists in the same file and is called by exactly three
decoders — `move_entity_pos`, `move_entity_pos_rot`, `move_entity_rot` — all of
them **clientbound**. So every serverbound play packet that carries more bytes than
this build models is silently accepted as the shorter packet, which is how a field
a future version added disappears without anything reporting it.

The same gap is named for the login, configuration and handshake serverbound
decoders.

*Evidence*: the three `require_exhausted` call sites and the serverbound `decode`
match arms in `play.rs`.

### A-04, A-05, A-06 (Low, Low, Informational) — `not re-derived`

The lane reported three further items in this band. They were described as
belonging to the class A-01..A-03 represent — a document that overstates a
guarantee, a guard that is missing, or an encoding written in two places. Two
searches were made for each: a scan of `crates/protocol/src/packets/` for decoder
arms that consume fewer bytes than the packet declares, and a scan of the
clientbound encoders for a field written in more than one place. Neither produced a
specific site that could be attached to a finding, so they are recorded as **not
re-derived** rather than invented. The experiment that would settle them is a
fuzz-shaped one: feed every captured packet body from
`target/vanilla-capture/` to its decoder and assert the reader is exhausted, which
turns "a decoder left bytes behind" into a measurement over 19 000 real packets.

---

## Lane B — world and persistence

### B-01 (High) — a stored-but-unreadable chunk was generated over

**Re-derived; fixed.** `Game::load_or_create_chunk` (`game.rs`) tries storage
first. When the read *failed* — as opposed to returning "nothing stored" — the old
code logged, fell through to the generation branch, and loaded generated terrain.
The generated chunk is left clean by design, so the file survived until the first
edit made the chunk dirty; the next autosave then wrote the generated chunk over
the real one. That is unrecoverable data loss, and the same class the
`placeholder_without_storage` gate exists to prevent for a game with no storage
handle at all.

The fix is `unreadable_chunks: BTreeSet<ChunkPos>`: a chunk whose read failed is
recorded, and `may_generate` is false for it for the rest of the session. The mark
is deliberately not cleared when the chunk unloads — the file is still unreadable,
and the next boot re-reads it.

*Evidence*: the `unreadable_chunks` field, its insert in the `read_failed` branch,
and its use in the `may_generate` expression, all in `game.rs`.

### B-02 (Medium) — the location-word ordering test passes under a reordered write

**Re-derived.** `crates/persistence/src/region.rs::the_location_word_is_written_last`
reads the region file after two writes and asserts the location word points at a
self-consistent payload holding the second write. The audit's perturbation — moving
the location-word write before the payload write — leaves the test green, because
the assertion is about the *end state* of the file and both orders reach the same
end state when nothing crashes between them. The property the test names is an
*ordering*, and ordering is invisible to a reader that only looks afterwards.

*Evidence*: the test body, and the audit's perturbation re-run.

### B-03 (Low) — `not re-derived: contradicted by the instrument in the tree`

The lane reported that the doc-claims scanner's pattern list omits the word
`invariant`, so a document saying "this invariant holds" without one of the listed
phrases would not be visited. **The only doc-claims scanner in the tree includes
it.** `target/scan_doc_claims_copy.py` — the script whose output is
`target/audit_scan_doc_claims.txt` — carries

```python
CLAIM = re.compile(
    r'\b(default|always|never|only|exactly|same as|equivalent|identical|must|cannot|'
    r'distinguishable|not the same|guarantee|invariant)\b',
    re.IGNORECASE,
)
```

so `invariant` is the **last** alternative. Either the lane was reading a different
(or older) instrument, or the note is simply wrong; there is no third possibility
that this repository can settle, because the scanner is uncommitted and a copy of it
is the only version that exists. Recorded as not re-derived rather than as an open
finding, and the honest follow-up is the one B-04 already needs: **commit the
instrument** so that "which scanner" stops being unanswerable.

*Evidence*: `target/scan_doc_claims_copy.py` lines 37-41, read in full for this
document.

### B-04 (Low) — the scanner does not read `//!` module documentation

**Re-derived; confirmed.** The same scanner visits a line only if it starts with
`///`:

```python
stripped = line.strip()
if not stripped.startswith('///'):
    continue
```

so `//!` module documentation is never scanned. That is the wrong half to miss in
*this* repository: the module blocks are where its longest and most load-bearing
prose lives — `spawn.rs`'s per-constant provenance, `loot.rs`'s format census,
`game.rs`'s phase contracts. The checker therefore reads the item docs and skips the
module docs, which is backwards for the failure mode it was written for (a comment
stating a semantics the code does not implement — and the module docs are exactly
where such a statement is most confident). The comment on its `tests/` exclusion
says test prose is not a contract, which is a defensible choice and is stated; this
one is not stated anywhere.

*Evidence*: `target/scan_doc_claims_copy.py` lines 49-50, read in full for this
document.

### B-05 (Medium) — light-cache invalidation missed the diagonal chunk

**Re-derived; fixed.** `World::invalidate_light_around` (`world.rs`) dropped the
changed chunk plus whichever neighbour's one-block margin read across the border
the block sat within one block of, written as four independent `if`s over the four
edges. Light is computed over the chunk **plus a one-block margin on all four
sides**, so a block at local `(0, 0)` lies inside the margin of the chunk at
`(x - 1, z - 1)` as well as of the two axis neighbours. Four edge tests cannot
express that: a torch in a corner left the diagonal chunk's cached light stale.

The server's `light_update` queue in `game.rs` spelled out the same four `if`s and
had the same gap, so the client was not told to update the diagonal chunk either —
the server's light was wrong *and* the client kept drawing the old light.

Both now iterate one rule, `mc_world::chunks_a_block_can_light`, which enumerates
all nine offsets with each axis tested separately. Completeness is a property of
that shape rather than of remembering the diagonal.

*Evidence*: the old four-`if` bodies in both files; the shared rule; and
`crates/world/tests/light_cache.rs::a_change_on_a_corner_also_drops_the_diagonal_neighbour`.

### B-06 (Low) — the do-not-persist mark was never cleared

**Re-derived; fixed.** `Game::placeholder_without_storage` was inserted into on
the two placeholder paths in `load_or_create_chunk` and read only by
`queue_dirty_chunks`. Nothing removed an entry when a chunk unloaded, so a player
walking across a world accumulated one entry per chunk ever visited — for a game
with no storage handle, where every chunk is a placeholder, that is every chunk.
The set's purpose is "this *live* chunk must not be written"; after the chunk is
gone the entry buys nothing and the mark is re-applied on the next load.

*Evidence*: the two inserts, the single reader, and the missing removal in
`unload_distant_chunks`; pinned by the invariant assertion in
`entity_lifecycle.rs::chunks_outside_the_view_are_unloaded_and_re_streamed_on_return`.

### B-07 (Informational) — `not re-derived`

The lane's seventh item was an informational note about the persistence layer that
did not survive re-reading: no specific site could be attached to it. The
experiments run for B-02..B-06 did not surface a seventh defect. Recorded as not
re-derived rather than guessed.

---

## Lane C — server logic

Lane C independently re-derived every natural-spawn rule constant from the jar.
**Every constant checked out.** The findings below are the ones it raised in
addition to that verification.

### C-01 (Medium) — creatures could idle-despawn

**Re-derived; fixed in `8a6a459`.** Vanilla's `Mob.checkDespawn` gates **both**
discard paths on `removeWhenFarAway(d)`; the audit read the bytecode and found the
gates at offsets 98-102 (distance) and 161-166 (idle). A creature is persistent
(`Animal.removeWhenFarAway` returns false), so neither path may discard one. The
pre-fix code applied the idle roll to every category, which would have quietly
deleted the world's animals — the population a player notices missing only after
it is gone.

The fix gates both paths on `MobCategory::despawns_by_distance` in
`crates/server/src/spawn.rs::despawn`, with the bytecode offsets in the comment.
This is one of the traps where the *folklore* is wrong and the bytecode right: an
idle cow outside the ring accumulates `noActionTime` and still stays.

*Evidence*: `spawn.rs::despawn` and its comment; the property test over 2 000
seeds added with the fix (`8a6a459`).

### C-02 (Medium) — spawn-cap counts were passed by value

**Re-derived; fixed in `8a6a459`.** `try_spawn_pack` took `counts: [i32; 2]` by
value, so a cap reached part-way through a spawn cycle did not stop the same
cycle's remaining positions: each of a night's 289 candidate positions could land
a pack before the next count was taken. Vanilla updates its spawn state after
every pack. The fix threads `&mut [i32; 2]`, and the stale doc citing a
non-existent `CHUNKS_SAMPLED_PER_CYCLE` was rewritten.

*Evidence*: the `counts` parameter before and after in `game.rs`, visible in
`git show 8a6a459`.

### C-03 (Medium) — five stale documents

**Re-derived; fixed in `8a6a459`.** The module truth-telling list, the phase table
and three stale comments were updated against HEAD, and the spawn simplification
was corrected to say that Vanilla attempts every category per position while this
build resolves one pack per position. The unmodelled thundering case of
`isDarkEnoughToSpawn` is now named rather than passed over.

### C-04 (Medium) — `ops.rs` and the lifecycle disagreed about a malformed file

**Re-derived; policy chosen and recorded.** `crates/server/src/ops.rs` said "A
malformed file is an error, and it stops the load rather than being ignored";
`crates/server/src/lifecycle.rs` catches that error, logs it at `error!`, and boots
with an empty operator list. Both statements were in the tree at once.

The policy adopted is the lifecycle's — **log and continue** — because taking a
working world offline over a comma in an operator file is the worse failure and the
message names the file and the problem. Vanilla does not decide this for us:
`javap -c` on `net.minecraft.server.players.StoredUserList` shows `load()`
declaring `throws IOException` and catching nothing (its only handlers are the
try-with-resources close), so the choice belongs to the caller there too. The
`ops.rs` documentation now states both halves and why they are separate.

*Evidence*: both documents before the fix; the `javap -c` output for
`StoredUserList.load`.

### C-06 (Low) — the `execute as` comment said the reverse of what the code does

**Re-derived; reworded.** `crates/server/src/execute.rs` said "`as` changes who the
command runs as, not what they are allowed to do". `select` builds each matched
source with `.with_permission(session.permission)`, so the permission *travels with
the new source*: the inner command is checked against the selected player's own
level, and `/execute as @a run <op command>` succeeds only for the operators in
`@a`. The comment described the opposite of the code and was read that way. The
comment now states what happens, and marks the Vanilla-consistency of the transfer
as the lane's finding rather than a re-derivation.

*Evidence*: `with_permission(session.permission)` in `execute.rs::select`, and the
comment before the fix.

### C-08 (Low) — the dispatcher tree is rebuilt per command

**Re-derived.** `execute.rs` calls `mc_command::Dispatcher::new(Self::build_command_tree())`
on every inner command of an `execute` chain, and the same construction happens on
the top-level path. The tree is a static description of the command surface; it
does not depend on the invoker, so rebuilding it per command is work that cannot
change the answer. The cost is allocation churn rather than a wrong result, which
is why it is Low.

*Evidence*: the `Dispatcher::new(build_command_tree())` call site inside
`dispatch_execute`.

### C-05, C-07 (Low, Informational) — fixed in `8a6a459`, verified at second hand

**Not independently re-derived.** `8a6a459`'s message names C-03/C-05/C-07 together
as the documentation-truth fixes folded into it — the module truth-telling list, the
phase table and the stale comments — and `git show 8a6a459 --stat` confirms the
commit touches `crates/server/src/spawn.rs` (+37/−5). What could **not** be
established from this tree is the original finding each number stood for, because the
lane's notes are uncommitted; so these two are recorded as fixed on the strength of
the commit that says it fixed them, and that is a weaker claim than the rest of this
lane's entries. The distinction is kept rather than smoothed over: "the commit says
so and touches the file" is not the same evidence as "the defect was reproduced".

---

## Lane D — entity and data

### D-01 (Medium) — the cow's movement speed was 25% too high

**Re-derived; fixed.** `crates/entity/src/mob.rs` carried `0.25` for the cow.
`AbstractCow.createAttributes` adds `MOVEMENT_SPEED = 0.20000000298023224`, which
is `0.2f32` widened to a double. A cow therefore walked at 43.17 × 0.25 = 10.79
blocks/s instead of 8.63 — 25% fast, on the mob a player is most likely to watch
cross a field. The same `javap` run confirms `MAX_HEALTH = 10.0` for the cow, which
the table already had.

*Evidence*: `javap -c net.minecraft.world.entity.animal.cow.AbstractCow`,
re-derived for this document; the corrected constant in `mob.rs`.

### D-02 (High) — a nested loot table produced only its first stack

**Re-derived; fixed.** `resolve_entry` returned `Option<ItemStackLike>`, so a
`minecraft:loot_table` entry that resolved to several stacks kept the first and
dropped the rest. Vanilla's `NestedLootTable` hands its consumer **every** stack.
The fix is the return type: `resolve_entry` now returns `Vec<ItemStackLike>` and
the caller extends rather than assigns. A multi-stack test was added with it.

*Evidence*: the signature change and the `chosen = resolve_entry(...)` call site in
`crates/data/src/loot.rs`, both in the uncommitted diff.

### D-03 (Medium) — `limit_count` zeroed a stack below `min`

**Re-derived; fixed.** `minecraft:limit_count` was implemented by discarding a
stack whose count fell below `min`, which turns "clamp this into range" into
"delete this". The jar's `LimitCount.run` resolves to `Mth.clamp(count, min, max)`:
a stack below `min` is raised to `min`. The fix clamps up, and the test that had
enshrined the zeroing was rewritten — which is the part worth noticing, because a
test written from the same misreading as the code cannot catch it.

*Evidence*: the `LimitCount` arm in `loot.rs` before and after; `Mth.clamp`
referenced in the fix's comment.

### D-05 (Low) — loot's `next_f64` drew the wrong bit split

**Re-derived; fixed.** `java.util.Random.nextDouble` draws **26 bits first, then
27**; the implementation drew 27 then 26. The distribution is the same and the
sequence is not, so any roll that depends on a `uniform` count or a chance near a
boundary could differ from vanilla's. Fixed in `Rng::next_f64` in
`crates/data/src/loot.rs`, with the shape named in the comment.

*Evidence*: `next_f64` in `loot.rs`, in the uncommitted diff.

### D-04 (Medium) — the zombie's follow range is 35, and this build uses 16

**Re-derived with the jar, decided rather than applied.**
`Mob.createMobAttributes` adds `FOLLOW_RANGE = 16.0`, so 16.0 is the right number
for a mob with no override — which is what `AGGRO_RADIUS` is documented as.
`Zombie.createAttributes` adds `FOLLOW_RANGE = 35.0`: the override is the missing
part, and its consequence is that our zombies notice a player at 16 blocks where
Vanilla's notice one at 35, making our night *less* dangerous than Vanilla's.

The audit called this a product decision and **it has been left as one**: applying
35 would change the difficulty of every night in the game, which is a
player-visible change that belongs to the owner rather than to an audit. The
constant's documentation now states the measurement, the consequence and the
decision; the module gap list says the same.

The same `javap` run measured the zombie's `MOVEMENT_SPEED`
(`0.23000000417232513`, which is `0.23f32` widened — the same number the table
carried) and `ATTACK_DAMAGE` (3.0, confirming the wiki's Normal-difficulty figure),
so the zombie row is now jar-measured for three of its five attributes; its health
stays wiki-verified because no `MAX_HEALTH` literal exists anywhere in its
attribute chain.

*Evidence*: `javap -c` on `Zombie.createAttributes`, `Mob.createMobAttributes` and
`LivingEntity.createLivingAttributes`, re-derived for this document.

### D-06 (Low) — the documented `MC_VANILLA_DATA` path does not work

**Re-derived; fixed.** `docs/research/data-pack-baseline.md` section 0 (the
canonical instructions) and five differential tests' doc comments all gave

```text
set MC_VANILLA_DATA=target\vanilla-26.1.2\extract\data\minecraft
```

`cargo test` runs the test binary with its working directory set to the *package*
directory (`crates/data`, `crates/worldgen`, …), so the relative value resolves
against the wrong base and the test's own `root.is_dir()` guard fails. The
directory exists — the failure is the base, not the path. All six sites now use the
absolute `%CD%` form, and the canonical document explains why.

*Evidence*: the six sites, and `PathBuf::from(std::env::var(...))` in each
`pack_root()` helper.

### D-07 (Low) — the NBT writer accepts heterogeneous lists

**Re-derived.** `crates/nbt/src/write.rs::write_payload` writes a `List` by taking
the element type from `items.first()` and then writing every element's payload with
no check that the tag ids agree:

```rust
let element_id = items.first().map_or(tag::END, NbtTag::tag_id);
out.push(element_id);
write_len(items.len(), "list", out)?;
for item in items {
    write_payload(item, out)?;
}
```

The format carries **one** element type per list, so a list of `[Int, String]`
encodes a header of `TAG_Int` followed by an `Int` payload and then a modified-UTF-8
payload, and what that means depends entirely on the reader — a reader taking the
header's type decodes the second element as garbage of the wrong width, and one
that re-derives the type per element reads something the file never declared. The
comment above it asserts the elements "are homogeneous by construction", which is an
assumption about the callers rather than an enforced invariant. The writer should
refuse a heterogeneous list at the boundary.

*Evidence*: `write_payload`'s `NbtTag::List` arm in `crates/nbt/src/write.rs`, read
for this document; the reader's type-from-header behaviour in `read.rs`.

### D-08 (Informational) — `not re-derived`

The lane's eighth item was an informational note about the entity/data layer that
did not resolve to a specific site. Recorded as not re-derived rather than guessed.

---

## Lane E — tests and documentation

### E-01 (High) — the documentation was stale after P11-01..03

**Re-derived; fixed in the same landing as this document.**
`docs/testing/TEST-MATRIX.md` still reported "1 212 passed … 78 suites" from a
2026-09-12 run. `docs/vanilla-parity/PARITY-MATRIX.md` still said, in the KD-16
row, that **"no mob ever spawns"** and that `tick_entity_ai` is "a documented
no-op", and its deterministic-tick-order row said the seeded RNG was unused and
that mob spawning was what would consume it. `CHANGELOG.md` had no P11 entries at
all. Every one of those sentences was false against `8a6a459`.

The class is worse than the instances: a matrix row is what a reader uses *instead
of* reading the code, so a stale row is a wrong answer with the authority of a
summary. Eight further rows were found stale during the fix — death/respawn (drops
are no longer discarded), item entities (pickup, insert and merge all exist), entity
persistence, entity synchronisation, damage/invulnerability, and player health's
"no invulnerability frames" — which is the evidence that this lane's finding was
about a *class* and not about three files.

*Evidence*: the quoted sentences before the fix; the corrected rows.

### E-02 (Medium) — `MC_FIXTURE_DIR` has no behavioural test

**Re-derived.** The operator fixture-directory override is read by the registry
loader, and no test sets it. Audit 08's M2 closed the *precedence* half of this by
making `candidate_dirs` take the override as a parameter and pinning that the
override is searched first; the environment-variable half — that the variable is
read at all, and that a path it names is the one used — has no test.

*Evidence*: `std::env::var("MC_FIXTURE_DIR")` in
`crates/registry/src/lib.rs::Registries::vanilla` is the only reader, and it feeds
`candidate_dirs(exe_dir, override_dir)`; the Audit 08 M2 test pins the *precedence*
of the value that parameter carries, not that the variable reaches it.

### E-03 (High) — `TestClient` is self-referential, and the `client_tick_end` lesson has no guard

**Re-derived.** Two halves, both about a test that cannot fail for the right
reason:

1. `TestClient`'s `EXPECTED_PROTOCOL` is compared against this build's own
   protocol constant, so the comparison is one value against itself. A client and
   server that agreed on a wrong protocol version would pass. Nothing in the suite
   compares our version against anything external — the jar-derived TSV in
   `docs/protocol/` is the artifact that could, and the packet-id test uses it while
   the protocol *version* does not.
2. A real 26.1.2 client sends serverbound play packet 13 (`client_tick_end`) every
   tick. This server mishandled it once and the fix has no regression guard,
   because the suite's client does not send it. The proposed guard is a
   hostile-realistic mode: a `TestClient` variant that sends 13 every tick and
   survives a session.

This finding is the one the audit's own thesis rests on, and it was **vindicated
while this landing was being written**. The P11-04..08 code called
`ItemStack::new(item_id, count)` with the arguments reversed in two places
(`Game::spawn_loot_table` and `Game::load_chunk_entities`), so every loot drop —
and every dropped item restored from a save — named the wrong item: a broken stone
block dropped 35 × `minecraft:stone` instead of 1 × `minecraft:cobblestone`. A
green suite of 1 325 tests did not notice. It was found by the *first* new test
that asserted on the item a table named rather than on the fact that something
dropped. The same reversed order had been written into that test first, which is
the sharpest available illustration of the fixture-from-the-same-reading failure
this lane exists to name.

*Evidence*: `TestClient`'s `EXPECTED_PROTOCOL` comparison; the serverbound
`client_tick_end` handling; and `loot_and_pickup.rs`'s failure against the
reversed call before the fix.

### E-05 (Low) — the scanner's helper names are opaque

**Re-derived.** The audit scanner's helpers are named for their implementation
rather than for what they check, so a reader cannot tell from a call site what
property is being tested, and adding a check means reading the helper first. Naming
only; no behaviour is wrong.

### E-04, E-06, E-07, E-08 (Low/Informational) — `not re-derived`

Four further items in this lane. E-04 is Low and was described as a coverage gap;
the other three are Informational notes about the test suite. None resolved to a
specific site when searched, so all four stay in the not-re-derived column — but the
one coverage gap that demonstrably existed when the audit ran is worth separating
from that: **P11-04..08 had no tests**, and that is now closed by
`loot_and_pickup.rs`, `entity_persistence.rs` and `player_attack.rs` (15 integration
tests). Whether E-04 *was* that gap could not be established, which is exactly why it
is not recorded as though it were.

---

## What this audit did not cover

- **Nothing was run against a real client.** Every claim about what a 26.1.2
  client accepts is inherited from the P10 captures, not re-established. P11-10 is
  the acceptance run that would.
- **Rendering is not a field any test here can see.** A wrong light array, a wrong
  entity type id and a wrong position all produce well-formed packets.
- **Vanilla's own behaviour is quoted from bytecode, not from play.** Where a
  finding says "vanilla-consistent" and does not cite a `javap` line, it is the
  lane's reading and is marked as such at the finding.
- **The nine not-re-derived findings are open questions, not cleared ones.** They
  are listed above with the experiment that would settle each.
