"""Reader for Mojang's OFFICIAL data-generator reports (``generated/reports``).

    java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports

writes ``registries.json``, ``blocks.json`` and ``packets.json``.  This module
turns them into the same in-memory tables the bytecode analysis produces, so
that people who are able to run the generator can use it as the id source.

            *** UNTESTED AGAINST REAL GENERATOR OUTPUT ***

The environment this tool was written in is not allowed to run the server jar,
so this reader has only ever been exercised with *synthetic* reports rebuilt
from the bytecode-derived tables (see selftest_reports.py).  It follows the
report formats as published for 1.21+; verify its output before relying on it.

Formats read:

  registries.json  {"minecraft:block": {"default": .., "protocol_id": 4,
                        "entries": {"minecraft:stone": {"protocol_id": 1}, ..}}, ..}
  blocks.json      {"minecraft:oak_stairs": {"properties": {"facing": ["north", ..], ..},
                        "states": [{"id": 5463, "properties": {..}, "default": true?}, ..]}, ..}
  packets.json     {"play": {"clientbound": {"minecraft:add_entity": {"protocol_id": 1}, ..},
                             "serverbound": {..}}, ..}
"""

import os

from tables import load_json, state_properties


class ReportError(Exception):
    pass


def _ordered(entries, what):
    try:
        pairs = sorted(((v["protocol_id"], k) for k, v in entries.items()))
    except (KeyError, TypeError, AttributeError):
        raise ReportError("%s: entries without protocol_id" % what)
    ids = [i for i, _k in pairs]
    if ids != list(range(len(ids))):
        raise ReportError("%s: protocol ids are not 0..n-1" % what)
    return [k for _i, k in pairs]


def read_registries(path):
    d = load_json(path)
    out = {}
    order = sorted(d.items(), key=lambda kv: kv[1].get("protocol_id", 0))
    for rid, body in order:
        out[rid] = _ordered(body.get("entries", {}), "registry %s" % rid)
    return out


def read_packets(path):
    d = load_json(path)
    out = {}
    for state, flows in d.items():
        out[state] = {}
        for flow, table in flows.items():
            out[state][flow] = _ordered(table, "packets %s/%s" % (state, flow))
    return out


def read_blocks(path, block_order):
    """-> (blocks table in the same shape tables.build_blocks produces, state_count)"""
    d = load_json(path)
    missing = [b for b in block_order if b not in d]
    if missing:
        raise ReportError("blocks.json lacks %s" % missing[:5])
    extra = sorted(set(d) - set(block_order))
    if extra:
        raise ReportError("blocks.json has blocks that are not in the block registry: %s" % extra[:5])
    blocks = []
    offset = 0
    for bid, name in enumerate(block_order):
        body = d[name]
        props = body.get("properties", {})
        states = body.get("states", [])
        if not states:
            raise ReportError("block %s has no states" % name)
        plist = [{"name": p, "values": list(props[p])} for p in sorted(props)]
        n = 1
        for p in plist:
            n *= len(p["values"])
        ids = sorted(s["id"] for s in states)
        if len(states) != n or ids != list(range(ids[0], ids[0] + n)):
            raise ReportError("block %s: states are not a contiguous cartesian product" % name)
        if ids[0] != offset:
            raise ReportError("block %s: first state id %d, expected %d (blocks not in registry order?)"
                              % (name, ids[0], offset))
        b = {"name": name, "id": bid, "min_state": offset, "default_state": None,
             "properties": plist, "item": None, "hardness": None, "blast_resistance": None,
             "_states": n}
        by_id = {s["id"]: s for s in states}
        for k, expect in enumerate(state_properties(b)):
            s = by_id[offset + k]
            if dict(s.get("properties", {})) != expect:
                raise ReportError("block %s: state %d does not follow the sorted-property product order"
                                  % (name, offset + k))
            if s.get("default"):
                if b["default_state"] is not None:
                    raise ReportError("block %s has two default states" % name)
                b["default_state"] = offset + k
        if b["default_state"] is None:
            raise ReportError("block %s has no default state" % name)
        blocks.append(b)
        offset += n
    return blocks, offset


def read_all(directory):
    """-> dict with 'registries', 'packets', 'blocks', 'state_count'"""
    def p(name):
        path = os.path.join(directory, name)
        if not os.path.isfile(path):
            raise ReportError("missing %s" % path)
        return path

    regs = read_registries(p("registries.json"))
    if "minecraft:block" not in regs:
        raise ReportError("registries.json has no minecraft:block registry")
    blocks, count = read_blocks(p("blocks.json"), regs["minecraft:block"])
    return {"registries": regs, "packets": read_packets(p("packets.json")),
            "blocks": blocks, "state_count": count}
