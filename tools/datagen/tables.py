"""Builders for the output tables (blocks, block states, items, tags, ...)."""

import json
import os

import datapack

FLAG_AIR = 1 << 0
FLAG_COLLISION = 1 << 1
FLAG_FULL_CUBE = 1 << 2
FLAG_FLUID = 1 << 3
FLAG_REPLACEABLE = 1 << 4
FLAG_BLOCK_ENTITY = 1 << 5

# bits of Pumpkin's per-state "state_flags" (Pumpkin-MC/Extractor, Blocks.kt)
P_AIR = 1 << 0
P_REPLACEABLE = 1 << 4
P_LIQUID = 1 << 5          # !state.getFluidState().isEmpty()
P_FULL_CUBE = 1 << 7       # state.isCollisionShapeFullBlock(EmptyBlockGetter, ZERO)


def short(name):
    return name[len("minecraft:"):] if name.startswith("minecraft:") else name


def load_json(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


# --------------------------------------------------------------------------
# blocks and block state ids
# --------------------------------------------------------------------------
def build_blocks(block_order, mcmeta_blocks):
    """Block table with state ids.

    States of a block are the cartesian product of its properties sorted by
    name, the first property varying slowest; property values in the order
    given by ``mcmeta_blocks`` (the game's ``getPossibleValues()`` order).
    -> (blocks, state_count, problems)
    """
    problems = []
    blocks = []
    offset = 0
    names = set(short(n) for n in block_order)
    extra = sorted(set(mcmeta_blocks) - names)
    if extra:
        problems.append("blocks in mcmeta blocks/data.json that are not registered: %s" % extra[:10])
    for bid, name in enumerate(block_order):
        entry = mcmeta_blocks.get(short(name))
        if entry is None:
            problems.append("block %s has no entry in mcmeta blocks/data.json" % name)
            return None, None, problems
        props, defaults = entry
        pnames = sorted(props)
        if sorted(defaults) != pnames:
            problems.append("block %s: default state does not cover its properties" % name)
            return None, None, problems
        n = 1
        index = 0
        plist = []
        for p in pnames:
            vals = list(props[p])
            if not vals or len(set(vals)) != len(vals):
                problems.append("block %s: bad value list for property %s" % (name, p))
                return None, None, problems
            if defaults[p] not in vals:
                problems.append("block %s: default value of %s is not a possible value" % (name, p))
                return None, None, problems
            n *= len(vals)
            index = index * len(vals) + vals.index(defaults[p])
            plist.append({"name": p, "values": vals})
        blocks.append({
            "name": name, "id": bid, "min_state": offset, "default_state": offset + index,
            "properties": plist, "item": None, "hardness": None, "blast_resistance": None,
            "_states": n,
        })
        offset += n
    return blocks, offset, problems


def state_properties(block):
    """Yield the property dict of every state of ``block`` in state-id order."""
    props = block["properties"]
    sizes = [len(p["values"]) for p in props]
    total = block["_states"]
    for i in range(total):
        rem = i
        idx = [0] * len(props)
        for k in range(len(props) - 1, -1, -1):
            idx[k] = rem % sizes[k]
            rem //= sizes[k]
        yield {props[k]["name"]: props[k]["values"][idx[k]] for k in range(len(props))}


# --------------------------------------------------------------------------
# Pumpkin assets
# --------------------------------------------------------------------------
class Pumpkin:
    """Read-only view of Pumpkin-MC's generated ``assets/`` directory."""

    def __init__(self, assets_dir):
        self.dir = assets_dir
        self._cache = {}

    def has(self, name):
        return os.path.isfile(os.path.join(self.dir, name))

    def load(self, name):
        if name not in self._cache:
            self._cache[name] = load_json(os.path.join(self.dir, name))
        return self._cache[name]

    def blocks(self):
        return sorted(self.load("blocks.json")["blocks"], key=lambda b: b["id"])

    def shapes(self):
        return self.load("blocks.json")["shapes"]

    def properties(self):
        return {p["hash_key"]: p for p in self.load("properties.json")}


def _pumpkin_prop_values(p):
    t = p["type"]
    if t == "boolean":
        return ["true", "false"]
    if t == "int":
        return [str(i) for i in range(p["min"], p["max"] + 1)]
    return list(p["values"])


def check_blocks_against_pumpkin(blocks, state_count, pumpkin, item_order):
    """Compare ids, state ranges, defaults, properties, item, hardness with Pumpkin.

    -> list of (check name, ok, detail)
    """
    res = []
    pb = pumpkin.blocks()
    mine_names = [short(b["name"]) for b in blocks]
    theirs = [b["name"] for b in pb]
    res.append(("block names and order", mine_names == theirs,
                "%d blocks" % len(blocks) if mine_names == theirs else
                "mine %d, pumpkin %d" % (len(mine_names), len(theirs))))
    if mine_names != theirs:
        return res
    pprops = pumpkin.properties() if pumpkin.has("properties.json") else None
    bad_range = []
    bad_default = []
    bad_props = []
    bad_item = []
    bad_hard = []
    bad_blast = []
    total = 0
    n_prop_checked = 0
    for b, p in zip(blocks, pb):
        ids = [s["id"] for s in p["states"]]
        total += len(ids)
        if ids != list(range(b["min_state"], b["min_state"] + b["_states"])):
            bad_range.append(b["name"])
        if p["default_state_id"] != b["default_state"]:
            bad_default.append(b["name"])
        if pprops is not None:
            try:
                theirs_p = [(pprops[h]["serialized_name"], _pumpkin_prop_values(pprops[h]))
                            for h in p["properties"]]
            except KeyError:
                theirs_p = None
            mine_p = [(q["name"], q["values"]) for q in b["properties"]]
            n_prop_checked += 1
            if theirs_p != mine_p:
                bad_props.append(b["name"])
        item = b["item"]
        want = item_order.index(item) if item is not None else 0
        if p["item_id"] != want:
            bad_item.append(b["name"])
        if b["hardness"] is None or abs(b["hardness"] - p["hardness"]) > 1e-6:
            bad_hard.append(b["name"])
        if b["blast_resistance"] is None or abs(b["blast_resistance"] - p["blast_resistance"]) > 1e-6:
            bad_blast.append(b["name"])

    def r(name, bad, okdetail):
        res.append((name, not bad, okdetail if not bad else "%d differ: %s" % (len(bad), bad[:8])))

    res.append(("total number of block states", total == state_count,
                "%d" % state_count if total == state_count else "mine %d, pumpkin %d" % (state_count, total)))
    r("state id range of every block", bad_range, "all %d blocks" % len(blocks))
    r("default state id of every block", bad_default, "all %d blocks" % len(blocks))
    if pprops is not None:
        r("property names and value order of every block", bad_props, "all %d blocks" % n_prop_checked)
    r("item of every block (Block.asItem())", bad_item, "all %d blocks" % len(blocks))
    r("hardness of every block", bad_hard, "all %d blocks" % len(blocks))
    r("blast resistance of every block", bad_blast, "all %d blocks" % len(blocks))
    return res


# --------------------------------------------------------------------------
# per-state physical properties
# --------------------------------------------------------------------------
def build_block_states(blocks, state_count, infos, pumpkin):
    """-> (table, checks, notes)

    ``table`` = {"luminance": [...], "opacity": [...], "flags": [...]} indexed by state id.

    Sources:
      * bits 0 (air), 4 (replaceable), 5 (block entity): bytecode (``infos``);
      * bit 3 (fluid), bits 1/2 (collision shape), luminance, opacity: Pumpkin's
        per-state dump, cross-checked against what the bytecode does tell us.
    Without Pumpkin the latter are filled with documented defaults (see README).
    """
    lum = [0] * state_count
    opa = [0] * state_count
    flags = [0] * state_count
    checks = []
    notes = []
    pb = None
    if pumpkin is not None:
        pb = pumpkin.blocks()
        if [b["name"] for b in pb] != [short(b["name"]) for b in blocks]:
            checks.append(("pumpkin block list matches", False, "block lists differ - pumpkin data not used"))
            pb = None
    unit_cube = None
    if pb is not None:
        for i, s in enumerate(pumpkin.shapes()):
            if s["min"] == [0.0, 0.0, 0.0] and s["max"] == [1.0, 1.0, 1.0]:
                unit_cube = i
                break
    bad = {"air": [], "replaceable": [], "block_entity": [], "light": [], "fluid_waterlogged": [],
           "fluid_class": [], "no_collision": [], "full_cube_shape": [], "ranges": []}
    n_light_const = 0
    n_light_states = 0          # states whose luminance was derived from the bytecode
    light_unknown = set()
    for bi, b in enumerate(blocks):
        info = infos.get(b["name"])
        base = b["min_state"]
        n = b["_states"]
        f_static = 0
        known = info is not None and info.cls is not None
        if known:
            if info.is_air:
                f_static |= FLAG_AIR
            if info.replaceable:
                f_static |= FLAG_REPLACEABLE
            if info.has_block_entity:
                f_static |= FLAG_BLOCK_ENTITY
        if pb is None:
            # ---- documented fallback (no per-state source available)
            wl = [p for p in b["properties"] if p["name"] == "waterlogged"]
            for k, props in enumerate(state_properties(b)):
                f = f_static
                if known and info.has_collision:
                    f |= FLAG_COLLISION
                if props.get("waterlogged") == "true" or short(b["name"]) in ("water", "lava"):
                    f |= FLAG_FLUID
                flags[base + k] = f
                lv = _bytecode_light(info, props) if known else None
                if lv is None:
                    light_unknown.add(b["name"])
                    lv = 0
                else:
                    n_light_states += 1
                lum[base + k] = lv
                opa[base + k] = 15 if (known and info.has_collision and not (f & FLAG_AIR)) else 0
            continue
        p = pb[bi]
        states = p["states"]
        if len(states) != n or states[0]["id"] != base:
            bad["ranges"].append(b["name"])
            continue
        prop_iter = state_properties(b)
        for k, s in enumerate(states):
            props = next(prop_iter)
            sf = s["state_flags"]
            f = f_static
            if s["collision_shapes"]:
                f |= FLAG_COLLISION
            if sf & P_FULL_CUBE:
                f |= FLAG_FULL_CUBE
            if sf & P_LIQUID:
                f |= FLAG_FLUID
            flags[base + k] = f
            lv = _bytecode_light(info, props) if known else None
            if lv is None:
                light_unknown.add(b["name"])
                lum[base + k] = int(s["luminance"])
            else:
                n_light_states += 1
                lum[base + k] = lv
                if lv != s["luminance"]:
                    bad["light"].append(b["name"])
            opa[base + k] = int(s["opacity"])
            # ---- cross-checks between the two sources
            if known:
                if bool(sf & P_AIR) != bool(info.is_air):
                    bad["air"].append(b["name"])
                if bool(sf & P_REPLACEABLE) != bool(info.replaceable):
                    bad["replaceable"].append(b["name"])
                if ("block_entity_type" in s) != bool(info.has_block_entity):
                    bad["block_entity"].append(b["name"])
                if (sf & P_LIQUID) and info.overrides_fluid_state is False:
                    bad["fluid_class"].append(b["name"])
                if info.has_collision is False and s["collision_shapes"] and \
                        not info_overrides_collision(info):
                    bad["no_collision"].append(b["name"])
            if "waterlogged" in props and bool(sf & P_LIQUID) != (props["waterlogged"] == "true"):
                bad["fluid_waterlogged"].append(b["name"])
            if unit_cube is not None and (s["collision_shapes"] == [unit_cube]) != bool(sf & P_FULL_CUBE):
                bad["full_cube_shape"].append(b["name"])
            if not (0 <= s["luminance"] <= 15 and 0 <= s["opacity"] <= 15):
                bad["ranges"].append(b["name"])
        if known and info.light_const is not None:
            n_light_const += 1

    def r(name, key, okdetail):
        lst = sorted(set(bad[key]))
        checks.append((name, not lst, okdetail if not lst else "%d blocks differ: %s" % (len(lst), lst[:8])))

    if pb is not None:
        nb = len(blocks)
        r("pumpkin: state list of every block lines up / values in range", "ranges", "all %d blocks" % nb)
        r("air flag: bytecode (Properties.isAir) == pumpkin (state.isAir) for every state", "air",
          "all %d states" % state_count)
        r("replaceable flag: bytecode (Properties.replaceable) == pumpkin (canBeReplaced)", "replaceable",
          "all %d states" % state_count)
        r("block entity flag: bytecode (class implements EntityBlock) == pumpkin (a block entity type is valid)",
          "block_entity", "all %d states" % state_count)
        r("luminance: bytecode (Properties.lightEmission evaluated per state) == pumpkin (state.lightEmission)",
          "light", "%d of %d states derived from bytecode (%d blocks with a constant emission), all agree%s" % (
              n_light_states, state_count, n_light_const,
              "" if not light_unknown else "; pumpkin's value used for %d blocks: %s" % (
                  len(light_unknown), sorted(light_unknown)[:6])))
        r("fluid flag == (waterlogged == true) for every state that has the property",
          "fluid_waterlogged", "all waterloggable states")
        r("fluid flag only on blocks whose class overrides getFluidState", "fluid_class", "holds")
        r("blocks built with noCollision() have no collision shape", "no_collision", "holds")
        r("full-cube flag == (collision shape is exactly the unit cube box)", "full_cube_shape",
          "all %d states" % state_count)
    else:
        notes.append("no per-state source: opacity, collision-shape and fluid bits are DEFAULTS (see README); "
                     "luminance derived from bytecode for %d of %d states%s" % (
                         n_light_states, state_count,
                         "" if not light_unknown else ", 0 assumed for %s" % sorted(light_unknown)[:6]))
    return {"luminance": lum, "opacity": opa, "flags": flags}, checks, notes


def _bytecode_light(info, props):
    if info.light_const is not None:
        return info.light_const
    if info.light_fn is not None:
        return info.light_fn(props)
    return None


def info_overrides_collision(info):
    # None (undecidable) counts as "may override"
    return info.overrides_collision is not False


# --------------------------------------------------------------------------
# items
# --------------------------------------------------------------------------
def build_items(item_order, infos, components):
    problems = []
    items = []
    missing = sorted(set(short(n) for n in item_order) - set(components))
    if missing:
        problems.append("items without mcmeta components: %s" % missing[:10])
    for iid, name in enumerate(item_order):
        comp = components.get(short(name))
        stack = comp.get("minecraft:max_stack_size") if comp is not None else None
        if not isinstance(stack, int):
            problems.append("item %s has no minecraft:max_stack_size" % name)
            stack = None
        info = infos.get(name)
        items.append({"name": name, "id": iid, "max_stack_size": stack,
                      "block": info.block if info is not None else None})
    return items, problems


# --------------------------------------------------------------------------
# synchronized registries and tags
# --------------------------------------------------------------------------
def build_synced(pack, synced_order):
    entries = {}
    for rid in synced_order:
        entries[rid] = datapack.registry_entries(pack, rid)
    return {"order": list(synced_order), "entries": entries}


def build_tags(pack, builtin, synced):
    """-> (tags table, problems, per registry stats)

    ``builtin``: registry id -> ordered entries (protocol id = index)
    ``synced``:  the table from build_synced
    Only registries that have at least one tag file are included -- vanilla
    skips registries whose tag payload is empty.
    """
    out = {}
    problems = []
    stats = {}
    sources = []
    for rid, names in builtin.items():
        sources.append((rid, names))
    for rid in synced["order"]:
        sources.append((rid, synced["entries"][rid]))
    for rid, names in sources:
        raw = datapack.raw_tags(pack, rid)
        if not raw:
            continue
        index = {n: i for i, n in enumerate(names)}
        resolved, probs = datapack.resolve_tags(raw, names)
        for p in probs:
            problems.append("%s: %s" % (rid, p))
        table = {}
        for tid in sorted(resolved):
            table[tid] = [index[e] for e in resolved[tid]]
        out[rid] = table
        stats[rid] = (len(raw), len(table))
    return out, problems, stats
