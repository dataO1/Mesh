"""Peak, RMS and crest factor (dBFS) per frequency band of a WAV file (mono sum).
Usage: bandpk.py FILE
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
m = x.mean(1)
for lo, hi, tag in [(20, 120, "kick/sub"), (120, 400, "snare body"), (400, 2000, "mid"), (2000, 6000, "click 2-6k"), (6000, 18000, "hats/air")]:
    b = sosfiltfilt(butter(4, [lo, hi], btype="band", fs=sr, output="sos"), m)
    pk = np.abs(b).max()
    rms = np.sqrt((b**2).mean())
    print(f"  {tag}: peak {20 * np.log10(pk):+.1f} dBFS, rms {20 * np.log10(rms):+.1f}, crest {20 * np.log10(pk / rms):.1f}")
