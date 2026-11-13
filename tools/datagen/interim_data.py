#!/usr/bin/env python3
"""Interim generator for data/26.3/*.json.

Sources (nothing from the Mojang jar is executed):
  * server jar read as a zip: data/minecraft/** (synchronised registries, tags, dimension types)
  * javap on Registries / RegistryDataLoader for registry keys and the synchronised list
  * misode/mcmeta 26.3 summary: block property names and ordered values, name sets
  * Pumpkin-MC assets (GPL-3.0): numeric registry orders and per-state physical properties
  * minecraft.wiki Command_data page: command argument type ids

usage: interim_data.py <out_dir>
"""
import json
import re
import subprocess
import sys
import zipfile

JAR = '/home/user/upstream/vanilla/inner/META-INF/versions/26.3/server-26.3.jar'
CLASSES = '/home/user/upstream/vanilla/classes'
JAVAP = '/opt/jdk25/bin/javap'
MCMETA = '/home/user/upstream/mcmeta/summary/'
PUMPKIN = '/home/user/upstream/pumpkin/assets/'
WIKI_COMMANDS = '/home/user/upstream/wiki/Command_data.wiki'

out_dir = sys.argv[1]
problems = []


def note(msg):
    problems.append(msg)
    print('NOTE:', msg)


def dump(name, obj):
    with open(f'{out_dir}/{name}', 'w') as f:
        json.dump(obj, f, separators=(',', ':'))
    print(f'wrote {name}')


def ns(name):
    return name if ':' in name else 'minecraft:' + name


def javap(cls):
    return subprocess.run([JAVAP, '-p', '-c', '-cp', CLASSES, cls], capture_output=True, text=True, env={'JAVA_TOOL_OPTIONS': ''}).stdout


mcmeta_reg = json.load(open(MCMETA + 'registries/data.json'))
mcmeta_blocks = json.load(open(MCMETA + 'blocks/data.json'))
jar = zipfile.ZipFile(JAR)
jar_names = jar.namelist()

# ---------------------------------------------------------------------------------------------
# Built-in registries
# ---------------------------------------------------------------------------------------------
registries = {}


def check_set(registry, names, mcmeta_key=None):
    key = mcmeta_key or registry.split(':', 1)[1]
    if key not in mcmeta_reg:
        note(f'{registry}: no mcmeta list {key} to check against')
        return
    expected = {ns(n) for n in mcmeta_reg[key]}
    got = set(names)
    if len(names) != len(got):
        note(f'{registry}: duplicate entries')
    if got != expected:
        note(f'{registry}: differs from mcmeta: missing {sorted(expected - got)[:8]} extra {sorted(got - expected)[:8]}')


def by_id(d):
    items = sorted(d.items(), key=lambda kv: kv[1]['id'])
    assert [v['id'] for _, v in items] == list(range(len(items))), 'ids not dense'
    return [ns(k) for k, _ in items]


def P(name):
    return json.load(open(PUMPKIN + name))


pblocks = P('blocks.json')
block_list = sorted(pblocks['blocks'], key=lambda b: b['id'])
assert [b['id'] for b in block_list] == list(range(len(block_list)))
registries['minecraft:block'] = [ns(b['name']) for b in block_list]
check_set('minecraft:block', registries['minecraft:block'])

pitems = P('items.json')
registries['minecraft:item'] = by_id(pitems)
check_set('minecraft:item', registries['minecraft:item'])

registries['minecraft:entity_type'] = by_id(P('entities.json'))
check_set('minecraft:entity_type', registries['minecraft:entity_type'])

registries['minecraft:block_entity_type'] = [ns(n) for n in pblocks['block_entity_types']]
check_set('minecraft:block_entity_type', registries['minecraft:block_entity_type'])

registries['minecraft:sound_event'] = [ns(n) for n in P('sounds.json')]
check_set('minecraft:sound_event', registries['minecraft:sound_event'])

registries['minecraft:particle_type'] = [ns(n) for n in P('particles.json')]
check_set('minecraft:particle_type', registries['minecraft:particle_type'])

registries['minecraft:menu'] = [ns(n) for n in P('screens.json')]
check_set('minecraft:menu', registries['minecraft:menu'])

fluids = sorted(P('fluids.json'), key=lambda f: f['id'])
registries['minecraft:fluid'] = [ns(f['name']) for f in fluids]
check_set('minecraft:fluid', registries['minecraft:fluid'])

registries['minecraft:game_event'] = [ns(g['name']) for g in P('game_event.json')]
check_set('minecraft:game_event', registries['minecraft:game_event'])

registries['minecraft:data_component_type'] = by_id(P('data_component.json'))
check_set('minecraft:data_component_type', registries['minecraft:data_component_type'])

registries['minecraft:attribute'] = by_id(P('attributes.json'))
check_set('minecraft:attribute', registries['minecraft:attribute'])

registries['minecraft:mob_effect'] = by_id(P('effect.json'))
check_set('minecraft:mob_effect', registries['minecraft:mob_effect'])

registries['minecraft:potion'] = by_id(P('potion.json'))
check_set('minecraft:potion', registries['minecraft:potion'])

registries['minecraft:point_of_interest_type'] = by_id(P('poi_types.json'))
check_set('minecraft:point_of_interest_type', registries['minecraft:point_of_interest_type'])

# Command argument types: registration order in ArgumentTypeInfos.bootstrap (bytecode).
# The wiki's table is stale for 26.3, so it is not used.
arg_src = javap('net.minecraft.commands.synchronization.ArgumentTypeInfos')
boot = arg_src[arg_src.index(' bootstrap('):]
boot = boot[:boot.index('areturn')]
arg_names = re.findall(r'// String ([a-z_:0-9]+)', boot)
registries['minecraft:command_argument_type'] = [ns(n) for n in arg_names]
check_set('minecraft:command_argument_type', registries['minecraft:command_argument_type'])

meta = P('meta_data_type.json')
meta_sorted = sorted(meta.items(), key=lambda kv: kv[1])
assert [v for _, v in meta_sorted] == list(range(len(meta_sorted)))
registries['paper-rs:entity_data_serializer'] = [k for k, _ in meta_sorted]

dump('registries.json', registries)

# ---------------------------------------------------------------------------------------------
# Blocks and block states
# ---------------------------------------------------------------------------------------------
item_names = registries['minecraft:item']
ALWAYS_WATERLOGGED = {'minecraft:kelp', 'minecraft:kelp_plant', 'minecraft:seagrass', 'minecraft:tall_seagrass', 'minecraft:bubble_column'}
blocks_out = []
luminance, opacity, flags = [], [], []
state_id = 0
for b in block_list:
    name = ns(b['name'])
    props_def, defaults = mcmeta_blocks[b['name']]
    props = sorted(props_def.items())
    count = 1
    for _, values in props:
        count *= len(values)
    states = sorted(b['states'], key=lambda s: s['id'])
    if len(states) != count or states[0]['id'] != state_id:
        raise SystemExit(f'{name}: state layout mismatch: pumpkin {len(states)} states from {states[0]["id"]}, expected {count} from {state_id}')
    # Default state from mcmeta's default property values.
    index = 0
    for pname, values in props:
        index = index * len(values) + values.index(defaults[pname])
    default_state = state_id + index
    if default_state != b['default_state_id']:
        raise SystemExit(f'{name}: default state mismatch {default_state} vs {b["default_state_id"]}')
    for i, st in enumerate(states):
        # Decode this state's property values (first property varies slowest).
        rem, values_of = i, {}
        for pname, values in reversed(props):
            values_of[pname] = values[rem % len(values)]
            rem //= len(values)
        sf = st['state_flags']
        shapes = st['collision_shapes']
        f = 0
        if sf & 1:
            f |= 1
        if shapes:
            f |= 2
        if shapes == [0]:
            f |= 4
        if (sf & 32) or values_of.get('waterlogged') == 'true' or name in ALWAYS_WATERLOGGED:
            f |= 8
        if sf & 16:
            f |= 16
        if 'block_entity_type' in st:
            f |= 32
        luminance.append(st['luminance'])
        opacity.append(st['opacity'])
        flags.append(f)
    item = item_names[b['item_id']] if b['item_id'] else None
    blocks_out.append({
        'name': name, 'id': b['id'], 'min_state': state_id, 'default_state': default_state,
        'properties': [{'name': pname, 'values': values} for pname, values in props],
        'item': item, 'hardness': b['hardness'], 'blast_resistance': b['blast_resistance'],
    })
    state_id += count
dump('blocks.json', {'state_count': state_id, 'blocks': blocks_out})
dump('block_states.json', {'luminance': luminance, 'opacity': opacity, 'flags': flags})
print('blocks', len(blocks_out), 'states', state_id)

# ---------------------------------------------------------------------------------------------
# Items
# ---------------------------------------------------------------------------------------------
block_names = set(registries['minecraft:block'])
placed_by = {}
for b in blocks_out:
    if b['item']:
        placed_by.setdefault(b['item'], []).append(b['name'])
items_out = []
for name in item_names:
    short = name.split(':', 1)[1]
    comps = pitems[short]['components']
    candidates = placed_by.get(name, [])
    block = name if name in candidates else (candidates[0] if candidates else None)
    items_out.append({'name': name, 'id': len(items_out), 'max_stack_size': comps.get('minecraft:max_stack_size', 64), 'block': block})
dump('items.json', {'items': items_out})

# ---------------------------------------------------------------------------------------------
# Synchronised registries
# ---------------------------------------------------------------------------------------------
reg_src = javap('net.minecraft.core.registries.Registries')
field_key = {}
last = None
for line in reg_src.split('\n'):
    m = re.search(r'ldc(?:_w)?\s+#\d+\s+// String (\S+)', line)
    if m:
        last = m.group(1)
    m = re.search(r'putstatic\s+#\d+\s+// Field ([A-Z_0-9]+):Lnet/minecraft/resources/ResourceKey;', line)
    if m and last:
        field_key[m.group(1)] = last
        last = None
loader_src = javap('net.minecraft.resources.RegistryDataLoader')
clinit = loader_src[loader_src.index('static {};'):]
order_fields, current = None, []
for line in clinit.split('\n'):
    m = re.search(r'getstatic\s+#\d+\s+// Field net/minecraft/core/registries/Registries\.([A-Z_0-9]+):', line)
    if m:
        current.append(m.group(1))
    m = re.search(r'putstatic\s+#\d+\s+// Field ([A-Z_]+):Ljava/util/List;', line)
    if m:
        if m.group(1) == 'SYNCHRONIZED_REGISTRIES':
            order_fields = current
        current = []
assert order_fields, 'SYNCHRONIZED_REGISTRIES not found'
order = ['minecraft:' + field_key[f] for f in order_fields]
entries = {}
for registry in order:
    path = registry.split(':', 1)[1]
    prefix = f'data/minecraft/{path}/'
    ids = sorted('minecraft:' + n[len(prefix):-5] for n in jar_names if n.startswith(prefix) and n.endswith('.json'))
    entries[registry] = ids
    if path in mcmeta_reg and set(ids) != {ns(n) for n in mcmeta_reg[path]}:
        note(f'{registry}: jar entries differ from mcmeta')
    if not ids:
        note(f'{registry}: no entries in the jar')
dump('synced_registries.json', {'order': order, 'entries': entries})
print('synced registries:', {r.split(':')[1]: len(e) for r, e in entries.items()})

# ---------------------------------------------------------------------------------------------
# Tags
# ---------------------------------------------------------------------------------------------
STATIC_TAGGED = ['minecraft:block', 'minecraft:item', 'minecraft:fluid', 'minecraft:entity_type', 'minecraft:game_event',
                 'minecraft:point_of_interest_type', 'minecraft:potion']
tag_dirs = set()
for n in jar_names:
    m = re.match(r'data/minecraft/tags/(.+)/[^/]+\.json$', n)
    if m:
        tag_dirs.add(m.group(1))
tags_out = {}
unhandled = []


def id_table(registry):
    names = registries.get(registry) or entries.get(registry)
    return {n: i for i, n in enumerate(names)}


for registry in STATIC_TAGGED + order:
    path = registry.split(':', 1)[1]
    prefix = f'data/minecraft/tags/{path}/'
    files = {n[len(prefix):-5]: n for n in jar_names if n.startswith(prefix) and n.endswith('.json')}
    # A deeper registry path (worldgen/biome) must not be mistaken for tags of a shallower one.
    if not files:
        continue
    table = id_table(registry)
    raw = {name: json.loads(jar.read(path_in_jar)) for name, path_in_jar in files.items()}
    resolved = {}

    def resolve(tag, stack=()):
        if tag in resolved:
            return resolved[tag]
        if tag in stack:
            raise SystemExit(f'tag cycle at {registry} #{tag}')
        out = []
        for value in raw[tag]['values']:
            required = True
            if isinstance(value, dict):
                required = value.get('required', True)
                value = value['id']
            if value.startswith('#'):
                ref = value[1:].split(':', 1)[1] if ':' in value else value[1:]
                if ref not in raw:
                    if required:
                        raise SystemExit(f'{registry}: tag #{tag} references missing #{ref}')
                    continue
                for v in resolve(ref, stack + (tag,)):
                    if v not in out:
                        out.append(v)
            else:
                value = ns(value)
                if value not in table:
                    if required:
                        raise SystemExit(f'{registry}: tag #{tag} references unknown entry {value}')
                    continue
                if table[value] not in out:
                    out.append(table[value])
        resolved[tag] = out
        return out

    tags_out[registry] = {'minecraft:' + tag: resolve(tag) for tag in sorted(raw)}
handled = {r.split(':', 1)[1] for r in tags_out}
for d in sorted(tag_dirs):
    top = d
    # tag_dirs contains nested tag folders too (e.g. block/mineable); reduce to registry roots.
    if not any(top == h or top.startswith(h + '/') for h in handled):
        unhandled.append(d)
print('tag registries:', {r.split(':')[1]: len(t) for r, t in tags_out.items()})
print('tag folders not sent (registry not synchronised):', sorted({u for u in unhandled}))
dump('tags.json', tags_out)

# ---------------------------------------------------------------------------------------------
# Dimension types, feature flags, version
# ---------------------------------------------------------------------------------------------
dims = {}
for n in sorted(jar_names):
    m = re.match(r'data/minecraft/dimension_type/(.+)\.json$', n)
    if m:
        dims['minecraft:' + m.group(1)] = json.loads(jar.read(n))
dump('dimension_types.json', dims)
dump('feature_flags.json', ['minecraft:vanilla'])

print('\nproblems:', len(problems))
for p in problems:
    print('  -', p)
