#!/usr/bin/env python
"""
Read brute_match_results.json and auto-update patch/16-9.patch with the
confidently-matched decoders.

Confidence filter:
  - Winning RVA must beat runner-up by >= MIN_GAP distinct offsets
  - "Noisy" RVAs (those that win as top match for >= NOISE_THRESHOLD
    different netids) are excluded — they're functions that write to
    many offsets unconditionally regardless of input, and falsely beat
    real matches by sheer write count.

Usage: scripts/wire_confirmed_decoders.py [--dry-run]
"""

from __future__ import annotations

import json
import os
import struct
import sys
import zipfile
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
PATCH_PATH = ROOT / "patch" / "16-9.patch"
RESULTS_PATH = ANALYSIS / "brute_match_results.json"
BINARY_PATH = ANALYSIS / "league_16-9.exe"

MIN_GAP = 2          # winner must lead runner-up by at least this many writes
NOISE_THRESHOLD = 3  # an RVA that wins for >= N netids is "noisy"
MIN_WRITES = 3       # winner must produce at least this many distinct offsets


def load_pdata_sizes(binary_path: Path) -> dict[int, int]:
    """Function start RVA -> size, from .pdata."""
    with open(binary_path, "rb") as f:
        binary = f.read()
    pe_off = struct.unpack_from("<I", binary, 0x3C)[0]
    n_secs = struct.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs: dict[str, tuple[int, int, int, int]] = {}
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
    pdata_bytes = binary[pd[2] : pd[2] + min(pd[3], pd[1])]
    sizes: dict[int, int] = {}
    for i in range(0, len(pdata_bytes) - 12 + 1, 12):
        b, e, _ = struct.unpack_from("<III", pdata_bytes, i)
        if b > 0 and e > b:
            sizes[b] = e - b
    return sizes


def main() -> int:
    dry_run = "--dry-run" in sys.argv
    if not RESULTS_PATH.exists():
        print(f"missing {RESULTS_PATH}; run scripts/brute_match_decoders.py first")
        return 1

    with open(RESULTS_PATH) as f:
        brute = json.load(f)
    fn_size = load_pdata_sizes(BINARY_PATH)

    # Identify noisy RVAs (top winners across many netids)
    top_winner_count: Counter[str] = Counter()
    for netid, hits in brute["results"].items():
        if hits:
            top_winner_count[hits[0]["rva"]] += 1
    noisy = {rva for rva, n in top_winner_count.items() if n >= NOISE_THRESHOLD}
    print(f"noisy RVAs (winning >= {NOISE_THRESHOLD} netids): {sorted(noisy)}")
    print()

    # Pick best non-noisy match per netid with gap filter
    confirmed: list[tuple[int, str, int, int, int]] = []  # (netid, rva, writes, gap, size)
    for netid_s, hits in brute["results"].items():
        netid = int(netid_s)
        # Filter out noisy RVAs from the candidate list
        clean = [h for h in hits if h["rva"] not in noisy]
        if not clean:
            continue
        top = clean[0]
        runner_up = clean[1] if len(clean) > 1 else None
        gap = top["distinct_offsets"] - (runner_up["distinct_offsets"] if runner_up else 0)
        if top["distinct_offsets"] < MIN_WRITES:
            continue
        if gap < MIN_GAP:
            continue
        rva_int = int(top["rva"], 16)
        size = fn_size.get(rva_int, 0)
        if size == 0:
            continue
        confirmed.append((netid, top["rva"], top["distinct_offsets"], gap, size))

    print(f"=== {len(confirmed)} confidently matched netids ===")
    print(f'{"netid":>6}  {"decoder":>10}  {"writes":>6}  {"gap":>4}  {"size":>5}')
    for netid, rva, writes, gap, size in sorted(confirmed):
        print(f"{netid:>6}  {rva:>10}  {writes:>6}  {gap:>4}  {size:>5}")
    print()

    # Update patch
    with zipfile.ZipFile(PATCH_PATH) as z:
        files = {n: z.read(n) for n in z.namelist()}
    result = json.loads(files["result.json"])
    extras = []
    for netid, rva, writes, gap, size in confirmed:
        rva_int = int(rva, 16)
        extras.append(
            {
                "name": f"netid_{netid}",
                "netid": netid,
                "rva_start": rva,
                "rva_end": f"0x{rva_int + size:x}",
                "struct_size": "0x90",
                "semantic_hint": f"brute-matched ({writes} writes, gap {gap} over runner-up)",
            }
        )
    result["extra_decoders"] = extras

    if dry_run:
        print("--dry-run, not writing")
        return 0

    files["result.json"] = json.dumps(result, indent=2).encode()
    with zipfile.ZipFile(PATCH_PATH, "w", zipfile.ZIP_DEFLATED) as z:
        for n, data in files.items():
            z.writestr(n, data)
    print(f"wrote {len(extras)} extra_decoders to {PATCH_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
