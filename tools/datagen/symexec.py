"""Static symbolic evaluator for JVM bytecode (constant propagation).

This is *static analysis*: the class files are decoded by classfile.py and the
instruction streams of the interesting static initialisers / bootstrap methods
are walked with a symbolic operand stack in order to recover

  * the order of registry registrations (``WritableRegistry.register`` calls),
  * the constant names passed to them,
  * the builder objects that hold packet lists, block properties, ...

Nothing is loaded into a JVM and no library code (JDK, Guava, DFU, netty, ...)
is available or evaluated; calls into libraries are either modelled by a small
table of well-known collection/string helpers (see intrinsics.py) or yield an
``Unknown`` value.  The evaluator never guesses: whenever something it needs
(a branch condition, a registry key, ...) is ``Unknown`` it raises ``Abort``
and the affected unit of work is reported as failed.

Evaluation strategy
-------------------
Code is evaluated strictly in program order (so effects on shared objects are
observed in the order the JVM would produce them), with two failure modes:

  * "hard" frames -- methods that can reach a *sink* (e.g. a registry
    registration) through the static call graph, or that receive a lambda that
    can: any ``Abort`` inside is fatal for the current unit of work (a class
    initialiser or a registry bootstrap), which is then reported as failed;
  * all other calls are tolerant: an ``Abort`` inside merely turns the result
    into ``Unknown`` and marks the objects involved as partially known.

Class initialisers follow the JVM rules: a class is initialised once, superclass
first, at its first active use (static field access, static call, ``new``).
Initialisers that can reach a sink are hard units, all others are tolerant.
(``Evaluator(lazy_init=True)`` instead defers the tolerant ones until one of
their static fields is actually read -- ``Thunk`` values stand for such
pending reads.  Both strategies must give the same tables; extract.py can run
both and compare, see ``--double-check``.)
"""

import struct
import sys

import classfile as cf

sys.setrecursionlimit(1000000)


# --------------------------------------------------------------------------
# values
# --------------------------------------------------------------------------
class Abort(Exception):
    """Evaluation cannot continue (unknown value needed, unsupported construct)."""

    def __init__(self, msg, trace=None):
        Exception.__init__(self, msg)
        self.msg = msg
        self.trace = trace
        self.recorded = False


class Unknown:
    __slots__ = ("desc", "why")

    def __init__(self, desc="Ljava/lang/Object;", why=None):
        self.desc = desc
        self.why = why

    def __repr__(self):
        return "Unknown<%s%s>" % (self.desc, (" " + self.why) if self.why else "")


class JLong(int):
    """primitive long (category 2)"""
    __slots__ = ()


class JDouble(float):
    """primitive double (category 2)"""
    __slots__ = ()


class JBool(int):
    """boxed java.lang.Boolean"""
    __slots__ = ()


class JChar(int):
    """boxed java.lang.Character"""
    __slots__ = ()


class BoxedLong(int):
    __slots__ = ()


class BoxedDouble(float):
    __slots__ = ()


class _Void:
    def __repr__(self):
        return "VOID"


VOID = _Void()


class Obj:
    """Instance of a class from the analysed jar."""
    __slots__ = ("cls", "fields", "partial", "hot", "origins", "enum_name", "enum_ordinal",
                 "ctor_args", "tag")

    def __init__(self, cls):
        self.cls = cls          # class name
        self.fields = {}
        self.partial = False    # a constructor/mutator was aborted: unset fields are Unknown
        self.hot = False
        self.origins = None
        self.enum_name = None
        self.enum_ordinal = None
        self.ctor_args = None
        self.tag = None

    def __repr__(self):
        if self.enum_name is not None:
            return "<%s.%s>" % (self.cls.rsplit("/", 1)[-1], self.enum_name)
        return "<%s@%x>" % (self.cls.rsplit("/", 1)[-1], id(self) & 0xFFFFFF)


class LibObj:
    """Opaque instance of a library class that is not modelled."""
    __slots__ = ("cls", "args")

    def __init__(self, cls, args=None):
        self.cls = cls
        self.args = args

    def __repr__(self):
        return "<lib %s>" % self.cls


class Arr:
    __slots__ = ("desc", "items", "hot", "origins")

    def __init__(self, desc, items):
        self.desc = desc        # element descriptor
        self.items = items
        self.hot = False
        self.origins = None

    def __repr__(self):
        return "Arr<%s x%d>" % (self.desc, len(self.items))


class ClassRef:
    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return isinstance(other, ClassRef) and other.name == self.name

    def __hash__(self):
        return hash(("ClassRef", self.name))

    def __repr__(self):
        return "class:" + self.name


class Lambda:
    """A lambda / method reference created by invokedynamic."""
    __slots__ = ("iface", "sam", "kind", "owner", "name", "desc", "captured", "hot", "site")

    def __init__(self, iface, sam, kind, owner, name, desc, captured, site):
        self.iface = iface
        self.sam = sam
        self.kind = kind
        self.owner = owner
        self.name = name
        self.desc = desc
        self.captured = captured
        self.hot = False
        self.site = site

    def __repr__(self):
        return "<lambda %s -> %s.%s>" % (self.iface.rsplit("/", 1)[-1],
                                           self.owner.rsplit("/", 1)[-1], self.name)


class Thunk:
    """A deferred, value-returning call (or static field read)."""
    __slots__ = ("fn", "desc", "label", "state", "value")

    def __init__(self, fn, desc, label):
        self.fn = fn
        self.desc = desc
        self.label = label
        self.state = 0      # 0 = pending, 1 = running, 2 = done
        self.value = None

    def __repr__(self):
        return "<thunk %s%s>" % (self.label, "" if self.state != 2 else " = %r" % (self.value,))


class PyFn:
    """A tool-side callback standing in for a functional-interface object.

    Lets the tool hand a visitor to vanilla's own enumeration code (e.g.
    ``ProtocolInfo.Details.listPackets``).  ``fn(ev, method_name, desc, args)``.
    """
    __slots__ = ("fn",)

    def __init__(self, fn):
        self.fn = fn


class Ident:
    """net.minecraft.resources.Identifier (modelled natively)."""
    __slots__ = ("ns", "path")

    def __init__(self, ns, path):
        self.ns = ns
        self.path = path

    def __eq__(self, other):
        return isinstance(other, Ident) and other.ns == self.ns and other.path == self.path

    def __hash__(self):
        return hash((self.ns, self.path))

    def __str__(self):
        return "%s:%s" % (self.ns, self.path)

    __repr__ = __str__


class RKey:
    """net.minecraft.resources.ResourceKey (modelled natively, interned by value)."""
    __slots__ = ("registry", "ident")

    def __init__(self, registry, ident):
        self.registry = registry
        self.ident = ident

    def __eq__(self, other):
        return isinstance(other, RKey) and other.registry == self.registry and other.ident == self.ident

    def __hash__(self):
        return hash((self.registry, self.ident))

    def __repr__(self):
        return "ResourceKey[%s / %s]" % (self.registry, self.ident)


def is_wide(v):
    if isinstance(v, (JLong, JDouble)):
        return True
    if isinstance(v, (Unknown, Thunk)):
        return v.desc == "J" or v.desc == "D"
    return False


def default_value(desc):
    c = desc[0]
    if c in "ZBCSI":
        return 0
    if c == "J":
        return JLong(0)
    if c == "F":
        return 0.0
    if c == "D":
        return JDouble(0.0)
    return None


def i32(x):
    x &= 0xFFFFFFFF
    return x - 0x100000000 if x & 0x80000000 else x


def i64(x):
    x &= 0xFFFFFFFFFFFFFFFF
    return x - 0x10000000000000000 if x & 0x8000000000000000 else x


def f32(x):
    try:
        return struct.unpack(">f", struct.pack(">f", x))[0]
    except OverflowError:
        return float("inf") if x > 0 else float("-inf")


def java_string_hash(s):
    """java.lang.String.hashCode()"""
    h = 0
    data = s.encode("utf-16-be")
    for i in range(0, len(data), 2):
        h = (31 * h + ((data[i] << 8) | data[i + 1])) & 0xFFFFFFFF
    return i32(h)


def f32_repr(x):
    """Shortest decimal string that round-trips to the same float32 (Java Float.toString style)."""
    if x != x:
        return "NaN"
    if x in (float("inf"), float("-inf")):
        return "Infinity" if x > 0 else "-Infinity"
    for prec in range(1, 12):
        s = "%.*g" % (prec, x)
        if f32(float(s)) == x:
            break
    f = float(s)
    if f == int(f) and abs(f) < 1e7:
        return "%.1f" % f
    return repr(f)


# --------------------------------------------------------------------------
# frame
# --------------------------------------------------------------------------
class Frame:
    __slots__ = ("m", "c", "locals", "stack", "pc", "hard")

    def __init__(self, m, nlocals, hard):
        self.m = m
        self.c = m.cls
        self.locals = [None] * nlocals
        self.stack = []
        self.pc = 0
        self.hard = hard


class MethodInfo:
    __slots__ = ("targets", "leaf", "fields", "index", "succ", "ipdom")

    def __init__(self):
        self.targets = []
        self.leaf = False
        self.fields = frozenset()
        self.index = None
        self.succ = None
        self.ipdom = None


_RETURNS = frozenset(["ireturn", "lreturn", "freturn", "dreturn", "areturn", "return"])
_INVOKES = frozenset(["invokevirtual", "invokespecial", "invokestatic", "invokeinterface"])

HARD_CAP = 400000000
SOFT_BUDGET = 3000000


class Evaluator:
    def __init__(self, cp, lazy_init=False):
        self.cp = cp
        self.statics = {}
        self.class_state = {}       # name -> 'running' | 'done' | 'partial' | 'failed'
        self.sinks = {}             # (name, desc) -> (owners or None, handler)
        self.always_run = {}        # (owner, name) -> unit label function
        self.hot_fields = set()
        self.hot_version = 0
        self._dpos = set()
        self._dneg = {}
        self._minfo = {}
        self._cha = {}
        self._subs = None
        self.unit_stack = ["<top>"]
        self.failures = []          # (unit, message, trace)
        self.warnings = []          # (unit, message)
        self.env_queries = []       # system properties the analysed code asked for
        self.soft_failures = {}     # message -> count
        self.insn_count = 0
        self.limit = HARD_CAP
        self.frames = []
        self.intrinsics = None      # set by intrinsics.install()
        self.native_jar_classes = {}  # owner -> handler(ev, kind, name, desc, args)
        self.class_init_log = []    # order in which classes were initialised
        self.eager_init = not lazy_init
        import intrinsics
        intrinsics.install(self)

    # ------------------------------------------------------------------ misc
    def trace(self, n=14):
        out = []
        for f in self.frames[-n:]:
            out.append("%s.%s%s @%d" % (f.m.cls.name, f.m.name, f.m.desc, f.pc))
        return out

    def abort(self, msg):
        raise Abort(msg, self.trace())

    def note_soft(self, e):
        key = e.msg if len(e.msg) < 160 else e.msg[:160]
        self.soft_failures[key] = self.soft_failures.get(key, 0) + 1

    def record_failure(self, e):
        if not e.recorded:
            e.recorded = True
            self.failures.append((self.unit_stack[-1], e.msg, e.trace))

    # ------------------------------------------------------------ hierarchy
    def subclasses(self, name):
        if self._subs is None:
            subs = {}
            for n in sorted(self.cp.names()):
                c = self.cp.get(n)
                if c.super_name:
                    subs.setdefault(c.super_name, []).append(n)
                for i in c.interfaces:
                    subs.setdefault(i, []).append(n)
            self._subs = subs
        return self._subs.get(name, ())

    def all_subclasses(self, name):
        out = []
        seen = set()
        stack = [name]
        while stack:
            n = stack.pop()
            for s in self.subclasses(n):
                if s not in seen:
                    seen.add(s)
                    out.append(s)
                    stack.append(s)
        return out

    def cha_targets(self, owner, name, desc):
        key = (owner, name, desc)
        r = self._cha.get(key)
        if r is None:
            r = []
            seen = set()
            m = self.cp.find_method(owner, name, desc)
            if m is not None and m.code is not None:
                r.append(m)
                seen.add(m.key)
            for s in self.all_subclasses(owner):
                c = self.cp.get(s)
                mm = c.method(name, desc)
                if mm is not None and mm.code is not None and mm.key not in seen:
                    seen.add(mm.key)
                    r.append(mm)
            self._cha[key] = r
        return r

    def resolve_virtual(self, cls_name, name, desc):
        """Method selected for a receiver of runtime class ``cls_name`` (or None)."""
        cn = cls_name
        ifaces = []
        while cn is not None:
            c = self.cp.get(cn)
            if c is None:
                break
            m = c.method(name, desc)
            if m is not None and m.code is not None:
                return m
            ifaces.extend(c.interfaces)
            cn = c.super_name
        seen = set()
        while ifaces:
            i = ifaces.pop(0)
            if i in seen:
                continue
            seen.add(i)
            c = self.cp.get(i)
            if c is None:
                continue
            m = c.method(name, desc)
            if m is not None and m.code is not None:
                return m
            ifaces.extend(c.interfaces)
        return None

    def instance_of(self, value, cname):
        """Java ``instanceof`` for a concrete value; returns True/False or None (unknown)."""
        if value is None:
            return False
        if isinstance(value, Obj):
            if self.cp.is_subclass(value.cls, cname):
                return True
            if self.cp.has(cname) or cname == "java/lang/Object":
                return cname == "java/lang/Object"
            # library type: decide by walking the object's library ancestors
            return self._lib_ancestor(value.cls, cname)
        if isinstance(value, str):
            return cname in ("java/lang/String", "java/lang/CharSequence", "java/lang/Object",
                             "java/lang/Comparable")
        if isinstance(value, Lambda):
            if value.iface == cname or cname == "java/lang/Object":
                return True
            if self.cp.has(value.iface) and self.cp.is_subclass(value.iface, cname):
                return True
            if self.cp.has(cname):
                return False
            return None
        if isinstance(value, Ident):
            return cname in ("net/minecraft/resources/Identifier", "java/lang/Object", "java/lang/Comparable")
        if isinstance(value, RKey):
            return cname in ("net/minecraft/resources/ResourceKey", "java/lang/Object")
        return None

    def _lib_ancestor(self, cls_name, target):
        seen = set()
        stack = [cls_name]
        unknown = False
        while stack:
            n = stack.pop()
            if n in seen or n is None:
                continue
            seen.add(n)
            if n == target:
                return True
            c = self.cp.get(n)
            if c is None:
                if n != "java/lang/Object" and n != "java/lang/Record" and n != "java/lang/Enum":
                    unknown = True
                continue
            stack.append(c.super_name)
            stack.extend(c.interfaces)
        return None if unknown else False

    # ------------------------------------------------------ static analysis
    def minfo(self, m):
        info = self._minfo.get(m.key)
        if info is not None:
            return info
        info = MethodInfo()
        self._minfo[m.key] = info
        c = m.cls
        targets = {}
        fields = set()
        leaf = False
        insns = m.insns()
        for ins in insns:
            op = ins.op
            if op in _INVOKES:
                _, owner, name, desc = c.member_ref(ins.arg)
                sk = self.sinks.get((name, desc))
                if sk is not None and (sk[0] is None or owner in sk[0]):
                    leaf = True
                    continue
                if (owner, name) in self.always_run:
                    leaf = True
                    continue
                if not self.cp.has(owner):
                    continue
                if op == "invokestatic" or op == "invokespecial":
                    t = self.cp.find_method(owner, name, desc)
                    if t is not None and t.code is not None:
                        targets[t.key] = t
                else:
                    for t in self.cha_targets(owner, name, desc):
                        targets[t.key] = t
            elif op == "invokedynamic":
                bidx, name, desc = c.invoke_dynamic(ins.arg)
                ref, args = c.bootstrap_methods[bidx]
                bk = c.method_handle(ref)
                if bk[1] == "java/lang/invoke/LambdaMetafactory":
                    kind, owner, hname, hdesc, _ = c.method_handle(args[1])
                    if self.cp.has(owner):
                        if kind in (cf.REF_invokeStatic, cf.REF_invokeSpecial, cf.REF_newInvokeSpecial):
                            t = self.cp.find_method(owner, hname, hdesc)
                            if t is not None and t.code is not None:
                                targets[t.key] = t
                        else:
                            for t in self.cha_targets(owner, hname, hdesc):
                                targets[t.key] = t
            elif op == "getstatic" or op == "getfield":
                _, owner, name, desc = c.member_ref(ins.arg)
                if self.cp.has(owner):
                    fields.add((owner, name))
        info.targets = list(targets.values())
        info.leaf = leaf
        info.fields = frozenset(fields)
        return info

    def in_D(self, m):
        """Can ``m`` reach a sink through the static call graph?"""
        k = m.key
        if k in self._dpos:
            return True
        ver = self.hot_version
        if self._dneg.get(k) == ver:
            return False
        seen = {k}
        stack = [m]
        visited = []
        found = False
        hot_fields = self.hot_fields
        dneg = self._dneg
        dpos = self._dpos
        while stack:
            x = stack.pop()
            info = self.minfo(x)
            if info.leaf or x.key in dpos or (hot_fields and not hot_fields.isdisjoint(info.fields)):
                found = True
                break
            visited.append(x.key)
            for t in info.targets:
                tk = t.key
                if tk not in seen and dneg.get(tk) != ver:
                    seen.add(tk)
                    stack.append(t)
        if found:
            dpos.add(k)
            return True
        for vk in visited:
            dneg[vk] = ver
        return False

    def cha_in_D(self, owner, name, desc):
        for t in self.cha_targets(owner, name, desc):
            if self.in_D(t):
                return True
        return False

    def is_hot(self, v):
        return getattr(v, "hot", False)

    def mark_hot_container(self, cont):
        if not cont.hot:
            cont.hot = True
            if cont.origins:
                for o in cont.origins:
                    if o not in self.hot_fields:
                        self.hot_fields.add(o)
                        self.hot_version += 1

    def note_origin(self, v, key):
        if isinstance(v, (Obj, Arr)) or hasattr(v, "origins"):
            try:
                if v.origins is None:
                    v.origins = set()
                v.origins.add(key)
            except AttributeError:
                return
            if v.hot and key not in self.hot_fields:
                self.hot_fields.add(key)
                self.hot_version += 1

    def lambda_hot(self, lam):
        for cv in lam.captured:
            if self.is_hot(cv):
                return True
        if not self.cp.has(lam.owner):
            return False
        if lam.kind in (cf.REF_invokeStatic, cf.REF_invokeSpecial, cf.REF_newInvokeSpecial):
            t = self.cp.find_method(lam.owner, lam.name, lam.desc)
            return t is not None and t.code is not None and self.in_D(t)
        return self.cha_in_D(lam.owner, lam.name, lam.desc)

    # --------------------------------------------------------------- forcing
    def force(self, v):
        """Evaluate thunks until a non-thunk value is reached."""
        while isinstance(v, Thunk):
            if v.state == 2:
                v = v.value
                continue
            if v.state == 1:
                self.abort("cyclic thunk " + v.label)
            v.state = 1
            saved_limit = self.limit
            self.limit = min(saved_limit, self.insn_count + SOFT_BUDGET)
            try:
                try:
                    r = v.fn()
                except Abort as e:
                    self.note_soft(e)
                    r = Unknown(v.desc, "thunk %s failed: %s" % (v.label, e.msg))
            finally:
                self.limit = saved_limit
            v.fn = None
            v.value = r
            v.state = 2
            v = r
        return v

    def resolve(self, v):
        """Follow already-evaluated thunks without forcing anything."""
        while isinstance(v, Thunk) and v.state == 2:
            v = v.value
        return v

    def same_ref(self, a, b):
        a = self.force(a)
        b = self.force(b)
        if isinstance(a, Unknown) or isinstance(b, Unknown):
            return None
        if a is b:
            return True
        if isinstance(a, RKey) and isinstance(b, RKey):
            return a == b
        if isinstance(a, str) and isinstance(b, str):
            return a == b
        if isinstance(a, ClassRef) and isinstance(b, ClassRef):
            return a == b
        return False

    # ------------------------------------------------------------ class init
    def field_decl(self, owner, name):
        c = self.cp.find_field_owner(owner, name)
        return c.name if c is not None else None

    def clinit_in_D(self, cname):
        c = self.cp.get(cname)
        if c is None:
            return False
        m = c.method("<clinit>", "()V")
        return m is not None and self.in_D(m)

    def ensure_init(self, cname):
        """Active use of ``cname`` (static access, static call, ``new``).

        Default: initialise the class now, as the JVM does.  With ``lazy_init``
        only the initialisers that can register something run here; the others
        wait until one of their static fields is read.
        """
        if cname in self.class_state:
            return
        c = self.cp.get(cname)
        if c is None:
            return
        if self.eager_init:
            self.run_clinit(cname)
            return
        chain = []
        n = cname
        while n is not None and self.cp.has(n) and n not in self.class_state:
            chain.append(n)
            n = self.cp.get(n).super_name
        for n in reversed(chain):
            if n not in self.class_state and self.clinit_in_D(n):
                self.run_clinit(n)

    def force_init(self, cname):
        if cname in self.class_state:
            return
        if self.cp.get(cname) is None:
            return
        self.run_clinit(cname)

    def run_clinit(self, cname):
        c = self.cp.get(cname)
        self.class_state[cname] = "running"
        if c.super_name and self.cp.has(c.super_name):
            self.force_init(c.super_name)
        for f in c.fields:
            if f.is_static and f.const_value_index is not None:
                kind, val = c.loadable(f.const_value_index)
                if kind == "long":
                    val = JLong(val)
                elif kind == "double":
                    val = JDouble(val)
                self.statics[(cname, f.name)] = val
        m = c.method("<clinit>", "()V")
        self.class_init_log.append(cname)
        if m is None:
            self.class_state[cname] = "done"
            return
        hard = self.in_D(m)
        saved_limit = self.limit
        if hard:
            self.unit_stack.append("clinit:" + cname)
            self.limit = HARD_CAP
            try:
                self.exec(m, [], True)
                self.class_state[cname] = "done"
            except Abort as e:
                self.record_failure(e)
                self.class_state[cname] = "failed"
            finally:
                self.unit_stack.pop()
                self.limit = saved_limit
        else:
            self.limit = min(saved_limit, self.insn_count + SOFT_BUDGET)
            try:
                self.exec(m, [])
                self.class_state[cname] = "done"
            except Abort as e:
                self.note_soft(e)
                self.class_state[cname] = "partial"
            finally:
                self.limit = saved_limit

    def read_static(self, decl, name, desc):
        key = (decl, name)
        if key in self.statics:
            return self.statics[key]
        if self.class_state.get(decl) in ("partial", "failed"):
            return Unknown(desc, "static %s.%s of partially initialised class" % (decl, name))
        return default_value(desc)

    def getstatic(self, owner, name, desc):
        decl = self.field_decl(owner, name)
        if decl is None:
            return self.intrinsics.lib_getstatic(self, owner, name, desc)
        self.ensure_init(decl)
        if decl not in self.class_state:
            def fn(decl=decl, name=name, desc=desc):
                self.force_init(decl)
                v = self.read_static(decl, name, desc)
                self.note_origin(self.resolve(v), (decl, name))
                return v
            return Thunk(fn, desc, "static %s.%s" % (decl.rsplit("/", 1)[-1], name))
        v = self.read_static(decl, name, desc)
        self.note_origin(self.resolve(v), (decl, name))
        return v

    def putstatic(self, owner, name, desc, value):
        decl = self.field_decl(owner, name)
        if decl is None:
            return
        self.ensure_init(decl)
        if decl not in self.class_state:
            self.force_init(decl)
        self.statics[(decl, name)] = value
        rv = self.resolve(value)
        if self.is_hot(rv):
            if (decl, name) not in self.hot_fields:
                self.hot_fields.add((decl, name))
                self.hot_version += 1
        self.note_origin(rv, (decl, name))

    def static_value(self, cname, name):
        """Tool-side accessor: fully initialise ``cname`` and return a forced static."""
        self.ensure_init(cname)
        self.force_init(cname)
        c = self.cp.get(cname)
        f = c.field(name)
        return self.force(self.read_static(cname, name, f.desc if f else "Ljava/lang/Object;"))

    # ---------------------------------------------------------------- fields
    def getfield(self, obj, owner, name, desc):
        obj = self.force(obj)
        if obj is None:
            self.abort("null receiver for getfield %s.%s" % (owner, name))
        if isinstance(obj, Obj):
            if name in obj.fields:
                v = obj.fields[name]
            elif obj.partial:
                return Unknown(desc, "field %s of partially constructed %s" % (name, obj.cls))
            else:
                v = default_value(desc)
            decl = self.field_decl(owner, name) or owner
            self.note_origin(self.resolve(v), (decl, name))
            return v
        if isinstance(obj, Unknown):
            return Unknown(desc, "field %s of unknown" % name)
        h = self.native_jar_classes.get(owner)
        if h is not None:
            return h(self, "getfield", name, desc, [obj])
        return Unknown(desc, "field %s.%s of %r" % (owner, name, obj))

    def putfield(self, obj, owner, name, desc, value):
        obj = self.force(obj)
        if obj is None:
            self.abort("null receiver for putfield %s.%s" % (owner, name))
        if isinstance(obj, Obj):
            obj.fields[name] = value
            rv = self.resolve(value)
            if self.is_hot(rv):
                decl = self.field_decl(owner, name) or owner
                if (decl, name) not in self.hot_fields:
                    self.hot_fields.add((decl, name))
                    self.hot_version += 1
                self.mark_hot_container(obj)
            return
        # unknown receiver: nothing we can record

    # ----------------------------------------------------------------- calls
    def call_jar(self, m, args):
        """Call a method of the analysed jar.

        Calls that can register something (or receive a lambda that can) are
        "hard": a failure inside them is fatal for the current unit of work.
        Everything else is evaluated right away as well -- in program order, so
        that side effects on shared objects are seen in the order the JVM would
        produce them -- but tolerantly: a failure only makes the result Unknown.
        """
        if m.code is None:
            self.abort("call to abstract/native method %r" % (m,))
        hard = self.in_D(m)
        if not hard:
            for a in args:
                if self.is_hot(a):
                    hard = True
                    break
        if hard:
            return self.exec(m, args, True)
        return self.run_soft(m, args)

    def run_soft(self, m, args):
        """Evaluate a call that cannot register anything, tolerating failure."""
        saved_limit = self.limit
        self.limit = min(saved_limit, self.insn_count + SOFT_BUDGET)
        try:
            return self.exec(m, args, False)
        except Abort as e:
            self.note_soft(e)
            self.taint_args(args)
            ret = cf.method_desc(m.desc)[1]
            if ret == "V":
                return VOID
            return Unknown(ret, "%s.%s failed: %s" % (m.cls.name.rsplit("/", 1)[-1], m.name, e.msg))
        finally:
            self.limit = saved_limit

    def taint_args(self, args):
        """A call was abandoned half way: objects it may have been mutating are suspect."""
        for a in args:
            r = self.resolve(a)
            if isinstance(r, Obj):
                r.partial = True
            elif hasattr(r, "tainted"):
                r.tainted = True

    def invoke_virtual_now(self, recv, name, desc, args):
        """Dispatch on a concrete receiver. ``args`` excludes the receiver."""
        recv = self.force(recv)
        if recv is None:
            self.abort("null receiver for %s%s" % (name, desc))
        if isinstance(recv, Obj):
            m = self.resolve_virtual(recv.cls, name, desc)
            if m is not None:
                return self.call_jar(m, [recv] + list(args))
            return self.intrinsics.obj_method(self, recv, name, desc, list(args))
        if isinstance(recv, Lambda):
            return self.invoke_lambda(recv, name, desc, list(args))
        if isinstance(recv, Unknown):
            return self.unknown_result(desc, "call %s on unknown receiver" % name, list(args))
        if isinstance(recv, PyFn):
            return recv.fn(self, name, desc, list(args))
        return self.intrinsics.native_method(self, recv, None, name, desc, list(args))

    def unknown_result(self, desc, why, args=()):
        for a in args:
            if self.is_hot(self.resolve(a)):
                self.abort("registering lambda escapes into unmodelled code (%s)" % why)
        ret = cf.method_desc(desc)[1]
        if ret == "V":
            return VOID
        return Unknown(ret, why)

    def invoke_lambda(self, lam, name, desc, args):
        if name != lam.sam:
            return self.intrinsics.lambda_default_method(self, lam, name, desc, args)
        allargs = list(lam.captured) + list(args)
        kind = lam.kind
        if not self.cp.has(lam.owner):
            return self.intrinsics.lib_method_ref(self, lam, allargs)
        nh = self.native_jar_classes.get(lam.owner)
        if nh is not None:
            k = {cf.REF_invokeStatic: "invokestatic", cf.REF_newInvokeSpecial: "new"}.get(kind, "invokevirtual")
            return nh(self, k, lam.name, lam.desc, allargs)
        if kind == cf.REF_invokeStatic:
            m = self.cp.find_method(lam.owner, lam.name, lam.desc)
            if m is None:
                self.abort("lambda impl not found %s.%s" % (lam.owner, lam.name))
            self.ensure_init(m.cls.name)
            return self.call_jar(m, allargs)
        if kind == cf.REF_newInvokeSpecial:
            return self.new_object(lam.owner, lam.desc, allargs)
        if kind == cf.REF_invokeSpecial:
            m = self.cp.find_method(lam.owner, lam.name, lam.desc)
            if m is None:
                self.abort("lambda impl not found %s.%s" % (lam.owner, lam.name))
            return self.call_jar(m, allargs)
        # virtual / interface method reference
        recv = allargs[0]
        rest = allargs[1:]
        return self.invoke_virtual(lam.owner, lam.name, lam.desc, recv, rest)

    def invoke_virtual(self, owner, name, desc, recv, args):
        """invokevirtual/invokeinterface on a jar-declared method."""
        r = self.force(recv)
        if isinstance(r, Unknown):
            if self.cha_in_D(owner, name, desc):
                self.abort("unknown receiver for possibly-registering call %s.%s (%s)" % (owner, name, r.why))
            return self.unknown_result(desc, "call %s.%s on unknown receiver" % (owner, name), args)
        return self.invoke_virtual_now(r, name, desc, args)

    def new_object(self, cname, ctor_desc, args):
        """``new cname(args)`` (used for constructor references and by intrinsics)."""
        if not self.cp.has(cname):
            return self.intrinsics.lib_new(self, cname, ctor_desc, args)
        self.ensure_init(cname)
        o = Obj(cname)
        self.construct(o, cname, ctor_desc, args, fresh=True)
        return o

    def construct(self, obj, cname, desc, args, fresh):
        """Run constructor ``cname.<init>desc`` on ``obj``."""
        c = self.cp.get(cname)
        m = c.method("<init>", desc)
        if m is None:
            self.abort("constructor not found %s.<init>%s" % (cname, desc))
        if fresh:
            obj.ctor_args = list(args)
        allargs = [obj] + list(args)
        hard = self.in_D(m)
        if not hard:
            for a in args:
                if self.is_hot(a):
                    hard = True
                    break
        if hard:
            self.exec(m, allargs, True)
        else:
            self.run_soft(m, allargs)

    # ---------------------------------------------------------- interpreter
    def method_index(self, m):
        info = self.minfo(m)
        if info.index is None:
            info.index = {ins.pc: i for i, ins in enumerate(m.insns())}
        return info.index

    def exec(self, m, args, hard=False):
        code = m.code
        if code is None:
            self.abort("no code for %r" % (m,))
        frame = Frame(m, code.max_locals + 2, hard)
        i = 0
        params = cf.method_desc(m.desc)[0]
        loc = frame.locals
        ai = 0
        if not m.is_static:
            loc[0] = args[0]
            i = 1
            ai = 1
        for p in params:
            loc[i] = args[ai]
            ai += 1
            i += 2 if (p == "J" or p == "D") else 1
        self.frames.append(frame)
        if len(self.frames) > 2500:
            self.frames.pop()
            self.abort("call depth exceeded")
        try:
            return self.run(frame)
        except Abort as e:
            if e.trace is None:
                e.trace = self.trace()
            raise
        except RecursionError:
            raise Abort("python recursion limit", self.trace())
        finally:
            self.frames.pop()

    def run(self, frame):
        m = frame.m
        c = frame.c
        insns = m.insns()
        index = self.method_index(m)
        stack = frame.stack
        loc = frame.locals
        push = stack.append
        pop = stack.pop
        i = 0
        force = self.force
        n_insns = len(insns)
        while True:
            if i >= n_insns:
                self.abort("fell off the end of %r" % (m,))
            ins = insns[i]
            i += 1
            op = ins.op
            frame.pc = ins.pc
            self.insn_count += 1
            if self.insn_count > self.limit:
                self.abort("instruction budget exceeded")

            # ---- loads / stores / constants
            if op == "aload" or op == "iload" or op == "fload" or op == "lload" or op == "dload":
                push(loc[ins.arg])
            elif op == "astore" or op == "istore" or op == "fstore" or op == "lstore" or op == "dstore":
                loc[ins.arg] = pop()
            elif op == "getstatic":
                _, owner, name, desc = c.member_ref(ins.arg)
                push(self.getstatic(owner, name, desc))
            elif op == "putstatic":
                _, owner, name, desc = c.member_ref(ins.arg)
                self.putstatic(owner, name, desc, pop())
            elif op == "ldc":
                kind, val = c.loadable(ins.arg)
                if kind == "string" or kind == "int" or kind == "float":
                    push(val)
                elif kind == "long":
                    push(JLong(val))
                elif kind == "double":
                    push(JDouble(val))
                elif kind == "class":
                    push(ClassRef(val))
                else:
                    push(Unknown("Ljava/lang/Object;", "ldc " + kind))
            elif op == "iconst":
                push(ins.arg)
            elif op == "aconst_null":
                push(None)
            elif op == "fconst":
                push(ins.arg)
            elif op == "lconst":
                push(JLong(ins.arg))
            elif op == "dconst":
                push(JDouble(ins.arg))
            elif op == "invokestatic" or op == "invokevirtual" or op == "invokeinterface" \
                    or op == "invokespecial":
                r = self.do_invoke(op, c, ins.arg, stack, frame)
                if r is not VOID:
                    push(r)
            elif op == "invokedynamic":
                self.do_invokedynamic(c, ins, stack, frame)
            elif op == "getfield":
                _, owner, name, desc = c.member_ref(ins.arg)
                push(self.getfield(pop(), owner, name, desc))
            elif op == "putfield":
                _, owner, name, desc = c.member_ref(ins.arg)
                v = pop()
                self.putfield(pop(), owner, name, desc, v)
            elif op == "new":
                cname = c.class_name(ins.arg)
                if self.cp.has(cname):
                    self.ensure_init(cname)
                    push(Obj(cname))
                else:
                    push(self.intrinsics.lib_alloc(self, cname))
            elif op == "dup":
                push(stack[-1])
            elif op == "pop":
                pop()
            elif op == "areturn" or op == "ireturn" or op == "freturn" or op == "lreturn" \
                    or op == "dreturn":
                return pop()
            elif op == "return":
                return VOID
            elif op == "checkcast":
                pass
            elif op == "goto":
                i = index[ins.arg]
            elif op in _IF1:
                v = force(pop())
                if isinstance(v, Unknown):
                    i = self.unknown_branch(frame, ins, i, index)
                    continue
                if isinstance(v, bool):
                    v = int(v)
                if not isinstance(v, int):
                    self.abort("%s on non-int %r" % (op, v))
                if _IF1[op](v):
                    i = index[ins.arg]
            elif op in _IF2:
                b = force(pop())
                a = force(pop())
                if isinstance(a, Unknown) or isinstance(b, Unknown):
                    i = self.unknown_branch(frame, ins, i, index)
                    continue
                if _IF2[op](a, b):
                    i = index[ins.arg]
            elif op == "ifnull" or op == "ifnonnull":
                v = force(pop())
                if isinstance(v, Unknown):
                    i = self.unknown_branch(frame, ins, i, index)
                    continue
                if (v is None) == (op == "ifnull"):
                    i = index[ins.arg]
            elif op == "if_acmpeq" or op == "if_acmpne":
                b = pop()
                a = pop()
                same = self.same_ref(a, b)
                if same is None:
                    i = self.unknown_branch(frame, ins, i, index)
                    continue
                if same == (op == "if_acmpeq"):
                    i = index[ins.arg]
            elif op == "anewarray" or op == "newarray":
                n = force(pop())
                if not isinstance(n, int):
                    self.abort("array length unknown")
                if op == "anewarray":
                    push(Arr("L" + c.class_name(ins.arg) + ";", [None] * n))
                else:
                    d = _ATYPE[ins.arg]
                    push(Arr(d, [default_value(d)] * n))
            elif op == "aastore" or op == "iastore" or op == "bastore" or op == "castore" \
                    or op == "sastore" or op == "fastore" or op == "lastore" or op == "dastore":
                v = pop()
                idx = force(pop())
                arr = force(pop())
                if isinstance(arr, Arr) and isinstance(idx, int):
                    if idx < 0 or idx >= len(arr.items):
                        self.abort("array index out of bounds")
                    arr.items[idx] = v
                    if self.is_hot(self.resolve(v)):
                        self.mark_hot_container(arr)
                elif arr is None:
                    self.abort("null array store")
                elif isinstance(arr, Arr):
                    self.abort("array store with unknown index")
            elif op == "aaload" or op == "iaload" or op == "baload" or op == "caload" \
                    or op == "saload" or op == "faload" or op == "laload" or op == "daload":
                idx = force(pop())
                arr = force(pop())
                if isinstance(arr, Arr) and isinstance(idx, int):
                    if idx < 0 or idx >= len(arr.items):
                        self.abort("array index out of bounds")
                    push(arr.items[idx])
                elif arr is None:
                    self.abort("null array load")
                else:
                    push(Unknown(_ALOAD_DESC[op], "array load from unknown"))
            elif op == "arraylength":
                arr = force(pop())
                if isinstance(arr, Arr):
                    push(len(arr.items))
                elif arr is None:
                    self.abort("arraylength of null")
                else:
                    push(Unknown("I", "arraylength of unknown"))
            elif op == "iinc":
                idx, delta = ins.arg
                v = force(loc[idx])
                loc[idx] = i32(v + delta) if isinstance(v, int) else Unknown("I", "iinc unknown")
            elif op == "instanceof":
                v = force(pop())
                if isinstance(v, Unknown):
                    push(Unknown("I", "instanceof unknown"))
                else:
                    r = self.instance_of(v, c.class_name(ins.arg))
                    if r is None:
                        r = self.intrinsics.native_instanceof(self, v, c.class_name(ins.arg))
                    push(Unknown("I", "instanceof undecidable") if r is None else int(r))
            elif op == "athrow":
                self.abort("athrow reached in %s.%s" % (c.name, m.name))
            elif op == "dup_x1":
                a = pop()
                b = pop()
                push(a)
                push(b)
                push(a)
            elif op == "dup_x2":
                a = pop()
                b = pop()
                if is_wide(b):
                    push(a)
                    push(b)
                    push(a)
                else:
                    cc = pop()
                    push(a)
                    push(cc)
                    push(b)
                    push(a)
            elif op == "dup2":
                a = stack[-1]
                if is_wide(a):
                    push(a)
                else:
                    b = stack[-2]
                    push(b)
                    push(a)
            elif op == "dup2_x1":
                a = pop()
                if is_wide(a):
                    b = pop()
                    push(a)
                    push(b)
                    push(a)
                else:
                    b = pop()
                    cc = pop()
                    push(b)
                    push(a)
                    push(cc)
                    push(b)
                    push(a)
            elif op == "dup2_x2":
                self.abort("dup2_x2 not supported")
            elif op == "pop2":
                a = pop()
                if not is_wide(a):
                    pop()
            elif op == "swap":
                a = pop()
                b = pop()
                push(a)
                push(b)
            elif op == "tableswitch":
                v = force(pop())
                if not isinstance(v, int):
                    self.abort("tableswitch on unknown value")
                default, low, targets = ins.arg
                if low <= v < low + len(targets):
                    i = index[targets[v - low]]
                else:
                    i = index[default]
            elif op == "lookupswitch":
                v = force(pop())
                if not isinstance(v, int):
                    self.abort("lookupswitch on unknown value")
                default, table = ins.arg
                i = index[table.get(v, default)]
            elif op == "multianewarray":
                self.abort("multianewarray not supported")
            elif op == "monitorenter" or op == "monitorexit":
                pop()
            elif op == "nop":
                pass
            elif op in _ARITH:
                _ARITH[op](self, stack)
            else:
                self.abort("unsupported opcode " + op)

    # ----------------------------------------------------- unknown branches
    def cfg(self, m):
        info = self.minfo(m)
        if info.succ is None:
            insns = m.insns()
            index = self.method_index(m)
            succ = []
            for k, ins in enumerate(insns):
                op = ins.op
                if op == "goto":
                    succ.append((index[ins.arg],))
                elif op in _IF1 or op in _IF2 or op in ("ifnull", "ifnonnull", "if_acmpeq", "if_acmpne"):
                    succ.append((k + 1, index[ins.arg]))
                elif op == "tableswitch":
                    succ.append(tuple(sorted(set([index[ins.arg[0]]] + [index[t] for t in ins.arg[2]]))))
                elif op == "lookupswitch":
                    succ.append(tuple(sorted(set([index[ins.arg[0]]] + [index[t] for t in ins.arg[1].values()]))))
                elif op in _RETURNS or op == "athrow":
                    succ.append(())
                else:
                    succ.append((k + 1,))
            info.succ = succ
        return info.succ

    def relevant_insn(self, c, ins):
        """Could skipping this instruction lose a registration?"""
        op = ins.op
        if op in _INVOKES:
            _, owner, name, desc = c.member_ref(ins.arg)
            sk = self.sinks.get((name, desc))
            if sk is not None and (sk[0] is None or owner in sk[0]):
                return True
            if (owner, name) in self.always_run:
                return True
            if not self.cp.has(owner):
                return False
            if op in ("invokestatic", "invokespecial"):
                t = self.cp.find_method(owner, name, desc)
                if t is not None and t.code is not None and self.in_D(t):
                    return True
                if op == "invokestatic" and t is not None and t.cls.name not in self.class_state \
                        and self.clinit_in_D(t.cls.name):
                    return True
                return False
            return self.cha_in_D(owner, name, desc)
        if op == "invokedynamic":
            bidx, name, desc = c.invoke_dynamic(ins.arg)
            ref, args = c.bootstrap_methods[bidx]
            if c.method_handle(ref)[1] != "java/lang/invoke/LambdaMetafactory":
                return False
            kind, owner, hname, hdesc, _ = c.method_handle(args[1])
            if not self.cp.has(owner):
                return False
            if kind in (cf.REF_invokeStatic, cf.REF_invokeSpecial, cf.REF_newInvokeSpecial):
                t = self.cp.find_method(owner, hname, hdesc)
                return t is not None and t.code is not None and self.in_D(t)
            return self.cha_in_D(owner, hname, hdesc)
        if op in ("getstatic", "putstatic"):
            _, owner, name, desc = c.member_ref(ins.arg)
            decl = self.field_decl(owner, name)
            if decl is not None and decl not in self.class_state and self.clinit_in_D(decl):
                return True
            if op == "getstatic" and decl is not None and (decl, name) in self.hot_fields:
                return True
            return False
        if op == "new":
            cname = c.class_name(ins.arg)
            return self.cp.has(cname) and cname not in self.class_state and self.clinit_in_D(cname)
        return False

    def leads_to_throw(self, m, k):
        """Does straight-line code starting at instruction index k end in athrow?"""
        insns = m.insns()
        index = self.method_index(m)
        c = m.cls
        steps = 0
        while k < len(insns) and steps < 400:
            ins = insns[k]
            op = ins.op
            if op == "athrow":
                return True
            if op == "goto":
                k = index[ins.arg]
                steps += 1
                continue
            if op in _RETURNS or op in _IF1 or op in _IF2 or op in (
                    "ifnull", "ifnonnull", "if_acmpeq", "if_acmpne", "tableswitch", "lookupswitch"):
                return False
            if self.relevant_insn(c, ins):
                return False
            k += 1
            steps += 1
        return False

    def unknown_branch(self, frame, ins, nxt, index):
        """A two-way branch whose condition is Unknown.  Returns the instruction index to continue at."""
        m = frame.m
        target = index[ins.arg]
        t_throw = self.leads_to_throw(m, target)
        f_throw = self.leads_to_throw(m, nxt)
        # rule 1: vanilla bootstrap completes without throwing
        if t_throw and not f_throw:
            return nxt
        if f_throw and not t_throw:
            return target
        # rule 2: skip a side-effect free region up to the join point
        j = self.skip_region(frame, nxt, target, ins.pc)
        if j is not None:
            return j
        self.abort("branch on unknown value in %s.%s%s @%d" % (m.cls.name, m.name, m.desc, ins.pc))

    def skip_region(self, frame, a, b, pc):
        """Continue at the join point of an undecidable branch, if nothing in between matters.

        In tolerant frames the skipped region must be free of side effects, so
        that values computed afterwards stay trustworthy.  In hard frames (code
        that performs registrations) it is enough that the region cannot
        register anything: its effects on the heap are *not* modelled, locals
        and static fields it assigns become Unknown, and a warning is recorded.
        """
        strict = not frame.hard
        side_effects = False
        m = frame.m
        c = m.cls
        insns = m.insns()
        succ = self.cfg(m)
        # forward reachability sets from both successors
        def reach(start):
            seen = []
            mark = set()
            stack = [start]
            while stack:
                k = stack.pop()
                if k in mark:
                    continue
                mark.add(k)
                seen.append(k)
                stack.extend(succ[k])
            return mark
        ra = reach(a)
        rb = reach(b)
        common = ra & rb
        if not common:
            return None
        # the join point: first common instruction (smallest index reached by both
        # such that neither path reaches it only through the other)
        j = min(common)
        # region = instructions reachable from a or b before j
        region = set()
        for start in (a, b):
            stack = [start]
            while stack:
                k = stack.pop()
                if k == j or k in region:
                    continue
                region.add(k)
                stack.extend(succ[k])
        if len(region) > 400:
            return None
        havoc = []
        havoc_statics = []
        for k in region:
            x = insns[k]
            op = x.op
            if not succ[k]:
                return None                      # a path leaves the method
            for s in succ[k]:
                if s != j and s not in region:
                    return None
            if op in ("putfield", "putstatic", "aastore", "iastore", "bastore", "castore",
                      "sastore", "fastore", "lastore", "dastore", "monitorenter"):
                if strict:
                    return None
                side_effects = True
                if op == "putstatic":
                    if self.relevant_insn(c, x):
                        return None
                    _, owner, name, desc = c.member_ref(x.arg)
                    havoc_statics.append((owner, name, desc))
            if op in _INVOKES:
                _, owner, name, desc = c.member_ref(x.arg)
                if self.cp.has(owner):
                    if strict:
                        return None
                    side_effects = True
                if self.relevant_insn(c, x):
                    return None
            elif op == "invokedynamic":
                if self.relevant_insn(c, x):
                    return None
            elif op in ("getstatic", "new"):
                if self.relevant_insn(c, x):
                    return None
            elif op in ("astore", "istore", "fstore", "lstore", "dstore"):
                havoc.append((x.arg, {"astore": "Ljava/lang/Object;", "istore": "I", "fstore": "F",
                                      "lstore": "J", "dstore": "D"}[op]))
            elif op == "iinc":
                havoc.append((x.arg[0], "I"))
        # the operand stack must be balanced over the region: only accept the
        # statement-level shape (javac leaves the stack as it was at the branch)
        if not self._region_stack_neutral(c, insns, succ, a, j) or \
                not self._region_stack_neutral(c, insns, succ, b, j):
            return None
        for idx, d in havoc:
            frame.locals[idx] = Unknown(d, "assigned in skipped region")
        for owner, name, desc in havoc_statics:
            decl = self.field_decl(owner, name)
            if decl is not None:
                self.statics[(decl, name)] = Unknown(desc, "assigned in skipped region")
        if side_effects:
            self.warnings.append((self.unit_stack[-1],
                                  "skipped a region with unmodelled side effects (undecidable branch) "
                                  "in %s.%s%s @%d" % (m.cls.name, m.name, m.desc, pc)))
        return j

    def _region_stack_neutral(self, c, insns, succ, start, j):
        depth = 0
        k = start
        steps = 0
        while k != j and steps < 500:
            x = insns[k]
            d = _stack_delta(c, x)
            if d is None:
                return False
            depth += d
            if depth < 0:
                return False
            s = succ[k]
            if not s:
                return False
            k = s[0] if len(s) == 1 else max(s)
            steps += 1
        return k == j and depth == 0

    # ------------------------------------------------------------- invokes
    def do_invoke(self, op, c, cpidx, stack, frame):
        _, owner, name, desc = c.member_ref(cpidx)
        params, ret = cf.method_desc(desc)
        n = len(params)
        if n:
            args = stack[-n:]
            del stack[-n:]
        else:
            args = []
        static = op == "invokestatic"
        recv = None if static else stack.pop()

        sk = self.sinks.get((name, desc))
        if sk is not None and (sk[0] is None or owner in sk[0]):
            return sk[1](self, recv, args)

        if (owner, name) in self.always_run:
            return self.always_run[(owner, name)](self, recv, name, desc, args)

        nh = self.native_jar_classes.get(owner)
        if nh is not None:
            if static:
                return nh(self, op, name, desc, args)
            r0 = self.force(recv)
            if not isinstance(r0, Obj):
                return nh(self, op, name, desc, [r0] + args)
            recv = r0

        if not self.cp.has(owner):
            # ---- library method
            if static:
                return self.intrinsics.lib_static(self, owner, name, desc, args)
            if name == "<init>":
                return self.intrinsics.lib_init(self, owner, desc, recv, args, frame)
            r = self.force(recv)
            if r is None:
                self.abort("null receiver for %s.%s" % (owner, name))
            if isinstance(r, Lambda):
                return self.invoke_lambda(r, name, desc, args)
            if isinstance(r, Obj):
                m = self.resolve_virtual(r.cls, name, desc)
                if m is not None:
                    return self.call_jar(m, [r] + args)
                return self.intrinsics.obj_method(self, r, name, desc, args)
            if isinstance(r, Unknown):
                return self.unknown_result(desc, "%s.%s on %s" % (owner.rsplit("/", 1)[-1], name, r.why or "unknown"), args)
            if isinstance(r, PyFn):
                return r.fn(self, name, desc, args)
            return self.intrinsics.native_method(self, r, owner, name, desc, args)

        # ---- method of the analysed jar
        if static:
            m = self.cp.find_method(owner, name, desc)
            if m is None:
                self.abort("unresolved static method %s.%s%s" % (owner, name, desc))
            self.ensure_init(m.cls.name)
            return self.call_jar(m, args)
        if op == "invokespecial":
            if name == "<init>":
                r = self.resolve(recv)
                if not isinstance(r, Obj):
                    self.abort("constructor call on non-object %r" % (r,))
                fresh = r.ctor_args is None and r.cls == owner
                self.construct(r, owner, desc, args, fresh)
                return VOID
            m = self.cp.find_method(owner, name, desc)
            if m is None:
                self.abort("unresolved special method %s.%s%s" % (owner, name, desc))
            return self.call_jar(m, [recv] + args)
        return self.invoke_virtual(owner, name, desc, recv, args)

    def do_invokedynamic(self, c, ins, stack, frame):
        bidx, name, desc = c.invoke_dynamic(ins.arg)
        ref, bargs = c.bootstrap_methods[bidx]
        _, bowner, bname, _, _ = c.method_handle(ref)
        params, ret = cf.method_desc(desc)
        n = len(params)
        if n:
            args = stack[-n:]
            del stack[-n:]
        else:
            args = []
        if bowner == "java/lang/invoke/LambdaMetafactory":
            kind, owner, hname, hdesc, _ = c.method_handle(bargs[1])
            lam = Lambda(ret[1:-1], name, kind, owner, hname, hdesc, args,
                         (frame.m.cls.name, frame.m.name, ins.pc))
            lam.hot = self.lambda_hot(lam)
            stack.append(lam)
            return
        if bowner == "java/lang/invoke/StringConcatFactory":
            kind, recipe = c.loadable(bargs[0])
            consts = [c.loadable(a)[1] for a in bargs[1:]]
            out = []
            ai = 0
            ci = 0
            unknown = False
            for ch in recipe:
                if ch == "\x01":
                    s = self.intrinsics.java_str(self, args[ai], params[ai])
                    ai += 1
                    if isinstance(s, Unknown):
                        unknown = True
                    else:
                        out.append(s)
                elif ch == "\x02":
                    out.append(str(consts[ci]))
                    ci += 1
                else:
                    out.append(ch)
            stack.append(Unknown("Ljava/lang/String;", "string concat with unknown part") if unknown
                         else "".join(out))
            return
        r = self.intrinsics.other_indy(self, c, bowner, bname, bargs, name, desc, args)
        if r is not VOID:
            stack.append(r)


# --------------------------------------------------------------------------
# opcode tables
# --------------------------------------------------------------------------
_IF1 = {
    "ifeq": lambda v: v == 0, "ifne": lambda v: v != 0, "iflt": lambda v: v < 0,
    "ifge": lambda v: v >= 0, "ifgt": lambda v: v > 0, "ifle": lambda v: v <= 0,
}
_IF2 = {
    "if_icmpeq": lambda a, b: a == b, "if_icmpne": lambda a, b: a != b,
    "if_icmplt": lambda a, b: a < b, "if_icmpge": lambda a, b: a >= b,
    "if_icmpgt": lambda a, b: a > b, "if_icmple": lambda a, b: a <= b,
}
_ATYPE = {4: "Z", 5: "C", 6: "F", 7: "D", 8: "B", 9: "S", 10: "I", 11: "J"}
_ALOAD_DESC = {"aaload": "Ljava/lang/Object;", "iaload": "I", "baload": "I", "caload": "I",
               "saload": "I", "faload": "F", "laload": "J", "daload": "D"}


def _binop(desc, fn):
    def h(ev, stack):
        b = ev.force(stack.pop())
        a = ev.force(stack.pop())
        if isinstance(a, Unknown) or isinstance(b, Unknown) or a is None or b is None:
            stack.append(Unknown(desc, "arithmetic on unknown"))
            return
        try:
            stack.append(fn(a, b))
        except ZeroDivisionError:
            ev.abort("division by zero")
    return h


def _unop(desc, fn):
    def h(ev, stack):
        a = ev.force(stack.pop())
        if isinstance(a, Unknown) or a is None:
            stack.append(Unknown(desc, "arithmetic on unknown"))
            return
        stack.append(fn(a))
    return h


def _idiv(a, b):
    q = abs(a) // abs(b)
    return -q if (a < 0) != (b < 0) else q


def _irem(a, b):
    return a - _idiv(a, b) * b


def _cmp(a, b):
    return (a > b) - (a < b)


def _fcmp(nan):
    def f(a, b):
        if a != a or b != b:
            return nan
        return (a > b) - (a < b)
    return f


def _f2i(bits):
    lo = -(1 << (bits - 1))
    hi = (1 << (bits - 1)) - 1

    def f(a):
        if a != a:
            return 0
        if a >= hi:
            return hi
        if a <= lo:
            return lo
        return int(a)
    return f


_ARITH = {
    "iadd": _binop("I", lambda a, b: i32(a + b)), "isub": _binop("I", lambda a, b: i32(a - b)),
    "imul": _binop("I", lambda a, b: i32(a * b)), "idiv": _binop("I", lambda a, b: i32(_idiv(a, b))),
    "irem": _binop("I", lambda a, b: i32(_irem(a, b))),
    "iand": _binop("I", lambda a, b: i32(a & b)), "ior": _binop("I", lambda a, b: i32(a | b)),
    "ixor": _binop("I", lambda a, b: i32(a ^ b)),
    "ishl": _binop("I", lambda a, b: i32(a << (b & 31))),
    "ishr": _binop("I", lambda a, b: i32(a >> (b & 31))),
    "iushr": _binop("I", lambda a, b: i32((a & 0xFFFFFFFF) >> (b & 31))),
    "ineg": _unop("I", lambda a: i32(-a)),
    "ladd": _binop("J", lambda a, b: JLong(i64(a + b))), "lsub": _binop("J", lambda a, b: JLong(i64(a - b))),
    "lmul": _binop("J", lambda a, b: JLong(i64(a * b))),
    "ldiv": _binop("J", lambda a, b: JLong(i64(_idiv(a, b)))),
    "lrem": _binop("J", lambda a, b: JLong(i64(_irem(a, b)))),
    "land": _binop("J", lambda a, b: JLong(i64(a & b))), "lor": _binop("J", lambda a, b: JLong(i64(a | b))),
    "lxor": _binop("J", lambda a, b: JLong(i64(a ^ b))),
    "lshl": _binop("J", lambda a, b: JLong(i64(a << (b & 63)))),
    "lshr": _binop("J", lambda a, b: JLong(i64(a >> (b & 63)))),
    "lushr": _binop("J", lambda a, b: JLong(i64((a & 0xFFFFFFFFFFFFFFFF) >> (b & 63)))),
    "lneg": _unop("J", lambda a: JLong(i64(-a))),
    "fadd": _binop("F", lambda a, b: f32(a + b)), "fsub": _binop("F", lambda a, b: f32(a - b)),
    "fmul": _binop("F", lambda a, b: f32(a * b)), "fdiv": _binop("F", lambda a, b: f32(a / b) if b else float("nan")),
    "frem": _binop("F", lambda a, b: f32(a - b * int(a / b)) if b else float("nan")),
    "fneg": _unop("F", lambda a: -a),
    "dadd": _binop("D", lambda a, b: JDouble(a + b)), "dsub": _binop("D", lambda a, b: JDouble(a - b)),
    "dmul": _binop("D", lambda a, b: JDouble(a * b)),
    "ddiv": _binop("D", lambda a, b: JDouble(a / b) if b else JDouble(float("nan"))),
    "drem": _binop("D", lambda a, b: JDouble(a - b * int(a / b)) if b else JDouble(float("nan"))),
    "dneg": _unop("D", lambda a: JDouble(-a)),
    "i2l": _unop("J", lambda a: JLong(a)), "i2f": _unop("F", lambda a: f32(float(a))),
    "i2d": _unop("D", lambda a: JDouble(float(a))), "l2i": _unop("I", lambda a: i32(a)),
    "l2f": _unop("F", lambda a: f32(float(a))), "l2d": _unop("D", lambda a: JDouble(float(a))),
    "f2i": _unop("I", _f2i(32)), "f2l": _unop("J", lambda a: JLong(_f2i(64)(a))),
    "f2d": _unop("D", lambda a: JDouble(a)), "d2i": _unop("I", _f2i(32)),
    "d2l": _unop("J", lambda a: JLong(_f2i(64)(a))), "d2f": _unop("F", lambda a: f32(a)),
    "i2b": _unop("I", lambda a: ((a + 128) & 0xFF) - 128), "i2c": _unop("I", lambda a: a & 0xFFFF),
    "i2s": _unop("I", lambda a: ((a + 32768) & 0xFFFF) - 32768),
    "lcmp": _binop("I", _cmp), "fcmpl": _binop("I", _fcmp(-1)), "fcmpg": _binop("I", _fcmp(1)),
    "dcmpl": _binop("I", _fcmp(-1)), "dcmpg": _binop("I", _fcmp(1)),
}

_SIMPLE_DELTA = {
    "nop": 0, "aconst_null": 1, "iconst": 1, "fconst": 1, "lconst": 2, "dconst": 2,
    "iload": 1, "fload": 1, "aload": 1, "lload": 2, "dload": 2,
    "istore": -1, "fstore": -1, "astore": -1, "lstore": -2, "dstore": -2,
    "pop": -1, "pop2": -2, "dup": 1, "dup_x1": 1, "dup_x2": 1, "dup2": 2, "dup2_x1": 2,
    "dup2_x2": 2, "swap": 0, "iinc": 0, "checkcast": 0, "instanceof": 0, "arraylength": 0,
    "goto": 0, "new": 1, "newarray": 0, "anewarray": 0, "athrow": -1,
    "ifeq": -1, "ifne": -1, "iflt": -1, "ifge": -1, "ifgt": -1, "ifle": -1,
    "ifnull": -1, "ifnonnull": -1,
    "if_icmpeq": -2, "if_icmpne": -2, "if_icmplt": -2, "if_icmpge": -2, "if_icmpgt": -2,
    "if_icmple": -2, "if_acmpeq": -2, "if_acmpne": -2,
    "iaload": -1, "faload": -1, "aaload": -1, "baload": -1, "caload": -1, "saload": -1,
    "laload": 0, "daload": 0,
    "iadd": -1, "isub": -1, "imul": -1, "idiv": -1, "irem": -1, "iand": -1, "ior": -1,
    "ixor": -1, "ishl": -1, "ishr": -1, "iushr": -1, "ineg": 0,
    "fadd": -1, "fsub": -1, "fmul": -1, "fdiv": -1, "frem": -1, "fneg": 0,
    "i2f": 0, "f2i": 0, "i2b": 0, "i2c": 0, "i2s": 0, "i2l": 1, "i2d": 1, "f2l": 1, "f2d": 1,
    "l2i": -1, "d2i": -1, "l2f": -1, "d2f": -1, "l2d": 0, "d2l": 0,
    "fcmpl": -1, "fcmpg": -1, "lcmp": -3, "dcmpl": -3, "dcmpg": -3,
}


def _slots(desc):
    return 2 if desc in ("J", "D") else (0 if desc == "V" else 1)


def _stack_delta(c, ins):
    """Net operand-stack effect in slots, or None when not modelled."""
    op = ins.op
    d = _SIMPLE_DELTA.get(op)
    if d is not None:
        return d
    if op == "ldc":
        kind, _ = c.loadable(ins.arg)
        return 2 if kind in ("long", "double") else 1
    if op in ("getstatic", "getfield", "putstatic", "putfield"):
        _, _, _, desc = c.member_ref(ins.arg)
        s = _slots(desc)
        return {"getstatic": s, "getfield": s - 1, "putstatic": -s, "putfield": -s - 1}[op]
    if op in _INVOKES:
        _, _, _, desc = c.member_ref(ins.arg)
        params, ret = cf.method_desc(desc)
        return _slots(ret) - sum(_slots(p) for p in params) - (0 if op == "invokestatic" else 1)
    if op == "invokedynamic":
        _, _, desc = c.invoke_dynamic(ins.arg)
        params, ret = cf.method_desc(desc)
        return _slots(ret) - sum(_slots(p) for p in params)
    return None
