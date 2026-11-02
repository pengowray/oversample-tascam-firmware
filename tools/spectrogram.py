# High-resolution spectrogram of an excerpt of a WAV file, for looking at call
# shapes: mono mix, 512-point FFT every 64 samples, 0 to 96 kHz at 192 kHz.
# Reads only the excerpt, so it works on multi-gigabyte X8 recordings.
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
dt = np.float32 if tag == 3 else (np.int16 if bits == 16 else np.int32)
x = np.frombuffer(raw[:len(raw)//block*block], dtype=dt).reshape(-1, ch).astype(np.float64).mean(axis=1)
N, hop = 512, 64
win = np.hanning(N)
cols = [np.abs(np.fft.rfft(x[k:k+N] * win)) for k in range(0, len(x) - N, hop)]
S = 20 * np.log10(np.array(cols).T + 1e-12)          # bins x time
S = S[:N//2][::-1]                                    # 0..96 kHz, high at top
floor = np.median(S); img = np.clip((S - floor - 3) / 50, 0, 1)
rgb = (np.stack([img**0.6, img**1.2, img**3], -1) * 255).astype(np.uint8)
im = Image.fromarray(rgb).resize((min(1600, rgb.shape[1]), 512), Image.NEAREST)
d = ImageDraw.Draw(im)
for k in range(10, 100, 10):
    y = int(512 * (1 - k / 96)); d.line([(0, y), (12, y)], fill=(90, 160, 255)); d.text((14, y - 6), f'{k}', fill=(90, 160, 255))
d.text((4, 498), f'{start:.2f} s + {dur} s   (kHz on the left)', fill=(255, 255, 255))
im.save(out)
