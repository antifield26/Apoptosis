"""Census of data/minecraft/loot_table/ and advancement/ from the 26.1.2 jar.

Independent of the Rust loader: this walks the JSON directly and reports the same four
levels the loader's report counts (table / entry / function / condition), plus the
number-provider arguments. Used to *fix* the expected numbers the differential test
asserts, so the test is checked against a measurement rather than against itself.

Standard library only. Read-only.
"""
import collections
import json
import os
import zipfile

JAR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "server-26.1.2.jar")
PREFIX = "data/minecraft/"


def load(prefix):
    with zipfile.ZipFile(JAR) as z:
        names = sorted(n for n in z.namelist() if n.startswith(prefix) and not n.endswith("/"))
        out = []
        for name in names:
            with z.open(name) as handle:
                out.append((name[len(prefix):], json.loads(handle.read().decode("utf-8"))))
        return out


class Census:
    def __init__(self):
        self.tables = collections.Counter()
        self.entries = collections.Counter()
        self.functions = collections.Counter()
        self.conditions = collections.Counter()
        self.providers = collections.Counter()

    def provider(self, value):
        if isinstance(value, dict):
            self.providers[value.get("type", "<no type>")] += 1
        elif isinstance(value, (int, float)):
            self.providers["constant"] += 1
        else:
            self.providers["<absent or other>"] += 1

    def conditions_list(self, items):
        for c in items or []:
            if not isinstance(c, dict):
                continue
            self.conditions[c.get("condition", "<absent>")] += 1
            self.conditions_list(c.get("terms"))
            self.conditions_list(c.get("conditions"))
            # `inverted` uses the SINGULAR key `term`. Missing it made the first version of
            # this census report 39 any_of / 178 match_tool, when the files contain 51 and
            # 203: everything under `term` was invisible.
            term = c.get("term")
            if isinstance(term, dict):
                self.conditions_list([term])

    def functions_list(self, items):
        for f in items or []:
            if not isinstance(f, dict):
                continue
            name = f.get("function", "<absent>")
            self.functions[name] += 1
            self.conditions_list(f.get("conditions"))
            self.functions_list(f.get("functions"))
            if name == "minecraft:set_count":
                self.provider(f.get("count"))
            if name == "minecraft:set_damage":
                self.provider(f.get("damage"))

    def pool(self, p):
        if not isinstance(p, dict):
            return
        self.provider(p.get("rolls"))
        self.provider(p.get("bonus_rolls"))
        self.conditions_list(p.get("conditions"))
        self.functions_list(p.get("functions"))
        for e in p.get("entries") or []:
            self.entry(e)

    def table(self, doc, named):
        kind = doc.get("type", "minecraft:inline" if not named else "<absent>")
        self.tables[kind] += 1
        self.functions_list(doc.get("functions"))
        for p in doc.get("pools") or []:
            self.pool(p)

    def entry(self, e):
        if not isinstance(e, dict):
            return
        kind = e.get("type", "<absent>")
        self.entries[kind] += 1
        self.conditions_list(e.get("conditions"))
        # `functions` is legal on EVERY entry type, not only on `minecraft:item`. Reading it
        # only there (as the first version of this census did) loses the 2 functions vanilla
        # puts on `minecraft:loot_table` entries, plus their conditions and those conditions'
        # terms: 2 functions, 2 any_of and 4 entity_properties.
        self.functions_list(e.get("functions"))
        for c in e.get("children") or []:
            self.entry(c)
        if kind == "minecraft:loot_table" and isinstance(e.get("value"), dict):
            self.table(e["value"], named=False)


def survey_loot():
    files = load(PREFIX + "loot_table/")
    c = Census()
    for _, doc in files:
        c.table(doc, named=True)
    print("loot_table files:", len(files))
    for title, counter in (
        ("table types", c.tables),
        ("entry types", c.entries),
        ("function types", c.functions),
        ("condition types", c.conditions),
        ("number providers", c.providers),
    ):
        print(f"\n--- {title}: total {sum(counter.values())}, distinct {len(counter)} ---")
        for key, count in sorted(counter.items(), key=lambda kv: (-kv[1], kv[0])):
            print(f"{count:6d}  {key}")


def survey_adv():
    files = load(PREFIX + "advancement/")
    ids = set()
    parents = {}
    dupes = []
    criteria = collections.Counter()
    with_conditions = 0
    triggers = collections.Counter()
    displays = 0
    frames = collections.Counter()
    rewards = 0
    recipes = 0
    experience = 0
    groups = 0
    group_names = collections.Counter()
    undefined = collections.Counter()
    unreferenced = collections.Counter()
    for name, doc in files:
        rid = "minecraft:" + name[: -len(".json")]
        if rid in ids:
            dupes.append(rid)
        ids.add(rid)
        if isinstance(doc.get("parent"), str):
            parents[rid] = doc["parent"]
        crit = doc.get("criteria")
        if isinstance(crit, dict):
            for cname, c in crit.items():
                criteria[c.get("trigger")] += 1
                triggers[c.get("trigger")] += 1
                if c.get("conditions") is not None:
                    with_conditions += 1
        display = doc.get("display")
        if isinstance(display, dict):
            displays += 1
            frames[str(display.get("frame", "<absent>"))] += 1
        r = doc.get("rewards")
        if isinstance(r, dict):
            rewards += 1
            if "recipes" in r:
                recipes += len(r["recipes"])
            if "experience" in r:
                experience += 1
        req = doc.get("requirements")
        names_in_groups = []
        if isinstance(req, list):
            for g in req:
                groups += 1
                for n in g:
                    group_names[n] += 1
                    names_in_groups.append(n)
        # effective requirements: an absent field means "every criterion"
        if not names_in_groups and isinstance(crit, dict):
            names_in_groups = list(crit.keys())
        for n in names_in_groups:
            if not isinstance(crit, dict) or n not in crit:
                undefined[n] += 1
        if isinstance(crit, dict):
            for cname in crit:
                if cname not in names_in_groups:
                    unreferenced[cname] += 1
    print("\n\nadvancement files:", len(files))
    print("criteria total:", sum(criteria.values()))
    print("criteria with conditions:", with_conditions)
    print("distinct triggers:", len(triggers))
    print("displays:", displays, "frames:", dict(frames))
    print("with parent:", len(parents), "roots:", len(ids) - len(parents))
    print("rewards blocks:", rewards, "recipes listed:", recipes, "with experience:", experience)
    print("requirement groups:", groups)
    print("undefined requirement names:", sum(undefined.values()), dict(undefined))
    print("unreferenced criteria:", sum(unreferenced.values()))
    missing = [(c, p) for c, p in sorted(parents.items()) if p not in ids]
    print("missing parents:", len(missing))
    colour = {}
    cycles = 0
    for start in sorted(ids):
        if colour.get(start):
            continue
        path = []
        node = start
        while node is not None and colour.get(node) is None:
            colour[node] = 1
            path.append(node)
            node = parents.get(node)
        if node is not None and colour.get(node) == 1:
            cycles += 1
        for n in path:
            colour[n] = 2
    print("cycles:", cycles, "duplicate ids:", len(dupes))
    depth = {}

    def depth_of(node):
        seen = []
        d = 1
        cur = node
        while True:
            if cur in seen:
                return None
            seen.append(cur)
            if cur not in parents:
                return d
            cur = parents[cur]
            if cur not in ids:
                return None
            d += 1

    hist = collections.Counter()
    for i in sorted(ids):
        hist[depth_of(i)] += 1
    print("depth histogram:", dict(sorted(hist.items(), key=lambda kv: (kv[0] is None, kv[0]))))
    print("\ndistinct triggers:")
    for key, count in sorted(triggers.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"{count:6d}  {key}")


def survey_functions():
    with zipfile.ZipFile(JAR) as z:
        names = [n for n in z.namelist() if not n.endswith("/")]
    print("\n\n.mcfunction files in the whole jar:", len([n for n in names if n.endswith(".mcfunction")]))
    print(
        "data/minecraft/function entries:",
        len([n for n in names if n.startswith(PREFIX + "function/")]),
    )
    # The built-in packs are the only other place a function could hide.
    packs = sorted({n.split("/")[3] for n in names if n.startswith(PREFIX + "datapacks/") and len(n.split("/")) > 4})
    print("built-in datapacks:", packs)
    for pack in packs:
        prefix = PREFIX + "datapacks/" + pack + "/"
        dirs = collections.Counter(
            n[len(prefix):].split("/")[1] if len(n[len(prefix):].split("/")) > 1 else "<file>"
            for n in names
            if n.startswith(prefix)
        )
        print(f"  {pack}: {dict(dirs)}")


if __name__ == "__main__":
    survey_loot()
    survey_adv()
    survey_functions()
