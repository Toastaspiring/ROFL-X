#!/usr/bin/env python
"""Targeted brute-force for netids that the main run couldn't match.

Uses the 327 prologue-only candidates (decoder_prologue_candidates.json)
instead of the 41 high-confidence intersection. The prologue-only set
includes specialized decoders that aren't referenced via the dispatch
table (some packets go through alternate paths).

Inputs:
  ~/Tools/analysis/16-9/decoder_prologue_candidates.json  — 327 RVAs
  ~/Tools/analysis/16-9/brute_match_results.json          — existing
  patch/16-9.patch                                        — current
                                                            wired set

For each netid in the histogram NOT covered by the existing wiring,
runs trace-decoder over all 327 candidates. Updates the brute_match
results in-place. The existing wire_confirmed_decoders.py will then
pick up new winners on next run.

Usage: python scripts/brute_match_uncovered.py [--all]
  --all : sweep all 327 against ALL netids (very slow)
  default: sweep against just the netids with no hits in current results
"""

from __future__ import annotations

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ROFL_X = ROOT / "target" / "release" / "rofl-x.exe"
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
PATCH_PATH = ROOT / "patch" / "16-9.patch"

_REPLAY_BASE = Path(os.environ["USERPROFILE"]) / "Documents" / "League of Legends"
REPLAYS = _REPLAY_BASE / "Replays" if (_REPLAY_BASE / "Replays").exists() else _REPLAY_BASE / "replays"


def load_pdata_sizes(binary_path: Path) -> dict[int, int]:
    with open(binary_path, "rb") as f:
        binary = f.read()
    pe_off = struct.unpack_from("<I", binary, 0x3C)[0]
    n_secs = struct.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs: dict[str, tuple] = {}
    for i in range(n_secs):
        b = sec_start + 40 * i
        name = binary[b : b + 8].rstrip(b"\0").decode("latin-1")
        secs[name] = (
            struct.unpack_from("<I", binary, b + 12)[0],
            struct.unpack_from("<I", binary, b + 8)[0],
            struct.unpack_from("<I", binary, b + 20)[0],
            struct.unpack_from("<I", binary, b + 16)[0],
        )
    pd = secs[".pdata"]
    pdata = binary[pd[2] : pd[2] + min(pd[3], pd[1])]
    sizes = {}
    for i in range(0, len(pdata) - 12 + 1, 12):
        b, e, _ = struct.unpack_from("<III", pdata, i)
        if b > 0 and e > b:
            sizes[b] = e - b
    return sizes


def load_prologue_candidates() -> list[tuple[int, int]]:
    with open(ANALYSIS / "decoder_prologue_candidates.json") as f:
        d = json.load(f)
    return [(int(c["rva"], 16), int(c["rva_end"], 16)) for c in d["candidates"]]


def trace_one(replay: Path, netid: int, rva_start: int, rva_end: int, samples: int = 2) -> int:
    cmd = [
        str(ROFL_X), "trace-decoder",
        "--replay", str(replay),
        "--netid", str(netid),
        "--patch-dir", str(ROOT / "patch"),
        "--rva-start", f"0x{rva_start:x}",
        "--rva-end", f"0x{rva_end:x}",
        "--samples", str(samples),
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=20)
    except subprocess.TimeoutExpired:
        return -1
    out = result.stdout + result.stderr
    m = re.search(r"\((\d+) distinct offsets across", out)
    if m:
        return int(m.group(1))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true")
    args = ap.parse_args()

    candidates = load_prologue_candidates()
    print(f"loaded {len(candidates)} prologue candidates")

    # Pick the largest replay
    rofls = sorted(REPLAYS.glob("*.rofl"), key=lambda p: p.stat().st_size, reverse=True)
    if not rofls:
        print(f"no replays in {REPLAYS}")
        return 1
    replay = rofls[0]
    print(f"using replay: {replay.name}")

    # Determine target netids
    results_path = ANALYSIS / "brute_match_results.json"
    with open(results_path) as f:
        existing = json.load(f)
    if args.all:
        # All netids that exist in observed histogram
        target_netids = sorted(int(n) for n in existing["results"])
    else:
        # Only the ones with NO hits OR currently uncovered in patch
        with zipfile.ZipFile(PATCH_PATH) as z:
            cfg = json.loads(z.read("result.json"))
        covered = {e["netid"] for e in cfg.get("extra_decoders", [])}
        covered.add(cfg["mov_decrypt"]["netid"])
        target_netids = []
        for n_s, hits in existing["results"].items():
            n = int(n_s)
            if n in covered:
                continue
            target_netids.append(n)
        target_netids.sort()
    print(f"sweeping {len(target_netids)} target netids x {len(candidates)} candidates "
          f"~= {len(target_netids) * len(candidates) * 1.2 / 60:.0f} min")

    # Sweep — write incrementally so progress survives interruption
    out_path = ANALYSIS / "brute_match_results_extended.json"
    new_results = {}
    for idx, netid in enumerate(target_netids):
        print(f"=== [{idx + 1}/{len(target_netids)}] netid {netid} ===", flush=True)
        per_netid = []
        for rva, rva_end in candidates:
            n = trace_one(replay, netid, rva, rva_end)
            if n > 0:
                per_netid.append((rva, n))
        per_netid.sort(key=lambda x: -x[1])
        for rva, n in per_netid[:5]:
            print(f"  RVA 0x{rva:x}: {n} distinct offsets", flush=True)
        new_results[netid] = per_netid

        # Flush incrementally
        merged = dict(existing["results"])
        for n_id, hits in new_results.items():
            if hits:
                merged[str(n_id)] = [
                    {"rva": f"0x{rva:x}", "distinct_offsets": cnt}
                    for rva, cnt in hits
                ]
        with open(out_path, "w") as f:
            json.dump({"replay": existing.get("replay", replay.name), "results": merged}, f, indent=2)
    print(f"wrote {out_path}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
