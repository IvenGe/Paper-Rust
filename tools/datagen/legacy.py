#!/usr/bin/env python3
"""Builds `data/26.3/legacy.json`: what the pre-1.13 numeric block and item ids (`id:data`)
are in 26.3. The plugin host uses it for Bukkit's legacy materials (`UnsafeValues.fromLegacy`
and `toLegacy`, `Material.isBlock` on `LEGACY_` constants).

Input: `data/pc/common/legacy.json` of PrismarineJS/minecraft-data (MIT licence), which maps
1.12 ids to 1.13 block states and item names. Names that changed after 1.13 are renamed here,
and every block, property, value and item is checked against `blocks.json` and `items.json`;
a property 26.3 no longer has is left out.

    python3 -I tools/datagen/legacy.py <minecraft-data>/data/pc/common/legacy.json data/26.3
"""
import json
import sys

# Renamed since 1.13 (1.14: signs, the smooth stone slab and three dyes; 1.16: zombified
# piglins; 1.17: dirt paths; 1.20.3: short grass; 1.20.5: turtle scutes).
RENAMED = {
    'minecraft:sign': 'minecraft:oak_sign',
    'minecraft:wall_sign': 'minecraft:oak_wall_sign',
    'minecraft:stone_slab': 'minecraft:smooth_stone_slab',
    'minecraft:rose_red': 'minecraft:red_dye',
    'minecraft:cactus_green': 'minecraft:green_dye',
    'minecraft:dandelion_yellow': 'minecraft:yellow_dye',
    'minecraft:zombie_pigman_spawn_egg': 'minecraft:zombified_piglin_spawn_egg',
    'minecraft:grass_path': 'minecraft:dirt_path',
    'minecraft:grass': 'minecraft:short_grass',
    'minecraft:scute': 'minecraft:turtle_scute',
}


def parse_state(text):
    name, _, rest = text.partition('[')
    props = {}
    if rest:
        for pair in rest.rstrip(']').split(','):
            key, _, value = pair.partition('=')
            props[key] = value
    return name, props


def main(source, out_dir):
    legacy = json.load(open(source))
    blocks = {b['name']: b for b in json.load(open(f'{out_dir}/blocks.json'))['blocks']}
    items = {i['name'] for i in json.load(open(f'{out_dir}/items.json'))['items']}
    problems = []
    out_blocks = {}
    for key, state in legacy['blocks'].items():
        name, props = parse_state(state)
        name = RENAMED.get(name, name)
        block = blocks.get(name)
        if block is None:
            problems.append(f'block {key}: {state}')
            continue
        known = {p['name']: p['values'] for p in block['properties']}
        kept = []
        for prop, value in props.items():
            if prop not in known:
                continue
            # 1.16: wall sides became none / low / tall.
            if name.endswith('_wall') and prop in ('north', 'south', 'east', 'west'):
                value = {'false': 'none', 'true': 'low'}.get(value, value)
            if value not in known[prop]:
                problems.append(f'block {key}: {state}: {prop}={value}')
                continue
            kept.append(f'{prop}={value}')
        out_blocks[key] = name + (f'[{",".join(kept)}]' if kept else '')
    out_items = {}
    for key, item in legacy['items'].items():
        item = RENAMED.get(item, item)
        if item not in items:
            problems.append(f'item {key}: {item}')
            continue
        out_items[key] = item
    if problems:
        print('\n'.join(problems), file=sys.stderr)
        sys.exit(1)
    order = lambda kv: tuple(int(n) for n in kv[0].split(':'))
    with open(f'{out_dir}/legacy.json', 'w') as f:
        json.dump({'blocks': dict(sorted(out_blocks.items(), key=order)), 'items': dict(sorted(out_items.items(), key=order))}, f, indent=1)
        f.write('\n')
    print(f'{len(out_blocks)} block states, {len(out_items)} items')


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
