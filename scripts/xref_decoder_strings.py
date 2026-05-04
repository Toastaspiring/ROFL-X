#!/usr/bin/env python
"""For each wired decoder + its netid-specific constructor init, find
LEA instructions that reference printable strings in .rdata/.data.

Goal: pin down decoders to a specific Riot class name when the
binary references a string near the decoder body. Each PKT_*_s name
is dead data (no u64 refs), but classes also use OTHER strings:

  - Game-event names: 'evtCastSpell1', 'MutatedReplication2',
    '[GameEvents]evtCastSpell1', etc.
  - Property names: '_Health', '_Mana', '_AttackSpeed',
    'ManaCost_Ex1', etc.
  - Mangled function names containing class hints
  - Per-class field names: when a decoder writes to a serializer
    that records (field_name, value), the field_name is loaded as
    an LEA target.

Strategy:
  1. Build a set of plausible class-name strings from the binary
     (capitalized, length >= 5, not stdlib noise).
  2. For each decoder + each ctor-init function (one per netid)
     that we know from earlier analysis, walk the body looking for
     `lea reg, [rip+disp32]` where disp32 lands on a tracked string.
  3. Output per-decoder: list of (string, source_function, offset).

This is the cheap path-to-proof: if decoder X's init references
"PKT_NPC_CastSpellAns_s" or "evtCastSpell1", we have a much
stronger link than shape-match alone.
"""

from __future__ import annotations

import json
import os
import re
import struct
import sys
import zipfile
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PATCH_PATH = ROOT / "patch" / "16-9.patch"
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
BINARY_PATH = ANALYSIS / "league_16-9.exe"

IMAGE_BASE = 0x140000000
SCAN_LIMIT = 4000  # max bytes of a decoder/init body to scan
MAX_CALL_DEPTH = 2  # follow callees up to N levels deep
MIN_STRING_LEN = 5
MAX_STRING_LEN = 80


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
            struct.unpack_from("<I", binary, b + 12)[0],  # rva
            struct.unpack_from("<I", binary, b + 8)[0],   # virt_size
            struct.unpack_from("<I", binary, b + 20)[0],  # raw_off
            struct.unpack_from("<I", binary, b + 16)[0],  # raw_size
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


def build_string_index(binary, secs):
    """Find printable C-strings in .rdata and .data with file offset
    + start VA. Filter to ones that look interesting (capitalized,
    length in range, not common stdlib).
    Returns: dict {VA: string}.
    """
    # Match printable ASCII followed by NUL.
    pat = re.compile(rb"([\x20-\x7e]{%d,%d})\x00" % (MIN_STRING_LEN, MAX_STRING_LEN))
    blacklist_substrings = (
        "std::", "Microsoft", "DirectX", "Windows", "kernel32",
        "vfprintf", "msvcrt", "ucrtbase", "ntdll", "VCRUNTIME",
        "ASN1_", "BIO_", "EVP_", "SSL_", "ERR_", "RSA_", "X509",
        "OPENSSL_", "UNKNOWN", "Adieresissmall",
    )
    out = {}
    for sec_name in (".rdata", ".data"):
        rva, vsz, raw_off, raw_sz = secs[sec_name]
        sec_buf = binary[raw_off : raw_off + min(vsz, raw_sz)]
        for m in pat.finditer(sec_buf):
            s = m.group(1).decode("latin-1", errors="replace")
            # Filter
            if any(b in s for b in blacklist_substrings):
                continue
            # Require the first char to be a letter or underscore
            if not (s[0].isalpha() or s[0] == "_" or s[0] in "[("):
                continue
            # Must contain at least one uppercase letter (heuristic for code identifier)
            if not any(c.isupper() for c in s):
                continue
            va = IMAGE_BASE + rva + m.start()
            out[va] = s
    return out


def scan_function_for_string_refs(binary, text_off, text_rva, fn_rva, sizes,
                                   string_index, depth, visited, hits):
    """Walk a function body, collect LEA targets that are string VAs.
    Recurse into call rel32 up to MAX_CALL_DEPTH levels.
    """
    if fn_rva in visited or depth > MAX_CALL_DEPTH:
        return
    visited.add(fn_rva)
    size = sizes.get(fn_rva, SCAN_LIMIT)
    body_off = text_off + (fn_rva - text_rva)
    if body_off < 0 or body_off + size > len(binary):
        return
    body = binary[body_off : body_off + min(size, SCAN_LIMIT)]
    callees = []
    i = 0
    while i + 7 < len(body):
        b0 = body[i]
        # lea r64, [rip+disp32] (REX.W=1, opcode 8d, modrm mod=00 rm=101)
        if b0 in (0x48, 0x4c) and body[i + 1] == 0x8d and (body[i + 2] & 0xc7) == 0x05:
            disp32 = struct.unpack_from("<i", body, i + 3)[0]
            target_va = IMAGE_BASE + fn_rva + i + 7 + disp32
            target_va &= 0xFFFFFFFFFFFFFFFF
            if target_va in string_index:
                hits.append({
                    "fn_rva": f"0x{fn_rva:x}",
                    "instr_offset": i,
                    "target_va": f"0x{target_va:x}",
                    "string": string_index[target_va],
                    "depth": depth,
                })
            i += 7
            continue
        # call rel32 (e8 disp32)
        if b0 == 0xe8 and i + 5 < len(body):
            disp32 = struct.unpack_from("<i", body, i + 1)[0]
            target_rva = (fn_rva + i + 5 + disp32) & 0xffffffff
            if text_rva <= target_rva < text_rva + len(body) + 0x10000000:  # sanity
                callees.append(target_rva)
            i += 5
            continue
        # ret = stop
        if b0 == 0xc3 or b0 == 0xc2:
            if i > 32:
                break
        i += 1
    for callee in callees:
        scan_function_for_string_refs(binary, text_off, text_rva, callee, sizes,
                                       string_index, depth + 1, visited, hits)


def main() -> int:
    with open(BINARY_PATH, "rb") as f:
        binary = f.read()
    secs = parse_pe(binary)
    sizes = load_pdata_sizes(binary, secs)
    text = secs[".text"]
    text_rva = text[0]
    text_off = text[2]
    string_index = build_string_index(binary, secs)
    print(f"loaded {len(string_index)} candidate strings")

    # Load wired decoders
    with zipfile.ZipFile(PATCH_PATH) as z:
        cfg = json.loads(z.read("result.json"))
    netid_to_rva = {e["netid"]: int(e["rva_start"], 16) for e in cfg.get("extra_decoders", [])}
    netid_to_rva[cfg["mov_decrypt"]["netid"]] = int(cfg["mov_decrypt"]["rva_start"], 16)

    # Also load the netid-jumptable to get per-netid ctor blocks
    jt_off = text_off + (0xe93818 - text_rva)
    netid_to_ctor: dict[int, int] = {}
    for n in netid_to_rva.keys():
        if n < 0x4ae:
            netid_to_ctor[n] = struct.unpack_from("<I", binary, jt_off + n * 4)[0]

    # For each unique decoder RVA, scan it + its ctor-init for string refs
    rva_to_netids = defaultdict(list)
    for n, r in netid_to_rva.items():
        rva_to_netids[r].append(n)

    decoder_strings: dict[int, list] = {}
    for decoder_rva, netids in rva_to_netids.items():
        hits = []
        # 1. Scan decoder body itself (and callees one level deep)
        scan_function_for_string_refs(binary, text_off, text_rva, decoder_rva,
                                       sizes, string_index, 0, set(), hits)
        # 2. For each netid this decoder handles, scan the ctor block + its
        #    inner init function (which writes the vtable + class-specific data).
        for n in netids:
            ctor_rva = netid_to_ctor.get(n)
            if ctor_rva is None:
                continue
            # Find init function = second `e8 disp32` in ctor body
            ctor_body = binary[text_off + (ctor_rva - text_rva):
                                text_off + (ctor_rva - text_rva) + 64]
            calls = []
            i = 0
            while i + 5 < len(ctor_body):
                if ctor_body[i] == 0xe8:
                    disp = struct.unpack_from("<i", ctor_body, i + 1)[0]
                    calls.append((ctor_rva + i + 5 + disp) & 0xffffffff)
                    i += 5
                elif ctor_body[i] == 0xc3:
                    break
                else:
                    i += 1
            init_rva = calls[1] if len(calls) >= 2 else None
            ctor_hits = []
            scan_function_for_string_refs(binary, text_off, text_rva, ctor_rva,
                                           sizes, string_index, 0, set(),
                                           ctor_hits)
            if init_rva and text_rva <= init_rva < text_rva + text[1]:
                init_hits = []
                scan_function_for_string_refs(binary, text_off, text_rva,
                                               init_rva, sizes, string_index,
                                               0, set(), init_hits)
                for h in init_hits:
                    h["netid"] = n
                    h["source"] = "init"
                hits.extend(init_hits)
            for h in ctor_hits:
                h["netid"] = n
                h["source"] = "ctor"
            hits.extend(ctor_hits)
        # Deduplicate hits by string per decoder
        unique_strings = {}
        for h in hits:
            s = h["string"]
            if s not in unique_strings:
                unique_strings[s] = h
        decoder_strings[decoder_rva] = sorted(unique_strings.values(),
                                              key=lambda h: h.get("depth", 0))

    # Output
    out = {
        "_comment": "Per-decoder string references found via LEA scan of the decoder body, the netid-specific ctor block, and the inner init function. Each entry is a candidate identifier (game-event name, property name, etc.) loaded by code in or near the decoder.",
        "decoders": {},
    }
    for rva, hits in sorted(decoder_strings.items(), key=lambda x: -len(x[1])):
        if not hits:
            continue
        out["decoders"][f"0x{rva:x}"] = {
            "netids": sorted(rva_to_netids[rva]),
            "string_refs": [
                {
                    "string": h["string"],
                    "from_function": h.get("fn_rva", ""),
                    "depth": h.get("depth", 0),
                    "source": h.get("source", "decoder"),
                }
                for h in hits[:30]
            ],
        }
    out_path = ANALYSIS / "decoder_string_xrefs.json"
    with open(out_path, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {out_path}")
    print(f"decoders with at least 1 string ref: {len(out['decoders'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
