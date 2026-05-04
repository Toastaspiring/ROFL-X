#!/usr/bin/env python
"""Resolve every netid to its decoder RVA via the jump-table dispatcher.

Discovery:
  - Function 0xe83930 is the netid dispatcher. Its prologue is:
      if (netid < 0x4ae) {
          jump_target = imageBase + table[netid * 4]
          jmp jump_target
      }
    where the table is at .text RVA 0xe93818 (1198 u32 entries).

  - Each table[netid] points to a "constructor" function that allocates
    a class instance and writes a vtable pointer at offset 0. The vtable
    is one of the 538 dispatch sub-tables we identified, and slot[1] of
    that vtable is the per-class decoder.

Strategy:
  For each netid in [0..0x4ae):
    1. Read jump_table_rva = 0xe93818
    2. ctor_rva = jump_table[netid]
    3. Disassemble the ctor body. Look for either:
       - `lea reg, [rip+disp32]` with target = a known sub-table start VA
       - `mov [reg], imm64` where imm64 = a known sub-table start VA
       - `mov reg, [rip+disp32]` loading a vtable pointer from .rdata
    4. The vtable's slot[1] (offset +8) is the decoder.
  Output: ~/Tools/analysis/16-9/netid_to_decoder.json
"""

from __future__ import annotations

import json
import os
import struct
import sys
from pathlib import Path


ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
BINARY = ANALYSIS / "league_16-9.exe"
DISPATCH_PATH = ANALYSIS / "dispatch_table_full.json"
OUT_PATH = ANALYSIS / "netid_to_decoder.json"

IMAGE_BASE = 0x140000000
JUMPTABLE_RVA = 0xe93818
NUM_NETIDS = 0x4ae  # 1198, per dispatcher's `if (netid < 0x4ae)` check

# Scan up to this many bytes from the constructor entry point looking
# for the vtable-load instruction. Most ctors are small (~50 bytes);
# a generous limit catches longer ones too.
CTOR_SCAN_LIMIT = 800
# Many ctors don't load the vtable directly — they delegate via
# `call subctor` to a 2nd-level "init" that does the vtable assign.
# Allow up to this many transitive call hops to find the LEA.
MAX_CALL_DEPTH = 3


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
    text = secs[".text"]
    rdata = secs[".rdata"]
    text_bytes = binary[text["raw_off"] : text["raw_off"] + min(text["raw_size"], text["virt_size"])]
    text_rva = text["rva"]

    rdata_off = rdata["raw_off"]
    rdata_rva = rdata["rva"]
    rdata_bytes = binary[rdata_off : rdata_off + min(rdata["raw_size"], rdata["virt_size"])]

    with open(DISPATCH_PATH) as f:
        dispatch = json.load(f)

    # Build set of valid sub-table start VAs (these are vtable pointers)
    sub_table_va_to_decoder = {}
    sub_table_intervals = []
    for st in dispatch["sub_tables"]:
        sub_off = int(st["rdata_off"], 16)
        sub_va = IMAGE_BASE + rdata_rva + sub_off
        # slot[1] is the decoder = first decoder in the entries list
        decoder_rva = int(st["decoders"][0], 16) if st["decoders"] else None
        sub_table_va_to_decoder[sub_va] = decoder_rva
        sub_table_intervals.append((sub_va, sub_va + 48 * st["entries"], decoder_rva, sub_off))
    sub_table_intervals.sort()

    def find_subtable_at(va: int) -> tuple[int, int] | None:
        """Return (decoder_rva_for_this_offset, sub_off) or None."""
        lo, hi = 0, len(sub_table_intervals)
        while lo < hi:
            mid = (lo + hi) // 2
            if sub_table_intervals[mid][0] <= va:
                lo = mid + 1
            else:
                hi = mid
        if lo == 0:
            return None
        s, e, _, off = sub_table_intervals[lo - 1]
        if s <= va < e:
            # va might be at slot[0], slot[1], etc. We need the slot[1]
            # decoder of the entry containing va.
            entry_off = (va - s) // 48
            entry_start_rdata_off = off + entry_off * 48
            slot1 = struct.unpack_from("<Q", rdata_bytes, entry_start_rdata_off + 8)[0]
            slot1_rva = slot1 - IMAGE_BASE
            return (slot1_rva, off)
        return None

    # Read jump table
    jt_off = JUMPTABLE_RVA - text_rva  # text raw_off equals text rva for League
    jt_off = text["raw_off"] + JUMPTABLE_RVA - text_rva
    print(f"jump table at file offset 0x{jt_off:x}, {NUM_NETIDS} entries")

    jumptable = []
    for i in range(NUM_NETIDS):
        rva = struct.unpack_from("<I", binary, jt_off + i * 4)[0]
        jumptable.append(rva)

    def collect_lea_hits(start_rva: int, depth: int, visited: set, hits: list) -> None:
        """Collect ALL LEA targets pointing into sub-tables, in execution
        order (linear walk; recurse into call rel32 as encountered).
        C++ ctors set the vtable LAST (most-derived class wins), so we
        keep all hits and pick the LAST one as the final vtable.
        """
        if start_rva in visited or depth > MAX_CALL_DEPTH:
            return
        if not (text_rva <= start_rva < text_rva + text["virt_size"]):
            return
        visited.add(start_rva)
        body_off = text["raw_off"] + start_rva - text_rva
        body = binary[body_off : body_off + CTOR_SCAN_LIMIT]

        i = 0
        while i + 7 < len(body):
            b0 = body[i]
            if b0 in (0x48, 0x4c) and body[i + 1] == 0x8d and (body[i + 2] & 0xc7) == 0x05:
                disp32 = struct.unpack_from("<i", body, i + 3)[0]
                pc_after = start_rva + i + 7
                target_rva = pc_after + disp32
                target_va = IMAGE_BASE + target_rva
                if target_va in sub_table_va_to_decoder:
                    hits.append((
                        sub_table_va_to_decoder[target_va],
                        target_va,
                        f"lea start fn=0x{start_rva:x} +0x{i:x}",
                    ))
                else:
                    hit = find_subtable_at(target_va)
                    if hit and hit[0]:
                        hits.append((
                            hit[0],
                            target_va,
                            f"lea inside-sub fn=0x{start_rva:x} +0x{i:x} sub_off=0x{hit[1]:x}",
                        ))
                i += 7
                continue
            if b0 == 0xe8 and i + 5 < len(body):
                disp32 = struct.unpack_from("<i", body, i + 1)[0]
                pc_after = start_rva + i + 5
                target_rva = pc_after + disp32
                if text_rva <= target_rva < text_rva + text["virt_size"]:
                    # Recurse INLINE so the resulting hit ordering matches
                    # execution order. C++ derived ctor calls base ctor
                    # FIRST, so base-class vtable hits land before derived.
                    collect_lea_hits(target_rva, depth + 1, visited, hits)
                i += 5
                continue
            if b0 == 0xc3 or b0 == 0xc2:
                if i > 32:
                    break
            i += 1

    netid_to_decoder: dict[int, dict] = {}
    for netid in range(NUM_NETIDS):
        ctor_rva = jumptable[netid]
        if not (text_rva <= ctor_rva < text_rva + text["virt_size"]):
            continue
        hits: list[tuple[int, int, str]] = []
        collect_lea_hits(ctor_rva, 0, set(), hits)
        if not hits:
            continue
        # Pick the LAST hit — derived-most vtable assignment
        decoder_rva, vtable_va, source = hits[-1]
        netid_to_decoder[netid] = {
            "ctor_rva": f"0x{ctor_rva:x}",
            "vtable_va": f"0x{vtable_va:x}",
            "decoder_rva": f"0x{decoder_rva:x}",
            "source": source,
            "all_hits_count": len(hits),
        }
    found_count = len(netid_to_decoder)

    print(f"resolved decoder for {found_count} / {NUM_NETIDS} netids")

    out = {
        "jumptable_rva": f"0x{JUMPTABLE_RVA:x}",
        "num_netids": NUM_NETIDS,
        "resolved": {str(n): info for n, info in netid_to_decoder.items()},
    }
    with open(OUT_PATH, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {OUT_PATH}")

    # Sanity check: 916 should map to fb4070
    if 916 in netid_to_decoder:
        info = netid_to_decoder[916]
        print(f"netid 916 -> {info['decoder_rva']} (expected 0xfb4070)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
