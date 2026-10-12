#!/usr/bin/env python3
"""Find the spell-cast class on a packed patch, and check it is one.

The exact statsJson proof (docs/EXPLORE.md § 2) wants a netid whose
per-champion block count *equals* one of Riot's counters, give or take
one constant, for all ten players in every game. Deaths satisfy that.
Casts do not: a held ability, a recast, a toggle or a steerable missile
sends more than one packet, so the count runs over by an amount that
differs per champion. The identity is real but the equality is not, and
a matcher built on equality cannot see it.

So this asks the weaker question first -- which netid *moves with* the
cast counters -- and then spends its effort ruling out the two ways
that answer could be worthless:

  the activity confound   A player who casts more also farms, fights
                          and buys more, so any busy-player class
                          correlates with any busy-player counter. The
                          check is whether the candidate tracks casts
                          *and not* unrelated activity. Minions killed
                          is the sharpest of those, being almost pure
                          activity with nothing to do with casting.

  the coincidence         With a few hundred netids, something will
                          correlate. The check is a prediction the
                          right answer must satisfy and a coincidence
                          need not: the surplus over the counter has to
                          land on the champions whose abilities recast,
                          toggle, transform or channel, and not on the
                          point-and-click ones. That is a statement
                          about names, not numbers.

Finally, run the whole thing on a patch that decodes, where the winner
can be looked up by name. If it does not pick the real cast class
there, it has not earned the answer it gives on a packed one.

Produce the inputs with:

    rofl-x explore -r <replay> [-r ...] --dump-counts counts.json -o /dev/null

    # the packed patch under test, then the patch that validates it
    python scripts/cast_class.py --counts counts19.json \\
        --validate counts9.json --archive patch/16-9.patch
"""
from __future__ import annotations

import argparse
import collections
import glob
import json
import re
import statistics as stat
import zipfile

# Casts go out the same way whether they came from an ability or a
# summoner spell, so the sum is the counter to beat. `explore` adds
# both of these to a replay's counters.
CASTS = "SPELL1..4_CAST+SUMMON_SPELL1..2_CAST"
ABILITIES = "SPELL1..4_CAST"
# Counters a cast class should *not* track, most of all the first.
CONFOUNDS = [
    "MINIONS_KILLED",
    "CHAMPIONS_KILLED+NUM_DEATHS+ASSISTS",
    "ITEMS_PURCHASED",
    "TOTAL_TIME_SPENT_DEAD",
    "PING",
]
# A counter column has to vary before it can test anything, and a
# player with a handful of casts tells us nothing about a ratio.
MIN_SPREAD, MIN_CASTS = 6, 30


def pearson(x: list[float], y: list[float]) -> float:
    n = len(x)
    if n < 3:
        return 0.0
    mx, my = sum(x) / n, sum(y) / n
    sx = sum((a - mx) ** 2 for a in x) ** 0.5
    sy = sum((b - my) ** 2 for b in y) ** 0.5
    if sx == 0 or sy == 0:
        return 0.0
    return sum((a - mx) * (b - my) for a, b in zip(x, y)) / (sx * sy)


def pooled(rows, netid: str, field: str):
    """Every (blocks, counter) pair for one netid across the replays."""
    xs, ys, games = [], [], 0
    for r in rows:
        b = r["blocks_per_champion"].get(netid)
        c = r["counters"].get(field)
        if b and c and len(b) == len(c) and len(set(c)) >= MIN_SPREAD:
            xs += b
            ys += c
            games += 1
    return xs, ys, games


def rank(rows, field: str, names: dict[str, str]):
    """Netids by how closely they move with `field`."""
    netids: set[str] = set()
    for r in rows:
        netids |= set(r["blocks_per_champion"])
    out = []
    for netid in netids:
        xs, ys, games = pooled(rows, netid, field)
        # Judge a netid on the replays that could test it, bar one.
        if games < len(rows) - 1 or not xs:
            continue
        ratios = [x / y for x, y in zip(xs, ys) if y > 0]
        out.append((pearson(xs, ys), netid, games,
                    stat.median(ratios) if ratios else 0.0,
                    names.get(netid, "")))
    out.sort(reverse=True)
    return out


def archive_names(path: str) -> dict[str, str]:
    """Class name per netid, from a format-2 patch archive."""
    if not path:
        return {}
    with zipfile.ZipFile(path) as z:
        res = json.loads(z.read("result.json"))
    out = {}
    for netid, entries in (res.get("packets") or {}).items():
        if entries:
            first = entries[0]
            out[str(netid)] = first.get("class", "") if isinstance(first, dict) \
                else str(first)
    return out


def champion_names(replay_glob: str, patch: str) -> list[list[str]]:
    """Each replay's champions, in statsJson order -- the same order the
    counts are dumped in, which is why they can be zipped together."""
    out = []
    for f in sorted(glob.glob(replay_glob)):
        head = open(f, "rb").read(512)
        if patch and not re.search(patch.replace(".", r"\.").encode(), head):
            continue
        b = open(f, "rb").read()
        i = b.find(b'{"gameLength"')
        if i < 0:
            continue
        d = json.loads(b[i:b.rfind(b"}") + 1].decode("utf-8", "replace"))
        out.append([p.get("SKIN", "?") for p in json.loads(d["statsJson"])])
    return out


def surplus_by_champion(rows, netid: str, rosters: list[list[str]]):
    """The overshoot over the cast counter, per champion.

    This is the test that a coincidence fails. A champion whose
    abilities are held, recast, toggled or steered sends more than one
    packet per cast; a point-and-click one sends about one.
    """
    per: dict[str, list[float]] = collections.defaultdict(list)
    for roster, r in zip(rosters, rows):
        b = r["blocks_per_champion"].get(netid)
        c = r["counters"].get(CASTS)
        if not b or not c or len(roster) != len(b) or len(b) != len(c):
            continue
        for champ, blocks, casts in zip(roster, b, c):
            if casts >= MIN_CASTS:
                per[champ].append((blocks - casts) / casts)
    return sorted(((stat.median(v), len(v), k) for k, v in per.items() if len(v) >= 2),
                  reverse=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--counts", required=True, help="--dump-counts for the patch under test")
    ap.add_argument("--validate", help="--dump-counts for a patch that decodes")
    ap.add_argument("--archive", help="that patch's .patch archive, to name its netids")
    ap.add_argument("--replays", help="glob of the replays behind --counts, for champion names")
    ap.add_argument("--patch", default="", help="version prefix selecting those replays")
    args = ap.parse_args()

    if args.validate:
        names = archive_names(args.archive)
        board = rank(json.load(open(args.validate)), CASTS, names)
        print("validation, on a patch whose classes are known:")
        for r, netid, games, ratio, name in board[:5]:
            print(f"  netid {netid:>5}  r={r:+.3f}  {games} games  "
                  f"blocks/cast {ratio:5.2f}  {name or '(unnamed)'}")
        top = board[0][4] if board else ""
        print("  -> the method picks "
              f"{'the real cast class' if 'CastSpell' in top else 'something else: do not trust it'}\n")

    rows = json.load(open(args.counts))
    board = rank(rows, CASTS, {})
    print(f"candidates on the patch under test, {len(rows)} replays:")
    for r, netid, games, ratio, _ in board[:5]:
        print(f"  netid {netid:>5}  r={r:+.3f}  {games} games  blocks/cast {ratio:5.2f}")
    if not board:
        return 1
    best = board[0][1]

    print(f"\nnetid {best} against counters it should not track:")
    for field in [CASTS, ABILITIES] + CONFOUNDS:
        xs, ys, games = pooled(rows, best, field)
        mark = "  <- casts" if field in (CASTS, ABILITIES) else ""
        print(f"  {field:<42} r={pearson(xs, ys):+.3f} ({games} games){mark}")

    if args.replays:
        rosters = champion_names(args.replays, args.patch)
        board2 = surplus_by_champion(rows, best, rosters)
        print(f"\nsurplus over the counter, by champion ({len(board2)} seen twice or more):")
        print("  most, which should be the held / recast / toggled / transforming ones:")
        for m, n, k in board2[:8]:
            print(f"    {k:<16} {m:+7.1%}  ({n} games)")
        print("  least, which should be the point-and-click ones:")
        for m, n, k in board2[-8:]:
            print(f"    {k:<16} {m:+7.1%}  ({n} games)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
