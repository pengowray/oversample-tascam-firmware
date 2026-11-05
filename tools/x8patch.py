"""Build a patched Portacapture X8 update file from the x8-patch crate.

    cargo build -p x8-patch --release --target thumbv7em-none-eabihf
    python3 tools/x8patch.py "Tascam_X8_official/extracted/#PortaX8_0217.203" \\
        target/thumbv7em-none-eabihf/release/x8-patch "work/patch/#PortaX8_0219.203" --build 219

The patch is linked to run from SDRAM 0x81830000 (see crates/x8-patch). The
bootloader copies flash 0x20000 onwards into SDRAM from 0x80000000, so storing
the patch at flash 0x1850000, straight after the stock image (which ends with
its checksum at 0x184ffff), puts it at that address before the application
starts. The tool then points the stock firmware's hook words, tail calls,
calls and two function entries at the patch's entry points, all read from the
patch's header (see BUILDS), and packs the result with `x8fw.py pack --base`,
which refuses changes to the bootloader and the known start-up and update-mode
code, and refuses an image long enough to reach the settings sectors.

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
# entries: the first 4 bytes of a stock function (whole instructions, given as
#   two halfwords), replaced by a `b.w` to a header field's function, which
#   does the stock function's whole job.
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
            # The USER SETTINGS 2 and 3 tiles' handlers end
            # `pop.w {r4,lr}; b.w 0x80d6a1c0`, the bridge call that loads a
            # preset, with r0 = bridge and r1 = the slot (1 or 2).
            ("USER SETTINGS 2 tile", 0x81733D2C, 0x80D6A1C0, "tile_hook"),
            ("USER SETTINGS 3 tile", 0x81733D64, 0x80D6A1C0, "tile_hook"),
            # The delete listener (after YES in the delete pop-up) ends
            # `pop.w {r4,r5,lr}; b.w 0x80d6a0f8` with r1 = the slot to delete.
            ("delete preset", 0x81728160, 0x80D6A0F8, "delete_hook"),
        ],
        "calls": [
            # The touch poll's only call that passes a touch to Qt,
            # `FUN_80e0364a(x, y, pressed)` (work/notes/input.md section 3).
            ("touch to Qt", 0x80DF501A, 0x80E0364A, "touch_hook"),
            # The launcher's text bindings (out, binding, root), each called once
            # (work/notes/launcher.md sections 6 and 7).
            ("launcher title", 0x814A765C, 0x80F2326A, "title_hook"),
            ("launcher description line 1", 0x814A7730, 0x80F2393C, "line1_hook"),
            ("launcher description line 2", 0x814A7804, 0x80F23DA8, "line2_hook"),
            ("launcher description line 3", 0x814A78D8, 0x80F24224, "line3_hook"),
            ("launcher description line 4", 0x814A79AC, 0x80F24694, "line4_hook"),
            ("launcher description line 5", 0x814A7A80, 0x80F24AD8, "line5_hook"),
            ("launcher description line 6", 0x814A7B54, 0x80F24E58, "line6_hook"),
            # The label drawn over the title in delete mode, the title's y, and
            # the USER SETTINGS 2 and 3 delete badges, each called once.
            ("launcher delete mode label", 0x814A7F78, 0x80F29138, "label_hook"),
            ("launcher title y", 0x8155C71C, 0x80F23220, "title_y_hook"),
            ("USER SETTINGS 2 delete badge", 0x815CD486, 0x80F2A7E4, "badge2_hook"),
            ("USER SETTINGS 3 delete badge", 0x815CE106, 0x80F2B2B8, "badge3_hook"),
            # The USER SETTINGS 2 and 3 tap handlers' read of the delete mode
            # property, `FUN_80e02a34(root + 0xd370)`.
            ("USER SETTINGS 2 tap, delete mode", 0x8161DA06, 0x80E02A34, "delete_mode_hook"),
            ("USER SETTINGS 3 tap, delete mode", 0x8161D8AA, 0x80E02A34, "delete_mode_hook"),
        ],
        "entries": [
            # The bridge's USER SETTINGS slot getter (bridge, slot):
            # `cmp r1,#3; ldr r3,[pc,#0x2fc]`. Its only callers are the
            # launcher's slot properties.
            ("slot getter", 0x80D6A238, (0x2903, 0x4BBF), "slot_hook"),
            # The bridge's first free slot (1 to 3, or 0 when all are in use),
            # read by the apps' save screens: `ldr.w ip,[pc,#0x2e4]`.
            ("first free slot", 0x80D6A252, (0xF8DF, 0xC2E4), "free_slot_hook"),
        ],
    },
}

HEADER_FIELDS = ["magic", "version", "key_hook", "rec_key_hook", "touch_hook", "tile_hook", "slot_hook",
                 "free_slot_hook", "title_hook", "line1_hook", "line2_hook", "line3_hook", "line4_hook",
                 "line5_hook", "line6_hook", "label_hook", "title_y_hook", "delete_mode_hook", "delete_hook",
                 "badge2_hook", "badge3_hook"]
HEADER_FORMAT = "<4s20I"

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

    for name, address, stock, field in build["entries"]:
        target = fields[field]
        if (half(address), half(address + 2)) != stock:
            return f"{name}: 0x{address:08x} does not start with {stock[0]:04x} {stock[1]:04x}"
        if not (target & 1 and in_patch(target)):
            return f"{field} = 0x{target:08x} is not a Thumb address inside the patch"
        hw1, hw2 = encode_bw(address, target & ~1)
        put_half(address, hw1)
        put_half(address + 2, hw2)
        print(f"{name}: entry 0x{address:08x} -> b.w 0x{target & ~1:08x}")
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
