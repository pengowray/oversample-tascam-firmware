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
patch's header (see BUILDS), gives the logo screen the patch's lower image, with the bat and
"Oversample Edition" (tools/make_splash.py), and packs the result with `x8fw.py pack --base`,
which refuses changes to the bootloader and the known start-up and update-mode
code, and refuses an image long enough to reach the settings sectors.

Only V2.03 build 217 is supported: the hook addresses are that build's.
"""
import argparse
import hashlib
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
# facts: a stock word the patch's constants depend on, checked and left alone.
# entries: the first 4 bytes of a stock function (whole instructions, given as
#   two halfwords), replaced by a `b.w` to a header field's function, which
#   does the stock function's whole job.
# stock_pixels: stock image pixels that the patch's own images point at, checked by the
#   SHA-256 of the stock bytes and left alone.
# handles: a literal holding the address of a stock image handle, replaced by a header field
#   that is the address of a handle in the patch. The image it leads to is checked (see
#   check_image) and its height is the header field's name with "_height" added.
# immediates: a `movs r0, #value` (16 bits), whose value is replaced by a header field's.
# halfwords: a 16-bit value in stock data, checked and replaced by a fixed value (not a header
#   field). The data is used in every mode, update mode included.
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
            # The USER SETTINGS 2 and 3 icons' visibility (hidden in delete
            # mode, where the badge takes their place), each called once.
            ("USER SETTINGS 2 icon shown", 0x815CD6DE, 0x80F2AA40, "icon2_hook"),
            ("USER SETTINGS 3 icon shown", 0x815CE35E, 0x80F2B510, "icon3_hook"),
            # Those tiles' icon: its image, and its height and width, from which
            # the stock bindings centre it. Each is called once.
            ("USER SETTINGS 2 icon image", 0x814A8470, 0x80F2A80C, "icon_image2_hook"),
            ("USER SETTINGS 3 icon image", 0x814A86EC, 0x80F2B2E0, "icon_image3_hook"),
            ("USER SETTINGS 2 icon height", 0x815CD54E, 0x80F2A968, "icon_height2_hook"),
            ("USER SETTINGS 3 icon height", 0x815CE1CE, 0x80F2B438, "icon_height3_hook"),
            ("USER SETTINGS 2 icon width", 0x815CD616, 0x80F2A9D4, "icon_width2_hook"),
            ("USER SETTINGS 3 icon width", 0x815CE296, 0x80F2B4A4, "icon_width3_hook"),
            # The USER SETTINGS 2 and 3 tap handlers' read of the delete mode
            # property, `FUN_80e02a34(root + 0xd370)`.
            ("USER SETTINGS 2 tap, delete mode", 0x8161DA06, 0x80E02A34, "delete_mode_hook"),
            ("USER SETTINGS 3 tap, delete mode", 0x8161D8AA, 0x80E02A34, "delete_mode_hook"),
            # The power-off sequence FUN_80dca8b8's settings save, between
            # `cpsid i` and `cpsie i`. The save function's other callers are left alone.
            ("power-off settings save", 0x80DCA996, 0x80DA2778, "power_off_save_hook"),
            # The only call that opens a take's files for recording, in FUN_80dc99dc.
            # The file open function's other caller writes the master for overdub.
            ("open take files", 0x80DC9AFE, 0x80DB19B8, "open_take_hook"),
            # FUN_80db63d0's call to the remaining time function, straight after it
            # has rebuilt the file mask. That function's other callers are left alone.
            ("file mask built", 0x80DB6532, 0x80DB1DC8, "file_mask_hook"),
            # FUN_80db216c's call to the file size function, straight after it has rewritten a WAV
            # file's header at the end of a take or a split. Its two earlier calls are left alone.
            ("WAV header rewritten", 0x80DB236A, 0x80DE77CE, "guano_hook"),
            # The USB task's start-up FUN_80dc7064 calls the device stack's class init once, before
            # the device starts (docs/HOMEBREW.md, "192 kHz stereo over USB").
            ("USB class init", 0x80DC70A0, 0x80DFAE66, "usb_init_hook"),
            # The device callback's SET_INTERFACE branch calls interface 2's handler (through the
            # thunk to ITCM 0x152c8) with r2 = the alternate setting.
            ("USB interface 2 alternate setting", 0x80DC6FD8, 0x80DC6E48, "usb_alt_hook"),
        ],
        "facts": [
            # The veneer FUN_80e06310 goes to ITCM 0x1e484, which returns 0x1e498,
            # the alpha mask layout the patch's launcher glyph uses.
            ("alpha mask layout veneer", 0x80E06314, 0x0001E485),
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
        "stock_pixels": [
            # The logo screen's 8-bit alpha masks for "Portacapture X8" (docs/HOMEBREW.md,
            # "The logo screen"), 245 x 21 and 6 x 7. TASCAM's logo is left as it is.
            ("logo: Portacapture X8", 0x812F4F44, 245 * 21,
             "c2647a76e971f4e261f7f4ca2a494911317ff9a5e2fefaa13369ab8be85a054b"),
            ("logo: descender of the p", 0x81766F5C, 6 * 7,
             "2feccdbb579fa6448d567c10338fea9add41e39778c893cdd496e8e187d89984"),
        ],
        "handles": [
            # The literal that the logo screen's constructor, FUN_81259484, reads the lower
            # image's handle address from. Nothing else reads it.
            ("logo screen lower image", 0x81259B88, 0x819A040C, "logo_lower"),
        ],
        "immediates": [
            # The y of the logo screen's two items, and the height of the lower one, in
            # FUN_81259484. The widths, 124 and 243, are unchanged, so each item is still
            # centred by its x binding.
            ("logo screen upper item y", 0x8125971C, 190, "logo_upper_y"),
            ("logo screen lower item y", 0x81259784, 254, "logo_lower_y"),
            ("logo screen lower item height", 0x81259754, 24, "logo_lower_height"),
        ],
        "halfwords": [
            # wMaxPacketSize of the UAC2 endpoints of the 2-channel alternate settings, in the
            # descriptor bytes that FUN_80df471c copies from 0x813b69fc. The stock sizes hold 13
            # frames, enough for 96 kHz; 25 frames are needed at 192 kHz.
            ("USB interface 2 alt 2 packet size (2 x 3 bytes)", 0x813B6AB2, 78, 150),
            ("USB interface 3 alt 1 packet size (2 x 3 bytes)", 0x813B6B17, 78, 150),
            ("USB interface 3 alt 2 packet size (2 x 4 bytes)", 0x813B6B4C, 104, 200),
        ],
    },
}

HEADER_FIELDS = ["magic", "version", "key_hook", "rec_key_hook", "touch_hook", "tile_hook", "slot_hook",
                 "free_slot_hook", "title_hook", "line1_hook", "line2_hook", "line3_hook", "line4_hook",
                 "line5_hook", "line6_hook", "label_hook", "title_y_hook", "delete_mode_hook", "delete_hook",
                 "badge2_hook", "badge3_hook", "icon2_hook", "icon3_hook", "icon_image2_hook", "icon_image3_hook",
                 "icon_height2_hook", "icon_height3_hook", "icon_width2_hook", "icon_width3_hook",
                 "power_off_save_hook", "open_take_hook", "file_mask_hook", "guano_hook", "usb_init_hook",
                 "usb_alt_hook", "logo_lower",
                 "logo_upper_y", "logo_lower_y", "build"]
HEADER_FORMAT = "<4s38I"
# The packer writes the update file's build number here, for the CPU test to show.
BUILD_OFFSET = struct.calcsize(HEADER_FORMAT) - 4
HEADER_VERSION = 20

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

    for name, address, stock in build["facts"]:
        if word(address) != stock:
            return f"{name} at 0x{address:08x} holds 0x{word(address):08x}, expected 0x{stock:08x}"

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

    for name, address, size, stock_sha256 in build["stock_pixels"]:
        at = flash(address)
        if hashlib.sha256(image[at:at + size]).hexdigest() != stock_sha256:
            return f"{name}: the {size} bytes at 0x{address:08x} are not the stock pixels"

    stock_pixels = {address: size for _, address, size, _ in build["stock_pixels"]}
    for name, address, stock, field in build["handles"]:
        handle = fields[field]
        if word(address) != stock:
            return f"{name} at 0x{address:08x} holds 0x{word(address):08x}, expected 0x{stock:08x}"
        if handle & 3 or not in_patch(handle):
            return f"{field} = 0x{handle:08x} is not a word inside the patch"
        height, error = check_image(patch, struct.unpack_from("<I", patch, handle - PATCH_BASE)[0], stock_pixels)
        if error:
            return f"{name}: {error}"
        fields[field + "_height"] = height
        put_word(address, handle)
        print(f"{name}: handle 0x{stock:08x} -> 0x{handle:08x}")

    for name, address, stock, field in build["immediates"]:
        value = fields[field]
        if half(address) != MOVS_R0 | stock:
            return f"{name}: no movs r0, #{stock} at 0x{address:08x}"
        if not 0 <= value <= 0xFF:
            return f"{field} = {value} does not fit a movs"
        put_half(address, MOVS_R0 | value)
        print(f"{name}: movs r0, #{stock} -> #{value}")

    for name, address, stock, value in build["halfwords"]:
        if half(address) != stock:
            return f"{name} at 0x{address:08x} holds {half(address)}, expected {stock}"
        put_half(address, value)
        print(f"{name}: {stock} -> {value}")
    return None


MOVS_R0 = 0x2000
ALPHA8_LAYOUT = 0x1E498
LOGO_COLOUR = 0xFF3E3E3E
# The stock screens draw tiles of up to 5,145 pixels (245 x 21) through the PXP blend's scratch
# buffer; no bigger tile has been tried.
MAX_TILE_PIXELS = 245 * 21


def check_image(patch, descriptor, stock_pixels):
    """Check an alpha mask image of the patch's that a stock screen will draw: every descriptor and
    every pixel it reads is inside the patch or is a whole run of `stock_pixels` (address: size),
    and it has the stock layout and colour. Returns (the image's height, None) or (None, error)."""
    def read(address, size):
        if not (PATCH_BASE <= address and address + size <= PATCH_BASE + len(patch)):
            raise ValueError(f"0x{address:08x} is not inside the patch")
        return patch[address - PATCH_BASE:address - PATCH_BASE + size]
    try:
        colour, width, height, tile_count, tiles = struct.unpack("<5I", read(descriptor, 20))
        if colour != LOGO_COLOUR or not 0 < tile_count <= 8:
            return None, f"image at 0x{descriptor:08x}: colour 0x{colour:08x}, {tile_count} tiles"
        for k in range(tile_count):
            at = tiles + 36 * k
            pixels, layout, flags, offset, size, stride, trim, cache_size, cache_entry = \
                struct.unpack("<9I", read(at, 36))
            x, y = struct.unpack("<hh", struct.pack("<I", offset))
            tile_width, tile_height = size & 0xFFFF, size >> 16
            if layout != ALPHA8_LAYOUT or cache_size or cache_entry or stride != tile_width:
                return None, f"tile at 0x{at:08x} is not a plain 8-bit alpha tile"
            if tile_width * tile_height > MAX_TILE_PIXELS:
                return None, f"tile at 0x{at:08x} is {tile_width} x {tile_height}"
            if (flags, trim) == (0, 0):
                inner = (x, y, x + tile_width, y + tile_height)
            elif (flags, trim) == (2, 0x01010101):
                inner = (x + 1, y + 1, x + tile_width - 1, y + tile_height - 1)
            else:
                return None, f"tile at 0x{at:08x} has flags {flags} and trim 0x{trim:08x}"
            if inner[0] < 0 or inner[1] < 0 or inner[2] > width or inner[3] > height:
                return None, f"tile at 0x{at:08x} is outside its {width} x {height} image"
            if stock_pixels.get(pixels) != tile_width * tile_height:
                read(pixels, tile_width * tile_height)
    except ValueError as e:
        return None, str(e)
    return height, None


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


# Functions that run only after `load_itcm` has filled ITCM: the bat screen's start-up,
# which calls it first, the DSP task it starts, and the tick, which waits for the bat screen.
ITCM_SAFE_ROOTS = ("6screen10enter_work", "4task8dsp_task", "4task9tick_work")


def elf_functions(data, shoff, shentsize, shnum):
    """(name, address, size) of the ELF file's functions, and a reader for bytes at an address."""
    funcs, sections = [], []
    for i in range(shnum):
        _, kind, flags, addr, offset, size, link = struct.unpack_from("<IIIIIII", data, shoff + i * shentsize)
        if kind == 1 and flags & 4:         # SHT_PROGBITS, SHF_EXECINSTR
            sections.append((addr, offset, size))
        if kind != 2:                       # SHT_SYMTAB
            continue
        strings = struct.unpack_from("<I", data, shoff + link * shentsize + 16)[0]
        for k in range(size // 16):
            name, value, length, info = struct.unpack_from("<IIIB", data, offset + k * 16)
            if info & 0xF == 2 and length:  # STT_FUNC
                funcs.append((data[strings + name:data.index(b"\0", strings + name)].decode(), value & ~1, length))

    def code(addr, size):
        for base, offset, length in sections:
            if base <= addr and addr + size <= base + length:
                return data[offset + addr - base:offset + addr - base + size]
        return b""
    return funcs, code


def check_itcm_calls(data, shoff, shentsize, shnum, itcm_start, itcm_end):
    """Code in ITCM runs only after `load_itcm` copies it there. Find every function outside
    ITCM that calls into it, directly or through a linker thunk, follow its direct callers up
    to functions that nothing calls directly, and return an error naming any of those that is
    not in ITCM_SAFE_ROOTS. Callers that ITCM code calls are safe too. Calls through function
    pointers are not followed."""
    funcs, code = elf_functions(data, shoff, shentsize, shnum)
    in_itcm = lambda a: itcm_start <= a < itcm_end
    itcm_names = {name for name, a, _ in funcs if in_itcm(a)}
    starts = {a: name for name, a, _ in funcs}
    calls_itcm, callers, called_from_itcm = set(), {}, set()
    for name, a, size in funcs:
        body = code(a, size)
        for k in range(0, len(body) - 3, 2):
            hw1, hw2 = struct.unpack_from("<HH", body, k)
            target = decode_bw(hw1, hw2, a + k, BL) or decode_bw(hw1, hw2, a + k, B_W)
            callee = starts.get(target)
            if callee is None:
                continue
            thunk = callee.partition("LongThunk_")[2]
            if in_itcm(a):
                # Anything ITCM code calls runs after load_itcm too.
                called_from_itcm.add(thunk or callee)
            elif in_itcm(target) or thunk in itcm_names:
                calls_itcm.add(name)
            elif callee != name:
                callers.setdefault(callee, set()).add(name)
    tops, seen, todo = set(), set(), list(calls_itcm)
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        if name in called_from_itcm or any(root in name for root in ITCM_SAFE_ROOTS):
            continue
        if name in callers:
            todo.extend(callers[name])
        else:
            tops.add(name)
    if tops:
        return "code in ITCM can run before load_itcm, from: " + ", ".join(sorted(tops))
    print(f"patch: {len(calls_itcm)} functions outside ITCM call into it, all after load_itcm")
    return None


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
    error = check_itcm_calls(data, shoff, shentsize, shnum, symbols["__itcm_start"], symbols["__itcm_end"])
    if error:
        raise ValueError(error)
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

    try:
        patch = elf_image(args.elf)
    except ValueError as e:
        print(f"refused: {e}")
        return 1
    if patch[:4] != b"BATX":
        print("the patch does not start with its header (magic BATX)")
        return 1
    fields = dict(zip(HEADER_FIELDS, struct.unpack_from(HEADER_FORMAT, patch, 0)))
    print(f"patch: {len(patch):,} bytes at 0x{PATCH_BASE:08x}, header version {fields['version']}")
    if fields["version"] != HEADER_VERSION or fields["build"] != 0:
        print(f"the patch header is version {fields['version']}, and this packer reads version {HEADER_VERSION}")
        return 1
    patch = bytearray(patch)
    struct.pack_into("<I", patch, BUILD_OFFSET, args.build)
    patch = bytes(patch)
    print(f"patch: build {args.build} written into the header")

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
