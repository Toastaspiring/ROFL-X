# Find candidate decoder functions in the current Ghidra program.
#
# Heuristic: a packet decoder has the signature
#   void decode(struct* out_packet /*RCX*/, byte* payload /*RDX*/, byte* payload_end /*R8*/)
# and its body writes to many small positive offsets of [RCX]. We score every
# function by:
#   - number of distinct positive offsets within [0x10, 0x200] written to via [RCX+disp]
#   - total number of [RCX+disp] write instructions
#   - body size within a plausible range (200..15000 bytes)
#
# The top N candidates are written to a JSON file alongside metadata. This
# does NOT identify which decoder corresponds to which packet class — that
# requires netid mapping, still TODO. But it narrows the search space for
# manual inspection or follow-up byte-pattern matching from ~50000 functions
# to ~50 likely decoders.
#
# Output JSON shape (consumed by Rust-side analysis tools):
#   {
#     "image_base": "0x140000000",
#     "candidates": [
#       {
#         "rva_start": "0xe45710",
#         "rva_end":   "0xe45b35",
#         "size_bytes": 1061,
#         "name": "FUN_140e45710",
#         "distinct_rcx_offsets": [24, 32],
#         "total_rcx_writes": 4,
#         "score": 6.83
#       },
#       ...
#     ]
#   }
#
# @author rofl-x
# @category ROFL-X
# @runtime Jython

import json
import re

# Pre-compiled patterns we care about (instruction mnemonic + first operand).
# Detection works at the instruction level (post-disassembly) for accuracy
# rather than relying on byte patterns; this avoids false positives from
# things like `mov rcx, [...]` (which reads RCX, not writes through it).

MIN_BODY_BYTES = 200
MAX_BODY_BYTES = 15000
MIN_RCX_OFFSET = 0x10
MAX_RCX_OFFSET = 0x200
TOP_N = 200

OUTPUT_PATH_DEFAULT = None  # set to a path or leave None to be prompted

# pylint: disable=undefined-variable


def is_rcx_offset_write(instr):
    """Return the disp if this instruction writes to [RCX+disp] for disp >= 0,
    else None. We treat MOV / MOVZX / MOVSX / MOVQ etc. with the destination
    operand being a memory reference that uses RCX as base + a small positive
    displacement.
    """
    mnem = instr.mnemonicString.upper()
    if mnem not in (
        "MOV", "MOVQ", "MOVUPS", "MOVAPS", "MOVDQA", "MOVDQU",
        "MOVNTI", "MOVZX", "MOVSX", "MOVSXD",
    ):
        return None
    n = instr.numOperands
    if n < 2:
        return None
    # Destination = operand 0 (Ghidra uses dst-first when the language defines so).
    op0_type = instr.getOperandType(0)
    # OperandType.DYNAMIC = 0x4, INDIRECT bit = 0x8 — but the cleanest API path is
    # checking that operand 0 is a memory reference and base register is RCX.
    refs = instr.getOperandReferences(0)
    if not refs:
        # No symbolic ref. Fall back to a string parse — slower but reliable.
        s = instr.getDefaultOperandRepresentation(0)
        m = re.match(r"^\[\s*RCX\s*\+\s*0x([0-9a-fA-F]+)\s*\]$", s)
        if m:
            return int(m.group(1), 16)
        m = re.match(r"^qword\s+ptr\s+\[\s*RCX\s*\+\s*0x([0-9a-fA-F]+)\s*\]$", s, re.IGNORECASE)
        if m:
            return int(m.group(1), 16)
        return None
    # We have refs but they may include data refs (string tables etc) instead
    # of stack offsets. The string parse is most reliable.
    s = instr.getDefaultOperandRepresentation(0)
    m = re.search(r"\[\s*RCX\s*([+-])\s*0x([0-9a-fA-F]+)\s*\]", s)
    if m:
        sign = 1 if m.group(1) == "+" else -1
        return sign * int(m.group(2), 16)
    return None


def score_function(func, listing, base):
    """Score a function. Returns (score, info) or None to skip."""
    body = func.body
    start_addr = body.minAddress
    end_addr = body.maxAddress
    size_bytes = int(end_addr.subtract(start_addr)) + 1
    if size_bytes < MIN_BODY_BYTES or size_bytes > MAX_BODY_BYTES:
        return None

    # Walk instructions. AddressSetView -> all instructions in body.
    distinct_offsets = set()
    total_writes = 0
    instr_iter = listing.getInstructions(body, True)
    for instr in instr_iter:
        disp = is_rcx_offset_write(instr)
        if disp is None:
            continue
        if MIN_RCX_OFFSET <= disp <= MAX_RCX_OFFSET:
            distinct_offsets.add(disp)
            total_writes += 1

    if total_writes == 0:
        return None

    # Score: distinct offsets weigh more than raw total writes.
    score = len(distinct_offsets) * 2.0 + total_writes * 0.1
    info = {
        "name": func.name,
        "rva_start": "0x{:x}".format(start_addr.offset - base),
        "rva_end": "0x{:x}".format(end_addr.offset - base + 1),
        "size_bytes": size_bytes,
        "distinct_rcx_offsets": sorted(list(distinct_offsets)),
        "total_rcx_writes": total_writes,
        "score": score,
    }
    return (score, info)


def main():
    # In headless mode, accept the output path as the first script arg.
    # In GUI mode, fall back to askString.
    output_path = OUTPUT_PATH_DEFAULT
    args = getScriptArgs()
    if args and len(args) >= 1:
        output_path = args[0]
    if output_path is None:
        output_path = askString("ROFL-X find_decoder_candidates",
                                "Output JSON path:")

    prog = currentProgram
    listing = prog.listing
    base = prog.imageBase.offset

    print("scanning %d functions in %s ..." %
          (prog.functionManager.functionCount, prog.name))

    results = []
    fcount = 0
    skip = 0
    for func in prog.functionManager.getFunctions(True):
        fcount += 1
        if fcount % 5000 == 0:
            print("  ... %d / %d (kept %d)" % (
                fcount, prog.functionManager.functionCount, len(results)))
        scored = score_function(func, listing, base)
        if scored is None:
            skip += 1
            continue
        results.append(scored)

    results.sort(reverse=True, key=lambda x: x[0])
    top = [info for _score, info in results[:TOP_N]]
    print("kept %d candidate decoders out of %d functions, "
          "writing top %d to JSON" % (len(results), fcount, len(top)))

    out = {
        "image_base": "0x{:x}".format(base),
        "program_name": prog.name,
        "candidates": top,
    }
    with open(output_path, "w") as f:
        json.dump(out, f, indent=2)
    print("wrote %s" % output_path)


main()
