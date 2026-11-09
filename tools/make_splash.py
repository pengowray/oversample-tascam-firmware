"""Make the alpha masks for the X8's logo screen: the Oversample bat and "Oversample Edition".

    python3 tools/make_splash.py ../batchi/about-icon.png \\
        /usr/share/fonts/truetype/open-sans/OpenSans-Semibold.ttf tools/splash_pixels.py
    python3 tools/make_splash.py --preview work/v203.bin preview.png

The X8 shows the same Qt screen at start-up and, with POWER OFF added, at power-off: TASCAM's
logo above "Portacapture X8" (docs/HOMEBREW.md, "The logo screen"). Both images are 8-bit alpha
masks that the firmware draws in dark grey (0xff3e3e3e) on white. tools/x8patch.py writes the
masks made here over TASCAM's pixels, at the same addresses and with the same sizes, so no image
descriptor or code changes:

- UPPER, 124 x 21, in place of "TASCAM": the bat from the Oversample logo (about-icon.png in the
  Oversample repository, the same drawing as BAT DETECTOR's launcher glyph), 21 pixels high and
  centred. Each pixel's alpha is the logo's opacity times its darkness, so the logo's black
  outlines and eyes stay dark and its light ears stay light. The launcher glyph does the
  opposite, because the launcher draws its glyphs in white on a dark background.
- LOWER, 245 x 21, in place of the main tile of "Portacapture X8". The firmware draws this tile
  one pixel left of and above the 243 x 24 image, so the text is kept to columns 1 to 243 and
  rows 1 to 20, which are inside the image. The text is centred on the image's middle column.
- DESCENDER, 6 x 7, the stock image's second tile (the descender of the p in "Portacapture"),
  all zero.

Font: Open Sans Semibold, 20 pixels (Apache License 2.0; Debian package fonts-open-sans).

Needs Pillow and numpy. tools/splash_pixels.py, the output, is committed, so a build does not
depend on the Pillow or FreeType version. `--preview` draws the logo screen from a flash image
(from `x8fw.py unpack`, or the image x8patch.py writes next to the update file) as a PNG.
"""
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

TEXT = "Oversample Edition"
FONT_PIXELS = 20

UPPER_WIDTH, UPPER_HEIGHT = 124, 21
LOWER_WIDTH, LOWER_HEIGHT = 245, 21
DESCENDER_WIDTH, DESCENDER_HEIGHT = 6, 7

# Where the firmware draws each mask on the 320 x 480 screen, and in what colour (V2.03 build
# 217; docs/HOMEBREW.md, "The logo screen"). Only --preview uses these.
SCREEN = (320, 480)
INK = (0x3E, 0x3E, 0x3E)
UPPER_AT = (98, 190)
LOWER_AT = (38, 253)
DESCENDER_AT = (155, 272)
UPPER_ADDRESS, LOWER_ADDRESS, DESCENDER_ADDRESS = 0x8132EA64, 0x812F4F44, 0x81766F5C


def upper_pixels(logo_path):
    logo = np.asarray(Image.open(logo_path).convert("RGBA")).astype(float) / 255
    opacity = logo[..., 3]
    luminance = 0.2126 * logo[..., 0] + 0.7152 * logo[..., 1] + 0.0722 * logo[..., 2]
    shape = Image.fromarray((opacity * 255).astype("uint8"), "L").getbbox()
    mask = Image.fromarray((opacity * (1 - luminance) * 255).round().astype("uint8"), "L").crop(shape)
    width = round(mask.width * UPPER_HEIGHT / mask.height)
    mask = mask.resize((width, UPPER_HEIGHT), Image.BOX)
    out = Image.new("L", (UPPER_WIDTH, UPPER_HEIGHT), 0)
    out.paste(mask, ((UPPER_WIDTH - width) // 2, 0))
    return out.tobytes()


def lower_pixels(font_path):
    font = ImageFont.truetype(font_path, FONT_PIXELS)
    canvas = Image.new("L", (LOWER_WIDTH * 2, LOWER_HEIGHT * 2), 0)
    ImageDraw.Draw(canvas).text((10, 10), TEXT, font=font, fill=255)
    ink = canvas.crop(canvas.getbbox())
    if ink.width > LOWER_WIDTH - 2 or ink.height > LOWER_HEIGHT - 1:
        raise SystemExit(f"the text is {ink.width} x {ink.height} pixels; it must fit in 243 x 20")
    out = Image.new("L", (LOWER_WIDTH, LOWER_HEIGHT), 0)
    out.paste(ink, (1 + (LOWER_WIDTH - 2 - ink.width) // 2, 1 + (LOWER_HEIGHT - 1 - ink.height) // 2))
    return out.tobytes()


def write(target, masks):
    with open(target, "w") as out:
        out.write('"""Alpha masks for the X8\'s logo screen. Written by tools/make_splash.py, which explains them."""\n')
        for name, width, height, pixels in masks:
            assert len(pixels) == width * height
            out.write(f"\n{name}_WIDTH, {name}_HEIGHT = {width}, {height}\n")
            out.write(f"{name} = bytes.fromhex(\n")
            for y in range(height):
                out.write(f'    "{pixels[y * width:(y + 1) * width].hex()}"\n')
            out.write(")\n")


def preview(image_path, target):
    """Draw the logo screen at start-up from a flash image, as the firmware composes it."""
    image = open(image_path, "rb").read()
    flash = lambda address, size: image[address - 0x80000000 + 0x20000:][:size]
    screen = Image.new("RGB", SCREEN, (255, 255, 255))
    ink = Image.new("RGB", SCREEN, INK)
    for address, size, at in (
        (UPPER_ADDRESS, (UPPER_WIDTH, UPPER_HEIGHT), UPPER_AT),
        (LOWER_ADDRESS, (LOWER_WIDTH, LOWER_HEIGHT), LOWER_AT),
        (DESCENDER_ADDRESS, (DESCENDER_WIDTH, DESCENDER_HEIGHT), DESCENDER_AT),
    ):
        alpha = Image.new("L", SCREEN, 0)
        alpha.paste(Image.frombytes("L", size, flash(address, size[0] * size[1])), at)
        screen = Image.composite(ink, screen, alpha)
    screen.save(target)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--preview":
        preview(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 4:
        write(sys.argv[3], [
            ("UPPER", UPPER_WIDTH, UPPER_HEIGHT, upper_pixels(sys.argv[1])),
            ("LOWER", LOWER_WIDTH, LOWER_HEIGHT, lower_pixels(sys.argv[2])),
            ("DESCENDER", DESCENDER_WIDTH, DESCENDER_HEIGHT, bytes(DESCENDER_WIDTH * DESCENDER_HEIGHT)),
        ])
    else:
        sys.exit("usage: make_splash.py LOGO.png FONT.ttf TARGET.py, or make_splash.py --preview IMAGE.bin OUT.png")
