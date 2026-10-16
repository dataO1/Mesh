"""Generate 4-bar riff clips from the riff rules (references/music-theory.md,
sections 2, 3, 7.2 and 8) and keep the ones closest to the riffs the user
approved.

Usage: riff_gen.py --approved DIR --out DIR [--n 12] [--candidates 600]
                   [--seed 1] [--root 41] [--mutate CLIP,CLIP...] [--prefix NAME]

--approved: the clips the user liked (their medians are the target).
--mutate: instead of new sentences, close variations of these clips (timing by a
16th, short/long notes, a neighbouring degree, the turnaround pickup).

Rhythm: onsets drawn from the approved riffs' step histogram (no downbeats,
few snare hits, pickups on 3/7/14/15). A 4-bar sentence: bar 1 the call,
bar 2 the answer (the call's rhythm, one onset displaced, a different
ending), bar 3 the call varied (sequence or displacement), bar 4 the
turnaround with a pickup into the next phrase.
Pitch: a root pedal with steps, few 4ths/5ths, from 1 b3 4 5 b7 (+ the
octave); b5, b2, b6 and 7 only as short colour notes that resolve by step.
Each candidate is scored against the approved riffs' medians
(riff_stats.py); the best distinct ones are written as MIDI clips in F minor
(the engine moves them to the lane's key) and printed as grids.
"""
import os
import random
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import riff_from_ref as W  # noqa: E402  (write_mid)
import riff_stats as S  # noqa: E402

DEGREES = [0, 3, 5, 7, 10, 12]  # 1 b3 4 5 b7 8
COLOURS = {6: (5, 7), 1: (0, 3), 8: (7, 10), 11: (12, 10)}  # colour: neighbours it resolves to
NAMES = "C C# D D# E F F# G G# A A# B".split()
FEATURES = ["offbeat", "downbeat", "on 4/12", "pickup 14/15/7/3", "repeat", "step1-2", "4th/5th", "len>=4",
            "notes/bar", "rest-bars", "LHL/bar", "rep2", "range"]


def step_weights(approved):
    """Onset weight per step from the approved clips (smoothed, step 0 nearly off)."""
    h = np.ones(16) * 0.5
    for notes in approved:
        for s, _, _ in notes:
            h[s % 16] += 1
    h[0] *= 0.1
    h[4] *= 0.5
    h[12] *= 0.5
    return h / h.sum()


def rhythm(rng, w, n):
    """n distinct onsets, weighted sampling without replacement."""
    w = list(w)
    out = []
    for _ in range(n):
        s = rng.choices(range(16), weights=w, k=1)[0]
        out.append(s)
        w[s] = 0.0
        # Neighbours of a chosen step are a little less likely (no clusters).
        for d in (-1, 1):
            if 0 <= s + d < 16:
                w[s + d] *= 0.6
    return sorted(out)


def lengths(rng, onsets, end=16):
    out = []
    for i, s in enumerate(onsets):
        gap = (onsets[i + 1] if i + 1 < len(onsets) else end) - s
        last = i + 1 == len(onsets)
        legato = rng.random() < (0.3 if last else 0.45)
        cap = gap if not last else min(gap, rng.choice([2, 3, 4, 6, 8]))
        out.append(cap if legato else max(1, min(gap, rng.choice([1, 2, 2, 3]))))
    # One long note per bar where there is room.
    if out and max(out) < 4:
        i = max(range(len(onsets)), key=lambda k: (onsets[k + 1] if k + 1 < len(onsets) else end) - onsets[k])
        out[i] = (onsets[i + 1] if i + 1 < len(onsets) else end) - onsets[i]
    return out


def next_pitch(rng, cur, colour_left):
    """Next semitone offset from the root: repeat, step, rare leaps, colours."""
    r = rng.random()
    if r < 0.33:
        return cur, False
    if colour_left and r < 0.45:
        c = rng.choice(list(COLOURS))
        return c, True
    near = sorted(DEGREES, key=lambda d: abs(d - cur))
    if r < 0.85:
        cand = [d for d in near if 1 <= abs(d - cur) <= 3] or near[1:2]
    elif r < 0.95:
        cand = [d for d in near if 3 < abs(d - cur) <= 7] or near[1:2]
    else:
        cand = [0, 12]
    return rng.choice(cand), False


def bar_pitches(rng, n, start, colours):
    out, cur, used = [], start, 0
    for i in range(n):
        if i == 0:
            out.append(cur)
            continue
        prev_colour = out[-1] in COLOURS
        if prev_colour:
            cur = rng.choice(COLOURS[out[-1]])
        else:
            cur, col = next_pitch(rng, cur, used < colours)
            used += col
        out.append(cur)
    if out and out[-1] in COLOURS:
        out[-1] = COLOURS[out[-1]][0]
    return out


def sentence(rng, w):
    """A 4-bar call / answer / variation / turnaround riff: [(step, len, semis)]."""
    n = rng.choice([3, 4, 4, 5, 5, 6])
    call_on = rhythm(rng, w, n)
    call_len = lengths(rng, call_on)
    call_p = bar_pitches(rng, n, rng.choice([0, 0, 0, 7, 12]), colours=1)
    notes = [(s, l, p) for s, l, p in zip(call_on, call_len, call_p)]
    # Answer: same rhythm, one onset displaced by a 16th, a different ending.
    ans_on = list(call_on)
    k = rng.randrange(len(ans_on))
    ans_on[k] = min(15, max(1, ans_on[k] + rng.choice([-1, 1])))
    ans_on = sorted(set(ans_on))
    ans_p = call_p[:len(ans_on) - 2] + bar_pitches(rng, 2, call_p[min(len(call_p) - 1, len(ans_on) - 2)], colours=1)
    ans_p = (ans_p + call_p)[:len(ans_on)]
    notes += [(16 + s, l, p) for s, l, p in zip(ans_on, lengths(rng, ans_on), ans_p)]
    # Variation: the call a scale step lower or displaced.
    if rng.random() < 0.5:
        shift = rng.choice([-2, -3, 2])
        var = [(32 + s, l, p + shift if p + shift in DEGREES else p) for s, l, p in notes[:len(call_on)]]
    else:
        d = rng.choice([1, 2, 3])
        var = [(32 + min(15, s + d), l, p) for s, l, p in notes[:len(call_on)]]
    notes += var
    # Turnaround: a new figure with a pickup on 14/15.
    tn = rng.choice([2, 3, 4])
    t_on = sorted(set(rhythm(rng, w, tn - 1) + [rng.choice([14, 15])]))
    t_p = bar_pitches(rng, len(t_on), rng.choice([0, 7, 10]), colours=1)
    t_p[-1] = rng.choice([10, 11, 12, 0]) if t_p[-1] not in (10, 11, 12) else t_p[-1]
    notes += [(48 + s, l, p) for s, l, p in zip(t_on, lengths(rng, t_on), t_p)]
    # Monophonic and inside 4 bars.
    notes.sort()
    out = []
    for s, l, p in notes:
        if out and out[-1][0] == s:
            continue
        if out and out[-1][0] + out[-1][1] > s:
            out[-1] = (out[-1][0], s - out[-1][0], out[-1][2])
        out.append((s, min(l, 64 - s), p))
    return out


def mutate(rng, notes, root):
    """A close variation of a liked riff: one or two of these moves.
    - timing: one non-first note of a bar moves a 16th earlier or later
    - length: a short note becomes long (to the next onset) or a long one short
    - pitch: a non-anchor note moves to a neighbouring degree (colours resolve)
    - turnaround: the last bar's pickup moves between 14 and 15
    """
    out = [list(n) for n in notes]
    for _ in range(rng.choice([1, 2, 2, 3])):
        kind = rng.choice(["timing", "timing", "length", "length", "pitch", "turn"])
        i = rng.randrange(len(out))
        s, l, k = out[i]
        if kind == "timing" and s % 16 != 0:
            ns = s + rng.choice([-1, 1])
            if ns % 16 != 0 and all(o[0] != ns for o in out):
                out[i][0] = ns
        elif kind == "length":
            nxt = min([o[0] for o in out if o[0] > s] or [64])
            out[i][1] = nxt - s if l < 4 else rng.choice([1, 2])
        elif kind == "pitch" and l < 4:
            rel = (k - root) % 12
            cands = [d for d in DEGREES if 1 <= abs(d - rel) <= 3]
            if cands:
                out[i][2] = root + rng.choice(cands) + 12 * ((k - root) // 12)
        elif kind == "turn":
            last = [o for o in out if o[0] >= 48 and o[0] % 16 in (14, 15)]
            if last:
                last[-1][0] = 48 + (29 - last[-1][0] % 16)
    out.sort()
    mono = []
    for s, l, k in out:
        if mono and mono[-1][0] == s:
            continue
        if mono and mono[-1][0] + mono[-1][1] > s:
            mono[-1] = (mono[-1][0], s - mono[-1][0], mono[-1][2])
        mono.append((s, max(1, min(l, 64 - s)), k))
    return mono


def tonic_root(notes):
    """MIDI note of the riff's tonic nearest its lowest note."""
    t = S.tonic_of(notes)
    lo = min(k for _, _, k in notes)
    return lo - ((lo - t) % 12)


def score(stats, target, spread):
    if stats["downbeat"] > 0.15 or stats["4th/5th"] > 0.25 or stats["major 3/6"] > 0:
        return float("inf")
    return sum(((stats[k] - target[k]) / spread[k]) ** 2 for k in FEATURES)


def main():
    a = sys.argv[1:]
    opt = dict(zip(a[::2], a[1::2]))
    if "--approved" not in opt or "--out" not in opt:
        sys.exit(__doc__)
    rng = random.Random(int(opt.get("--seed", 1)))
    root = int(opt.get("--root", 41))  # F2
    approved = [S.read_mid(os.path.join(opt["--approved"], f)) for f in sorted(os.listdir(opt["--approved"])) if f.endswith(".mid")]
    ast = [S.stats(n) for n in approved if n]
    target = {k: float(np.median([s[k] for s in ast])) for k in FEATURES}
    spread = {k: max(float(np.std([s[k] for s in ast])), 0.05 if k != "range" and k != "notes/bar" else 1.0) for k in FEATURES}
    w = step_weights(approved)
    cands = []
    seeds = [S.read_mid(m) for m in opt.get("--mutate", "").split(",") if m]
    for _ in range(int(opt.get("--candidates", 600))):
        if seeds:
            base = rng.choice(seeds)
            notes = mutate(rng, base, tonic_root(base))
        else:
            r = sentence(rng, w)
            notes = [(s, l, root + p) for s, l, p in r]
        st = S.stats(notes)
        cands.append((score(st, target, spread), notes, st))
    cands.sort(key=lambda c: c[0])
    chosen = []
    for sc, notes, st in cands:
        if sc == float("inf"):
            break
        onsets = {s for s, _, _ in notes}
        min_diff = 1 if opt.get("--mutate") else 8
        if any(notes == c[1] or len(onsets ^ {s for s, _, _ in c[1]}) < min_diff for c in chosen):
            continue
        chosen.append((sc, notes, st))
        if len(chosen) == int(opt.get("--n", 12)):
            break
    os.makedirs(opt["--out"], exist_ok=True)
    print("target (approved medians): " + ", ".join(f"{k} {target[k]:.2f}" for k in FEATURES))
    for i, (sc, notes, st) in enumerate(chosen):
        name = f"{opt.get('--prefix', 'gen')}-{i + 1:02d}"
        W.write_mid(os.path.join(opt["--out"], f"{name}.mid"), [list(n) for n in notes], 4)
        print(f"\n{name}  score {sc:.1f}  " + ", ".join(f"{k} {st[k]:.2f}" for k in ("offbeat", "pickup 14/15/7/3", "repeat", "4th/5th", "range", "notes/bar")) + f"  degrees {st['degrees']}")
        for b in range(4):
            row = ["."] * 16
            for s, l, k in notes:
                if b * 16 <= s < (b + 1) * 16:
                    row[s - b * 16] = f"{NAMES[k % 12]}{k // 12 - 1}/{l}"
            print("  " + " ".join(f"{c:6s}" for c in row))


if __name__ == "__main__":
    main()
