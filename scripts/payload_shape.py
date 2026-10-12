#!/usr/bin/env python3
"""Find a class's bounded fields in its payload, without its table.

`rofl-x fields` profiles a class by decoding it, so it needs the
patch's binary. This gets part of the same answer from the payload
bytes alone, which is all a packed patch gives.

The trick is that a substitution is a *permutation* of byte values, so
it preserves how many distinct values a position takes. A field that
is bounded therefore stays bounded through the obfuscation:

  an enum of two values          -> 2 distinct ciphers
  a float confined to the map    -> its leading byte is sign plus the
                                    top seven exponent bits, which for
                                    a value in [0, 16384) can only be
                                    0x00 or 0x3f..0x46: 9 ciphers
  a coordinate triple            -> three such positions, four apart

Everything else -- mantissas, net ids, hashes, counters -- takes most
of the 256 values and is invisible to this. So a narrow position is a
bounded field, and three narrow positions four bytes apart are very
likely three floats.

That makes it a layout comparison that works across a patch gap: run
it on a class that decodes and on the class a packed patch is supposed
to match, and see whether the signature is in the same places.

Validated on 16.9, where the answer is known: netid 985's three
trailing coordinates show up at exactly the positions `decode
--layout` assigns them, and nothing else in the last sixteen bytes is
narrow.

Usage:

    rofl-x explore -r <replay> [-r ...] --dump-netid 985 --dump a.jsonl
    python scripts/payload_shape.py a.jsonl [b.jsonl ...]
"""
from __future__ import annotations

import argparse
import collections
import json
import sys

# A float in [0, 16384) has a leading big-endian byte of 0x00 or
# 0x3f..0x46, so a coordinate's exponent position shows nine ciphers.
FLOAT_CEILING = 12
# Below this a position is an enum rather than a float exponent.
ENUM_CEILING = 3


def positions(path: str):
    """Distinct cipher counts per position, at the commonest length.

    One length at a time, because a variable-length payload puts its
    fields in different places and pooling lengths blurs exactly the
    signal being looked for.
    """
    pays = [bytes.fromhex(json.loads(line)["payload"])
            for line in open(path, encoding="utf-8")]
    if not pays:
        return None
    lens = collections.Counter(len(p) for p in pays)
    common, n = lens.most_common(1)[0]
    fixed = [p for p in pays if len(p) == common]
    counts = [len({p[i] for p in fixed}) for i in range(common)]
    return pays, lens, common, n, fixed, counts


def report(path: str) -> dict[int, int] | None:
    got = positions(path)
    if got is None:
        print(f"{path}: no blocks")
        return None
    pays, lens, common, n, fixed, counts = got
    print(f"\n{path}: {len(pays)} blocks, {len(lens)} distinct lengths, "
          f"commonest {common} bytes ({n} blocks)")
    narrow = {i: c for i, c in enumerate(counts) if c <= FLOAT_CEILING}
    if not narrow:
        print("  no bounded field found: every position takes many values")
        return narrow
    print(f"  {'pos':>5} {'end':>6} {'ciphers':>8}  reading")
    for i, c in narrow.items():
        kind = "enum" if c <= ENUM_CEILING else "float exponent, or a wider enum"
        print(f"  {i:>5} {i - common:>6} {c:>8}  {kind}")
    triples = [i for i in narrow if i + 4 in narrow and i + 8 in narrow]
    if triples:
        print(f"  three narrow positions four apart at {triples}"
              " -- a float triple, so probably a position")
    else:
        print("  no three narrow positions four apart")
    return narrow


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dumps", nargs="+", help="--dump-netid outputs to compare")
    args = ap.parse_args()

    found = [(path, report(path)) for path in args.dumps]
    if len(found) < 2:
        return 0

    # Comparing two patches' versions of the same class is the point:
    # the signature is what carries across a patch gap, since the
    # tables do not.
    print("\ncomparing signatures:")
    for (pa, na), (pb, nb) in zip(found, found[1:]):
        if na is None or nb is None:
            continue
        shared = sorted(set(na) & set(nb))
        print(f"  {pa} vs {pb}")
        print(f"    both narrow at   : {shared}")
        print(f"    only the first   : {sorted(set(na) - set(nb))}")
        print(f"    only the second  : {sorted(set(nb) - set(na))}")
        if shared:
            print("    a shared signature near the payload start is the field order"
                  " surviving; a position missing on one side is a field that moved"
                  " or went away.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
