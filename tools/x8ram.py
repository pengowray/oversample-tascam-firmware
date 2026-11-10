"""Unpack what the X8's main application copies into RAM when it starts.

The application was built with IAR Embedded Workbench. Its initialised data,
and the code that runs from the processor's tightly coupled memory (ITCM),
are stored packed in flash; at start-up IAR's init table zeroes some ranges
and unpacks three `lz77` runs into SDRAM, the ITCM and the DTCM. The format
and the table layout come from qubero2's `crates/core/src/codec/iar.rs` and
`crates/core/src/formats/tascamfw.rs`, which checked them against the
firmware's own unpacking routines.

    python3 tools/x8ram.py work/v203.bin work/ram

writes one file per unpacked run, named by its address (for example
`itcm-00000400.bin`), and `map.txt` listing every range the table fills, so a
disassembler can load each run at the address the code expects.
"""
import os
import struct
import sys

APP_AT = 0x20000          # flash offset the bootloader copies from
APP_BASE = 0x80000000     # where it copies to

# IAR's __iar_init_core and two bytes of padding; the init table follows it.
INIT_CORE = bytes([
    0x4e, 0xf6, 0x88, 0x51, 0xce, 0xf2, 0x00, 0x01, 0x08, 0x68, 0x40, 0xf4, 0x70, 0x00, 0x08, 0x60, 0xbf, 0xf3,
    0x4f, 0x8f, 0xbf, 0xf3, 0x6f, 0x8f, 0x4f, 0xf0, 0x00, 0x70, 0xe1, 0xee, 0x10, 0x0a, 0x70, 0x47, 0x00, 0x00,
])


def lz77(data):
    """IAR's lz77 packing, as qubero2's `codec::iar::lz77` reads it."""
    out = bytearray()
    at = 0
    while at < len(data):
        token = data[at]
        at += 1
        literals = token & 3
        if literals == 0:
            literals = data[at] + 3
            at += 1
        literals -= 1
        length = token >> 4
        if length == 15:
            length = data[at] + 15
            at += 1
        out += data[at:at + literals]
        at += literals
        if length == 0:
            continue
        lo = data[at]
        at += 1
        hi = (token >> 2) & 3
        if hi == 3:
            hi = data[at]
            at += 1
        dist = lo | hi << 8
        if dist == 0 or dist > len(out):
            raise ValueError(f"bad match distance {dist} at packed byte {at}")
        start = len(out) - dist
        for k in range(length + 2):
            out.append(out[start + k])
    return bytes(out)


def region(address):
    if address < 0x20000000:
        return "itcm"
    if address < 0x30000000:
        return "dtcm"
    if 0x80000000 <= address < 0x90000000:
        return "sdram"
    return "ram"


def init_table(image):
    """Read IAR's init table from a flash image. Returns the table's flash offset, the zeroed
    ranges as (address, size), the unpacked runs as (address, data, flash offset, packed size),
    and the flash offset of the constructors routine."""
    at = image.find(INIT_CORE, APP_AT)
    if at < 0:
        raise ValueError("__iar_init_core not found in the application")
    table = at + len(INIT_CORE)
    word = lambda o: struct.unpack_from("<I", image, o)[0]
    sword = lambda o: struct.unpack_from("<i", image, o)[0]

    # Zero ranges: routine, then (size, destination) pairs ended by a size of 0.
    zeroed = []
    p = table + 4
    while word(p) != 0:
        zeroed.append((word(p + 4), word(p)))
        p += 8
    p += 4

    # Three packed runs, back to back in flash.
    runs = []
    for _ in range(3):
        sources, dests = word(p + 4) >> 1, word(p + 8) >> 1
        q = p + 12
        src = []
        for _ in range(sources):
            src.append((q + sword(q), word(q + 4)))
            q += 8
        dst = []
        for _ in range(dests):
            dst.append((word(q), word(q + 4)))
            q += 8
        (start, packed_len), (dest, length) = src[0], dst[0]
        data = lz77(image[start:start + packed_len])
        if len(data) != length:
            raise ValueError(f"run at flash 0x{start:x} unpacked to {len(data)} bytes, expected {length}")
        runs.append((dest, data, start, packed_len))
        p = q
    return table, zeroed, runs, p


def main(argv):
    image_path, out_dir = argv
    image = open(image_path, "rb").read()
    try:
        table, zeroed, runs, ctors = init_table(image)
    except ValueError as e:
        print(e)
        return 1
    addr = lambda o: o - APP_AT + APP_BASE
    os.makedirs(out_dir, exist_ok=True)
    lines = [f"init table at 0x{addr(table):08x} (flash offset 0x{table:x})"]
    lines += [f"zero  0x{dest:08x} +0x{size:x}" for dest, size in zeroed]
    for dest, data, start, packed_len in runs:
        name = f"{region(dest)}-{dest:08x}.bin"
        open(os.path.join(out_dir, name), "wb").write(data)
        lines.append(f"lz77  0x{dest:08x} +0x{len(data):x} from flash 0x{start:x} ({packed_len} packed) -> {name}")
    lines.append(f"ctors routine entry at 0x{addr(ctors):08x}")
    open(os.path.join(out_dir, "map.txt"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
