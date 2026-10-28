#!/usr/bin/env python3
"""Measure a class's entry length by autocorrelation, assuming nothing.

Every other probe in `scripts/` needs a field to anchor on — the net id's
`0x40` top byte, the move speed's exponent, a pair of universal bytes.
On a patch that rearranged its entry those all return noise, and worse,
they return it without saying why. This needs no field at all.

A buffer of similar records repeats: the same field of two neighbouring
entries holds similar values, a zero or a shared high byte or the same
constant, so bytes exactly one record apart agree more often than
chance. Measuring that per lag finds the record length directly.

The baseline is not one in 256, because these payloads are far from
flat — `T(0x00)` alone is a fifth of them — so it is the sum of squared
byte frequencies, about 0.06 here, and what matters is which lags stand
above their own neighbours.

Measured on three patches:

| patch | strongest lag | against baseline | lag 14 |
| --- | --- | --- | --- |
| 16.19 | **14** | 2.97x | — |
| 16.20 | **14** | 2.71x | — |
| 16.21 | **28** | 2.82x | 0.42x |

So 16.19 and 16.20 share a 14-byte entry, which is what their format
documents: a ten-byte head and two 16-bit coordinates. **16.21's is 28,
and its lag 14 is below baseline** rather than merely unremarkable — the
entry did not grow by a field, it is a different size, and every probe
that assumed 14 was reading half of one.

Both patches also show the harmonic at twice their stride, which is what
a periodic structure does and a spurious peak does not.

Usage:

    python scripts/entry_stride.py --dump mv1621.jsonl \\
        --dump mv1267.jsonl --max-lag 40
"""
from __future__ import annotations

import argparse
import collections
import json


def measure(path, limit, max_lag):
    """Agreement rate at each lag, and the flat-data baseline."""
    hits = [0] * (max_lag + 1)
    pairs = [0] * (max_lag + 1)
    freq = collections.Counter()
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i >= limit:
                break
            p = bytes.fromhex(json.loads(line)["payload"])
            freq.update(p)
            n = len(p)
            for lag in range(1, max_lag + 1):
                if n <= lag:
                    break
                m = n - lag
                pairs[lag] += m
                hits[lag] += sum(1 for k in range(m) if p[k] == p[k + lag])
    total = sum(freq.values())
    if not total:
        return None, None
    base = sum((v / total) ** 2 for v in freq.values())
    rate = [hits[l] / pairs[l] if pairs[l] else 0.0 for l in range(max_lag + 1)]
    return rate, base


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dump", action="append", required=True,
                    help="a movement class dump; repeatable, so a patch whose "
                         "entry length is known can sit beside one whose is not")
    ap.add_argument("--max-lag", type=int, default=40)
    ap.add_argument("--limit", type=int, default=20000,
                    help="blocks per dump; the measurement is quadratic in "
                         "the lag range, so this keeps it quick")
    args = ap.parse_args()

    for path in args.dump:
        rate, base = measure(path, args.limit, args.max_lag)
        if rate is None:
            print(f"{path}: nothing to measure")
            continue
        peaks = [(rate[l] / base, l) for l in range(2, args.max_lag)
                 if rate[l] > rate[l - 1] and rate[l] > rate[l + 1]]
        peaks.sort(reverse=True)
        best = max(range(1, args.max_lag + 1), key=lambda l: rate[l])
        print(f"{path}: baseline {base:.3f} from the byte frequencies")
        print(f"   strongest lag {best} at {rate[best] / base:.2f}x")
        print("   lags above both neighbours: "
              + ", ".join(f"{l} ({r:.2f}x)" for r, l in peaks[:6]))
        # a real period repeats at twice the lag; a fluke does not
        twice = 2 * best
        if twice <= args.max_lag:
            print(f"   the harmonic at {twice} is "
                  f"{rate[twice] / base:.2f}x"
                  + ("   -- consistent with a period" if rate[twice] > base
                     else "   -- absent, so treat the peak with suspicion"))
        half = best // 2
        if half >= 2:
            print(f"   half of it, {half}, is {rate[half] / base:.2f}x"
                  + ("   -- below baseline, so the record really is "
                     f"{best} and not {half}" if rate[half] < base else ""))
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
