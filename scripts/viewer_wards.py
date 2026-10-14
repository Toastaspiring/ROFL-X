#!/usr/bin/env python3
"""Wards, for a patch that decodes.

`rofl-x file` already reads them where an archive exists: name, owner,
position and how long each one lived, checked against the replay's own
`WARD_PLACED` and passing 10/10. This only reshapes that for the page,
which needs a ward to appear when it is placed and disappear when it
dies rather than sit on the map for the whole game.

A ward with no duration was never destroyed and lasted to the final
whistle, so it is given the rest of the game.

The three names the game uses are its own: `YellowTrinket` is the free
totem every champion carries, `SightWard` the stealth ward bought from
the shop, and `JammerDevice` the control ward -- the pink one, which
also reveals and so gets its own icon.

Usage:

    rofl-x file -r <replay>.rofl --patch-dir ./patch -o decoded.json
    python scripts/viewer_wards.py --decoded decoded.json --out wards.json
"""
from __future__ import annotations

import argparse
import collections
import json

# What the game calls them, and what the page should draw.
KIND = {
    "YellowTrinket": "ward",
    "SightWard": "ward",
    "JammerDevice": "controlWard",
}
SIDE = {"Blue": "order", "Red": "chaos"}
# A ward sees 900 units, the same whether it is a totem, a stealth ward
# or a control ward (League wiki, Sight). A farsight totem sees 500 and
# is not one of these three.
WARD_SIGHT = 900.0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--decoded", required=True,
                    help="output of `rofl-x file` for a patch with an archive")
    ap.add_argument("--out", default="wards916.json",
                    help="where to write the page's version")
    ap.add_argument("--sight", type=float, default=WARD_SIGHT,
                    help=f"how far a ward sees (default {WARD_SIGHT:.0f})")
    args = ap.parse_args()

    d = json.load(open(args.decoded, encoding="utf-8"))
    wards = d.get("wards") or []
    if not wards:
        print("no wards in that file -- was it decoded with an archive?")
        return 1
    end = max(w["timestamp"] + (w.get("duration") or 0) for w in wards)
    end = max(end, float(d.get("metadata", {}).get("game_length_s") or 0), end)

    out = []
    for w in wards:
        t = w["timestamp"]
        # No duration means it outlived the game rather than lasting no
        # time at all, so it runs to the end.
        life = w.get("duration")
        out.append({
            "t": round(t, 2),
            "until": round(t + life, 2) if life is not None else round(end, 2),
            "killed": life is not None,
            "kind": KIND.get(w["name"], "ward"),
            "name": w["name"],
            "team": SIDE.get(w.get("team"), "order"),
            "role": (w.get("owner") or {}).get("role") or "",
            "x": round(w["pos"][0]),
            "z": round(w["pos"][1]),
            "sight": args.sight,
        })
    out.sort(key=lambda w: w["t"])

    by = collections.Counter(w["name"] for w in out)
    team = collections.Counter(w["team"] for w in out)
    alive = sum(1 for w in out if not w["killed"])
    print(f"{len(out)} wards: " + ", ".join(f"{n} {k}" for k, n in by.most_common()))
    print(f"   {team['order']} order, {team['chaos']} chaos; "
          f"{alive} were never destroyed and run to the end")
    json.dump(out, open(args.out, "w", encoding="utf-8"), separators=(",", ":"))
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
