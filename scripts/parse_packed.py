#!/usr/bin/env python3
"""The parse of a packed replay: one JSON with everything that is read.

`rofl-x file` needs a patch archive and so stops at 16.9. On a patch
whose client ships packed there is no archive and never will be, and
what is readable has been won class by class -- documented in
`docs/EXPLORE.md`, but scattered across a dozen intermediate files. This
collects it.

Every entry says where it came from, because the difference matters:

  read      Taken from the file. Deaths are named by a plaintext field;
            positions, paths and speeds come through a recovered table;
            creations and missile completions need no table at all,
            their classes carrying a constant payload so that all the
            content is the unit and the moment.
  inferred  Reconstructed from shape rather than stated. A recall is a
            silence that starts away from base, ends at the fountain,
            has no death to explain it, and covers more ground than the
            champion could have walked.
  modelled  Not in the file. A ward's duration is the game's own figure,
            not a reading, and is marked as such wherever it appears.

What is *not* here is as much the point. Damage, item purchases, levels
and ability identities are not readable on a packed patch today;
`docs/EXPLORE.md` says for each one how far it got and what blocks it.

Usage, after the dumps in `viewer/README.md`:

    python scripts/parse_packed.py --out parsed_16_19.json
"""
from __future__ import annotations

import argparse
import collections
import json
import os


def load_jsonl(path, limit=None):
    try:
        with open(path, encoding="utf-8") as f:
            for i, line in enumerate(f):
                if limit is not None and i >= limit:
                    return
                yield json.loads(line)
    except FileNotFoundError:
        return


def load_json(path, empty):
    try:
        return json.load(open(path, encoding="utf-8"))
    except FileNotFoundError:
        return empty


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--positions", default="pos1sr_t.jsonl")
    ap.add_argument("--meta", default="e1sr.json")
    ap.add_argument("--events", default="events1619.json",
                    help="deaths and recalls, from viewer_events/_recalls")
    ap.add_argument("--wards", default="wards1619.json")
    ap.add_argument("--creations", default="n1230.jsonl",
                    help="the unit-creation class; its payload is constant")
    ap.add_argument("--missiles", default="n325.jsonl",
                    help="missiles finishing their flight; payload constant")
    ap.add_argument("--missiles-born", default="n138.jsonl",
                    help="missiles being created; names them in plaintext too")
    ap.add_argument("--unseen", default="n69.jsonl",
                    help="leaving a team's vision; payload is one bit")
    ap.add_argument("--viewer-data", default="viewer/viewer_data.json",
                    help="for the champion names already solved")
    ap.add_argument("--patch", default="patch_16_19")
    ap.add_argument("--out", default="parsed_16_19.json")
    ap.add_argument("--no-tracks", action="store_true",
                    help="leave out per-unit movement, which dominates the size")
    args = ap.parse_args()

    meta = load_json(args.meta, {}).get("replays", [{}])[0]
    named = {k: v for k, v in
             load_json(args.viewer_data, {}).get(args.patch, {})
             .get("units", {}).items() if v.get("name")}

    # --- movement: every unit's orders, not positions on a grid --------
    tracks = collections.defaultdict(list)
    for r in load_jsonl(args.positions):
        if r.get("replay", 0) != 0:
            continue
        tracks[r["unit"]].append({
            "t": round(r["t"], 3),
            "speed": round(r.get("speed", 0.0), 1),
            "path": [[round(x, 1), round(z, 1)] for x, z in
                     (r.get("path") or [[r["x"], r["z"]]])],
        })

    events = []
    for e in load_json(args.events, []):
        kind = e.get("kind")
        if kind == "death":
            events.append({"t": e["t"], "type": "death", "champion": e["name"],
                           "team": e["team"], "at": e.get("at"),
                           "until": e.get("until"), "source": "read"})
        elif kind == "recall":
            events.append({"t": e["t"], "type": "recall", "champion": e["name"],
                           "team": e["team"], "from": e.get("from"),
                           "channel_s": e.get("channel"), "source": "inferred"})
    for w in load_json(args.wards, []):
        events.append({"t": w["t"], "type": "ward_placed",
                       "champion": w.get("role"), "team": w["team"],
                       "x": w["x"], "z": w["z"],
                       "accuracy_map_units": w.get("accuracy", 0),
                       "ward": w.get("name"),
                       "until": w.get("until"),
                       "until_source": "modelled", "source": "read"})
    for r in load_jsonl(args.creations):
        events.append({"t": round(r["t"], 3), "type": "unit_created",
                       "unit": r["param"], "source": "read"})
    # A projectile is created by one class and ended by another, both
    # naming it in plaintext, so the pair needs no table: 5,538 of the
    # sample game's 6,102 missiles have both ends, and the gap between
    # them is a flight time -- a median of half a second, a tenth of
    # them under 0.2s and a tenth over 1.0s.
    #
    # Which cast threw which missile is *not* joined here. Casts are
    # frequent enough that a missile falls shortly after one by chance:
    # matching on time beats shuffled times by only 2.2x at its best and
    # 1.2x at a half-second window, which is not enough to name a
    # thrower.
    done = {}
    for r in load_jsonl(args.missiles):
        done.setdefault(r["param"], round(r["t"], 3))
    # The creating class fires about three times per missile, so the
    # first block is the birth and the rest are repeats.
    seen = set()
    for r in load_jsonl(args.missiles_born):
        if r["param"] in seen:
            continue
        seen.add(r["param"])
        t = round(r["t"], 3)
        ended = done.pop(r["param"], None)
        events.append({"t": t, "type": "missile",
                       "missile": r["param"], "ended": ended,
                       "flight_s": round(ended - t, 3) if ended else None,
                       "source": "read"})
    for mid, t in done.items():
        events.append({"t": t, "type": "missile_finished",
                       "missile": mid, "source": "read"})
    for r in load_jsonl(args.unseen):
        events.append({"t": round(r["t"], 3), "type": "left_vision",
                       "unit": r["param"], "source": "read"})
    events.sort(key=lambda e: e["t"])

    out = {
        "patch": meta.get("patch"),
        "game_length_s": meta.get("game_length_s"),
        "players": meta.get("players", []),
        "champions": {k: {"name": v["name"], "team": v["team"]}
                      for k, v in named.items()},
        "events": events,
        "counts": collections.Counter(e["type"] for e in events),
        "provenance": {
            "read": "taken from the file",
            "inferred": "reconstructed from shape, not stated anywhere",
            "modelled": "not in the file at all; the game's own figure",
        },
        "not_read": "damage, items, levels and which ability a cast was are "
                    "not readable on a packed patch today; docs/EXPLORE.md "
                    "says for each how far it got and what blocks it",
    }
    if not args.no_tracks:
        out["tracks"] = [{"unit": u, "orders": o} for u, o in sorted(tracks.items())]

    json.dump(out, open(args.out, "w", encoding="utf-8"), separators=(",", ":"))
    size = os.path.getsize(args.out) / 1e6
    print(f"{out['patch']}, {out['game_length_s']:.0f}s")
    print(f"   {len(named)} champions named, {len(tracks)} units with movement")
    for k, n in sorted(out["counts"].items(), key=lambda kv: -kv[1]):
        print(f"   {n:>7,}  {k}")
    print(f"wrote {args.out} ({size:.1f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
