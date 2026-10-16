"""Reference analysis: tempo, loudness (LUFS, LRA, PLR), spectrum, stereo
width, crest per band and an 8-bar structure summary, one JSON per track.
Usage: ref.py [--out DIR] FILE...   (JSON: DIR or
$LIVE_REF_ROOT/analysis/<set>/<name>.json; set = parent directory name)
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from scipy.signal import butter, resample_poly, sosfiltfilt, spectrogram, welch

import _common as C

CENTERS = 1000 * 2.0 ** (np.arange(-16, 14) / 3)
PICK = [31.5, 50, 80, 125, 250, 500, 1000, 2000, 3150, 5000, 8000, 12500, 16000]


def bandpass(x, lo, hi, fs):
    return sosfiltfilt(butter(4, [lo, min(hi, fs * 0.49)], btype="band", fs=fs, output="sos"), x)


def third_octave(sig, fs):
    f, p = welch(sig, fs, nperseg=16384)
    out = []
    for c in CENTERS:
        m = (f >= c / 2 ** (1 / 6)) & (f < c * 2 ** (1 / 6))
        out.append(10 * np.log10(p[m].sum() * (f[1] - f[0]) + 1e-20))
    return np.array(out)


def structure(M, fs, cum, bpm, phase):
    bar = 240.0 / bpm
    seg_bars = 8
    off = phase % (60.0 / bpm)
    dur = len(M) / fs
    nbars = (dur - off) / bar
    starts = []
    b = 0
    while b + 4 <= nbars:
        starts.append((b, min(seg_bars, int(nbars - b))))
        b += seg_bars
    if len(starts) < 2:
        return None
    f, t, S = spectrogram(M, fs, nperseg=4096, noverlap=2048, mode="psd")
    bands = {"low": (20, 150), "mid": (150, 2000), "high": (2000, 16000)}
    bp = {k: S[(f >= lo) & (f < hi)].sum(axis=0) for k, (lo, hi) in bands.items()}
    segs = []
    for b0, nb in starts:
        a, e = off + b0 * bar, off + (b0 + nb) * bar
        ia, ie = int(a * fs), min(int(e * fs), len(M))
        z = (cum[ie] - cum[ia]) / max(ie - ia, 1)
        m = (t >= a) & (t < e)
        d = dict(start_bar=b0, bars=nb, lufs=float(-0.691 + C.db(z.sum())))
        for k in bands:
            d[k + "_db"] = float(C.db(bp[k][m].mean())) if m.any() else -120.0
        segs.append(d)
    L = np.array([s["lufs"] for s in segs]); low = np.array([s["low_db"] for s in segs])
    F = np.percentile(L, 80)
    high = (L >= F - 2.5) & (low >= low.max() - 6)
    lab = []
    fh = int(np.argmax(high)) if high.any() else None
    lh = len(high) - 1 - int(np.argmax(high[::-1])) if high.any() else None
    for i in range(len(segs)):
        if fh is None:
            lab.append("intro" if i == 0 else "breakdown")
        elif high[i]:
            lab.append("drop")
        elif i < fh:
            lab.append("build" if (i == fh - 1 and i > 0 and L[i] - L[i - 1] >= 1) else "intro")
        elif i > lh:
            lab.append("outro")
        else:
            lab.append("breakdown")
    # last segment of a breakdown run that rises into the next drop is a build
    for i in range(1, len(segs) - 1):
        if lab[i] == "breakdown" and lab[i + 1] == "drop" and lab[i - 1] == "breakdown" and L[i] - L[i - 1] >= 1:
            lab[i] = "build"
    for s, l in zip(segs, lab):
        s["label"] = l
    jumps, edge = [], []
    for i in range(1, len(segs)):
        if lab[i] == "drop" and lab[i - 1] != "drop":
            jumps.append(float(L[i] - L[i - 1]))
            a = off + segs[i]["start_bar"] * bar
            def ms(x0, x1):
                i0, i1 = int(x0 * fs), int(x1 * fs)
                return float(-0.691 + C.db(((cum[i1] - cum[i0]) / max(i1 - i0, 1)).sum()))
            edge.append(ms(a, a + 4 * bar) - ms(max(a - 4 * bar, 0), a))
    n_bars = int(nbars)
    res = dict(segments=segs, n_bars=n_bars)
    m = {"n_bars": n_bars, "n_drops": len(jumps) + (1 if lab[0] == "drop" else 0)}
    for k in ("intro", "build", "drop", "breakdown", "outro"):
        m["bars_" + k] = int(sum(s["bars"] for s in segs if s["label"] == k))
    m["first_drop_bar"] = int(segs[lab.index("drop")]["start_bar"]) if "drop" in lab else None
    m["drop_jump_lu"] = jumps[0] if jumps else None
    m["drop_jump_max_lu"] = max(jumps) if jumps else None
    m["drop_jump_edge_lu"] = edge[0] if edge else None
    m["seg_lufs_range"] = float(L.max() - L.min())
    runs, i = [], 0
    while i < len(lab):
        if lab[i] == "drop":
            j = i
            while j + 1 < len(lab) and lab[j + 1] == "drop": j += 1
            runs.append((sum(segs[k]["bars"] for k in range(i, j + 1)), float(np.mean(L[i:j + 1])), i)); i = j + 1
        else:
            i += 1
    if runs:
        r = max(runs, key=lambda q: (q[0], q[1]))
        m["main_drop_start_bar"] = int(segs[r[2]]["start_bar"]); m["main_drop_bars"] = int(r[0])
        m["main_drop_start_s"] = float(off + segs[r[2]]["start_bar"] * bar)
    for k in ("low", "mid", "high"):
        v = [s[k + "_db"] for s in segs if s["label"] == "drop"]
        m[f"drop_{k}_db"] = float(np.mean(v)) if v else None
    res["metrics"] = m
    return res


def analyse(path):
    t0 = time.time()
    x, fs = C.read_audio(path)
    M = x.mean(axis=1); Sd = (x[:, 0] - x[:, 1]) / 2
    dur = len(x) / fs
    mono22 = resample_poly(M, 1, 2) if fs == 44100 else M
    sr_t = fs // 2 if fs == 44100 else fs
    tem = C.estimate_tempo(mono22, sr_t)
    bpm = tem["tempo"]
    sb = C.state_bpm(path)
    stt = C.state_table(path)
    if sb and stt:
        tem["phase"] = float(stt["time_s"][0]) % (60.0 / sb)
    tempo_src = "estimate"
    if sb:
        bpm, tempo_src = sb, "state.tsv"
    ls = C.loudness_stats(C.k_weight(x, fs), fs)
    tp = C.true_peak_db(x)
    m = {"duration_s": dur, "tempo": bpm, "tempo_librosa": tem["tempo_librosa"], "tempo_confidence": tem["confidence"],
         "lufs": ls["lufs"], "lra": ls["lra"], "true_peak_db": tp, "plr": tp - ls["lufs"]}
    lv = third_octave(M, fs); rel = lv - lv[np.argmin(abs(CENTERS - 1000))]
    sel = (CENTERS >= 100) & (CENTERS <= 10000)
    m["tilt_db_oct"] = float(np.polyfit(np.log2(CENTERS[sel]), lv[sel], 1)[0])
    for p in PICK:
        m[f"rel_{p:g}Hz"] = float(rel[np.argmin(abs(CENTERS - p))])
    for tag, lo, hi in (("low", 20, 150), ("mid", 150, 2000), ("high", 2000, 12000)):
        s, mm = bandpass(Sd, lo, hi, fs), bandpass(M, lo, hi, fs)
        m[f"width_{tag}_db"] = float(10 * np.log10((s ** 2).mean() / ((mm ** 2).mean() + 1e-20) + 1e-12))
    for tag, lo, hi in (("sub", 30, 120), ("lowmid", 120, 500), ("mid", 500, 2000), ("2-6k", 2000, 6000), ("air", 6000, 16000)):
        b = bandpass(M, lo, hi, fs)
        m[f"crest_{tag}_db"] = float(20 * np.log10(np.abs(b).max() / (np.sqrt((b ** 2).mean()) + 1e-12)))
    st = structure(M, fs, ls["cum"], bpm, tem["phase"])
    out = {"file": os.path.abspath(path), "set": C.set_name(path), "tempo_source": tempo_src, "beat_phase_s": tem["phase"]}
    if st:
        m.update({"s_" + k: v for k, v in st["metrics"].items()})
        for k in ("main_drop_start_s", "main_drop_start_bar", "main_drop_bars"):
            if "s_" + k in m: m[k] = m.pop("s_" + k)
        out["structure"] = st["segments"]
    mw = C.mesh_window(path)
    if mw:
        m["main_drop_start_s"] = mw["start_s"]; m["main_drop_start_bar"] = mw["start_bar"]; m["main_drop_bars"] = 16
        out["main_drop_source"] = "state.tsv" + ("" if mw["full"] else " (best partial)")
    m = {k: C.clean(float(v) if isinstance(v, (np.floating, float)) else v) for k, v in m.items()}
    out["metrics"] = m
    out["main_drop_start_s"] = m.get("main_drop_start_s")
    out["runtime_s"] = round(time.time() - t0, 1)
    return out


def main():
    args = sys.argv[1:]
    outd = None
    if args and args[0] == "--out":
        outd, args = args[1], args[2:]
    for p in args:
        try:
            r = analyse(p)
            d = C.out_dir(r["set"], outd)
            C.dump(r, os.path.join(d, C.base_name(p) + ".json"))
            m = r["metrics"]
            print(f"{C.base_name(p)}: {m['tempo']:.1f} BPM, {m['lufs']:.1f} LUFS, PLR {m['plr']:.1f}, "
                  f"drop jump {m.get('s_drop_jump_lu')}, {r['runtime_s']} s")
        except Exception as e:
            import traceback; traceback.print_exc()
            print(f"FAILED {p}: {e}", file=sys.stderr)


main()
