#!/usr/bin/env python3
"""Generate the version data tables for paper-rs from the vanilla server jar.

    python3 -I tools/datagen/extract.py \\
        --jar      <server.jar | inner server-<ver>.jar> \\
        --mcmeta   <misode/mcmeta "summary" checkout> \\
        --pumpkin  <Pumpkin-MC/Pumpkin checkout>/assets \\
        --out      data/<version>

Nothing from the jar is executed or loaded into a JVM: class files are decoded
and analysed statically (see symexec.py).  Standard library only.  All inputs
are treated as untrusted data; run with ``python3 -I``.

See README.md for provenance, validation and the (untested) ``--reports`` mode.
"""

import argparse
import io
import json
import os
import sys
import threading
import time
import zipfile

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import classfile as cf  # noqa: E402
import crosscheck  # noqa: E402
import datapack  # noqa: E402
import reports as reports_mod  # noqa: E402
import tables  # noqa: E402
import vanilla  # noqa: E402

SERIALIZER_REGISTRY = "paper-rs:entity_data_serializer"

OUTPUTS = ["version.json", "packets.json", "registries.json", "blocks.json", "block_states.json",
           "items.json", "synced_registries.json", "tags.json", "dimension_types.json",
           "feature_flags.json"]

STATES = ["handshake", "status", "login", "configuration", "play"]
FLOWS = ["serverbound", "clientbound"]


# --------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------
class Report:
    """Collects check results.  status: True = ok, False = FAILED, None = informational."""

    def __init__(self):
        self.sections = []
        self.fatal = []

    def section(self, title, rows):
        self.sections.append((title, [(n, s, d) for n, s, d in rows]))

    def failures(self):
        return [(t, n, d) for t, rows in self.sections for n, s, d in rows if s is False]

    def dump(self, out):
        for title, rows in self.sections:
            fails = sum(1 for _n, s, _d in rows if s is False)
            oks = sum(1 for _n, s, _d in rows if s is True)
            out.write("\n== %s  [%d ok, %d FAILED]\n" % (title, oks, fails))
            for n, s, d in rows:
                tag = {True: "ok  ", False: "FAIL", None: "info"}[s]
                out.write("  %s %s%s\n" % (tag, n, (": " + d) if d else ""))
        if self.fatal:
            out.write("\n== FATAL\n")
            for f in self.fatal:
                out.write("  " + f + "\n")

    def to_json(self):
        return {
            "sections": [{"title": t, "checks": [{"name": n, "status": {True: "ok", False: "FAILED", None: "info"}[s],
                                                    "detail": d} for n, s, d in rows]}
                         for t, rows in self.sections],
            "fatal": list(self.fatal),
        }


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------
def open_server_jar(path):
    """Open either the bundler ``server.jar`` or the inner ``server-<version>.jar``."""
    zf = zipfile.ZipFile(path)
    names = set(zf.namelist())
    if "net/minecraft/server/Bootstrap.class" in names:
        return zf, path
    inner = sorted(n for n in names if n.startswith("META-INF/versions/") and n.endswith(".jar"))
    if not inner:
        raise SystemExit("%s is neither a vanilla server jar nor a bundler jar" % path)
    data = zf.read(inner[0])
    return zipfile.ZipFile(io.BytesIO(data)), "%s!/%s" % (path, inner[0])


def dumps(obj):
    return json.dumps(obj, separators=(",", ":"), ensure_ascii=False) + "\n"


def sha256_of(path):
    import hashlib
    h = hashlib.sha256()
    try:
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    except OSError:
        return None
    return h.hexdigest()


def write_outputs(out_dir, files):
    os.makedirs(out_dir, exist_ok=True)
    written = []
    for name in OUTPUTS:
        if name not in files:
            continue
        data = dumps(files[name]).encode("utf-8")
        path = os.path.join(out_dir, name)
        tmp = path + ".tmp"
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
        written.append((name, len(data)))
    return written


def read_version(zf):
    try:
        v = json.loads(zf.read("version.json").decode("utf-8"))
    except KeyError:
        raise SystemExit("the jar has no version.json")
    pv = v.get("pack_version", {})
    return {
        "id": v["id"],
        "protocol": v["protocol_version"],
        "data_version": v["world_version"],
        "data_pack_version": [pv["data_major"], pv.get("data_minor", 0)],
        "resource_pack_version": [pv["resource_major"], pv.get("resource_minor", 0)],
    }


def tag_network_checks(cp):
    """Which registries' tags does vanilla serialise to the network?  Checked on the bytecode."""
    rows = []

    def calls_and_statics(cname, mname):
        c = cp.get(cname)
        found = []
        if c is None:
            return None
        for m in c.methods_named(mname):
            for ins in m.insns():
                if ins.op in ("invokestatic", "invokevirtual", "invokeinterface"):
                    _, owner, name, _d = c.member_ref(ins.arg)
                    found.append("%s.%s" % (owner.rsplit("/", 1)[-1], name))
                elif ins.op == "getstatic":
                    _, owner, name, _d = c.member_ref(ins.arg)
                    found.append("%s.%s" % (owner.rsplit("/", 1)[-1], name))
                elif ins.op == "invokedynamic":
                    bidx, _n, _d = c.invoke_dynamic(ins.arg)
                    ref, args = c.bootstrap_methods[bidx]
                    if c.method_handle(ref)[1] == "java/lang/invoke/LambdaMetafactory":
                        h = c.method_handle(args[1])
                        found.append("lambda:%s.%s" % (h[1].rsplit("/", 1)[-1], h[2]))
        return found

    tns = "net/minecraft/tags/TagNetworkSerialization"
    rs = "net/minecraft/core/RegistrySynchronization"
    a = calls_and_statics(tns, "serializeTagsToNetwork")
    rows.append(("TagNetworkSerialization.serializeTagsToNetwork starts from "
                 "RegistrySynchronization.networkSafeRegistries",
                 bool(a) and "RegistrySynchronization.networkSafeRegistries" in a, ""))
    f = calls_and_statics(tns, "lambda$serializeTagsToNetwork$1")
    rows.append(("... and drops registries whose tag payload is empty (NetworkPayload.isEmpty)",
                 bool(f) and "TagNetworkSerialization$NetworkPayload.isEmpty" in f, ""))
    b = calls_and_statics(rs, "networkSafeRegistries")
    rows.append(("networkSafeRegistries = all registries of RegistryLayer.STATIC (the built-in ones) "
                 "+ networkedRegistries(...)",
                 bool(b) and "RegistryLayer.STATIC" in b and "RegistrySynchronization.networkedRegistries" in b
                 and "Stream.concat" in b, ""))
    c1 = calls_and_statics(rs, "networkedRegistries")
    c2 = calls_and_statics(rs, "ownedNetworkableRegistries")
    c3 = calls_and_statics(rs, "isNetworkable")
    rows.append(("networkedRegistries = registries from RegistryLayer.WORLD upwards filtered by isNetworkable "
                 "(key in NETWORKABLE_REGISTRIES)",
                 bool(c1) and "RegistryLayer.WORLD" in c1 and bool(c2)
                 and "lambda:RegistrySynchronization.lambda$ownedNetworkableRegistries$0" in c2
                 and bool(c3) and "RegistrySynchronization.NETWORKABLE_REGISTRIES" in c3, ""))
    d = calls_and_statics(rs, "<clinit>")
    rows.append(("NETWORKABLE_REGISTRIES = keys of RegistryDataLoader.SYNCHRONIZED_REGISTRIES",
                 bool(d) and "RegistryDataLoader.SYNCHRONIZED_REGISTRIES" in d, ""))
    tl = cp.get("net/minecraft/tags/TagLoader")
    ok = False
    if tl is not None:
        for m in tl.methods_named("tryBuildTag"):
            news = [tl.class_name(i.arg) for i in m.insns() if i.op == "new"]
            calls = [tl.member_ref(i.arg)[2] for i in m.insns() if i.op == "invokevirtual"]
            ok = "java/util/LinkedHashSet" in news and "build" in calls
    rows.append(("TagLoader.tryBuildTag collects entries in file order into a LinkedHashSet", ok, ""))
    return rows


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------
def run(args):
    t0 = time.time()
    rep = Report()

    def log(msg):
        if not args.quiet:
            sys.stderr.write("[%6.1fs] %s\n" % (time.time() - t0, msg))

    files = {}
    zf, jar_desc = open_server_jar(args.jar)
    version = read_version(zf)
    files["version.json"] = version
    log("jar %s: Minecraft %s, protocol %d, data version %d" % (
        jar_desc, version["id"], version["protocol"], version["data_version"]))

    # ---------------------------------------------------------------- bytecode
    van = vanilla.analyse(zf, log=log)
    cp = cf.ClassPath(zf)
    rep.section("bytecode: structural facts the id rules rest on",
                [(d, ok, detail) for d, ok, detail in van.structure_checks]
                + [("Registries.elementsDirPath/tagsDirPath are '<path>' and 'tags/<path>'",
                    not datapack.verify_dir_paths(cp), "; ".join(datapack.verify_dir_paths(cp)))]
                + tag_network_checks(cp))
    rows = []
    for unit, msg, trace in van.failures:
        rows.append(("unit %s failed" % unit, False, msg + (" @ " + trace[-1] if trace else "")))
    for unit, msg in van.warnings:
        rows.append(("unit %s" % unit, None, msg))
    multi = {k: u for k, u in van.registry_units.items() if len(u) > 1}
    for rid, units in multi.items():
        rows.append(("registry %s is filled from more than one class initialiser / bootstrap" % rid, False,
                     "units %s -- the interleaving is not established by this tool" % units))
    rows.append(("every registry is filled by exactly one class initialiser / bootstrap method",
                 not multi, "%d registries" % len(van.registry_units)))
    rows.append(("system properties consulted by the analysed code (modelled as not set)", None,
                 ", ".join(van.env_queries) or "none"))
    rows.append(("instructions evaluated", None, str(van.insn_count)))
    if args.double_check:
        log("second analysis with deferred class initialisation ...")
        van2 = vanilla.analyse(zf, lazy_init=True)
        diffs = vanilla.differences(van, van2)
        rows.append(("second analysis with a different evaluation strategy (deferred class initialisation) "
                     "gives identical registries, packets, serializers, block and item data",
                     not diffs and not van2.failures, "; ".join(diffs[:6])
                     or ("the second analysis had failures" if van2.failures else "")))
    rep.section("bytecode: evaluation of Bootstrap.bootStrap()", rows)
    failed_units = set(u for u, _m, _t in van.failures)

    # ---------------------------------------------------------------- mcmeta
    mc_reg = tables.load_json(os.path.join(args.mcmeta, "registries", "data.json"))
    mc_blocks = tables.load_json(os.path.join(args.mcmeta, "blocks", "data.json"))
    mc_items = tables.load_json(os.path.join(args.mcmeta, "item_components", "data.json"))
    rows = []
    mv_path = os.path.join(args.mcmeta, "version.json")
    if os.path.isfile(mv_path):
        mv = tables.load_json(mv_path)
        same = (mv.get("id") == version["id"] and mv.get("protocol_version") == version["protocol"]
                and mv.get("data_version") == version["data_version"]
                and [mv.get("data_pack_version"), mv.get("data_pack_version_minor", 0)] == version["data_pack_version"]
                and [mv.get("resource_pack_version"), mv.get("resource_pack_version_minor", 0)]
                == version["resource_pack_version"])
        rows.append(("mcmeta summary is for the same version as the jar (id, protocol, data version, pack versions)",
                     same, "%s / %s" % (mv.get("id"), version["id"])))
        if not same:
            rep.fatal.append("the mcmeta summary is for a different version than the jar")
    else:
        rows.append(("mcmeta version.json", None, "not present, version match not checked"))
    inputs = [("server jar", args.jar),
              ("mcmeta registries/data.json", os.path.join(args.mcmeta, "registries", "data.json")),
              ("mcmeta blocks/data.json", os.path.join(args.mcmeta, "blocks", "data.json")),
              ("mcmeta item_components/data.json", os.path.join(args.mcmeta, "item_components", "data.json"))]
    if args.pumpkin:
        inputs.append(("pumpkin assets/blocks.json", os.path.join(args.pumpkin, "blocks.json")))
    if args.reports:
        for n in ("registries.json", "blocks.json", "packets.json"):
            inputs.append(("official report %s" % n, os.path.join(args.reports, n)))
    for label, path in inputs:
        rows.append(("sha256 of %s" % label, None, "%s  (%s)" % (sha256_of(path), path)))
    rep.section("inputs", rows)

    # ---------------------------------------------------------------- registries
    registries = dict(van.registries)
    source = "bytecode"
    official = None
    if args.reports:
        try:
            official = reports_mod.read_all(args.reports)
        except (reports_mod.ReportError, OSError, ValueError) as e:
            rep.fatal.append("--reports: %s" % e)
    if official is not None:
        rows = []
        for rid, names in official["registries"].items():
            mine = van.registries.get(rid)
            if mine is None:
                rows.append((rid, None, "only in the official report (%d entries)" % len(names)))
            else:
                rows.append((rid, mine == names, "%d entries, identical order" % len(names) if mine == names
                             else "bytecode-derived order differs from the official report"))
        for rid in van.registries:
            if rid not in official["registries"]:
                rows.append((rid, None, "derived from bytecode but not in the official report"))
        for state in STATES:
            for flow in FLOWS:
                a = official["packets"].get(state, {}).get(flow, [])
                b = van.packets.get(state, {}).get(flow, [])
                rows.append(("packets %s/%s" % (state, flow), a == b,
                             "%d packets, identical" % len(a) if a == b else "bytecode-derived list differs"))
        rep.section("official generator reports vs bytecode (reports are used for the ids)", rows)
        registries = dict(official["registries"])
        source = "official reports"

    valid, rows = crosscheck.registries_vs_mcmeta(registries, mc_reg)
    valid = [r for r in valid if not (set(van.registry_units.get(r, [])) & failed_units)]
    for rid in van.root_order:
        if rid not in registries:
            rows.append((rid, False, "built-in registry was created but nothing was registered in it"))
    rep.section("registries (%s) vs mcmeta: identical set, count, no duplicates" % source, rows)
    order = [r for r in van.root_order if r in valid] + [r for r in registries if r in valid
                                                         and r not in van.root_order]
    reg_out = {rid: registries[rid] for rid in order}
    omitted = [r for r in registries if r not in valid]
    if van.serializers and not van.serializer_problems:
        reg_out[SERIALIZER_REGISTRY] = [s.lower() for s in van.serializers]
    rows = [("registries written", None, "%d built-in + %s" % (
        len(order), SERIALIZER_REGISTRY if SERIALIZER_REGISTRY in reg_out else "no serializer table"))]
    for r in omitted:
        rows.append(("registry %s OMITTED" % r, False, "did not pass validation"))
    for p in van.serializer_problems:
        rows.append(("entity data serializers", False, p))
    rep.section("registries.json", rows)
    files["registries.json"] = reg_out

    # ---------------------------------------------------------------- packets
    packets = official["packets"] if official is not None else van.packets
    rows = [("packet extraction", False, p) for p in van.packet_problems]
    pk_out = {}
    for state in STATES:
        pk_out[state] = {}
        for flow in FLOWS:
            pk_out[state][flow] = list(packets.get(state, {}).get(flow, []))
    extra_states = sorted(set(packets) - set(STATES))
    if extra_states:
        rows.append(("unexpected connection states", False, str(extra_states)))
    rows.append(("packets per state (serverbound/clientbound)", None, ", ".join(
        "%s %d/%d" % (s, len(pk_out[s]["serverbound"]), len(pk_out[s]["clientbound"])) for s in STATES)))
    if not van.packet_problems or official is not None:
        files["packets.json"] = pk_out
    rep.section("packets.json", rows)

    # ---------------------------------------------------------------- blocks / items
    have_blocks = "minecraft:block" in reg_out and "minecraft:item" in reg_out
    blocks = None
    state_count = None
    if not have_blocks:
        rep.fatal.append("block / item registry not available: blocks.json, items.json, "
                         "block_states.json and tags.json cannot be produced")
    else:
        block_order = reg_out["minecraft:block"]
        item_order = reg_out["minecraft:item"]
        if official is not None:
            blocks, state_count = official["blocks"], official["state_count"]
            b2, n2, probs = tables.build_blocks(block_order, mc_blocks)
            same = b2 is not None and n2 == state_count and all(
                x["min_state"] == y["min_state"] and x["default_state"] == y["default_state"]
                and x["properties"] == y["properties"] for x, y in zip(blocks, b2))
            rep.section("block states: official report vs (registry order x mcmeta properties)",
                        [("state ids, defaults and property value order agree", same, "; ".join(probs))])
        else:
            blocks, state_count, probs = tables.build_blocks(block_order, mc_blocks)
            if blocks is None:
                rep.fatal.append("block state table: " + "; ".join(probs))
            elif probs:
                rep.section("blocks", [("mcmeta blocks/data.json", False, p) for p in probs])
    if blocks is not None:
        rows = []
        for b in blocks:
            info = van.blocks.get(b["name"])
            if info is not None:
                b["hardness"] = info.hardness
                b["blast_resistance"] = info.blast_resistance
            b["item"] = van.block_to_item.get(b["name"])
        anchors = {x["name"]: x for x in blocks}
        air, stone, grass = (anchors.get("minecraft:" + n) for n in ("air", "stone", "grass_block"))
        rows.append(("anchor: minecraft:air is block 0 / state 0", bool(air) and air["id"] == 0
                     and air["min_state"] == 0, ""))
        rows.append(("anchor: minecraft:stone is state 1", bool(stone) and stone["min_state"] == 1
                     and stone["default_state"] == 1, ""))
        gdef = None
        if grass:
            idx = grass["default_state"] - grass["min_state"]
            gdef = list(tables.state_properties(grass))[idx]
        rows.append(("anchor: default state of minecraft:grass_block is snowy=false",
                     gdef == {"snowy": "false"}, str(gdef)))
        nulls = [b["name"] for b in blocks if b["hardness"] is None or b["blast_resistance"] is None]
        rows.append(("hardness and blast resistance derived for every block", not nulls,
                     "%d blocks" % len(blocks) if not nulls else "null for %d: %s" % (len(nulls), nulls[:8])))
        rows.append(("blocks with an item (Item.BY_BLOCK)", None,
                     "%d of %d" % (sum(1 for b in blocks if b["item"]), len(blocks))))
        rows.append(("total block states", None, str(state_count)))
        for p in van.cross_ref_problems:
            rows.append(("block/item cross references", False, p))
        notes = [(b.name, b.notes) for b in van.blocks.values() if b.notes]
        for n, ns in notes[:20]:
            rows.append(("block %s" % n, False, "; ".join(ns)))
        rep.section("blocks.json", rows)
        files["blocks.json"] = {
            "state_count": state_count,
            "blocks": [{k: v for k, v in b.items() if not k.startswith("_")} for b in blocks],
        }

        items, probs = tables.build_items(item_order, van.items, mc_items)
        rows = [("mcmeta item components", False, p) for p in probs]
        bad = [i["name"] for i in items if i["block"] is not None and i["block"] not in anchors]
        rows.append(("every item->block reference is a registered block", not bad, str(bad[:5]) if bad else
                     "%d block items" % sum(1 for i in items if i["block"])))
        # an item that places a block must be what that block answers for asItem(), except
        # where vanilla maps several blocks to one item or several items to one block
        back = [i["name"] for i in items if i["block"] is not None
                and van.block_to_item.get(i["block"]) != i["name"]]
        rows.append(("items whose block does not map back to them via Block.asItem()", None,
                     "%d: %s" % (len(back), back[:12])))
        rep.section("items.json", rows)
        if not probs:
            files["items.json"] = {"items": items}
        else:
            rep.fatal.append("items.json: " + "; ".join(probs[:3]))

    # ---------------------------------------------------------------- pumpkin
    pumpkin = tables.Pumpkin(args.pumpkin) if args.pumpkin else None
    if pumpkin is not None and not pumpkin.has("blocks.json"):
        rep.fatal.append("--pumpkin %s has no blocks.json" % args.pumpkin)
        pumpkin = None
    if pumpkin is not None:
        rep.section("independent order check: registries vs Pumpkin extractor dumps",
                    crosscheck.registries_vs_pumpkin(reg_out, pumpkin))
        if blocks is not None:
            rep.section("independent check: block table vs Pumpkin blocks.json",
                        tables.check_blocks_against_pumpkin(blocks, state_count, pumpkin,
                                                            reg_out["minecraft:item"]))
        if van.serializers:
            rep.section("independent check: entity data serializers",
                        crosscheck.serializers_vs_pumpkin(van.serializers, pumpkin))
    if blocks is not None:
        table, checks, notes = tables.build_block_states(blocks, state_count, van.blocks, pumpkin)
        rows = list(checks) + [("note", None, n) for n in notes]
        rows.append(("source of luminance / opacity / collision and fluid bits", None,
                     "Pumpkin assets/blocks.json" if pumpkin is not None and not notes else
                     "DEFAULTS (no per-state source given)"))
        rep.section("block_states.json", rows)
        files["block_states.json"] = table

    # ---------------------------------------------------------------- packets cross-checks
    rows = []
    refs = []
    if args.pumpkin_packets:
        refs.append((args.pumpkin_packets, "pumpkin-packets.json"))
    if pumpkin is not None and pumpkin.has("packets.json"):
        refs.append((os.path.join(pumpkin.dir, "packets.json"), "pumpkin assets/packets.json"))
    for path, label in refs:
        r, ver = crosscheck.packets_vs_pumpkin(pk_out, path, label)
        rows.append(("%s declares protocol version" % label, ver == version["protocol"], str(ver)))
        rows.extend(r)
    if args.wiki:
        rows.extend(crosscheck.packets_vs_wiki(pk_out, os.path.join(args.wiki, "packets.wiki")))
    if rows:
        rep.section("independent check: packet ids", rows)
    if args.wiki:
        rep.section("independent check: numeric id tables on the wiki pages",
                    crosscheck.registries_vs_wiki_tables(reg_out, args.wiki))
    if args.wiki and van.serializers:
        rep.section("independent check: metadata types on the wiki",
                    crosscheck.serializers_vs_wiki(van.serializers, os.path.join(args.wiki, "Entity_metadata.wiki")))
    if args.burger:
        rep.section("independent check: Burger dump", crosscheck.vs_burger(reg_out, args.burger))
    if args.minecraft_data:
        rep.section("sanity only: older PrismarineJS minecraft-data",
                    crosscheck.vs_minecraft_data(reg_out, blocks, args.minecraft_data))

    # ---------------------------------------------------------------- data pack content
    pack = datapack.DataPack(zf)
    if van.synced_registries is None:
        rep.fatal.append("RegistryDataLoader.SYNCHRONIZED_REGISTRIES could not be evaluated")
    else:
        synced = tables.build_synced(pack, van.synced_registries)
        rows = crosscheck.sets_vs_mcmeta(synced["entries"], mc_reg, "entries")
        empty = [r for r in synced["order"] if not synced["entries"][r]]
        rows.append(("synchronized registries without any entry in the jar", None, str(empty)))
        rep.section("synced_registries.json (entries vs mcmeta)", rows)
        files["synced_registries.json"] = synced
        if have_blocks:
            builtin = {rid: reg_out[rid] for rid in order}
            tags, probs, stats = tables.build_tags(pack, builtin, synced)
            rows = [("tag resolution", False, p) for p in probs]
            for rid, table in tags.items():
                exp = mc_reg.get("tag/" + tables.short(rid))
                got = sorted(tables.short(t) for t in table)
                if exp is None:
                    rows.append((rid, None, "%d tags (mcmeta has no tag list for it)" % len(got)))
                else:
                    rows.append((rid, got == sorted(exp), "%d tags, same names as mcmeta" % len(got)
                                 if got == sorted(exp) else "tag names differ from mcmeta: %s" %
                                 sorted(set(got) ^ set(exp))[:6]))
            other = sorted(k for k in mc_reg if k.startswith("tag/") and
                           "minecraft:" + k[4:] not in tags)
            rows.append(("registries that have tags in the data pack but are NOT sent "
                         "(neither built-in nor synchronized)", None, ", ".join(other) or "none"))
            rep.section("tags.json", rows)
            files["tags.json"] = tags
            if args.burger:
                names_of = dict(builtin)
                names_of.update(synced["entries"])
                raw_cache = {}

                def raw_of(rid):
                    if rid not in raw_cache:
                        raw_cache[rid] = datapack.raw_tags(pack, rid)
                    return raw_cache[rid]

                rep.section("independent check: tag resolution vs Burger",
                            crosscheck.tags_vs_burger(tags, names_of, raw_of, args.burger))
    dims = datapack.dimension_types(pack)
    rep.section("dimension_types.json", [("dimension types", bool(dims), ", ".join(dims))])
    if dims:
        files["dimension_types.json"] = dims
    rows = [("FeatureFlags", None, n) for n in van.feature_flag_notes]
    if van.default_feature_flags is None:
        rep.fatal.append("default feature flags could not be evaluated")
    else:
        rows.append(("flags contained in FeatureFlags.DEFAULT_FLAGS (what REGISTRY.toNames(DEFAULT_FLAGS) yields)",
                     True, str(van.default_feature_flags)))
        rows.append(("all known feature flags", None, str(van.all_feature_flags)))
        files["feature_flags.json"] = van.default_feature_flags
    rep.section("feature_flags.json", rows)

    # ---------------------------------------------------------------- write
    missing = [n for n in OUTPUTS if n not in files]
    for n in missing:
        rep.fatal.append("%s was NOT produced" % n)
    written = write_outputs(args.out, files)
    rep.section("output (%s)" % args.out, [(n, None, "%d bytes" % size) for n, size in written])

    if not args.quiet:
        rep.dump(sys.stdout)
    report_path = args.report
    if report_path is None:
        report_path = os.path.join(HERE, "reports", "%s-validation.json" % version["id"])
    if report_path != "-":
        os.makedirs(os.path.dirname(os.path.abspath(report_path)), exist_ok=True)
        doc = rep.to_json()
        doc["version"] = version
        doc["id_source"] = source
        with open(report_path, "w", encoding="utf-8") as f:
            json.dump(doc, f, indent=1, ensure_ascii=False)
            f.write("\n")
    fails = rep.failures()
    sys.stdout.write("\n%d outputs written to %s; %d failed checks, %d fatal problems (%.1fs)\n" % (
        len(written), args.out, len(fails), len(rep.fatal), time.time() - t0))
    if rep.fatal:
        return 2
    return 1 if fails else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--jar", required=True, help="vanilla server.jar (bundler) or the inner server-<version>.jar")
    ap.add_argument("--mcmeta", required=True, help="checkout of misode/mcmeta at tag <version>-summary")
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--pumpkin", help="Pumpkin-MC/Pumpkin 'assets' directory: per-state block physics + cross-checks")
    ap.add_argument("--pumpkin-packets", help="a Pumpkin packets.json to compare packet ids with")
    ap.add_argument("--wiki", help="directory with packets.wiki / Entity_metadata.wiki (cross-check only)")
    ap.add_argument("--burger", help="a Burger dump json (cross-check only)")
    ap.add_argument("--minecraft-data", help="PrismarineJS minecraft-data directory of an OLDER version (sanity only)")
    ap.add_argument("--reports", help="UNTESTED: 'generated/reports' of Mojang's data generator; "
                                      "its registries/blocks/packets ids are used instead of the bytecode-derived ones")
    ap.add_argument("--report", help="where to write the validation report json ('-' for none; "
                                     "default tools/datagen/reports/<version>-validation.json)")
    ap.add_argument("--double-check", action="store_true",
                    help="analyse the bytecode a second time with a different evaluation strategy and "
                         "require identical results (doubles the run time)")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)
    result = []

    def target():
        try:
            result.append(run(args))
        except SystemExit as e:
            result.append(e)
        except BaseException as e:      # noqa: B902 - re-raised in the main thread
            result.append(e)

    threading.stack_size(512 * 1024 * 1024)
    t = threading.Thread(target=target)
    t.start()
    t.join()
    r = result[0] if result else 3
    if isinstance(r, BaseException):
        raise r
    return r


if __name__ == "__main__":
    sys.exit(main())
