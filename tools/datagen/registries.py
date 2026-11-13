"""Built-in registry extraction: registration order == protocol id.

Vanilla assigns a built-in registry entry the id ``byId.size()`` at the moment
``MappedRegistry.register`` runs (verified against the bytecode by
``verify_id_assignment``), so the numeric ids are exactly the order in which
``WritableRegistry.register(key, value, RegistrationInfo.BUILT_IN)`` is reached
while ``net.minecraft.server.Bootstrap.bootStrap()`` runs.  This module walks
that entry point with the symbolic evaluator and records every such call.
"""

import classfile as cf
from symexec import Abort, Unknown, Obj, RKey, Ident, Lambda, HARD_CAP
import intrinsics

SINK_DESC = ("(Lnet/minecraft/resources/ResourceKey;Ljava/lang/Object;"
             "Lnet/minecraft/core/RegistrationInfo;)Lnet/minecraft/core/Holder$Reference;")
SINK_OWNERS = frozenset([
    "net/minecraft/core/WritableRegistry", "net/minecraft/core/MappedRegistry",
    "net/minecraft/core/DefaultedMappedRegistry",
])
BOOTSTRAP_IFACE = "net/minecraft/core/registries/BuiltInRegistries$RegistryBootstrap"
MAPPED_REGISTRY = "net/minecraft/core/MappedRegistry"
KEY_DESC = "()Lnet/minecraft/resources/ResourceKey;"


class Registration:
    __slots__ = ("registry", "ident", "value", "unit")

    def __init__(self, registry, ident, value, unit):
        self.registry = registry    # str, e.g. "minecraft:block"
        self.ident = ident          # str, e.g. "minecraft:stone"
        self.value = value          # symbolic value that was registered
        self.unit = unit


class RegistryRun:
    def __init__(self, ev):
        self.ev = ev
        self.log = []
        self.by_registry = {}       # registry id -> [Registration]
        self.registry_objects = {}  # registry id -> Obj
        ev.sinks[("register", SINK_DESC)] = (SINK_OWNERS, self.on_register)
        ev.always_run[(BOOTSTRAP_IFACE, "run")] = self.on_bootstrap

    # -- sink ---------------------------------------------------------------
    def registry_key(self, reg):
        ev = self.ev
        if not isinstance(reg, Obj):
            ev.abort("registration on unknown registry object %r" % (reg,))
        if reg.tag is None:
            k = ev.force(ev.invoke_virtual_now(reg, "key", KEY_DESC, []))
            if not isinstance(k, RKey):
                ev.abort("cannot determine key of registry object %r: %r" % (reg, k))
            reg.tag = k
            # The registration itself is intercepted here, so the registry's own
            # lookup tables are never filled in: make them explicitly unknown.
            for fname, fval in list(reg.fields.items()):
                if not isinstance(ev.resolve(fval), (RKey, Ident, str, int)):
                    reg.fields[fname] = Unknown("Ljava/lang/Object;", "registry internals are not modelled")
            reg.partial = True
        return reg.tag

    def on_register(self, ev, recv, args):
        key, value, info = args
        reg = ev.force(recv)
        regkey = self.registry_key(reg)
        k = ev.force(key)
        if not isinstance(k, RKey):
            ev.abort("registry key is not known for a registration in %s: %r" % (regkey.ident, k))
        if k.registry != regkey.ident:
            ev.abort("key %r registered in registry %s" % (k, regkey.ident))
        rid = str(regkey.ident)
        r = Registration(rid, str(k.ident), value, ev.unit_stack[-1])
        self.log.append(r)
        self.by_registry.setdefault(rid, []).append(r)
        if regkey.registry == regkey.ident:
            # registration of a registry in the root registry
            v = ev.resolve(value)
            if isinstance(v, Obj):
                self.registry_objects[str(k.ident)] = v
        return intrinsics.JHolder(regkey, k, value)

    # -- registry bootstrap lambdas -----------------------------------------
    def on_bootstrap(self, ev, recv, name, desc, args):
        f = ev.force(recv)
        label = "bootstrap:?"
        try:
            reg = ev.force(args[0])
            label = "bootstrap:%s" % (self.registry_key(reg).ident,)
        except Abort:
            pass
        ev.unit_stack.append(label)
        saved = ev.limit
        ev.limit = HARD_CAP
        try:
            if isinstance(f, Lambda) and f.kind == cf.REF_invokeStatic and ev.cp.has(f.owner):
                m = ev.cp.find_method(f.owner, f.name, f.desc)
                if m is None or m.code is None:
                    ev.abort("bootstrap implementation not found: %r" % (f,))
                ev.ensure_init(m.cls.name)
                return ev.exec(m, list(f.captured) + list(args), True)
            return ev.invoke_virtual_now(f, name, desc, args)
        except Abort as e:
            ev.record_failure(e)
            return Unknown("Ljava/lang/Object;", "registry bootstrap failed")
        finally:
            ev.limit = saved
            ev.unit_stack.pop()

    # -- driver ---------------------------------------------------------------
    def run(self):
        ev = self.ev
        c = ev.cp.get("net/minecraft/server/Bootstrap")
        m = c.method("bootStrap", "()V")
        ev.unit_stack.append("Bootstrap.bootStrap")
        try:
            ev.exec(m, [], True)
        except Abort as e:
            ev.record_failure(e)
        finally:
            ev.unit_stack.pop()

    def ordered(self, registry):
        return [r.ident for r in self.by_registry.get(registry, [])]

    def units(self, registry):
        seen = []
        for r in self.by_registry.get(registry, []):
            if r.unit not in seen:
                seen.append(r.unit)
        return seen


def verify_id_assignment(cp):
    """Check on the bytecode that MappedRegistry.register assigns id = byId.size().

    Returns a list of problems (empty when the expected shape is found):
        int id = this.byId.size();  this.byId.add(holder);  this.toId.put(value, id);
    """
    problems = []
    c = cp.get(MAPPED_REGISTRY)
    if c is None:
        return ["class %s not found" % MAPPED_REGISTRY]
    m = c.method("register", SINK_DESC)
    if m is None:
        return ["MappedRegistry.register%s not found" % SINK_DESC]
    seq = []
    for ins in m.insns():
        if ins.op == "getfield":
            seq.append(("getfield", c.member_ref(ins.arg)[2]))
        elif ins.op in ("invokeinterface", "invokevirtual"):
            _, owner, name, desc = c.member_ref(ins.arg)
            seq.append(("call", name))
        elif ins.op == "istore":
            seq.append(("istore", ins.arg))
        elif ins.op == "iload":
            seq.append(("iload", ins.arg))
    text = " ".join("%s:%s" % p for p in seq)
    import re
    mo = re.search(r"getfield:byId call:size istore:(\d+) getfield:byId call:add "
                   r"getfield:toId iload:(\d+) call:put", text)
    if not mo or mo.group(1) != mo.group(2):
        problems.append("MappedRegistry.register does not have the expected "
                        "'id = byId.size(); byId.add(..); toId.put(value, id)' shape")
    return problems
