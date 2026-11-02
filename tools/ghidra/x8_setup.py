# Ghidra pre-script (Jython) for the X8 main application, imported as a raw
# binary at 0x80000000 with the ARM:LE:32:Cortex language. Adds the RAM runs
# that tools/x8ram.py unpacks (ITCM code, DTCM and SDRAM data) at the
# addresses the code uses, the zeroed RAM ranges and the peripheral space as
# uninitialised blocks, turns on Thumb mode everywhere, labels the main
# peripherals, and creates functions at the interrupt vectors.
#@category X8
import os, java.io
from java.math import BigInteger
from ghidra.program.model.symbol import SourceType

RAM_DIR = os.environ.get("X8_RAM_DIR", "work/ram")
mem = currentProgram.getMemory()
space = currentProgram.getAddressFactory().getDefaultAddressSpace()
A = lambda x: space.getAddress(x)

app = mem.getBlocks()[0]
app.setName("APP")
app.setExecute(True)
app.setWrite(True)

def loaded(name, addr, path, execute):
    f = java.io.File(path)
    stream = java.io.FileInputStream(f)
    b = mem.createInitializedBlock(name, A(addr), stream, f.length(), monitor, False)
    stream.close()
    b.setRead(True); b.setWrite(True); b.setExecute(execute)
    return b

loaded("ITCM", 0x400, os.path.join(RAM_DIR, "itcm-00000400.bin"), True)
loaded("DTCM_DATA", 0x20000000, os.path.join(RAM_DIR, "dtcm-20000000.bin"), False)
loaded("SDRAM_DATA", 0x81900000, os.path.join(RAM_DIR, "sdram-81900000.bin"), False)

# Ranges the init table zeroes (see work/ram/map.txt), and peripherals.
for name, start, size in [
    ("DTCM_BSS_A", 0x20004180, 0xcb44), ("DTCM_BSS_B", 0x20011cc8, 0x10), ("OCRAM_BSS", 0x20200000, 0x4c80),
    ("SDRAM_BSS_A", 0x819b1870, 0x742268), ("SDRAM_BSS_B", 0x822f3ad8, 0x42ee2b0), ("SDRAM_BSS_C", 0x86600000, 0x17f08f4),
    ("SDRAM_BSS_D", 0x87e00000, 0xa6004), ("PERIPHERALS", 0x40000000, 0x400000), ("PPB", 0xe0000000, 0x100000),
]:
    b = mem.createUninitializedBlock(name, A(start), size, False)
    b.setRead(True); b.setWrite(True)
    if name in ("PERIPHERALS", "PPB"):
        b.setVolatile(True)

# Thumb everywhere code can be.
ctx = currentProgram.getProgramContext()
tmode = ctx.getRegister("TMode")
for b in mem.getBlocks():
    if b.isExecute() and b.isInitialized():
        ctx.setValue(tmode, b.getStart(), b.getEnd(), BigInteger.ONE)

st = currentProgram.getSymbolTable()
for name, addr in [
    ("eDMA", 0x400e8000), ("DMAMUX", 0x400ec000), ("SAI1", 0x40384000), ("SAI2", 0x40388000), ("SAI3", 0x4038c000),
    ("LCDIF", 0x402b8000), ("PXP", 0x402b4000), ("GPIO1", 0x401b8000), ("GPIO2", 0x401bc000), ("GPIO3", 0x401c0000),
    ("GPIO4", 0x401c4000), ("GPIO5", 0x400c0000), ("GPIO6", 0x42000000), ("LPI2C1", 0x403f0000), ("LPI2C2", 0x403f4000),
    ("LPI2C3", 0x403f8000), ("LPI2C4", 0x403fc000), ("LPSPI1", 0x40394000), ("LPSPI2", 0x40398000), ("LPSPI3", 0x4039c000),
    ("LPSPI4", 0x403a0000), ("TMR1", 0x401dc000), ("TMR2", 0x401e0000), ("TMR3", 0x401e4000), ("TMR4", 0x401e8000),
    ("GPT1", 0x401ec000), ("GPT2", 0x401f0000), ("ENC1", 0x403c8000), ("ADC1", 0x400c4000), ("ADC2", 0x400c8000),
    ("CCM", 0x400fc000), ("IOMUXC", 0x401f8000), ("USDHC1", 0x402c0000), ("USDHC2", 0x402c4000), ("USB1", 0x402e0000),
    ("FLEXSPI", 0x402a8000), ("SEMC", 0x402f0000), ("SCB_VTOR", 0xe000ed08),
]:
    st.createLabel(A(addr), name, SourceType.USER_DEFINED)

# Interrupt vectors: 16 system entries and 160 interrupts.
names = {1: "Reset", 2: "NMI", 3: "HardFault", 4: "MemManage", 5: "BusFault", 6: "UsageFault", 11: "SVCall",
         12: "DebugMon", 14: "PendSV", 15: "SysTick", 58: "LCDIF_IRQ", 97: "GPIO1_16_31_IRQ", 99: "GPIO2_16_31_IRQ",
         116: "GPT1_IRQ", 117: "GPT2_IRQ", 128: "USB_OTG2_IRQ", 129: "USB_OTG1_IRQ", 151: "TMR3_IRQ"}
seen = set()
for i in range(176):
    slot = A(0x80000000 + 4 * i)
    createDWord(slot)
    v = getInt(slot) & 0xffffffff
    if i == 0 or not (v & 1):
        continue
    target = A(v & ~1)
    if mem.getBlock(target) is None or v in seen:
        continue
    seen.add(v)
    disassemble(target)
    f = createFunction(target, names.get(i, "vector_%d" % i))
