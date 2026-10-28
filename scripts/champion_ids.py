#!/usr/bin/env python3
"""Check a recovered movement table against the ten champions' net ids.

This is the strongest check in the set, because nothing in it was used
to build the table.

The explorer names the champions as a run of ten consecutive net ids,
found from the longest-lived consecutive run of netids in the file --
no table, no payload decoded. Their low bytes are therefore ten known
consecutive plaintexts.

In the entry head the net id is four bytes, of which the third is zero
for any id below `0x10000` and the fourth is the `0x40` every id ends
with. So the sequence `(X, T(0x00), T(0x00), T(0x40))` picks out
precisely the units whose id is `0x400000XX`, and `X` is `T(XX)`. On
16.20 that yields exactly ten frequent values covering 100 % of 48,450
occurrences -- the ten champions -- against nineteen distinct values in
all, the rest being rare.

Then the table has to predict them. 16.20's came from move speeds and
the entry stride and never saw a net id, so the ten ciphers are held
out: the structural law plus `L(22)` and `L(23)` says what each must be,
and all ten come out right, the predicted set equal to the observed set.

It is also a sharp test of whether the id is stored as four raw bytes at
all. On 16.21 the same pattern gives 256 distinct values with the ten
commonest covering 56 %, and the two most frequent are the anchor bytes
themselves -- noise. 16.21 does not store its net ids this way, which is
one of the reasons `docs/EXPLORE.md` suspects a variable-length
encoding there.

Usage:

    python scripts/champion_ids.py --dump mv1620_both.jsonl \\
        --table tab1620_mv_L.json --first 0x400000ae --last 0x400000b7
"""
from __future__ import annotations

import argparse
import collections
import json


def pattern_counts(path, zero, top, limit):
    """`X` in every `(X, T(0), T(0), T(0x40))`, and how often."""
    out = collections.Counter()
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i >= limit:
                break
            p = bytes.fromhex(json.loads(line)["payload"])
            for k in range(len(p) - 3):
                if p[k + 1] == zero and p[k + 2] == zero and p[k + 3] == top:
                    out[p[k]] += 1
    return out


def anchors(path, limit):
    """T(0x00) and T(0x40): the two bytes in almost every block, ranked."""
    present, overall, n = collections.Counter(), collections.Counter(), 0
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i >= limit:
                break
            p = bytes.fromhex(json.loads(line)["payload"])
            n += 1
            present.update(set(p))
            overall.update(p)
    both = sorted((b for b, k in present.items() if k >= n * 0.99),
                  key=lambda b: -overall[b])
    return (both + [None, None])[:2]


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dump", required=True, help="one movement class, jsonl")
    ap.add_argument("--table", help="tab*_mv_L.json, to test its predictions")
    ap.add_argument("--first", type=lambda s: int(s, 0), required=True,
                    help="the first champion net id, as the explorer reports it")
    ap.add_argument("--last", type=lambda s: int(s, 0), required=True)
    ap.add_argument("--limit", type=int, default=40000)
    args = ap.parse_args()

    zero, top = anchors(args.dump, args.limit)
    if zero is None or top is None:
        print("!! could not find the two near-universal bytes")
        return 1
    print(f"T(0x00) = {zero:#04x}, T(0x40) = {top:#04x}")

    counts = pattern_counts(args.dump, zero, top, args.limit)
    tot = sum(counts.values())
    if not tot:
        print("!! the pattern (X, T(0), T(0), T(0x40)) never occurs")
        return 1
    ten = counts.most_common(10)
    share = sum(k for _, k in ten) / tot
    print(f"   (X, T(0), T(0), T(0x40)): {tot} occurrences, "
          f"{len(counts)} distinct X")
    print(f"   the ten commonest cover {share:.0%} of them"
          + ("   -- the champions" if share > 0.9 and len(counts) < 40
             else "   -- too diffuse to be ten champions, so the net id is "
                  "not\n      four raw bytes on this patch"))
    for x, k in ten:
        print(f"      X = {x:#04x}  {k:>7}")
    if share <= 0.9 or len(counts) >= 40:
        return 0

    ids = list(range(args.first, args.last + 1))
    if len(ids) != 10:
        print(f"   !! {len(ids)} ids given, expected ten")
    observed = {x for x, _ in ten}
    if not args.table:
        print(f"\n   so T(0x{args.first & 0xFF:02x})..T(0x{args.last & 0xFF:02x}) "
              f"are {sorted(hex(x) for x in observed)}, in some order")
        return 0

    raw = json.load(open(args.table, encoding="utf-8"))
    L = {int(k): v for k, v in (raw.get("L", raw)).items()}
    print(f"\n   what the table predicts for each champion's low byte:")
    pred, hit, open_groups = set(), 0, 0
    for nid in ids:
        p = nid & 0xFF
        q = (p + 4) & 0xFF
        g = q >> 3
        if g not in L:
            open_groups += 1
            print(f"      {p:#04x}: group {g} is open, nothing predicted")
            continue
        c = ((q & 7) << 5) | L[g]
        pred.add(c)
        hit += c in observed
        print(f"      {p:#04x}: group {g:>2}, L = {L[g]:>2} -> {c:#04x}  "
              + ("seen" if c in observed else "NOT SEEN"))
    print(f"\n   {hit} of {len(ids) - open_groups} predictions are among the "
          f"ten observed")
    if pred == observed:
        print("   the predicted set equals the observed set exactly")
        print("   -- and no net id was used to build the table, so this is "
              "held out")
    elif hit:
        print("   !! partial agreement, which a correct table would not give")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
