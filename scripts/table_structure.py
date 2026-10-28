#!/usr/bin/env python3
"""Recover a movement table from the format, and look for structure in it.

Three routes, none of which needs a decodable patch, and each checking
the ones before it.

**The universal bytes.** Every movement block holds at least one entry
and every net id ends `0x40`, so `T(0x40)` appears somewhere in every
block whatever the alignment. Exactly two bytes do that, and the
frequency ranking separates them: the net id's zero byte is the
commonest byte in the class by a wide margin. That gives `T(0x00)` and
`T(0x40)` with nothing assumed.

**The entry stride.** The flags byte is the waypoint count doubled plus
a bit saying an extra byte follows the head, which fixes the entry's
length outright when the count is one: head 10, the extra byte if the
bit is set, then two bytes per axis. So `T(2)` is the flags value whose
entries are 14 bytes long and `T(3)` the one whose entries are 15, with
no freedom anywhere, and `T(4)` and `T(5)` follow from the looser
17..20 that two waypoints allow. Heads are located by the pair above, so
this needs no table either.

**The chain.** A unit rarely crosses a 256-unit boundary, so a
coordinate's high byte is usually unchanged between packets and changes
by one when it changes at all. The cipher bytes seen turning into one
another therefore lay the alphabet out in order. The stride has already
read three entries off the format, so the walk has three independent
answers to reproduce rather than one.

Then the table is tested for structure, because a generated permutation
is not always shapeless. On 16.20 every entry satisfies

    cipher = (low3(p + 4) << 5) | L(high5(p + 4))

-- the plaintext's low three bits rotated into the cipher's top three,
and its high five put through a 32-entry map. That reduces the table
from 256 unknowns to 32. 16.19's solved table does not satisfy it, so it
is a property of the patch rather than of the scheme.

Usage, on a dump of one movement class (`rofl-x explore --dump-netid <n>
--dump <file>`), optionally against a patch whose table is solved:

    python scripts/table_structure.py --dump mv1620.jsonl --forward \
        --check table1619.json --out table1620.json
"""
from __future__ import annotations

import argparse
import collections
import json

ENTRY_HEAD = 10
#: Times a pair of cipher bytes must be seen changing into one another
#: before the step is believed to be one. `explore.rs` uses the same.
CHAIN_MIN = 15
#: What each flags value must measure, for a count of one or of two.
#: A single waypoint is exact; two allow a bitmap and short axes.
EXPECT = {2: (14, 14), 3: (15, 15), 4: (17, 19), 5: (18, 20)}


def payloads(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            yield bytes.fromhex(json.loads(line)["payload"])


def universal(pays):
    """`T(0x00)` and `T(0x40)`: present in every block, ranked by count."""
    present, overall = collections.Counter(), collections.Counter()
    for p in pays:
        present.update(set(p))
        overall.update(p)
    both = sorted((b for b, k in present.items() if k == len(pays)),
                  key=lambda b: -overall[b])
    return both, present, overall


def heads(p, zero, top, forward):
    """Offsets of entry heads, found by the adjacent net id pair."""
    n = len(p)
    a, b = (zero, top) if forward else (top, zero)
    spots = [i for i in range(n - 1) if p[i] == a and p[i + 1] == b]
    if forward:
        return sorted(i - 4 for i in spots if i >= 4)
    return sorted(d for d in (n - 6 - i for i in spots) if d >= 0)


def from_stride(pays, zero, top, forward):
    """`T(2)`..`T(5)`, from lengths the format does not let vary."""
    seen = collections.defaultdict(collections.Counter)
    for p in pays:
        n = len(p)
        hs = heads(p, zero, top, forward)
        at = (lambda h: p[h]) if forward else (lambda h: p[n - 1 - h])
        for a, b in zip(hs, hs[1:]):
            if 0 <= (a if forward else n - 1 - a) < n:
                seen[at(a)][b - a] += 1
    out = {}
    for f, dist in seen.items():
        tot = sum(dist.values())
        if tot < 200:
            continue
        for plain, (lo, hi) in EXPECT.items():
            share = sum(k for d, k in dist.items() if lo <= d <= hi) / tot
            if share >= 0.90 and plain not in out:
                out[plain] = (f, share, tot)
    return out


def from_chain(pays, zero, top, forward, known):
    """The coordinate alphabet, walked outward and checked against `known`."""
    units = collections.defaultdict(list)
    flags_one = known.get(2, (None,))[0]
    for order, p in enumerate(pays):
        n = len(p)
        hs = heads(p, zero, top, forward)
        if not hs:
            continue
        h = min(hs) if forward else min(hs)
        at = (lambda j: p[h + j] if h + j < n else None) if forward else \
             (lambda j: p[n - 1 - h - j] if 0 <= n - 1 - h - j else None)
        # Only a single waypoint puts the coordinates at a fixed place;
        # a bitmap byte shifts anything longer.
        if at(0) != flags_one or at(13) is None:
            continue
        units[(at(2), at(3))].append((order, at(11), at(13)))

    pairs = []
    for v in units.values():
        v.sort()
        for a, b in zip(v, v[1:]):
            pairs.extend(((a[1], b[1]), (a[2], b[2])))
    adjacent = collections.defaultdict(collections.Counter)
    unchanged = 0
    for a, b in pairs:
        if a == b:
            unchanged += 1
            continue
        adjacent[a][b] += 1
        adjacent[b][a] += 1

    def walk(step):
        table, taken = {0: zero}, {zero}
        for first, up in ((step, True), (None, False)):
            if first is not None:
                if adjacent[zero][first] < CHAIN_MIN:
                    continue
                cur = first
            else:
                cand = [(k, c) for c, k in adjacent[zero].items()
                        if c not in taken and k >= CHAIN_MIN]
                if not cand:
                    continue
                cur = max(cand)[1]
            prev, value = zero, 1 if up else -1
            while cur not in taken:
                taken.add(cur)
                table[value & 0xFF] = cur
                cand = [(k, c) for c, k in adjacent[cur].items()
                        if c != prev and c not in taken and k >= CHAIN_MIN]
                if not cand:
                    break
                prev, cur = cur, max(cand)[1]
                value += 1 if up else -1
        return table

    want = {k: v[0] for k, v in known.items()}
    best = []
    for step, k in adjacent[zero].items():
        if k < CHAIN_MIN:
            continue
        t = walk(step)
        best.append((sum(1 for a, b in want.items() if t.get(a) == b),
                     len(t), step, k, t))
    best.sort(reverse=True)
    return best, len(pairs), unchanged / max(len(pairs), 1), len(units)


def structure(table):
    """Test `cipher = (low3(p+4) << 5) | L(high5(p+4))` and return L."""
    L, bad = {}, []
    for p, c in table.items():
        q = (p + 4) & 0xFF
        if (c & 0xE0) != (q & 7) << 5:
            bad.append(p)
            continue
        L.setdefault(q >> 3, set()).add(c & 0x1F)
    return L, bad


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dump", required=True, help="one movement class, jsonl")
    ap.add_argument("--forward", action="store_true",
                    help="the buffer runs forwards (16.20); default backwards")
    ap.add_argument("--check",
                    help="a solved table. Entry-by-entry confirmation "
                         "only when it is the same patch, which T(0x00) "
                         "decides; otherwise it serves the structural "
                         "comparison alone")
    ap.add_argument("--out", help="write the recovered table here")
    args = ap.parse_args()

    pays = list(payloads(args.dump))
    both, present, overall = universal(pays)
    print(f"{len(pays)} blocks, {sum(map(len, pays)):,} bytes")
    if len(both) != 2:
        print(f"   !! {len(both)} bytes present in every block, expected two")
        return 1
    zero, top = both
    print(f"   T(0x00) = {zero:#04x} ({overall[zero] / sum(overall.values()):.1%} "
          f"of bytes), T(0x40) = {top:#04x}")

    known = from_stride(pays, zero, top, args.forward)
    truth = ({int(k, 16): int(v, 16)
              for k, v in json.load(open(args.check, encoding="utf-8")).items()}
             if args.check else None)
    # A table from another patch is unrelated entry by entry -- the
    # substitution is re-decided per patch -- so it may only be used for
    # the structural comparison at the end. `T(0x00)` says which it is.
    same_patch = truth is not None and truth.get(0) == zero
    for plain, (f, share, tot) in sorted(known.items()):
        mark = ""
        if same_patch:
            mark = ("  confirmed" if truth.get(plain) == f
                    else f"  !! solved table says {truth.get(plain):#04x}")
        print(f"   T({plain}) = {f:#04x}   {share:.1%} of {tot} entries have "
              f"the length the format demands{mark}")

    best, npairs, unchanged, nunits = from_chain(pays, zero, top,
                                                 args.forward, known)
    print(f"   chain: {nunits} units, {npairs} pairs, the high byte unchanged "
          f"in {unchanged:.1%} of them")
    if not best:
        print("   !! no attested step; the walk cannot start")
        return 1
    agree, size, step, obs, table = best[0]
    runner = best[1][0] if len(best) > 1 else -1
    if same_patch and truth.get(1) is not None:
        print(f"   the solved table says T(1) = {truth[1]:#04x}"
              + ("  confirmed" if truth[1] == step else "  !! differs"))
    print(f"   T(1) = {step:#04x} ({obs} observations): {size} entries, "
          f"{agree}/{len(known)} known reproduced; next best {runner}")
    if agree < len(known):
        print("   !! the walk does not reproduce what the stride read")

    L, bad = structure(table)
    ok = len(table) - len(bad)
    print(f"\nstructure: {ok}/{len(table)} entries satisfy "
          f"cipher = (low3(p+4) << 5) | L(high5(p+4))")
    if ok == len(table) and all(len(v) == 1 for v in L.values()):
        print("   L is single-valued on every group it covers: "
              + ", ".join(f"L({k})={list(v)[0]}" for k, v in sorted(L.items())))
        ident = sum(1 for k, v in L.items() if list(v)[0] == k)
        print(f"   {ident} of {len(L)} are fixed points -- but L is not the "
              f"identity, so the\n   table is not a bare rotation and the "
              f"remaining {32 - len(L)} groups are open")
    if truth:
        fit = sum(1 for p, c in truth.items()
                  if (c & 0xE0) == (((p + 4) & 7) << 5))
        print(f"   the solved table fits the same law {fit}/256 "
              f"({fit / 256:.1%}); chance gives 12.5%")

    if args.out:
        json.dump({f"{k:02x}": f"{v:02x}" for k, v in sorted(table.items())},
                  open(args.out, "w", encoding="utf-8"), indent=0)
        print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
