#!/usr/bin/env python
"""Build a format-2 `.patch` archive from an unpacked League of Legends binary.

A format-2 archive lets `rofl-x` decode every packet class through the
game's own code, without per-class reverse engineering:

  netid --(factory jump table)--> packet object with its real vtable
        --(vtable slot 1)-------> that class's Deserialize(this, &cursor, end)

Everything the emulator needs is discovered here from the binary alone:

  - factory      : the netid dispatcher. A function that range-checks the
                   netid, then jumps through a u32 table into one
                   constructor block per class (`mov ecx, SIZE; call new;
                   ... call Ctor`).
  - alloc / free : the global operator new (most common call target in the
                   constructor blocks) and its operator delete / free, both
                   replaced by a bump allocator / no-op in the emulator.
  - base_deserialize : the base-class Deserialize every class calls first.
                   It reads the sender net id varint. Replay blocks carry
                   that net id in the block header (`param`) instead of the
                   payload, so the decoder re-encodes it in front of the
                   payload. Discovered by constructing objects through the
                   factory and reading the first call of their Deserialize.
  - team_registry: a global the enum readers range-check against (count at
                   +0x08, ready flag at +0x70). Built at session start; the
                   emulator substitutes an object with the same count.
  - import_slots : `call/jmp [rip+X]` slots whose on-disk value is not an
                   in-image pointer (Riot's stub resolves imports at
                   runtime); the emulator points them at `xor eax,eax; ret`.
  - tls, cfg_dispatch : thread-local template (thread-safe statics read
                   `_Init_thread_epoch` through gs:[0x58]) and the Control
                   Flow Guard dispatch thunk.
  - packets      : netid -> Riot class name, from the std::function handler
                   registrations (`MakeFunction<Client, ..., PKT_X_s>` lambda
                   type descriptors + `mov r8d, <netid>; call
                   RegisterHandler`).

The binary must be unpacked: from patch 16.10-ish onwards the shipped
`League of Legends.exe` keeps its code sections encrypted on disk (new
`.riot0/.riot1/.riot2` sections), and this script refuses those. See
docs/COMPATIBILITY.md.

Requires: pip install pefile capstone numpy unicorn

Usage:
    python scripts/build_patch.py --binary <League of Legends.exe> --output patch/16-9.patch
    python scripts/build_patch.py --binary <exe> --output <archive> --replay <same-patch .rofl>
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import math
import re
import struct
import sys
import zipfile
from pathlib import Path

import numpy as np
import pefile
from capstone import CS_ARCH_X86, CS_MODE_64, Cs
from unicorn import UC_HOOK_CODE, UC_PROT_ALL, Uc, UcError, UC_ARCH_X86, UC_MODE_64
from unicorn.x86_const import (
    UC_X86_REG_GS_BASE, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RAX,
    UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_RIP, UC_X86_REG_RSP,
)

FORMAT = 2
SKIP_SECTIONS = {".rsrc", ".reloc"}
MD = Cs(CS_ARCH_X86, CS_MODE_64)

# Registry check inside the enum readers:
#   test rax, rax ; je ; cmp byte ptr [rax+0x70], 0 ; je ; cmp <r8>, byte ptr [rax+8] ; jae
REGISTRY_CHECK = re.compile(rb"\x48\x85\xc0\x74.\x80\x78\x70\x00\x74.(?:\x3a|\x44\x3a)[\x40-\x7f]\x08\x73", re.S)
LAMBDA_TD = re.compile(
    rb"\.\?AV<lambda_\d+>@\?\d\?\?\?\$MakeFunction@V(\w+?)@@V1@_NAEBV(PKT_\w+?_s)@@@Riot@@[\x21-\x7e]*?\x00"
)
LEA_RIP = re.compile(rb"[\x48\x4c]\x8d[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]")
# mov eax, dword ptr [r14 + rsi*4 + 0xe93818]
JUMP_TABLE_LOAD = re.compile(r"^\w+, dword ptr \[\w+ \+ \w+\*4 \+ (0x[0-9a-f]+)\]$")


# --------------------------------------------------------------------------
# PE helpers
# --------------------------------------------------------------------------

class Image:
    def __init__(self, path: Path):
        self.raw = path.read_bytes()
        pe = pefile.PE(data=self.raw, fast_load=True)
        pe.parse_data_directories(directories=[
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_TLS"],
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_LOAD_CONFIG"],
        ])
        self.pe = pe
        self.base = pe.OPTIONAL_HEADER.ImageBase
        self.size = pe.OPTIONAL_HEADER.SizeOfImage
        self.sections: list[tuple[str, int, int, bytes]] = []
        for s in pe.sections:
            name = s.Name.rstrip(b"\0").decode("latin-1")
            data = s.get_data()
            vsize = max(s.Misc_VirtualSize, len(data))
            self.sections.append((name, s.VirtualAddress, vsize, data[:vsize]))
        text = self.section(".text")
        self.text_rva, self.text = text[1], text[3]
        self._funcs = self._load_pdata()

    def section(self, name: str):
        return next(s for s in self.sections if s[0] == name)

    def read(self, rva: int, n: int) -> bytes:
        for _, va, vsize, data in self.sections:
            if va <= rva < va + vsize:
                chunk = data[rva - va: rva - va + n]
                return chunk + b"\0" * (n - len(chunk))
        raise KeyError(hex(rva))

    def q(self, rva: int) -> int:
        return struct.unpack("<Q", self.read(rva, 8))[0]

    def in_image(self, va: int) -> bool:
        return self.base <= va < self.base + self.size

    def _load_pdata(self):
        try:
            _, va, vsize, data = self.section(".pdata")
        except StopIteration:
            return [], []
        starts, ends = [], []
        for i in range(0, len(data) - 12, 12):
            b, e, _ = struct.unpack_from("<III", data, i)
            if b == 0:
                break
            starts.append(b)
            ends.append(e)
        order = np.argsort(starts)
        return [starts[i] for i in order], [ends[i] for i in order]

    def function_of(self, rva: int):
        starts, ends = self._funcs
        i = int(np.searchsorted(starts, rva, side="right")) - 1
        if i >= 0 and rva < ends[i]:
            return starts[i], ends[i]
        return None

    def disasm(self, rva: int, n: int):
        return MD.disasm(self.read(rva, n), self.base + rva)


def hx(v: int) -> str:
    return f"0x{v:x}"


def file_version(img: Image) -> str | None:
    """VS_VERSIONINFO FileVersion, e.g. "16.9.771.8383"."""
    try:
        img.pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_RESOURCE"]])
        for info in getattr(img.pe, "FileInfo", None) or []:
            for entry in info:
                for table in getattr(entry, "StringTable", []):
                    v = table.entries.get(b"FileVersion")
                    if v:
                        return v.decode()
    except pefile.PEFormatError:
        pass
    return None


def text_entropy(text: bytes) -> float:
    counts = collections.Counter(text[: 4 << 20])
    total = sum(counts.values())
    return -sum(c / total * math.log2(c / total) for c in counts.values())


class RipRefs:
    """Every 4-byte window of .text read as a rip-relative disp32."""

    def __init__(self, img: Image):
        d = np.frombuffer(img.text, dtype=np.uint8).astype(np.int64)
        n = len(d) - 4
        disp = d[0:n] | (d[1:n + 1] << 8) | (d[2:n + 2] << 16) | (d[3:n + 3] << 24)
        disp = np.where(disp >= 2**31, disp - 2**32, disp)
        self.after_disp = img.text_rva + np.arange(n, dtype=np.int64) + 4 + disp
        self.text_rva = img.text_rva

    def to(self, target_rva: int, tail: int = 0) -> list[int]:
        """Text RVAs of disp32 fields that resolve to `target_rva` for an
        instruction ending `tail` bytes after the displacement."""
        idx = np.nonzero(self.after_disp + tail == target_rva)[0]
        return [self.text_rva + int(i) for i in idx]


def direct_calls(img: Image) -> dict[int, list[int]]:
    t = np.frombuffer(img.text, dtype=np.uint8)
    out: dict[int, list[int]] = collections.defaultdict(list)
    for i in np.nonzero(t[:-5] == 0xE8)[0]:
        disp = struct.unpack_from("<i", img.text, int(i) + 1)[0]
        out[img.text_rva + int(i) + 5 + disp].append(img.text_rva + int(i))
    return out


# --------------------------------------------------------------------------
# Discovery
# --------------------------------------------------------------------------

def find_factory(img: Image, refs: RipRefs):
    """The netid dispatcher: `cmp reg, MAX; ja default; lea base, [rip-image];
    mov eax, [base + reg*4 + table]; add rax, base; jmp rax`."""
    best = None
    for disp_at in refs.to(0):                      # lea rX, [rip - rva] == image base
        lea = disp_at - 3
        if not LEA_RIP.fullmatch(img.read(lea, 3)):
            continue
        table = jmp = None
        for ins in img.disasm(lea, 24):
            m = JUMP_TABLE_LOAD.match(ins.op_str) if ins.mnemonic == "mov" else None
            if m:
                table = int(m.group(1), 16)
            if ins.mnemonic == "jmp":
                jmp = ins.address - img.base
                break
        if table is None or jmp is None:
            continue
        func = img.function_of(lea)
        if not func:
            continue
        bound = None
        for ins in img.disasm(func[0], lea - func[0]):
            if ins.mnemonic == "cmp" and ins.op_str.split(", ")[-1].startswith("0x"):
                bound = int(ins.op_str.split(", ")[-1], 16)
        if bound is None or bound > 0x4000:
            continue
        cases = [struct.unpack("<i", img.read(table + 4 * i, 4))[0] for i in range(bound + 1)]
        text_end = img.text_rva + len(img.text)
        if not all(img.text_rva <= c < text_end for c in cases):
            continue
        # A packet factory's cases allocate: `mov ecx, SIZE ; call new`.
        allocating = sum(img.read(c, 1) == b"\xb9" and img.read(c + 5, 1) == b"\xe8" for c in set(cases))
        score = (allocating, bound)
        if best is None or score > best["score"]:
            best = dict(rva=func[0], table=table, max_netid=bound + 1, cases=cases, score=score)
    if best is None:
        raise SystemExit("factory not found (no image-base jump table dispatcher)")
    return best


def find_alloc(img: Image, factory) -> int:
    """Most common `mov ecx, imm; call X` target in the factory's case blocks."""
    targets = collections.Counter()
    for case in set(factory["cases"]):
        prev = None
        for ins in img.disasm(case, 32):
            if ins.mnemonic == "call" and prev and prev.mnemonic == "mov" and prev.op_str.startswith("ecx, "):
                targets[int(ins.op_str, 16) - img.base] += 1
                break
            if ins.mnemonic in ("jmp", "ret"):
                break
            prev = ins
    return targets.most_common(1)[0][0]


def find_free(img: Image, alloc: int) -> list[int]:
    """Operator delete sits right after operator new (`jmp free`)."""
    out = []
    end = alloc
    for ins in img.disasm(alloc, 0x40):
        end = ins.address - img.base + ins.size
        if ins.mnemonic == "jmp":
            break
    nxt = (end + 15) & ~15
    head = img.read(nxt, 5)
    if head[0] == 0xE9:
        out.append(nxt)
        out.append(nxt + 5 + struct.unpack("<i", head[1:])[0])
    return out


def find_import_slots(img: Image) -> list[int]:
    slots = set()
    for m in re.finditer(rb"(?:\xff\x15|\xff\x25|\x48\xff\x25)(....)", img.text, re.S):
        slot = img.text_rva + m.end() + struct.unpack("<i", m.group(1))[0]
        try:
            v = img.q(slot)
        except KeyError:
            continue
        if not img.in_image(v):
            slots.add(slot)
    return sorted(slots)


def find_team_registry(img: Image, refs: RipRefs):
    """Global behind the enum readers' range check, and the count written
    into it by the session-start initializer (`mov dword [reg+8], N`)."""
    globals_ = collections.Counter()
    for m in REGISTRY_CHECK.finditer(img.text):
        win_start = max(0, m.start() - 40)
        window = img.text[win_start:m.start()]
        k = window.rfind(b"\x48\x8b\x05")
        if k >= 0:
            disp = struct.unpack_from("<i", window, k + 3)[0]
            globals_[img.text_rva + win_start + k + 7 + disp] += 1
    if not globals_:
        return None
    g = globals_.most_common(1)[0][0]
    count = None
    for disp_at in refs.to(g):
        store = disp_at - 3
        b = img.read(store, 3)
        if b[0] not in (0x48, 0x4C) or b[1] != 0x89:
            continue
        func = img.function_of(store)
        if not func:
            continue
        reg = None
        for ins in img.disasm(store, 8):
            reg = ins.op_str.split(", ")[1]
            break
        last = None
        for ins in img.disasm(func[0], store - func[0]):
            if ins.mnemonic == "mov" and ins.op_str.startswith(f"dword ptr [{reg} + 8], 0x"):
                last = int(ins.op_str.split(", ")[1], 16)
        if last is not None:
            count = last
    return dict(global_rva=g, count=count if count is not None else 4, readers=sum(globals_.values()))


def find_tls(img: Image):
    t = img.pe.DIRECTORY_ENTRY_TLS.struct
    return dict(template=t.StartAddressOfRawData - img.base,
                size=t.EndAddressOfRawData - t.StartAddressOfRawData)


def find_cfg_dispatch(img: Image):
    lc = img.pe.DIRECTORY_ENTRY_LOAD_CONFIG.struct
    if not lc.GuardCFDispatchFunctionPointer:
        return None
    return img.q(lc.GuardCFDispatchFunctionPointer - img.base) - img.base


def find_packet_names(img: Image, refs: RipRefs) -> dict[int, list[tuple[str, str]]]:
    """netid -> [(PKT class, client class)] from std::function handler registrations."""
    tds = {}
    for name, va, _, data in img.sections:
        if name not in (".data", ".rdata"):
            continue
        for m in LAMBDA_TD.finditer(data):
            tds[va + m.start() - 0x10] = (m.group(1).decode(), m.group(2).decode())
    _, rva, _, rdata = img.section(".rdata")
    words = np.frombuffer(rdata[: len(rdata) // 8 * 8], dtype="<u8")
    slots_by_value: dict[int, list[int]] = collections.defaultdict(list)
    for i in np.nonzero((words > img.base) & (words < img.base + img.size))[0]:
        slots_by_value[int(words[i]) - img.base].append(rva + 8 * int(i))
    callers = direct_calls(img)

    def registration_netid(start: int, register):
        netid = None
        for ins in img.disasm(start, 0x180):
            if ins.mnemonic == "ret":
                return "ret"
            if ins.mnemonic == "mov" and ins.op_str.startswith("r8d, 0x"):
                netid = int(ins.op_str.split(", ")[1], 16)
            if ins.mnemonic == "call" and ins.op_str.startswith("0x"):
                target = int(ins.op_str, 16) - img.base
                if register is None:
                    if netid is not None:
                        return netid, target
                    continue
                if target == register:
                    return netid
                inner = None
                for w in img.disasm(target, 0x80):          # per-class Register<PKT> wrapper
                    if w.mnemonic == "mov" and w.op_str.startswith("r8d, 0x"):
                        inner = int(w.op_str.split(", ")[1], 16)
                    if w.mnemonic == "call" and w.op_str.startswith("0x"):
                        if int(w.op_str, 16) - img.base == register:
                            return inner
                        break
                    if w.mnemonic in ("ret", "jmp"):
                        break
        return None

    sites = []   # (construction start, client, pkt)
    for td, (client, pkt) in tds.items():
        for disp_at in refs.to(td):
            thunk = disp_at - 3                     # lea rax, [rip+TD] ; ret
            if img.read(thunk, 3) != b"\x48\x8d\x05" or img.read(thunk + 7, 1) != b"\xc3":
                continue
            for slot in slots_by_value.get(thunk, []):
                vtable = slot - 0x18                # _Target_type is _Func_impl slot 3
                for d2 in refs.to(vtable):
                    lea = d2 - 3
                    if LEA_RIP.fullmatch(img.read(lea, 3)):
                        sites.append((lea, client, pkt))
    # RegisterHandler: most common call target right after an inline registration.
    reg_votes = collections.Counter()
    for lea, _, _ in sites:
        r = registration_netid(lea, None)
        if isinstance(r, tuple):
            reg_votes[r[1]] += 1
    register = reg_votes.most_common(1)[0][0] if reg_votes else None

    names: dict[int, set] = collections.defaultdict(set)
    for lea, client, pkt in sites:
        netid = registration_netid(lea, register)
        if netid == "ret":                          # built inside a MakeFunction<> helper
            func = img.function_of(lea)
            for call in callers.get(func[0], []) if func else []:
                n = registration_netid(call + 5, register)
                if isinstance(n, int):
                    names[n].add((pkt, client))
        elif isinstance(netid, int):
            names[netid].add((pkt, client))
    return {k: sorted(v) for k, v in names.items()}, register


# --------------------------------------------------------------------------
# Emulation (only what discovery and the optional validation need)
# --------------------------------------------------------------------------

STACK, STACK_SIZE = 0x7FFF_0000_0000, 0x10_0000
HEAP, HEAP_SIZE = 0x7FFE_0000_0000, 0x400_0000
STOP = 0x7FFD_0000_0000
TEB = 0x7FFC_0000_0000
STUBS = 0x7FFB_0000_0000


class Emu:
    def __init__(self, img: Image, cfg: dict):
        self.img = img
        uc = self.uc = Uc(UC_ARCH_X86, UC_MODE_64)
        uc.mem_map(img.base, (img.size + 0xFFF) & ~0xFFF, UC_PROT_ALL)
        for _, va, _, data in img.sections:
            uc.mem_write(img.base + va, bytes(data))
        for base, size in ((STACK, STACK_SIZE), (HEAP, HEAP_SIZE), (STOP, 0x1000), (TEB, 0x4000), (STUBS, 0x1000)):
            uc.mem_map(base, size)
        uc.mem_write(STOP, b"\xf4" * 0x1000)
        uc.mem_write(STUBS, b"\x31\xc0\xc3")
        for slot in cfg["import_slots"]:
            uc.mem_write(img.base + slot, struct.pack("<Q", STUBS))
        tls = TEB + 0x2000
        uc.mem_write(tls, img.read(cfg["tls"]["template"], cfg["tls"]["size"]))
        uc.mem_write(TEB + 0x1000, struct.pack("<Q", tls) * 64)
        uc.mem_write(TEB + 0x30, struct.pack("<Q", TEB))
        uc.mem_write(TEB + 0x58, struct.pack("<Q", TEB + 0x1000))
        uc.reg_write(UC_X86_REG_GS_BASE, TEB)
        if cfg.get("team_registry"):
            reg = STUBS + 0x100
            uc.mem_write(reg + 8, struct.pack("<I", cfg["team_registry"]["count"]))
            uc.mem_write(reg + 0x70, b"\x01")
            uc.mem_write(img.base + cfg["team_registry"]["global_rva"], struct.pack("<Q", reg))
        for rva in cfg["free"]:
            uc.mem_write(img.base + rva, b"\xc3")
        alloc_va = img.base + cfg["alloc"]
        uc.mem_write(alloc_va, b"\xc3")
        uc.hook_add(UC_HOOK_CODE, lambda u, a, s, _: u.reg_write(UC_X86_REG_RAX, self.alloc(u.reg_read(UC_X86_REG_RCX))),
                    begin=alloc_va, end=alloc_va)
        if cfg.get("cfg_dispatch") is not None:
            va = img.base + cfg["cfg_dispatch"]

            def redirect(u, a, s, _):
                if not self.mapped(u.reg_read(UC_X86_REG_RAX)):
                    u.reg_write(UC_X86_REG_RAX, STUBS)
            uc.hook_add(UC_HOOK_CODE, redirect, begin=va, end=va)
        self.cursor = HEAP

    def mapped(self, a: int) -> bool:
        return (self.img.in_image(a) or HEAP <= a < HEAP + HEAP_SIZE
                or STUBS <= a < STUBS + 0x1000 or STACK <= a < STACK + STACK_SIZE)

    def alloc(self, n: int) -> int:
        self.cursor = (self.cursor + 15) & ~15
        p = self.cursor
        self.cursor += max(n, 1)
        self.uc.mem_write(p, b"\0" * max(n, 1))
        return p

    def call(self, rva: int, *args: int) -> tuple[int, bool]:
        uc = self.uc
        rsp = STACK + STACK_SIZE - 0x2008
        uc.mem_write(rsp, struct.pack("<Q", STOP))
        for reg, a in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9), args):
            uc.reg_write(reg, a)
        uc.reg_write(UC_X86_REG_RSP, rsp)
        try:
            uc.emu_start(self.img.base + rva, STOP, count=5_000_000)
        except UcError:
            return 0, False
        return uc.reg_read(UC_X86_REG_RAX), uc.reg_read(UC_X86_REG_RIP) == STOP

    def q(self, a: int) -> int:
        return struct.unpack("<Q", self.uc.mem_read(a, 8))[0]

    def construct(self, factory: int, netid: int) -> int:
        self.cursor = HEAP
        out = self.alloc(16)
        _, ok = self.call(factory, out, netid)
        return self.q(out) if ok else 0


def find_base_deserialize(img: Image, emu: Emu, factory) -> int:
    counts = collections.Counter(factory["cases"])
    default_case = counts.most_common(1)[0][0]
    votes = collections.Counter()
    for netid, case in enumerate(factory["cases"]):
        if case == default_case:
            continue
        obj = emu.construct(factory["rva"], netid)
        if not obj:
            continue
        deser = emu.q(emu.q(obj) + 8) - img.base
        for ins in img.disasm(deser, 0x40):
            if ins.mnemonic == "call" and ins.op_str.startswith("0x"):
                votes[int(ins.op_str, 16) - img.base] += 1
                break
        if sum(votes.values()) >= 64:
            break
    return votes.most_common(1)[0][0]


# --------------------------------------------------------------------------
# Optional validation against a replay of the same patch
# --------------------------------------------------------------------------

def replay_blocks(path: Path):
    import zstandard
    data = path.read_bytes()
    pos = 0x0F + data[0x0E]
    md_len = struct.unpack_from("<I", data, len(data) - 4)[0]
    end = len(data) - 4 - md_len - 256
    dctx = zstandard.ZstdDecompressor()
    while pos < end:
        _, _, id2, ulen, clen = struct.unpack_from("<IBIII", data, pos)
        pos += 17
        body = data[pos:pos + (clen or ulen)]
        pos += clen or ulen
        if id2 >> 24 != 1:
            continue
        buf = dctx.decompress(body, max_output_size=ulen) if clen else body
        c, pid, param = 0, 0, 0
        while c < len(buf):
            m = buf[c]; c += 1
            c += 1 if m & 0x80 else 4
            if m & 0x10:
                ln = buf[c]; c += 1
            else:
                ln = struct.unpack_from("<I", buf, c)[0]; c += 4
            if not m & 0x40:
                pid = struct.unpack_from("<H", buf, c)[0]; c += 2
            if m & 0x20:
                param = (param + buf[c]) & 0xFFFFFFFF; c += 1
            else:
                param = struct.unpack_from("<I", buf, c)[0]; c += 4
            yield pid, param, buf[c:c + ln]
            c += ln


def validate(img: Image, emu: Emu, cfg: dict, replay: Path, per_netid: int = 20):
    """Decode a sample of every netid; report how many consume their whole
    payload and return success (the decoder's own definition of valid)."""
    factory, base = cfg["factory"]["rva"], cfg["base_deserialize"]
    obj, buf, cur = emu.alloc(0x40), emu.alloc(16), emu.alloc(8)
    term, cont = {}, {}

    def run_base(bs):
        emu.uc.mem_write(obj, b"\0" * 0x40)
        emu.uc.mem_write(buf, bytes(bs))
        emu.uc.mem_write(cur, struct.pack("<Q", buf))
        rax, ok = emu.call(base, obj, cur, buf + len(bs))
        val = struct.unpack("<I", emu.uc.mem_read(obj + 0xC, 4))[0]
        return ok and rax & 0xFF == 1, emu.q(cur) - buf, (val ^ 0x40000000 if val & 0xFFFFFF else val)

    for b in range(256):
        ok, used, v = run_base([b])
        if ok and used == 1:
            term.setdefault(v, b)
    for b in range(256):
        if b not in term.values():
            ok, used, v = run_base([b, term[0]])
            if ok and used == 2:
                cont.setdefault(v & 0x7F, b)

    def encode(net_id):
        v = net_id ^ 0x40000000 if net_id & 0xFFFFFF else net_id
        out = []
        while True:
            g, v = v & 0x7F, v >> 7
            if not v:
                return bytes(out + [term[g]])
            out.append(cont[g])

    blocks = collections.Counter()
    samples = collections.defaultdict(list)
    for pid, param, payload in replay_blocks(replay):
        blocks[pid] += 1
        if len(samples[pid]) < per_netid:
            samples[pid].append((param, payload))
    clean = 0.0
    failing = []
    for pid, items in samples.items():
        ok_n = 0
        for param, payload in items:
            o = emu.construct(factory, pid)
            if not o:
                continue
            stream = encode(param) + payload
            b2 = emu.alloc(len(stream)); emu.uc.mem_write(b2, stream)
            c2 = emu.alloc(8); emu.uc.mem_write(c2, struct.pack("<Q", b2))
            rax, ok = emu.call(emu.q(emu.q(o) + 8) - img.base, o, c2, b2 + len(stream))
            ok_n += ok and rax & 0xFF == 1 and emu.q(c2) - b2 == len(stream)
        clean += blocks[pid] * ok_n / len(items)
        if ok_n < len(items):
            failing.append((blocks[pid], pid, ok_n, len(items)))
    total = sum(blocks.values())
    print(f"validation: {len(samples)} netids, {clean / total * 100:.3f}% of {total} blocks decode cleanly")
    for count, pid, ok_n, n in sorted(failing, reverse=True)[:10]:
        print(f"  netid {pid:5}  blocks={count:7}  clean {ok_n}/{n}")


# --------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--binary", required=True, type=Path, help="unpacked League of Legends.exe")
    ap.add_argument("--output", required=True, type=Path, help="archive to write, e.g. patch/16-9.patch")
    ap.add_argument("--replay", type=Path, help="optional same-patch .rofl to validate against")
    args = ap.parse_args()

    img = Image(args.binary)
    entropy = text_entropy(img.text)
    if entropy > 7.9 or b"\x48\x89\x5c\x24\x08" not in img.text[: 1 << 20]:
        raise SystemExit(
            f".text entropy {entropy:.2f} and no ordinary x86 prologues: this binary's code is "
            "encrypted on disk (Riot's packer). It cannot be used; see docs/COMPATIBILITY.md."
        )
    version = file_version(img)

    refs = RipRefs(img)
    factory = find_factory(img, refs)
    alloc = find_alloc(img, factory)
    free = find_free(img, alloc)
    cfg = dict(
        import_slots=find_import_slots(img),
        tls=find_tls(img),
        cfg_dispatch=find_cfg_dispatch(img),
        team_registry=find_team_registry(img, refs),
        alloc=alloc,
        free=free,
    )
    emu = Emu(img, cfg)
    base = find_base_deserialize(img, emu, factory)
    names, register = find_packet_names(img, refs)

    print(f"factory           0x{factory['rva']:x} (table 0x{factory['table']:x}, {factory['max_netid']} netids)")
    print(f"alloc / free      0x{alloc:x} / {[hex(f) for f in free]}")
    print(f"base_deserialize  0x{base:x}")
    tr = cfg["team_registry"]
    print(f"team_registry     " + (f"0x{tr['global_rva']:x} count={tr['count']} ({tr['readers']} readers)" if tr else "none"))
    print(f"import slots      {len(cfg['import_slots'])}")
    print(f"tls               0x{cfg['tls']['template']:x} +{cfg['tls']['size']}")
    print(f"cfg dispatch      " + (f"0x{cfg['cfg_dispatch']:x}" if cfg["cfg_dispatch"] is not None else "none"))
    print(f"packet names      {len(names)} netids (RegisterHandler 0x{register:x})")

    sections = [(n, va, vs, d) for n, va, vs, d in img.sections if n not in SKIP_SECTIONS]
    result = {
        "format": FORMAT,
        "binary": {"file_version": version, "md5": hashlib.md5(img.raw).hexdigest()},
        "image_base": hx(img.base),
        "image_size": hx(img.size),
        "sections": [{"name": n, "rva": hx(va), "size": hx(vs), "file": f"section{i}.bin"}
                     for i, (n, va, vs, _) in enumerate(sections)],
        "factory": {"rva": hx(factory["rva"]), "max_netid": factory["max_netid"]},
        "base_deserialize": hx(base),
        "alloc": hx(alloc),
        "free": [hx(f) for f in free],
        "cfg_dispatch": hx(cfg["cfg_dispatch"]) if cfg["cfg_dispatch"] is not None else None,
        "tls": {"template": hx(cfg["tls"]["template"]), "size": cfg["tls"]["size"]},
        "import_slots": [hx(s) for s in cfg["import_slots"]],
        "team_registry": {"global": hx(tr["global_rva"]), "count": tr["count"]} if tr else None,
        "packets": {str(k): [{"class": p, "client": c} for p, c in v] for k, v in sorted(names.items())},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(args.output, "w", compression=zipfile.ZIP_DEFLATED) as z:
        z.writestr("result.json", json.dumps(result, indent=1))
        for i, (_, _, _, data) in enumerate(sections):
            z.writestr(f"section{i}.bin", bytes(data))
    print(f"wrote {args.output} ({args.output.stat().st_size / 1e6:.1f} MB)")

    if args.replay:
        validate(img, emu, dict(cfg, factory={"rva": factory["rva"]}, base_deserialize=base), args.replay)


if __name__ == "__main__":
    main()
