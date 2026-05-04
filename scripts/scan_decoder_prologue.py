#!/usr/bin/env python
"""Find functions in 16.9 .text matching the 5-5 decoder prologue shape.

Donor: 5-5's `mov_decrypt` at RVA 0xe45710 and `ward_spawn_decrypt` at
RVA 0xe3d7b0 share the same 28-byte stereotyped prologue:

    48895c2408 48896c2410 4889742418 57 4154 4155 4156 4157 4883ec20 49

(MSVC: save rbx/rbp/rsi to shadow space, push rdi/r12-r15, sub rsp, 0x20)

This is the "decoder shape" because Riot's per-class decoders all take
3 args (RCX=output, RDX=payload, R8=end) and need 4 callee-saved regs
to do the bit-reading work. Many other functions also match this
prologue, so this filter alone is not sufficient — but combined with
the dispatch-table presence it narrows to ~41 high-confidence decoders.

Usage: python scripts/scan_decoder_prologue.py
"""

from __future__ import annotations

import json
import os
import struct
import sys
from pathlib import Path


ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
BINARY = ANALYSIS / "league_16-9.exe"
OUT_PATH = ANALYSIS / "decoder_prologue_candidates.json"

# 28-byte donor prologue from 5-5 mov_decrypt and ward_spawn_decrypt.
# (Differs only at byte 28: 8bf0 vs 8be8 — first instruction after the
# common prologue.)
PROLOGUE = bytes.fromhex(
    "48895c2408"          # mov [rsp+8], rbx
    "48896c2410"          # mov [rsp+0x10], rbp
    "4889742418"          # mov [rsp+0x18], rsi
    "57"                  # push rdi
    "4154 4155 4156 4157" # push r12, r13, r14, r15
    "4883ec20"            # sub rsp, 0x20
    .replace(" ", "")
)
assert len(PROLOGUE) == 28, len(PROLOGUE)


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


def load_pdata_sizes(binary: bytes) -> dict[int, int]:
    secs = parse_pe_sections(binary)
    pd = secs[".pdata"]
    pdata_bytes = binary[pd["raw_off"] : pd["raw_off"] + min(pd["raw_size"], pd["virt_size"])]
    sizes = {}
    for i in range(0, len(pdata_bytes) - 12 + 1, 12):
        b, e, _ = struct.unpack_from("<III", pdata_bytes, i)
        if b > 0 and e > b:
            sizes[b] = e - b
    return sizes


def main() -> int:
    with open(BINARY, "rb") as f:
        binary = f.read()

    secs = parse_pe_sections(binary)
    text = secs[".text"]
    text_bytes = binary[text["raw_off"] : text["raw_off"] + min(text["raw_size"], text["virt_size"])]
    text_rva = text["rva"]

    sizes = load_pdata_sizes(binary)

    # Find every occurrence of the prologue pattern in .text
    print(f"scanning .text ({len(text_bytes)} bytes) for 28-byte donor prologue")
    prologue_hits = []
    pos = 0
    while True:
        p = text_bytes.find(PROLOGUE, pos)
        if p < 0:
            break
        rva = text_rva + p
        # Cross-check: this RVA must be a function start per .pdata
        if rva in sizes:
            prologue_hits.append({
                "rva": rva,
                "size": sizes[rva],
            })
        pos = p + 1

    print(f"found {len(prologue_hits)} functions matching the donor prologue")
    out = {
        "donor_prologue_hex": PROLOGUE.hex(),
        "candidates": [
            {
                "rva": f"0x{c['rva']:x}",
                "rva_end": f"0x{c['rva'] + c['size']:x}",
                "size_bytes": c["size"],
            }
            for c in sorted(prologue_hits, key=lambda c: c["rva"])
        ],
    }
    with open(OUT_PATH, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {OUT_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
