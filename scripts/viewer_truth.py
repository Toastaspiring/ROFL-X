#!/usr/bin/env python3
"""Pull the answers out of the replay's own scoreboard.

statsJson carries, per player, both summoner spells and how many times
each was cast, and how many times the player died. That makes it a
referee for anything read out of the packet stream: a detector that
finds 357 teleports in a game whose scoreboard records two is wrong, and
says so without any judgement call.

It is what showed that the death class is read correctly -- its 74
deaths split per champion exactly as `NUM_DEATHS` does -- and what
showed that teleports cannot be separated from ability dashes at all.

Usage:

    rofl-x summary -r <replay>.rofl > summary.txt
    python scripts/viewer_truth.py --summary summary.txt --out truth1619.json
"""
from __future__ import annotations

import argparse
import json
import re

# Riot's summoner spell ids, as they appear in SUMMONER_SPELL_1/2.
SPELL = {1: "Cleanse", 3: "Exhaust", 4: "Flash", 6: "Ghost", 7: "Heal",
         11: "Smite", 12: "Teleport", 13: "Clarity", 14: "Ignite",
         21: "Barrier", 32: "Mark", 39: "Mark", 54: "Placeholder"}
FIELDS = ("SUMMONER_SPELL_1", "SUMMONER_SPELL_2",
          "SUMMON_SPELL1_CAST", "SUMMON_SPELL2_CAST", "NUM_DEATHS")
# Wards are counted several ways, and the distinction matters: WARD_PLACED
# is every ward, WARD_PLACED_DETECTOR only the control wards among them.
WARD_FIELDS = ("WARD_PLACED", "WARD_PLACED_DETECTOR", "WARD_KILLED",
               "VISION_WARDS_BOUGHT_IN_GAME", "SIGHT_WARDS_BOUGHT_IN_GAME",
               "VISION_SCORE")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--summary", required=True,
                    help="output of `rofl-x summary -r <replay>`")
    ap.add_argument("--out", default="truth1619.json",
                    help="where to write it")
    args = ap.parse_args()

    txt = open(args.summary, encoding="utf-8", errors="replace").read()
    champs = re.findall(r'"SKIN":\s*"([^"]+)"', txt)
    if not champs:
        print("no players found; is this a `rofl-x summary` dump?")
        return 1
    vals = {f: re.findall(rf'"{f}":\s*"(-?\d+)"', txt)
            for f in FIELDS + WARD_FIELDS}
    short = [f for f, v in vals.items()
             if len(v) != len(champs) and f in FIELDS]
    if short:
        print(f"the scoreboard is missing {', '.join(short)} for some players")
        return 1

    out = {}
    name = lambda i: SPELL.get(i, str(i))
    print(f"{'champion':<14} {'spell 1':<10}{'casts':>6}   "
          f"{'spell 2':<10}{'casts':>6}   deaths")
    for i, champ in enumerate(champs):
        s1, s2 = int(vals["SUMMONER_SPELL_1"][i]), int(vals["SUMMONER_SPELL_2"][i])
        c1, c2 = int(vals["SUMMON_SPELL1_CAST"][i]), int(vals["SUMMON_SPELL2_CAST"][i])
        deaths = int(vals["NUM_DEATHS"][i])
        print(f"{champ:<14} {name(s1):<10}{c1:>6}   {name(s2):<10}{c2:>6}"
              f"   {deaths:>6}")
        out[champ] = {"spells": {name(s1): c1, name(s2): c2},
                      "deaths": deaths,
                      "wards": {f: int(vals[f][i]) for f in WARD_FIELDS}}

    json.dump(out, open(args.out, "w", encoding="utf-8"), indent=1)
    print(f"\nwrote {len(out)} players to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
