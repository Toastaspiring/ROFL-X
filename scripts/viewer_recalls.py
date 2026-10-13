"""Find recalls the way the game makes them: a stand-still, then the base.

A champion that recalls holds one spot for eight seconds -- four with
Baron's empowered recall -- and only then is moved to its fountain.
From the file, holding a spot and being unreported look the same: both
are silence in the movement class. So the signature is the silence that
ends at the fountain, and its length should land on eight or four
seconds rather than spread evenly, which is what would say this is the
channel and not just a gap.

Deaths are excluded, since a respawn also ends at the fountain, and
they are known exactly from the death class.
Inputs: pos1sr_t.jsonl, e1sr.json, table1619.json, deaths19.jsonl,
viewer/viewer_data.json. Writes recalls1619.json, which is merged with
the deaths from viewer_events.py into events1619.json.
"""
from __future__ import annotations

import argparse
import collections
import json
import math

FOUNTAIN = {"order": (420.0, 440.0), "chaos": (14340.0, 14390.0)}
PARK = {"order": (-632.0, -558.0), "chaos": (15362.0, 15398.0)}
AT_BASE = 1800.0
FAR = 2500.0

def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--positions", default="pos1sr_t.jsonl",
                    help="`explore --dump-positions` output")
    ap.add_argument("--meta", default="e1sr.json",
                    help="the `explore` report for the same replay")
    ap.add_argument("--table", default="table1619.json",
                    help="the movement class's substitution table")
    ap.add_argument("--deaths", default="deaths19.jsonl",
                    help="`explore --dump-netid <death class>` output")
    ap.add_argument("--viewer-data", default="viewer/viewer_data.json",
                    help="a build of the page's data, for the named units")
    ap.add_argument("--patch", default="patch_16_19",
                    help="which patch inside that file to read")
    ap.add_argument("--out", default="recalls1619.json",
                    help="where to write the recalls")
    ap.add_argument("--events", default="events1619.json",
                    help="the deaths from viewer_events.py, merged with "
                         "these recalls in place; --no-merge to skip")
    ap.add_argument("--no-merge", action="store_true",
                    help="write the recalls only, and leave the events alone")
    args = ap.parse_args()

    data = json.load(open(args.viewer_data, encoding="utf-8"))[args.patch]
    champs = {k: v for k, v in data["units"].items() if v.get("name")}
    first = int(json.load(open(args.meta, encoding="utf-8"))["replays"][0]["entities"]["champions"]["first"], 16)
    TAB = {int(p, 16): int(c, 16) for p, c in json.load(open(args.table, encoding="utf-8")).items()}
    INV = {c: p for p, c in TAB.items()}
    dead = collections.defaultdict(list)
    for line in open(args.deaths, encoding="utf-8"):
        r = json.loads(line)
        p = int(r["param"], 16) if isinstance(r["param"], str) else r["param"]
        if 0 <= p - first < 10:
            dead[p - first].append(r["t"])

    rows = collections.defaultdict(list)
    for line in open(args.positions, encoding="utf-8"):
        r = json.loads(line)
        if r["unit"] in champs:
            rows[r["unit"]].append(r)

    gaps, found = [], []
    for key, rec in champs.items():
        team = rec["team"]
        lo, hi = INV.get(int(key[0:2], 16)), INV.get(int(key[2:4], 16))
        idx = (0x40000000 | (hi << 8) | lo) - first if lo is not None else -1
        ds = sorted(dead.get(idx, []))
        # Only the corners are meaningless. The fountain sits on the map's
        # edge, so an arrival there often reads a few hundred units past it
        # -- that is the residual table error, not a bad event, and
        # demanding the arrival be strictly on the map throws away most
        # real recalls.
        v = sorted((r for r in rows[key]
                    if math.dist((r["x"], r["z"]), PARK[team]) > 900),
                   key=lambda r: r["t"])
        for a, b in zip(v, v[1:]):
            arrive = math.dist((b["x"], b["z"]), FOUNTAIN[team]) < AT_BASE
            leave = math.dist((a["x"], a["z"]), FOUNTAIN[team]) > FAR
            if not (arrive and leave):
                continue
            if any(a["t"] - 5 <= d <= b["t"] for d in ds):
                continue                      # a respawn, not a recall
            # It has to be a teleport, not a walk home. Standing still and
            # being out of sight look the same here, so a long silence would
            # otherwise let a champion stroll back to base and be counted.
            gap = b["t"] - a["t"]
            # The channel itself is a floor: eight seconds to recall, four
            # with Baron's empowered one. Anything shorter cannot be a
            # recall however much it looks like one.
            if gap < 3.5:
                continue
            walkable = max(a.get("speed") or 0, 340.0) * gap
            if math.dist((a["x"], a["z"]), (b["x"], b["z"])) < walkable:
                continue
            gaps.append(gap)
            found.append({"unit": key, "name": rec["name"], "team": team,
                          "kind": "recall", "t": round(b["t"], 2),
                          "channel": round(b["t"] - a["t"], 2),
                          "from": [round(a["x"]), round(a["z"])],
                          "to": [round(b["x"]), round(b["z"])]})

    gaps.sort()
    print(f"{len(gaps)} silences that start away from base and end at the fountain,"
          f" with no death to explain them:\n")
    buckets = collections.Counter(min(int(g), 20) for g in gaps)
    for sec in range(21):
        if buckets[sec]:
            bar = "#" * buckets[sec]
            mark = ("  <- Baron's 4s" if sec == 4 else
                    "  <- the 8s recall" if sec == 8 else "")
            print(f"   {sec:>2}-{sec+1:>2}s {bar} ({buckets[sec]}){mark}")
    q = lambda p: gaps[int(len(gaps) * p)] if gaps else 0
    print(f"\n   median {q(.5):.1f}s")
    in8 = sum(1 for g in gaps if 7.5 <= g < 9.5)
    in4 = sum(1 for g in gaps if 3.5 <= g < 5.5)
    print(f"   {in8} land in 7.5-9.5s, {in4} in 3.5-5.5s, "
          f"{len(gaps) - in8 - in4} elsewhere")
    per = collections.Counter(f["name"] for f in found)
    print(f"\n   per champion: {dict(per.most_common())}")
    json.dump(found, open(args.out, "w", encoding="utf-8"),
              separators=(",", ":"))
    print(f"wrote {len(found)} recalls to {args.out}")

    if not args.no_merge:
        try:
            have = json.load(open(args.events, encoding="utf-8"))
        except FileNotFoundError:
            have = []
        deaths = [e for e in have if e["kind"] != "recall"]
        json.dump(deaths + found, open(args.events, "w", encoding="utf-8"),
                  separators=(",", ":"))
        print(f"merged into {args.events}: {len(deaths)} deaths "
              f"and {len(found)} recalls")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
