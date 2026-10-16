"""Mix analysis of a rendered live-mode WAV: 1/3-octave spectrum and slope,
crest per band, side/mid, transient sharpness and bar-to-bar repetition, per
section of the demo arrangement (bars below; edit SECTIONS for other renders).
Usage: mix.py FILE [BPM]   (BPM default 172; 4/4)
"""
import sys

import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, hilbert, sosfiltfilt, welch


def read_wav(path):
    """Float samples in [-1, 1], shape (frames, channels), and the rate."""
    sr, x = wavfile.read(path)
    scale = {np.dtype("int16"): 32768.0, np.dtype("int32"): 2.0**31, np.dtype("uint8"): 128.0}.get(x.dtype, 1.0)
    x = x.astype(np.float64) / scale
    return sr, (x[:, None] if x.ndim == 1 else x)


path = sys.argv[1]
sr, x = read_wav(path)
L, R = x[:,0], x[:,1]
M = (L + R) / 2
Sd = (L - R) / 2
bpm = float(sys.argv[2]) if len(sys.argv) > 2 else 172.0
bar = 60 / bpm * 4
def seg(b0, b1): return slice(int(b0*bar*sr), int(b1*bar*sr))
SECTIONS = {
  "drums+bass (bars 4-31)": seg(4, 31),
  "bass only, drums off (32-35)": seg(32.1, 35.9),
  "drums+bass (36-56)": seg(36, 56),
  "all + lead (57-87)": seg(57, 87),
}
centers = 1000 * 2.0 ** (np.arange(-16, 14) / 3)  # 1/3 octaves ~25 Hz..20 kHz
def third_oct(sig):
    f, p = welch(sig, sr, nperseg=16384)
    out = []
    for c in centers:
        lo, hi = c / 2**(1/6), c * 2**(1/6)
        m = (f >= lo) & (f < hi)
        out.append(10*np.log10(p[m].sum() * (f[1]-f[0]) + 1e-20))
    return np.array(out)
print("1/3-octave band levels (dB, relative to the 1 kHz band) and slope:")
for name, s in SECTIONS.items():
    lv = third_oct(M[s]); ref = lv[np.argmin(abs(centers-1000))]
    rel = lv - ref
    sel = (centers >= 100) & (centers <= 10000)
    slope = np.polyfit(np.log2(centers[sel]), lv[sel], 1)[0]
    pick = [31.5, 50, 80, 125, 250, 500, 1000, 2000, 3150, 5000, 8000, 12500, 16000]
    vals = " ".join(f"{int(p)}:{rel[np.argmin(abs(centers-p))]:+.1f}" for p in pick)
    print(f"  {name}: slope {slope:+.2f} dB/oct (100 Hz-10 kHz)\n     {vals}")
def band(sig, lo, hi):
    sos = butter(4, [lo, hi], btype='band', fs=sr, output='sos')
    return sosfiltfilt(sos, sig)
print("\nCrest factor (peak/RMS dB) per band, and mono check (side/mid dB):")
for name, s in SECTIONS.items():
    row = []
    for lo, hi, tag in [(30, 120, "sub/kick"), (120, 500, "low-mid"), (500, 2000, "mid"), (2000, 6000, "2-6k"), (6000, 16000, "air")]:
        b = band(M[s], lo, hi); row.append(f"{tag} {20*np.log10(np.abs(b).max()/np.sqrt((b**2).mean())):.1f}")
    lowside = band(Sd[s], 30, 150); lowmid = band(M[s], 30, 150)
    hiside = band(Sd[s], 2000, 12000); himid = band(M[s], 2000, 12000)
    row.append(f"| side<150Hz {10*np.log10((lowside**2).mean()/(lowmid**2).mean()+1e-12):.1f}")
    row.append(f"side 2-12k {10*np.log10((hiside**2).mean()/(himid**2).mean()+1e-12):.1f}")
    print(f"  {name}: " + ", ".join(row))
# Transient sharpness: 2-6 kHz envelope, ratio of the first 10 ms after onsets to the next 40 ms.
def onset_ratio(sig):
    env = np.abs(hilbert(band(sig, 2000, 6000)))
    k = int(0.002*sr); env = np.convolve(env, np.ones(k)/k, mode='same')
    step = 60 / bpm / 4
    ratios = []
    for i in range(int(len(sig)/sr/step)-1):
        a = int(i*step*sr); w = env[a:a+int(0.06*sr)]
        if len(w) < int(0.06*sr) or w.max() < 1e-3: continue
        pk = w[:int(0.012*sr)].max(); tail = w[int(0.015*sr):int(0.055*sr)].mean()
        ratios.append(20*np.log10(pk/(tail+1e-9)))
    r = np.array(ratios); return np.median(r), np.percentile(r, 90)
print("\nTransient vs body, 2-6 kHz (onset peak over the following 15-55 ms, dB; median / 90th pct):")
for name, s in SECTIONS.items():
    m, p90 = onset_ratio(M[s]); print(f"  {name}: {m:.1f} / {p90:.1f}")
# Repetition: bar-to-bar similarity of the band envelope (0..1) at lags of 1, 2, 4 bars.
def repetition(sig, lo, hi, b0, b1):
    env = np.abs(hilbert(band(sig, lo, hi)))
    n = int(bar*sr/64)  # 64 slots per bar
    bars = []
    for b in range(b0, b1):
        a = int(b*bar*sr); e = env[a:a+64*n].reshape(64, n).mean(1); bars.append(e/ (np.linalg.norm(e)+1e-9))
    bars = np.array(bars)
    out = {}
    for lag in (1, 2, 4):
        sims = [float(bars[i] @ bars[i+lag]) for i in range(len(bars)-lag)]
        out[lag] = np.mean(sims)
    return out
print("\nBar-to-bar repetition (cosine similarity of the onset envelope at lags 1/2/4 bars):")
for name, (lo, hi, b0, b1) in {"bass <250 Hz (bars 36-56)": (35, 250, 36, 56), "bass <250 Hz (bars 57-87)": (35, 250, 57, 87), "lead band 600-3k, lead on (57-87)": (600, 3000, 57, 87), "same band, lead off (36-56)": (600, 3000, 36, 56)}.items():
    r = repetition(M, lo, hi, b0, b1); print(f"  {name}: " + " ".join(f"{k}:{v:.2f}" for k, v in r.items()))
