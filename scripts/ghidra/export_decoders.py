# Export labeled decoder functions from a Ghidra project as JSON for
# `rofl-x scan-decoder`.
#
# Run from the Ghidra Script Manager (Tools -> Script Manager) on a project
# whose binary has the decoder functions labeled with the names you want
# to export. The script targets Ghidra's Jython (Python 2.7) for maximum
# portability across versions; PyGhidra (Python 3) also runs it unchanged.
#
# Output: one JSON file with the schema in src/scan_decoder.rs
# (DonorDump). Pass an output path via the GUI prompt, or set
# OUTPUT_PATH below.
#
# What gets exported:
#   - patch tag (free-form, prompted)
#   - .text section RVA (auto-detected)
#   - one entry per matching function: rva_start, rva_end, anchors
#
# Anchors: short distinctive byte runs picked from the function body. We
# avoid the first instruction (often boilerplate prologue shared with many
# functions) and avoid any 4-byte window that contains zeros, which catches
# most RIP-relative displacements and immediates that change between
# patches. The remaining bytes are the opcodes and short immediates that
# tend to be stable patch-to-patch.
#
# @author rofl-x
# @category ROFL-X
# @runtime Jython
# pylint: disable=undefined-variable,import-error
#
# The names below are injected by Ghidra's script runtime:
#   currentProgram, askString, askChoice, getBytes, getFunctionContaining

import json
import re

# Match these function names exactly (case-insensitive). Edit to taste.
DEFAULT_DECODER_NAMES = [
    "ward_spawn_decrypt",
    "mov_decrypt",
    "create_hero",
    "hero_die",
    "create_turret",
    "replication",
    "unit_apply_damage",
    "npc_die_map_view",
    "spawn_minion",
    "create_neutral",
    "do_set_cooldown",
    "cast_spell_ans",
    "basic_attack_pos",
    "use_item",
    "buy_item",
    "remove_item",
    "swap_item",
    "barrack_spawn_unit",
    "leave_fog",
    "enter_fog",
    "waypoint_group",
    "waypoint_group_with_speed",
]

# Anchor-picker tunables.
ANCHOR_LEN = 8                 # bytes per anchor; 8 is a sweet spot
ANCHOR_STRIDE = 32             # one anchor every N bytes of function body
MAX_ANCHORS_PER_FN = 32        # cap so the JSON stays small
SKIP_HEAD_BYTES = 16           # skip the first N bytes (shared prologues)


def section_rva(prog, name):
    """Return the RVA of a named memory block, or None."""
    for block in prog.memory.blocks:
        if block.name == name:
            return block.start.offset - prog.imageBase.offset
    return None


def function_bytes(func):
    """Return the raw bytes of a function's body, [entry, max_addr]."""
    body = func.body
    start = body.minAddress
    end = body.maxAddress
    length = int(end.subtract(start)) + 1
    raw = getBytes(start, length)
    # getBytes returns Java signed bytes; coerce to 0..255.
    return bytearray((b & 0xff) for b in raw)


def looks_like_immediate(window):
    """Heuristic: avoid 4-byte windows that look like a constant /
    relocation: any zero byte, or three identical bytes in a row."""
    for i in range(len(window) - 3):
        chunk = window[i:i + 4]
        if 0 in chunk:
            return True
        if chunk[0] == chunk[1] == chunk[2]:
            return True
    return False


def pick_anchors(body):
    """Pick stride-spaced 8-byte anchors that don't look like immediates."""
    anchors = []
    pos = SKIP_HEAD_BYTES
    while pos + ANCHOR_LEN <= len(body) and len(anchors) < MAX_ANCHORS_PER_FN:
        window = body[pos:pos + ANCHOR_LEN]
        if not looks_like_immediate(window):
            anchors.append({
                "offset_in_function": pos,
                "bytes_hex": "".join("{:02x}".format(b) for b in window),
            })
        pos += ANCHOR_STRIDE
    return anchors


def normalize_name(name):
    return re.sub(r"[^a-z0-9_]+", "_", name.lower()).strip("_")


def main():
    # User config via Ghidra's prompts.
    patch_tag = askString("ROFL-X export", "Patch tag (e.g. 15.5):")

    # askString refuses empty input, so we accept a sentinel that means
    # "use the builtin decoder name list". Anything else is treated as a
    # regex.
    name_glob = askString(
        "ROFL-X export",
        "Function-name regex (use 'default' for the builtin decoder list):",
    )
    output_path = askString("ROFL-X export", "Output JSON path:")

    prog = currentProgram
    text_rva = section_rva(prog, ".text")
    if text_rva is None:
        text_rva = 0  # fall back if the binary uses a non-standard name

    if name_glob.strip().lower() == "default":
        defaults_lower = set(s.lower() for s in DEFAULT_DECODER_NAMES)
        match_fn = lambda n: normalize_name(n) in defaults_lower
    else:
        try:
            pattern = re.compile(name_glob, re.IGNORECASE)
        except re.error as e:
            print("invalid regex %r: %s" % (name_glob, e))
            return
        match_fn = lambda n: bool(pattern.search(n))

    fns_out = []
    for func in prog.functionManager.getFunctions(True):
        norm = normalize_name(func.name)
        if not match_fn(func.name):
            continue
        body = function_bytes(func)
        if len(body) < SKIP_HEAD_BYTES + ANCHOR_LEN:
            print("skipping %s: body too short (%d bytes)" % (func.name, len(body)))
            continue
        rva_start = func.entryPoint.offset - prog.imageBase.offset
        rva_end = func.body.maxAddress.offset - prog.imageBase.offset + 1
        anchors = pick_anchors(body)
        if not anchors:
            print("skipping %s: no usable anchors" % func.name)
            continue
        fns_out.append({
            "name": norm,
            "rva_start": "0x{:x}".format(rva_start),
            "rva_end": "0x{:x}".format(rva_end),
            "anchors": anchors,
        })
        print("exported %s (%d anchors, %d bytes)" % (func.name, len(anchors), len(body)))

    dump = {
        "patch": patch_tag,
        "text_rva": "0x{:x}".format(text_rva),
        "functions": fns_out,
    }

    with open(output_path, "w") as f:
        json.dump(dump, f, indent=2)
    print("wrote %d function(s) to %s" % (len(fns_out), output_path))


main()
