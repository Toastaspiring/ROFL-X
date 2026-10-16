"""Pin the last ciphers on where casts are actually aimed.

Elimination on the caster's own position has gone as far as it goes: it
says the target is within reach, which rules out most candidates but
rarely all but one. A tighter statement is available for a good share of
casts, because a spell thrown at somebody lands on them, and every
champion's position is already known to a couple of map units.

So a candidate is tested twice. It must put the target within reach of
the caster -- that is the loose constraint, true of every cast. And for
the casts aimed at a champion it must put the target *on* one, which is
exact rather than approximate. Neither is true of every cast, so both
are scored rather than demanded, and the bijection does the rest: 176
entries known leaves 80 candidates, not 256.
"""
import bisect
import collections
import glob
import json
import os
import struct
import sys

ON_CHAMPION = 110.0      # how near counts as aimed at one
CAST_REACH = 1500.0
WARD_REACH = 650.0
WARD_IDS = {"954826ad", "6047b32c"}
CLEAR = float(sys.argv[1]) if len(sys.argv) > 1 else 0.85
MARGIN = int(sys.argv[2]) if len(sys.argv) > 2 else 2

TAB = {int(p, 16): int(c, 16) for p, c in json.load(open("table1619.json")).items()}
key = lambda nid: (f"{TAB[nid & 0xFF]:02x}{TAB[(nid >> 8) & 0xFF]:02x}"
                   f"{TAB[(nid >> 16) & 0xFF]:02x}")
T = {int(c, 16): int(q, 16) for c, q in json.load(open("tab474.json")).items()}
val = lambda e, m: struct.unpack("<f", bytes([0, 0, m, e]))[0]

samples = []
for pos_path in sorted(glob.glob("pos_g*.jsonl")):
    cast_path = pos_path.replace("pos_", "cast_")
    meta_path = pos_path.replace("pos_", "meta_").replace(".jsonl", ".json")
    if not (os.path.exists(cast_path) and os.path.exists(meta_path)):
        continue
    try:
        m = json.load(open(meta_path, encoding="utf-8"))["replays"][0]
        first = int(m["entities"]["champions"]["first"], 16)
    except (KeyError, IndexError):
        continue
    champ_keys = [key(first + i) for i in range(10)]
    units = collections.defaultdict(list)
    for line in open(pos_path, encoding="utf-8"):
        r = json.loads(line)
        units[r["unit"]].append((r["t"], r["x"], r["z"]))
    for v in units.values():
        v.sort()
    times = {k: [q[0] for q in v] for k, v in units.items()}

    def at(k, t, slack=0.4):
        v = units.get(k)
        if not v:
            return None
        i = bisect.bisect_left(times[k], t)
        b = None
        for j in (i - 1, i):
            if 0 <= j < len(v) and (b is None or abs(v[j][0] - t) < abs(b[0] - t)):
                b = v[j]
        return (b[1], b[2]) if b and abs(b[0] - t) <= slack else None

    for r in (json.loads(l) for l in open(cast_path, encoding="utf-8")):
        pay = bytes.fromhex(r["payload"])
        if len(pay) <= 30:
            continue
        me = at(key(int(r["param"], 16)), r["t"])
        if not me:
            continue
        here = [p for p in (at(k, r["t"], 0.6) for k in champ_keys) if p]
        ward = pay[16:20].hex() in WARD_IDS
        samples.append((pay, me, here, ward))
print(f"{len(samples)} casts placed; "
      f"{sum(1 for s in samples if s[2]) } with champions located too")
print(f"starting from {len(T)} of 256 entries\n")

for rnd in range(1, 9):
    taken = set(T.values())
    free = [m for m in range(256) if m not in taken]
    need = collections.defaultdict(list)
    for pay, me, here, ward in samples:
        reach = WARD_REACH if ward else CAST_REACH
        for exp_off, man_off, pick in ((26, 25, 0), (30, 29, 1)):
            e = T.get(pay[exp_off])
            if e is None or pay[man_off] in T:
                continue
            need[pay[man_off]].append((e, me[pick], reach,
                                       tuple(p[pick] for p in here)))
    found = {}
    for c, obs in need.items():
        if len(obs) < 4:
            continue
        score = []
        for m in free:
            near = sum(1 for e, mine, reach, _ in obs
                       if abs(val(e, m) - mine) <= reach)
            on = sum(1 for e, _, _, champs in obs
                     if any(abs(val(e, m) - q) <= ON_CHAMPION for q in champs))
            # landing on a champion is exact, so it counts for more than
            # merely being in range
            score.append((near + 3 * on, m))
        score.sort(reverse=True)
        best, m = score[0]
        rival = score[1][0] if len(score) > 1 else 0
        reachable = sum(1 for e, mine, reach, _ in obs
                        if abs(val(e, m) - mine) <= reach)
        if reachable >= len(obs) * CLEAR and best - rival >= MARGIN:
            found[c] = (m, best - rival)
    claimed, added = set(), 0
    for c, (m, margin) in sorted(found.items(), key=lambda kv: -kv[1][1]):
        if m in claimed:
            continue
        claimed.add(m)
        T[c] = m
        added += 1
    print(f"   round {rnd}: {added} pinned, {len(T)} of 256 known")
    if not added:
        break

json.dump({f"{c:02x}": f"{q:02x}" for c, q in sorted(T.items())},
          open("tab474.json", "w"), indent=0)
print(f"\nwrote {len(T)} entries")
