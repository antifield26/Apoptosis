"""Extract the synced registries a 26.1.2 client requires, from the jar's own data pack.

P10-02 measured what the client refuses on. Its protocol-error report names both problems, and both trace back
to fields `registry_data.rs` already declares on the overworld `dimension_type`:

    timelines:     "#minecraft:in_overworld"      -> the tag `minecraft:in_overworld` must be bound
    default_clock: "minecraft:overworld"          -> the `minecraft:world_clock` registry must contain it

So the schema was right and the *registries those fields point at* were never sent. This extracts them, on the
same pipeline as `blocks.tsv`/`items.tsv`: read the jar's pack with a committed tool, emit a committed
fixture, and let the server load it.

## Tag references are resolved here, deliberately

`minecraft:in_overworld` contains `#minecraft:universal`, a **tag reference**. The `update_tags` wire format
carries a tag's members as numeric registry ids, so a nested tag cannot be sent as a tag — it has to be
resolved to its members first. The resolver is applied at extraction time and the fixture records the resolved
**names**; the server then maps names to ids from its own entry order, so the id assignment stays in one
place.

A registry's tags directory is optional: most of the variant registries have none, and the fixture records an
empty tag list for those rather than omitting the registry.

## What it emits

`crates/network/src/registry_data/synced-registries.json`, one section per registry plus a `tags` section
keyed by namespaced registry name:

```json
{
  "world_clock": {"minecraft:overworld": {}, "minecraft:the_end": {}},
  "timeline":    {"minecraft:day": {…}, …, "minecraft:villager_schedule": {…}},
  "cat_variant": {"minecraft:tabby": {…}, …},
  "tags":        {"minecraft:timeline": {"in_overworld": ["minecraft:day", …], …}}
}
```
"""

import json
import sys
from pathlib import Path

ROOT = Path(r'C:\Users\25371\projects\MinecraftServer')
PACK = ROOT / 'target' / 'vanilla-26.1.2' / 'extract' / 'data' / 'minecraft'
OUT = ROOT / 'crates' / 'network' / 'src' / 'registry_data' / 'synced-registries.json'

# Every synced registry a 26.1.2 client requires to be non-empty, grown from what two real-client runs
# actually named. Order is fixed here because it becomes the wire id order that the tags index into.
REGISTRIES = (
    # named by the first run (KD-39): references the overworld `dimension_type` makes
    'world_clock',
    'timeline',
    # named by the second run: "Registry must be non-empty"
    'cat_sound_variant',
    'cat_variant',
    'chicken_sound_variant',
    'chicken_variant',
    'cow_sound_variant',
    'cow_variant',
    'frog_variant',
    'painting_variant',
    'pig_sound_variant',
    'pig_variant',
    'wolf_sound_variant',
    'wolf_variant',
    'zombie_nautilus_variant',
    # named by the third run: `Missing tag TagKey[minecraft:damage_type / minecraft:is_fire]`, so this one is
    # required *with its tags*. The rest of this group are added in the same pass rather than one refusal at a
    # time, because run 2 already established that the client validates every synced registry it knows about.
    'banner_pattern',
    'chat_type',
    'damage_type',
    'dialog',
    'enchantment_provider',
    'instrument',
    'jukebox_song',
    'test_environment',
    'test_instance',
    'trade_set',
    'trial_spawner',
    'trim_material',
    'trim_pattern',
    # `enchantment` is **excluded**, and the reason is a limit of this pipeline rather than a missing
    # registry. Every entry comes back from a real client as `Failed to parse value`: its fields use
    # *dispatch* codecs (a bare number or an object with a `type`), and JSON shape cannot express which.
    # NBT lists are homogeneous and a float is a different tag from a double, so a converter that infers
    # everything from shape cannot encode it. The remedy is to stop re-implementing codecs and capture
    # the real payload from a vanilla server (see the module doc).
    # `villager_trade` is **excluded**, and the reason is a real limit of this pipeline rather than an
    # oversight. Seven of its arrays are mixed-type (`number_of_dyes.summands` is `[{…}, 1]`), because vanilla
    # encodes that field with a *dispatch* codec — a number or an object — not a list of one shape. NBT lists
    # are homogeneous, so a shape-based converter cannot represent it, and guessing the dispatch encoding is
    # not something this project does. Nothing has shown a client requires this registry; if one does, the
    # finding is that the converter needs a schema.
)


def json_kind(value) -> str:
    if isinstance(value, bool):
        return 'bool'
    if isinstance(value, (int, float)):
        return 'number'
    if isinstance(value, str):
        return 'string'
    if isinstance(value, list):
        return 'array'
    if isinstance(value, dict):
        return 'object'
    return 'null'


def mixed_arrays(node, path='') -> list:
    """Every array in `node` whose elements are not all of one JSON kind."""
    found = []
    if isinstance(node, dict):
        for key, value in node.items():
            found.extend(mixed_arrays(value, f'{path}.{key}'))
    elif isinstance(node, list):
        kinds = {json_kind(item) for item in node}
        if len(kinds) > 1:
            found.append((path, sorted(kinds)))
        for index, value in enumerate(node):
            found.extend(mixed_arrays(value, f'{path}[{index}]'))
    return found


def load_registry(name: str) -> dict:
    """Every element of one registry, keyed `minecraft:<file stem>`."""
    directory = PACK / name
    if not directory.is_dir():
        raise SystemExit(f'the pack has no {name}/ directory at {directory}')
    entries = {}
    for path in sorted(directory.rglob('*.json')):
        stem = path.relative_to(directory).with_suffix('').as_posix()
        entries[f'minecraft:{stem}'] = json.loads(path.read_text(encoding='utf-8'))
    if not entries:
        raise SystemExit(f'{name}/ is empty')
    return entries


def load_tags(registry: str) -> dict:
    """Every tag of one registry, keyed by bare name (the pack stores them without a namespace dir)."""
    directory = PACK / 'tags' / registry
    if not directory.is_dir():
        raise SystemExit(f'the pack has no tags/{registry}/ directory')
    tags = {}
    for path in sorted(directory.rglob('*.json')):
        stem = path.relative_to(directory).with_suffix('').as_posix()
        body = json.loads(path.read_text(encoding='utf-8'))
        values = body.get('values')
        if not isinstance(values, list):
            raise SystemExit(f'{path} has no values array')
        tags[stem] = values
    return tags


def resolve(tag_name: str, raw: dict, entries: set, seen=None) -> list:
    """Expand a tag to the element names it contains, following `#` references.

    The wire cannot carry a nested tag inside a tag, so this has to happen before sending. A cycle would be a
    pack bug rather than something to tolerate, so it is an error here.
    """
    seen = seen or set()
    if tag_name in seen:
        raise SystemExit(f'tag cycle through {tag_name}')
    seen = seen | {tag_name}
    if tag_name not in raw:
        raise SystemExit(f'reference to unknown tag {tag_name}')
    members = []
    for value in raw[tag_name]:
        if value.startswith('#'):
            referenced = value[1:]
            if referenced.startswith('minecraft:'):
                referenced = referenced[len('minecraft:'):]
            members.extend(resolve(referenced, raw, entries, seen))
        else:
            name = value if value.startswith('minecraft:') else f'minecraft:{value}'
            if name not in entries:
                raise SystemExit(f'tag {tag_name} names {name}, which is not in the registry')
            members.append(name)
    # Deduplicate while preserving order: a tag listing the same member twice is legal but the wire would
    # carry it twice, and the client's set semantics do not care.
    ordered = []
    for member in members:
        if member not in ordered:
            ordered.append(member)
    return ordered


def main() -> int:
    document = {}
    registry_sizes = {}
    tags = {}

    for registry in REGISTRIES:
        entries = load_registry(registry)
        document[registry] = entries
        registry_sizes[registry] = len(entries)
        # Most variant registries have no tags; an absent directory means an empty tag set, not an error.
        if (PACK / 'tags' / registry).is_dir():
            raw = load_tags(registry)
            tags[f'minecraft:{registry}'] = {
                name: resolve(name, raw, set(entries)) for name in raw
            }

    document['tags'] = tags

    # Refuse a registry this pipeline cannot represent, naming the offending path. Failing here is far
    # better than emitting NBT that decodes as garbage on a live connection.
    for registry, entries in document.items():
        if registry == 'tags':
            continue
        for entry_id, element in entries.items():
            for path, kinds in mixed_arrays(element):
                raise SystemExit(
                    f'{registry}/{entry_id}{path} is a mixed-type array {kinds}; NBT lists are homogeneous, '
                    'so a shape-based converter cannot represent it. Exclude the registry or teach the '
                    'converter the field\'s schema.'
                )

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + '\n',
                   encoding='utf-8', newline='\n')

    print(f'wrote {OUT.relative_to(ROOT).as_posix()}  ({OUT.stat().st_size} bytes)')
    for registry, size in registry_sizes.items():
        print(f'  {registry}: {size} entries')
        for name in document[registry]:
            print(f'    {name}')
    for registry, resolved in tags.items():
        print(f'  tags/{registry}: {len(resolved)} tags')
        for name, members in resolved.items():
            print(f'    {name} -> {members}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
