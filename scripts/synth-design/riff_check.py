"""A/B check of riff_from_ref.py transcriptions: for each clip, 4 bars of the
original stem, then the same 4 bars played from the transcription (a plain
saw), and the chroma correlation between them (1 = same pitch classes over time).
Usage: riff_check.py SET TRACK_SUBSTRING... [--stem other] [--at BAR] --out DIR
"""
import json, os, sys, glob
import numpy as np, soundfile as sf
sys.path.insert(0, os.path.dirname(__file__))
import riff_from_ref as R

def chroma(x, sr, hop):
    import librosa
    return librosa.feature.chroma_cqt(y=x, sr=sr, hop_length=hop)

def main():
    a = sys.argv[1:]
    out = a[a.index("--out") + 1]; a = a[:a.index("--out")] + a[a.index("--out") + 2:]
    stem = "other"
    if "--stem" in a:
        stem = a[a.index("--stem") + 1]; a = [x for i, x in enumerate(a) if i not in (a.index("--stem"), a.index("--stem") + 1)]
    set_, subs = a[0], a[1:]
    os.makedirs(out, exist_ok=True)
    parts, sr = [], 44100
    for sub in subs:
        p = [x for x in glob.glob(os.path.join(R.REF, "analysis", set_, f"*.melody-{stem}.json")) if sub.lower() in x.lower()][0]
        mj, ref = json.load(open(p)), json.load(open(p.replace(f".melody-{stem}.json", ".json")))
        bar = 240 / ref["metrics"]["tempo"]
        phase = ref.get("beat_phase_s", 0.0) or 0.0
        drop = phase + round((ref["main_drop_start_s"] - phase) / bar) * bar
        if os.environ.get("RIFF_ENGINE") == "basic-pitch":
            mj = {"notes": R.basic_pitch_notes(set_, p, stem, drop, bar)}
        elif os.environ.get("RIFF_ENGINE") == "melodia":
            mj = {"notes": R.melodia_notes(set_, p, stem, drop, bar)}
        def window(b):
            t0 = drop + b * bar
            return [n for n in mj["notes"] if t0 - bar / 32 <= n[0] < t0 + 4 * bar - bar / 32], t0
        start = max(range(0, 13, 2), key=lambda b: len(window(b)[0]))
        notes, t0 = window(start)
        name = os.path.basename(p).split(".melody")[0]
        wav = os.path.join(R.REF, "stems", set_, f"{name}_({stem.capitalize()})_htdemucs_ft.wav")
        info = sf.info(wav); sr = info.samplerate
        orig, _ = sf.read(wav, start=int(t0 * sr), frames=int(4 * bar * sr), always_2d=True)
        orig = orig.mean(1)
        synth = np.zeros_like(orig)
        for on, dur, midi in notes:
            s = round((on - t0) / bar * 16) * bar / 16
            l = max(1, round(dur / bar * 16)) * bar / 16
            i0, i1 = int(s * sr), min(len(synth), int((s + l) * sr))
            if i0 >= len(synth) or i1 <= i0:
                continue
            f = 440 * 2 ** ((round(midi) - 69) / 12)
            t = np.arange(i1 - i0) / sr
            env = np.minimum(1, np.minimum(t / 0.005, (t[-1] - t + 1e-9) / 0.02))
            synth[i0:i1] += 0.3 * env * (2 * ((t * f) % 1) - 1)
        hop = 1024
        ca, cb = chroma(orig, sr, hop), chroma(synth + 1e-6, sr, hop)
        n = min(ca.shape[1], cb.shape[1])
        act = cb[:, :n].sum(0) > 0.5
        corr = float(np.mean([np.corrcoef(ca[:, i], cb[:, i])[0, 1] for i in range(n) if act[i]])) if act.any() else float("nan")
        print(f"{name[:50]:50s} bars {start}-{start + 4}: chroma corr {corr:.2f} ({len(notes)} notes)")
        norm = lambda x: x / (np.abs(x).max() + 1e-9) * 0.7
        gap = np.zeros(int(sr * 0.7))
        parts += [norm(orig), gap, norm(synth), gap, gap]
    sf.write(os.path.join(out, "ab-check.wav"), np.concatenate(parts), sr)

main()
