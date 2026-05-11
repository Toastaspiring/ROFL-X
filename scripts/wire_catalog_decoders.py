#!/usr/bin/env python
"""Wire catalog-mapped decoders into a `.patch` archive's `extra_decoders[]`.

`wire_confirmed_decoders.py` writes decoders that brute-force matched
real packet payloads to a function in `.text`. This script handles the
complementary case: decoders the *catalog's* netid->decoder mapping
claims should fire for a given netid, even though brute-force matching
didn't (or didn't run) on that netid.

The catalog mapping is structural (walks 0xe83930's constructor chain
to find each netid's vtable[1]), so it can wire netids that never fired
in the brute-force corpus. That sometimes produces FAIL/no_data verdicts
when emulation runs — that's information too: the catalog over-claims
on those netids.

For netids the patch archive *already* wires to a different RVA, we
skip rather than override; brute-force evidence on real payloads beats
structural inference.

rva_end values come from `pdata_sizes.py` against the live binary.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
import zipfile
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pdata_sizes import load_pdata_sizes  # type: ignore


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--patch", required=True,
                    help="path to .patch archive, modified in place")
    ap.add_argument("--catalog", default="scripts/semantic_field_names.json")
    ap.add_argument("--binary", required=True,
                    help="path to the League of Legends.exe to read .pdata from")
    ap.add_argument("--rvas", nargs="+", required=True,
                    help="decoder RVAs to wire (hex with 0x prefix)")
    args = ap.parse_args()

    catalog = json.loads(Path(args.catalog).read_text())
    netid_to_decoder = catalog["netid_to_decoder"]
    sizes = load_pdata_sizes(Path(args.binary))

    with zipfile.ZipFile(args.patch) as z:
        members = {n: z.read(n) for n in z.namelist()}
    result = json.loads(members["result.json"])
    eds = result["extra_decoders"]

    # netids already claimed by some entry (any decoder).
    claimed: dict[int, str] = {e["netid"]: e["rva_start"] for e in eds}

    added: list[dict] = []
    skipped: list[tuple[int, str, str]] = []  # (netid, want_rva, has_rva)

    for rva_hex in args.rvas:
        rva = int(rva_hex, 16)
        size = sizes.get(rva)
        if size is None:
            print(f"WARN: {rva_hex} not in .pdata, skipping entire RVA",
                  file=sys.stderr)
            continue
        end = rva + size
        # Find all netids the catalog maps to this RVA.
        netids = sorted(
            int(n) for n, r in netid_to_decoder.items() if r == rva_hex
        )
        print(f"\n== {rva_hex} (end=0x{end:x}, size=0x{size:x}) — "
              f"catalog claims {len(netids)} netid(s) ==")
        for n in netids:
            if n in claimed:
                if claimed[n] == rva_hex:
                    print(f"  netid {n}: already wired to {rva_hex}, skip")
                else:
                    print(f"  netid {n}: CONFLICT (wired to {claimed[n]}, "
                          f"keeping that — brute-force evidence wins)")
                    skipped.append((n, rva_hex, claimed[n]))
                continue
            entry = {
                "name": f"netid_{n}",
                "netid": n,
                "rva_start": rva_hex,
                "rva_end": f"0x{end:x}",
                "struct_size": "0x90",
                "semantic_hint": "catalog-mapped (no brute-force match)",
            }
            eds.append(entry)
            claimed[n] = rva_hex
            added.append(entry)
            print(f"  netid {n}: added")

    print(f"\n--- summary ---")
    print(f"added: {len(added)}")
    print(f"skipped due to existing wire: {len(skipped)}")
    if not added:
        print("nothing to write; archive unchanged")
        return 0

    # Sort for stable diffs.
    eds.sort(key=lambda e: e["netid"])
    members["result.json"] = json.dumps(result, indent=2).encode("utf-8")

    # Rewrite the archive (zipfile can't edit in place).
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in members.items():
            z.writestr(name, data)
    Path(args.patch).write_bytes(out.getvalue())
    print(f"updated {args.patch}: {len(eds)} extra_decoders (was "
          f"{len(eds) - len(added)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
