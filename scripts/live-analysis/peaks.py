"""Peak, RMS and crest factor (dBFS) of each WAV file. Usage: peaks.py FILE..."""
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
    m = x.mean(1)
    pk = 20 * np.log10(np.abs(x).max())
    rms = 20 * np.log10(np.sqrt((m**2).mean()))
    print(f"{f.split('/')[-1]}: peak {pk:+.1f} dBFS, rms {rms:+.1f}, crest {pk - rms:.1f}")
