#!/usr/bin/env python
"""Fill in `result.json` of an existing `patch/16-9.patch` skeleton.

Uses the RVAs documented in `docs/PATCH_16_9_ANALYSIS.md` as the seed.
Sizes resolved from the binary's `.pdata` so end-RVAs are exact.

Run after `rofl-x extract-patch` has produced the skeleton.
"""

from __future__ import annotations

import json
import os
import struct
import sys
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
PATCH_PATH = ROOT / "patch" / "16-9.patch"
BINARY_PATH = ANALYSIS / "league_16-9.exe"


def load_pdata_sizes(binary_path: Path) -> dict[int, int]:
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
    sizes = load_pdata_sizes(BINARY_PATH)

    def end(rva_hex: str) -> str:
        rva = int(rva_hex, 16)
        size = sizes.get(rva)
        if size is None:
            raise SystemExit(f"RVA {rva_hex} not found in .pdata")
        return f"0x{rva + size:x}"

    # Seed RVAs from docs/PATCH_16_9_ANALYSIS.md and README cold-start guide.
    MOV_RVA = "0xfb4070"
    WARD_RVA = "0xea9ec0"  # placeholder; ward netid not yet identified

    result = {
        "_comment": "Built by scripts/build_16_9_patch.py from doc-confirmed RVAs.",
        "player_id_start": "0x40000099",
        "alloc1_rva": "0x10053f0",
        "alloc2_rva": "0x10053f0",
        "skip_rva": "0x11b8430",
        "mov_decrypt": {
            "netid": 916,
            "rva_start": MOV_RVA,
            "rva_end": end(MOV_RVA),
            "output_format": "inline-floats",
            "inline_x_offset": "0x20",
            "inline_y_offset": "0x24",
            "payload_offset": "0x18",
            "payload_size_offset": "0x20",
        },
        "ward_spawn_decrypt": {
            "netid": 999999,  # not yet identified on 16.9; sentinel that won't match
            "rva_start": WARD_RVA,
            "rva_end": end(WARD_RVA),
            "id_offset": "0x48",
            "owner_id_offset": "0x18",
            "name_offset": "0x60",
            "name_len_offset": "0x68",
            "x_offset": "0x20",
            "x_write_count": "4",
            "y_offset": "0x28",
            "y_write_count": "4",
        },
        "text": {"rva": "0x1000", "size": 26530248},
        "data": {"rva": "0x1d7c000", "size": 1537248},
        "rdata": {"rva": "0x194f000", "size": 4377912},
        "extra_decoders": [],
    }

    # Read existing zip, replace result.json, write back
    if not PATCH_PATH.exists():
        raise SystemExit(f"missing {PATCH_PATH}; run extract-patch first")
    with zipfile.ZipFile(PATCH_PATH) as z:
        files = {n: z.read(n) for n in z.namelist()}
    files["result.json"] = json.dumps(result, indent=2).encode()
    with zipfile.ZipFile(PATCH_PATH, "w", zipfile.ZIP_DEFLATED) as z:
        for n, data in files.items():
            z.writestr(n, data)
    print(f"updated {PATCH_PATH}")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
