"""Monophonic melody statistics of a separated stem (pyin pitch track).
Usage: melody.py STEM_WAV... [BPM]   (a trailing number is the BPM; without
it the tempo is read from the track's ref JSON or estimated from the stem).
The stem kind comes from the file name: _(Bass)_ -> fmin E1, otherwise E2.
JSON: <analysis>/<set>/<name>.melody-<kind>.json (stem path .../stems/<set>/...)
or next to the stem.
"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import json

import numpy as np

import _common as C

SR = 22050
HOP = 256


def pitch_track(y, kind):
    import librosa
    fmin = librosa.note_to_hz("E1" if kind == "bass" else "E2")
    fmax = librosa.note_to_hz("E4" if kind == "bass" else "C6")
    f0, vf, vp = librosa.pyin(y, fmin=fmin, fmax=fmax, sr=SR, frame_length=4096 if kind == "bass" else 2048,
                              hop_length=HOP, fill_na=np.nan)
    rms = librosa.feature.rms(y=y, frame_length=2048, hop_length=HOP, center=True)[0]
    n = min(len(f0), len(rms))
    f0, vf, vp, rms = f0[:n], vf[:n], vp[:n], rms[:n]
    gate = rms > rms.max() * 10 ** (-45 / 20)
    voiced = vf & gate & np.isfinite(f0)
    return f0, voiced, vp, gate


def segment_notes(f0, voiced, min_s):
    """Notes (onset_s, dur_s, midi) from the pitch track: split on unvoiced
    gaps of >= 2 frames and on pitch jumps of >= 0.8 semitone."""
    fr = HOP / SR
    midi = np.where(voiced, 69 + 12 * np.log2(np.where(voiced, f0, 440.0) / 440.0), np.nan)
    v = voiced.copy()
    i, n = 0, len(v)
    while i < n:                                  # bridge single-frame gaps
        if not v[i]:
            j = i
            while j < n and not v[j]: j += 1
            if j - i == 1 and i > 0 and j < n and abs(midi[i - 1] - midi[j]) < 0.8:
                v[i] = True; midi[i] = (midi[i - 1] + midi[j]) / 2
            i = j
        else:
            i += 1
    notes, cur = [], []
    def flush():
        if cur:
            seg = [midi[k] for k in cur]
            notes.append((cur[0] * fr, len(cur) * fr, float(np.median(seg))))
    for k in range(n):
        if not v[k]:
            flush(); cur = []; continue
        if cur and abs(midi[k] - np.median([midi[q] for q in cur[-6:]])) >= 0.8:
            flush(); cur = []
        cur.append(k)
    flush()
    return [nt for nt in notes if nt[1] >= min_s]


def fit_phase(notes, step):
    """16th-grid phase (s) that best fits the note onsets."""
    if not notes: return 0.0
    on = np.array([n[0] for n in notes])
    ph = np.angle(np.sum(np.exp(2j * np.pi * on / step))) / (2 * np.pi) * step
    return ph % step


def stats(notes, y_len_s, bpm, phase_hint=None):
    beat = 60.0 / bpm; step = beat / 4; bar = beat * 4
    ph = fit_phase(notes, step)
    nbars = int(y_len_s / bar)
    ev = []   # (bar, pos, length16, midi)
    for on, du, md in notes:
        idx = int(round((on - ph) / step))
        if idx < 0: continue
        ev.append((idx // 16, idx % 16, max(1, int(round(du / step))), md))
    # one onset per grid slot (keep the longer)
    slots = {}
    for b, p, l, md in ev:
        k = (b, p)
        if k not in slots or l > slots[k][0]: slots[k] = (l, md)
    ev = sorted((b, p, l, md) for (b, p), (l, md) in slots.items())
    m = {"n_notes": len(ev), "n_bars": nbars}
    if not ev or nbars < 1:
        return m, {}
    bars_idx = np.array([e[0] for e in ev]); pos = np.array([e[1] for e in ev])
    ln = np.array([e[2] for e in ev]); md = np.array([e[3] for e in ev])
    nb = np.bincount(np.clip(bars_idx, 0, nbars - 1), minlength=nbars)[:nbars]
    # active share: note-covered time per bar
    cov = np.zeros(nbars)
    for on, du, _ in notes:
        a, e = on - ph, on - ph + du
        for b in range(max(int(a // bar), 0), min(int(e // bar) + 1, nbars)):
            cov[b] += max(0.0, min(e, (b + 1) * bar) - max(a, b * bar))
    cov = np.clip(cov / bar, 0, 1)
    m.update(active_share=float(cov.mean()), bars_with_notes=float((nb > 0).mean()),
             notes_per_bar=float(nb.mean()), notes_per_active_bar=float(nb[nb > 0].mean()))
    m.update(pitch_median_midi=float(np.median(md)), pitch_p5=float(np.percentile(md, 5)),
             pitch_p95=float(np.percentile(md, 95)), pitch_range_st=float(np.percentile(md, 95) - np.percentile(md, 5)))
    # intervals between consecutive notes closer than 2 beats
    iv = []
    for k in range(1, len(ev)):
        gap = (ev[k][0] * 16 + ev[k][1]) - (ev[k - 1][0] * 16 + ev[k - 1][1]) - ev[k - 1][2]
        if gap <= 8: iv.append(abs(int(round(ev[k][3] - ev[k - 1][3]))))
    iv = np.array(iv)
    if len(iv):
        m.update(int_unison=float((iv == 0).mean()), int_step=float(((iv >= 1) & (iv <= 2)).mean()),
                 int_third=float(((iv >= 3) & (iv <= 4)).mean()), int_fourth_fifth=float(((iv >= 5) & (iv <= 7)).mean()),
                 int_sixth_seventh=float(((iv >= 8) & (iv <= 11)).mean()), int_octave_plus=float((iv >= 12).mean()),
                 stepwise_share=float(((iv >= 1) & (iv <= 2)).mean()), n_intervals=int(len(iv)))
    m.update(len_staccato=float((ln <= 1).mean()), len_short=float(((ln >= 2) & (ln <= 3)).mean()),
             len_long=float((ln >= 4).mean()), len_median_16ths=float(np.median(ln)))
    hist = np.bincount(pos, minlength=16) / len(pos)
    m["sync_off8th_share"] = float((pos % 2 == 1).mean())
    m["offbeat_share"] = float((pos % 4 != 0).mean())
    m["downbeat_share"] = float((pos == 0).mean())
    # per-bar patterns for repetition
    pat = [dict() for _ in range(nbars)]
    for b, p, l, md_ in ev:
        if b < nbars: pat[b][p] = int(round(md_))
    def block(i, L):
        d = {}
        for j in range(L):
            for p, v in pat[i + j].items(): d[j * 16 + p] = v
        return d
    def sim(a, b):
        ka, kb = set(a), set(b)
        if not (ka or kb): return None, None
        u = ka | kb; c = ka & kb
        ps = (sum(1 for k in c if a[k] % 12 == b[k] % 12) / len(c)) if c else 0.0
        return len(c) / len(u), ps
    for L in (1, 2, 4):
        o, pp = [], []
        for i in range(0, nbars - 2 * L + 1):
            s = sim(block(i, L), block(i + L, L))
            if s[0] is not None: o.append(s[0]); pp.append(s[1])
        if o:
            m[f"rep{L}_onset"] = float(np.mean(o)); m[f"rep{L}_pitch"] = float(np.mean(pp))
    best = (None, -1.0)
    for lag in range(1, min(17, nbars)):
        o = [sim(pat[i], pat[i + lag])[0] for i in range(nbars - lag)]
        o = [v for v in o if v is not None]
        if len(o) >= 3 and np.mean(o) > best[1]: best = (lag, float(np.mean(o)))
    if best[0]:
        m["rep_best_lag_bars"], m["rep_best_score"] = best
    # call / response: which bars have notes
    act = (nb > 0).astype(int)
    runs, cur_, cnt = [], act[0], 1
    for v in act[1:]:
        if v == cur_: cnt += 1
        else: runs.append((cur_, cnt)); cur_, cnt = v, 1
    runs.append((cur_, cnt))
    on_r = [c for v, c in runs if v == 1]; off_r = [c for v, c in runs if v == 0]
    m["run_notes_bars_mean"] = float(np.mean(on_r)) if on_r else 0.0
    m["run_silent_bars_mean"] = float(np.mean(off_r)) if off_r else 0.0
    m["alternations_per_8bars"] = float((len(runs) - 1) / nbars * 8)
    a = act - act.mean()
    best_p = None
    if a.std() > 0:
        ac = [float(np.dot(a[:-l], a[l:]) / np.dot(a, a)) for l in (2, 4, 8) if len(a) > l + 2]
        lags = [l for l in (2, 4, 8) if len(a) > l + 2]
        if ac: best_p = lags[int(np.argmax(ac))]; m["call_response_period_bars"] = best_p; m["call_response_score"] = max(ac)
    extra = dict(onset_hist=hist.round(4).tolist(), bar_activity="".join("#" if v else "." for v in act[:96]),
                 grid_phase_s=ph)
    return m, extra


def analyse(path, bpm_arg):
    import librosa
    t0 = time.time()
    mo = re.search(r"_\((Bass|Other|Drums|Vocals)\)_", path)
    kind = mo.group(1).lower() if mo else "other"
    y, _ = librosa.load(path, sr=SR, mono=True)
    dur = len(y) / SR
    bpm, src = bpm_arg, "arg"
    if bpm is None:
        mm = re.search(r"/stems/([^/]+)/(.+)_\((?:Bass|Other|Drums|Vocals)\)_", path)
        if mm:
            jp = os.path.join(path.split("/stems/")[0], "analysis", mm.group(1), mm.group(2) + ".json")
            if os.path.exists(jp):
                bpm, src = json.load(open(jp))["metrics"]["tempo"], "ref json"
    if bpm is None:
        bpm, src = C.estimate_tempo(y, SR)["tempo"], "estimate"
    f0, voiced, vp, gate = pitch_track(y, kind)
    notes = segment_notes(f0, voiced, 60.0 / bpm / 8)
    m, extra = stats(notes, dur, bpm)
    m["bpm"] = bpm
    m["voiced_frac"] = float(voiced.mean())
    m["voiced_prob_mean"] = float(np.nanmean(vp[voiced])) if voiced.any() else 0.0
    m["gated_frac"] = float(gate.mean())
    m["confidence"] = float(voiced.sum() / max(gate.sum(), 1))
    extra["notes"] = [[round(a, 3), round(b, 3), round(c, 2)] for a, b, c in notes]
    res = dict(file=os.path.abspath(path), kind=kind, bpm_source=src, metrics={k: C.clean(v) for k, v in m.items()},
               runtime_s=round(time.time() - t0, 1), **extra)
    return res, kind


def out_path(path, kind):
    mm = re.search(r"^(.*)/stems/([^/]+)/(.+)_\((?:Bass|Other|Drums|Vocals)\)_[^/]*$", path)
    if mm:
        d = C.out_dir(mm.group(2), os.path.join(mm.group(1), "analysis", mm.group(2)))
        return os.path.join(d, f"{mm.group(3)}.melody-{kind}.json")
    return os.path.splitext(path)[0] + ".melody.json"


def main():
    args = sys.argv[1:]
    bpm = None
    if args and re.fullmatch(r"\d+(\.\d+)?", args[-1]):
        bpm, args = float(args[-1]), args[:-1]
    for p in args:
        try:
            r, kind = analyse(os.path.abspath(p), bpm)
            C.dump(r, out_path(os.path.abspath(p), kind))
            m = r["metrics"]
            print(f"{os.path.basename(p)}: {kind} bpm {m['bpm']:.1f} notes {m['n_notes']} active {m.get('active_share', 0):.2f} "
                  f"conf {m['confidence']:.2f} {r['runtime_s']} s")
        except Exception as e:
            import traceback; traceback.print_exc()
            print(f"FAILED {p}: {e}", file=sys.stderr)


main()
