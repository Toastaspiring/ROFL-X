#!/usr/bin/env python3
"""Recover the cast class's coordinate table, on a patch with no archive.

Placing a ward is a cast, and the cast class carries what the cast was
aimed at -- so reading that coordinate is what puts wards on the map for
a patch whose payloads are obfuscated. The class is netid 474 on 16.19
(docs/EXPLORE.md S 3); its table is its own, unrelated to the movement
class's or to 16.9's for the same class.

The known plaintext comes from the game itself. Every block is addressed
to the champion that cast, and the movement class already says where
that champion was to a couple of map units, so:

  the exponents   The coordinate's exponent byte says which octave it is
                  in, and a cast lands in the caster's own octave almost
                  always. Three cipher values cover 99% of casts and
                  agree 95% of the time: that is x done, and z gives the
                  same three answers, which one shared table requires.

  the mantissas   These do not track the caster, because the field is
                  what the cast was *aimed* at, not where it came from.
                  But a good share of casts are aimed at the caster's own
                  feet, so the most common vote is the right one while
                  the rest scatter over 255 values and cancel. Read
                  against x at one offset and against z at another --
                  different casts, different numbers -- the two agree on
                  63% of ciphers where chance is 0.4%.

  the rest        With the exponent known an unknown mantissa is not
                  free: it has to put the target within reach of a known
                  point. Those windows intersect over many casts, and
                  each entry recovered makes more casts readable, so it
                  runs until it stops finding any.

Nothing here reads a game binary.

Usage:

    rofl-x explore -r <replay> --dump-netid 474 --dump cast474.jsonl -o /dev/null
    rofl-x explore -r <replay> --table table1619.json --positions-netid 1267         --dump-positions pos.jsonl -o meta.json
    python scripts/cast_table.py          # reads every pos_*/cast_* pair it finds
"""
import bisect, collections, glob, json, struct

MIN_SEEN = 10
TAB = {int(p, 16): int(c, 16) for p, c in json.load(open("table1619.json")).items()}
key = lambda nid: f"{TAB[nid & 0xFF]:02x}{TAB[(nid >> 8) & 0xFF]:02x}"

pairs = [("pos1sr_t.jsonl", "cast474.jsonl")]
for p in sorted(glob.glob("pos_r*.jsonl")):
    c = p.replace("pos_", "cast_")
    pairs.append((p, c))

samples = []
for pos_path, cast_path in pairs:
    units = collections.defaultdict(list)
    for line in open(pos_path, encoding="utf-8"):
        r = json.loads(line)
        if r.get("replay", 0) == 0:
            units[r["unit"]].append((r["t"], r["x"], r["z"]))
    for v in units.values():
        v.sort()
    times = {k: [q[0] for q in v] for k, v in units.items()}

    def at(k, t, slack=0.4):
        v = units.get(k)
        if not v:
            return None
        i = bisect.bisect_left(times[k], t)
        best = None
        for j in (i - 1, i):
            if 0 <= j < len(v) and (best is None or abs(v[j][0] - t) < abs(best[0] - t)):
                best = v[j]
        return (best[1], best[2]) if best and abs(best[0] - t) <= slack else None

    n = 0
    try:
        for r in (json.loads(l) for l in open(cast_path, encoding="utf-8")):
            p = at(key(int(r["param"], 16)), r["t"])
            if p:
                samples.append((bytes.fromhex(r["payload"]), p))
                n += 1
    except FileNotFoundError:
        continue
    print(f"   {cast_path}: {n} casts placed")
print(f"\n{len(samples)} casts in all\n")

def modes(off, pick):
    v = collections.defaultdict(collections.Counter)
    for pay, pos in samples:
        if off < len(pay):
            v[pay[off]][struct.pack("<f", float(pos[pick]))[2]] += 1
    out = {}
    for c, cnt in v.items():
        tot = sum(cnt.values())
        if tot < MIN_SEEN:
            continue
        (top, n), = cnt.most_common(1)
        runner = cnt.most_common(2)[1][1] if len(cnt) > 1 else 0
        out[c] = (top, n, runner, tot)
    return out

mx, mz = modes(25, 0), modes(29, 1)
both = set(mx) & set(mz)
agree = [c for c in both if mx[c][0] == mz[c][0]]
print(f"offset 25: {len(mx)} ciphers seen {MIN_SEEN}+ times; offset 29: {len(mz)}; "
      f"{len(both)} in common")
print(f"   agree on {len(agree)} of {len(both)} ({len(agree)/max(len(both),1):.1%}); "
      f"chance would be 0.4%\n")

strong = [c for c in agree if mx[c][1] > mx[c][2] and mz[c][1] > mz[c][2]]
table = {c: mx[c][0] for c in strong}
clash = collections.Counter(table.values())
dup = {p for p, n in clash.items() if n > 1}
table = {c: p for c, p in table.items() if p not in dup}
table.update({0x80: 0x46, 0x4d: 0x45, 0x18: 0x44})
print(f"{len(strong)} also beat their runner-up on both sides; "
      f"{len(dup)} plaintexts claimed twice and dropped")
print(f"-> {len(table)} entries of netid 474's table")
json.dump({f"{c:02x}": f"{p:02x}" for c, p in sorted(table.items())},
          open("tab474.json", "w"), indent=0)

# --- then fill what is left, by range ---
val = lambda e, m: struct.unpack("<f", bytes([0, 0, m, e]))[0]
for rnd in range(6):
    want = collections.defaultdict(list)
    for pay, pos in samples:
        for exp_off, man_off, pick in ((26, 25, 0), (30, 29, 1)):
            if man_off >= len(pay):
                continue
            e = T.get(pay[exp_off])
            if e is None or pay[man_off] in T:
                continue
            want[pay[man_off]].append((e, pos[pick]))
    taken = set(T.values())
    found = {}
    for c, lst in want.items():
        if len(lst) < 6:
            continue
        score = [0] * 256
        for e, near in lst:
            for m in range(256):
                if m not in taken and abs(val(e, m) - near) <= REACH:
                    score[m] += 1
        best = max(range(256), key=lambda m: score[m])
        rival = max((score[m] for m in range(256) if abs(m - best) > 1), default=0)
        if score[best] >= len(lst) * 0.9 and score[best] > rival:
            found[c] = (best, score[best], rival, len(lst))
    added = 0
    for c, (m, hit, rival, n) in sorted(found.items(),
                                        key=lambda kv: -(kv[1][1] - kv[1][2])):
        if m in taken:
            continue
        taken.add(m)
        T[c] = m
        added += 1
    print(f"   round {rnd + 1}: {added} new entries, {len(T)} total")
    if not added:
        break
json.dump({f"{c:02x}": f"{p:02x}" for c, p in sorted(T.items())},
          open("tab474.json", "w"), indent=0)
print(f"wrote {len(T)} entries")
