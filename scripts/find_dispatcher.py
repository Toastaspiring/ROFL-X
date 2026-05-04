#!/usr/bin/env python
"""Find the netid dispatcher function by scanning .text for LEAs to
dispatch sub-table VAs.

The packet dispatcher is the function that takes a netid (u16/u32) and
indexes into a dispatch sub-table to call the per-class decoder. It
must reference one or more of the 538 sub-tables we've located in
`dispatch_table_full.json` via `lea reg, [rip+disp32]`.

Strategy:
  1. Load all sub-table VAs.
  2. Walk every byte of .text. At positions that start with the LEA
     prefix `48 8d` or `4c 8d`, read the 32-bit displacement and
     compute the target VA.
  3. If target VA is a known sub-table VA, record (lea_rva, target_off).
  4. Group by enclosing function (using .pdata's function ranges).
  5. Functions with many sub-table refs are the dispatcher.

This is a heuristic — the table may also be referenced via vtable
slots or indirect computations. But for the main dispatcher it should
work.

Output: ~/Tools/analysis/16-9/dispatcher_candidates.json
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
DISPATCH_PATH = ANALYSIS / "dispatch_table_full.json"
OUT_PATH = ANALYSIS / "dispatcher_candidates.json"
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


def load_pdata_functions(binary: bytes) -> list[tuple[int, int]]:
    """Returns sorted list of (start_rva, end_rva) per .pdata."""
    secs = parse_pe_sections(binary)
    pd = secs[".pdata"]
    pdata_bytes = binary[pd["raw_off"] : pd["raw_off"] + min(pd["raw_size"], pd["virt_size"])]
    fns = []
    for i in range(0, len(pdata_bytes) - 12 + 1, 12):
        b, e, _ = struct.unpack_from("<III", pdata_bytes, i)
        if b > 0 and e > b:
            fns.append((b, e))
    fns.sort()
    return fns


def find_function(fns: list[tuple[int, int]], rva: int) -> tuple[int, int] | None:
    # Binary search
    lo, hi = 0, len(fns)
    while lo < hi:
        mid = (lo + hi) // 2
        if fns[mid][0] <= rva:
            lo = mid + 1
        else:
            hi = mid
    if lo == 0:
        return None
    s, e = fns[lo - 1]
    if s <= rva < e:
        return (s, e)
    return None


def main() -> int:
    with open(BINARY, "rb") as f:
        binary = f.read()
    secs = parse_pe_sections(binary)
    text = secs[".text"]
    text_bytes = binary[text["raw_off"] : text["raw_off"] + min(text["raw_size"], text["virt_size"])]
    text_rva = text["rva"]

    with open(DISPATCH_PATH) as f:
        dispatch = json.load(f)

    # Build set of valid sub-table VAs (start of each sub-table) + their RVAs
    rdata_rva = int(dispatch["rdata_rva"], 16)
    sub_table_va_to_off = {}
    for st in dispatch["sub_tables"]:
        sub_off = int(st["rdata_off"], 16)
        sub_va = IMAGE_BASE + rdata_rva + sub_off
        sub_table_va_to_off[sub_va] = sub_off
    sub_table_vas = set(sub_table_va_to_off.keys())

    # Also accept VAs anywhere INSIDE a sub-table (e.g., the mid-sub-table
    # entry could be the LEA target). Build an interval lookup.
    sub_table_intervals = []
    for st in dispatch["sub_tables"]:
        sub_off = int(st["rdata_off"], 16)
        sub_va = IMAGE_BASE + rdata_rva + sub_off
        sub_table_intervals.append((sub_va, sub_va + 48 * st["entries"], sub_off))
    sub_table_intervals.sort()

    def find_subtable(target_va: int) -> int | None:
        lo, hi = 0, len(sub_table_intervals)
        while lo < hi:
            mid = (lo + hi) // 2
            if sub_table_intervals[mid][0] <= target_va:
                lo = mid + 1
            else:
                hi = mid
        if lo == 0:
            return None
        s, e, off = sub_table_intervals[lo - 1]
        if s <= target_va < e:
            return off
        return None

    fns = load_pdata_functions(binary)
    print(f".text size: {len(text_bytes)}, scanning for LEAs to {len(sub_table_vas)} sub-tables")
    print(f"functions: {len(fns)}")

    # Walk text. At each position, check if it starts with a LEA REX
    # prefix that gives RIP-relative disp32:
    #   48 8d XX disp32     for LEA r64, [rip+disp32]   (REX.W=1, 7 bytes, modrm XX byte 0)
    #   4c 8d XX disp32     for LEA r64, [rip+disp32]   (with R8-R15 dest, 7 bytes)
    # The modrm byte XX must have mod=00, rm=101 to indicate RIP-relative.
    # That is XX & 0xc7 == 0x05.
    func_hits = defaultdict(list)
    n = len(text_bytes)
    i = 0
    while i + 7 < n:
        b0 = text_bytes[i]
        if b0 in (0x48, 0x4c):
            b1 = text_bytes[i + 1]
            if b1 == 0x8d:
                modrm = text_bytes[i + 2]
                if modrm & 0xc7 == 0x05:
                    disp32 = struct.unpack_from("<i", text_bytes, i + 3)[0]
                    pc_after = text_rva + i + 7
                    target_rva = pc_after + disp32
                    target_va = IMAGE_BASE + target_rva
                    sub_off = find_subtable(target_va)
                    if sub_off is not None:
                        lea_rva = text_rva + i
                        fn = find_function(fns, lea_rva)
                        if fn:
                            func_hits[fn[0]].append({
                                "lea_rva": f"0x{lea_rva:x}",
                                "target_subtable_off": f"0x{sub_off:x}",
                                "is_subtable_start": (target_va in sub_table_vas),
                            })
                        i += 7
                        continue
        i += 1

    # Sort by hit count and number of distinct sub-tables targeted
    ranked = []
    for fn_start, hits in func_hits.items():
        distinct_subs = len(set(h["target_subtable_off"] for h in hits))
        ranked.append((distinct_subs, len(hits), fn_start, hits))
    ranked.sort(reverse=True)

    print(f"functions referencing >= 1 sub-table: {len(ranked)}")
    print(f"{'rva':>10}  {'distinct_subs':>14}  {'total_leas':>10}  {'fn_size':>10}")
    for distinct, total, rva, hits in ranked[:20]:
        fn = find_function(fns, rva)
        size = (fn[1] - fn[0]) if fn else 0
        print(f"  0x{rva:8x}  {distinct:>14}  {total:>10}  {size:>10}")

    out = {
        "candidates": [
            {
                "fn_rva": f"0x{rva:x}",
                "fn_size": (find_function(fns, rva)[1] - find_function(fns, rva)[0])
                    if find_function(fns, rva) else 0,
                "distinct_subtables": distinct,
                "total_leas": total,
                "hits": hits[:10],  # cap for readability
            }
            for distinct, total, rva, hits in ranked[:50]
        ],
    }
    with open(OUT_PATH, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {OUT_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
