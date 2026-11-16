#!/usr/bin/env python3
"""Writes data/<version>/item_providers.json: the items that provide an armour trim
material (smithing tables) or banner patterns (looms), from the default item components of
misode/mcmeta's `<version>-summary` (`item_components/data.json`).

Usage: item_providers.py <mcmeta summary dir> <data dir>
"""

import json
import sys
from pathlib import Path


def ns(name):
    return name if ":" in name else f"minecraft:{name}"


def main():
    summary = Path(sys.argv[1])
    out = Path(sys.argv[2])
    components = json.loads((summary / "item_components" / "data.json").read_text())
    ours = {i["name"] for i in json.loads((out / "items.json").read_text())["items"]}
    trim_materials = {}
    banner_patterns = {}
    errors = []
    for name, c in components.items():
        item = ns(name)
        material = c.get("minecraft:provides_trim_material")
        patterns = c.get("minecraft:provides_banner_patterns")
        if (material or patterns) and item not in ours:
            errors.append(f"{item} is not an item here")
            continue
        if isinstance(material, str):
            trim_materials[item] = ns(material)
        elif material is not None:
            errors.append(f"{item}: provides_trim_material is not a reference: {material!r}")
        if isinstance(patterns, str) and patterns.startswith("#"):
            banner_patterns[item] = ns(patterns[1:])
        elif patterns is not None:
            errors.append(f"{item}: provides_banner_patterns is not a tag: {patterns!r}")
    if errors:
        sys.exit("\n".join(errors))
    data = {"trim_materials": trim_materials, "banner_patterns": banner_patterns}
    (out / "item_providers.json").write_text(json.dumps(data, sort_keys=True, indent=1) + "\n")
    print(f"{len(trim_materials)} trim material items, {len(banner_patterns)} banner pattern items")


if __name__ == "__main__":
    main()
