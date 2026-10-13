"""Sort a champion's position changes into walks and everything else.

A champion's position changes for two reasons. It walks -- and then the
path and the speed in the packet predict where it goes, to a median of
17 units. Or the game puts it somewhere, and nothing in the packet
predicts that. The second kind must not be smoothed: spreading a
15,000-unit residual across an interval draws a champion gliding over
the whole map when what happened was instant.

The replay says what the answer should be, which is what makes this
checkable rather than plausible. statsJson carries each player's two
summoner spells and how many times each was cast, and the death class
names its victim in a plaintext field. So:

  death     Taken from the death class, not inferred. Its 74 deaths
            split per champion exactly as statsJson's NUM_DEATHS does,
            which is what says that class is being read correctly.
  recall    An unexplained jump landing at the champion's own spawn
            that no death accounts for.
  teleport  A long unexplained jump landing elsewhere. statsJson says
            one player had Teleport and cast it twice.
  blink     A short one, the range of a flash. statsJson says who
            carried Flash and how often they used it.

Consecutive unexplained steps are one event: the game takes several
packets to put a champion down, and counting each as its own teleport
is how 2 real teleports first read as 357.
Inputs: pos1sr_t.jsonl, e1sr.json, table1619.json, deaths19.jsonl
(`explore --dump-netid <death class>`), truth1619.json (summoner spells
and deaths pulled out of `rofl-x summary`), viewer/viewer_data.json.

Writes jumps1619.json (every event, including the ones too uncertain to
draw) and events1619.json (the deaths). Recalls are replaced by
viewer_recalls.py, which finds them by their channel instead.
"""
import collections, json, math

FOUNTAIN = {"order": (420.0, 440.0), "chaos": (14340.0, 14390.0)}
# A champion that is dead or not yet in play is parked just outside its
# own corner of the map, which is where it reappears from.
PARK = {"order": (-632.0, -558.0), "chaos": (15362.0, 15398.0)}
AT_SPAWN = 1600.0
# A recall crosses the map. A jump that both starts and ends near the
# fountain is the model losing a champion that was standing there: the
# two groups separate cleanly, 42 starting beyond 6,000 units and 37
# within 2,000, with nothing in between.
CAME_FROM = 2000.0
BLINK = 800.0
UNEXPLAINED = 600.0
SAME_EVENT = 1.5      # consecutive unexplained steps are one event
REVIVE = 90.0         # longest a respawn can take


def along(path, dist):
    if len(path) == 1:
        return path[0]
    left = dist
    for a, b in zip(path, path[1:]):
        seg = math.dist(a, b)
        if seg == 0:
            continue
        if left <= seg:
            u = left / seg
            return (a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u)
        left -= seg
    return path[-1]


data = json.load(open("viewer/viewer_data.json"))["patch_16_19"]
champs = {k: v for k, v in data["units"].items() if v.get("name")}
first = int(json.load(open("e1sr.json"))["replays"][0]["entities"]["champions"]["first"], 16)
TAB = {int(p, 16): int(c, 16) for p, c in json.load(open("table1619.json")).items()}
INV = {c: p for p, c in TAB.items()}
truth = json.load(open("truth1619.json"))

def parked(r, team):
    """A position at the side's own corner, outside the map, is not a
    position: it is the stream declining to report the unit. 93% of the
    ones that fall there with the champion alive are moments the enemy
    could not see it. Treating them as positions invents a jump across
    the map and back."""
    return math.dist((r["x"], r["z"]), PARK[team]) < 900


rows = collections.defaultdict(list)
for line in open("pos1sr_t.jsonl", encoding="utf-8"):
    r = json.loads(line)
    rows[r["unit"]].append(r)
for k, rec in champs.items():
    rows[k] = [r for r in rows[k] if not parked(r, rec["team"])]
deaths_by_idx = collections.defaultdict(list)
for line in open("deaths19.jsonl", encoding="utf-8"):
    r = json.loads(line)
    p = int(r["param"], 16) if isinstance(r["param"], str) else r["param"]
    if 0 <= p - first < 10:
        deaths_by_idx[p - first].append(r["t"])

events = []
for key, rec in champs.items():
    lo, hi = INV.get(int(key[0:2], 16)), INV.get(int(key[2:4], 16))
    idx = (0x40000000 | (hi << 8) | lo) - first if lo is not None and hi is not None else -1
    ds = sorted(deaths_by_idx.get(idx, []))
    team, name = rec["team"], rec["name"]
    v = sorted(rows[key], key=lambda r: r["t"])

    # Deaths come from the death class. A champion is down from the
    # death until its next packet, and that reappearance is a respawn,
    # not a walk.
    for d in ds:
        nxt = next((r for r in v if r["t"] > d), None)
        # Where it fell: the last position it reported while still alive.
        last = None
        for r in v:
            if r["t"] > d:
                break
            last = r
        events.append({"unit": key, "name": name, "team": team, "kind": "death",
                       "t": round(d, 2), "until": round(nxt["t"], 2) if nxt else None,
                       "at": [round(last["x"]), round(last["z"])] if last else None,
                       "to": [round(nxt["x"]), round(nxt["z"])] if nxt else None})

    raw = []
    for a, b in zip(v, v[1:]):
        dt = b["t"] - a["t"]
        if dt <= 0:
            continue
        path = [tuple(p) for p in (a.get("path") or [(a["x"], a["z"])])]
        sp = a.get("speed") or 0
        landed = along(path, dt * sp) if 100 < sp < 2200 else path[0]
        err = math.dist(landed, (b["x"], b["z"]))
        if err > UNEXPLAINED:
            raw.append((b["t"], landed, (b["x"], b["z"]), err))

    # Collapse runs into single events.
    for t, src, dst, err in raw:
        if events and events[-1].get("unit") == key and events[-1]["kind"] != "death" \
                and t - events[-1]["t"] < SAME_EVENT:
            events[-1]["t"] = round(t, 2)
            events[-1]["to"] = [round(dst[0]), round(dst[1])]
            events[-1]["jump"] = round(math.dist(events[-1]["raw_from"], dst))
            continue
        after_death = any(d < t <= d + REVIVE for d in ds)
        at_spawn = (math.dist(dst, FOUNTAIN[team]) < AT_SPAWN
                    or math.dist(dst, PARK[team]) < AT_SPAWN)
        from_far = math.dist(src, FOUNTAIN[team]) > CAME_FROM
        kind = ("respawn" if after_death and at_spawn else
                "recall" if at_spawn and from_far else
                "near-spawn" if at_spawn else
                "blink" if err < BLINK else "teleport")
        events.append({"unit": key, "name": name, "team": team, "kind": kind,
                       "t": round(t, 2), "from": [round(c) for c in src],
                       "raw_from": src, "to": [round(c) for c in dst],
                       "jump": round(err)})
for e in events:
    e.pop("raw_from", None)

kinds = collections.Counter(e["kind"] for e in events)
print("events found, against what the replay says they should be:\n")
print(f"   {'champion':<13} {'death':>13} {'respawn':>8} {'recall':>7} "
      f"{'teleport':>18} {'blink':>15}")
tot = collections.Counter()
for name in sorted({e["name"] for e in events}):
    c = collections.Counter(e["kind"] for e in events if e["name"] == name)
    sp = truth[name]["spells"]
    tp_truth = sp.get("Teleport"); fl_truth = sp.get("Flash")
    tot["tp_truth"] += tp_truth or 0
    tot["fl_truth"] += fl_truth or 0
    print(f"   {name:<13} {c['death']:>5} of {truth[name]['deaths']:<4} "
          f"{c['respawn']:>8} {c['recall']:>7} "
          f"{c['teleport']:>8} of {('%s' % tp_truth) if tp_truth is not None else 'none':<6} "
          f"{c['blink']:>6} of {('%s' % fl_truth) if fl_truth is not None else 'none':<5}")
print(f"\n   totals: {kinds['death']} deaths of {sum(t['deaths'] for t in truth.values())}, "
      f"{kinds['respawn']} respawns, {kinds['recall']} recalls, "
      f"{kinds['teleport']} teleports of {tot['tp_truth']}, "
      f"{kinds['blink']} blinks of {tot['fl_truth']} flashes")
json.dump(events, open("jumps1619.json", "w"), separators=(",", ":"))

keep = [e for e in events if e["kind"] in ("death", "recall")]
json.dump(keep, open("events1619.json", "w"), separators=(",", ":"))
print()
print(f"wrote {len(keep)} drawable events to events1619.json "
      f"({kinds['death']} deaths, {kinds['recall']} recalls)")
print(f"left undrawn: {kinds['teleport']} unexplained jumps that are not "
      f"teleports -- statsJson says the game held 2 -- plus {kinds['blink']} "
      f"short ones against 33 flashes. Neither is separable yet.")
