#!/usr/bin/env python
"""Intersect prologue-pattern hits with dispatch-table slot[1] entries.

Reads:
  ~/Tools/analysis/16-9/decoder_prologue_candidates.json
  ~/Tools/analysis/16-9/dispatch_table_full.json

Writes:
  ~/Tools/analysis/16-9/decoders_confirmed.json
    {
      "high_confidence_decoders": ["0x...", ...],   # prologue ∩ dispatch
      ...
    }

This is the input to scripts/brute_match_decoders.py (which has hardcoded
the path).
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
PROLOGUE_PATH = ANALYSIS / "decoder_prologue_candidates.json"
DISPATCH_PATH = ANALYSIS / "dispatch_table_full.json"
OUT_PATH = ANALYSIS / "decoders_confirmed.json"


def main() -> int:
    with open(PROLOGUE_PATH) as f:
        prologue = json.load(f)
    with open(DISPATCH_PATH) as f:
        dispatch = json.load(f)

    prologue_rvas = {int(c["rva"], 16): c for c in prologue["candidates"]}
    dispatch_rvas = {int(d["rva"], 16): d for d in dispatch["decoders"]}

    confirmed = sorted(prologue_rvas.keys() & dispatch_rvas.keys())

    print(f"prologue candidates: {len(prologue_rvas)}")
    print(f"dispatch slot[1] decoders: {len(dispatch_rvas)}")
    print(f"intersection (high-confidence): {len(confirmed)}")

    out = {
        "prologue_count": len(prologue_rvas),
        "dispatch_count": len(dispatch_rvas),
        "high_confidence_decoders": [f"0x{r:x}" for r in confirmed],
        "details": [
            {
                "rva": f"0x{r:x}",
                "size_bytes": prologue_rvas[r]["size_bytes"],
                "in_subtables": dispatch_rvas[r]["in_subtables"],
            }
            for r in confirmed
        ],
    }
    with open(OUT_PATH, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {OUT_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
