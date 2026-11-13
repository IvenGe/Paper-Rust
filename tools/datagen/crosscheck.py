"""Cross-validation of the bytecode-derived tables against independent sources.

None of these sources is used to *produce* ids; they only confirm (or contradict)
what was derived from the jar:

  * misode/mcmeta ``registries/data.json`` -- alphabetical name sets per registry
    (produced by Mojang's data generator): set / count / duplicate check;
  * Pumpkin-MC ``assets/`` -- dumps taken from a running 26.3 game by Pumpkin's
    extractor mod: order check for the registries it covers, packets, serializers;
  * minecraft.wiki protocol pages -- packet order per state, metadata type order;
  * a Burger dump -- particle and sound ids;
  * PrismarineJS minecraft-data of an *older* version -- relative order sanity only.
"""

import os
import re

from tables import short, load_json


# --------------------------------------------------------------------------
# mcmeta
# --------------------------------------------------------------------------
def registries_vs_mcmeta(registries, mcmeta):
    """-> (valid registry ids, results [(registry, ok, detail)])"""
    valid = []
    results = []
    for rid, names in registries.items():
        exp = mcmeta.get(short(rid))
        if exp is None:
            results.append((rid, False, "no such registry in mcmeta registries/data.json"))
            continue
        got = [short(n) for n in names]
        problems = []
        if len(set(got)) != len(got):
            dup = sorted(n for n in set(got) if got.count(n) > 1)
            problems.append("duplicates %s" % dup[:5])
        if len(got) != len(exp):
            problems.append("count %d, mcmeta %d" % (len(got), len(exp)))
        if set(got) != set(exp):
            problems.append("missing %s, unexpected %s" % (sorted(set(exp) - set(got))[:5],
                                                           sorted(set(got) - set(exp))[:5]))
        if problems:
            results.append((rid, False, "; ".join(problems)))
        else:
            valid.append(rid)
            results.append((rid, True, "%d entries, same set" % len(got)))
    return valid, results


def sets_vs_mcmeta(table, mcmeta, what):
    """Generic set comparison for data-driven registries (synced registry entries)."""
    results = []
    for rid, names in table.items():
        exp = mcmeta.get(short(rid))
        if exp is None:
            results.append((rid, False, "no such registry in mcmeta"))
            continue
        got = [short(n) for n in names]
        ok = len(got) == len(exp) and set(got) == set(exp) and len(set(got)) == len(got)
        results.append((rid, ok, "%d %s, same set" % (len(got), what) if ok else
                        "mine %d, mcmeta %d; missing %s, unexpected %s" % (
                            len(got), len(exp), sorted(set(exp) - set(got))[:5],
                            sorted(set(got) - set(exp))[:5])))
    return results


# --------------------------------------------------------------------------
# Pumpkin
# --------------------------------------------------------------------------
def _by_id(d):
    return [k for k, _v in sorted(d.items(), key=lambda kv: kv[1]["id"])]


def _order_result(label, mine, theirs):
    mine = [short(n) for n in mine]
    theirs = [short(n) for n in theirs]
    if mine == theirs:
        return (label, True, "%d entries, identical order" % len(mine))
    first = next((i for i, (a, b) in enumerate(zip(mine, theirs)) if a != b), min(len(mine), len(theirs)))
    return (label, False, "mine %d, theirs %d, first difference at id %d: %r vs %r (same set: %s)" % (
        len(mine), len(theirs), first, mine[first:first + 2], theirs[first:first + 2],
        set(mine) == set(theirs)))


PUMPKIN_REGISTRIES = [
    # registry, file, extractor
    ("minecraft:block", "blocks.json", lambda d: [b["name"] for b in sorted(d["blocks"], key=lambda b: b["id"])]),
    ("minecraft:block_entity_type", "blocks.json", lambda d: d["block_entity_types"]),
    ("minecraft:item", "items.json", _by_id),
    ("minecraft:entity_type", "entities.json", _by_id),
    ("minecraft:sound_event", "sounds.json", lambda d: d),
    ("minecraft:particle_type", "particles.json", lambda d: d),
    ("minecraft:mob_effect", "effect.json", _by_id),
    ("minecraft:attribute", "attributes.json", _by_id),
    ("minecraft:fluid", "fluids.json", lambda d: [f["name"] for f in sorted(d, key=lambda f: f["id"])]),
    ("minecraft:game_event", "game_event.json", lambda d: [g["name"] for g in d]),
    ("minecraft:data_component_type", "data_component.json", _by_id),
    ("minecraft:potion", "potion.json", _by_id),
    ("minecraft:custom_stat", "custom_stats.json", lambda d: d),
    ("minecraft:point_of_interest_type", "poi_types.json", _by_id),
    ("minecraft:menu", "screens.json", lambda d: d),
    ("minecraft:chunk_status", "chunk_status.json", lambda d: d),
    ("minecraft:map_decoration_type", "map_decorations.json", lambda d: [m["name"] for m in d]),
    ("minecraft:position_source_type", "position_source_type.json", lambda d: d),
    ("minecraft:stat_type", "stats.json", _by_id),
    # written by iterating the registry; json object key order is the iteration order
    ("minecraft:game_rule", "game_rules.json", lambda d: list(d)),
    ("minecraft:villager_profession", "villager_data.json", lambda d: list(d["professions"])),
    ("minecraft:villager_type", "villager_data.json", lambda d: list(d["types"])),
]


def registries_vs_pumpkin(registries, pumpkin):
    results = []
    for rid, fname, extract in PUMPKIN_REGISTRIES:
        if rid not in registries:
            results.append((rid, False, "registry was not extracted"))
            continue
        if not pumpkin.has(fname):
            results.append((rid, None, "pumpkin assets/%s not present" % fname))
            continue
        try:
            theirs = extract(pumpkin.load(fname))
        except (KeyError, TypeError, ValueError) as e:
            results.append((rid, None, "pumpkin assets/%s has an unexpected shape (%s)" % (fname, e)))
            continue
        results.append(_order_result(rid, registries[rid], theirs))
    return results


_PUMPKIN_STATES = {"handshake": "handshake", "status": "status", "login": "login",
                   "config": "configuration", "play": "play"}


def packets_vs_pumpkin(packets, path, label):
    """Compare with a Pumpkin ``packets.json`` (name -> id per direction and state)."""
    results = []
    try:
        d = load_json(path)
    except (OSError, ValueError) as e:
        return [(label, None, "cannot read %s: %s" % (path, e))], None
    version = d.get("version")
    seen = set()
    for flow in ("serverbound", "clientbound"):
        for pstate, table in d.get(flow, {}).items():
            state = _PUMPKIN_STATES.get(pstate, pstate)
            theirs = ["minecraft:" + n for n, _i in sorted(table.items(), key=lambda kv: kv[1])]
            ids = sorted(table.values())
            mine = packets.get(state, {}).get(flow, [])
            seen.add((state, flow))
            name = "%s %s/%s" % (label, state, flow)
            if ids != list(range(len(ids))):
                results.append((name, False, "ids in the reference are not 0..n-1"))
            elif mine == theirs:
                results.append((name, True, "%d packets, identical ids" % len(mine)))
            else:
                first = next((i for i, (a, b) in enumerate(zip(mine, theirs)) if a != b),
                             min(len(mine), len(theirs)))
                results.append((name, False, "mine %d, reference %d, first difference at id %d: %r vs %r" % (
                    len(mine), len(theirs), first, mine[first:first + 1], theirs[first:first + 1])))
    for state, flows in packets.items():
        for flow, lst in flows.items():
            if (state, flow) not in seen and lst:
                results.append(("%s %s/%s" % (label, state, flow), False, "not present in the reference"))
    return results, version


_WIKI_STATES = {"handshaking": "handshake", "status": "status", "login": "login",
                "configuration": "configuration", "play": "play"}


def parse_wiki_packets(path):
    """The 'List of packets' section: packets appear in id order, with their resource names."""
    out = {}
    cur = None
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            m = re.match(r"\{\{packet list/begin\|([^|}]+)\|([^|}]+)\}\}", line.strip())
            if m:
                state = _WIKI_STATES.get(m.group(1).strip().lower())
                cur = (state, m.group(2).strip().lower())
                out[cur] = []
                continue
            m = re.match(r"\{\{packet list\|([^|}]+)\|([^|}]+)\}\}", line.strip())
            if m and cur is not None:
                out[cur].append(("minecraft:" + m.group(2).strip(), m.group(1).strip()))
                continue
            if line.strip().startswith("|}"):
                cur = None
    return out


def packets_vs_wiki(packets, path):
    results = []
    try:
        wiki = parse_wiki_packets(path)
    except OSError as e:
        return [("wiki packets", None, "cannot read %s: %s" % (path, e))]
    if not wiki:
        return [("wiki packets", None, "no packet lists found in %s" % path)]
    seen = set()
    for (state, flow), lst in wiki.items():
        mine = packets.get(state, {}).get(flow, [])
        theirs = [n for n, _t in lst]
        seen.add((state, flow))
        name = "wiki %s/%s" % (state, flow)
        if mine == theirs:
            results.append((name, True, "%d packets, identical order" % len(mine)))
        elif len(mine) == len(theirs):
            diffs = ["0x%02X %s (wiki: %s)" % (i, a, b) for i, (a, b) in enumerate(zip(mine, theirs)) if a != b]
            results.append((name, False, "%d packets; %d resource names are spelled differently on the wiki: %s" % (
                len(mine), len(diffs), "; ".join(diffs[:12]))))
        else:
            first = next((i for i, (a, b) in enumerate(zip(mine, theirs)) if a != b),
                         min(len(mine), len(theirs)))
            results.append((name, False, "mine %d, wiki %d, first difference at id 0x%02X: %r vs %r" % (
                len(mine), len(theirs), first, mine[first:first + 1], theirs[first:first + 1])))
    for state, flows in packets.items():
        for flow, lst in flows.items():
            if (state, flow) not in seen and lst:
                results.append(("wiki %s/%s" % (state, flow), False, "not listed on the wiki page"))
    return results


def _wiki_rows(path):
    rows = []
    cur = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            t = line.strip()
            if t.startswith("|-") or t.startswith("|}") or t.startswith("{|") or t.startswith("!"):
                if cur:
                    rows.append(cur)
                cur = []
            elif t.startswith("|"):
                cell = t[1:].strip()
                if re.match(r"(rowspan|colspan|style)=", cell) and not cur:
                    continue        # a spanning label cell in front of the row's own cells
                cur.append(cell)
    if cur:
        rows.append(cur)
    return rows


_WIKI_IDENT = r"<code>([a-z0-9_]+(?::[a-z0-9_/.]+)?)</code>"


def wiki_id_runs(path):
    """All wiki-table row runs of the form  | 0 | .. <code>identifier</code> | ..,  | 1 | ..

    -> list of identifier lists; the position in a list is the numeric id printed in the table.
    """
    runs = []
    run = None
    for r in _wiki_rows(path):
        ident = None
        if r and re.fullmatch(r"\d+", r[0]):
            for cell in r[1:6]:
                m = re.search(_WIKI_IDENT, cell)
                if m:
                    ident = m.group(1)
                    break
        if ident is None:
            continue
        i = int(r[0])
        if i == 0:
            run = [ident]
            runs.append(run)
        elif run is not None and i == len(run):
            run.append(ident)
        else:
            run = None
    return runs


def wiki_identifier_column(path):
    """Identifiers of the rows whose first cell is exactly ``<code>identifier</code>``, in page order."""
    out = []
    for r in _wiki_rows(path):
        if r:
            m = re.fullmatch(_WIKI_IDENT, r[0])
            if m:
                out.append(m.group(1))
    return out


WIKI_ID_TABLES = [
    ("Entity_metadata.wiki", "minecraft:entity_type"),
    ("Command_data.wiki", "minecraft:command_argument_type"),
    ("Inventory.wiki", "minecraft:menu"),
    ("Slot_data.wiki", "minecraft:data_component_type"),
]


def registries_vs_wiki_tables(registries, wiki_dir):
    """Numeric id tables on the protocol wiki pages vs the registries."""
    results = []
    for page, rid in WIKI_ID_TABLES:
        path = os.path.join(wiki_dir, page)
        label = "wiki %s table vs %s" % (page, rid)
        if rid not in registries:
            continue
        if not os.path.isfile(path) or os.path.getsize(path) == 0:
            results.append((label, None, "page not available"))
            continue
        mine = registries[rid]
        names = set(mine)
        best = None
        for run in wiki_id_runs(path):
            run = [x if ":" in x else "minecraft:" + x for x in run]
            score = sum(1 for x in run if x in names)
            if best is None or score > best[0]:
                best = (score, run)
        if best is None or best[0] == 0:
            # a table without an id column: its row order is the id order
            seen = set()
            col = []
            for x in wiki_identifier_column(path):
                x = x if ":" in x else "minecraft:" + x
                if x in names and x not in seen:
                    seen.add(x)
                    col.append(x)
            if not col:
                results.append((label, None, "no table for this registry found on the page"))
            elif col == mine:
                results.append((label, True, "%d rows (no id column), same order" % len(mine)))
            else:
                pos = {n: i for i, n in enumerate(mine)}
                seq = [pos[x] for x in col]
                missing = [n for n in mine if n not in seen]
                if seq == sorted(seq):
                    # the page prints no ids, so an incomplete list is not a contradiction
                    results.append((label, None, "the page has no id column and lists only %d of the %d "
                                    "entries, in the same relative order; not on the page: %s" % (
                                        len(col), len(mine), ", ".join(short(n) for n in missing)[:400])))
                else:
                    moved = [col[i] for i in range(1, len(seq)) if seq[i] < seq[i - 1]]
                    results.append((label, None, "the page has no id column, lists only %d of the %d entries "
                                    "and not in registry order (%d places, first %s) -- it cannot confirm or "
                                    "contradict ids; not on the page: %s" % (
                                        len(col), len(mine), len(moved), short(moved[0]),
                                        ", ".join(short(n) for n in missing)[:300])))
            continue
        wiki = best[1]
        if wiki == mine:
            results.append((label, True, "%d ids, identical" % len(mine)))
            continue
        diffs = ["%d: %s (wiki: %s)" % (i, a, b) for i, (a, b) in enumerate(zip(mine, wiki)) if a != b]
        detail = "bytecode/mcmeta have %d entries, the wiki table %d" % (len(mine), len(wiki))
        if diffs:
            detail += "; %d ids differ: %s" % (len(diffs), "; ".join(diffs[:6]))
        if len(mine) > len(wiki):
            detail += "; not on the wiki: %s" % ", ".join(
                "%d %s" % (i, n) for i, n in enumerate(mine) if n not in set(wiki))[:300]
        results.append((label, False, detail))
    return results


# the wiki names metadata types after their wire format, not after the field names
_WIKI_SERIALIZER_ALIASES = {
    "varint": "int", "varlong": "long", "text_component": "component",
    "optional_text_component": "optional_component", "slot": "item_stack", "position": "block_pos",
    "optional_position": "optional_block_pos", "optional_varint": "optional_unsigned_int",
    "optional_global_position": "optional_global_pos",
}


def serializers_vs_wiki(serializers, path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        return [("wiki metadata types", None, "cannot read %s: %s" % (path, e))]
    names = re.findall(r"\{\{Metadata type definition\|([^|}]+)\}\}", text)
    if not names:
        return [("wiki metadata types", None, "no metadata type table found")]
    wiki = []
    for n in names:
        k = n.strip().lower().replace(" ", "_")
        wiki.append(_WIKI_SERIALIZER_ALIASES.get(k, k))
    mine = [s.lower() for s in serializers]
    if mine == wiki:
        return [("wiki metadata types", True, "%d types, identical order (wiki names mapped to field names)" % len(mine))]
    first = next((i for i, (a, b) in enumerate(zip(mine, wiki)) if a != b), min(len(mine), len(wiki)))
    return [("wiki metadata types", False, "mine %d, wiki %d, first difference at id %d: %r vs %r" % (
        len(mine), len(wiki), first, mine[first:first + 1], wiki[first:first + 1]))]


def serializers_vs_pumpkin(serializers, pumpkin):
    if not pumpkin.has("meta_data_type.json"):
        return [("pumpkin meta_data_type.json", None, "not present")]
    d = pumpkin.load("meta_data_type.json")
    theirs = [k for k, _v in sorted(d.items(), key=lambda kv: kv[1])]
    mine = [s.lower() for s in serializers]
    ok = mine == theirs and sorted(d.values()) == list(range(len(d)))
    return [("pumpkin meta_data_type.json", ok,
             "%d serializers, identical ids" % len(mine) if ok else "mine %s..., pumpkin %s..." % (mine[:4], theirs[:4]))]


# --------------------------------------------------------------------------
# Burger
# --------------------------------------------------------------------------
def vs_burger(registries, path):
    results = []
    try:
        d = load_json(path)
    except (OSError, ValueError) as e:
        return [("burger", None, "cannot read %s: %s" % (path, e))]
    if isinstance(d, list) and d:
        d = d[0]
    pt = d.get("particletypes")
    if isinstance(pt, list) and "minecraft:particle_type" in registries:
        results.append(_order_result("burger particle types", registries["minecraft:particle_type"], pt))
    snd = d.get("sounds")
    if isinstance(snd, dict) and "minecraft:sound_event" in registries:
        try:
            theirs = [k for k, _v in sorted(snd.items(), key=lambda kv: kv[1]["id"])]
        except (KeyError, TypeError):
            results.append(("burger sound ids", None, "unexpected shape"))
            return results
        mine = [short(n) for n in registries["minecraft:sound_event"]]
        if set(theirs) < set(mine):
            # an incomplete dump cannot contradict ids; compare what it has by relative order
            pos = {n: i for i, n in enumerate(mine)}
            seq = [pos[n] for n in theirs]
            missing = [n for n in mine if n not in snd]
            results.append(("burger sound events (dump is INCOMPLETE: %d of %d, lacks e.g. %s)" % (
                len(theirs), len(mine), ", ".join(missing[:3])), seq == sorted(seq),
                "the %d it has are in the same relative order; its numeric ids are not comparable" % len(theirs)
                if seq == sorted(seq) else "relative order differs as well"))
        else:
            results.append(_order_result("burger sound ids", mine, theirs))
    return results


def tags_vs_burger(tags, names_of, raw_of, path):
    """Burger resolves tags with its own code (from the client jar's copy of the data pack).

    ``names_of``: registry id -> list of entry names (index = the numeric id used in ``tags``).
    ``raw_of``:   registry id -> {tag id: [(is_tag, id, required)]} as written in the tag files.
    Burger keeps duplicates where vanilla's LinkedHashSet drops them, so its lists
    are de-duplicated (first occurrence wins) before comparing.
    """
    try:
        d = load_json(path)
    except (OSError, ValueError) as e:
        return [("burger tags", None, "cannot read %s: %s" % (path, e))]
    if isinstance(d, list) and d:
        d = d[0]
    bt = d.get("tags")
    if not isinstance(bt, dict):
        return [("burger tags", None, "the dump has no tags")]
    by_path = sorted(((short(rid), rid) for rid in tags), key=lambda x: -len(x[0]))
    same = 0
    differ = []
    unexpanded = []
    absent = []
    skipped = 0
    for key, body in bt.items():
        rid = None
        for pth, r in by_path:
            if key.startswith(pth + "/"):
                rid = r
                tid = "minecraft:" + key[len(pth) + 1:]
                break
        if rid is None:
            skipped += 1
            continue
        if tid not in tags[rid]:
            absent.append("%s %s" % (rid, tid))
            continue
        seen = set()
        theirs = []
        for v in body.get("values", []):
            if v not in seen:
                seen.add(v)
                theirs.append(v)
        mine = [names_of[rid][i] for i in tags[rid][tid]]
        if mine == theirs:
            same += 1
            continue
        direct = []
        for is_tag, ref, _required in raw_of(rid).get(tid, []):
            if not is_tag and ref not in direct:
                direct.append(ref)
        if theirs == direct:
            unexpanded.append("%s %s" % (rid, tid))     # Burger dropped the '#tag' references
        else:
            differ.append("%s %s" % (rid, tid))
    total = sum(len(t) for t in tags.values())
    rows = [("burger tags: resolved contents and element order", not differ and not absent,
             "%d of my %d tags identical (after de-duplicating Burger's lists); %d Burger tags belong to "
             "registries that are not sent" % (same, total, skipped) if not differ and not absent else
             "%d identical, %d differ %s, %d missing on my side %s" % (
                 same, len(differ), differ[:5], len(absent), absent[:5]))]
    if unexpanded:
        regs = sorted(set(u.split(" ")[0] for u in unexpanded))
        rows.append(("burger tags it could not resolve itself", None,
                     "%d tags of %s: Burger's list is exactly the tag file's direct elements, i.e. it did "
                     "not expand the '#tag' references there (e.g. %s)" % (len(unexpanded), regs, unexpanded[0])))
    return rows


# --------------------------------------------------------------------------
# older minecraft-data (sanity only)
# --------------------------------------------------------------------------
def _relative_order(mine, theirs):
    common = set(mine) & set(theirs)
    a = [n for n in mine if n in common]
    b = [n for n in theirs if n in common]
    pos = {n: i for i, n in enumerate(b)}
    seq = [pos[n] for n in a]
    # longest increasing subsequence = entries that keep their relative order
    import bisect
    tails = []
    for x in seq:
        i = bisect.bisect_left(tails, x)
        if i == len(tails):
            tails.append(x)
        else:
            tails[i] = x
    return len(common), len(tails)


def vs_minecraft_data(registries, blocks, directory):
    """Informational: an older version's ids differ, but the relative order mostly survives."""
    results = []
    for rid, fname in (("minecraft:block", "blocks.json"), ("minecraft:item", "items.json"),
                       ("minecraft:entity_type", "entities.json")):
        p = os.path.join(directory, fname)
        if not os.path.isfile(p) or rid not in registries:
            continue
        try:
            d = load_json(p)
            theirs = [e["name"] for e in sorted(d, key=lambda e: e["id"])]
        except (ValueError, KeyError, TypeError):
            continue
        common, kept = _relative_order([short(n) for n in registries[rid]], theirs)
        results.append(("%s vs %s" % (rid, os.path.basename(os.path.normpath(directory))), None,
                        "%d names in common, %d keep their relative order" % (common, kept)))
    p = os.path.join(directory, "blocks.json")
    if blocks and os.path.isfile(p):
        try:
            old = {e["name"]: e for e in load_json(p)}
            same = diff = 0
            examples = []
            for b in blocks:
                o = old.get(short(b["name"]))
                if o is None or b["hardness"] is None:
                    continue
                if abs(o.get("hardness", -99) - b["hardness"]) < 1e-6 and \
                        abs(o.get("resistance", -99) - b["blast_resistance"]) < 1e-6:
                    same += 1
                else:
                    diff += 1
                    if len(examples) < 5:
                        examples.append(short(b["name"]))
            results.append(("hardness/resistance vs older minecraft-data", None,
                            "%d common blocks identical, %d differ %s" % (same, diff, examples)))
        except (ValueError, KeyError, TypeError):
            pass
    return results
