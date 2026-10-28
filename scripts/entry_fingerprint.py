#!/usr/bin/env python3
"""Compare a movement class's entry layout against a patch that decodes.

`layout_direction.py` answers one question — which way round the buffer
runs — and it answers it by assuming the net id's zero byte and `0x40`
top sit next to each other in the entry head, which is true on 16.9,
16.19 and 16.20. When that assumption fails there is nothing to anchor
on, and every offset-based probe returns noise. 16.21 is that case.

So this asks two questions that assume no offsets at all.

**How narrow is each column?** Take payloads of one short fixed length —
a header plus a single entry — and count the distinct values at each
position. That is a fingerprint of the field layout rather than of the
substitution: the flags' high byte takes two values whatever table is in
use, the net id's top byte takes one, the move speed's exponent a
handful, a coordinate's low byte all 256. Sliding one patch's
fingerprint against another's, in both directions, then says whether the
layout is the same thing moved or a different thing. Measured this way
16.20 matches 16.19 at **87 %** of columns, forwards with a two-byte
header, and its narrow columns land exactly where 16.19's are. 16.21
reaches **73 %** at best, with no shift standing out.

**Does any byte pair recur once per entry?** Entries tile the buffer, so
a field every entry carries appears once per entry and the gaps between
appearances are entry lengths. On 16.19 the net id pair is absent from
only 2.7 % of blocks and 40 % of its gaps are exactly 14, which is a
single-waypoint entry's length. On 16.21 the best candidate pair is
absent from 64.7 % of blocks and its gaps are scattered — so no pair is
a per-entry field, and the entry is not laid out as a fixed sequence of
byte fields the way earlier patches' are.

A variable-length encoding of the net id would produce exactly this,
since the fields after it would then sit at an offset that depends on
the id's value. That is a hypothesis the measurements are consistent
with, not a finding: confirming it needs a reading that does not assume
fixed offsets anywhere.

Usage:

    python scripts/entry_fingerprint.py --known mv1267.jsonl \\
        --unknown mv1621.jsonl --length 19 \\
        --pair cc,db --pair 25,0b --known-pair 29,42
"""
from __future__ import annotations

import argparse
import collections
import json

#: A column holding this many values or fewer counts as narrow. The
#: fields that matter are far below it and the coordinates far above, so
#: the exact figure does not decide anything.
NARROW = 40


def payloads(path, length=None):
    with open(path, encoding="utf-8") as f:
        for line in f:
            p = bytes.fromhex(json.loads(line)["payload"])
            if length is None or len(p) == length:
                yield p


def fingerprint(group):
    """Distinct values at each position, across payloads of one length."""
    width = len(group[0])
    return [len(collections.Counter(p[j] for p in group)) for j in range(width)]


def align(ref_buffer, fp):
    """Best (agreement, forward, shift) of `fp` against a buffer order."""
    out = []
    for forward in (True, False):
        seq = fp if forward else list(reversed(fp))
        for shift in range(-6, 7):
            agree = overlap = 0
            for j, rv in enumerate(ref_buffer):
                k = j + shift
                if not 0 <= k < len(seq):
                    continue
                overlap += 1
                agree += (rv <= NARROW) == (seq[k] <= NARROW)
            if overlap >= 12:
                out.append((agree / overlap, overlap, forward, shift))
    out.sort(reverse=True)
    return out


def pair_stride(path, a, b, limit):
    """Where a byte pair appears, and how far apart its appearances are."""
    gaps = collections.Counter()
    blocks = absent = 0
    for i, p in enumerate(payloads(path)):
        if i >= limit:
            break
        blocks += 1
        hits = [k for k in range(len(p) - 1) if p[k] == a and p[k + 1] == b]
        if not hits:
            absent += 1
        for x, y in zip(hits, hits[1:]):
            gaps[y - x] += 1
    tot = sum(gaps.values())
    near = sum(n for g, n in gaps.items() if g <= 24) / tot if tot else 0.0
    return blocks, absent, gaps, near


def byte_pair(text):
    a, b = text.split(",")
    return int(a, 16), int(b, 16)


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--known", required=True,
                    help="movement dump for a patch whose layout is solved")
    ap.add_argument("--unknown", required=True)
    ap.add_argument("--length", type=int, default=19,
                    help="payload length to fingerprint: short enough to "
                         "hold one entry, common enough to measure")
    ap.add_argument("--known-header", type=int, default=4,
                    help="header bytes on the known patch at this length")
    ap.add_argument("--known-pair", type=byte_pair, default=(0x29, 0x42),
                    help="the known patch's per-entry pair, as hex a,b")
    ap.add_argument("--pair", type=byte_pair, action="append", default=[],
                    help="a candidate per-entry pair on the unknown patch; "
                         "repeatable")
    ap.add_argument("--backward-known", action="store_true", default=True,
                    help="the known patch writes its buffer backwards "
                         "(16.9, 16.19); pass --no-backward-known for 16.20")
    ap.add_argument("--no-backward-known", dest="backward_known",
                    action="store_false")
    ap.add_argument("--limit", type=int, default=60000)
    args = ap.parse_args()

    kg = list(payloads(args.known, args.length))
    ug = list(payloads(args.unknown, args.length))
    if len(kg) < 100 or len(ug) < 100:
        print(f"!! too few payloads of {args.length} bytes "
              f"({len(kg)} known, {len(ug)} unknown) -- pick another length")
        return 1
    kfp, ufp = fingerprint(kg), fingerprint(ug)
    print(f"distinct values per position, {args.length}-byte payloads")
    print(f"   known   ({len(kg):>5}): " + " ".join(f"{v:>3}" for v in kfp))
    print(f"   unknown ({len(ug):>5}): " + " ".join(f"{v:>3}" for v in ufp))

    # the known patch's entry, in logical buffer order
    entry = kfp[args.known_header:]
    ref = list(reversed(entry)) if args.backward_known else entry
    print(f"\nknown patch's entry in buffer order: "
          + " ".join(f"{v:>3}" for v in ref))
    print(f"   narrow columns at "
          + str([j for j, v in enumerate(ref) if v <= NARROW]))

    print("\nbest alignments of the unknown against it:")
    for sc, ov, fwd, sh in align(ref, ufp)[:4]:
        seq = ufp if fwd else list(reversed(ufp))
        narrow = sorted(j - sh for j, v in enumerate(seq)
                        if v <= NARROW and 0 <= j - sh < 16)
        print(f"   {'forwards ' if fwd else 'backwards'} shift {sh:+d}: "
              f"{sc:.0%} of {ov} columns agree; narrow columns land at "
              f"{narrow}")
    print("   (16.20 against 16.19 reaches 88%; below about 75% means the "
          "layout is\n    not the same fields moved)")

    print("\ndoes a byte pair recur once per entry?")
    for label, path, (a, b) in ([("known", args.known, args.known_pair)]
                                + [("unknown", args.unknown, p)
                                   for p in args.pair]):
        blocks, absent, gaps, near = pair_stride(path, a, b, args.limit)
        tot = sum(gaps.values())
        print(f"   {label:<8} ({a:#04x},{b:#04x}): absent from "
              f"{absent / max(blocks,1):5.1%} of blocks; "
              + (", ".join(f"gap {g}:{n / tot:.0%}"
                           for g, n in gaps.most_common(4)) if tot
                 else "no gaps to measure"))
        print(f"      {near:.0%} of gaps are 24 or less"
              + ("   -- an entry stride" if near > 0.7
                 else "   -- scattered, so not a per-entry field"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
