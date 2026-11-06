"""Make the launcher glyph for the bat tiles from the Oversample logo.

    python3 tools/make_glyph.py ~/projects/batchi/about-icon.png crates/x8-patch/src/patch/bat_glyph.rs

The X8's launcher icons are alpha masks that the firmware draws in white (see
work/notes/launcher.md section 7), so the glyph is the logo's shape, with its
dark outlines and eyes left out so the drawing still shows in one colour.
Needs Pillow and numpy.
"""
import sys

import numpy as np
from PIL import Image

WIDTH, HEIGHT = 38, 34
# Luminance below which the logo's outlines and eyes become transparent.
OUTLINE = 0.28


def main(source, target):
    logo = Image.open(source).convert("RGBA")
    rgba = np.asarray(logo).astype(float)
    alpha = rgba[..., 3] / 255
    luminance = (0.2126 * rgba[..., 0] + 0.7152 * rgba[..., 1] + 0.0722 * rgba[..., 2]) / 255
    mask = alpha * np.clip((luminance - OUTLINE) / 0.12, 0, 1)
    shape = Image.fromarray((alpha * 255).astype("uint8"), "L").getbbox()
    glyph = Image.fromarray((mask * 255).astype("uint8"), "L").crop(shape).resize((WIDTH, HEIGHT), Image.LANCZOS)
    pixels = glyph.tobytes()
    with open(target, "w") as out:
        out.write("//! The bat tiles' launcher glyph, an alpha mask from the Oversample logo.\n")
        out.write("//! Written by tools/make_glyph.py.\n\n")
        out.write(f"pub(super) const WIDTH: u32 = {WIDTH};\n")
        out.write(f"pub(super) const HEIGHT: u32 = {HEIGHT};\n")
        out.write(f"pub(super) static PIXELS: [u8; {len(pixels)}] = [\n")
        for y in range(HEIGHT):
            row = pixels[y * WIDTH : (y + 1) * WIDTH]
            out.write("    " + ", ".join(str(p) for p in row) + ",\n")
        out.write("];\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
