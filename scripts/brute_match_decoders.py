#!/usr/bin/env python
"""
Brute-force match high-frequency netids to candidate decoders.

For each (netid, candidate_decoder) pair, run trace-decoder via the rofl-x
CLI and count how many distinct struct offsets the candidate writes to
when given that netid's payload bytes. The (netid, decoder) pair that
produces the most consistent struct writes is the likely decoder for
that packet class.

This narrows the search from "find the right decoder among 41 candidates
for each of N unknown netids" to a ranked table the user can refine.

Output: brute_match_results.json under the analysis dir, plus a
human-readable summary printed to stdout.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ROFL_X = ROOT / "target" / "release" / "rofl-x.exe"
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
PATCH_DIR = ROOT / "patch"
REPLAYS = Path(os.environ["USERPROFILE"]) / "Documents" / "League of Legends" / "replays"

# Top high-frequency netids we want to identify decoders for. The brute
# force is per-pair x ~1 sec, so 30 netids x 41 candidates ~= 20 min.
# Priority order: highest replay-frequency first.
TARGET_NETIDS = [
    (1068, 283279, "0x042c"),
    (707,   44376, "0x02c3"),
    (652,   35310, "0x028c"),
    (806,   27301, "0x0326"),
    (916,   23159, "0x0394 (mov, confirmed)"),
    (406,   19734, "0x0196"),
    (684,   18414, "0x02ac"),
    (1112,  12019, "0x0458"),
    (717,   10807, "0x02cd"),
    (1055,  10674, "0x041f"),
    (100,   10538, "0x0064"),
    (132,   9230,  "0x0084"),
    (347,   9061,  "0x015b"),
    (438,   8332,  "0x01b6"),
    (201,   7636,  "0x00c9"),
    (876,   6686,  "0x036c"),
    (828,   6550,  "0x033c"),
    (807,   6494,  "0x0327"),
    (89,    6348,  "0x0059"),
    (598,   5793,  "0x0256"),
    (446,   5500,  "0x01be"),
    (1102,  5000,  "0x044e"),
    (782,   4500,  "0x030e"),
    (1117,  4000,  "0x045d"),
    (52,    3500,  "0x0034"),
    (124,   3200,  "0x007c"),
    (1062,  3000,  "0x0426"),
    (476,   2800,  "0x01dc"),
    (1083,  2600,  "0x043b"),
    (1061,  2400,  "0x0425"),
    # positions 30-60 (next batch — covers ~3.5% additional blocks)
    (964,  5467, "0x03c4"),
    (104,  5105, "0x0068"),
    (1139, 5028, "0x0473"),
    (559,  4943, "0x022f"),
    (220,  4678, "0x00dc"),
    (452,  3818, "0x01c4"),
    (360,  3613, "0x0168"),
    (346,  3599, "0x015a"),
    (27,   3531, "0x001b"),
    (920,  3503, "0x0398"),
    (165,  3472, "0x00a5"),
    (429,  2545, "0x01ad"),
    (436,  2545, "0x01b4"),
    (700,  2401, "0x02bc"),
    (831,  2333, "0x033f"),
    (985,  2296, "0x03d9"),
    (627,  2272, "0x0273"),
    (742,  2099, "0x02e6"),
    (756,  1711, "0x02f4"),
    (421,  1590, "0x01a5"),
    (1178, 1444, "0x049a"),
    (758,  1259, "0x02f6"),
    (188,  1242, "0x00bc"),
    (64,   1225, "0x0040"),
    (975,  1181, "0x03cf"),
    (720,  1158, "0x02d0"),
    (1159, 1070, "0x0487"),
    (785,  999,  "0x0311"),
    (214,  881,  "0x00d6"),
    (1151, 871,  "0x047f"),
]


def load_high_confidence() -> list[tuple[int, int, str]]:
    """Returns (rva_start, size, label)."""
    with open(ANALYSIS / "decoders_confirmed.json") as f:
        data = json.load(f)
    rvas = [int(s, 16) for s in data["high_confidence_decoders"]]

    # Get sizes from the live binary's .pdata
    import struct as st
    with open(ANALYSIS / "league_16-9.exe", "rb") as f:
        binary = f.read()
    pe_off = st.unpack_from("<I", binary, 0x3C)[0]
    n_secs = st.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = st.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs: dict[str, tuple[int, int, int, int]] = {}
    for i in range(n_secs):
        b = sec_start + 40 * i
        name = binary[b : b + 8].rstrip(b"\0").decode("latin-1")
        secs[name] = (
            st.unpack_from("<I", binary, b + 12)[0],
            st.unpack_from("<I", binary, b + 8)[0],
            st.unpack_from("<I", binary, b + 20)[0],
            st.unpack_from("<I", binary, b + 16)[0],
        )
    pd_off = secs[".pdata"][2]
    pdata = binary[pd_off : pd_off + min(secs[".pdata"][3], secs[".pdata"][1])]
    fn_size: dict[int, int] = {}
    for i in range(0, len(pdata) - 12 + 1, 12):
        b, e, _ = st.unpack_from("<III", pdata, i)
        if b > 0 and e > b:
            fn_size[b] = e - b
    return [(rva, fn_size.get(rva, 0), f"0x{rva:x}") for rva in sorted(rvas) if rva in fn_size]


def trace_one(replay: Path, netid: int, rva_start: int, rva_end: int, samples: int = 3) -> int:
    """Run trace-decoder and return number of distinct offsets it found."""
    cmd = [
        str(ROFL_X),
        "trace-decoder",
        "--replay",
        str(replay),
        "--netid",
        str(netid),
        "--patch-dir",
        str(PATCH_DIR),
        "--rva-start",
        f"0x{rva_start:x}",
        "--rva-end",
        f"0x{rva_end:x}",
        "--samples",
        str(samples),
    ]
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=20
        )
    except subprocess.TimeoutExpired:
        return -1
    out = result.stdout + result.stderr
    # Parse "X distinct offsets"
    m = re.search(r"\((\d+) distinct offsets across", out)
    if m:
        return int(m.group(1))
    return 0


def main() -> int:
    candidates = load_high_confidence()
    print(f"loaded {len(candidates)} high-confidence candidates")

    # Pick the largest 16.9 replay
    rofls = sorted(REPLAYS.glob("EUW1-7841*.rofl"), key=lambda p: p.stat().st_size, reverse=True)
    if not rofls:
        print("no 16.9 replays found")
        return 1
    replay = rofls[0]
    print(f"using replay: {replay.name}")
    print()

    # For each target netid, sweep candidates. Save results.
    results: dict[int, list[tuple[int, int]]] = {}
    for netid, freq, sig in TARGET_NETIDS:
        print(f"=== netid {netid} ({sig}, freq {freq}) ===")
        per_netid: list[tuple[int, int]] = []  # (rva, distinct_offsets)
        for rva, size, label in candidates:
            rva_end = rva + size
            n = trace_one(replay, netid, rva, rva_end, samples=2)
            if n > 0:
                per_netid.append((rva, n))
        per_netid.sort(key=lambda x: -x[1])
        for rva, n in per_netid[:5]:
            print(f"  RVA 0x{rva:x}: {n} distinct offsets")
        if not per_netid:
            print("  no candidate produced struct writes")
        results[netid] = per_netid
        print()

    out_path = ANALYSIS / "brute_match_results.json"
    with open(out_path, "w") as f:
        json.dump(
            {
                "replay": replay.name,
                "results": {
                    str(netid): [{"rva": f"0x{rva:x}", "distinct_offsets": n} for rva, n in results[netid]]
                    for netid in results
                },
            },
            f,
            indent=2,
        )
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
