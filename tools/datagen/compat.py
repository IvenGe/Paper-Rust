#!/usr/bin/env python3
"""Writes data/compat/<version>.json: what the server needs to talk to clients of an older
version, for each version in VERSIONS.

Each file holds:

* `packets`: the version's packet names, in id order, by state and direction;
* `mappings`: for each id registry that differs, the id in the older version of every
  26.3 id (-1: none), as runs `[first 26.3 id, its old id, length]` of consecutive ids;
* `registries`: for each data-driven registry the server sends, the 26.3 entries the older
  version knows (by index) and a fallback for the others, or `dropped` if the older version
  does not have the registry at all.

The id mappings come from ViaVersion and ViaBackwards (https://github.com/ViaVersion, GPL-3.0
or later, as this project): their backwards mapping files, step by step from 26.3 down,
composed. Packet orders come from ViaVersion's packet enums. Which data-driven registry
entries a version has comes from misode/mcmeta's `<version>-summary` tags.

Built-in registries that ViaVersion does not map (game events, ...) are mapped by name: the
older version's ids are taken to be 26.3's order without the entries it does not have, as
holds when entries are only added. The core data pack versions a client may report are the
release names sharing its protocol in minecraft-data (MIT).

Usage: compat.py <viaversion checkout> <viabackwards checkout> <mcmeta versions dir>
                 <minecraft-data protocolVersions.json> <repo>

where <mcmeta versions dir>/<version>/ holds that version's summary (`registries/`,
`data/<registry>/data.min.json`), as fetched from
https://raw.githubusercontent.com/misode/mcmeta/<version>-summary/...
"""

import gzip
import json
import re
import struct
import sys
from pathlib import Path

# Newest first: each version's file maps 26.3 straight to it, through every step above it.
# `summary`: the mcmeta summary of the version (the last release with its protocol); `play`
# and `config`: ViaVersion's packet enums, by direction.
PLAY_26_1 = {"clientbound": "v1_21_11to26_1/packet/ClientboundPackets26_1.java", "serverbound": "v1_21_11to26_1/packet/ServerboundPackets26_1.java"}
CONFIG_1_21_9 = {"clientbound": "v1_21_7to1_21_9/packet/ClientboundConfigurationPackets1_21_9.java", "serverbound": "v1_21_7to1_21_9/packet/ServerboundConfigurationPackets1_21_9.java"}
CONFIG_1_21_6 = {"clientbound": "v1_21_5to1_21_6/packet/ClientboundConfigurationPackets1_21_6.java", "serverbound": "v1_21_5to1_21_6/packet/ServerboundConfigurationPackets1_21_6.java"}
SERVERBOUND_1_21_6 = "v1_21_5to1_21_6/packet/ServerboundPackets1_21_6.java"
VERSIONS = [
    {"name": "26.2", "protocol": 776, "step": "26.3to26.2", "summary": "26.2", "play": PLAY_26_1, "config": CONFIG_1_21_9},
    {"name": "26.1", "protocol": 775, "step": "26.2to26.1", "summary": "26.1", "play": PLAY_26_1, "config": CONFIG_1_21_9},
    {"name": "1.21.11", "protocol": 774, "step": "26.1to1.21.11", "summary": "1.21.11",
     "play": {"clientbound": "v1_21_9to1_21_11/packet/ClientboundPackets1_21_11.java", "serverbound": SERVERBOUND_1_21_6}, "config": CONFIG_1_21_9},
    {"name": "1.21.9", "protocol": 773, "step": "1.21.11to1.21.9", "summary": "1.21.10",
     "play": {"clientbound": "v1_21_7to1_21_9/packet/ClientboundPackets1_21_9.java", "serverbound": SERVERBOUND_1_21_6}, "config": CONFIG_1_21_9},
    {"name": "1.21.7", "protocol": 772, "step": "1.21.9to1.21.7", "summary": "1.21.8",
     "play": {"clientbound": "v1_21_5to1_21_6/packet/ClientboundPackets1_21_6.java", "serverbound": SERVERBOUND_1_21_6}, "config": CONFIG_1_21_6},
    {"name": "1.21.6", "protocol": 771, "step": "1.21.7to1.21.6", "summary": "1.21.6",
     "play": {"clientbound": "v1_21_5to1_21_6/packet/ClientboundPackets1_21_6.java", "serverbound": SERVERBOUND_1_21_6}, "config": CONFIG_1_21_6},
    {"name": "1.21.5", "protocol": 770, "step": "1.21.6to1.21.5", "summary": "1.21.5",
     "play": {"clientbound": "v1_21_4to1_21_5/packet/ClientboundPackets1_21_5.java", "serverbound": "v1_21_4to1_21_5/packet/ServerboundPackets1_21_5.java"},
     "config": {"clientbound": "v1_20_5to1_21/packet/ClientboundConfigurationPackets1_21.java", "serverbound": "v1_20_3to1_20_5/packet/ServerboundConfigurationPackets1_20_5.java"}},
]

# Packets an older version names differently, by its name: 26.3's.
RENAMED_PACKETS = {
    "minecraft:horse_screen_open": "minecraft:mount_screen_open",
}

# Id registries of the mapping files, by the name of the registry in 26.3.
MAPPING_KEYS = {
    "blockstates": None,
    "blocks": "minecraft:block",
    "items": "minecraft:item",
    "entities": "minecraft:entity_type",
    "sounds": "minecraft:sound_event",
    "particles": "minecraft:particle_type",
    "data_component_type": "minecraft:data_component_type",
    "argumenttypes": "minecraft:command_argument_type",
    "statistics": "minecraft:custom_stat",
    "recipe_serializers": "minecraft:recipe_serializer",
    "attributes": "minecraft:attribute",
    "blockentities": "minecraft:block_entity_type",
}

# What a client is told instead of a registry entry it does not know.
FALLBACKS = {
    "minecraft:worldgen/biome": "minecraft:plains",
    "minecraft:damage_type": "minecraft:generic",
    "minecraft:dimension_type": "minecraft:overworld",
    "minecraft:chat_type": "minecraft:chat",
}


# --- NBT and Via's mapping format ---------------------------------------------------------

def read_nbt(data):
    if data[:2] == b"\x1f\x8b":
        data = gzip.decompress(data)
    pos = 0

    def take(n):
        nonlocal pos
        chunk = data[pos:pos + n]
        pos += n
        return chunk

    def string():
        (n,) = struct.unpack(">H", take(2))
        return take(n).decode("utf-8", "replace")

    def payload(t):
        if t == 1:
            return struct.unpack(">b", take(1))[0]
        if t == 2:
            return struct.unpack(">h", take(2))[0]
        if t == 3:
            return struct.unpack(">i", take(4))[0]
        if t == 4:
            return struct.unpack(">q", take(8))[0]
        if t == 5:
            return struct.unpack(">f", take(4))[0]
        if t == 6:
            return struct.unpack(">d", take(8))[0]
        if t == 7:
            (n,) = struct.unpack(">i", take(4))
            return bytes(take(n))
        if t == 8:
            return string()
        if t == 9:
            element = take(1)[0]
            (n,) = struct.unpack(">i", take(4))
            return [payload(element) for _ in range(n)]
        if t == 10:
            out = {}
            while True:
                element = take(1)[0]
                if element == 0:
                    return out
                key = string()
                out[key] = payload(element)
        if t == 11:
            (n,) = struct.unpack(">i", take(4))
            return list(struct.unpack(">%di" % n, take(4 * n)))
        if t == 12:
            (n,) = struct.unpack(">i", take(4))
            return list(struct.unpack(">%dq" % n, take(8 * n)))
        raise ValueError(f"tag type {t}")

    root = take(1)[0]
    string()
    return payload(root)


def varint(buf, i):
    value = shift = 0
    while True:
        b = buf[i]
        i += 1
        value |= (b & 0x7F) << shift
        shift += 7
        if not b & 0x80:
            return value, i


def zigzag(v):
    return (v >> 1) ^ -(v & 1)


def via_mappings(tag):
    """A mapping of Via's format (version 2) as a list: old id of each id, -1 for none."""
    strategy, size = tag["id"], tag.get("size")
    if strategy == 0:  # direct: zigzag deltas
        buf, i, prev, out = tag["val"], 0, 0, []
        for _ in range(size):
            v, i = varint(buf, i)
            prev += zigzag(v)
            out.append(prev)
        return out
    if strategy == 3:  # identity
        return list(range(size))
    buf, i, at, values, prev_at, prev_value = tag["val"], 0, [], [], -1, 0
    while i < len(buf):
        v, i = varint(buf, i)
        prev_at += 1 + v
        v, i = varint(buf, i)
        prev_value += zigzag(v)
        at.append(prev_at)
        values.append(prev_value)
    out = [-1] * size
    if strategy == 1:  # shifts
        for id in range(0, at[0]):
            out[id] = id
        for k, start in enumerate(at):
            end = size if k == len(at) - 1 else at[k + 1]
            mapped = values[k]
            for id in range(start, end):
                out[id] = mapped
                mapped += 1
        return out
    if strategy == 2:  # changes
        fill = "nofill" not in tag
        next_id = 0
        for k, changed in enumerate(at):
            if fill:
                for id in range(next_id, changed):
                    out[id] = id
                next_id = changed + 1
            out[changed] = values[k]
        if fill:
            for id in range(next_id, size):
                out[id] = id
        return out
    raise ValueError(f"mapping strategy {strategy}")


def runs(mapping):
    """`[[first id, old id, length], ...]` of consecutive ids (old id -1: none)."""
    out = []
    for id, old in enumerate(mapping):
        if out:
            first, first_old, length = out[-1]
            if first + length == id and ((old == -1 and first_old == -1) or (old != -1 and first_old != -1 and first_old + length == old)):
                out[-1][2] += 1
                continue
        out.append([id, old, 1])
    return out


# --- sources ---------------------------------------------------------------------------------

def enum_names(path):
    """The constants of a Java enum, in order, as packet names (`ADD_ENTITY` -> `minecraft:add_entity`)."""
    text = Path(path).read_text()
    body = text[text.index("{", text.index(" enum ")) + 1:]
    body = body[: body.index(";")]
    names = re.findall(r"^\s*([A-Z0-9_]+)\s*(?:[,(]|$)", body, re.M)
    return [f"minecraft:{n.lower()}" for n in names]


def compose(first, second):
    return [second[i] if 0 <= i < len(second) else -1 for i in first]


def main():
    via, backwards, mcmeta, protocol_versions, repo = (Path(a) for a in sys.argv[1:6])
    releases = json.loads(protocol_versions.read_text())
    via_protocols = via / "common/src/main/java/com/viaversion/viaversion/protocols"
    via_data = via / "common/src/main/resources/assets/viaversion/data"
    back_data = backwards / "common/src/main/resources/assets/viabackwards/data"
    ours_packets = json.loads((repo / "data/26.3/packets.json").read_text())
    registries = json.loads((repo / "data/26.3/registries.json").read_text())
    synced = json.loads((repo / "data/26.3/synced_registries.json").read_text())
    out_dir = repo / "data/compat"
    out_dir.mkdir(parents=True, exist_ok=True)

    # Our packet names are Mojang's; ViaVersion's enums spell a few differently
    # (`DEBUG_BLOCK_VALUE` for `debug/block_value`, ...). Line the two up for 26.3, where the
    # orders agree, and use our names for every version.
    aliases = {}
    for state, file in (("play", "v26_2to26_3/packet/{dir}Packets26_3.java"), ("configuration", "v26_2to26_3/packet/ClientboundConfigurationPackets26_3.java")):
        for direction in ("clientbound", "serverbound") if state == "play" else ("clientbound",):
            names = enum_names(via_protocols / file.format(dir=direction.capitalize()))
            ours = ours_packets[state][direction]
            if len(names) != len(ours):
                sys.exit(f"26.3 {state} {direction}: {len(ours)} packets, ViaVersion has {len(names)}")
            for theirs, mine in zip(names, ours):
                if theirs != mine:
                    if theirs.replace("_", "") != mine.replace("/", "").replace("_", "").replace("tracked", "") and theirs.removeprefix("minecraft:tracked_") != mine.removeprefix("minecraft:"):
                        sys.exit(f"26.3 {state} {direction}: ViaVersion's {theirs} is our {mine}?")
                    aliases[theirs] = mine

    # The id mappings so far (26.3 -> the version), per key, and the size of each registry
    # in the version.
    current, sizes = {}, {}
    for key, registry in MAPPING_KEYS.items():
        size = int(json.loads((repo / "data/26.3/blocks.json").read_text())["state_count"]) if key == "blockstates" else len(registries[registry])
        current[key] = list(range(size))
        sizes[key] = size

    errors = []
    for version in VERSIONS:
        step = read_nbt((back_data / f"mappings-{version['step']}.nbt").read_bytes())
        for key in MAPPING_KEYS:
            if key in step:
                current[key] = compose(current[key], via_mappings(step[key]))
                sizes[key] = step[key]["mappedSize"]

        packets = {}
        for state in ("handshake", "status", "login"):
            packets[state] = ours_packets[state]
        for state, key in (("configuration", "config"), ("play", "play")):
            packets[state] = {d: [RENAMED_PACKETS.get(n, aliases.get(n, n)) for n in enum_names(via_protocols / version[key][d])] for d in ("clientbound", "serverbound")}

        summary = mcmeta / version["summary"]
        if not summary.is_dir():
            sys.exit(f"no mcmeta summary for {version['summary']} in {mcmeta}")
        data_registries = {}
        for registry in synced["order"]:
            entries = synced["entries"][registry]
            path = summary / "data" / registry.removeprefix("minecraft:") / "data.min.json"
            if not path.is_file():
                data_registries[registry] = {"dropped": True}
                continue
            known = {f"minecraft:{k}" if ":" not in k else k for k in json.loads(path.read_text())}
            keep = [i for i, e in enumerate(entries) if e in known]
            if not keep:
                errors.append(f"{version['name']}: no entry of {registry} is known")
                continue
            fallback_name = FALLBACKS.get(registry)
            fallback = entries.index(fallback_name) if fallback_name in entries and entries.index(fallback_name) in keep else keep[0]
            data_registries[registry] = {"keep": keep, "fallback": fallback} if len(keep) != len(entries) else {}

        # Built-in registries ViaVersion does not map, by name.
        mapped = {r for r in MAPPING_KEYS.values() if r}
        old_registries = json.loads((summary / "registries" / "data.min.json").read_text())
        by_name = {}
        for registry, entries in registries.items():
            short = registry.removeprefix("minecraft:")
            if registry in mapped or short not in old_registries or registry in synced["entries"]:
                continue
            known = {f"minecraft:{e}" if ":" not in e else e for e in old_registries[short]}
            if all(e in known for e in entries):
                continue
            next_id, mapping = 0, []
            for e in entries:
                if e in known:
                    mapping.append(next_id)
                    next_id += 1
                else:
                    mapping.append(-1)
            by_name[f"registry:{registry}"] = mapping

        out = {
            "name": version["name"],
            "protocol": version["protocol"],
            "pack_versions": [r["minecraftVersion"] for r in releases if r["version"] == version["protocol"] and r.get("usesNetty", True)],
            "sizes": dict(sizes),
            "packets": packets,
            "mappings": {key: runs(m) for key, m in list(current.items()) + list(by_name.items()) if m != list(range(len(m)))},
            "registries": {k: v for k, v in data_registries.items() if v},
        }
        (out_dir / f"{version['name']}.json").write_text(json.dumps(out, separators=(",", ":")) + "\n")
        counts = {k: sum(1 for x in m if x >= 0) for k, m in current.items()}
        print(f"{version['name']}: mapped {counts}; registries changed {sorted(out['registries'])}")
    if errors:
        sys.exit("\n".join(errors))


if __name__ == "__main__":
    main()
