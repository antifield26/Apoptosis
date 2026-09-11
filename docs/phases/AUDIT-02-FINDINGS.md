# Audit 02 — Phases 00–04 re-verification (independent)

Date: 2026-09-11. Method: two **independent adversarial subagents** with read-only
scope (no write access to the workspace), checking claims against code rather than
against other documents, plus direct verification by the primary agent of every
finding before acting on it. This is the second audit pass; the first
(`AUDIT-01-FINDINGS.md`) covered Phases 00–02 in depth.

## 0. Why a second pass

`EXECUTION-LOOP.md` §7 and `AGENTS.md` §3.1 require evidence rather than
assertion, and a phase report written by the same agent that wrote the code is the
weakest possible evidence for its own correctness. Two things were therefore
checked by parties who did not write the code:

- **Audit A** (`docs/phases/PHASE-04-REPORT.md` claims): every P04 task, plus the
  ability to *defeat* the security-relevant logic.
- **Audit B** (claim-vs-reality across all phases): whether each documented claim
  survives contact with the code, and whether Audit 01's ten promised fixes
  actually landed.

## 1. What the audits confirmed

Audit A re-derived **all 29 873 block-state ids** from `blocks.tsv`'s mixed-radix
axes and diffed them against the verbose jar dump: **0 mismatches, 0 missing**,
and `items.tsv` row-identical to its dump. It tried to construct a state the
parser would decode wrongly (value-order and axis-order permutations) and could
not. That is stronger evidence for P04-01 than the phase report claimed.

Audit B reproduced the test count exactly (396 / 0 / 3) and the per-crate
breakdown, verified all six `PHASE-04-REPORT.md` §6 artefact paths, confirmed the
registry fixture counts, and confirmed all ten Audit 01 **code** fixes are present.

Two things were verified by measurement rather than reading:

- `RandomSource` was checked against the **real JDK** (`java.util.Random` on JDK
  25, `RandomProbe.java`) — see §3.1.
- The audit re-ran the tick baseline and reproduced the same order of magnitude
  (p50/p95/p99 = 0.75/1.33/2.04 ms, max 3.93, save 40.8 s) but not the exact
  figures in the report.

## 2. Defects the audits found (all fixed)

### 2.1 Critical — data loss: placeholders were saved over real terrain

`Game::send_chunk` → `World::ensure_chunk` → `Chunk::air` sets `dirty: true`, and
`Game::save_all` persists every dirty chunk. Nothing in `crates/server/src` ever
called `read_chunk` (grep: 0 hits). So a second run recreated the same chunk
position as all air and **wrote it over the real chunk on disk**; existing terrain
never appeared in game either.

This is the single most serious defect found in the project so far, and it
invalidated the P04-17 "survives restart" claim: the test read the chunk straight
out of storage and set its blocks with `world_mut().set_block`, bypassing `Game`
entirely.

**Fix**: a disk→world load path (`Game::load_or_create_chunk`) that reads the
chunk first, converts it with `Chunk::from_chunk_data`, clears the dirty flag, and
only falls back to a **non-dirty** placeholder when the chunk genuinely does not
exist or the read failed. Plus a real restart test that goes through `Game`.

### 2.2 Critical — remote denial of service via one block placement

`apply_use_item_on` called `self.world.set_block(..)?`; for an out-of-range `y`
that returns `InvalidAction`, which propagated out of `Game::tick()` and up to
`lifecycle.rs`, terminating the server process. It was latent only because an
empty hand bailed out earlier. **Fix**: validate the target's `y` against the
world's build range before touching the world and ignore the action instead of
erroring — hostile input must never take the server down (`AGENTS.md` §9/§10).

### 2.3 Saturation could go negative

`tick_food` clamped `saturation` to a *ceiling* and then did `saturation -= 1.0`,
leaving a fractional saturation negative — reachable every tick for a full player
regenerating at partial saturation, after which regeneration stopped and
`is_regenerating()` misreported. **Fixed** with a floor at zero.

### 2.4 XP formula could panic on a hostile save file

`experience_needed_for_level` computed `9 * level - 158`, which overflows `i32`
above ~2.4e8. `XpLevel` comes from persisted NBT, so a hand-edited file could
panic a debug build. **Fixed** with saturating arithmetic. The *level cap* itself
is still not enforced — its vanilla basis could not be verified, so it is recorded
as a gap rather than guessed.

### 2.5 Collision solver had no finiteness guard

Defence in depth: a non-finite delta or box would poison the clip arithmetic and
could leave an entity at NaN, where every later comparison is false. Unreachable
today (the caller rejects such input), but it is the single choke point every
mover passes through. **Fixed** at the solver.

### 2.6 Chunks were never unloaded

`World::unload_chunk` had no caller; each retained placeholder is ~384 KiB, so a
player walking in a line accumulated them without bound. **Fixed** with
view-distance-based unloading plus re-streaming on return.

### 2.7 A tautological test assertion

`network_game_bridge.rs` asserted `after != before || player.is_some()`, always
true for a live player, proving nothing about movement. **Fixed** to assert a
falsifiable property.

### 2.8 Claim-versus-reality mismatches (documentation)

| Claim | Reality | Correction |
|---|---|---|
| "§0 saves the edited world so a restart reads the same blocks back" | No restart path read anything (§2.1) | Rewritten after the fix landed |
| "P04-17 place two diamond blocks **through gameplay**" | Called `set_block` directly | Test now goes through the gameplay path |
| "§5.12 `Game::flush` is a no-op counter" | No `Game::flush` exists | Replaced with the real gap: no separate broadcast phase yet |
| "§5.10 `container_click` decoded but not applied" | Not decoded at all | Corrected |
| "§1 13 packets added" | 14 named, 15 present | Corrected |
| "§4 join + `stream_all` both sent chunks" | `join` sends none | Corrected |
| `PARITY-MATRIX` login tolerance `full` | Code existed, zero tests | Downgraded to `partial`; the missing tests were then written |
| `DEPENDENCY-POLICY` "92 packages" | 90 | Corrected |
| `PHASE-01` "`Cargo.lock` committed" | No commits exist | Corrected |
| `PARITY-MATRIX` header "Phase 03 update" | Contains P04 rows | Bumped |
| `TEST-MATRIX` "`ray::tests` (11 cases)" | 12 | Corrected |
| Audit 01 §2.8 doc half: policy not cross-referenced | Only one-way link | Reverse link added to `third-party.md` |
| Audit 01 T05/T07 cited tests that never sent the packets | No such coverage | `crates/network/tests/login_tolerance.rs` written |

## 3. Claims the primary agent got wrong and corrected

### 3.1 My own "armour permutation is inverted" fix was wrong

Audit A reported that `to_container_payload` mapped stored slot 36 to client slot
5, "putting boots in the helmet slot", and I initially accepted it and reversed
the mapping. Before committing to that, I disassembled the official jar:

```
javap -c net.minecraft.world.inventory.InventoryMenu   (static initializer)
  SLOT_IDS = [EquipmentSlot.FEET, LEGS, CHEST, HEAD]
  constructor loop i = 0..3:
    ArmorSlot(inventory, owner, SLOT_IDS[i], 39 - i, 8, 8 + i*18, icon)
    -> menu index 5 + i holds SLOT_IDS[i]
  ARMOR_SLOT_START = 5, ARMOR_SLOT_END = 9,
  USE_ROW_SLOT_START = 36 (hotbar), offhand = InventoryMenu$1 at menu 45
```

So menu 5 is **FEET** and menu 8 is **HEAD**. Our storage is documented
boots-first (36..39 = boots, leggings, chestplate, helmet), so stored 36 → menu 5
is **correct** — the reversal would have introduced the very bug it reported. The
change was reverted, the constants `ARMOR_MENU_START`/`OFFHAND_MENU_SLOT` were
added to name the two index spaces explicitly, and the test now derives the
expected mapping from those constants instead of restating the implementation's
formula.

**Lesson recorded**: an audit finding is a hypothesis, not a fact. Both of this
phase's "audit found X, fix it" mistakes would have been shipped without checking
the primary evidence.

### 3.2 The `nextLong` bug in my own new code

`RandomSource::next_i64` treated Java's two `next(32)` halves as unsigned, but Java
sign-extends each. Caught by the golden-vector test against the real JDK. My
`nextInt`-based rejection sampling was also wrong: I evaluated the bias check in
`i64`, where it can never be negative, so the rejection could never fire.

## 4. Deliberately not changed

- **Tick-baseline exact figures.** Reproducible only to the same order of
  magnitude; the report now quotes the re-measurement and says the numbers are
  host-load dependent rather than presenting one run as *the* number.
- **`MAX_SATURATION = 5.0`.** Not vanilla's cap, but no caller depends on it and
  correcting it requires the saturation-restoration table, which is not verified.
  Recorded as a gap.
- **Vanilla's collision axis order** is Y→X→Z; ours is X→Y→Z. Observable only in
  rare corner cases. Recorded in the parity matrix rather than changed blind.
- **`ray.rs`'s claim that an unloaded block stops the ray** does not match the
  code, but the function has no production caller. Recorded, not fixed.

## 5. Standing gaps carried forward

1. ~~The repository still has **no commits**~~ — resolved by `b7b1c99` (2026-09-11). CI has now run for the first time on that commit; the result is recorded in `PHASE-06-REPORT.md`.
2. No real 26.1.2 client, so "real client" exit-gate clauses stay qualified.
3. Reference clones have no `.git`, so pinned SHAs remain unrecordable.
4. A runbook is still absent (`docs/operations/` holds only the dependency
   policy); it is P08-15 and its absence is recorded rather than hidden.
