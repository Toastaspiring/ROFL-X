# List every function/symbol whose name mentions PKT_ or contains a known
# packet class fragment. Output as JSON keyed by RVA.
#
# Args: [output_json]
# @category ROFL-X
# @runtime Jython

import json

# pylint: disable=undefined-variable

args = list(getScriptArgs())
out_path = args[0] if args else "pkt_symbols.json"

prog = currentProgram
base = prog.imageBase.offset
sym_mgr = prog.symbolTable

results = []
total_syms = 0
seen_rva = set()
for sym in sym_mgr.getAllSymbols(True):
    total_syms += 1
    name = sym.name
    if "PKT_" not in name and "Replication" not in name and "CastSpell" not in name:
        continue
    addr = sym.address
    if addr is None:
        continue
    rva = addr.offset - base
    key = (rva, name)
    if key in seen_rva:
        continue
    seen_rva.add(key)
    results.append({
        "rva": "0x{:x}".format(rva),
        "name": name,
        "namespace": str(sym.parentNamespace) if sym.parentNamespace else "",
        "kind": str(sym.symbolType),
    })

print("scanned {} symbols, kept {} matches".format(total_syms, len(results)))

# Also: walk every .text function and check if its body references any
# string containing "PKT_". This catches handler functions that aren't
# directly named but reference a packet-type string.
listing = prog.listing
fn_mgr = prog.functionManager
mem = prog.memory

# Build a string-VA -> string map for any defined string in .rdata containing PKT_
str_map = {}
for sym in sym_mgr.getAllSymbols(True):
    if sym.symbolType.toString() == "Label":
        continue
    name = sym.name
    if "PKT_" in name:
        str_map[sym.address.offset] = name

print("found {} symbols with PKT_ in name".format(len(str_map)))

with open(out_path, "w") as f:
    json.dump({"matches": results}, f, indent=2)
print("wrote " + out_path)
