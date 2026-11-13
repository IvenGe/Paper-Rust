#!/usr/bin/env python3
"""Tiny javap-like disassembler built on classfile.py (debugging aid).

    python3 -I dis.py <server.jar> <class/Name> [method-name-substring]

Prints the decoded instructions of the selected methods with resolved
constant-pool references.  Read-only; nothing is executed.
"""
import os
import sys
import zipfile

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import classfile as cf  # noqa: E402


def fmt_insn(c, ins):
    op, arg = ins.op, ins.arg
    if op in ("getstatic", "putstatic", "getfield", "putfield"):
        _, owner, name, desc = c.member_ref(arg)
        return "%-15s %s.%s : %s" % (op, owner, name, desc)
    if op in ("invokevirtual", "invokespecial", "invokestatic", "invokeinterface"):
        _, owner, name, desc = c.member_ref(arg)
        return "%-15s %s.%s%s" % (op, owner, name, desc)
    if op == "invokedynamic":
        bidx, name, desc = c.invoke_dynamic(arg)
        ref, args = c.bootstrap_methods[bidx]
        kind, owner, bname, bdesc, _ = c.method_handle(ref)
        extra = []
        for a in args:
            try:
                k, v = c.loadable(a)
            except cf.ClassFormatError:
                k, v = "?", a
            if k == "methodhandle":
                extra.append("%s.%s%s[kind=%d]" % (v[1], v[2], v[3], v[0]))
            else:
                extra.append("%s:%r" % (k, v))
        return "%-15s %s%s via %s.%s {%s}" % (op, name, desc, owner.split("/")[-1], bname,
                                              ", ".join(extra))
    if op == "ldc":
        k, v = c.loadable(arg)
        return "%-15s %s %r" % (op, k, v)
    if op in ("new", "anewarray", "checkcast", "instanceof"):
        return "%-15s %s" % (op, c.class_name(arg))
    if op == "multianewarray":
        return "%-15s %s dims=%d" % (op, c.class_name(arg[0]), arg[1])
    if arg is None:
        return op
    return "%-15s %r" % (op, arg)


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    zf = zipfile.ZipFile(argv[1])
    cp = cf.ClassPath(zf)
    c = cp.get(argv[2].replace(".", "/"))
    if c is None:
        print("class not found:", argv[2])
        return 1
    want = argv[3] if len(argv) > 3 else None
    print("class %s extends %s implements %s" % (c.name, c.super_name, ", ".join(c.interfaces)))
    if want is None:
        for f in c.fields:
            print("  field %s%s : %s" % ("static " if f.is_static else "", f.name, f.desc))
    for m in c.methods:
        if want is not None and want not in m.name:
            continue
        print("\n  %s%s%s" % ("static " if m.is_static else "", m.name, m.desc))
        if m.code is None:
            continue
        for ins in m.insns():
            print("    %5d: %s" % (ins.pc, fmt_insn(c, ins)))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except BrokenPipeError:
        sys.exit(0)
