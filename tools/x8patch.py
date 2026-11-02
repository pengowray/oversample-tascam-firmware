"""Build a patched Portacapture X8 update file from the x8-patch crate.

    cargo build -p x8-patch --release --target thumbv7em-none-eabihf
    python3 tools/x8patch.py "Tascam_X8_official/extracted/#PortaX8_0217.203" \\
        target/thumbv7em-none-eabihf/release/x8-patch "work/patch/#PortaX8_0219.203" --build 219

The patch is linked to run from SDRAM 0x81830000 (see crates/x8-patch). The
bootloader copies flash 0x20000 onwards into SDRAM from 0x80000000, so storing
the patch at flash 0x1850000, straight after the stock image (which ends with
its checksum at 0x184ffff), puts it at that address before the application
starts. The tool then points the stock firmware's hook words, one tail call
and one call at the patch's entry points, and the TUNER tile's launcher texts
at the patch's strings, all read from the patch's header (see BUILDS), and
packs the result with `x8fw.py pack --base`, which refuses changes to the bootloader and the
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

def flash(address):
    """Flash image offset of a running address in the stock application."""
    return address - x8fw.APP_BASE + x8fw.APP_OFFSET


# What the patch changes in each supported build. Addresses are running
# addresses; the stock value at each is checked before it is changed.
#
# words: a word holding a stock function pointer, replaced by a header field.
# branches: a `b.w` tail call, retargeted to a header field's function.
# calls: a `bl` call, retargeted the same way.
# texts: one launcher text (the TUNER tile's title or a line of its
#   description), replaced by a header field's string. Each is set in three
#   places: the English literal word, the `mov.w r1,#length<<8` that gives the
#   English length, and the 10 per-language pointers (whose length the
#   firmware counts at run time). See work/notes/launcher.md section 6.
BUILDS = {
    (203, 217): {
        "words": [
            # Literal that 0x80da98e0 installs as the app screens' key handler;
            # nothing else reads it. Update mode installs its own handler instead.
            ("app screen key handler", 0x80DA9A20, 0x80DAC0D5, "key_hook"),
            # Literal that 0x80dd2cfc installs as the key handler from record
            # standby until the end of a take; nothing else reads it.
            ("recording screen key handler", 0x80DD3A24, 0x80DD3F9D, "rec_key_hook"),
        ],
        "branches": [
            # The TUNER tile's handler ends `pop.w {r4,lr}; b.w 0x80d6946c`,
            # the bridge setter, with r1 = 6 (the app property), r2 = 7 (TUNER).
            ("TUNER tile", 0x81732C10, 0x80D6946C, "launch_hook"),
        ],
        "calls": [
            # The touch poll's only call that passes a touch to Qt,
            # `FUN_80e0364a(x, y, pressed)` (work/notes/input.md section 3).
            ("touch to Qt", 0x80DF501A, 0x80E0364A, "touch_hook"),
        ],
        "texts": [
            # (name, literal, stock string, mov.w at, stock length, table)
            ("tile title", 0x80F23D9C, 0x813614F4, 0x80F2347C, 5, 0x813B0E48, "title"),
            ("description line 1", 0x80F24670, 0x813614DC, 0x80F23B38, 22, 0x813B32FC, "line1"),
            ("description line 2", 0x80F24AC0, 0x81361968, 0x80F23FA6, 26, 0x813B361C, "line2"),
            ("description line 3", 0x80F24E4C, 0x813618EC, 0x80F2441C, 24, 0x813B3284, "line3"),
            ("description line 4", 0x80F25444, 0x813617EC, 0x80F24856, 22, 0x813B8078, "line4"),
            ("description line 5", 0x80F25744, 0x81361460, 0x80F24C58, 23, 0x813B8168, "line5"),
        ],
    },
}
LANGUAGES = 10

HEADER_FIELDS = ["magic", "version", "key_hook", "rec_key_hook", "launch_hook", "touch_hook",
                 "title", "line1", "line2", "line3", "line4", "line5"]
HEADER_FORMAT = "<4s11I"

B_W, BL = 0x9000, 0xD000   # bits 15, 14 and 12 of the second halfword


def decode_bw(hw1, hw2, at, kind=B_W):
    """Target of a Thumb-2 `b.w` (encoding T4) or `bl` (kind=BL) at `at`, or None."""
    if hw1 & 0xF800 != 0xF000 or hw2 & 0xD000 != kind:
        return None
    s = hw1 >> 10 & 1
    i1 = 1 - ((hw2 >> 13 & 1) ^ s)
    i2 = 1 - ((hw2 >> 11 & 1) ^ s)
    offset = s << 24 | i1 << 23 | i2 << 22 | (hw1 & 0x3FF) << 12 | (hw2 & 0x7FF) << 1
    if s:
        offset -= 1 << 25
    return at + 4 + offset


def encode_bw(at, target, kind=B_W):
    offset = target - (at + 4)
    if offset & 1 or not -(1 << 24) <= offset < (1 << 24):
        raise ValueError(f"b.w at 0x{at:08x} cannot reach 0x{target:08x}")
    s = offset >> 24 & 1
    i1, i2 = offset >> 23 & 1, offset >> 22 & 1
    j1, j2 = (1 - i1) ^ s, (1 - i2) ^ s
    hw1 = 0xF000 | s << 10 | (offset >> 12 & 0x3FF)
    hw2 = kind | j1 << 13 | j2 << 11 | (offset >> 1 & 0x7FF)
    assert decode_bw(hw1, hw2, at, kind) == target
    return hw1, hw2


def expand_imm(imm12):
    """Arm's ThumbExpandImm: the value of a modified immediate constant."""
    if imm12 >> 10 == 0:
        x = imm12 & 0xFF
        return [x, x * 0x00010001, x * 0x01000100, x * 0x01010101][imm12 >> 8 & 3]
    value, rotate = 0x80 | imm12 & 0x7F, imm12 >> 7
    return (value >> rotate | value << (32 - rotate)) & 0xFFFFFFFF


def decode_mov_r1(hw1, hw2):
    """The constant of `mov.w r1,#const` (encoding T2), or None."""
    if hw1 & 0xFBFF != 0xF04F or hw2 & 0x8F00 != 0x0100:
        return None
    return expand_imm((hw1 >> 10 & 1) << 11 | (hw2 >> 12 & 7) << 8 | hw2 & 0xFF)


def encode_mov_r1(value):
    imm12 = next(i for i in range(4096) if expand_imm(i) == value)
    hw1, hw2 = 0xF04F | (imm12 >> 11) << 10, (imm12 >> 8 & 7) << 12 | 0x0100 | imm12 & 0xFF
    assert decode_mov_r1(hw1, hw2) == value
    return hw1, hw2


def patch_string(patch, address):
    """The NUL-terminated ASCII string at `address` in the patch."""
    at = address - PATCH_BASE
    if not 0 <= at < len(patch) or b"\0" not in patch[at:]:
        raise ValueError(f"string at 0x{address:08x} is not inside the patch")
    text = patch[at:patch.index(b"\0", at)]
    if not text.isascii() or len(text) > 255:
        raise ValueError(f"string at 0x{address:08x} is not ASCII of 255 characters or fewer")
    return text.decode()


def apply(image, patch, fields, build):
    """Make the build's changes in `image`; returns an error message or None."""
    word = lambda address: struct.unpack_from("<I", image, flash(address))[0]
    half = lambda address: struct.unpack_from("<H", image, flash(address))[0]
    put_word = lambda address, value: struct.pack_into("<I", image, flash(address), value)
    put_half = lambda address, value: struct.pack_into("<H", image, flash(address), value)
    in_patch = lambda target: PATCH_BASE <= target < PATCH_BASE + len(patch)

    for name, address, stock, field in build["words"]:
        target = fields[field]
        if word(address) != stock:
            return f"{name} at 0x{address:08x} holds 0x{word(address):08x}, expected 0x{stock:08x}"
        if not (target & 1 and in_patch(target)):
            return f"{field} = 0x{target:08x} is not a Thumb address inside the patch"
        put_word(address, target)
        print(f"{name}: 0x{stock:08x} -> 0x{target:08x}")

    branches = [(b, B_W, "b.w") for b in build["branches"]] + [(c, BL, "bl") for c in build["calls"]]
    for (name, address, stock, field), kind, mnemonic in branches:
        target = fields[field]
        current = decode_bw(half(address), half(address + 2), address, kind)
        if current != stock or encode_bw(address, stock, kind) != (half(address), half(address + 2)):
            return f"{name}: no {mnemonic} 0x{stock:08x} at 0x{address:08x}"
        if not (target & 1 and in_patch(target)):
            return f"{field} = 0x{target:08x} is not a Thumb address inside the patch"
        hw1, hw2 = encode_bw(address, target & ~1, kind)
        put_half(address, hw1)
        put_half(address + 2, hw2)
        print(f"{name}: {mnemonic} 0x{stock:08x} -> 0x{target & ~1:08x}")

    for name, literal, stock, mov_at, stock_len, table, field in build["texts"]:
        target = fields[field]
        text = patch_string(patch, target)
        if word(literal) != stock or word(table) != stock:
            return f"{name}: literal 0x{literal:08x} or table 0x{table:08x} does not hold 0x{stock:08x}"
        if decode_mov_r1(half(mov_at), half(mov_at + 2)) != stock_len << 8:
            return f"{name}: no mov.w r1,#0x{stock_len << 8:x} at 0x{mov_at:08x}"
        for k in range(LANGUAGES):
            if not 0x81300000 <= word(table + 4 * k) < 0x81900000:
                return f"{name}: table entry {k} at 0x{table + 4 * k:08x} is not a string pointer"
        put_word(literal, target)
        hw1, hw2 = encode_mov_r1(len(text) << 8)
        put_half(mov_at, hw1)
        put_half(mov_at + 2, hw2)
        for k in range(LANGUAGES):
            put_word(table + 4 * k, target)
        print(f"{name}: {text!r}")
    return None


def elf_symbols(data, shoff, shentsize, shnum):
    """Names and values of the ELF file's symbols."""
    out = {}
    for i in range(shnum):
        _, kind, _, _, offset, size, link = struct.unpack_from("<IIIIIII", data, shoff + i * shentsize)
        if kind != 2:      # SHT_SYMTAB
            continue
        strings = struct.unpack_from("<I", data, shoff + link * shentsize + 16)[0]
        for k in range(size // 16):
            name, value = struct.unpack_from("<II", data, offset + k * 16)
            out[data[strings + name:data.index(b"\0", strings + name)].decode()] = value
    return out


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
    symbols = elf_symbols(data, shoff, shentsize, shnum)
    out = bytearray()
    for i in range(shnum):
        name, kind, flags, addr, offset, size = struct.unpack_from("<IIIIII", data, shoff + i * shentsize)
        if not flags & 2 or size == 0:     # SHF_ALLOC
            continue
        section = data[names_at + name:data.index(b"\0", names_at + name)].decode()
        if section == ".itcm":
            # Runs in ITCM, stored at its load address in the patch region.
            print(f"patch: .itcm {size:,} bytes, runs at 0x{addr:08x}, stored at 0x{symbols['__itcm_load']:08x}")
            addr = symbols["__itcm_load"]
        if section == ".dtcm":
            print(f"patch: .dtcm {size:,} bytes at 0x{addr:08x} to 0x{addr + size:08x}, not stored")
            continue
        if not (PATCH_BASE <= addr and addr + size <= PATCH_LIMIT):
            raise ValueError(f"section {section} at 0x{addr:08x} is outside the patch region")
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
    build = BUILDS.get((info["version"], info["build"]))
    if build is None:
        print(f"no hook addresses for V{info['version']} build {info['build']}")
        return 1

    patch = elf_image(args.elf)
    if patch[:4] != b"BATX":
        print("the patch does not start with its header (magic BATX)")
        return 1
    fields = dict(zip(HEADER_FIELDS, struct.unpack_from(HEADER_FORMAT, patch, 0)))
    print(f"patch: {len(patch):,} bytes at 0x{PATCH_BASE:08x}, header version {fields['version']}")

    image = bytearray(image)
    try:
        error = apply(image, patch, fields, build)
    except ValueError as e:
        error = str(e)
    if error:
        print(f"refused: {error}")
        return 1

    padded = patch + bytes(-len(patch) % 4)
    image = bytes(image) + padded + bytes(4)   # the last 4 bytes become the sum32 trailer
    work = os.path.splitext(args.update)[0] + "-image.bin"
    os.makedirs(os.path.dirname(os.path.abspath(args.update)), exist_ok=True)
    open(work, "wb").write(image)
    return x8fw.main(["pack", work, args.update, "--base", args.base, "--build", str(args.build)])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
