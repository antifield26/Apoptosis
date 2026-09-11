# Data Pack Baseline — measured from the 26.1.2 jar (P07-03)

Date: 2026-09-11. Method: enumerate and parse the **official 26.1.2 server jar's own
data pack**, which ships inside `server-26.1.2.jar` as `data/minecraft/`. Nothing here
is recalled.

Reproduce with `target/vanilla-26.1.2/survey_tags.py` for the tag figures; the
file/type counts come from a straight `zipfile` enumeration of the same jar.

## 1. What a vanilla pack contains

| Directory | Files | Loaded by `mc-data` |
|---|---|---|
| `advancement/` | 1 633 | no |
| `recipe/` | 1 516 | **7 of 21 types** |
| `structure/` | 1 359 | no |
| `loot_table/` | 1 353 | no |
| `worldgen/` | 1 029 | no |
| `tags/` | 800 (758 `.json`) | **yes** |
| `villager_trade/` | 468 | no |
| `datapacks/` | 132 | no (built-in packs) |
| `trade_set/` | 83 | no |
| remainder (33 directories) | ~250 | no |
| **total** | **8 775** | |

## 2. Tags — the one format fully measured

```text
tag files                    758   across 17 registries
nested (#other) references   384   ⇒ transitive resolution is the common case
deepest nesting                4   (block/supports_crimson_fungus)
cycles                         0   ⇒ the detector is exercised only by our tests
"replace": true                0   ⇒ implemented, UNVERIFIED against vanilla
{"id": …, "required": false}   0   ⇒ implemented, UNVERIFIED against vanilla
```

Registries by file count: `block` 248, `item` 207, `worldgen` 92, `villager_trade` 73,
`entity_type` 47, `damage_type` 33, `enchantment` 24, `banner_pattern` 13, `fluid` 7,
`game_event` 6, `timeline` 5, `instrument` 4, `point_of_interest_type` 4, `dialog` 3,
`painting_variant` 2, `potion` 2, plus one without a subdirectory.

The file format:

```json
{
  "values": [
    "minecraft:oak_planks",
    "#minecraft:wooden_slabs",
    { "id": "minecraft:cherry_planks", "required": false }
  ]
}
```

A `#`-prefixed string is a reference to another tag **in the same registry** — the
format has no way to name a different registry from a value, which is why the registry
is inherited from the containing tag's path rather than parsed.

## 3. Recipes — 21 types, 7 modelled

| Type | Files | Modelled | Why not |
|---|---|---|---|
| `crafting_shaped` | 708 | yes | |
| `crafting_shapeless` | 322 | yes | |
| `stonecutting` | 275 | yes | |
| `smelting` | 73 | yes | |
| `crafting_transmute` | 33 | **no** | new in 26.1; semantics not verified |
| `blasting` | 25 | yes | |
| `smithing_trim` | 18 | **no** | needs a smithing menu |
| `crafting_special_bannerduplicate` | 16 | **no** | hard-coded behaviour, not data |
| `smithing_transform` | 12 | **no** | needs a smithing menu |
| `campfire_cooking` | 9 | yes | |
| `smoking` | 9 | yes | |
| `crafting_dye` | 6 | **no** | new in 26.1; semantics not verified |
| `crafting_special_*` (book cloning, firework, map, repair, shield) | 5 | **no** | hard-coded behaviour |
| `crafting_decorated_pot`, `crafting_imbue` | 2 | **no** | new in 26.1 |

Every skipped type is **counted** in `RecipeLoadReport::unmodelled`, so a pack whose
recipes did nothing is distinguishable from one that failed to load.

Two real values that replace Phase 06's guessed ones, both confirmed verbatim:

```json
// data/minecraft/recipe/iron_ingot_from_smelting_raw_iron.json
{ "type": "minecraft:smelting", "cookingtime": 200, "experience": 0.7,
  "ingredient": "minecraft:raw_iron", "result": { "id": "minecraft:iron_ingot" } }
```

## 4. The four cooking types share one shape

`smelting`, `blasting`, `smoking` and `campfire_cooking` differ only in the block that
performs them, so `mc-data` parses them with one struct (`CookingRecipe`) and a
`SmeltingKind` discriminator rather than four near-identical parsers.

The observed `cookingtime` per type (from the jar's own files):
`blasting` 100, `smoking` 100, `campfire_cooking` 100, `smelting` 200. Vanilla **always**
states the field, so the defaults in `SmeltingKind::default_cooking_time` are a fallback
for a pack that omits it and are labelled as not vanilla-exercised.

## 5. Pack metadata

`pack.mcmeta` carries `{"pack": {"pack_format": N, "description": …}}` plus optional
`filter` and `overlay` sections. `mc-data` reads `pack_format` and a **string**
description; a translatable-component description (`{"translate": …}`) is reported as
absent rather than flattened to a wrong string. `filter` and `overlay` are not modelled,
and `pack_format` is reported but not enforced — the accepted range for 26.1.2 is not
verified.

## 6. What this baseline does not cover

- **`.zip` packs.** Directory packs only; a zip needs an archive reader (new dependency
  + licence review). No world on this machine uses one.
- **The world's enabled-pack list.** `level.dat → DataPacks` is not read, so every
  discovered pack loads rather than only the enabled ones.
- **`data/minecraft/datapacks/`** (132 entries) — the built-in packs a new world gets.
  They are inside the jar and are loaded by the same mechanism when present on disk,
  but nothing extracts them.
- **Loot tables, advancements, functions, predicates, item modifiers, worldgen.** Not
  loaded; listed in `DATA_DIRECTORIES` as intent and absent from the parity claims.
