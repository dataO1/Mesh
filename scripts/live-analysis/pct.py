"""RMS and sample-peak percentiles (dBFS) of each WAV file. Usage: pct.py FILE..."""
import sys

import numpy as np
from scipy.io import wavfile


def read_wav(path):
    """Float samples in [-1, 1], shape (frames, channels), and the rate."""
    sr, x = wavfile.read(path)
    scale = {np.dtype("int16"): 32768.0, np.dtype("int32"): 2.0**31, np.dtype("uint8"): 128.0}.get(x.dtype, 1.0)
    x = x.astype(np.float64) / scale
    return sr, (x[:, None] if x.ndim == 1 else x)


for f in sys.argv[1:]:
    sr, x = read_wav(f)
    a = np.abs(x).max(1)
    r = np.sqrt((x**2).mean())
    q = np.percentile(a, [99, 99.9, 99.99, 100])
    print(f"{f.split('/')[-1]}: rms {20 * np.log10(r):+.1f}, p99 {20 * np.log10(q[0]):+.1f}, p99.9 {20 * np.log10(q[1]):+.1f}, p99.99 {20 * np.log10(q[2]):+.1f}, max {20 * np.log10(q[3]):+.1f} dBFS")
