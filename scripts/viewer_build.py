"""Build the viewer's dataset: named champions, and every other entity.

A unit is named by decoding its net id with the class's table and
looking the id up in the champion run, which `explore` finds from the
framing. That works wherever the table reaches those bytes; where it
does not, the unit keeps its obfuscated key and its team is inferred
from which fountain it visits.

Minions, monsters and the rest come along as a thinned layer. They are
the best evidence in the file that the positions are real: they walk
the three lanes and sit in the camps, and no table error produces that
by accident.
Inputs, all produced by the steps in `viewer/README.md` and read from the
working directory by these fixed names:

    pos1sr_t.jsonl   `explore --dump-positions` for the 16.19 replay
    e1sr.json        the `explore` report for it
    pos916.jsonl     the same for the 16.9 control
    e9meta.json      its report
    table1619.json   the movement class's table, 16.19
    true916_table.json              the same for 16.9
    objects1619.json camps1619.json lit1619.json fog1619.json
    owners1619.json  named1619.json static_vision.json base_vision.json
    events1619.json  deaths and recalls, from viewer_events.py and
                     viewer_recalls.py

Writes `viewer/viewer_data.json`. Pass any argument to use the solved
16.19 table; without one it falls back to the unsolved low byte.
"""
from __future__ import annotations

import argparse
import collections
import json
import math
import os

ORDER, CHAOS = (400.0, 400.0), (14400.0, 14400.0)
ENTITY_CAP = 30000

def build(pos_path, meta_path, table_path, anchor, label, accuracy,
          extras=False, wards=None):
    meta = json.load(open(meta_path))["replays"][0]
    roster = [p["champion"] for p in meta["players"]]
    first = int(meta["entities"]["champions"]["first"], 16)
    inv = {}
    if table_path:
        tab = {int(p, 16): int(c, 16) for p, c in json.load(open(table_path)).items()}
        inv = {c: p for p, c in tab.items()}
    units = collections.defaultdict(list)
    on_map = lambda p: -250.0 <= p[0] <= 15100.0 and -250.0 <= p[1] <= 15100.0
    for line in open(pos_path, encoding="utf-8"):
        r = json.loads(line)
        if r["replay"] != 0:
            continue
        # Nothing stands off the map. A position there is either the
        # stream declining to report a unit -- it parks one at its own
        # corner while the unit is dead or out of sight, and 93% of the
        # moments a champion sits there alive are moments the enemy
        # could not see it -- or a misread. Either way it is not where
        # the unit is, and drawing it throws the unit across the map
        # and back.
        if not on_map((r["x"], r["z"])):
            continue
        # A path is only as good as its points. The parser checks the
        # first one against the map, but a later one can still be a
        # misread -- 1% of all path points land off it -- and walking
        # into one drags the unit off the map with it. So the path stops
        # at its first bad point.
        path = r.get("path") or [[r["x"], r["z"]]]
        good = 1
        while good < len(path) and on_map(path[good]):
            good += 1
        units[r["unit"]].append((r["t"], r["x"], r["z"], r["accuracy_map_units"],
                                 r.get("speed", 0), path[:good]))
    # `anchor` is T(0x00): net id byte 1 is zero for every unit the game
    # spawns early, champions among them, so its cipher marks them out.
    # It is given rather than guessed -- the commonest second byte across
    # all units is a minion's, not a champion's.
    for v in units.values():
        v.sort()

    low = {u: v for u, v in units.items() if int(u[2:4], 16) == anchor}
    champs = sorted(low.items(), key=lambda kv: -len(kv[1]))[:10]
    out = {}
    for u, v in champs:
        lo, hi = inv.get(int(u[0:2], 16)), inv.get(int(u[2:4], 16))
        name, team = None, None
        if lo is not None and hi is not None:
            idx = (0x40000000 | (hi << 8) | lo) - first
            if 0 <= idx < len(roster):
                name = roster[idx]
                team = "order" if idx < len(roster) // 2 else "chaos"
        if team is None:
            no = sum(1 for p in v if math.dist((p[1], p[2]), ORDER) < 2000)
            nc = sum(1 for p in v if math.dist((p[1], p[2]), CHAOS) < 2000)
            team = "order" if no > nc else "chaos" if nc > no else "unknown"
        # Orders, not samples. A packet is an instruction to walk a
        # path at a speed, so keeping the order lets the viewer put the
        # unit where it actually was between packets instead of
        # snapping it from one reported point to the next.
        orders = [[round(t, 2), round(sp), [[round(px), round(pz)] for px, pz in path]]
                  for t, x, z, acc, sp, path in v]
        thin, last = [], -99.0
        for t, x, z, acc, sp, path in v:
            if t - last >= 1.0:
                thin.append([round(t, 1), round(x), round(z), acc])
                last = t
        out[u] = {"name": name, "team": team, "packets": len(v),
                  "named_from": "net id" if name else "fountain only",
                  "track": thin, "orders": orders}

    # Everything that is not a champion, carrying whose side it is so the
    # viewer can colour it. Which side is read from the visibility
    # packets and from where the unit was born (see ownteam2).
    owners, inv = {}, {}
    try:
        owners = {int(k, 16): v for k, v in
                  json.load(open(args.owners, encoding="utf-8")).items()}
        tab = {int(p, 16): int(c, 16) for p, c in
               json.load(open(args.table, encoding="utf-8")).items()}
        inv = {c: p for p, c in tab.items()}
    except FileNotFoundError:
        pass

    def side(unit):
        lo, hi = inv.get(int(unit[0:2], 16)), inv.get(int(unit[2:4], 16))
        if lo is None or hi is None:
            return 0
        return {"order": 1, "chaos": 2}.get(
            owners.get(0x40000000 | (hi << 8) | lo), 0)

    picked = set(u for u, _ in champs)
    others = [(p[0], p[1], p[2], side(u)) for u, v in units.items()
              if u not in picked for p in v]
    step = max(1, len(others) // ENTITY_CAP)
    ents = [[round(t, 1), round(x), round(z), s] for t, x, z, s in others[::step]]

    # The map furniture, the fog and the events are only built for the
    # patch they were derived from. Which one that is used to be guessed
    # from the positions file's name; it is now said outright.
    extra = {}
    if extras:
        fog = load(args.fog, {})
        for rec in out.values():
            if rec["name"] in fog:
                rec["seen_by"] = {"order": fog[rec["name"]]["by_order"],
                                  "chaos": fog[rec["name"]]["by_chaos"]}
        extra["fog"] = bool(fog)
        extra["camps"] = load(args.camps, [])
        # Positions of the units each side owns: the ground it lights.
        extra["lit"] = load(args.lit, {})
        extra["objects"] = load(args.objects, [])
        # Vision from the things that never move.
        extra["static_vision"] = load(args.towers, [])
        extra["base_vision"] = load(args.regions, [])
        # Deaths and recalls, so the page can stop drawing a champion
        # that is not on the field.
        extra["events"] = load(args.events, [])
    else:
        extra = {"fog": False, "camps": [], "lit": {}, "objects": [],
                 "static_vision": [], "base_vision": [], "events": []}
    # Wards are read outright wherever the patch has an archive, so they
    # are attached per patch rather than with the rest.
    extra["wards"] = load(wards, []) if wards else []
    return {"label": label, "accuracy": accuracy, "roster": roster,
            "units": out, "entities": ents, "entity_total": len(others), **extra}

def load(path, empty):
    """A file the page can do without, or what to use when it is absent."""
    try:
        return json.load(open(path, encoding="utf-8"))
    except FileNotFoundError:
        return empty


def main() -> int:
    global args
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_argument_group("the patch under judgement")
    g.add_argument("--positions", default="pos1sr_t.jsonl",
                   help="`explore --dump-positions` output")
    g.add_argument("--meta", default="e1sr.json", help="its `explore` report")
    g.add_argument("--table", default="table1619.json",
                   help="the movement class's table; omit if it is unsolved")
    g.add_argument("--label", default="16.19 — read from the file alone")

    g = ap.add_argument_group("the control, decoded by the game's own binary")
    g.add_argument("--control-positions", default="pos916.jsonl")
    g.add_argument("--control-meta", default="e9meta.json")
    g.add_argument("--control-table", default="true916_table.json")
    g.add_argument("--control-wards", default="wards916.json",
                   help="wards from viewer_wards.py; a patch with an "
                        "archive decodes them outright")
    g.add_argument("--control-label",
                   default="16.9 — control, verified identical to the game's own decoder")

    g = ap.add_argument_group("what else the page draws, all optional")
    g.add_argument("--owners", default="owners1619.json")
    g.add_argument("--fog", default="fog1619.json")
    g.add_argument("--camps", default="camps1619.json")
    g.add_argument("--lit", default="lit1619.json")
    g.add_argument("--objects", default="objects1619.json")
    g.add_argument("--towers", default="static_vision.json")
    g.add_argument("--regions", default="base_vision.json")
    g.add_argument("--events", default="events1619.json")

    ap.add_argument("--out", default="viewer/viewer_data.json")
    args = ap.parse_args()

    if not os.path.exists(args.table):
        print(f"no table at {args.table}: the low byte stays unknown, "
              f"so positions carry ±256 map units")
        args.table = None

    tab9 = {int(p, 16): int(c, 16) for p, c in
            json.load(open(args.control_table, encoding="utf-8")).items()}
    anchor = int(json.load(open(args.meta, encoding="utf-8"))
                 ["waypoint_table"]["anchor"], 16)

    data = {
        "patch_16_9": build(args.control_positions, args.control_meta,
                            args.control_table, tab9[0x00],
                            args.control_label, "exact", extras=False,
                            wards=args.control_wards),
        "patch_16_19": build(args.positions, args.meta, args.table, anchor,
                             args.label,
                             "high byte exact, low byte searched" if args.table
                             else "high byte exact, low byte unknown: "
                                  "±256 map units",
                             extras=True, wards=None),
    }
    for k, v in data.items():
        named = sum(1 for u in v["units"].values() if u["name"])
        print(f"{k}: {named}/10 named, {len(v['entities'])} entity dots "
              f"of {v['entity_total']}, teams "
              f"{collections.Counter(u['team'] for u in v['units'].values())}")
    json.dump(data, open(args.out, "w", encoding="utf-8"),
              separators=(",", ":"))
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
