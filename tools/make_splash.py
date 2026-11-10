"""Make the X8's logo screen: TASCAM's logo and "Portacapture X8" as stock, then the Oversample bat
and "Oversample Edition" below them.

    python3 tools/make_splash.py ../batchi/about-icon.png \\
        /usr/share/fonts/truetype/open-sans/OpenSans-Semibold.ttf crates/x8-patch/src/patch/logo_pixels.rs
    python3 tools/make_splash.py --preview work/v203.bin preview.png

The X8 shows the same Qt screen at start-up and, with POWER OFF added, at power-off: TASCAM's
logo (the upper image, 124 x 21) above "Portacapture X8" (the lower image, 243 x 24), both 8-bit
alpha masks that the firmware draws in dark grey (0xff3e3e3e) on white (docs/HOMEBREW.md, "The
logo screen"). The patch keeps the upper image as it is and gives the lower image's item a new
image, `LOWER_IMAGE` in crates/x8-patch/src/patch/logo.rs, with four tiles:

- the two stock tiles of "Portacapture X8", which point at the stock pixels;
- the bat from the Oversample logo (about-icon.png in the Oversample repository, the same drawing
  as BAT DETECTOR's launcher glyph), BAT_HEIGHT pixels high, twice the height of TASCAM's logo.
  Each pixel's alpha is the logo's opacity times its darkness, so the logo's black outlines and
  eyes stay dark and its light ears stay light. The launcher glyph does the opposite, because the
  launcher draws its glyphs in white on a dark background;
- "Oversample Edition" in Open Sans Semibold (Apache License 2.0; Debian package
  fonts-open-sans), FONT_PIXELS pixels, cropped to its ink.

tools/x8patch.py sets the lower item's height to the new image's height, and moves both items so
that the whole group is centred where the stock one was.

This writes the layout and the two new masks as Rust (logo_pixels.rs, committed, so a build does
not depend on the Pillow or FreeType version). Needs Pillow and numpy. `--preview` draws the logo
screen at start-up from a flash image (from `x8fw.py unpack`, or the image x8patch.py writes next
to the update file) as a PNG, following the constructor's literals and immediates and the image
descriptors, so it shows the stock screen for a stock image.
"""
import struct
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

TEXT = "Oversample Edition"
FONT_PIXELS = 18
BAT_HEIGHT = 42

SCREEN_WIDTH, SCREEN_HEIGHT = 320, 480
INK = (0x3E, 0x3E, 0x3E)

# The stock items: the upper image is 21 high at y 190, the lower one 243 x 24 at y 254.
UPPER_HEIGHT = 21
STOCK_UPPER_Y, STOCK_LOWER_Y = 190, 254
LOWER_WIDTH, STOCK_LOWER_HEIGHT = 243, 24
# Gaps in the lower image: from the bottom of "Portacapture X8" to the bat, and from the bat to
# the text.
GAP_ABOVE_BAT, GAP_BELOW_BAT = 24, 10
# The bottom of the POWER OFF text, 90 pixels above the bottom of the screen. Its top is not
# known; the group must end this far above the text's bottom.
POWER_OFF_BOTTOM = SCREEN_HEIGHT - 90
CLEAR_OF_POWER_OFF = 50
# Tiles of more than 50 pixels are copied into a 0x4c80-byte scratch buffer before the PXP
# blend; the stock screen's biggest tile has 5,145 pixels, so no new tile is bigger.
MAX_TILE_PIXELS = 245 * 21

# Where the logo screen's constructor, FUN_81259484, sets each item's height, width and y with
# `movs r0, #value` (V2.03 build 217), and the literals that hold the address of each image's
# handle.
MOVS = {"upper": (0x812596EC, 0x812596FA, 0x8125971C), "lower": (0x81259754, 0x81259762, 0x81259784)}
HANDLE_LITERALS = {"upper": 0x81259B80, "lower": 0x81259B88}


def bat_pixels(logo_path):
    logo = np.asarray(Image.open(logo_path).convert("RGBA")).astype(float) / 255
    opacity = logo[..., 3]
    luminance = 0.2126 * logo[..., 0] + 0.7152 * logo[..., 1] + 0.0722 * logo[..., 2]
    shape = Image.fromarray((opacity * 255).astype("uint8"), "L").getbbox()
    mask = Image.fromarray((opacity * (1 - luminance) * 255).round().astype("uint8"), "L").crop(shape)
    width = round(mask.width * BAT_HEIGHT / mask.height)
    return mask.resize((width, BAT_HEIGHT), Image.BOX)


def text_pixels(font_path):
    font = ImageFont.truetype(font_path, FONT_PIXELS)
    canvas = Image.new("L", (FONT_PIXELS * len(TEXT), FONT_PIXELS * 3), 0)
    ImageDraw.Draw(canvas).text((FONT_PIXELS, FONT_PIXELS), TEXT, font=font, fill=255)
    return canvas.crop(canvas.getbbox())


def layout(bat, text):
    """Positions of the new tiles in the lower image, its height, and the two items' y."""
    for name, mask in (("bat", bat), ("text", text)):
        if mask.width > LOWER_WIDTH or mask.width * mask.height > MAX_TILE_PIXELS:
            raise SystemExit(f"the {name} is {mask.width} x {mask.height} pixels: too big for one tile")
    bat_y = STOCK_LOWER_HEIGHT + GAP_ABOVE_BAT
    text_y = bat_y + bat.height + GAP_BELOW_BAT
    lower_height = text_y + text.height
    # Keep the middle of the group where the stock group's middle was.
    stock_middle = (STOCK_UPPER_Y + STOCK_LOWER_Y + STOCK_LOWER_HEIGHT) / 2
    upper_y = round(stock_middle - (STOCK_LOWER_Y - STOCK_UPPER_Y + lower_height) / 2)
    lower_y = upper_y + STOCK_LOWER_Y - STOCK_UPPER_Y
    if lower_y + lower_height > POWER_OFF_BOTTOM - CLEAR_OF_POWER_OFF or max(lower_height, lower_y) > 255:
        raise SystemExit(f"the lower image, {lower_height} high at y {lower_y}, does not fit")
    return {
        "UPPER_Y": upper_y,
        "LOWER_Y": lower_y,
        "LOWER_HEIGHT": lower_height,
        "BAT_X": (LOWER_WIDTH - bat.width) // 2,
        "BAT_Y": bat_y,
        "BAT_WIDTH": bat.width,
        "BAT_HEIGHT": bat.height,
        "EDITION_X": (LOWER_WIDTH - text.width) // 2,
        "EDITION_Y": text_y,
        "EDITION_WIDTH": text.width,
        "EDITION_HEIGHT": text.height,
    }


def write(target, values, masks):
    with open(target, "w") as out:
        out.write("//! The logo screen's layout and the masks the patch adds to it.\n")
        out.write("//! Written by tools/make_splash.py, which explains them.\n\n")
        for name, value in values.items():
            out.write(f"pub(super) const {name}: u32 = {value};\n")
        for name, mask in masks:
            pixels = mask.tobytes()
            out.write(f"\n#[rustfmt::skip]\npub(super) static {name}: [u8; {len(pixels)}] = [\n")
            for y in range(mask.height):
                row = pixels[y * mask.width:(y + 1) * mask.width]
                out.write("    " + ", ".join(str(b) for b in row) + ",\n")
            out.write("];\n")


class Memory:
    """The main application's memory as the logo screen sees it: the flash image as the
    bootloader copies it, with IAR's unpacked runs over it."""

    def __init__(self, image):
        import x8ram
        self.image = image
        _, _, self.runs, _ = x8ram.init_table(image)

    def read(self, address, size):
        for dest, data, _, _ in self.runs:
            if dest <= address and address + size <= dest + len(data):
                return data[address - dest:address - dest + size]
        at = address - 0x80000000 + 0x20000
        return self.image[at:at + size]

    def word(self, address):
        return struct.unpack("<I", self.read(address, 4))[0]

    def movs(self, address):
        half = struct.unpack("<H", self.read(address, 2))[0]
        if half & 0xFF00 != 0x2000:
            raise ValueError(f"no movs r0, #value at 0x{address:08x}")
        return half & 0xFF


def preview(image_path, target):
    """Draw the logo screen at start-up from a flash image, as the firmware composes it."""
    memory = Memory(open(image_path, "rb").read())
    screen = Image.new("RGB", (SCREEN_WIDTH, SCREEN_HEIGHT), (255, 255, 255))
    ink = Image.new("RGB", screen.size, INK)
    for item in ("upper", "lower"):
        height, width, y = (memory.movs(a) for a in MOVS[item])
        x = SCREEN_WIDTH // 2 - width // 2
        descriptor = memory.word(memory.word(HANDLE_LITERALS[item]))
        _, image_width, image_height, tile_count, tiles = struct.unpack("<5I", memory.read(descriptor, 20))
        if (image_width, image_height) != (width, height):
            raise ValueError(f"the {item} item is {width} x {height}, its image {image_width} x {image_height}")
        alpha = Image.new("L", screen.size, 0)
        for k in range(tile_count):
            pixels, _, _, offset, size, stride = struct.unpack("<6I", memory.read(tiles + 36 * k, 24))
            tile_x, tile_y = struct.unpack("<hh", struct.pack("<I", offset))
            tile_width, tile_height = size & 0xFFFF, size >> 16
            rows = b"".join(memory.read(pixels + row * stride, tile_width) for row in range(tile_height))
            alpha.paste(Image.frombytes("L", (tile_width, tile_height), rows), (x + tile_x, y + tile_y))
        screen = Image.composite(ink, screen, alpha)
    screen.save(target)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--preview":
        preview(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 4:
        bat, text = bat_pixels(sys.argv[1]), text_pixels(sys.argv[2])
        write(sys.argv[3], layout(bat, text), [("BAT", bat), ("EDITION", text)])
    else:
        sys.exit("usage: make_splash.py LOGO.png FONT.ttf TARGET.rs, or make_splash.py --preview IMAGE.bin OUT.png")
