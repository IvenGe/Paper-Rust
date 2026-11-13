#!/usr/bin/env python3
"""Extra 26.3 tables derived from the Pumpkin project's extracted assets.

Vanilla defines these in code rather than data files: default item components (food,
tools, durability, attack damage, armour, fuel), per-state collision shapes and block
behaviour flags, entity sizes, attributes and spawn rules, and the entity metadata layout.
Pumpkin (https://github.com/Pumpkin-MC/Pumpkin) extracts them from vanilla 26.3; its
assets/NOTICE.md states the extracted files are not covered by Pumpkin's licence.

Usage:  python3 -I tools/datagen/pumpkin_extra.py <pumpkin>/assets data/26.3

Checks that Pumpkin's block state ids and item ids agree with ours before writing.
"""

import json
import sys
from pathlib import Path

ITEM_COMPONENTS = [
    "minecraft:max_damage",
    "minecraft:food",
    "minecraft:consumable",
    "minecraft:use_remainder",
    "minecraft:tool",
    "minecraft:weapon",
    "minecraft:attribute_modifiers",
    "minecraft:equippable",
    "minecraft:cooking_fuel",
    "minecraft:damage_resistant",
    "minecraft:block_state",
    "minecraft:enchantable",
    "minecraft:repairable",
    "minecraft:use_cooldown",
    "minecraft:death_protection",
    "minecraft:glider",
]

# Pumpkin's block state flag bits (pumpkin-data/src/block_state.rs).
FLAG_NAMES = {
    1 << 0: "air",
    1 << 1: "burnable",
    1 << 2: "tool_required",
    1 << 4: "replaceable",
    1 << 5: "liquid",
    1 << 6: "solid",
    1 << 7: "full_cube",
    1 << 8: "solid_block",
    1 << 9: "random_ticks",
    1 << 10: "solid_render",
    1 << 12: "analog_output",
    1 << 13: "waterlogged",
}


def ns(name):
    return name if ":" in name else "minecraft:" + name


def main():
    pumpkin = Path(sys.argv[1])
    out = Path(sys.argv[2])
    ours_blocks = json.loads((out / "blocks.json").read_text())["blocks"]
    ours_items = json.loads((out / "items.json").read_text())["items"]
    errors = []

    # --- items -------------------------------------------------------------------------
    items = json.loads((pumpkin / "items.json").read_text())
    by_id = {i["id"]: i["name"] for i in ours_items}
    components = {}
    for name, item in items.items():
        full = ns(name)
        if by_id.get(item["id"]) != full:
            errors.append(f"item {full}: pumpkin id {item['id']} is {by_id.get(item['id'])} here")
            continue
        picked = {k.split(":", 1)[1]: v for k, v in item["components"].items() if k in ITEM_COMPONENTS}
        mods = picked.get("attribute_modifiers")
        if mods == []:
            del picked["attribute_modifiers"]
        if picked:
            components[full] = picked
    (out / "item_components.json").write_text(json.dumps({"items": components}, sort_keys=True, separators=(",", ":")) + "\n")

    # --- blocks ------------------------------------------------------------------------
    blocks = json.loads((pumpkin / "blocks.json").read_text())
    shapes = [s["min"] + s["max"] for s in blocks["shapes"]]
    shape_sets = []
    shape_set_index = {}
    state_count = sum(1 for b in blocks["blocks"] for _ in b["states"])
    state_shapes = [0] * state_count
    state_flags = [0] * state_count
    per_block = {}
    ours_by_name = {b["name"]: b for b in ours_blocks}
    for block in blocks["blocks"]:
        name = ns(block["name"])
        mine = ours_by_name.get(name)
        if mine is None or mine["id"] != block["id"] or mine["min_state"] != block["states"][0]["id"]:
            errors.append(f"block {name}: state ids differ")
            continue
        info = {}
        if block.get("loot_table"):
            info["loot_table"] = block["loot_table"]
        for key, default in (("slipperiness", 0.6), ("velocity_multiplier", 1.0), ("jump_velocity_multiplier", 1.0)):
            if abs(block.get(key, default) - default) > 1e-9:
                info[key] = block[key]
        if info:
            per_block[name] = info
        for state in block["states"]:
            key = tuple(state.get("collision_shapes", []))
            if key not in shape_set_index:
                shape_set_index[key] = len(shape_sets)
                shape_sets.append(list(key))
            state_shapes[state["id"]] = shape_set_index[key]
            flags = []
            for bit, flag in FLAG_NAMES.items():
                if state["state_flags"] & bit:
                    flags.append(flag)
            state_flags[state["id"]] = state["state_flags"]
    block_entity_types = blocks.get("block_entity_types", [])
    (out / "block_physics.json").write_text(json.dumps({
        "flag_bits": {v: k for k, v in FLAG_NAMES.items()},
        "shapes": shapes,
        "shape_sets": shape_sets,
        "state_shape_sets": state_shapes,
        "state_flags": state_flags,
        "blocks": per_block,
        "block_entity_types": block_entity_types,
    }, sort_keys=True, separators=(",", ":")) + "\n")

    # --- entities ------------------------------------------------------------------------
    entities = json.loads((pumpkin / "entities.json").read_text())
    picked = {}
    for name, e in entities.items():
        attrs = {}
        for a in e.get("attributes", []):
            attrs.update(a)
        entry = {
            "id": e["id"],
            "category": e.get("category", "MISC"),
            "dimension": e.get("dimension", [0.6, 1.8]),
            "eye_height": e.get("eye_height", 1.62),
            "mob": e.get("mob", False),
            "fire_immune": e.get("fire_immune", False),
            "tracking_range": e.get("client_tracking_range", 5),
            "update_interval": e.get("update_interval", 3),
            "track_deltas": e.get("track_deltas", False),
            "attributes": attrs,
        }
        for key in ("max_health", "experience_reward", "limit_per_chunk", "spawn_restriction", "hurt_sound", "death_sound", "ambient_sound", "sound_source", "is_enemy"):
            if key in e:
                entry[key] = e[key]
        picked[ns(name)] = entry
    (out / "entities.json").write_text(json.dumps({"entities": picked}, sort_keys=True, separators=(",", ":")) + "\n")

    tracked = json.loads((pumpkin / "tracked_data.json").read_text())
    data = {ns(name): {field: [v["id"], v["type_id"]] for field, v in fields.items()} for name, fields in tracked.items()}
    (out / "entity_data.json").write_text(json.dumps({"entities": data}, sort_keys=True, separators=(",", ":")) + "\n")

    if errors:
        print("\n".join(errors[:50]), file=sys.stderr)
        print(f"{len(errors)} mismatches", file=sys.stderr)
        return 1
    print(f"items with components: {len(components)}, shape sets: {len(shape_sets)}, entities: {len(picked)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
