"""Build a patched Portacapture X8 update file from the x8-patch crate.

    cargo build -p x8-patch --release --target thumbv7em-none-eabihf
    python3 tools/x8patch.py "Tascam_X8_official/extracted/#PortaX8_0217.203" \\
        target/thumbv7em-none-eabihf/release/x8-patch "work/patch/#PortaX8_0219.203" --build 219

The patch is linked to run from SDRAM 0x81830000 (see crates/x8-patch). The
bootloader copies flash 0x20000 onwards into SDRAM from 0x80000000, so storing
the patch at flash 0x1850000, straight after the stock image (which ends with
its checksum at 0x184ffff), puts it at that address before the application
starts. The tool then points the stock firmware's hook words at the patch's
entry points, which it reads from the patch's header, and packs the result
with `x8fw.py pack --base`, which refuses changes to the bootloader and the
known start-up and update-mode code, and refuses an image long enough to reach
the settings sectors.

Only V2.03 build 217 is supported: the hook addresses are that build's.
"""
import argparse
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(__file__))
import x8fw  # noqa: E402

PATCH_BASE = 0x81830000
PATCH_LIMIT = 0x81900000
FLASH_AT = 0x1850000          # flash image offset of PATCH_BASE
STOCK_IMAGE_LEN = 0x1850000

# Hook words: flash image offset of a word holding a stock function pointer,
# its stock value, and the header field (by index) that replaces it.
HOOKS = {
    (203, 217): [
        # Literal that 0x80da98e0 installs as the app screens' key handler;
        # nothing else reads it. Update mode installs its own handler instead.
        ("app screen key handler", 0x80DA9A20 - x8fw.APP_BASE + x8fw.APP_OFFSET, 0x80DAC0D5, "key_hook"),
    ],
}

HEADER_FIELDS = ["magic", "version", "key_hook"]


def elf_image(path):
    """The patch's bytes from PATCH_BASE to the end of its last section, with
    .bss as zeros. .noinit, which the patch writes before reading, is left out
    of the update file (it must come last)."""
    data = open(path, "rb").read()
    if data[:4] != b"\x7fELF" or data[4] != 1 or data[5] != 1:
        raise ValueError("not a 32-bit little-endian ELF file")
    shoff, = struct.unpack_from("<I", data, 0x20)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x2E)
    names_at = struct.unpack_from("<I", data, shoff + shstrndx * shentsize + 16)[0]
    out = bytearray()
    for i in range(shnum):
        name, kind, flags, addr, offset, size = struct.unpack_from("<IIIIII", data, shoff + i * shentsize)
        if not flags & 2 or size == 0:     # SHF_ALLOC
            continue
        if not (PATCH_BASE <= addr and addr + size <= PATCH_LIMIT):
            raise ValueError(f"section at 0x{addr:08x} is outside the patch region")
        section = data[names_at + name:data.index(b"\0", names_at + name)].decode()
        if section == ".noinit":
            print(f"patch: .noinit {size:,} bytes at 0x{addr:08x} to 0x{addr + size:08x}, not stored")
            continue
        end = addr + size - PATCH_BASE
        if len(out) < end:
            out.extend(bytes(end - len(out)))
        if kind != 8:                       # SHT_NOBITS stays zero
            out[addr - PATCH_BASE:end] = data[offset:offset + size]
    return bytes(out)


def main(argv):
    p = argparse.ArgumentParser(description="Build a patched X8 update file from the x8-patch crate.")
    p.add_argument("base", help="official update file, for example #PortaX8_0217.203")
    p.add_argument("elf", help="the x8-patch binary built for thumbv7em-none-eabihf")
    p.add_argument("update", help="patched update file to write")
    p.add_argument("--build", type=int, required=True, help="build number shown on the update screen")
    args = p.parse_args(argv)

    info, image = x8fw.parse(open(args.base, "rb").read())
    if x8fw.check(info) or len(image) != STOCK_IMAGE_LEN:
        print("the base file is not an unmodified official update")
        return 1
    hooks = HOOKS.get((info["version"], info["build"]))
    if hooks is None:
        print(f"no hook addresses for V{info['version']} build {info['build']}")
        return 1

    patch = elf_image(args.elf)
    if patch[:4] != b"BATX":
        print("the patch does not start with its header (magic BATX)")
        return 1
    fields = dict(zip(HEADER_FIELDS, struct.unpack_from("<4sII", patch, 0)))
    print(f"patch: {len(patch):,} bytes at 0x{PATCH_BASE:08x}, header version {fields['version']}")

    image = bytearray(image)
    for name, offset, stock, field in hooks:
        current, = struct.unpack_from("<I", image, offset)
        if current != stock:
            print(f"refused: {name} at flash 0x{offset:x} holds 0x{current:08x}, expected 0x{stock:08x}")
            return 1
        target = fields[field]
        if not (target & 1 and PATCH_BASE <= target < PATCH_BASE + len(patch)):
            print(f"refused: {field} = 0x{target:08x} is not a Thumb address inside the patch")
            return 1
        struct.pack_into("<I", image, offset, target)
        print(f"{name}: 0x{stock:08x} -> 0x{target:08x}")

    padded = patch + bytes(-len(patch) % 4)
    image = bytes(image) + padded + bytes(4)   # the last 4 bytes become the sum32 trailer
    work = os.path.splitext(args.update)[0] + "-image.bin"
    os.makedirs(os.path.dirname(os.path.abspath(args.update)), exist_ok=True)
    open(work, "wb").write(image)
    return x8fw.main(["pack", work, args.update, "--base", args.base, "--build", str(args.build)])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
