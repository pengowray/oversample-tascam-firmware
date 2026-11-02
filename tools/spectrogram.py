# High-resolution spectrogram of an excerpt of a WAV file, for looking at call
# shapes: mono mix, 0 to 96 kHz, 375 Hz per row and 1/3 ms per column (a
# 512-point FFT every 64 samples at 192 kHz). Other sample rates get the FFT
# size and hop scaled to keep the same scale, so recordings at 96, 192 or
# 384 kHz can be compared side by side; above a 96 kHz file's 48 kHz limit the
# image is black. Reads only the excerpt, so it works on multi-gigabyte X8
# recordings. 16, 24 and 32-bit integer and 32-bit float WAVs.
#
# Usage: python3 tools/spectrogram.py IN.wav START_S DURATION_S OUT.png
# Needs numpy and Pillow.
import sys, struct
import numpy as np
from PIL import Image, ImageDraw
path, start, dur, out = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), sys.argv[4]
f = open(path, 'rb'); head = f.read(65536)
i = 12; fmt = None
while i < len(head) - 8:
    cid = head[i:i+4]; sz = struct.unpack('<I', head[i+4:i+8])[0]
    if cid == b'fmt ': fmt = struct.unpack('<HHIIHH', head[i+8:i+24])
    if cid == b'data': off = i + 8; break
    i += 8 + sz + (sz & 1)
tag, ch, rate, _, block, bits = fmt
f.seek(off + int(start * rate) * block)
raw = f.read(int(dur * rate) * block)
raw = raw[:len(raw)//block*block]
if tag == 3:
    x = np.frombuffer(raw, dtype=np.float32)
elif bits == 24:
    b = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
    x = (b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)) << 8 >> 8   # sign-extend
else:
    x = np.frombuffer(raw, dtype=np.int16 if bits == 16 else np.int32)
x = x.reshape(-1, ch).astype(np.float64).mean(axis=1)
ROWS, HZ_PER_ROW, COL_S = 256, 375.0, 64 / 192000
N = int(round(rate / HZ_PER_ROW)); hop = int(round(rate * COL_S))
win = np.hanning(N)
cols = [np.abs(np.fft.rfft(x[k:k+N] * win)) for k in range(0, len(x) - N, hop)]
S = 20 * np.log10(np.array(cols).T + 1e-12)          # bins x time
floor = np.median(S[1:min(ROWS, len(S))])
if len(S) < ROWS:                                     # below 192 kHz: pad
    S = np.vstack([S, np.full((ROWS - len(S), S.shape[1]), floor - 100)])
S = S[:ROWS][::-1]                                    # 0..96 kHz, high at top
img = np.clip((S - floor - 3) / 50, 0, 1)
rgb = (np.stack([img**0.6, img**1.2, img**3], -1) * 255).astype(np.uint8)
im = Image.fromarray(rgb).resize((min(1600, rgb.shape[1]), 512), Image.NEAREST)
d = ImageDraw.Draw(im)
for k in range(10, 100, 10):
    y = int(512 * (1 - k / 96)); d.line([(0, y), (12, y)], fill=(90, 160, 255)); d.text((14, y - 6), f'{k}', fill=(90, 160, 255))
d.text((4, 498), f'{start:.2f} s + {dur} s   (kHz on the left)', fill=(255, 255, 255))
im.save(out)
