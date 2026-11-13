"""Minimal, read-only JVM class-file parser and bytecode disassembler.

Pure standard library.  Nothing from the parsed classes is ever executed or
loaded into a JVM; this module only decodes the binary class-file structures
(JVMS chapter 4) so that the rest of the tool can analyse the bytecode.

Only the parts needed for the data extraction are decoded: the constant pool,
fields, methods, the ``Code`` attribute (with its exception table), and the
class-level ``BootstrapMethods`` attribute (needed to resolve
``invokedynamic`` call sites, i.e. lambdas and string concatenation).
"""

import struct

# --- constant pool tags -----------------------------------------------------
CP_UTF8 = 1
CP_INTEGER = 3
CP_FLOAT = 4
CP_LONG = 5
CP_DOUBLE = 6
CP_CLASS = 7
CP_STRING = 8
CP_FIELDREF = 9
CP_METHODREF = 10
CP_IMETHODREF = 11
CP_NAMEANDTYPE = 12
CP_METHODHANDLE = 15
CP_METHODTYPE = 16
CP_DYNAMIC = 17
CP_INVOKEDYNAMIC = 18
CP_MODULE = 19
CP_PACKAGE = 20

ACC_PUBLIC = 0x0001
ACC_PRIVATE = 0x0002
ACC_STATIC = 0x0008
ACC_FINAL = 0x0010
ACC_SYNTHETIC = 0x1000
ACC_INTERFACE = 0x0200
ACC_ABSTRACT = 0x0400
ACC_NATIVE = 0x0100
ACC_ENUM = 0x4000
ACC_VARARGS = 0x0080

# Method handle reference kinds (JVMS 4.4.8)
REF_getField = 1
REF_getStatic = 2
REF_putField = 3
REF_putStatic = 4
REF_invokeVirtual = 5
REF_invokeStatic = 6
REF_invokeSpecial = 7
REF_newInvokeSpecial = 8
REF_invokeInterface = 9


class ClassFormatError(Exception):
    pass


def _decode_mutf8(raw):
    """Decode the JVM's "modified UTF-8"."""
    try:
        s = raw.decode("utf-8")
        if "\x00" not in s:
            return s
    except UnicodeDecodeError:
        pass
    out = []
    i = 0
    n = len(raw)
    while i < n:
        b = raw[i]
        if b < 0x80:
            out.append(b)
            i += 1
        elif (b & 0xE0) == 0xC0:
            out.append(((b & 0x1F) << 6) | (raw[i + 1] & 0x3F))
            i += 2
        elif (b & 0xF0) == 0xE0:
            out.append(((b & 0x0F) << 12) | ((raw[i + 1] & 0x3F) << 6) | (raw[i + 2] & 0x3F))
            i += 3
        else:
            raise ClassFormatError("bad modified UTF-8")
    # surrogate pairs -> code points
    chars = []
    j = 0
    while j < len(out):
        c = out[j]
        if 0xD800 <= c <= 0xDBFF and j + 1 < len(out) and 0xDC00 <= out[j + 1] <= 0xDFFF:
            chars.append(chr(0x10000 + ((c - 0xD800) << 10) + (out[j + 1] - 0xDC00)))
            j += 2
        else:
            chars.append(chr(c))
            j += 1
    return "".join(chars)


class Field:
    __slots__ = ("access", "name", "desc", "const_value_index")

    def __init__(self, access, name, desc, const_value_index):
        self.access = access
        self.name = name
        self.desc = desc
        self.const_value_index = const_value_index

    @property
    def is_static(self):
        return bool(self.access & ACC_STATIC)


class Method:
    __slots__ = ("cls", "access", "name", "desc", "code", "_insns")

    def __init__(self, cls, access, name, desc, code):
        self.cls = cls
        self.access = access
        self.name = name
        self.desc = desc
        self.code = code  # Code or None (abstract / native)
        self._insns = None

    @property
    def is_static(self):
        return bool(self.access & ACC_STATIC)

    @property
    def is_abstract(self):
        return bool(self.access & ACC_ABSTRACT)

    @property
    def key(self):
        return (self.cls.name, self.name, self.desc)

    def insns(self):
        """Decoded instruction list (cached)."""
        if self._insns is None:
            self._insns = disassemble(self.code.code) if self.code else []
        return self._insns

    def __repr__(self):
        return "%s.%s%s" % (self.cls.name, self.name, self.desc)


class Code:
    __slots__ = ("max_stack", "max_locals", "code", "exception_table", "_index")

    def __init__(self, max_stack, max_locals, code, exception_table):
        self.max_stack = max_stack
        self.max_locals = max_locals
        self.code = code
        self.exception_table = exception_table  # list of (start, end, handler, catch_type_index)
        self._index = None


class ClassFile:
    """A parsed class file."""

    def __init__(self, data):
        self.data = data
        self._parse(data)

    # -- parsing -------------------------------------------------------------
    def _parse(self, data):
        if data[:4] != b"\xca\xfe\xba\xbe":
            raise ClassFormatError("not a class file")
        self.minor, self.major = struct.unpack_from(">HH", data, 4)
        (count,) = struct.unpack_from(">H", data, 8)
        pos = 10
        cp = [None] * count
        i = 1
        while i < count:
            tag = data[pos]
            if tag == CP_UTF8:
                (ln,) = struct.unpack_from(">H", data, pos + 1)
                cp[i] = (tag, _decode_mutf8(data[pos + 3:pos + 3 + ln]))
                pos += 3 + ln
            elif tag == CP_INTEGER:
                cp[i] = (tag, struct.unpack_from(">i", data, pos + 1)[0])
                pos += 5
            elif tag == CP_FLOAT:
                cp[i] = (tag, struct.unpack_from(">f", data, pos + 1)[0])
                pos += 5
            elif tag == CP_LONG:
                cp[i] = (tag, struct.unpack_from(">q", data, pos + 1)[0])
                pos += 9
                i += 1
            elif tag == CP_DOUBLE:
                cp[i] = (tag, struct.unpack_from(">d", data, pos + 1)[0])
                pos += 9
                i += 1
            elif tag in (CP_CLASS, CP_STRING, CP_METHODTYPE, CP_MODULE, CP_PACKAGE):
                cp[i] = (tag, struct.unpack_from(">H", data, pos + 1)[0])
                pos += 3
            elif tag in (CP_FIELDREF, CP_METHODREF, CP_IMETHODREF, CP_NAMEANDTYPE,
                         CP_DYNAMIC, CP_INVOKEDYNAMIC):
                cp[i] = (tag,) + struct.unpack_from(">HH", data, pos + 1)
                pos += 5
            elif tag == CP_METHODHANDLE:
                cp[i] = (tag,) + struct.unpack_from(">BH", data, pos + 1)
                pos += 4
            else:
                raise ClassFormatError("unknown constant pool tag %d" % tag)
            i += 1
        self.cp = cp
        self.access, this_idx, super_idx, icount = struct.unpack_from(">HHHH", data, pos)
        pos += 8
        self.name = self.class_name(this_idx)
        self.super_name = self.class_name(super_idx) if super_idx else None
        self.interfaces = []
        for _ in range(icount):
            (idx,) = struct.unpack_from(">H", data, pos)
            pos += 2
            self.interfaces.append(self.class_name(idx))
        # fields
        (fcount,) = struct.unpack_from(">H", data, pos)
        pos += 2
        self.fields = []
        for _ in range(fcount):
            access, name_idx, desc_idx, acount = struct.unpack_from(">HHHH", data, pos)
            pos += 8
            const_idx = None
            for _a in range(acount):
                aname_idx, alen = struct.unpack_from(">HI", data, pos)
                pos += 6
                if self.utf8(aname_idx) == "ConstantValue":
                    (const_idx,) = struct.unpack_from(">H", data, pos)
                pos += alen
            self.fields.append(Field(access, self.utf8(name_idx), self.utf8(desc_idx), const_idx))
        # methods
        (mcount,) = struct.unpack_from(">H", data, pos)
        pos += 2
        self.methods = []
        for _ in range(mcount):
            access, name_idx, desc_idx, acount = struct.unpack_from(">HHHH", data, pos)
            pos += 8
            code = None
            for _a in range(acount):
                aname_idx, alen = struct.unpack_from(">HI", data, pos)
                pos += 6
                if self.utf8(aname_idx) == "Code":
                    code = self._parse_code(data, pos)
                pos += alen
            self.methods.append(Method(self, access, self.utf8(name_idx), self.utf8(desc_idx), code))
        # class attributes
        (acount,) = struct.unpack_from(">H", data, pos)
        pos += 2
        self.bootstrap_methods = []
        self.source_file = None
        self.record_components = None
        for _ in range(acount):
            aname_idx, alen = struct.unpack_from(">HI", data, pos)
            pos += 6
            aname = self.utf8(aname_idx)
            if aname == "BootstrapMethods":
                p = pos
                (n,) = struct.unpack_from(">H", data, p)
                p += 2
                for _b in range(n):
                    ref, nargs = struct.unpack_from(">HH", data, p)
                    p += 4
                    args = list(struct.unpack_from(">%dH" % nargs, data, p))
                    p += 2 * nargs
                    self.bootstrap_methods.append((ref, args))
            elif aname == "SourceFile":
                self.source_file = self.utf8(struct.unpack_from(">H", data, pos)[0])
            elif aname == "Record":
                p = pos
                (n,) = struct.unpack_from(">H", data, p)
                p += 2
                comps = []
                for _c in range(n):
                    nidx, didx, ac = struct.unpack_from(">HHH", data, p)
                    p += 6
                    for _x in range(ac):
                        _an, al = struct.unpack_from(">HI", data, p)
                        p += 6 + al
                    comps.append((self.utf8(nidx), self.utf8(didx)))
                self.record_components = comps
            pos += alen
        self._method_index = {}
        for m in self.methods:
            self._method_index[(m.name, m.desc)] = m
        self._field_index = {}
        for f in self.fields:
            self._field_index[f.name] = f

    def _parse_code(self, data, pos):
        max_stack, max_locals, code_len = struct.unpack_from(">HHI", data, pos)
        pos += 8
        code = data[pos:pos + code_len]
        pos += code_len
        (ecount,) = struct.unpack_from(">H", data, pos)
        pos += 2
        table = []
        for _ in range(ecount):
            table.append(struct.unpack_from(">HHHH", data, pos))
            pos += 8
        return Code(max_stack, max_locals, code, table)

    # -- constant pool helpers -------------------------------------------------
    def utf8(self, idx):
        e = self.cp[idx]
        if e is None or e[0] != CP_UTF8:
            raise ClassFormatError("cp[%d] is not Utf8" % idx)
        return e[1]

    def class_name(self, idx):
        e = self.cp[idx]
        if e is None or e[0] != CP_CLASS:
            raise ClassFormatError("cp[%d] is not Class" % idx)
        return self.utf8(e[1])

    def name_and_type(self, idx):
        e = self.cp[idx]
        return self.utf8(e[1]), self.utf8(e[2])

    def member_ref(self, idx):
        """-> (tag, owner, name, descriptor) for Fieldref/Methodref/InterfaceMethodref."""
        e = self.cp[idx]
        if e is None or e[0] not in (CP_FIELDREF, CP_METHODREF, CP_IMETHODREF):
            raise ClassFormatError("cp[%d] is not a member ref" % idx)
        name, desc = self.name_and_type(e[2])
        return e[0], self.class_name(e[1]), name, desc

    def method_handle(self, idx):
        """-> (kind, owner, name, descriptor, is_interface)"""
        e = self.cp[idx]
        if e is None or e[0] != CP_METHODHANDLE:
            raise ClassFormatError("cp[%d] is not a MethodHandle" % idx)
        tag, owner, name, desc = self.member_ref(e[2])
        return e[1], owner, name, desc, tag == CP_IMETHODREF

    def invoke_dynamic(self, idx):
        """-> (bootstrap_index, name, descriptor)"""
        e = self.cp[idx]
        if e is None or e[0] not in (CP_INVOKEDYNAMIC, CP_DYNAMIC):
            raise ClassFormatError("cp[%d] is not InvokeDynamic" % idx)
        name, desc = self.name_and_type(e[2])
        return e[1], name, desc

    def loadable(self, idx):
        """Describe a loadable constant (for ldc): (kind, value)."""
        e = self.cp[idx]
        tag = e[0]
        if tag == CP_STRING:
            return "string", self.utf8(e[1])
        if tag == CP_INTEGER:
            return "int", e[1]
        if tag == CP_FLOAT:
            return "float", e[1]
        if tag == CP_LONG:
            return "long", e[1]
        if tag == CP_DOUBLE:
            return "double", e[1]
        if tag == CP_CLASS:
            return "class", self.utf8(e[1])
        if tag == CP_METHODTYPE:
            return "methodtype", self.utf8(e[1])
        if tag == CP_METHODHANDLE:
            return "methodhandle", self.method_handle(idx)
        if tag == CP_DYNAMIC:
            return "condy", self.invoke_dynamic(idx)
        raise ClassFormatError("cp[%d] (tag %d) is not loadable" % (idx, tag))

    # -- lookups ---------------------------------------------------------------
    def method(self, name, desc):
        return self._method_index.get((name, desc))

    def methods_named(self, name):
        return [m for m in self.methods if m.name == name]

    def field(self, name):
        return self._field_index.get(name)

    @property
    def is_interface(self):
        return bool(self.access & ACC_INTERFACE)

    @property
    def is_enum(self):
        return bool(self.access & ACC_ENUM)


# --- bytecode ---------------------------------------------------------------
# operand kinds:
#   ''     none
#   'b'    signed byte            'B' unsigned byte
#   's'    signed short           'c' u2 constant pool index
#   'l'    u1 local index         'j' s2 branch offset   'J' s4 branch offset
#   'ii'   iinc                   'w' ldc (u1 cp index)
#   'ifc'  invokeinterface        'dyn' invokedynamic
#   'na'   newarray               'mna' multianewarray
#   'ts'   tableswitch            'ls' lookupswitch      'wide'
_OPS = {
    0x00: ("nop", ""), 0x01: ("aconst_null", ""), 0x02: ("iconst_m1", ""),
    0x03: ("iconst_0", ""), 0x04: ("iconst_1", ""), 0x05: ("iconst_2", ""),
    0x06: ("iconst_3", ""), 0x07: ("iconst_4", ""), 0x08: ("iconst_5", ""),
    0x09: ("lconst_0", ""), 0x0A: ("lconst_1", ""), 0x0B: ("fconst_0", ""),
    0x0C: ("fconst_1", ""), 0x0D: ("fconst_2", ""), 0x0E: ("dconst_0", ""),
    0x0F: ("dconst_1", ""), 0x10: ("bipush", "b"), 0x11: ("sipush", "s"),
    0x12: ("ldc", "w"), 0x13: ("ldc_w", "c"), 0x14: ("ldc2_w", "c"),
    0x15: ("iload", "l"), 0x16: ("lload", "l"), 0x17: ("fload", "l"),
    0x18: ("dload", "l"), 0x19: ("aload", "l"),
    0x1A: ("iload_0", ""), 0x1B: ("iload_1", ""), 0x1C: ("iload_2", ""), 0x1D: ("iload_3", ""),
    0x1E: ("lload_0", ""), 0x1F: ("lload_1", ""), 0x20: ("lload_2", ""), 0x21: ("lload_3", ""),
    0x22: ("fload_0", ""), 0x23: ("fload_1", ""), 0x24: ("fload_2", ""), 0x25: ("fload_3", ""),
    0x26: ("dload_0", ""), 0x27: ("dload_1", ""), 0x28: ("dload_2", ""), 0x29: ("dload_3", ""),
    0x2A: ("aload_0", ""), 0x2B: ("aload_1", ""), 0x2C: ("aload_2", ""), 0x2D: ("aload_3", ""),
    0x2E: ("iaload", ""), 0x2F: ("laload", ""), 0x30: ("faload", ""), 0x31: ("daload", ""),
    0x32: ("aaload", ""), 0x33: ("baload", ""), 0x34: ("caload", ""), 0x35: ("saload", ""),
    0x36: ("istore", "l"), 0x37: ("lstore", "l"), 0x38: ("fstore", "l"),
    0x39: ("dstore", "l"), 0x3A: ("astore", "l"),
    0x3B: ("istore_0", ""), 0x3C: ("istore_1", ""), 0x3D: ("istore_2", ""), 0x3E: ("istore_3", ""),
    0x3F: ("lstore_0", ""), 0x40: ("lstore_1", ""), 0x41: ("lstore_2", ""), 0x42: ("lstore_3", ""),
    0x43: ("fstore_0", ""), 0x44: ("fstore_1", ""), 0x45: ("fstore_2", ""), 0x46: ("fstore_3", ""),
    0x47: ("dstore_0", ""), 0x48: ("dstore_1", ""), 0x49: ("dstore_2", ""), 0x4A: ("dstore_3", ""),
    0x4B: ("astore_0", ""), 0x4C: ("astore_1", ""), 0x4D: ("astore_2", ""), 0x4E: ("astore_3", ""),
    0x4F: ("iastore", ""), 0x50: ("lastore", ""), 0x51: ("fastore", ""), 0x52: ("dastore", ""),
    0x53: ("aastore", ""), 0x54: ("bastore", ""), 0x55: ("castore", ""), 0x56: ("sastore", ""),
    0x57: ("pop", ""), 0x58: ("pop2", ""), 0x59: ("dup", ""), 0x5A: ("dup_x1", ""),
    0x5B: ("dup_x2", ""), 0x5C: ("dup2", ""), 0x5D: ("dup2_x1", ""), 0x5E: ("dup2_x2", ""),
    0x5F: ("swap", ""),
    0x60: ("iadd", ""), 0x61: ("ladd", ""), 0x62: ("fadd", ""), 0x63: ("dadd", ""),
    0x64: ("isub", ""), 0x65: ("lsub", ""), 0x66: ("fsub", ""), 0x67: ("dsub", ""),
    0x68: ("imul", ""), 0x69: ("lmul", ""), 0x6A: ("fmul", ""), 0x6B: ("dmul", ""),
    0x6C: ("idiv", ""), 0x6D: ("ldiv", ""), 0x6E: ("fdiv", ""), 0x6F: ("ddiv", ""),
    0x70: ("irem", ""), 0x71: ("lrem", ""), 0x72: ("frem", ""), 0x73: ("drem", ""),
    0x74: ("ineg", ""), 0x75: ("lneg", ""), 0x76: ("fneg", ""), 0x77: ("dneg", ""),
    0x78: ("ishl", ""), 0x79: ("lshl", ""), 0x7A: ("ishr", ""), 0x7B: ("lshr", ""),
    0x7C: ("iushr", ""), 0x7D: ("lushr", ""), 0x7E: ("iand", ""), 0x7F: ("land", ""),
    0x80: ("ior", ""), 0x81: ("lor", ""), 0x82: ("ixor", ""), 0x83: ("lxor", ""),
    0x84: ("iinc", "ii"),
    0x85: ("i2l", ""), 0x86: ("i2f", ""), 0x87: ("i2d", ""), 0x88: ("l2i", ""),
    0x89: ("l2f", ""), 0x8A: ("l2d", ""), 0x8B: ("f2i", ""), 0x8C: ("f2l", ""),
    0x8D: ("f2d", ""), 0x8E: ("d2i", ""), 0x8F: ("d2l", ""), 0x90: ("d2f", ""),
    0x91: ("i2b", ""), 0x92: ("i2c", ""), 0x93: ("i2s", ""),
    0x94: ("lcmp", ""), 0x95: ("fcmpl", ""), 0x96: ("fcmpg", ""), 0x97: ("dcmpl", ""),
    0x98: ("dcmpg", ""),
    0x99: ("ifeq", "j"), 0x9A: ("ifne", "j"), 0x9B: ("iflt", "j"), 0x9C: ("ifge", "j"),
    0x9D: ("ifgt", "j"), 0x9E: ("ifle", "j"),
    0x9F: ("if_icmpeq", "j"), 0xA0: ("if_icmpne", "j"), 0xA1: ("if_icmplt", "j"),
    0xA2: ("if_icmpge", "j"), 0xA3: ("if_icmpgt", "j"), 0xA4: ("if_icmple", "j"),
    0xA5: ("if_acmpeq", "j"), 0xA6: ("if_acmpne", "j"),
    0xA7: ("goto", "j"), 0xA8: ("jsr", "j"), 0xA9: ("ret", "l"),
    0xAA: ("tableswitch", "ts"), 0xAB: ("lookupswitch", "ls"),
    0xAC: ("ireturn", ""), 0xAD: ("lreturn", ""), 0xAE: ("freturn", ""),
    0xAF: ("dreturn", ""), 0xB0: ("areturn", ""), 0xB1: ("return", ""),
    0xB2: ("getstatic", "c"), 0xB3: ("putstatic", "c"), 0xB4: ("getfield", "c"),
    0xB5: ("putfield", "c"), 0xB6: ("invokevirtual", "c"), 0xB7: ("invokespecial", "c"),
    0xB8: ("invokestatic", "c"), 0xB9: ("invokeinterface", "ifc"),
    0xBA: ("invokedynamic", "dyn"), 0xBB: ("new", "c"), 0xBC: ("newarray", "na"),
    0xBD: ("anewarray", "c"), 0xBE: ("arraylength", ""), 0xBF: ("athrow", ""),
    0xC0: ("checkcast", "c"), 0xC1: ("instanceof", "c"), 0xC2: ("monitorenter", ""),
    0xC3: ("monitorexit", ""), 0xC4: ("wide", "wide"), 0xC5: ("multianewarray", "mna"),
    0xC6: ("ifnull", "j"), 0xC7: ("ifnonnull", "j"), 0xC8: ("goto_w", "J"),
    0xC9: ("jsr_w", "J"),
}

BRANCH_OPS = frozenset(
    ["ifeq", "ifne", "iflt", "ifge", "ifgt", "ifle", "if_icmpeq", "if_icmpne", "if_icmplt",
     "if_icmpge", "if_icmpgt", "if_icmple", "if_acmpeq", "if_acmpne", "goto", "jsr",
     "ifnull", "ifnonnull", "goto_w", "jsr_w"])


class Insn:
    """One decoded instruction.

    ``op`` is the mnemonic; ``arg`` depends on the instruction:
      * constant-pool instructions: the cp index
      * local variable instructions: the local index (the ``_N`` short forms are
        normalised, e.g. ``aload_1`` -> op ``aload``, arg 1)
      * branches: the absolute target pc
      * bipush/sipush: the immediate value
      * iinc: (index, delta)
      * tableswitch: (default_pc, low, [target_pcs])
      * lookupswitch: (default_pc, {match: target_pc})
      * newarray: atype; multianewarray: (cp index, dims)
    """
    __slots__ = ("pc", "op", "arg", "next_pc")

    def __init__(self, pc, op, arg, next_pc):
        self.pc = pc
        self.op = op
        self.arg = arg
        self.next_pc = next_pc

    def __repr__(self):
        return "%d: %s %r" % (self.pc, self.op, self.arg)


_SHORT_LOCAL = {}
for _base in ("iload", "lload", "fload", "dload", "aload",
              "istore", "lstore", "fstore", "dstore", "astore"):
    for _n in range(4):
        _SHORT_LOCAL["%s_%d" % (_base, _n)] = (_base, _n)

_CONSTS = {
    "aconst_null": ("aconst_null", None),
    "iconst_m1": ("iconst", -1), "iconst_0": ("iconst", 0), "iconst_1": ("iconst", 1),
    "iconst_2": ("iconst", 2), "iconst_3": ("iconst", 3), "iconst_4": ("iconst", 4),
    "iconst_5": ("iconst", 5), "lconst_0": ("lconst", 0), "lconst_1": ("lconst", 1),
    "fconst_0": ("fconst", 0.0), "fconst_1": ("fconst", 1.0), "fconst_2": ("fconst", 2.0),
    "dconst_0": ("dconst", 0.0), "dconst_1": ("dconst", 1.0),
    "bipush": ("iconst", None), "sipush": ("iconst", None),
}


def disassemble(code):
    """Decode a ``Code`` byte string into a list of :class:`Insn`."""
    out = []
    pc = 0
    n = len(code)
    while pc < n:
        opcode = code[pc]
        try:
            name, kind = _OPS[opcode]
        except KeyError:
            raise ClassFormatError("unknown opcode 0x%02x at pc %d" % (opcode, pc))
        start = pc
        pc += 1
        arg = None
        if kind == "":
            if name in _SHORT_LOCAL:
                name, arg = _SHORT_LOCAL[name]
            elif name in _CONSTS:
                name, arg = _CONSTS[name]
        elif kind == "b":
            arg = struct.unpack_from(">b", code, pc)[0]
            pc += 1
            name = "iconst"
        elif kind == "s":
            arg = struct.unpack_from(">h", code, pc)[0]
            pc += 2
            name = "iconst"
        elif kind == "w":
            arg = code[pc]
            pc += 1
        elif kind == "c":
            arg = struct.unpack_from(">H", code, pc)[0]
            pc += 2
            if name == "ldc_w":
                name = "ldc"
            elif name == "ldc2_w":
                name = "ldc"
        elif kind == "l":
            arg = code[pc]
            pc += 1
        elif kind == "j":
            arg = start + struct.unpack_from(">h", code, pc)[0]
            pc += 2
        elif kind == "J":
            arg = start + struct.unpack_from(">i", code, pc)[0]
            pc += 4
            name = name[:-2]
        elif kind == "ii":
            arg = (code[pc], struct.unpack_from(">b", code, pc + 1)[0])
            pc += 2
        elif kind == "ifc":
            arg = struct.unpack_from(">H", code, pc)[0]
            pc += 4
        elif kind == "dyn":
            arg = struct.unpack_from(">H", code, pc)[0]
            pc += 4
        elif kind == "na":
            arg = code[pc]
            pc += 1
        elif kind == "mna":
            arg = (struct.unpack_from(">H", code, pc)[0], code[pc + 2])
            pc += 3
        elif kind == "ts":
            pc += (4 - pc % 4) % 4
            default, low, high = struct.unpack_from(">iii", code, pc)
            pc += 12
            cnt = high - low + 1
            targets = [start + t for t in struct.unpack_from(">%di" % cnt, code, pc)]
            pc += 4 * cnt
            arg = (start + default, low, targets)
        elif kind == "ls":
            pc += (4 - pc % 4) % 4
            default, npairs = struct.unpack_from(">ii", code, pc)
            pc += 8
            table = {}
            for _ in range(npairs):
                match, off = struct.unpack_from(">ii", code, pc)
                pc += 8
                table[match] = start + off
            arg = (start + default, table)
        elif kind == "wide":
            sub = code[pc]
            subname = _OPS[sub][0]
            pc += 1
            if subname == "iinc":
                arg = struct.unpack_from(">Hh", code, pc)
                pc += 4
            else:
                arg = struct.unpack_from(">H", code, pc)[0]
                pc += 2
            name = subname
        else:  # pragma: no cover
            raise ClassFormatError("unhandled operand kind %r" % kind)
        out.append(Insn(start, name, arg, pc))
    return out


# --- descriptors ------------------------------------------------------------
def parse_method_desc(desc):
    """'(ILjava/lang/String;[J)V' -> (['I', 'Ljava/lang/String;', '[J'], 'V')"""
    assert desc[0] == "("
    i = 1
    params = []
    while desc[i] != ")":
        j = i
        while desc[j] == "[":
            j += 1
        if desc[j] == "L":
            j = desc.index(";", j)
        params.append(desc[i:j + 1])
        i = j + 1
    return params, desc[i + 1:]


_desc_cache = {}


def method_desc(desc):
    r = _desc_cache.get(desc)
    if r is None:
        r = _desc_cache[desc] = parse_method_desc(desc)
    return r


def is_wide(type_desc):
    """True for the two-slot (category 2) types long and double."""
    return type_desc in ("J", "D")


class ClassPath:
    """Lazily parsed classes read straight out of a jar (zip) archive."""

    def __init__(self, zf, prefixes=("net/minecraft/", "com/mojang/")):
        self.zf = zf
        self._names = set()
        for n in zf.namelist():
            if n.endswith(".class") and n.startswith(prefixes):
                self._names.add(n[:-6])
        self._cache = {}

    def has(self, name):
        return name in self._names

    def names(self):
        return self._names

    def get(self, name):
        """Parsed class or None if the class is not in the jar."""
        c = self._cache.get(name)
        if c is None:
            if name not in self._names:
                return None
            c = self._cache[name] = ClassFile(self.zf.read(name + ".class"))
        return c

    def find_method(self, owner, name, desc):
        """Resolve a method by walking superclasses and superinterfaces."""
        seen = set()
        stack = [owner]
        while stack:
            cn = stack.pop(0)
            if cn in seen or cn is None:
                continue
            seen.add(cn)
            c = self.get(cn)
            if c is None:
                continue
            m = c.method(name, desc)
            if m is not None:
                return m
            stack.append(c.super_name)
            stack.extend(c.interfaces)
        return None

    def find_field_owner(self, owner, name):
        seen = set()
        stack = [owner]
        while stack:
            cn = stack.pop(0)
            if cn in seen or cn is None:
                continue
            seen.add(cn)
            c = self.get(cn)
            if c is None:
                continue
            if c.field(name) is not None:
                return c
            stack.append(c.super_name)
            stack.extend(c.interfaces)
        return None

    def is_subclass(self, name, ancestor):
        """True if ``name`` equals or (transitively) extends/implements ``ancestor``."""
        seen = set()
        stack = [name]
        while stack:
            cn = stack.pop()
            if cn is None or cn in seen:
                continue
            if cn == ancestor:
                return True
            seen.add(cn)
            c = self.get(cn)
            if c is None:
                continue
            stack.append(c.super_name)
            stack.extend(c.interfaces)
        return False
