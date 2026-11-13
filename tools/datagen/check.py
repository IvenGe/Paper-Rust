#!/usr/bin/env python3
"""Self-check of the generated tables.

    python3 -I tools/datagen/check.py data/26.3

Re-loads every output file and asserts internal consistency: ids contiguous,
state ranges tile the state space, default states inside their block's range,
every cross reference (block<->item, tag ids, dimension types) resolves.
Exits non-zero and lists every violation if anything is off.  Uses nothing but
the output files themselves.
"""

import json
import os
import sys

FILES = ["version.json", "packets.json", "registries.json", "blocks.json", "block_states.json",
         "items.json", "synced_registries.json", "tags.json", "dimension_types.json",
         "feature_flags.json"]
STATES = ["handshake", "status", "login", "configuration", "play"]
FLOWS = ["serverbound", "clientbound"]
SERIALIZERS = "paper-rs:entity_data_serializer"

FLAG_AIR, FLAG_COLLISION, FLAG_FULL_CUBE, FLAG_FLUID, FLAG_REPLACEABLE, FLAG_BLOCK_ENTITY = (
    1, 2, 4, 8, 16, 32)


class Checker:
    def __init__(self):
        self.errors = []
        self.count = 0

    def ok(self, cond, msg):
        self.count += 1
        if not cond:
            if len(self.errors) < 200:
                self.errors.append(msg)
        return cond


def is_id(s):
    return isinstance(s, str) and ":" in s and s == s.strip() and " " not in s


def unique_ids(c, lst, what):
    if not c.ok(isinstance(lst, list), "%s is not a list" % what):
        return False
    good = c.ok(all(is_id(x) for x in lst), "%s has entries that are not namespaced ids" % what)
    good = c.ok(len(set(lst)) == len(lst), "%s has duplicates" % what) and good
    return good


def check(directory):
    c = Checker()
    d = {}
    for name in FILES:
        path = os.path.join(directory, name)
        if not c.ok(os.path.isfile(path), "missing file %s" % name):
            continue
        try:
            with open(path, "r", encoding="utf-8") as f:
                d[name] = json.load(f)
        except ValueError as e:
            c.ok(False, "%s is not valid JSON: %s" % (name, e))
    if len(d) != len(FILES):
        return c

    # ---- version.json
    v = d["version.json"]
    c.ok(isinstance(v, dict) and set(v) == {"id", "protocol", "data_version", "data_pack_version",
                                            "resource_pack_version"}, "version.json: unexpected keys")
    c.ok(isinstance(v.get("id"), str) and isinstance(v.get("protocol"), int)
         and isinstance(v.get("data_version"), int), "version.json: bad id/protocol/data_version")
    for k in ("data_pack_version", "resource_pack_version"):
        x = v.get(k)
        c.ok(isinstance(x, list) and len(x) == 2 and all(isinstance(i, int) for i in x),
             "version.json: %s is not [major, minor]" % k)

    # ---- packets.json
    p = d["packets.json"]
    c.ok(isinstance(p, dict) and list(p) == STATES, "packets.json: states are not %s" % STATES)
    for s in STATES:
        st = p.get(s, {})
        c.ok(isinstance(st, dict) and set(st) == set(FLOWS), "packets.json: %s lacks a direction" % s)
        for fl in FLOWS:
            unique_ids(c, st.get(fl, []), "packets.json %s/%s" % (s, fl))
    c.ok(p.get("handshake", {}).get("serverbound", [None])[:1] == ["minecraft:intention"],
         "packets.json: handshake/serverbound[0] is not minecraft:intention")
    for s in STATES[1:]:
        for fl in FLOWS:
            c.ok(len(p.get(s, {}).get(fl, [])) > 0, "packets.json: %s/%s is empty" % (s, fl))

    # ---- registries.json
    r = d["registries.json"]
    c.ok(isinstance(r, dict) and len(r) > 0, "registries.json is not an object")
    for rid, lst in r.items():
        c.ok(is_id(rid), "registries.json: bad registry id %r" % (rid,))
        if rid == SERIALIZERS:
            c.ok(isinstance(lst, list) and len(set(lst)) == len(lst) and lst
                 and all(isinstance(x, str) and x == x.lower() for x in lst),
                 "registries.json: bad serializer table")
        else:
            unique_ids(c, lst, "registries.json %s" % rid)
            c.ok(len(lst) > 0, "registries.json: %s is empty" % rid)
    for need in ("minecraft:block", "minecraft:item", "minecraft:entity_type", SERIALIZERS):
        c.ok(need in r, "registries.json: %s is missing" % need)
    block_reg = r.get("minecraft:block", [])
    item_reg = r.get("minecraft:item", [])
    item_index = {n: i for i, n in enumerate(item_reg)}
    block_index = {n: i for i, n in enumerate(block_reg)}

    # ---- blocks.json
    b = d["blocks.json"]
    blocks = b.get("blocks", []) if isinstance(b, dict) else []
    state_count = b.get("state_count") if isinstance(b, dict) else None
    c.ok(isinstance(state_count, int) and state_count > 0, "blocks.json: bad state_count")
    c.ok([x.get("name") for x in blocks] == block_reg, "blocks.json: blocks are not the block registry in order")
    offset = 0
    ranges = []
    keys = {"name", "id", "min_state", "default_state", "properties", "item", "hardness", "blast_resistance"}
    for i, x in enumerate(blocks):
        name = x.get("name")
        c.ok(set(x) == keys, "blocks.json %s: unexpected keys" % name)
        c.ok(x.get("id") == i, "blocks.json %s: id %r != index %d" % (name, x.get("id"), i))
        props = x.get("properties", [])
        pn = [q.get("name") for q in props]
        c.ok(pn == sorted(pn) and len(set(pn)) == len(pn), "blocks.json %s: properties not sorted/unique" % name)
        n = 1
        for q in props:
            vals = q.get("values", [])
            c.ok(isinstance(vals, list) and vals and len(set(vals)) == len(vals)
                 and all(isinstance(s, str) for s in vals), "blocks.json %s: bad values of %s" % (name, q.get("name")))
            n *= max(1, len(vals))
        c.ok(x.get("min_state") == offset, "blocks.json %s: min_state %r, expected %d" % (name, x.get("min_state"), offset))
        ds = x.get("default_state")
        c.ok(isinstance(ds, int) and offset <= ds < offset + n,
             "blocks.json %s: default_state %r outside [%d, %d)" % (name, ds, offset, offset + n))
        it = x.get("item")
        c.ok(it is None or it in item_index, "blocks.json %s: item %r is not a registered item" % (name, it))
        for k in ("hardness", "blast_resistance"):
            val = x.get(k)
            c.ok(val is None or (isinstance(val, (int, float)) and not isinstance(val, bool)),
                 "blocks.json %s: %s is not a number or null" % (name, k))
        ranges.append((name, offset, n))
        offset += n
    c.ok(offset == state_count, "blocks.json: states of all blocks sum to %d, state_count is %r" % (offset, state_count))
    if blocks:
        c.ok(blocks[0].get("name") == "minecraft:air" and blocks[0].get("default_state") == 0,
             "blocks.json: block 0 / state 0 is not minecraft:air")
    stone = [x for x in blocks if x.get("name") == "minecraft:stone"]
    c.ok(bool(stone) and stone[0].get("min_state") == 1, "blocks.json: minecraft:stone is not state 1")

    # ---- block_states.json
    bs = d["block_states.json"]
    c.ok(isinstance(bs, dict) and set(bs) == {"luminance", "opacity", "flags"}, "block_states.json: unexpected keys")
    for k, hi in (("luminance", 15), ("opacity", 15), ("flags", 63)):
        arr = bs.get(k, []) if isinstance(bs, dict) else []
        c.ok(isinstance(arr, list) and len(arr) == state_count,
             "block_states.json: %s has %d entries, state_count is %r" % (k, len(arr), state_count))
        c.ok(all(isinstance(x, int) and not isinstance(x, bool) and 0 <= x <= hi for x in arr),
             "block_states.json: %s has values outside 0..%d" % (k, hi))
    flags = bs.get("flags", []) if isinstance(bs, dict) else []
    if len(flags) == state_count:
        c.ok(all(not (f & FLAG_FULL_CUBE) or (f & FLAG_COLLISION) for f in flags),
             "block_states.json: a state is a full cube without having a collision shape")
        c.ok(all(not (f & FLAG_AIR) or not (f & (FLAG_COLLISION | FLAG_FLUID | FLAG_BLOCK_ENTITY)) for f in flags),
             "block_states.json: an air state has collision / fluid / block entity")
        c.ok(bool(flags) and flags[0] & FLAG_AIR, "block_states.json: state 0 is not air")
        # air / replaceable / block entity are properties of the block: constant over its states
        per_block = FLAG_AIR | FLAG_REPLACEABLE | FLAG_BLOCK_ENTITY
        for name, lo, n in ranges:
            seg = flags[lo:lo + n]
            c.ok(len(set(f & per_block for f in seg)) <= 1,
                 "block_states.json: air/replaceable/block-entity flags differ between states of %s" % name)

    # ---- items.json
    items = d["items.json"].get("items", []) if isinstance(d["items.json"], dict) else []
    c.ok([x.get("name") for x in items] == item_reg, "items.json: items are not the item registry in order")
    for i, x in enumerate(items):
        name = x.get("name")
        c.ok(set(x) == {"name", "id", "max_stack_size", "block"}, "items.json %s: unexpected keys" % name)
        c.ok(x.get("id") == i, "items.json %s: id %r != index %d" % (name, x.get("id"), i))
        ms = x.get("max_stack_size")
        c.ok(isinstance(ms, int) and not isinstance(ms, bool) and 1 <= ms <= 99,
             "items.json %s: max_stack_size %r" % (name, ms))
        blk = x.get("block")
        c.ok(blk is None or blk in block_index, "items.json %s: block %r is not a registered block" % (name, blk))
    # every block's item must be an item, and an item that names a block must be reachable from it
    by_name = {x.get("name"): x for x in blocks}
    for x in items:
        blk = x.get("block")
        if blk is not None and blk in by_name:
            c.ok(by_name[blk].get("item") is not None,
                 "items.json %s places %s but that block has no item" % (x.get("name"), blk))

    # ---- synced_registries.json
    s = d["synced_registries.json"]
    order = s.get("order", []) if isinstance(s, dict) else []
    entries = s.get("entries", {}) if isinstance(s, dict) else {}
    unique_ids(c, order, "synced_registries.json order")
    c.ok(list(entries) == order, "synced_registries.json: entries keys are not the order list")
    for rid in order:
        lst = entries.get(rid, [])
        unique_ids(c, lst, "synced_registries.json %s" % rid)
        c.ok(lst == sorted(lst), "synced_registries.json %s: entries are not sorted" % rid)
        c.ok(rid not in r, "synced_registries.json: %s is also a built-in registry" % rid)

    # ---- tags.json
    t = d["tags.json"]
    c.ok(isinstance(t, dict), "tags.json is not an object")
    for rid, table in (t.items() if isinstance(t, dict) else []):
        if rid in r and rid != SERIALIZERS:
            size = len(r[rid])
        elif rid in entries:
            size = len(entries[rid])
        else:
            c.ok(False, "tags.json: %s is neither a built-in nor a synchronized registry" % rid)
            continue
        c.ok(isinstance(table, dict) and len(table) > 0, "tags.json: %s has no tags" % rid)
        for tid, ids in table.items():
            c.ok(is_id(tid), "tags.json %s: bad tag id %r" % (rid, tid))
            c.ok(isinstance(ids, list) and all(isinstance(i, int) and not isinstance(i, bool)
                                                and 0 <= i < size for i in ids),
                 "tags.json %s %s: id out of range 0..%d" % (rid, tid, size - 1))
            c.ok(len(set(ids)) == len(ids), "tags.json %s %s: duplicate ids" % (rid, tid))
    for need in ("minecraft:block", "minecraft:item"):
        c.ok(need in t, "tags.json: no tags for %s" % need)

    # ---- dimension_types.json
    dt = d["dimension_types.json"]
    c.ok(isinstance(dt, dict) and len(dt) > 0, "dimension_types.json is empty")
    if "minecraft:dimension_type" in entries:
        c.ok(sorted(dt) == sorted(entries["minecraft:dimension_type"]),
             "dimension_types.json keys differ from the dimension_type entries in synced_registries.json")
    for did, body in (dt.items() if isinstance(dt, dict) else []):
        my, h = body.get("min_y"), body.get("height")
        c.ok(isinstance(my, int) and isinstance(h, int) and my % 16 == 0 and h % 16 == 0 and h > 0,
             "dimension_types.json %s: bad min_y / height (%r, %r)" % (did, my, h))

    # ---- feature_flags.json
    ff = d["feature_flags.json"]
    unique_ids(c, ff, "feature_flags.json")
    c.ok(isinstance(ff, list) and len(ff) > 0, "feature_flags.json is empty")
    return c


def main(argv):
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    c = check(argv[1])
    if c.errors:
        for e in c.errors:
            sys.stdout.write("FAIL " + e + "\n")
        sys.stdout.write("%d of %d assertions FAILED for %s\n" % (len(c.errors), c.count, argv[1]))
        return 1
    sys.stdout.write("all %d assertions hold for %s\n" % (c.count, argv[1]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
