"""Shared helpers for the reference and melody analysis scripts: audio
decoding (WAV via soundfile, anything else via ffmpeg), BS.1770 loudness,
tempo and beat-phase estimation, output paths."""
import json
import os
import re
import subprocess
import sys

import numpy as np
from scipy.signal import lfilter, resample_poly

REF_ROOT = os.environ.get("LIVE_REF_ROOT", os.path.expanduser("~/Music/mesh-live-dev/references"))


def set_name(path):
    """Set a file belongs to: its parent directory name (recordings are 'mesh')."""
    d = os.path.basename(os.path.dirname(os.path.abspath(path)))
    return "mesh" if d == "mesh-recordings" else d


def base_name(path):
    return os.path.splitext(os.path.basename(path))[0]


def out_dir(set_, out=None):
    d = out or os.path.join(REF_ROOT, "analysis", set_)
    os.makedirs(d, exist_ok=True)
    return d


def read_audio(path, sr=44100):
    """Float64 stereo (frames, 2) and the rate. WAV keeps its rate, others
    are decoded by ffmpeg to `sr`."""
    if path.lower().endswith((".wav", ".flac")):
        import soundfile as sf
        x, r = sf.read(path, dtype="float64", always_2d=True)
    else:
        raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-f", "f32le", "-ac", "2", "-ar", str(sr), "-"],
                             check=True, stdout=subprocess.PIPE).stdout
        x, r = np.frombuffer(raw, dtype="<f4").astype(np.float64).reshape(-1, 2), sr
    if x.shape[1] == 1:
        x = np.repeat(x, 2, axis=1)
    return x[:, :2], r


def db(v, floor=1e-12):
    return 10 * np.log10(np.maximum(v, floor))


def k_weight(x, fs):
    """ITU-R BS.1770 K-weighting (high shelf + high pass), x shape (n, ch)."""
    G, Q, fc = 3.999843853973347, 0.7071752369554196, 1681.9744509555319
    K = np.tan(np.pi * fc / fs); Vh = 10 ** (G / 20); Vb = Vh ** 0.4996667741545416
    a0 = 1 + K / Q + K * K
    b1 = [(Vh + Vb * K / Q + K * K) / a0, 2 * (K * K - Vh) / a0, (Vh - Vb * K / Q + K * K) / a0]
    a1 = [1, 2 * (K * K - 1) / a0, (1 - K / Q + K * K) / a0]
    Q2, fc2 = 0.5003270373253953, 38.13547087613982
    K = np.tan(np.pi * fc2 / fs); a0 = 1 + K / Q2 + K * K
    b2 = [1, -2, 1]; a2 = [1, 2 * (K * K - 1) / a0, (1 - K / Q2 + K * K) / a0]
    return lfilter(b2, a2, lfilter(b1, a1, x, axis=0), axis=0)


def window_loudness(msq_cum, fs, win_s, hop_s, n):
    """Loudness (LUFS) of sliding windows from the cumulative K-weighted power sum."""
    w, h = int(win_s * fs), int(hop_s * fs)
    starts = np.arange(0, n - w + 1, h)
    z = (msq_cum[starts + w] - msq_cum[starts]) / w
    return -0.691 + db(z.sum(axis=1)), starts


def loudness_stats(xk, fs):
    """Integrated LUFS (gated), LRA (LU) and a cumulative power sum for reuse."""
    n = len(xk)
    cum = np.vstack([np.zeros((1, xk.shape[1])), np.cumsum(xk ** 2, axis=0)])
    out = {"cum": cum}
    if n < int(0.4 * fs):
        out.update(lufs=float("nan"), lra=float("nan")); return out
    # 400 ms blocks, 75 % overlap
    w, h = int(0.4 * fs), int(0.1 * fs)
    starts = np.arange(0, n - w + 1, h)
    z = (cum[starts + w] - cum[starts]) / w
    lb = -0.691 + db(z.sum(axis=1))
    keep = lb > -70
    if keep.any():
        rel = -0.691 + db(z[keep].sum(axis=1).mean()) - 10
        sel = lb > max(-70, rel)
        out["lufs"] = float(-0.691 + db(z[sel].sum(axis=1).mean())) if sel.any() else float("nan")
    else:
        out["lufs"] = float("nan")
    st, _ = window_loudness(cum, fs, 3.0, 1.0, n)
    st = st[st > -70]
    if len(st) > 2:
        rel = -0.691 + db(np.mean(10 ** ((st + 0.691) / 10))) - 20
        st = st[st > rel]
    out["lra"] = float(np.percentile(st, 95) - np.percentile(st, 10)) if len(st) > 2 else float("nan")
    return out


def true_peak_db(x):
    """4x oversampled peak (dBFS) over channels."""
    return float(20 * np.log10(max(np.abs(resample_poly(x[:, c], 4, 1)).max() for c in range(x.shape[1])) + 1e-12))


def onset_env(y, sr, hop=256):
    import librosa
    return librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)


def estimate_tempo(y, sr, lo=150.0, hi=200.0, hop=256):
    """Tempo (BPM, folded into [lo, hi)) and beat phase (s) from a comb on the
    onset envelope. Returns dict(tempo, phase, tempo_librosa, confidence)."""
    import librosa
    env = onset_env(y, sr, hop)
    env = env - env.mean()
    t = np.arange(len(env)) * hop / sr
    raw = float(librosa.feature.tempo(onset_envelope=env.clip(0), sr=sr, hop_length=hop, start_bpm=172)[0])
    fold = raw
    while fold < lo: fold *= 2
    while fold >= hi: fold /= 2

    def score(bpm):
        f = bpm / 60.0
        s = 0.0
        for hmc, wgt in ((1, 1.0), (2, 0.5), (4, 0.5)):
            s += wgt * abs(np.sum(env * np.exp(-2j * np.pi * hmc * f * t)))
        return s
    coarse = np.arange(lo, hi, 0.1)
    sc = np.array([score(b) for b in coarse])
    best = coarse[int(np.argmax(sc))]
    fine = np.arange(best - 0.15, best + 0.151, 0.01)
    fs_ = np.array([score(b) for b in fine])
    bpm = float(fine[int(np.argmax(fs_))])
    f = bpm / 60.0
    z = np.sum(env * np.exp(-2j * np.pi * f * t))
    phase = float((np.angle(z) / (2 * np.pi * f)) % (1 / f))
    conf = float(fs_.max() / (np.median(sc) + 1e-9))
    return dict(tempo=bpm, phase=phase, tempo_librosa=fold, confidence=conf)


def state_bpm(path):
    """Median BPM from a Mesh recording's .state.tsv, else None."""
    p = os.path.splitext(path)[0] + ".state.tsv"
    if not os.path.exists(p):
        return None
    with open(p) as fh:
        head = fh.readline().rstrip("\n").split("\t")
        if "bpm" not in head:
            return None
        i = head.index("bpm")
        v = [float(l.split("\t")[i]) for l in fh if l.strip()]
    return float(np.median(v)) if v else None


def dump(obj, path):
    with open(path, "w") as fh:
        json.dump(obj, fh, indent=1, default=lambda o: o.item() if hasattr(o, "item") else str(o))


def clean(v):
    if isinstance(v, float) and not np.isfinite(v):
        return None
    return v


def state_table(path):
    """Mesh recording state log as dict of column -> list of str, or None."""
    p = os.path.splitext(path)[0] + ".state.tsv"
    if not os.path.exists(p):
        return None
    with open(p) as fh:
        head = fh.readline().rstrip("\n").split("\t")
        rows = [l.rstrip("\n").split("\t") for l in fh if l.strip()]
    return {h: [r[i] for r in rows if len(r) > i] for i, h in enumerate(head)}


def mesh_window(path, bars=16):
    """Start time (s) and bar of a stretch of `bars` bars with drums, bass and
    lead on (best effort), from the state log. None without a log."""
    t = state_table(path)
    if not t or "lead.on" not in t:
        return None
    on = np.array([int(a) + int(b) + int(c) for a, b, c in zip(t["drums.on"], t["bass.on"], t["lead.on"])])
    tm = [float(v) for v in t["time_s"]]
    if len(on) < bars:
        return None
    sc = np.convolve(on, np.ones(bars), mode="valid")
    i = int(np.argmax(sc))
    return dict(start_s=tm[i], start_bar=i, full=bool(sc[i] == 3 * bars))
