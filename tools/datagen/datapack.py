"""Data-driven content read straight from the jar's ``data/`` tree (no code involved):
entries of the synchronized registries, tags, dimension types.

Rules implemented here and where they come from in vanilla:

  * a registry's elements live in ``data/<namespace>/<registry path>/**.json`` and
    its tags in ``data/<namespace>/tags/<registry path>/**.json``
    (``Registries.elementsDirPath`` / ``tagsDirPath``; checked by
    ``verify_dir_paths`` against the bytecode);
  * a tag is built by walking its entries in file order into a ``LinkedHashSet``
    (``TagLoader.tryBuildTag``): ``#tag`` references are expanded in place,
    duplicates keep their first position, an optional entry
    (``{"id": .., "required": false}``) whose target does not exist is skipped,
    a missing *required* target makes the whole tag fail to load.
"""

import json

DATA = "data/"


def verify_dir_paths(cp):
    """-> list of problems if Registries.elementsDirPath/tagsDirPath are not '<path>' / 'tags/<path>'."""
    problems = []
    c = cp.get("net/minecraft/core/registries/Registries")
    if c is None:
        return ["class Registries not found"]

    def body(name):
        ms = c.methods_named(name)
        if len(ms) != 1:
            return None
        m = ms[0]
        out = []
        for ins in m.insns():
            if ins.op in ("invokestatic", "invokevirtual"):
                out.append(c.member_ref(ins.arg)[2])
            elif ins.op == "invokedynamic":
                bidx, n, d = c.invoke_dynamic(ins.arg)
                ref, args = c.bootstrap_methods[bidx]
                out.append("concat:" + c.loadable(args[0])[1])
        return out

    if body("registryDirPath") != ["identifier", "getPath"]:
        problems.append("Registries.registryDirPath is not key.identifier().getPath()")
    if body("elementsDirPath") != ["registryDirPath"]:
        problems.append("Registries.elementsDirPath is not registryDirPath(key)")
    if body("tagsDirPath") != ["registryDirPath", "concat:tags/\x01"]:
        problems.append("Registries.tagsDirPath is not 'tags/' + registryDirPath(key)")
    return problems


class DataPack:
    """Index of the json files under ``data/`` of the server jar (the vanilla data pack)."""

    def __init__(self, zf):
        self.zf = zf
        self.files = {}         # namespace -> [path below data/<ns>/]
        for n in zf.namelist():
            if not n.startswith(DATA) or not n.endswith(".json"):
                continue
            rest = n[len(DATA):]
            ns, _, path = rest.partition("/")
            if not path:
                continue
            self.files.setdefault(ns, []).append(path)

    def read_json(self, ns, path):
        raw = self.zf.read("%s%s/%s" % (DATA, ns, path))
        return json.loads(raw.decode("utf-8"))

    def list(self, prefix):
        """All (id, namespace, path) of json files below ``data/*/<prefix>/``."""
        out = []
        pre = prefix.rstrip("/") + "/"
        for ns, paths in self.files.items():
            for p in paths:
                if p.startswith(pre):
                    out.append(("%s:%s" % (ns, p[len(pre):-5]), ns, p))
        return out


def _path_of(registry_id):
    ns, _, path = registry_id.partition(":")
    return path


def registry_entries(pack, registry_id):
    """Entry ids of a data-driven registry.

    Sorted the way vanilla's resource listing orders identifiers (by path, then
    namespace), which for the single ``minecraft`` namespace is plain string order.
    """
    ids = [i for i, _ns, _p in pack.list(_path_of(registry_id))]

    def key(i):
        ns, _, path = i.partition(":")
        return (path, ns)

    return sorted(ids, key=key)


def raw_tags(pack, registry_id):
    """tag id -> list of (is_tag, id, required) in file order."""
    out = {}
    for tid, ns, path in pack.list("tags/" + _path_of(registry_id)):
        doc = pack.read_json(ns, path)
        entries = []
        for v in doc.get("values", []):
            required = True
            if isinstance(v, dict):
                required = bool(v.get("required", True))
                v = v["id"]
            is_tag = v.startswith("#")
            if is_tag:
                v = v[1:]
            if ":" not in v:
                v = "minecraft:" + v
            entries.append((is_tag, v, required))
        out[tid] = entries
    return out


def resolve_tags(raw, valid_elements):
    """Resolve tag references.

    -> (tag id -> ordered element ids, problems).  Tags that vanilla would fail
    to load (missing required reference, reference cycle) are left out and
    reported in ``problems``.
    """
    valid = set(valid_elements)
    done = {}
    failed = {}
    problems = []

    def build(tid, stack):
        if tid in done:
            return done[tid]
        if tid in failed:
            return None
        if tid in stack:
            failed[tid] = "reference cycle"
            problems.append("tag %s: reference cycle" % tid)
            return None
        seen = set()
        order = []
        missing = []
        for is_tag, ref, required in raw[tid]:
            if is_tag:
                if ref not in raw:
                    if required:
                        missing.append("#" + ref)
                    continue
                sub = build(ref, stack + [tid])
                if sub is None:
                    if required:
                        missing.append("#" + ref)
                    continue
                for e in sub:
                    if e not in seen:
                        seen.add(e)
                        order.append(e)
            else:
                if ref not in valid:
                    if required:
                        missing.append(ref)
                    continue
                if ref not in seen:
                    seen.add(ref)
                    order.append(ref)
        if missing:
            failed[tid] = "missing " + ", ".join(missing)
            problems.append("tag %s: missing required references: %s" % (tid, ", ".join(missing)))
            return None
        done[tid] = order
        return order

    for tid in sorted(raw):
        build(tid, [])
    return done, problems


def dimension_types(pack):
    out = {}
    for did in registry_entries(pack, "minecraft:dimension_type"):
        ns, _, path = did.partition(":")
        out[did] = pack.read_json(ns, "dimension_type/%s.json" % path)
    return out
