# P17-05 Adversarial Review (agent half)

Independent of the implementation pass. Every claim in the owner-session
follow-up is a row: what would settle it, the verdict, and the evidence.
A refuted claim is a result. Counts are decomposed (N compared / M skipped
with a named reason).

## Instrument

- Owner real-client session: `docs/testing/P17-BUILD-SESSION.md` (verdict
  sheet filled 2026-09-23).
- Automated: `cargo test -p mc-container --lib` (135), `cargo test -p
  mc-server --lib` (67), `cargo test -p mc-redstone --test vanilla_observer`
  (3), `vanilla_pack` / `vanilla_crafting` / `vanilla_smelting`
  differentials against `target/vanilla-26.1.2`.
- Code read for every wire/semantic claim (`path:line` where it matters).

## Claim table

| Claim | Instrument | Verdict | Evidence |
|---|---|---|---|
| Creative takes work | owner retest after `c66f1d2` | confirmed | `set_creative_mode_slot` decode + `apply_creative_slot`; owner: takes stick |
| Door panel toward the player (A1) | owner retest after `08cd8b9` | confirmed | `DoorBlock.getStateForPlacement` = `getHorizontalDirection().getOpposite()` and that helper is `look.getOpposite()`; owner: facing pass |
| Double-door pair opens together | owner retest after `d95ce2a` | confirmed | `toggle_paired_door` walks both halves from the lower y |
| Iron door places continuously | owner retest after `80d4558` | confirmed | refused hand toggle falls through to `place_held_block` |
| Button pulses, does not stick on | owner retest after `ae94f4b` | confirmed | place writes `powered=false` and skips `redstone_feed`; press schedules unpress |
| Wall lever/button attach to the clicked face | owner retest after `2984341` | confirmed | `face_facing(face_id)` for wall; look-derived facing was the snap |
| Sneak places onto containers | owner retest after `a9b1c63` | confirmed | `player_input` bit 5; `is_sneaking` gates the container/door arms |
| Hopper output follows the clicked face | owner retest after `ebf1f19` | confirmed | `clickedFace.getOpposite()` map |
| Chest under a hopper opens | owner retest after `ebf1f19` | confirmed | cover test is `is_full_cube`, not `is_solid_or_unknown` |
| Chest has a texture after reconnect | owner retest after `777a4e0` | confirmed | chunk `block_entities` populated; `packed_xz` is u8 (two bytes shifted the packet and the client refused it) |
| Inventory clicks stick | owner retest after `7753e03` | confirmed | `bump_state` on every accepted click; test contract updated |
| Crafting leftovers return | owner retest after `f1582fa` | confirmed | 2×2 + 3×3 return on close; close force-sends window 0 (delta vs a mirrored menu was empty) |
| Furnace flame and front | owner retest after `7753e03` / `08cd8b9` | confirmed | `lit` flips with burn; furnace `facing = opposite(look)` |
| Old ground stacks survive a merge | owner retest after `747836b` | confirmed | `age >=` keeps the older; survivor count rides `set_entity_data` |
| Thrown items do not jitter | owner retest after `dbf8a7d` | confirmed | `add_entity` movement = `entity.velocity` |
| Chest open/close animation | owner: absent | **refuted as present** | `block_event` unmodelled — named gap, not hidden |

## Decomposition

- Owner findings acted on: 14 named. Confirmed fixed by owner retest: 13.
  Named open: 1 (chest animation). Skipped 0.
- Automated gate at review time: 135 + 67 + 3 lib/integration tests green;
  three vanilla differentials green. Compared 4 suites / skipped 0.
- Wire/semantic claims checked against jar-captured fixtures or Pumpkin
  `path:line`: 8. Checked 8 / skipped 0.

## Findings this review raised (and how they closed)

1. **Menu revision skipped no-ops** — the real client advances its counter
   on every click; the next click was eaten as stale (place failures, ghost
   copies). Fixed in `7753e03`; the old unit assertion ("a no-op must not
   bump") was inverted to match the observed client.
2. **`packed_xz` was u16** — the chunk block-entity list was two bytes per
   entry too wide and a real client refused `level_chunk_with_light`.
   Fixed in `777a4e0` against Pumpkin `ChunkBlockEntity.packed_xz: u8`.
3. **Ground merge kept the younger stack** — the comment said "older
   survives" and the comparison did the opposite. Fixed in `747836b`.

## Verdict

P17-05's automated and owner halves are consistent with the claims made
in CHANGELOG. The one refuted-as-present item (chest animation) is named.
No silent gaps. P17 exit gate is satisfied on the evidence above.
