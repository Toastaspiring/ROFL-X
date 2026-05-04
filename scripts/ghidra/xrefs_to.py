# Dump all xrefs (code + data) to a given RVA.
# Args: [output_json, target_rva_hex]
# @category ROFL-X
# @runtime Jython

import json

# pylint: disable=undefined-variable

args = list(getScriptArgs())
out_path = args[0]
target_rva = int(args[1], 16)

prog = currentProgram
fm = prog.functionManager
ref_mgr = prog.referenceManager
base = prog.imageBase.offset
target_addr = prog.imageBase.add(target_rva)

print("dumping xrefs to 0x{:x} ({}):".format(target_rva, target_addr))

refs = ref_mgr.getReferencesTo(target_addr)
ref_list = []
for r in refs:
    from_addr = r.fromAddress
    rva = from_addr.offset - base
    fn = fm.getFunctionContaining(from_addr)
    fn_name = fn.name if fn else "<no function>"
    fn_rva = (fn.entryPoint.offset - base) if fn else None
    ref_list.append({
        "from_rva": "0x{:x}".format(rva),
        "type": r.referenceType.toString(),
        "from_function_name": fn_name,
        "from_function_rva": "0x{:x}".format(fn_rva) if fn_rva is not None else None,
    })
    print("  from 0x{:x}  type={}  in {}@0x{:x}".format(
        rva, r.referenceType, fn_name, fn_rva or 0))

with open(out_path, "w") as f:
    json.dump({"target_rva": "0x{:x}".format(target_rva), "xrefs": ref_list}, f, indent=2)
print("wrote " + out_path)
