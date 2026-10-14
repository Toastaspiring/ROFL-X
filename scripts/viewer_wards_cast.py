#!/usr/bin/env python3
"""Wards on a packed patch, read from the cast class.

Placing a ward is a cast, and the cast class carries both what was cast
and where it was aimed. Two spell ids account for 77 of this game's 84
wards: the trinket every champion carries, and the support item's, which
is why the support's count was the one the trinket alone could not
explain.

The coordinate is read with the table recovered from the casts
themselves. Its duration is *not* read -- nothing here says when a ward
dies -- so the page is told the game's own figure and that it is a
model, not a reading.
"""
import bisect, collections, json, struct

SOURCES = {
    "954826ad": "trinket",       # per-champion count matches WARD_PLACED
    "6047b32c": "support item",  # covers the support's remainder exactly
}
# The coordinate is a little-endian float, so x occupies 23..26 and z
# 27..30: byte 0 lowest, the exponent last. Every byte it can read makes
# the position finer -- with only the exponent and the byte beside it a
# coordinate lands on a 64-unit grid, and the one below that takes it to
# a quarter of a unit.
SPELL_OFF = 16
X = (23, 24, 25, 26)
Z = (27, 28, 29, 30)
MODEL_LIFE = 120.0     # a stealth ward's own duration; see the docstring
WARD_SIGHT = 900.0

T = {int(c, 16): int(p, 16) for c, p in json.load(open("tab474.json")).items()}
TAB = {int(p, 16): int(c, 16) for p, c in json.load(open("table1619.json")).items()}
key = lambda nid: f"{TAB[nid & 0xFF]:02x}{TAB[(nid >> 8) & 0xFF]:02x}"
data = json.load(open("viewer/viewer_data.json"))["patch_16_19"]["units"]
named = {k: v for k, v in data.items() if v.get("name")}

units = collections.defaultdict(list)
for line in open("pos1sr_t.jsonl", encoding="utf-8"):
    r = json.loads(line)
    units[r["unit"]].append((r["t"], r["x"], r["z"]))
for v in units.values():
    v.sort()
times = {k: [q[0] for q in v] for k, v in units.items()}

def at(k, t, slack=1.0):
    v = units.get(k)
    if not v:
        return None
    i = bisect.bisect_left(times[k], t)
    best = None
    for j in (i - 1, i):
        if 0 <= j < len(v) and (best is None or abs(v[j][0] - t) < abs(best[0] - t)):
            best = v[j]
    return (best[1], best[2]) if best and abs(best[0] - t) <= slack else None

def coord(pay, offs):
    """Read as much of the float as the table covers.

    The exponent and the byte under it are needed; the two below them
    are a bonus. A byte that cannot be read is filled with the middle of
    its range rather than with zero, because zero is not an estimate --
    it is the bottom of the interval, and using it pushes every
    coordinate the same way. The midpoint halves the worst error and
    removes the bias."""
    b0, b1, b2, b3 = (T.get(pay[o]) for o in offs)
    if b3 is None or b2 is None:
        return None, None
    known = 2 + (b1 is not None) + (b1 is not None and b0 is not None)
    if b1 is None:
        b1, b0 = 0x80, 0x80
    elif b0 is None:
        b0 = 0x80
    v = struct.unpack("<f", bytes([b0, b1, b2, b3]))[0]
    # how wide the interval the unread bytes leave
    step = struct.unpack("<f", bytes([0, 0, b2, b3]))[0]
    nxt = struct.unpack("<f", bytes([0, 0, (b2 + 1) & 0xFF, b3]))[0]
    grid = abs(nxt - step) if (b2 + 1) <= 0xFF else 0.0
    err = 0.0 if known >= 3 else grid / 2.0
    return v, err

out, unread, bad = [], 0, 0
for line in open("cast474.jsonl", encoding="utf-8"):
    r = json.loads(line)
    k = key(int(r["param"], 16))
    if k not in named:
        continue
    pay = bytes.fromhex(r["payload"])
    if len(pay) < 31:
        continue
    src = SOURCES.get(pay[SPELL_OFF:SPELL_OFF + 4].hex())
    if not src:
        continue
    (x, ex), (z, ez) = coord(pay, X), coord(pay, Z)
    if x is None or z is None:
        unread += 1
        continue
    me = at(k, r["t"])
    d = ((x - me[0]) ** 2 + (z - me[1]) ** 2) ** 0.5 if me else 0
    if not (-250 <= x <= 15100 and -250 <= z <= 15100) or d > 1200:
        bad += 1
        continue
    out.append({"t": round(r["t"], 2), "until": round(r["t"] + MODEL_LIFE, 2),
                "killed": False, "kind": "ward", "name": src,
                "team": named[k]["team"], "role": named[k]["name"],
                "x": round(x, 1), "z": round(z, 1),
                "accuracy": round(max(ex, ez), 1), "sight": WARD_SIGHT,
                "modelled_life": True})
out.sort(key=lambda w: w["t"])
by = collections.Counter(w["role"] for w in out)
truth = json.load(open("truth1619.json"))
print(f"{len(out)} wards placed and located; {unread} used table entries not yet "
      f"recovered, {bad} failed the map or range check\n")
print(f"   {'champion':<14}{'found':>7}{'scoreboard':>12}")
for n in sorted(truth, key=lambda n: -truth[n]["wards"]["WARD_PLACED"]):
    print(f"   {n:<14}{by[n]:>7}{truth[n]['wards']['WARD_PLACED']:>12}")
print(f"\n   {len(out)} of {sum(truth[n]['wards']['WARD_PLACED'] for n in truth)}")
json.dump(out, open("wards1619.json", "w"), separators=(",", ":"))
print("wrote wards1619.json")
