#!/usr/bin/env python3
"""Find the movement buffer's layout on a patch with no table at all.

The position parser needs the first entry's start. On 16.19 the payload
announces it: the buffer is written backwards and flush with the end, so
the byte five from the end is `T(0x40)` -- the top byte every net id
shares -- and takes exactly one value. That anchors everything.

On a new patch it may announce nothing. 16.20 does not: no byte takes a
single value at any offset from either end. This finds the layout anyway,
from constraints the format cannot escape and a substitution cannot
disturb.

  1. Which bytes appear in *every* block? Every movement block holds at
     least one entry and every net id ends `0x40`, so `T(0x40)` must
     appear somewhere in all of them -- a question that asks nothing
     about alignment. In practice exactly two bytes qualify, and they are
     `T(0x00)` and `T(0x40)`, which the frequency ranking separates.

  2. Where is an entry head? Those two bytes are adjacent in it, so the
     pair is a signature. Running it against a patch whose layout is
     known is the control: on 16.19 it must put the head flush with the
     payload end.

  3. Which way round is the buffer? Read a head and profile each index.
     Two of them are fixed by the format -- index 1 is the flags' high
     byte, which is only ever 0 or 2, and index 0 is the waypoint count
     doubled -- and two are proportions a substitution cannot change. If
     the shares at index 1 and index 9 have traded places against the
     known patch, the fields are mirrored: an entry head is ten bytes, so
     1 and 9 reflect, and reading a forward-written buffer backwards is
     what produces that.

Usage, on dumps of one movement class each (`rofl-x explore --dump-netid
<n> --dump <file>`), the first being a patch whose layout is known:

    python scripts/layout_direction.py --known mv1267.jsonl \
        --unknown mv1620.jsonl
"""
from __future__ import annotations

import argparse
import collections
import json

#: Bytes in an entry head. Index 1 and index 9 reflect about it, which is
#: what makes a mirrored field order visible.
ENTRY_HEAD = 10
#: Offsets of the two net id bytes within the head: the zero byte of any
#: id below 0x10000, and the top byte, 0x40 for everything in the game.
ZERO_AT, TOP_AT = 4, 5

WHAT = {
    0: "waypoint count doubled",
    1: "flags high, 0 or 2 only",
    ZERO_AT: "net id zero byte",
    TOP_AT: "net id top, 0x40",
    9: "move speed, top byte",
}


def payloads(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            yield bytes.fromhex(json.loads(line)["payload"])


def universal(pays):
    """Byte values present in every block, commonest first.

    Two are expected. The ranking separates them: the net id's zero byte
    is the commonest byte in the class by a wide margin, because every
    entry carries it and coordinates are full of zero high bytes too.
    """
    present = collections.Counter()
    overall = collections.Counter()
    for p in pays:
        present.update(set(p))
        overall.update(p)
    # Present in *almost* every block. Pooling several games is enough
    # for one of them to hold a block without a zero net id byte, and
    # the third-placed byte sits near 98%, so 99% separates the two that
    # matter without being brittle.
    floor = len(pays) * 0.99
    everywhere = [b for b, k in present.items() if k >= floor]
    everywhere.sort(key=lambda b: -overall[b])
    return everywhere[:2], present, overall


def heads(pays, zero, top, forward):
    """Offsets of entry heads, located by the adjacent net id pair.

    A forward buffer puts the zero byte before the top byte in the
    payload; a backward one puts it after. Yields the head's offset in
    whichever direction is being tested, so index `j` of the head is
    read the same way in both cases.
    """
    a, b = (zero, top) if forward else (top, zero)
    for p in pays:
        n = len(p)
        spots = [i for i in range(n - 1) if p[i] == a and p[i + 1] == b]
        if forward:
            spots = [i - ZERO_AT for i in spots if i >= ZERO_AT]
            # the first entry sits behind the header, so the smallest
            yield p, (min(spots) if spots else None), True
        else:
            spots = [n - 1 - (i + TOP_AT) for i in spots]
            spots = [d for d in spots if d >= 0]
            yield p, (min(spots) if spots else None), False


def profile(pays, zero, top, forward, width=16):
    cols = [collections.Counter() for _ in range(width)]
    found = 0
    offsets = collections.Counter()
    for p, off, fwd in heads(pays, zero, top, forward):
        if off is None:
            continue
        found += 1
        offsets[off] += 1
        n = len(p)
        for j in range(width):
            k = off + j if fwd else n - 1 - off - j
            if 0 <= k < n:
                cols[j][p[k]] += 1
    return cols, found, offsets


def show(label, cols, found, total):
    print(f"   {label}: a head placed in {found}/{total} blocks "
          f"({found / max(total, 1):.1%})")
    for j, c in enumerate(cols):
        if not c:
            continue
        s = sum(c.values())
        top2 = "  ".join(f"{v:#04x} {k / s:4.0%}" for v, k in c.most_common(2))
        print(f"      index {j:>2}: {len(c):>3} values   {top2:<22}"
              f"  {WHAT.get(j, '')}")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--known", required=True,
                    help="movement dump for a patch whose layout is solved")
    ap.add_argument("--unknown", required=True)
    ap.add_argument("--known-label", default="known patch")
    ap.add_argument("--unknown-label", default="new patch")
    args = ap.parse_args()

    out = {}
    for label, path in ((args.known_label, args.known),
                        (args.unknown_label, args.unknown)):
        pays = list(payloads(path))
        everywhere, present, overall = universal(pays)
        print(f"{label}: {len(pays)} blocks, {sum(map(len, pays)):,} bytes")
        print(f"   bytes present in every block: "
              f"{[hex(b) for b in everywhere]}")
        for b, k in present.most_common(6):
            print(f"      {b:#04x} in {k / len(pays):7.2%} of blocks, "
                  f"{overall[b] / sum(overall.values()):6.2%} of bytes")
        if len(everywhere) != 2:
            print(f"   !! expected two bytes in 99% of blocks, got "
                  f"{len(everywhere)} -- the pair signature is not reliable")
            continue
        zero, top = (everywhere + [None, None])[:2]
        print(f"   taking T(0x00) = {zero:#04x}, T(0x40) = {top:#04x} "
              f"by frequency")
        for forward in (False, True):
            way = "forwards" if forward else "backwards"
            cols, found, offsets = profile(pays, zero, top, forward)
            show(f"read {way}", cols, found, len(pays))
            common = offsets.most_common(3)
            print(f"      head offset: "
                  + ", ".join(f"{k}:{n / max(found,1):.0%}" for k, n in common))
            out[(label, forward)] = cols
        print()

    # The decision: whichever direction reproduces the known patch's
    # shares at the two indices the format constrains.
    print("Index 1 is the flags' high byte and index 9 the move speed's "
          "top; both\nare narrow. If they are swapped against the known "
          "patch the head is\nmirrored, so the buffer runs the other way.")
    ref = out[(args.known_label, False)]
    def share(cols, j):
        c = cols[j]
        return max(c.values()) / sum(c.values()) if c else 0.0
    print(f"\n   {args.known_label}, backwards: "
          f"index 1 {share(ref, 1):.1%}, index 9 {share(ref, 9):.1%}")
    for forward in (False, True):
        cols = out[(args.unknown_label, forward)]
        way = "forwards " if forward else "backwards"
        gap = (abs(share(cols, 1) - share(ref, 1))
               + abs(share(cols, 9) - share(ref, 9)))
        print(f"   {args.unknown_label}, {way}: "
              f"index 1 {share(cols, 1):.1%}, index 9 {share(cols, 9):.1%}"
              f"   mismatch {gap:.3f}")
    print(f"\nThe smaller mismatch is the direction the buffer is written "
          f"in.\nIndex {ENTRY_HEAD - 1} and index 1 reflect about a "
          f"{ENTRY_HEAD}-byte head.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
