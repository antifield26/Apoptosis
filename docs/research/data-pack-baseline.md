# Data Pack Baseline — measured from the 26.1.2 jar (P07-03)

Date: 2026-09-11. Method: enumerate and parse the **official 26.1.2 server jar's own
data pack**, which ships inside `server-26.1.2.jar` as `data/minecraft/`. Nothing here
is recalled.

Reproduce with `tools/vanilla-probe/survey_tags.py` for the tag figures; the
file/type counts come from a straight `zipfile` enumeration of the same jar.

## 0. Extracting the pack

The differential test reads a directory rather than the jar, so an archive reader is
not part of the thing under test. The extractor is the snippet below — it is short enough to live in
this document, which is the honest place for it: an earlier version pointed at
`target/vanilla-26.1.2/extract_pack.py`, but that file was a byte-identical copy of the recount script and
contained no extraction code at all (*Audit 07, found during M3 remediation*). Save the snippet as
`extract_pack.py` beside the jar and run it from there:

```python
"""Extract data/minecraft/ from the 26.1.2 server jar."""
import os
import shutil
import zipfile

JAR = "server-26.1.2.jar"
PREFIX = "data/minecraft/"
OUT = "extract/data/minecraft"

if os.path.isdir(OUT):
    shutil.rmtree(OUT)
with zipfile.ZipFile(JAR) as archive:
    for info in archive.infolist():
        if info.filename.startswith(PREFIX) and not info.filename.endswith("/"):
            dest = os.path.join(OUT, info.filename[len(PREFIX):])
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with archive.open(info) as src, open(dest, "wb") as dst:
                shutil.copyfileobj(src, dst)
```

Then:

```text
set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract\data\minecraft
cargo test -p mc-data --test vanilla_pack -- --ignored --nocapture
```

**The value must be absolute** (AUDIT-09 D-06). The relative form
`target\vanilla-26.1.2\extract\data\minecraft` does not work as written: `cargo test`
runs the test binary with its working directory set to the *package* directory
(`crates/data`), so the test resolves the value against the wrong place and its
`root.is_dir()` guard fails with a message pointing at the directory it actually
looked in. `%CD%` expands to the repository root when the command is run from there;
in a POSIX shell use
`export MC_VANILLA_DATA="$PWD/target/vanilla-26.1.2/extract/data/minecraft"`, which is
the form `docs/testing/TEST-MATRIX.md` already uses.

The jar itself is obtained per `docs/research/protocol-baseline.md` section 1. Note the
distinction from the tools this project has *not* committed (Audit 05's reproducibility
gap): this script is reproduced in full above, so it is recoverable from the document
alone rather than from an uncommitted file.

## 0a. Counting method (a correction)

The first version of this document counted `zipfile` namelist entries, which include
**directory entries** — separate names ending in `/`. Every figure was therefore one too
high per directory. The loading test exposed it: the loader reported
1 421 + 94 = **1 515** recipes while the test expected 1 516, and the loader was right.

| Figure | First written | Measured precisely |
|---|---|---|
| `data/minecraft/` | 8 775 files | **8 282 files** (8 775 entries) |
| `recipe/` | 1 516 | **1 515** |
| `advancement/` | 1 633 | **1 617** |
| `loot_table/` | 1 353 | **1 326** |
| `structure/` | 1 359 | **1 202** |
| `worldgen/` | 1 029 | **951** |
| `villager_trade/` | 468 | **387** |
| `datapacks/` | 132 | **106** |
| `trade_set/` | 83 | **68** |
| `tags/` | 758 `.json` | **758** (correct) |

Reproduce with `tools/vanilla-probe/recount.py`, which filters on
`not name.endswith("/")`. The lesson is worth recording because it is the third time in
this project that a hastily-counted figure had to be corrected against a precise one —
and the first two were also caught by a test disagreeing with a document.

## 1. What a vanilla pack contains

Precise **file** counts (see section 0a for why the first version was wrong):

| Directory | Files | Loaded by `mc-data` |
|---|---|---|
| `advancement/` | 1 617 | no |
| `recipe/` | 1 515 | **7 of 21 types** |
| `loot_table/` | 1 326 | no |
| `structure/` | 1 202 | no |
| `worldgen/` | 951 | no |
| `tags/` | 758 | **yes** |
| `villager_trade/` | 387 | no |
| `datapacks/` | 106 | no (built-in packs) |
| `trade_set/` | 68 | no |
| `painting_variant/` | 51 | no |
| `damage_type/` | 50 | no |
| `banner_pattern/` | 43 | no |
| remainder | ~208 | no |
| **total** | **8 282 files** (8 775 zip entries) | |

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

### 2a. The registry of a tag file is a *known set*, not a path split

This cost two wrong attempts, so it is written down. A tag lives at
`tags/<registry>/<tag path>.json`, and the split point is **not recoverable from the
path**:

| File | Registry | Tag name |
|---|---|---|
| `tags/block/mineable/axe.json` | `block` | `minecraft:mineable/axe` |
| `tags/villager_trade/armorer/level_1.json` | `villager_trade` | `minecraft:armorer/level_1` |
| `tags/worldgen/biome/is_beach.json` | `worldgen/biome` | `minecraft:is_beach` |

`block` and `villager_trade` are one-segment registries whose tags have sub-paths;
`worldgen/biome` is a two-segment registry whose tags are flat. The directory shapes do
not distinguish them: `block/` holds 244 flat files and one subdirectory, while
`villager_trade/` holds **zero** flat files and fifteen subdirectories.

What settles it is the **references**:

- `tags/villager_trade/armorer/level_1.json` contains `#minecraft:common_smith/level_1`,
  which must be `tags/villager_trade/common_smith/level_1.json` — so the registry is
  `villager_trade`, not `villager_trade/armorer`;
- `tags/worldgen/biome/has_structure/buried_treasure.json` contains `#minecraft:is_beach`,
  which is `tags/worldgen/biome/is_beach.json` — so there the registry *is* two segments.

Splitting at the last separator produced **103** spurious missing-tag problems;
splitting at the first left **46**. `mc_data::tag::TAG_REGISTRIES` is the answer: 20
registry paths, longest-match first, with an unknown prefix falling back to the first
segment **and being reported** so a registry this build does not know is visible rather
than silently mis-split.

With the table, the real pack resolves with **758 of 758 tags and zero problems**.

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
