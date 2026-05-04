#!/usr/bin/env python
"""Probe each unmatched netid using its constructor-derived decoder.

For netids that didn't get a brute-force match, try the decoder RVA
that came out of the e83930 jump-table → constructor → LEA-scan.

For each (netid, decoder_rva), runs trace-decoder and reports the
write count. If non-zero, wires it into patch/16-9.patch.
"""

from __future__ import annotations

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
BINARY_PATH = ANALYSIS / "league_16-9.exe"

_REPLAY_BASE = Path(os.environ["USERPROFILE"]) / "Documents" / "League of Legends"
REPLAYS = _REPLAY_BASE / "Replays" if (_REPLAY_BASE / "Replays").exists() else _REPLAY_BASE / "replays"


def load_pdata_sizes(binary_path: Path) -> dict[int, int]:
    with open(binary_path, "rb") as f:
        binary = f.read()
    pe_off = struct.unpack_from("<I", binary, 0x3C)[0]
    n_secs = struct.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs: dict = {}
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


def trace_one(replay: Path, netid: int, rva_start: int, rva_end: int, samples: int = 3) -> int:
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
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        return -1
    out = result.stdout + result.stderr
    m = re.search(r"\((\d+) distinct offsets across", out)
    if m:
        return int(m.group(1))
    return 0


def main() -> int:
    with open(ANALYSIS / "netid_to_decoder.json") as f:
        nm = json.load(f)["resolved"]

    rofls = sorted(REPLAYS.glob("*.rofl"), key=lambda p: p.stat().st_size, reverse=True)
    if not rofls:
        print(f"no replays in {REPLAYS}")
        return 1
    replay = rofls[0]

    # Load current wired netids to find the unmatched ones
    with zipfile.ZipFile(PATCH_PATH) as z:
        cfg = json.loads(z.read("result.json"))
    covered = {e["netid"] for e in cfg.get("extra_decoders", [])}
    covered.add(cfg["mov_decrypt"]["netid"])

    sizes = load_pdata_sizes(BINARY_PATH)

    # Find unmatched netids that DO have a constructor mapping
    candidates_to_probe = []
    for netid_s, info in nm.items():
        netid = int(netid_s)
        if netid in covered:
            continue
        decoder_rva = int(info["decoder_rva"], 16)
        if decoder_rva not in sizes:
            continue
        candidates_to_probe.append((netid, decoder_rva, sizes[decoder_rva]))

    print(f"probing {len(candidates_to_probe)} unmatched netids "
          f"with constructor-derived decoder RVAs")

    confirmed = []
    for netid, rva, size in candidates_to_probe:
        rva_end = rva + size
        n = trace_one(replay, netid, rva, rva_end, samples=3)
        marker = "match" if n > 0 else "no writes"
        print(f"  netid {netid:5d}: 0x{rva:x} (size {size}) -> {n} offsets [{marker}]", flush=True)
        if n > 0:
            confirmed.append((netid, rva, rva_end, size, n))

    print(f"\nfound {len(confirmed)} new matches")

    # Wire into patch
    if confirmed:
        extras = list(cfg.get("extra_decoders", []))
        for netid, rva, rva_end, size, writes in confirmed:
            extras.append({
                "name": f"netid_{netid}",
                "netid": netid,
                "rva_start": f"0x{rva:x}",
                "rva_end": f"0x{rva_end:x}",
                "struct_size": "0x90",
                "semantic_hint": f"ctor-mapped ({writes} writes via e83930 jumptable[{netid}])",
            })
        cfg["extra_decoders"] = extras
        with zipfile.ZipFile(PATCH_PATH) as z:
            files = {n: z.read(n) for n in z.namelist()}
        files["result.json"] = json.dumps(cfg, indent=2).encode()
        with zipfile.ZipFile(PATCH_PATH, "w", zipfile.ZIP_DEFLATED) as z:
            for n, data in files.items():
                z.writestr(n, data)
        print(f"wrote {len(extras)} total extra_decoders to {PATCH_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
