"""Score an audition against a reference set: the design's lane against the
set's separated stem (Other for leads, Bass for mid basses).

Usage: score.py AUDITION_DIR --set SET [--stem other|bass] [--lane 2] [--bpm 174]

Per design (AUDITION_DIR/renders/<name>.context.lane<L>.wav):

- Spectrum: 1/3-octave levels relative to 1 kHz against the set's stem over the
  first 30 s of each track's main drop (median and IQR, cached per set in
  analysis/<set>/stemspec-<stem>.json). Flag: more than 3 dB off the median.
- Melody and rhythm: melody.py on the lane against the set's melody-<stem>
  JSONs. Flag: outside the interquartile range.
- Morph loudness spread from index.txt. Flag: more than 3 LU.

Writes AUDITION_DIR/score.md and prints it. Needs the live-analyse Python
environment (librosa); scripts/synth-design.sh score runs it there.
"""
import glob
import json
import os
import re
import subprocess
import sys

import numpy as np
import soundfile as sf
from scipy.signal import welch

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
REF = os.environ.get("LIVE_REF_ROOT", os.path.expanduser("~/Music/mesh-live-dev/references"))
CENTERS = 1000 * 2.0 ** (np.arange(-12, 13) / 3)  # 63 Hz .. 16 kHz
SHOW = [63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]
MELODY_KEYS = [
    "notes_per_active_bar", "active_share", "pitch_range_st", "int_unison", "int_step", "int_third",
    "int_fourth_fifth", "int_octave_plus", "len_staccato", "len_long", "len_median_16ths",
    "sync_off8th_share", "downbeat_share", "rep2_pitch", "rep4_pitch", "run_notes_bars_mean",
]


def spectrum(x, sr):
    m = x.mean(1) if x.ndim > 1 else x
    f, p = welch(m, sr, nperseg=16384)
    lv = np.array([10 * np.log10(p[(f >= c / 2 ** (1 / 6)) & (f < c * 2 ** (1 / 6))].sum() * (f[1] - f[0]) + 1e-20) for c in CENTERS])
    return lv - lv[np.argmin(abs(CENTERS - 1000))]


def ref_spectra(set_, stem):
    cache = os.path.join(REF, "analysis", set_, f"stemspec-{stem}.json")
    if os.path.exists(cache):
        return np.array(json.load(open(cache))["spectra"])
    out = []
    for jp in sorted(glob.glob(os.path.join(REF, "analysis", set_, "*.json"))):
        name = os.path.basename(jp)[:-5]
        if "." in name and name.split(".")[-1] in ("develop", "melody-bass", "melody-other"):
            continue
        r = json.load(open(jp))
        drop = r.get("main_drop_start_s") or 0.0
        wav = os.path.join(REF, "stems", set_, f"{name}_({stem.capitalize()})_htdemucs_ft.wav")
        if not os.path.exists(wav):
            continue
        info = sf.info(wav)
        start = int(drop * info.samplerate)
        x, sr = sf.read(wav, start=start, frames=int(30 * info.samplerate), always_2d=True)
        if len(x) > sr:
            out.append(spectrum(x, sr).tolist())
    json.dump({"centers": CENTERS.tolist(), "spectra": out}, open(cache, "w"))
    return np.array(out)


def ref_melody(set_, stem):
    rows = []
    for jp in glob.glob(os.path.join(REF, "analysis", set_, f"*.melody-{stem}.json")):
        rows.append(json.load(open(jp))["metrics"])
    return rows


def run_melody(wav, stem, bpm, tmp):
    """melody.py on a lane stem; the file name tells it the stem kind."""
    link = os.path.join(tmp, f"lane_({stem.capitalize()})_design.wav")
    if os.path.lexists(link):
        os.remove(link)
    os.symlink(os.path.abspath(wav), link)
    subprocess.run([sys.executable, os.path.join(ROOT, "scripts", "live-analysis", "melody.py"), link, str(bpm)],
                   capture_output=True, text=True)
    out = os.path.splitext(link)[0] + ".melody.json"
    return json.load(open(out))["metrics"] if os.path.exists(out) else {}


def main():
    a = sys.argv[1:]
    if not a or "--set" not in a:
        sys.exit(__doc__)
    aud = a[0]
    opt = dict(zip(a[1::2], a[2::2]))
    set_, stem, lane, bpm = opt["--set"], opt.get("--stem", "other"), opt.get("--lane", "2"), float(opt.get("--bpm", 174))
    tmp = os.path.join(aud, "score-tmp")
    os.makedirs(tmp, exist_ok=True)

    refs = ref_spectra(set_, stem)
    q1, med, q3 = np.percentile(refs, [25, 50, 75], axis=0)
    mel = ref_melody(set_, stem)
    index = open(os.path.join(aud, "index.txt")).read() if os.path.exists(os.path.join(aud, "index.txt")) else ""

    md = [f"# Score vs {set_} ({stem} stem, {len(refs)} tracks)", ""]
    for wav in sorted(glob.glob(os.path.join(aud, "renders", f"*.context.lane{lane}.wav"))):
        name = os.path.basename(wav).split(".context.")[0]
        x, sr = sf.read(wav, always_2d=True)
        x = x[int(240 / bpm * sr):]  # skip the first bar
        spec = spectrum(x, sr)
        md += [f"## {name}", "", "| band (Hz) | " + " | ".join(str(c) for c in SHOW) + " |", "|---" * (len(SHOW) + 1) + "|"]
        pick = [int(np.argmin(abs(CENTERS - c))) for c in SHOW]
        md.append("| design (dB vs 1k) | " + " | ".join(f"{spec[i]:+.1f}" for i in pick) + " |")
        md.append("| set median | " + " | ".join(f"{med[i]:+.1f}" for i in pick) + " |")
        md.append("| off by | " + " | ".join(f"{spec[i] - med[i]:+.1f}{' !' if abs(spec[i] - med[i]) > 3 else ''}" for i in pick) + " |")
        md.append("")
        m = run_melody(wav, stem, bpm, tmp)
        md += ["| metric | design | set median [q1..q3] | |", "|---|---|---|---|"]
        for k in MELODY_KEYS:
            vals = np.array([r[k] for r in mel if isinstance(r.get(k), (int, float))], dtype=float)
            if k not in m or not len(vals):
                continue
            lo, mid, hi = np.percentile(vals, [25, 50, 75])
            flag = "" if lo <= m[k] <= hi else ("low" if m[k] < lo else "high")
            md.append(f"| {k} | {m[k]:.3g} | {mid:.3g} [{lo:.3g}..{hi:.3g}] | {flag} |")
        mm = re.search(rf"{re.escape(name)}.*?morph LUFS: ([^\n]*)", index, re.S)
        if mm:
            md += ["", f"Morph loudness: {mm.group(1).strip()}"]
        md.append("")
    open(os.path.join(aud, "score.md"), "w").write("\n".join(md) + "\n")
    print("\n".join(md))


if __name__ == "__main__":
    main()
