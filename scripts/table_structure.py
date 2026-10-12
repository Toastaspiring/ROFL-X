#!/usr/bin/env python3
"""What shape is a class's substitution, and can it be fitted from a few pairs?

`docs/DECODING.md` establishes that a payload is its plaintext run
through one 256-entry byte substitution per class. That says nothing
about how the substitution itself is built, and it turns out not to be
built the same way for every class.

Measured on 16.9, the cast class's table is **affine over the bits**:
every entry is `T(0) XOR perm(plaintext)`, where `perm` permutes the
eight bit positions. Five other classes' tables fit nothing of the
kind. The difference matters enormously, because an affine table is not
256 free choices but 16 bits of structure:

  * nine pairs whose plaintext differences are linearly independent
    determine all 256 entries outright, and
  * the whole family has only 8! * 256 = 10,321,920 members, so it can
    be enumerated against whatever few constraints a packed patch
    offers -- a range, an enum, a parse that has to come out exact.

So on a patch with no binary, the first question about a class is not
"what are its 256 entries" but "is it affine", and that question can be
answered from a handful of pairs.

Usage, on the output of `rofl-x decode --netid N --layout`:

    rofl-x decode -r <replay> --netid 985 --layout > layout.txt
    python scripts/table_structure.py layout.txt
    python scripts/table_structure.py layout.txt --fit 9

`--fit k` holds back all but `k` pairs and reports how much of the rest
the fit predicts, which is the honest measure of "nine pairs are
enough".
"""
from __future__ import annotations

import argparse
import re
import sys

ROW = re.compile(r"^    ([0-9a-f]{2})  ((?:[0-9a-f-]{2} ?){16})$", re.M)


def parse_tables(text: str) -> list[dict[int, int]]:
    """Every `plaintext -> payload byte` table printed in a layout report."""
    tables, current, last_base = [], {}, -1
    for base, cells in ROW.findall(text):
        b = int(base, 16)
        if b <= last_base and current:
            tables.append(current)
            current = {}
        last_base = b
        for i, cell in enumerate(cells.split()):
            if cell != "--":
                current[b + i] = int(cell, 16)
    if current:
        tables.append(current)
    return tables


def fit_affine(table: dict[int, int], pairs: dict[int, int] | None = None):
    """Fit `T(p) = T(0) XOR sum of basis images` and score it on `table`.

    The basis comes from `pairs` when given -- that is how a fit from a
    few known entries is tested -- and otherwise from the table itself.
    Returns None when the basis cannot be formed, which is itself worth
    reporting: it means the pairs on hand do not span the byte.
    """
    src = pairs if pairs is not None else table
    if 0 not in src or any((1 << k) not in src for k in range(8)):
        return None
    t0 = src[0]
    basis = [src[1 << k] ^ t0 for k in range(8)]
    # A permutation of the bit positions is the special case seen in the
    # wild; a general invertible matrix would also be affine.
    is_perm = all(b.bit_count() == 1 for b in basis) and len(set(basis)) == 8

    def predict(p: int) -> int:
        out = t0
        for k in range(8):
            if p >> k & 1:
                out ^= basis[k]
        return out

    ok = sum(1 for p, c in table.items() if predict(p) == c)
    return ok, len(table), is_perm, t0, basis, predict


def other_shapes(table: dict[int, int]) -> list[tuple[str, int]]:
    """The structures worth ruling out before calling a table arbitrary."""
    n = len(table)
    out = []
    best = 0
    for a in range(1, 256, 2):          # odd multipliers are the invertible ones
        for b in range(256):
            best = max(best, sum(1 for p, c in table.items() if (a * p + b) & 0xFF == c))
    out.append(("affine mod 256", best))
    rev = lambda x: int(f"{x:08b}"[::-1], 2)
    rot = lambda x, r: ((x << r) | (x >> (8 - r))) & 0xFF
    for name, f in (("rotate, XOR k", rot), ("bit-reverse, rotate, XOR k",
                                             lambda x, r: rot(rev(x), r))):
        best = 0
        for r in range(8):
            for k in range(256):
                best = max(best, sum(1 for p, c in table.items() if f(p, r) ^ k == c))
        out.append((name, best))
    out.append(("identity-like (fixed points)", sum(1 for p, c in table.items() if p == c)))
    return [(name, score) for name, score in out]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("layout", help="output of `decode --netid N --layout`")
    ap.add_argument("--fit", action="store_true",
                    help="fit from only T(0) and the eight single-bit plaintexts, "
                         "then predict the other 247 -- the honest test of "
                         "whether nine pairs are enough")
    args = ap.parse_args()

    text = open(args.layout, encoding="utf-8").read()
    tables = parse_tables(text)
    if not tables:
        print("no table in that report: the class's fields were not pinned down")
        return 1
    netid = re.search(r"netid (\d+)", text)
    print(f"netid {netid.group(1) if netid else '?'}: "
          f"{len(tables)} table(s) in the report")

    for i, table in enumerate(tables):
        print(f"\ntable {i + 1}: {len(table)} of 256 entries")
        a = fit_affine(table)
        if a is None:
            print("  cannot test affinity: the table lacks T(0) or a power-of-two entry")
            continue
        ok, total, is_perm, t0, basis, _ = a
        print(f"  affine over the bits : {ok} of {total}"
              f"{'  -- EXACT' if ok == total else ''}")
        print(f"  T(0) = 0x{t0:02x}, bit images "
              f"{' '.join(f'0x{b:02x}' for b in basis)}")
        print(f"  a permutation of the bit positions: {is_perm}")
        if ok != total:
            print("  so not affine; other shapes, for comparison:")
            for name, score in other_shapes(table):
                print(f"    {name:<30} {score} of {total}")
            continue
        if args.fit:
            # Nine pairs can only determine the map if their plaintext
            # differences span the byte, so the spanning set is the one
            # to test: zero and the eight single bits.
            wanted = [0] + [1 << k for k in range(8)]
            pairs = {p: table[p] for p in wanted if p in table}
            b = fit_affine(table, pairs)
            if b is None:
                print(f"  fit from {len(pairs)} pairs: the table is missing one of"
                      " them, so the byte is not spanned")
            else:
                *_, predict = b
                rest = {p: c for p, c in table.items() if p not in pairs}
                right = sum(1 for p, c in rest.items() if predict(p) == c)
                print(f"  fit from {len(pairs)} pairs (zero and the single bits):"
                      f" predicts {right} of {len(rest)} held-out entries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
