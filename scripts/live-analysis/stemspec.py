"""Spectrum and crest factor of a separated reference track: sums
PREFIX_(*)_htdemucs_ft.wav stems (audio-separator output), takes 40 s from a
quarter in, prints the 1/3-octave levels relative to 1 kHz, the slope and the
crest per band. Usage: stemspec.py PREFIX...
"""
import glob
import sys

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, sosfiltfilt, welch


def read_wav(path):
    """Float samples in [-1, 1], shape (frames, channels), and the rate."""
    sr, x = wavfile.read(path)
    scale = {np.dtype("int16"): 32768.0, np.dtype("int32"): 2.0**31, np.dtype("uint8"): 128.0}.get(x.dtype, 1.0)
    x = x.astype(np.float64) / scale
    return sr, (x[:, None] if x.ndim == 1 else x)


CENTERS = 1000 * 2.0 ** (np.arange(-16, 14) / 3)  # 1/3 octaves ~25 Hz..20 kHz
PICK = [31.5, 50, 80, 125, 250, 500, 1000, 2000, 3150, 5000, 8000, 12500, 16000]


def band(sig, sr, lo, hi):
    return sosfiltfilt(butter(4, [lo, hi], btype="band", fs=sr, output="sos"), sig)


for title in sys.argv[1:]:
    files = sorted(glob.glob(title + "_(*)_htdemucs_ft.wav"))
    if not files:
        print(f"{title}: no stems found")
        continue
    mix = None
    for f in files:
        sr, x = read_wav(f)
        mix = x if mix is None else mix[:len(x)] + x[:len(mix)]
    M = mix.mean(1)
    n = len(M)
    M = M[n // 4:n // 4 + int(40 * sr)]  # 40 s from a quarter in (main section)
    f, p = welch(M, sr, nperseg=16384)
    lv = np.array([10 * np.log10(p[(f >= c / 2 ** (1 / 6)) & (f < c * 2 ** (1 / 6))].sum() * (f[1] - f[0]) + 1e-20) for c in CENTERS])
    rel = lv - lv[np.argmin(abs(CENTERS - 1000))]
    sel = (CENTERS >= 100) & (CENTERS <= 10000)
    slope = np.polyfit(np.log2(CENTERS[sel]), lv[sel], 1)[0]
    print(f"{title.split('/')[-1]} ({len(files)} stems): slope {slope:+.2f} dB/oct")
    print("   " + " ".join(f"{int(q)}:{rel[np.argmin(abs(CENTERS - q))]:+.1f}" for q in PICK))
    row = []
    for lo, hi, tag in [(30, 120, "sub/kick"), (120, 500, "low-mid"), (500, 2000, "mid"), (2000, 6000, "2-6k"), (6000, 16000, "air")]:
        b = band(M, sr, lo, hi)
        row.append(f"{tag} {20 * np.log10(np.abs(b).max() / np.sqrt((b**2).mean())):.1f}")
    print("   crest: " + ", ".join(row) + f", broadband {20 * np.log10(np.abs(M).max() / np.sqrt((M**2).mean())):.1f}")
