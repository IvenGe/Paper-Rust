"""Everything that is derived from the bytecode of the vanilla server jar.

``analyse(zipfile)`` walks ``net.minecraft.server.Bootstrap.bootStrap()`` with
the symbolic evaluator (symexec.py) and returns a ``Vanilla`` object holding

  * the built-in registries in registration (= protocol id) order,
  * the packet ids per connection state and direction,
  * the entity-data serializer ids,
  * per block: class, hardness, blast resistance and a few simple flags,
  * per item: class and the block it places; the ``Item.BY_BLOCK`` map,
  * the default feature flags and the list of synchronized registries.

No class of the jar is loaded into a JVM; see symexec.py for how the
evaluation works and what it refuses to do.
"""

import classfile as cf
import intrinsics
import registries
from symexec import (Evaluator, Abort, Unknown, Obj, Lambda, Ident, RKey, PyFn, VOID, f32_repr,
                     JBool)

EDS = "net/minecraft/network/syncher/EntityDataSerializers"
EDS_SINK = ("registerSerializer", "(Lnet/minecraft/network/syncher/EntityDataSerializer;)V")
PACKET_REPORT = "net/minecraft/data/info/PacketReport"
FEATURE_FLAGS = "net/minecraft/world/flag/FeatureFlags"
REGISTRY_DATA_LOADER = "net/minecraft/resources/RegistryDataLoader"
ITEM = "net/minecraft/world/item/Item"
BLOCK_ITEM = "net/minecraft/world/item/BlockItem"
ENTITY_BLOCK = "net/minecraft/world/level/block/EntityBlock"
BLOCK_BEHAVIOUR = "net/minecraft/world/level/block/state/BlockBehaviour"
STATE_DEFINITION = "net/minecraft/world/level/block/state/StateDefinition"

ROOT = "minecraft:root"


class BlockInfo:
    __slots__ = ("name", "cls", "hardness", "blast_resistance", "is_air", "replaceable",
                 "has_collision", "has_block_entity", "light_const", "overrides_fluid_state",
                 "overrides_collision", "light_fn", "notes")

    def __init__(self, name):
        self.name = name
        self.cls = None
        self.hardness = None
        self.blast_resistance = None
        self.is_air = None
        self.replaceable = None
        self.has_collision = None
        self.has_block_entity = None
        self.light_const = None             # int when the light emission is state independent
        self.overrides_fluid_state = None
        self.overrides_collision = None
        self.light_fn = None                # props dict -> int | None, for state dependent emission
        self.notes = []


class ItemInfo:
    __slots__ = ("name", "cls", "block", "is_block_item")

    def __init__(self, name):
        self.name = name
        self.cls = None
        self.block = None           # name of the block a BlockItem places
        self.is_block_item = None


class Vanilla:
    def __init__(self):
        self.registries = {}        # registry id -> [entry ids] in protocol-id order
        self.registry_units = {}    # registry id -> [units that registered entries]
        self.root_order = []        # built-in registries in creation order
        self.failures = []
        self.warnings = []
        self.env_queries = []
        self.soft_failures = {}
        self.packets = {}           # state -> flow -> [packet ids]
        self.packet_problems = []
        self.serializers = []       # field names in id order
        self.serializer_problems = []
        self.blocks = {}            # block id -> BlockInfo
        self.items = {}             # item id -> ItemInfo
        self.block_to_item = {}     # block id -> item id   (Item.BY_BLOCK, i.e. Block.asItem())
        self.cross_ref_problems = []
        self.default_feature_flags = None
        self.all_feature_flags = None
        self.feature_flag_notes = []
        self.synced_registries = None
        self.structure_checks = []  # (description, ok, detail)
        self.insn_count = 0


def _f(x):
    """float32 held as a Python float -> the shortest decimal with the same float32 value."""
    return float(f32_repr(x))


def analyse(zf, log=None, lazy_init=False):
    cp = cf.ClassPath(zf)
    ev = Evaluator(cp, lazy_init=lazy_init)
    out = Vanilla()

    # -- structural facts the id rules rest on, checked on the bytecode itself
    probs = registries.verify_id_assignment(cp)
    out.structure_checks.append((
        "MappedRegistry.register assigns id = byId.size() (registration order is the protocol id)",
        not probs, "; ".join(probs)))
    out.structure_checks.append(_check_state_definition(cp))
    out.structure_checks.append(_check_block_state_ids(cp))
    out.structure_checks.append(_check_packet_ids(cp))
    out.structure_checks.append(_check_serializer_ids(cp))
    out.structure_checks.append(_check_block_item_lookup(cp))

    # -- sinks must be installed before anything is evaluated
    run = registries.RegistryRun(ev)
    serializer_values = []

    def on_serializer(ev_, recv, args):
        serializer_values.append(args[0])
        return VOID

    ev.sinks[EDS_SINK] = (frozenset([EDS]), on_serializer)

    if log:
        log("walking Bootstrap.bootStrap() ...")
    run.run()

    # -- registries
    for rid, regs in run.by_registry.items():
        names = [r.ident for r in regs]
        if rid == ROOT:
            out.root_order = names
        else:
            out.registries[rid] = names
        out.registry_units[rid] = run.units(rid)

    # -- entity data serializers
    _serializers(ev, out, serializer_values)
    # -- packets
    if log:
        log("packets ...")
    _packets(ev, out)
    # -- feature flags, synchronized registries
    _feature_flags(ev, out)
    _synced(ev, out)
    # -- blocks / items
    if log:
        log("block and item details ...")
    _blocks_and_items(ev, run, out)

    out.failures = [(u, m, t) for (u, m, t) in ev.failures]
    out.warnings = list(ev.warnings)
    out.env_queries = sorted(set(ev.env_queries))
    out.soft_failures = dict(ev.soft_failures)
    out.insn_count = ev.insn_count
    return out


# --------------------------------------------------------------------------
# structural checks
# --------------------------------------------------------------------------
def _calls(c, m):
    out = []
    for ins in m.insns():
        if ins.op in ("invokestatic", "invokevirtual", "invokeinterface", "invokespecial"):
            _, owner, name, desc = c.member_ref(ins.arg)
            out.append((owner, name))
    return out


def _check_state_definition(cp):
    desc = "StateDefinition sorts properties by name (ImmutableSortedMap) before enumerating states"
    c = cp.get(STATE_DEFINITION)
    if c is None:
        return (desc, False, "class not found")
    for m in c.methods_named("<init>"):
        calls = _calls(c, m)
        if ("com/google/common/collect/ImmutableSortedMap", "copyOf") in calls:
            return (desc, True, "")
    return (desc, False, "no ImmutableSortedMap.copyOf call in StateDefinition.<init>")


def _check_block_state_ids(cp):
    desc = ("Blocks.<clinit> fills Block.BLOCK_STATE_REGISTRY by iterating BuiltInRegistries.BLOCK "
            "and each block's getPossibleStates() in order")
    c = cp.get("net/minecraft/world/level/block/Blocks")
    if c is None:
        return (desc, False, "class not found")
    for m in c.methods:
        if not m.code:
            continue
        seq = []
        for ins in m.insns():
            if ins.op == "getstatic":
                _, owner, name, _d = c.member_ref(ins.arg)
                seq.append(name)
            elif ins.op in ("invokevirtual", "invokeinterface"):
                _, owner, name, _d = c.member_ref(ins.arg)
                seq.append(name)
        text = " ".join(seq)
        if "BLOCK iterator" in text and "getStateDefinition getPossibleStates iterator" in text \
                and "BLOCK_STATE_REGISTRY" in text and " add" in text:
            return (desc, True, "in %s" % m.name)
    return (desc, False, "expected loop not found")


def _check_packet_ids(cp):
    desc = "ProtocolInfo.Details.listPackets reports packet id = index in the builder's packet list"
    c = cp.get("net/minecraft/network/protocol/ProtocolInfoBuilder$1")
    if c is None:
        return (desc, False, "class ProtocolInfoBuilder$1 not found")
    for m in c.methods_named("listPackets"):
        ops = [ins.op for ins in m.insns()]
        calls = _calls(c, m)
        names = [n for _o, n in calls]
        if names == ["size", "get", "accept"] and "iinc" in ops:
            return (desc, True, "")
    return (desc, False, "listPackets does not have the expected index loop")


def _check_serializer_ids(cp):
    desc = ("EntityDataSerializers.registerSerializer adds to a CrudeIncrementalIntIdentityHashBiMap, "
            "whose add() hands out nextId() (sequential ids)")
    c = cp.get(EDS)
    bimap = "net/minecraft/util/CrudeIncrementalIntIdentityHashBiMap"
    b = cp.get(bimap)
    if c is None or b is None:
        return (desc, False, "class not found")
    reg = [m for m in c.methods_named("registerSerializer")]
    add = [m for m in b.methods_named("add")]
    if len(reg) != 1 or len(add) != 1:
        return (desc, False, "unexpected overloads")
    if _calls(c, reg[0]) != [(bimap, "add")]:
        return (desc, False, "registerSerializer is not a plain SERIALIZERS.add(serializer)")
    if _calls(b, add[0]) != [(bimap, "nextId"), (bimap, "addMapping")]:
        return (desc, False, "CrudeIncrementalIntIdentityHashBiMap.add is not nextId() + addMapping()")
    return (desc, True, "")


def _check_block_item_lookup(cp):
    desc = "Block.asItem() is Item.byBlock(block) = Item.BY_BLOCK.getOrDefault(block, Items.AIR)"
    blk = cp.get("net/minecraft/world/level/block/Block")
    itm = cp.get(ITEM)
    if blk is None or itm is None:
        return (desc, False, "class not found")
    a = blk.methods_named("asItem")
    b = itm.methods_named("byBlock")
    if len(a) != 1 or len(b) != 1:
        return (desc, False, "unexpected overloads")
    statics = [itm.member_ref(i.arg)[2] for i in b[0].insns() if i.op == "getstatic"]
    ok = _calls(blk, a[0]) == [(ITEM, "byBlock")] and statics == ["BY_BLOCK", "AIR"] \
        and _calls(itm, b[0]) == [("java/util/Map", "getOrDefault")]
    return (desc, ok, "" if ok else "unexpected shape")


# --------------------------------------------------------------------------
# entity data serializers
# --------------------------------------------------------------------------
def _serializers(ev, out, values):
    try:
        ev.ensure_init(EDS)
        ev.force_init(EDS)
    except Abort as e:
        out.serializer_problems.append("EntityDataSerializers initialisation failed: " + e.msg)
        return
    if ev.class_state.get(EDS) != "done":
        out.serializer_problems.append("EntityDataSerializers.<clinit> did not complete (%s)"
                                       % ev.class_state.get(EDS))
        return
    c = ev.cp.get(EDS)
    by_identity = {}
    for f in c.fields:
        if f.is_static and f.desc == "Lnet/minecraft/network/syncher/EntityDataSerializer;":
            v = ev.resolve(ev.statics.get((EDS, f.name)))
            if v is None or isinstance(v, Unknown):
                out.serializer_problems.append("static %s has no known value" % f.name)
                continue
            if id(v) in by_identity:
                out.serializer_problems.append("statics %s and %s hold the same object"
                                               % (by_identity[id(v)], f.name))
            by_identity[id(v)] = f.name
    names = []
    for v in values:
        r = ev.resolve(v)
        n = by_identity.get(id(r))
        if n is None:
            out.serializer_problems.append("a registered serializer is not held by a static field")
            return
        names.append(n)
    if len(set(names)) != len(names):
        out.serializer_problems.append("a serializer is registered twice")
        return
    out.serializers = names


# --------------------------------------------------------------------------
# packets
# --------------------------------------------------------------------------
def _packets(ev, out):
    """Packet ids exactly as vanilla's own PacketReport data provider lists them."""
    c = ev.cp.get(PACKET_REPORT)
    if c is None:
        out.packet_problems.append("class %s not found" % PACKET_REPORT)
        return
    providers = []
    for m in c.methods_named("serializePackets"):
        for ins in m.insns():
            if ins.op == "getstatic":
                _, owner, name, desc = c.member_ref(ins.arg)
                providers.append((owner, name))
    if not providers:
        out.packet_problems.append("PacketReport.serializePackets lists no protocol templates")
        return
    D = "Lnet/minecraft/network/"
    result = {}
    for owner, name in providers:
        label = "%s.%s" % (owner.rsplit("/", 1)[-1], name)
        try:
            tpl = ev.static_value(owner, name)
            det = ev.force(ev.invoke_virtual_now(tpl, "details", "()" + D + "ProtocolInfo$Details;", []))
            state = ev.force(ev.invoke_virtual_now(det, "id", "()" + D + "ConnectionProtocol;", []))
            state_id = ev.force(ev.invoke_virtual_now(state, "id", "()Ljava/lang/String;", []))
            flow = ev.force(ev.invoke_virtual_now(det, "flow", "()" + D + "protocol/PacketFlow;", []))
            flow_id = ev.force(ev.invoke_virtual_now(flow, "id", "()Ljava/lang/String;", []))
            if not isinstance(state_id, str) or not isinstance(flow_id, str):
                raise Abort("state / flow name of %s is not known" % label)
            entries = []

            def visit(ev_, mname, mdesc, args, entries=entries, flow=flow):
                ptype = ev_.force(args[0])
                index = ev_.force(args[1])
                pid = ev_.force(ev_.invoke_virtual_now(ptype, "id", "()Lnet/minecraft/resources/Identifier;", []))
                pflow = ev_.force(ev_.invoke_virtual_now(ptype, "flow", "()" + D + "protocol/PacketFlow;", []))
                if not isinstance(pid, Ident) or not isinstance(index, int):
                    raise Abort("packet type / id not known")
                entries.append((index, str(pid), pflow is flow))
                return VOID

            ev.invoke_virtual_now(det, "listPackets",
                                  "(" + D + "ProtocolInfo$Details$PacketVisitor;)V", [PyFn(visit)])
        except Abort as e:
            out.packet_problems.append("%s: %s" % (label, e.msg))
            continue
        ids = [i for i, _n, _ok in entries]
        if ids != list(range(len(entries))) or not entries:
            out.packet_problems.append("%s: packet ids are not 0..n-1" % label)
            continue
        if not all(ok for _i, _n, ok in entries):
            out.packet_problems.append("%s: a packet type has the wrong direction" % label)
            continue
        names = [n for _i, n, _ok in entries]
        if len(set(names)) != len(names):
            out.packet_problems.append("%s: duplicate packet name" % label)
            continue
        if flow_id in result.setdefault(state_id, {}):
            out.packet_problems.append("%s: second list for %s/%s" % (label, state_id, flow_id))
            continue
        result[state_id][flow_id] = names
    out.packets = result


# --------------------------------------------------------------------------
# feature flags / synchronized registries
# --------------------------------------------------------------------------
def _feature_flags(ev, out):
    """Names of the flags contained in FeatureFlags.DEFAULT_FLAGS.

    Vanilla computes this with ``FeatureFlags.REGISTRY.toNames(set)``, which walks
    the registry's name map (a ``Map.copyOf`` -- no defined iteration order) and
    keeps the names whose flag ``set.contains``.  The result is a set, so the
    same membership test is applied here to every entry and the names are sorted.
    """
    try:
        reg = ev.static_value(FEATURE_FLAGS, "REGISTRY")
        dflt = ev.static_value(FEATURE_FLAGS, "DEFAULT_FLAGS")
        if not isinstance(reg, Obj) or not isinstance(dflt, Obj):
            raise Abort("FeatureFlags.REGISTRY / DEFAULT_FLAGS not known")
        names = ev.force(reg.fields.get("names"))
        if not isinstance(names, intrinsics.JMap) or names.tainted:
            raise Abort("FeatureFlagRegistry.names is not a known map")
        entries = intrinsics.map_entries(ev, names, for_iteration=False)
        if names.imprecise or not entries:
            raise Abort("FeatureFlagRegistry.names has unknown keys")
        enabled = []
        everything = []
        for k, flag in entries:
            ident = ev.force(k)
            if not isinstance(ident, Ident):
                raise Abort("feature flag name not known")
            r = ev.force(ev.invoke_virtual_now(
                dflt, "contains", "(Lnet/minecraft/world/flag/FeatureFlag;)Z", [flag]))
            if isinstance(r, bool) or not isinstance(r, int):
                raise Abort("FeatureFlagSet.contains did not evaluate for %s" % ident)
            everything.append(str(ident))
            if r:
                enabled.append(str(ident))
        out.default_feature_flags = sorted(enabled)
        out.all_feature_flags = sorted(everything)
    except Abort as e:
        out.feature_flag_notes.append("could not evaluate FeatureFlags: " + e.msg)
    # where the default set is used (documentation of what "default" means)
    users = []
    for cname in ("net/minecraft/world/level/WorldDataConfiguration",):
        c = ev.cp.get(cname)
        if c is None:
            continue
        for m in c.methods:
            if not m.code:
                continue
            for ins in m.insns():
                if ins.op == "getstatic":
                    _, owner, name, _d = c.member_ref(ins.arg)
                    if owner == FEATURE_FLAGS and name == "DEFAULT_FLAGS":
                        users.append("%s.%s" % (cname.rsplit("/", 1)[-1], m.name))
    if users:
        out.feature_flag_notes.append("FeatureFlags.DEFAULT_FLAGS is the enabled set of " + ", ".join(sorted(set(users))))


def _synced(ev, out):
    try:
        lst = ev.static_value(REGISTRY_DATA_LOADER, "SYNCHRONIZED_REGISTRIES")
        if not isinstance(lst, intrinsics.JList) or lst.tainted:
            raise Abort("RegistryDataLoader.SYNCHRONIZED_REGISTRIES is not a known list")
        keys = []
        for x in lst.items:
            o = ev.force(x)
            k = ev.force(ev.invoke_virtual_now(o, "key", "()Lnet/minecraft/resources/ResourceKey;", []))
            if not isinstance(k, RKey):
                raise Abort("registry key of a synchronized registry is not known")
            keys.append(str(k.ident))
        if len(set(keys)) != len(keys):
            raise Abort("duplicate synchronized registry")
        out.synced_registries = keys
    except Abort as e:
        out.warnings.append(("synced", "could not evaluate SYNCHRONIZED_REGISTRIES: " + e.msg))


# --------------------------------------------------------------------------
# blocks and items
# --------------------------------------------------------------------------
def _num(ev, v):
    v = ev.force(v)
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, (int, float)):
        return v
    return None


PROPERTY = "net/minecraft/world/level/block/state/properties/Property"
BOOLEAN_PROPERTY = "net/minecraft/world/level/block/state/properties/BooleanProperty"
INTEGER_PROPERTY = "net/minecraft/world/level/block/state/properties/IntegerProperty"
ENUM_PROPERTY = "net/minecraft/world/level/block/state/properties/EnumProperty"


def _property_value(ev, prop, text):
    """The Java value a block state holds for ``prop`` when its serialised form is ``text``."""
    cp = ev.cp
    if cp.is_subclass(prop.cls, BOOLEAN_PROPERTY):
        if text not in ("true", "false"):
            raise Abort("bad boolean property value %r" % (text,))
        return JBool(1 if text == "true" else 0)
    if cp.is_subclass(prop.cls, INTEGER_PROPERTY):
        return int(text)
    if cp.is_subclass(prop.cls, ENUM_PROPERTY):
        names = ev.force(prop.fields.get("names"))
        if not isinstance(names, intrinsics.JMap) or names.tainted:
            raise Abort("EnumProperty.names not known")
        v = ev.force(intrinsics.map_get(ev, names, text))
        if not isinstance(v, Obj):
            raise Abort("enum property has no value named %r" % (text,))
        return v
    raise Abort("unknown property class %s" % prop.cls)


def _synthetic_state(props):
    """Stand-in for a BlockState that answers getValue & co. from a property dict."""
    def handler(ev, name, desc, args):
        if name in ("getValue", "getValueOrElse", "getOptionalValue", "hasProperty"):
            prop = ev.force(args[0])
            if not isinstance(prop, Obj):
                raise Abort("block state property object not known")
            pname = ev.force(ev.getfield(prop, PROPERTY, "name", "Ljava/lang/String;"))
            if not isinstance(pname, str):
                raise Abort("block state property name not known")
            if name == "hasProperty":
                return int(pname in props)
            if pname not in props:
                if name == "getValueOrElse":
                    return args[1]
                if name == "getOptionalValue":
                    return intrinsics.JOptional(False)
                raise Abort("block state has no property %s" % pname)
            v = _property_value(ev, prop, props[pname])
            return intrinsics.JOptional(True, v) if name == "getOptionalValue" else v
        return ev.unknown_result(desc, "BlockState.%s" % name, args)
    return PyFn(handler)


def _light_fn(ev, lam):
    """Evaluate a block's light-emission function for one state (properties as strings)."""
    def fn(props):
        try:
            v = ev.force(ev.invoke_lambda(lam, lam.sam, "(Ljava/lang/Object;)I", [_synthetic_state(props)]))
        except Abort:
            return None
        if isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= 15:
            return int(v)
        return None
    return fn


def _declares_between(cp, cls, base, name, desc):
    """Does ``cls`` or an ancestor below ``base`` declare the method?"""
    n = cls
    while n is not None and n != base:
        c = cp.get(n)
        if c is None:
            return None
        m = c.method(name, desc)
        if m is not None and m.code is not None:
            return True
        n = c.super_name
    return False


def _blocks_and_items(ev, run, out):
    cp = ev.cp
    block_regs = run.by_registry.get("minecraft:block", [])
    item_regs = run.by_registry.get("minecraft:item", [])
    block_by_identity = {}
    for r in block_regs:
        info = BlockInfo(r.ident)
        out.blocks[r.ident] = info
        b = ev.force(r.value)
        if not isinstance(b, Obj):
            info.notes.append("block object not known")
            continue
        if id(b) in block_by_identity:
            out.cross_ref_problems.append("blocks %s and %s are the same object"
                                          % (block_by_identity[id(b)], r.ident))
        block_by_identity[id(b)] = r.ident
        info.cls = b.cls
        info.has_block_entity = bool(cp.is_subclass(b.cls, ENTITY_BLOCK))
        info.overrides_fluid_state = _declares_between(
            cp, b.cls, BLOCK_BEHAVIOUR, "getFluidState",
            "(Lnet/minecraft/world/level/block/state/BlockState;)Lnet/minecraft/world/level/material/FluidState;")
        info.overrides_collision = _declares_between(
            cp, b.cls, BLOCK_BEHAVIOUR, "getCollisionShape",
            "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;"
            "Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/phys/shapes/CollisionContext;)"
            "Lnet/minecraft/world/phys/shapes/VoxelShape;")
        props = ev.force(b.fields.get("properties"))
        if not isinstance(props, Obj):
            info.notes.append("block properties not known")
            continue
        key = ev.force(props.fields.get("id"))
        if not isinstance(key, RKey) or str(key.ident) != r.ident:
            info.notes.append("properties object carries id %r" % (key,))
            continue
        dt = _num(ev, props.fields.get("destroyTime", 0.0))
        er = _num(ev, b.fields.get("explosionResistance"))
        er2 = _num(ev, props.fields.get("explosionResistance", 0.0))
        if dt is not None:
            info.hardness = _f(dt)
        if er is not None and er2 is not None and er == er2:
            info.blast_resistance = _f(er)
        elif er is not None or er2 is not None:
            info.notes.append("explosion resistance differs between block (%r) and properties (%r)" % (er, er2))
        for attr, fld, default in (("is_air", "isAir", 0), ("replaceable", "replaceable", 0),
                                   ("has_collision", "hasCollision", None)):
            v = _num(ev, props.fields.get(fld, default))
            if v is not None:
                setattr(info, attr, bool(v))
        le = ev.force(props.fields.get("lightEmission"))
        if isinstance(le, Lambda):
            try:
                state = Unknown("Lnet/minecraft/world/level/block/state/BlockState;", "any block state")
                lv = ev.force(ev.invoke_lambda(le, le.sam, "(Ljava/lang/Object;)I", [state]))
                if isinstance(lv, int) and not isinstance(lv, bool):
                    info.light_const = int(lv)
            except Abort:
                pass
            if info.light_const is None:
                info.light_fn = _light_fn(ev, le)

    # ---- items
    item_by_identity = {}
    for r in item_regs:
        info = ItemInfo(r.ident)
        out.items[r.ident] = info
        it = ev.force(r.value)
        if not isinstance(it, Obj):
            continue
        item_by_identity[id(it)] = r.ident
        info.cls = it.cls
        info.is_block_item = bool(cp.is_subclass(it.cls, BLOCK_ITEM))
        if info.is_block_item:
            blk = ev.force(it.fields.get("block"))
            name = block_by_identity.get(id(blk)) if isinstance(blk, Obj) else None
            if name is None:
                out.cross_ref_problems.append("block of BlockItem %s is not a registered block" % r.ident)
            info.block = name

    # ---- Item.BY_BLOCK  (what Block.asItem() answers)
    try:
        by_block = ev.static_value(ITEM, "BY_BLOCK")
    except Abort as e:
        by_block = None
        out.cross_ref_problems.append("Item.BY_BLOCK: " + e.msg)
    if not isinstance(by_block, intrinsics.JMap) or by_block.tainted:
        out.cross_ref_problems.append("Item.BY_BLOCK is not a known map")
        return
    try:
        entries = intrinsics.map_entries(ev, by_block, for_iteration=False)
    except Abort as e:
        out.cross_ref_problems.append("Item.BY_BLOCK: " + e.msg)
        return
    if by_block.imprecise:
        out.cross_ref_problems.append("Item.BY_BLOCK has entries with unknown keys")
        return
    for k, v in entries:
        kb = ev.force(k)
        vi = ev.force(v)
        bname = block_by_identity.get(id(kb)) if isinstance(kb, Obj) else None
        iname = item_by_identity.get(id(vi)) if isinstance(vi, Obj) else None
        if bname is None or iname is None:
            out.cross_ref_problems.append("Item.BY_BLOCK entry %r -> %r does not resolve" % (kb, vi))
            continue
        out.block_to_item[bname] = iname
    # every BlockItem must have put its own block into the map
    for name, info in out.items.items():
        if info.is_block_item and info.block is not None and info.block not in out.block_to_item:
            out.cross_ref_problems.append("BlockItem %s: its block %s is missing from Item.BY_BLOCK"
                                          % (name, info.block))


def differences(a, b):
    """Compare two analyses (e.g. eager vs lazy class initialisation). -> list of differences."""
    out = []
    if a.registries != b.registries:
        for rid in sorted(set(a.registries) | set(b.registries)):
            if a.registries.get(rid) != b.registries.get(rid):
                out.append("registry %s differs" % rid)
    if a.root_order != b.root_order:
        out.append("order of registries differs")
    if a.packets != b.packets:
        out.append("packets differ")
    if a.serializers != b.serializers:
        out.append("entity data serializers differ")
    if a.default_feature_flags != b.default_feature_flags or a.all_feature_flags != b.all_feature_flags:
        out.append("feature flags differ")
    if a.synced_registries != b.synced_registries:
        out.append("synchronized registries differ")
    if a.block_to_item != b.block_to_item:
        out.append("Item.BY_BLOCK differs")
    for name in sorted(set(a.blocks) | set(b.blocks)):
        x, y = a.blocks.get(name), b.blocks.get(name)
        if x is None or y is None:
            out.append("block %s only in one analysis" % name)
            continue
        for attr in ("cls", "hardness", "blast_resistance", "is_air", "replaceable", "has_collision",
                     "has_block_entity", "light_const", "overrides_fluid_state", "overrides_collision"):
            if getattr(x, attr) != getattr(y, attr):
                out.append("block %s: %s differs (%r / %r)" % (name, attr, getattr(x, attr), getattr(y, attr)))
    for name in sorted(set(a.items) | set(b.items)):
        x, y = a.items.get(name), b.items.get(name)
        if x is None or y is None or (x.cls, x.block, x.is_block_item) != (y.cls, y.block, y.is_block_item):
            out.append("item %s differs" % name)
    return out
