"""Audition riff clips one by one, in context, on one designed sound.

Usage: riff_audition.py AUDITION_DIR CLIP_DIR --out OUT_DIR [--bars 8] [--lane lead]
       [--seed 7] [--tempo 174]

AUDITION_DIR is an audition.py output (its style/ folder holds the sound and the
showcase form). Each clip in CLIP_DIR plays alone on the lane (the lane's motif
folder holds only that clip) for BARS bars with all lanes, numbered, into
OUT_DIR/riffs.wav with OUT_DIR/index.txt, so the user can rate them by number.
"""
import os
import shutil
import subprocess
import sys

import numpy as np
import soundfile as sf
import yaml

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
REL = os.path.join(ROOT, "target", "release")


def main():
    a = sys.argv[1:]
    if len(a) < 2 or "--out" not in a:
        sys.exit(__doc__)
    aud, clips = os.path.abspath(a[0]), os.path.abspath(a[1])
    opt = dict(zip(a[2::2], a[3::2]))
    out = os.path.abspath(opt["--out"])
    os.makedirs(out, exist_ok=True)
    style_dir = os.path.join(aud, "style")
    style = yaml.safe_load(open(os.path.join(style_dir, "style.yaml")))
    lane_name = opt.get("--lane", "lead")
    li = next(k for k, l in enumerate(style["lanes"]) if l["name"] == lane_name)
    show_form = len(style["forms"]) - 1
    lead_dir = os.path.join(style_dir, "motifs", "lead")
    one = os.path.join(out, "one-clip")
    parts, index, t, sr = [], [], 0.0, 48000
    names = sorted(f for f in os.listdir(clips) if f.endswith(".mid"))
    for i, name in enumerate(names, 1):
        if os.path.isdir(one):
            shutil.rmtree(one)
        os.makedirs(one)
        shutil.copy(os.path.join(clips, name), one)
        if os.path.islink(lead_dir) or os.path.exists(lead_dir):
            os.remove(lead_dir) if os.path.islink(lead_dir) else shutil.rmtree(lead_dir)
        os.symlink(one, lead_dir)
        wav = os.path.join(out, f"{i:02d}-{name[:-4]}.wav")
        r = subprocess.run([os.path.join(REL, "live-render"), "--style", style_dir, "--clap", "--seed", opt.get("--seed", "7"),
                            "--tempo", opt.get("--tempo", "174"), "--form", f"{li}={show_form}", "--bars", opt.get("--bars", "8"),
                            "--out", wav], capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(r.stderr[-2000:])
        x, sr = sf.read(wav, always_2d=True)
        index.append(f"{int(t // 60):02d}:{t % 60:04.1f}  #{i:02d}  {name[:-4]}")
        parts += [x, np.zeros((int(1.5 * sr), x.shape[1]))]
        t += len(x) / sr + 1.5
    sf.write(os.path.join(out, "riffs.wav"), np.concatenate(parts), sr, subtype="FLOAT")
    open(os.path.join(out, "index.txt"), "w").write("\n".join(index) + "\n")
    print("\n".join(index))


if __name__ == "__main__":
    main()
