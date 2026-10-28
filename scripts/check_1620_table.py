#!/usr/bin/env python3
"""Judge a recovered movement table by how units move, not by the map.

**On-map does not test a coordinate's low byte.** A coordinate is two
bytes scaled onto a 15,000-unit square: getting the high byte wrong puts
a unit nowhere near the Rift, which is why the bare rotation managed
6.4 %, but getting the low byte wrong moves it at most 510 map units and
usually leaves it on the map. So a table can read 99.9 % on-map and
still have most of its low-byte groups wrong. That mistake was made here
and is the reason this script exists.

What does test the low byte is the step between consecutive sightings of
one unit. A champion walking 350 units a second covers 175 map units in
half a second, so the steps have a distribution with a firm shape, and a
wrong low byte throws readings hundreds of units out of place -- which
shows in the tail rather than the median.

The yardstick has to come from a patch that decodes, and it has to be
crippled to the same coverage first. That matters: a partial table reads
a unit's positions sparsely, so consecutive readable ones sit further
apart in time and a step measured over 0.4 s is naturally bigger than
one over 0.05 s. Measured on 16.19, the 90th percentile turns out to be
robust to that -- 121.1 map units on the whole table, 129.1 keeping 66 %
of the alphabet, 116.1 keeping 45 % -- so a candidate sitting far above
it is wrong rather than merely sparse.

Used on the 16.20 groups the move speeds proposed, this rejected five of
twenty-one. Adding them one at a time shows where the damage enters:
`L(18) = 31` lifted the 90th percentile by 143 map units, `L(29) = 4` by
93 and `L(15) = 5` by 32, while every other group left it alone. Two of
the five came from the same speed pattern as `L(18)`, so they went with
it.

Usage:

    python scripts/check_1620_table.py --known mv1267.jsonl \\
        --known-table table1619.json --unknown mv1620_both.jsonl \\
        --table tab1620_mv_L.json
"""
from __future__ import annotations

import argparse
import collections
import json
import random

ENTRY_HEAD = 10
LO, HI = -1000.0, 16000.0
#: Steps are compared over this window, so every table answers the same
#: question: how far does a unit move in half a second?
WINDOW = 0.5


def walk(blocks, plain, zero, top, forward):
    """Each block's entries, from the buffer start, as far as they size.

    The first entry's head is located by the (T(0x00), T(0x40)) pair,
    which needs no table; the walk then stops at the first entry it
    cannot size rather than discarding the block, because a partial
    table cannot size them all.
    """
    read = on = off = 0
    tracks = collections.defaultdict(list)
    for t, rep, p in blocks:
        n = len(p)
        a, b = (zero, top) if forward else (top, zero)
        spots = [i for i in range(4, n - 1) if p[i] == a and p[i + 1] == b]
        spots = ([i - 4 for i in spots] if forward
                 else [d for d in (n - 6 - i for i in spots) if d >= 0])
        if not spots:
            continue
        at = ((lambda k: p[k] if 0 <= k < n else None) if forward
              else (lambda k: p[n - 1 - k] if 0 <= n - 1 - k < n else None))
        rd = lambda k: plain.get(at(k)) if at(k) is not None else None
        j = min(spots)
        while j + ENTRY_HEAD <= n:
            f = rd(j)
            if f is None or f >> 1 == 0:
                break
            count = f >> 1
            k = j + ENTRY_HEAD + (f & 1)
            if count > 1:
                width = ((count - 2) >> 2) + 1
                bm = [rd(k + q) for q in range(width)]
                if any(x is None for x in bm):
                    break
                k += width
                delta = lambda i: bm[i >> 3] & (1 << (i & 7)) != 0
            else:
                delta = lambda i: False
            first = None
            for i in range(count):
                cur = []
                for axis in (0, 1):
                    if i and delta(2 * (i - 1) + axis):
                        k += 1
                    else:
                        cur.append((k, k + 1))
                        k += 2
                if i == 0 and len(cur) == 2:
                    first = cur
            if k > n or not first:
                break
            v = [rd(q) for q in (first[0][0], first[0][1],
                                 first[1][0], first[1][1])]
            if None not in v:
                read += 1
                x = v[0] | (v[1] << 8)
                z = v[2] | (v[3] << 8)
                x = x - 65536 if x > 32767 else x
                z = z - 65536 if z > 32767 else z
                mx, mz = x * 2.0 + 7358.0, z * 2.0 + 7412.0
                if LO <= mx < HI and LO <= mz < HI:
                    on += 1
                    tracks[(rep, at(j + 2), at(j + 3), at(j + 4))].append(
                        (t, mx, mz))
                else:
                    off += 1
            j = k
    steps = []
    for v in tracks.values():
        v.sort()
        for p1, p2 in zip(v, v[1:]):
            if 0 < p2[0] - p1[0] <= WINDOW:
                steps.append(((p2[1] - p1[1]) ** 2
                              + (p2[2] - p1[2]) ** 2) ** 0.5)
    steps.sort()
    q = lambda f: (steps[min(int(len(steps) * f), len(steps) - 1)]
                   if steps else float("nan"))
    return {"read": read, "on": on, "off": off, "units": len(tracks),
            "steps": len(steps), "median": q(0.5), "p90": q(0.9),
            "p99": q(0.99)}


def load(path, limit=None):
    out = []
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if limit is not None and i >= limit:
                break
            r = json.loads(line)
            out.append((float(r.get("t", 0.0)), r.get("replay", ""),
                        bytes.fromhex(r["payload"])))
    return out


def show(label, r):
    print(f"   {label:<42} read {r['read']:>7}  on map "
          f"{r['on'] / max(r['read'], 1):6.2%}  units {r['units']:>5}  "
          f"median {r['median']:6.1f}  90th {r['p90']:6.1f}  "
          f"99th {r['p99']:7.1f}")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--known", required=True)
    ap.add_argument("--known-table", required=True)
    ap.add_argument("--unknown", required=True)
    ap.add_argument("--table", required=True,
                    help="the candidate, as written by solve_1620_speeds.py "
                         "(a bare group map) or tab1620_mv_L.json")
    ap.add_argument("--incremental", action="store_true",
                    help="add the non-firm groups one at a time and report "
                         "where the tail jumps")
    ap.add_argument("--limit", type=int, help="blocks per dump, for a quick run")
    args = ap.parse_args()

    T = {int(k, 16): int(v, 16)
         for k, v in json.load(open(args.known_table, encoding="utf-8")).items()}
    kb = load(args.known, args.limit)
    plain = {c: p for p, c in T.items()}
    print(f"the yardstick -- {args.known}, solved table, and the same "
          f"crippled to a partial alphabet:")
    show("whole table", walk(kb, plain, T[0x00], T[0x40], False))
    rnd = random.Random(11)
    for keep in (0.66, 0.45):
        part = {c: p for p, c in T.items() if p <= 19 or rnd.random() < keep}
        show(f"keeping {keep:.0%} of the alphabet ({len(part)}/256)",
             walk(kb, part, T[0x00], T[0x40], False))

    raw = json.load(open(args.table, encoding="utf-8"))
    L = {int(k): v for k, v in (raw.get("L", raw)).items()}
    ub = load(args.unknown, args.limit)
    pays = [p for _, _, p in ub]
    present, overall = collections.Counter(), collections.Counter()
    for p in pays:
        present.update(set(p))
        overall.update(p)
    floor = len(pays) * 0.99
    both = sorted((b for b, k in present.items() if k >= floor),
                  key=lambda b: -overall[b])[:2]
    if len(both) < 2:
        print("!! could not find the two near-universal bytes")
        return 1
    zero, top = both

    def plain_of(Lx):
        out = {}
        for p in range(256):
            q = (p + 4) & 0xFF
            if (q >> 3) in Lx:
                out[((q & 7) << 5) | Lx[q >> 3]] = p
        return out

    print(f"\n{args.unknown}, T(0x00) = {zero:#04x}, T(0x40) = {top:#04x}:")
    FIRM = {0, 1, 2, 8, 30, 31}
    if args.incremental:
        cur = {g: v for g, v in L.items() if g in FIRM}
        prev = walk(ub, plain_of(cur), zero, top, True)
        show(f"firm {len(cur)} groups", prev)
        for g in [g for g in L if g not in FIRM]:
            cur[g] = L[g]
            r = walk(ub, plain_of(cur), zero, top, True)
            jump = r["p90"] - prev["p90"]
            show(f"+ L({g})={L[g]} ({len(cur)} groups)"
                 + ("   <== the tail jumps" if jump > 40 else ""), r)
            prev = r
    else:
        show(f"{len(L)} groups ({len(plain_of(L))}/256 entries)",
             walk(ub, plain_of(L), zero, top, True))
    print("\nA 90th percentile far above the crippled yardstick means wrong "
          "groups,\nnot sparse coverage. On-map cannot tell the difference.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
