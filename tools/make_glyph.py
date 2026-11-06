"""Make the launcher glyphs for the bat tiles.

    python3 tools/make_glyph.py bat ~/projects/batchi/about-icon.png crates/x8-patch/src/patch/bat_glyph.rs
    python3 tools/make_glyph.py play crates/x8-patch/src/patch/play_glyph.rs

The X8's launcher icons are alpha masks that the firmware draws in white (see
work/notes/launcher.md section 7). The bat for BAT DETECTOR is the Oversample
logo's shape, with its dark outlines and eyes left out so the drawing still shows
in one colour. The play button for BAT PLAYBACK is a solid triangle.
Needs Pillow and numpy.
"""
import sys

import numpy as np
from PIL import Image

# The launcher's icon height and width hooks give one size for both tiles.
WIDTH, HEIGHT = 38, 34
# Luminance below which the logo's outlines and eyes become transparent.
OUTLINE = 0.28

# The play button's triangle, in pixels. The left edge is on a whole pixel, so it stays
# sharp. It is further from the canvas's left side than the tip is from its right side,
# because a triangle centred by its outer edges looks too far left: most of its area is
# near the left edge.
PLAY_LEFT, PLAY_TIP = 9.0, 35.0
PLAY_HEIGHT = 30.0
PLAY_CORNER_RADIUS = 1.5
OVERSAMPLE = 8


def bat_pixels(source):
    logo = Image.open(source).convert("RGBA")
    rgba = np.asarray(logo).astype(float)
    alpha = rgba[..., 3] / 255
    luminance = (0.2126 * rgba[..., 0] + 0.7152 * rgba[..., 1] + 0.0722 * rgba[..., 2]) / 255
    mask = alpha * np.clip((luminance - OUTLINE) / 0.12, 0, 1)
    shape = Image.fromarray((alpha * 255).astype("uint8"), "L").getbbox()
    glyph = Image.fromarray((mask * 255).astype("uint8"), "L").crop(shape).resize((WIDTH, HEIGHT), Image.LANCZOS)
    return glyph.tobytes()


def play_pixels(left=PLAY_LEFT, tip=PLAY_TIP, height=PLAY_HEIGHT, radius=PLAY_CORNER_RADIUS):
    """A right-pointing triangle with rounded corners, centred vertically."""
    middle = HEIGHT / 2
    top, bottom = middle - height / 2, middle + height / 2
    corners = np.array([(left, top), (tip, middle), (left, bottom)])
    # Sample centres on a grid OVERSAMPLE times finer than the pixels.
    xs = (np.arange(WIDTH * OVERSAMPLE) + 0.5) / OVERSAMPLE
    ys = (np.arange(HEIGHT * OVERSAMPLE) + 0.5) / OVERSAMPLE
    x, y = np.meshgrid(xs, ys)
    # Inset the triangle by the radius. A point is inside the rounded shape if it is
    # within the radius of the inset triangle.
    centre = corners.mean(axis=0)
    edges = np.roll(corners, -1, axis=0) - corners
    normals = np.stack([edges[:, 1], -edges[:, 0]], axis=1)
    normals /= np.linalg.norm(normals, axis=1, keepdims=True)
    offsets = (normals * corners).sum(axis=1)
    # Point every normal outwards.
    flip = (normals * centre).sum(axis=1) > offsets
    normals[flip] *= -1
    offsets[flip] *= -1
    inset_offsets = offsets - radius
    inset = np.array(
        [
            np.linalg.solve(np.stack([normals[i - 1], normals[i]]), [inset_offsets[i - 1], inset_offsets[i]])
            for i in range(3)
        ]
    )
    distance = np.full(x.shape, np.inf)
    for i in range(3):
        a, b = inset[i], inset[(i + 1) % 3]
        edge = b - a
        t = np.clip(((x - a[0]) * edge[0] + (y - a[1]) * edge[1]) / (edge @ edge), 0, 1)
        distance = np.minimum(distance, np.hypot(x - a[0] - t * edge[0], y - a[1] - t * edge[1]))
    side = np.stack([normals[i, 0] * x + normals[i, 1] * y - inset_offsets[i] for i in range(3)])
    inside = (side <= 0).all(axis=0)
    mask = inside | (distance <= radius)
    coverage = mask.reshape(HEIGHT, OVERSAMPLE, WIDTH, OVERSAMPLE).mean(axis=(1, 3))
    return (coverage * 255).round().astype("uint8").tobytes()


def write(target, about, pixels):
    with open(target, "w") as out:
        out.write(f"//! {about}\n")
        out.write("//! Written by tools/make_glyph.py.\n\n")
        out.write(f"pub(super) const WIDTH: u32 = {WIDTH};\n")
        out.write(f"pub(super) const HEIGHT: u32 = {HEIGHT};\n")
        out.write(f"pub(super) static PIXELS: [u8; {len(pixels)}] = [\n")
        for y in range(HEIGHT):
            row = pixels[y * WIDTH : (y + 1) * WIDTH]
            out.write("    " + ", ".join(str(p) for p in row) + ",\n")
        out.write("];\n")


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "bat":
        write(sys.argv[3], "BAT DETECTOR's launcher glyph, an alpha mask from the Oversample logo.", bat_pixels(sys.argv[2]))
    elif len(sys.argv) == 3 and sys.argv[1] == "play":
        write(sys.argv[2], "BAT PLAYBACK's launcher glyph, an alpha mask of a play button.", play_pixels())
    else:
        sys.exit("usage: make_glyph.py bat LOGO.png TARGET.rs, or make_glyph.py play TARGET.rs")
