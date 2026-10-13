"""How far past its structures does a base floor reach? Ask the minimap.

The hull of a base's structures is the right shape but stops short of
the ground, because the floor runs on past the inhibitor turrets to the
walls. The minimap texture draws that floor, and it is a different
colour from the open ground outside -- blue-grey against olive -- so the
distance from the hull to the edge of the floor can be measured rather
than guessed.

Measured per side, from the floor's own pixels.
The minimap is not committed -- viewer/README.md says how to fetch it
from Data Dragon.

Usage:

    python scripts/viewer_basepad.py --minimap viewer/map11.png         --regions base_vision.json
"""
from __future__ import annotations

import argparse
import json
import math
from collections import deque

import numpy as np
from PIL import Image

W = 512
MINX, MAXX, MINZ, MAXZ = -120, 14870, -120, 14980
UNITS_PER_PX = (MAXX - MINX) / W

def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--minimap", default="viewer/map11.png",
                    help="Riot's Summoner's Rift minimap, 512x512")
    ap.add_argument("--regions", default="base_vision.json",
                    help="the base regions to measure and rewrite in place")
    ap.add_argument("--cover", type=float, default=98.0,
                    help="percentile of floor to cover (default 98)")
    args = ap.parse_args()

    im = np.asarray(Image.open(args.minimap).convert("RGB")).astype(int)
    R, G, B = im[..., 0], im[..., 1], im[..., 2]
    # The base floor is the only large blue-grey field: blue at least as
    # strong as green, and clearly stronger than red. Open ground is olive
    # (blue lowest) and the background is near black.
    blue = (B >= G - 6) & (B > 100) & (R < B - 15)

    tx = lambda x: (x - MINX) / (MAXX - MINX) * W
    ty = lambda z: W - (z - MINZ) / (MAXZ - MINZ) * W
    wx = lambda c: MINX + (c + 0.5) / W * (MAXX - MINX)
    wz = lambda r: MINZ + (W - r - 0.5) / W * (MAXZ - MINZ)


    def flood(seed):
        """The blue field the fountain stands in, and nothing else blue."""
        seen = np.zeros_like(blue, bool)
        q = deque([seed])
        seen[seed] = True
        while q:
            r, c = q.popleft()
            for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                p = (r + dr, c + dc)
                if 0 <= p[0] < W and 0 <= p[1] < W and blue[p] and not seen[p]:
                    seen[p] = True
                    q.append(p)
        return seen


    def to_ring(ring):
        return [(float(x), float(z)) for x, z in ring]


    def dist_to_poly(pts, ring):
        """Distance from each point to the polygon; 0 for points inside."""
        pts = np.asarray(pts, float)
        best = np.full(len(pts), np.inf)
        inside = np.zeros(len(pts), bool)
        n = len(ring)
        for i in range(n):
            a, b = np.array(ring[i]), np.array(ring[(i + 1) % n])
            ab = b - a
            tt = np.clip(((pts - a) @ ab) / (ab @ ab), 0, 1)
            best = np.minimum(best, np.hypot(*(pts - (a + tt[:, None] * ab)).T))
            # ray cast for the inside test
            cond = (ring[i][1] > pts[:, 1]) != (ring[(i + 1) % n][1] > pts[:, 1])
            xint = (ring[(i + 1) % n][0] - ring[i][0]) * (pts[:, 1] - ring[i][1]) / \
                   (ring[(i + 1) % n][1] - ring[i][1] + 1e-12) + ring[i][0]
            inside ^= cond & (pts[:, 0] < xint)
        return np.where(inside, 0.0, best)


    regions = json.load(open(args.regions, encoding="utf-8"))
    FOUNTAIN = {"order": (420.0, 440.0), "chaos": (14340.0, 14390.0)}
    for reg in regions:
        f = FOUNTAIN[reg["team"]]
        seed = (int(ty(f[1])), int(tx(f[0])))
        if not blue[seed]:
            # The fountain pool is tinted its side's colour, so the seed has
            # to step off it onto the floor proper -- which on both sides is
            # the same blue-grey. The pool is wider than a few pixels, so
            # search outward in rings until the floor is reached.
            found = None
            for ring in range(1, 40):
                for dr in range(-ring, ring + 1):
                    for dc in (-ring, ring) if abs(dr) < ring else range(-ring, ring + 1):
                        r, c = seed[0] + dr, seed[1] + dc
                        if 0 <= r < W and 0 <= c < W and blue[r, c]:
                            found = (r, c)
                            break
                    if found:
                        break
                if found:
                    break
            assert found, f"no base floor near the {reg['team']} fountain"
            seed = found
        field = flood(seed)
        rs, cs = np.nonzero(field)
        pts = np.stack([wx(cs), wz(rs)], 1)
        d = dist_to_poly(pts, to_ring(reg["ring"]))
        out = d[d > 0]
        q = lambda p: float(np.percentile(out, p)) if len(out) else 0.0
        print(f"{reg['team']}: floor is {len(rs)} px "
              f"({len(rs) * UNITS_PER_PX ** 2 / 1e6:.2f}M sq units), "
              f"{len(out)} px outside the hull")
        print(f"   distance past the hull: median {q(50):.0f}  "
              f"90th {q(90):.0f}  98th {q(98):.0f}  max {out.max() if len(out) else 0:.0f}")
        was, reg["pad"] = reg["pad"], round(q(98) / 25) * 25
        print(f"   -> pad {reg['pad']:.0f} (was {was:.0f})")

        # What that pad actually does: how much of the floor it lights, and
        # how much ground outside the floor it lights by mistake. Measured
        # on the same grid the floor was measured on.
        rr, cc = np.nonzero(np.ones_like(blue, bool))
        allpts = np.stack([wx(cc), wz(rr)], 1)
        within = dist_to_poly(allpts, to_ring(reg["ring"])) <= reg["pad"]
        floor = field[rr, cc]
        print(f"   covers {within[floor].mean():6.1%} of the base floor; "
              f"{(~floor & within).sum() / max(within.sum(), 1):6.1%} of what it "
              f"lights is not floor")
    json.dump(regions, open(args.regions, "w", encoding="utf-8"),
              separators=(",", ":"))
    print(f"wrote {args.regions}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
