# Minimal Ghidra headless smoke test: confirm -process mode + arg passing.
# Writes a JSON describing the program and the args it received.
# @category ROFL-X
# @runtime Jython

import json

# pylint: disable=undefined-variable

args = list(getScriptArgs())
print("getScriptArgs() returned: " + repr(args))

prog = currentProgram
out = {
    "program_name": prog.name,
    "image_base_hex": "0x{:x}".format(prog.imageBase.offset),
    "function_count": prog.functionManager.functionCount,
    "args_received": args,
}

if args:
    target = args[0]
    print("Writing to: " + target)
    with open(target, "w") as f:
        json.dump(out, f, indent=2)
    print("OK wrote " + target)
else:
    print("NO ARGS; would-have-written: " + json.dumps(out))
