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
# Replays directory: Riot uses "Replays" (capital R) on Windows but lowercase
# elsewhere; check both.
_REPLAY_BASE = Path(os.environ["USERPROFILE"]) / "Documents" / "League of Legends"
REPLAYS = _REPLAY_BASE / "Replays" if (_REPLAY_BASE / "Replays").exists() else _REPLAY_BASE / "replays"

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
    (410, 824, "0x019a"),
    (1000, 717, "0x03e8"),
    (1134, 663, "0x046e"),
    (1099, 614, "0x044b"),
    (647, 602, "0x0287"),
    (1034, 507, "0x040a"),
    (569, 493, "0x0239"),
    (1104, 492, "0x0450"),
    (959, 486, "0x03bf"),
    (166, 479, "0x00a6"),
    (338, 475, "0x0152"),
    (153, 446, "0x0099"),
    (105, 344, "0x0069"),
    (760, 291, "0x02f8"),
    (575, 273, "0x023f"),
    (799, 272, "0x031f"),
    (1193, 236, "0x04a9"),
    (962, 232, "0x03c2"),
    (97, 231, "0x0061"),
    (51, 224, "0x0033"),
    (422, 204, "0x01a6"),
    (157, 202, "0x009d"),
    (843, 190, "0x034b"),
    (449, 173, "0x01c1"),
    (65, 170, "0x0041"),
    (306, 169, "0x0132"),
    (803, 168, "0x0323"),
    (615, 167, "0x0267"),
    (838, 159, "0x0346"),
    (653, 157, "0x028d"),
    (733, 147, "0x02dd"),
    (1196, 147, "0x04ac"),
    (781, 135, "0x030d"),
    (423, 130, "0x01a7"),
    (757, 130, "0x02f5"),
    (950, 130, "0x03b6"),
    (499, 128, "0x01f3"),
    (1011, 126, "0x03f3"),
    (175, 121, "0x00af"),
    (593, 118, "0x0251"),
    (1077, 113, "0x0435"),
    (462, 112, "0x01ce"),
    (644, 106, "0x0284"),
    (115, 99, "0x0073"),
    (430, 99, "0x01ae"),
    (990, 98, "0x03de"),
    (591, 97, "0x024f"),
    (1045, 85, "0x0415"),
    (951, 78, "0x03b7"),
    (928, 72, "0x03a0"),
    (298, 68, "0x012a"),
    (660, 62, "0x0294"),
    (1021, 55, "0x03fd"),
    (567, 53, "0x0237"),
    (400, 52, "0x0190"),
    (888, 51, "0x0378"),
    (907, 51, "0x038b"),
    (522, 49, "0x020a"),
    (252, 40, "0x00fc"),
    (277, 37, "0x0115"),
    (413, 37, "0x019d"),
    (472, 37, "0x01d8"),
    (715, 37, "0x02cb"),
    (794, 37, "0x031a"),
    (447, 36, "0x01bf"),
    (896, 36, "0x0380"),
    (296, 35, "0x0128"),
    (143, 33, "0x008f"),
    (259, 31, "0x0103"),
    (934, 30, "0x03a6"),
    (531, 28, "0x0213"),
    (648, 28, "0x0288"),
    (939, 28, "0x03ab"),
    (362, 27, "0x016a"),
    (572, 24, "0x023c"),
    (1093, 23, "0x0445"),
    (1110, 22, "0x0456"),
    (613, 21, "0x0265"),
    (9, 20, "0x0009"),
    (156, 20, "0x009c"),
    (268, 20, "0x010c"),
    (465, 20, "0x01d1"),
    (793, 20, "0x0319"),
    (451, 18, "0x01c3"),
    (1138, 18, "0x0472"),
    (471, 17, "0x01d7"),
    (505, 17, "0x01f9"),
    (1190, 17, "0x04a6"),
    (497, 16, "0x01f1"),
    (708, 16, "0x02c4"),
    (76, 15, "0x004c"),
    (271, 15, "0x010f"),
    (191, 13, "0x00bf"),
    (491, 11, "0x01eb"),
    (128, 10, "0x0080"),
    (174, 10, "0x00ae"),
    (224, 10, "0x00e0"),
    (378, 10, "0x017a"),
    (390, 10, "0x0186"),
    (455, 10, "0x01c7"),
    (533, 10, "0x0215"),
    (873, 10, "0x0369"),
    (1153, 9, "0x0481"),
    (1195, 9, "0x04ab"),
    (463, 8, "0x01cf"),
    (982, 7, "0x03d6"),
    (60, 6, "0x003c"),
    (41, 5, "0x0029"),
    (205, 5, "0x00cd"),
    (276, 5, "0x0114"),
    (560, 5, "0x0230"),
    (738, 4, "0x02e2"),
    (835, 4, "0x0343"),
    (666, 3, "0x029a"),
    (955, 3, "0x03bb"),
    (168, 2, "0x00a8"),
    (382, 2, "0x017e"),
    (641, 2, "0x0281"),
    (1107, 2, "0x0453"),
    (1123, 2, "0x0463"),
    (1179, 2, "0x049b"),
    (21, 1, "0x0015"),
    (33, 1, "0x0021"),
    (86, 1, "0x0056"),
    (155, 1, "0x009b"),
    (173, 1, "0x00ad"),
    (216, 1, "0x00d8"),
    (223, 1, "0x00df"),
    (392, 1, "0x0188"),
    (442, 1, "0x01ba"),
    (506, 1, "0x01fa"),
    (623, 1, "0x026f"),
    (658, 1, "0x0292"),
    (999, 1, "0x03e7"),
    (1058, 1, "0x0422"),
    (1177, 1, "0x0499"),
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

    # Pick the largest 16.9 replay. Older runs hard-coded EUW1-7841* (the
    # specific replay used during initial RE); for portability we just take
    # the largest .rofl in the Replays dir.
    rofls = sorted(REPLAYS.glob("*.rofl"), key=lambda p: p.stat().st_size, reverse=True)
    if not rofls:
        print(f"no replays found in {REPLAYS}")
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
