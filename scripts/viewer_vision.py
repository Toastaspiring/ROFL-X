"""What the fountain turret lights: the base, as the base's own shape.

The turret at the spawn point sees far enough to cover the whole base.
A circle is the wrong way to say that -- the base is a triangle with
three lane mouths, and a circle wide enough to reach the far corner
spills into the river and the jungle, lighting ground the team cannot
actually see.

So the region is taken from the base's own structures instead. The
fountain, the nexus, its two turrets, the three inhibitors and their
turrets stand at the base's corners; their convex hull is the base, and
the renderer widens it to cover the ground between them and the walls.

Nothing here is measured from the replay. The structures' positions are,
the shape follows from them, and the widening is a drawing choice.
The `pad` it writes is a placeholder; viewer_basepad.py measures it
from the minimap and rewrites it.

Usage:

    python scripts/viewer_vision.py --objects objects1619.json         --towers static_vision.json --regions base_vision.json
"""
from __future__ import annotations

import argparse
import json

PAD = 650.0          # a placeholder; viewer_basepad.py measures it
TOWER_SIGHT = 1350.0  # the game's own value, same as a champion's
BASE_PARTS = ("nexus", "inhibitor")
FOUNTAIN = {"order": (420.0, 440.0), "chaos": (14340.0, 14390.0)}


def hull(points):
    """Andrew's monotone chain, counter-clockwise."""
    pts = sorted(set(points))
    if len(pts) < 3:
        return pts
    cross = lambda o, a, b: ((a[0] - o[0]) * (b[1] - o[1])
                             - (a[1] - o[1]) * (b[0] - o[0]))
    half = []
    for p in pts:
        while len(half) >= 2 and cross(half[-2], half[-1], p) <= 0:
            half.pop()
        half.append(p)
    lower, half = half, []
    for p in reversed(pts):
        while len(half) >= 2 and cross(half[-2], half[-1], p) <= 0:
            half.pop()
        half.append(p)
    return lower[:-1] + half[:-1]


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--objects", default="objects1619.json",
                    help="the map's structures, camps and pits")
    ap.add_argument("--towers", default="static_vision.json",
                    help="where to write one vision source per turret")
    ap.add_argument("--regions", default="base_vision.json",
                    help="where to write one lit region per side")
    ap.add_argument("--sight", type=float, default=TOWER_SIGHT,
                    help=f"how far a turret sees (default {TOWER_SIGHT:.0f})")
    args = ap.parse_args()

    objs = json.load(open(args.objects, encoding="utf-8"))
    towers = [{"x": o["x"], "z": o["z"], "team": o["team"],
               "sight": args.sight, "what": "tower"}
              for o in objs if o.get("team") and o["kind"] == "tower"]

    regions = []
    for team, f in FOUNTAIN.items():
        pts = [f]
        for o in objs:
            if o.get("team") != team:
                continue
            # The base is what stands behind the inhibitors: the nexus,
            # the inhibitors themselves, and the turrets guarding both.
            if o["kind"] in BASE_PARTS or (o["kind"] == "tower"
                                           and ("nexus" in o["name"]
                                                or "inhib" in o["name"])):
                pts.append((o["x"], o["z"]))
        ring = hull(pts)
        regions.append({"team": team, "pad": PAD,
                        "ring": [[round(x, 1), round(z, 1)] for x, z in ring]})
        print(f"{team}: base from {len(pts)} structures -> {len(ring)} corners")

    json.dump(towers, open(args.towers, "w", encoding="utf-8"),
              separators=(",", ":"))
    json.dump(regions, open(args.regions, "w", encoding="utf-8"),
              separators=(",", ":"))
    print(f"wrote {len(towers)} tower sources to {args.towers} and "
          f"{len(regions)} base regions to {args.regions}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
