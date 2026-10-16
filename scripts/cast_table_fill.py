"""Finish the cast table by elimination rather than by voting.

The vote that recovered 167 entries is spent: it works on ciphers common
enough to out-vote their rivals, and what is left is rare by definition.
Going from seven replays to twenty added four entries.

So stop counting and start eliminating. Three things narrow a rare
cipher that the vote never used:

  the bijection   A substitution is one-to-one, so 167 entries known
                  leaves an unknown cipher with 89 candidates, not 256.
                  Every entry recovered takes one more away.

  the reach       A cast is thrown from somewhere. With the exponent
                  known the mantissa decides where in that octave the
                  target sits, so a candidate that puts it beyond what
                  the spell could reach is impossible -- not unlikely,
                  impossible. One observation kills perhaps half; a
                  dozen, spread over different octaves and different
                  places on the map, leave very few.

  the ward casts  A ward is put on the ground at the caster's feet,
                  within about 600 units, where a spell may reach 1,500.
                  Those casts are known by their spell id, and each is
                  worth several ordinary ones.

Both coordinates are read through the one table, so x and z observations
of the same cipher are pooled. Run to a fixed point, because each entry
recovered makes more casts readable and so constrains the rest.
"""
import bisect
import collections
import glob
import json
import os
import struct

WARD_IDS = {"954826ad", "6047b32c"}      # trinket, and the support item's
WARD_REACH = 650.0
import sys
MUST_CLEAR = float(sys.argv[1]) if len(sys.argv) > 1 else 0.80
MIN_MARGIN = int(sys.argv[2]) if len(sys.argv) > 2 else 1
CAST_REACH = 1500.0
EXP_OFF_X, MAN_OFF_X = 26, 25
EXP_OFF_Z, MAN_OFF_Z = 30, 29
SPELL_OFF = 16

TAB = {int(p, 16): int(c, 16) for p, c in json.load(open("table1619.json")).items()}
key = lambda nid: (f"{TAB[nid & 0xFF]:02x}{TAB[(nid >> 8) & 0xFF]:02x}"
                   f"{TAB[(nid >> 16) & 0xFF]:02x}")
T = {int(c, 16): int(q, 16) for c, q in json.load(open("tab474.json")).items()}
val = lambda e, m: struct.unpack("<f", bytes([0, 0, m, e]))[0]

samples = []
for pos_path in sorted(glob.glob("pos_g*.jsonl")):
    cast_path = pos_path.replace("pos_", "cast_")
    if not os.path.exists(cast_path):
        continue
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
        if len(pay) <= max(EXP_OFF_Z, SPELL_OFF + 4):
            continue
        p = at(key(int(r["param"], 16)), r["t"])
        if p:
            ward = pay[SPELL_OFF:SPELL_OFF + 4].hex() in WARD_IDS
            samples.append((pay, p, ward))
print(f"{len(samples)} casts placed, "
      f"{sum(1 for s in samples if s[2])} of them ward placements")
print(f"starting from {len(T)} of 256 entries\n")

for rnd in range(1, 9):
    taken = set(T.values())
    free = [m for m in range(256) if m not in taken]
    # cipher -> list of (exponent, the caster's coordinate, reach)
    need = collections.defaultdict(list)
    for pay, pos, ward in samples:
        reach = WARD_REACH if ward else CAST_REACH
        for exp_off, man_off, pick in ((EXP_OFF_X, MAN_OFF_X, 0),
                                       (EXP_OFF_Z, MAN_OFF_Z, 1)):
            e = T.get(pay[exp_off])
            if e is None or pay[man_off] in T:
                continue
            need[pay[man_off]].append((e, pos[pick], reach))

    found = {}
    for c, obs in need.items():
        if len(obs) < 4:
            continue
        # One cast aimed across the lane is enough to rule out the true
        # answer, so count how many observations each candidate satisfies
        # rather than demanding all of them. The right one should clear
        # most and beat its rivals.
        score = [(sum(1 for e, near, reach in obs
                      if abs(val(e, m) - near) <= reach), m) for m in free]
        score.sort(reverse=True)
        best, m = score[0]
        rival = score[1][0] if len(score) > 1 else 0
        if best >= len(obs) * MUST_CLEAR and best - rival >= MIN_MARGIN:
            found[c] = (m, best - rival, len(obs))
    # a plaintext may only be claimed once; the best-evidenced cipher wins
    claimed, added = set(), 0
    for c, (m, margin, n) in sorted(found.items(), key=lambda kv: -kv[1][1]):
        if m in claimed:
            continue
        claimed.add(m)
        T[c] = m
        added += 1
    left = sum(1 for c in need if c not in T)
    print(f"   round {rnd}: {added} pinned to a single candidate, "
          f"{len(T)} of 256 known, {left} ciphers still open")
    if not added:
        break

json.dump({f"{c:02x}": f"{q:02x}" for c, q in sorted(T.items())},
          open("tab474.json", "w"), indent=0)
print(f"\nwrote {len(T)} entries to tab474.json")
