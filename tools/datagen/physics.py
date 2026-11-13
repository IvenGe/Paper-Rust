#!/usr/bin/env python3
"""Per-state block physics for block updates, fluids and redstone.

    python3 -I tools/datagen/physics.py \\
        --pumpkin /home/user/upstream/pumpkin/assets \\
        --data    data/26.3

Reads Pumpkin's `assets/blocks.json` (generated from a running Minecraft by
Pumpkin-MC/Extractor, see README.md "Attribution and licence of the Pumpkin
data") and writes `<data>/block_physics.json` with five arrays indexed by state
id. Before anything is written the block list, the block order and every
block's state id range are compared with `<data>/blocks.json`; any difference
aborts the run, so the columns can never be attached to the wrong states.

* `support`: Pumpkin `side_flags` unchanged. Bits 0-5: the face is sturdy for
  `SupportType.FULL` (`isFaceSturdy`) in direction down, up, north, south,
  west, east. Bit 6/7: the down/up face gives `SupportType.CENTER` support.
* `flags`: bit 0 `isSolid()` (vanilla's legacy "solid" used by fluids and
  `blocksMotion`), bit 1 `isRedstoneConductor`, bit 2
  `hasAnalogOutputSignal`, bits 3-4 piston push reaction (0 normal,
  1 destroy, 2 block, 3 push only).
* `collision_faces`: bits 0-5 (down, up, north, south, west, east): the
  collision shape covers that whole face of the block, i.e. what
  `Shapes.mergedFaceOccludes` needs. Computed here from the collision boxes.
* `instrument`: index into `instruments` (the values of `note_block`'s
  `instrument` property): the note block sound of a block placed under (or,
  for mob heads, over) a note block.

Standard library only; output is compact and deterministic.
"""

import argparse
import json
import os
import sys

PISTON = {"PUSH_PULL": 0, "POPPED": 1, "IMMOVEABLE": 2, "PUSH": 3}
IS_SOLID, IS_SOLID_BLOCK, HAS_ANALOG = 1 << 6, 1 << 8, 1 << 12

# (axis, positive) for down, up, north, south, west, east; axis 0=x 1=y 2=z.
FACES = [(1, False), (1, True), (2, False), (2, True), (0, False), (0, True)]


def face_full(boxes, axis, positive):
    """Whether the boxes touching the face plane cover the whole unit face."""
    plane = 1.0 if positive else 0.0
    extent = max(b["max"][axis] for b in boxes) if positive else min(b["min"][axis] for b in boxes)
    if abs(extent - plane) > 1e-7:
        return False
    other = [a for a in (0, 1, 2) if a != axis]
    rects = []
    for b in boxes:
        edge = b["max"][axis] if positive else b["min"][axis]
        if abs(edge - plane) > 1e-7:
            continue
        lo = [max(0.0, b["min"][a]) for a in other]
        hi = [min(1.0, b["max"][a]) for a in other]
        if lo[0] < hi[0] and lo[1] < hi[1]:
            rects.append((lo, hi))
    if not rects:
        return False
    us = sorted({0.0, 1.0} | {r[0][0] for r in rects} | {r[1][0] for r in rects})
    vs = sorted({0.0, 1.0} | {r[0][1] for r in rects} | {r[1][1] for r in rects})
    us = [u for u in us if 0.0 <= u <= 1.0]
    vs = [v for v in vs if 0.0 <= v <= 1.0]
    for i in range(len(us) - 1):
        for j in range(len(vs) - 1):
            cu, cv = (us[i] + us[i + 1]) / 2, (vs[j] + vs[j + 1]) / 2
            if not any(lo[0] <= cu <= hi[0] and lo[1] <= cv <= hi[1] for lo, hi in rects):
                return False
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--pumpkin", required=True, help="Pumpkin-MC/Pumpkin 'assets' directory")
    ap.add_argument("--data", required=True, help="the data/<version> directory to write into")
    args = ap.parse_args()

    with open(os.path.join(args.pumpkin, "blocks.json"), encoding="utf-8") as f:
        pk = json.load(f)
    with open(os.path.join(args.data, "blocks.json"), encoding="utf-8") as f:
        ours = json.load(f)

    errors = []
    if len(pk["blocks"]) != len(ours["blocks"]):
        errors.append("block count differs: pumpkin %d, ours %d" % (len(pk["blocks"]), len(ours["blocks"])))
    for a, b in zip(pk["blocks"], ours["blocks"]):
        name = "minecraft:" + a["name"]
        states = [s["id"] for s in a["states"]]
        count = 1
        for p in b["properties"]:
            count *= len(p["values"])
        if name != b["name"] or states != list(range(b["min_state"], b["min_state"] + count)):
            errors.append("block %s / %s: names or state ids differ" % (name, b["name"]))
    if errors:
        print("\n".join(errors[:20]), file=sys.stderr)
        sys.exit(2)

    note = next(b for b in ours["blocks"] if b["name"] == "minecraft:note_block")
    instruments = next(p["values"] for p in note["properties"] if p["name"] == "instrument")

    n = ours["state_count"]
    support, flags, faces, instrument = [0] * n, [0] * n, [0] * n, [0] * n
    shapes = pk["shapes"]
    for block in pk["blocks"]:
        for s in block["states"]:
            i = s["id"]
            support[i] = s["side_flags"]
            sf = s["state_flags"]
            v = 0
            if sf & IS_SOLID:
                v |= 1
            if sf & IS_SOLID_BLOCK:
                v |= 2
            if sf & HAS_ANALOG:
                v |= 4
            if s["piston_behavior"] not in PISTON:
                print("unknown piston behaviour %s" % s["piston_behavior"], file=sys.stderr)
                sys.exit(2)
            v |= PISTON[s["piston_behavior"]] << 3
            flags[i] = v
            boxes = [shapes[k] for k in s["collision_shapes"]]
            bits = 0
            if boxes:
                for d, (axis, positive) in enumerate(FACES):
                    if face_full(boxes, axis, positive):
                        bits |= 1 << d
            faces[i] = bits
            if s["instrument"] not in instruments:
                print("unknown instrument %s" % s["instrument"], file=sys.stderr)
                sys.exit(2)
            instrument[i] = instruments.index(s["instrument"])

    doc = {
        "instruments": instruments,
        "support": support,
        "flags": flags,
        "collision_faces": faces,
        "instrument": instrument,
    }
    out = os.path.join(args.data, "block_physics.json")
    with open(out, "w", encoding="utf-8") as f:
        f.write(json.dumps(doc, separators=(",", ":"), ensure_ascii=False) + "\n")
    full = sum(1 for x in faces if x == 63)
    print("wrote %s: %d states, %d with six full collision faces, %d conductors" % (out, n, full, sum(1 for x in flags if x & 2)))


if __name__ == "__main__":
    main()
