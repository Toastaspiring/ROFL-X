#!/usr/bin/env python3
"""Measure how recoverable a movement class's coordinate low byte is.

The high byte of a coordinate falls out easily: it barely moves between
packets, so the pairs seen changing lay the plaintext alphabet out in
order (docs/EXPLORE.md section 3).  The low byte does not, and this
script is why -- it measures the three things that decide whether any
given method can work, and scores the best method found so far against
a patch where the true table is known.

Run it on a patch ROFL-X can decode, so that every number has an answer
to be checked against.  Produce the two inputs like this:

    # the class's blocks, framing only
    rofl-x explore -r <replay> [-r ...] --dump-netid 916 --dump blocks.jsonl

    # the true table, from the payload against the deserializer's buffer
    rofl-x decode -r <replay> --netid 916 --jsonl d.jsonl --heap
    python scripts/lowbyte_probe.py --true-from d.jsonl > true.json

    python scripts/lowbyte_probe.py --blocks blocks.jsonl --true true.json

What it reports, and what each number means:

  step distribution  How far a unit moves between consecutive packets,
                     in coordinate units.  A method that walks the
                     alphabet one step at a time needs this to be
                     peaked at +/-1.  Measured on 16.9 it is not: it is
                     bimodal, 37.8 % exactly zero against 55.2 % over
                     six, with +/-1 only 16.2 % of the rest.  That is
                     the measurement that rules the chain out.  (On the
                     lone-waypoint subset it reads 64.2 % zero, since a
                     one-waypoint packet is often a unit standing
                     still.  The conclusion is the same either way.)

  band width         Among pairs close enough in time to be useful, how
                     far apart the two low bytes actually are.  This is
                     the width of the only structure the low byte has,
                     and it sets how much a locality method can know.

  seriation score    Treat "these two bytes are close in value" as a
                     graph and recover the circular order spectrally,
                     anchored on the high-byte chain.  Uses locality
                     only, and throws the step sizes away.  Reported
                     unrefined here; adding pairwise local search to it
                     reached a median of 8, far behind the below.

  speed score        The best method so far, and it keeps the sizes: a
                     unit travels at its own fixed speed, so between two
                     packets dt apart it covers speed * dt.  Fit one
                     speed per unit by least squares, and search for the
                     table that makes a unit's own fits agree.

Both are scored byte by byte against the true table, so the comparison
is measured rather than argued.  The search cost tracks that score
closely enough to stand in for it on a patch that has no true table --
which is the only reason any of this transfers.  Anyone trying a new
method should add it here and report the same numbers.

Where it got to on 16.9, fed by the parser rather than by payload
shape, at dt <= 1.5 s:

    exact 53 of 256,  within 4: 207,  within 16: 256,
    median error 2 plaintext steps,  90th percentile 5

So every entry is within 16 and half are within 2, which is +/-4 map
units on a map 15,000 across -- and because the high byte is exact, no
entry however wrong can move a position by more than 256.  The cost
stops falling at about 85,000 swaps, so what is left is the model's
limit and not the search's: a table this accurate is still only 53
entries *right*, and making it exact needs a constraint this objective
does not have.  Three candidates for that are measured and rejected in
docs/EXPLORE.md -- the path deltas give no exact arithmetic.
"""
from __future__ import annotations

import argparse
import collections
import json
import sys

# A coordinate is stored as a signed 16-bit value; the map position is
# that doubled, plus an origin (docs/PACKETS.md, WaypointGroup).
ORIGIN_X, ORIGIN_Z = 7358, 7412
# Plaintexts the high-byte chain reaches on 16.9 and 16.19 alike.
CHAIN_LO, CHAIN_HI = 0x0D, 0xF3


def true_table(path: str) -> dict[int, int]:
    """Recover a class's table by lining its payload up against the
    buffer its own deserializer filled, back to front."""
    table: dict[int, int] = {}
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        w = r.get("words", {})
        lo, hi, ln = w.get("0x018"), w.get("0x01c"), w.get("0x020")
        if not (lo and hi and ln):
            continue
        addr = (int(hi, 16) << 32) | int(lo, 16)
        blk = next((b for b in r.get("heap", []) if int(b["addr"], 16) == addr), None)
        if blk is None:
            continue
        buf = bytes.fromhex(blk["bytes"])[: int(ln, 16)]
        pay = bytes.fromhex(r["payload"])
        for j, plain in enumerate(buf):
            table.setdefault(plain, pay[len(pay) - 1 - j])
    return table


def parse_entries(payload: bytes, header: int, known: dict[int, int]):
    """Every entry in one payload's buffer, or None if it does not parse.

    The payload is the entry buffer written backwards behind a header of
    varints, so buffer index `j` is `payload[-1 - j]`, and the header
    length is not known in advance -- the caller tries each.  Only the
    structural bytes need reading: the flags' low byte for the waypoint
    count, and the bitmap that says which later coordinates are
    one-byte deltas.  Those are small numbers, so the high-byte chain
    already names them, which is the whole reason this works without
    the table.

    Yields each entry's net id bytes (to group a unit's packets without
    reading the id) and its first waypoint, which is where the unit is.
    """
    n = len(payload)
    limit = n - header
    if limit <= 0:
        return None
    at = lambda j: payload[n - 1 - j]
    j, out = 0, []
    while j < limit:
        if j + 10 > limit:
            return None
        flags = known.get(at(j))
        if flags is None or flags >> 1 == 0:
            return None
        count = flags >> 1
        uid = bytes(at(j + 2 + k) for k in range(4))
        j += 10 + (flags & 1)
        if count > 1:
            width = ((count - 2) >> 2) + 1
            if j + width > limit:
                return None
            bitmap = [known.get(at(j + b)) for b in range(width)]
            if any(b is None for b in bitmap):
                return None
            j += width
            delta = lambda i: bitmap[i >> 3] & (1 << (i & 7)) != 0
        else:
            delta = lambda i: False
        first = []
        for i in range(count):
            for axis in (0, 1):
                if i and delta(2 * (i - 1) + axis):
                    if j + 1 > limit:
                        return None
                    j += 1
                else:
                    if j + 2 > limit:
                        return None
                    if i == 0:
                        first += [at(j), at(j + 1)]   # low cipher, high cipher
                    j += 2
        if len(first) != 4:
            return None
        out.append((uid, first))
    return out if j == limit else None


def parsed_tracks(path: str, inv: dict[int, int]):
    """Positions per unit from *every* entry, not just lone waypoints.

    The shape-based reader below sees only single-waypoint blocks, which
    is a quarter of the file.  Parsing the buffer instead reaches every
    entry of every payload that parses, and each entry's first waypoint
    is a position -- about seven times as many, over twice as many
    units.  That difference is most of the accuracy in the table.
    """
    known = {c: p for c, p in inv.items() if p <= CHAIN_LO or p >= CHAIN_HI}
    tracks = collections.defaultdict(list)
    parsed = 0
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        pay = bytes.fromhex(r["payload"])
        hits = [e for e in (parse_entries(pay, h, known) for h in range(1, 13)) if e]
        if len(hits) != 1:          # ambiguous header length: take nothing
            continue
        parsed += 1
        for uid, (lo_x, hi_x, lo_z, hi_z) in hits[0]:
            tracks[(r.get("replay"), uid)].append((r["t"], hi_x, lo_x, hi_z, lo_z))
    for v in tracks.values():
        v.sort()
    return tracks, parsed


def unit_tracks(path: str, inv: dict[int, int]):
    """Single-waypoint blocks per unit, in time order.

    Only an entry with one waypoint puts its coordinates at a fixed
    offset from the payload end; any other shape shifts them by a
    bitmap byte.  Kept as a fallback for when the buffer will not
    parse.  Units are keyed by the cipher of their net id, which groups
    a unit's own packets together without being able to read the id.
    """
    tracks = collections.defaultdict(list)
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        pay = bytes.fromhex(r["payload"])
        if len(pay) < 14:
            continue
        at = lambda k: pay[len(pay) - k]
        if inv.get(at(1)) != 2:
            continue
        tracks[(r.get("replay"), at(3), at(4))].append(
            (r["t"], at(12), at(11), at(14), at(13))
        )
    for v in tracks.values():
        v.sort()
    return tracks


def pairs(tracks, dt_max: float):
    """Consecutive packets for one unit, per axis, within `dt_max`."""
    for v in tracks.values():
        for a, b in zip(v, v[1:]):
            dt = b[0] - a[0]
            if 0 < dt <= dt_max:
                yield dt, a[1], a[2], b[1], b[2]
                yield dt, a[3], a[4], b[3], b[4]


def report_steps(tracks, inv) -> None:
    hist = collections.Counter()
    total = 0
    for dt, ha, la, hb, lb in pairs(tracks, 3.0):
        if ha not in inv or hb not in inv:
            continue
        va = inv[ha] * 256 + inv.get(la, 0)
        vb = inv[hb] * 256 + inv.get(lb, 0)
        d = ((vb - va + 32768) % 65536) - 32768
        hist[d if abs(d) <= 6 else "over 6"] += 1
        total += 1
    print(f"step distribution over {total} observations, dt <= 3 s")
    zero = hist[0]
    small = sum(v for k, v in hist.items() if isinstance(k, int) and k != 0)
    print(f"  exactly zero : {zero:>7} ({100 * zero / max(total, 1):.1f} %)")
    print(f"  within 6     : {small:>7} ({100 * small / max(total, 1):.1f} %)")
    print(f"  over 6       : {hist['over 6']:>7} ({100 * hist['over 6'] / max(total, 1):.1f} %)")
    pm1 = hist[1] + hist[-1]
    print(f"  of the small nonzero steps, +/-1 is {100 * pm1 / max(small, 1):.1f} %")
    print("  a chain needs that last number to dominate. It does not.")


def report_band(tracks, inv) -> None:
    print("\nband width: among pairs sharing a high byte, how far apart the low bytes are")
    for dt_max in (0.4, 0.7, 1.5, 3.0):
        edges: set[tuple[int, int]] = set()
        widths = []
        for dt, ha, la, hb, lb in pairs(tracks, dt_max):
            if ha != hb or la == lb or ha not in inv:
                continue
            edges.add((min(la, lb), max(la, lb)))
            if la in inv and lb in inv:
                d = abs(inv[la] - inv[lb])
                widths.append(min(d, 256 - d))
        widths.sort()
        med = widths[len(widths) // 2] if widths else 0
        p90 = widths[int(len(widths) * 0.9)] if widths else 0
        print(
            f"  dt <= {dt_max:>4}s: {len(edges):>6} edges, "
            f"true separation median {med:>3}, 90th {p90:>3}"
        )


def report_seriation(tracks, inv, dt_max: float) -> None:
    try:
        import numpy as np
        from scipy.optimize import linear_sum_assignment
        from scipy.sparse import coo_matrix
        from scipy.sparse.linalg import eigsh
    except ImportError:
        print("\nseriation: needs numpy and scipy; skipped")
        return
    anchor = {c: p for c, p in inv.items() if p <= CHAIN_LO or p >= CHAIN_HI}
    acc: dict[tuple[int, int], float] = collections.defaultdict(float)
    for dt, ha, la, hb, lb in pairs(tracks, dt_max):
        if ha != hb or la == lb or ha not in anchor:
            continue
        acc[(min(la, lb), max(la, lb))] += 1.0 / max(dt, 0.05)
    if len(acc) < 64:
        print("\nseriation: too few edges; skipped")
        return
    nodes = sorted({x for e in acc for x in e})
    idx = {b: i for i, b in enumerate(nodes)}
    n = len(nodes)
    r, c, w = [], [], []
    for (a, b), k in acc.items():
        r += [idx[a], idx[b]]
        c += [idx[b], idx[a]]
        w += [k, k]
    W = coo_matrix((w, (r, c)), shape=(n, n)).tocsr()
    lap = np.diag(np.asarray(W.sum(axis=1)).ravel()) - W.toarray()
    vals, vecs = eigsh(lap, k=3, sigma=-1e-6, which="LM")
    o = np.argsort(vals)
    angle = np.arctan2(vecs[:, o[2]], vecs[:, o[1]])

    known = [(idx[cb], p) for cb, p in anchor.items() if cb in idx]
    best = None
    for flip in (1, -1):
        for shift in np.linspace(-np.pi, np.pi, 1441):
            pos = (((angle * flip) + shift) % (2 * np.pi)) / (2 * np.pi) * 256.0
            err = sum(min(abs(pos[i] - p), 256 - abs(pos[i] - p)) for i, p in known)
            if best is None or err < best[0]:
                best = (err, flip, shift)
    _, flip, shift = best
    pos = (((angle * flip) + shift) % (2 * np.pi)) / (2 * np.pi) * 256.0
    cost = np.abs(pos[:, None] - np.arange(256)[None, :])
    cost = np.minimum(cost, 256 - cost)
    for i, p in known:
        cost[i] = 1e6
        cost[i][p] = 0.0
    _, col = linear_sum_assignment(cost)
    dev = sorted(
        min(abs(int(col[i]) - inv[nodes[i]]), 256 - abs(int(col[i]) - inv[nodes[i]]))
        for i in range(n)
        if nodes[i] in inv
    )
    exact = sum(1 for d in dev if d == 0)
    print(f"\nseriation at dt <= {dt_max}s, spectral only, scored against truth:")
    print(f"  exactly right : {exact} of {len(dev)}  ({len(known)} of those are anchors)")
    print(f"  within 4      : {sum(1 for d in dev if d <= 4)}")
    print(f"  median error  : {dev[len(dev) // 2]} plaintext steps, "
          f"so about {2 * dev[len(dev) // 2]} map units")
    print(f"  90th pct      : {dev[int(len(dev) * 0.9)]}")
    print("  not exact, so not usable as a table yet.")


def report_speed(tracks, inv, dt_max: float, iters: int) -> None:
    """Make every unit move at one speed, and see which table that wants.

    Seriation only knows that two consecutive low bytes are *close*.  The
    size of the step says far more: a unit travels at its own fixed
    speed, so between two packets dt apart it covers speed * dt and no
    more.  The high byte is already known from the chain, so the step is
    exact in the unknowns -- dv = (high step) * 256 + L[lo2] - L[lo1] --
    and the right table is the one that leaves each unit's implied
    speeds agreeing with a single fitted speed.
    """
    try:
        import numpy as np
    except ImportError:
        print("\nconstant speed: needs numpy; skipped")
        return
    anchor = {c: p for c, p in inv.items() if p <= CHAIN_LO or p >= CHAIN_HI}
    units: dict[object, int] = {}
    recs = []
    for key, v in tracks.items():
        for a, b in zip(v, v[1:]):
            dt = b[0] - a[0]
            # A tighter window is a tighter bound on how far the unit
            # can have gone, and the band measured above is narrowest
            # at the short gaps, so this is deliberately not 3 s.
            if not (0 < dt <= dt_max):
                continue
            for ha, la, hb, lb in ((a[1], a[2], b[1], b[2]), (a[3], a[4], b[3], b[4])):
                pa, pb = anchor.get(ha), anchor.get(hb)
                if pa is None or pb is None:
                    continue
                step = ((pb - pa + 128) % 256) - 128
                if abs(step) > 1:
                    continue        # further than one 256-block: not one move
                recs.append((units.setdefault(key, len(units)), dt, step * 256.0, la, lb))
    if len(recs) < 1000:
        print("\nconstant speed: too few observations; skipped")
        return

    nu = len(units)
    U = np.array([r[0] for r in recs])
    DTA = np.array([r[1] for r in recs])
    STEP = np.array([r[2] for r in recs])
    LA = np.array([r[3] for r in recs])
    LB = np.array([r[4] for r in recs])
    TT = DTA * DTA

    def cost(table) -> float:
        """Residual left after fitting one speed per unit, summed.

        The fit is a sum per unit, which `bincount` does for all of them
        in one pass -- a loop over units costs more per swap than the
        extra observations are worth.
        """
        d = np.abs(STEP + table[LB] - table[LA])
        moving = d > 0
        um = U[moving]
        num = np.bincount(um, weights=d[moving] * DTA[moving], minlength=nu)
        den = np.bincount(um, weights=TT[moving], minlength=nu)
        cnt = np.bincount(um, minlength=nu)
        # A unit with fewer than two moving pairs cannot fit a speed at
        # all; its cost is then just how far it moved, which s = 0 gives.
        s = np.where((cnt >= 2) & (den > 0), num / np.maximum(den, 1e-9), 0.0)
        res = d - s[U] * DTA
        return float(res @ res)

    # Anchors are held; every other entry starts shuffled, so the search
    # has to earn each one it gets right.
    rng = np.random.default_rng(0)
    table = np.zeros(256)
    free = [c for c in range(256) if c not in anchor]
    vals = [v for v in range(256) if v not in set(anchor.values())]
    rng.shuffle(vals)
    for c, p in anchor.items():
        table[c] = p
    for c, v in zip(free, vals):
        table[c] = v

    cur = start = cost(table)
    best_table, best = table.copy(), cur
    for it in range(iters):
        temp = 0.6 * (0.002 / 0.6) ** (it / iters)
        i, j = rng.choice(len(free), 2, replace=False)
        a, b = free[i], free[j]
        table[a], table[b] = table[b], table[a]
        new = cost(table)
        delta = new - cur
        if delta < 0 or rng.random() < np.exp(-delta / max(temp * cur / len(recs), 1e-12)):
            cur = new
            if cur < best:
                best, best_table = cur, table.copy()
        else:
            table[a], table[b] = table[b], table[a]

    dev = sorted(
        min(abs(int(best_table[c]) - inv[c]), 256 - abs(int(best_table[c]) - inv[c]))
        for c in range(256)
        if c in inv
    )
    print(f"\nconstant speed at dt <= {dt_max}s, {len(recs)} observations "
          f"over {len(units)} tracks, {iters} swaps:")
    print(f"  cost          : {start:.3e} -> {best:.3e}")
    print(f"  exactly right : {sum(1 for d in dev if d == 0)} of {len(dev)}"
          f"  ({len(anchor)} of those are anchors)")
    print(f"  within 4      : {sum(1 for d in dev if d <= 4)}")
    print(f"  within 16     : {sum(1 for d in dev if d <= 16)}")
    print(f"  median error  : {dev[len(dev) // 2]} plaintext steps, "
          f"so about {2 * dev[len(dev) // 2]} map units")
    print(f"  90th pct      : {dev[int(len(dev) * 0.9)]}")
    print("  the high byte is exact, so however wrong an entry is, it cannot")
    print("  move a position by more than 256 of the map's 15000 units.")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--true-from", help="decode --jsonl --heap dump; prints the true table")
    ap.add_argument("--blocks", help="explore --dump-netid dump of the movement class")
    ap.add_argument("--true", help="true table as written by --true-from")
    ap.add_argument("--dt", type=float, default=1.5, help="pair time limit for seriation")
    ap.add_argument("--iters", type=int, default=120_000,
                    help="swaps the constant-speed search gets; the cost "
                         "stops falling around 85,000, so more than the "
                         "default buys nothing")
    args = ap.parse_args()

    if args.true_from:
        table = true_table(args.true_from)
        print(json.dumps({f"{k:02x}": f"{v:02x}" for k, v in table.items()}))
        print(f"{len(table)} of 256 entries", file=sys.stderr)
        return 0
    if not (args.blocks and args.true):
        ap.error("need --blocks and --true, or --true-from")

    table = {int(k, 16): int(v, 16) for k, v in json.load(open(args.true)).items()}
    inv = {c: p for p, c in table.items()}
    tracks, parsed = parsed_tracks(args.blocks, inv)
    if tracks:
        print(f"{parsed} payloads parsed, "
              f"{sum(len(v) for v in tracks.values())} positions over "
              f"{len(tracks)} unit tracks")
    else:
        # Nothing parsed, so fall back to the shape-based reader and say
        # so -- the numbers below are then off a quarter of the data.
        tracks = unit_tracks(args.blocks, inv)
        print(f"buffer would not parse; falling back to "
              f"{sum(len(v) for v in tracks.values())} single-waypoint "
              f"blocks over {len(tracks)} tracks")
    print()
    report_steps(tracks, inv)
    report_band(tracks, inv)
    report_seriation(tracks, inv, args.dt)
    report_speed(tracks, inv, args.dt, args.iters)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
