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
Input: objects1619.json. Writes static_vision.json (one entry per
turret) and base_vision.json (one region per side). The region's `pad`
is a placeholder until viewer_basepad.py measures it.
"""
import json

PAD = 650.0          # how far past the structures the base floor reaches
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


objs = json.load(open("objects1619.json"))
regions, towers = [], []
for o in objs:
    if o.get("team") and o["kind"] == "tower":
        towers.append({"x": o["x"], "z": o["z"], "team": o["team"],
                       "sight": 1350, "what": "tower"})

for team, f in FOUNTAIN.items():
    pts = [f]
    for o in objs:
        if o.get("team") != team:
            continue
        # The base is what stands behind the inhibitors: the nexus, the
        # inhibitors themselves, and the turrets guarding both.
        if o["kind"] in BASE_PARTS or (o["kind"] == "tower"
                                       and ("nexus" in o["name"]
                                            or "inhib" in o["name"])):
            pts.append((o["x"], o["z"]))
    ring = hull(pts)
    regions.append({"team": team, "pad": PAD,
                    "ring": [[round(x, 1), round(z, 1)] for x, z in ring]})
    print(f"{team}: base from {len(pts)} structures -> {len(ring)} corners")

json.dump(towers, open("static_vision.json", "w"), separators=(",", ":"))
json.dump(regions, open("base_vision.json", "w"), separators=(",", ":"))
print(f"wrote {len(towers)} tower sources and {len(regions)} base regions")
