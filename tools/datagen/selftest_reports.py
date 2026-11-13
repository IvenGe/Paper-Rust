#!/usr/bin/env python3
"""Round-trip test of the (otherwise untested) ``--reports`` input mode.

    python3 -I tools/datagen/selftest_reports.py --data data/26.3 --work <scratch dir> \\
        --jar <server.jar> --mcmeta <mcmeta summary> [--pumpkin <assets>]

1. rebuilds files in the *format of Mojang's data-generator reports*
   (``registries.json``, ``blocks.json``, ``packets.json``) from the tables in
   ``--data`` -- these are SYNTHETIC reports, not generator output;
2. runs extract.py with ``--reports`` pointing at them;
3. compares every produced file byte for byte with ``--data``.

This exercises the report reader and proves that a re-run is byte-identical.
It cannot prove that the reader copes with what the real generator writes.
"""

import argparse
import json
import os
import sys

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import extract  # noqa: E402
import tables  # noqa: E402


def synthetic_reports(data_dir, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    regs = tables.load_json(os.path.join(data_dir, "registries.json"))
    blocks = tables.load_json(os.path.join(data_dir, "blocks.json"))["blocks"]
    packets = tables.load_json(os.path.join(data_dir, "packets.json"))

    rdoc = {}
    for pid, (rid, names) in enumerate((k, v) for k, v in regs.items() if k.startswith("minecraft:")):
        rdoc[rid] = {"entries": {n: {"protocol_id": i} for i, n in enumerate(names)}, "protocol_id": pid}
    bdoc = {}
    for b in blocks:
        props = b["properties"]
        entry = {}
        if props:
            entry["properties"] = {p["name"]: p["values"] for p in props}
        b2 = dict(b)
        n = 1
        for p in props:
            n *= len(p["values"])
        b2["_states"] = n
        states = []
        for k, sp in enumerate(tables.state_properties(b2)):
            s = {"id": b["min_state"] + k}
            if sp:
                s["properties"] = sp
            if b["min_state"] + k == b["default_state"]:
                s["default"] = True
            states.append(s)
        entry["states"] = states
        bdoc[b["name"]] = entry
    pdoc = {}
    for state, flows in packets.items():
        for flow, names in flows.items():
            if names:
                pdoc.setdefault(state, {})[flow] = {n: {"protocol_id": i} for i, n in enumerate(names)}
    for name, doc in (("registries.json", rdoc), ("blocks.json", bdoc), ("packets.json", pdoc)):
        with open(os.path.join(out_dir, name), "w", encoding="utf-8") as f:
            json.dump(doc, f, sort_keys=True)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", required=True, help="directory with previously generated tables")
    ap.add_argument("--work", required=True, help="scratch directory (created)")
    ap.add_argument("--jar", required=True)
    ap.add_argument("--mcmeta", required=True)
    ap.add_argument("--pumpkin")
    args = ap.parse_args(argv)
    rep_dir = os.path.join(args.work, "synthetic-reports")
    out_dir = os.path.join(args.work, "out")
    synthetic_reports(args.data, rep_dir)
    cmd = ["--jar", args.jar, "--mcmeta", args.mcmeta, "--out", out_dir, "--reports", rep_dir,
           "--report", os.path.join(args.work, "validation.json"), "--quiet"]
    if args.pumpkin:
        cmd += ["--pumpkin", args.pumpkin]
    code = extract.main(cmd)
    bad = []
    for name in extract.OUTPUTS:
        a = os.path.join(args.data, name)
        b = os.path.join(out_dir, name)
        if not os.path.isfile(b):
            bad.append("%s was not produced" % name)
            continue
        with open(a, "rb") as fa, open(b, "rb") as fb:
            if fa.read() != fb.read():
                bad.append("%s differs" % name)
    if bad or code != 0:
        sys.stdout.write("reports-mode round trip FAILED (exit code %s): %s\n" % (code, "; ".join(bad) or "-"))
        return 1
    sys.stdout.write("reports-mode round trip ok: %d files byte-identical to %s (synthetic reports only)\n"
                     % (len(extract.OUTPUTS), args.data))
    return 0


if __name__ == "__main__":
    sys.exit(main())
