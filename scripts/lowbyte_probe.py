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
                     bimodal, two thirds exactly zero and a third over
                     six, with +/-1 a small minority of the rest.  That
                     is the measurement that rules the chain out.

  band width         Among pairs close enough in time to be useful, how
                     far apart the two low bytes actually are.  This is
                     the width of the only structure the low byte has,
                     and it sets how much a locality method can know.

  seriation score    Treat "these two bytes are close in value" as a
                     graph and recover the circular order spectrally,
                     anchored on the high-byte chain.  Uses locality
                     only, and throws the step sizes away.  Reported
                     unrefined here; adding pairwise local search to it
                     reached a median of 8, still behind the below.

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


def unit_tracks(path: str, inv: dict[int, int]):
    """Single-waypoint blocks per unit, in time order.

    Only an entry with one waypoint puts its coordinates where we read
    them; any other shape shifts them by a bitmap byte.  Units are keyed
    by the cipher of their net id, which groups a unit's own packets
    together without being able to read the id.
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

    U = np.array([r[0] for r in recs])
    DTA = np.array([r[1] for r in recs])
    STEP = np.array([r[2] for r in recs])
    LA = np.array([r[3] for r in recs])
    LB = np.array([r[4] for r in recs])

    rows: dict[int, list[int]] = collections.defaultdict(list)
    touch: dict[int, set[int]] = collections.defaultdict(set)
    for i, u in enumerate(U):
        rows[u].append(i)
        touch[int(LA[i])].add(u)
        touch[int(LB[i])].add(u)
    by_unit = {u: np.array(ix) for u, ix in rows.items()}

    def unit_cost(table, ix) -> float:
        """Residual left after fitting this unit's one speed."""
        d = np.abs(STEP[ix] + table[LB[ix]] - table[LA[ix]])
        t = DTA[ix]
        moving = d > 0
        if moving.sum() < 2:
            return float(np.dot(d, d))
        s = np.dot(d[moving], t[moving]) / max(np.dot(t[moving], t[moving]), 1e-9)
        res = d - s * t
        return float(np.dot(res, res))

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

    cache = {u: unit_cost(table, ix) for u, ix in by_unit.items()}
    cur = start = sum(cache.values())
    best_table, best = table.copy(), cur
    for it in range(iters):
        temp = 0.6 * (0.002 / 0.6) ** (it / iters)
        i, j = rng.choice(len(free), 2, replace=False)
        a, b = free[i], free[j]
        hit = touch[a] | touch[b]
        if not hit:
            continue
        before = sum(cache[u] for u in hit)
        table[a], table[b] = table[b], table[a]
        after = {u: unit_cost(table, by_unit[u]) for u in hit}
        delta = sum(after.values()) - before
        if delta < 0 or rng.random() < np.exp(-delta / max(temp * cur / len(recs), 1e-12)):
            cache.update(after)
            cur += delta
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
    ap.add_argument("--iters", type=int, default=400_000,
                    help="swaps the constant-speed search gets; its cost is "
                         "still falling at the default, so raise it to do better")
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
    tracks = unit_tracks(args.blocks, inv)
    print(f"{sum(len(v) for v in tracks.values())} single-waypoint blocks, "
          f"{len(tracks)} unit tracks\n")
    report_steps(tracks, inv)
    report_band(tracks, inv)
    report_seriation(tracks, inv, args.dt)
    report_speed(tracks, inv, 3.0, args.iters)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
