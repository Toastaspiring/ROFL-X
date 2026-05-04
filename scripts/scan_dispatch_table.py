#!/usr/bin/env python
"""Scan a PE binary's `.rdata` for the packet-class dispatch table.

The 16.9 dispatch architecture (per docs/PATCH_16_9_ANALYSIS.md):

  - 48-byte stride entries, each holding 6 u64 values
  - Entries store full virtual addresses (image_base + RVA)
  - A "sub-table" is a run of >= 5 contiguous entries where slot[0]
    is constant (typically 0x1dc000 = `ret 0` stub VA)
  - Slot[1] is the per-packet decoder
  - Sub-tables are NOT all at the same alignment within .rdata; each
    one starts at its own 8-byte-aligned offset.

Strategy:
  - At every 8-byte aligned offset, treat (off, off+48, off+96, ...)
    as candidate consecutive entries.
  - For each starting offset, count how many consecutive entries have
    text-section VAs in slot[0] AND slot[1] AND share the same slot[0].
  - Run-length >= 5 = a sub-table.
  - Merge overlapping sub-tables that start in the same window.

Output: JSON at <analysis>/dispatch_table_full.json with every
slot[1] decoder RVA found across all sub-tables.

Usage: python scripts/scan_dispatch_table.py
"""

from __future__ import annotations

import json
import os
import struct
import sys
from collections import defaultdict
from pathlib import Path


ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
BINARY = ANALYSIS / "league_16-9.exe"
OUT_PATH = ANALYSIS / "dispatch_table_full.json"

ENTRY_SIZE = 48          # 6 u64 fields
SLOT_COUNT = 6
MIN_SUBTABLE_LEN = 5     # consecutive entries with constant slot[0]
IMAGE_BASE = 0x140000000


def parse_pe_sections(binary: bytes) -> dict:
    pe_off = struct.unpack_from("<I", binary, 0x3C)[0]
    n_secs = struct.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs = {}
    for i in range(n_secs):
        b = sec_start + 40 * i
        name = binary[b : b + 8].rstrip(b"\0").decode("latin-1")
        secs[name] = {
            "rva": struct.unpack_from("<I", binary, b + 12)[0],
            "virt_size": struct.unpack_from("<I", binary, b + 8)[0],
            "raw_off": struct.unpack_from("<I", binary, b + 20)[0],
            "raw_size": struct.unpack_from("<I", binary, b + 16)[0],
        }
    return secs


def main() -> int:
    with open(BINARY, "rb") as f:
        binary = f.read()

    secs = parse_pe_sections(binary)
    rdata = secs[".rdata"]
    text = secs[".text"]
    rdata_bytes = binary[rdata["raw_off"] : rdata["raw_off"] + min(rdata["raw_size"], rdata["virt_size"])]
    rdata_rva = rdata["rva"]

    text_va_lo = IMAGE_BASE + text["rva"]
    text_va_hi = IMAGE_BASE + text["rva"] + text["virt_size"]
    print(f".text VA range: 0x{text_va_lo:x}..0x{text_va_hi:x}")
    print(f".rdata size: {len(rdata_bytes)} bytes")

    # Cache: at each 8-byte aligned offset, is the u64 a text VA?
    is_text_va: dict[int, bool] = {}
    n = len(rdata_bytes)
    for off in range(0, n - 7, 8):
        v = struct.unpack_from("<Q", rdata_bytes, off)[0]
        is_text_va[off] = text_va_lo <= v < text_va_hi

    # Scan: at every 8-byte aligned offset that has slot[0] AND slot[1] as
    # text VAs, see how long a constant-slot[0] run extends.
    sub_tables = []
    visited_starts = set()
    for start in range(0, n - ENTRY_SIZE, 8):
        if start in visited_starts:
            continue
        if not is_text_va.get(start) or not is_text_va.get(start + 8):
            continue
        slot0 = struct.unpack_from("<Q", rdata_bytes, start)[0]
        # Walk consecutive entries
        i = start
        run_len = 0
        decoders = []
        while i + ENTRY_SIZE <= n:
            cur_slot0 = struct.unpack_from("<Q", rdata_bytes, i)[0]
            slot1 = struct.unpack_from("<Q", rdata_bytes, i + 8)[0]
            if cur_slot0 != slot0:
                break
            if not (text_va_lo <= slot1 < text_va_hi):
                break
            decoders.append(slot1 - IMAGE_BASE)
            run_len += 1
            i += ENTRY_SIZE
        if run_len >= MIN_SUBTABLE_LEN:
            sub_tables.append({
                "rdata_off": start,
                "rdata_rva": rdata_rva + start,
                "entries": run_len,
                "slot0_va": slot0,
                "slot0_rva": slot0 - IMAGE_BASE,
                "decoders": decoders,
            })
            for k in range(start, start + run_len * ENTRY_SIZE, 8):
                visited_starts.add(k)

    print(f"found {len(sub_tables)} sub-tables of >= {MIN_SUBTABLE_LEN} entries")
    for st in sorted(sub_tables, key=lambda s: -s["entries"])[:5]:
        print(f"  rdata_off=0x{st['rdata_off']:x}  entries={st['entries']:4d}  "
              f"slot0=0x{st['slot0_rva']:x}")

    # Collect every distinct slot[1] decoder RVA across all sub-tables.
    decoder_rvas = set()
    decoder_with_subtables = defaultdict(list)
    for st in sub_tables:
        for rva in st["decoders"]:
            decoder_rvas.add(rva)
            decoder_with_subtables[rva].append(st["rdata_off"])

    print(f"distinct slot[1] decoders: {len(decoder_rvas)}")

    # Sanity check: known fb4070 should be in the table
    if 0xfb4070 in decoder_rvas:
        print("[OK] confirmed: 0xfb4070 (mov decoder) is in slot[1] of dispatch table")
    else:
        print("[!!] 0xfb4070 NOT found in slot[1] -- scanner is missing entries")

    # Output
    out = {
        "image_base": f"0x{IMAGE_BASE:x}",
        "rdata_rva": f"0x{rdata_rva:x}",
        "rdata_size": rdata["virt_size"],
        "sub_tables": [
            {
                "rdata_off": f"0x{st['rdata_off']:x}",
                "rdata_rva": f"0x{st['rdata_rva']:x}",
                "entries": st["entries"],
                "slot0_rva": f"0x{st['slot0_rva']:x}",
                "decoders": [f"0x{r:x}" for r in st["decoders"]],
            }
            for st in sub_tables
        ],
        "decoders": [
            {
                "rva": f"0x{rva:x}",
                "in_subtables": [f"0x{o:x}" for o in decoder_with_subtables[rva]],
            }
            for rva in sorted(decoder_rvas)
        ],
    }
    with open(OUT_PATH, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {OUT_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
