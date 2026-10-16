"""Loudest 2-6 kHz peaks of a WAV file with their bar position, width and
sample jump (finds clicks and harsh transients).
Usage: spikes.py FILE [BPM]   (BPM default 172; 4/4)
"""
import sys

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, sosfiltfilt


def read_wav(path):
    """Float samples in [-1, 1], shape (frames, channels), and the rate."""
    sr, x = wavfile.read(path)
    scale = {np.dtype("int16"): 32768.0, np.dtype("int32"): 2.0**31, np.dtype("uint8"): 128.0}.get(x.dtype, 1.0)
    x = x.astype(np.float64) / scale
    return sr, (x[:, None] if x.ndim == 1 else x)


sr, x = read_wav(sys.argv[1])
bpm = float(sys.argv[2]) if len(sys.argv) > 2 else 172.0
m = x.mean(1)
b = sosfiltfilt(butter(4, [2000, 6000], btype="band", fs=sr, output="sos"), m)
a = np.abs(b)
rms = np.sqrt((b**2).mean())
seen = []
bar = 60 / bpm * 4
for i in np.argsort(a)[::-1]:
    if all(abs(i - j) > sr * 0.05 for j in seen):
        seen.append(i)
        # Width: samples above half the peak around i.
        w = a[max(0, i - 2000):i + 2000]
        width = (w > a[i] / 2).sum()
        # Raw sample jump at i.
        jump = np.abs(np.diff(m[max(0, i - 5):i + 5])).max()
        t = i / sr
        print(f"t={t:6.3f}s bar {t / bar:6.2f} step {((t / bar) % 1) * 16:5.2f}: band peak {20 * np.log10(a[i]):+.1f} dBFS ({20 * np.log10(a[i] / rms):.0f} dB over rms), half-width {width / sr * 1000:.2f} ms, max sample jump {jump:.3f}")
    if len(seen) >= 8:
        break
print("median sample jump", np.median(np.abs(np.diff(m))))
