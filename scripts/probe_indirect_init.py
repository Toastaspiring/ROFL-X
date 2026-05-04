#!/usr/bin/env python
"""Probe non-fallback vtables in indirect-init constructors.

Some unmatched netids have their primary "init" function set the
FALLBACK vtable first (0x1964850, slot[1]=0x1dbc10) and then a
SECONDARY LEA to a real vtable. The secondary's slot[1] is the actual
decoder.

Strategy: for each unmatched netid, follow ctor -> init_fn, scan ALL
LEAs into .rdata for vtables, probe each non-fallback vtable's
slot[1] via trace-decoder, wire the one that produces writes.
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

IMAGE_BASE = 0x140000000
JUMPTABLE_RVA = 0xe93818
INIT_SCAN_LIMIT = 400


def parse_pe(binary):
    pe_off = struct.unpack_from("<I", binary, 0x3C)[0]
    n_secs = struct.unpack_from("<H", binary, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", binary, pe_off + 20)[0]
    sec_start = pe_off + 24 + opt_size
    secs = {}
    for i in range(n_secs):
        b = sec_start + 40 * i
        name = binary[b : b + 8].rstrip(b"\0").decode("latin-1")
        secs[name] = (
            struct.unpack_from("<I", binary, b + 12)[0],
            struct.unpack_from("<I", binary, b + 8)[0],
            struct.unpack_from("<I", binary, b + 20)[0],
            struct.unpack_from("<I", binary, b + 16)[0],
        )
    return secs


def load_pdata_sizes(binary, secs):
    pd = secs[".pdata"]
    pdata = binary[pd[2] : pd[2] + min(pd[3], pd[1])]
    sizes = {}
    for i in range(0, len(pdata) - 12 + 1, 12):
        b, e, _ = struct.unpack_from("<III", pdata, i)
        if b > 0 and e > b:
            sizes[b] = e - b
    return sizes


def trace_one(replay, netid, rva_start, rva_end, samples=3):
    cmd = [
        str(ROFL_X), "trace-decoder",
        "--replay", str(replay), "--netid", str(netid),
        "--patch-dir", str(ROOT / "patch"),
        "--rva-start", f"0x{rva_start:x}", "--rva-end", f"0x{rva_end:x}",
        "--samples", str(samples),
    ]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        return -1
    out = r.stdout + r.stderr
    m = re.search(r"\((\d+) distinct offsets across", out)
    return int(m.group(1)) if m else 0


def find_inner_init(binary, secs, ctor_rva):
    """Find the second `call rel32` in the ctor block (first is alloc)."""
    text_off = secs[".text"][2]
    text_rva = secs[".text"][0]
    body = binary[text_off + (ctor_rva - text_rva) : text_off + (ctor_rva - text_rva) + 64]
    calls = []
    i = 0
    while i < 60:
        if body[i] == 0xe8:
            disp32 = struct.unpack_from("<i", body, i + 1)[0]
            target = (ctor_rva + i + 5 + disp32) & 0xffffffff
            calls.append(target)
            i += 5
        elif body[i] == 0xc3:
            break
        else:
            i += 1
    if len(calls) < 2:
        return None
    # First is alloc (~0x1123c60); init is the second call
    return calls[1]


def find_all_vtable_hits(binary, secs, init_rva):
    """Walk init function body, find LEAs into .rdata, return list of
    (lea_offset, target_rva, slot1_rva) hits where slot1 is a plausible
    .text VA (not the fallback).
    """
    rdata = secs[".rdata"]
    text = secs[".text"]
    rdata_va_lo = IMAGE_BASE + rdata[0]
    rdata_va_hi = rdata_va_lo + rdata[1]
    text_va_lo = IMAGE_BASE + text[0]
    text_va_hi = text_va_lo + text[1]
    body_off = text[2] + (init_rva - text[0])
    if body_off + INIT_SCAN_LIMIT > len(binary):
        return []
    body = binary[body_off : body_off + INIT_SCAN_LIMIT]
    hits = []
    i = 0
    while i + 7 < len(body):
        if body[i] in (0x48, 0x4c) and body[i + 1] == 0x8d and (body[i + 2] & 0xc7) == 0x05:
            disp32 = struct.unpack_from("<i", body, i + 3)[0]
            target_rva = (init_rva + i + 7 + disp32) & 0xffffffff
            target_va = IMAGE_BASE + target_rva
            if rdata_va_lo <= target_va < rdata_va_hi:
                vt_off = rdata[2] + (target_rva - rdata[0])
                if vt_off + 16 < len(binary):
                    slot1 = struct.unpack_from("<Q", binary, vt_off + 8)[0]
                    if text_va_lo <= slot1 < text_va_hi:
                        slot1_rva = slot1 - IMAGE_BASE
                        # skip fallbacks
                        if slot1_rva not in (0x1dbc10, 0x1dc000):
                            hits.append((i, target_rva, slot1_rva))
        i += 1
    return hits


def main():
    with open(BINARY_PATH, "rb") as f:
        binary = f.read()
    secs = parse_pe(binary)
    sizes = load_pdata_sizes(binary, secs)

    text = secs[".text"]
    jt_off = text[2] + (JUMPTABLE_RVA - text[0])

    # Load uncovered list from current patch
    with zipfile.ZipFile(PATCH_PATH) as z:
        cfg = json.loads(z.read("result.json"))
    covered = {e["netid"] for e in cfg.get("extra_decoders", [])}
    covered.add(cfg["mov_decrypt"]["netid"])

    # Replay
    rofls = sorted(REPLAYS.glob("*.rofl"), key=lambda p: p.stat().st_size, reverse=True)
    replay = rofls[0]

    # For each potentially-decodable unmatched netid, find non-fallback vtables
    # and probe their slot[1] decoder.
    confirmed = []
    target_netids = sorted(set(range(1198)) - covered)
    print(f"checking {len(target_netids)} uncovered netids for non-fallback vtables")
    for netid in target_netids:
        ctor_rva = struct.unpack_from("<I", binary, jt_off + netid * 4)[0]
        if not (text[0] <= ctor_rva < text[0] + text[1]):
            continue
        init_rva = find_inner_init(binary, secs, ctor_rva)
        if init_rva is None or not (text[0] <= init_rva < text[0] + text[1]):
            continue
        hits = find_all_vtable_hits(binary, secs, init_rva)
        if not hits:
            continue
        # Try each non-fallback slot[1] until one gives writes
        for lea_off, vt_rva, slot1_rva in hits:
            if slot1_rva not in sizes:
                continue
            n = trace_one(replay, netid, slot1_rva, slot1_rva + sizes[slot1_rva], samples=3)
            if n > 0:
                print(f"  netid {netid}: vtable=0x{vt_rva:x} slot[1]=0x{slot1_rva:x} -> {n} offsets [match]", flush=True)
                confirmed.append((netid, slot1_rva, slot1_rva + sizes[slot1_rva], sizes[slot1_rva], n))
                break

    print(f"\nfound {len(confirmed)} matches via indirect-init scan")

    # Wire
    if confirmed:
        extras = list(cfg.get("extra_decoders", []))
        for netid, rva, rva_end, size, writes in confirmed:
            extras.append({
                "name": f"netid_{netid}",
                "netid": netid,
                "rva_start": f"0x{rva:x}",
                "rva_end": f"0x{rva_end:x}",
                "struct_size": "0x90",
                "semantic_hint": f"indirect-init mapped ({writes} writes via secondary vtable)",
            })
        cfg["extra_decoders"] = extras
        with zipfile.ZipFile(PATCH_PATH) as z:
            files = {n: z.read(n) for n in z.namelist()}
        files["result.json"] = json.dumps(cfg, indent=2).encode()
        with zipfile.ZipFile(PATCH_PATH, "w", zipfile.ZIP_DEFLATED) as z:
            for n, data in files.items():
                z.writestr(n, data)
        print(f"wrote {len(extras)} total extras to {PATCH_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
