"""Models of the few library (JDK / Guava / fastutil) classes the evaluator needs.

The analysed jar only contains Mojang's own classes; library code is not
available (and is never executed).  Everything here is a hand-written model of
*well-known, documented* library behaviour: strings, boxed numbers, ordered
collections, Optional, sequential streams.  Anything not modelled evaluates to
``Unknown`` -- and the evaluator refuses to continue if such a value is needed
to decide something that matters (see symexec.py).

Two classes of the jar itself are modelled natively as well because they are
the vocabulary every registration is expressed in: ``Identifier`` and
``ResourceKey``.  The constants they rely on (default namespace, separator,
root registry name) are still read from the bytecode, see ``IdentModel``.
"""

import math
import sys

import classfile as cf
from symexec import (Unknown, JLong, JDouble, JBool, JChar, BoxedLong, BoxedDouble, VOID,
                     Obj, LibObj, Arr, ClassRef, Lambda, Thunk, Ident, RKey, f32, f32_repr,
                     java_string_hash, i32)

OBJ = "Ljava/lang/Object;"
STR = "Ljava/lang/String;"


# --------------------------------------------------------------------------
# native collection models
# --------------------------------------------------------------------------
class JList:
    __slots__ = ("items", "hot", "origins", "tainted")

    def __init__(self, items=None):
        self.items = items if items is not None else []
        self.hot = False
        self.origins = None
        self.tainted = False

    def __repr__(self):
        return "JList(%d)" % len(self.items)


class JSet:
    __slots__ = ("items", "ordered", "hot", "origins", "tainted")

    def __init__(self, items=None, ordered=True):
        self.items = items if items is not None else []
        self.ordered = ordered
        self.hot = False
        self.origins = None
        self.tainted = False

    def __repr__(self):
        return "JSet(%d)" % len(self.items)


class JMap:
    """Map model.  ``order``: 'insertion', 'none' (hash order, must not be iterated),
    'enum' (by ordinal) or 'sorted' (natural key order)."""
    __slots__ = ("raw", "order", "hot", "origins", "_norm", "_done", "imprecise", "tainted", "identity")

    def __init__(self, order="insertion", identity=False):
        self.raw = []           # [key, value] in insertion order (keys possibly lazy)
        self.order = order
        self.identity = identity    # keys compared by reference (IdentityHashMap)
        self.hot = False
        self.origins = None
        self.tainted = False
        self._norm = {}
        self._done = 0
        self.imprecise = False

    def __repr__(self):
        return "JMap(%d,%s)" % (len(self.raw), self.order)


class JEntry:
    __slots__ = ("key", "value")

    def __init__(self, key, value):
        self.key = key
        self.value = value


class JIter:
    __slots__ = ("items", "pos")

    def __init__(self, items):
        self.items = items
        self.pos = 0


class JOptional:
    __slots__ = ("present", "value", "hot", "origins")

    def __init__(self, present, value=None):
        self.present = present
        self.value = value
        self.hot = False
        self.origins = None


class JSB:
    __slots__ = ("parts", "unknown", "tainted")

    def __init__(self):
        self.parts = []
        self.unknown = False
        self.tainted = False


class JStream:
    __slots__ = ("items", "ops")

    def __init__(self, items, ops=()):
        self.items = items          # list or range
        self.ops = list(ops)

    def with_op(self, op):
        return JStream(self.items, self.ops + [op])


class Collector:
    __slots__ = ("kind", "args")

    def __init__(self, kind, args=()):
        self.kind = kind
        self.args = args


class JHolder:
    """Holder.Reference returned by a built-in registration."""
    __slots__ = ("registry", "key", "value")

    def __init__(self, registry, key, value):
        self.registry = registry
        self.key = key
        self.value = value

    def __repr__(self):
        return "Holder<%s>" % (self.key,)


_LIST_CLASSES = frozenset([
    "java/util/ArrayList", "java/util/concurrent/CopyOnWriteArrayList",
    "it/unimi/dsi/fastutil/objects/ObjectArrayList",
])
_IDENTITY_MAPS = frozenset(["java/util/IdentityHashMap"])
_MAP_CLASSES = {
    "java/util/LinkedHashMap": "insertion",
    "it/unimi/dsi/fastutil/objects/Object2ObjectLinkedOpenHashMap": "insertion",
    "it/unimi/dsi/fastutil/objects/Reference2ObjectLinkedOpenHashMap": "insertion",
    "it/unimi/dsi/fastutil/objects/Object2ObjectArrayMap": "insertion",
    "it/unimi/dsi/fastutil/objects/Reference2ObjectArrayMap": "insertion",
    "java/util/HashMap": "none", "java/util/IdentityHashMap": "none",
    "java/util/concurrent/ConcurrentHashMap": "none", "java/util/WeakHashMap": "none",
    "java/util/Hashtable": "none",
    "it/unimi/dsi/fastutil/objects/Object2ObjectOpenHashMap": "none",
    "it/unimi/dsi/fastutil/objects/Reference2ObjectOpenHashMap": "none",
    "it/unimi/dsi/fastutil/objects/Object2ReferenceOpenHashMap": "none",
    "it/unimi/dsi/fastutil/objects/Reference2ReferenceOpenHashMap": "none",
    "java/util/EnumMap": "enum", "java/util/TreeMap": "sorted",
}
_SET_CLASSES = {
    "java/util/LinkedHashSet": True, "it/unimi/dsi/fastutil/objects/ObjectLinkedOpenHashSet": True,
    "it/unimi/dsi/fastutil/objects/ObjectArraySet": True,
    "it/unimi/dsi/fastutil/objects/ReferenceLinkedOpenHashSet": True,
    "java/util/HashSet": False, "it/unimi/dsi/fastutil/objects/ObjectOpenHashSet": False,
    "it/unimi/dsi/fastutil/objects/ReferenceOpenHashSet": False,
}
_SB_CLASSES = frozenset(["java/lang/StringBuilder", "java/lang/StringBuffer"])


def install(ev):
    ev.intrinsics = sys.modules[__name__]
    model = IdentModel(ev)
    ev.ident_model = model
    ev.native_jar_classes["net/minecraft/resources/Identifier"] = model.identifier
    ev.native_jar_classes["net/minecraft/resources/ResourceKey"] = model.resource_key


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------
def _forced(ev, args):
    return [ev.force(a) for a in args]


def _has_unknown(vals):
    for v in vals:
        if isinstance(v, Unknown):
            return True
    return False


def ret_desc(desc):
    return cf.method_desc(desc)[1]


def unknown_ret(ev, desc, why, args=()):
    return ev.unknown_result(desc, why, args)


def call_fn(ev, f, args, candidates=()):
    """Invoke a functional object (lambda or jar object implementing the interface)."""
    f = ev.force(f)
    if isinstance(f, Lambda):
        return ev.invoke_lambda(f, f.sam, "(" + OBJ * len(args) + ")" + OBJ, list(args))
    if isinstance(f, Obj):
        for name, desc in candidates:
            m = ev.resolve_virtual(f.cls, name, desc)
            if m is not None:
                return ev.call_jar(m, [f] + list(args))
        ev.abort("cannot call functional object %r" % (f,))
    if f is None:
        ev.abort("null functional object")
    for a in args:
        if ev.is_hot(ev.resolve(a)):
            ev.abort("registering value passed to unknown function")
    return Unknown(OBJ, "call of unknown function")


_FN1 = (("apply", "(Ljava/lang/Object;)Ljava/lang/Object;"), ("accept", "(Ljava/lang/Object;)V"),
        ("test", "(Ljava/lang/Object;)Z"), ("apply", "(I)Ljava/lang/Object;"), ("accept", "(I)V"),
        ("applyAsInt", "(Ljava/lang/Object;)I"))
_FN2 = (("apply", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;"),
        ("accept", "(Ljava/lang/Object;Ljava/lang/Object;)V"))
_FN0 = (("get", "()Ljava/lang/Object;"), ("run", "()V"))


def key_id(ev, k, identity=False):
    """Hashable identity of a map key under Java equals()/hashCode() semantics, or None."""
    k = ev.force(k)
    if identity and isinstance(k, Obj):
        return ("o", id(k))
    if isinstance(k, Unknown):
        return None
    if k is None:
        return ("null",)
    if isinstance(k, (str, Ident, RKey, ClassRef)):
        return ("v", k)
    if isinstance(k, float):
        return ("f", float(k))
    if isinstance(k, int):
        return ("i", int(k))
    if isinstance(k, Obj):
        c = ev.cp.get(k.cls)
        if c is not None and c.super_name == "java/lang/Record" and c.record_components is not None:
            parts = []
            for name, _d in c.record_components:
                sub = key_id(ev, k.fields.get(name))
                if sub is None:
                    return None
                parts.append(sub)
            return ("r", k.cls, tuple(parts))
        return ("o", id(k))
    return ("o", id(k))


def map_sync(ev, m):
    raw = m.raw
    while m._done < len(raw):
        ent = raw[m._done]
        m._done += 1
        kid = key_id(ev, ent[0], m.identity)
        if kid is None:
            m.imprecise = True
            continue
        old = m._norm.get(kid)
        if old is not None:
            old[1] = ent[1]
            ent[0] = _DEAD
        else:
            m._norm[kid] = ent


_DEAD = object()


def map_entries(ev, m, for_iteration=True):
    map_sync(ev, m)
    ents = [e for e in m.raw if e[0] is not _DEAD]
    if not for_iteration:
        return ents
    if m.imprecise:
        ev.abort("iteration over map with unknown keys")
    if m.order == "none" and len(ents) > 1:
        ev.abort("iteration order of a hash map is not defined")
    if m.order == "enum":
        def ordinal(e):
            k = ev.force(e[0])
            if not isinstance(k, Obj) or k.enum_ordinal is None:
                ev.abort("EnumMap key is not an enum constant")
            return k.enum_ordinal
        ents = sorted(ents, key=ordinal)
    elif m.order == "sorted":
        keys = [ev.force(e[0]) for e in ents]
        if not all(isinstance(k, str) for k in keys) and not all(isinstance(k, int) for k in keys):
            ev.abort("sorted map with non-trivial keys")
        ents = [e for _, e in sorted(zip(keys, ents), key=lambda p: p[0])]
    return ents


def map_get(ev, m, k):
    map_sync(ev, m)
    kid = key_id(ev, k, m.identity)
    if kid is None or m.imprecise:
        return Unknown(OBJ, "map lookup with unknown key")
    e = m._norm.get(kid)
    return e[1] if e is not None else None


def map_has(ev, m, k):
    map_sync(ev, m)
    kid = key_id(ev, k, m.identity)
    if kid is None or m.imprecise:
        return None
    return kid in m._norm


def map_put(ev, m, k, v):
    old = None
    if m.raw:
        map_sync(ev, m)
        kid = key_id(ev, k, m.identity) if not isinstance(ev.resolve(k), Thunk) else None
        if kid is not None and not m.imprecise:
            e = m._norm.get(kid)
            old = e[1] if e is not None else None
        elif isinstance(ev.resolve(k), Thunk):
            old = Unknown(OBJ, "previous value for lazy key")
    m.raw.append([k, v])
    if ev.is_hot(ev.resolve(v)) or ev.is_hot(ev.resolve(k)):
        ev.mark_hot_container(m)
    return old


def iter_items(ev, coll):
    """Elements of a forced collection-like value in iteration order (list) or None."""
    coll = ev.force(coll)
    if getattr(coll, "tainted", False):
        return None
    if isinstance(coll, JList):
        return list(coll.items)
    if isinstance(coll, JSet):
        if not coll.ordered and len(coll.items) > 1:
            ev.abort("iteration order of a hash set is not defined")
        return list(coll.items)
    if isinstance(coll, Arr):
        return list(coll.items)
    if isinstance(coll, JStream):
        return list(stream_iter(ev, coll))
    if isinstance(coll, JIter):
        r = coll.items[coll.pos:]
        coll.pos = len(coll.items)
        return r
    if isinstance(coll, JMap):
        return None
    if isinstance(coll, JOptional):
        return [coll.value] if coll.present else []
    return None


def add_all(ev, target_items, src, container=None):
    items = iter_items(ev, src)
    if items is None:
        return False
    target_items.extend(items)
    if container is not None:
        for it in items:
            if ev.is_hot(ev.resolve(it)):
                ev.mark_hot_container(container)
                break
    return True


def set_add(ev, s, v):
    kid = key_id(ev, v) if not isinstance(ev.resolve(v), Thunk) else ("lazy", id(v))
    for it in s.items:
        r = ev.resolve(it)
        if isinstance(r, Thunk):
            if r is v:
                return 0
            continue
        if key_id(ev, it) == kid:
            return 0
    s.items.append(v)
    if ev.is_hot(ev.resolve(v)):
        ev.mark_hot_container(s)
    return 1


def stream_iter(ev, s):
    ops = s.ops

    def run(value, k):
        """Push ``value`` through ops[k:], yielding results."""
        while k < len(ops):
            op = ops[k]
            kind = op[0]
            if kind == "map":
                value = call_fn(ev, op[1], [value], _FN1)
            elif kind == "filter":
                r = ev.force(call_fn(ev, op[1], [value], _FN1))
                if isinstance(r, Unknown):
                    ev.abort("stream filter with unknown predicate result")
                if not r:
                    return
            elif kind == "peek":
                call_fn(ev, op[1], [value], _FN1)
            elif kind == "flatMap":
                sub = ev.force(call_fn(ev, op[1], [value], _FN1))
                items = iter_items(ev, sub)
                if items is None:
                    ev.abort("flatMap over unknown stream")
                for it in items:
                    for out in run(it, k + 1):
                        yield out
                return
            k += 1
        yield value

    for item in s.items:
        for out in run(item, 0):
            yield out


def java_equals(ev, a, b):
    a = ev.force(a)
    b = ev.force(b)
    if isinstance(a, Unknown) or isinstance(b, Unknown):
        return None
    if a is None or b is None:
        return a is b
    if a is b:
        return True
    if isinstance(a, (str, Ident, RKey, ClassRef)):
        return type(a) is type(b) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return a == b
    if isinstance(a, Obj) and isinstance(b, Obj):
        ka = key_id(ev, a)
        kb = key_id(ev, b)
        if ka is None or kb is None:
            return None
        if ka[0] == "r" or kb[0] == "r":
            return ka == kb
        m = ev.resolve_virtual(a.cls, "equals", "(Ljava/lang/Object;)Z")
        if m is None:
            return False
        r = ev.force(ev.call_jar(m, [a, b]))
        if isinstance(r, Unknown):
            return None
        return bool(r)
    return False


def java_str(ev, v, desc=None):
    """Java string conversion (String.valueOf / string concatenation)."""
    v = ev.force(v)
    if isinstance(v, Unknown):
        return v
    if v is None:
        return "null"
    if isinstance(v, str):
        return v
    if isinstance(v, JBool):
        return "true" if v else "false"
    if isinstance(v, JChar):
        return chr(v)
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (JDouble, BoxedDouble)):
        f = float(v)
        if f == int(f) and abs(f) < 1e7:
            return "%.1f" % f
        return repr(f)
    if isinstance(v, int):
        if desc == "Z":
            return "true" if v else "false"
        if desc == "C":
            return chr(v)
        return str(int(v))
    if isinstance(v, float):
        if desc == "D":
            return java_str(ev, JDouble(v))
        return f32_repr(v)
    if isinstance(v, (Ident, RKey)):
        return str(v) if isinstance(v, Ident) else repr(v)
    if isinstance(v, Obj):
        m = ev.resolve_virtual(v.cls, "toString", "()Ljava/lang/String;")
        if m is not None:
            r = ev.force(ev.call_jar(m, [v]))
            return r if isinstance(r, (str, Unknown)) else Unknown(STR, "toString() gave non-string")
        if v.enum_name is not None:
            return v.enum_name
        return Unknown(STR, "Object.toString() of %s" % v.cls)
    if isinstance(v, ClassRef):
        return "class " + v.name.replace("/", ".")
    if isinstance(v, JSB):
        return sb_str(v)
    if isinstance(v, JHolder):
        return Unknown(STR, "holder toString")
    return Unknown(STR, "toString of %r" % (type(v).__name__,))


def sb_str(sb):
    if sb.unknown:
        return Unknown(STR, "StringBuilder with unknown part")
    return "".join(sb.parts)


# --------------------------------------------------------------------------
# Identifier / ResourceKey
# --------------------------------------------------------------------------
class IdentModel:
    """Native model of Identifier and ResourceKey; constants come from the bytecode."""

    def __init__(self, ev):
        cp = ev.cp
        self.default_ns = None
        self.separator = None
        c = cp.get("net/minecraft/resources/Identifier")
        if c is not None:
            for m in c.methods_named("withDefaultNamespace"):
                for ins in m.insns():
                    if ins.op == "ldc":
                        kind, val = c.loadable(ins.arg)
                        if kind == "string":
                            self.default_ns = val
                            break
            for m in c.methods_named("parse"):
                for ins in m.insns():
                    if ins.op == "iconst":
                        self.separator = chr(ins.arg)
        self._root = None
        self.ev = ev

    def need(self):
        if self.default_ns is None or self.separator is None:
            self.ev.abort("could not read Identifier constants from bytecode")

    def root_name(self):
        if self._root is None:
            v = self.ev.force(self.ev.getstatic("net/minecraft/core/registries/Registries",
                                                "ROOT_REGISTRY_NAME",
                                                "Lnet/minecraft/resources/Identifier;"))
            if not isinstance(v, Ident):
                self.ev.abort("Registries.ROOT_REGISTRY_NAME is not an identifier: %r" % (v,))
            self._root = v
        return self._root

    def split(self, s, sep):
        i = s.find(sep)
        if i >= 0:
            path = s[i + 1:]
            if i != 0:
                return Ident(s[:i], path)
            return Ident(self.default_ns, path)
        return Ident(self.default_ns, s)

    def identifier(self, ev, kind, name, desc, args):
        self.need()
        a = _forced(ev, args)
        ret = ret_desc(desc) if desc and desc[0] == "(" else desc
        if _has_unknown(a):
            if ret == "V":
                return VOID
            return Unknown(ret, "Identifier.%s with unknown argument" % name)
        if kind == "invokestatic":
            if name == "withDefaultNamespace":
                return Ident(self.default_ns, a[0])
            if name in ("fromNamespaceAndPath", "createUntrusted", "tryBuild"):
                return Ident(a[0], a[1])
            if name in ("parse", "tryParse"):
                return self.split(a[0], self.separator)
            if name in ("bySeparator", "tryBySeparator"):
                return self.split(a[0], chr(a[1]))
            if name in ("isValidPath", "isValidNamespace", "validPathChar", "validNamespaceChar",
                        "isAllowedInIdentifier"):
                return 1
            if name in ("assertValidPath",):
                return a[1]
            if name in ("assertValidNamespace",):
                return a[0]
            return Unknown(ret, "Identifier.%s" % name)
        if kind == "getfield":
            s = a[0]
            if name == "path":
                return s.path
            if name == "namespace":
                return s.ns
            return Unknown(desc, "Identifier field " + name)
        s = a[0]
        if s is None:
            ev.abort("null Identifier receiver for " + name)
        if not isinstance(s, Ident):
            return Unknown(ret, "Identifier.%s on %r" % (name, s))
        if name == "getPath":
            return s.path
        if name == "getNamespace":
            return s.ns
        if name == "toString":
            return str(s)
        if name == "withPath":
            if isinstance(a[1], str):
                return Ident(s.ns, a[1])
            r = ev.force(call_fn(ev, a[1], [s.path], _FN1))
            return Ident(s.ns, r) if isinstance(r, str) else Unknown(ret, "withPath(unknown)")
        if name == "withPrefix":
            return Ident(s.ns, a[1] + s.path)
        if name == "withSuffix":
            return Ident(s.ns, s.path + a[1])
        if name == "equals":
            return int(isinstance(a[1], Ident) and a[1] == s)
        if name == "hashCode":
            return i32(31 * java_string_hash(s.ns) + java_string_hash(s.path))
        if name == "compareTo":
            o = a[1]
            if not isinstance(o, Ident):
                return Unknown("I", "compareTo non-identifier")
            x, y = (s.path, s.ns), (o.path, o.ns)
            return (x > y) - (x < y)
        if name == "toLanguageKey":
            base = s.ns + "." + s.path
            if len(a) == 1:
                return base
            if len(a) == 2:
                return a[1] + "." + base
            return a[1] + "." + base + "." + a[2]
        if name == "toShortString":
            return s.path if s.ns == self.default_ns else str(s)
        if name == "toDebugFileName":
            return str(s).replace("/", "_").replace(":", "_")
        return Unknown(ret, "Identifier.%s" % name)

    def resource_key(self, ev, kind, name, desc, args):
        self.need()
        a = _forced(ev, args)
        ret = ret_desc(desc) if desc and desc[0] == "(" else desc
        if _has_unknown(a):
            if ret == "V":
                return VOID
            return Unknown(ret, "ResourceKey.%s with unknown argument" % name)
        if kind == "invokestatic":
            if name == "create":
                reg, ident = a
                if isinstance(reg, RKey):
                    reg = reg.ident
                if isinstance(reg, Ident) and isinstance(ident, Ident):
                    return RKey(reg, ident)
                ev.abort("ResourceKey.create with %r, %r" % (reg, ident))
            if name == "createRegistryKey":
                if isinstance(a[0], Ident):
                    return RKey(self.root_name(), a[0])
                ev.abort("ResourceKey.createRegistryKey with %r" % (a[0],))
            return Unknown(ret, "ResourceKey.%s" % name)
        s = a[0]
        if kind == "getfield":
            if not isinstance(s, RKey):
                return Unknown(desc, "ResourceKey field of %r" % (s,))
            return s.ident if name == "identifier" else s.registry
        if s is None:
            ev.abort("null ResourceKey receiver for " + name)
        if not isinstance(s, RKey):
            return Unknown(ret, "ResourceKey.%s on %r" % (name, s))
        if name == "identifier":
            return s.ident
        if name == "registry":
            return s.registry
        if name == "dependent":
            # ResourceKey.create(otherRegistry, this.identifier.withSuffix(s) / .withPath(op))
            reg = a[1]
            if not isinstance(reg, RKey):
                return Unknown(ret, "ResourceKey.dependent with %r" % (reg,))
            if isinstance(a[2], str):
                return RKey(reg.ident, Ident(s.ident.ns, s.ident.path + a[2]))
            r = ev.force(call_fn(ev, a[2], [s.ident.path], _FN1))
            if isinstance(r, str):
                return RKey(reg.ident, Ident(s.ident.ns, r))
            return Unknown(ret, "ResourceKey.dependent(unknown operator result)")
        if name == "registryKey":
            return RKey(self.root_name(), s.registry)
        if name == "isFor":
            return int(isinstance(a[1], RKey) and s.registry == a[1].ident)
        if name == "cast":
            ok = isinstance(a[1], RKey) and s.registry == a[1].ident
            return JOptional(ok, s if ok else None)
        if name == "toString":
            return repr(s)
        if name == "equals":
            return int(isinstance(a[1], RKey) and a[1] == s)
        if name == "hashCode":
            return Unknown("I", "identity hash")
        return Unknown(ret, "ResourceKey.%s" % name)


# --------------------------------------------------------------------------
# library statics
# --------------------------------------------------------------------------
def lib_getstatic(ev, owner, name, desc):
    if owner == "java/lang/Boolean":
        if name == "TRUE":
            return JBool(1)
        if name == "FALSE":
            return JBool(0)
    return Unknown(desc, "library static %s.%s" % (owner.rsplit("/", 1)[-1], name))


def _varargs(ev, desc, args):
    """Flatten a (possibly varargs) argument list of a collection factory."""
    params = cf.method_desc(desc)[0]
    out = list(args)
    if params and params[-1].startswith("["):
        last = ev.force(out.pop())
        if isinstance(last, Arr):
            out.extend(last.items)
        elif last is None:
            pass
        else:
            return None
    return out


def _new_list(ev, items):
    l = JList(list(items))
    for it in l.items:
        if ev.is_hot(ev.resolve(it)):
            l.hot = True
            break
    return l


def _new_set(ev, items, ordered=True):
    s = JSet([], ordered)
    for it in items:
        set_add(ev, s, it)
    return s


def _map_from_pairs(ev, flat, order="insertion"):
    m = JMap(order)
    for i in range(0, len(flat) - 1, 2):
        map_put(ev, m, flat[i], flat[i + 1])
    return m


def _box(kind):
    def f(ev, desc, args):
        v = ev.force(args[0])
        if isinstance(v, Unknown):
            return Unknown(ret_desc(desc), "boxing unknown")
        ps = cf.method_desc(desc)[0]
        if len(ps) != 1:
            return Unknown(ret_desc(desc), "number conversion variant")
        p = ps[0]
        if p == STR or p == "Ljava/lang/CharSequence;":
            if not isinstance(v, str):
                return Unknown(ret_desc(desc), "parse of non-string")
            try:
                if kind == "int":
                    return int(v)
                if kind == "bool":
                    return JBool(1 if v.lower() == "true" else 0)
                if kind == "float":
                    return f32(float(v))
                if kind == "double":
                    return BoxedDouble(float(v))
                if kind == "long":
                    return BoxedLong(int(v))
            except ValueError:
                ev.abort("number format")
        if kind == "int":
            return int(v)
        if kind == "bool":
            return JBool(1 if v else 0)
        if kind == "char":
            return JChar(v)
        if kind == "float":
            return float(v)
        if kind == "double":
            return BoxedDouble(v)
        if kind == "long":
            return BoxedLong(v)
        return v
    return f


def _parse_boolean(ev, desc, args):
    v = ev.force(args[0])
    if isinstance(v, Unknown):
        return Unknown("Z", "parseBoolean(unknown)")
    return int(isinstance(v, str) and v.lower() == "true")


def _system_property(ev, desc, args):
    """System properties / environment variables: modelled as *not set*.

    Vanilla only consults them for debug switches (MC_DEBUG_*).  The names
    asked for are recorded so that the assumption is visible in the report.
    """
    name = ev.force(args[0]) if args else None
    ev.env_queries.append(name if isinstance(name, str) else "?")
    if len(args) >= 2:
        return args[1]
    return None


def _system_flag(ev, desc, args):
    name = ev.force(args[0]) if args else None
    ev.env_queries.append(name if isinstance(name, str) else "?")
    return 0


def _require_non_null(ev, desc, args):
    return args[0]


def _require_non_null_else(ev, desc, args):
    v = ev.force(args[0])
    if isinstance(v, Unknown):
        return v
    return args[1] if v is None else args[0]


def _string_valueof(ev, desc, args):
    p = cf.method_desc(desc)[0]
    if len(p) == 1:
        return java_str(ev, args[0], p[0])
    return Unknown(STR, "String.valueOf variant")


def _list_of(ev, desc, args):
    flat = _varargs(ev, desc, args)
    if flat is None:
        return Unknown(ret_desc(desc), "list factory with unknown array")
    return _new_list(ev, flat)


def _set_of(ev, desc, args):
    flat = _varargs(ev, desc, args)
    if flat is None:
        return Unknown(ret_desc(desc), "set factory with unknown array")
    return _new_set(ev, flat)


def _map_of(ev, desc, args):
    return _map_from_pairs(ev, list(args))


def _jdk_map_of(ev, desc, args):
    # java.util.Map.of: iteration order is unspecified
    return _map_from_pairs(ev, list(args), "none")


def _new_map(order, identity=False):
    """Factory with an optional source map (Maps.newHashMap(map), newEnumMap(map), ...)."""
    def f(ev, desc, args):
        m = JMap(order, identity)
        for a in args:
            v = ev.force(a)
            if isinstance(v, (int, float, ClassRef)) and not isinstance(v, bool):
                continue
            if isinstance(v, JMap) and not v.tainted:
                for k, val in map_entries(ev, v, for_iteration=False):
                    map_put(ev, m, k, val)
                continue
            return unknown_ret(ev, desc, "map factory with an unmodelled argument", args)
        return m
    return f


def _copy_list(ev, desc, args):
    if not args:
        return JList([])
    items = iter_items(ev, args[0])
    if items is None:
        return unknown_ret(ev, desc, "copy of unknown collection", args)
    return _new_list(ev, items)


def _copy_set(ev, desc, args):
    if not args:
        return JSet([])
    items = iter_items(ev, args[0])
    if items is None:
        return unknown_ret(ev, desc, "copy of unknown collection", args)
    return _new_set(ev, items)


def _copy_map(ev, desc, args):
    src = ev.force(args[0])
    if not isinstance(src, JMap) or src.tainted:
        return unknown_ret(ev, desc, "copy of unknown map", args)
    m = JMap("insertion" if src.order != "none" else "none")
    for k, v in map_entries(ev, src, for_iteration=False):
        map_put(ev, m, k, v)
    return m


def _empty_list(ev, desc, args):
    return JList([])


def _empty_set(ev, desc, args):
    return JSet([])


def _empty_map(ev, desc, args):
    return JMap("insertion")


def _jdk_copy_map(ev, desc, args):
    m = _copy_map(ev, desc, args)
    if isinstance(m, JMap):
        m.order = "none"            # java.util.Map.copyOf: unspecified iteration order
    return m


def _jdk_set_of(ev, desc, args):
    s = _set_of(ev, desc, args)
    if isinstance(s, JSet):
        s.ordered = False           # java.util.Set.of: unspecified iteration order
    return s


def _jdk_copy_set(ev, desc, args):
    s = _copy_set(ev, desc, args)
    if isinstance(s, JSet):
        s.ordered = False
    return s


def _hash_set(ev, desc, args):
    if args:
        items = iter_items(ev, args[0])
        if items is None:
            flat = _varargs(ev, desc, args)
            if flat is None:
                return unknown_ret(ev, desc, "set of unknown", args)
            items = flat
        return _new_set(ev, items, ordered=False)
    return JSet([], ordered=False)


def _linked_set(ev, desc, args):
    if args:
        items = iter_items(ev, args[0])
        if items is None:
            return unknown_ret(ev, desc, "set of unknown", args)
        return _new_set(ev, items)
    return JSet([])


def _new_array_list(ev, desc, args):
    if not args:
        return JList([])
    params = cf.method_desc(desc)[0]
    if params == ["I"]:
        return JList([])
    if params[-1].startswith("["):
        flat = _varargs(ev, desc, args)
        return _new_list(ev, flat) if flat is not None else unknown_ret(ev, desc, "list of unknown", args)
    items = iter_items(ev, args[0])
    if items is None:
        return unknown_ret(ev, desc, "list of unknown", args)
    return _new_list(ev, items)


def _optional_of(ev, desc, args):
    o = JOptional(True, args[0])
    if ev.is_hot(ev.resolve(args[0])):
        o.hot = True
    return o


def _optional_nullable(ev, desc, args):
    v = ev.force(args[0])
    if isinstance(v, Unknown):
        return Unknown(ret_desc(desc), "Optional.ofNullable(unknown)")
    return JOptional(v is not None, v)


def _optional_empty(ev, desc, args):
    return JOptional(False)


def _int_range(ev, desc, args):
    a, b = _forced(ev, args)
    if not isinstance(a, int) or not isinstance(b, int):
        return Unknown(ret_desc(desc), "IntStream.range with unknown bounds")
    return JStream(range(a, b))


def _int_range_closed(ev, desc, args):
    a, b = _forced(ev, args)
    if not isinstance(a, int) or not isinstance(b, int):
        return Unknown(ret_desc(desc), "IntStream.rangeClosed with unknown bounds")
    return JStream(range(a, b + 1))


def _stream_of(ev, desc, args):
    flat = _varargs(ev, desc, args)
    if flat is None:
        return unknown_ret(ev, desc, "Stream.of unknown", args)
    return JStream(flat)


def _arrays_stream(ev, desc, args):
    items = iter_items(ev, args[0])
    if items is None:
        return unknown_ret(ev, desc, "Arrays.stream of unknown", args)
    return JStream(items)


def _collector(kind):
    def f(ev, desc, args):
        return Collector(kind, list(args))
    return f


def _enum_value_of(ev, desc, args):
    cls, name = _forced(ev, args)
    if not isinstance(cls, ClassRef) or not isinstance(name, str):
        return Unknown(ret_desc(desc), "Enum.valueOf unknown")
    for v in enum_constants(ev, cls.name):
        v = ev.force(v)
        if isinstance(v, Obj) and v.enum_name == name:
            return v
    ev.abort("no enum constant %s.%s" % (cls.name, name))


def enum_constants(ev, cname):
    c = ev.cp.get(cname)
    if c is None:
        ev.abort("enum constants of library class " + cname)
    m = c.method("values", "()[L%s;" % cname)
    if m is None:
        ev.abort("class %s has no values()" % cname)
    arr = ev.force(ev.exec(m, [], False))
    if not isinstance(arr, Arr):
        ev.abort("values() of %s unknown" % cname)
    return arr.items


def _math(fn, rdesc):
    def f(ev, desc, args):
        a = _forced(ev, args)
        if _has_unknown(a):
            return Unknown(ret_desc(desc), "Math on unknown")
        r = fn(*a)
        rd = ret_desc(desc)
        if rd == "J":
            return JLong(int(r))
        if rd == "D":
            return JDouble(float(r))
        if rd == "F":
            return f32(float(r))
        return int(r)
    return f


def _identity_fn(ev, desc, args):
    return LibObj("java/util/function/Function$identity")


def _string_join(ev, desc, args):
    a = _forced(ev, args)
    if _has_unknown(a) or not isinstance(a[0], str):
        return Unknown(STR, "String.join unknown")
    items = iter_items(ev, a[1])
    if items is None:
        return Unknown(STR, "String.join unknown elements")
    parts = [java_str(ev, x) for x in items]
    if _has_unknown(parts):
        return Unknown(STR, "String.join unknown elements")
    return a[0].join(parts)


def _objects_equals(ev, desc, args):
    r = java_equals(ev, args[0], args[1])
    return Unknown("Z", "Objects.equals unknown") if r is None else int(r)


def _util_noop_first(ev, desc, args):
    return args[0]


LIB_STATIC = {
    ("java/util/Objects", "requireNonNull"): _require_non_null,
    ("java/util/Objects", "requireNonNullElse"): _require_non_null_else,
    ("java/util/Objects", "equals"): _objects_equals,
    ("com/google/common/base/Preconditions", "checkNotNull"): _require_non_null,
    ("java/lang/String", "valueOf"): _string_valueof,
    ("java/lang/String", "join"): _string_join,
    ("java/lang/Integer", "valueOf"): _box("int"),
    ("java/lang/Integer", "parseInt"): _box("int"),
    ("java/lang/Short", "valueOf"): _box("int"),
    ("java/lang/Byte", "valueOf"): _box("int"),
    ("java/lang/Boolean", "valueOf"): _box("bool"),
    ("java/lang/Boolean", "parseBoolean"): _parse_boolean,
    ("java/lang/System", "getProperty"): _system_property,
    ("java/lang/System", "getenv"): _system_property,
    ("java/lang/Boolean", "getBoolean"): _system_flag,
    ("java/lang/Integer", "getInteger"): _system_property,
    ("java/lang/Long", "getLong"): _system_property,
    ("java/lang/Character", "valueOf"): _box("char"),
    ("java/lang/Float", "valueOf"): _box("float"),
    ("java/lang/Double", "valueOf"): _box("double"),
    ("java/lang/Long", "valueOf"): _box("long"),
    ("java/util/List", "of"): _list_of,
    ("java/util/List", "copyOf"): _copy_list,
    ("java/util/Set", "of"): _jdk_set_of,
    ("java/util/Set", "copyOf"): _jdk_copy_set,
    ("java/util/Map", "of"): _jdk_map_of,
    ("java/util/Map", "copyOf"): _jdk_copy_map,
    ("java/util/Map", "entry"): lambda ev, desc, args: JEntry(args[0], args[1]),
    ("java/util/Arrays", "asList"): _list_of,
    ("java/util/Arrays", "stream"): _arrays_stream,
    ("java/util/Collections", "emptyList"): _empty_list,
    ("java/util/Collections", "emptySet"): _empty_set,
    ("java/util/Collections", "emptyMap"): _empty_map,
    ("java/util/Collections", "singletonList"): _list_of,
    ("java/util/Collections", "singleton"): _set_of,
    ("java/util/Collections", "unmodifiableList"): _util_noop_first,
    ("java/util/Collections", "unmodifiableSet"): _util_noop_first,
    ("java/util/Collections", "unmodifiableMap"): _util_noop_first,
    ("java/util/Collections", "unmodifiableCollection"): _util_noop_first,
    ("java/util/Optional", "of"): _optional_of,
    ("java/util/Optional", "ofNullable"): _optional_nullable,
    ("java/util/Optional", "empty"): _optional_empty,
    ("java/util/stream/IntStream", "range"): _int_range,
    ("java/util/stream/IntStream", "rangeClosed"): _int_range_closed,
    ("java/util/stream/Stream", "of"): _stream_of,
    ("java/util/stream/Stream", "empty"): lambda ev, desc, args: JStream([]),
    ("java/util/stream/Collectors", "toList"): _collector("list"),
    ("java/util/stream/Collectors", "toUnmodifiableList"): _collector("list"),
    ("java/util/stream/Collectors", "toSet"): _collector("hashset"),
    ("java/util/stream/Collectors", "toUnmodifiableSet"): _collector("hashset"),
    ("java/util/stream/Collectors", "toMap"): _collector("hashmap"),
    ("java/util/stream/Collectors", "toUnmodifiableMap"): _collector("hashmap"),
    ("java/util/stream/Collectors", "joining"): _collector("joining"),
    ("com/google/common/collect/ImmutableList", "toImmutableList"): _collector("list"),
    ("com/google/common/collect/ImmutableSet", "toImmutableSet"): _collector("set"),
    ("com/google/common/collect/ImmutableMap", "toImmutableMap"): _collector("map"),
    ("com/google/common/collect/ImmutableList", "of"): _list_of,
    ("com/google/common/collect/ImmutableList", "copyOf"): _copy_list,
    ("com/google/common/collect/ImmutableList", "builder"): _empty_list,
    ("com/google/common/collect/ImmutableList", "builderWithExpectedSize"): _empty_list,
    ("com/google/common/collect/ImmutableSet", "of"): _set_of,
    ("com/google/common/collect/ImmutableSet", "copyOf"): _copy_set,
    ("com/google/common/collect/ImmutableSet", "builder"): _empty_set,
    ("com/google/common/collect/ImmutableMap", "of"): _map_of,
    ("com/google/common/collect/ImmutableMap", "copyOf"): _copy_map,
    ("com/google/common/collect/ImmutableMap", "builder"): _empty_map,
    ("com/google/common/collect/ImmutableMap", "builderWithExpectedSize"): _empty_map,
    ("com/google/common/collect/Lists", "newArrayList"): _new_array_list,
    ("com/google/common/collect/Lists", "newArrayListWithCapacity"): _empty_list,
    ("com/google/common/collect/Lists", "newArrayListWithExpectedSize"): _empty_list,
    ("com/google/common/collect/Lists", "newLinkedList"): _new_array_list,
    ("com/google/common/collect/Maps", "newLinkedHashMap"): _new_map("insertion"),
    ("com/google/common/collect/Maps", "newHashMap"): _new_map("none"),
    ("com/google/common/collect/Maps", "newIdentityHashMap"): _new_map("none", True),
    ("com/google/common/collect/Maps", "newConcurrentMap"): _new_map("none"),
    ("com/google/common/collect/Maps", "newEnumMap"): _new_map("enum"),
    ("com/google/common/collect/Sets", "newHashSet"): _hash_set,
    ("com/google/common/collect/Sets", "newIdentityHashSet"): _hash_set,
    ("com/google/common/collect/Sets", "newLinkedHashSet"): _linked_set,
    ("com/google/common/collect/Sets", "newConcurrentHashSet"): _hash_set,
    ("java/lang/Enum", "valueOf"): _enum_value_of,
    ("java/lang/Math", "max"): _math(max, None),
    ("java/lang/Math", "min"): _math(min, None),
    ("java/lang/Math", "abs"): _math(abs, None),
    ("java/lang/Math", "floor"): _math(math.floor, None),
    ("java/lang/Math", "ceil"): _math(math.ceil, None),
    ("java/lang/Math", "floorDiv"): _math(lambda a, b: a // b, None),
    ("java/lang/Math", "floorMod"): _math(lambda a, b: a % b, None),
    ("java/util/function/Function", "identity"): _identity_fn,
    ("java/util/function/UnaryOperator", "identity"): _identity_fn,
}


def lib_static(ev, owner, name, desc, args):
    h = LIB_STATIC.get((owner, name))
    if h is not None:
        return h(ev, desc, args)
    return ev.unknown_result(desc, "library call %s.%s" % (owner.rsplit("/", 1)[-1], name), args)


# --------------------------------------------------------------------------
# library object creation
# --------------------------------------------------------------------------
def lib_alloc(ev, cname):
    if cname in _LIST_CLASSES:
        return JList([])
    order = _MAP_CLASSES.get(cname)
    if order is not None:
        return JMap(order, cname in _IDENTITY_MAPS)
    ordered = _SET_CLASSES.get(cname)
    if ordered is not None:
        return JSet([], ordered)
    if cname in _SB_CLASSES:
        return JSB()
    return LibObj(cname)


def lib_init(ev, owner, desc, recv, args, frame):
    r = ev.resolve(recv)
    if isinstance(r, Obj):
        # super constructor of a jar class that lives in a library
        if owner == "java/lang/Enum":
            name, ordinal = _forced(ev, args)
            r.enum_name = name if isinstance(name, str) else None
            r.enum_ordinal = ordinal if isinstance(ordinal, int) else None
        return VOID
    params = cf.method_desc(desc)[0]
    if isinstance(r, (JList, JSet, JMap)):
        # arguments are capacities / load factors / element classes (ignored) or a
        # source collection (copied); anything else makes the contents unknown
        for a in args:
            v = ev.force(a)
            if isinstance(v, (int, float, ClassRef)) and not isinstance(v, bool):
                continue
            if isinstance(r, JMap):
                if isinstance(v, JMap) and not v.tainted:
                    for k, val in map_entries(ev, v, for_iteration=False):
                        map_put(ev, r, k, val)
                    continue
            else:
                items = iter_items(ev, v) if not isinstance(v, JMap) else None
                if items is not None:
                    if isinstance(r, JList):
                        r.items.extend(items)
                    else:
                        for it in items:
                            set_add(ev, r, it)
                    for it in items:
                        if ev.is_hot(ev.resolve(it)):
                            ev.mark_hot_container(r)
                            break
                    continue
            if ev.is_hot(ev.resolve(a)):
                ev.abort("registering value passed to a collection constructor in an unmodelled way")
            r.tainted = True
        return VOID
    if isinstance(r, JSB):
        if len(params) == 1 and params[0] in (STR, "Ljava/lang/CharSequence;"):
            s = java_str(ev, args[0])
            if isinstance(s, Unknown):
                r.unknown = True
            else:
                r.parts.append(s)
        return VOID
    if isinstance(r, LibObj):
        r.args = list(args)
        for a in args:
            if ev.is_hot(ev.resolve(a)):
                ev.abort("registering lambda passed to constructor of unmodelled %s" % owner)
    return VOID


def lib_new(ev, cname, desc, args):
    o = lib_alloc(ev, cname)
    lib_init(ev, cname, desc, o, args, None)
    return o


# --------------------------------------------------------------------------
# methods on jar objects that are inherited from library classes
# --------------------------------------------------------------------------
def obj_method(ev, obj, name, desc, args):
    if name == "getClass" and not args:
        return ClassRef(obj.cls)
    if name == "name" and not args and obj.enum_name is not None:
        return obj.enum_name
    if name == "ordinal" and not args and obj.enum_ordinal is not None:
        return obj.enum_ordinal
    if name == "toString" and not args:
        return java_str(ev, obj)
    if name == "equals" and len(args) == 1:
        r = java_equals(ev, obj, args[0])
        return Unknown("Z", "equals unknown") if r is None else int(r)
    if name == "hashCode" and not args:
        return Unknown("I", "identity hash code")
    if name == "compareTo" and len(args) == 1 and obj.enum_ordinal is not None:
        o = ev.force(args[0])
        if isinstance(o, Obj) and o.enum_ordinal is not None:
            return obj.enum_ordinal - o.enum_ordinal
    if name == "getDeclaringClass" and obj.enum_name is not None:
        c = ev.cp.get(obj.cls)
        return ClassRef(obj.cls if c.super_name == "java/lang/Enum" else c.super_name)
    return ev.unknown_result(desc, "inherited library method %s on %s" % (name, obj.cls), args)


def lambda_default_method(ev, lam, name, desc, args):
    if name in ("equals", "hashCode", "toString", "getClass"):
        return ev.unknown_result(desc, "%s on lambda" % name)
    if lam.hot:
        ev.abort("default method %s on a registering lambda is not modelled" % name)
    return ev.unknown_result(desc, "default method %s of %s" % (name, lam.iface), args)


def lib_method_ref(ev, lam, allargs):
    if lam.kind == cf.REF_invokeStatic:
        return lib_static(ev, lam.owner, lam.name, lam.desc, allargs)
    if lam.kind == cf.REF_newInvokeSpecial:
        return lib_new(ev, lam.owner, lam.desc, allargs)
    recv = ev.force(allargs[0])
    rest = allargs[1:]
    if recv is None:
        ev.abort("null receiver for method reference %s.%s" % (lam.owner, lam.name))
    if isinstance(recv, Lambda):
        return ev.invoke_lambda(recv, lam.name, lam.desc, rest)
    if isinstance(recv, Obj):
        m = ev.resolve_virtual(recv.cls, lam.name, lam.desc)
        if m is not None:
            return ev.call_jar(m, [recv] + rest)
        return obj_method(ev, recv, lam.name, lam.desc, rest)
    if isinstance(recv, Unknown):
        return ev.unknown_result(lam.desc, "method ref %s on unknown" % lam.name, rest)
    return native_method(ev, recv, lam.owner, lam.name, lam.desc, rest)


def native_instanceof(ev, v, cname):
    if isinstance(v, JList):
        if cname in ("java/util/List", "java/util/Collection", "java/lang/Iterable"):
            return True
        return None
    if isinstance(v, JMap):
        return True if cname == "java/util/Map" else None
    if isinstance(v, JSet):
        if cname in ("java/util/Set", "java/util/Collection", "java/lang/Iterable"):
            return True
        return None
    return None


def other_indy(ev, c, bowner, bname, bargs, name, desc, args):
    ret = ret_desc(desc)
    if bowner == "java/lang/runtime/ObjectMethods":
        if name == "equals":
            r = java_equals(ev, args[0], args[1])
            return Unknown("Z", "record equals unknown") if r is None else int(r)
        return Unknown(ret, "record %s" % name)
    if bowner == "java/lang/runtime/SwitchBootstraps":
        target = ev.force(args[0])
        restart = ev.force(args[1])
        if isinstance(target, Unknown) or not isinstance(restart, int):
            return Unknown("I", "pattern switch on unknown")
        if target is None:
            return -1
        labels = []
        for a in bargs:
            labels.append(c.loadable(a))
        for idx in range(restart, len(labels)):
            kind, val = labels[idx]
            if kind == "class":
                r = ev.instance_of(target, val)
                if r is None:
                    return Unknown("I", "pattern switch undecidable")
                if r:
                    return idx
            elif kind == "string":
                if bname == "enumSwitch":
                    if isinstance(target, Obj) and target.enum_name == val:
                        return idx
                elif isinstance(target, str) and target == val:
                    return idx
            elif kind == "int":
                if isinstance(target, int) and target == val:
                    return idx
            else:
                return Unknown("I", "pattern switch label kind " + kind)
        return len(labels)
    for a in args:
        if ev.is_hot(ev.resolve(a)):
            ev.abort("registering lambda captured by unmodelled invokedynamic %s.%s" % (bowner, bname))
    if ret == "V":
        return VOID
    return Unknown(ret, "invokedynamic %s.%s" % (bowner.rsplit("/", 1)[-1], bname))


# --------------------------------------------------------------------------
# methods on native values
# --------------------------------------------------------------------------
_MUTATORS = frozenset(["add", "addAll", "put", "putAll", "append"])


def native_method(ev, recv, owner, name, desc, args):
    t = type(recv)
    if getattr(recv, "tainted", False) and name not in _MUTATORS:
        # a call that was mutating this collection was abandoned: contents unknown
        return ev.unknown_result(desc, "%s on a collection with unknown contents" % name, args)
    if t is str:
        return str_method(ev, recv, name, desc, args)
    if t is JList:
        return list_method(ev, recv, name, desc, args)
    if t is JMap:
        return map_method(ev, recv, name, desc, args)
    if t is JSet:
        return set_method(ev, recv, name, desc, args)
    if t is JIter:
        return iter_method(ev, recv, name, desc, args)
    if t is JEntry:
        if name == "getKey":
            return recv.key
        if name == "getValue":
            return recv.value
        return ev.unknown_result(desc, "Map.Entry." + name, args)
    if t is JOptional:
        return optional_method(ev, recv, name, desc, args)
    if t is JStream:
        return stream_method(ev, recv, name, desc, args)
    if t is JSB:
        return sb_method(ev, recv, name, desc, args)
    if t is ClassRef:
        return class_method(ev, recv, name, desc, args)
    if t is JHolder:
        return holder_method(ev, recv, name, desc, args)
    if t is Arr:
        if name == "clone":
            a = Arr(recv.desc, list(recv.items))
            a.hot = recv.hot
            return a
        if name == "getClass":
            return ClassRef("[" + recv.desc)
        return ev.unknown_result(desc, "array method " + name, args)
    if isinstance(recv, (Ident, RKey)):
        h = ev.native_jar_classes["net/minecraft/resources/Identifier" if isinstance(recv, Ident)
                                  else "net/minecraft/resources/ResourceKey"]
        return h(ev, "invokevirtual", name, desc, [recv] + list(args))
    if isinstance(recv, (int, float)):
        return number_method(ev, recv, name, desc, args)
    if t is LibObj:
        if recv.cls == "java/util/function/Function$identity" and name == "apply":
            return args[0]
        return ev.unknown_result(desc, "%s.%s on unmodelled %s" % (
            (owner or "?").rsplit("/", 1)[-1], name, recv.cls), args)
    if t is Collector:
        return ev.unknown_result(desc, "collector method " + name, args)
    return ev.unknown_result(desc, "method %s on %s" % (name, t.__name__), args)


def number_method(ev, v, name, desc, args):
    if name in ("intValue", "shortValue", "byteValue"):
        return int(v)
    if name == "booleanValue":
        return int(v)
    if name == "charValue":
        return int(v)
    if name == "longValue":
        return JLong(int(v))
    if name == "floatValue":
        return f32(float(v))
    if name == "doubleValue":
        return JDouble(float(v))
    if name == "toString":
        return java_str(ev, v)
    if name == "equals":
        o = ev.force(args[0])
        if isinstance(o, Unknown):
            return Unknown("Z", "equals unknown")
        return int(isinstance(o, (int, float)) and o == v)
    if name == "hashCode":
        if isinstance(v, JBool):
            return 1231 if v else 1237
        if isinstance(v, int) and not isinstance(v, (BoxedLong, JLong)):
            return int(v)
        return Unknown("I", "hashCode of number")
    if name == "compareTo" or name == "compare":
        o = ev.force(args[0])
        if isinstance(o, Unknown):
            return Unknown("I", "compare unknown")
        return (v > o) - (v < o)
    return ev.unknown_result(desc, "number method " + name, args)


def str_method(ev, s, name, desc, args):
    if name in ("toLowerCase", "toUpperCase"):
        return s.lower() if name == "toLowerCase" else s.upper()    # locale argument is irrelevant here
    a = _forced(ev, args)
    if _has_unknown(a):
        return ev.unknown_result(desc, "String.%s with unknown argument" % name)
    n = len(a)
    if name == "length":
        return len(s)
    if name == "isEmpty":
        return int(len(s) == 0)
    if name == "isBlank":
        return int(len(s.strip()) == 0)
    if name == "charAt" or name == "codePointAt":
        if a[0] < 0 or a[0] >= len(s):
            ev.abort("string index out of range")
        return ord(s[a[0]])
    if name == "equals":
        return int(isinstance(a[0], str) and a[0] == s)
    if name == "equalsIgnoreCase":
        return int(isinstance(a[0], str) and a[0].lower() == s.lower())
    if name == "hashCode":
        return java_string_hash(s)
    if name in ("toString", "intern", "strip", "trim"):
        return s.strip() if name in ("strip", "trim") else s
    if name == "substring":
        return s[a[0]:] if n == 1 else s[a[0]:a[1]]
    if name == "subSequence":
        return s[a[0]:a[1]]
    if name == "indexOf":
        needle = chr(a[0]) if isinstance(a[0], int) else a[0]
        return s.find(needle) if n == 1 else s.find(needle, a[1])
    if name == "lastIndexOf":
        needle = chr(a[0]) if isinstance(a[0], int) else a[0]
        return s.rfind(needle) if n == 1 else s.rfind(needle, 0, a[1] + len(needle))
    if name == "startsWith":
        return int(s.startswith(a[0]) if n == 1 else s.startswith(a[0], a[1]))
    if name == "endsWith":
        return int(s.endswith(a[0]))
    if name == "contains":
        return int(isinstance(a[0], str) and a[0] in s)
    if name == "concat":
        return s + a[0]
    if name == "toLowerCase":
        return s.lower()
    if name == "toUpperCase":
        return s.upper()
    if name == "replace":
        x, y = a
        if isinstance(x, int):
            x = chr(x)
        if isinstance(y, int):
            y = chr(y)
        if not isinstance(x, str) or not isinstance(y, str):
            return Unknown(STR, "String.replace with non-strings")
        return s.replace(x, y)
    if name == "compareTo":
        return (s > a[0]) - (s < a[0])
    if name == "repeat":
        return s * a[0]
    if name == "toCharArray":
        return Arr("C", [ord(ch) for ch in s])
    if name == "chars":
        return JStream([ord(ch) for ch in s])
    if name == "split" and n == 1 and isinstance(a[0], str) and not any(ch in a[0] for ch in ".[]()*+?^$|\\{}"):
        parts = s.split(a[0])
        while parts and parts[-1] == "":
            parts.pop()
        return Arr(STR, parts)
    return ev.unknown_result(desc, "String." + name)


def sb_method(ev, sb, name, desc, args):
    if name == "append":
        p = cf.method_desc(desc)[0]
        if len(p) != 1:
            sb.unknown = True
            return sb
        s = java_str(ev, args[0], p[0])
        if isinstance(s, Unknown):
            sb.unknown = True
        else:
            sb.parts.append(s)
        return sb
    if name == "toString":
        return sb_str(sb)
    if name == "length":
        s = sb_str(sb)
        return Unknown("I", "length of unknown builder") if isinstance(s, Unknown) else len(s)
    if name == "isEmpty":
        s = sb_str(sb)
        return Unknown("Z", "unknown builder") if isinstance(s, Unknown) else int(len(s) == 0)
    sb.unknown = True
    r = ret_desc(desc)
    if r == "V":
        return VOID
    if r in ("Ljava/lang/StringBuilder;", "Ljava/lang/StringBuffer;"):
        return sb
    return Unknown(r, "StringBuilder." + name)


def _ret_self_or_bool(ev, self_obj, desc, flag=1):
    r = ret_desc(desc)
    if r == "V":
        return VOID
    if r == "Z":
        return flag
    return self_obj


def list_method(ev, l, name, desc, args):
    n = len(args)
    if name == "add":
        if n == 1:
            l.items.append(args[0])
            if ev.is_hot(ev.resolve(args[0])):
                ev.mark_hot_container(l)
            return _ret_self_or_bool(ev, l, desc)
        idx = ev.force(args[0])
        if not isinstance(idx, int):
            ev.abort("List.add at unknown index")
        l.items.insert(idx, args[1])
        if ev.is_hot(ev.resolve(args[1])):
            ev.mark_hot_container(l)
        return VOID
    if name == "addAll":
        src = args[-1]
        flat = None
        if n == 1:
            params = cf.method_desc(desc)[0]
            if params[0].startswith("["):
                arr = ev.force(src)
                flat = list(arr.items) if isinstance(arr, Arr) else None
            else:
                flat = iter_items(ev, src)
        if flat is None:
            ev.abort("List.addAll of unknown collection")
        l.items.extend(flat)
        for it in flat:
            if ev.is_hot(ev.resolve(it)):
                ev.mark_hot_container(l)
                break
        return _ret_self_or_bool(ev, l, desc)
    if name == "get" or name == "elementAt":
        idx = ev.force(args[0])
        if not isinstance(idx, int):
            return Unknown(OBJ, "List.get(unknown)")
        if idx < 0 or idx >= len(l.items):
            ev.abort("list index out of bounds")
        return l.items[idx]
    if name == "getFirst":
        if not l.items:
            ev.abort("empty list")
        return l.items[0]
    if name == "getLast":
        if not l.items:
            ev.abort("empty list")
        return l.items[-1]
    if name == "set":
        idx = ev.force(args[0])
        if not isinstance(idx, int):
            ev.abort("List.set at unknown index")
        old = l.items[idx]
        l.items[idx] = args[1]
        return old
    if name == "size":
        return len(l.items)
    if name == "isEmpty":
        return int(not l.items)
    if name == "iterator" or name == "listIterator":
        return JIter(list(l.items))
    if name == "forEach":
        for it in list(l.items):
            call_fn(ev, args[0], [it], _FN1)
        return VOID
    if name == "stream":
        return JStream(list(l.items))
    if name in ("build", "buildOrThrow", "asList", "reversed"):
        nl = JList(list(l.items) if name != "reversed" else list(reversed(l.items)))
        nl.hot = l.hot
        return nl
    if name == "contains":
        for it in l.items:
            r = java_equals(ev, it, args[0])
            if r is None:
                return Unknown("Z", "List.contains unknown")
            if r:
                return 1
        return 0
    if name == "indexOf":
        for i, it in enumerate(l.items):
            r = java_equals(ev, it, args[0])
            if r is None:
                return Unknown("I", "List.indexOf unknown")
            if r:
                return i
        return -1
    if name == "toArray":
        return Arr(OBJ, list(l.items))
    if name == "clear":
        del l.items[:]
        return VOID
    if name == "subList":
        a, b = _forced(ev, args)
        return JList(l.items[a:b])
    if name in ("hashCode", "equals", "toString", "getClass"):
        return ev.unknown_result(desc, "List." + name)
    ev.abort("unmodelled List method %s%s" % (name, desc))


def set_method(ev, s, name, desc, args):
    if name == "add":
        r = set_add(ev, s, args[0])
        return _ret_self_or_bool(ev, s, desc, r)
    if name == "addAll":
        params = cf.method_desc(desc)[0]
        if params[0].startswith("["):
            arr = ev.force(args[0])
            items = list(arr.items) if isinstance(arr, Arr) else None
        else:
            items = iter_items(ev, args[0])
        if items is None:
            ev.abort("Set.addAll of unknown collection")
        for it in items:
            set_add(ev, s, it)
        return _ret_self_or_bool(ev, s, desc)
    if name == "contains":
        kid = key_id(ev, args[0])
        if kid is None:
            return Unknown("Z", "Set.contains(unknown)")
        for it in s.items:
            k2 = key_id(ev, it)
            if k2 is None:
                return Unknown("Z", "Set.contains with unknown element")
            if k2 == kid:
                return 1
        return 0
    if name == "size":
        return len(s.items)
    if name == "isEmpty":
        return int(not s.items)
    if name == "iterator":
        return JIter(iter_items(ev, s))
    if name == "forEach":
        for it in iter_items(ev, s):
            call_fn(ev, args[0], [it], _FN1)
        return VOID
    if name == "stream":
        return JStream(iter_items(ev, s))
    if name in ("build", "asList"):
        if name == "asList":
            return JList(iter_items(ev, s))
        ns = JSet(list(s.items), s.ordered)
        ns.hot = s.hot
        return ns
    if name in ("hashCode", "equals", "toString", "getClass"):
        return ev.unknown_result(desc, "Set." + name)
    ev.abort("unmodelled Set method %s%s" % (name, desc))


def map_method(ev, m, name, desc, args):
    ret = ret_desc(desc)
    if name == "put":
        old = map_put(ev, m, args[0], args[1])
        if ret == OBJ:
            return old
        if ret == "V":
            return VOID
        return m
    if name == "putIfAbsent":
        has = map_has(ev, m, args[0])
        if has is None:
            ev.abort("Map.putIfAbsent with unknown key")
        if has:
            return map_get(ev, m, args[0])
        map_put(ev, m, args[0], args[1])
        return None
    if name == "putAll":
        src = ev.force(args[0])
        if not isinstance(src, JMap):
            ev.abort("Map.putAll of unknown map")
        for k, v in map_entries(ev, src):
            map_put(ev, m, k, v)
        return VOID if ret == "V" else m
    if name == "get":
        return map_get(ev, m, args[0])
    if name == "getOrDefault":
        has = map_has(ev, m, args[0])
        if has is None:
            return Unknown(OBJ, "Map.getOrDefault unknown key")
        return map_get(ev, m, args[0]) if has else args[1]
    if name == "containsKey":
        has = map_has(ev, m, args[0])
        return Unknown("Z", "Map.containsKey unknown key") if has is None else int(has)
    if name == "size":
        map_sync(ev, m)
        if m.imprecise:
            return Unknown("I", "size of map with unknown keys")
        return len(m._norm)
    if name == "isEmpty":
        return int(not m.raw)
    if name == "forEach":
        for k, v in map_entries(ev, m):
            call_fn(ev, args[0], [k, v], _FN2)
        return VOID
    if name == "entrySet" or name == "entries":
        return JSet([JEntry(k, v) for k, v in map_entries(ev, m)])
    if name == "keySet":
        return JSet([k for k, v in map_entries(ev, m)])
    if name == "values":
        return JList([v for k, v in map_entries(ev, m)])
    if name == "computeIfAbsent":
        has = map_has(ev, m, args[0])
        if has is None:
            ev.abort("Map.computeIfAbsent with unknown key")
        if has:
            return map_get(ev, m, args[0])
        v = call_fn(ev, args[1], [args[0]], _FN1)
        map_put(ev, m, args[0], v)
        return v
    if name in ("build", "buildOrThrow", "buildKeepingLast"):
        nm = JMap("insertion" if m.order == "insertion" else m.order)
        for k, v in map_entries(ev, m, for_iteration=False):
            map_put(ev, nm, k, v)
        nm.hot = m.hot
        return nm
    if name == "remove":
        ev.abort("Map.remove is not modelled")
    if name in ("hashCode", "equals", "toString", "getClass"):
        return ev.unknown_result(desc, "Map." + name)
    ev.abort("unmodelled Map method %s%s" % (name, desc))


def iter_method(ev, it, name, desc, args):
    if name == "hasNext":
        return int(it.pos < len(it.items))
    if name == "next":
        if it.pos >= len(it.items):
            ev.abort("iterator exhausted")
        v = it.items[it.pos]
        it.pos += 1
        return v
    if name == "forEachRemaining":
        while it.pos < len(it.items):
            v = it.items[it.pos]
            it.pos += 1
            call_fn(ev, args[0], [v], _FN1)
        return VOID
    ev.abort("unmodelled Iterator method " + name)


def optional_method(ev, o, name, desc, args):
    if name == "isPresent":
        return int(o.present)
    if name == "isEmpty":
        return int(not o.present)
    if name in ("get", "orElseThrow"):
        if not o.present:
            ev.abort("Optional.get on empty")
        return o.value
    if name == "orElse":
        return o.value if o.present else args[0]
    if name == "orElseGet":
        return o.value if o.present else call_fn(ev, args[0], [], _FN0)
    if name == "ifPresent":
        if o.present:
            call_fn(ev, args[0], [o.value], _FN1)
        return VOID
    if name == "map":
        if not o.present:
            return o
        return JOptional(True, call_fn(ev, args[0], [o.value], _FN1))
    if name == "stream":
        return JStream([o.value] if o.present else [])
    return ev.unknown_result(desc, "Optional." + name, args)


def stream_method(ev, s, name, desc, args):
    if name in ("map", "mapToObj", "mapToInt"):
        return s.with_op(("map", args[0]))
    if name == "filter":
        return s.with_op(("filter", args[0]))
    if name == "peek":
        return s.with_op(("peek", args[0]))
    if name == "flatMap":
        return s.with_op(("flatMap", args[0]))
    if name in ("boxed", "sequential", "parallel", "unordered", "onClose"):
        return s
    if name in ("forEach", "forEachOrdered"):
        for v in stream_iter(ev, s):
            call_fn(ev, args[0], [v], _FN1)
        return VOID
    if name == "toList":
        return _new_list(ev, list(stream_iter(ev, s)))
    if name == "toArray":
        return Arr(OBJ, list(stream_iter(ev, s)))
    if name == "iterator":
        return JIter(list(stream_iter(ev, s)))
    if name == "count":
        return JLong(len(list(stream_iter(ev, s))))
    if name == "findFirst" or name == "findAny":
        for v in stream_iter(ev, s):
            return JOptional(True, v)
        return JOptional(False)
    if name == "collect" and len(args) == 1:
        items = list(stream_iter(ev, s))        # evaluates the pipeline in encounter order
        col = ev.force(args[0])
        if isinstance(col, Collector):
            if col.kind == "list":
                return _new_list(ev, items)
            if col.kind == "set":
                return _new_set(ev, items)
            if col.kind == "hashset":
                return _new_set(ev, items, ordered=False)
            if col.kind in ("map", "hashmap") and len(col.args) >= 2:
                m = JMap("insertion" if col.kind == "map" else "none")
                for it in items:
                    k = call_fn(ev, col.args[0], [it], _FN1)
                    v = call_fn(ev, col.args[1], [it], _FN1)
                    map_put(ev, m, k, v)
                return m
            if col.kind == "joining" and len(col.args) <= 1:
                parts = [java_str(ev, x) for x in items]
                sep = ev.force(col.args[0]) if col.args else ""
                if _has_unknown(parts) or not isinstance(sep, str):
                    return Unknown(STR, "joining unknown")
                return sep.join(parts)
        return Unknown(ret_desc(desc), "stream collected by unmodelled collector")
    if name in ("anyMatch", "allMatch", "noneMatch"):
        want = {"anyMatch": True, "allMatch": False, "noneMatch": True}[name]
        for v in stream_iter(ev, s):
            r = ev.force(call_fn(ev, args[0], [v], _FN1))
            if isinstance(r, Unknown):
                return Unknown("Z", "stream match unknown")
            if bool(r) == want:
                return int(name == "anyMatch")
        return int(name != "anyMatch")
    for a in args:
        if ev.is_hot(ev.resolve(a)):
            ev.abort("registering lambda passed to unmodelled Stream.%s" % name)
    ev.abort("unmodelled Stream method %s%s" % (name, desc))


def class_method(ev, cref, name, desc, args):
    if name == "desiredAssertionStatus":
        return 0
    if name == "getSimpleName":
        n = cref.name.rsplit("/", 1)[-1]
        return n.rsplit("$", 1)[-1]
    if name == "getName" or name == "getTypeName":
        return cref.name.replace("/", ".")
    if name == "getEnumConstants":
        if not ev.cp.has(cref.name):
            return Unknown(ret_desc(desc), "enum constants of library class")
        return Arr("L%s;" % cref.name, list(enum_constants(ev, cref.name)))
    if name == "isInstance":
        v = ev.force(args[0])
        if isinstance(v, Unknown):
            return Unknown("Z", "isInstance unknown")
        r = ev.instance_of(v, cref.name)
        return Unknown("Z", "isInstance undecidable") if r is None else int(r)
    if name == "cast":
        return args[0]
    if name == "isEnum":
        c = ev.cp.get(cref.name)
        return Unknown("Z", "isEnum of library class") if c is None else int(c.super_name == "java/lang/Enum")
    if name in ("equals",):
        o = ev.force(args[0])
        return int(isinstance(o, ClassRef) and o == cref)
    return ev.unknown_result(desc, "Class." + name, args)


def holder_method(ev, h, name, desc, args):
    if name == "value":
        return h.value
    if name == "key":
        return h.key
    if name == "unwrapKey":
        return JOptional(True, h.key)
    if name == "getRegisteredName":
        return str(h.key.ident)
    if name == "isBound":
        return 1
    if name == "is" and len(args) == 1:
        a = ev.force(args[0])
        if isinstance(a, RKey):
            return int(a == h.key)
        if isinstance(a, Ident):
            return int(a == h.key.ident)
    return ev.unknown_result(desc, "Holder." + name, args)
