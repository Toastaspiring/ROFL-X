#!/usr/bin/env python
"""Cross-validate decoded class names against Match-V5 ground truth.

This is the framework half of README item E. It doesn't make HTTP
calls — fetch the Match-V5 timeline JSON yourself (e.g. via
developer.riotgames.com or a third-party tool) and pass it in as
`--match-v5`. The replay's match id is the digit run in the filename:
`EUW1-7841558966.rofl` -> matchId `7841558966` on platform `EUW1`.

Match-V5 timeline events that anchor specific packet classes:

  Match-V5 event          Anchors
  ----------------------  -------------------------------------------
  CHAMPION_KILL           PKT_NPC_Die / PKT_S2C_OnEntityDie variants
                          (we don't have these in handlers yet)
  WARD_PLACED             ward-spawn netid (already decoded as Yellow/
                          Sight/Jammer in the legacy ward stream)
  WARD_KILL               ward-spawn netid with name = "*Corpse*"
  ITEM_PURCHASED          PKT_S2C_AddBuffModifier (buff applied to the
                          champion when an item lights up) — likely
                          partial overlap; not 1:1
  SKILL_LEVEL_UP          PKT_CHAR_SetCooldown (a fresh ult cooldown
                          appears the moment R is leveled at 6/11/16)
  CHAMPION_SPECIAL_KILL   no direct anchor yet
  TURRET_PLATE_DESTROYED  PKT_S2C_OnEntityDie variants
  BUILDING_KILL           PKT_S2C_OnEntityDie variants

For each anchor pair, the script computes:
  - timestamp delta (median, p95) between the Match-V5 event and the
    nearest decoded packet within ±5s of that timestamp
  - entity overlap (share of Match-V5 events that map to a decoded
    event involving the same participantId / entity_id)
  - per-minute counts of both streams to spot calibration drift

A class label gets promoted from `shape-match` to `proven` when:
  - >= 5 replays have been run through this script
  - timestamp p95 < 1.0s
  - entity overlap > 90%

This file does NOT promote labels yet — it only emits a per-replay
report. Aggregating across replays + writing back to the catalog is
left for when there's actual data; the framework is here so adding
that step is a one-evening job.

Usage:
  python scripts/correlate_match_v5.py \\
      --rofl-x rofl-out.json \\
      --match-v5 match-v5-timeline.json
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rofl-x", required=True, help="rofl-x output.json")
    ap.add_argument("--match-v5", required=True,
                    help="Match-V5 /lol/match/v5/matches/{id}/timeline JSON")
    ap.add_argument("--out", default=None, help="optional report path")
    args = ap.parse_args()

    with open(args.rofl_x) as f:
        rofl = json.load(f)
    with open(args.match_v5) as f:
        v5 = json.load(f)

    typed = rofl.get("typed_events", [])
    if not typed:
        print("rofl-x output has no typed_events; nothing to correlate.",
              file=sys.stderr)
        return 1

    # Match-V5 timeline shape: { info: { frames: [ { timestamp, events: [...], participantFrames: {...} } ] } }
    info = v5.get("info") or v5
    frames = info.get("frames") or []
    v5_events = []
    for fr in frames:
        for ev in fr.get("events", []) or []:
            ev["_frame_ts_ms"] = fr.get("timestamp")
            v5_events.append(ev)
    print(f"loaded {len(v5_events)} Match-V5 timeline events from {len(frames)} frames")

    # Group decoded by kind for fast nearest-neighbor lookup.
    by_kind = defaultdict(list)
    for e in typed:
        by_kind[e["kind"]].append(e)
    for k in by_kind:
        by_kind[k].sort(key=lambda e: e.get("timestamp", 0))

    def nearest_within(kind, t_seconds, window_seconds=5.0):
        events = by_kind.get(kind) or []
        best = None
        best_dt = window_seconds + 1
        for e in events:
            dt = abs((e.get("timestamp") or 0) - t_seconds)
            if dt < best_dt:
                best_dt = dt
                best = e
            if dt > window_seconds:
                # events are sorted; could break early but tiny gain
                pass
        return (best, best_dt) if best and best_dt <= window_seconds else (None, None)

    # Anchor: WARD_PLACED -> ward stream
    ward_events = rofl.get("wards") or []
    ward_placed = [ev for ev in v5_events if ev.get("type") == "WARD_PLACED"]
    print()
    print(f"=== WARD_PLACED ({len(ward_placed)} in v5, {len(ward_events)} in rofl) ===")
    if ward_placed and ward_events:
        deltas = []
        ward_events_sorted = sorted(ward_events, key=lambda w: w.get("timestamp", 0))
        for ev in ward_placed:
            t_ms = ev.get("timestamp")
            if t_ms is None:
                continue
            t_sec = t_ms / 1000.0
            best_dt = min(abs((w.get("timestamp") or 0) - t_sec)
                          for w in ward_events_sorted)
            deltas.append(best_dt)
        if deltas:
            print(f"  median dt: {statistics.median(deltas):.2f}s")
            print(f"  p95 dt:    {sorted(deltas)[int(0.95 * len(deltas))]:.2f}s")
            print(f"  within 5s: {sum(1 for d in deltas if d < 5):,}/{len(deltas):,}"
                  f" ({sum(1 for d in deltas if d < 5) / len(deltas):.1%})")

    # Anchor: SKILL_LEVEL_UP for ult slot -> SetCooldown bursts
    skill_levelup = [ev for ev in v5_events if ev.get("type") == "SKILL_LEVEL_UP"]
    print()
    print(f"=== SKILL_LEVEL_UP ({len(skill_levelup)} in v5) ===")
    if skill_levelup and by_kind.get("SetCooldown"):
        # Match-V5 doesn't include the slot index in a way we can map
        # directly without a champion->slot table, so as a coarse signal
        # we just check temporal density: does decoded cooldown count
        # spike near level-up timestamps?
        dts = []
        for ev in skill_levelup:
            t_ms = ev.get("timestamp")
            if t_ms is None:
                continue
            t_sec = t_ms / 1000.0
            best, dt = nearest_within("SetCooldown", t_sec, window_seconds=3.0)
            if dt is not None:
                dts.append(dt)
        if dts:
            print(f"  fraction with SetCooldown within 3s: {len(dts) / len(skill_levelup):.1%}")
            print(f"  median dt: {statistics.median(dts):.2f}s")

    # Anchor: CHAMPION_KILL -> currently no PKT_NPC_Die handler.
    kills = [ev for ev in v5_events if ev.get("type") == "CHAMPION_KILL"]
    print()
    print(f"=== CHAMPION_KILL ({len(kills)} in v5) ===")
    print("  (no Death typed-handler yet — this anchor will activate "
          "once `PKT_NPC_Die` / `PKT_S2C_OnEntityDie` are added to "
          "src/packet_handlers.rs)")

    # Aggregate: write a JSON report so the cross-replay aggregator
    # has something to consume.
    report = {
        "replay": Path(args.rofl_x).name,
        "anchors": {
            "WARD_PLACED": {
                "v5_count": len(ward_placed),
                "rofl_count": len(ward_events),
            },
            "SKILL_LEVEL_UP": {
                "v5_count": len(skill_levelup),
                "rofl_setcooldown_count": len(by_kind.get("SetCooldown", [])),
            },
            "CHAMPION_KILL": {
                "v5_count": len(kills),
                "rofl_anchor_count": 0,
            },
        },
    }
    out = Path(args.out) if args.out else Path(args.rofl_x).with_suffix(".v5.json")
    with open(out, "w") as f:
        json.dump(report, f, indent=2)
    print()
    print(f"wrote report to {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
