"""Builds data/26.3/worldgen/block_physics.bin.gz from Pumpkin's assets/blocks.json.

Per block state id (in order), three bytes: state_flags (u16 LE) and side_flags (u8), exactly as
Pumpkin's extractor recorded them. Checks that every state id belongs to the same block in the
repository's data/26.3/blocks.json.
"""
import gzip, json, struct, sys

pumpkin_blocks, repo_blocks, out = sys.argv[1], sys.argv[2], sys.argv[3]
p = json.load(open(pumpkin_blocks))
r = json.load(open(repo_blocks))
count = r['state_count']
owner = [None] * count
for b in r['blocks']:
    nxt = None
    owner_name = b['name']
    # state counts: product of property value counts
    n = 1
    for prop in b['properties']:
        n *= len(prop['values']) if isinstance(prop, dict) else len(prop[1])
    for s in range(b['min_state'], b['min_state'] + n):
        owner[s] = owner_name
flags = [None] * count
for b in p['blocks']:
    name = 'minecraft:' + b['name']
    for s in b['states']:
        sid = s['id']
        if owner[sid] != name:
            raise SystemExit(f'state {sid}: pumpkin {name} vs repo {owner[sid]}')
        flags[sid] = (s['state_flags'], s['side_flags'])
missing = [i for i, f in enumerate(flags) if f is None]
if missing:
    raise SystemExit(f'missing states {missing[:10]}')
raw = b''.join(struct.pack('<HB', f, s) for f, s in flags)
with open(out, 'wb') as f:
    f.write(gzip.compress(raw, 9, mtime=0))
print('states', count, 'bytes', len(raw))
