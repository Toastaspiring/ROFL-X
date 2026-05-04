# Decompile a list of functions to readable C using Ghidra's decompiler.
# Args: [output_dir, rva_hex_1, rva_hex_2, ...]
# Writes one .c file per function: <output_dir>/<rva>.c
# @category ROFL-X
# @runtime Jython

import os

# pylint: disable=undefined-variable
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor

args = list(getScriptArgs())
print("args: " + repr(args))
if len(args) < 2:
    print("usage: decompile_funcs.py OUT_DIR RVA1 [RVA2...]")
    raise SystemExit(1)

out_dir = args[0]
rva_args = args[1:]

if not os.path.isdir(out_dir):
    os.makedirs(out_dir)

prog = currentProgram
fm = prog.functionManager

decomp = DecompInterface()
decomp.openProgram(prog)
monitor = ConsoleTaskMonitor()

for rva_hex in rva_args:
    rva = int(rva_hex, 16)
    addr = prog.imageBase.add(rva)
    func = fm.getFunctionAt(addr)
    if func is None:
        print("no function at " + rva_hex)
        continue
    res = decomp.decompileFunction(func, 60, monitor)
    if res is None or not res.decompileCompleted():
        print("decompile failed for " + rva_hex)
        continue
    c_code = res.decompiledFunction.c
    out_path = os.path.join(out_dir, rva_hex.replace("0x", "") + ".c")
    with open(out_path, "w") as f:
        f.write(c_code)
    nlines = c_code.count("\n")
    print("wrote " + out_path + " (" + str(nlines) + " lines)")

decomp.dispose()
print("done")
