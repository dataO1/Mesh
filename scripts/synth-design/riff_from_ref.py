"""Copy a reference riff 1:1 into a MIDI motif clip.

Usage: riff_from_ref.py SET TRACK_SUBSTRING [--stem other|bass] [--bars 4]
                        [--at BAR] [--out DIR] [--name NAME] [--engine pyin|basic-pitch|melodia]

Takes the pyin notes of the track's separated stem (melody-<stem>.json from
scripts/live-analyse.sh melody), from the main drop (or BAR bars after it, or
the 4-bar window with the most notes in the drop's first 16 bars when --at is
not given), quantizes onsets and lengths to 16ths on the track's beat grid,
and writes DIR/<name>.mid (one track, 96 ticks per quarter, the original
pitches). The motif loader detects the clip's key and re-pitches it to the
lane's key and mode. Prints the riff as a 16th grid.
"""
import glob
import json
import os
import struct
import sys

REF = os.environ.get("LIVE_REF_ROOT", os.path.expanduser("~/Music/mesh-live-dev/references"))
NAMES = "C C# D D# E F F# G G# A A# B".split()
TPQ = 96


def vlq(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.append(0x80 | (n & 0x7F))
        n >>= 7
    return bytes(reversed(out))


def write_mid(path, notes, bars):
    """notes: (step, length_steps, midi) in 16ths."""
    step = TPQ // 4
    events = []
    for s, l, k in notes:
        events.append((s * step, 1, k))
        events.append(((s + l) * step - 1, 0, k))
    events.sort(key=lambda e: (e[0], e[1]))
    data, t = b"", 0
    for at, on, k in events:
        data += vlq(at - t) + bytes([0x90 if on else 0x80, k, 100 if on else 0])
        t = at
    data += vlq(bars * 16 * step - t) + b"\xff\x2f\x00"
    with open(path, "wb") as f:
        f.write(b"MThd" + struct.pack(">IHHH", 6, 0, 1, TPQ))
        f.write(b"MTrk" + struct.pack(">I", len(data)) + data)


def basic_pitch_notes(set_, melody_json, stem, drop, bar):
    """basic-pitch notes over the drop's first 16 bars, reduced to one voice:
    at each onset the loudest note (ties: the higher one)."""
    import subprocess
    name = os.path.basename(melody_json).split(".melody")[0]
    wav = os.path.join(REF, "stems", set_, f"{name}_({stem.capitalize()})_htdemucs_ft.wav")
    venv = os.path.expanduser("~/.cache/mesh-transcribe")
    env = dict(os.environ, LD_LIBRARY_PATH=open(os.path.join(venv, "ldpath")).read().strip(), TF_CPP_MIN_LOG_LEVEL="3")
    out = subprocess.run([os.path.join(venv, "venv", "bin", "python"), os.path.join(os.path.dirname(__file__), "bp_notes.py"),
                          wav, str(drop), str(16 * bar)], capture_output=True, text=True, env=env)
    ev = json.loads(out.stdout.strip().splitlines()[-1])
    top = os.environ.get("RIFF_VOICE") == "top"
    ev.sort(key=lambda e: (round(e[0] / (bar / 16)), -e[2]) if top else (round(e[0] / (bar / 16)), -e[3], -e[2]))
    mono, last = [], None
    for on, dur, midi, amp in ev:
        step = round(on / (bar / 16))
        if step == last:
            continue
        last = step
        mono.append([drop + on, dur, float(midi)])
    return mono


def melodia_notes(set_, melody_json, stem, drop, bar):
    """Predominant-melody notes (Melodia) over the drop's first 16 bars."""
    import subprocess
    name = os.path.basename(melody_json).split(".melody")[0]
    wav = os.path.join(REF, "stems", set_, f"{name}_({stem.capitalize()})_htdemucs_ft.wav")
    venv = os.path.expanduser("~/.cache/mesh-transcribe")
    env = dict(os.environ, LD_LIBRARY_PATH=open(os.path.join(venv, "ldpath")).read().strip())
    out = subprocess.run([os.path.join(venv, "venv", "bin", "python"), os.path.join(os.path.dirname(__file__), "mel_notes.py"),
                          wav, str(drop), str(16 * bar), str(bar / 16)], capture_output=True, text=True, env=env)
    ev = json.loads(out.stdout.strip().splitlines()[-1])
    return [[drop + on, d, float(m)] for on, d, m, _ in ev]


def main():
    a = sys.argv[1:]
    if len(a) < 2:
        sys.exit(__doc__)
    set_, sub = a[0], a[1]
    opt = dict(zip(a[2::2], a[3::2]))
    stem, bars = opt.get("--stem", "other"), int(opt.get("--bars", 4))
    cands = [p for p in glob.glob(os.path.join(REF, "analysis", set_, f"*.melody-{stem}.json")) if sub.lower() in p.lower()]
    if not cands:
        sys.exit(f"no {stem} melody JSON in {set_} matching '{sub}'")
    mj = json.load(open(cands[0]))
    ref = json.load(open(cands[0].replace(f".melody-{stem}.json", ".json")))
    bpm = ref["metrics"]["tempo"]
    bar = 240.0 / bpm
    drop = ref["main_drop_start_s"]
    # The drop start is on the 8-bar grid; snap it to the beat phase.
    phase = ref.get("beat_phase_s", 0.0) or 0.0
    drop = phase + round((drop - phase) / bar) * bar
    notes = mj["notes"]
    if opt.get("--engine") == "basic-pitch":
        notes = basic_pitch_notes(set_, cands[0], stem, drop, bar)
    elif opt.get("--engine") == "melodia":
        notes = melodia_notes(set_, cands[0], stem, drop, bar)

    def window(start_bar):
        t0 = drop + start_bar * bar
        return [n for n in notes if t0 - bar / 32 <= n[0] < t0 + bars * bar - bar / 32], t0

    if "--at" in opt:
        start = int(opt["--at"])
    else:
        start = max(range(0, 16 - bars + 1, 2), key=lambda b: len(window(b)[0]))
    sel, t0 = window(start)
    grid = []
    for on, dur, midi in sel:
        s = round((on - t0) / bar * 16)
        l = max(1, round(dur / bar * 16))
        if 0 <= s < bars * 16:
            grid.append([s, min(l, bars * 16 - s), int(round(midi))])
    # Monophonic: a note ends where the next starts.
    grid.sort()
    mono = []
    for g in grid:
        if mono and mono[-1][0] == g[0]:
            continue
        if mono and mono[-1][0] + mono[-1][1] > g[0]:
            mono[-1][1] = g[0] - mono[-1][0]
        mono.append(g)
    name = opt.get("--name") or f"ref-{set_}-{os.path.basename(cands[0]).split(' - ')[-1].split('.')[0][:24]}".replace(" ", "_")
    out = opt.get("--out", ".")
    os.makedirs(out, exist_ok=True)
    write_mid(os.path.join(out, f"{name}.mid"), mono, bars)
    print(f"{name}: {os.path.basename(cands[0])}, bars {start}..{start + bars} after the drop, {len(mono)} notes")
    for b in range(bars):
        row = ["."] * 16
        for s, l, k in mono:
            if b * 16 <= s < (b + 1) * 16:
                row[s - b * 16] = f"{NAMES[k % 12]}{k // 12 - 1}/{l}"
        print("  " + " ".join(f"{c:7s}" for c in row))


if __name__ == "__main__":
    main()
