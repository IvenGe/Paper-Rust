#!/usr/bin/env python3
"""Writes data/compat/legacy.json: what a translator for clients from before the flattening
(1.12 and older, down to 1.8) needs to show them the 26.3 world.

* `blocks`: the legacy block (`id << 4 | meta`, 0 for air) of every 26.3 block state, as
  little-endian u16s;
* `items`: the legacy item (`id << 16 | damage`) of every 26.3 item, as little-endian u32s;
* `sounds`: for every 26.3 sound event, the index of its legacy sound in `sound_names`
  (0xFFFF: none), as little-endian u16s;

each base64-encoded, per legacy version.

They are ViaBackwards' mappings (https://github.com/ViaVersion/ViaBackwards, GPL-3.0 or later,
as this project), step by step from 26.3 to 1.13, then its flattening table (1.13 to 1.12), then
the replacements ViaBackwards and ViaRewind (https://github.com/ViaVersion/ViaRewind) give
blocks and items each older version lacks.

Usage: compat_legacy.py <viaversion checkout> <viabackwards checkout> <viarewind checkout> <repo>
"""

import base64
import json
import struct
import sys
from pathlib import Path

from compat import read_nbt, via_mappings


def packed(fmt, values):
    return base64.b64encode(struct.pack("<%d%s" % (len(values), fmt), *values)).decode()

# ViaBackwards' steps from 26.3 down to 1.12, in order.
STEPS = [
    "26.3to26.2", "26.2to26.1", "26.1to1.21.11", "1.21.11to1.21.9", "1.21.9to1.21.7", "1.21.7to1.21.6",
    "1.21.6to1.21.5", "1.21.5to1.21.4", "1.21.4to1.21.2", "1.21.2to1.21", "1.21to1.20.5", "1.20.5to1.20.3",
    "1.20.3to1.20.2", "1.20.2to1.20", "1.20to1.19.4", "1.19.4to1.19.3", "1.19.3to1.19", "1.19to1.18",
    "1.18to1.17", "1.17to1.16.2", "1.16.2to1.16", "1.16to1.15", "1.15to1.14", "1.14to1.13.2",
    "1.13.2to1.13", "1.13to1.12", "1.12to1.11", "1.11to1.10", "1.10to1.9.4",
]

# The replacement tables of each legacy step, newest first: (version, project, file).
REPLACEMENTS = [
    ("1.12", "backwards", "item-mappings-1.12.json"),
    ("1.11.1", "backwards", "item-mappings-1.11.1.json"),
    ("1.11", "backwards", "item-mappings-1.11.json"),
    ("1.10", "backwards", "item-mappings-1.10.json"),
    ("1.9", "rewind", "item-mappings-1.9.json"),
]

# Legacy targets and the replacement steps down to each.
TARGETS = {"1.8": ["1.12", "1.11.1", "1.11", "1.10", "1.9"]}


def compose(first, second):
    return [second[i] if 0 <= i < len(second) else -1 for i in first]


def load_replacements(path):
    """`(kind, (id, data or -1))` -> (id, data or -1 to keep it), for kinds blocks/items."""
    doc = json.loads(path.read_text())
    out = {"blocks": {}, "items": {}}
    for section, kinds in (("items", ("items",)), ("block-items", ("blocks", "items")), ("blocks", ("blocks",))):
        for key, value in doc.get(section, {}).items():
            target = (int(value["id"]), int(value.get("data", 0)))
            if "-" in key:
                first, last = (int(k) for k in key.split("-", 1))
                keys = [(i, -1) for i in range(first, last + 1)]
            elif ":" in key:
                i, d = key.split(":", 1)
                keys = [(int(i), int(d))]
            else:
                keys = [(int(key), -1)]
            for kind in kinds:
                for k in keys:
                    out[kind][k] = target
    return out


def replace(table, legacy_id, data):
    """Applies one step's replacements to a legacy (id, data)."""
    hit = table.get((legacy_id, data)) or table.get((legacy_id, -1))
    if hit is None:
        return legacy_id, data
    new_id, new_data = hit
    return new_id, data if new_data == -1 else new_data


def main():
    via, backwards, rewind, repo = (Path(a) for a in sys.argv[1:5])
    via_data = via / "common/src/main/resources/assets/viaversion/data"
    back_data = backwards / "common/src/main/resources/assets/viabackwards/data"
    rewind_data = rewind / "common/src/main/resources/assets/viarewind/data"
    registries = json.loads((repo / "data/26.3/registries.json").read_text())
    state_count = int(json.loads((repo / "data/26.3/blocks.json").read_text())["state_count"])

    current = {"blockstates": list(range(state_count)), "items": list(range(len(registries["minecraft:item"]))), "sounds": list(range(len(registries["minecraft:sound_event"])))}
    for step in STEPS:
        mappings = read_nbt((back_data / f"mappings-{step}.nbt").read_bytes())
        for key in current:
            if key in mappings:
                current[key] = compose(current[key], via_mappings(mappings[key]))
        if step == "1.13to1.12":
            # Blocks and items are legacy values from here: `id << 4 | data`.
            flattened = dict(current)
    blocks_1_12 = flattened["blockstates"]
    items_1_12 = flattened["items"]

    # 1.9 to 1.8 sounds, and 1.8's sound names.
    rewind_mappings = read_nbt((rewind_data / "mappings-1.9to1.8.nbt").read_bytes())
    sounds_1_8 = compose(current["sounds"], via_mappings(rewind_mappings["sounds"]))
    table = read_nbt((via_data / "identifier-table.nbt").read_bytes())
    sound_names = [table["sounds"][i] for i in via_mappings(read_nbt((via_data / "identifiers-1.8.nbt").read_bytes())["sounds"])]

    replacements = {}
    for version, project, file in REPLACEMENTS:
        base = back_data if project == "backwards" else rewind_data
        replacements[version] = load_replacements(base / file)

    item_names = registries["minecraft:item"]
    out = {}
    for target, steps in TARGETS.items():
        blocks = []
        for value in blocks_1_12:
            if value < 0:
                blocks.append(0)
                continue
            legacy_id, data = value >> 4, value & 15
            for version in steps:
                legacy_id, data = replace(replacements[version]["blocks"], legacy_id, data)
            # A block the version cannot show at all: air.
            blocks.append(legacy_id << 4 | (data & 15) if 0 < legacy_id < 256 else 0)
        items = []
        for index, value in enumerate(items_1_12):
            if value < 0:
                items.append(1 << 16)
                continue
            if value >= 1 << 16:
                # Spawn eggs: the translator gives them their mob.
                items.append(383 << 16)
                continue
            legacy_id, data = value >> 4, value & 15
            for version in steps:
                legacy_id, data = replace(replacements[version]["items"], legacy_id, data)
            items.append(legacy_id << 16 | (data & 0xFFFF))
            del index
        out[target] = {"blocks": packed("H", blocks), "items": packed("I", items), "sounds": packed("H", [s if s >= 0 else 0xFFFF for s in sounds_1_8])}
        # Spot checks against the 1.8 ids everyone knows.
        state = lambda name: next(b for b in json.loads((repo / "data/26.3/blocks.json").read_text())["blocks"] if b["name"] == name)["default_state"]
        for name, expected in (("minecraft:stone", 1 << 4), ("minecraft:bedrock", 7 << 4), ("minecraft:diamond_block", 57 << 4), ("minecraft:oak_planks", 5 << 4)):
            got = blocks[state(name)]
            assert got == expected, f"{target} {name}: {got >> 4}:{got & 15}"
        for name, expected in (("minecraft:diamond", 264 << 16), ("minecraft:red_wool", 35 << 16 | 14), ("minecraft:stone", 1 << 16)):
            got = items[item_names.index(name)]
            assert got == expected, f"{target} {name}: {got >> 16}:{got & 0xFFFF}"
    out["sound_names"] = sound_names
    (repo / "data/compat/legacy.json").write_text(json.dumps(out, separators=(",", ":")) + "\n")
    for target in TARGETS:
        print(target, {k: len(v) for k, v in out[target].items()})


if __name__ == "__main__":
    main()
