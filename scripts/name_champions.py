#!/usr/bin/env python3
"""Name a packed patch's champions, and get table entries for doing it.

The movement class calls a unit by its net id, which sits behind the
class's substitution. Where the table reaches those bytes the id
decodes and the champion run names the champion outright — that is the
mechanism, and on a patch that decodes it names all ten with nothing
else needed.

Where the table does *not* reach them yet, the replay still says who is
who, through three things it gives up in plaintext:

  deaths   The death class fires once per death and addresses its
           victim, and a block's `param` is plaintext. So every
           champion's exact death times are readable with no table at
           all, and a champion that dies stops sending waypoints for
           the death timer. Matching the two names a unit. It says
           nothing about a champion that never dies — one survivor is
           fine, since the assignment fills the last place by
           elimination, but two is not.

  lane     statsJson gives every player a position, and early in a game
           a player is in that lane. Always available; blind to the
           difference between a bottom carry and its support, who share
           a lane.

  side     In any lane the blue-side player stands nearer the blue
           base. Always available, and unharmed by a constant error in
           the coordinate origin, because both players in a lane shift
           together — which matters, since 16.19 has one.

No one of them is enough, so all three are scored and one assignment is
solved over the sum. Where they disagree the margin shows it.

The point is not the labels. A named champion hands over its net id's
low byte, which is an exact table entry at a plaintext the high-byte
chain never reaches — so this bootstraps the table that then makes it
unnecessary. The constant-speed search cannot find those bytes on its
own: it only constrains ciphers that appear as *coordinates*, and a
cipher appearing as a net id is invisible to it.

Usage:

    rofl-x explore -r <replay> --dump-positions pos.jsonl -o meta.json
    rofl-x explore -r <replay> --dump-netid <death netid> --dump deaths.jsonl
    python scripts/name_champions.py --positions pos.jsonl --meta meta.json \\
        --deaths deaths.jsonl --table table.json
"""
from __future__ import annotations

import argparse
import collections
import json
import math

# The three lanes, as the polylines a laner walks. Bottom and utility
# share one, which is why side and deaths have to finish the job.
LANES = {
    "TOP": [(1000, 1800), (1000, 12400), (12400, 13600)],
    "BOTTOM": [(1800, 1000), (12400, 1000), (13600, 12400)],
    "MIDDLE": [(1900, 1900), (12900, 12900)],
}
# The lane each role is played in. The report spells roles its own way,
# and a support shares the bottom lane with the carry it protects, which
# is why lane alone can never separate those two.
LANE_OF_ROLE = {
    "TOP": "TOP",
    "JUNGLE": "JUNGLE",
    "MID": "MIDDLE", "MIDDLE": "MIDDLE",
    "ADC": "BOTTOM", "BOTTOM": "BOTTOM",
    "SUPPORT": "BOTTOM", "UTILITY": "BOTTOM",
}
# Further than this from every lane and a unit is in the jungle.
JUNGLE_AT = 1700.0
# A champion is in its lane between these times; before, everyone is
# walking out of the fountain, and after, lanes break down.
EARLY = (90.0, 420.0)
# A death timer is at least six seconds, so a champion that died is
# silent across this window where a living one would still be moving.
AFTER_DEATH = (1.5, 6.0)
BEFORE_DEATH = 6.0
# Deaths are the sharpest signal where they exist; lane and side carry
# a champion that never dies.
W_DEATH, W_LANE, W_SIDE = 1.6, 1.0, 0.7


def seg_distance(p, a, b) -> float:
    vx, vy = b[0] - a[0], b[1] - a[1]
    wx, wy = p[0] - a[0], p[1] - a[1]
    length = vx * vx + vy * vy
    t = 0.0 if length == 0 else max(0.0, min(1.0, (wx * vx + wy * vy) / length))
    return math.hypot(wx - t * vx, wy - t * vy)


def lane_of(points) -> str:
    """The lane a unit sits in, by its median distance to each."""
    best = {}
    for name, poly in LANES.items():
        ds = sorted(
            min(seg_distance(p, poly[i], poly[i + 1]) for i in range(len(poly) - 1))
            for p in points
        )
        best[name] = ds[len(ds) // 2]
    lane = min(best, key=best.get)
    return "JUNGLE" if best[lane] > JUNGLE_AT else lane


def died_then(times, at: float) -> bool:
    """Moving just before `at`, silent just after: a death."""
    return any(at - BEFORE_DEATH <= t <= at + 0.5 for t in times) and not any(
        at + AFTER_DEATH[0] <= t <= at + AFTER_DEATH[1] for t in times
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--positions", required=True, help="--dump-positions JSONL")
    ap.add_argument("--meta", required=True, help="the explore report for the replay")
    ap.add_argument("--deaths", help="--dump-netid JSONL for the death class")
    ap.add_argument("--table", required=True, help="the movement class's table")
    ap.add_argument("--out", help="write the net id entries this pins, as JSON")
    args = ap.parse_args()

    meta = json.load(open(args.meta, encoding="utf-8"))["replays"][0]
    roster = [p["champion"] for p in meta["players"]]
    roles = [LANE_OF_ROLE.get(str(p.get("role", "")).upper(), "") for p in meta["players"]]
    first = int(meta["entities"]["champions"]["first"], 16)
    table = {int(p, 16): int(c, 16) for p, c in
             json.load(open(args.table, encoding="utf-8")).items()}
    anchor = table[0x00]

    times, early = collections.defaultdict(list), collections.defaultdict(list)
    for line in open(args.positions, encoding="utf-8"):
        r = json.loads(line)
        if int(r["unit"][2:4], 16) != anchor:
            continue
        times[r["unit"]].append(r["t"])
        if EARLY[0] <= r["t"] <= EARLY[1]:
            early[r["unit"]].append((r["x"], r["z"]))
    units = [u for u, _ in sorted(times.items(), key=lambda kv: -len(kv[1]))[:10]]
    if len(units) < 10:
        print(f"only {len(units)} champion-shaped units; needs ten")
        return 1
    for u in units:
        times[u].sort()

    deaths: dict[int, list[float]] = collections.defaultdict(list)
    if args.deaths:
        for line in open(args.deaths, encoding="utf-8"):
            r = json.loads(line)
            p = int(r["param"], 16) if isinstance(r["param"], str) else r["param"]
            if 0 <= p - first < 10:
                deaths[p - first].append(r["t"])

    lanes = {u: lane_of(early[u]) if len(early[u]) >= 20 else "?" for u in units}
    along = {u: sum(x + z for x, z in early[u]) / max(len(early[u]), 1) for u in units}
    by_lane = collections.defaultdict(list)
    for u in units:
        by_lane[lanes[u]].append((along[u], u))
    side = {}
    for group in by_lane.values():
        group.sort()
        for i, (_, u) in enumerate(group):
            side[u] = "order" if i < len(group) // 2 else "chaos"

    try:
        from scipy.optimize import linear_sum_assignment
        import numpy as np
    except ImportError:
        print("needs numpy and scipy")
        return 1

    cost = np.zeros((10, 10))
    detail = {}
    for i, u in enumerate(units):
        for idx in range(10):
            want_lane = roles[idx] if idx < len(roles) else ""
            ds = deaths.get(idx, [])
            death = sum(died_then(times[u], d) for d in ds) / len(ds) if ds else 0.0
            lane_ok = bool(want_lane) and lanes[u] == want_lane
            side_ok = side[u] == ("order" if idx < 5 else "chaos")
            cost[i, idx] = W_DEATH * death + W_LANE * lane_ok + W_SIDE * side_ok
            detail[(u, idx)] = (death, lane_ok, side_ok)
    rows, cols = linear_sum_assignment(-cost)

    print(f"  {'unit':>6} {'champion':<14} {'deaths':>7} {'lane':>5} {'side':>5} {'margin':>7}")
    entries, named = {}, {}
    for i, idx in zip(rows, cols):
        u = units[i]
        death, lane_ok, side_ok = detail[(u, idx)]
        ranked = sorted(cost[i], reverse=True)
        print(f"  {u:>6} {roster[idx]:<14} {death * 100:>6.0f}% "
              f"{'yes' if lane_ok else 'no':>5} {'yes' if side_ok else 'no':>5} "
              f"{ranked[0] - ranked[1]:>7.2f}")
        entries[f"{(first + idx) & 0xFF:02x}"] = f"{int(u[0:2], 16):02x}"
        named[u] = roster[idx]

    inverse = {c: p for p, c in table.items()}
    agree = 0
    for u, idx in zip(units, cols[np.argsort(rows)]):
        lo, hi = inverse.get(int(u[0:2], 16)), inverse.get(int(u[2:4], 16))
        if lo is not None and hi is not None:
            agree += (0x40000000 | (hi << 8) | lo) - first == idx
    print(f"\n  the table's own net id decode agrees with {agree} of 10")
    print("  (ten means the table already names them and this was not needed;"
          " fewer means these entries are worth feeding back as anchors)")
    if args.out:
        json.dump(entries, open(args.out, "w", encoding="utf-8"))
        print(f"  wrote {len(entries)} net id entries to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
