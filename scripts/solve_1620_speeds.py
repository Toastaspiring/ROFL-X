#!/usr/bin/env python3
"""Recover 16.20's movement substitution from the move speeds it reports.

The table has structure -- `docs/EXPLORE.md` derives it -- so it is not
256 unknowns but 32:

    cipher = (low3(p + 4) << 5) | L(high5(p + 4))

The plaintext's low three bits rotate into the cipher's top three with
no part of `L` involved, which means the top three bits of every cipher
byte are *known* before anything is solved. What is left is `L`, a
substitution on five bits.

The entry head carries the unit's move speed as a four-byte float, and a
speed the game reports is known plaintext -- so each speed pattern
constrains `L`. Four constraints apply, and the fourth is the one that
matters:

  top bits      `(c & 0xE0) == (low3(p + 4) << 5)`, always.
  known groups  where `p`'s group is already in `L` the whole cipher
                byte is determined, so the pattern must show exactly it.
  injectivity   within one pattern equal plaintexts must have equal
                ciphers and different plaintexts different ones.
  bijection     across patterns one plaintext has one cipher, and no two
                groups share an `L` value.

Injectivity is what an earlier attempt lacked, and without it the
commonest pattern `80 90 98 e8` reads as 400 units a second -- 400.0
packs to `00 00 c8 43`, which would need the plaintext `0x00` to
encipher as `0x80` at one position and `0x90` at another. A substitution
does not do that, and the four groups that attempt produced disagreed
with every other route.

The candidate speeds are *measured*, not invented. A grid of made-up
values returned speeds like 492.5 and 506; 16.19 decodes fully, so its
own distribution is the list -- 292 values led by 350.000 at 34.8% of
first entries, which is what a minion walks.

Then propagate. A pattern left with one candidate contributes its
groups, those prune the others, and round it goes; nothing is taken
while two candidates remain, so each entry is forced rather than chosen.

The result is checked on what the solver never looked at: the shape of
the decoded distribution. 16.19 measures 63.4% whole numbers over
0..504. Thirteen groups put 350 top with 1,228 entries, 70.8% whole,
over 0..510 -- and the groups came from two patterns, so the peak at a
minion's own speed is held out. A wrong `L` cannot make 350.0 out of
mantissa bytes.

One group is argued outside the solver and seeded. For a pattern whose
mantissa is zero below its top byte -- ciphers `0x80, 0x80, X, 0xe8` --
the float works out to exactly twice that byte above 256, so every such
pattern carries a whole speed and the byte is the speed halved. A minion
walks 350, which 16.19 confirms is the commonest speed by a wide margin,
and among 16.20's patterns of that shape the commonest is `808076e8`
with 1,228 entries. 350/2 = 175, whose group is 22 and whose low three
bits must appear as `0x60` -- and `0x76` does. So `L(22) = 22`, with no
candidate grid anywhere in the argument.

Usage, on movement dumps of the two patches (`rofl-x explore
--dump-netid <n> --dump <file>`):

    python scripts/solve_1620_speeds.py --known mv1267.jsonl \
        --known-table table1619.json --unknown mv1620.jsonl \
        --out L1620.json
"""
from __future__ import annotations

import argparse
import collections
import json
import struct

#: Groups settled without any speed, in `docs/EXPLORE.md`: the two bytes
#: present in every block, the entry stride, and the chain.
FIRM = {0: 0, 1: 3, 2: 2, 8: 8, 30: 30, 31: 1}
rot_top = lambda p: ((p + 4) & 7) << 5
grp = lambda p: ((p + 4) & 0xFF) >> 3


def plain_of(L):
    """Cipher to plaintext, for the groups `L` covers."""
    out = {}
    for p in range(256):
        q = (p + 4) & 0xFF
        if (q >> 3) in L:
            out[((q & 7) << 5) | L[q >> 3]] = p
    return out


def first_speeds(path, forward, zero, top, flags_one):
    """The speed bytes of each block's first single-waypoint entry.

    A single waypoint is the only shape whose fields sit at a fixed
    offset, and the head is located by the (T(0x00), T(0x40)) pair,
    which needs no table.
    """
    out = collections.Counter()
    with open(path, encoding="utf-8") as f:
        for line in f:
            p = bytes.fromhex(json.loads(line)["payload"])
            n = len(p)
            a, b = (zero, top) if forward else (top, zero)
            for i in range(4, n - 1):
                if p[i] != a or p[i + 1] != b:
                    continue
                h = i - 4 if forward else n - 6 - i
                if h < 0:
                    break
                at = ((lambda j: p[h + j] if 0 <= h + j < n else None) if forward
                      else (lambda j: p[n - 1 - h - j]
                            if 0 <= n - 1 - h - j < n else None))
                if at(0) != flags_one or at(9) is None:
                    break
                out[bytes(at(6 + k) for k in range(4))] += 1
                break
    return out


def candidates(cipher, cands, L):
    """Candidate speeds for one pattern, with the groups each implies."""
    taken = set(L.values())
    out = []
    for s, (want, weight) in cands.items():
        ok = True
        for c, p in zip(cipher, want):
            if (c & 0xE0) != rot_top(p):
                ok = False
                break
            g = grp(p)
            if g in L and (c & 0x1F) != L[g]:
                ok = False
                break
        if not ok:
            continue
        seen = {}
        for c, p in zip(cipher, want):
            if seen.setdefault(p, c) != c:
                ok = False
                break
        if not ok or len(set(seen.values())) != len(seen):
            continue
        add = {}
        for c, p in zip(cipher, want):
            g = grp(p)
            if g in L:
                continue
            if add.setdefault(g, c & 0x1F) != (c & 0x1F):
                ok = False
                break
        if not ok or len(set(add.values())) != len(add):
            continue
        if any(v in taken for v in add.values()):
            continue
        out.append((weight, s, add))
    out.sort(reverse=True)
    return out


def validate(path, L, zero, top, flags_one):
    """Decode every speed and describe the distribution."""
    plain = plain_of(L)
    vals, seen, read = collections.Counter(), 0, 0
    for cipher, k in first_speeds(path, True, zero, top, flags_one).items():
        seen += k
        b = [plain.get(c) for c in cipher]
        if None in b:
            continue
        read += k
        vals[round(struct.unpack("<f", bytes(b))[0], 3)] += k
    tot = sum(vals.values())
    if not tot:
        return None
    whole = sum(k for v, k in vals.items() if abs(v - round(v)) < 1e-3)
    return {"readable": read / max(seen, 1), "whole": whole / tot,
            "range": (min(vals), max(vals)), "top": vals.most_common(6)}


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--known", required=True,
                    help="movement dump of a patch that decodes, for the "
                         "candidate speeds")
    ap.add_argument("--known-table", required=True)
    ap.add_argument("--unknown", required=True)
    ap.add_argument("--min-entries", type=int, default=2,
                    help="the lowest pattern count to consider; the solver "
                         "sweeps down to it from 40, because a group settled "
                         "on a busy pattern is what makes a quiet one forced")
    ap.add_argument("--seed", default="",
                    help="groups argued outside the solver, as g:v,g:v. "
                         "L(22)=22 is the one worth seeding; the module "
                         "docstring gives the argument")
    ap.add_argument("--out")
    args = ap.parse_args()

    T = {int(k, 16): int(v, 16)
         for k, v in json.load(open(args.known_table, encoding="utf-8")).items()}
    P = {c: p for p, c in T.items()}
    cands = {}
    for cipher, k in first_speeds(args.known, False,
                                  T[0x00], T[0x40], T[0x02]).items():
        b = [P.get(c) for c in cipher]
        if None not in b:
            cands[struct.unpack("<f", bytes(b))[0]] = (bytes(b), k)
    print(f"candidate speeds measured on the known patch: {len(cands)}")
    lead = sorted(cands.items(), key=lambda kv: -kv[1][1])[:3]
    for s, (_, k) in lead:
        print(f"   {s:9.3f} u/s, {k} first entries")

    # The unknown patch's own T(0x00)/T(0x40), from the two bytes present
    # in every block: see scripts/table_structure.py.
    pays = [bytes.fromhex(json.loads(l)["payload"])
            for l in open(args.unknown, encoding="utf-8")]
    present, overall = collections.Counter(), collections.Counter()
    for p in pays:
        present.update(set(p))
        overall.update(p)
    both = sorted((b for b, k in present.items() if k == len(pays)),
                  key=lambda b: -overall[b])
    if len(both) != 2:
        print(f"!! {len(both)} bytes present in every block, expected two")
        return 1
    zero, top = both
    L = dict(FIRM)
    for item in filter(None, args.seed.split(",")):
        g, v = (int(x) for x in item.split(":"))
        L[g] = v
    assert len(set(L.values())) == len(L), "the seed must keep L a bijection"
    flags_one = ((((2 + 4) & 7) << 5) | L[grp(2)])
    print(f"   T(0x00) = {zero:#04x}, T(0x40) = {top:#04x}, "
          f"T(2) = {flags_one:#04x}")

    all_pats = first_speeds(args.unknown, True, zero, top, flags_one)
    print(f"   {len(all_pats)} distinct speed patterns\n")

    # Sweep the threshold down. A quiet pattern is as good as a busy one
    # once it is forced -- the constraints do not count entries -- but a
    # group settled on a busy pattern is what turns a quiet one from
    # ambiguous into forced, so the busy ones have to go first.
    for th in (40, 20, 10, 4, args.min_entries):
        if th < args.min_entries:
            continue
        pats = [(k, c) for c, k in all_pats.most_common() if k >= th]
        amb = 0
        for _ in range(30):
            forced, amb = [], 0
            for k, c in pats:
                cs = candidates(c, cands, L)
                if len(cs) == 1:
                    forced.append((k, c, cs[0]))
                elif cs:
                    amb += 1
            added = {}
            for k, cipher, (w, s, add) in sorted(forced, reverse=True):
                if any(add.get(g, v) != v for g, v in added.items() if g in add):
                    continue
                for g, v in add.items():
                    if (g not in L and g not in added
                            and v not in set(L.values()) | set(added.values())):
                        added[g] = v
                        print(f"   {cipher.hex()} ({k:>5} entries) "
                              f"= {s:g} u/s  ->  L({g}) = {v}")
            if not added:
                break
            L.update(added)
        print(f"   threshold {th:>3}: {len(pats)} patterns, {amb} ambiguous, "
              f"L has {len(L)}/32")

    assert len(set(L.values())) == len(L), "L must stay a bijection"
    print(f"\nL = " + ", ".join(f"{g}:{v}" for g, v in sorted(L.items())))
    print(f"   open groups: {[g for g in range(32) if g not in L]}")

    print("\nheld-out check -- the decoded distribution, which the solver "
          "never looked at:")
    base = validate(args.unknown, FIRM, zero, top, flags_one)
    got = validate(args.unknown, L, zero, top, flags_one)
    for name, r in (("firm only", base), (f"{len(L)} groups", got)):
        if not r:
            continue
        print(f"   {name:<14} readable {r['readable']:6.1%}   "
              f"whole {r['whole']:6.1%}   range {r['range'][0]:.1f}"
              f"..{r['range'][1]:.1f}")
        print(f"                  commonest " + ", ".join(
            f"{v:g}({k})" for v, k in r["top"]))
    print("   the known patch measures 63.4% whole over 0..504, led by "
          "350.000 -- a minion")

    if args.out:
        json.dump({str(g): v for g, v in sorted(L.items())},
                  open(args.out, "w", encoding="utf-8"), indent=0)
        print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
