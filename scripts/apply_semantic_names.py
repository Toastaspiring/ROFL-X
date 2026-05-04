#!/usr/bin/env python
"""Augment a rofl-x output JSON with semantic field names per netid.

Reads:
  - <input>.json from `rofl-x file --output ...`
  - scripts/semantic_field_names.json (this directory)

Writes (in-place if --inplace, else to <input>.semantic.json):
  Each entry under `extra_decoders.<name>_netid<N>[i].decoded_fields[]`
  gets an extra `name` and `class` field when its offset is in the
  semantic map.

Usage:
  python scripts/apply_semantic_names.py <output.json> [--inplace] [--patch 16.9]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


SEMANTIC_PATH = Path(__file__).resolve().parent / "semantic_field_names.json"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("--inplace", action="store_true")
    ap.add_argument("--patch", default="16.9")
    args = ap.parse_args()

    with open(args.input) as f:
        out_json = json.load(f)
    with open(SEMANTIC_PATH) as f:
        semantic = json.load(f)

    if args.patch not in semantic:
        print(f"no semantic map for patch {args.patch}", file=sys.stderr)
        return 1
    by_netid = semantic[args.patch]

    if "extra_decoders" not in out_json:
        print("input has no extra_decoders[]; nothing to annotate", file=sys.stderr)
        return 0

    annotated = 0
    classes_named = 0
    for label, samples in out_json["extra_decoders"].items():
        # Label format: <name>_netid<N>
        if "_netid" not in label:
            continue
        netid_s = label.rsplit("_netid", 1)[1]
        netid = str(int(netid_s))
        if netid not in by_netid:
            continue
        spec = by_netid[netid]
        classes_named += 1
        for sample in samples:
            sample.setdefault("class_hypothesis", spec.get("class"))
            sample.setdefault("class_comment", spec.get("comment"))
            for f in sample.get("decoded_fields", []):
                off_hex = f"0x{f['offset']:04x}" if isinstance(f.get("offset"), int) else f.get("offset")
                if off_hex in spec.get("fields", {}):
                    fdef = spec["fields"][off_hex]
                    f.setdefault("name", fdef["name"])
                    f.setdefault("type_hint", fdef["type"])
                    annotated += 1

    print(f"annotated {classes_named} classes, {annotated} field instances")

    out_path = Path(args.input) if args.inplace else Path(args.input).with_suffix(".semantic.json")
    with open(out_path, "w") as f:
        json.dump(out_json, f, indent=2)
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
