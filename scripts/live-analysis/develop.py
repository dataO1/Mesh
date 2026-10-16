"""Per-track development and interplay analysis plus images, from the full mix,
its four htdemucs_ft stems, the ref JSON and the melody JSONs.
Usage: develop.py [--no-images] FILE...   (FILE = the original track; needs
stems/<set>/<name>_(Drums|Bass|Other|Vocals)_htdemucs_ft.wav, analysis/<set>/<name>.json
and .melody-other/.melody-bass JSON under $LIVE_REF_ROOT)
Writes analysis/<set>/<name>.develop.json and images/<set>/<name>_*.png.
"""
import json
import os
import sys
import time
from fractions import Fraction

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import soundfile as sf
from scipy.signal import butter, resample_poly, sosfilt

import _common as C

SR = 22050
KINDS = ("drums", "bass", "other", "vocals")
COLORS = {"intro": "#6c8ebf", "build": "#d6b656", "drop": "#b85450", "breakdown": "#82b366", "outro": "#9673a6"}
ACTIVE_DB = -18.0      # a stem block is active if within 18 dB (power) of its loudest block
EVENT_T = 0.30         # block-to-block total change counted as a change event


def load22(path):
    x, fs = sf.read(path, dtype="float32", always_2d=True)
    m = x.mean(axis=1).astype(np.float64)
    f = Fraction(SR, fs)
    return resample_poly(m, f.numerator, f.denominator)


def drum_onsets(y):
    import librosa
    on = librosa.onset.onset_detect(y=y, sr=SR, hop_length=256, units="time", backtrack=False)
    def cum(lo, hi):
        sos = butter(4, [lo, hi], btype="band", fs=SR, output="sos")
        return np.concatenate([[0], np.cumsum(sosfilt(sos, y) ** 2)])
    cl, cb = cum(25, 110), cum(180, 400)
    def win(c, t):
        a, b = int(max(t - 0.005, 0) * SR), int((t + 0.06) * SR)
        return c[min(b, len(c) - 1)] - c[a]
    L = np.array([win(cl, t) for t in on]); B = np.array([win(cb, t) for t in on])
    if len(on) == 0:
        return dict(all=[], kick=[], snare=[])
    Lr = L / (np.percentile(L, 90) + 1e-12); Br = B / (np.percentile(B, 90) + 1e-12)
    kick = on[(Lr > 0.35) & (L > B)]
    snare = on[(Br > 0.35) & (B >= L)]
    return dict(all=on, kick=kick, snare=snare)


def slots(times, off, step, n):
    s = np.round((np.asarray(times, dtype=float) - off) / step).astype(int)
    return np.unique(s[(s >= 0) & (s < n)])


def cosd(a, b):
    na, nb = np.linalg.norm(a), np.linalg.norm(b)
    if na == 0 and nb == 0: return 0.0
    if na == 0 or nb == 0: return 1.0
    return float(1 - a @ b / (na * nb))


def analyse(path, images=True):
    t0 = time.time()
    set_, base = C.set_name(path), C.base_name(path)
    root = C.REF_ROOT
    ref = json.load(open(os.path.join(root, "analysis", set_, base + ".json")))
    mo = json.load(open(os.path.join(root, "analysis", set_, base + ".melody-other.json")))
    mb = json.load(open(os.path.join(root, "analysis", set_, base + ".melody-bass.json")))
    bpm, phase = ref["metrics"]["tempo"], ref["beat_phase_s"]
    beat = 60 / bpm; bar = 4 * beat; step = beat / 4; off = phase % beat
    st = {k: load22(os.path.join(root, "stems", set_, f"{base}_({k.capitalize()})_htdemucs_ft.wav")) for k in KINDS}
    dur = min(len(v) for v in st.values()) / SR
    nbars = int((dur - off) / bar)
    nslot = nbars * 16
    # bar-level RMS per stem (dB)
    barlev = {k: np.array([C.db(np.mean(st[k][int((off + b * bar) * SR):int((off + (b + 1) * bar) * SR)] ** 2) + 1e-14)
                           for b in range(nbars)]) for k in KINDS}
    dr = drum_onsets(st["drums"])
    g = {k: slots(dr[k], off, step, nslot) for k in ("all", "kick", "snare")}
    bn = np.array(mb["notes"]).reshape(-1, 3); ln = np.array(mo["notes"]).reshape(-1, 3)
    g["bass"] = slots(bn[:, 0], off, step, nslot) if len(bn) else np.array([], int)
    g["lead"] = slots(ln[:, 0], off, step, nslot) if len(ln) else np.array([], int)
    sound = np.zeros(nslot, bool)           # slots covered by a sounding bass note
    for on, du, _ in bn:
        a = int(round((on - off) / step)); e = int(np.ceil((on + du - off) / step))
        sound[max(a, 0):max(min(e, nslot), 0)] = True
    drum_any = np.union1d(g["kick"], g["snare"])
    m = {}
    lead = g["lead"]
    if len(lead):
        sb = set(g["bass"]); sk = set(g["kick"]); ss = set(g["snare"]); sd = set(drum_any)
        m["ip_lead_onsets"] = int(len(lead))
        m["ip_lead_on_bass_onset"] = float(np.mean([s in sb for s in lead]))
        m["ip_lead_in_bass_gap"] = float(np.mean([not sound[s] for s in lead]))
        m["ip_lead_on_kick"] = float(np.mean([s in sk for s in lead]))
        m["ip_lead_on_snare"] = float(np.mean([s in ss for s in lead]))
        trip = [s in sb and s in sd for s in lead]
        m["ip_all3_of_lead"] = float(np.mean(trip))
        anyon = set(g["bass"]) | set(lead) | sd
        m["ip_all3_of_any"] = float(sum(trip) / max(len(anyon), 1))
    if len(g["bass"]):
        sk = set(g["kick"])
        m["ip_bass_on_kick"] = float(np.mean([s in sk for s in g["bass"]]))
    for tag, w in (("bar", 16), ("halfbar", 8)):
        nb = nslot // w
        la = np.array([np.any((lead >= i * w) & (lead < (i + 1) * w)) for i in range(nb)]) if nb else np.array([])
        ba = np.array([np.any((g["bass"] >= i * w) & (g["bass"] < (i + 1) * w)) for i in range(nb)]) if nb else np.array([])
        if nb and (la | ba).any():
            only = (la ^ ba)
            m[f"ip_alt_share_{tag}"] = float(only.sum() / (la | ba).sum())
            owner = [(i, bool(la[i])) for i in range(nb) if only[i]]
            flips = [owner[i][1] != owner[i - 1][1] for i in range(1, len(owner))]
            m[f"ip_alt_flip_{tag}"] = float(np.mean(flips)) if flips else 0.0
            m[f"ip_both_share_{tag}"] = float((la & ba).sum() / (la | ba).sum())
    # blocks
    segs = ref.get("structure") or []
    blocks = []
    for s in segs:
        b0, nb = s["start_bar"], min(s["bars"], nbars - s["start_bar"])
        if nb < 1: continue
        d = dict(start_bar=b0, bars=nb, label=s["label"], band_db=[s["low_db"], s["mid_db"], s["high_db"]])
        d["level_db"] = {k: float(C.db(np.mean(10 ** (barlev[k][b0:b0 + nb] / 10)))) for k in KINDS}
        r = (slice(b0 * 16, (b0 + nb) * 16))
        def hist(arr):
            a = arr[(arr >= r.start) & (arr < r.stop)]
            return np.bincount(a % 16, minlength=16).astype(float), len(a)
        d["hist"] = {"drums": hist(g["all"])[0], "bass": hist(g["bass"])[0], "lead": hist(lead)[0]}
        tl = ln[(ln[:, 0] >= off + b0 * bar) & (ln[:, 0] < off + (b0 + nb) * bar)] if len(ln) else np.zeros((0, 3))
        d["pc"] = np.bincount((tl[:, 2].round().astype(int) % 12), minlength=12).astype(float) if len(tl) else np.zeros(12)
        d["lead_notes_per_bar"] = float(len(tl) / nb)
        d["lead_pitch_range"] = float(np.percentile(tl[:, 2], 95) - np.percentile(tl[:, 2], 5)) if len(tl) > 2 else 0.0
        d["bass_notes_per_bar"] = float(hist(g["bass"])[1] / nb)
        blocks.append(d)
    nbk = len(blocks)
    if nbk >= 2:
        mx = {k: max(b["level_db"][k] for b in blocks) for k in KINDS}
        for b in blocks:
            b["active"] = {k: bool(b["level_db"][k] >= mx[k] + ACTIVE_DB) for k in KINDS}
        def feat(b, e):
            if e == "lead": return np.concatenate([b["hist"]["lead"], b["pc"]]) if b["active"]["other"] else np.zeros(28)
            k = "drums" if e == "drums" else "bass"
            return b["hist"][e] if b["active"][k] else np.zeros(16)
        lvk = {"drums": "drums", "bass": "bass", "lead": "other"}
        def change(a, b, e):
            fa, fb = feat(a, e), feat(b, e)
            lv = min(1.0, abs(a["level_db"][lvk[e]] - b["level_db"][lvk[e]]) / 12)
            return 0.5 * cosd(fa, fb) + 0.5 * lv
        def spec(a, b):
            return min(1.0, float(np.mean(np.abs(np.array(a["band_db"]) - np.array(b["band_db"]))) / 12))
        def total(a, b):
            c = {e: change(a, b, e) for e in ("drums", "bass", "lead")}; c["spectrum"] = spec(a, b)
            c["total"] = float(np.mean(list(c.values()))); return c
        for lag, tag in ((1, "8"), (2, "16"), (4, "32")):
            cs = [total(blocks[i], blocks[i + lag]) for i in range(nbk - lag)]
            if cs:
                for e in ("drums", "bass", "lead", "spectrum", "total"):
                    m[f"chg{tag}_{e}"] = float(np.mean([c[e] for c in cs]))
        bnd = [(blocks[i]["start_bar"], total(blocks[i - 1], blocks[i])) for i in range(1, nbk)]
        ev = [b for b, c in bnd if c["total"] >= EVENT_T]
        m["change_events"] = len(ev)
        m["change_events_per_32bars"] = float(len(ev) / max(nbars, 1) * 32)
        if len(ev) >= 2:
            gaps = np.diff(ev)
            m["change_interval_bars"] = float(np.median(gaps))
            m["change_gap_8_16_32_share"] = float(np.mean([min(abs(gp - q) for q in (8, 16, 32)) <= 2 for gp in gaps]))
        top = sorted(bnd, key=lambda q: -q[1]["total"])[:3]
        m["max_change"] = float(top[0][1]["total"]); m["max_change_bar"] = int(top[0][0])
        contrasts = [dict(bar=int(bb), total=c["total"], dominant=max(("drums", "bass", "lead", "spectrum"), key=lambda e: c[e])) for bb, c in top]
        flips = {k: sum(blocks[i]["active"][k] != blocks[i - 1]["active"][k] for i in range(1, nbk)) for k in KINDS}
        ent = sum((not blocks[i - 1]["active"][k]) and blocks[i]["active"][k] for i in range(1, nbk) for k in KINDS)
        lev = sum(blocks[i - 1]["active"][k] and (not blocks[i]["active"][k]) for i in range(1, nbk) for k in KINDS)
        for k in KINDS: m[f"flips_{k}_per32"] = float(flips[k] / max(nbars, 1) * 32)
        m["enter_events"], m["leave_events"] = int(ent), int(lev)
        m["flips_total_per32"] = float(sum(flips.values()) / max(nbars, 1) * 32)
        for k in KINDS: m[f"active_share_{k}"] = float(np.mean([b["active"][k] for b in blocks]))
        sig = [np.array([max(b["level_db"][k], -60) for k in KINDS] + b["band_db"] + [40.0 * b["active"][k] for k in KINDS]) for b in blocks]
        cl, labs = [], []
        for v in sig:
            for ci, c in enumerate(cl):
                if np.mean(np.abs(v - c)) < 4.0: labs.append(ci); break
            else:
                cl.append(v); labs.append(len(cl) - 1)
        m["n_distinct_blocks"] = len(cl)
        m["n_block_runs"] = 1 + sum(labs[i] != labs[i - 1] for i in range(1, len(labs)))
        m["lead_notes_per_bar_cv"] = float(np.std([b["lead_notes_per_bar"] for b in blocks]) / (np.mean([b["lead_notes_per_bar"] for b in blocks]) + 1e-9))
        m["lead_pitch_range_block_mean"] = float(np.mean([b["lead_pitch_range"] for b in blocks]))
    else:
        contrasts = []
    out_blocks = [{k: (v.tolist() if isinstance(v, np.ndarray) else v) for k, v in b.items() if k not in ("hist", "pc")} for b in blocks]
    res = dict(file=os.path.abspath(path), set=set_, onset_counts={k: int(len(v)) for k, v in g.items()},
               metrics={k: C.clean(float(v) if isinstance(v, (float, np.floating)) else v) for k, v in m.items()},
               main_drop_start_s=ref["metrics"].get("main_drop_start_s"), contrasts=contrasts, blocks=out_blocks)
    if images:
        draw(path, set_, base, ref, st, barlev, off, bar, nbars)
    res["runtime_s"] = round(time.time() - t0, 1)
    return res


# ------------------------------------------------------------------ images
def strip_fig(segs, off, bar, tmax, nrows=1, h=None):
    fig = plt.figure(figsize=(16, 9), dpi=100)
    gs = fig.add_gridspec(nrows + 1, 1, height_ratios=[0.07] + [1] * nrows, hspace=0.06)
    top = fig.add_subplot(gs[0])
    axes = [fig.add_subplot(gs[i + 1], sharex=top) for i in range(nrows)]
    top.set_ylim(0, 1); top.set_yticks([])
    i = 0
    while i < len(segs):
        j = i
        while j + 1 < len(segs) and segs[j + 1]["label"] == segs[i]["label"]: j += 1
        a = off + segs[i]["start_bar"] * bar; b = off + (segs[j]["start_bar"] + segs[j]["bars"]) * bar
        top.axvspan(a, b, color=COLORS.get(segs[i]["label"], "#999"), alpha=0.85)
        if (b - a) > tmax * 0.03:
            top.text((a + b) / 2, 0.5, segs[i]["label"], ha="center", va="center", fontsize=9, color="white")
        i = j + 1
    plt.setp(top.get_xticklabels(), visible=False)
    return fig, top, axes


def grid8(ax, off, bar, tmax, every=8, label=False):
    for b in range(0, int(tmax / bar) + 1, every):
        ax.axvline(off + b * bar, color="white" if not label else "k", alpha=0.35, lw=0.6)


def bar_ticks(ax, off, bar, tmax, every=8):
    ticks = [off + b * bar for b in range(0, int(tmax / bar) + 1, every)]
    ax.set_xticks(ticks); ax.set_xticklabels([str(b) for b in range(0, int(tmax / bar) + 1, every)], fontsize=8)
    ax.set_xlabel("bar")


def draw(path, set_, base, ref, st, barlev, off, bar, nbars):
    import librosa
    segs = ref.get("structure") or []
    d = os.path.join(C.REF_ROOT, "analysis", "images", set_); os.makedirs(d, exist_ok=True)
    x, fs = C.read_audio(path); mono = x.mean(axis=1)
    tmax = len(mono) / fs
    freqs_ticks = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 16000]
    def mel_db(sig, hop, nm=256):
        S = librosa.feature.melspectrogram(y=sig, sr=fs, n_fft=8192, hop_length=hop, n_mels=nm, fmin=20, fmax=16000, power=2.0)
        return librosa.power_to_db(S, ref=np.max, top_db=80), librosa.mel_frequencies(nm, fmin=20, fmax=16000), np.arange(S.shape[1]) * hop / fs
    def spec_ax(ax, sig, hop, t_off=0.0):
        D, f, t = mel_db(sig, hop)
        im = ax.pcolormesh(t + t_off, f, D, shading="auto", cmap="magma", vmin=-80, vmax=0, rasterized=True)
        ax.set_yscale("log"); ax.set_ylim(20, 16000); ax.set_yticks(freqs_ticks); ax.set_yticklabels([f"{v/1000:g}k" if v >= 1000 else str(v) for v in freqs_ticks], fontsize=8)
        ax.set_ylabel("Hz"); return im
    # (a) full spectrogram with sections
    fig, top, (ax,) = strip_fig(segs, off, bar, tmax)
    spec_ax(ax, mono, 2048); grid8(ax, off, bar, tmax); bar_ticks(ax, off, bar, tmax)
    top.set_title(f"{base}  ({ref['metrics']['tempo']:.1f} BPM, {ref['metrics']['lufs']:.1f} LUFS)  full mix, 8-bar grid", fontsize=10)
    fig.savefig(os.path.join(d, base + "_spec_full.png"), bbox_inches="tight"); plt.close(fig)
    # (b) stem activity lanes
    fig, top, axes = strip_fig(segs, off, bar, tmax, nrows=4)
    tb = off + (np.arange(nbars) + 0.5) * bar
    for ax, k in zip(axes, ("drums", "bass", "other", "vocals")):
        lv = np.clip(barlev[k] - barlev[k].max(), -50, 0)
        ax.fill_between(tb, -50, lv, step="mid", color="#4c72b0"); ax.set_ylim(-50, 0)
        ax.set_ylabel(k + "\ndB re max", fontsize=8); grid8(ax, off, bar, tmax, label=True); ax.tick_params(labelsize=7)
        if ax is not axes[-1]: plt.setp(ax.get_xticklabels(), visible=False)
    bar_ticks(axes[-1], off, bar, tmax); top.set_title(f"{base}  stem level per bar", fontsize=10)
    fig.savefig(os.path.join(d, base + "_activity_full.png"), bbox_inches="tight"); plt.close(fig)
    # (c) chroma of the other stem
    names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
    def chroma(sig, sr, hop):
        C_ = librosa.feature.chroma_stft(y=sig, sr=sr, n_fft=4096, hop_length=hop)
        return C_ / (C_.max(axis=0, keepdims=True) + 1e-9)
    def chroma_ax(ax, C_, hop, sr, t_off=0.0):
        t = np.arange(C_.shape[1]) * hop / sr + t_off
        ax.pcolormesh(t, np.arange(12), C_, shading="nearest", cmap="viridis", rasterized=True)
        ax.set_yticks(range(12)); ax.set_yticklabels(names, fontsize=8)
    fig, top, (ax,) = strip_fig(segs, off, bar, tmax)
    hop = 2048; chroma_ax(ax, chroma(st["other"], SR, hop), hop, SR); grid8(ax, off, bar, tmax); bar_ticks(ax, off, bar, tmax)
    ax.set_xlim(0, tmax); top.set_title(f"{base}  chroma of the other stem, 8-bar grid", fontsize=10)
    fig.savefig(os.path.join(d, base + "_chroma_full.png"), bbox_inches="tight"); plt.close(fig)
    # 16-bar drop images
    s0 = ref["metrics"].get("main_drop_start_s")
    if s0 is None: return
    s0 = max(0.0, s0 - ((s0 - off) % bar)) if False else s0
    a, b = s0, min(s0 + 16 * bar, tmax)
    ia, ib = int(a * fs), int(b * fs)
    fig, axs = plt.subplots(1, 1, figsize=(16, 9), dpi=100)
    spec_ax(axs, mono[ia:ib], 512, a)
    for k in range(17):
        axs.axvline(a + k * bar, color="white", alpha=0.5 if k % 4 == 0 else 0.25, lw=0.8)
    axs.set_xlim(a, b); axs.set_xticks([a + k * bar for k in range(0, 17, 2)]); axs.set_xticklabels([str(k) for k in range(0, 17, 2)])
    axs.set_xlabel("bar of the drop"); axs.set_title(f"{base}  16 bars from the main drop at {a:.1f} s", fontsize=10)
    fig.savefig(os.path.join(d, base + "_spec.png"), bbox_inches="tight"); plt.close(fig)
    fig, axs = plt.subplots(2, 1, figsize=(16, 9), dpi=100, sharex=True)
    hop = 512
    chroma_ax(axs[0], chroma(mono[ia:ib], fs, hop), hop, fs, a); axs[0].set_title(f"{base}  chroma, full mix (top) and other stem (bottom), 16 bars from {a:.1f} s", fontsize=10)
    ja, jb = int(a * SR), int(b * SR)
    chroma_ax(axs[1], chroma(st["other"][ja:jb], SR, 256), 256, SR, a)
    for ax in axs:
        for k in range(17): ax.axvline(a + k * bar, color="white", alpha=0.5 if k % 4 == 0 else 0.25, lw=0.8)
    axs[1].set_xlim(a, b); axs[1].set_xticks([a + k * bar for k in range(0, 17, 2)]); axs[1].set_xticklabels([str(k) for k in range(0, 17, 2)]); axs[1].set_xlabel("bar of the drop")
    fig.savefig(os.path.join(d, base + "_chroma.png"), bbox_inches="tight"); plt.close(fig)


def main():
    args = sys.argv[1:]
    images = True
    if args and args[0] == "--no-images":
        images, args = False, args[1:]
    for p in args:
        try:
            r = analyse(p, images)
            C.dump(r, os.path.join(C.out_dir(r["set"]), C.base_name(p) + ".develop.json"))
            m = r["metrics"]
            print(f"{C.base_name(p)}: events {m.get('change_events')} interval {m.get('change_interval_bars')} "
                  f"distinct {m.get('n_distinct_blocks')} lead_on_bass {m.get('ip_lead_on_bass_onset')} {r['runtime_s']} s")
        except FileNotFoundError as e:
            print(f"SKIP {C.base_name(p)}: missing {e.filename}", file=sys.stderr)
        except Exception as e:
            import traceback; traceback.print_exc()
            print(f"FAILED {p}: {e}", file=sys.stderr)


main()
