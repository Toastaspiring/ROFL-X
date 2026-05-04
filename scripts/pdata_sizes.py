#!/usr/bin/env python
"""Print function size for an RVA from a PE's .pdata section.

Usage: python scripts/pdata_sizes.py <binary> <rva_hex> [<rva_hex>...]
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path


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
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    sizes = load_pdata_sizes(Path(sys.argv[1]))
    for rva_hex in sys.argv[2:]:
        rva = int(rva_hex, 16)
        size = sizes.get(rva)
        if size is None:
            print(f"{rva_hex}: NOT FOUND in .pdata")
        else:
            print(f"{rva_hex}: size={size} (0x{size:x}), end=0x{rva + size:x}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
