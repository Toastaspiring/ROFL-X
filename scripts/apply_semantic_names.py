#!/usr/bin/env python
"""Augment a rofl-x output JSON with semantic field names per netid.

Reads:
  - <input>.json from `rofl-x file --output ...`
  - scripts/semantic_field_names.json (per-decoder catalog)

Writes (in-place if --inplace, else to <input>.semantic.json):
  Each entry under `extra_decoders.<name>_netid<N>[i].decoded_fields[]`
  gets an extra `name` and `type_hint` field when its offset is in the
  catalog. Each sample also gets `class` and `class_comment`.

Usage:
  python scripts/apply_semantic_names.py <output.json> [--inplace]
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
    args = ap.parse_args()

    with open(args.input) as f:
        out_json = json.load(f)
    with open(SEMANTIC_PATH) as f:
        catalog = json.load(f)

    netid_to_decoder = catalog.get("netid_to_decoder", {})
    decoders = catalog.get("decoders", {})

    if "extra_decoders" not in out_json:
        print("input has no extra_decoders[]; nothing to annotate", file=sys.stderr)
        return 0

    annotated = 0
    samples_annotated = 0
    classes_named = 0
    for label, samples in out_json["extra_decoders"].items():
        if "_netid" not in label:
            continue
        try:
            netid = label.rsplit("_netid", 1)[1]
        except ValueError:
            continue
        decoder_rva = netid_to_decoder.get(netid)
        if not decoder_rva:
            continue
        decoder_info = decoders.get(decoder_rva, {})
        cls = decoder_info.get("class", "Unknown")
        cls_comment = decoder_info.get("comment", "")
        confidence = decoder_info.get("confidence", "")
        fields_map = decoder_info.get("fields", {})
        if cls != "Unknown":
            classes_named += 1
        for sample in samples:
            sample.setdefault("class", cls)
            sample.setdefault("class_comment", cls_comment)
            sample.setdefault("class_confidence", confidence)
            samples_annotated += 1
            for f in sample.get("decoded_fields", []):
                off = f.get("offset")
                off_hex = f"0x{off:04x}" if isinstance(off, int) else off
                if off_hex in fields_map:
                    fdef = fields_map[off_hex]
                    f.setdefault("name", fdef["name"])
                    f.setdefault("type_hint", fdef["type"])
                    annotated += 1

    print(f"annotated {classes_named} classes, "
          f"{samples_annotated} samples, "
          f"{annotated} fields with names")

    out_path = Path(args.input) if args.inplace else Path(args.input).with_suffix(".semantic.json")
    with open(out_path, "w") as f:
        json.dump(out_json, f, indent=2)
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
