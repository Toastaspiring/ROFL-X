# Dump direct callees of given candidate RVAs in the analyzed program.
# Args: [output_json, rva_hex_1, rva_hex_2, ...]
# Output JSON shape:
#   { "<rva_hex>": { "name": ..., "size": ..., "callees": [
#         {"rva": ..., "name": ..., "size": ..., "call_site": ...}
#     ]}, ...}
# @category ROFL-X
# @runtime Jython

import json

# pylint: disable=undefined-variable

args = list(getScriptArgs())
print("args: " + repr(args))
out_path = args[0]
rva_args = args[1:]

prog = currentProgram
listing = prog.listing
fm = prog.functionManager
base = prog.imageBase.offset

result = {}
for rva_hex in rva_args:
    rva = int(rva_hex, 16)
    addr = prog.imageBase.add(rva)
    func = fm.getFunctionAt(addr)
    if func is None:
        result[rva_hex] = {"error": "no function at 0x{:x}".format(rva)}
        continue
    body = func.body
    callees = []
    instr_iter = listing.getInstructions(body, True)
    seen_callees = set()
    for ins in instr_iter:
        if ins.flowType.isCall():
            for ref in ins.referencesFrom:
                target = ref.toAddress
                if target is None: continue
                callee_func = fm.getFunctionAt(target)
                callee_rva = target.offset - base
                if callee_func:
                    name = callee_func.name
                    csize = int(callee_func.body.maxAddress.offset - callee_func.body.minAddress.offset) + 1
                else:
                    name = "<unmapped>"
                    csize = 0
                key = (callee_rva, name)
                if key in seen_callees: continue
                seen_callees.add(key)
                callees.append({
                    "rva": "0x{:x}".format(callee_rva),
                    "name": name,
                    "size": csize,
                    "call_site": "0x{:x}".format(ins.address.offset - base),
                })
    fsize = int(body.maxAddress.offset - body.minAddress.offset) + 1
    result[rva_hex] = {
        "name": func.name,
        "rva": rva_hex,
        "size": fsize,
        "callees": callees,
    }
    print("0x{:x} {} ({} bytes) calls {} unique".format(
        rva, func.name, fsize, len(callees)))

with open(out_path, "w") as f:
    json.dump(result, f, indent=2)
print("wrote " + out_path)
