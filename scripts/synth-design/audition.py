"""Audition compiled sound designs in a live style, through the real engine.

Usage: audition.py DESIGN_DIR NAME... --style BASE_STYLE --out OUT_DIR
                   [--motifs DIR]  (the lead lane plays the clips in DIR)
                   [--lane lead] [--bars 16] [--seed 7] [--solo-bars 8]
                   [--morph-control 3] [--key Am] [--tempo 174] [--set K=V]...

For the designs NAME (DESIGN_DIR/NAME.fxp + NAME.lane.yaml, from compile.py):

1. Builds OUT_DIR/style: a copy of BASE_STYLE (assets linked, not copied)
   whose lane LANE plays the designs as its options, with the first design's
   lane FX, controls and high-pass (`hpf`), then calibrates that lane's
   loudness (style-calibrate --lanes LANE).
2. Renders each design: BARS in context (all lanes); a showcase in context
   where each of the 4 controls sweeps 0 -> 1 in turn; SOLO_BARS alone; and the
   morph control at 0, 0.5 and 1 (SOLO_BARS each, alone) for the loudness
   check. Stems of the context render go to OUT_DIR/renders/.
3. Writes OUT_DIR/audition.wav (everything in order with 1 s gaps) and
   OUT_DIR/index.txt (mm:ss of each part, LUFS of the morph renders).

--set K=V passes `--control`, `--lane-seed`, `--form` style overrides to
live-render, e.g. --set control=2.1=0.8 --set form=2=3.
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
GAP_S = 1.0
# Form the showcase, solo and morph renders play: the synth lane plays all
# 64 bars (the built-in synth forms rest a lot; the context render keeps them).
SHOWCASE_FORM = "A15 C1 A15 D1 B15 C1 A15 D1 | A16 A'16 B16 A'16"


def default_forms():
    """The built-in forms with their synth forms, read from form.rs (a style
    without `forms` plays these)."""
    import re
    src = open(os.path.join(ROOT, "crates", "mesh-core", "src", "live", "form.rs")).read()
    def block(name):
        body = src.split(f"pub const {name}")[1].split("];")[0]
        out = []
        for entry in re.split(r"\n    (?=concat!|\")", body)[1:]:
            parts = re.findall(r'"([^"]*)"', entry)
            if parts:
                out.append("".join(parts).strip())
        return out
    mains, leads = block("DEFAULT_FORMS"), block("DEFAULT_LEAD_FORMS")
    assert len(mains) == len(leads) == 10, (len(mains), len(leads))
    return [f"{m} | {l}" for m, l in zip(mains, leads)]
# Showcase: bars at the defaults before the first sweep, bars per 0 -> 1 sweep.
LEAD_IN = 4
SWEEP_BARS = 8


def run(cmd):
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"failed: {' '.join(cmd)}\n{r.stderr[-3000:]}")
    return r.stdout + r.stderr


def build_bins():
    run(["cargo", "build", "--release", "-q", "-p", "mesh-core", "--bin", "live-render", "--bin", "style-calibrate"])


def link_style(base, dst):
    """dst/style.yaml is a real copy, everything else links into base."""
    if os.path.exists(dst):
        shutil.rmtree(dst)
    os.makedirs(os.path.join(dst, "assets"))
    for e in os.listdir(base):
        if e in ("style.yaml", "assets") or e.endswith(".bak"):
            continue
        os.symlink(os.path.join(base, e), os.path.join(dst, e))
    for e in os.listdir(os.path.join(base, "assets")):
        os.symlink(os.path.join(base, "assets", e), os.path.join(dst, "assets", e))
    os.makedirs(os.path.join(dst, "assets", "designs"))


def use_motifs(style_dir, base, kind, src):
    """The style's motifs/<kind> clips become the ones in `src` (the other kind
    stays the base style's)."""
    link = os.path.join(style_dir, "motifs")
    if os.path.islink(link):
        os.remove(link)
    os.makedirs(link, exist_ok=True)
    for k in ("bass", "lead"):
        target = os.path.abspath(src) if k == kind else os.path.join(base, "motifs", k)
        if os.path.exists(target) and not os.path.exists(os.path.join(link, k)):
            os.symlink(target, os.path.join(link, k))


def measure_lufs_py(path):
    """BS.1770 integrated loudness (K-weighting, 400 ms gates), for the index."""
    from scipy.signal import lfilter

    x, sr = sf.read(path, always_2d=True)
    # K-weighting at 48 kHz (BS.1770-4 coefficients).
    b1, a1 = [1.53512485958697, -2.69169618940638, 1.19839281085285], [1.0, -1.69065929318241, 0.73248077421585]
    b2, a2 = [1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]
    y = lfilter(b2, a2, lfilter(b1, a1, x, axis=0), axis=0)
    blk, hop = int(0.4 * sr), int(0.1 * sr)
    ms = np.array([np.mean(y[i:i + blk] ** 2, axis=0).sum() for i in range(0, len(y) - blk, hop)])
    if len(ms) == 0:
        return float("-inf")
    l = -0.691 + 10 * np.log10(ms + 1e-20)
    ms = ms[l > -70]
    if len(ms) == 0:
        return float("-inf")
    rel = -0.691 + 10 * np.log10(ms.mean()) - 10
    ms = ms[-0.691 + 10 * np.log10(ms + 1e-20) > rel]
    return float(-0.691 + 10 * np.log10(ms.mean()))


def main():
    a = sys.argv[1:]
    opt = {"--motifs": None, "--lane": "lead", "--bars": "16", "--seed": "7", "--solo-bars": "8", "--morph-control": "3", "--key": None, "--tempo": "174"}
    sets, pos = [], []
    i = 0
    while i < len(a):
        if a[i] == "--set":
            sets.append(a[i + 1]); i += 2
        elif a[i] in opt or a[i] in ("--style", "--out"):
            opt[a[i]] = a[i + 1]; i += 2
        else:
            pos.append(a[i]); i += 1
    if len(pos) < 2 or "--style" not in opt or "--out" not in opt:
        sys.exit(__doc__)
    design_dir, names = pos[0], pos[1:]
    out = os.path.abspath(opt["--out"])
    os.makedirs(os.path.join(out, "renders"), exist_ok=True)
    build_bins()

    style_dir = os.path.join(out, "style")
    link_style(os.path.abspath(opt["--style"]), style_dir)
    if opt["--motifs"]:
        use_motifs(style_dir, os.path.abspath(opt["--style"]), "lead", opt["--motifs"])
    style = yaml.safe_load(open(os.path.join(opt["--style"], "style.yaml")))
    li = next(k for k, l in enumerate(style["lanes"]) if l["name"] == opt["--lane"])
    lane = style["lanes"][li]
    blocks = []
    for n in names:
        shutil.copy(os.path.join(design_dir, f"{n}.fxp"), os.path.join(style_dir, "assets", "designs", f"{n}.fxp"))
        blocks.append(yaml.safe_load(open(os.path.join(design_dir, f"{n}.lane.yaml"))))
    lane["options"] = [b["option"] for b in blocks]
    lane["default_option"] = 0
    lane.pop("output_trims_db", None)
    first = blocks[0]
    if first.get("fx"):
        lane["fx"] = first["fx"]
    if first.get("controls"):
        lane["controls"] = first["controls"]
    if first.get("generator"):
        lane.setdefault("generator", {}).setdefault("params", {}).update(first["generator"])
    if first.get("hpf") is not None:
        hpf = style.setdefault("glue", {}).setdefault("role_hpf", {})
        hpf[lane["role"]] = max(float(first["hpf"]), 10.0)
    if not style.get("forms"):
        style["forms"] = default_forms()
    style["forms"].append(SHOWCASE_FORM)
    show_form = len(style["forms"]) - 1
    with open(os.path.join(style_dir, "style.yaml"), "w") as f:
        yaml.safe_dump(style, f, sort_keys=False)
    cal = run([os.path.join(REL, "style-calibrate"), "--style", style_dir, "--lanes", opt["--lane"], "--clap"])
    print("\n".join(l for l in cal.splitlines() if opt["--lane"] in l or "skipped" in l))

    common = ["--style", style_dir, "--seed", opt["--seed"], "--clap"]
    if opt["--key"]:
        common += ["--key", opt["--key"]]
    if opt["--tempo"]:
        common += ["--tempo", opt["--tempo"]]
    for s in sets:
        k, v = s.split("=", 1)
        common += [f"--{k}", v]
    mute_others = ",".join(str(k) for k in range(len(style["lanes"])) if k != li)
    morph_c = opt["--morph-control"]

    parts, index = [], ["Audition: context (all lanes), showcase (each control swept in context), solo, morph 0 / 0.5 / 1 (solo). LUFS of the solo morph renders.", ""]
    sr = 48000
    t = 0.0
    for k, n in enumerate(names):
        base = os.path.join(out, "renders", n)
        run([os.path.join(REL, "live-render"), *common, "--option", f"{li}={k}", "--bars", opt["--bars"], "--stems", "--out", base + ".context.wav"])
        # Showcase: each control in turn, in context: default -> 0 (1 bar), 0 -> 1
        # (SWEEP_BARS), 1 -> default (1 bar), after LEAD_IN bars at the defaults.
        controls = blocks[k].get("controls") or lane.get("controls") or []
        sweeps, marks, at = [], [], float(LEAD_IN)
        for c, spec in enumerate(controls[:4]):
            d = float(spec.get("default", 0.5))
            lab = spec.get("label") or f"c{c}"
            sweeps += [f"{li}.{c}={d}:0@{at}-{at + 1}", f"{li}.{c}=0:1@{at + 1}-{at + 1 + SWEEP_BARS}",
                       f"{li}.{c}=1:{d}@{at + 1 + SWEEP_BARS}-{at + 2 + SWEEP_BARS}"]
            marks.append((at, lab, d))
            at += SWEEP_BARS + 2
        show_bars = int(at) + 1
        args = [a for s in sweeps for a in ("--sweep", s)]
        sf_ = ["--form", f"{li}={show_form}"]
        run([os.path.join(REL, "live-render"), *common, *sf_, "--option", f"{li}={k}", "--bars", str(show_bars), *args, "--out", base + ".showcase.wav"])
        run([os.path.join(REL, "live-render"), *common, "--option", f"{li}={k}", "--bars", opt["--solo-bars"], "--mute", mute_others, "--form", f"{li}={show_form}", "--out", base + ".solo.wav"])
        morph = []
        for v in ("0", "0.5", "1"):
            p = f"{base}.morph{v}.wav"
            run([os.path.join(REL, "live-render"), *common, "--option", f"{li}={k}", "--bars", opt["--solo-bars"], "--mute", mute_others,
                 "--control", f"{li}.{morph_c}={v}", "--form", f"{li}={show_form}", "--out", p])
            morph.append((v, measure_lufs_py(p)))
        spread = max(m for _, m in morph) - min(m for _, m in morph)
        for label, p in [("context", base + ".context.wav"), ("showcase", base + ".showcase.wav"), ("solo", base + ".solo.wav")] + [(f"morph {v}", f"{base}.morph{v}.wav") for v, _ in morph]:
            x, sr = sf.read(p, always_2d=True)
            index.append(f"{int(t // 60):02d}:{t % 60:04.1f}  {n:28s} {label}")
            if label == "showcase":
                bar_s = 240.0 / float(opt["--tempo"] or 174)
                for at_bar, lab, d in marks:
                    s0 = t + at_bar * bar_s
                    index.append(f"         {int(s0 // 60):02d}:{s0 % 60:04.1f}  {lab}: {d:g} -> 0, then 0 -> 1 over {SWEEP_BARS} bars, back to {d:g}")
            parts += [x, np.zeros((int(GAP_S * sr), x.shape[1]))]
            t += len(x) / sr + GAP_S
        index.append(f"       morph LUFS: " + ", ".join(f"{v}: {m:.1f}" for v, m in morph) + f"  (spread {spread:.1f} LU{' > 3 LU!' if spread > 3 else ''})")
        index.append("")
    sf.write(os.path.join(out, "audition.wav"), np.concatenate(parts), sr, subtype="FLOAT")
    open(os.path.join(out, "index.txt"), "w").write("\n".join(index) + "\n")
    print("\n".join(index))


if __name__ == "__main__":
    main()
