# Ghidra post-script (Jython): answer questions about the analysed X8 program.
# Run with analyzeHeadless ... -process -noanalysis -postScript x8_query.py CMD ARGS
#   decomp ADDR...   decompiled C of the functions containing each address
#   xrefs ADDR...    references to each address, with the function they are in
#   callers ADDR...  functions that call the function at each address
#   disasm ADDR N    N instructions from ADDR
#   rawdis ADDR N    N Thumb instructions from ADDR, decoded from the bytes, for
#                    code Ghidra's analysis left as data (nothing is saved)
#   refsin LO HI     every reference whose target is in [LO, HI)
#   offs OFF...      every load and store with one of these immediate offsets,
#                    such as settings fields: offs 0x215 0x23e (slow, whole program)
# Several commands can go in one run, separated by ";" (quote it in the shell).
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

def run(cmd, args):
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
    elif cmd == "refsin":
        lo, hi = A(args[0]), A(args[1])
        rm = currentProgram.getReferenceManager()
        it = rm.getReferenceDestinationIterator(lo, True)
        n = 0
        while it.hasNext():
            dst = it.next()
            if dst.compareTo(hi) >= 0:
                break
            for r in getReferencesTo(dst):
                n += 1
                out.write("  %s <- %s %s in %s\n" % (dst, r.getFromAddress(), r.getReferenceType(), fname(r.getFromAddress())))
        out.write("%d references\n" % n)
    elif cmd == "disasm":
        a, n = A(args[0]), int(args[1])
        ins = getInstructionAt(a) or getInstructionAfter(a)
        for _ in range(n):
            if ins is None: break
            out.write("%s  %s\n" % (ins.getAddress(), ins))
            ins = ins.getNext()
    elif cmd == "rawdis":
        from java.math import BigInteger
        a, n = A(args[0]), int(args[1])
        pc = currentProgram.getProgramContext()
        tmode = pc.getRegister("TMode")
        for _ in range(n):
            ins = getInstructionAt(a)
            if ins is None:
                if tmode is not None:
                    try:
                        pc.setValue(tmode, a, a.add(1), BigInteger.ONE)
                    except Exception:
                        pass
                disassemble(a)
                ins = getInstructionAt(a)
            if ins is None:
                out.write("%s  .short 0x%04x\n" % (a, getShort(a) & 0xffff))
                a = a.add(2)
                continue
            out.write("%s  %s\n" % (a, ins))
            a = a.add(ins.getLength())
    elif cmd == "offs":
        # Every load and store whose operands include one of the given
        # immediates, such as [r5,#0x23e]. It cannot see a field reached
        # through a pointer plus a different offset, or a block copy.
        offs = set(int(s, 16) for s in args)
        for ins in currentProgram.getListing().getInstructions(True):
            m = ins.getMnemonicString().lower()
            if not (m.startswith("ldr") or m.startswith("str")):
                continue
            hit = None
            for i in range(ins.getNumOperands()):
                for o in ins.getOpObjects(i):
                    if hasattr(o, "getValue") and o.getValue() in offs:
                        hit = o.getValue()
            if hit is not None:
                out.write("%s  %-36s %s  in %s\n" % (ins.getAddress(), ins, hex(hit), fname(ins.getAddress())))

groups, cur = [], []
for a in args:
    if a == ";":
        groups.append(cur); cur = []
    else:
        cur.append(a)
groups.append(cur)
for g in groups:
    if g:
        run(g[0], g[1:])
if out is not sys.stdout:
    out.close()
