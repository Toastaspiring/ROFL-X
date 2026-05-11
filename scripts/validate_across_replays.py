#!/usr/bin/env python
"""Run typed-event validation across many replays and aggregate.

Each individual replay only proves so much: a class might happen to
look right on one game and fail on another. The README's promotion
threshold from "shape-match" to "proven" is "5+ replays". This
script walks a directory of `.rofl` files, runs the full pipeline
on each, then runs `validate_typed_events.py` on each output, and
aggregates the per-class verdicts.

Outputs a confidence matrix:

  Class                  PASS  WEAK  FAIL  verdict_across_replays
  OnLeaveVisibility         3     2     0  WEAK (consistent)
  BasicAttackPos            0     5     0  WEAK (very consistent)
  SetCooldown               0     0     5  FAIL (writes-log gap)

A class earns "proven" when PASS >= 80% across replays. WEAK with
high consistency tells us the catalog has a real but offset-level
bug. FAIL across all replays means the class label or the offsets
are wrong, not just bad luck.

Usage:
  python scripts/validate_across_replays.py \\
      --replays-dir "%USERPROFILE%\\Documents\\League of Legends\\Replays" \\
      --patch-dir patch \\
      --rofl-x-bin target/release/rofl-x.exe \\
      --max-replays 5 \\
      --out cross-replay-report.json
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path


def run_pipeline(rofl_x_bin: Path, patch_dir: Path, replay: Path, out: Path) -> bool:
    """Returns True on success."""
    proc = subprocess.run(
        [str(rofl_x_bin), "file",
         "-r", str(replay),
         "-o", str(out),
         "--patch-dir", str(patch_dir)],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        print(f"  rofl-x failed: {proc.stderr.strip()[:200]}", file=sys.stderr)
        return False
    return True


def run_validator(validator: Path, output_json: Path) -> dict:
    """Run validate_typed_events.py and parse stdout into a dict.

    The validator prints a structured per-class block; we just re-
    derive verdicts from the JSON since we have it loaded anyway.
    Importing the module is cleaner than re-parsing CLI text — even
    though it's a script.
    """
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    try:
        import validate_typed_events as v  # type: ignore
    finally:
        sys.path.pop(0)

    with open(output_json) as f:
        rofl = json.load(f)

    typed = rofl.get("typed_events", []) or []
    by_kind = defaultdict(list)
    all_eids = set()
    for e in typed:
        by_kind[e["kind"]].append(e)
        if v.is_entity_id(e.get("entity_id")):
            all_eids.add(e["entity_id"])
    for label, samples in (rofl.get("extra_decoders") or {}).items():
        for s in samples:
            eid = s.get("entity_id")
            if v.is_entity_id(eid):
                all_eids.add(eid)

    return {
        "OnLeaveVisibility": v.validate_on_leave_visibility(
            by_kind.get("OnLeaveVisibility", []), all_eids),
        "BasicAttackPos": v.validate_basic_attack_pos(
            by_kind.get("BasicAttackPos", [])),
        "SetCooldown": v.validate_set_cooldown(
            by_kind.get("SetCooldown", [])),
        "NpcBuffAdd": v.validate_npc_buff_add(
            by_kind.get("NpcBuffAdd", [])),
        "AddBuffModifier": v.validate_add_buff_modifier(
            by_kind.get("AddBuffModifier", [])),
        "NpcHeroDie": v.validate_npc_hero_die(
            by_kind.get("NpcHeroDie", [])),
        "NpcDieMapView": v.validate_npc_die_map_view(
            by_kind.get("NpcDieMapView", [])),
        "AddFollowTargetPosition": v.validate_add_follow_target_position(
            by_kind.get("AddFollowTargetPosition", [])),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--replays-dir", required=True)
    ap.add_argument("--patch-dir", default="patch")
    ap.add_argument("--rofl-x-bin", default="target/release/rofl-x.exe")
    ap.add_argument("--max-replays", type=int, default=5,
                    help="cap replays tried; default 5 matches README threshold")
    ap.add_argument("--out", default="cross-replay-report.json")
    args = ap.parse_args()

    rd = Path(os.path.expandvars(args.replays_dir))
    rofls = sorted(rd.glob("*.rofl"))[: args.max_replays]
    if not rofls:
        print(f"no .rofl files in {rd}", file=sys.stderr)
        return 1

    print(f"running across {len(rofls)} replay(s) from {rd}")
    per_replay = {}
    skipped = []
    with tempfile.TemporaryDirectory() as td:
        for i, r in enumerate(rofls):
            print(f"\n[{i+1}/{len(rofls)}] {r.name}")
            tmp_out = Path(td) / f"out_{i}.json"
            if not run_pipeline(Path(args.rofl_x_bin), Path(args.patch_dir),
                                r, tmp_out):
                skipped.append(r.name)
                continue
            try:
                results = run_validator(
                    Path(__file__).resolve().parent / "validate_typed_events.py",
                    tmp_out,
                )
            except Exception as e:
                print(f"  validator failed: {e}", file=sys.stderr)
                skipped.append(r.name)
                continue
            per_replay[r.name] = results
            for kind, result in results.items():
                if result is None:
                    continue
                print(f"  {kind:20s} -> {result['verdict']:5s}  "
                      f"({result['samples']:>6,} samples)")

    # Aggregate
    print(f"\n=== Aggregate across {len(per_replay)} replay(s) "
          f"({len(skipped)} skipped) ===")
    aggregate = {}
    classes = ["OnLeaveVisibility", "BasicAttackPos",
               "SetCooldown", "NpcBuffAdd", "AddBuffModifier",
               "NpcHeroDie", "NpcDieMapView", "AddFollowTargetPosition"]
    print(f"{'Class':>20s}  {'PASS':>5s}  {'WEAK':>5s}  {'FAIL':>5s}  consensus")
    for kind in classes:
        verdicts = []
        for r_name, results in per_replay.items():
            res = results.get(kind)
            if res is not None:
                verdicts.append(res["verdict"])
        cnt = Counter(verdicts)
        # Consensus rule: PASS in 80%+ -> proven; PASS in 0%, FAIL in 0%
        # but WEAK consistent -> "consistent gap"; otherwise "mixed".
        n = len(verdicts)
        if n == 0:
            consensus = "no_data"
        elif cnt["PASS"] / n >= 0.8:
            consensus = "proven"
        elif cnt["FAIL"] / n >= 0.8:
            consensus = "fail_consistent"
        elif cnt["WEAK"] / n >= 0.8:
            consensus = "weak_consistent"
        else:
            consensus = "mixed"
        aggregate[kind] = {
            "verdicts": dict(cnt),
            "n_replays": n,
            "consensus": consensus,
        }
        print(f"{kind:>20s}  {cnt['PASS']:>5d}  {cnt['WEAK']:>5d}  "
              f"{cnt['FAIL']:>5d}  {consensus}")

    report = {
        "n_replays_run": len(per_replay),
        "n_replays_skipped": len(skipped),
        "skipped_replays": skipped,
        "per_replay": per_replay,
        "aggregate": aggregate,
    }
    with open(args.out, "w") as f:
        json.dump(report, f, indent=2)
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
