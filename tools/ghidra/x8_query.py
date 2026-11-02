# Ghidra post-script (Jython): answer questions about the analysed X8 program.
# Run with analyzeHeadless ... -process -noanalysis -postScript x8_query.py CMD ARGS
#   decomp ADDR...   decompiled C of the functions containing each address
#   xrefs ADDR...    references to each address, with the function they are in
#   callers ADDR...  functions that call the function at each address
#   disasm ADDR N    N instructions from ADDR
# Output goes to the file named by X8_OUT (default: stdout).
#@category X8
import os, sys
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor

args = list(getScriptArgs())
out = open(os.environ["X8_OUT"], "w") if os.environ.get("X8_OUT") else sys.stdout
fm = currentProgram.getFunctionManager()
A = lambda s: toAddr(int(s, 16) if s.lower().startswith("0x") else int(s, 16))

def fname(a):
    f = fm.getFunctionContaining(a)
    return "%s@%s" % (f.getName(), f.getEntryPoint()) if f else "(no function)"

cmd = args.pop(0)
if cmd == "decomp":
    di = DecompInterface()
    di.openProgram(currentProgram)
    for s in args:
        f = fm.getFunctionContaining(A(s))
        if f is None:
            out.write("// no function at %s\n" % s)
            continue
        r = di.decompileFunction(f, 120, ConsoleTaskMonitor())
        out.write("// ===== %s at %s =====\n" % (f.getName(), f.getEntryPoint()))
        out.write((r.getDecompiledFunction().getC() if r.decompileCompleted() else "// failed: " + r.getErrorMessage()) + "\n")
elif cmd == "xrefs":
    for s in args:
        out.write("== refs to %s\n" % s)
        for r in getReferencesTo(A(s)):
            out.write("  %s %s in %s\n" % (r.getFromAddress(), r.getReferenceType(), fname(r.getFromAddress())))
elif cmd == "callers":
    for s in args:
        f = fm.getFunctionAt(A(s)) or fm.getFunctionContaining(A(s))
        out.write("== callers of %s\n" % (f.getName() if f else s))
        if f:
            for c in f.getCallingFunctions(ConsoleTaskMonitor()):
                out.write("  %s at %s\n" % (c.getName(), c.getEntryPoint()))
elif cmd == "disasm":
    a, n = A(args[0]), int(args[1])
    ins = getInstructionAt(a) or getInstructionAfter(a)
    for _ in range(n):
        if ins is None: break
        out.write("%s  %s\n" % (ins.getAddress(), ins))
        ins = ins.getNext()
if out is not sys.stdout:
    out.close()
