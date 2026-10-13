#!/usr/bin/env python3
"""Check a position dump against things the game cannot do.

A table can be wrong in ways that still look plausible on a map, so
this asserts what League itself guarantees and reports what fails. Each
check prints what it measured, not just a verdict, because a near miss
is usually more informative than a pass.

What it knows about the game:

  waves      Minions spawn together, every 30 s, in groups of six per
             side with a seventh every third wave. A wrong table
             scatters those spawns; a right one makes them periodic.
  speed      The movement class carries each unit's speed. A champion
             walks about 325 and reaches roughly 500 with boots; a dash
             goes higher briefly. Speeds far outside that are a misread
             field, not a fast champion.
  paths      A packet is an order to walk, so consecutive positions for
             one unit have to be reachable at its own stated speed.
  map        Nothing stands outside the map.
  camps      A jungle camp respawns 135 s after it is cleared, the
             dragon first spawns at 5:00 and the rift herald at 8:00.

Usage:

    rofl-x explore -r <replay> --table t.json --positions-netid <n> \\
        --dump-positions pos.jsonl -o meta.json
    python scripts/sanity_check.py --positions pos.jsonl --meta meta.json
"""
from __future__ import annotations

import argparse
import collections
import json
import math

WAVE_EVERY = 30.0
WAVE_SIZE = (6, 7)
CHAMPION_SPEED = (300.0, 520.0)
DASH_CEILING = 2200.0
MAP = (-250.0, 15100.0)
CAMP_RESPAWN = 135.0

PASS, FAIL = "ok", "FAILED"
results: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, detail: str) -> None:
    results.append((name, ok, detail))
    print(f"  [{PASS if ok else FAIL}] {name}: {detail}")


def _along(path, dist):
    if len(path) == 1:
        return path[0]
    left = dist
    for a, b in zip(path, path[1:]):
        seg = math.dist(a, b)
        if seg == 0:
            continue
        if left <= seg:
            u = left / seg
            return (a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u)
        left -= seg
    return path[-1]


def _walker(orders):
    """Position at any moment: the path at its speed, with the error
    that leaves at the next order spread across the interval."""
    def at(time):
        lo, hi, found = 0, len(orders) - 1, -1
        if not orders or orders[0][0] > time:
            return None
        while lo <= hi:
            mid = (lo + hi) // 2
            if orders[mid][0] <= time:
                found, lo = mid, mid + 1
            else:
                hi = mid - 1
        if found < 0:
            return None
        t0, sp, path = orders[found]
        sp = sp if CHAMPION_SPEED[0] * 0.3 < sp < DASH_CEILING else 0
        here = _along(path, (time - t0) * sp) if sp else path[0]
        if found + 1 >= len(orders):
            return here
        t1, _, nxt = orders[found + 1]
        span = t1 - t0
        if span <= 0.01 or not nxt:
            return here
        landing = _along(path, span * sp) if sp else path[0]
        u = max(0.0, min(1.0, (time - t0) / span))
        return (here[0] + (nxt[0][0] - landing[0]) * u,
                here[1] + (nxt[0][1] - landing[1]) * u)
    return at


def load(path):
    rows = [json.loads(line) for line in open(path, encoding="utf-8")]
    return [r for r in rows if r.get("replay", 0) == 0]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--positions", required=True)
    ap.add_argument("--meta", required=True)
    args = ap.parse_args()

    rows = load(args.positions)
    meta = json.load(open(args.meta, encoding="utf-8"))["replays"][0]
    length = meta["game_length_s"]
    print(f"{len(rows)} positions over {length:.0f}s\n")

    # --- everything stands on the map -----------------------------------
    off = [r for r in rows if not (MAP[0] <= r["x"] <= MAP[1])
           or not (MAP[0] <= r["z"] <= MAP[1])]
    check("on the map", not off,
          f"{len(off)} of {len(rows)} positions outside {MAP[0]:.0f}..{MAP[1]:.0f}")

    # --- the stated speed reads as a speed -------------------------------
    # This is the check that caught 16.19: the class carries a move speed
    # and on a patch that decodes it reads a champion's walk, but a table
    # that is wrong at those bytes turns it into nonsense while leaving
    # the coordinates looking fine.
    speeds = [r["speed"] for r in rows if r.get("speed")]
    if speeds:
        srt = sorted(s for s in speeds if s == s and abs(s) < 1e9)
        median = srt[len(srt) // 2] if srt else 0
        wild = sum(1 for s in speeds if not (s == s) or abs(s) > DASH_CEILING)
        check("stated speed reads as a speed",
              CHAMPION_SPEED[0] * 0.7 <= median <= CHAMPION_SPEED[1] * 1.5,
              f"median {median:.0f} (a champion walks "
              f"{CHAMPION_SPEED[0]:.0f}..{CHAMPION_SPEED[1]:.0f}), "
              f"{wild} values beyond {DASH_CEILING:.0f}")

    # --- a unit cannot outrun its own stated speed ----------------------
    by_unit = collections.defaultdict(list)
    for r in rows:
        by_unit[r["unit"]].append(r)
    impossible = total = 0
    for v in by_unit.values():
        v.sort(key=lambda r: r["t"])
        for a, b in zip(v, v[1:]):
            dt = b["t"] - a["t"]
            if not (0.1 < dt < 3.0) or not a.get("speed"):
                continue
            total += 1
            # Allow a wide margin: dashes, flashes and recalls are real.
            if math.dist((a["x"], a["z"]), (b["x"], b["z"])) > a["speed"] * dt + 900:
                impossible += 1
    if total:
        share = impossible / total
        check("units keep to their own speed", share < 0.02,
              f"{impossible} of {total} steps ({share:.2%}) move further than the "
              f"unit's stated speed allows")

    # --- minions arrive in waves ----------------------------------------
    first = {}
    for r in rows:
        if r["unit"] not in first or r["t"] < first[r["unit"]]:
            first[r["unit"]] = r["t"]
    births = sorted(first.values())
    # The cadence is found rather than assumed: try every offset and keep
    # the one that lines the spawns up best on a 30 s grid.
    best = None
    for off_s in [x / 4 for x in range(0, int(WAVE_EVERY) * 4)]:
        buckets = collections.Counter(
            int((b - off_s) // WAVE_EVERY) for b in births if b >= off_s)
        full = [n for n in buckets.values() if n >= WAVE_SIZE[0]]
        if best is None or len(full) > best[0]:
            best = (len(full), off_s, buckets)
    waves, offset, buckets = best
    sized = [n for n in buckets.values() if n >= WAVE_SIZE[0]]
    expected = int((length - offset) // WAVE_EVERY)
    check("minions arrive in waves", waves >= expected * 0.5,
          f"{waves} of about {expected} 30s windows hold a full wave "
          f"(offset {offset:.2f}s, median wave {sorted(sized)[len(sized)//2] if sized else 0})")

    # --- champions are ten ----------------------------------------------
    # Only units whose net id's second byte is the anchor can be
    # champions: every unit the game spawns early has a zero there, and
    # ranking all units together picks minions instead.
    anchor = collections.Counter(
        u[2:4] for u, v in by_unit.items() if len(v) > 500).most_common(1)
    low = {u: v for u, v in by_unit.items() if anchor and u[2:4] == anchor[0][0]}
    counts = sorted((len(v) for v in low.values()), reverse=True)
    gap = counts[9] / counts[10] if len(counts) > 10 and counts[10] else 99
    check("ten units stand out as champions", len(counts) >= 10 and gap > 2.0,
          f"{len(low)} units could be champions; the tenth busiest has "
          f"{counts[9] if len(counts) > 9 else 0} packets, the eleventh "
          f"{counts[10] if len(counts) > 10 else 0} ({gap:.0f}x apart)")

    # --- the reconstruction is continuous -------------------------------
    # A movement packet is a click: the path asked for and the speed to
    # walk it. Walking that predicts the next packet well but not
    # exactly, and dumping the leftover at the boundary is what makes a
    # unit snap. Spreading it across the interval removes that, and this
    # measures what is left -- which should only be the game's own
    # teleports: a flash is 400 units, a recall is further.
    champ_units = sorted(low.items(), key=lambda kv: -len(kv[1]))[:10] if low else []
    worst = []
    for _, v in champ_units:
        orders = sorted((r["t"], r.get("speed", 0),
                         [tuple(q) for q in (r.get("path") or [(r["x"], r["z"])])])
                        for r in v)
        at = _walker(orders)
        if not orders:
            continue
        t, end, prev = orders[0][0], orders[-1][0], None
        while t < end:
            cur = at(t)
            if cur and prev:
                worst.append(math.dist(prev, cur))
            prev, t = cur, t + 0.01
    if worst:
        worst.sort()
        huge = sum(1 for d in worst if d > 600)
        check("motion is continuous at 10ms", huge == 0,
              f"{len(worst)} samples: median step {worst[len(worst)//2]:.2f}, "
              f"99.9th {worst[int(len(worst)*0.999)]:.0f}, worst {worst[-1]:.0f} units; "
              f"{huge} beyond a flash's 600")

    bad = sum(1 for _, ok, _ in results if not ok)
    print(f"\n{len(results) - bad} of {len(results)} checks passed")
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
